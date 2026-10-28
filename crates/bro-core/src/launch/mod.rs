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
//!
//! Proxy mode ([`LaunchCtx::shared_logins`]): a launch on a login (its own, another family's, or
//! the pool) runs on every signed-in login of that family instead (`logins:claude` /
//! `logins:codex`). Claude Code and Codex stay signed in as one login, so they are themselves as
//! ever (their own `/model` lists, plans and limits); only their requests go to bro-proxy, which
//! sends each one as the login with the most usage left.
mod claude;
mod codex;
mod pi;
mod route;
mod which;

pub use pi::{omp_models_path, pi_models_path, write_omp_config, write_pi_config};
pub use route::{UpstreamTarget, resolve_upstream, route_id};
pub use which::{extra_bin_dirs, program_and_args, which};

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
    /// Launch as bro's orchestrator: its instructions, skill and the bridge's tools (see
    /// [`crate::orchestrator`]). Needs [`LaunchCtx::bridge_url`].
    #[serde(default)]
    pub orchestrator: bool,
}

/// Where the running bro-proxy lives, so cross-format launches can be routed through it.
#[derive(Debug, Clone, Default)]
pub struct LaunchCtx {
    /// e.g. "http://127.0.0.1:3458"
    pub proxy_base: Option<String>,
    /// Token the proxy expects from local clients
    pub proxy_token: Option<String>,
    /// Route native Claude / Codex logins through the proxy as their own login (request
    /// compression is on; see `ProxySettings::routes_native`). Ignored without `proxy_base`.
    pub route_native: bool,
    /// Privacy / behaviour env for Claude launches (`Settings::claude_env`).
    pub claude_env: crate::claude_env::ClaudeEnvSettings,
    /// Caveman mode for Claude / Codex / Pi launches (`Settings::caveman`).
    pub caveman: crate::caveman::CavemanSettings,
    /// Claude / Codex launches get the JustTools skill (`Settings::justtools`).
    pub justtools_skill: bool,
    /// Claude / Codex launches get the JustWatch skill (`Settings::justwatch`).
    pub justwatch_skill: bool,
    /// The `bro-open` skill is switched off (`Settings::skills`): launches don't write it.
    pub bro_open_off: bool,
    /// Watched skill repos (`Settings::skills`): their cloned skills go into the launch's home too, which the
    /// last sync may not have seen (a login added since).
    pub skill_repos: Vec<String>,
    /// Loopback URL of the running bridge (`http://127.0.0.1:<port>`): an orchestrator launch
    /// points its tools here.
    pub bridge_url: Option<String>,
    /// bro's Claude Code mod (`Settings::claude_mod`). None: Claude as it ships.
    pub claude_mod: Option<crate::claude_mod::ClaudeModSettings>,
    /// The bro executable: Claude launches run it as the hooks that keep messages between sessions to their
    /// project (see [`crate::peers`]). None: no such hooks, Claude Code's own reach.
    pub bro_bin: Option<PathBuf>,
    /// PeonPing sounds (`Settings::peon`): which harnesses run `bro hook peon` (needs `bro_bin`).
    pub peon: crate::peon::PeonSettings,
    /// Proxy mode (`ProxySettings::shared`): launches on a login run on all signed-in logins of its
    /// family, through bro-proxy (see the module docs).
    pub shared_logins: bool,
}

impl LaunchCtx {
    /// The proxy is running and native logins should go through it.
    fn routes_native(&self) -> bool {
        self.route_native && self.proxy_base.as_deref().is_some_and(|b| !b.is_empty())
    }
}

/// Upstream id of proxy mode's shared Claude logins.
pub const LOGINS_CLAUDE: &str = "logins:claude";
/// Upstream id of proxy mode's shared Codex (ChatGPT) logins.
pub const LOGINS_CODEX: &str = "logins:codex";

/// The shared logins of the Claude family (`true`) or the Codex one.
pub fn logins_upstream(claude: bool) -> &'static str {
    if claude { LOGINS_CLAUDE } else { LOGINS_CODEX }
}

/// Id of the route proxy-mode sessions share (one per family, and per model for a harness running
/// on the other family's logins). It is the route's key (Claude Code and Codex keep their own sign-in
/// and can't send the proxy's), so it comes from the proxy's token: unguessable, and the same in the
/// next window, whose proxy gets the token back from the session server.
pub fn logins_route_id(token: &str, upstream: &str, model: Option<&str>) -> String {
    format!("k{}", crate::util::short_hash(&format!("{token}\u{0}{upstream}\u{0}{}", model.unwrap_or("")), 24))
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
    /// The caveman level this launch got, for the session's badge (None: off, or the harness
    /// can't take it).
    pub caveman: Option<crate::caveman::Level>,
    /// The login whose folder the session runs in, where its transcript is written ("claude:work").
    /// None for harnesses that keep theirs elsewhere (Pi, omp).
    pub store: Option<String>,
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
    /// Proxy mode: every signed-in login of the Claude family (`true`) or the Codex one
    Logins(bool),
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

fn resolve(spec: &LaunchSpec, shared: bool) -> anyhow::Result<Resolved> {
    let mut profile = spec.profile_id.as_deref().filter(|s| !s.is_empty()).map(get_profile).transpose()?;
    let provider_id = spec.provider_id.as_deref().filter(|s| !s.is_empty());
    let target = match provider_id {
        Some(LOGINS_CLAUDE) => Target::Logins(true),
        Some(LOGINS_CODEX) => Target::Logins(false),
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
    let target = if shared { share(spec.harness, target, profile.as_ref()) } else { target };
    if let Target::Logins(claude) = target {
        let all = if claude { profiles::list_claude() } else { profiles::list_codex() };
        let members = profiles::subscription_logins_in(&all, claude);
        if members.is_empty() {
            let who = if claude { "Claude" } else { "Codex (signed in with ChatGPT)" };
            bail!("proxy mode shares your signed-in {who} logins, and none is signed in: add one in profiles (alt+o)");
        }
        // Running on its own family's logins, the harness is signed in as one of them: the login the
        // launch names (it holds the conversation being resumed), else the machine's own.
        if matches!((spec.harness, claude), (Harness::Claude, true) | (Harness::Codex, false)) {
            let signed_in = |p: &&Profile| p.authenticated && family_matches(spec.harness, p) && p.plan.as_deref() != Some("api");
            let named = profile.as_ref().filter(signed_in).cloned();
            profile = named.or_else(|| members.first().map(|p| (*p).clone()));
        }
    }
    Ok(Resolved { harness: spec.harness, profile, target })
}

/// Proxy mode's take on a target: anything that runs on a login runs on all of that family's logins.
fn share(harness: Harness, target: Target, profile: Option<&Profile>) -> Target {
    let family = |id: &str| if id == "pool" || id.starts_with("claude:") { Some(true) } else if id.starts_with("codex:") { Some(false) } else { None };
    match target {
        Target::Login => match harness {
            Harness::Claude => Target::Logins(true),
            Harness::Codex => Target::Logins(false),
            // Pi and omp keep the login they're configured with
            Harness::Pi | Harness::Omp => Target::Login,
        },
        Target::Upstream(up) => match family(&up) {
            Some(claude) => Target::Logins(claude),
            None => Target::Upstream(up),
        },
        // "Claude's own backend": the Claude logins, for whichever harness asked
        Target::Provider(p, key) if p.mode == ProviderMode::Native => match harness {
            Harness::Claude | Harness::Codex => Target::Logins(true),
            Harness::Pi | Harness::Omp if profile.is_some_and(|p| p.is_claude()) => Target::Logins(true),
            Harness::Pi | Harness::Omp => Target::Provider(p, key),
        },
        other => other,
    }
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

/// A launch needs a translating proxy before its command can be built. Callers may start one and retry.
#[derive(Debug)]
pub struct ProxyRequired(pub String);

impl std::fmt::Display for ProxyRequired {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "this launch goes through bro-proxy ({}), but the proxy isn't running", self.0)
    }
}

impl std::error::Error for ProxyRequired {}

fn proxy_route(upstream: &str, model: Option<&str>, ctx: &LaunchCtx) -> anyhow::Result<ProxyRoute> {
    let base = ctx
        .proxy_base
        .as_deref()
        .filter(|b| !b.is_empty())
        .ok_or_else(|| ProxyRequired(upstream.to_string()))?;
    let token = ctx.proxy_token.clone().filter(|t| !t.is_empty()).unwrap_or_else(|| "bro".into());
    let shared = upstream == LOGINS_CLAUDE || upstream == LOGINS_CODEX;
    let id = if shared { logins_route_id(&token, upstream, model) } else { route_id(upstream, model) };
    let anthropic_upstream = upstream == "pool"
        || upstream == LOGINS_CLAUDE
        || upstream.starts_with("claude:")
        || (!upstream.starts_with("codex:") && !shared && {
            let cfg = crate::config::Config::load().unwrap_or_default();
            crate::providers::find(&crate::providers::load(&cfg), upstream)
                .is_some_and(|p| matches!(p.for_model(model).mode, ProviderMode::Native | ProviderMode::Anthropic))
        });
    Ok(ProxyRoute {
        request: RouteRequest { route_id: id.clone(), upstream: upstream.to_string(), model: model.map(str::to_string) },
        base: format!("{}/r/{id}", base.trim_end_matches('/')),
        token,
        anthropic_upstream,
    })
}

fn require_key(p: &Provider, key: &Option<String>) -> anyhow::Result<()> {
    if p.is_google_login() {
        if !crate::gemini::signed_in(&crate::gemini::creds_path()) {
            bail!("{}", crate::gemini::SIGN_IN_HINT);
        }
        return Ok(());
    }
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
    if spec.orchestrator && orchestrator_bridge(ctx).is_none() {
        bail!("the orchestrator drives bro through its bridge, which isn't running (turn it on in the bridge view)");
    }
    // the orchestrator always runs in its own folder in your profile
    let moved;
    let spec = if spec.orchestrator {
        let dir = crate::orchestrator::ensure_workspace().with_context(|| format!("creating {}", crate::orchestrator::workspace().display()))?;
        moved = LaunchSpec { cwd: dir, ..spec.clone() };
        &moved
    } else {
        spec
    };
    let r = resolve(spec, ctx.shared_logins)?;
    let mut out = match r.harness {
        Harness::Claude => claude::build(spec, ctx, &r),
        Harness::Codex => codex::build(spec, ctx, &r),
        Harness::Pi | Harness::Omp => pi::build(spec, ctx, &r),
    };
    if let Ok(cmd) = &mut out {
        cmd.cwd = spec.cwd.clone();
        if spec.orchestrator {
            cmd.set_env(crate::orchestrator::ENV_FLAG, "1");
            cmd.label = format!("orchestrator · {}", cmd.label);
        }
    }
    out.with_context(|| format!("building {} launch", spec.harness.label()))
}

/// The bridge an orchestrator launch talks to.
fn orchestrator_bridge(ctx: &LaunchCtx) -> Option<&str> {
    ctx.bridge_url.as_deref().map(str::trim).filter(|u| !u.is_empty())
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
