//! A client's connection to the server.
//!
//! [`Client::spawn`] and [`Client::attach`] give a [`Remote`]: a reader yielding the session's output (an attach
//! yields the snapshot first) with its size changes in line, a writer for its input, and resize / close / exit
//! status. Nothing here blocks the caller except `spawn`, which waits for the server to start the program (as
//! opening a pty in-process would). Frames out go through one writer thread; frames in are read on another and
//! handed to each session's reader through a bounded queue, so a window that falls behind slows the program down
//! as a terminal would.
//!
//! The server says something at least every [`crate::server::HEARTBEAT`]; hearing nothing for three of them, the
//! client takes the connection as lost.

use crate::answer::Profile;
use crate::wire::{self, Added, Blob, Control, Exit, Hello, Id, Ids, PeersMsg, Refused, Relay, Resize, Set, Spawned};
use crate::{HostInfo, PROTOCOL, Peers, Role, SessionInfo, SpawnRequest, record};
use parking_lot::Mutex;
use serde_json::Value;
use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::time::{Duration, Instant};

/// Something the client should know about its connection, or about sessions another client changed.
#[derive(Clone, Debug, PartialEq)]
pub enum ClientEvent {
    /// The connection went without warning (the server crashed or went quiet): it may be gone, or still running.
    Lost,
    /// The server was asked to stop (`bro host stop`, another window restarting it): its sessions ended.
    Stopped,
    /// A window connected and drives the server now.
    TakenOver,
    /// Another client started a session.
    Added(SessionInfo),
    /// Another client changed a session's metadata (a rename).
    Blob { id: String, blob: Value },
    /// A viewer's message for the primary (this client).
    Relayed(Value),
    /// A message this client relayed had no primary to go to.
    Unrelayed(Value),
    /// Who's connected changed.
    Peers(Peers),
}

const CONNECTED: u8 = 0;
const LOST: u8 = 1;
const TAKEN: u8 = 2;
const STOPPED: u8 = 3;

/// How long `spawn` waits for the server to start a program.
const SPAWN_WAIT: Duration = Duration::from_secs(20);
/// Silence longer than this (three heartbeats) means the server is gone or stuck.
const SILENCE: Duration = Duration::from_secs(crate::server::HEARTBEAT.as_secs() * 3);
/// Output chunks queued for one session's reader before the connection waits for it.
const SLOT: usize = 1024;

type Listener = Box<dyn Fn(ClientEvent) + Send + Sync>;
/// Where `spawn` waits for the server's answer: the program's pid, or why it didn't start.
type SpawnReply = SyncSender<Result<Option<u32>, String>>;

/// What a session's reader gets, in order.
enum Item {
    Data(Vec<u8>),
    Resize(u16, u16),
}

/// One session's end of the connection.
struct Slot {
    /// to the session's reader; dropped once the session exits
    out: Mutex<Option<SyncSender<Item>>>,
    /// Some once the session exited (its code, when it had one)
    exit: Mutex<Option<Option<i32>>>,
    /// this client controls it (its size is ours)
    control: AtomicBool,
    /// the session's size, as the server last said
    size: Mutex<(u16, u16)>,
}

struct Inner {
    tx: Sender<Vec<u8>>,
    info: HostInfo,
    slots: Mutex<HashMap<String, Arc<Slot>>>,
    spawns: Mutex<HashMap<String, SpawnReply>>,
    lists: Mutex<Vec<SyncSender<Vec<SessionInfo>>>>,
    state: AtomicU8,
    /// The connection has closed (the reader saw its end).
    closed: AtomicBool,
    listener: Listener,
    values: Mutex<HashMap<String, String>>,
    peers: Mutex<Peers>,
}

/// A connection to the session server. Cheap to clone.
#[derive(Clone)]
pub struct Client {
    inner: Arc<Inner>,
}

/// Why connecting didn't work.
#[derive(Debug)]
pub enum ConnectError {
    /// No server is running for this folder.
    NotRunning,
    /// A window drives the server (the background bro doesn't take over from one).
    Busy,
    /// A server runs but this client can't use it (another protocol, a stale token).
    Refused(String),
    Io(io::Error),
}

impl std::fmt::Display for ConnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConnectError::NotRunning => write!(f, "the session server isn't running"),
            ConnectError::Busy => write!(f, "a bro window drives the session server"),
            ConnectError::Refused(why) => write!(f, "the session server refused this window: {why}"),
            ConnectError::Io(e) => write!(f, "couldn't reach the session server: {e}"),
        }
    }
}

impl std::error::Error for ConnectError {}

impl From<io::Error> for ConnectError {
    fn from(e: io::Error) -> Self {
        ConnectError::Io(e)
    }
}

/// Say hello to the server recorded in `dir`. Returns the socket and its welcome.
fn open(dir: &Path, role: Role) -> Result<(TcpStream, HostInfo), ConnectError> {
    let rec = record::read(dir).ok_or(ConnectError::NotRunning)?;
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, rec.port));
    let mut sock = TcpStream::connect_timeout(&addr, Duration::from_secs(2)).map_err(|_| ConnectError::NotRunning)?;
    let _ = sock.set_nodelay(true);
    // a window taking over waits while the one it replaces leaves
    sock.set_read_timeout(Some(Duration::from_secs(15)))?;
    let hello = Hello { protocol: PROTOCOL, token: rec.token, role, pid: std::process::id(), profile: None };
    wire::write_all(&mut sock, &wire::json(wire::HELLO, &hello))?;
    let (kind, body) = wire::read_frame(&mut sock)?;
    sock.set_read_timeout(None)?;
    match kind {
        wire::WELCOME => Ok((sock, serde_json::from_slice(&body).map_err(|e| ConnectError::Refused(e.to_string()))?)),
        wire::REFUSED => match serde_json::from_slice::<Refused>(&body) {
            Ok(r) if r.busy => Err(ConnectError::Busy),
            Ok(r) => Err(ConnectError::Refused(r.reason)),
            Err(_) => Err(ConnectError::Refused("no reason given".into())),
        },
        _ => Err(ConnectError::Refused("it answered something unexpected".into())),
    }
}

impl Client {
    /// Connect to the server running for `dir` as `role` (a window takes over from whoever drove it). `listener`
    /// hears about the connection and about sessions other clients change; it runs on the connection's reader
    /// thread.
    pub fn connect(dir: &Path, role: Role, listener: impl Fn(ClientEvent) + Send + Sync + 'static) -> Result<Client, ConnectError> {
        let (sock, info) = open(dir, role)?;
        let reader = sock.try_clone()?;
        // the reader's own handle: Windows keeps a timeout per handle
        reader.set_read_timeout(Some(SILENCE))?;
        let mut writer = sock;
        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        std::thread::Builder::new().name("bro-host-client-out".into()).spawn(move || {
            while let Ok(frame) = rx.recv() {
                if wire::write_all(&mut writer, &frame).is_err() {
                    break;
                }
            }
            let _ = writer.shutdown(std::net::Shutdown::Both);
        })?;
        let values = Mutex::new(info.values.clone());
        let peers = Mutex::new(info.peers);
        let inner = Arc::new(Inner {
            tx,
            info,
            slots: Mutex::new(HashMap::new()),
            spawns: Mutex::new(HashMap::new()),
            lists: Mutex::new(vec![]),
            state: AtomicU8::new(CONNECTED),
            closed: AtomicBool::new(false),
            listener: Box::new(listener),
            values,
            peers,
        });
        let weak = Arc::downgrade(&inner);
        std::thread::Builder::new().name("bro-host-client-in".into()).spawn(move || read_loop(weak, reader))?;
        Ok(Client { inner })
    }

    /// Connect, starting the server first when none runs: `exe host` in the background, out of this window's
    /// console and job, so it outlives the window.
    pub fn connect_or_start(dir: &Path, exe: &Path, role: Role, listener: impl Fn(ClientEvent) + Send + Sync + 'static) -> Result<Client, ConnectError> {
        let listener: Arc<dyn Fn(ClientEvent) + Send + Sync> = Arc::new(listener);
        let l = listener.clone();
        match Client::connect(dir, role, move |e| l(e)) {
            Err(ConnectError::NotRunning) => {}
            other => return other,
        }
        let mut server = spawn_detached(exe, &["host"], dir)?;
        let until = Instant::now() + Duration::from_secs(10);
        loop {
            let l = listener.clone();
            match Client::connect(dir, role, move |e| l(e)) {
                Err(ConnectError::NotRunning) if Instant::now() >= until => {
                    return Err(ConnectError::Refused("it couldn't start (is another one stuck? bro host stop ends it)".into()));
                }
                // it gave up: another holds the lock, one on its way out (stopped for a restart, or crashing). Once
                // that one's gone, the next try runs
                Err(ConnectError::NotRunning) if matches!(server.try_wait(), Ok(Some(_))) => {
                    std::thread::sleep(Duration::from_millis(200));
                    server = spawn_detached(exe, &["host"], dir)?;
                }
                Err(ConnectError::NotRunning) => std::thread::sleep(Duration::from_millis(30)),
                other => {
                    // reaped when it ends, so a Unix window isn't left holding a zombie
                    std::thread::spawn(move || server.wait());
                    return other;
                }
            }
        }
    }

    /// What the server said when this client connected.
    pub fn info(&self) -> &HostInfo {
        &self.inner.info
    }

    /// Still connected (and, for a primary, still driving the server).
    pub fn connected(&self) -> bool {
        self.inner.state.load(Ordering::SeqCst) == CONNECTED
    }

    /// Sessions this client has open on the server that haven't exited.
    pub fn live(&self) -> usize {
        self.inner.slots.lock().values().filter(|s| s.exit.lock().is_none()).count()
    }

    /// Who's connected, as the server last said.
    pub fn peers(&self) -> Peers {
        *self.inner.peers.lock()
    }

    /// A value kept on the server (see [`HostInfo::values`]).
    pub fn value(&self, key: &str) -> Option<String> {
        self.inner.values.lock().get(key).cloned()
    }

    pub fn set_value(&self, key: &str, value: &str) {
        self.inner.values.lock().insert(key.into(), value.into());
        self.send(wire::json(wire::SET, &Set { key: key.into(), value: value.into() }));
    }

    /// What this client's terminal looks like: the server answers the programs it controls for it.
    pub fn set_profile(&self, profile: &Profile) {
        self.send(wire::json(wire::PROFILE, profile));
    }

    /// Hand `body` to the primary (it hears [`ClientEvent::Relayed`]); with none, this client hears
    /// [`ClientEvent::Unrelayed`].
    pub fn relay(&self, body: Value) {
        self.send(wire::json(wire::RELAY, &Relay { body }));
    }

    /// Lost sessions (see [`HostInfo::lost`]) this client brought back or let go.
    pub fn forget_lost(&self, ids: Vec<String>) {
        if !ids.is_empty() {
            self.send(wire::json(wire::FORGET_LOST, &Ids { ids }));
        }
    }

    fn send(&self, frame: Vec<u8>) {
        let _ = self.inner.tx.send(frame);
    }

    fn slot(&self, id: &str, size: (u16, u16), control: bool) -> (Arc<Slot>, Receiver<Item>) {
        let (tx, rx) = mpsc::sync_channel(SLOT);
        let slot = Arc::new(Slot { out: Mutex::new(Some(tx)), exit: Mutex::new(None), control: AtomicBool::new(control), size: Mutex::new(size) });
        self.inner.slots.lock().insert(id.to_string(), slot.clone());
        (slot, rx)
    }

    /// Start a program on the server; output streams from its first byte, and this client controls it.
    pub fn spawn(&self, req: SpawnRequest) -> Result<Remote, String> {
        if !self.connected() {
            return Err("the session server isn't connected".into());
        }
        let id = req.id.clone();
        let (slot, rx) = self.slot(&id, (req.rows.max(2), req.cols.max(10)), true);
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        self.inner.spawns.lock().insert(id.clone(), reply_tx);
        self.send(wire::json(wire::SPAWN, &req));
        let result = reply_rx.recv_timeout(SPAWN_WAIT).unwrap_or_else(|_| Err("the session server didn't start it in time".into()));
        self.inner.spawns.lock().remove(&id);
        match result {
            Ok(pid) => Ok(Remote { id, client: self.clone(), slot, rx: Some(rx), pid }),
            Err(e) => {
                self.inner.slots.lock().remove(&id);
                Err(e)
            }
        }
    }

    /// Watch a session that's already running: its reader yields a snapshot that rebuilds the screen, then the
    /// live output.
    pub fn attach(&self, info: &SessionInfo) -> Remote {
        let (slot, rx) = self.slot(&info.id, (info.rows, info.cols), false);
        if self.connected() {
            self.send(wire::json(wire::ATTACH, &Id { id: info.id.clone() }));
        } else {
            *slot.exit.lock() = Some(None);
            slot.out.lock().take();
        }
        Remote { id: info.id.clone(), client: self.clone(), slot, rx: Some(rx), pid: info.pid }
    }

    /// Forget a session (ending it if it still runs).
    pub fn close(&self, id: &str) {
        self.send(wire::json(wire::CLOSE, &Id { id: id.into() }));
    }

    /// The sessions the server holds now. None when it doesn't answer within `wait`.
    pub fn list(&self, wait: Duration) -> Option<Vec<SessionInfo>> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.inner.lists.lock().push(tx);
        self.send(wire::frame(wire::LIST, b""));
        rx.recv_timeout(wait).ok()
    }

    /// Stop the server, ending every session it holds; returns once it has let go of this client (or `wait`
    /// passed).
    pub fn shutdown(&self, wait: Duration) {
        self.send(wire::frame(wire::SHUTDOWN, b""));
        let until = Instant::now() + wait;
        while !self.inner.closed.load(Ordering::SeqCst) && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

/// Stop the server running for `dir` without driving it (`bro host stop`, or one this window can't use).
/// True when there was one.
pub fn stop(dir: &Path, wait: Duration) -> bool {
    let Some(rec) = record::read(dir) else { return false };
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, rec.port));
    let Ok(mut sock) = TcpStream::connect_timeout(&addr, Duration::from_secs(2)) else { return false };
    let _ = sock.set_read_timeout(Some(Duration::from_secs(5)));
    // whatever protocol it speaks, HELLO and SHUTDOWN keep their shape
    let hello = Hello { protocol: PROTOCOL, token: rec.token, role: Role::Query, pid: std::process::id(), profile: None };
    if wire::write_all(&mut sock, &wire::json(wire::HELLO, &hello)).is_err() {
        return false;
    }
    match wire::read_frame(&mut sock) {
        Ok((wire::WELCOME, _)) => {
            let _ = wire::write_all(&mut sock, &wire::frame(wire::SHUTDOWN, b""));
        }
        // an older protocol refuses the hello: end its process instead
        _ => kill_pid(rec.pid),
    }
    let until = Instant::now() + wait;
    while record::read(dir).is_some_and(|r| r.pid == rec.pid) && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(30));
    }
    true
}

/// What the server for `dir` holds, without driving it (`bro host status`).
pub fn status(dir: &Path) -> Result<HostInfo, ConnectError> {
    open(dir, Role::Query).map(|(_, info)| info)
}

fn kill_pid(pid: u32) {
    #[cfg(windows)]
    let _ = std::process::Command::new("taskkill").args(["/PID", &pid.to_string(), "/T", "/F"]).output();
    #[cfg(unix)]
    unsafe {
        libc::kill(pid as i32, libc::SIGTERM);
    }
}

/// `exe args` as a background process of its own, for bro folder `dir`: no console window, its own process group,
/// out of this window's job when that allows it (a terminal that kills its job on close then takes nothing with
/// it), standing in your home folder (Windows can't remove a folder a process stands in).
pub fn spawn_detached(exe: &Path, args: &[&str], dir: &Path) -> io::Result<std::process::Child> {
    let mut cmd = std::process::Command::new(exe);
    cmd.args(args)
        .env("BRO_DIR", dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .current_dir(bro_core::util::home_dir());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        let flags = CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP;
        // a job that forbids breaking away fails the spawn: try again inside it
        if let Ok(child) = cmd.creation_flags(flags | CREATE_BREAKAWAY_FROM_JOB).spawn() {
            return Ok(child);
        }
        cmd.creation_flags(flags).spawn()
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // a session of its own: the terminal's hangup doesn't reach it
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
        cmd.spawn()
    }
    #[cfg(not(any(windows, unix)))]
    cmd.spawn()
}

fn read_loop(inner: std::sync::Weak<Inner>, mut sock: TcpStream) {
    loop {
        let frame = wire::read_frame(&mut sock);
        let Some(inner) = inner.upgrade() else { return };
        let (kind, body) = match frame {
            Ok(f) => f,
            // closed, broken, or silent for three heartbeats
            Err(_) => break,
        };
        let slot = |id: &str| inner.slots.lock().get(id).cloned();
        match kind {
            wire::OUTPUT | wire::SNAPSHOT => {
                if let Some((id, bytes)) = wire::split_data(&body)
                    && let Some(tx) = slot(id).and_then(|s| s.out.lock().clone())
                {
                    // waits while the reader is a whole queue behind: the server holds the program back meanwhile
                    let _ = tx.send(Item::Data(bytes.to_vec()));
                }
            }
            wire::RESIZED => {
                if let Ok(r) = serde_json::from_slice::<Resize>(&body)
                    && let Some(s) = slot(&r.id)
                {
                    *s.size.lock() = (r.rows, r.cols);
                    if let Some(tx) = s.out.lock().clone() {
                        let _ = tx.send(Item::Resize(r.rows, r.cols));
                    }
                }
            }
            wire::CONTROL => {
                if let Ok(c) = serde_json::from_slice::<Control>(&body)
                    && let Some(s) = slot(&c.id)
                {
                    s.control.store(c.you, Ordering::SeqCst);
                }
            }
            wire::EXIT => {
                if let Ok(e) = serde_json::from_slice::<Exit>(&body)
                    && let Some(s) = slot(&e.id)
                {
                    let mut exit = s.exit.lock();
                    if exit.is_none() {
                        *exit = Some(e.code);
                    }
                    s.out.lock().take();
                }
            }
            wire::SPAWNED => {
                if let Ok(s) = serde_json::from_slice::<Spawned>(&body)
                    && let Some(reply) = inner.spawns.lock().remove(&s.id)
                {
                    let _ = reply.send(match s.error {
                        Some(e) => Err(e),
                        None => Ok(s.pid),
                    });
                }
            }
            wire::LISTED => {
                if let Ok(list) = serde_json::from_slice::<Vec<SessionInfo>>(&body) {
                    let waiting = std::mem::take(&mut *inner.lists.lock());
                    for w in waiting {
                        let _ = w.send(list.clone());
                    }
                }
            }
            wire::TAKEN => {
                inner.state.store(TAKEN, Ordering::SeqCst);
                (inner.listener)(ClientEvent::TakenOver);
            }
            wire::STOPPING => {
                if inner.state.compare_exchange(CONNECTED, STOPPED, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
                    (inner.listener)(ClientEvent::Stopped);
                }
            }
            wire::ADDED => {
                if let Ok(a) = serde_json::from_slice::<Added>(&body) {
                    (inner.listener)(ClientEvent::Added(a.session));
                }
            }
            wire::BLOBBED => {
                if let Ok(b) = serde_json::from_slice::<Blob>(&body) {
                    (inner.listener)(ClientEvent::Blob { id: b.id, blob: b.blob });
                }
            }
            wire::RELAYED => {
                if let Ok(r) = serde_json::from_slice::<Relay>(&body) {
                    (inner.listener)(ClientEvent::Relayed(r.body));
                }
            }
            wire::UNRELAYED => {
                if let Ok(r) = serde_json::from_slice::<Relay>(&body) {
                    (inner.listener)(ClientEvent::Unrelayed(r.body));
                }
            }
            wire::PEERS => {
                if let Ok(p) = serde_json::from_slice::<PeersMsg>(&body) {
                    *inner.peers.lock() = p.peers;
                    (inner.listener)(ClientEvent::Peers(p.peers));
                }
            }
            _ => {}
        }
    }
    let Some(inner) = inner.upgrade() else { return };
    inner.closed.store(true, Ordering::SeqCst);
    // the connection is gone (or this client was taken off): every session here ends for it
    for slot in inner.slots.lock().values() {
        let mut exit = slot.exit.lock();
        if exit.is_none() {
            *exit = Some(None);
        }
        slot.out.lock().take();
    }
    for (_, reply) in inner.spawns.lock().drain() {
        let _ = reply.send(Err("lost the session server".into()));
    }
    if inner.state.compare_exchange(CONNECTED, LOST, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
        (inner.listener)(ClientEvent::Lost);
    }
}

/// One session on the server, as a client watches and drives it.
pub struct Remote {
    id: String,
    client: Client,
    slot: Arc<Slot>,
    rx: Option<Receiver<Item>>,
    pid: Option<u32>,
}

impl Remote {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn pid(&self) -> Option<u32> {
        self.pid
    }

    /// The session's output (once): blocks for more, ends when the session exits or the connection goes. Size
    /// changes are dropped (see [`Remote::take_reader_with`]).
    pub fn take_reader(&mut self) -> Option<Box<dyn Read + Send>> {
        self.take_reader_with(|_, _| {})
    }

    /// The session's output, calling `on_resize` with its new size between the bytes before and after it changed.
    pub fn take_reader_with(&mut self, on_resize: impl FnMut(u16, u16) + Send + 'static) -> Option<Box<dyn Read + Send>> {
        self.rx.take().map(|rx| Box::new(ChanReader { rx, buf: vec![], pos: 0, on_resize: Box::new(on_resize) }) as Box<dyn Read + Send>)
    }

    /// The session's input. Writes never block.
    pub fn writer(&self) -> Box<dyn Write + Send> {
        Box::new(ChanWriter { id: self.id.clone(), tx: self.client.inner.tx.clone() })
    }

    /// The size this client would like: the session takes it while this client controls it (typing into it takes
    /// control).
    pub fn resize(&self, rows: u16, cols: u16) {
        self.client.send(wire::json(wire::RESIZE, &Resize { id: self.id.clone(), rows, cols }));
    }

    /// This client controls the session (the last to type into it): its size is ours.
    pub fn controlling(&self) -> bool {
        self.slot.control.load(Ordering::SeqCst)
    }

    /// The session's size, as the server last said.
    pub fn size(&self) -> (u16, u16) {
        *self.slot.size.lock()
    }

    /// End the session and have the server forget it.
    pub fn close(&self) {
        self.client.close(&self.id);
    }

    /// What a client wants back when it attaches (`blob`, in the server's memory) and what brings the session back
    /// after the server stops (`keep`, written to disk: no secrets).
    pub fn set_blob(&self, blob: Value, keep: Value) {
        self.client.send(wire::json(wire::BLOB, &Blob { id: self.id.clone(), blob, keep }));
    }

    /// Some once the session exited: its code, or -1 when it had none (it was killed, or the server went).
    pub fn exit_code(&self) -> Option<i32> {
        self.slot.exit.lock().map(|c| c.unwrap_or(-1))
    }

    /// Stop watching this session here, leaving it running on the server.
    pub fn forget(&self) {
        self.client.inner.slots.lock().remove(&self.id);
        self.client.send(wire::json(wire::DETACH, &Id { id: self.id.clone() }));
    }
}

struct ChanReader {
    rx: Receiver<Item>,
    buf: Vec<u8>,
    pos: usize,
    on_resize: Box<dyn FnMut(u16, u16) + Send>,
}

impl Read for ChanReader {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        while self.pos >= self.buf.len() {
            match self.rx.recv() {
                Ok(Item::Data(chunk)) => {
                    self.buf = chunk;
                    self.pos = 0;
                }
                // everything before it was read already: the caller has processed it
                Ok(Item::Resize(rows, cols)) => (self.on_resize)(rows, cols),
                Err(_) => return Ok(0),
            }
        }
        let n = out.len().min(self.buf.len() - self.pos);
        out[..n].copy_from_slice(&self.buf[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

struct ChanWriter {
    id: String,
    tx: Sender<Vec<u8>>,
}

impl Write for ChanWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        self.tx.send(wire::data(wire::INPUT, &self.id, bytes)).map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "lost the session server"))?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// bro's folder for the server (`BRO_DIR` honoured).
pub fn default_dir() -> PathBuf {
    bro_core::paths::bro_dir()
}
