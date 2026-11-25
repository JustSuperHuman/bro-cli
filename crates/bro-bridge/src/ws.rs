//! The `/ws` client protocol.
//!
//! On connect the server sends `hello`. Clients `subscribe` a session into a
//! slot (`main` by default) and get its `snapshot`, then live `output` for
//! every subscribed session and a data-free `activity` ping for the rest.
//! Global events (`sessions`, `session`, `projects`, `profiles`, `hosts`,
//! `notify`, `orchestrator*`, `exit`) go to everyone. Client actions become
//! [`crate::BridgeCommand`]s.

use crate::BridgeCommand;
use crate::commands::launch_terminal;
use crate::model::ClientMessage;
use crate::state::{AppState, COMMAND_INPUT, COMMAND_KILL, COMMAND_RESIZE};
use axum::extract::rejection::ExtensionRejection;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, Uri, header};
use axum::response::Response;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::Ordering;
use tokio::sync::{broadcast, mpsc};

pub(crate) async fn upgrade(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    peer: Result<ConnectInfo<SocketAddr>, ExtensionRejection>,
    relayed: Option<axum::Extension<crate::relay::RelayPeer>>,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    let user_agent = headers.get(header::USER_AGENT).and_then(|v| v.to_str().ok());
    let about = crate::devices::describe(uri.query(), user_agent);
    let address = peer.ok().map(|ConnectInfo(peer)| peer.ip().to_canonical());
    // through the account: the Remote Control page names the person beside their phone
    let via = relayed.map(|axum::Extension(peer)| if peer.staff { format!("{} (Just Superhuman support)", peer.label()) } else { peer.label() });
    ws.on_upgrade(move |socket| client_socket(socket, state, about, address, via))
}

/// Keeps the connected-client count and the device list accurate however the loop exits.
struct ClientGuard(AppState, u64);

impl ClientGuard {
    fn new(state: &AppState, about: crate::devices::About, address: Option<IpAddr>, via: Option<String>) -> Self {
        state.clients.fetch_add(1, Ordering::Relaxed);
        let id = state.devices.add(about, address, via);
        Self(state.clone(), id)
    }
}

impl Drop for ClientGuard {
    fn drop(&mut self) {
        self.0.clients.fetch_sub(1, Ordering::Relaxed);
        self.0.devices.remove(self.1);
    }
}

/// This connection's slot -> session subscriptions. Keeps [`AppState::viewers`] in step
/// on every change and releases them all when the connection ends, however it ends.
struct Subscriptions {
    state: AppState,
    slots: HashMap<String, String>,
}

impl Subscriptions {
    fn new(state: &AppState) -> Self {
        Self {
            state: state.clone(),
            slots: HashMap::new(),
        }
    }

    fn insert(&mut self, slot: String, session_id: String) {
        self.state.add_viewer(&session_id);
        if let Some(previous) = self.slots.insert(slot, session_id) {
            self.state.remove_viewer(&previous);
        }
    }

    fn remove(&mut self, slot: &str) {
        if let Some(previous) = self.slots.remove(slot) {
            self.state.remove_viewer(&previous);
        }
    }

    fn watches(&self, session_id: &str) -> bool {
        self.slots.values().any(|value| value == session_id)
    }
}

impl Drop for Subscriptions {
    fn drop(&mut self) {
        for session_id in self.slots.values() {
            self.state.remove_viewer(session_id);
        }
    }
}

fn text(value: &Value) -> Message {
    Message::Text(value.to_string().into())
}

/// How often a connection is pinged. A client whose network is gone (a phone out of range, a laptop lid shut)
/// never says goodbye; writing to it is what makes the system notice, about a minute later.
const PING_EVERY: std::time::Duration = std::time::Duration::from_secs(30);
/// A client that takes the pings and answers none for this long is closed. A phone app the system has frozen in
/// the background looks like this: its connection stays up while nothing in it runs. Long, because a phone that
/// comes back within minutes resumes on the connection it had, without a new hello and snapshot.
const SILENT_LIMIT: std::time::Duration = std::time::Duration::from_secs(600);
/// A client that can't take one message in this long is gone or hopelessly behind.
const SEND_LIMIT: std::time::Duration = std::time::Duration::from_secs(30);

/// Whether a connection that last said anything at `heard` has been silent for `limit` at `now`.
fn gone_silent(heard: tokio::time::Instant, now: tokio::time::Instant, limit: std::time::Duration) -> bool {
    now.saturating_duration_since(heard) >= limit
}

/// How often to ping and how long silence may last, for connections to the bridge on `port`.
fn liveness(_port: u16) -> (std::time::Duration, std::time::Duration) {
    #[cfg(test)]
    if let Some(short) = TEST_LIVENESS.lock().get(&_port) {
        return *short;
    }
    (PING_EVERY, SILENT_LIMIT)
}

/// Tests shorten the clock for their own bridge (each has its own port), leaving every other test's alone.
#[cfg(test)]
static TEST_LIVENESS: parking_lot::Mutex<std::collections::BTreeMap<u16, (std::time::Duration, std::time::Duration)>> = parking_lot::Mutex::new(std::collections::BTreeMap::new());

#[cfg(test)]
pub(crate) fn test_liveness(port: u16, ping_every: std::time::Duration, silent_limit: std::time::Duration) {
    TEST_LIVENESS.lock().insert(port, (ping_every, silent_limit));
}

/// Sends one message; false when the client is gone (the send failed, or it took longer than [`SEND_LIMIT`]).
async fn deliver(sender: &mut futures_util::stream::SplitSink<WebSocket, Message>, message: Message) -> bool {
    matches!(tokio::time::timeout(SEND_LIMIT, sender.send(message)).await, Ok(Ok(())))
}

async fn client_socket(socket: WebSocket, state: AppState, about: crate::devices::About, address: Option<IpAddr>, via: Option<String>) {
    let _guard = ClientGuard::new(&state, about, address, via);
    let (mut sender, mut receiver) = socket.split();
    // Subscribe before `hello` so nothing published after it is missed.
    let mut events = state.events.subscribe();
    let mut shutdown = state.shutdown.subscribe();
    if *shutdown.borrow() || !deliver(&mut sender, text(&state.hello())).await {
        return;
    }
    // `hello` carried the last scan; a stale one is redone and arrives as `hosts`.
    crate::hosts::refresh_soon(&state);
    // Per-connection replies from background tasks (create results).
    let (direct_tx, mut direct_rx) = mpsc::unbounded_channel::<Value>();
    let mut subscriptions = Subscriptions::new(&state);
    // Output at or below this seq is already contained in the snapshot sent.
    let mut snapshot_seq: HashMap<String, u64> = HashMap::new();
    // Offscreen sessions need a freshness signal, not a frame for every PTY write.
    let mut activity: HashMap<String, Value> = HashMap::new();
    let mut activity_tick = tokio::time::interval_at(tokio::time::Instant::now() + std::time::Duration::from_millis(500), std::time::Duration::from_millis(500));
    activity_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // Viewers and the device list are only right while every connection counted is a live one.
    let mut heard = tokio::time::Instant::now();
    let (ping_every, silent_limit) = liveness(state.inner.lock().port);
    let mut ping_tick = tokio::time::interval_at(heard + ping_every, ping_every);
    ping_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            incoming = receiver.next() => {
                // anything at all from the client (a message, a pong) says it is there
                heard = tokio::time::Instant::now();
                let message = match incoming {
                    Some(Ok(Message::Text(message))) => message,
                    // Axum answers protocol pings; control frames are keepalives.
                    Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Binary(_))) => continue,
                    _ => break,
                };
                let Ok(message) = serde_json::from_str::<ClientMessage>(&message) else { continue };
                let reply = handle_message(&state, message, &mut subscriptions, &mut snapshot_seq, &direct_tx);
                if let Some(reply) = reply
                    && !deliver(&mut sender, text(&reply)).await
                {
                    break;
                }
            }
            direct = direct_rx.recv() => {
                let Some(direct) = direct else { continue };
                if !deliver(&mut sender, text(&direct)).await { break; }
            }
            event = events.recv() => {
                let event = match event {
                    Ok(event) => event,
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        // Missed events: resynchronise the session list.
                        let sessions = json!({ "type": "sessions", "sessions": state.summaries() });
                        if !deliver(&mut sender, text(&sessions)).await { break; }
                        // Terminal bytes were lost too. Restore each visible screen, then discard
                        // queued output already represented by that snapshot's sequence.
                        let ids: std::collections::HashSet<_> = subscriptions.slots.values().cloned().collect();
                        for id in ids {
                            if let Some((snapshot, seq)) = state.snapshot_with_seq(&id) {
                                snapshot_seq.insert(id, seq);
                                if !deliver(&mut sender, text(&snapshot)).await { return; }
                            }
                        }
                        activity.clear();
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                };
                let watched = event
                    .session_id
                    .as_ref()
                    .is_none_or(|id| subscriptions.watches(id));
                let value = if event.output && !watched {
                    if let Some(id) = event.session_id {
                        activity.insert(id, event.value["seq"].clone());
                    }
                    continue;
                } else if event.output {
                    let id = event.session_id.as_deref().unwrap_or_default();
                    let seq = event.value["seq"].as_u64().unwrap_or(u64::MAX);
                    if snapshot_seq.get(id).is_some_and(|covered| seq <= *covered) {
                        continue;
                    }
                    event.value
                } else {
                    event.value
                };
                if !deliver(&mut sender, text(&value)).await { break; }
            }
            _ = ping_tick.tick() => {
                if gone_silent(heard, tokio::time::Instant::now(), silent_limit) || !deliver(&mut sender, Message::Ping(Default::default())).await {
                    break;
                }
            }
            _ = activity_tick.tick(), if !activity.is_empty() => {
                for (id, seq) in std::mem::take(&mut activity) {
                    if !subscriptions.watches(&id) && !deliver(&mut sender, text(&json!({ "type": "activity", "sessionId": id, "seq": seq }))).await {
                        return;
                    }
                }
            }
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    let _ = deliver(&mut sender, Message::Close(None)).await;
                    break;
                }
            }
        }
    }
}

/// Applies one client message; returns an immediate reply for this client.
fn handle_message(
    state: &AppState,
    message: ClientMessage,
    subscriptions: &mut Subscriptions,
    snapshot_seq: &mut HashMap<String, u64>,
    direct: &mpsc::UnboundedSender<Value>,
) -> Option<Value> {
    match message {
        ClientMessage::Ping => Some(json!({ "type": "pong" })),
        ClientMessage::Subscribe { session_id, slot } => {
            subscriptions.insert(slot.unwrap_or_else(|| "main".into()), session_id.clone());
            let (snapshot, seq) = state.snapshot_with_seq(&session_id)?;
            snapshot_seq.insert(session_id, seq);
            Some(snapshot)
        }
        ClientMessage::Unsubscribe { slot } => {
            subscriptions.remove(slot.as_deref().unwrap_or("main"));
            None
        }
        ClientMessage::Input { session_id, data } => {
            let _ = state.dispatch(&session_id, COMMAND_INPUT, &data, 0, 0);
            None
        }
        ClientMessage::Resize {
            session_id,
            cols,
            rows,
        } => {
            let _ = state.dispatch(&session_id, COMMAND_RESIZE, "", rows, cols);
            None
        }
        ClientMessage::Rename { session_id, title } => {
            if state.rename(&session_id, title.clone()).is_some() && !title.trim().is_empty() {
                state.send_command(BridgeCommand::Rename {
                    id: session_id,
                    title,
                });
            }
            None
        }
        ClientMessage::Kill { session_id } => {
            let _ = state.dispatch(&session_id, COMMAND_KILL, "", 0, 0);
            None
        }
        ClientMessage::RefreshHost => {
            state.publish_profiles();
            state.publish_sessions();
            None
        }
        ClientMessage::Create {
            title,
            profile_id,
            shell,
            args,
            cwd,
            project_id,
        } => {
            let state = state.clone();
            let direct = direct.clone();
            let body = json!({
                "title": title, "profileId": profile_id, "shell": shell,
                "args": args, "cwd": cwd, "projectId": project_id
            });
            tokio::spawn(async move {
                if let Err(message) = launch_terminal(&state, body).await {
                    let _ = direct.send(json!({ "type": "error", "message": message }));
                }
            });
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_is_measured_from_the_last_thing_heard() {
        let heard = tokio::time::Instant::now();
        assert!(!gone_silent(heard, heard + SILENT_LIMIT - std::time::Duration::from_secs(1), SILENT_LIMIT));
        assert!(gone_silent(heard, heard + SILENT_LIMIT, SILENT_LIMIT));
        // a phone back within minutes keeps its connection: the limit is many pings long
        assert!(SILENT_LIMIT >= PING_EVERY * 10);
    }
}
