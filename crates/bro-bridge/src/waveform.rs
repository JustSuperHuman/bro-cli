//! `GET /api/sessions/{id}/waveform?path=[&bars=]`: a sound file's waveform for the phone's inline player,
//! `{ "peaks": [0..1, …], "durationMs": n }` (see [`bro_core::audio::waveform`]). Same resolver and media rule
//! as `/raw`; the file is read on a blocking thread.

use crate::file_preview::{self, Kind};
use crate::rest::api_error;
use crate::state::AppState;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use std::collections::HashMap;

/// Bars when the client doesn't say, and the most it may ask for.
const BARS: usize = 64;
const MOST: usize = 400;

pub(crate) async fn waveform(Path(id): Path<String>, Query(query): Query<HashMap<String, String>>, State(state): State<AppState>) -> Response {
    let Some(roots) = state.file_roots(&id) else {
        return api_error(StatusCode::NOT_FOUND, "Unknown terminal session.");
    };
    let target = query.get("path").cloned().unwrap_or_default();
    let bars = query.get("bars").and_then(|b| b.parse::<usize>().ok()).unwrap_or(BARS).clamp(1, MOST);
    let access = state.media_access();
    let read = tokio::task::spawn_blocking(move || {
        let resolved = file_preview::resolve_media(&roots, &target, &|p| access.allows(p)).map_err(|m| {
            let status = match m.as_str() {
                file_preview::NOT_FOUND => StatusCode::NOT_FOUND,
                file_preview::OUTSIDE | file_preview::PROTECTED => StatusCode::FORBIDDEN,
                _ => StatusCode::BAD_REQUEST,
            };
            (status, m)
        })?;
        if resolved.kind != Kind::File || !bro_core::audio::is_audio(&resolved.path) {
            return Err((StatusCode::BAD_REQUEST, "This path is not a sound file.".to_string()));
        }
        bro_core::audio::waveform(&resolved.path, bars).map_err(|_| (StatusCode::UNPROCESSABLE_ENTITY, "This sound can't be read here (it may still play).".to_string()))
    })
    .await;
    match read {
        Ok(Ok(w)) => Json(w).into_response(),
        Ok(Err((status, message))) => api_error(status, message),
        Err(_) => api_error(StatusCode::INTERNAL_SERVER_ERROR, "The sound could not be read."),
    }
}
