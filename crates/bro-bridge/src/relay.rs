//! The account relay, on the computer's side: how a phone signed in to the person's Just Superhuman account
//! reaches this Bro from anywhere, with nothing here listening on the internet.
//!
//! When someone opens this computer from their account, the account server says so in the answer to the worker's
//! next ask for work ([`crate::justconnect`]), and Bro dials out to it: one WebSocket, the tunnel. The server then
//! sends each of the phone's requests down it as ordinary HTTP/1.1, every request (and every WebSocket) on a
//! stream of its own, and Bro serves them from the same router the phone would reach on the local network.
//!
//! A stream never touches a network port here. Each one is an in-memory pipe handed to the router through
//! [`Door`], so a relayed request is known to be one ([`RelayPeer`] in its extensions) by where it came from, not
//! by anything it says about itself: no header a phone sends can claim it, and nothing on the local network can
//! pretend to it. The account server has already checked who is asking and that they may open this computer; who
//! that is rides in the frame that opens the stream. Relayed requests are let in without the pairing token, are
//! never treated as coming from this computer, and are refused the few things that belong at the computer itself
//! ([`closed_to_relay`]).
//!
//! Frames are binary WebSocket messages: a type byte, the stream number (u32, big-endian), then the payload.
//! `OPEN` (server only, JSON saying who asked), `DATA`, `END` (no more from the sender), `RESET` (drop it),
//! `WINDOW` ("send me this many more bytes"). Each side may have [`WINDOW`] bytes of a stream in flight that the
//! other hasn't asked for, so a slow download holds up only itself.

use crate::state::AppState;
use axum::Json;
use axum::extract::connect_info::Connected;
use axum::extract::{ConnectInfo, Request};
use axum::http::{Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::serve::{IncomingStream, Listener};
use bro_core::jsh;
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::future::Future;
use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
use tokio::sync::{Semaphore, mpsc};
use tokio::task::AbortHandle;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

const OPEN: u8 = 1;
const DATA: u8 = 2;
const END: u8 = 3;
const RESET: u8 = 4;
const GRANT: u8 = 5;
/// Bytes of one stream a side may send before the other asks for more.
pub(crate) const WINDOW: usize = 256 * 1024;
/// The most a `DATA` frame carries (the server refuses larger messages).
const CHUNK: usize = 32 * 1024;
/// The server pings every 20 s; this long with nothing from it means the line is dead.
const SILENCE: Duration = Duration::from_secs(70);
/// Streams open at once on one tunnel; the server keeps its own, lower, count.
const MAX_STREAMS: usize = 128;

/// Who a relayed request is from, as the account server vouched for them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RelayPeer {
    pub user_id: String,
    pub name: String,
    pub email: String,
    /// Stands for the phone's sign-in without being it: the same for every request from one phone.
    pub client: String,
    /// One of Just Superhuman's own people, here to help.
    pub staff: bool,
    stream: u32,
}

impl RelayPeer {
    fn from_meta(meta: &[u8], stream: u32) -> RelayPeer {
        let meta: Value = serde_json::from_slice(meta).unwrap_or(Value::Null);
        let text = |value: &Value, max: usize| value.as_str().unwrap_or_default().chars().filter(|c| !c.is_control()).take(max).collect::<String>();
        RelayPeer {
            user_id: text(&meta["actor"]["id"], 128),
            name: text(&meta["actor"]["name"], 80),
            email: text(&meta["actor"]["email"], 254),
            client: text(&meta["client"], 32),
            staff: meta["staff"] == true,
            stream,
        }
    }

    /// An address to stand where a network peer's would: never this computer's, never a real one (it is in the
    /// range kept for private use), and the same for every request from one phone, so what matches a phone's
    /// connections by address (the desktop it is watching, say) still does.
    pub fn address(&self) -> SocketAddr {
        let mut tail = [0u16; 4];
        for (i, part) in tail.iter_mut().enumerate() {
            *part = self.client.get(i * 4..i * 4 + 4).and_then(|hex| u16::from_str_radix(hex, 16).ok()).unwrap_or(0);
        }
        SocketAddr::new(IpAddr::V6(Ipv6Addr::new(0xfd62, 0x726f, 0, 0, tail[0], tail[1], tail[2], tail[3])), (self.stream & 0xffff) as u16)
    }

    /// "James Greene" or their address: who to name beside the phone on the Remote Control page.
    pub fn label(&self) -> String {
        let who = if self.name.trim().is_empty() { self.email.trim() } else { self.name.trim() };
        if who.is_empty() { "Someone on your account".into() } else { who.to_string() }
    }
}

/// Whether `address` is one [`RelayPeer::address`] makes.
pub fn is_relay_address(address: IpAddr) -> bool {
    matches!(address, IpAddr::V6(v6) if v6.segments()[0] == 0xfd62 && v6.segments()[1] == 0x726f)
}

/// What stays at the computer: its own sign-in and account settings, and the endpoints meant for programs
/// running here (agents' tools), which a person on a phone has no use for.
pub(crate) fn closed_to_relay(method: &Method, path: &str) -> bool {
    path == "/api/jsh/login"
        || path == "/api/jsh/logout"
        || (path.starts_with("/api/justconnect/") && method != Method::GET)
        || path == "/mcp"
        || path.starts_with("/mcp/")
        || path.starts_with("/api/mod/")
}

/// The first thing a relayed request passes: it is marked as one, given its stand-in address, and turned back if
/// it asks for something that stays at the computer.
async fn door(mut request: Request, next: Next) -> Response {
    let Some(ConnectInfo(peer)) = request.extensions().get::<ConnectInfo<RelayPeer>>().cloned() else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    if closed_to_relay(request.method(), request.uri().path()) {
        return (StatusCode::FORBIDDEN, Json(json!({ "message": "That can only be done at the computer itself." }))).into_response();
    }
    request.extensions_mut().insert(ConnectInfo(peer.address()));
    request.extensions_mut().insert(peer);
    next.run(request).await
}

/// Where the tunnel hands its streams to the router.
#[derive(Clone)]
pub(crate) struct Door(mpsc::Sender<(DuplexStream, RelayPeer)>);

struct Streams(mpsc::Receiver<(DuplexStream, RelayPeer)>);

impl Listener for Streams {
    type Io = DuplexStream;
    type Addr = RelayPeer;

    async fn accept(&mut self) -> (DuplexStream, RelayPeer) {
        match self.0.recv().await {
            Some(stream) => stream,
            // every door is gone: nothing more will ever arrive
            None => std::future::pending().await,
        }
    }

    fn local_addr(&self) -> std::io::Result<RelayPeer> {
        Ok(RelayPeer::default())
    }
}

impl Connected<IncomingStream<'_, Streams>> for RelayPeer {
    fn connect_info(stream: IncomingStream<'_, Streams>) -> Self {
        stream.remote_addr().clone()
    }
}

/// Starts serving relayed streams from this bridge's router, until Bro shuts down.
pub(crate) fn open_door(state: &AppState) -> Door {
    let (sender, streams) = mpsc::channel(64);
    let app = crate::server::router(state.clone()).layer(axum::middleware::from_fn(door));
    let mut shutdown = state.shutdown.subscribe();
    tokio::spawn(async move {
        let served = axum::serve(Streams(streams), app.into_make_service_with_connect_info::<RelayPeer>())
            .with_graceful_shutdown(async move {
                let _ = shutdown.wait_for(|stopped| *stopped).await;
            })
            .await;
        if let Err(error) = served {
            tracing::warn!("bro-bridge: the account relay stopped serving: {error}");
        }
    });
    Door(sender)
}

fn frame(kind: u8, id: u32, payload: &[u8]) -> Message {
    let mut bytes = Vec::with_capacity(5 + payload.len());
    bytes.push(kind);
    bytes.extend_from_slice(&id.to_be_bytes());
    bytes.extend_from_slice(payload);
    Message::Binary(bytes.into())
}

/// `wss://<account server>/api/justconnect/v1/computers/<id>/relay`
fn tunnel_url(origin: &str, computer: &str) -> String {
    let origin = origin.trim_end_matches('/');
    let base = origin
        .strip_prefix("https://")
        .map(|rest| format!("wss://{rest}"))
        .or_else(|| origin.strip_prefix("http://").map(|rest| format!("ws://{rest}")))
        .unwrap_or_else(|| origin.to_owned());
    format!("{base}/api/justconnect/v1/computers/{computer}/relay")
}

fn tls() -> tokio_tungstenite::Connector {
    // An explicit crypto provider: bro links rustls with both ring and aws-lc-rs, and rustls refuses to choose.
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .map(|builder| builder.with_root_certificates(roots).with_no_client_auth())
        .expect("ring supports the default TLS versions");
    tokio_tungstenite::Connector::Rustls(Arc::new(config))
}

/// How a tunnel ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Ended {
    /// The server wouldn't have it (this sign-in isn't this computer's, access is off): don't dial again unasked.
    Refused(u16),
    /// It never connected.
    Unreachable,
    /// It was open and closed: idle, the server restarted, the line dropped, or `stop` fired.
    Closed,
}

/// One stream's place in the tunnel.
struct Stream {
    /// What the server sends for it, in order; `None` is its `END`.
    down: mpsc::UnboundedSender<Option<Bytes>>,
    /// Bytes we may still send: the server's `WINDOW` frames add to it.
    credit: Arc<Semaphore>,
    tasks: [AbortHandle; 2],
    got_end: bool,
    up_done: bool,
}

impl Stream {
    fn drop_now(self) {
        for task in self.tasks {
            task.abort();
        }
    }
}

/// Dials the tunnel as `stored` for `computer` and carries streams through `door` until it closes or `stop`
/// completes.
pub(crate) async fn run(door: &Door, stored: &jsh::Stored, computer: &str, stop: impl Future<Output = ()>) -> Ended {
    let Ok(mut request) = tunnel_url(&stored.origin, computer).into_client_request() else {
        return Ended::Unreachable;
    };
    let Ok(bearer) = format!("Bearer {}", stored.token).parse() else {
        return Ended::Unreachable;
    };
    request.headers_mut().insert("Authorization", bearer);
    let connector = request.uri().scheme_str().is_some_and(|scheme| scheme == "wss").then(tls);
    let dialed = tokio::time::timeout(Duration::from_secs(20), tokio_tungstenite::connect_async_tls_with_config(request, None, true, connector)).await;
    let socket = match dialed {
        Ok(Ok((socket, _))) => socket,
        Ok(Err(tokio_tungstenite::tungstenite::Error::Http(response))) => return Ended::Refused(response.status().as_u16()),
        _ => return Ended::Unreachable,
    };
    let (mut sink, mut source) = socket.split();
    // every frame out goes through one writer, in the order it was queued
    let (out, mut outbox) = mpsc::channel::<Message>(256);
    let writer = tokio::spawn(async move {
        while let Some(message) = outbox.recv().await {
            if sink.send(message).await.is_err() {
                break;
            }
        }
        let _ = sink.close().await;
    });
    let (done, mut finished) = mpsc::unbounded_channel::<u32>();
    let mut streams: HashMap<u32, Stream> = HashMap::new();
    tokio::pin!(stop);
    loop {
        let message = tokio::select! {
            _ = &mut stop => break,
            Some(id) = finished.recv() => {
                // its bytes to the server are all sent; once the server's are all in, there is nothing left of it
                if let Some(stream) = streams.get_mut(&id) {
                    stream.up_done = true;
                    if stream.got_end {
                        streams.remove(&id);
                    }
                }
                continue;
            }
            message = tokio::time::timeout(SILENCE, source.next()) => message,
        };
        let data = match message {
            Ok(Some(Ok(Message::Binary(data)))) => data,
            Ok(Some(Ok(Message::Close(_)))) | Ok(Some(Err(_))) | Ok(None) | Err(_) => break,
            // pings are answered by the socket itself; nothing else is expected
            Ok(Some(Ok(_))) => continue,
        };
        if data.len() < 5 {
            continue;
        }
        let (kind, id, payload) = (data[0], u32::from_be_bytes([data[1], data[2], data[3], data[4]]), data.slice(5..));
        match kind {
            OPEN => {
                if streams.len() >= MAX_STREAMS || streams.contains_key(&id) {
                    let _ = out.send(frame(RESET, id, &[])).await;
                    continue;
                }
                let (ours, theirs) = tokio::io::duplex(64 * 1024);
                if door.0.try_send((theirs, RelayPeer::from_meta(&payload, id))).is_err() {
                    let _ = out.send(frame(RESET, id, &[])).await;
                    continue;
                }
                let (mut read, mut write) = tokio::io::split(ours);
                let credit = Arc::new(Semaphore::new(WINDOW));
                let (down, mut inbox) = mpsc::unbounded_channel::<Option<Bytes>>();
                // server → router: write what arrives, and ask for more as it is taken
                let asks = out.clone();
                let to_router = tokio::spawn(async move {
                    let mut taken = 0usize;
                    while let Some(Some(chunk)) = inbox.recv().await {
                        if write.write_all(&chunk).await.is_err() {
                            break;
                        }
                        taken += chunk.len();
                        if taken >= WINDOW / 4 {
                            let _ = asks.send(frame(GRANT, id, &(taken as u32).to_be_bytes())).await;
                            taken = 0;
                        }
                    }
                    let _ = write.shutdown().await;
                });
                // router → server: no more than the server has asked for
                let (frames, allowance, told) = (out.clone(), credit.clone(), done.clone());
                let to_server = tokio::spawn(async move {
                    let mut buffer = vec![0u8; CHUNK];
                    loop {
                        match read.read(&mut buffer).await {
                            Ok(0) => {
                                let _ = frames.send(frame(END, id, &[])).await;
                                break;
                            }
                            Ok(n) => {
                                let Ok(permit) = allowance.acquire_many(n as u32).await else { break };
                                permit.forget();
                                if frames.send(frame(DATA, id, &buffer[..n])).await.is_err() {
                                    break;
                                }
                            }
                            Err(_) => {
                                let _ = frames.send(frame(RESET, id, &[])).await;
                                break;
                            }
                        }
                    }
                    let _ = told.send(id);
                });
                streams.insert(id, Stream { down, credit, tasks: [to_router.abort_handle(), to_server.abort_handle()], got_end: false, up_done: false });
            }
            DATA => {
                if let Some(stream) = streams.get(&id) {
                    let _ = stream.down.send(Some(payload));
                }
            }
            END => {
                if let Some(stream) = streams.get_mut(&id) {
                    let _ = stream.down.send(None);
                    stream.got_end = true;
                    if stream.up_done {
                        streams.remove(&id);
                    }
                }
            }
            RESET => {
                if let Some(stream) = streams.remove(&id) {
                    stream.drop_now();
                }
            }
            GRANT if payload.len() >= 4 => {
                if let Some(stream) = streams.get(&id) {
                    let more = u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]) as usize;
                    // a server that grants without end doesn't get to overflow the count
                    if stream.credit.available_permits() < WINDOW * 64 {
                        stream.credit.add_permits(more.min(WINDOW * 16));
                    }
                }
            }
            _ => {}
        }
    }
    // the tunnel is gone: every request it carried ends here
    for (_, stream) in streams.drain() {
        stream.drop_now();
    }
    drop(out);
    writer.abort();
    Ended::Closed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_belongs_at_the_computer_is_closed_to_the_relay() {
        for (method, path) in [
            (Method::POST, "/api/jsh/login"),
            (Method::POST, "/api/jsh/logout"),
            (Method::POST, "/api/justconnect/disable"),
            (Method::POST, "/api/justconnect/select"),
            (Method::POST, "/mcp"),
            (Method::POST, "/mcp/studio"),
            (Method::GET, "/api/mod/abc/jobs"),
        ] {
            assert!(closed_to_relay(&method, path), "{method} {path}");
        }
        for (method, path) in [
            (Method::GET, "/api/bootstrap"),
            (Method::GET, "/ws"),
            (Method::GET, "/api/jsh"),
            (Method::GET, "/api/justconnect"),
            (Method::POST, "/api/sessions"),
            (Method::POST, "/api/sessions/abc/write"),
            (Method::POST, "/api/rmm/sessions"),
            (Method::GET, "/rmm/abc/ws"),
            (Method::POST, "/api/voice/calls"),
        ] {
            assert!(!closed_to_relay(&method, path), "{method} {path}");
        }
    }

    #[test]
    fn a_relayed_peer_is_never_this_computer_and_one_phone_keeps_one_address() {
        let meta = br#"{"actor":{"id":"u1","name":"James Greene","email":"james@example.com"},"client":"0123456789abcdef","staff":true}"#;
        let first = RelayPeer::from_meta(meta, 7);
        let second = RelayPeer::from_meta(meta, 70_000);
        assert_eq!(first.address().ip(), second.address().ip(), "the same phone");
        assert_ne!(first.address(), second.address(), "two of its requests");
        assert!(!crate::auth::is_loopback(first.address()));
        assert!(is_relay_address(first.address().ip()));
        assert!(!is_relay_address("192.168.1.20".parse().unwrap()));
        assert!(!is_relay_address("::1".parse().unwrap()));
        assert_eq!(first.address().ip().to_string(), "fd62:726f::123:4567:89ab:cdef");
        assert!(first.staff);
        assert_eq!(first.label(), "James Greene");
        let other = RelayPeer::from_meta(br#"{"actor":{"email":"sam@example.com"},"client":"ffffffffffffffff"}"#, 1);
        assert_ne!(other.address().ip(), first.address().ip(), "another phone");
        assert_eq!(other.label(), "sam@example.com");
        // nothing useful in the frame still makes a peer, and still not a local one
        let blank = RelayPeer::from_meta(b"not json", 1);
        assert_eq!(blank.label(), "Someone on your account");
        assert!(!crate::auth::is_loopback(blank.address()));
    }

    #[test]
    fn the_tunnel_is_dialed_on_the_account_server_over_the_same_scheme() {
        assert_eq!(tunnel_url("https://console.justsuperhuman.com", "abc"), "wss://console.justsuperhuman.com/api/justconnect/v1/computers/abc/relay");
        assert_eq!(tunnel_url("http://127.0.0.1:8790/", "abc"), "ws://127.0.0.1:8790/api/justconnect/v1/computers/abc/relay");
    }
}
