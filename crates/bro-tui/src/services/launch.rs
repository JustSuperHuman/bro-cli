//! Launching a session: `bro_core::launch::build` → register the proxy route it asks for → hand the command to
//! the UI, which spawns it into a PTY pane. Runs on a background thread.
//!
//! When `build` isn't available yet (the core is still `todo!()`), a small built-in launcher covers the plain
//! cases (a profile's own backend, model, permission, resume) so bro stays usable.

use super::{Services, State, guard, guard_res};
use crate::layout::PaneId;
use crate::pane::{Event, Place};
use bro_core::Harness;
use bro_core::launch::{CommandSpec, LaunchCtx, LaunchSpec, Permission, RouteRequest, UpstreamTarget};
use bro_core::profiles::{Profile, ProfileKind};
use bro_core::providers::ProviderMode;
use bro_core::sessions::SessionInfo;
use bro_proxy::{Route, Upstream};
use std::path::PathBuf;

/// What the UI (launcher, sidebar resume/fork, bridge create) asks for.
pub struct LaunchRequest {
    pub spec: LaunchSpec,
    pub place: Place,
    /// Fork: stage this session into `spec.profile_id` first (files are cleaned up on exit).
    pub stage: Option<SessionInfo>,
    /// A bridge client waits for the new session id.
    pub reply: Option<tokio::sync::oneshot::Sender<anyhow::Result<String>>>,
    /// A name for the session (sidebar / tab).
    pub name: Option<String>,
    /// Remember this combo in the launcher's recents.
    pub remember: bool,
    /// Moving a running session to another login: find its transcript first, then resume it in
    /// `spec.profile_id` (staged as a fork when that's a different login).
    pub find_live: Option<FindLive>,
    /// A queued resume's placeholder or a reloaded session: the new session takes its place.
    pub into: Option<PaneId>,
}

/// Which transcript a running session is writing.
pub struct FindLive {
    pub harness: bro_core::Harness,
    /// login whose folder holds it ("claude:work")
    pub store: Option<String>,
    pub cwd: std::path::PathBuf,
    pub since: std::time::SystemTime,
    /// The pane's bound transcript, when known; avoids choosing another session in the same folder.
    pub session_id: Option<String>,
}

impl LaunchRequest {
    pub fn new(spec: LaunchSpec, place: Place) -> LaunchRequest {
        LaunchRequest { spec, place, stage: None, reply: None, name: None, remember: false, find_live: None, into: None }
    }
}

/// The finished build, back on the UI thread.
pub struct Launched {
    pub spec: LaunchSpec,
    pub place: Place,
    pub reply: Option<tokio::sync::oneshot::Sender<anyhow::Result<String>>>,
    pub name: Option<String>,
    pub remember: bool,
    pub result: Result<CommandSpec, String>,
    /// Proxy route registered for this session (removed when it exits).
    pub route_id: Option<String>,
    /// Something worth telling the user (e.g. the built-in launcher was used).
    pub note: Option<String>,
    /// The placeholder pane this session replaces (see [`LaunchRequest::into`]).
    pub into: Option<PaneId>,
}

/// Build (and route) on this thread, then post `Event::Launched`.
pub(super) fn run(svc: &Services, req: LaunchRequest) {
    let LaunchRequest { mut spec, place, mut stage, reply, name, remember, find_live, into } = req;
    let mut note = None;
    let mut route_id = None;
    let result = (|| -> Result<CommandSpec, String> {
        if svc.is_demo() {
            let label = label_for(&spec);
            return Ok(super::demo::shell_command(Some(spec.harness), &label, spec.cwd.clone()));
        }
        if let Some(f) = find_live {
            stage = prepare_live_resume(&mut spec, f)?;
        }
        let mut staged = vec![];
        if let Some(s) = &stage {
            let target = spec.profile_id.clone().ok_or("fork needs a target profile")?;
            staged = guard_res("sessions::stage_for_profile", || bro_core::sessions::stage_for_profile(s, &target))?;
        }
        // the orchestrator drives bro through the bridge: start it when it's off and wait for it
        let bridge_url = if spec.orchestrator { Some(orchestrator_bridge(svc)?) } else { svc.bridge_local_url() };
        let mut ctx = {
            let st = svc.state();
            let claude_env = st.settings.claude_env.clone();
            let caveman = st.settings.caveman.clone();
            let justtools_skill = st.settings.justtools.skill;
            let justwatch_skill = st.settings.justwatch.skill;
            let bro_open_off = !st.settings.skills.bro_open;
            let skill_repos = st.settings.skills.repos.clone();
            let claude_mod = Some(st.settings.claude_mod.clone());
            // Claude sessions run bro as their peer hooks (see `bro_core::peers`)
            let bro_bin = std::env::current_exe().ok();
            let peon = st.settings.peon.clone();
            match st.proxy.status.ready() {
                Some(p) => LaunchCtx {
                    proxy_base: Some(p.base.clone()),
                    proxy_token: Some(p.token.clone()),
                    route_native: st.settings.proxy.routes_native(),
                    claude_env,
                    caveman,
                    justtools_skill,
                    justwatch_skill,
                    bro_open_off,
                    skill_repos,
                    bridge_url,
                    claude_mod,
                    bro_bin,
                    peon,
                },
                None => LaunchCtx { claude_env, caveman, justtools_skill, justwatch_skill, bro_open_off, skill_repos, bridge_url, claude_mod, bro_bin, peon, ..LaunchCtx::default() },
            }
        };
        let mut cmd = match guard("launch::build", || with_proxy_retry(svc, &mut ctx, |ctx| bro_core::launch::build(&spec, ctx))) {
            Ok(Ok(c)) => c,
            Ok(Err(e)) => return Err(format!("{e:#}")),
            Err(panic) => {
                note = Some(format!("{panic} — used the built-in launcher"));
                fallback_command(&spec, &svc.state())?
            }
        };
        cmd.cleanup.extend(staged);
        if let Some(route) = cmd.route.clone() {
            let upstream = resolve(&route, svc)?;
            let r = Route { id: route.route_id.clone(), upstream, default_model: route.model.clone(), small_model: None, model_map: vec![], label: cmd.label.clone() };
            svc.with_proxy("proxy::upsert_route", |p| p.upsert_route(r)).ok_or("this launch goes through the proxy, which isn't running")?;
            let routes = svc.with_proxy("proxy::routes", |p| p.routes()).unwrap_or_default();
            svc.update(|st| st.proxy.routes = routes);
            route_id = Some(route.route_id);
        }
        Ok(cmd)
    })();
    svc.send(Event::Launched(Box::new(Launched { spec, place, reply, name, remember, result, route_id, note, into })));
}

/// Resolve the pane's conversation before changing its process or staging a copy for another profile.
fn prepare_live_resume(spec: &mut LaunchSpec, f: FindLive) -> Result<Option<SessionInfo>, String> {
    let found = guard("sessions::find_live", || match f.session_id.as_deref() {
        Some(id) => bro_core::sessions::find_in_profile(f.harness, id, f.store.as_deref()),
        None => bro_core::sessions::latest_for(f.harness, f.store.as_deref(), &f.cwd, f.since),
    })
    .ok()
    .flatten()
    .ok_or("couldn't find this session's transcript — the agent needs to answer at least once")?;
    let same = transcript_store(spec) == f.store;
    spec.resume = Some(bro_core::launch::Resume { session_id: found.id.clone(), fork: !same });
    Ok((!same).then_some(found))
}

/// Where a launch writes its transcript, including agents routed through another provider.
pub fn transcript_store(spec: &LaunchSpec) -> Option<String> {
    let own = |family: &str| match &spec.profile_id {
        Some(p) if p.starts_with(family) => p.clone(),
        _ => format!("{family}local"),
    };
    match spec.harness {
        Harness::Claude => Some(own("claude:")),
        Harness::Codex => Some(own("codex:")),
        Harness::Pi | Harness::Omp => None,
    }
}

/// A secondary window can build native commands immediately. Start translation only when the launch needs it,
/// and wait for the listener before retrying, including when startup is already in flight.
fn with_proxy_retry<T>(svc: &Services, ctx: &mut LaunchCtx, mut build: impl FnMut(&LaunchCtx) -> anyhow::Result<T>) -> anyhow::Result<T> {
    match build(ctx) {
        Err(error) if error.is::<bro_core::launch::ProxyRequired>() => {
            let proxy = svc.ensure_proxy().map_err(anyhow::Error::msg)?;
            ctx.proxy_base = Some(proxy.base);
            ctx.proxy_token = Some(proxy.token);
            build(ctx)
        }
        result => result,
    }
}

/// How long an orchestrator launch waits for a bridge it just turned on.
const BRIDGE_WAIT: std::time::Duration = std::time::Duration::from_secs(20);

/// The running bridge's loopback URL for an orchestrator launch, turning the bridge on first if it's off.
fn orchestrator_bridge(svc: &Services) -> Result<String, String> {
    if let Some(url) = svc.bridge_local_url() {
        return Ok(url);
    }
    if !svc.state().bridge.enabled {
        svc.set_bridge_enabled(true);
        svc.send(Event::Toast(crate::alerts::Kind::Info, "turned on the bridge: the orchestrator drives bro through it".into()));
    }
    let until = std::time::Instant::now() + BRIDGE_WAIT;
    while std::time::Instant::now() < until {
        if let Some(url) = svc.bridge_local_url() {
            return Ok(url);
        }
        std::thread::sleep(std::time::Duration::from_millis(150));
    }
    Err("the bridge didn't start, and the orchestrator needs it to reach your sessions · check the bridge view".into())
}

/// "claude · work · opus" for a spec (used before a CommandSpec exists).
pub fn label_for(spec: &LaunchSpec) -> String {
    let mut parts = vec![spec.harness.label().to_string()];
    if spec.orchestrator {
        parts.insert(0, "orchestrator".into());
    }
    if let Some(p) = spec.provider_id.as_deref().or(spec.profile_id.as_deref()) {
        parts.push(if p.starts_with("gemini-sub:") { "Gemini subscription".into() } else if let Some(name) = p.strip_prefix("gemini:") { format!("Gemini/{name}") } else { p.split(':').next_back().unwrap_or(p).to_string() });
    }
    if let Some(m) = &spec.model {
        parts.push(short_model(m));
    }
    parts.join(" · ")
}

/// "claude-opus-5" → "opus-5", "gpt-5-codex" stays.
pub fn short_model(m: &str) -> String {
    m.strip_prefix("claude-").unwrap_or(m).to_string()
}

/// Resolve a route's upstream with bro-core (`launch::resolve_upstream`), falling back to [`map_upstream`]
/// only if that call panics.
fn resolve(route: &RouteRequest, svc: &Services) -> Result<Upstream, String> {
    let cfg = svc.state().config.clone().unwrap_or_default();
    match guard("launch::resolve_upstream", || bro_core::launch::resolve_upstream(&route.upstream, route.model.as_deref(), &cfg)) {
        Ok(Ok(t)) => Ok(to_proxy(t)),
        Ok(Err(e)) => Err(format!("{e:#}")),
        Err(_) => map_upstream(&route.upstream, &svc.state()),
    }
}

/// bro-core's mirror of the proxy upstream → the proxy's own type (field for field).
pub fn to_proxy(t: UpstreamTarget) -> Upstream {
    match t {
        UpstreamTarget::OpenAiChat { base_url, api_key } => Upstream::OpenAiChat { base_url, api_key },
        UpstreamTarget::OpenAiResponses { base_url, api_key } => Upstream::OpenAiResponses { base_url, api_key },
        UpstreamTarget::ChatGptCodex { codex_home } => Upstream::ChatGptCodex { codex_home },
        UpstreamTarget::Anthropic { base_url, api_key, bearer } => Upstream::Anthropic { base_url, api_key, bearer },
        UpstreamTarget::ClaudeOAuth { config_dir } => Upstream::ClaudeOAuth { config_dir },
        UpstreamTarget::ClaudePool { config_dirs } => Upstream::ClaudePool { config_dirs },
        UpstreamTarget::GeminiCodeAssist { creds_file } => Upstream::GeminiCodeAssist { creds_file },
    }
}

/// Local stand-in for `resolve_upstream`: a provider id, "pool", "codex:<p>" or "claude:<p>".
pub fn map_upstream(upstream: &str, st: &State) -> Result<Upstream, String> {
    let profiles: &[Profile] = st.profiles.ready().map(Vec::as_slice).unwrap_or(&[]);
    if upstream == "pool" {
        let config_dirs: Vec<PathBuf> = profiles.iter().filter(|p| p.kind == ProfileKind::ClaudeAccount && p.authenticated).map(|p| p.dir.clone()).collect();
        if config_dirs.is_empty() {
            return Err("the Claude pool has no logged-in accounts (add one in profiles, alt+o)".into());
        }
        return Ok(Upstream::ClaudePool { config_dirs });
    }
    if upstream.starts_with("codex:") || upstream.starts_with("claude:") {
        let p = profiles.iter().find(|p| p.id == upstream).ok_or_else(|| format!("no profile {upstream}"))?;
        return Ok(if p.is_codex() { Upstream::ChatGptCodex { codex_home: p.dir.clone() } } else { Upstream::ClaudeOAuth { config_dir: p.dir.clone() } });
    }
    let providers = st.providers.ready().ok_or("providers aren't loaded")?;
    let prov = providers.iter().find(|p| p.id == upstream).ok_or_else(|| format!("no provider {upstream}"))?;
    if prov.is_google_login() {
        let creds_file = bro_core::gemini::creds_path();
        if !bro_core::gemini::signed_in(&creds_file) {
            return Err(bro_core::gemini::SIGN_IN_HINT.into());
        }
        return Ok(Upstream::GeminiCodeAssist { creds_file });
    }
    let key = st.config.as_ref().and_then(|c| guard("config::key_for", || c.key_for(&prov.id, prov.key_env.as_deref())).ok().flatten());
    if key.is_none() && !prov.no_key {
        return Err(format!("no API key for {} — add one to ~/.bro/config.json", prov.name));
    }
    Ok(match prov.mode {
        ProviderMode::Openai => match (&prov.responses_base_url, &prov.base_url) {
            (Some(r), _) => Upstream::OpenAiResponses { base_url: r.clone(), api_key: key },
            (None, Some(b)) => Upstream::OpenAiChat { base_url: b.clone(), api_key: key },
            (None, None) => return Err(format!("{} has no base URL", prov.name)),
        },
        ProviderMode::Anthropic => Upstream::Anthropic { base_url: prov.base_url.clone().unwrap_or_else(|| "https://api.anthropic.com".into()), api_key: key, bearer: true },
        ProviderMode::Native => Upstream::Anthropic { base_url: prov.base_url.clone().unwrap_or_else(|| "https://api.anthropic.com".into()), api_key: key, bearer: false },
        // signed-in Google logins returned above
        ProviderMode::Antigravity => return Err(bro_core::gemini::SIGN_IN_HINT.into()),
    })
}

/// The login dir for a profile id when the profile list isn't available.
fn guess_profile_dir(id: &str) -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    let (fam, name) = id.split_once(':')?;
    Some(match (fam, name) {
        ("claude", "local") => home.join(".claude"),
        ("codex", "local") => home.join(".codex"),
        ("claude", n) => std::env::var_os("CLAUDE_POOL_DIR").map(PathBuf::from).unwrap_or_else(|| home.join(".claude-max-pool")).join("accounts").join(n),
        ("codex", n) => std::env::var_os("BRO_CODEX_PROFILES_DIR").map(PathBuf::from).unwrap_or_else(|| crate::util::bro_dir().join("codex-profiles")).join(n),
        _ => return None,
    })
}

/// The built-in launcher: native backends only (no providers, no proxy).
pub fn fallback_command(spec: &LaunchSpec, st: &State) -> Result<CommandSpec, String> {
    if spec.profile_id.as_deref().is_some_and(|p| bro_core::profiles::is_gemini_id(&p)) {
        return Err("Gemini launches need the translating proxy. Try again after it starts.".into());
    }
    if spec.provider_id.is_some() {
        return Err("provider and pool launches need bro-core's launcher, which isn't available yet".into());
    }
    let h = spec.harness;
    let mut args: Vec<String> = vec![];
    let mut env: Vec<(String, String)> = vec![];
    if let Some(id) = spec.profile_id.as_deref().filter(|id| !id.ends_with(":local")) {
        let dir = st.profiles.ready().and_then(|ps| ps.iter().find(|p| p.id == id).map(|p| p.dir.clone())).or_else(|| guess_profile_dir(id)).ok_or_else(|| format!("unknown profile {id}"))?;
        let var = if id.starts_with("codex:") { "CODEX_HOME" } else { "CLAUDE_CONFIG_DIR" };
        env.push((var.into(), dir.to_string_lossy().to_string()));
    }
    if let Some(r) = &spec.resume {
        match h {
            Harness::Claude => {
                args.extend(["--resume".into(), r.session_id.clone()]);
                if r.fork {
                    args.push("--fork-session".into());
                }
            }
            Harness::Codex => args.extend(["resume".into(), r.session_id.clone()]),
            Harness::Pi | Harness::Omp => args.extend(["--resume".into(), r.session_id.clone()]),
        }
    }
    if let Some(m) = &spec.model {
        args.extend(["--model".into(), m.clone()]);
    }
    match (spec.permission, h) {
        (Permission::Auto, Harness::Claude) => args.extend(["--permission-mode".into(), "auto".into()]),
        (Permission::Skip, Harness::Claude) => args.push("--dangerously-skip-permissions".into()),
        (Permission::Skip, Harness::Codex) => args.push("--yolo".into()),
        (Permission::Skip, Harness::Omp) => args.push("--yolo".into()),
        _ => {}
    }
    args.extend(spec.extra_args.iter().cloned());
    Ok(CommandSpec { program: h.label().into(), args, env, env_remove: vec![], cwd: spec.cwd.clone(), label: label_for(spec), route: None, cleanup: vec![], caveman: None })
}

#[cfg(test)]
mod tests {
    use super::*;
    use bro_core::browser::BrowserMode;
    use bro_core::launch::Resume;

    #[test]
    fn reload_uses_the_bound_conversation_and_only_stages_for_a_different_store() {
        crate::testkit::isolate();
        let source = format!("reload-{}", uuid::Uuid::new_v4());
        let profile = format!("claude:{source}");
        let root = bro_core::profiles::dir_for(ProfileKind::ClaudeAccount, &source).join("projects").join("F--reload");
        std::fs::create_dir_all(&root).unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let other = uuid::Uuid::new_v4().to_string();
        for session in [&id, &other] {
            let line = serde_json::json!({"type": "user", "sessionId": session, "cwd": "F:\\reload", "message": {"content": "keep my conversation"}});
            std::fs::write(root.join(format!("{session}.jsonl")), format!("{line}\n")).unwrap();
        }
        let find = || FindLive { harness: Harness::Claude, store: Some(profile.clone()), cwd: "F:\\reload".into(), since: std::time::SystemTime::now() + std::time::Duration::from_secs(120), session_id: Some(id.clone()) };
        let mut launch = spec(Harness::Claude);
        launch.profile_id = Some(profile.clone());
        assert!(prepare_live_resume(&mut launch, find()).unwrap().is_none());
        assert_eq!(launch.resume.as_ref().unwrap().session_id, id);
        assert!(!launch.resume.as_ref().unwrap().fork, "same profile resumes the existing conversation");
        launch.profile_id = Some("claude:another".into());
        assert_eq!(prepare_live_resume(&mut launch, find()).unwrap().unwrap().id, id);
        assert!(launch.resume.as_ref().unwrap().fork, "a different profile gets a copy");
        let mut missing = find();
        missing.session_id = Some(uuid::Uuid::new_v4().to_string());
        assert!(prepare_live_resume(&mut launch, missing).is_err(), "never substitute the other session in the folder");
        launch.profile_id = Some("codex:team".into());
        launch.provider_id = Some("openrouter".into());
        let mut local = spec(Harness::Claude);
        local.profile_id = None;
        local.provider_id = Some("openrouter".into());
        assert_eq!(transcript_store(&local).as_deref(), Some("claude:local"));
        assert_eq!(transcript_store(&launch).as_deref(), Some("claude:local"));
        for harness in [Harness::Pi, Harness::Omp] {
            launch.harness = harness;
            assert_eq!(transcript_store(&launch), None, "Pi and omp keep transcripts outside login profiles");
        }
    }

    #[test]
    fn guest_launch_starts_a_ready_proxy_and_retries_the_command() {
        crate::testkit::isolate();
        let mut settings = super::super::fallback_settings();
        settings.proxy.enabled = false;
        let (tx, _rx) = std::sync::mpsc::channel();
        let svc = Services::with_state(Services::empty_state(settings), tx, false, true);
        svc.update(|st| st.proxy.status = super::super::Avail::Unavailable("your other bro runs it".into()));
        let mut ctx = LaunchCtx::default();
        let mut attempts = 0;
        let result = with_proxy_retry(&svc, &mut ctx, |ctx| {
            attempts += 1;
            let Some(base) = &ctx.proxy_base else {
                return Err(bro_core::launch::ProxyRequired("gemini-sub:local".into()).into());
            };
            let token = ctx.proxy_token.as_deref().unwrap();
            let response = bro_core::http::get_json(&format!("{base}/health"), &[("x-api-key", token)], std::time::Duration::from_secs(2))?;
            anyhow::ensure!(response.ok() && response.body["ok"] == true, "proxy must listen before retry");
            Ok(base.clone())
        }).unwrap();
        assert_eq!(attempts, 2);
        assert_eq!(ctx.proxy_base.as_deref(), Some(result.as_str()));
        assert!(!svc.settings().proxy.enabled, "an explicit launch must not change saved proxy preferences");
        assert!(svc.inner.bridge.read().is_none(), "the original window keeps the phone bridge");
        svc.shutdown();
    }

    #[test]
    fn native_launches_and_validation_errors_do_not_start_a_proxy() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let svc = Services::with_state(Services::empty_state(super::super::fallback_settings()), tx, false, true);
        let mut ctx = LaunchCtx::default();
        assert_eq!(with_proxy_retry(&svc, &mut ctx, |_| Ok(7)).unwrap(), 7);
        let error = with_proxy_retry::<()>(&svc, &mut ctx, |_| anyhow::bail!("unknown profile")).unwrap_err();
        assert_eq!(error.to_string(), "unknown profile");
        assert!(svc.inner.proxy.lock().is_none());
    }

    #[test]
    fn concurrent_guest_launches_share_one_proxy_and_leave_the_original_running() {
        crate::testkit::isolate();
        let original = bro_proxy::ProxyHandle::start(bro_proxy::ProxyConfig { bind: "127.0.0.1".into(), port: 0, token: None }).unwrap();
        let mut settings = super::super::fallback_settings();
        settings.proxy.port = original.port();
        let (tx, _rx) = std::sync::mpsc::channel();
        let svc = Services::with_state(Services::empty_state(settings), tx, false, true);
        let ports = std::thread::scope(|scope| {
            let jobs: Vec<_> = (0..8).map(|_| scope.spawn(|| svc.ensure_proxy().unwrap().port)).collect();
            jobs.into_iter().map(|job| job.join().unwrap()).collect::<Vec<_>>()
        });
        assert!(ports.iter().all(|port| *port == ports[0]));
        assert_ne!(ports[0], original.port());
        let health = bro_core::http::get_json(&format!("{}/health", original.base_url()), &[], std::time::Duration::from_secs(2)).unwrap();
        assert!(health.ok(), "starting the guest must preserve the existing window's proxy");
        svc.shutdown();
        original.shutdown();
    }

    fn spec(h: Harness) -> LaunchSpec {
        LaunchSpec { harness: h, profile_id: Some("claude:work".into()), provider_id: None, model: Some("claude-opus-5".into()), cwd: PathBuf::from("."), resume: None, permission: Permission::Skip, browser: BrowserMode::Off, extra_args: vec![], orchestrator: false }
    }

    #[test]
    fn fallback_and_upstreams() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let svc = Services::offline(super::super::fallback_settings(), tx);
        let st = svc.state();
        let mut s = spec(Harness::Claude);
        s.resume = Some(Resume { session_id: "abc".into(), fork: true });
        let c = fallback_command(&s, &st).unwrap();
        assert_eq!(c.program, "claude");
        assert!(c.args.contains(&"--dangerously-skip-permissions".to_string()));
        assert!(c.args.windows(2).any(|w| w == ["--resume", "abc"]));
        assert!(c.env.iter().any(|(k, _)| k == "CLAUDE_CONFIG_DIR"));
        assert_eq!(c.label, "claude · work · opus-5");
        s.provider_id = Some("openrouter".into());
        assert!(fallback_command(&s, &st).is_err());
        assert!(matches!(map_upstream("pool", &st), Ok(Upstream::ClaudePool { .. })));
        assert!(matches!(map_upstream("codex:local", &st), Ok(Upstream::ChatGptCodex { .. })));
        assert!(matches!(map_upstream("claude:work", &st), Ok(Upstream::ClaudeOAuth { .. })));
        assert!(map_upstream("nope", &st).is_err());
    }
}
