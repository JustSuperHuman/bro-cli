//! Codex CLI launches (v1 `launchCodex` / `codexProviderConfig` / `runCodex`).
use super::{CommandSpec, LaunchCtx, LaunchSpec, Permission, Resolved, Target, label, prepare_resume, proxy_route, require_key};
use crate::providers::ProviderMode;
use crate::{Harness, paths};

/// The provider slot bro describes to codex via `-c` overrides (never written to
/// config.toml), and the env var codex reads its key from.
const SLOT: &str = "bro";
/// Env var holding the provider key for codex (`env_key`).
pub const CODEX_KEY_ENV: &str = "BRO_PROVIDER_API_KEY";

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
    let _ = crate::skills::write_bro_open_skill(&home);
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
                p.id.clone()
            }
        },
        Target::Upstream(up) => {
            provider = wire_proxy(&mut cmd, up, spec, ctx)?;
            up.clone()
        }
    };

    let mut args = Vec::new();
    if let Some(resume) = &spec.resume {
        let (id, fork) = prepare_resume(Harness::Codex, resume, &target_profile, &mut cmd.cleanup)?;
        args.extend([if fork { "fork" } else { "resume" }.to_string(), id]);
    }
    if spec.permission == Permission::Skip {
        args.push("--yolo".into());
    }
    // An elevated codex refuses to start its shared background server ("start the Windows daemon from a
    // non-elevated terminal") and exits before showing anything; without the daemon it runs in-process.
    if elevated() {
        args.push("--no-daemon".into());
    }
    if let Some(m) = spec.model.as_deref().filter(|m| !m.is_empty()) {
        args.extend(["--model".into(), m.to_string()]);
    }
    args.extend(provider);
    if let Some(level) = ctx.caveman.active() {
        // `-c` replaces the login's own developer_instructions, so those come first.
        let own = std::fs::read_to_string(home.join("config.toml"))
            .ok()
            .and_then(|s| s.parse::<toml::Table>().ok())
            .and_then(|t| t.get("developer_instructions").and_then(|v| v.as_str()).map(str::to_string))
            .filter(|s| !s.trim().is_empty());
        let text = match own {
            Some(own) => format!("{}

{}", own.trim_end(), level.rules()),
            None => level.rules().to_string(),
        };
        args.extend(["-c".into(), format!("developer_instructions={}", toml_str(&text))]);
        cmd.caveman = Some(level);
    }
    args.extend(spec.extra_args.iter().cloned());
    (cmd.program, cmd.args) = super::program_and_args("codex", args);
    cmd.label = label(Harness::Codex, &who, spec.model.as_deref());
    Ok(cmd)
}

/// Is bro running as administrator (an elevated token)? Children inherit it.
#[cfg(windows)]
pub(super) fn elevated() -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    static ELEVATED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ELEVATED.get_or_init(|| unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return false;
        }
        let mut e = TOKEN_ELEVATION { TokenIsElevated: 0 };
        let mut len = 0u32;
        let ok = GetTokenInformation(token, TokenElevation, (&raw mut e).cast(), size_of::<TOKEN_ELEVATION>() as u32, &mut len) != 0;
        CloseHandle(token);
        ok && e.TokenIsElevated != 0
    })
}

#[cfg(not(windows))]
pub(super) fn elevated() -> bool {
    false
}

fn wire_proxy(cmd: &mut CommandSpec, upstream: &str, spec: &LaunchSpec, ctx: &LaunchCtx) -> anyhow::Result<Vec<String>> {
    let route = proxy_route(upstream, spec.model.as_deref(), ctx)?;
    cmd.set_env(CODEX_KEY_ENV, route.token.clone());
    let args = provider_args(&format!("bro-proxy ({upstream})"), &format!("{}/v1", route.base), true);
    cmd.route = Some(route.request);
    Ok(args)
}
