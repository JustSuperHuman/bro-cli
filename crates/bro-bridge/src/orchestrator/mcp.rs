//! `POST /mcp`: the orchestrator's tools as a Model Context Protocol server
//! (streamable HTTP, plain JSON responses, stateless). The headless Claude
//! Code process that runs the orchestrator connects here, so every tool call
//! executes in this process against the live session registry. Any other MCP
//! client on this machine (or with the bridge token) can use it too.

use super::{storyboard, tools};
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

/// The protocol revision we answer with when the client asks for one we do
/// not know.
const PROTOCOL_VERSION: &str = "2025-06-18";
const KNOWN_VERSIONS: [&str; 4] = ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"];

pub const SERVER_NAME: &str = "bro";

fn result(id: &Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error(id: &Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// Which tools an endpoint offers: every one (`/mcp`), or only the storyboard's (`/mcp/studio`, for Create Slop sessions).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Scope {
    All,
    Studio,
}

/// Answers one JSON-RPC message; `None` for notifications.
#[cfg(test)]
pub(crate) async fn handle_message(app: &AppState, message: &Value) -> Option<Value> {
    handle_message_for_session(app, message, None, Scope::All).await
}

/// [`handle_message`] on one of the endpoints.
#[cfg(test)]
pub(crate) async fn handle_message_for_scope(app: &AppState, message: &Value, scope: Scope) -> Option<Value> {
    handle_message_for_session(app, message, None, scope).await
}

async fn handle_message_for_session(app: &AppState, message: &Value, caller: Option<&str>, scope: Scope) -> Option<Value> {
    let id = message.get("id").cloned()?;
    let method = message.get("method").and_then(Value::as_str).unwrap_or("");
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    Some(match method {
        "initialize" => {
            let requested = params
                .get("protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or(PROTOCOL_VERSION);
            let version = if KNOWN_VERSIONS.contains(&requested) {
                requested
            } else {
                PROTOCOL_VERSION
            };
            result(
                &id,
                json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": { "name": SERVER_NAME, "version": env!("CARGO_PKG_VERSION") },
                    "instructions": match scope {
                        Scope::All => "Tools over the user's live bro terminal sessions.",
                        Scope::Studio => "The storyboard of this Create Slop project: the plan the user sees beside the session. Write it with storyboard_plan, attach pictures and takes with storyboard_scene, and read the user's changes with storyboard_get.",
                    }
                }),
            )
        }
        "ping" => result(&id, json!({})),
        "tools/list" => result(&id, json!({ "tools": if scope == Scope::Studio { storyboard::definitions() } else { tools::definitions() } })),
        "tools/call" => {
            let Some(name) = params.get("name").and_then(Value::as_str) else {
                return Some(error(&id, -32602, "tools/call needs a tool name."));
            };
            let mut arguments = params
                .get("arguments")
                .cloned()
                .filter(Value::is_object)
                .unwrap_or_else(|| json!({}));
            if scope == Scope::Studio && !storyboard::is_tool(name) {
                return Some(result(&id, json!({ "content": [{ "type": "text", "text": "That tool isn't part of the storyboard server." }], "isError": true })));
            }
            if let Some(token) = message.get("_broWorkerToken").and_then(Value::as_str) { arguments["workerToken"] = json!(token); }
            let worker_scoped = arguments.get("workerToken").is_some();
            let (text, is_error) = match tools::execute_with_worker_scope(app, name, &arguments, caller, worker_scoped).await {
                Ok(text) => (text, false),
                Err(message) => (message, true),
            };
            result(
                &id,
                json!({ "content": [{ "type": "text", "text": text }], "isError": is_error }),
            )
        }
        "resources/list" => result(&id, json!({ "resources": [] })),
        "prompts/list" => result(&id, json!({ "prompts": [] })),
        other => error(&id, -32601, &format!("Method not found: {other}")),
    })
}

pub(crate) async fn post(State(app): State<AppState>, headers: HeaderMap, Json(body): Json<Value>) -> Response {
    serve(app, headers, body, Scope::All).await
}

/// `POST /mcp/studio`: the same server, offering only the storyboard tools.
pub(crate) async fn post_studio(State(app): State<AppState>, headers: HeaderMap, Json(body): Json<Value>) -> Response {
    serve(app, headers, body, Scope::Studio).await
}

async fn serve(app: AppState, headers: HeaderMap, body: Value, scope: Scope) -> Response {
    let caller = headers.get("x-bro-session-id").and_then(|value| value.to_str().ok());
    let token = headers.get("x-bro-task-token").and_then(|value| value.to_str().ok());
    let with_token = |mut message:Value| { if let Some(token)=token { message["_broWorkerToken"] = json!(token); } message };
    match body {
        Value::Array(batch) => {
            let mut answers = Vec::new();
            for message in batch {
                if let Some(answer) = handle_message_for_session(&app, &with_token(message), caller, scope).await {
                    answers.push(answer);
                }
            }
            if answers.is_empty() {
                StatusCode::ACCEPTED.into_response()
            } else {
                Json(Value::Array(answers)).into_response()
            }
        }
        message => match handle_message_for_session(&app, &with_token(message), caller, scope).await {
            Some(answer) => Json(answer).into_response(),
            None => StatusCode::ACCEPTED.into_response(),
        },
    }
}

/// No server-initiated stream and no sessions to end.
pub(crate) async fn not_allowed() -> Response {
    StatusCode::METHOD_NOT_ALLOWED.into_response()
}
