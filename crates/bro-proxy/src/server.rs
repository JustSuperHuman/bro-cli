//! axum server on its own thread + multi-threaded tokio runtime.

use crate::errors::{ErrorKind, ProxyError};
use crate::events::Recorder;
use crate::state::AppState;
use crate::upstream::{Endpoint, UpstreamCall};
use crate::{Dialect, ProxyConfig, ProxyOptions, auth, flow, models};
use axum::Router;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, OriginalUri, Path, State};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use http::{HeaderMap, Method, Uri};
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
        .route("/v1/responses", post(responses).get(upgrade_required))
        .route("/responses", post(responses).get(upgrade_required))
        // Codex in proxy mode: its own `openai` provider at `{route}/backend-api/codex`
        .route("/backend-api/codex/responses", post(responses).get(upgrade_required))
        .route("/v1/models", get(list_models))
        .route("/models", get(list_models))
        .fallback(relay_other);
    Router::new()
        .route("/health", get(health))
        .route("/usage", get(usage))
        .route("/v1/usage", get(usage))
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
    // /health answers without the key: a proxy-mode route's id is its key, so it stays out
    let routes: Vec<Value> = state
        .routes()
        .iter()
        .map(|r| {
            let id = if r.upstream.family().is_some() { "(hidden)" } else { r.id.as_str() };
            json!({"id": id, "label": r.label, "upstream": r.upstream.kind_str()})
        })
        .collect();
    axum::Json(json!({"ok": true, "via": "bro proxy", "routes": routes})).into_response()
}

/// Subscription meters for every login (bro_core's usage cache; no upstream request
/// is made) plus, per Claude pool route, each account's local counters and cooldown.
async fn usage(State(state): State<Arc<AppState>>) -> Response {
    let report = tokio::task::spawn_blocking(move || {
        let pools: Vec<Value> = state
            .routes()
            .iter()
            .filter_map(|r| match &r.upstream {
                crate::Upstream::ClaudePool { config_dirs } => Some(json!({
                    "route": r.id,
                    "label": r.label,
                    "accounts": state.pool().status(config_dirs),
                })),
                _ => None,
            })
            .collect();
        let mut report = bro_core::usage::cache::report();
        report["pools"] = json!(pools);
        report
    })
    .await
    .unwrap_or_else(|_| json!({ "error": "usage report failed" }));
    axum::Json(report).into_response()
}

async fn not_found(uri: Uri) -> Response {
    let path = uri.path();
    ProxyError::new(ErrorKind::NotFound, format!("no endpoint for {path}"))
        .into_response(auth::dialect_for_path(path))
}

/// Codex opens a WebSocket to `/responses` first; 426 makes it use HTTP for the session, at once and
/// without a retry.
async fn upgrade_required() -> Response {
    (http::StatusCode::UPGRADE_REQUIRED, "bro-proxy serves Responses over HTTP").into_response()
}

/// `/r/{id}{rest}` → (id, rest with the query)
fn split_route(uri: &Uri) -> Option<(String, String)> {
    let rest = uri.path().strip_prefix("/r/")?;
    let (id, path) = rest.split_once('/').map(|(i, p)| (i, format!("/{p}"))).unwrap_or((rest, String::new()));
    let query = uri.query().map(|q| format!("?{q}")).unwrap_or_default();
    Some((id.to_string(), format!("{path}{query}")))
}

/// Anything else under a route: for proxy mode's routes, the harness's own backend with its own
/// credentials (model lists, files…); for the rest, not found.
async fn relay_other(State(s): State<Arc<AppState>>, OriginalUri(uri): OriginalUri, method: Method, headers: HeaderMap, body: Bytes) -> Response {
    let route = split_route(&uri).and_then(|(id, rest)| Some((s.route(&id)?, rest)));
    match route {
        Some((r, rest)) if r.upstream.family().is_some() => relay(&s, &r, method, &rest, &headers, body).await,
        _ => not_found(uri).await,
    }
}

/// A proxy-mode route's own backend, relayed as the harness that asked (errors in its dialect).
async fn relay(s: &AppState, route: &crate::Route, method: Method, rest: &str, headers: &HeaderMap, body: Bytes) -> Response {
    let family = route.upstream.family().expect("a proxy-mode route");
    let mut headers = headers.clone();
    // a client that signs in with the proxy's own key (Pi, omp) has nothing to show the backend
    if s.token.as_deref().is_some_and(|t| auth::presented_key(&headers).is_some_and(|k| k.trim() == t)) {
        headers.remove(http::header::AUTHORIZATION);
        headers.remove("x-api-key");
    }
    match crate::upstream::logins::relay(s, family, method, rest, &headers, body).await {
        Ok(r) => r,
        Err(e) => e.into_response(route.upstream.dialect()),
    }
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

async fn list_models(State(s): State<Arc<AppState>>, Path(route_id): Path<String>, OriginalUri(uri): OriginalUri, headers: HeaderMap) -> Response {
    let Some(route) = s.route(&route_id) else {
        return ProxyError::new(ErrorKind::NotFound, format!("unknown route '{route_id}'"))
            .into_response(Dialect::Chat);
    };
    // proxy mode: the list the harness's own login gets
    if route.upstream.family().is_some()
        && let Some((_, rest)) = split_route(&uri)
    {
        return relay(&s, &route, Method::GET, &rest, &headers, Bytes::new()).await;
    }
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
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some(route) = s.route(&route_id) else {
        return ProxyError::new(ErrorKind::NotFound, format!("unknown route '{route_id}'"))
            .into_response(Dialect::Anthropic);
    };
    // proxy mode: counted by the harness's own login, as when it is signed in directly
    if matches!(route.upstream, crate::Upstream::ClaudeLogins)
        && let Some((_, rest)) = split_route(&uri)
    {
        return relay(&s, &route, Method::POST, &rest, &headers, body).await;
    }
    let estimate = || {
        axum::Json(json!({"input_tokens": (body.len() as u64).div_ceil(4).max(1)})).into_response()
    };
    if route.upstream.dialect() != Dialect::Anthropic {
        return estimate();
    }
    let Ok(v) = serde_json::from_slice::<Value>(&body) else {
        return ProxyError::invalid("request body was not valid JSON")
            .into_response(Dialect::Anthropic);
    };
    // Count what a real request would send: the same compression, without recording it.
    let trimmed = s.trim.apply(Dialect::Anthropic, v).await;
    let mut v = trimmed.json;
    let client_model = v
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let mapped = models::map_model(&route, &client_model).model;
    v["model"] = json!(mapped);
    let mut rec = Recorder::new(s.clone(), &route, Dialect::Anthropic);
    rec.set_model(&mapped);
    rec.set_trim(trimmed.tokens);
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
        user_id: None,
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
