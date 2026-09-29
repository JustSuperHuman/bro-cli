//! `/api/sessions/*` handlers. Same routes, bodies and status codes as the
//! reference host; actions become [`crate::BridgeCommand`]s for bro.

use crate::BridgeCommand;
use crate::commands::{compose_payload, launch_terminal, submit_settle, write_paced};
use crate::prompt;
use crate::rest::api_error;
use crate::state::{AppState, COMMAND_INPUT, COMMAND_KILL, COMMAND_RESIZE};
use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

const UNKNOWN_SESSION: &str = "Unknown terminal session.";
/// Largest attachment accepted.
pub(crate) const MAX_ATTACHMENT_BYTES: usize = 32 * 1024 * 1024;

pub(crate) async fn list(State(state): State<AppState>) -> Json<Value> {
    Json(json!(state.summaries()))
}

pub(crate) async fn create(State(state): State<AppState>, Json(body): Json<Value>) -> Response {
    match launch_terminal(&state, body).await {
        Ok(summary) => Json(summary).into_response(),
        Err(message) => api_error(StatusCode::BAD_REQUEST, message),
    }
}

pub(crate) async fn rename(
    Path(id): Path<String>,
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Response {
    let title = body
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let Some(summary) = state.rename(&id, title.clone()) else {
        return api_error(StatusCode::NOT_FOUND, UNKNOWN_SESSION);
    };
    if !title.trim().is_empty() {
        state.send_command(BridgeCommand::Rename { id, title });
    }
    Json(summary).into_response()
}

pub(crate) async fn kill(Path(id): Path<String>, State(state): State<AppState>) -> Response {
    match state.dispatch(&id, COMMAND_KILL, "", 0, 0) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(message) => api_error(StatusCode::NOT_FOUND, message),
    }
}

/// bro addition: bring the session to the foreground in the TUI.
pub(crate) async fn focus(Path(id): Path<String>, State(state): State<AppState>) -> Response {
    if state.summary(&id).is_none() {
        return api_error(StatusCode::NOT_FOUND, UNKNOWN_SESSION);
    }
    state.send_command(BridgeCommand::Focus { id });
    StatusCode::NO_CONTENT.into_response()
}

pub(crate) async fn text(
    Path(id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
    State(state): State<AppState>,
) -> Response {
    let Some(text) = state.plain_text(&id) else {
        return api_error(StatusCode::NOT_FOUND, UNKNOWN_SESSION);
    };
    let tail = query
        .get("tail")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(200)
        .clamp(1, 2000);
    let lines: Vec<_> = text.lines().collect();
    let start = lines.len().saturating_sub(tail);
    (
        StatusCode::OK,
        [("content-type", "text/plain; charset=utf-8")],
        lines[start..].join("\n"),
    )
        .into_response()
}

pub(crate) async fn input_context(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Response {
    state
        .input_context(&id)
        .map(|context| Json(context).into_response())
        .unwrap_or_else(|| api_error(StatusCode::NOT_FOUND, UNKNOWN_SESSION))
}

pub(crate) async fn prompt_response(
    Path(id): Path<String>,
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Response {
    let Some(context) = state.input_context(&id) else {
        return api_error(StatusCode::NOT_FOUND, UNKNOWN_SESSION);
    };
    match prompt::prompt_response_bytes(&context, &body) {
        Ok(data) => match state.dispatch(&id, COMMAND_INPUT, &data, 0, 0) {
            Ok(()) => Json(
                json!({ "accepted": true, "promptId": body["promptId"], "action": body["action"] }),
            )
            .into_response(),
            Err(message) => api_error(StatusCode::INTERNAL_SERVER_ERROR, message),
        },
        Err(message) if message.contains("changed") => (
            StatusCode::CONFLICT,
            Json(json!({ "message": message, "stale": true })),
        )
            .into_response(),
        Err(message) => api_error(StatusCode::BAD_REQUEST, message),
    }
}

#[derive(Deserialize)]
pub(crate) struct ComposeBody {
    #[serde(default)]
    pub text: String,
    #[serde(default = "default_true")]
    pub submit: bool,
}

fn default_true() -> bool {
    true
}

pub(crate) async fn compose(
    Path(id): Path<String>,
    State(state): State<AppState>,
    Json(body): Json<ComposeBody>,
) -> Response {
    let Some(context) = state.input_context(&id) else {
        return api_error(StatusCode::NOT_FOUND, UNKNOWN_SESSION);
    };
    let bracketed = context
        .get("bracketedPaste")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let data = compose_payload(&body.text, bracketed);
    let result = async {
        write_paced(&state, &id, &data).await?;
        if body.submit {
            // Enter must be its own write, after the paste has been read. Sent
            // in the same chunk, Claude Code and Codex see it as part of the
            // paste burst and insert a newline instead of submitting.
            if !data.is_empty() {
                tokio::time::sleep(submit_settle(data.len())).await;
            }
            state.dispatch(&id, COMMAND_INPUT, "\r", 0, 0)?;
        }
        Ok::<(), String>(())
    }
    .await;
    match result {
        Ok(()) => Json(json!({
            "method": if data.is_empty() { "none" } else if bracketed { "paste" } else { "raw" },
            "bytes": data.len(),
            "submitted": body.submit
        }))
        .into_response(),
        Err(message) => api_error(StatusCode::INTERNAL_SERVER_ERROR, message),
    }
}

pub(crate) async fn write(
    Path(id): Path<String>,
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Response {
    let data = body.get("data").and_then(Value::as_str).unwrap_or_default();
    match state.dispatch(&id, COMMAND_INPUT, data, 0, 0) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(message) => api_error(StatusCode::NOT_FOUND, message),
    }
}

pub(crate) async fn resize(
    Path(id): Path<String>,
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Response {
    let dimension = |key: &str| {
        body.get(key)
            .and_then(Value::as_u64)
            .map(|value| value.min(u64::from(u16::MAX)) as u16)
    };
    if state.summary(&id).is_none() {
        return api_error(StatusCode::NOT_FOUND, UNKNOWN_SESSION);
    }
    match state.dispatch(
        &id,
        COMMAND_RESIZE,
        "",
        dimension("rows").unwrap_or(0),
        dimension("cols").unwrap_or(0),
    ) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(message) => api_error(StatusCode::BAD_REQUEST, message),
    }
}

pub(crate) async fn export(
    Path(id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
    State(state): State<AppState>,
) -> Response {
    let Some(export) = state.export(&id) else {
        return api_error(StatusCode::NOT_FOUND, UNKNOWN_SESSION);
    };
    match query.get("format").map(String::as_str).unwrap_or("ansi") {
        "json" => Json(export).into_response(),
        "screen" => (
            [("content-type", "text/plain; charset=utf-8")],
            export["screen"].as_str().unwrap_or_default().to_owned(),
        )
            .into_response(),
        _ => (
            [("content-type", "application/octet-stream")],
            export["transcript"].as_str().unwrap_or_default().to_owned(),
        )
            .into_response(),
    }
}

/// Where uploaded images are written (cleaned after 24 h).
pub(crate) fn attachment_directory() -> PathBuf {
    std::env::temp_dir().join("bro-bridge-attachments")
}

/// Deletes attachments older than a day.
pub(crate) async fn clean_old_attachments() {
    let Ok(mut entries) = tokio::fs::read_dir(attachment_directory()).await else {
        return;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let Ok(metadata) = entry.metadata().await else {
            continue;
        };
        let old = metadata
            .modified()
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age > Duration::from_secs(24 * 60 * 60));
        if metadata.is_file() && old {
            let _ = tokio::fs::remove_file(entry.path()).await;
        }
    }
}

pub(crate) async fn attachment(
    Path(id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if state.summary(&id).is_none() {
        return api_error(StatusCode::NOT_FOUND, UNKNOWN_SESSION);
    }
    if body.is_empty() {
        return api_error(StatusCode::BAD_REQUEST, "Attachment body is empty.");
    }
    if body.len() > MAX_ATTACHMENT_BYTES {
        return api_error(StatusCode::PAYLOAD_TOO_LARGE, "Attachment exceeds 32 MB.");
    }
    let mime = headers
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if !mime.starts_with("image/") {
        return api_error(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "Only image attachments are supported.",
        );
    }
    let extension = match mime.as_str() {
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        "image/heic" => "heic",
        _ => "png",
    };
    let directory = attachment_directory();
    if let Err(error) = tokio::fs::create_dir_all(&directory).await {
        return api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
    }
    let path = directory.join(format!(
        "img-{}-{}.{}",
        chrono::Utc::now().format("%Y%m%dT%H%M%S"),
        &uuid::Uuid::new_v4().simple().to_string()[..8],
        extension
    ));
    if let Err(error) = tokio::fs::write(&path, body).await {
        return api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
    }
    let mut pasted = false;
    if query.get("paste").map(String::as_str) == Some("1") {
        let path_text = path.to_string_lossy();
        let text = if path_text.contains(' ') {
            format!("\"{path_text}\" ")
        } else {
            format!("{path_text} ")
        };
        pasted = state.dispatch(&id, COMMAND_INPUT, &text, 0, 0).is_ok();
    }
    (
        StatusCode::CREATED,
        Json(json!({ "path": path.to_string_lossy(), "pasted": pasted })),
    )
        .into_response()
}

pub(crate) async fn commands(Path(id): Path<String>, State(state): State<AppState>) -> Response {
    let Some(context) = state.input_context(&id) else {
        return api_error(StatusCode::NOT_FOUND, UNKNOWN_SESSION);
    };
    let agent = context["agent"].as_str().unwrap_or_default().to_string();
    let cwd = context["cwd"].as_str().unwrap_or_default().to_string();
    match tokio::task::spawn_blocking(move || {
        crate::slash_commands::list_slash_commands(&agent, &cwd)
    })
    .await
    {
        Ok(commands) => Json(json!({
            "agent": context["agent"],
            "agentLabel": context["agentLabel"],
            "cwd": context["cwd"],
            "commands": commands,
        }))
        .into_response(),
        Err(error) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Slash commands could not be listed: {error}"),
        ),
    }
}

/// Fuzzy file lookup under the session's working directory, backing `@file`
/// mentions from clients that cannot see the host filesystem.
pub(crate) async fn files(
    Path(id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
    State(state): State<AppState>,
) -> Response {
    let Some(summary) = state.summary(&id) else {
        return api_error(StatusCode::NOT_FOUND, UNKNOWN_SESSION);
    };
    let limit = crate::file_search::clamp_limit(query.get("limit").map(String::as_str));
    let text = query.get("q").cloned().unwrap_or_default();
    let cwd = summary.cwd.clone();
    match tokio::task::spawn_blocking(move || crate::file_search::search_files(&cwd, &text, limit))
        .await
    {
        Ok(files) => Json(json!({ "cwd": summary.cwd, "files": files })).into_response(),
        Err(error) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Files could not be listed: {error}"),
        ),
    }
}

pub(crate) async fn file_preview(
    Path(id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
    State(state): State<AppState>,
) -> Response {
    let Some(session) = state.summary(&id) else {
        return api_error(StatusCode::NOT_FOUND, UNKNOWN_SESSION);
    };
    let target = query.get("path").cloned().unwrap_or_default();
    let line = query
        .get("line")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(1);
    match tokio::task::spawn_blocking(move || {
        crate::file_preview::read_preview(&session.cwd, &target, line)
    })
    .await
    {
        Ok(Ok(preview)) => Json(preview).into_response(),
        Ok(Err(message)) => api_error(StatusCode::BAD_REQUEST, message),
        Err(_) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "File could not be opened.",
        ),
    }
}
