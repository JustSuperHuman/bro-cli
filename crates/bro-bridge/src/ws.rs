//! The `/ws` client protocol.
//!
//! On connect the server sends `hello`. Clients `subscribe` a session into a
//! slot (`main` by default) and get its `snapshot`, then live `output` for
//! every subscribed session and a data-free `activity` ping for the rest.
//! Global events (`sessions`, `session`, `projects`, `profiles`, `notify`,
//! `orchestrator*`, `exit`) go to everyone. Client actions become
//! [`crate::BridgeCommand`]s.

use crate::BridgeCommand;
use crate::commands::launch_terminal;
use crate::model::ClientMessage;
use crate::state::{AppState, COMMAND_INPUT, COMMAND_KILL, COMMAND_RESIZE};
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use tokio::sync::{broadcast, mpsc};

pub(crate) async fn upgrade(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.on_upgrade(move |socket| client_socket(socket, state))
}

/// Keeps the connected-client count accurate however the loop exits.
struct ClientGuard(AppState);

impl ClientGuard {
    fn new(state: &AppState) -> Self {
        state.clients.fetch_add(1, Ordering::Relaxed);
        Self(state.clone())
    }
}

impl Drop for ClientGuard {
    fn drop(&mut self) {
        self.0.clients.fetch_sub(1, Ordering::Relaxed);
    }
}

fn text(value: &Value) -> Message {
    Message::Text(value.to_string().into())
}

async fn client_socket(socket: WebSocket, state: AppState) {
    let _guard = ClientGuard::new(&state);
    let (mut sender, mut receiver) = socket.split();
    // Subscribe before `hello` so nothing published after it is missed.
    let mut events = state.events.subscribe();
    let mut shutdown = state.shutdown.subscribe();
    if *shutdown.borrow() || sender.send(text(&state.hello())).await.is_err() {
        return;
    }
    // Per-connection replies from background tasks (create results).
    let (direct_tx, mut direct_rx) = mpsc::unbounded_channel::<Value>();
    let mut subscriptions: HashMap<String, String> = HashMap::new();
    // Output at or below this seq is already contained in the snapshot sent.
    let mut snapshot_seq: HashMap<String, u64> = HashMap::new();
    loop {
        tokio::select! {
            incoming = receiver.next() => {
                let message = match incoming {
                    Some(Ok(Message::Text(message))) => message,
                    // Axum answers protocol pings; control frames are keepalives.
                    Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Binary(_))) => continue,
                    _ => break,
                };
                let Ok(message) = serde_json::from_str::<ClientMessage>(&message) else { continue };
                let reply = handle_message(&state, message, &mut subscriptions, &mut snapshot_seq, &direct_tx);
                if let Some(reply) = reply
                    && sender.send(text(&reply)).await.is_err()
                {
                    break;
                }
            }
            direct = direct_rx.recv() => {
                let Some(direct) = direct else { continue };
                if sender.send(text(&direct)).await.is_err() { break; }
            }
            event = events.recv() => {
                let event = match event {
                    Ok(event) => event,
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        // Missed events: resynchronise the session list.
                        let sessions = json!({ "type": "sessions", "sessions": state.summaries() });
                        if sender.send(text(&sessions)).await.is_err() { break; }
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                };
                let watched = event
                    .session_id
                    .as_ref()
                    .is_none_or(|id| subscriptions.values().any(|value| value == id));
                let value = if event.output && !watched {
                    json!({ "type": "activity", "sessionId": event.session_id, "seq": event.value["seq"] })
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
                if sender.send(text(&value)).await.is_err() { break; }
            }
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    let _ = sender.send(Message::Close(None)).await;
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
    subscriptions: &mut HashMap<String, String>,
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
            subscriptions.remove(&slot.unwrap_or_else(|| "main".into()));
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
