//! MCP Apps for the phone: a tool result whose server gave it a page of its own shows as that page in the phone's
//! focus view, as it does in the desktop app's (see `bro_core::mcp_apps`).
//!
//! - `GET /api/sessions/{id}/apps/{call}?theme=dark&bg=%23161618&width=360` answers bro's host page with the app's
//!   page loading in it, for the phone's web view: the call's input and result, and a host context in the
//!   phone's theme. The page comes from bro's disk (`~/.bro/mcp-apps`, which the desktop app fills too), else
//!   through the session's bro mod, over Claude Code's own connection to the server, and is kept.
//! - `POST /api/sessions/{id}/apps/{call}/ask` with `{ method, params }` does what the page asked of its own
//!   server (`tools/call`, `resources/read`) through the mod: `{ ok: true, result }` or `{ ok: false, error }`.

use crate::rest::api_error;
use crate::state::AppState;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use bro_core::mcp_apps::{self, Frame, Rgb};
use bro_core::sessions::conversation::{Item, McpApp};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;

#[derive(Deserialize)]
pub(crate) struct HostQuery {
    theme: Option<String>,
    /// the card's fill, `#rrggbb`
    bg: Option<String>,
    /// the card's width, CSS px
    width: Option<f32>,
}

pub(crate) async fn host(Path((id, call)): Path<(String, String)>, Query(q): Query<HostQuery>, State(state): State<AppState>) -> Response {
    let Some(app) = find(&state, &id, &call).await else {
        return api_error(StatusCode::NOT_FOUND, "That step has no app.");
    };
    let html = match page(&state, &id, &app).await {
        Ok(html) => html,
        Err(e) => return api_error(StatusCode::BAD_GATEWAY, &e),
    };
    let light = q.theme.as_deref() == Some("light");
    let fill = q.bg.as_deref().and_then(hex).unwrap_or(if light { [1.0; 3] } else { [0.086, 0.086, 0.094] });
    let frame = Frame { call: &call, light, fill, width: q.width.unwrap_or(360.0).clamp(120.0, 2000.0), max_h: 4000.0, mobile: true, version: env!("CARGO_PKG_VERSION") };
    let body = mcp_apps::host_page(&mcp_apps::payload(&app, &html, &frame));
    ([(header::CONTENT_TYPE, "text/html; charset=utf-8"), (header::CACHE_CONTROL, "no-store")], body).into_response()
}

#[derive(Deserialize)]
pub(crate) struct Ask {
    method: String,
    #[serde(default)]
    params: Value,
}

pub(crate) async fn ask(Path((id, call)): Path<(String, String)>, State(state): State<AppState>, Json(body): Json<Ask>) -> Response {
    let Some(app) = find(&state, &id, &call).await else {
        return api_error(StatusCode::NOT_FOUND, "That step has no app.");
    };
    let Some(job) = mcp_apps::job(&app, &body.method, &body.params) else {
        return Json(json!({ "ok": false, "error": format!("bro doesn't do {} for a page", body.method) })).into_response();
    };
    match by_mod(&state, &id, job).await {
        Ok(result) => Json(json!({ "ok": true, "result": result })).into_response(),
        Err(error) => Json(json!({ "ok": false, "error": error })).into_response(),
    }
}

/// `#rrggbb` as a colour.
fn hex(s: &str) -> Option<Rgb> {
    let s = s.trim().trim_start_matches('#');
    if s.len() != 6 {
        return None;
    }
    let c = |i: usize| u8::from_str_radix(&s[i..i + 2], 16).ok().map(|v| v as f32 / 255.0);
    Some([c(0)?, c(2)?, c(4)?])
}

/// The app of call `call` in session `id`'s conversation.
async fn find(state: &AppState, id: &str, call: &str) -> Option<Arc<McpApp>> {
    let (files, agent) = state.conversation_source(id)?;
    let transcript = files?;
    let harness = crate::conversation::harness_of(agent.as_deref(), &transcript);
    let (feeds, id, call) = (state.conversations.clone(), id.to_owned(), call.to_owned());
    tokio::task::spawn_blocking(move || {
        let (_, c) = feeds.read(&id, &transcript, harness);
        c.items.iter().rev().find_map(|it| match it.as_ref() {
            Item::Did { steps } => steps.iter().find(|s| s.id == call).map(|s| s.app.clone()),
            _ => None,
        })?
    })
    .await
    .ok()
    .flatten()
}

/// The session's mod does `job`.
async fn by_mod(state: &AppState, id: &str, job: Value) -> Result<Value, String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    crate::mod_jobs::ask(state, id, job, Box::new(move |result| {
        let _ = tx.send(result);
    }));
    rx.await.unwrap_or_else(|_| Err("bro's bridge stopped before the bro mod answered".into()))
}

/// The app's page: from bro's disk, else read through the session's mod (and kept for next time).
async fn page(state: &AppState, id: &str, app: &McpApp) -> Result<String, String> {
    // bro's folder holds the bridge's (`~/.bro/bridge`)
    let root = state.data_root.parent().map(std::path::Path::to_path_buf).unwrap_or_else(bro_core::paths::bro_dir);
    let path = mcp_apps::page_path(&root, &app.server, &app.resource);
    let kept = path.clone();
    if let Ok(Some(html)) = tokio::task::spawn_blocking(move || std::fs::read_to_string(&kept).ok().filter(|s| !s.trim().is_empty())).await {
        return Ok(html);
    }
    let read = by_mod(state, id, json!({ "kind": "read", "server": app.server, "uri": app.resource })).await?;
    let html = mcp_apps::page_text(&read)?;
    let copy = html.clone();
    let _ = tokio::task::spawn_blocking(move || {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        bro_core::util::atomic_write(&path, copy.as_bytes())
    })
    .await;
    Ok(html)
}

#[cfg(test)]
mod tests {
    use super::hex;

    #[test]
    fn a_fill_is_a_hex_colour() {
        assert_eq!(hex("#ff0000"), Some([1.0, 0.0, 0.0]));
        assert_eq!(hex("000000"), Some([0.0, 0.0, 0.0]));
        assert_eq!(hex("#fff"), None);
        assert_eq!(hex("#zzzzzz"), None);
    }
}
