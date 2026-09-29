//! Pi and omp launches (v1 `launchPi` / `writePiConfig`, `launchOmp` / `writeOmpConfig`).
//! Both harnesses read providers from a config file bro upserts one entry into:
//! Pi's `models.json` (key via env var, never on disk) and omp's `models.yml`.
use super::{CommandSpec, LaunchCtx, LaunchSpec, Permission, Resolved, Target, label, proxy_route, require_key};
use crate::providers::{Model, Provider, ProviderMode, normalize_openai_base_url};
use crate::util::atomic_write;
use crate::{Harness, paths};
use anyhow::{anyhow, bail};
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};

/// Env var Pi resolves the bro-managed provider key from.
pub const PI_KEY_ENV: &str = "BRO_PI_API_KEY";

/// Pi's `models.json` (`$PI_CODING_AGENT_DIR/models.json` or `~/.pi/agent/models.json`).
pub fn pi_models_path() -> PathBuf {
    paths::pi_agent_dir().join("models.json")
}

/// omp's `~/.omp/agent/models.yml`.
pub fn omp_models_path() -> PathBuf {
    paths::omp_agent_dir().join("models.yml")
}

fn api_for(p: &Provider) -> &'static str {
    match p.mode {
        ProviderMode::Openai => "openai-completions",
        _ => "anthropic-messages",
    }
}

fn base_for(p: &Provider) -> Option<String> {
    let b = p.base_url.as_deref().filter(|b| !b.is_empty())?;
    Some(if p.mode == ProviderMode::Openai { normalize_openai_base_url(b) } else { b.to_string() })
}

/// bro-managed providers live in their own namespace in Pi (v1 `piProviderId`).
pub fn pi_provider_id(p: &Provider) -> String {
    if p.mode == ProviderMode::Native {
        return "anthropic".into();
    }
    let mut safe = String::new();
    for c in p.id.to_lowercase().chars() {
        let ok = c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-');
        if ok {
            safe.push(c);
        } else if !safe.ends_with('-') {
            safe.push('-');
        }
    }
    let safe = safe.trim_matches('-');
    format!("bro-{}", if safe.is_empty() { "provider" } else { safe })
}

/// A blank model resolves to the provider's first concrete one (v1 `piModelFor`).
fn pi_model_for(p: &Provider, model: Option<&str>) -> Option<String> {
    model.filter(|m| !m.is_empty()).map(str::to_string).or_else(|| p.model_ids().first().map(|s| s.to_string()))
}

/// Drop a trailing `:thinking-level` suffix when the id isn't itself a known model.
fn configured_model_id(model: &str, known: &[&str]) -> String {
    if known.contains(&model) {
        return model.into();
    }
    for level in ["off", "minimal", "low", "medium", "high", "xhigh", "max"] {
        if let Some(stripped) = model.strip_suffix(&format!(":{level}")) {
            return stripped.into();
        }
    }
    model.into()
}

fn models_list(p: &Provider, model: Option<&str>) -> Vec<(String, Option<String>)> {
    let known = p.model_ids();
    let mut ids: Vec<String> = known.iter().map(|s| s.to_string()).collect();
    if let Some(m) = model.filter(|m| !m.is_empty()).map(|m| configured_model_id(m, &known))
        && !ids.contains(&m)
    {
        ids.insert(0, m);
    }
    ids.into_iter()
        .map(|id| {
            let name = p.models.iter().find(|m| m.id == id).and_then(|m| m.name.clone());
            (id, name)
        })
        .collect()
}

/// v1 `piProviderEntry`.
fn pi_entry(p: &Provider, model: Option<&str>) -> Value {
    let mut e = Map::new();
    if let Some(b) = base_for(p) {
        e.insert("baseUrl".into(), json!(b));
    }
    e.insert("api".into(), json!(api_for(p)));
    e.insert("apiKey".into(), json!(PI_KEY_ENV));
    e.insert("authHeader".into(), json!(true));
    let models: Vec<Value> = models_list(p, model)
        .into_iter()
        .map(|(id, name)| match name {
            Some(n) => json!({ "id": id, "name": n }),
            None => json!({ "id": id }),
        })
        .collect();
    e.insert("models".into(), Value::Array(models));
    Value::Object(e)
}

/// Upsert `provider` into Pi's models.json (atomic; other providers untouched). Native
/// providers use Pi's own `anthropic` provider and write nothing (v1 `writePiConfig`).
pub fn write_pi_config(provider: &Provider, model: Option<&str>, path: &Path) -> anyhow::Result<()> {
    if provider.mode == ProviderMode::Native {
        return Ok(());
    }
    let mut config = if path.exists() {
        let text = std::fs::read_to_string(path)?;
        serde_json::from_str::<Value>(text.trim_start_matches('\u{feff}')).map_err(|e| {
            anyhow!("Could not update Pi's provider config because {} is not valid JSON: {e}", path.display())
        })?
    } else {
        json!({})
    };
    let Some(obj) = config.as_object_mut() else {
        bail!("Could not update Pi's provider config because {} must contain a JSON object.", path.display());
    };
    let providers = obj.entry("providers").or_insert_with(|| json!({}));
    let Some(providers) = providers.as_object_mut() else {
        bail!("Could not update Pi's provider config because its \"providers\" value must be an object.");
    };
    providers.insert(pi_provider_id(provider), pi_entry(provider, model));
    let text = format!("{}\n", serde_json::to_string_pretty(&config)?);
    atomic_write(path, text.as_bytes())?;
    Ok(())
}

fn yaml_str(v: &str) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| format!("\"{v}\""))
}

/// v1 `ompProviderBlock`.
fn omp_block(p: &Provider, model: Option<&str>, api_key: Option<&str>) -> String {
    let mut lines = vec![format!("  {}:", p.id)];
    if let Some(b) = base_for(p) {
        lines.push(format!("    baseUrl: {}", yaml_str(&b)));
    }
    lines.push(format!("    api: {}", api_for(p)));
    if p.no_key {
        lines.push("    auth: none".into());
    } else {
        lines.push(format!("    apiKey: {}", yaml_str(api_key.or(p.key_env.as_deref()).unwrap_or(""))));
        if p.mode == ProviderMode::Openai {
            lines.push("    authHeader: true".into());
        }
    }
    if p.disable_1m_context || p.mode == ProviderMode::Anthropic {
        lines.push("    disableStrictTools: true".into());
    }
    let models = models_list(p, model);
    if !models.is_empty() {
        lines.push("    models:".into());
        for (id, name) in models {
            lines.push(format!("      - id: {}", yaml_str(&id)));
            if let Some(n) = name {
                lines.push(format!("        name: {}", yaml_str(&n)));
            }
        }
    }
    lines.join("\n")
}

fn is_provider_key_line(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("  ")?;
    if rest.starts_with(' ') {
        return None;
    }
    let key = rest.trim_end().strip_suffix(':')?;
    (!key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))).then_some(key)
}

/// Replace (or add) one provider under the top-level `providers:` map, keeping the
/// rest of the file byte-for-byte (v1 `upsertOmpProvider`).
pub(crate) fn upsert_omp_yaml(text: &str, id: &str, block: &str) -> String {
    let normalized = text.replace("\r\n", "\n");
    let mut lines: Vec<String> = if normalized.is_empty() { vec![] } else { normalized.split('\n').map(str::to_string).collect() };
    let is_section = |l: &str| l.trim_end() == "providers:";
    let start = match lines.iter().position(|l| is_section(l)) {
        Some(s) => s,
        None => {
            if lines.last().is_some_and(|l| !l.is_empty()) {
                lines.push(String::new());
            }
            lines.push("providers:".into());
            lines.len() - 1
        }
    };
    let end = (start + 1..lines.len())
        .find(|&i| lines[i].chars().next().is_some_and(|c| !c.is_whitespace()) && !is_section(&lines[i]))
        .unwrap_or(lines.len());
    let mut next: Vec<String> = Vec::new();
    let mut i = start + 1;
    while i < end {
        if is_provider_key_line(&lines[i]) == Some(id) {
            i += 1;
            while i < end && is_provider_key_line(&lines[i]).is_none() {
                i += 1;
            }
            continue;
        }
        next.push(lines[i].clone());
        i += 1;
    }
    while next.last().is_some_and(|l| l.is_empty()) {
        next.pop();
    }
    next.extend(block.split('\n').map(str::to_string));
    let mut merged: Vec<String> = lines[..=start].to_vec();
    merged.extend(next);
    merged.extend(lines[end..].iter().cloned());
    let mut out = merged.join("\n");
    while out.contains("\n\n\n") {
        out = out.replace("\n\n\n", "\n\n");
    }
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// Upsert `provider` into omp's models.yml (atomic). Native providers write nothing.
pub fn write_omp_config(provider: &Provider, model: Option<&str>, api_key: Option<&str>, path: &Path) -> anyhow::Result<()> {
    if provider.mode == ProviderMode::Native {
        return Ok(());
    }
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let merged = upsert_omp_yaml(&text, &provider.id, &omp_block(provider, model, api_key));
    atomic_write(path, merged.as_bytes())?;
    Ok(())
}

/// A synthetic Anthropic-format provider pointing at a bro-proxy route.
fn proxy_provider(upstream: &str, base: &str, model: Option<&str>) -> Provider {
    let slug: String = upstream.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
    Provider {
        id: format!("proxy-{slug}"),
        name: format!("bro-proxy ({upstream})"),
        mode: ProviderMode::Anthropic,
        base_url: Some(base.to_string()),
        responses_base_url: None,
        key_env: None,
        key_url: None,
        no_key: false,
        disable_1m_context: true,
        env: Default::default(),
        models: model
            .filter(|m| !m.is_empty())
            .map(|m| vec![Model { id: m.into(), name: None, context: None, extra: Default::default() }])
            .unwrap_or_default(),
        extra: Default::default(),
    }
}

pub(super) fn build(spec: &LaunchSpec, ctx: &LaunchCtx, r: &Resolved) -> anyhow::Result<CommandSpec> {
    let mut cmd = CommandSpec::default();
    let model = spec.model.as_deref().filter(|m| !m.is_empty());
    // (provider to configure, its key, who for the label)
    let (provider, key, who): (Option<Provider>, Option<String>, String) = match &r.target {
        Target::Login => (None, None, String::new()),
        Target::Provider(p, _) if p.mode == ProviderMode::Native => {
            match r.profile.as_ref().filter(|p| p.is_claude()) {
                // A Claude subscription login: route through bro-proxy.
                Some(login) => {
                    let route = proxy_route(&login.id, model, ctx)?;
                    let prov = proxy_provider(&login.id, &route.base, model);
                    let token = route.token.clone();
                    cmd.route = Some(route.request);
                    (Some(prov), Some(token), login.id.clone())
                }
                None => (Some((**p).clone()), None, p.id.clone()),
            }
        }
        Target::Provider(p, key) => {
            require_key(p, key)?;
            (Some((**p).clone()), key.clone(), p.id.clone())
        }
        Target::Upstream(up) => {
            let route = proxy_route(up, model, ctx)?;
            let prov = proxy_provider(up, &route.base, model);
            let token = route.token.clone();
            cmd.route = Some(route.request);
            (Some(prov), Some(token), up.clone())
        }
    };

    let mut args = Vec::new();
    match spec.harness {
        Harness::Pi => {
            if let Some(p) = &provider {
                let active = pi_model_for(p, model).ok_or_else(|| {
                    anyhow!("Pi needs a concrete model for {}. Pass a model or add models to the provider config.", p.display_name())
                })?;
                write_pi_config(p, Some(&active), &pi_models_path())?;
                if p.mode != ProviderMode::Native {
                    cmd.set_env(PI_KEY_ENV, key.clone().unwrap_or_else(|| "not-needed".into()));
                }
                args.extend(["--provider".into(), pi_provider_id(p), "--model".into(), active]);
            } else if let Some(m) = model {
                args.extend(["--model".into(), m.to_string()]);
            }
        }
        _ => {
            if spec.permission == Permission::Skip {
                args.push("--yolo".into());
            }
            match &provider {
                Some(p) => {
                    write_omp_config(p, model, key.as_deref(), &omp_models_path())?;
                    if let Some(m) = model {
                        args.extend(["--model".into(), format!("{}/{m}", p.id)]);
                    }
                }
                None => {
                    if let Some(m) = model {
                        args.extend(["--model".into(), m.to_string()]);
                    }
                }
            }
        }
    }
    if let Some(resume) = &spec.resume {
        // Both take a session file path (or an id / id prefix they look up themselves).
        let target = crate::sessions::find_by_id(spec.harness, &resume.session_id)
            .map(|s| s.path.to_string_lossy().into_owned())
            .unwrap_or_else(|| resume.session_id.clone());
        let flag = match (spec.harness, resume.fork) {
            (_, true) => "--fork",
            (Harness::Pi, false) => "--session",
            _ => "--resume",
        };
        args.extend([flag.to_string(), target]);
    }
    args.extend(spec.extra_args.iter().cloned());
    let program = spec.harness.label();
    (cmd.program, cmd.args) = super::program_and_args(program, args);
    cmd.label = label(spec.harness, &who, model);
    Ok(cmd)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn omp_yaml_upsert_replaces_only_its_block() {
        let text = "theme: dark\nproviders:\n  keep:\n    api: x\n  zai:\n    api: old\n    models:\n      - id: \"a\"\nother: 1\n";
        let out = upsert_omp_yaml(text, "zai", "  zai:\n    api: new");
        assert_eq!(out, "theme: dark\nproviders:\n  keep:\n    api: x\n  zai:\n    api: new\nother: 1\n");
        let fresh = upsert_omp_yaml("", "p", "  p:\n    api: a");
        assert_eq!(fresh, "providers:\n  p:\n    api: a\n");
        let appended = upsert_omp_yaml("a: 1", "p", "  p:\n    api: a");
        assert_eq!(appended, "a: 1\n\nproviders:\n  p:\n    api: a\n");
    }

    #[test]
    fn pi_ids_and_models() {
        let mut p = proxy_provider("claude:work", "http://x/r/1", Some("m"));
        assert_eq!(p.id, "proxy-claude-work");
        assert_eq!(pi_provider_id(&p), "bro-proxy-claude-work");
        p.id = "Open Router!".into();
        assert_eq!(pi_provider_id(&p), "bro-open-router");
        assert_eq!(configured_model_id("glm-5:high", &["glm-5"]), "glm-5");
        assert_eq!(configured_model_id("x:high", &["x:high"]), "x:high");
    }
}
