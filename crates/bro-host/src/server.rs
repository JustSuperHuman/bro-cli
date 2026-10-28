//! The server: accepts clients on loopback, starts and owns sessions, keeps each one's screen current (answering
//! what its program asks the terminal) and streams its output to every client watching it.
//!
//! Threads, all blocking: the accept loop; a heartbeat; the store's writer; per connection a reader (frames in)
//! and a writer (frames out, from a bounded queue); per session a pty reader, an input writer (a child that stops
//! reading can't stall anything else) and a waiter that reaps the child.
//!
//! Ordering: a session's output is parsed and queued for its watchers under the same lock an attach takes to cut
//! its snapshot, and its size changes go out under that lock too, so a client sees every byte exactly once (in the
//! snapshot or after it) and resizes its screen exactly where the program's was.
//!
//! Flow control, as in a terminal: the primary and a session's controller get its output with a blocking send,
//! so one that stops reading slows the program down instead of growing the server without bound. A viewer that
//! falls a whole queue behind is dropped instead (it reconnects and catches up from a snapshot).

use crate::answer::Profile;
use crate::screen::Screen;
use crate::store::{self, Kept, Store};
use crate::wire::{self, Added, Blob, Control, Exit, Hello, Id, Ids, PeersMsg, Refused, Relay, Resize, Set, Spawned};
use crate::{HostInfo, PROTOCOL, Peer, Peers, Role, SessionInfo, SpawnRequest, now_ms, record};
use anyhow::Context;
use bro_core::proc_tree::ProcessTree;
use parking_lot::Mutex;
use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};
use serde_json::Value;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender, SyncSender, TrySendError};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

pub struct ServeOpts {
    /// bro's folder: the record and the lock live here.
    pub dir: PathBuf,
    /// The bro version serving (shown in settings; a window says when it's older than itself).
    pub version: String,
    /// Stop after this long with no session and no client...
    pub idle: Duration,
    /// ...unless this says the server for `dir` stays running (bro starts it at sign-in: [`crate::service::resident`]).
    /// Asked only once it has been idle that long.
    pub resident: fn(&Path) -> bool,
}

/// Frames queued for a client before its sessions' output waits for it (or, for a viewer, before it's dropped).
const QUEUE: usize = 8192;
/// After the child is reaped, how long its last output may take to drain before watchers hear it exited
/// (ConPTY can hold its output pipe open past the child).
const DRAIN: Duration = Duration::from_secs(3);
/// A client hears something at least this often (see [`crate::client`]: one that hears nothing for a while takes
/// the connection as lost).
#[cfg(not(test))]
pub(crate) const HEARTBEAT: Duration = Duration::from_secs(15);
#[cfg(test)]
pub(crate) const HEARTBEAT: Duration = Duration::from_secs(1);
/// How long a window taking over waits for the client it replaces to leave (it stops its proxy and phone bridge
/// first, so the newcomer can start them on the same ports).
const HANDOVER: Duration = Duration::from_secs(8);

struct Host {
    opts: ServeOpts,
    token: String,
    pid: u32,
    port: u16,
    started_ms: u64,
    sessions: Mutex<HashMap<String, Arc<Session>>>,
    conns: Mutex<HashMap<u64, Arc<Conn>>>,
    /// The connection driving the server (0: none).
    primary: AtomicU64,
    values: Mutex<HashMap<String, String>>,
    /// Sessions a server that stopped unasked left running, until a window brings them back or lets them go.
    lost: Mutex<Vec<SessionInfo>>,
    store: OnceLock<Store>,
    stopping: AtomicBool,
    next_conn: AtomicU64,
    /// The last moment the server had a client or a session.
    busy_at: Mutex<Instant>,
}

impl Host {
    fn changed(&self) {
        if let Some(s) = self.store.get() {
            s.changed();
        }
    }

    fn conn(&self, id: u64) -> Option<Arc<Conn>> {
        self.conns.lock().get(&id).cloned()
    }

    fn peers(&self) -> Peers {
        let conns = self.conns.lock();
        let primary = conns.get(&self.primary.load(Ordering::SeqCst)).map(|c| Peer { pid: c.pid, role: c.role });
        Peers { primary, viewers: conns.values().filter(|c| c.role == Role::Viewer).count() }
    }

    /// Tell every client that isn't only asking (but `except`).
    fn broadcast(&self, frame: &[u8], except: u64) {
        let conns: Vec<Arc<Conn>> = self.conns.lock().values().filter(|c| c.id != except && c.role != Role::Query).cloned().collect();
        for c in conns {
            let _ = c.tx.try_send(frame.to_vec());
        }
    }
}

/// One client.
struct Conn {
    id: u64,
    role: Role,
    pid: u32,
    tx: SyncSender<Vec<u8>>,
    sock: Arc<TcpStream>,
    /// What its terminal looks like (None: it has no screen of its own, as the background bro).
    profile: Mutex<Option<Arc<Profile>>>,
    /// Taken over by a newer window: what it still sends is ignored while it leaves.
    retired: AtomicBool,
}

impl Conn {
    /// Cut it off (it fell too far behind, or the server is stopping).
    fn drop_connection(&self) {
        let _ = self.sock.shutdown(Shutdown::Both);
    }
}

struct View {
    screen: Screen,
    /// Who gets the output.
    subs: Vec<Arc<Conn>>,
    /// Whose input it last took: its size is the session's, its terminal answers the program's queries.
    controller: Option<u64>,
    /// The size each watcher would like (applied while it controls the session).
    want: HashMap<u64, (u16, u16)>,
    profile: Arc<Profile>,
}

impl View {
    /// Queue `frame` for every watcher (see the module doc for who waits and who is dropped).
    fn deliver(&mut self, host: &Host, frame: &[u8]) {
        let (primary, controller) = (host.primary.load(Ordering::SeqCst), self.controller);
        self.subs.retain(|c| {
            if c.id == primary || Some(c.id) == controller {
                return c.tx.send(frame.to_vec()).is_ok();
            }
            match c.tx.try_send(frame.to_vec()) {
                Ok(()) => true,
                Err(TrySendError::Full(_)) => {
                    c.drop_connection();
                    false
                }
                Err(TrySendError::Disconnected(_)) => false,
            }
        });
    }
}

/// How far a session got in ending: watchers hear it exited once the output drained (or [`DRAIN`] passed) and
/// the child was reaped.
#[derive(Default)]
struct End {
    eof: bool,
    /// Some once the child was reaped (its code, when it had one)
    code: Option<Option<i32>>,
    sent: bool,
}

struct Session {
    id: String,
    pid: Option<u32>,
    created: u64,
    blob: Mutex<Value>,
    keep: Mutex<Value>,
    view: Mutex<View>,
    input: Mutex<Option<Sender<Vec<u8>>>>,
    master: Mutex<Option<Box<dyn MasterPty + Send>>>,
    killer: Mutex<Option<Box<dyn ChildKiller + Send + Sync>>>,
    tree: Mutex<Option<ProcessTree>>,
    end: Mutex<End>,
}

impl Session {
    fn info(&self) -> SessionInfo {
        let (rows, cols) = self.view.lock().screen.size();
        let end = self.end.lock();
        SessionInfo {
            id: self.id.clone(),
            pid: self.pid,
            rows,
            cols,
            exited: end.code.is_some(),
            code: end.code.flatten(),
            blob: self.blob.lock().clone(),
        }
    }

    fn running(&self) -> bool {
        self.end.lock().code.is_none()
    }

    fn write(&self, bytes: Vec<u8>) {
        if let Some(tx) = self.input.lock().as_ref() {
            let _ = tx.send(bytes);
        }
    }

    fn kill(&self) {
        if let Some(t) = self.tree.lock().as_mut() {
            t.terminate();
        }
        if let Some(k) = self.killer.lock().as_mut() {
            let _ = k.kill();
        }
    }

    /// Record progress towards the end; tells the watchers once the output drained and the child is reaped.
    fn ended(&self, host: &Host, f: impl FnOnce(&mut End)) {
        let send = {
            let mut e = self.end.lock();
            let was_running = e.code.is_none();
            f(&mut e);
            if was_running && e.code.is_some() {
                // no longer something to bring back
                host.changed();
            }
            let done = e.eof && e.code.is_some() && !e.sent;
            if done {
                e.sent = true;
            }
            done.then(|| e.code.flatten())
        };
        if let Some(code) = send {
            let frame = wire::json(wire::EXIT, &Exit { id: self.id.clone(), code });
            self.view.lock().deliver(host, &frame);
        }
    }

    /// `conn` controls the session now: it hears so (and whoever had it), its size applies and its terminal
    /// answers the program.
    fn take_control(&self, host: &Host, v: &mut View, conn: &Arc<Conn>) {
        if v.controller == Some(conn.id) {
            return;
        }
        let old = v.controller.replace(conn.id);
        if let Some(p) = conn.profile.lock().clone() {
            v.profile = p;
        }
        for c in &v.subs {
            if c.id == conn.id || Some(c.id) == old {
                let _ = c.tx.try_send(wire::json(wire::CONTROL, &Control { id: self.id.clone(), you: c.id == conn.id }));
            }
        }
        if let Some(&(rows, cols)) = v.want.get(&conn.id) {
            self.apply_size(host, v, rows, cols);
        }
    }

    /// Resize the pty and the screen; watchers hear it in line with the output.
    fn apply_size(&self, host: &Host, v: &mut View, rows: u16, cols: u16) {
        if rows == 0 || cols == 0 || (rows, cols) == v.screen.size() {
            return;
        }
        v.screen.resize(rows, cols);
        if let Some(m) = self.master.lock().as_ref() {
            let _ = m.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 });
        }
        v.deliver(host, &wire::json(wire::RESIZED, &Resize { id: self.id.clone(), rows, cols }));
    }
}

/// Run the server until it's told to stop or sits idle. Holds the lock in `opts.dir` and writes the record once it
/// accepts connections. Returns when it has stopped (the sessions it owned are ended).
pub fn serve(opts: ServeOpts) -> anyhow::Result<()> {
    std::fs::create_dir_all(&opts.dir).with_context(|| format!("couldn't create {}", opts.dir.display()))?;
    let lock = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(record::lock_path(&opts.dir))?;
    if lock.try_lock().is_err() {
        anyhow::bail!("a session server is already running for {}", opts.dir.display());
    }
    let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))?;
    let port = listener.local_addr()?.port();
    // what the last server left running (it stopped without being asked to)
    let lost = store::take_over(&opts.dir);
    let host = Arc::new(Host {
        token: uuid::Uuid::new_v4().simple().to_string(),
        pid: std::process::id(),
        port,
        started_ms: now_ms(),
        sessions: Mutex::new(HashMap::new()),
        conns: Mutex::new(HashMap::new()),
        primary: AtomicU64::new(0),
        values: Mutex::new(HashMap::new()),
        lost: Mutex::new(lost),
        store: OnceLock::new(),
        stopping: AtomicBool::new(false),
        next_conn: AtomicU64::new(1),
        busy_at: Mutex::new(Instant::now()),
        opts,
    });
    {
        let weak = Arc::downgrade(&host);
        let _ = host.store.set(Store::start(host.opts.dir.clone(), move || weak.upgrade().filter(|h| !h.stopping.load(Ordering::SeqCst)).map(|h| kept(&h))));
    }
    record::write(&host.opts.dir, &record::Record { pid: host.pid, port, token: host.token.clone(), protocol: PROTOCOL, version: host.opts.version.clone() })?;
    {
        let host = host.clone();
        std::thread::Builder::new().name("bro-host-idle".into()).spawn(move || idle_watch(host))?;
    }
    {
        let weak = Arc::downgrade(&host);
        std::thread::Builder::new().name("bro-host-beat".into()).spawn(move || heartbeat(weak))?;
    }
    for conn in listener.incoming() {
        if host.stopping.load(Ordering::SeqCst) {
            break;
        }
        let Ok(sock) = conn else { continue };
        let host = host.clone();
        let _ = std::thread::Builder::new().name("bro-host-conn".into()).spawn(move || connection(host, sock));
    }
    record::remove(&host.opts.dir, host.pid);
    drop(lock);
    Ok(())
}

/// What the store writes: the running sessions that can come back, and the lost ones not yet taken.
fn kept(host: &Host) -> store::File {
    // the sessions first, then each one's own locks: a session busy delivering output holds up only itself
    let all: Vec<Arc<Session>> = host.sessions.lock().values().cloned().collect();
    let mut sessions: Vec<(u64, Kept)> = all
        .iter()
        .filter(|s| s.running())
        .filter_map(|s| {
            let keep = s.keep.lock().clone();
            let (rows, cols) = s.view.lock().screen.size();
            (!keep.is_null()).then(|| (s.created, Kept { id: s.id.clone(), keep, rows, cols }))
        })
        .collect();
    sessions.sort_by_key(|(c, _)| *c);
    store::File { sessions: sessions.into_iter().map(|(_, k)| k).collect(), lost: host.lost.lock().clone() }
}

/// Stop the server once it has had neither a session nor a client for `idle`, unless it's resident.
fn idle_watch(host: Arc<Host>) {
    loop {
        std::thread::sleep(Duration::from_millis(500).min(host.opts.idle));
        if host.stopping.load(Ordering::SeqCst) {
            return;
        }
        // sessions that ended while nobody watched don't keep the server up
        let running = host.sessions.lock().values().any(|s| s.running());
        let busy = running || host.conns.lock().values().any(|c| c.role != Role::Query);
        let mut at = host.busy_at.lock();
        if busy || (at.elapsed() >= host.opts.idle && (host.opts.resident)(&host.opts.dir)) {
            *at = Instant::now();
        } else if at.elapsed() >= host.opts.idle {
            drop(at);
            stop(&host);
            return;
        }
    }
}

/// Something for every client now and then, so a client can tell a quiet server from a dead one.
fn heartbeat(host: std::sync::Weak<Host>) {
    loop {
        std::thread::sleep(HEARTBEAT);
        let Some(host) = host.upgrade() else { return };
        if host.stopping.load(Ordering::SeqCst) {
            return;
        }
        let conns: Vec<Arc<Conn>> = host.conns.lock().values().cloned().collect();
        for c in conns {
            let _ = c.tx.try_send(wire::frame(wire::PING, b""));
        }
    }
}

/// End every session (it was asked for: nothing is left to bring back) and let the accept loop return.
fn stop(host: &Arc<Host>) {
    if host.stopping.swap(true, Ordering::SeqCst) {
        return;
    }
    let sessions: Vec<Arc<Session>> = host.sessions.lock().drain().map(|(_, s)| s).collect();
    for s in &sessions {
        s.kill();
    }
    let _ = store::write(&host.opts.dir, &store::File { sessions: vec![], lost: host.lost.lock().clone() });
    record::remove(&host.opts.dir, host.pid);
    // the clients hear it was asked for (nothing to reconnect to), and at once, not when this process is gone
    let conns: Vec<Arc<Conn>> = host.conns.lock().values().cloned().collect();
    for c in &conns {
        let _ = c.tx.try_send(wire::frame(wire::STOPPING, b""));
    }
    std::thread::sleep(Duration::from_millis(150));
    for c in &conns {
        c.drop_connection();
    }
    // wake the accept loop
    let _ = TcpStream::connect_timeout(&SocketAddr::from((Ipv4Addr::LOCALHOST, host.port)), Duration::from_secs(1));
}

fn connection(host: Arc<Host>, sock: TcpStream) {
    let _ = sock.set_nodelay(true);
    // a peer gets a few seconds to say hello. The timeout goes on and comes off this very handle: on Windows a
    // clone keeps its own, and a reader left with it would drop a client that was quiet for that long.
    let _ = sock.set_read_timeout(Some(Duration::from_secs(5)));
    let hello = match wire::read_frame(&mut &sock) {
        Ok((wire::HELLO, body)) => serde_json::from_slice::<Hello>(&body).ok(),
        _ => None,
    };
    if sock.set_read_timeout(None).is_err() {
        return;
    }
    let (Ok(mut reader), Ok(mut out)) = (sock.try_clone(), sock.try_clone()) else { return };
    let refuse = |out: &mut TcpStream, reason: String, busy: bool| {
        let _ = wire::write_all(out, &wire::json(wire::REFUSED, &Refused { reason, protocol: PROTOCOL, busy }));
    };
    let Some(hello) = hello.filter(|h| h.token == host.token) else {
        return refuse(&mut out, "wrong token".into(), false);
    };
    if hello.protocol != PROTOCOL {
        return refuse(&mut out, format!("this session server speaks protocol {PROTOCOL}, the window {}", hello.protocol), false);
    }
    let id = host.next_conn.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = mpsc::sync_channel::<Vec<u8>>(QUEUE);
    let sock = Arc::new(sock);
    let me = Arc::new(Conn { id, role: hello.role, pid: hello.pid, tx: tx.clone(), sock: sock.clone(), profile: Mutex::new(hello.profile.map(Arc::new)), retired: AtomicBool::new(false) });
    // the background bro only drives a server no window drives
    if hello.role == Role::Background && host.primary.compare_exchange(0, id, Ordering::SeqCst, Ordering::SeqCst).is_err() {
        return refuse(&mut out, "a window drives the session server".into(), true);
    }
    {
        let sock = sock.clone();
        let _ = std::thread::Builder::new().name("bro-host-out".into()).spawn(move || {
            while let Ok(frame) = rx.recv() {
                if wire::write_all(&mut out, &frame).is_err() {
                    break;
                }
            }
            let _ = sock.shutdown(Shutdown::Both);
        });
    }
    if hello.role == Role::Window {
        // one window drives the server: a new one takes over from the last (or from the background bro)
        let old = host.primary.swap(id, Ordering::SeqCst);
        if old != 0
            && let Some(o) = host.conn(old)
        {
            o.retired.store(true, Ordering::SeqCst);
            let _ = o.tx.try_send(wire::frame(wire::TAKEN, b""));
            let until = Instant::now() + HANDOVER;
            while host.conns.lock().contains_key(&old) && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(20));
            }
            o.drop_connection();
        }
    }
    let _ = tx.send(wire::json(wire::WELCOME, &welcome(&host, &me)));
    // only now does it hear broadcasts and heartbeats: the welcome comes first
    host.conns.lock().insert(id, me.clone());
    if hello.role != Role::Query {
        host.broadcast(&wire::json(wire::PEERS, &PeersMsg { peers: host.peers() }), 0);
    }
    while let Ok((kind, body)) = wire::read_frame(&mut reader) {
        if !handle(&host, &me, kind, &body) {
            break;
        }
    }
    leave(&host, &me);
}

/// A client went: it watches and controls nothing any more, and if it drove the server nobody does.
fn leave(host: &Arc<Host>, me: &Arc<Conn>) {
    let sessions: Vec<Arc<Session>> = host.sessions.lock().values().cloned().collect();
    for s in sessions {
        let mut v = s.view.lock();
        v.subs.retain(|c| c.id != me.id);
        v.want.remove(&me.id);
        if v.controller == Some(me.id) {
            v.controller = None;
        }
    }
    let _ = host.primary.compare_exchange(me.id, 0, Ordering::SeqCst, Ordering::SeqCst);
    host.conns.lock().remove(&me.id);
    if me.role != Role::Query && !host.stopping.load(Ordering::SeqCst) {
        host.broadcast(&wire::json(wire::PEERS, &PeersMsg { peers: host.peers() }), 0);
    }
}

/// What a client hears first (it isn't among the connections yet: counted in by hand).
fn welcome(host: &Host, me: &Conn) -> HostInfo {
    let mut peers = host.peers();
    if host.primary.load(Ordering::SeqCst) == me.id {
        peers.primary = Some(Peer { pid: me.pid, role: me.role });
    }
    if me.role == Role::Viewer {
        peers.viewers += 1;
    }
    HostInfo {
        protocol: PROTOCOL,
        pid: host.pid,
        version: host.opts.version.clone(),
        started_ms: host.started_ms,
        sessions: list(host),
        values: host.values.lock().clone(),
        lost: host.lost.lock().clone(),
        peers,
        role: me.role,
    }
}

/// The sessions, oldest first.
fn list(host: &Host) -> Vec<SessionInfo> {
    let all: Vec<Arc<Session>> = host.sessions.lock().values().cloned().collect();
    let mut sessions: Vec<(u64, SessionInfo)> = all.iter().map(|s| (s.created, s.info())).collect();
    sessions.sort_by_key(|(c, _)| *c);
    sessions.into_iter().map(|(_, s)| s).collect()
}

fn session(host: &Host, id: &str) -> Option<Arc<Session>> {
    host.sessions.lock().get(id).cloned()
}

/// One frame from a client. False ends the connection.
fn handle(host: &Arc<Host>, me: &Arc<Conn>, kind: u8, body: &[u8]) -> bool {
    match kind {
        wire::LIST => {
            let _ = me.tx.send(wire::json(wire::LISTED, &list(host)));
            return true;
        }
        wire::SHUTDOWN => {
            stop(host);
            return false;
        }
        _ if me.role == Role::Query => return true,
        // a client that was taken over may still be sending while it leaves: it only lets go of sessions
        wire::DETACH => {}
        _ if me.retired.load(Ordering::SeqCst) => return true,
        _ => {}
    }
    match kind {
        wire::INPUT => {
            if let Some((id, bytes)) = wire::split_data(body)
                && let Some(s) = session(host, id)
            {
                s.write(bytes.to_vec());
                // typing into a session takes it over
                let mut v = s.view.lock();
                if v.subs.iter().any(|c| c.id == me.id) {
                    s.take_control(host, &mut v, me);
                }
            }
        }
        wire::SPAWN => {
            let Ok(req) = serde_json::from_slice::<SpawnRequest>(body) else { return true };
            let id = req.id.clone();
            let reply = match spawn(host, req, me) {
                Ok(pid) => Spawned { id: id.clone(), pid, error: None },
                Err(e) => Spawned { id: id.clone(), pid: None, error: Some(e) },
            };
            let started = reply.error.is_none();
            let _ = me.tx.send(wire::json(wire::SPAWNED, &reply));
            if started && let Some(s) = session(host, &id) {
                host.broadcast(&wire::json(wire::ADDED, &Added { session: s.info() }), me.id);
                host.changed();
            }
        }
        wire::RESIZE => {
            if let Ok(r) = serde_json::from_slice::<Resize>(body)
                && let Some(s) = session(host, &r.id)
                && r.rows > 0
                && r.cols > 0
            {
                let mut v = s.view.lock();
                v.want.insert(me.id, (r.rows, r.cols));
                match v.controller {
                    Some(c) if c == me.id => s.apply_size(host, &mut v, r.rows, r.cols),
                    // nobody's: the first to size it has it
                    None if v.subs.iter().any(|c| c.id == me.id) => s.take_control(host, &mut v, me),
                    _ => {}
                }
            }
        }
        wire::KILL => {
            if let Ok(r) = serde_json::from_slice::<Id>(body)
                && let Some(s) = session(host, &r.id)
            {
                s.kill();
            }
        }
        wire::ATTACH => {
            if let Ok(r) = serde_json::from_slice::<Id>(body)
                && let Some(s) = session(host, &r.id)
            {
                let mut v = s.view.lock();
                v.subs.retain(|c| c.id != me.id);
                // its size first, so the snapshot lands in a screen of the size it was cut at
                let (rows, cols) = v.screen.size();
                let _ = me.tx.send(wire::json(wire::RESIZED, &Resize { id: s.id.clone(), rows, cols }));
                let snapshot = v.screen.replay();
                let _ = me.tx.send(wire::data(wire::SNAPSHOT, &s.id, &snapshot));
                v.subs.push(me.clone());
                if v.controller.is_none() {
                    s.take_control(host, &mut v, me);
                } else {
                    let _ = me.tx.send(wire::json(wire::CONTROL, &Control { id: s.id.clone(), you: false }));
                }
                let end = s.end.lock();
                if end.sent {
                    let _ = me.tx.send(wire::json(wire::EXIT, &Exit { id: s.id.clone(), code: end.code.flatten() }));
                }
            } else if let Ok(r) = serde_json::from_slice::<Id>(body) {
                // gone: say so, so the client doesn't wait on it
                let _ = me.tx.send(wire::json(wire::EXIT, &Exit { id: r.id, code: None }));
            }
        }
        wire::DETACH => {
            if let Ok(r) = serde_json::from_slice::<Id>(body)
                && let Some(s) = session(host, &r.id)
            {
                let mut v = s.view.lock();
                v.subs.retain(|c| c.id != me.id);
                v.want.remove(&me.id);
                if v.controller == Some(me.id) {
                    v.controller = None;
                }
            }
        }
        wire::BLOB => {
            if let Ok(b) = serde_json::from_slice::<Blob>(body)
                && let Some(s) = session(host, &b.id)
            {
                *s.blob.lock() = b.blob.clone();
                *s.keep.lock() = b.keep;
                host.broadcast(&wire::json(wire::BLOBBED, &Blob { id: b.id, blob: b.blob, keep: Value::Null }), me.id);
                host.changed();
            }
        }
        wire::CLOSE => {
            if let Ok(r) = serde_json::from_slice::<Id>(body)
                && let Some(s) = host.sessions.lock().remove(&r.id)
            {
                s.kill();
                host.changed();
            }
        }
        wire::SET => {
            if let Ok(s) = serde_json::from_slice::<Set>(body) {
                host.values.lock().insert(s.key, s.value);
            }
        }
        wire::PROFILE => {
            if let Ok(p) = serde_json::from_slice::<Profile>(body) {
                let p = Arc::new(p);
                *me.profile.lock() = Some(p.clone());
                let sessions: Vec<Arc<Session>> = host.sessions.lock().values().cloned().collect();
                for s in sessions {
                    let mut v = s.view.lock();
                    if v.controller == Some(me.id) {
                        v.profile = p.clone();
                    }
                }
            }
        }
        wire::RELAY => {
            if let Ok(r) = serde_json::from_slice::<Relay>(body) {
                let primary = host.primary.load(Ordering::SeqCst);
                match host.conn(primary).filter(|p| p.id != me.id) {
                    Some(p) => {
                        let _ = p.tx.send(wire::json(wire::RELAYED, &r));
                    }
                    None => {
                        let _ = me.tx.try_send(wire::json(wire::UNRELAYED, &r));
                    }
                }
            }
        }
        wire::FORGET_LOST => {
            if let Ok(r) = serde_json::from_slice::<Ids>(body) {
                host.lost.lock().retain(|s| !r.ids.contains(&s.id));
                host.changed();
            }
        }
        _ => {}
    }
    true
}

/// Start `req` on a new pseudo-terminal, streaming to `me` (its controller) from its first byte.
fn spawn(host: &Arc<Host>, req: SpawnRequest, me: &Arc<Conn>) -> Result<Option<u32>, String> {
    if host.stopping.load(Ordering::SeqCst) {
        return Err("the session server is stopping".into());
    }
    if host.sessions.lock().contains_key(&req.id) {
        return Err("a session with this id is already running".into());
    }
    let (rows, cols) = (req.rows.max(2), req.cols.max(10));
    let pair = native_pty_system().openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 }).map_err(|e| format!("couldn't open a pty: {e}"))?;
    let mut cmd = CommandBuilder::new(&req.program);
    cmd.args(&req.args);
    let cwd = if req.cwd.is_dir() { req.cwd.clone() } else { bro_core::util::home_dir() };
    cmd.cwd(&cwd);
    cmd.env_clear();
    for (k, v) in &req.env {
        cmd.env(k, v);
    }
    let tree = ProcessTree::new().map_err(|e| format!("session process owner: {e}"))?;
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        // SAFETY: tree owns this job until after spawn_command returns, including error paths.
        unsafe { cmd.set_windows_job_handle(tree.job_handle().as_raw_handle()) };
    }
    let mut child = pair.slave.spawn_command(cmd).map_err(|e| format!("couldn't start {}: {e:#}", req.program))?;
    drop(pair.slave);
    let pid = child.process_id();
    let mut tree = tree;
    let setup = (|| -> Result<_, String> {
        tree.watch_pty(pid.ok_or("the session has no process id")?).map_err(|e| format!("session exit watcher: {e}"))?;
        let writer = pair.master.take_writer().map_err(|e| format!("pty writer: {e}"))?;
        let reader = pair.master.try_clone_reader().map_err(|e| format!("pty reader: {e}"))?;
        Ok((writer, reader))
    })();
    let (writer, reader) = match setup {
        Ok(x) => x,
        Err(e) => {
            tree.terminate();
            let _ = child.kill();
            let _ = child.wait();
            return Err(e);
        }
    };
    let input = input_thread(writer);
    let profile = me.profile.lock().clone().unwrap_or_default();
    let view = View { screen: Screen::new(rows, cols), subs: vec![me.clone()], controller: Some(me.id), want: HashMap::from([(me.id, (rows, cols))]), profile };
    let s = Arc::new(Session {
        id: req.id.clone(),
        pid,
        created: now_ms(),
        blob: Mutex::new(req.blob),
        keep: Mutex::new(req.keep),
        view: Mutex::new(view),
        input: Mutex::new(Some(input)),
        master: Mutex::new(Some(pair.master)),
        killer: Mutex::new(Some(child.clone_killer())),
        tree: Mutex::new(Some(tree)),
        end: Mutex::new(End::default()),
    });
    host.sessions.lock().insert(req.id, s.clone());
    {
        let (s, host) = (s.clone(), host.clone());
        let _ = std::thread::Builder::new().name("bro-host-pty".into()).spawn(move || pump(host, s, reader));
    }
    let host = host.clone();
    let _ = std::thread::Builder::new().name("bro-host-wait".into()).spawn(move || reap(host, s, child));
    Ok(pid)
}

/// The pty's input, written on a thread of its own: a program that stops reading blocks only this.
fn input_thread(mut pipe: Box<dyn Write + Send>) -> Sender<Vec<u8>> {
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    let _ = std::thread::Builder::new().name("bro-host-input".into()).spawn(move || {
        while let Ok(bytes) = rx.recv() {
            if pipe.write_all(&bytes).and_then(|_| pipe.flush()).is_err() {
                break;
            }
        }
    });
    tx
}

/// Output: into the screen (answering what the program asked), then to the watchers.
fn pump(host: Arc<Host>, s: Arc<Session>, mut reader: Box<dyn Read + Send>) {
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        let data = &buf[..n];
        let mut v = s.view.lock();
        let profile = v.profile.clone();
        let reply = v.screen.feed(data, &profile);
        if !reply.is_empty() {
            s.write(reply);
        }
        v.deliver(&host, &wire::data(wire::OUTPUT, &s.id, data));
    }
    s.ended(&host, |e| e.eof = true);
}

/// Wait for the child; then end what it left behind and close the pty so its output drains.
fn reap(host: Arc<Host>, s: Arc<Session>, mut child: Box<dyn portable_pty::Child + Send + Sync>) {
    let code = child.wait().ok().map(|st| st.exit_code() as i32);
    if let Some(t) = s.tree.lock().as_mut() {
        t.terminate();
    }
    // closing the pty ends its output once the last of it is read
    drop(s.master.lock().take());
    s.input.lock().take();
    s.ended(&host, |e| e.code = Some(code));
    std::thread::sleep(DRAIN);
    s.ended(&host, |e| e.eof = true);
}
