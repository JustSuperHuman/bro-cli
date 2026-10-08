//! Claude Code launches (v1 `launch.js` claude path, `pool.js` `runAccountProfile`).
use super::{CommandSpec, LaunchCtx, LaunchSpec, Permission, Resolved, Target, label, prepare_resume, proxy_route, require_key};
use crate::browser::{self, BrowserCtx};
use crate::profiles::ProfileKind;
use crate::providers::ProviderMode;
use crate::{Harness, paths};

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
    match &r.target {
        Target::Login => {
            third_party = false;
            who = login.map(|p| p.name.clone()).unwrap_or_else(|| "local".into());
            if ctx.routes_native() {
                wire_proxy(&mut cmd, &target_profile, spec, ctx)?;
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
            wire_proxy(&mut cmd, &p.id, spec, ctx)?;
        }
        Target::Upstream(up) => {
            who = up.clone();
            wire_proxy(&mut cmd, up, spec, ctx)?;
        }
    }

    if third_party && let Some(m) = spec.model.as_deref().filter(|m| !m.is_empty()) {
        for var in MODEL_VARS {
            cmd.set_env(var, m.to_string());
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

fn wire_proxy(cmd: &mut CommandSpec, upstream: &str, spec: &LaunchSpec, ctx: &LaunchCtx) -> anyhow::Result<()> {
    let route = proxy_route(upstream, spec.model.as_deref(), ctx)?;
    cmd.set_env("ANTHROPIC_BASE_URL", route.base.clone());
    cmd.set_env("ANTHROPIC_AUTH_TOKEN", route.token.clone());
    cmd.remove_env("ANTHROPIC_API_KEY");
    if !route.anthropic_upstream {
        cmd.set_env("CLAUDE_CODE_DISABLE_1M_CONTEXT", "1");
    }
    cmd.route = Some(route.request);
    Ok(())
}
