//! bro's session server (see [`bro_host`]): connecting at startup in this process's role (a window, the background
//! bro that stands in while no window is open, or a viewer beside another window), restarting it from settings,
//! picking the connection up again when it drops, starting the server when you sign in, and what the settings view
//! shows about it.

use super::Services;
use crate::alerts::Kind;
use crate::pane::Event;
use bro_host::Role;
use bro_host::answer::Profile;
use bro_host::client::ConnectError;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// This bro runs beside another (`bro --new`): that one drives the session server, and this one's sessions run
/// inside it as they always did.
static SOLO: AtomicBool = AtomicBool::new(false);

/// How this process connects (before services start).
static ROLE: AtomicU8 = AtomicU8::new(0);

/// What this window's terminal looks like: the server answers the programs it controls with it, and the terminal
/// UI answers its own sessions with it.
static PROFILE: Mutex<Option<Arc<Profile>>> = Mutex::new(None);

/// The background bro waits this long for a window that's closing to let go of the server.
const HANDOVER: Duration = Duration::from_secs(10);
/// How long a dropped connection is retried before its sessions are given up.
const RECONNECT: Duration = Duration::from_secs(30);

/// Keep this bro off the session server (before services start).
pub fn solo() {
    SOLO.store(true, Ordering::Relaxed);
}

/// Connect as `role` (before services start).
pub fn set_role(role: Role) {
    ROLE.store(
        match role {
            Role::Window | Role::Query => 0,
            Role::Background => 1,
            Role::Viewer => 2,
        },
        Ordering::Relaxed,
    );
}

/// How this process connects to the session server.
pub fn role() -> Role {
    match ROLE.load(Ordering::Relaxed) {
        1 => Role::Background,
        2 => Role::Viewer,
        _ => Role::Window,
    }
}

/// What this window's terminal looks like (the default until the window says).
pub fn answer_profile() -> Arc<Profile> {
    match PROFILE.lock() {
        Ok(mut p) => p.get_or_insert_with(Arc::default).clone(),
        Err(_) => Arc::default(),
    }
}

/// Say what this window's terminal looks like before services start (they connect with it).
pub fn set_profile(profile: Profile) {
    if let Ok(mut p) = PROFILE.lock() {
        *p = Some(Arc::new(profile));
    }
}

/// The session server as the settings view shows it.
#[derive(Clone, Debug, Default)]
pub struct HostState {
    /// The server this window drives.
    pub server: Option<ServerInfo>,
    /// A restart is under way (since when).
    pub restarting: Option<Instant>,
    /// The connection dropped and is being picked up again (since when).
    pub reconnecting: Option<Instant>,
    /// Why bro isn't using a server, when it should be.
    pub problem: Option<String>,
    /// Whether it starts when you sign in (read at startup and after each change).
    pub startup: Option<bro_host::service::Startup>,
    /// Changing that is under way (since when).
    pub startup_busy: Option<Instant>,
    /// Who else is connected: the window or background bro driving it, and viewers.
    pub peers: bro_host::Peers,
}

#[derive(Clone, Debug)]
pub struct ServerInfo {
    pub pid: u32,
    /// The bro version it runs (older than this bro after an update, until it's restarted).
    pub version: String,
    pub started_ms: u64,
}

/// What happened to the connection, or on the server, for the app.
#[derive(Clone, Debug)]
pub enum HostEvent {
    /// The connection dropped without warning (the server may have crashed, or may still be running).
    Lost,
    /// The server was asked to stop (`bro host stop`, a restart): its sessions ended.
    Stopped,
    /// A window drives the server now.
    TakenOver,
    /// A restart finished (or why it didn't).
    Restarted(Result<(), String>),
    /// The connection is back after it dropped (or why it isn't).
    Reconnected(Result<(), String>),
    /// Another client started a session.
    Added(Box<bro_host::SessionInfo>),
    /// Another client renamed a session.
    Blob { id: String, blob: serde_json::Value },
    /// A viewer asks this bro (the primary) for something.
    Relayed(serde_json::Value),
    /// What this viewer asked had nobody to go to.
    Unrelayed(serde_json::Value),
    /// Who's connected changed.
    Peers(bro_host::Peers),
}

impl Services {
    /// The session server this window drives, while connected.
    pub fn host(&self) -> Option<bro_host::Client> {
        self.inner.host.read().clone().filter(|c| c.connected())
    }

    /// Tests: drive this server (a real one on a folder of the test's own).
    #[cfg(test)]
    pub(crate) fn set_host_for_test(&self, client: Option<bro_host::Client>) {
        *self.inner.host.write() = client;
    }

    /// New sessions should run in the server (and one is connected). A viewer's always do: they're shared.
    pub fn hosting(&self) -> Option<bro_host::Client> {
        self.host().filter(|_| self.settings().server.keep_sessions || role() == Role::Viewer)
    }

    /// Off the server entirely (demo, `--new` beside a running bro, a guest window).
    fn off_server(&self) -> bool {
        self.inner.demo || SOLO.load(Ordering::Relaxed) || (self.inner.guest && role() != Role::Viewer)
    }

    /// Connect to the session server at startup, starting it when sessions should keep running. Blocks briefly
    /// (a running server answers at once; starting one takes a moment) so sessions a past window left come back
    /// before anything new launches.
    pub(crate) fn connect_host(&self) {
        if self.inner.demo {
            return;
        }
        // a registry value or a file: quick
        let startup = bro_host::service::startup();
        self.update(|st| st.host.startup = Some(startup));
        if self.off_server() {
            return;
        }
        let start = self.should_start();
        let result = self.open_host(start);
        self.adopt_host(result, start);
    }

    /// Start a server when none runs: a window that keeps its sessions there, and the background bro.
    fn should_start(&self) -> bool {
        match role() {
            Role::Viewer => false,
            Role::Background => true,
            _ => self.settings().server.keep_sessions,
        }
    }

    fn open_host(&self, start: bool) -> Result<bro_host::Client, ConnectError> {
        let dir = bro_host::client::default_dir();
        let role = role();
        let until = Instant::now() + HANDOVER;
        loop {
            let tx = self.inner.tx.lock().clone();
            let listener = move |e: bro_host::ClientEvent| {
                let ev = match e {
                    bro_host::ClientEvent::Lost => HostEvent::Lost,
                    bro_host::ClientEvent::Stopped => HostEvent::Stopped,
                    bro_host::ClientEvent::TakenOver => HostEvent::TakenOver,
                    bro_host::ClientEvent::Added(info) => HostEvent::Added(Box::new(info)),
                    bro_host::ClientEvent::Blob { id, blob } => HostEvent::Blob { id, blob },
                    bro_host::ClientEvent::Relayed(body) => HostEvent::Relayed(body),
                    bro_host::ClientEvent::Unrelayed(body) => HostEvent::Unrelayed(body),
                    bro_host::ClientEvent::Peers(p) => HostEvent::Peers(p),
                };
                let _ = tx.send(Event::Host(ev));
            };
            let result = match std::env::current_exe() {
                Ok(exe) if start => bro_host::Client::connect_or_start(&dir, &exe, role, listener),
                _ => bro_host::Client::connect(&dir, role, listener),
            };
            match result {
                // a window that's closing still holds it for a moment
                Err(ConnectError::Busy) if role == Role::Background && Instant::now() < until => std::thread::sleep(Duration::from_millis(100)),
                Ok(c) => {
                    if let Some(p) = PROFILE.lock().ok().and_then(|p| p.clone()) {
                        c.set_profile(&p);
                    }
                    return Ok(c);
                }
                other => return other,
            }
        }
    }

    fn adopt_host(&self, result: Result<bro_host::Client, ConnectError>, wanted: bool) {
        let (client, problem) = match result {
            Ok(c) => (Some(c), None),
            Err(ConnectError::NotRunning) if !wanted => (None, None),
            Err(e) => (None, Some(e.to_string())),
        };
        let server = client.as_ref().map(|c| ServerInfo { pid: c.info().pid, version: c.info().version.clone(), started_ms: c.info().started_ms });
        let peers = client.as_ref().map(|c| c.peers()).unwrap_or_default();
        *self.inner.host.write() = client;
        self.update(|st| {
            st.host.server = server;
            st.host.problem = problem;
            st.host.peers = peers;
        });
    }

    /// Start the server now (sessions should keep running and none is connected yet). Off the UI thread.
    pub(crate) fn ensure_host(&self) {
        if self.host().is_some() || self.off_server() || role() == Role::Viewer {
            return;
        }
        let svc = self.clone();
        std::thread::spawn(move || {
            let result = svc.open_host(true);
            svc.adopt_host(result, true);
            svc.wake();
        });
    }

    /// Replace the session server with a fresh one run by this bro: the old one ends every session it holds.
    /// Off the UI thread; the app hears [`HostEvent::Restarted`] when the new one is connected.
    pub fn restart_host(&self) {
        if self.off_server() || role() == Role::Viewer || self.state().host.restarting.is_some() {
            return;
        }
        self.update(|st| st.host.restarting = Some(Instant::now()));
        let svc = self.clone();
        std::thread::spawn(move || {
            let old = svc.inner.host.write().take();
            match old {
                Some(c) => c.shutdown(Duration::from_secs(10)),
                // one this window couldn't use (another protocol): stop it all the same
                None => {
                    bro_host::client::stop(&bro_host::client::default_dir(), Duration::from_secs(10));
                }
            }
            let result = svc.open_host(true);
            let outcome = result.as_ref().map(|_| ()).map_err(|e| e.to_string());
            svc.adopt_host(result, true);
            svc.keep_proxy_token();
            svc.update(|st| st.host.restarting = None);
            svc.send(Event::Host(HostEvent::Restarted(outcome)));
        });
    }

    /// The connection dropped: connect again (starting the server again if it went), for up to [`RECONNECT`].
    /// Off the UI thread; the app hears [`HostEvent::Reconnected`].
    pub(crate) fn reconnect_host(&self) {
        if self.off_server() || self.state().host.reconnecting.is_some() {
            return;
        }
        self.update(|st| st.host.reconnecting = Some(Instant::now()));
        let svc = self.clone();
        std::thread::spawn(move || {
            let until = Instant::now() + RECONNECT;
            let outcome = loop {
                match svc.open_host(svc.should_start()) {
                    Ok(c) => {
                        svc.adopt_host(Ok(c), true);
                        svc.keep_proxy_token();
                        break Ok(());
                    }
                    // a window took over meanwhile: it has the sessions
                    Err(ConnectError::Busy) => break Err(ConnectError::Busy.to_string()),
                    Err(e) if Instant::now() >= until => {
                        let why = e.to_string();
                        svc.adopt_host(Err(e), true);
                        break Err(why);
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(500)),
                }
            };
            svc.update(|st| st.host.reconnecting = None);
            svc.send(Event::Host(HostEvent::Reconnected(outcome)));
        });
    }

    /// The translating proxy keeps its token on the server: sessions that come back through it still get in.
    fn keep_proxy_token(&self) {
        if let (Some(h), Some(p)) = (self.host(), self.state().proxy.status.ready().cloned()) {
            h.set_value(PROXY_TOKEN, &p.token);
        }
    }

    /// The connection dropped under this window.
    pub(crate) fn host_lost(&self) {
        {
            // a restart or a reconnect may already have put a new connection in its place
            let mut host = self.inner.host.write();
            if host.as_ref().is_some_and(|c| c.connected()) {
                return;
            }
            *host = None;
        }
        self.update(|st| {
            st.host.server = None;
            if st.host.restarting.is_none() {
                st.host.problem = Some("lost the session server".into());
            }
        });
    }

    /// What this window's terminal looks like, for the answers to its programs' queries.
    pub(crate) fn set_answer_profile(&self, profile: Profile) {
        if let Ok(mut p) = PROFILE.lock() {
            if p.as_deref() == Some(&profile) {
                return;
            }
            *p = Some(Arc::new(profile.clone()));
        }
        if let Some(h) = self.host() {
            h.set_profile(&profile);
        }
    }

    /// Start the background bro, which drives the server (the phone bridge, proxy, auto-resume, watchdogs) until a
    /// window opens again. True when it started.
    pub(crate) fn start_background(&self) -> bool {
        let Ok(exe) = std::env::current_exe() else { return false };
        bro_host::client::spawn_detached(&exe, &["background"], &bro_host::client::default_dir()).map(|mut child| std::thread::spawn(move || child.wait())).is_ok()
    }

    /// Start the server when you sign in, or stop doing so. Off the UI thread.
    pub fn set_host_startup(&self, on: bool) {
        if self.inner.demo {
            return self.send(Event::Toast(Kind::Info, "the demo doesn't change what starts when you sign in".into()));
        }
        if self.state().host.startup_busy.is_some() {
            return;
        }
        self.update(|st| st.host.startup_busy = Some(Instant::now()));
        let svc = self.clone();
        std::thread::spawn(move || {
            let done = if on { crate::host_cmd::install_startup() } else { bro_host::service::uninstall() };
            let startup = bro_host::service::startup();
            svc.update(|st| {
                st.host.startup = Some(startup);
                st.host.startup_busy = None;
            });
            match done {
                Ok(()) if on => svc.send(Event::Toast(Kind::Info, "bro starts in the background when you sign in now, and stays running".into())),
                Ok(()) => svc.send(Event::Toast(Kind::Info, "bro no longer starts when you sign in".into())),
                Err(e) => svc.send(Event::Toast(Kind::Error, format!("{e:#}"))),
            }
        });
    }

    /// The server outlives this window even with nothing to run: it starts when you sign in. Checked as bro quits
    /// (quick).
    pub(crate) fn host_resident(&self) -> bool {
        bro_host::service::resident(&bro_host::client::default_dir())
    }

    /// The token the translating proxy starts with: the one the session server keeps, so sessions that outlived
    /// a window still get in through the next window's proxy; a new one (kept there) otherwise.
    pub(crate) fn proxy_token(&self) -> String {
        if let Some(h) = self.host() {
            if let Some(t) = h.value(PROXY_TOKEN) {
                return t;
            }
            let t = uuid::Uuid::new_v4().simple().to_string();
            h.set_value(PROXY_TOKEN, &t);
            return t;
        }
        uuid::Uuid::new_v4().simple().to_string()
    }
}

const PROXY_TOKEN: &str = "proxy_token";

/// The terminal UI's look, for the answers: its theme's colours; a terminal neither reports its cell size nor
/// passes keys on in the kitty protocol.
pub(crate) fn tui_profile(t: &crate::theme::Theme) -> Profile {
    use ratatui::style::Color;
    let rgb = |c: Color| match c {
        Color::Rgb(r, g, b) => Some([r, g, b]),
        _ => None,
    };
    let d = Profile::default();
    Profile {
        fg: rgb(t.fg).unwrap_or(d.fg),
        bg: rgb(t.bg).unwrap_or(d.bg),
        cursor: t.cursor.and_then(rgb).or_else(|| rgb(t.fg)).unwrap_or(d.cursor),
        palette: t.ansi.map(|a| a.iter().zip(d.palette.iter()).map(|(c, def)| rgb(*c).unwrap_or(*def)).collect()).unwrap_or(d.palette),
        cell: None,
        kitty: false,
    }
}
