//! Claude Code launches (v1 `launch.js` claude path, `pool.js` `runAccountProfile`).
use super::{CommandSpec, LaunchCtx, LaunchSpec, Permission, Resolved, Target, label, prepare_resume, proxy_route, require_key};
use crate::browser::{self, BrowserCtx};
use crate::profiles::ProfileKind;
use crate::providers::ProviderMode;
use crate::{Harness, caveman, paths};
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
    match login {
        Some(p) if p.kind == ProfileKind::ClaudeAccount => cmd.set_env("CLAUDE_CONFIG_DIR", p.dir.to_string_lossy()),
        _ => cmd.remove_env("CLAUDE_CONFIG_DIR"),
    }
    for var in BACKEND_VARS.iter().chain(&MODEL_VARS) {
        cmd.remove_env(var);
    }

    let who;
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
            if ctx.routes_native() {
                proxied_anthropic = wire_proxy(&mut cmd, &target_profile, spec, ctx)?;
                proxied_login = true;
            }
        }
        Target::Provider(p, _) if p.mode == ProviderMode::Native => {
            third_party = false;
            who = login.map(|p| p.name.clone()).unwrap_or_else(|| "local".into());
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
        }
        Target::Provider(p, key) => {
            // OpenAI-format provider: Claude Code speaks Anthropic, so bro-proxy translates.
            require_key(p, key)?;
            who = p.id.clone();
            proxied_anthropic = wire_proxy(&mut cmd, &p.id, spec, ctx)?;
        }
        Target::Upstream(up) => {
            who = up.clone();
            proxied_anthropic = wire_proxy(&mut cmd, up, spec, ctx)?;
        }
    }

    if third_party && let Some(m) = spec.model.as_deref().filter(|m| !m.is_empty()) {
        for var in MODEL_VARS {
            cmd.set_env(var, m.to_string());
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
    cmd.caveman = apply_caveman(&mut cmd, &mut args, spec, ctx);
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

/// The options part of `args`: everything before a `--`.
fn options_end(args: &[String]) -> usize {
    args.iter().position(|a| a == "--").unwrap_or(args.len())
}

/// Caveman mode (see [`crate::caveman`]): the rules in the system prompt, hooks for subagents
/// and resumes. Best effort: a file bro can't write leaves the launch as it was. Returns the
/// level applied.
fn apply_caveman(cmd: &mut CommandSpec, args: &mut Vec<String>, spec: &LaunchSpec, ctx: &LaunchCtx) -> Option<caveman::Level> {
    let level = ctx.caveman.active();
    let rules = level.and_then(|l| caveman::rules_file(l).ok());
    if let Some(merged) = merge_prompt_files(args, rules.as_deref(), &spec.cwd) {
        *args = merged;
    }
    let opts = &args[..options_end(args)];
    let has = |names: &[&str]| opts.iter().any(|a| names.iter().any(|n| a == n || a.starts_with(&format!("{n}="))));
    let resuming = spec.resume.is_some() || has(&["--continue", "-c", "--resume", "-r"]);
    // Claude Code keeps one --settings; yours wins (caveman then has no subagent / resume hooks).
    if !has(&["--settings"])
        && let Ok(Some(file)) = caveman::claude_settings_file(&ctx.caveman, resuming)
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
