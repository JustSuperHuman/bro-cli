//! Services: the only place bro-tui calls into bro-core, bro-proxy and bro-bridge.
//!
//! Every call runs on a background thread (or is known-cheap) and is wrapped in [`guard`], so an unimplemented
//! or panicking dependency degrades to an "unavailable" state. Results are published into [`State`] behind a
//! lock and the UI is woken with `Event::Services`; the UI thread only ever reads snapshots.

pub mod demo;
pub mod guard;
pub mod justimagine;
pub mod justpaste;
pub mod justtools;
pub mod justwatch;
pub mod skills;
pub mod updates;
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
use std::sync::atomic::{AtomicBool, Ordering};
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
    /// The projects as the phone / web arrange them (by bro project key): the project tabs follow their
    /// order and names.
    pub projects: Vec<ClientProject>,
}

/// A project as the remote clients arrange it.
#[derive(Clone, Debug, PartialEq)]
pub struct ClientProject {
    /// bro's project key
    pub key: String,
    /// the name a client gave it
    pub name: Option<String>,
    /// its place in a client's order
    pub order: Option<usize>,
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
    pub justtools: justtools::JustToolsState,
    pub justimagine: justimagine::JustImagineState,
    /// The watched skill repos.
    pub skills: skills::SkillsState,
    pub updates: updates::UpdateState,
    /// Every session's watchdog as the bridge last reported it, by session id.
    pub watchdogs: HashMap<String, bro_bridge::WatchdogSnapshot>,
    pub voice: VoiceState,
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
    guest: bool,
    proxy_starting: AtomicBool,
    proxy: Mutex<Option<ProxyHandle>>,
    bridge: RwLock<Option<Bridge>>,
    bridge_fuse: guard::Fuse,
    usage_kick: Mutex<Option<Sender<()>>>,
    projects: Mutex<HashMap<PathBuf, ProjectKey>>,
    /// Push-to-talk listener; dropping it unhooks the hotkey.
    voice: Mutex<Option<bro_voice::Voice>>,
    /// Bumped on every (re)start, so a slow start that lost the race stops itself.
    voice_gen: std::sync::atomic::AtomicU64,
    /// Bridge id of the session in the focused pane (kept current by the UI loop).
    focused: Mutex<Option<String>>,
    /// Profiles with a login tab open that a watcher is polling until they read signed in.
    login_watch: Mutex<HashMap<String, Instant>>,
    /// Agent teams running in sessions here (kept current by the UI loop), for the settings view.
    teams: Mutex<Vec<LiveTeam>>,
}

/// An agent team running in one of bro's Claude sessions.
#[derive(Clone, Debug)]
pub struct LiveTeam {
    /// the lead session's name
    pub lead: String,
    pub project: String,
    /// launch order (the sidebar's)
    pub seq: u64,
    pub team: Arc<bro_core::agent_teams::Team>,
}

/// Push-to-talk as it stands, for the settings view.
#[derive(Clone, Debug, Default)]
pub struct VoiceState {
    /// the hotkey name it listens on, while it does
    pub running: Option<String>,
    /// an OpenAI key was found in the configured variable (None: not looked yet)
    pub key_found: Option<bool>,
    /// why it didn't start
    pub error: Option<String>,
    /// waiting for the key to listen on (see [`Services::capture_voice_hotkey`])
    pub capturing: Option<Instant>,
}

/// How long "press the key you want" waits.
const CAPTURE_FOR: std::time::Duration = std::time::Duration::from_secs(15);

/// Words spoken with push-to-talk, on their way to the orchestrator session.
#[derive(Clone, Debug)]
pub struct VoiceMsg {
    pub text: String,
    /// bridge id of the session in focus when the key went down
    pub focused: Option<String>,
    /// the live voice's delegation id: the orchestrator's answer goes back to be spoken
    pub tag: Option<String>,
}

/// GPT-Live's standing instructions: it is Hugh's voice; Hugh (Claude, through the bridge) does the work.
pub(crate) const LIVE_INSTRUCTIONS: &str = "You are Hugh, the voice of bro, the user's terminal workspace. Coding agents \
(Claude Code, Codex) run there in sessions named Mal, Pinky, Toast, Minty, Lilac, Sky and Bitty, in projects such as justgains. \
You cannot see or do anything in the workspace yourself: delegate every request about sessions, projects, code, chats or \
anything the user wants done, including words the user dictates for the agent they are looking at. Delegate right away \
without asking first. Be extremely brief. While a delegation runs, say one word at most (\"Sure.\") or nothing, then wait \
quietly. When the result arrives, say it in as few words as it takes, usually under eight: no preamble, no restating the \
request, no offers or follow-up questions, never add anything the result doesn't say. Small talk: a few words. \
The user speaks while holding a key; when they let go, they are done talking.";

/// Longest reply bro speaks or toasts; Hugh's full answer stays in the orchestrator transcript.
const BRIEF_CHARS: usize = 120;

/// Hugh's reply cut to what is worth hearing: markdown dropped, the first sentence, no trailing offer
/// ("Want me to...?"), at most [`BRIEF_CHARS`] ending on a word.
pub(crate) fn brief(text: &str) -> String {
    let plain: String = text.replace(['*', '`', '#'], "").split_whitespace().collect::<Vec<_>>().join(" ");
    let mut sentences = vec![];
    let mut rest = plain.as_str();
    while !rest.is_empty() {
        let end = rest.char_indices().find(|&(i, c)| matches!(c, '.' | '!' | '?') && rest[i + c.len_utf8()..].starts_with(' ')).map(|(i, c)| i + c.len_utf8());
        let (sentence, tail) = rest.split_at(end.unwrap_or(rest.len()));
        sentences.push(sentence.trim());
        rest = tail.trim_start();
    }
    let offer = |s: &str| {
        let s = s.to_lowercase();
        ["want me", "should i", "shall i", "would you like", "do you want", "let me know", "i can also", "anything else"].iter().any(|o| s.starts_with(o))
    };
    let first = sentences.iter().find(|s| !s.is_empty() && !offer(s)).or(sentences.first()).copied().unwrap_or("");
    if first.chars().count() <= BRIEF_CHARS {
        return first.to_string();
    }
    let cut: String = first.chars().take(BRIEF_CHARS).collect();
    let cut = cut.rsplit_once(' ').map(|(head, _)| head).unwrap_or(&cut);
    format!("{}…", cut.trim_end_matches([',', ';', ':', ' ']))
}

/// Names speech recognition should spell exactly: the orchestrator and the squad that names its sessions.
pub(crate) fn voice_keywords() -> Vec<String> {
    ["Hugh", "Mal", "Pinky", "Toast", "Minty", "Lilac", "Sky", "Bitty", "bro"].iter().map(|w| w.to_string()).collect()
}

/// v2.toml settings with every default spelled out (used when `Settings::load` isn't available).
pub fn fallback_settings() -> Settings {
    Settings {
        theme: crate::theme::DEFAULT.into(),
        prefix: "ctrl+space".into(),
        shell: None,
        nerd_font: true,
        usage_expanded: false,
        sidebar_width: None,
        icons: "auto".into(),
        rain: Default::default(),
        font: None,
        material: Default::default(),
        material_tint: bro_core::config::MATERIAL_TINT,
        bridge: BridgeSettings::default(),
        proxy: ProxySettings::default(),
        voice: bro_core::config::VoiceSettings::default(),
        insights: bro_core::config::InsightsSettings::default(),
        auto_resume: bro_core::config::AutoResumeSettings::default(),
        auto_approve: false,
        summary_screensaver_secs: 0,
        shared_borders: false,
        project_tabs: false,
        permission: Default::default(),
        claude_env: Default::default(),
        caveman: Default::default(),
        claude_mod: Default::default(),
        peon: Default::default(),
        justtools: Default::default(),
        justwatch: Default::default(),
        skills: Default::default(),
        updates: Default::default(),
        watchdog: Default::default(),
        setup_done: false,
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
    fn with_state(state: State, tx: Sender<Event>, demo: bool, guest: bool) -> Services {
        Services {
            inner: Arc::new(Inner {
                state: RwLock::new(state),
                tx: Mutex::new(tx),
                demo,
                guest,
                proxy_starting: AtomicBool::new(false),
                proxy: Mutex::new(None),
                bridge: RwLock::new(None),
                bridge_fuse: guard::Fuse::default(),
                usage_kick: Mutex::new(None),
                projects: Mutex::new(HashMap::new()),
                voice: Mutex::new(None),
                voice_gen: Default::default(),
                focused: Mutex::new(None),
                login_watch: Mutex::new(Default::default()),
                teams: Mutex::new(vec![]),
            }),
        }
    }

    fn empty_state(settings: Settings) -> State {
        State {
            proxy: ProxyState { status: Avail::Loading, routes: vec![], events: VecDeque::new(), pool: vec![], enabled: settings.proxy.enabled },
            bridge: BridgeState { status: Avail::Loading, qr: None, enabled: settings.bridge.enabled, projects: vec![] },
            settings,
            justtools: Default::default(),
            justimagine: Default::default(),
            skills: Default::default(),
            updates: Default::default(),
            config: None,
            profiles: Avail::Loading,
            providers: Avail::Loading,
            models: BTreeMap::new(),
            past: Avail::Loading,
            usage: BTreeMap::new(),
            usage_at: None,
            picks: (None, None),
            watchdogs: HashMap::new(),
            voice: VoiceState::default(),
            issues: vec![],
        }
    }

    /// Real services: loads data, starts the proxy and bridge (when enabled) and the usage refresher, all on
    /// background threads. Returns immediately.
    pub fn start(settings: Settings, tx: Sender<Event>) -> Services {
        let svc = Services::with_state(Services::empty_state(settings), tx, false, false);
        workers::spawn_all(&svc);
        svc
    }

    /// A second bro beside a running one (the desktop app opened with `--new`): your real settings, profiles,
    /// usage and past sessions. The phone bridge, voice and update checks stay with the bro that already runs
    /// them; a launch that needs translation starts this window's own proxy on an unused loopback port.
    pub fn start_guest(settings: Settings, tx: Sender<Event>) -> Services {
        let svc = Services::with_state(Services::empty_state(settings), tx, false, true);
        workers::spawn_guest(&svc);
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
        Services::with_state(st, tx, true, false)
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

    /// Change settings for this run only, nothing written (scripted checks of the desktop app).
    pub fn set_settings_unsaved(&self, s: Settings) {
        self.update(|st| st.settings = s);
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

    /// Re-scan earlier sessions in the background (the continue picker asks for this when it opens).
    pub fn refresh_sessions(&self) {
        if self.is_demo() {
            return;
        }
        let svc = self.clone();
        std::thread::spawn(move || workers::load_past(&svc));
    }

    /// Fetch usage now (the background refresher also runs every 60 s).
    pub fn refresh_usage(&self) {
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

    /// Watch a profile while its login tab is open: once it reads signed in (or as another account), publish the
    /// profiles again so the sidebar meters, usage view, launcher and phone pick it up without a manual reload.
    /// Claude's login keeps running after you sign in, so this can't wait for the tab to end.
    pub fn watch_login(&self, id: String) {
        const EVERY: std::time::Duration = std::time::Duration::from_secs(2);
        const FOR: std::time::Duration = std::time::Duration::from_secs(20 * 60);
        if self.is_demo() {
            return;
        }
        // a fresh watch replaces one already running for this profile (each knows its own start)
        let start = Instant::now();
        self.inner.login_watch.lock().insert(id.clone(), start);
        let before = self.state().profiles.ready().and_then(|ps| ps.iter().find(|p| p.id == id).map(|p| (p.authenticated, p.email.clone())));
        let svc = self.clone();
        std::thread::spawn(move || {
            let mine = |svc: &Services| svc.inner.login_watch.lock().get(&id) == Some(&start);
            while start.elapsed() < FOR && mine(&svc) {
                std::thread::sleep(EVERY);
                let Ok(Some(p)) = guard("profiles::get", || bro_core::profiles::get(&id)) else { break };
                if p.authenticated && before.as_ref() != Some(&(true, p.email.clone())) {
                    workers::load_profiles(&svc);
                    let who = p.email.as_deref().map(|e| format!(" as {e}")).unwrap_or_default();
                    svc.send(Event::Toast(crate::alerts::Kind::Info, format!("{} signed in{who}", p.id)));
                    break;
                }
            }
            let mut w = svc.inner.login_watch.lock();
            if w.get(&id) == Some(&start) {
                w.remove(&id);
            }
        });
    }

    /// A login tab closed: stop watching it and read the profiles again (it may have signed in, or out).
    pub fn login_closed(&self, id: &str) {
        self.inner.login_watch.lock().remove(id);
        if self.is_demo() {
            return;
        }
        let svc = self.clone();
        std::thread::spawn(move || workers::load_profiles(&svc));
    }

    /// Create a profile off the UI thread. The pane opens login after success.
    pub fn create_profile(&self, kind: bro_core::profiles::ProfileKind, name: String) -> std::sync::mpsc::Receiver<Result<Profile, String>> {
        let (tx, rx) = std::sync::mpsc::channel();
        let svc = self.clone();
        std::thread::spawn(move || {
            let r = if svc.is_demo() { Err("demo mode: profiles are read-only".to_string()) } else { guard_res("profiles::create", || bro_core::profiles::create(kind, &name)) };
            if r.is_ok() { workers::load_profiles(&svc); }
            let _ = tx.send(r);
        });
        rx
    }

    /// Check and save a Gemini key without running a billable prompt.
    pub fn save_gemini_profile(&self, name: String, key: String, existing: bool) -> std::sync::mpsc::Receiver<Result<Profile, String>> {
        let (tx, rx) = std::sync::mpsc::channel();
        let svc = self.clone();
        std::thread::spawn(move || {
            let result = guard_res("Gemini key check", || {
                if svc.is_demo() { anyhow::bail!("Demo mode: profiles are read-only."); }
                let detected = key.trim().is_empty();
                let key = if detected { bro_core::gemini_api::detected_key(&bro_core::config::Config::load()?).ok_or_else(|| anyhow::anyhow!("Paste a Gemini API key."))? } else { key.trim().to_string() };
                let models = bro_core::gemini_api::models(&key)?;
                let p = if existing { bro_core::profiles::get(&format!("gemini:{name}")).ok_or_else(|| anyhow::anyhow!("Profile no longer exists."))? } else { bro_core::profiles::create(bro_core::profiles::ProfileKind::GeminiApi, &name)? };
                let saved = if detected { bro_core::gemini_api::save_detected_key(&p.dir) } else { bro_core::gemini_api::save_key(&p.dir, &key) };
                if let Err(error) = saved {
                    if !existing { let _ = std::fs::remove_dir(&p.dir); }
                    return Err(error);
                }
                svc.update(|st| { st.models.insert(p.id.clone(), models); });
                Ok(bro_core::profiles::describe(p.kind, &name, p.dir))
            });
            if result.is_ok() { workers::load_profiles(&svc); }
            let _ = tx.send(result);
        });
        rx
    }

    /// Remove a profile (its login dir) on a background thread.
    pub fn remove_profile(&self, id: String) -> std::sync::mpsc::Receiver<Result<(), String>> {
        let (tx, rx) = std::sync::mpsc::channel();
        let svc = self.clone();
        std::thread::spawn(move || {
            let r = if svc.is_demo() { Err("demo mode: profiles are read-only".to_string()) } else { guard_res("profiles::remove", || bro_core::profiles::remove(&id)) };
            if r.is_ok() { workers::load_profiles(&svc); }
            let removed = r.is_ok();
            let _ = tx.send(r);
            if removed { workers::load_past(&svc); }
        });
        rx
    }

    // ------------------------------------------------------------------ proxy

    pub(crate) fn set_proxy(&self, h: Option<ProxyHandle>) {
        *self.inner.proxy.lock() = h;
    }

    /// Start a proxy when an explicit launch needs it. Called only from a launch worker, never the UI thread.
    pub(crate) fn ensure_proxy(&self) -> Result<ProxyInfo, String> {
        workers::spawn_proxy(self);
        let until = Instant::now() + std::time::Duration::from_secs(20);
        loop {
            match self.state().proxy.status.clone() {
                Avail::Ready(info) => return Ok(info),
                Avail::Unavailable(reason) if !self.inner.proxy_starting.load(Ordering::Acquire) => return Err(reason),
                _ => {}
            }
            if Instant::now() >= until { return Err("The translating proxy did not start. Check the Proxy page and try again.".into()); }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
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

    /// Loopback URL of the running bridge, for `BRO_BRIDGE_URL` in pane env (`bro open`).
    pub fn bridge_local_url(&self) -> Option<String> {
        let mut url = None;
        self.with_bridge("bridge::local_url", |b| url = b.local_url());
        url
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
    /// Remote clients (phone, web) subscribed to session `id` (0 when the bridge is off).
    pub fn bridge_remote_viewers(&self, id: &str) -> usize {
        let mut n = 0;
        self.with_bridge("bridge::remote_viewers", |b| n = b.remote_viewers(id));
        n
    }
    /// `[bridge] remote_min_*`: the size a remotely watched session is grown to.
    pub fn remote_min(&self) -> crate::panes::remote_min::MinSize {
        let st = self.state();
        let b = &st.settings.bridge;
        crate::panes::remote_min::MinSize { rows: b.remote_min_rows, cols: b.remote_min_cols, grace: std::time::Duration::from_secs(b.remote_min_grace_secs) }
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
    /// Why the session waits and its recap (the bridge debounces and publishes on change).
    pub fn bridge_set_insights(&self, id: &str, insights: bro_bridge::SessionInsights) {
        self.with_bridge("bridge::set_insights", |b| b.set_insights(id, insights));
    }
    /// The files the agent sent / touched.
    pub fn bridge_set_agent_files(&self, id: &str, files: bro_bridge::AgentFiles) {
        self.with_bridge("bridge::set_agent_files", |b| b.set_agent_files(id, files));
    }

    /// Tell the bridge's orchestrator where the user works (open projects, recent launch dirs, past sessions'
    /// project roots) so a spoken "open a session in justgains" finds the folder. Blocking file reads: call on
    /// a worker thread.
    pub(crate) fn push_known_folders(&self) {
        if !self.bridge_running() || self.is_demo() {
            return;
        }
        let mut folders: Vec<std::path::PathBuf> = crate::projects::OpenProjects::load().roots;
        folders.extend(crate::recents::dirs(&crate::recents::load()));
        if let Some(past) = self.state().past.ready() {
            folders.extend(past.iter().filter_map(|s| s.project.as_ref().map(|p| p.root.clone()).or_else(|| s.cwd.clone())));
        }
        let mut seen = std::collections::HashSet::new();
        folders.retain(|f| seen.insert(f.to_string_lossy().to_lowercase()));
        // project names are words speech recognition should spell exactly
        if let Some(voice) = self.inner.voice.lock().as_ref() {
            let mut words: Vec<String> = voice_keywords();
            words.extend(folders.iter().take(60).filter_map(|f| f.file_name().map(|n| n.to_string_lossy().into_owned())));
            voice.set_keywords(words);
        }
        self.with_bridge("bridge::set_known_folders", |b| b.set_known_folders(folders));
    }

    /// The UI loop reports which session is focused (voice routes dictated prompts to it).
    pub(crate) fn set_focused_session(&self, sid: Option<&str>) {
        let mut focused = self.inner.focused.lock();
        if focused.as_deref() != sid {
            *focused = sid.map(str::to_owned);
        }
    }

    /// The UI loop reports the agent teams running in its sessions.
    pub(crate) fn set_live_teams(&self, teams: Vec<LiveTeam>) {
        *self.inner.teams.lock() = teams;
    }

    /// Agent teams running in sessions here, in sidebar order.
    pub fn live_teams(&self) -> Vec<LiveTeam> {
        self.inner.teams.lock().clone()
    }

    pub(crate) fn focused_session(&self) -> Option<String> {
        self.inner.focused.lock().clone()
    }

    /// A dictated message for the orchestrator session, with the session the user was looking at when they
    /// started speaking. The UI delivers it (starting an orchestrator when none runs).
    pub(crate) fn orchestrator_send_voice(&self, text: String, focused: Option<String>, tag: Option<String>) {
        self.send(Event::Voice(Box::new(VoiceMsg { text, focused, tag })));
    }

    /// The orchestrator session's answer to a panel message the bridge routed to it.
    pub(crate) fn orchestrator_answer(&self, id: &str, text: &str, ok: bool) {
        self.with_bridge("bridge::orchestrator_answer", |b| b.orchestrator_answer(id, text, ok));
    }

    /// The orchestrator closed: its session-only scheduled triggers go with it.
    pub(crate) fn schedule_clear_session(&self) {
        self.with_bridge("bridge::schedule_clear_session", |b| b.schedule_clear_session());
    }

    /// Claim a watchdog wake-up for the orchestrator: (generation, the reviewer brief).
    pub fn watchdog_brief(&self, sid: &str, wake: bro_bridge::WatchdogWake) -> Result<(u64, String), String> {
        if self.is_demo() {
            return Err("the demo doesn't run reviews".into());
        }
        let mut out = Err("the bridge isn't running".to_string());
        self.with_bridge("bridge::watchdog_brief", |b| out = b.watchdog_brief(sid, wake));
        out
    }

    /// The orchestrator's review of a watchdog wake-up.
    pub fn watchdog_finish(&self, sid: &str, generation: u64, summary: String, ok: bool) {
        self.with_bridge("bridge::watchdog_finish", |b| b.watchdog_finish(sid, generation, summary, ok));
    }

    /// GPT-Live: Hugh's answer to a delegation, for the voice to speak.
    pub(crate) fn voice_delegation_result(&self, id: &str, text: &str) {
        if let Some(voice) = self.inner.voice.lock().as_ref() {
            voice.delegation_result(id, &brief(text));
        }
    }

    /// GPT-Live: what Hugh is doing for a delegation (the voice knows, doesn't read it out).
    pub(crate) fn voice_delegation_progress(&self, id: &str, text: &str) {
        if let Some(voice) = self.inner.voice.lock().as_ref() {
            voice.delegation_progress(id, text);
        }
    }

    /// Whether this bro runs push-to-talk: the first bro (a second one leaves it to that), with the phone
    /// bridge on, since the words go to Hugh through it.
    pub(crate) fn runs_voice(&self) -> bool {
        !self.inner.guest && !self.is_demo() && self.settings().bridge.enabled
    }

    /// Voice settings changed: stop the listener and start it again with them (or not, when it's off).
    pub(crate) fn restart_voice(&self) {
        if !self.runs_voice() {
            return;
        }
        self.set_voice(None);
        self.update(|st| st.voice.running = None);
        workers::spawn_voice(self);
    }

    /// Start a new listener generation; [`Services::keep_voice`] takes a listener only from the newest.
    pub(super) fn voice_generation(&self) -> u64 {
        self.inner.voice_gen.fetch_add(1, Ordering::AcqRel) + 1
    }

    /// Keep `voice` if no restart came after generation `gen` (else stop it). Returns whether it was kept.
    pub(super) fn keep_voice(&self, voice: bro_voice::Voice, gen_: u64) -> bool {
        let mut slot = self.inner.voice.lock();
        if self.inner.voice_gen.load(Ordering::Acquire) != gen_ {
            voice.stop();
            return false;
        }
        if let Some(old) = slot.replace(voice) {
            old.stop();
        }
        true
    }

    /// "Press the key you want": wait for the next key pressed anywhere, make it the push-to-talk key and
    /// restart voice on it. Esc, or nothing for a while, keeps the one there was.
    pub(crate) fn capture_voice_hotkey(&self) {
        if self.state().voice.capturing.is_some() {
            return;
        }
        self.update(|st| st.voice.capturing = Some(Instant::now()));
        let svc = self.clone();
        std::thread::spawn(move || {
            let got = guard_res("voice::capture_hotkey", || bro_voice::Voice::capture_hotkey(CAPTURE_FOR));
            svc.update(|st| st.voice.capturing = None);
            match got {
                Ok(Some(name)) => {
                    let label = bro_voice::Voice::hotkey_label(&name);
                    let mut s = svc.settings();
                    s.voice.hotkey = Some(name);
                    svc.save_settings(s);
                    svc.restart_voice();
                    svc.send(Event::Toast(crate::alerts::Kind::Info, format!("push-to-talk is on {label} now")));
                }
                Ok(None) => svc.send(Event::Toast(crate::alerts::Kind::Info, "voice key unchanged".into())),
                Err(e) => svc.send(Event::Toast(crate::alerts::Kind::Error, e.to_string())),
            }
        });
    }

    /// Keep the push-to-talk listener alive (None stops it).
    pub(crate) fn set_voice(&self, voice: Option<bro_voice::Voice>) {
        if let Some(old) = std::mem::replace(&mut *self.inner.voice.lock(), voice) {
            old.stop();
        }
    }

    // ------------------------------------------------------------------ watchdogs

    /// The session's watchdog, as last reported (cheap: the cache, no bridge call).
    pub fn watchdog(&self, sid: &str) -> Option<bro_bridge::WatchdogSnapshot> {
        self.state().watchdogs.get(sid).cloned()
    }

    fn cache_watchdog(&self, sid: &str, snapshot: Option<bro_bridge::WatchdogSnapshot>) {
        self.update(|st| match snapshot {
            Some(s) => {
                st.watchdogs.insert(sid.to_string(), s);
            }
            None => {
                st.watchdogs.remove(sid);
            }
        });
    }

    /// Start a watchdog on session `sid`. Err when the bridge (which runs the reviewers) is off.
    pub fn watchdog_start(&self, sid: &str, plan: bro_bridge::WatchdogPlan) -> Result<(), String> {
        if self.is_demo() {
            // the demo shows the pages without running reviewers
            let snapshot = bro_bridge::WatchdogSnapshot { session_id: sid.to_string(), plan, started_at: chrono::Utc::now().to_rfc3339(), ..Default::default() };
            self.cache_watchdog(sid, Some(snapshot));
            return Ok(());
        }
        if !self.bridge_running() {
            return Err("watchdogs need the bridge (it serves the reviewer's tools): turn it on in the bridge view (alt+g)".into());
        }
        let mut snapshot = None;
        self.with_bridge("bridge::watchdog_start", |b| snapshot = Some(b.watchdog_start(sid, plan)));
        self.cache_watchdog(sid, snapshot);
        Ok(())
    }

    pub fn watchdog_set_plan(&self, sid: &str, plan: bro_bridge::WatchdogPlan) {
        if self.is_demo() {
            self.update(|st| {
                if let Some(d) = st.watchdogs.get_mut(sid) {
                    d.plan = plan;
                }
            });
            return;
        }
        self.with_bridge("bridge::watchdog_set_plan", |b| {
            b.watchdog_set_plan(sid, plan);
        });
    }

    pub fn watchdog_stop(&self, sid: &str) {
        if self.state().watchdogs.contains_key(sid) {
            self.with_bridge("bridge::watchdog_stop", |b| {
                b.watchdog_stop(sid);
            });
            self.cache_watchdog(sid, None);
        }
    }

    pub fn watchdog_pause(&self, sid: &str, paused: bool) {
        if self.is_demo() {
            self.update(|st| {
                if let Some(d) = st.watchdogs.get_mut(sid) {
                    d.paused = paused;
                }
            });
            return;
        }
        self.with_bridge("bridge::watchdog_pause", |b| {
            b.watchdog_pause(sid, paused);
        });
    }

    /// Bridge updates: keep the cache current and hand the event to the UI.
    pub(crate) fn on_watchdog_update(&self, u: bro_bridge::WatchdogUpdate) {
        if let bro_bridge::WatchdogUpdate::Changed { session_id, snapshot } = &u {
            self.cache_watchdog(session_id, snapshot.as_deref().cloned());
            return;
        }
        self.send(Event::Watchdog(u));
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
            for p in ps.iter().filter(|p| p.authenticated && !p.is_gemini()) {
                let h = if p.is_codex() { "codex" } else { "claude" };
                let desc = [p.plan.clone(), p.email.clone()].into_iter().flatten().collect::<Vec<_>>().join(" · ");
                list.push(tp(&p.id, format!("{} · {}", if p.is_codex() { "Codex" } else { "Claude" }, p.name), h, vec![], "agent", (!desc.is_empty()).then_some(desc), Some(h)));
            }
        }
        list.push(tp("pi", "Pi".into(), "pi", vec![], "agent", Some("pi coding agent".into()), None));
        list.push(tp("omp", "omp".into(), "omp", vec![], "agent", Some("oh-my-pi".into()), None));
        self.with_bridge("bridge::set_profiles", |b| b.set_profiles(list));
    }

    /// Request compression (llmtrim) in bro-proxy on or off: saved, applied to the running proxy
    /// at once. Native logins launched from now on are routed through the proxy while it is on.
    pub fn set_trim(&self, on: bool) {
        let mut s = self.settings();
        s.proxy.trim = on;
        let level = bro_proxy::TrimLevel::parse(&s.proxy.trim_level);
        self.save_settings(s);
        self.with_proxy("proxy::set_trim", |p| p.set_trim(on, level));
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
    fn brief_keeps_the_outcome_and_drops_the_rest() {
        assert_eq!(brief("Opened Toast in justgains."), "Opened Toast in justgains.");
        assert_eq!(brief("I opened a Claude session in **justgains**. It's named Toast. Want me to send it a task?"), "I opened a Claude session in justgains.");
        assert_eq!(brief("Want me to retry? Mal's build failed."), "Mal's build failed.");
        assert_eq!(brief("Version 2.1 is out. Done."), "Version 2.1 is out.", "a dot inside a word isn't a sentence end");
        let long = "Mal is working through the migration of every settings screen to the new theme system and has touched forty files so far without errors";
        let cut = brief(long);
        assert!(cut.chars().count() <= BRIEF_CHARS + 1 && cut.ends_with('…'), "{cut}");
        assert_eq!(brief("   "), "");
    }

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
