//! Authenticated gateway to the separately installed RMM process.
//! Leases own input permission; disabling the feature revokes every transport.
use crate::state::AppState;
use axum::body::{Body, to_bytes};
use axum::extract::{
    Path, State, WebSocketUpgrade,
    ws::{Message, WebSocket},
};
use axum::http::{HeaderMap, Method, Request, StatusCode, Uri, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get, post};
use axum::{Json, Router};
use bro_core::proc_tree::ProcessTree;
use bro_core::rmm as package;
use futures_util::{SinkExt, StreamExt};
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};
use tokio::sync::watch;
use tokio_tungstenite::tungstenite::{self, client::IntoClientRequest};

const LEASE_TTL: Duration = Duration::from_secs(90);
const IDLE_STOP: Duration = Duration::from_secs(15);
const MAX_VIEWERS: usize = 4;

#[cfg(test)]
mod acceptance_tests;

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    Disabled,
    NotInstalled,
    Installing,
    Ready,
    Starting,
    Running,
    Error,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub enabled: bool,
    pub supported: bool,
    pub read_only: bool,
    pub phase: Phase,
    pub version: Option<String>,
    pub message: Option<String>,
    pub active_viewers: usize,
    pub relay_configured: bool,
}

struct Process {
    child: Child,
    tree: ProcessTree,
    target: Target,
}

impl Drop for Process {
    fn drop(&mut self) {
        // SendInput state belongs to Windows, so let the input worker release held
        // keys/buttons before ending its process. This socket never uses a proxy.
        if self.child.try_wait().ok().flatten().is_none()
            && let Some(address) = self
                .target
                .origin
                .strip_prefix("http://")
                .and_then(|address| address.parse::<std::net::SocketAddr>().ok())
            && let Ok(mut socket) =
                std::net::TcpStream::connect_timeout(&address, Duration::from_millis(150))
        {
            let _ = socket.set_write_timeout(Some(Duration::from_millis(200)));
            let _ = socket.set_read_timeout(Some(Duration::from_millis(300)));
            let request = format!(
                "POST /shutdown HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}",
                self.target.password
            );
            if socket.write_all(request.as_bytes()).is_ok() {
                let mut response = [0u8; 64];
                if socket
                    .read(&mut response)
                    .is_ok_and(|count| response[..count].starts_with(b"HTTP/1.1 200"))
                {
                    let until = Instant::now() + Duration::from_millis(500);
                    while Instant::now() < until && self.child.try_wait().ok().flatten().is_none() {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                }
            }
        }
        self.tree.terminate();
        let _ = self.child.wait();
    }
}

#[derive(Clone)]
struct Target {
    origin: String,
    password: String,
}

struct Lease {
    id: String,
    read_only: bool,
    transport: String,
    deadline: Mutex<Instant>,
    reconnect_at: Instant,
    cancelled: watch::Sender<bool>,
    attached: AtomicBool,
    rtc_id: Mutex<Option<String>>,
    ice_servers: Value,
    _control_lock: Option<File>,
}

impl Drop for Lease {
    fn drop(&mut self) {
        self.cancelled.send_replace(true);
    }
}

struct Inner {
    phase: Phase,
    version: Option<String>,
    message: Option<String>,
    process: Option<Process>,
    leases: HashMap<String, Arc<Lease>>,
    idle_since: Instant,
    generation: u64,
}

pub(crate) struct Rmm {
    root: PathBuf,
    config: RwLock<package::Settings>,
    inner: Mutex<Inner>,
    installing: AtomicBool,
    start_lock: tokio::sync::Mutex<()>,
    http: reqwest::Client,
}

impl Rmm {
    pub fn config_settings(&self) -> package::Settings {
        self.config.read().clone()
    }
    pub fn new(root: PathBuf) -> Self {
        let version = package::installed(&root)
            .ok()
            .map(|(_, manifest)| manifest.version);
        Self {
            root,
            config: RwLock::new(package::Settings::default()),
            inner: Mutex::new(Inner {
                phase: Phase::Disabled,
                version,
                message: None,
                process: None,
                leases: HashMap::new(),
                idle_since: Instant::now(),
                generation: 0,
            }),
            installing: AtomicBool::new(false),
            start_lock: tokio::sync::Mutex::new(()),
            http: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(20))
                .build()
                .expect("RMM HTTP client"),
        }
    }

    pub fn status(&self) -> Status {
        let config = self.config.read().clone();
        let inner = self.inner.lock();
        Status {
            enabled: config.enabled,
            supported: package::supported(),
            read_only: config.read_only,
            phase: if !config.enabled {
                Phase::Disabled
            } else {
                inner.phase
            },
            version: inner.version.clone(),
            message: inner.message.clone(),
            active_viewers: inner.leases.len(),
            relay_configured: self.root.join("relay.json").is_file(),
        }
    }

    pub fn configure(self: &Arc<Self>, config: package::Settings) {
        let (changed, was_enabled) = {
            let mut current = self.config.write();
            let changed = *current != config;
            let was_enabled = current.enabled;
            *current = config.clone();
            (changed, was_enabled)
        };
        if changed {
            self.stop();
        }
        if config.enabled {
            if !was_enabled {
                self.inner.lock().phase = Phase::NotInstalled;
            }
            self.install();
        }
    }

    pub fn install(self: &Arc<Self>) {
        if !self.config.read().enabled || self.installing.swap(true, Ordering::AcqRel) {
            return;
        }
        {
            let mut inner = self.inner.lock();
            if matches!(inner.phase, Phase::Ready | Phase::Running | Phase::Starting) {
                self.installing.store(false, Ordering::Release);
                return;
            }
            inner.phase = Phase::Installing;
            inner.message = None;
        }
        let owner = self.clone();
        std::thread::spawn(move || {
            let result = package::install(&owner.root);
            let enabled = owner.config.read().enabled;
            let mut inner = owner.inner.lock();
            match result {
                Ok(manifest) => {
                    inner.version = Some(manifest.version);
                    inner.phase = if enabled {
                        Phase::Ready
                    } else {
                        Phase::Disabled
                    };
                    inner.message = None;
                }
                Err(error) => {
                    inner.phase = if enabled {
                        Phase::Error
                    } else {
                        Phase::Disabled
                    };
                    inner.message = Some(format!("{error:#}"));
                }
            }
            owner.installing.store(false, Ordering::Release);
        });
    }

    pub fn stop(&self) {
        self.stop_target(None);
    }

    fn stop_target(&self, expected: Option<&Target>) {
        let enabled = self.config.read().enabled;
        let (process, leases) = {
            let mut inner = self.inner.lock();
            if expected.is_some_and(|expected| {
                inner.process.as_ref().is_none_or(|process| {
                    process.target.origin != expected.origin
                        || process.target.password != expected.password
                })
            }) {
                return;
            }
            inner.generation = inner.generation.wrapping_add(1);
            for lease in inner.leases.values() {
                lease.cancelled.send_replace(true);
            }
            let leases = std::mem::take(&mut inner.leases);
            inner.phase = if enabled && inner.version.is_some() {
                Phase::Ready
            } else {
                Phase::Disabled
            };
            inner.message = None;
            (inner.process.take(), leases)
        };
        drop(process);
        // Keep exclusive input ownership until the old process is dead.
        drop(leases);
    }

    pub fn retry(self: &Arc<Self>) {
        self.stop();
        self.inner.lock().phase = Phase::NotInstalled;
        self.install();
    }

    fn lease(&self, id: &str) -> Result<Arc<Lease>, ApiError> {
        if !self.config.read().enabled {
            return Err(failure(
                StatusCode::FORBIDDEN,
                "Remote control is disabled in Mobile Companion settings.",
            ));
        }
        self.inner
            .lock()
            .leases
            .get(id)
            .filter(|lease| *lease.deadline.lock() > Instant::now() && !*lease.cancelled.borrow())
            .cloned()
            .ok_or_else(|| {
                failure(
                    StatusCode::GONE,
                    "This remote-control session has ended. Connect again.",
                )
            })
    }

    fn target(&self) -> Result<Target, ApiError> {
        self.inner
            .lock()
            .process
            .as_ref()
            .map(|p| p.target.clone())
            .ok_or_else(|| {
                failure(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "The desktop stream stopped. Connect again.",
                )
            })
    }

    async fn start(&self) -> Result<Target, ApiError> {
        let _starting = self.start_lock.lock().await;
        if let Ok(target) = self.target() {
            return Ok(target);
        }
        let config = self.config.read().clone();
        if !config.enabled {
            return Err(failure(
                StatusCode::FORBIDDEN,
                "Remote control is disabled.",
            ));
        }
        let generation = {
            let mut inner = self.inner.lock();
            if !matches!(inner.phase, Phase::Ready | Phase::Running) {
                return Err(failure(
                    StatusCode::SERVICE_UNAVAILABLE,
                    inner.message.as_deref().unwrap_or(
                        "Remote control is still downloading. Try again when it is ready.",
                    ),
                ));
            }
            inner.phase = Phase::Starting;
            inner.generation
        };
        let root = self.root.clone();
        let result = tokio::task::spawn_blocking(move || -> anyhow::Result<Process> {
            let (exe, _) = package::installed(&root)?;
            let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
            let port = listener.local_addr()?.port();
            let password = format!(
                "{:032x}{:032x}",
                rand::random::<u128>(),
                rand::random::<u128>()
            );
            let mut command = Command::new(exe);
            command
                .args([
                    "--addr=127.0.0.1",
                    &format!("--port={port}"),
                    "--title=Bro remote desktop",
                    "--audio-enabled=false",
                    "--enable-clipboard=true",
                    "--file-transfers=none",
                    "--enable-resize=false",
                    "--framerate=60",
                    "--stripes=1",
                    "--max-kbps=12000",
                    "--min-kbps=800",
                    "--max-capture-width=2560",
                    "--max-capture-height=1440",
                    "--max-capture-pixels=3686400",
                    &format!("--display={}", config.display),
                ])
                .env("VOXL_PASSWORD", &password)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            drop(listener);
            let (child, tree) = ProcessTree::spawn(&mut command)?;
            Ok(Process {
                child,
                tree,
                target: Target {
                    origin: format!("http://127.0.0.1:{port}"),
                    password,
                },
            })
        })
        .await
        .map_err(|_| {
            failure(
                StatusCode::INTERNAL_SERVER_ERROR,
                "The remote-control worker stopped.",
            )
        })?;
        let mut process = match result {
            Ok(process) => process,
            Err(error) => {
                self.start_failed(generation, format!("{error:#}"));
                return Err(failure(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Could not start remote control. Check Mobile Companion settings.",
                ));
            }
        };
        let mut ready = false;
        for _ in 0..60 {
            if self.inner.lock().generation != generation {
                return Err(failure(StatusCode::GONE, "Remote control was turned off."));
            }
            if process.child.try_wait().ok().flatten().is_some() {
                break;
            }
            let target = &process.target;
            if let Ok(response) = self
                .http
                .get(format!("{}/stats", target.origin))
                .bearer_auth(&target.password)
                .timeout(Duration::from_millis(500))
                .send()
                .await
                && response.status().is_success()
            {
                ready = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        if !ready {
            self.start_failed(
                generation,
                "The desktop stream did not start. Unlock Windows and try again.".into(),
            );
            return Err(failure(
                StatusCode::SERVICE_UNAVAILABLE,
                "The desktop stream did not start. Unlock Windows and try again.",
            ));
        }
        let mut inner = self.inner.lock();
        if inner.generation != generation {
            return Err(failure(StatusCode::GONE, "Remote control was turned off."));
        }
        let target = process.target.clone();
        inner.process = Some(process);
        inner.phase = Phase::Running;
        inner.idle_since = Instant::now();
        Ok(target)
    }

    fn start_failed(&self, generation: u64, message: String) {
        let mut inner = self.inner.lock();
        if inner.generation == generation {
            inner.phase = Phase::Error;
            inner.message = Some(message);
        }
    }

    async fn close(&self, id: &str) {
        let lease = { self.inner.lock().leases.remove(id) };
        if let Some(lease) = lease {
            lease.cancelled.send_replace(true);
            let rtc = lease.rtc_id.lock().take();
            if let (Some(peer), Ok(target)) = (rtc, self.target()) {
                let closed = self
                    .http
                    .delete(format!("{}/rtc/{peer}", target.origin))
                    .bearer_auth(&target.password)
                    .timeout(Duration::from_secs(2))
                    .send()
                    .await;
                if !closed.is_ok_and(|response| {
                    response.status().is_success() || response.status() == StatusCode::NOT_FOUND
                }) {
                    // A WebRTC peer can still reach us over UDP even if its HTTP endpoint is stuck.
                    self.stop_target(Some(&target));
                }
            }
        }
    }

    async fn ice_servers(&self) -> (Value, Option<String>) {
        let fallback = json!([{"urls":["stun:stun.cloudflare.com:3478"]}]);
        let Some(config) = std::fs::read(self.root.join("relay.json"))
            .ok()
            .filter(|data| data.len() < 16 * 1024)
            .and_then(|data| serde_json::from_slice::<Value>(&data).ok())
        else {
            return (fallback, None);
        };
        let Some(url) = config["brokerUrl"]
            .as_str()
            .filter(|url| url.starts_with("https://"))
        else {
            return (fallback, None);
        };
        let Some(token) = config["token"].as_str() else {
            return (fallback, None);
        };
        let result = self
            .http
            .post(url)
            .bearer_auth(token)
            .json(&json!({}))
            .timeout(Duration::from_secs(8))
            .send()
            .await;
        if let Ok(response) = result
            && response.status().is_success()
            && let Ok(body) = response.json::<Value>().await
            && let Some(servers) = body["iceServers"]
                .as_array()
                .filter(|servers| !servers.is_empty() && servers.len() <= 8)
        {
            return (Value::Array(servers.clone()), None);
        }
        (
            fallback,
            Some(
                "The relay is unavailable. Trying a direct connection, with TCP as fallback."
                    .into(),
            ),
        )
    }

    async fn reap_closed_peers(&self) {
        let peers: Vec<(String, String)> = {
            let inner = self.inner.lock();
            inner
                .leases
                .values()
                .filter_map(|lease| {
                    lease
                        .rtc_id
                        .lock()
                        .clone()
                        .map(|peer| (lease.id.clone(), peer))
                })
                .collect()
        };
        if peers.is_empty() {
            return;
        }
        let Ok(target) = self.target() else {
            return;
        };
        let response = self
            .http
            .get(format!("{}/rtc", target.origin))
            .bearer_auth(&target.password)
            .timeout(Duration::from_secs(2))
            .send()
            .await;
        // An unavailable inventory is not evidence that a peer has closed.
        if let Ok(response) = response
            && response.status().is_success()
            && let Ok(inventory) = response.json::<Value>().await
            && let Some(active) = inventory["peers"].as_array().filter(|active| {
                let mut ids = std::collections::HashSet::new();
                active.len() <= 32
                    && active.iter().all(|entry| {
                        entry["id"]
                            .as_str()
                            .is_some_and(|id| safe_id(id) && ids.insert(id))
                    })
            })
        {
            for (lease, peer) in peers {
                if !active
                    .iter()
                    .any(|entry| entry["id"].as_str() == Some(&peer))
                {
                    self.close(&lease).await;
                }
            }
        }
    }
}

pub(crate) fn spawn(state: &AppState) {
    let rmm = state.rmm.clone();
    let mut shutdown = state.shutdown.subscribe();
    tokio::spawn(async move {
        let mut last_peer_check = Instant::now();
        loop {
            tokio::select! { _ = tokio::time::sleep(Duration::from_secs(1)) => {}, _ = shutdown.changed() => break }
            let expired: Vec<String> = {
                let inner = rmm.inner.lock();
                inner
                    .leases
                    .values()
                    .filter(|lease| *lease.deadline.lock() <= Instant::now())
                    .map(|lease| lease.id.clone())
                    .collect()
            };
            for id in expired {
                rmm.close(&id).await;
            }
            if last_peer_check.elapsed() > Duration::from_secs(5) {
                rmm.reap_closed_peers().await;
                last_peer_check = Instant::now();
            }
            let process = {
                let mut inner = rmm.inner.lock();
                if !inner.leases.is_empty() {
                    inner.idle_since = Instant::now();
                }
                let exited = inner
                    .process
                    .as_mut()
                    .is_some_and(|p| p.child.try_wait().ok().flatten().is_some());
                if exited
                    || (inner.process.is_some()
                        && inner.leases.is_empty()
                        && inner.idle_since.elapsed() > IDLE_STOP)
                {
                    for lease in inner.leases.values() {
                        lease.cancelled.send_replace(true);
                    }
                    inner.leases.clear();
                    inner.phase = Phase::Ready;
                    inner.process.take()
                } else {
                    None
                }
            };
            drop(process);
        }
        rmm.stop();
    });
}

type ApiError = (StatusCode, Json<Value>);
fn failure(code: StatusCode, message: &str) -> ApiError {
    (code, Json(json!({"message": message})))
}

pub(crate) fn routes(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/api/rmm", get(status))
        .route("/api/rmm/sessions", post(open))
        .route("/api/rmm/sessions/{id}", axum::routing::delete(close))
        .route("/api/rmm/sessions/{id}/heartbeat", post(heartbeat))
        .route("/rmm/{id}/{*path}", any(proxy))
        .route_layer(middleware::from_fn_with_state(state, paired))
}

/// Unlike ordinary loopback tools, desktop control always needs the pairing token.
async fn paired(State(state): State<AppState>, request: Request<Body>, next: Next) -> Response {
    if !paired_request(request.headers(), request.uri(), &state.token) {
        return failure(
            StatusCode::UNAUTHORIZED,
            "Pair with Bro before opening remote control.",
        )
        .into_response();
    }
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    response.headers_mut().insert(
        "referrer-policy",
        header::HeaderValue::from_static("no-referrer"),
    );
    response.headers_mut().insert(
        "x-content-type-options",
        header::HeaderValue::from_static("nosniff"),
    );
    response
}

fn paired_request(headers: &HeaderMap, uri: &Uri, token: &str) -> bool {
    let synthetic_peer = Some(std::net::SocketAddr::from(([192, 0, 2, 1], 1)));
    if !crate::auth::is_authorized(synthetic_peer, headers, uri, token) {
        return false;
    }
    if headers
        .get("sec-fetch-site")
        .is_some_and(|v| v == "cross-site")
    {
        return false;
    }
    if let Some(origin) = headers.get(header::ORIGIN) {
        let host = headers.get(header::HOST).and_then(|h| h.to_str().ok());
        let Ok(origin) = origin
            .to_str()
            .ok()
            .and_then(|o| url::Url::parse(o).ok())
            .ok_or(())
        else {
            return false;
        };
        let expected = origin.host_str().map(|h| match origin.port() {
            Some(port) => format!("{h}:{port}"),
            None => h.to_owned(),
        });
        if !matches!(origin.scheme(), "http" | "https") || expected.as_deref() != host {
            return false;
        }
    }
    true
}

async fn status(State(state): State<AppState>) -> Json<Status> {
    Json(state.rmm.status())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Open {
    #[serde(default)]
    read_only: bool,
    #[serde(default = "default_transport")]
    transport: String,
}
fn default_transport() -> String {
    "webrtc".into()
}

async fn open(
    State(state): State<AppState>,
    Json(request): Json<Open>,
) -> Result<Json<Value>, ApiError> {
    if !matches!(request.transport.as_str(), "webrtc" | "websocket") {
        return Err(failure(StatusCode::BAD_REQUEST, "Choose WebRTC or TCP."));
    }
    let (settings, generation) = {
        let settings = state.rmm.config.read();
        (settings.clone(), state.rmm.inner.lock().generation)
    };
    if !settings.enabled {
        return Err(failure(
            StatusCode::FORBIDDEN,
            "Enable remote control in Bro’s Mobile Companion settings first.",
        ));
    }
    let read_only = settings.read_only || request.read_only;
    let control_lock = if read_only {
        None
    } else {
        Some(package::lock(&state.rmm.root, "control.lock")
        .map_err(|_| failure(StatusCode::CONFLICT, "Another device is controlling this computer. End its connection or choose View only."))?)
    };
    // Fetch relay credentials before starting capture; a slow broker cannot hold up the screen worker.
    let (ice_servers, message) = if request.transport == "webrtc" {
        state.rmm.ice_servers().await
    } else {
        (json!([]), None)
    };
    state.rmm.start().await?;
    let lease = Arc::new(Lease {
        id: uuid::Uuid::new_v4().simple().to_string(),
        read_only,
        transport: request.transport,
        deadline: Mutex::new(Instant::now() + LEASE_TTL),
        reconnect_at: Instant::now() + Duration::from_secs(50 * 60),
        cancelled: watch::channel(false).0,
        attached: AtomicBool::new(false),
        rtc_id: Mutex::new(None),
        ice_servers,
        _control_lock: control_lock,
    });
    let current_config = state.rmm.config.read();
    let mut inner = state.rmm.inner.lock();
    if inner.leases.len() >= MAX_VIEWERS {
        return Err(failure(
            StatusCode::CONFLICT,
            "This computer already has four remote viewers.",
        ));
    }
    if *current_config != settings || inner.generation != generation || inner.process.is_none() {
        return Err(failure(
            StatusCode::GONE,
            "Remote-control settings changed. Connect again.",
        ));
    }
    let result = lease_json(&lease, message);
    inner.leases.insert(lease.id.clone(), lease);
    Ok(Json(result))
}

fn lease_json(lease: &Lease, message: Option<String>) -> Value {
    let timestamp = |instant: Instant| {
        (chrono::Utc::now()
            + chrono::Duration::from_std(instant.saturating_duration_since(Instant::now()))
                .unwrap_or_default())
        .to_rfc3339()
    };
    json!({"id":lease.id,"basePath":format!("/rmm/{}/",lease.id),"wsPath":format!("/rmm/{}/ws",lease.id),
        "rtcPath":format!("/rmm/{}/rtc/offer",lease.id),"readOnly":lease.read_only,"transport":lease.transport,
        "iceServers":lease.ice_servers,"expiresAt":timestamp(*lease.deadline.lock()),"reconnectAt":timestamp(lease.reconnect_at),"message":message})
}

async fn close(State(state): State<AppState>, Path(id): Path<String>) -> StatusCode {
    state.rmm.close(&id).await;
    StatusCode::NO_CONTENT
}

async fn heartbeat(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let lease = state.rmm.lease(&id)?;
    *lease.deadline.lock() = (Instant::now() + LEASE_TTL).min(lease.reconnect_at);
    Ok(Json(lease_json(&lease, None)))
}

async fn proxy(
    State(state): State<AppState>,
    Path((id, path)): Path<(String, String)>,
    ws: Result<WebSocketUpgrade, axum::extract::ws::rejection::WebSocketUpgradeRejection>,
    request: Request<Body>,
) -> Result<Response, ApiError> {
    let lease = state.rmm.lease(&id)?;
    let target = state.rmm.target()?;
    match (request.method(), path.as_str()) {
        (&Method::GET, "ws") if lease.transport == "websocket" => {
            let ws = ws.map_err(|_| {
                failure(
                    StatusCode::BAD_REQUEST,
                    "A WebSocket connection is required.",
                )
            })?;
            if lease.attached.swap(true, Ordering::AcqRel) {
                return Err(failure(
                    StatusCode::CONFLICT,
                    "This viewer is already connected.",
                ));
            }
            let url = format!(
                "{}/ws?readOnly={}",
                target.origin.replacen("http:", "ws:", 1),
                lease.read_only
            );
            let mut upstream = url.into_client_request().map_err(|_| {
                failure(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Could not open the desktop stream.",
                )
            })?;
            upstream.headers_mut().insert(
                header::AUTHORIZATION,
                format!("Bearer {}", target.password).parse().unwrap(),
            );
            let connection = tokio::time::timeout(
                Duration::from_secs(5),
                tokio_tungstenite::connect_async(upstream),
            )
            .await;
            let socket = match connection {
                Ok(Ok((socket, _))) => socket,
                _ => {
                    lease.attached.store(false, Ordering::Release);
                    return Err(failure(
                        StatusCode::BAD_GATEWAY,
                        "The desktop stream closed. Connect again.",
                    ));
                }
            };
            if *lease.cancelled.borrow() {
                return Err(failure(
                    StatusCode::GONE,
                    "This remote-control session has ended.",
                ));
            }
            Ok(ws
                .max_message_size(256 * 1024)
                .on_upgrade(move |client| relay(client, socket, lease, state.rmm))
                .into_response())
        }
        (&Method::POST, "rtc/offer") if lease.transport == "webrtc" => {
            if lease.attached.swap(true, Ordering::AcqRel) {
                return Err(failure(
                    StatusCode::CONFLICT,
                    "This viewer is already connected.",
                ));
            }
            let result = async {
                let body = to_bytes(request.into_body(), 256 * 1024)
                    .await
                    .map_err(|_| {
                        failure(
                            StatusCode::PAYLOAD_TOO_LARGE,
                            "The connection offer is too large.",
                        )
                    })?;
                let mut offer: Value = serde_json::from_slice(&body).map_err(|_| {
                    failure(StatusCode::BAD_REQUEST, "The connection offer is invalid.")
                })?;
                if !offer.is_object()
                    || offer["type"] != "offer"
                    || offer["sdp"].as_str().is_none_or(|s| s.len() > 240 * 1024)
                {
                    return Err(failure(
                        StatusCode::BAD_REQUEST,
                        "The connection offer is invalid.",
                    ));
                }
                offer["readOnly"] = json!(lease.read_only);
                offer["iceServers"] = lease.ice_servers.clone();
                let response = state
                    .rmm
                    .http
                    .post(format!("{}/rtc/offer", target.origin))
                    .bearer_auth(&target.password)
                    .json(&offer)
                    .send()
                    .await
                    .map_err(|_| {
                        failure(
                            StatusCode::BAD_GATEWAY,
                            "WebRTC could not connect. Try TCP.",
                        )
                    })?;
                let code = response.status();
                let answer: Value = response.json().await.map_err(|_| {
                    failure(
                        StatusCode::BAD_GATEWAY,
                        "The desktop returned an invalid connection answer.",
                    )
                })?;
                if !code.is_success() {
                    return Err(failure(
                        StatusCode::BAD_GATEWAY,
                        "WebRTC could not connect. Try TCP.",
                    ));
                }
                let peer = answer["id"]
                    .as_str()
                    .filter(|id| safe_id(id))
                    .ok_or_else(|| {
                        failure(
                            StatusCode::BAD_GATEWAY,
                            "The desktop returned an invalid peer id.",
                        )
                    })?
                    .to_owned();
                *lease.rtc_id.lock() = Some(peer.clone());
                if *lease.cancelled.borrow() {
                    let closed = state
                        .rmm
                        .http
                        .delete(format!("{}/rtc/{peer}", target.origin))
                        .bearer_auth(&target.password)
                        .send()
                        .await;
                    if !closed.is_ok_and(|response| {
                        response.status().is_success() || response.status() == StatusCode::NOT_FOUND
                    }) {
                        state.rmm.stop_target(Some(&target));
                    }
                    return Err(failure(
                        StatusCode::GONE,
                        "This remote-control session has ended.",
                    ));
                }
                Ok(Json(answer).into_response())
            }
            .await;
            if result.is_err() {
                lease.attached.store(false, Ordering::Release);
            }
            result
        }
        (&Method::DELETE, _) if path.starts_with("rtc/") => {
            let peer = lease.rtc_id.lock().clone();
            if peer.as_deref() != path.strip_prefix("rtc/") {
                return Err(failure(StatusCode::NOT_FOUND, "Unknown remote viewer."));
            }
            state.rmm.close(&lease.id).await;
            Ok(StatusCode::NO_CONTENT.into_response())
        }
        (&Method::GET, "stats" | "screenshot") => {
            let response = state
                .rmm
                .http
                .get(format!("{}/{path}", target.origin))
                .bearer_auth(&target.password)
                .send()
                .await
                .map_err(|_| failure(StatusCode::BAD_GATEWAY, "The desktop stream stopped."))?;
            let code = response.status();
            let content = response.headers().get(header::CONTENT_TYPE).cloned();
            let bytes = response.bytes().await.map_err(|_| {
                failure(
                    StatusCode::BAD_GATEWAY,
                    "Could not read the desktop stream.",
                )
            })?;
            let mut response = (code, bytes).into_response();
            if let Some(content) = content {
                response.headers_mut().insert(header::CONTENT_TYPE, content);
            }
            Ok(response)
        }
        _ => Err(failure(
            StatusCode::NOT_FOUND,
            "This remote-control route is unavailable.",
        )),
    }
}

fn safe_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
}

fn viewer_command(text: &str, read_only: bool) -> bool {
    let name = text.split(',').next().unwrap_or("");
    if read_only {
        return matches!(name, "a" | "v" | "au" | "kf" | "po");
    }
    matches!(
        name,
        "a" | "v" | "au" | "kf" | "po" | "m" | "b" | "s" | "sp" | "k" | "r" | "c"
    )
}

async fn relay(
    client: WebSocket,
    upstream: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    lease: Arc<Lease>,
    rmm: Arc<Rmm>,
) {
    if *lease.cancelled.borrow() {
        rmm.close(&lease.id).await;
        return;
    }
    let (mut client_tx, mut client_rx) = client.split();
    let (mut host_tx, mut host_rx) = upstream.split();
    let mut cancelled = lease.cancelled.subscribe();
    let to_host = async {
        while let Some(Ok(message)) = client_rx.next().await {
            let message = match message {
                Message::Text(text) if viewer_command(&text, lease.read_only) => {
                    tungstenite::Message::Text(text.to_string().into())
                }
                Message::Ping(bytes) => tungstenite::Message::Ping(bytes),
                Message::Pong(bytes) => tungstenite::Message::Pong(bytes),
                Message::Close(_) => break,
                _ => continue,
            };
            if host_tx.send(message).await.is_err() {
                break;
            }
        }
    };
    let to_phone = async {
        while let Some(Ok(message)) = host_rx.next().await {
            let message = match message {
                tungstenite::Message::Text(text) => Message::Text(text.to_string().into()),
                tungstenite::Message::Binary(bytes) => Message::Binary(bytes),
                tungstenite::Message::Ping(bytes) => Message::Ping(bytes),
                tungstenite::Message::Pong(bytes) => Message::Pong(bytes),
                tungstenite::Message::Close(_) => break,
                _ => continue,
            };
            // Bound a slow reader. Voxl's acknowledgements provide normal flow control.
            if !matches!(
                tokio::time::timeout(Duration::from_secs(5), client_tx.send(message)).await,
                Ok(Ok(()))
            ) {
                break;
            }
        }
    };
    tokio::select! { biased; _ = cancelled.wait_for(|stopped| *stopped) => {}, _ = to_host => {}, _ = to_phone => {} }
    rmm.close(&lease.id).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn localhost_still_requires_pairing_and_cross_site_is_refused() {
        let uri: Uri = "/api/rmm".parse().unwrap();
        let mut headers = HeaderMap::new();
        assert!(!paired_request(&headers, &uri, "secret"));
        headers.insert(header::AUTHORIZATION, "Bearer secret".parse().unwrap());
        assert!(paired_request(&headers, &uri, "secret"));
        headers.insert(header::HOST, "127.0.0.1:10001".parse().unwrap());
        headers.insert(header::ORIGIN, "https://other.example".parse().unwrap());
        assert!(!paired_request(&headers, &uri, "secret"));
        headers.insert(header::ORIGIN, "http://127.0.0.1:10001".parse().unwrap());
        assert!(paired_request(&headers, &uri, "secret"));
        headers.insert("sec-fetch-site", "cross-site".parse().unwrap());
        assert!(!paired_request(&headers, &uri, "secret"));
    }

    #[test]
    fn view_only_cannot_inject_keys_pointer_clipboard_or_release_controller() {
        for command in [
            "m,12,13",
            "b,1,1",
            "sp,0,15",
            "k,65,1",
            "c,secret",
            "r",
            "mi,1",
            "sz,800,600",
        ] {
            assert!(!viewer_command(command, true), "{command}");
        }
        for command in ["a,1", "v,1", "kf", "po,123"] {
            assert!(viewer_command(command, true));
        }
        assert!(viewer_command("m,12,13", false));
        assert!(!viewer_command("sz,800,600", false));
    }

    #[tokio::test]
    async fn disabled_feature_has_no_process_or_network_install() {
        let root = tempfile::tempdir().unwrap();
        let rmm = Arc::new(Rmm::new(root.path().to_owned()));
        assert_eq!(rmm.status().phase, Phase::Disabled);
        rmm.install();
        assert!(!rmm.installing.load(Ordering::Acquire));
        assert!(!root.path().join("packages").exists());
        assert!(rmm.start().await.is_err());
        assert!(rmm.lease("unknown").is_err());
    }

    #[tokio::test]
    async fn disable_revokes_existing_leases_and_heartbeat_cannot_restore_them() {
        let root = tempfile::tempdir().unwrap();
        let rmm = Arc::new(Rmm::new(root.path().to_owned()));
        rmm.config.write().enabled = true;
        let lease = Arc::new(Lease {
            id: "viewer".into(),
            read_only: true,
            transport: "webrtc".into(),
            deadline: Mutex::new(Instant::now() + LEASE_TTL),
            reconnect_at: Instant::now() + Duration::from_secs(50 * 60),
            cancelled: watch::channel(false).0,
            attached: AtomicBool::new(false),
            rtc_id: Mutex::new(None),
            ice_servers: json!([]),
            _control_lock: None,
        });
        rmm.inner
            .lock()
            .leases
            .insert(lease.id.clone(), lease.clone());
        assert!(rmm.lease("viewer").is_ok());
        rmm.configure(package::Settings::default());
        assert!(*lease.cancelled.borrow());
        assert!(rmm.lease("viewer").is_err());
        assert_eq!(rmm.status().active_viewers, 0);
    }
}
