//! Turns "harness × profile × provider × model × cwd" into a concrete command.
//! No process spawning here (the TUI spawns into a PTY). May write harness config
//! files that v1 also wrote (pi models.json, omp models.yml) — never ccr.
//!
//! Routing (see [`build`]):
//!
//! | harness | native login | Anthropic-format provider | OpenAI-format provider | other login / `pool` |
//! |---|---|---|---|---|
//! | claude | `CLAUDE_CONFIG_DIR` | `ANTHROPIC_BASE_URL` direct | bro-proxy | bro-proxy |
//! | codex | `CODEX_HOME` | Responses URL direct if it has one, else bro-proxy | `-c model_providers.bro` direct | bro-proxy (`wire_api=responses`) |
//! | pi / omp | the harness's own | models.json / models.yml direct | direct | bro-proxy (anthropic-messages) |
mod claude;
mod codex;
mod pi;
mod route;
mod which;

pub use pi::{omp_models_path, pi_models_path, write_omp_config, write_pi_config};
pub use route::{UpstreamTarget, resolve_upstream, route_id};
pub use which::{program_and_args, which};

use crate::Harness;
use crate::profiles::{self, Profile};
use crate::providers::{Provider, ProviderMode};
use anyhow::{Context, anyhow, bail};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    Default,
    /// claude `--permission-mode auto`
    Auto,
    /// claude `--dangerously-skip-permissions`, codex and omp `--yolo`. The default: bro always
    /// launches agents unattended.
    #[default]
    Skip,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Resume {
    pub session_id: String,
    /// Fork instead of continuing (cross-profile resume always forks)
    pub fork: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LaunchSpec {
    pub harness: Harness,
    /// Login to run as ("claude:work", "codex:local"). None + provider = key-based provider.
    pub profile_id: Option<String>,
    /// Provider id from the catalogue, or "pool" (Claude account pool via proxy).
    /// None = the profile's native backend.
    pub provider_id: Option<String>,
    pub model: Option<String>,
    pub cwd: PathBuf,
    pub resume: Option<Resume>,
    pub permission: Permission,
    pub browser: crate::browser::BrowserMode,
    pub extra_args: Vec<String>,
}

/// Where the running bro-proxy lives, so cross-format launches can be routed through it.
#[derive(Debug, Clone, Default)]
pub struct LaunchCtx {
    /// e.g. "http://127.0.0.1:3458"
    pub proxy_base: Option<String>,
    /// Token the proxy expects from local clients
    pub proxy_token: Option<String>,
}

/// A proxy route the caller must register with bro-proxy before spawning.
/// Harness talks to `{proxy_base}/r/{route_id}/v1/...`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteRequest {
    pub route_id: String,
    /// Provider id, "pool", "codex:<profile>" (ChatGPT backend) or "claude:<profile>" (OAuth)
    pub upstream: String,
    pub model: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct CommandSpec {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub env_remove: Vec<String>,
    pub cwd: PathBuf,
    /// Short human label for tabs, e.g. "claude · work · opus"
    pub label: String,
    /// Route to register with the proxy (cross-format provider launches)
    pub route: Option<RouteRequest>,
    /// Temp files to delete when the session exits (staged cross-profile resumes);
    /// pass to [`crate::sessions::cleanup_staged`].
    pub cleanup: Vec<PathBuf>,
}

impl CommandSpec {
    fn set_env(&mut self, key: &str, value: impl Into<String>) {
        self.env.retain(|(k, _)| k != key);
        self.env_remove.retain(|k| k != key);
        self.env.push((key.to_string(), value.into()));
    }

    fn remove_env(&mut self, key: &str) {
        self.env.retain(|(k, _)| k != key);
        if !self.env_remove.iter().any(|k| k == key) {
            self.env_remove.push(key.to_string());
        }
    }
}

/// What a launch talks to, after resolving profile/provider ids.
#[derive(Debug, Clone)]
enum Target {
    /// The harness's own login: the given profile (or its default)
    Login,
    /// A catalogue provider with its key
    Provider(Box<Provider>, Option<String>),
    /// Through bro-proxy: "pool", "claude:<p>", "codex:<p>" or a provider id
    Upstream(String),
}

/// Resolved inputs shared by the per-harness builders.
struct Resolved {
    harness: Harness,
    profile: Option<Profile>,
    target: Target,
}

fn family_matches(harness: Harness, profile: &Profile) -> bool {
    match harness {
        Harness::Claude => profile.is_claude(),
        Harness::Codex => profile.is_codex(),
        Harness::Pi | Harness::Omp => false,
    }
}

fn get_profile(id: &str) -> anyhow::Result<Profile> {
    profiles::get(id).ok_or_else(|| anyhow!("unknown profile {id:?}"))
}

fn resolve(spec: &LaunchSpec) -> anyhow::Result<Resolved> {
    let mut profile = spec.profile_id.as_deref().filter(|s| !s.is_empty()).map(get_profile).transpose()?;
    let provider_id = spec.provider_id.as_deref().filter(|s| !s.is_empty());
    let target = match provider_id {
        Some("pool") => Target::Upstream("pool".into()),
        Some(id) if id.starts_with("claude:") || id.starts_with("codex:") => {
            let login = get_profile(id)?;
            if family_matches(spec.harness, &login) && profile.as_ref().is_none_or(|p| p.id == login.id) {
                profile = Some(login);
                Target::Login
            } else {
                Target::Upstream(login.id)
            }
        }
        Some(id) => {
            let cfg = crate::config::Config::load().unwrap_or_default();
            let providers = crate::providers::load(&cfg);
            let p = crate::providers::find(&providers, id)
                .ok_or_else(|| anyhow!("unknown provider {id:?}"))?
                .for_model(spec.model.as_deref());
            let key = cfg.key_for(&p.id, p.key_env.as_deref());
            Target::Provider(Box::new(p), key)
        }
        None => match &profile {
            Some(p) if !family_matches(spec.harness, p) => Target::Upstream(p.id.clone()),
            _ => Target::Login,
        },
    };
    Ok(Resolved { harness: spec.harness, profile, target })
}

/// Proxy wiring for one launch.
pub(crate) struct ProxyRoute {
    pub request: RouteRequest,
    /// `{proxy_base}/r/{route_id}` (no trailing slash, no `/v1`)
    pub base: String,
    pub token: String,
    /// Upstream speaks Anthropic Messages (pool / Claude login / Anthropic provider)
    pub anthropic_upstream: bool,
}

fn proxy_route(upstream: &str, model: Option<&str>, ctx: &LaunchCtx) -> anyhow::Result<ProxyRoute> {
    let base = ctx
        .proxy_base
        .as_deref()
        .filter(|b| !b.is_empty())
        .ok_or_else(|| anyhow!("this launch goes through bro-proxy ({upstream}), but the proxy isn't running"))?;
    let id = route_id(upstream, model);
    let anthropic_upstream = upstream == "pool"
        || upstream.starts_with("claude:")
        || (!upstream.starts_with("codex:") && {
            let cfg = crate::config::Config::load().unwrap_or_default();
            crate::providers::find(&crate::providers::load(&cfg), upstream)
                .is_some_and(|p| p.for_model(model).mode != ProviderMode::Openai)
        });
    Ok(ProxyRoute {
        request: RouteRequest { route_id: id.clone(), upstream: upstream.to_string(), model: model.map(str::to_string) },
        base: format!("{}/r/{id}", base.trim_end_matches('/')),
        token: ctx.proxy_token.clone().filter(|t| !t.is_empty()).unwrap_or_else(|| "bro".into()),
        anthropic_upstream,
    })
}

fn require_key(p: &Provider, key: &Option<String>) -> anyhow::Result<()> {
    if key.is_none() && !p.no_key && p.mode != ProviderMode::Native {
        let env = p.key_env.as_deref().map(|e| format!(" or ${e}")).unwrap_or_default();
        bail!("no API key for {} (set keys.{} in ~/.bro/config.json{env})", p.display_name(), p.id);
    }
    Ok(())
}

/// Short tab label: "claude · work · opus".
fn label(harness: Harness, who: &str, model: Option<&str>) -> String {
    let mut parts = vec![harness.label().to_string()];
    if !who.is_empty() {
        parts.push(who.to_string());
    }
    if let Some(m) = model.filter(|m| !m.is_empty()) {
        parts.push(m.to_string());
    }
    parts.join(" · ")
}

/// Build the command for one launch. Performs no spawning; may stage session files for
/// a cross-profile resume (listed in `cleanup`) and upsert Pi/omp provider configs.
pub fn build(spec: &LaunchSpec, ctx: &LaunchCtx) -> anyhow::Result<CommandSpec> {
    let r = resolve(spec)?;
    let mut out = match r.harness {
        Harness::Claude => claude::build(spec, ctx, &r),
        Harness::Codex => codex::build(spec, ctx, &r),
        Harness::Pi | Harness::Omp => pi::build(spec, ctx, &r),
    };
    if let Ok(cmd) = &mut out {
        cmd.cwd = spec.cwd.clone();
    }
    out.with_context(|| format!("building {} launch", spec.harness.label()))
}

/// Cross-profile resume for Claude/Codex: when the session lives in another login,
/// stage it into `target_profile_id` and force a fork. Returns (id, fork).
fn prepare_resume(
    harness: Harness,
    resume: &Resume,
    target_profile_id: &str,
    cleanup: &mut Vec<PathBuf>,
) -> anyhow::Result<(String, bool)> {
    let id = resume.session_id.clone();
    if crate::sessions::find_in_profile(harness, &id, Some(target_profile_id)).is_some() {
        return Ok((id, resume.fork));
    }
    match crate::sessions::find_by_id(harness, &id) {
        Some(session) => {
            let staged = crate::sessions::stage_for_profile(&session, target_profile_id)?;
            cleanup.extend(staged);
            Ok((id, true))
        }
        // Unknown here: let the harness look it up (it may know better).
        None => Ok((id, resume.fork)),
    }
}

#[cfg(test)]
mod tests;
