//! Claude Code launches (v1 `launch.js` claude path, `pool.js` `runAccountProfile`).
use super::{CommandSpec, LaunchCtx, LaunchSpec, Permission, Resolved, Target, label, prepare_resume, proxy_route, require_key};
use crate::browser::{self, BrowserCtx};
use crate::profiles::ProfileKind;
use crate::providers::ProviderMode;
use crate::{Harness, caveman, paths};
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

/// Vars that would point Claude Code at some other backend than the one chosen.
const BACKEND_VARS: [&str; 4] = ["ANTHROPIC_BASE_URL", "ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_API_KEY", "CLAUDE_CODE_DISABLE_1M_CONTEXT"];

/// Claude Code's model slots. On a third-party backend every slot is pinned to the chosen
/// model, so its background (haiku) and subagent requests don't ask for Claude models the
/// backend doesn't serve.
pub(crate) const MODEL_VARS: [&str; 6] = [
    "ANTHROPIC_MODEL",
    "ANTHROPIC_DEFAULT_OPUS_MODEL",
    "ANTHROPIC_DEFAULT_SONNET_MODEL",
    "ANTHROPIC_DEFAULT_HAIKU_MODEL",
    "ANTHROPIC_SMALL_FAST_MODEL",
    "CLAUDE_CODE_SUBAGENT_MODEL",
];

/// v1 `permissionArgs`.
pub(crate) fn permission_args(p: Permission) -> Vec<String> {
    match p {
        Permission::Default => vec![],
        Permission::Auto => vec!["--permission-mode".into(), "auto".into()],
        Permission::Skip => vec!["--dangerously-skip-permissions".into()],
    }
}

pub(super) fn build(spec: &LaunchSpec, ctx: &LaunchCtx, r: &Resolved) -> anyhow::Result<CommandSpec> {
    let mut cmd = CommandSpec::default();
    // Which CLAUDE_CONFIG_DIR the session runs in (and where its transcripts live).
    let login = r.profile.as_ref().filter(|p| p.is_claude());
    let config_dir = login.map(|p| p.dir.clone()).unwrap_or_else(paths::claude_local_dir);
    let target_profile = login.map(|p| p.id.clone()).unwrap_or_else(|| "claude:local".into());
    // An account profile runs with the root `~/.claude`'s skills and MCP servers.
    let account = login.filter(|p| p.kind == ProfileKind::ClaudeAccount);
    // Best effort: a read-only profile dir must not block the launch.
    if let Some(p) = account {
        let _ = crate::claude_shared::share_skills(&p.dir);
    }
    let _ = crate::skills::write_bro_open_skill(&config_dir);
    if ctx.justtools_skill {
        let _ = crate::justtools::write_skill(&config_dir);
    }
    if ctx.justwatch_skill {
        let _ = crate::justwatch::write_skill(&config_dir);
    }
    match login {
        Some(p) if p.kind == ProfileKind::ClaudeAccount => cmd.set_env("CLAUDE_CONFIG_DIR", p.dir.to_string_lossy()),
        _ => cmd.remove_env("CLAUDE_CONFIG_DIR"),
    }
    for var in BACKEND_VARS.iter().chain(&MODEL_VARS) {
        cmd.remove_env(var);
    }

    let who;
    // who serves the model, for bro's mod: the login, a provider id, or what the proxy routes to
    let backend;
    let mut third_party = true;
    // A native login routed through bro-proxy for request compression: Claude Code then
    // authenticates with the proxy token, but the backend and models stay first-party.
    let mut proxied_login = false;
    // Through bro-proxy to an Anthropic-format upstream (pool, a Claude login, an Anthropic provider).
    let mut proxied_anthropic = false;
    match &r.target {
        Target::Login => {
            third_party = false;
            who = login.map(|p| p.name.clone()).unwrap_or_else(|| "local".into());
            backend = target_profile.clone();
            if ctx.routes_native() {
                proxied_anthropic = wire_proxy(&mut cmd, &target_profile, spec, ctx)?;
                proxied_login = true;
            }
        }
        Target::Provider(p, _) if p.mode == ProviderMode::Native => {
            third_party = false;
            who = login.map(|p| p.name.clone()).unwrap_or_else(|| "local".into());
            backend = target_profile.clone();
        }
        Target::Provider(p, key) if p.mode == ProviderMode::Anthropic => {
            require_key(p, key)?;
            cmd.set_env("ANTHROPIC_BASE_URL", p.base_url.clone().unwrap_or_default());
            cmd.set_env("ANTHROPIC_AUTH_TOKEN", key.clone().unwrap_or_default());
            cmd.set_env("ANTHROPIC_API_KEY", "");
            if p.disable_1m_context {
                cmd.set_env("CLAUDE_CODE_DISABLE_1M_CONTEXT", "1");
            }
            for (k, v) in &p.env {
                cmd.set_env(k, v.clone());
            }
            who = p.id.clone();
            backend = p.id.clone();
        }
        Target::Provider(p, key) => {
            // OpenAI-format provider: Claude Code speaks Anthropic, so bro-proxy translates.
            require_key(p, key)?;
            who = p.id.clone();
            backend = p.id.clone();
            proxied_anthropic = wire_proxy(&mut cmd, &p.id, spec, ctx)?;
        }
        Target::Upstream(up) => {
            who = up.clone();
            backend = up.clone();
            proxied_anthropic = wire_proxy(&mut cmd, up, spec, ctx)?;
        }
    }

    if third_party && let Some(m) = spec.model.as_deref().filter(|m| !m.is_empty()) {
        for var in MODEL_VARS {
            cmd.set_env(var, m.to_string());
        }
        // Claude Code assumes 200k for a model it doesn't know (and warns): give it the real
        // window and output limit (see [`crate::catalogue::KnownLimits`]). The provider's own `env` wins.
        if !m.to_lowercase().starts_with("claude") {
            let limits = crate::catalogue::KnownLimits::load().get(m, listed_limits(&r.target, m));
            let own_env = |var| matches!(&r.target, Target::Provider(p, _) if p.env.contains_key(var));
            for (var, value) in [("CLAUDE_CODE_MAX_CONTEXT_TOKENS", limits.context), ("CLAUDE_CODE_MAX_OUTPUT_TOKENS", limits.max_output)] {
                if let Some(v) = value
                    && !own_env(var)
                {
                    cmd.set_env(var, v.to_string());
                }
            }
        }
    }

    // The settings preset; what the provider's own `env` already set wins.
    for (k, v) in ctx.claude_env.vars(proxied_anthropic) {
        if !cmd.env.iter().any(|(set, _)| *set == k) {
            cmd.set_env(&k, v);
        }
    }

    let mut args = permission_args(spec.permission);
    let wants_browser = !spec.extra_args.iter().any(|a| a == "--chrome" || a == "--no-chrome");
    if wants_browser {
        let claude_path = super::which("claude").map(|p| p.to_string_lossy().into_owned());
        let bctx = BrowserCtx {
            profile_dir: Some(&config_dir),
            third_party_auth: third_party || proxied_login,
            skip_permissions: spec.permission == Permission::Skip,
            claude_path: claude_path.as_deref(),
        };
        let (bargs, benv) = browser::launch_additions_with(spec.browser, Harness::Claude, &bctx);
        args.extend(bargs);
        for (k, v) in benv {
            cmd.set_env(&k, v);
        }
    }
    if let Some(m) = spec.model.as_deref().filter(|m| !m.is_empty()) {
        args.extend(["--model".into(), m.to_string()]);
    }
    if let Some(resume) = &spec.resume {
        let (id, fork) = prepare_resume(Harness::Claude, resume, &target_profile, &mut cmd.cleanup)?;
        args.extend(["--resume".into(), id]);
        if fork {
            args.push("--fork-session".into());
        }
    }
    args.extend(spec.extra_args.iter().cloned());
    let mut extra_settings = bro_mod(&mut cmd, &mut args, ctx, &config_dir, &target_profile, &backend);
    if spec.orchestrator {
        orchestrator_args(&mut args, &config_dir, ctx)?;
    } else if let (Some(scope), Some(bin)) = (crate::peers::Scope::of(&ctx.claude_env), &ctx.bro_bin) {
        // messages between sessions stay in this one's project; the orchestrator spans them all
        crate::peers::add_hooks(&mut extra_settings, scope, bin, &spec.cwd);
    }
    // PeonPing sounds, unless this login's own settings already play them
    if let Some(bin) = ctx.bro_bin.as_ref().filter(|_| ctx.peon.wires(Harness::Claude))
        && !crate::peon::wire::claude_own(&config_dir).covered()
    {
        crate::peon::wire::add_claude_hooks(&mut extra_settings, bin);
    }
    cmd.caveman = apply_caveman(&mut cmd, &mut args, spec, ctx, extra_settings);
    let strict_mcp = spec.extra_args.iter().any(|a| a == "--strict-mcp-config");
    if let Some(p) = account.filter(|_| !strict_mcp)
        && let Ok(Some(servers)) = crate::claude_shared::write_shared_mcp(&p.dir, &spec.cwd)
    {
        // `--mcp-config` is variadic and would swallow a prompt that followed it, so it
        // goes last; after a `--` nothing is an option any more, so there it leads.
        let shared = ["--mcp-config".to_string(), servers.to_string_lossy().into_owned()];
        if spec.extra_args.iter().any(|a| a == "--") {
            args.splice(0..0, shared);
        } else {
            args.extend(shared);
        }
    }
    (cmd.program, cmd.args) = super::program_and_args("claude", args);
    cmd.label = label(Harness::Claude, &who, spec.model.as_deref());
    Ok(cmd)
}

const PROMPT_FILE: &str = "--append-system-prompt-file";

/// The orchestrator's instructions (merged with any others by [`apply_caveman`]), its skill, and the
/// bridge's tools as the `bro` MCP server, allowed without asking.
fn orchestrator_args(args: &mut Vec<String>, config_dir: &Path, ctx: &LaunchCtx) -> anyhow::Result<()> {
    use crate::orchestrator as orch;
    let bridge = super::orchestrator_bridge(ctx).unwrap_or_default();
    let prompt = orch::write_prompt(Harness::Claude)?;
    // best effort, like bro's other skills
    let _ = orch::write_skill(config_dir);
    let end = options_end(args);
    args.splice(end..end, [PROMPT_FILE.to_string(), prompt.to_string_lossy().into_owned()]);
    let mcp = orch::write_mcp_config(bridge)?;
    // both options are variadic: last, or first when a `--` ends the options
    let tools = [
        "--allowedTools".to_string(),
        format!("mcp__{}", orch::MCP_SERVER),
        "--mcp-config".to_string(),
        mcp.to_string_lossy().into_owned(),
    ];
    if args.iter().any(|a| a == "--") {
        args.splice(0..0, tools);
    } else {
        args.extend(tools);
    }
    Ok(())
}

/// bro's Claude Code mod and theme (see [`crate::claude_mod`]): the plugin folder, bro's theme written
/// into this login's config dir, and what the mod reads about the launch. Returns the `--settings`
/// entries they need. Best effort: a file bro can't write leaves Claude as it ships.
fn bro_mod(cmd: &mut CommandSpec, args: &mut Vec<String>, ctx: &LaunchCtx, config_dir: &Path, profile: &str, backend: &str) -> Map<String, Value> {
    let Some(settings) = &ctx.claude_mod else { return Map::new() };
    // written before Claude starts: it watches its themes folder only if the folder was there
    let themed = settings.theme && ctx.palette.as_ref().is_some_and(|p| crate::claude_theme::write(config_dir, p).is_ok());
    if settings.on
        && let Ok(dir) = crate::claude_mod::install()
    {
        let end = options_end(args);
        args.splice(end..end, ["--plugin-dir".to_string(), dir.to_string_lossy().into_owned()]);
    }
    cmd.set_env("BRO_PROFILE", profile);
    cmd.set_env("BRO_BACKEND", backend);
    cmd.set_env("BRO_VERSION", env!("CARGO_PKG_VERSION"));
    settings.launch_settings(themed)
}

/// What the backend itself lists for `model`: the provider's catalogue, a Gemini sign-in's, or the
/// models a ChatGPT login reports to Codex.
fn listed_limits(target: &Target, model: &str) -> Option<crate::catalogue::Limits> {
    let provider = |id: &str| {
        let cfg = crate::config::Config::load().unwrap_or_default();
        crate::providers::find(&crate::providers::load(&cfg), id).and_then(|p| p.model(model).map(Into::into))
    };
    match target {
        Target::Login => None,
        Target::Provider(p, _) => p.model(model).map(Into::into),
        Target::Upstream(id) if crate::profiles::is_gemini_id(id) => provider("gemini"),
        Target::Upstream(id) => match crate::profiles::get(id) {
            Some(login) if login.is_codex() => crate::catalogue::codex_models(&login.dir)
                .into_iter()
                .find(|m| m.id == model)
                .map(|m| crate::catalogue::Limits { context: m.context, max_output: None }),
            Some(_) => None,
            None => provider(id),
        },
    }
}

/// The options part of `args`: everything before a `--`.
fn options_end(args: &[String]) -> usize {
    args.iter().position(|a| a == "--").unwrap_or(args.len())
}

/// Caveman mode (see [`crate::caveman`]): the rules in the system prompt, hooks for subagents
/// and resumes. The `--settings` file it writes also carries bro's own settings and `extra` (the
/// mod's). Best effort: a file bro can't write leaves the launch as it was. Returns the level applied.
fn apply_caveman(cmd: &mut CommandSpec, args: &mut Vec<String>, spec: &LaunchSpec, ctx: &LaunchCtx, extra: Map<String, Value>) -> Option<caveman::Level> {
    let level = ctx.caveman.active();
    let rules = level.and_then(|l| caveman::rules_file(l).ok());
    if let Some(merged) = merge_prompt_files(args, rules.as_deref(), &spec.cwd) {
        *args = merged;
    }
    let opts = &args[..options_end(args)];
    let has = |names: &[&str]| opts.iter().any(|a| names.iter().any(|n| a == n || a.starts_with(&format!("{n}="))));
    let resuming = spec.resume.is_some() || has(&["--continue", "-c", "--resume", "-r"]);
    // Claude Code keeps one --settings; yours wins (no focus view default, no bro theme, and caveman
    // then has no subagent / resume hooks).
    let mut base = ctx.claude_env.settings();
    base.extend(extra);
    if !has(&["--settings"])
        && let Ok(Some(file)) = caveman::claude_settings_file(&ctx.caveman, resuming, base)
    {
        args.splice(0..0, ["--settings".to_string(), file.to_string_lossy().into_owned()]);
    }
    let applied = level.filter(|_| rules.is_some());
    if applied.is_some() {
        // An installed upstream Caveman plugin stays quiet instead of injecting a second copy.
        cmd.set_env("CAVEMAN_DEFAULT_MODE", "off");
    }
    applied
}

/// Claude Code keeps only the last `--append-system-prompt-file`, so every one (bro's browser
/// briefing, your own) plus `extra` become one combined file. None when there's nothing to do.
fn merge_prompt_files(args: &[String], extra: Option<&Path>, cwd: &Path) -> Option<Vec<String>> {
    let mut out = args.to_vec();
    let mut end = options_end(&out);
    let mut files: Vec<PathBuf> = vec![];
    let mut at = None;
    let mut i = 0;
    while i < end {
        let file = if out[i] == PROMPT_FILE && i + 1 < end {
            end -= 2;
            out.drain(i..i + 2).nth(1)
        } else if let Some(f) = out[i].strip_prefix(PROMPT_FILE).and_then(|r| r.strip_prefix('=')).map(str::to_string) {
            end -= 1;
            out.remove(i);
            Some(f)
        } else {
            i += 1;
            None
        };
        if let Some(f) = file {
            at.get_or_insert(i);
            files.push(cwd.join(f));
        }
    }
    let had = files.len();
    files.extend(extra.map(Path::to_path_buf));
    let file = match files.len() {
        0 => return None,
        1 if had == 1 => return None,
        1 => files.remove(0),
        _ => {
            let parts: Vec<String> = files.iter().filter_map(|f| std::fs::read_to_string(f).ok()).collect();
            caveman::combined_prompt_file(&parts).ok()?
        }
    };
    let at = at.unwrap_or(0);
    out.splice(at..at, [PROMPT_FILE.to_string(), file.to_string_lossy().into_owned()]);
    Some(out)
}

/// Returns whether the upstream speaks Anthropic Messages.
fn wire_proxy(cmd: &mut CommandSpec, upstream: &str, spec: &LaunchSpec, ctx: &LaunchCtx) -> anyhow::Result<bool> {
    let route = proxy_route(upstream, spec.model.as_deref(), ctx)?;
    cmd.set_env("ANTHROPIC_BASE_URL", route.base.clone());
    cmd.set_env("ANTHROPIC_AUTH_TOKEN", route.token.clone());
    cmd.remove_env("ANTHROPIC_API_KEY");
    if !route.anthropic_upstream {
        cmd.set_env("CLAUDE_CODE_DISABLE_1M_CONTEXT", "1");
    }
    cmd.route = Some(route.request);
    Ok(route.anthropic_upstream)
}
