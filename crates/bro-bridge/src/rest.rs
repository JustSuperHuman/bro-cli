//! Non-session REST handlers: bootstrap/health, notifications, projects,
//! orchestrator, and the unavailable ACP / peer adapters.

use crate::model::{TerminalNotification, parse_since};
use crate::orchestrator;
use crate::state::AppState;
use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use std::collections::HashMap;

/// `{ "message": ... }` with a status, the error shape every client reads.
pub(crate) fn api_error(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(json!({ "message": message.into() }))).into_response()
}

pub(crate) async fn bootstrap(State(state): State<AppState>) -> Json<Value> {
    Json(state.bootstrap())
}

pub(crate) async fn acp(State(state): State<AppState>) -> Json<Value> {
    Json(state.inner.lock().acp.clone())
}

pub(crate) async fn health(State(state): State<AppState>) -> Json<Value> {
    let port = state.inner.lock().port;
    Json(json!({
        "ok": true,
        "runtime": "bro",
        "sessions": state.summaries().len(),
        "endpoint": format!("127.0.0.1:{port}"),
        "orchestrator": true
    }))
}

pub(crate) async fn notifications(
    Query(query): Query<HashMap<String, String>>,
    State(state): State<AppState>,
) -> Json<Value> {
    Json(json!(state.notification_history(parse_since(
        query.get("since").map(String::as_str)
    ))))
}

/// Fire-and-forget notification from hooks. Fields come from an optional
/// JSON body or the query string (`curl -X POST ".../api/notify?sound=done"`).
pub(crate) async fn notify(
    Query(query): Query<HashMap<String, String>>,
    State(state): State<AppState>,
    body: Bytes,
) -> Response {
    let body: Value = if body.iter().all(u8::is_ascii_whitespace) {
        Value::Null
    } else {
        match serde_json::from_slice(&body) {
            Ok(value) => value,
            Err(_) => {
                return api_error(
                    StatusCode::BAD_REQUEST,
                    "The notification body must be JSON.",
                );
            }
        }
    };
    let field = |key: &str| {
        body.get(key)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| query.get(key).cloned())
            .filter(|value| !value.is_empty())
    };
    let session = field("sessionId").and_then(|id| state.summary(&id));
    state.notify(
        TerminalNotification {
            id: String::new(),
            at: String::new(),
            origin: "api".into(),
            session_id: session.as_ref().map(|session| session.id.clone()),
            session_title: session.map(|session| session.title),
            title: field("title"),
            body: field("body"),
            sound: field("sound"),
        },
        true,
    );
    StatusCode::NO_CONTENT.into_response()
}

pub(crate) async fn projects(State(state): State<AppState>) -> Json<Value> {
    Json(json!(state.projects()))
}

pub(crate) async fn create_project(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Response {
    let cwd = body
        .get("cwd")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
    if cwd.is_empty() {
        return api_error(StatusCode::BAD_REQUEST, "A project directory is required.");
    }
    let project = state.create_project(body.get("name").and_then(Value::as_str), cwd);
    (StatusCode::CREATED, Json(project)).into_response()
}

pub(crate) async fn rename_project(
    Path(id): Path<String>,
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Response {
    let name = body.get("name").and_then(Value::as_str).unwrap_or_default();
    if name.trim().is_empty() {
        return api_error(StatusCode::BAD_REQUEST, "A project name is required.");
    }
    match state.rename_project(&id, name) {
        Some(project) => Json(project).into_response(),
        None => api_error(StatusCode::NOT_FOUND, "Unknown project."),
    }
}

pub(crate) async fn reorder_projects(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Response {
    let ids: Vec<String> = body
        .get("ids")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect();
    state.reorder_projects(ids);
    Json(json!(state.projects())).into_response()
}

pub(crate) async fn recent_projects(State(state): State<AppState>) -> Json<Value> {
    Json(json!(state.recent_projects()))
}

pub(crate) async fn delete_project(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Response {
    state.delete_project(&id);
    StatusCode::NO_CONTENT.into_response()
}

pub(crate) async fn orchestrator_status(
    Query(query): Query<HashMap<String, String>>,
    State(state): State<AppState>,
) -> Json<Value> {
    let since = query
        .get("since")
        .and_then(|value| value.parse::<u64>().ok());
    Json(state.orchestrator.status(since, true))
}

pub(crate) async fn orchestrator_config(State(state): State<AppState>) -> Json<Value> {
    Json(state.orchestrator.public_config())
}

pub(crate) async fn orchestrator_update_config(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Response {
    match state.orchestrator.update_config(&body) {
        Ok(config) => Json(config).into_response(),
        Err(message) => api_error(StatusCode::BAD_REQUEST, message),
    }
}

pub(crate) async fn orchestrator_models(State(state): State<AppState>) -> Json<Value> {
    Json(state.orchestrator.models())
}

pub(crate) async fn orchestrator_send(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Response {
    let text = body
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    match orchestrator::send_message(&state, text).await {
        Ok(status) => (StatusCode::ACCEPTED, Json(status)).into_response(),
        Err((code, message)) => api_error(
            StatusCode::from_u16(code).unwrap_or(StatusCode::BAD_REQUEST),
            message,
        ),
    }
}

pub(crate) async fn orchestrator_clear(State(state): State<AppState>) -> Json<Value> {
    state.orchestrator.clear();
    Json(state.orchestrator.status(None, true))
}

pub(crate) async fn orchestrator_cancel(State(state): State<AppState>) -> Json<Value> {
    let cancelled = state.orchestrator.cancel();
    let mut status = state.orchestrator.status(None, false);
    status["cancelled"] = json!(cancelled);
    Json(status)
}

pub(crate) async fn orchestrator_test(State(state): State<AppState>) -> Json<Value> {
    Json(state.orchestrator.test_connection().await)
}

/// Routes of the retired TUI-based orchestrator.
pub(crate) async fn orchestrator_retired() -> Response {
    api_error(
        StatusCode::GONE,
        "The orchestrator is now a built-in chat agent: send messages to POST /api/orchestrator/messages.",
    )
}

/// ACP, agent attach and peer hosts are not part of bro.
pub(crate) async fn unavailable() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({
            "message": "ACP agent sessions are not available in bro's bridge. Use the terminal session directly; agents run in bro's own terminals.",
            "error": "unavailable",
            "available": false
        })),
    )
        .into_response()
}

/// bro is a single host; there are no peer hosts to proxy to.
pub(crate) async fn peers_unavailable() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({
            "message": "Peer hosts are not available in bro's bridge; every session is served by this host.",
            "error": "unavailable",
            "available": false
        })),
    )
        .into_response()
}
