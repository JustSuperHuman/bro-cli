//! Codex CLI launches (v1 `launchCodex` / `codexProviderConfig` / `runCodex`).
use super::{CommandSpec, LaunchCtx, LaunchSpec, Permission, Resolved, Target, label, prepare_resume, proxy_route, require_key};
use crate::providers::ProviderMode;
use crate::{Harness, paths};

/// The provider slot bro describes to codex via `-c` overrides (never written to
/// config.toml), and the env var codex reads its key from.
const SLOT: &str = "bro";
/// Env var holding the provider key for codex (`env_key`).
pub const CODEX_KEY_ENV: &str = "BRO_PROVIDER_API_KEY";

/// A model entry in Codex's own catalogue format, used when no Codex login has a
/// `models_cache.json` to copy one from.
const MODEL_TEMPLATE: &str = include_str!("codex_model_template.json");

/// A model entry to copy: the first one in a Codex login's `models_cache.json` (the
/// installed Codex's own format), else the bundled one.
fn model_template(home: &std::path::Path) -> serde_json::Value {
    let profiles = std::fs::read_dir(paths::codex_profiles_dir()).into_iter().flatten().flatten().map(|e| e.path());
    std::iter::once(home.to_path_buf())
        .chain(std::iter::once(paths::codex_local_dir()))
        .chain(profiles)
        .find_map(|dir| {
            let doc = crate::util::read_json(&dir.join("models_cache.json"))?;
            doc.get("models")?.as_array()?.first().filter(|m| m.is_object()).cloned()
        })
        .or_else(|| serde_json::from_str(MODEL_TEMPLATE).ok())
        .unwrap_or_default()
}

/// One catalogue entry for a provider model, so Codex knows its real context window instead
/// of warning "Model metadata … not found" and assuming a default.
fn model_entry(template: &serde_json::Value, m: &crate::providers::Model, context: u64, priority: usize) -> serde_json::Value {
    use serde_json::{Value, json};
    let mut e = template.clone();
    let Some(o) = e.as_object_mut() else { return e };
    let name = m.name.clone().unwrap_or_else(|| m.id.clone());
    for k in ["availability_nux", "upgrade", "comp_hash", "available_access_programs"] {
        o.remove(k);
    }
    o.insert("slug".into(), json!(m.id));
    o.insert("display_name".into(), json!(name));
    o.insert("description".into(), json!(format!("{name} through bro")));
    o.insert("context_window".into(), json!(context));
    o.insert("max_context_window".into(), json!(context));
    o.insert("priority".into(), json!(priority));
    o.insert("visibility".into(), json!("list"));
    o.insert("default_reasoning_level".into(), json!("medium"));
    o.insert("default_reasoning_summary".into(), json!("auto"));
    o.insert(
        "supported_reasoning_levels".into(),
        json!([
            { "effort": "low", "description": "Fast responses with lighter reasoning" },
            { "effort": "medium", "description": "Balances speed and reasoning depth for everyday tasks" },
            { "effort": "high", "description": "Greater reasoning depth for complex problems" }
        ]),
    );
    // Only OpenAI serves these.
    o.insert("service_tiers".into(), json!([]));
    o.insert("additional_speed_tiers".into(), json!([]));
    o.insert("supports_search_tool".into(), json!(false));
    o.insert("support_verbosity".into(), json!(false));
    o.entry("experimental_supported_tools").or_insert_with(|| json!([]));
    // Codex's prompt names the GPT model it was written for.
    if let Some(Value::String(t)) = o.get_mut("model_messages").and_then(|mm| mm.get_mut("instructions_template")) {
        *t = rename_model(t, &name);
    }
    if let Some(Value::String(t)) = o.get_mut("base_instructions") {
        *t = rename_model(t, &name);
    }
    e
}

/// "an agent based on GPT-6" → "an agent based on Gemini 4 Argon".
fn rename_model(text: &str, name: &str) -> String {
    let Some(at) = text.find("based on GPT-") else { return text.to_string() };
    let start = at + "based on ".len();
    let end = text[start..].find(|c: char| c.is_whitespace() || c == ',').map(|i| start + i).unwrap_or(text.len());
    let end = if text[..end].ends_with('.') { end - 1 } else { end };
    format!("{}{name}{}", &text[..start], &text[end..])
}

/// `~/.bro/codex-catalog/<provider>.json` with the provider's models (and the one launched) that
/// have a known context window (see [`crate::catalogue::KnownLimits`]), for `-c model_catalog_json=…`.
/// None when there are none.
fn model_catalog(p: &crate::providers::Provider, launched: Option<&str>, home: &std::path::Path) -> Option<std::path::PathBuf> {
    let mut ids = p.model_ids();
    if let Some(m) = launched.filter(|m| !m.is_empty() && !ids.contains(m)) {
        ids.push(m);
    }
    let known = crate::catalogue::KnownLimits::load();
    let models: Vec<_> = ids
        .into_iter()
        .filter_map(|id| {
            let listed = p.models.iter().find(|m| m.id == id);
            let context = known.get(id, p.model(id).map(Into::into)).context?;
            let model = listed.cloned().unwrap_or_else(|| crate::providers::Model { id: id.into(), name: None, context: None, extra: Default::default() });
            Some((model, context))
        })
        .collect();
    if models.is_empty() {
        return None;
    }
    let template = model_template(home);
    let entries: Vec<_> = models.iter().enumerate().map(|(i, (m, ctx))| model_entry(&template, m, *ctx, i + 1)).collect();
    let safe: String = p.id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect();
    let path = paths::bro_dir().join("codex-catalog").join(format!("{safe}.json"));
    crate::util::write_json_compact(&path, &serde_json::json!({ "models": entries })).ok()?;
    Some(path)
}

/// TOML-quote a value so codex parses it as a string.
fn toml_str(v: &str) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| format!("\"{v}\""))
}

/// `-c model_provider=bro -c model_providers.bro.…` for a Responses endpoint.
fn provider_args(name: &str, base_url: &str, with_key: bool) -> Vec<String> {
    let mut a = vec![
        "-c".into(),
        format!("model_provider={}", toml_str(SLOT)),
        "-c".into(),
        format!("model_providers.{SLOT}.name={}", toml_str(name)),
        "-c".into(),
        format!("model_providers.{SLOT}.base_url={}", toml_str(base_url)),
        "-c".into(),
        format!("model_providers.{SLOT}.wire_api={}", toml_str("responses")),
    ];
    if with_key {
        a.extend(["-c".into(), format!("model_providers.{SLOT}.env_key={}", toml_str(CODEX_KEY_ENV))]);
    }
    a
}

pub(super) fn build(spec: &LaunchSpec, ctx: &LaunchCtx, r: &Resolved) -> anyhow::Result<CommandSpec> {
    let mut cmd = CommandSpec::default();
    let login = r.profile.as_ref().filter(|p| p.is_codex());
    let home = login.map(|p| p.dir.clone()).unwrap_or_else(paths::codex_local_dir);
    let target_profile = login.map(|p| p.id.clone()).unwrap_or_else(|| "codex:local".into());
    cmd.set_env("CODEX_HOME", home.to_string_lossy());
    // Best effort: a read-only CODEX_HOME must not block the launch.
    if !ctx.bro_open_off {
        let _ = crate::skills::write_bro_open_skill(&home);
    }
    crate::skill_repos::write_cached(&home, &ctx.skill_repos);
    if spec.orchestrator {
        let _ = crate::orchestrator::write_skill(&home);
    }
    if ctx.justtools_skill {
        let _ = crate::justtools::write_skill(&home);
    }
    if ctx.justwatch_skill {
        let _ = crate::justwatch::write_skill(&home);
    }

    let mut provider = Vec::new();
    let who = match &r.target {
        Target::Login => {
            // Request compression: talk to bro-proxy, which signs in as this same login.
            if ctx.routes_native() {
                provider = wire_proxy(&mut cmd, &target_profile, spec, ctx)?;
            }
            login.map(|p| p.name.clone()).unwrap_or_else(|| "local".into())
        }
        Target::Provider(p, key) => match p.responses_base() {
            Some(base) => {
                require_key(p, key)?;
                provider = provider_args(p.display_name(), &base, key.is_some());
                if let Some(cat) = model_catalog(p, spec.model.as_deref(), &home) {
                    provider.extend(["-c".into(), format!("model_catalog_json={}", toml_str(&cat.to_string_lossy()))]);
                }
                // before your own args, so a `-c` of yours still wins
                for setting in p.codex_overrides() {
                    provider.extend(["-c".into(), setting]);
                }
                if let Some(k) = key {
                    cmd.set_env(CODEX_KEY_ENV, k.clone());
                }
                p.id.clone()
            }
            None => {
                // Anthropic-only provider (or first-party Claude): bro-proxy serves Responses.
                let upstream = if p.mode == ProviderMode::Native {
                    let claude_login = r.profile.as_ref().filter(|p| p.is_claude());
                    claude_login.map(|p| p.id.clone()).unwrap_or_else(|| "claude:local".into())
                } else {
                    p.id.clone()
                };
                if p.mode != ProviderMode::Native {
                    require_key(p, key)?;
                }
                provider = wire_proxy(&mut cmd, &upstream, spec, ctx)?;
                if p.mode != ProviderMode::Native
                    && let Some(cat) = model_catalog(p, spec.model.as_deref(), &home)
                {
                    provider.extend(["-c".into(), format!("model_catalog_json={}", toml_str(&cat.to_string_lossy()))]);
                }
                for setting in p.codex_overrides() {
                    provider.extend(["-c".into(), setting]);
                }
                p.id.clone()
            }
        },
        Target::Upstream(up) => {
            provider = wire_proxy(&mut cmd, up, spec, ctx)?;
            up.clone()
        }
        Target::Logins(false) => {
            // Proxy mode: Codex stays signed in with ChatGPT as this login and keeps its own `openai`
            // provider (its /model list, plan, limits, web search, compaction); only that provider's
            // address moves to bro-proxy, which sends each request as the login with the most left.
            // Ending in /backend-api/codex keeps the backend's own routes on.
            let route = proxy_route(super::LOGINS_CODEX, None, ctx)?;
            provider = vec![
                "-c".into(),
                format!("openai_base_url={}", toml_str(&format!("{}/backend-api/codex", route.base))),
                // its bodies stay readable to the proxy (zstd is only for the trip over the internet)
                "-c".into(),
                "features.enable_request_compression=false".into(),
            ];
            cmd.route = Some(route.request);
            String::new()
        }
        Target::Logins(true) => {
            // Codex on Claude models, through the shared Claude logins
            provider = wire_proxy(&mut cmd, super::LOGINS_CLAUDE, spec, ctx)?;
            "claude logins".into()
        }
    };
    cmd.store = Some(target_profile.clone());

    let mut args = Vec::new();
    if let Some(resume) = &spec.resume {
        let (id, fork) = prepare_resume(Harness::Codex, resume, &target_profile, &mut cmd.cleanup)?;
        args.extend([if fork { "fork" } else { "resume" }.to_string(), id]);
    }
    if spec.permission == Permission::Skip {
        args.push("--yolo".into());
    }
    // Bro owns each pane's process and CODEX_HOME. Do not depend on Codex's shared background
    // server: it can fail outside elevated terminals too, before the interactive session starts.
    // Keep an explicit user flag, but a literal after `--` is a prompt, not an option.
    if !spec.extra_args.iter().take_while(|a| a.as_str() != "--").any(|a| a == "--no-daemon") {
        args.push("--no-daemon".into());
    }
    if let Some(m) = spec.model.as_deref().filter(|m| !m.is_empty()) {
        args.extend(["--model".into(), m.to_string()]);
    }
    args.extend(provider);
    // PeonPing sounds, unless this home's own config already plays them
    if let Some(bin) = ctx.bro_bin.as_ref().filter(|_| ctx.peon.wires(Harness::Codex))
        && !crate::peon::wire::codex_own(&home).covered()
    {
        args.extend(crate::peon::wire::codex_args(bin));
    }
    if spec.orchestrator {
        args.extend(crate::orchestrator::codex_mcp_args(super::orchestrator_bridge(ctx).unwrap_or_default()));
    }
    // Instructions bro adds: caveman's rules, the orchestrator's brief.
    let mut added: Vec<String> = vec![];
    if spec.orchestrator {
        added.push(crate::orchestrator::scoped_prompt(Harness::Codex, &spec.cwd));
    }
    if let Some(level) = ctx.caveman.active() {
        added.push(level.rules().to_string());
        cmd.caveman = Some(level);
    }
    if !added.is_empty() {
        // `-c` replaces the login's own developer_instructions, so those come first.
        let own = std::fs::read_to_string(home.join("config.toml"))
            .ok()
            .and_then(|s| s.parse::<toml::Table>().ok())
            .and_then(|t| t.get("developer_instructions").and_then(|v| v.as_str()).map(str::to_string))
            .filter(|s| !s.trim().is_empty());
        let text = own.into_iter().chain(added).map(|s| s.trim_end().to_string()).collect::<Vec<_>>().join("\n\n");
        args.extend(["-c".into(), format!("developer_instructions={}", toml_str(&text))]);
    }
    args.extend(spec.extra_args.iter().cloned());
    (cmd.program, cmd.args) = super::program_and_args("codex", args);
    cmd.label = label(Harness::Codex, &who, spec.model.as_deref());
    Ok(cmd)
}

fn wire_proxy(cmd: &mut CommandSpec, upstream: &str, spec: &LaunchSpec, ctx: &LaunchCtx) -> anyhow::Result<Vec<String>> {
    let route = proxy_route(upstream, spec.model.as_deref(), ctx)?;
    cmd.set_env(CODEX_KEY_ENV, route.token.clone());
    let args = provider_args(&format!("bro-proxy ({upstream})"), &format!("{}/v1", route.base), true);
    cmd.route = Some(route.request);
    Ok(args)
}
