//! Services: the only place bro-tui calls into bro-core, bro-proxy and bro-bridge.
//!
//! Every call runs on a background thread (or is known-cheap) and is wrapped in [`guard`], so an unimplemented
//! or panicking dependency degrades to an "unavailable" state. Results are published into [`State`] behind a
//! lock and the UI is woken with `Event::Services`; the UI thread only ever reads snapshots.

pub mod demo;
pub mod guard;
pub mod launch;
mod workers;

pub use guard::{guard, guard_res};
pub use launch::{LaunchRequest, Launched};

use crate::pane::Event;
use bro_bridge::{Bridge, BridgeStatus, SessionMeta};
use bro_core::config::{BridgeSettings, Config, ProxySettings, Settings};
use bro_core::profiles::Profile;
use bro_core::projects::ProjectKey;
use bro_core::providers::Provider;
use bro_core::sessions::SessionInfo;
use bro_core::usage::{Headroom, Usage};
use bro_proxy::{ProxyEvent, ProxyHandle, Route};
use parking_lot::{Mutex, RwLock, RwLockReadGuard};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::Sender;
use std::time::Instant;

/// A value that is loading, ready, or unavailable (with the reason).
#[derive(Clone, Debug)]
pub enum Avail<T> {
    Loading,
    Ready(T),
    Unavailable(String),
}

impl<T> Avail<T> {
    pub fn ready(&self) -> Option<&T> {
        match self {
            Avail::Ready(v) => Some(v),
            _ => None,
        }
    }
    /// "loading…" / the reason, for placeholders.
    pub fn why(&self) -> String {
        match self {
            Avail::Loading => "loading…".into(),
            Avail::Ready(_) => String::new(),
            Avail::Unavailable(e) => format!("unavailable — {e}"),
        }
    }
}

/// One profile's meters.
#[derive(Clone, Debug, Default)]
pub struct UsageEntry {
    pub usage: Option<Usage>,
    pub headroom: Option<Headroom>,
    pub error: Option<String>,
    pub fetching: bool,
}

/// The running proxy as the UI sees it.
#[derive(Clone, Debug)]
pub struct ProxyInfo {
    pub port: u16,
    pub base: String,
    pub token: String,
}

#[derive(Clone, Debug)]
pub struct ProxyState {
    pub status: Avail<ProxyInfo>,
    pub routes: Vec<Route>,
    /// newest last, capped
    pub events: VecDeque<ProxyEvent>,
    /// Claude pool accounts as the proxy sees them (availability, cooldowns, 5h window counts)
    pub pool: Vec<bro_proxy::PoolAccountStatus>,
    pub enabled: bool,
}

#[derive(Clone, Debug)]
pub struct BridgeState {
    pub status: Avail<BridgeStatus>,
    pub qr: Option<Vec<String>>,
    pub enabled: bool,
}

/// Everything the UI reads.
pub struct State {
    pub settings: Settings,
    pub config: Option<Config>,
    pub profiles: Avail<Vec<Profile>>,
    pub providers: Avail<Vec<Provider>>,
    /// pickable models per provider id, plus "codex" (ChatGPT) and "claude" (Anthropic) for cross-login
    /// launches; OpenRouter is the live catalogue
    pub models: BTreeMap<String, Vec<bro_core::catalogue::ModelRow>>,
    pub past: Avail<Vec<SessionInfo>>,
    pub usage: BTreeMap<String, UsageEntry>,
    pub usage_at: Option<Instant>,
    /// (large-task pick, small-task pick) among Claude profiles
    pub picks: (Option<String>, Option<String>),
    pub proxy: ProxyState,
    pub bridge: BridgeState,
    /// Things that didn't work at startup, shown once as toasts and in the views.
    pub issues: Vec<String>,
}

/// Max proxy events kept for the live log.
pub const EVENT_CAP: usize = 400;

/// Cheap-to-clone handle to all background services.
#[derive(Clone)]
pub struct Services {
    inner: Arc<Inner>,
}

struct Inner {
    state: RwLock<State>,
    tx: Mutex<Sender<Event>>,
    demo: bool,
    proxy: Mutex<Option<ProxyHandle>>,
    bridge: RwLock<Option<Bridge>>,
    bridge_fuse: guard::Fuse,
    usage_kick: Mutex<Option<Sender<()>>>,
    projects: Mutex<HashMap<PathBuf, ProjectKey>>,
}

/// v2.toml settings with every default spelled out (used when `Settings::load` isn't available).
pub fn fallback_settings() -> Settings {
    Settings {
        theme: crate::theme::DEFAULT.into(),
        prefix: "ctrl+space".into(),
        shell: None,
        nerd_font: true,
        usage_expanded: false,
        bridge: BridgeSettings::default(),
        proxy: ProxySettings::default(),
        keys: BTreeMap::new(),
    }
}

/// Load settings (guarded). Returns the settings and a problem to report, if any.
pub fn load_settings() -> (Settings, Option<String>) {
    match guard("config::Settings::load", Settings::load) {
        Ok(mut s) => {
            if s.theme.trim().is_empty() {
                s.theme = crate::theme::DEFAULT.into();
            }
            if s.prefix.trim().is_empty() {
                s.prefix = "ctrl+space".into();
            }
            (s, None)
        }
        Err(e) => (fallback_settings(), Some(e)),
    }
}

impl Services {
    fn with_state(state: State, tx: Sender<Event>, demo: bool) -> Services {
        Services {
            inner: Arc::new(Inner {
                state: RwLock::new(state),
                tx: Mutex::new(tx),
                demo,
                proxy: Mutex::new(None),
                bridge: RwLock::new(None),
                bridge_fuse: guard::Fuse::default(),
                usage_kick: Mutex::new(None),
                projects: Mutex::new(HashMap::new()),
            }),
        }
    }

    fn empty_state(settings: Settings) -> State {
        State {
            proxy: ProxyState { status: Avail::Loading, routes: vec![], events: VecDeque::new(), pool: vec![], enabled: settings.proxy.enabled },
            bridge: BridgeState { status: Avail::Loading, qr: None, enabled: settings.bridge.enabled },
            settings,
            config: None,
            profiles: Avail::Loading,
            providers: Avail::Loading,
            models: BTreeMap::new(),
            past: Avail::Loading,
            usage: BTreeMap::new(),
            usage_at: None,
            picks: (None, None),
            issues: vec![],
        }
    }

    /// Real services: loads data, starts the proxy and bridge (when enabled) and the usage refresher, all on
    /// background threads. Returns immediately.
    pub fn start(settings: Settings, tx: Sender<Event>) -> Services {
        let svc = Services::with_state(Services::empty_state(settings), tx, false);
        workers::spawn_all(&svc);
        svc
    }

    /// `bro --demo`: realistic fake data, a ticking fake proxy log, no real network or accounts.
    pub fn demo(settings: Settings, tx: Sender<Event>) -> Services {
        let svc = Services::offline(settings, tx);
        demo::spawn_ticker(&svc);
        svc
    }

    /// Demo data with no background threads at all (tests and snapshots: deterministic).
    pub fn offline(settings: Settings, tx: Sender<Event>) -> Services {
        let mut st = Services::empty_state(settings);
        demo::fill(&mut st);
        Services::with_state(st, tx, true)
    }

    pub fn is_demo(&self) -> bool {
        self.inner.demo
    }

    /// Read the shared state (keep the guard short-lived).
    pub fn state(&self) -> RwLockReadGuard<'_, State> {
        self.inner.state.read()
    }

    pub(crate) fn update(&self, f: impl FnOnce(&mut State)) {
        f(&mut self.inner.state.write());
        self.wake();
    }

    /// Tell the UI something changed.
    pub fn wake(&self) {
        let _ = self.inner.tx.lock().send(Event::Services);
    }

    pub(crate) fn send(&self, ev: Event) {
        let _ = self.inner.tx.lock().send(ev);
    }

    pub fn settings(&self) -> Settings {
        self.state().settings.clone()
    }

    /// Persist settings on a background thread (no-op in demo mode).
    pub fn save_settings(&self, s: Settings) {
        self.update(|st| st.settings = s.clone());
        if self.is_demo() {
            return;
        }
        let svc = self.clone();
        std::thread::spawn(move || {
            if let Err(e) = guard_res("config::Settings::save", || s.save()) {
                svc.send(Event::Toast(crate::alerts::Kind::Error, format!("couldn't save settings: {e}")));
            }
        });
    }

    /// Project identity for a directory: cached; bro-core's `project_for` when available, else a local git-root
    /// walk (demo mode always uses the local one: demo paths don't exist).
    pub fn project_for(&self, cwd: &Path) -> ProjectKey {
        if let Some(k) = self.inner.projects.lock().get(cwd) {
            return k.clone();
        }
        let k = if self.is_demo() { None } else { guard("projects::project_for", || bro_core::projects::project_for(cwd)).ok() };
        let k = k.unwrap_or_else(|| local_project_for(cwd));
        self.inner.projects.lock().insert(cwd.to_path_buf(), k.clone());
        k
    }

    /// Re-read profiles, providers and past sessions.
    pub fn refresh_data(&self) {
        if self.is_demo() {
            return;
        }
        workers::spawn_data(self);
    }

    /// Fetch usage now (the background refresher also runs every 60 s).
    pub fn refresh_usage(&self) {
        if self.is_demo() {
            return;
        }
        if let Some(k) = self.inner.usage_kick.lock().as_ref() {
            let _ = k.send(());
        }
    }

    /// Build + spawn a session (see [`launch`]).
    pub fn launch(&self, req: LaunchRequest) {
        let svc = self.clone();
        std::thread::spawn(move || launch::run(&svc, req));
    }

    /// The interactive login command for a profile (guarded; cheap).
    pub fn login_command(&self, p: &Profile) -> Result<bro_core::launch::CommandSpec, String> {
        if self.is_demo() {
            return Ok(demo::shell_command(None, &format!("login · {}", p.id), std::env::temp_dir()));
        }
        guard("profiles::login_command", || bro_core::profiles::login_command(p))
    }

    /// Create a profile on a background thread; `then` gets the result on that thread.
    pub fn create_profile(&self, kind: bro_core::profiles::ProfileKind, name: String) {
        let svc = self.clone();
        std::thread::spawn(move || {
            let r = if svc.is_demo() { Err("demo mode: profiles are read-only".to_string()) } else { guard_res("profiles::create", || bro_core::profiles::create(kind, &name)) };
            let (kind, text) = match &r {
                Ok(p) => (crate::alerts::Kind::Info, format!("created {} — log in with l", p.id)),
                Err(e) => (crate::alerts::Kind::Error, e.clone()),
            };
            svc.send(Event::Toast(kind, text));
            workers::load_data(&svc);
        });
    }

    /// Remove a profile (its login dir) on a background thread.
    pub fn remove_profile(&self, id: String) {
        let svc = self.clone();
        std::thread::spawn(move || {
            let r = if svc.is_demo() { Err("demo mode: profiles are read-only".to_string()) } else { guard_res("profiles::remove", || bro_core::profiles::remove(&id)) };
            let (kind, text) = match r {
                Ok(()) => (crate::alerts::Kind::Info, format!("removed {id}")),
                Err(e) => (crate::alerts::Kind::Error, e),
            };
            svc.send(Event::Toast(kind, text));
            workers::load_data(&svc);
        });
    }

    // ------------------------------------------------------------------ proxy

    pub(crate) fn set_proxy(&self, h: Option<ProxyHandle>) {
        *self.inner.proxy.lock() = h;
    }

    /// Run `f` against the live proxy (guarded). None if it isn't running or panicked.
    pub(crate) fn with_proxy<T>(&self, what: &str, f: impl FnOnce(&ProxyHandle) -> T) -> Option<T> {
        let h = self.inner.proxy.lock().clone()?;
        guard(what, || f(&h)).ok()
    }

    /// Drop a route when its session ends.
    pub fn remove_route(&self, id: &str) {
        let id = id.to_string();
        let svc = self.clone();
        std::thread::spawn(move || {
            svc.with_proxy("proxy::remove_route", |p| p.remove_route(&id));
            let routes = svc.with_proxy("proxy::routes", |p| p.routes());
            svc.update(|st| match routes {
                Some(r) => st.proxy.routes = r,
                None => st.proxy.routes.retain(|r| r.id != id),
            });
        });
    }

    // ------------------------------------------------------------------ bridge

    pub(crate) fn set_bridge(&self, b: Option<Bridge>) {
        self.inner.bridge_fuse.reset();
        *self.inner.bridge.write() = b;
    }

    fn with_bridge(&self, what: &str, f: impl FnOnce(&Bridge)) {
        let g = self.inner.bridge.read();
        if let Some(b) = g.as_ref() {
            self.inner.bridge_fuse.run(what, || f(b));
        }
    }

    pub fn bridge_running(&self) -> bool {
        self.inner.bridge.read().is_some() && !self.inner.bridge_fuse.blown()
    }

    pub fn bridge_register(&self, meta: SessionMeta) {
        self.with_bridge("bridge::register", |b| b.register(meta));
    }
    /// Raw PTY output (called from PTY reader threads; cheap when the bridge is off).
    pub fn bridge_output(&self, id: &str, data: &[u8]) {
        self.with_bridge("bridge::output", |b| b.output(id, data));
    }
    pub fn bridge_title(&self, id: &str, title: &str) {
        self.with_bridge("bridge::title", |b| b.title(id, title));
    }
    pub fn bridge_resize(&self, id: &str, cols: u16, rows: u16) {
        self.with_bridge("bridge::resize", |b| b.resize(id, cols, rows));
    }
    pub fn bridge_exit(&self, id: &str, code: Option<i32>) {
        self.with_bridge("bridge::exit", |b| b.exit(id, code));
    }
    pub fn bridge_unregister(&self, id: &str) {
        self.with_bridge("bridge::unregister", |b| b.unregister(id));
    }

    /// Tell the bridge what remote clients can launch: a shell, every logged-in profile, pi and omp.
    pub(crate) fn push_bridge_profiles(&self) {
        if !self.bridge_running() {
            return;
        }
        let (shell, shell_args) = crate::util::default_shell(self.settings().shell.as_deref());
        let tp = |id: &str, label: String, shell: &str, args: Vec<String>, group: &str, description: Option<String>, agent: Option<&str>| bro_bridge::TerminalProfile {
            id: id.into(),
            label,
            shell: shell.into(),
            args,
            group: group.into(),
            description,
            agent: agent.map(String::from),
            terminal_profile_guid: None,
        };
        let mut list = vec![tp("shell", "Shell".into(), &shell, shell_args, "shell", None, None)];
        if let Some(ps) = self.state().profiles.ready() {
            for p in ps.iter().filter(|p| p.authenticated) {
                let h = if p.is_codex() { "codex" } else { "claude" };
                let desc = [p.plan.clone(), p.email.clone()].into_iter().flatten().collect::<Vec<_>>().join(" · ");
                list.push(tp(&p.id, format!("{} · {}", if p.is_codex() { "Codex" } else { "Claude" }, p.name), h, vec![], "agent", (!desc.is_empty()).then_some(desc), Some(h)));
            }
        }
        list.push(tp("pi", "Pi".into(), "pi", vec![], "agent", Some("pi coding agent".into()), None));
        list.push(tp("omp", "omp".into(), "omp", vec![], "agent", Some("oh-my-pi".into()), None));
        self.with_bridge("bridge::set_profiles", |b| b.set_profiles(list));
    }

    /// Turn the bridge on or off (saved to settings).
    pub fn set_bridge_enabled(&self, on: bool) {
        let mut s = self.settings();
        s.bridge.enabled = on;
        self.save_settings(s);
        self.update(|st| st.bridge.enabled = on);
        if self.is_demo() {
            return;
        }
        if on {
            workers::spawn_bridge(self);
        } else {
            let b = self.inner.bridge.write().take();
            if let Some(b) = b {
                let _ = guard("bridge::shutdown", || b.shutdown());
            }
            self.update(|st| {
                st.bridge.status = Avail::Unavailable("turned off".into());
                st.bridge.qr = None;
            });
        }
    }

    /// Stop the proxy and bridge (on quit).
    pub fn shutdown(&self) {
        if let Some(p) = self.inner.proxy.lock().take() {
            let _ = guard("proxy::shutdown", || p.shutdown());
        }
        if let Some(b) = self.inner.bridge.write().take() {
            let _ = guard("bridge::shutdown", || b.shutdown());
        }
    }

    pub(crate) fn set_usage_kick(&self, k: Sender<()>) {
        *self.inner.usage_kick.lock() = Some(k);
    }
}

/// Local stand-in for `bro_core::projects::project_for`: walk up for `.git`, else the directory itself.
pub fn local_project_for(cwd: &Path) -> ProjectKey {
    let mut root = cwd.to_path_buf();
    let mut p = Some(cwd);
    while let Some(d) = p {
        if d.join(".git").exists() {
            root = d.to_path_buf();
            break;
        }
        p = d.parent();
    }
    let s = root.to_string_lossy().trim_end_matches(['/', '\\']).to_string();
    let root = if s.is_empty() { root } else { PathBuf::from(&s) };
    let key = if cfg!(windows) { s.to_lowercase().replace('\\', "/") } else { s.clone() };
    let name = root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| s.clone());
    ProjectKey { root, key, name }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_projects() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(repo.join("src/deep")).unwrap();
        let k = local_project_for(&repo.join("src/deep"));
        assert_eq!(k.name, "repo");
        assert_eq!(k.root, repo);
        let other = local_project_for(dir.path());
        assert_ne!(other.key, k.key);
    }

    /// Real services against the real crates (isolated BRO_DIR, loopback, ephemeral ports). Touches the
    /// network for usage, so opt-in: `cargo test -p bro-tui real_services -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn real_services_start_without_panics() {
        crate::testkit::isolate();
        let dir = crate::util::bro_dir();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("v2.toml"), "[bridge]
port = 0
bind = \"127.0.0.1\"
[proxy]
port = 0
").unwrap();
        let (settings, problem) = load_settings();
        assert!(problem.is_none(), "{problem:?}");
        let (tx, rx) = std::sync::mpsc::channel();
        let svc = Services::start(settings, tx);
        let t0 = Instant::now();
        loop {
            while rx.try_recv().is_ok() {}
            let st = svc.state();
            let settled = !matches!(st.profiles, Avail::Loading) && !matches!(st.past, Avail::Loading) && !matches!(st.proxy.status, Avail::Loading) && !matches!(st.bridge.status, Avail::Loading);
            if settled && (st.usage_at.is_some() || t0.elapsed().as_secs() > 25) || t0.elapsed().as_secs() > 40 {
                break;
            }
            drop(st);
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        let st = svc.state();
        println!("profiles: {:?}", st.profiles.ready().map(|p| p.iter().map(|p| (p.id.clone(), p.authenticated)).collect::<Vec<_>>()).ok_or(st.profiles.why()));
        println!("providers: {:?}", st.providers.ready().map(|p| p.len()).ok_or(st.providers.why()));
        println!("past sessions: {:?}", st.past.ready().map(|p| p.len()).ok_or(st.past.why()));
        println!("usage: {:?}", st.usage.iter().map(|(k, e)| (k.clone(), e.usage.as_ref().and_then(|u| u.five_hour.as_ref().map(|w| w.used_pct)), e.error.clone())).collect::<Vec<_>>());
        println!("proxy: {:?}", st.proxy.status.ready().map(|p| p.port).ok_or(st.proxy.status.why()));
        println!("bridge: {:?}", st.bridge.status.ready().map(|b| (b.port, b.running, b.urls.len())).ok_or(st.bridge.status.why()));
        println!("qr rows: {:?}", st.bridge.qr.as_ref().map(|q| q.len()));
        println!("issues: {:?}", st.issues);
        let all = format!("{:?} {} {} {} {}", st.issues, st.profiles.why(), st.past.why(), st.proxy.status.why(), st.bridge.status.why());
        assert!(!all.contains("not implemented"), "{all}");
        assert!(st.proxy.status.ready().is_some() && st.bridge.status.ready().is_some());
        drop(st);
        svc.shutdown();
    }

    #[test]
    fn offline_services_have_demo_data() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let svc = Services::offline(fallback_settings(), tx);
        let st = svc.state();
        assert!(st.profiles.ready().is_some_and(|p| p.len() >= 4));
        assert!(st.past.ready().is_some_and(|p| !p.is_empty()));
        assert!(st.proxy.status.ready().is_some());
        assert!(!st.usage.is_empty());
    }
}
