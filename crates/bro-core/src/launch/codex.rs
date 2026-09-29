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

    let mut provider = Vec::new();
    let who = match &r.target {
        Target::Login => login.map(|p| p.name.clone()).unwrap_or_else(|| "local".into()),
        Target::Provider(p, key) => match p.responses_base() {
            Some(base) => {
                require_key(p, key)?;
                provider = provider_args(p.display_name(), &base, key.is_some());
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
        args.push("--dangerously-bypass-approvals-and-sandbox".into());
    }
    if let Some(m) = spec.model.as_deref().filter(|m| !m.is_empty()) {
        args.extend(["--model".into(), m.to_string()]);
    }
    args.extend(provider);
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
