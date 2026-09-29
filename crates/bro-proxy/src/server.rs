//! axum server on its own thread + multi-threaded tokio runtime.

use crate::errors::{ErrorKind, ProxyError};
use crate::events::Recorder;
use crate::state::AppState;
use crate::upstream::{Endpoint, UpstreamCall};
use crate::{Dialect, ProxyConfig, ProxyOptions, auth, flow, models};
use axum::Router;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, State};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use http::{HeaderMap, Uri};
use parking_lot::Mutex;
use serde_json::{Value, json};
use std::future::IntoFuture;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::oneshot;

const BODY_LIMIT: usize = 256 * 1024 * 1024;

pub(crate) struct Running {
    pub port: u16,
    pub state: Arc<AppState>,
    shutdown: Mutex<Option<oneshot::Sender<()>>>,
}

impl Running {
    pub fn start(cfg: ProxyConfig, opts: ProxyOptions) -> anyhow::Result<Running> {
        let state = Arc::new(AppState::new(
            cfg.token.clone().filter(|t| !t.is_empty()),
            opts,
        )?);
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<anyhow::Result<u16>>();
        let (stop_tx, stop_rx) = oneshot::channel::<()>();
        let st = state.clone();
        std::thread::Builder::new()
            .name("bro-proxy".into())
            .spawn(move || {
                let rt = match tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(4)
                    .thread_name("bro-proxy-worker")
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => {
                        let _ = ready_tx.send(Err(e.into()));
                        return;
                    }
                };
                rt.block_on(async move {
                    let listener = match bind(&cfg).await {
                        Ok(l) => l,
                        Err(e) => {
                            let _ = ready_tx.send(Err(e));
                            return;
                        }
                    };
                    let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
                    let _ = ready_tx.send(Ok(port));
                    tracing::info!(port, "bro proxy listening");
                    let app = router(st);
                    let (drain_tx, drain_rx) = oneshot::channel::<()>();
                    let serve = axum::serve(
                        listener,
                        app.into_make_service_with_connect_info::<SocketAddr>(),
                    )
                    .with_graceful_shutdown(async move {
                        let _ = stop_rx.await;
                        let _ = drain_tx.send(());
                    });
                    // Graceful shutdown waits for open streams; cap that wait.
                    tokio::select! {
                        r = serve.into_future() => {
                            if let Err(e) = r {
                                tracing::error!("proxy server error: {e}");
                            }
                        }
                        _ = async {
                            let _ = drain_rx.await;
                            tokio::time::sleep(Duration::from_secs(3)).await;
                        } => tracing::info!("proxy shutdown: dropping open streams"),
                    }
                });
                rt.shutdown_timeout(Duration::from_secs(3));
                tracing::info!("bro proxy stopped");
            })?;
        let port = ready_rx
            .recv()
            .map_err(|_| anyhow::anyhow!("proxy thread exited before binding"))??;
        Ok(Running {
            port,
            state,
            shutdown: Mutex::new(Some(stop_tx)),
        })
    }

    pub fn shutdown(&self) {
        if let Some(tx) = self.shutdown.lock().take() {
            let _ = tx.send(());
        }
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.shutdown();
    }
}

async fn bind(cfg: &ProxyConfig) -> anyhow::Result<tokio::net::TcpListener> {
    let host = if cfg.bind.is_empty() {
        "127.0.0.1"
    } else {
        cfg.bind.as_str()
    };
    if cfg.port == 0 {
        return Ok(tokio::net::TcpListener::bind((host, 0)).await?);
    }
    let mut last = None;
    for p in cfg.port..=cfg.port.saturating_add(20) {
        match tokio::net::TcpListener::bind((host, p)).await {
            Ok(l) => return Ok(l),
            Err(e) => last = Some(e),
        }
    }
    Err(anyhow::anyhow!(
        "no free port in {}..={} on {host}: {}",
        cfg.port,
        cfg.port.saturating_add(20),
        last.map(|e| e.to_string()).unwrap_or_default()
    ))
}

fn router(state: Arc<AppState>) -> Router {
    let api = Router::new()
        .route("/v1/messages", post(messages))
        .route("/messages", post(messages))
        .route("/v1/messages/count_tokens", post(count_tokens))
        .route("/messages/count_tokens", post(count_tokens))
        .route("/v1/chat/completions", post(chat))
        .route("/chat/completions", post(chat))
        .route("/v1/responses", post(responses))
        .route("/responses", post(responses))
        .route("/v1/models", get(list_models))
        .route("/models", get(list_models));
    Router::new()
        .route("/health", get(health))
        .route("/", get(health))
        .nest("/r/{route}", api)
        .fallback(not_found)
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::middleware,
        ))
        .layer(DefaultBodyLimit::max(BODY_LIMIT))
        .with_state(state)
}

async fn health(State(state): State<Arc<AppState>>) -> Response {
    let routes: Vec<Value> = state
        .routes()
        .iter()
        .map(|r| json!({"id": r.id, "label": r.label, "upstream": r.upstream.kind_str()}))
        .collect();
    axum::Json(json!({"ok": true, "via": "bro proxy", "routes": routes})).into_response()
}

async fn not_found(uri: Uri) -> Response {
    let path = uri.path();
    ProxyError::new(ErrorKind::NotFound, format!("no endpoint for {path}"))
        .into_response(auth::dialect_for_path(path))
}

async fn messages(
    State(s): State<Arc<AppState>>,
    Path(route): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    flow::handle(s, Dialect::Anthropic, route, headers, body).await
}

async fn chat(
    State(s): State<Arc<AppState>>,
    Path(route): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    flow::handle(s, Dialect::Chat, route, headers, body).await
}

async fn responses(
    State(s): State<Arc<AppState>>,
    Path(route): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    flow::handle(s, Dialect::Responses, route, headers, body).await
}

async fn list_models(State(s): State<Arc<AppState>>, Path(route_id): Path<String>) -> Response {
    let Some(route) = s.route(&route_id) else {
        return ProxyError::new(ErrorKind::NotFound, format!("unknown route '{route_id}'"))
            .into_response(Dialect::Chat);
    };
    let list = models::list(&s, &route).await;
    let owner = match route.upstream.dialect() {
        Dialect::Anthropic => "anthropic",
        _ => "openai",
    };
    axum::Json(models::listing_json(&list, owner)).into_response()
}

/// Forwarded for Anthropic upstreams; estimated (chars/4, v1) otherwise or on failure.
async fn count_tokens(
    State(s): State<Arc<AppState>>,
    Path(route_id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some(route) = s.route(&route_id) else {
        return ProxyError::new(ErrorKind::NotFound, format!("unknown route '{route_id}'"))
            .into_response(Dialect::Anthropic);
    };
    let estimate = || {
        axum::Json(json!({"input_tokens": (body.len() as u64).div_ceil(4).max(1)})).into_response()
    };
    if route.upstream.dialect() != Dialect::Anthropic {
        return estimate();
    }
    let Ok(mut v) = serde_json::from_slice::<Value>(&body) else {
        return ProxyError::invalid("request body was not valid JSON")
            .into_response(Dialect::Anthropic);
    };
    let client_model = v
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let mapped = models::map_model(&route, &client_model).model;
    v["model"] = json!(mapped);
    let mut rec = Recorder::new(s.clone(), &route, Dialect::Anthropic);
    rec.set_model(&mapped);
    let call = UpstreamCall {
        endpoint: Endpoint::CountTokens,
        body: Some(Bytes::from(serde_json::to_vec(&v).unwrap_or_default())),
        stream: false,
        client_headers: crate::upstream::forwardable_client_headers(&headers),
        session_key: v
            .get("metadata")
            .and_then(|m| m.get("user_id"))
            .and_then(Value::as_str)
            .map(str::to_string),
        translated: false,
        codex_auth: None,
    };
    let result = async {
        let resp = crate::upstream::send(&s, &route, &call).await?;
        crate::upstream::read_all(resp.body).await
    }
    .await;
    // Counting isn't a completion: the pool account is not recorded.
    match result {
        Ok(bytes) => {
            rec.finish(200, None);
            (
                http::StatusCode::OK,
                [(http::header::CONTENT_TYPE, "application/json")],
                bytes,
            )
                .into_response()
        }
        Err(e) => {
            tracing::debug!("count_tokens upstream failed, estimating: {e}");
            rec.finish(200, None);
            estimate()
        }
    }
}
