//! Phone controls use the same choices and pending changes as the desktop reply box.

use crate::{AppState, BridgeCommand, FocusControlChange};
use crate::rest::api_error;
use axum::{Json, extract::{Path, State}, http::StatusCode, response::{IntoResponse, Response}};
use serde_json::json;
use std::time::Duration;

pub(crate) async fn quips() -> Response {
    match tokio::task::spawn_blocking(|| {
        let library = bro_core::quips::Library::load(bro_core::quips::path());
        json!({ "quips": library.items, "error": library.error })
    }).await {
        Ok(body) => Json(body).into_response(),
        Err(_) => api_error(StatusCode::INTERNAL_SERVER_ERROR, "Your Quips could not be read."),
    }
}

pub(crate) async fn get(Path(id): Path<String>, State(state): State<AppState>) -> Response {
    controls(&state, id, None).await
}

pub(crate) async fn set(Path(id): Path<String>, State(state): State<AppState>, Json(change): Json<FocusControlChange>) -> Response {
    controls(&state, id, Some(change)).await
}

async fn controls(state: &AppState, id: String, change: Option<FocusControlChange>) -> Response {
    let Some(session) = state.summary(&id) else {
        return api_error(StatusCode::NOT_FOUND, "Unknown terminal session.");
    };
    if session.status != "running" {
        return api_error(StatusCode::CONFLICT, "The session has exited.");
    }
    let (reply, answer) = tokio::sync::oneshot::channel();
    state.send_command(BridgeCommand::FocusControls { id, change, reply });
    match tokio::time::timeout(Duration::from_secs(8), answer).await {
        Ok(Ok(Ok(body))) => Json(body).into_response(),
        Ok(Ok(Err(message))) => api_error(StatusCode::BAD_REQUEST, message),
        _ => api_error(StatusCode::GATEWAY_TIMEOUT, "The session did not answer. Try again."),
    }
}
