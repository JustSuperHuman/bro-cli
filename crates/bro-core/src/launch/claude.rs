//! Claude Code launches (v1 `launch.js` claude path, `pool.js` `runAccountProfile`).
use super::{CommandSpec, LaunchCtx, LaunchSpec, Permission, Resolved, Target, label, prepare_resume, proxy_route, require_key};
use crate::browser::{self, BrowserCtx};
use crate::profiles::ProfileKind;
use crate::providers::ProviderMode;
use crate::{Harness, paths};

/// Vars that would point Claude Code at some other backend than the one chosen.
const BACKEND_VARS: [&str; 4] = ["ANTHROPIC_BASE_URL", "ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_API_KEY", "CLAUDE_CODE_DISABLE_1M_CONTEXT"];

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
    match login {
        Some(p) if p.kind == ProfileKind::ClaudeAccount => cmd.set_env("CLAUDE_CONFIG_DIR", p.dir.to_string_lossy()),
        _ => cmd.remove_env("CLAUDE_CONFIG_DIR"),
    }
    for var in BACKEND_VARS {
        cmd.remove_env(var);
    }

    let who;
    let mut third_party = true;
    match &r.target {
        Target::Login => {
            third_party = false;
            who = login.map(|p| p.name.clone()).unwrap_or_else(|| "local".into());
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

    let mut args = permission_args(spec.permission);
    let wants_browser = !spec.extra_args.iter().any(|a| a == "--chrome" || a == "--no-chrome");
    if wants_browser {
        let claude_path = super::which("claude").map(|p| p.to_string_lossy().into_owned());
        let bctx = BrowserCtx {
            profile_dir: Some(&config_dir),
            third_party_auth: third_party,
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
