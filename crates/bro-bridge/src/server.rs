//! Router, listener, embedded web client and the discovery file.

use crate::state::AppState;
use crate::{auth, rest, rest_sessions, ws};
use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{StatusCode, Uri, header};
use axum::middleware;
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get, patch, post};
use serde_json::{Value, json};
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;
use tokio::net::TcpListener;
use tower_http::compression::CompressionLayer;

/// Discovery file other tools read to find the running host.
pub(crate) const SERVER_INFO_FILE: &str = ".terminal-web-server.json";
/// How many ports after the configured one `automatic_port` tries.
const PORT_ATTEMPTS: u16 = 20;

pub(crate) struct EmbeddedClientAsset {
    pub path: &'static str,
    pub content_type: &'static str,
    pub bytes: &'static [u8],
}

include!(concat!(env!("OUT_DIR"), "/embedded_client.rs"));

pub(crate) fn embedded_asset(path: &str) -> Option<&'static EmbeddedClientAsset> {
    EMBEDDED_CLIENT_ASSETS
        .iter()
        .find(|asset| asset.path == path)
}

/// Serves the web client: exact asset, else `index.html` (SPA routes), else 404.
async fn embedded_client(State(state): State<AppState>, uri: Uri) -> Response {
    if !state.config.web_interface {
        return StatusCode::NOT_FOUND.into_response();
    }
    let requested = uri.path().trim_start_matches('/');
    let exact = embedded_asset(requested);
    let Some(asset) = exact.or_else(|| embedded_asset("index.html")) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let cache_control = if exact.is_some() && asset.path != "index.html" {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    Response::builder()
        .header(header::CONTENT_TYPE, asset.content_type)
        .header(header::CACHE_CONTROL, cache_control)
        .header("x-content-type-options", "nosniff")
        .body(Body::from(Bytes::from_static(asset.bytes)))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// Every route of the reference host except the `/bridge` peer socket
/// (bro is a single process, so there are no peers to join).
pub(crate) fn router(state: AppState) -> Router {
    let protected = Router::new()
        .route("/ws", any(ws::upgrade))
        .route("/api/bootstrap", get(rest::bootstrap))
        .route("/api/health", get(rest::health))
        .route("/api/acp", get(rest::acp))
        .route("/api/acp/{*rest}", any(rest::unavailable))
        .route(
            "/api/sessions",
            get(rest_sessions::list).post(rest_sessions::create),
        )
        .route(
            "/api/sessions/{id}",
            patch(rest_sessions::rename).delete(rest_sessions::kill),
        )
        .route("/api/sessions/{id}/text", get(rest_sessions::text))
        .route(
            "/api/sessions/{id}/input-context",
            get(rest_sessions::input_context),
        )
        .route(
            "/api/sessions/{id}/prompt-response",
            post(rest_sessions::prompt_response),
        )
        .route("/api/sessions/{id}/compose", post(rest_sessions::compose))
        .route("/api/sessions/{id}/write", post(rest_sessions::write))
        .route("/api/sessions/{id}/resize", post(rest_sessions::resize))
        .route("/api/sessions/{id}/export", get(rest_sessions::export))
        .route(
            "/api/sessions/{id}/attachments",
            post(rest_sessions::attachment),
        )
        .route("/api/sessions/{id}/commands", get(rest_sessions::commands))
        .route("/api/sessions/{id}/files", get(rest_sessions::files))
        .route("/api/sessions/{id}/file", get(rest_sessions::file_preview))
        .route("/api/sessions/{id}/focus", post(rest_sessions::focus))
        .route("/api/sessions/{id}/agent", get(rest::unavailable))
        .route("/api/sessions/{id}/agent/attach", any(rest::unavailable))
        .route("/api/notifications", get(rest::notifications))
        .route("/api/notify", post(rest::notify))
        .route(
            "/api/projects",
            get(rest::projects).post(rest::create_project),
        )
        .route("/api/projects/recent", get(rest::recent_projects))
        .route("/api/projects/order", patch(rest::reorder_projects))
        .route(
            "/api/projects/{id}",
            patch(rest::rename_project).delete(rest::delete_project),
        )
        .route("/api/peers/{*rest}", any(rest::peers_unavailable))
        .route("/api/orchestrator", get(rest::orchestrator_status))
        .route(
            "/api/orchestrator/config",
            get(rest::orchestrator_config)
                .put(rest::orchestrator_update_config)
                .patch(rest::orchestrator_update_config),
        )
        .route("/api/orchestrator/models", get(rest::orchestrator_models))
        .route(
            "/api/orchestrator/messages",
            post(rest::orchestrator_send).delete(rest::orchestrator_clear),
        )
        .route("/api/orchestrator/cancel", post(rest::orchestrator_cancel))
        .route("/api/orchestrator/test", post(rest::orchestrator_test))
        .route("/api/orchestrator/start", post(rest::orchestrator_retired))
        .route("/api/orchestrator/stop", post(rest::orchestrator_retired))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth::auth_middleware,
        ))
        .layer(DefaultBodyLimit::max(
            rest_sessions::MAX_ATTACHMENT_BYTES + 1024,
        ));

    let client = Router::new()
        .fallback(embedded_client)
        .layer(CompressionLayer::new().gzip(true));
    Router::new()
        .merge(protected)
        .merge(client)
        .with_state(state)
}

fn candidate_ports(start: u16, automatic: bool) -> Vec<u16> {
    if start == 0 {
        return vec![0];
    }
    let end = if automatic {
        start.saturating_add(PORT_ATTEMPTS)
    } else {
        start
    };
    (start..=end).collect()
}

/// Binds the configured address, walking forward from the configured port
/// when `automatic_port` is set. Port 0 asks the OS for an ephemeral port.
pub(crate) async fn bind(
    bind: &str,
    port: u16,
    automatic: bool,
) -> Result<(TcpListener, IpAddr, u16), String> {
    let bind = bind.trim();
    let host: IpAddr = if bind.is_empty() {
        IpAddr::from([0, 0, 0, 0])
    } else if bind.eq_ignore_ascii_case("localhost") {
        IpAddr::from([127, 0, 0, 1])
    } else {
        bind.parse()
            .map_err(|_| format!("Invalid bridge bind address: {bind}"))?
    };
    let mut last_error = None;
    for candidate in candidate_ports(port, automatic) {
        match TcpListener::bind(SocketAddr::new(host, candidate)).await {
            Ok(listener) => {
                let port = listener
                    .local_addr()
                    .map(|address| address.port())
                    .unwrap_or(candidate);
                return Ok((listener, host, port));
            }
            Err(error) => last_error = Some(error),
        }
    }
    let detail = last_error
        .map(|error| error.to_string())
        .unwrap_or_default();
    Err(if automatic && port != 0 {
        format!(
            "No bridge port was available in {port}-{}: {detail}",
            port.saturating_add(PORT_ATTEMPTS)
        )
    } else {
        format!("Bridge port {port} on {host} is unavailable: {detail}")
    })
}

/// Writes `<data_root>/.terminal-web-server.json` so existing tools find us.
pub(crate) fn write_server_info(state: &AppState) {
    let (host, port, started_at) = {
        let inner = state.inner.lock();
        (inner.host, inner.port, inner.started_at.clone())
    };
    let info = json!({
        "pid": std::process::id(),
        "host": host.to_string(),
        "port": port,
        "startedAt": started_at,
        "runtime": "bro"
    });
    let bytes = serde_json::to_vec_pretty(&info).unwrap_or_default();
    if let Err(error) =
        crate::project_store::write_atomic(&state.data_root.join(SERVER_INFO_FILE), &bytes)
    {
        tracing::warn!("bro-bridge: could not write {SERVER_INFO_FILE}: {error}");
    }
}

/// Removes the discovery file if it still describes this process.
pub(crate) fn remove_server_info(state: &AppState) {
    let path = state.data_root.join(SERVER_INFO_FILE);
    let ours = std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .is_some_and(|info| info["pid"].as_u64() == Some(u64::from(std::process::id())));
    if ours {
        let _ = std::fs::remove_file(path);
    }
}

/// Attachment cleanup and the observation sweep that settles sessions whose
/// last chunk arrived inside the observation throttle.
pub(crate) fn spawn_background(state: &AppState) {
    tokio::spawn(async {
        loop {
            rest_sessions::clean_old_attachments().await;
            tokio::time::sleep(Duration::from_secs(60 * 60)).await;
        }
    });
    let observer = state.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(500)).await;
            observer.observe_pending_sessions();
        }
    });
}

/// Serves until `state.shutdown` flips.
pub(crate) async fn serve(listener: TcpListener, state: AppState) -> std::io::Result<()> {
    let mut shutdown = state.shutdown.subscribe();
    axum::serve(
        listener,
        router(state).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        let _ = shutdown.wait_for(|stopped| *stopped).await;
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidate_ports_follow_the_reference_host() {
        assert_eq!(candidate_ports(10001, true).len(), 21);
        assert_eq!(candidate_ports(10001, true)[20], 10021);
        assert_eq!(candidate_ports(45000, false), vec![45000]);
        assert_eq!(candidate_ports(0, true), vec![0]);
    }

    #[test]
    fn embedded_client_table_is_consistent() {
        // The web client is optional at build time; when present it must
        // include an index and correctly typed assets.
        if let Some(index) = embedded_asset("index.html") {
            assert!(index.content_type.starts_with("text/html"));
            assert!(std::str::from_utf8(index.bytes).unwrap().contains("<html"));
            assert!(
                EMBEDDED_CLIENT_ASSETS
                    .iter()
                    .any(|asset| asset.content_type.starts_with("text/javascript"))
            );
        }
        assert!(
            EMBEDDED_CLIENT_ASSETS
                .iter()
                .all(|asset| !asset.path.starts_with('/'))
        );
    }
}
