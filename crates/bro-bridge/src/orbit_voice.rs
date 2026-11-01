//! Orbit's independent, authenticated loopback control channel.
//!
//! This is deliberately not part of the LAN bridge router or its authentication.
//! Receipt is ephemeral: only the durable inbox owner may acknowledge `queued`.

use anyhow::{Context, Result, bail};
use axum::extract::{ConnectInfo, DefaultBodyLimit, Path as RoutePath, Query, State};
use axum::http::{HeaderMap, Request, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine;
use parking_lot::Mutex;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, VecDeque};
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::{Semaphore, watch};

mod stage;
mod lifecycle;
mod tools;
pub use lifecycle::{CredentialSource, Microphone, Phase, Preferences, Provider, RuntimeFailure, RuntimeStatus, VoiceControl};
pub use stage::{
    StageFrame, StageKind, StagePoint, StageRect, StageSurface, StageViewport, StageWindow,
};

const PROTOCOL: u32 = 1;
const MAX_BODY: usize = 32 * 1024;
const MAX_ID: usize = 128;
const MAX_TEXT: usize = 16 * 1024;
const MAX_PROGRESS: usize = 4096;
const MAX_REQUESTS: usize = 512;
const MAX_UTTERANCES: usize = 1024;
const MAX_EVENTS: usize = 256;
const MAX_OUTCOMES: usize = 16;
const EVENT_BATCH: usize = 128;
const MAX_SESSIONS: usize = 256;
const MAX_HOST_BYTES: usize = 256 * 1024;
const THREAD_NAME: &str = "bro-orbit-voice";
const CAPABILITIES: &[&str] = &[
    "sessions.read",
    "sessions.focus",
    "orchestrator.submit",
    "requests.observe",
    "requests.cancel-queued",
    "avatar.desktop",
    "avatar.attached",
];

/// Transport commands, never raw terminal input. Callbacks must enqueue and return
/// promptly. The host must validate the pinned generation again before dispatch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    Submit {
        id: String,
        utterance_id: String,
        text: String,
        focused: Option<String>,
        generation: Option<String>,
    },
    Cancel {
        id: String,
    },
    Focus {
        session_id: String,
    },
}

/// Cheap to clone. The last handle stops the independent server.
#[derive(Clone)]
pub struct Handle {
    inner: Arc<HandleInner>,
}

struct HandleInner {
    shared: Arc<Shared>,
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
}

struct Shared {
    instance_id: String,
    token: String,
    authority: String,
    url: String,
    descriptor_path: PathBuf,
    lineage: String,
    lifecycle: Mutex<lifecycle::Lifecycle>,
    workspace: Mutex<Option<crate::Bridge>>,
    callback: Arc<dyn Fn(Command) + Send + Sync>,
    data: Mutex<Data>,
    running: AtomicBool,
    work_enabled: AtomicBool,
    shutdown: watch::Sender<bool>,
    in_flight: Arc<Semaphore>,
    stage: watch::Sender<stage::Snapshot>,
    stage_in_flight: Arc<Semaphore>,
    stage_rate: Mutex<stage::Rate>,
    last_seen: Mutex<Option<Instant>>,
    error: Mutex<Option<String>>,
}

struct Data {
    host: Value,
    published: bool,
    utterances: HashMap<String, Option<Target>>,
    read_scopes: HashMap<String, Vec<Target>>,
    read_scope_order: VecDeque<String>,
    requests: HashMap<String, RequestRecord>,
    events: VecDeque<Value>,
    sequence: u64,
    rate_start: Instant,
    rate_count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct Target {
    session_id: String,
    generation: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    scope: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RequestRecord {
    id: String,
    utterance_id: String,
    text: String,
    target: Option<Target>,
    state: RequestState,
    progress: Option<TextField>,
    result: Option<ResultField>,
    outcome: Option<ResultField>,
    error: Option<String>,
    #[serde(skip)]
    cancel_pending: bool,
    #[serde(skip)]
    outcome_keys: std::collections::HashSet<[u8; 32]>,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
enum RequestState {
    Received,
    Queued,
    Dispatched,
    Completed,
    Failed,
    Cancelled,
    Uncertain,
}

impl RequestState {
    fn terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

#[derive(Clone, Serialize)]
struct TextField {
    text: String,
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct ResultField {
    text: String,
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    task_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    revision: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    delivery_id: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Descriptor {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    lineage: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    owner_epoch: Option<String>,
    protocol: u32,
    instance_id: String,
    url: String,
    token: String,
}

/// Start an ephemeral IPv4-loopback server and publish
/// `<data_root>/orbit/connection.json`. This works without the normal bridge.
/// The token is never exposed through the handle or any HTTP response.
pub fn start(
    data_root: impl AsRef<Path>,
    callback: impl Fn(Command) + Send + Sync + 'static,
) -> Result<Handle> {
    start_service(data_root.as_ref(), callback, false)
}

/// Managed lifecycle owner. The kernel lock covers every GUI/TUI/background role.
pub fn start_managed(
    data_root: impl AsRef<Path>,
    callback: impl Fn(Command) + Send + Sync + 'static,
) -> Result<Handle> {
    start_service(data_root.as_ref(), callback, true)
}

/// Distinguish an occupied owner lease from setup, ACL, state or listener failures.
#[derive(Debug)]
pub struct ManagedOwnerBusy;
impl std::fmt::Display for ManagedOwnerBusy {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Orbit Voice is owned by another bro workspace driver")
    }
}
impl std::error::Error for ManagedOwnerBusy {}

fn managed_owner(directory: &Path) -> Result<std::fs::File> {
    let file = private_open(&directory.join("managed-owner.lock"), false)?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(std::fs::TryLockError::WouldBlock) => Err(ManagedOwnerBusy.into()),
        Err(std::fs::TryLockError::Error(error)) => Err(error.into()),
    }
}

fn start_service(data_root: &Path, callback: impl Fn(Command) + Send + Sync + 'static, managed: bool) -> Result<Handle> {
    let directory = data_root.join("orbit");
    private_directory(&directory).context("protecting Orbit discovery directory")?;
    let owner = if managed { Some(managed_owner(&directory)?) } else { None };
    let lifecycle = lifecycle::Lifecycle::load(&directory, managed)?;
    let lineage = if managed {
        let path = directory.join("pairing-lineage.json");
        match std::fs::read(&path) {
            Ok(bytes) if bytes.len() <= 128 => {
                let id: String = serde_json::from_slice(&bytes).context("Invalid Orbit pairing lineage")?;
                uuid::Uuid::parse_str(&id).context("Invalid Orbit pairing lineage")?;
                id
            }
            Ok(_) => bail!("Invalid Orbit pairing lineage"),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                let id = uuid::Uuid::new_v4().to_string();
                crate::project_store::write_atomic(&path, &serde_json::to_vec(&id)?)?;
                id
            }
            Err(e) => return Err(e.into()),
        }
    } else { uuid::Uuid::new_v4().to_string() };
    let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .context("binding Orbit loopback listener")?;
    listener.set_nonblocking(true)?;
    let authority = listener.local_addr()?.to_string();
    let instance_id = uuid::Uuid::new_v4().to_string();
    let mut secret = [0u8; 32];
    rand::rng().fill_bytes(&mut secret);
    let token = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(secret);
    let (shutdown, _) = watch::channel(false);
    let shared = Arc::new(Shared {
        instance_id: instance_id.clone(),
        token,
        url: format!("http://{authority}"),
        authority,
        descriptor_path: directory.join("connection.json"),
        lineage,
        lifecycle: Mutex::new(lifecycle),
        workspace: Mutex::new(None),
        callback: Arc::new(callback),
        data: Mutex::new(Data::new(&instance_id)),
        running: AtomicBool::new(false),
        work_enabled: AtomicBool::new(true),
        shutdown,
        in_flight: Arc::new(Semaphore::new(32)),
        stage: watch::channel(stage::Snapshot::default()).0,
        stage_in_flight: Arc::new(Semaphore::new(stage::MAX_WATCHERS)),
        stage_rate: Mutex::new(stage::Rate::new()),
        last_seen: Mutex::new(None),
        error: Mutex::new(None),
    });
    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
    let server = shared.clone();
    let thread = std::thread::Builder::new()
        .name(THREAD_NAME.into())
        .spawn(move || {
            let _owner = owner; // release only after the old server has actually exited
            let runtime = match tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .worker_threads(2)
                .thread_name("bro-orbit-voice-worker")
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    let _ = ready_tx.send(Err(error.to_string()));
                    return;
                }
            };
            runtime.block_on(async {
                let listener = match tokio::net::TcpListener::from_std(listener) {
                    Ok(listener) => listener,
                    Err(error) => {
                        let _ = ready_tx.send(Err(error.to_string()));
                        return;
                    }
                };
                if let Err(error) = write_descriptor(&server) {
                    remove_owned_descriptor(&server);
                    let _ = ready_tx.send(Err(format!("publishing Orbit discovery: {error}")));
                    return;
                }
                server.running.store(true, Ordering::Release);
                if ready_tx.send(Ok(())).is_ok() {
                    let mut stop = server.shutdown.subscribe();
                    let result = axum::serve(
                        listener,
                        router(server.clone()).into_make_service_with_connect_info::<SocketAddr>(),
                    )
                    .with_graceful_shutdown(async move {
                        let _ = stop.wait_for(|stopped| *stopped).await;
                    })
                    .await;
                    if let Err(error) = result {
                        *server.error.lock() = Some(error.to_string());
                    }
                }
                server.running.store(false, Ordering::Release);
                remove_owned_descriptor(&server);
            });
            runtime.shutdown_timeout(Duration::from_millis(200));
        })?;
    match ready_rx.recv() {
        Ok(Ok(())) => Ok(Handle {
            inner: Arc::new(HandleInner {
                shared,
                thread: Mutex::new(Some(thread)),
            }),
        }),
        outcome => {
            let _ = thread.join();
            let message = match outcome {
                Ok(Err(message)) => message,
                Err(error) => error.to_string(),
                Ok(Ok(())) => unreachable!(),
            };
            bail!("Orbit control server failed to start: {message}")
        }
    }
}

impl Handle {
    /// Attach the same canonical local workspace used by the optional LAN bridge.
    pub fn attach_workspace(&self, workspace: Option<crate::Bridge>) {
        let mut current = self.inner.shared.workspace.lock();
        if self.work_enabled() { *current = workspace; }
    }

    pub fn work_enabled(&self) -> bool { self.inner.shared.work_enabled.load(Ordering::Acquire) }

    /// Cheap immediate revocation; the independent native lifecycle lane stays alive
    /// long enough to apply Stop/Shutdown. No accepted work is cancelled or replayed.
    pub fn revoke_work(&self) {
        self.inner.shared.work_enabled.store(false, Ordering::Release);
        self.inner.shared.workspace.lock().take();
        let mut data = self.inner.shared.data.lock();
        data.published = false;
        data.host["sessions"] = json!([]);
        data.host["focusedSessionId"] = Value::Null;
        if let Some(capabilities) = data.host["capabilities"].as_array_mut() { capabilities.retain(|cap| cap != tools::CAPABILITY); }
        if let Some(host) = data.host.as_object_mut() { host.remove("toolCatalogRevision"); }
        stage::replace(&self.inner.shared.stage, None);
    }

    /// Publish only the protocol's safe host fields. Invalid snapshots leave the
    /// previous snapshot untouched; arbitrary extra fields are not advertised.
    pub fn publish_host(&self, snapshot: Value) -> bool {
        let shared = &self.inner.shared;
        let Some(mut snapshot) = normalize_host(snapshot, &shared.instance_id) else {
            return false;
        };
        if shared.lifecycle.lock().enabled() {
            snapshot["capabilities"].as_array_mut().unwrap().push(json!(lifecycle::CAPABILITY));
            snapshot["lineage"] = json!(shared.lineage);
            snapshot["ownerEpoch"] = json!(shared.instance_id);
        }
        if shared.workspace.lock().is_some() {
            snapshot["capabilities"].as_array_mut().unwrap().push(json!(tools::CAPABILITY));
            snapshot["toolCatalogRevision"] = json!(tools::REVISION);
        }
        let mut data = shared.data.lock();
        if !shared.work_enabled.load(Ordering::Acquire) { return false; }
        if data.published && data.host == snapshot {
            return true;
        }
        // Native identity changes invalidate geometry before exposing the new host.
        // Ordinary session/focus publishes do not replace the independent stage.
        if data.host["window"] != snapshot["window"] {
            stage::replace(&shared.stage, None);
        }
        data.host = snapshot;
        data.published = true;
        data.event("host", None, json!({}));
        true
    }

    /// Publish latest-only client-local geometry. Invalid frames clear any old
    /// geometry and return false without rejecting the valid host/session state.
    /// Publish the current host window before publishing its stage.
    pub fn publish_stage(&self, frame: Option<StageFrame>) -> bool {
        let shared = &self.inner.shared;
        let valid_geometry = frame.as_ref().is_none_or(stage::valid_frame);
        let data = shared.data.lock();
        let valid = valid_geometry
            && shared.work_enabled.load(Ordering::Acquire)
            && shared.running.load(Ordering::Acquire)
            && frame.as_ref().is_none_or(|frame| {
                data.published && stage::matches_window(frame, &data.host["window"])
            });
        stage::replace(&shared.stage, if valid { frame } else { None });
        valid
    }

    /// Stage revisions are independent of the bounded request-event sequence.
    pub fn stage_revision(&self) -> u64 {
        self.inner.shared.stage.borrow().revision
    }

    /// Call only after the corresponding inbox write has been durably saved.
    pub fn queued(&self, id: &str) -> bool {
        self.state(id, RequestState::Queued)
    }

    pub fn dispatched(&self, id: &str) -> bool {
        self.state(id, RequestState::Dispatched)
    }

    pub fn progress(&self, id: &str, text: &str) -> bool {
        let mut data = self.inner.shared.data.lock();
        let Some(request) = data.requests.get_mut(id) else {
            return false;
        };
        if request.state.terminal() {
            return false;
        }
        let progress = TextField {
            text: plain_text(text, MAX_PROGRESS),
        };
        request.progress = Some(progress.clone());
        data.event("request.progress", Some(id), json!({"progress":progress}));
        true
    }

    pub fn finish(&self, id: &str, text: &str, ok: bool) -> bool {
        let mut data = self.inner.shared.data.lock();
        let Some(request) = data.requests.get_mut(id) else {
            return false;
        };
        if request.state.terminal() {
            return false;
        }
        let result = ResultField {
            text: plain_text(text, MAX_TEXT),
            ok,
            ..Default::default()
        };
        request.state = if ok {
            RequestState::Completed
        } else {
            RequestState::Failed
        };
        request.result = Some(result.clone());
        request.cancel_pending = false;
        let state = request.state;
        data.event(
            "request.result",
            Some(id),
            json!({"state":state,"result":result}),
        );
        true
    }

    /// An accepted task's later outcome, separate from the coordinator's initial
    /// reply. This is non-user data: no state reopening, execution, or replacement
    /// of `result`. Duplicate observations do not produce duplicate voice events.
    pub fn outcome(&self, id: &str, text: &str, ok: bool) -> bool {
        self.record_outcome(id, ResultField { text: plain_text(text, MAX_TEXT), ok, ..Default::default() })
    }

    pub fn task_outcome(&self, id: &str, task_id: &str, revision: u64, text: &str) -> bool {
        if validate_id(task_id).is_err() || revision == 0 { return false; }
        self.record_outcome(id, ResultField { text: plain_text(text, MAX_TEXT), ok: true,
            task_id: Some(task_id.into()), revision: Some(revision),
            delivery_id: Some(format!("task-result:{task_id}:{revision}")) })
    }

    fn record_outcome(&self, id: &str, outcome: ResultField) -> bool {
        let mut digest = Sha256::new();
        digest.update([u8::from(outcome.ok)]);
        digest.update(outcome.text.as_bytes());
        if let Some(delivery) = &outcome.delivery_id { digest.update(delivery.as_bytes()); }
        let key: [u8; 32] = digest.finalize().into();
        let mut data = self.inner.shared.data.lock();
        let Some(request) = data.requests.get_mut(id) else {
            return false;
        };
        if request.outcome_keys.contains(&key) || request.outcome_keys.len() >= MAX_OUTCOMES {
            return false;
        }
        request.outcome_keys.insert(key);
        request.outcome = Some(outcome.clone());
        let state = request.state;
        data.event(
            "request.outcome",
            Some(id),
            json!({"state":state,"outcome":outcome}),
        );
        true
    }

    pub fn cancelled(&self, id: &str) -> bool {
        self.state(id, RequestState::Cancelled)
    }

    /// Reject a submit, or reject an unsafe cancellation without changing the
    /// underlying queued/dispatched work into an unrelated cancellation.
    pub fn reject(&self, id: &str, message: &str) -> bool {
        let mut data = self.inner.shared.data.lock();
        let Some(request) = data.requests.get_mut(id) else {
            return false;
        };
        if request.state.terminal() {
            return false;
        }
        let error = plain_text(message, MAX_PROGRESS);
        request.error = Some(error.clone());
        if request.cancel_pending {
            request.cancel_pending = false;
            data.event("request.cancel-rejected", Some(id), json!({"error":error}));
        } else {
            request.state = RequestState::Failed;
            data.event(
                "request.state",
                Some(id),
                json!({"state":"failed","error":error}),
            );
        }
        true
    }

    fn state(&self, id: &str, next: RequestState) -> bool {
        self.inner.shared.transition(id, next)
    }

    /// A valid host snapshot has been published and the listener is running.
    pub fn ready(&self) -> bool {
        self.inner.shared.running.load(Ordering::Acquire) && self.inner.shared.data.lock().published
    }

    /// Recent authenticated control traffic, not an audio/WebSocket connection.
    pub fn connected(&self) -> bool {
        self.inner.shared.running.load(Ordering::Acquire)
            && self
                .inner
                .shared
                .last_seen
                .lock()
                .is_some_and(|seen| seen.elapsed() < Duration::from_secs(30))
    }

    /// Metadata only; no provider connection or microphone probe.
    pub fn runtime_status(&self) -> Option<RuntimeStatus> {
        self.inner.shared.lifecycle.lock().status()
    }

    pub fn intentionally_stopped(&self) -> bool {
        self.inner.shared.lifecycle.lock().stopped()
    }

    pub fn previously_owned_runtime(&self) -> bool {
        self.inner.shared.lifecycle.lock().previously_owned()
    }

    pub fn launch_pending(&self) -> bool { self.inner.shared.lifecycle.lock().launch_pending() }

    /// Persist an explicit Quit even when the runtime cannot acknowledge it.
    pub fn suppress_runtime(&self) -> Result<(), String> { self.inner.shared.lifecycle.lock().mark_intentional_stop() }

    /// Call off-thread: persists user-authorized resume/quit state.
    pub fn resume_runtime(&self) -> Result<(), String> {
        self.inner.shared.lifecycle.lock().resume()
    }

    pub fn expect_runtime(&self, pid: u32, build_id: String) {
        self.inner.shared.lifecycle.lock().expect(pid, build_id);
    }

    /// Bind the heartbeat to the same lifetime verified by managed discovery.
    pub fn expect_runtime_process(&self, process: &bro_core::orbit_voice::ManagedLaunch) {
        self.inner.shared.lifecycle.lock().expect_process(process);
    }

    /// Native user controls only. Not part of the realtime tool catalog.
    pub fn control(&self, command: VoiceControl) -> Result<String, String> {
        self.inner.shared.lifecycle.lock().queue(command, &self.inner.shared.instance_id)
    }

    pub fn descriptor_path(&self) -> &Path {
        &self.inner.shared.descriptor_path
    }

    pub fn instance_id(&self) -> &str {
        &self.inner.shared.instance_id
    }

    pub fn local_url(&self) -> &str {
        &self.inner.shared.url
    }

    /// Safe readiness information; deliberately excludes the credential.
    pub fn status(&self) -> Value {
        json!({
            "running":self.inner.shared.running.load(Ordering::Acquire),
            "ready":self.ready(), "connected":self.connected(),
            "instanceId":self.instance_id(), "url":self.local_url(),
            "error":self.inner.shared.error.lock().clone()
        })
    }

    pub fn shutdown(&self) {
        self.inner.stop();
    }
}

impl HandleInner {
    fn stop(&self) {
        {
            let _data = self.shared.data.lock();
            self.shared.running.store(false, Ordering::Release);
            stage::replace(&self.shared.stage, None);
        }
        self.shared.shutdown.send_replace(true);
        let Some(thread) = self.thread.lock().take() else {
            return;
        };
        if std::thread::current()
            .name()
            .is_some_and(|name| name.starts_with(THREAD_NAME))
        {
            return;
        }
        let deadline = Instant::now() + Duration::from_secs(6);
        while !thread.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        if thread.is_finished() {
            let _ = thread.join();
        }
    }
}

impl Drop for HandleInner {
    fn drop(&mut self) {
        self.stop();
    }
}

impl Shared {
    fn transition(&self, id: &str, next: RequestState) -> bool {
        let mut data = self.data.lock();
        let Some(request) = data.requests.get_mut(id) else {
            return false;
        };
        if request.state == next {
            return true;
        }
        let allowed = match next {
            RequestState::Queued => request.state == RequestState::Received,
            RequestState::Dispatched => request.state == RequestState::Queued,
            RequestState::Cancelled | RequestState::Uncertain => !request.state.terminal(),
            _ => false,
        };
        if !allowed {
            return false;
        }
        request.state = next;
        if next == RequestState::Cancelled {
            request.cancel_pending = false;
        }
        data.event("request.state", Some(id), json!({"state":next}));
        true
    }

    fn dispatch(&self, command: Command) -> bool {
        // A partially executed callback must never cause a retry to execute again.
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (self.callback)(command))).is_ok()
    }
}

impl Data {
    fn new(instance_id: &str) -> Self {
        Self {
            host: json!({"protocol":PROTOCOL,"instanceId":instance_id,"name":"bro",
                "capabilities":CAPABILITIES,"mode":"desktop","window":null,
                "focusedSessionId":null,"sessions":[]}),
            published: false,
            utterances: HashMap::new(),
            read_scopes: HashMap::new(),
            read_scope_order: VecDeque::new(),
            requests: HashMap::new(),
            events: VecDeque::new(),
            sequence: 0,
            rate_start: Instant::now(),
            rate_count: 0,
        }
    }

    fn event(&mut self, kind: &str, id: Option<&str>, mut fields: Value) {
        // Reaching u64::MAX would require centuries at the transport rate limit.
        self.sequence = self.sequence.saturating_add(1);
        fields["sequence"] = json!(self.sequence);
        fields["type"] = json!(kind);
        fields["requestId"] = json!(id);
        self.events.push_back(fields);
        if self.events.len() > MAX_EVENTS {
            self.events.pop_front();
        }
    }

    fn target(&self) -> Option<Target> {
        let id = self.host["focusedSessionId"].as_str()?;
        let session = self.host["sessions"]
            .as_array()?
            .iter()
            .find(|session| session["id"].as_str() == Some(id))?;
        Some(Target {
            session_id: id.into(),
            generation: session["generation"].as_str()?.into(),
            scope: session["scope"].as_str().map(str::to_owned),
        })
    }

    fn has_target(&self, target: &Target) -> bool {
        self.host["sessions"].as_array().is_some_and(|sessions| {
            sessions.iter().any(|session| {
                session["id"].as_str() == Some(target.session_id.as_str())
                    && session["generation"].as_str() == Some(target.generation.as_str())
                    && target.scope.as_deref().is_none_or(|scope| session["scope"].as_str() == Some(scope))
            })
        })
    }

    fn batch(&self, after: u64) -> Value {
        let gap = after > self.sequence
            || self.events.front().is_some_and(|event| {
                after.saturating_add(1) < event["sequence"].as_u64().unwrap_or(0)
            });
        let effective_after = if after > self.sequence { 0 } else { after };
        let events: Vec<_> = self
            .events
            .iter()
            .filter(|event| event["sequence"].as_u64().unwrap_or(0) > effective_after)
            .take(EVENT_BATCH)
            .cloned()
            .collect();
        let cursor = events
            .last()
            .and_then(|event| event["sequence"].as_u64())
            .unwrap_or(self.sequence);
        json!({"events":events,"cursor":cursor,"gap":gap})
    }
}

type ApiError = (StatusCode, Json<Value>);
fn error(status: StatusCode, code: &str, message: &str) -> ApiError {
    (
        status,
        Json(json!({"error":{"code":code,"message":message}})),
    )
}

fn router(shared: Arc<Shared>) -> Router {
    Router::new()
        .route("/orbit/v1/host", get(host))
        .route("/orbit/v1/stage", get(stage::get))
        .route("/orbit/v1/runtime/heartbeat", post(lifecycle::heartbeat))
        .route("/orbit/v1/tools", get(tools::catalog))
        .route("/orbit/v1/tools/read", post(tools::read))
        .route("/orbit/v1/utterances", post(utterance))
        .route("/orbit/v1/requests", post(submit))
        .route("/orbit/v1/requests/{id}", get(request_status))
        .route("/orbit/v1/requests/{id}/cancel", post(cancel))
        .route("/orbit/v1/focus", post(focus))
        .route("/orbit/v1/events", get(events))
        .fallback(|| async {
            error(
                StatusCode::NOT_FOUND,
                "not_found",
                "Unknown Orbit endpoint.",
            )
        })
        .layer(DefaultBodyLimit::max(MAX_BODY))
        .layer(middleware::from_fn_with_state(shared.clone(), guard))
        .with_state(shared)
}

fn validate_peer(
    shared: &Shared,
    peer: Option<SocketAddr>,
    headers: &HeaderMap,
) -> Result<(), ApiError> {
    if headers.contains_key(header::ORIGIN) {
        return Err(error(
            StatusCode::FORBIDDEN,
            "origin_forbidden",
            "Browser origins are not supported.",
        ));
    }
    if headers.get_all(header::HOST).iter().count() != 1
        || headers
            .get(header::HOST)
            .and_then(|host| host.to_str().ok())
            != Some(&shared.authority)
    {
        return Err(error(
            StatusCode::FORBIDDEN,
            "invalid_host",
            "Incorrect loopback host.",
        ));
    }
    let authorized = headers.get_all(header::AUTHORIZATION).iter().count() == 1
        && headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .is_some_and(|value| constant_time_eq(value.as_bytes(), shared.token.as_bytes()));
    if !authorized {
        return Err(error(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "A bearer credential is required.",
        ));
    }
    // No absent-peer or loopback exemption. Real requests always have ConnectInfo.
    if !peer.is_some_and(|peer| match peer.ip() {
        IpAddr::V4(ip) => ip.is_loopback(),
        IpAddr::V6(ip) => {
            ip.is_loopback() || ip.to_ipv4_mapped().is_some_and(|ip| ip.is_loopback())
        }
    }) {
        return Err(error(
            StatusCode::FORBIDDEN,
            "invalid_peer",
            "A loopback peer is required.",
        ));
    }
    Ok(())
}

async fn guard(
    State(shared): State<Arc<Shared>>,
    request: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let response = async {
        let peer = request
            .extensions()
            .get::<ConnectInfo<SocketAddr>>()
            .map(|info| info.0);
        if let Err(error) = validate_peer(&shared, peer, request.headers()) {
            return error.into_response();
        }
        if request.uri().to_string().len() > 512
            || request
                .headers()
                .iter()
                .map(|(key, value)| key.as_str().len() + value.len())
                .sum::<usize>()
                > 8192
            || request
                .uri()
                .authority()
                .is_some_and(|authority| authority.as_str() != shared.authority)
            || request
                .uri()
                .scheme_str()
                .is_some_and(|scheme| scheme != "http")
        {
            return error(
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "Request headers or URL are invalid.",
            )
            .into_response();
        }
        if !shared.running.load(Ordering::Acquire) {
            return error(
                StatusCode::SERVICE_UNAVAILABLE,
                "stopped",
                "The Orbit host is stopping.",
            )
            .into_response();
        }
        let is_stage = request.uri().path() == "/orbit/v1/stage";
        let rate_allowed = if is_stage {
            shared.stage_rate.lock().admit()
        } else {
            let mut data = shared.data.lock();
            if data.rate_start.elapsed() >= Duration::from_secs(1) {
                data.rate_start = Instant::now();
                data.rate_count = 0;
            }
            if data.rate_count >= 256 {
                false
            } else {
                data.rate_count += 1;
                true
            }
        };
        if !rate_allowed {
            return error(
                StatusCode::TOO_MANY_REQUESTS,
                "rate_limited",
                "Too many requests in this transport lane.",
            )
            .into_response();
        }
        let permits = if is_stage {
            &shared.stage_in_flight
        } else {
            &shared.in_flight
        };
        let Ok(_permit) = permits.clone().try_acquire_owned() else {
            return error(
                StatusCode::TOO_MANY_REQUESTS,
                "busy",
                "Too many concurrent requests in this transport lane.",
            )
            .into_response();
        };
        *shared.last_seen.lock() = Some(Instant::now());
        let work = async move {
            // Bound every method, including ignored GET/cancel bodies, not only
            // Json extractors. Buffering happens only after authentication.
            let (parts, body) = request.into_parts();
            let body = match axum::body::to_bytes(body, MAX_BODY).await {
                Ok(body) => body,
                Err(_) => {
                    return error(
                        StatusCode::PAYLOAD_TOO_LARGE,
                        "body_limit",
                        "The control body exceeds its limit or could not be read.",
                    )
                    .into_response();
                }
            };
            next.run(Request::from_parts(parts, axum::body::Body::from(body)))
                .await
        };
        let timeout = if is_stage {
            Duration::from_secs(1)
        } else {
            Duration::from_secs(5)
        };
        let mut shutdown = shared.shutdown.subscribe();
        tokio::select! {
            result = tokio::time::timeout(timeout, work) => match result {
                Ok(response) => response,
                Err(_) => error(
                    StatusCode::REQUEST_TIMEOUT,
                    "timeout",
                    "The request timed out; inspect its status before retrying.",
                ).into_response(),
            },
            _ = shutdown.wait_for(|stopped| *stopped), if is_stage => stage::stopped().into_response(),
        }
    }
    .await;
    let mut response = response;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    response
        .headers_mut()
        .insert(header::X_CONTENT_TYPE_OPTIONS, "nosniff".parse().unwrap());
    response
}

async fn host(State(shared): State<Arc<Shared>>) -> Json<Value> {
    let data = shared.data.lock();
    let mut host = data.host.clone();
    // Preserve reviewed additive capabilities from the canonical publication.
    // Replacing this with the legacy constant silently hides lifecycle/tool support.
    host["stage"] = shared.stage.borrow().metadata();
    Json(host)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UtteranceInput {
    id: String,
}

async fn utterance(
    State(shared): State<Arc<Shared>>,
    Json(input): Json<UtteranceInput>,
) -> Result<Json<Value>, ApiError> {
    validate_id(&input.id)?;
    let mut data = shared.data.lock();
    if let Some(target) = data.utterances.get(&input.id) {
        return Ok(Json(
            json!({"id":input.id,"target":target,"instanceId":shared.instance_id}),
        ));
    }
    if !data.published || !shared.work_enabled.load(Ordering::Acquire) {
        return Err(not_ready());
    }
    if data.utterances.len() >= MAX_UTTERANCES {
        return Err(capacity());
    }
    let target = data.target();
    let frozen = target.as_ref().and_then(|target| target.scope.as_ref()).map(|scope| {
        data.host["sessions"].as_array().into_iter().flatten()
            .filter(|session| session["scope"].as_str() == Some(scope.as_str())).take(MAX_SESSIONS)
            .filter_map(|session| Some(Target { session_id: session["id"].as_str()?.into(), generation: session["generation"].as_str()?.into(), scope: Some(scope.clone()) })).collect::<Vec<_>>()
    }).unwrap_or_default();
    data.read_scopes.insert(input.id.clone(), frozen);
    data.read_scope_order.push_back(input.id.clone());
    if data.read_scope_order.len() > 128 && let Some(oldest) = data.read_scope_order.pop_front() { data.read_scopes.remove(&oldest); }
    data.utterances.insert(input.id.clone(), target.clone());
    Ok(Json(
        json!({"id":input.id,"target":target,"instanceId":shared.instance_id}),
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SubmitInput {
    id: String,
    utterance_id: String,
    text: String,
}

async fn submit(
    State(shared): State<Arc<Shared>>,
    Json(input): Json<SubmitInput>,
) -> Result<Json<Value>, ApiError> {
    validate_id(&input.id)?;
    validate_id(&input.utterance_id)?;
    if input.text.trim().is_empty() || input.text.len() > MAX_TEXT || input.text.contains('\0') {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "invalid_text",
            "Work text must be nonempty and at most 16384 bytes.",
        ));
    }
    let command = {
        let mut data = shared.data.lock();
        if let Some(existing) = data.requests.get(&input.id) {
            if existing.utterance_id != input.utterance_id || existing.text != input.text {
                return Err(error(
                    StatusCode::CONFLICT,
                    "id_conflict",
                    "The request ID already has different content.",
                ));
            }
            return Ok(Json(request_json(existing, &shared.instance_id)));
        }
        if !data.published || !shared.work_enabled.load(Ordering::Acquire) {
            return Err(not_ready());
        }
        let Some(target) = data.utterances.get(&input.utterance_id).cloned() else {
            return Err(error(
                StatusCode::NOT_FOUND,
                "unknown_utterance",
                "Latch an utterance before submitting work.",
            ));
        };
        // Selection may move while the user speaks. Only the original
        // session's lifetime matters; a null original target stays null.
        if target
            .as_ref()
            .is_some_and(|target| !data.has_target(target))
        {
            return Err(error(
                StatusCode::CONFLICT,
                "target_changed",
                "The latched session was removed or restarted; start a new utterance.",
            ));
        }
        if data.requests.len() >= MAX_REQUESTS {
            return Err(capacity());
        }
        let command = Command::Submit {
            id: input.id.clone(),
            utterance_id: input.utterance_id.clone(),
            text: input.text.clone(),
            focused: target.as_ref().map(|target| target.session_id.clone()),
            generation: target.as_ref().map(|target| target.generation.clone()),
        };
        data.requests.insert(
            input.id.clone(),
            RequestRecord {
                id: input.id.clone(),
                utterance_id: input.utterance_id,
                text: input.text,
                target,
                state: RequestState::Received,
                progress: None,
                result: None,
                outcome: None,
                error: None,
                cancel_pending: false,
                outcome_keys: std::collections::HashSet::new(),
            },
        );
        data.event(
            "request.state",
            Some(&input.id),
            json!({"state":"received"}),
        );
        command
    };
    if !shared.dispatch(command) {
        shared.transition(&input.id, RequestState::Uncertain);
    }
    let data = shared.data.lock();
    Ok(Json(request_json(
        &data.requests[&input.id],
        &shared.instance_id,
    )))
}

fn request_json(request: &RequestRecord, instance_id: &str) -> Value {
    let mut value = serde_json::to_value(request).expect("request fields are JSON serializable");
    value["instanceId"] = json!(instance_id);
    value
}

async fn request_status(
    State(shared): State<Arc<Shared>>,
    RoutePath(id): RoutePath<String>,
) -> Result<Json<Value>, ApiError> {
    validate_id(&id)?;
    let data = shared.data.lock();
    let request = data.requests.get(&id).ok_or_else(unknown_request)?;
    Ok(Json(request_json(request, &shared.instance_id)))
}

async fn cancel(
    State(shared): State<Arc<Shared>>,
    RoutePath(id): RoutePath<String>,
) -> Result<Json<Value>, ApiError> {
    validate_id(&id)?;
    {
        let mut data = shared.data.lock();
        let request = data.requests.get_mut(&id).ok_or_else(unknown_request)?;
        if request.state.terminal() || request.cancel_pending {
            return Ok(Json(request_json(request, &shared.instance_id)));
        }
        // Only the durable owner decides whether cancellation can target work.
        request.cancel_pending = true;
    }
    if !shared.dispatch(Command::Cancel { id: id.clone() }) {
        let mut data = shared.data.lock();
        if let Some(request) = data.requests.get_mut(&id) {
            request.cancel_pending = false;
        }
        return Err(error(
            StatusCode::SERVICE_UNAVAILABLE,
            "cancel_uncertain",
            "Cancellation could not be acknowledged; inspect the request status.",
        ));
    }
    let data = shared.data.lock();
    Ok(Json(request_json(&data.requests[&id], &shared.instance_id)))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FocusInput {
    session_id: String,
}

async fn focus(
    State(shared): State<Arc<Shared>>,
    Json(input): Json<FocusInput>,
) -> Result<Json<Value>, ApiError> {
    validate_id(&input.session_id)?;
    {
        let data = shared.data.lock();
        if !data.published || !shared.work_enabled.load(Ordering::Acquire) {
            return Err(not_ready());
        }
        if !data.host["sessions"].as_array().is_some_and(|sessions| {
            sessions
                .iter()
                .any(|session| session["id"].as_str() == Some(&input.session_id))
        }) {
            return Err(error(
                StatusCode::NOT_FOUND,
                "unknown_session",
                "The session is not currently advertised.",
            ));
        }
    }
    if !shared.dispatch(Command::Focus {
        session_id: input.session_id.clone(),
    }) {
        return Err(error(
            StatusCode::SERVICE_UNAVAILABLE,
            "focus_failed",
            "The host could not acknowledge focus.",
        ));
    }
    // The host must publish the actual focus; an accepted command is not focus.
    Ok(Json(json!({"sessionId":input.session_id,"accepted":true})))
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct EventsQuery {
    #[serde(default)]
    after: u64,
}

async fn events(
    State(shared): State<Arc<Shared>>,
    Query(query): Query<EventsQuery>,
) -> Json<Value> {
    Json(shared.data.lock().batch(query.after))
}

fn validate_id(id: &str) -> Result<(), ApiError> {
    if id.is_empty() || id.len() > MAX_ID || id.chars().any(char::is_control) || id.trim() != id {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "invalid_id",
            "IDs must contain 1 to 128 bytes without control characters or surrounding whitespace.",
        ));
    }
    Ok(())
}
fn unknown_request() -> ApiError {
    error(
        StatusCode::NOT_FOUND,
        "unknown_request",
        "The request is not known to this host instance.",
    )
}
fn not_ready() -> ApiError {
    error(
        StatusCode::SERVICE_UNAVAILABLE,
        "host_not_ready",
        "The host snapshot is not ready.",
    )
}
fn capacity() -> ApiError {
    error(
        StatusCode::SERVICE_UNAVAILABLE,
        "capacity",
        "This host instance has reached its retained ID limit; existing requests remain observable.",
    )
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter()
        .zip(b)
        .fold(0u8, |difference, (a, b)| difference | (a ^ b))
        == 0
}

fn plain_text(text: &str, limit: usize) -> String {
    let mut output = String::with_capacity(text.len().min(limit));
    for character in text
        .chars()
        .filter(|character| !character.is_control() || matches!(character, '\n' | '\t'))
    {
        if output.len() + character.len_utf8() > limit {
            break;
        }
        output.push(character);
    }
    output
}

fn normalize_host(input: Value, instance_id: &str) -> Option<Value> {
    let object = input.as_object()?;
    let sessions = object.get("sessions")?.as_array()?;
    if sessions.len() > MAX_SESSIONS {
        return None;
    }
    let mode = object
        .get("mode")
        .and_then(Value::as_str)
        .unwrap_or("desktop");
    if !matches!(mode, "desktop" | "attached") {
        return None;
    }
    let mut normalized = Vec::with_capacity(sessions.len());
    let mut ids = std::collections::HashSet::new();
    for session in sessions {
        let id = session.get("id")?.as_str()?;
        let generation = session.get("generation")?.as_str()?;
        if validate_id(id).is_err() || validate_id(generation).is_err() || !ids.insert(id) {
            return None;
        }
        let field = |key, limit| {
            plain_text(
                session.get(key).and_then(Value::as_str).unwrap_or(""),
                limit,
            )
        };
        let mut record = json!({"id":id,"generation":generation,"title":field("title",256),
            "agent":field("agent",64),"activity":field("activity",64),
            "context":if object.contains_key("avatarPlacement") && object.get("focusedSessionId").and_then(Value::as_str) != Some(id) { String::new() } else { field("context",4096) }});
        if let Some(scope) = session.get("scope").and_then(Value::as_str)
            .filter(|scope| scope.len() == 64 && scope.bytes().all(|c| c.is_ascii_hexdigit())) { record["scope"] = json!(scope); }
        normalized.push(record);
    }
    let focused = match object.get("focusedSessionId") {
        None | Some(Value::Null) => None,
        Some(Value::String(id)) if ids.contains(id.as_str()) => Some(id.as_str()),
        _ => return None,
    };
    let window = match object.get("window") {
        None | Some(Value::Null) => Value::Null,
        Some(window) => {
            let handle = window.get("handle")?.as_str()?;
            let pid = window.get("pid")?.as_u64()?;
            if handle.is_empty()
                || handle.len() > 32
                || !handle.bytes().all(|byte| byte.is_ascii_digit())
                || handle.parse::<u64>().ok()? == 0
                || pid == 0
                || pid > u32::MAX as u64
            {
                return None;
            }
            let mut window = json!({"handle":handle,"pid":pid});
            if let Some(epoch) = object.get("window").and_then(|window| window.get("epoch")) {
                let epoch = epoch.as_str()?;
                if !stage::safe_id(epoch) {
                    return None;
                }
                window["epoch"] = json!(epoch);
            }
            window
        }
    };
    let mut result = json!({"protocol":PROTOCOL,"instanceId":instance_id,"name":"bro",
        "capabilities":stage::capabilities(),"mode":mode,"window":window,
        "stage":{"schema":1,"revision":0,"available":false},
        "focusedSessionId":focused,"sessions":normalized});
    // Optional explicit containment permission. Missing/malformed metadata cannot
    // grant escape; legacy peers retain their reviewed fixed/roaming behavior.
    let placement = object.get("avatarPlacement").and_then(Value::as_object);
    let managed = object.contains_key("avatarPlacement");
    let allow_escape = placement.is_some_and(|p| p.len() == 2
        && p.get("schema").and_then(Value::as_u64) == Some(1)
        && p.get("allowEscape").and_then(Value::as_bool) == Some(true));
    if managed {
        result["mode"] = json!("attached");
        result["avatarPlacement"] = json!({"schema":1,"allowEscape":allow_escape});
        result["capabilities"].as_array_mut()?.push(json!("avatar.containment.v1"));
    }
    // Optional placement policy carries no execution authority. Ignore unknown or
    // malformed policy data instead of discarding otherwise valid session context.
    if let Some(roam) = object.get("avatarRoam").and_then(Value::as_object)
        && roam.len() == 2
        && roam.get("schema").and_then(Value::as_u64) == Some(1)
        && roam.get("policy").and_then(Value::as_str) == Some("auto")
        && (!managed || allow_escape)
        && (!result["window"].is_null() || managed && allow_escape)
    {
        result["avatarRoam"] = json!({"schema":1,"policy":"auto"});
    }
    if serde_json::to_vec(&result).ok()?.len() > MAX_HOST_BYTES {
        return None;
    }
    Some(result)
}

// A private parent protects every temporary/lock file, not just the final name.
fn private_directory(directory: &Path) -> io::Result<()> {
    if let Some(parent) = directory.parent() {
        std::fs::create_dir_all(parent)?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
        unsafe extern "C" {
            fn geteuid() -> u32;
        }
        let mut builder = std::fs::DirBuilder::new();
        builder.mode(0o700);
        match builder.create(directory) {
            Ok(()) => (),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => (),
            Err(error) => return Err(error),
        }
        let metadata = std::fs::symlink_metadata(directory)?;
        if !metadata.is_dir()
            || metadata.file_type().is_symlink()
            || metadata.uid() != unsafe { geteuid() }
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Orbit directory must be owned by the current user and not a symlink",
            ));
        }
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(windows)]
    windows_private::directory(directory)?;
    #[cfg(not(any(unix, windows)))]
    return Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "private Orbit discovery is not supported on this platform",
    ));
    Ok(())
}

fn private_open(path: &Path, create_new: bool) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    if create_new {
        options.create_new(true);
    } else {
        options.create(true).truncate(false);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    if let Ok(metadata) = std::fs::symlink_metadata(path) {
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Orbit private file must not be a symlink",
            ));
        }
    }
    let file = options.open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(file)
}

fn descriptor_lock(shared: &Shared) -> io::Result<File> {
    let lock = private_open(
        &shared.descriptor_path.with_file_name(".connection.lock"),
        false,
    )?;
    lock.lock()?;
    Ok(lock)
}

fn write_descriptor(shared: &Shared) -> io::Result<()> {
    let _lock = descriptor_lock(shared)?;
    let managed = shared.lifecycle.lock().enabled();
    let descriptor = Descriptor {
        lineage: managed.then(|| shared.lineage.clone()),
        owner_epoch: managed.then(|| shared.instance_id.clone()),
        protocol: PROTOCOL,
        instance_id: shared.instance_id.clone(),
        url: shared.url.clone(),
        token: shared.token.clone(),
    };
    let bytes = serde_json::to_vec(&descriptor).map_err(io::Error::other)?;
    let temporary = shared
        .descriptor_path
        .with_file_name(format!(".connection.{}.tmp", shared.instance_id));
    let mut owns_temporary = false;
    let result = (|| {
        let mut file = private_open(&temporary, true)?;
        owns_temporary = true;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        // On Windows std::fs::rename uses replacement, preserving the private ACL.
        std::fs::rename(&temporary, &shared.descriptor_path)?;
        #[cfg(unix)]
        File::open(shared.descriptor_path.parent().unwrap())?.sync_all()?;
        Ok(())
    })();
    if result.is_err() && owns_temporary {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

fn remove_owned_descriptor(shared: &Shared) {
    let Ok(_lock) = descriptor_lock(shared) else {
        return;
    };
    let Ok(metadata) = std::fs::symlink_metadata(&shared.descriptor_path) else {
        return;
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return;
    }
    let descriptor = File::open(&shared.descriptor_path).ok().and_then(|file| {
        let mut bytes = Vec::new();
        file.take(4097).read_to_end(&mut bytes).ok()?;
        serde_json::from_slice::<Descriptor>(&bytes).ok()
    });
    if descriptor.is_some_and(|descriptor| {
        descriptor.instance_id == shared.instance_id
            && constant_time_eq(descriptor.token.as_bytes(), shared.token.as_bytes())
    }) {
        let _ = std::fs::remove_file(&shared.descriptor_path);
    }
}

#[cfg(windows)]
mod windows_private {
    // No shell commands, usernames, inherited broad ACLs or new dependency. The
    // SID comes from the process token and the protected DACL grants only it.
    use super::*;
    use std::ffi::c_void;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::ptr::null_mut;

    type WinHandle = *mut c_void;
    #[repr(C)]
    struct SecurityAttributes {
        length: u32,
        descriptor: *mut c_void,
        inherit: i32,
    }
    #[repr(C)]
    struct SidAndAttributes {
        sid: *mut c_void,
        attributes: u32,
    }
    #[link(name = "advapi32")]
    unsafe extern "system" {
        fn OpenProcessToken(process: WinHandle, access: u32, token: *mut WinHandle) -> i32;
        fn GetTokenInformation(
            token: WinHandle,
            class: u32,
            information: *mut c_void,
            length: u32,
            needed: *mut u32,
        ) -> i32;
        fn ConvertSidToStringSidW(sid: *mut c_void, text: *mut *mut u16) -> i32;
        fn ConvertStringSecurityDescriptorToSecurityDescriptorW(
            text: *const u16,
            revision: u32,
            descriptor: *mut *mut c_void,
            size: *mut u32,
        ) -> i32;
        fn SetFileSecurityW(path: *const u16, information: u32, descriptor: *mut c_void) -> i32;
        fn GetNamedSecurityInfoW(
            path: *const u16,
            object_type: u32,
            information: u32,
            owner: *mut *mut c_void,
            group: *mut *mut c_void,
            dacl: *mut *mut c_void,
            sacl: *mut *mut c_void,
            descriptor: *mut *mut c_void,
        ) -> u32;
        fn EqualSid(first: *mut c_void, second: *mut c_void) -> i32;
        #[cfg(test)]
        fn ConvertSecurityDescriptorToStringSecurityDescriptorW(
            descriptor: *mut c_void,
            revision: u32,
            information: u32,
            text: *mut *mut u16,
            length: *mut u32,
        ) -> i32;
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> WinHandle;
        fn CloseHandle(handle: WinHandle) -> i32;
        fn LocalFree(memory: *mut c_void) -> *mut c_void;
        fn CreateDirectoryW(path: *const u16, attributes: *const SecurityAttributes) -> i32;
    }
    struct LocalMemory(*mut c_void);
    impl Drop for LocalMemory {
        fn drop(&mut self) {
            unsafe {
                LocalFree(self.0);
            }
        }
    }
    struct Token(WinHandle);
    impl Drop for Token {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }

    #[cfg(test)]
    pub(super) fn security_text(path: &Path) -> io::Result<String> {
        let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut descriptor = null_mut();
        let code = unsafe {
            GetNamedSecurityInfoW(
                path.as_ptr(),
                1,
                5,
                null_mut(),
                null_mut(),
                null_mut(),
                null_mut(),
                &mut descriptor,
            )
        };
        if code != 0 {
            return Err(io::Error::from_raw_os_error(code as i32));
        }
        let descriptor = LocalMemory(descriptor);
        let mut text = null_mut();
        if unsafe {
            ConvertSecurityDescriptorToStringSecurityDescriptorW(
                descriptor.0,
                1,
                5,
                &mut text,
                null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        let _text = LocalMemory(text.cast());
        let mut length = 0;
        while unsafe { *text.add(length) } != 0 {
            length += 1;
        }
        String::from_utf16(unsafe { std::slice::from_raw_parts(text, length) })
            .map_err(io::Error::other)
    }

    pub(super) fn directory(path: &Path) -> io::Result<()> {
        use std::os::windows::fs::MetadataExt;
        let mut token = null_mut();
        if unsafe { OpenProcessToken(GetCurrentProcess(), 0x0008, &mut token) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let token = Token(token);
        let mut needed = 0;
        unsafe {
            GetTokenInformation(token.0, 1, null_mut(), 0, &mut needed);
        }
        if needed == 0 {
            return Err(io::Error::last_os_error());
        }
        // Pointer alignment is required by TOKEN_USER, not just a byte buffer.
        let mut storage = vec![0usize; (needed as usize).div_ceil(std::mem::size_of::<usize>())];
        if unsafe {
            GetTokenInformation(token.0, 1, storage.as_mut_ptr().cast(), needed, &mut needed)
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        let sid = unsafe { (*(storage.as_ptr().cast::<SidAndAttributes>())).sid };
        let mut sid_text = null_mut();
        if unsafe { ConvertSidToStringSidW(sid, &mut sid_text) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let sid_memory = LocalMemory(sid_text.cast());
        let mut length = 0;
        while unsafe { *sid_text.add(length) } != 0 {
            length += 1;
        }
        let sid_string =
            String::from_utf16(unsafe { std::slice::from_raw_parts(sid_text, length) })
                .map_err(io::Error::other)?;
        drop(sid_memory);
        let sddl: Vec<u16> = format!("O:{sid_string}D:P(A;OICI;FA;;;{sid_string})")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let mut descriptor = null_mut();
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                1,
                &mut descriptor,
                null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        let descriptor = LocalMemory(descriptor);
        let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let attributes = SecurityAttributes {
            length: std::mem::size_of::<SecurityAttributes>() as u32,
            descriptor: descriptor.0,
            inherit: 0,
        };
        if unsafe { CreateDirectoryW(path.as_ptr(), &attributes) } == 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(183) {
                return Err(error);
            }
        }
        let native_path = std::ffi::OsString::from_wide(&path[..path.len() - 1]);
        let metadata = std::fs::symlink_metadata(Path::new(&native_path))?;
        if !metadata.is_dir() || metadata.file_attributes() & 0x400 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Orbit directory must not be a reparse point",
            ));
        }
        let mut owner = null_mut();
        let mut existing_descriptor = null_mut();
        let code = unsafe {
            GetNamedSecurityInfoW(
                path.as_ptr(),
                1,
                1,
                &mut owner,
                null_mut(),
                null_mut(),
                null_mut(),
                &mut existing_descriptor,
            )
        };
        if code != 0 {
            return Err(io::Error::from_raw_os_error(code as i32));
        }
        let _existing_descriptor = LocalMemory(existing_descriptor);
        if owner.is_null() || unsafe { EqualSid(owner, sid) } == 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Orbit directory must be owned by the current user",
            ));
        }
        if unsafe { SetFileSecurityW(path.as_ptr(), 0x8000_0004, descriptor.0) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod stage_tests;
    mod managed_runtime;

    struct Fixture {
        _root: tempfile::TempDir,
        handle: Handle,
        commands: Arc<Mutex<Vec<Command>>>,
    }

    fn test_handle(root: &Path) -> (Handle, Arc<Mutex<Vec<Command>>>) {
        let commands = Arc::new(Mutex::new(Vec::new()));
        let sink = commands.clone();
        let instance_id = uuid::Uuid::new_v4().to_string();
        let shared = Arc::new(Shared {
            instance_id: instance_id.clone(),
            token: uuid::Uuid::new_v4().to_string(),
            authority: "127.0.0.1:12345".into(),
            url: "http://127.0.0.1:12345".into(),
            descriptor_path: root.join("orbit/connection.json"),
            lineage: instance_id.clone(),
            lifecycle: Mutex::new(lifecycle::Lifecycle::load(root, false).unwrap()),
            workspace: Mutex::new(None),
            callback: Arc::new(move |command| sink.lock().push(command)),
            data: Mutex::new(Data::new(&instance_id)),
            running: AtomicBool::new(true),
            work_enabled: AtomicBool::new(true),
            shutdown: watch::channel(false).0,
            in_flight: Arc::new(Semaphore::new(32)),
            stage: watch::channel(stage::Snapshot::default()).0,
            stage_in_flight: Arc::new(Semaphore::new(stage::MAX_WATCHERS)),
            stage_rate: Mutex::new(stage::Rate::new()),
            last_seen: Mutex::new(None),
            error: Mutex::new(None),
        });
        (
            Handle {
                inner: Arc::new(HandleInner {
                    shared,
                    thread: Mutex::new(None),
                }),
            },
            commands,
        )
    }

    impl Fixture {
        fn new() -> Self {
            // Explicit scratch paths only; no home/config/environment discovery.
            let root = tempfile::tempdir().unwrap();
            let (handle, commands) = test_handle(root.path());
            Self {
                _root: root,
                handle,
                commands,
            }
        }
        fn shared(&self) -> Arc<Shared> {
            self.handle.inner.shared.clone()
        }
        fn publish(&self, selected: Option<&str>, generation: &str) {
            assert!(self.handle.publish_host(snapshot(selected, generation)));
        }
    }

    fn snapshot(selected: Option<&str>, generation: &str) -> Value {
        json!({"mode":"desktop","window":null,"focusedSessionId":selected,"sessions":[
            {"id":"a","generation":generation,"title":"A","agent":"claude","context":""},
            {"id":"b","generation":"b-generation","title":"B","agent":"codex"}
        ]})
    }

    async fn latch(fixture: &Fixture, id: &str) -> Value {
        utterance(
            State(fixture.shared()),
            Json(UtteranceInput { id: id.into() }),
        )
        .await
        .unwrap()
        .0
    }

    async fn send(
        fixture: &Fixture,
        id: &str,
        utterance_id: &str,
        text: &str,
    ) -> Result<Value, ApiError> {
        submit(
            State(fixture.shared()),
            Json(SubmitInput {
                id: id.into(),
                utterance_id: utterance_id.into(),
                text: text.into(),
            }),
        )
        .await
        .map(|result| result.0)
    }

    fn headers(shared: &Shared) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, shared.authority.parse().unwrap());
        headers.insert(
            header::AUTHORIZATION,
            format!("Bearer {}", shared.token).parse().unwrap(),
        );
        headers
    }

    #[test]
    fn bearer_is_mandatory_without_loopback_or_missing_peer_bypasses() {
        let fixture = Fixture::new();
        let shared = fixture.shared();
        let loopback = Some("127.0.0.1:3000".parse().unwrap());
        let mut headers = headers(&shared);
        assert!(validate_peer(&shared, loopback, &headers).is_ok());
        headers.remove(header::AUTHORIZATION);
        for peer in [loopback, None, Some("[::1]:3000".parse().unwrap())] {
            assert_eq!(
                validate_peer(&shared, peer, &headers).unwrap_err().0,
                StatusCode::UNAUTHORIZED
            );
        }
        headers.insert("x-terminal-web-token", shared.token.parse().unwrap());
        assert!(validate_peer(&shared, loopback, &headers).is_err());
        headers.insert(
            header::AUTHORIZATION,
            format!("Bearer {}", shared.token).parse().unwrap(),
        );
        assert_eq!(
            validate_peer(&shared, None, &headers).unwrap_err().0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            validate_peer(&shared, Some("10.1.2.3:3000".parse().unwrap()), &headers)
                .unwrap_err()
                .0,
            StatusCode::FORBIDDEN
        );
        for origin in ["null", "http://localhost", "https://example.com"] {
            headers.insert(header::ORIGIN, origin.parse().unwrap());
            assert_eq!(
                validate_peer(&shared, loopback, &headers).unwrap_err().0,
                StatusCode::FORBIDDEN
            );
        }
        headers.remove(header::ORIGIN);
        headers.insert(header::HOST, "localhost:12345".parse().unwrap());
        assert!(validate_peer(&shared, loopback, &headers).is_err());
        headers.insert(header::HOST, shared.authority.parse().unwrap());
        headers.append(
            header::AUTHORIZATION,
            format!("Bearer {}", shared.token).parse().unwrap(),
        );
        assert!(validate_peer(&shared, loopback, &headers).is_err());
        assert!(!constant_time_eq(b"abc", b"ab"));
        assert!(!constant_time_eq(b"abc", b"abd"));
    }

    #[tokio::test]
    async fn receipt_is_not_durable_and_retries_never_execute_twice() {
        let fixture = Fixture::new();
        fixture.publish(Some("a"), "generation-1");
        latch(&fixture, "utterance").await;
        let result = send(&fixture, "work", "utterance", "Review the code")
            .await
            .unwrap();
        assert_eq!(result["state"], "received");
        assert_eq!(fixture.commands.lock().len(), 1);
        assert!(!fixture.handle.dispatched("work"));
        assert!(fixture.handle.queued("work"));
        assert!(fixture.handle.dispatched("work"));
        fixture.publish(Some("b"), "generation-1");
        assert_eq!(
            send(&fixture, "work", "utterance", "Review the code")
                .await
                .unwrap()["state"],
            "dispatched"
        );
        assert_eq!(fixture.commands.lock().len(), 1);
        assert_eq!(
            send(&fixture, "work", "utterance", "Different work")
                .await
                .unwrap_err()
                .0,
            StatusCode::CONFLICT
        );
        assert_eq!(
            send(&fixture, "work", "different-utterance", "Review the code")
                .await
                .unwrap_err()
                .0,
            StatusCode::CONFLICT
        );
        assert_eq!(
            fixture.commands.lock()[0],
            Command::Submit {
                id: "work".into(),
                utterance_id: "utterance".into(),
                text: "Review the code".into(),
                focused: Some("a".into()),
                generation: Some("generation-1".into()),
            }
        );
    }

    #[tokio::test]
    async fn utterance_stays_pinned_when_selection_changes_or_clears() {
        let fixture = Fixture::new();
        fixture.publish(Some("a"), "first");
        let original = latch(&fixture, "u").await;
        fixture.publish(Some("b"), "first");
        assert_eq!(latch(&fixture, "u").await, original);
        assert_eq!(
            send(&fixture, "r", "u", "Work").await.unwrap()["target"],
            original["target"]
        );
        fixture.publish(None, "first");
        assert_eq!(
            send(&fixture, "other", "u", "More work").await.unwrap()["target"],
            original["target"]
        );
        let commands = fixture.commands.lock();
        assert_eq!(commands.len(), 2);
        for command in commands.iter() {
            assert!(
                matches!(command, Command::Submit { focused: Some(id), generation: Some(generation), .. }
                if id == "a" && generation == "first")
            );
        }
    }

    #[tokio::test]
    async fn utterance_rejects_removed_or_restarted_original_target() {
        let fixture = Fixture::new();
        fixture.publish(Some("a"), "first");
        latch(&fixture, "u").await;
        fixture.publish(Some("b"), "second");
        assert_eq!(
            send(&fixture, "r", "u", "Work").await.unwrap_err().0,
            StatusCode::CONFLICT
        );
        let mut without_original = snapshot(Some("b"), "first");
        without_original["sessions"]
            .as_array_mut()
            .unwrap()
            .retain(|session| session["id"] != "a");
        assert!(fixture.handle.publish_host(without_original));
        assert_eq!(
            send(&fixture, "r", "u", "Work").await.unwrap_err().0,
            StatusCode::CONFLICT
        );
        assert!(fixture.commands.lock().is_empty());
        assert!(fixture.shared().data.lock().requests.is_empty());
    }

    #[tokio::test]
    async fn null_target_stays_null_after_a_session_is_selected_or_restarted() {
        let fixture = Fixture::new();
        fixture.publish(None, "g");
        assert_eq!(latch(&fixture, "u").await["target"], Value::Null);
        fixture.publish(Some("a"), "g");
        assert_eq!(
            send(&fixture, "r", "u", "Coordinate this").await.unwrap()["target"],
            Value::Null
        );
        fixture.publish(Some("b"), "restarted-a");
        assert_eq!(
            send(&fixture, "other", "u", "More work").await.unwrap()["target"],
            Value::Null
        );
        let commands = fixture.commands.lock();
        assert_eq!(commands.len(), 2);
        for command in commands.iter() {
            assert!(matches!(
                command,
                Command::Submit {
                    focused: None,
                    generation: None,
                    ..
                }
            ));
        }
    }

    #[tokio::test]
    async fn cancellation_is_scoped_deduplicated_and_can_be_rejected_safely() {
        let fixture = Fixture::new();
        fixture.publish(Some("a"), "g");
        latch(&fixture, "u").await;
        send(&fixture, "r", "u", "Work").await.unwrap();
        fixture.handle.queued("r");
        assert_eq!(
            cancel(State(fixture.shared()), RoutePath("unknown".into()))
                .await
                .unwrap_err()
                .0,
            StatusCode::NOT_FOUND
        );
        let _ = cancel(State(fixture.shared()), RoutePath("r".into()))
            .await
            .unwrap();
        let _ = cancel(State(fixture.shared()), RoutePath("r".into()))
            .await
            .unwrap();
        assert_eq!(fixture.commands.lock().len(), 2);
        assert_eq!(
            fixture.commands.lock()[1],
            Command::Cancel { id: "r".into() }
        );
        assert!(
            fixture
                .handle
                .reject("r", "Cannot safely target this cancellation")
        );
        assert_eq!(
            request_status(State(fixture.shared()), RoutePath("r".into()))
                .await
                .unwrap()
                .0["state"],
            "queued"
        );
        assert!(fixture.handle.dispatched("r"));
        let _ = cancel(State(fixture.shared()), RoutePath("r".into()))
            .await
            .unwrap();
        assert!(fixture.handle.cancelled("r"));
        assert!(!fixture.handle.queued("r"));
        let count = fixture.commands.lock().len();
        let _ = cancel(State(fixture.shared()), RoutePath("r".into()))
            .await
            .unwrap();
        assert_eq!(fixture.commands.lock().len(), count);
    }

    #[tokio::test]
    async fn state_updates_are_monotonic_bounded_and_observable() {
        let fixture = Fixture::new();
        fixture.publish(None, "g");
        latch(&fixture, "u").await;
        send(&fixture, "r", "u", "Work").await.unwrap();
        assert!(fixture.handle.progress("r", &"é".repeat(MAX_PROGRESS)));
        assert!(fixture.handle.queued("r"));
        assert!(fixture.handle.queued("r"));
        assert!(fixture.handle.dispatched("r"));
        assert!(!fixture.handle.queued("r"));
        assert!(fixture.shared().transition("r", RequestState::Uncertain));
        assert!(fixture.handle.finish("r", "A recovered result\0", true));
        assert!(!fixture.handle.finish("r", "overwrite", false));
        assert!(!fixture.handle.cancelled("r"));
        assert!(!fixture.handle.progress("r", "late"));
        assert!(!fixture.handle.reject("r", "late"));
        let result = request_status(State(fixture.shared()), RoutePath("r".into()))
            .await
            .unwrap()
            .0;
        assert_eq!(result["state"], "completed");
        assert_eq!(
            result["result"],
            json!({"text":"A recovered result","ok":true})
        );
        assert_eq!(
            result["progress"]["text"].as_str().unwrap().len(),
            MAX_PROGRESS
        );
    }

    #[tokio::test]
    async fn accepted_outcomes_are_separate_bounded_non_reexecuting_observations() {
        let fixture = Fixture::new();
        fixture.publish(None, "g");
        latch(&fixture, "u").await;
        send(&fixture, "r", "u", "Work").await.unwrap();
        assert!(
            fixture
                .handle
                .finish("r", "The coordinator accepted the task", true)
        );
        let sequence = fixture.shared().data.lock().sequence;
        assert!(!fixture.handle.outcome("unknown", "Not a request", true));
        assert!(
            fixture
                .handle
                .outcome("r", "The accepted task passed review", true)
        );
        assert!(
            !fixture
                .handle
                .outcome("r", "The accepted task passed review", true)
        );
        assert_eq!(fixture.shared().data.lock().sequence, sequence + 1);
        let result = request_status(State(fixture.shared()), RoutePath("r".into()))
            .await
            .unwrap()
            .0;
        assert_eq!(result["state"], "completed");
        assert_eq!(
            result["result"],
            json!({"text":"The coordinator accepted the task","ok":true})
        );
        assert_eq!(
            result["outcome"],
            json!({"text":"The accepted task passed review","ok":true})
        );
        let event = fixture.shared().data.lock().events.back().cloned().unwrap();
        assert_eq!(event["type"], "request.outcome");
        assert_eq!(event["requestId"], "r");
        assert_eq!(event["state"], "completed");
        assert!(event.get("result").is_none());
        assert!(event.get("text").is_none());
        for number in 1..MAX_OUTCOMES {
            assert!(
                fixture
                    .handle
                    .outcome("r", &format!("Outcome {number}"), false)
            );
        }
        assert!(!fixture.handle.outcome("r", "Overflow", false));
        assert!(
            !fixture
                .handle
                .outcome("r", "The accepted task passed review", true)
        );
        assert!(
            !fixture
                .handle
                .finish("r", "Never replace the original result", false)
        );
        assert_eq!(fixture.commands.lock().len(), 1);
    }

    #[tokio::test]
    async fn queue_ack_does_not_forget_an_in_flight_cancel() {
        let fixture = Fixture::new();
        fixture.publish(None, "g");
        latch(&fixture, "u").await;
        send(&fixture, "r", "u", "Work").await.unwrap();
        let _ = cancel(State(fixture.shared()), RoutePath("r".into()))
            .await
            .unwrap();
        assert!(fixture.handle.queued("r"));
        assert!(fixture.handle.dispatched("r"));
        assert!(fixture.handle.reject("r", "Cancellation is not safe now"));
        assert_eq!(
            request_status(State(fixture.shared()), RoutePath("r".into()))
                .await
                .unwrap()
                .0["state"],
            "dispatched"
        );
    }

    #[tokio::test]
    async fn callback_partial_failure_is_uncertain_and_not_replayed() {
        let fixture = Fixture::new();
        let mut shared = fixture.shared();
        // A separate shared value allows replacing the test callback before use.
        drop(shared);
        let (handle, _) = test_handle(fixture._root.path());
        let mut inner = Arc::try_unwrap(handle.inner).ok().unwrap();
        let owned = Arc::get_mut(&mut inner.shared).unwrap();
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let seen = calls.clone();
        owned.callback = Arc::new(move |_| {
            seen.fetch_add(1, Ordering::Relaxed);
            panic!("simulated partial callback failure");
        });
        let handle = Handle {
            inner: Arc::new(inner),
        };
        assert!(handle.publish_host(snapshot(None, "g")));
        shared = handle.inner.shared.clone();
        let _ = utterance(
            State(shared.clone()),
            Json(UtteranceInput { id: "u".into() }),
        )
        .await
        .unwrap();
        for _ in 0..2 {
            let result = submit(
                State(shared.clone()),
                Json(SubmitInput {
                    id: "r".into(),
                    utterance_id: "u".into(),
                    text: "Work".into(),
                }),
            )
            .await
            .unwrap()
            .0;
            assert_eq!(result["state"], "uncertain");
        }
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        assert!(handle.finish("r", "Observed completion", true));
    }

    #[tokio::test]
    async fn focus_only_uses_advertised_ids_and_waits_for_host_publication() {
        let fixture = Fixture::new();
        fixture.publish(Some("a"), "g");
        assert!(
            focus(
                State(fixture.shared()),
                Json(FocusInput {
                    session_id: "unknown".into()
                })
            )
            .await
            .is_err()
        );
        let _ = focus(
            State(fixture.shared()),
            Json(FocusInput {
                session_id: "b".into(),
            }),
        )
        .await
        .unwrap();
        assert_eq!(
            fixture.commands.lock()[0],
            Command::Focus {
                session_id: "b".into()
            }
        );
        assert_eq!(latch(&fixture, "u").await["target"]["sessionId"], "a");
    }

    #[tokio::test]
    async fn host_transport_preserves_additive_capabilities_and_revocation_keeps_lifecycle() {
        let fixture = Fixture::new();
        *fixture.shared().lifecycle.lock() = lifecycle::Lifecycle::load(fixture._root.path(), true).unwrap();
        let snapshot = json!({"mode":"attached","sessions":[],"avatarPlacement":{"schema":1,"allowEscape":false}});
        assert!(fixture.handle.publish_host(snapshot.clone()));
        let transported = host(State(fixture.shared())).await.0;
        assert!(transported["capabilities"].as_array().unwrap().contains(&json!(lifecycle::CAPABILITY)));
        assert!(transported["capabilities"].as_array().unwrap().contains(&json!("avatar.containment.v1")));
        fixture.handle.revoke_work();
        assert!(!fixture.handle.publish_host(snapshot));
        let revoked = host(State(fixture.shared())).await.0;
        assert!(revoked["sessions"].as_array().unwrap().is_empty());
        assert!(revoked["capabilities"].as_array().unwrap().contains(&json!(lifecycle::CAPABILITY)));
    }

    #[test]
    fn explicit_containment_never_falls_back_to_desktop_or_infers_escape() {
        for permission in [Value::Null, json!({}), json!({"schema":1,"allowEscape":"true"}), json!({"schema":1,"allowEscape":false})] {
            let safe = normalize_host(json!({"mode":"desktop","sessions":[],"avatarPlacement":permission,"avatarRoam":{"schema":1,"policy":"auto"}}), "host").unwrap();
            assert_eq!(safe["mode"], "attached");
            assert_eq!(safe["avatarPlacement"]["allowEscape"], false);
            assert!(safe.get("avatarRoam").is_none());
        }
        let safe = normalize_host(json!({"mode":"attached","sessions":[],"avatarPlacement":{"schema":1,"allowEscape":true},"avatarRoam":{"schema":1,"policy":"auto"}}), "host").unwrap();
        assert!(safe["window"].is_null());
        assert_eq!(safe["mode"], "attached");
        assert_eq!(safe["avatarRoam"]["policy"], "auto");
    }

    #[test]
    fn host_snapshot_is_whitelisted_validated_and_bounded() {
        let fixture = Fixture::new();
        assert!(!fixture.handle.ready());
        let mut input = snapshot(Some("a"), "g");
        input["token"] = json!("not-for-clients");
        input["sessions"][0]["secret"] = json!("not-for-clients");
        input["sessions"][0]["context"] = json!("é\0".repeat(4000));
        assert!(fixture.handle.publish_host(input));
        assert!(fixture.handle.ready());
        let data = fixture.shared();
        let data = data.data.lock();
        assert!(data.host.get("token").is_none());
        assert!(data.host["sessions"][0].get("secret").is_none());
        assert!(data.host["sessions"][0]["context"].as_str().unwrap().len() <= 4096);
        assert_eq!(data.host["protocol"], 1);
        assert_eq!(data.host["instanceId"], fixture.handle.instance_id());
        drop(data);
        let mut input = snapshot(Some("a"), "g");
        input["focusedSessionId"] = json!("missing");
        assert!(!fixture.handle.publish_host(input));
        let mut input = snapshot(None, "g");
        input["sessions"][1]["id"] = json!("a");
        assert!(!fixture.handle.publish_host(input));
        let mut input = snapshot(None, "g");
        input["window"] = json!({"handle":"123","pid":0});
        assert!(!fixture.handle.publish_host(input));
        let mut input = snapshot(None, "g");
        input["window"] = json!({"handle":"123","pid":123});
        assert!(fixture.handle.publish_host(input));
        assert!(
            !fixture
                .handle
                .status()
                .to_string()
                .contains("not-for-clients")
        );
    }

    #[test]
    fn event_sequences_batches_and_gaps_are_bounded() {
        let mut data = Data::new("instance");
        assert_eq!(data.batch(0), json!({"events":[],"cursor":0,"gap":false}));
        for _ in 0..MAX_EVENTS + 40 {
            data.event("test", Some("r"), json!({"state":"received"}));
        }
        assert_eq!(data.events.len(), MAX_EVENTS);
        let first = data.batch(0);
        assert_eq!(first["gap"], true);
        assert_eq!(first["events"].as_array().unwrap().len(), EVENT_BATCH);
        assert_eq!(first["events"][0]["sequence"], 41);
        let second = data.batch(first["cursor"].as_u64().unwrap());
        assert_eq!(second["gap"], false);
        assert_eq!(second["cursor"], data.sequence);
        assert_eq!(data.batch(data.sequence)["events"], json!([]));
        assert_eq!(data.batch(data.sequence + 1)["gap"], true);
    }

    #[tokio::test]
    async fn retention_limits_preserve_dedup_instead_of_evicting_ids() {
        let fixture = Fixture::new();
        fixture.publish(None, "g");
        latch(&fixture, "u").await;
        send(&fixture, "r", "u", "Work").await.unwrap();
        {
            let shared = fixture.shared();
            let mut data = shared.data.lock();
            for number in 0..MAX_UTTERANCES {
                data.utterances.insert(format!("u-{number}"), None);
            }
            let record = data.requests["r"].clone();
            for number in 0..MAX_REQUESTS {
                data.requests.insert(format!("r-{number}"), record.clone());
            }
        }
        assert_eq!(
            utterance(
                State(fixture.shared()),
                Json(UtteranceInput { id: "new".into() })
            )
            .await
            .unwrap_err()
            .0,
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(latch(&fixture, "u").await["id"], "u");
        assert_eq!(
            send(&fixture, "new", "u", "Work").await.unwrap_err().0,
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(
            send(&fixture, "r", "u", "Work").await.unwrap()["state"],
            "received"
        );
        assert_eq!(fixture.commands.lock().len(), 1);
        assert!(
            serde_json::from_value::<SubmitInput>(
                json!({"id":"r","utteranceId":"u","text":"work","target":"a"})
            )
            .is_err()
        );
        for id in [
            "".to_string(),
            "x".repeat(MAX_ID + 1),
            "a\nb".into(),
            " a".into(),
        ] {
            assert!(validate_id(&id).is_err());
        }
    }

    #[test]
    fn private_descriptor_publication_replacement_and_cleanup_are_owner_safe() {
        let fixture = Fixture::new();
        let shared = fixture.shared();
        private_directory(shared.descriptor_path.parent().unwrap()).unwrap();
        write_descriptor(&shared).unwrap();
        let first: Descriptor =
            serde_json::from_slice(&std::fs::read(fixture.handle.descriptor_path()).unwrap())
                .unwrap();
        assert_eq!(first.protocol, 1);
        assert_eq!(first.instance_id, fixture.handle.instance_id());
        let (second, _) = test_handle(fixture._root.path());
        write_descriptor(&second.inner.shared).unwrap();
        remove_owned_descriptor(&shared);
        assert!(second.descriptor_path().exists());
        let published: Descriptor =
            serde_json::from_slice(&std::fs::read(second.descriptor_path()).unwrap()).unwrap();
        assert_eq!(published.instance_id, second.instance_id());
        remove_owned_descriptor(&second.inner.shared);
        assert!(!second.descriptor_path().exists());
        let collision = shared
            .descriptor_path
            .with_file_name(format!(".connection.{}.tmp", shared.instance_id));
        std::fs::write(&collision, b"preexisting").unwrap();
        assert!(write_descriptor(&shared).is_err());
        assert_eq!(std::fs::read(collision).unwrap(), b"preexisting");
    }

    #[cfg(windows)]
    #[test]
    fn windows_descriptor_acl_grants_only_the_current_user() {
        let fixture = Fixture::new();
        let shared = fixture.shared();
        let directory = shared.descriptor_path.parent().unwrap();
        private_directory(directory).unwrap();
        write_descriptor(&shared).unwrap();
        let directory_acl = windows_private::security_text(directory).unwrap();
        let current_sid = directory_acl
            .strip_prefix("O:")
            .unwrap()
            .split("D:")
            .next()
            .unwrap();
        assert!(current_sid.starts_with("S-1-5-"), "{directory_acl}");
        assert!(directory_acl.contains("D:P"), "{directory_acl}");
        for path in [&shared.descriptor_path, &directory.join(".connection.lock")] {
            let acl = windows_private::security_text(path).unwrap();
            assert_eq!(acl.matches('(').count(), 1, "{acl}");
            assert!(acl.contains(&format!(";;;{current_sid})")), "{acl}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn unix_permissions_and_symlink_refusal() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let fixture = Fixture::new();
        let shared = fixture.shared();
        private_directory(shared.descriptor_path.parent().unwrap()).unwrap();
        write_descriptor(&shared).unwrap();
        assert_eq!(
            std::fs::metadata(&shared.descriptor_path)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(shared.descriptor_path.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        let other = tempfile::tempdir().unwrap();
        let link = other.path().join("orbit");
        symlink(shared.descriptor_path.parent().unwrap(), &link).unwrap();
        assert!(private_directory(&link).is_err());
    }

    // Exercise the real HTTP router over in-memory IO, not an actual network.
    struct MemoryListener(Option<tokio::io::DuplexStream>);
    impl axum::serve::Listener for MemoryListener {
        type Io = tokio::io::DuplexStream;
        type Addr = SocketAddr;
        async fn accept(&mut self) -> (Self::Io, Self::Addr) {
            let io = match self.0.take() {
                Some(io) => io,
                None => std::future::pending().await,
            };
            (io, "127.0.0.1:3000".parse().unwrap())
        }
        fn local_addr(&self) -> io::Result<Self::Addr> {
            Ok("127.0.0.1:12345".parse().unwrap())
        }
    }

    async fn http(
        fixture: &Fixture,
        method: &str,
        path: &str,
        additional: &str,
        body: &str,
        authenticated: bool,
    ) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (mut client, server) = tokio::io::duplex(128 * 1024);
        let app = router(fixture.shared()).layer(axum::Extension(ConnectInfo(
            "127.0.0.1:3000".parse::<SocketAddr>().unwrap(),
        )));
        let server = tokio::spawn(async move {
            axum::serve(MemoryListener(Some(server)), app)
                .await
                .unwrap();
        });
        let authentication = if authenticated {
            format!("Authorization: Bearer {}\r\n", fixture.shared().token)
        } else {
            String::new()
        };
        let wire = format!(
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:12345\r\n{authentication}{additional}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        client.write_all(wire.as_bytes()).await.unwrap();
        let mut bytes = Vec::new();
        tokio::time::timeout(Duration::from_secs(2), client.read_to_end(&mut bytes))
            .await
            .unwrap()
            .unwrap();
        server.abort();
        String::from_utf8(bytes).unwrap()
    }

    #[tokio::test]
    async fn router_has_no_lan_auth_fallback_or_administration_routes() {
        let fixture = Fixture::new();
        fixture.publish(None, "g");
        for path in ["/orbit/v1/host", "/orbit/v1/events", "/api/sessions", "/ws"] {
            let response = http(&fixture, "GET", path, "", "", false).await;
            assert!(response.starts_with("HTTP/1.1 401"), "{response}");
        }
        assert!(
            http(
                &fixture,
                "GET",
                "/orbit/v1/host?token=legacy",
                "x-terminal-web-token: legacy\r\n",
                "",
                false
            )
            .await
            .starts_with("HTTP/1.1 401")
        );
        assert!(
            http(
                &fixture,
                "GET",
                "/orbit/v1/host",
                "Origin: null\r\n",
                "",
                true
            )
            .await
            .starts_with("HTTP/1.1 403")
        );
        for path in ["/api/sessions", "/ws", "/orbit/v1/admin", "/orbit/v1/input"] {
            assert!(
                http(&fixture, "GET", path, "", "", true)
                    .await
                    .starts_with("HTTP/1.1 404")
            );
        }
        let response = http(&fixture, "GET", "/orbit/v1/host", "", "", true).await;
        assert!(response.starts_with("HTTP/1.1 200"));
        assert!(response.contains("cache-control: no-store"));
        assert!(!response.contains("access-control-allow-origin"));
        assert!(!response.contains(&fixture.shared().token));
        assert!(fixture.handle.connected());
    }

    #[tokio::test]
    async fn router_submits_to_the_original_target_after_focus_changes() {
        let fixture = Fixture::new();
        fixture.publish(Some("a"), "first");
        assert!(
            http(
                &fixture,
                "POST",
                "/orbit/v1/utterances",
                "",
                r#"{"id":"u"}"#,
                true
            )
            .await
            .starts_with("HTTP/1.1 200")
        );
        fixture.publish(Some("b"), "first");
        assert!(
            http(
                &fixture,
                "POST",
                "/orbit/v1/requests",
                "",
                r#"{"id":"r","utteranceId":"u","text":"Work"}"#,
                true
            )
            .await
            .starts_with("HTTP/1.1 200")
        );
        assert!(matches!(&fixture.commands.lock()[0], Command::Submit {
            focused: Some(id), generation: Some(generation), ..
        } if id == "a" && generation == "first"));
        fixture.publish(Some("b"), "second");
        assert!(
            http(
                &fixture,
                "POST",
                "/orbit/v1/requests",
                "",
                r#"{"id":"other","utteranceId":"u","text":"Work"}"#,
                true
            )
            .await
            .starts_with("HTTP/1.1 409")
        );
        assert_eq!(fixture.commands.lock().len(), 1);

        let no_target = Fixture::new();
        no_target.publish(None, "first");
        assert!(
            http(
                &no_target,
                "POST",
                "/orbit/v1/utterances",
                "",
                r#"{"id":"u"}"#,
                true
            )
            .await
            .starts_with("HTTP/1.1 200")
        );
        no_target.publish(Some("a"), "first");
        assert!(
            http(
                &no_target,
                "POST",
                "/orbit/v1/requests",
                "",
                r#"{"id":"r","utteranceId":"u","text":"Work"}"#,
                true
            )
            .await
            .starts_with("HTTP/1.1 200")
        );
        assert!(matches!(
            &no_target.commands.lock()[0],
            Command::Submit {
                focused: None,
                generation: None,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn router_enforces_all_body_limits_validation_and_event_queries() {
        let fixture = Fixture::new();
        fixture.publish(None, "g");
        for path in [
            "/orbit/v1/host",
            "/orbit/v1/requests/r/cancel",
            "/orbit/v1/requests",
        ] {
            assert!(
                http(&fixture, "POST", path, "", &"x".repeat(MAX_BODY + 1), true)
                    .await
                    .starts_with("HTTP/1.1 413")
            );
        }
        assert!(
            http(
                &fixture,
                "POST",
                "/orbit/v1/utterances",
                "",
                r#"{"id":"u","target":"retarget"}"#,
                true
            )
            .await
            .starts_with("HTTP/1.1 422")
        );
        assert!(
            http(
                &fixture,
                "POST",
                "/orbit/v1/utterances",
                "",
                r#"{"id":"u"}"#,
                true
            )
            .await
            .starts_with("HTTP/1.1 200")
        );
        assert!(
            http(
                &fixture,
                "POST",
                "/orbit/v1/requests",
                "",
                r#"{"id":"r","utteranceId":"u","text":"Work"}"#,
                true
            )
            .await
            .starts_with("HTTP/1.1 200")
        );
        assert!(
            http(&fixture, "GET", "/orbit/v1/requests/r", "", "", true)
                .await
                .contains("\"state\":\"received\"")
        );
        assert!(
            http(
                &fixture,
                "GET",
                "/orbit/v1/events?after=invalid",
                "",
                "",
                true
            )
            .await
            .starts_with("HTTP/1.1 400")
        );
        assert!(
            http(&fixture, "GET", "/orbit/v1/events?after=0", "", "", true)
                .await
                .contains("request.state")
        );
        assert_eq!(fixture.commands.lock().len(), 1);
    }

    #[test]
    #[ignore = "actual loopback network test; uses only an explicit temporary data root"]
    fn independent_network_server_rotates_credentials_and_cleans_up() {
        let root = tempfile::tempdir().unwrap();
        let first = start(root.path(), |_| {}).unwrap();
        assert!(!first.ready());
        assert!(first.publish_host(snapshot(None, "g")));
        assert!(first.ready());
        let descriptor: Descriptor =
            serde_json::from_slice(&std::fs::read(first.descriptor_path()).unwrap()).unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let client = reqwest::Client::builder().no_proxy().build().unwrap();
            assert_eq!(
                client
                    .get(format!("{}/orbit/v1/host", first.local_url()))
                    .send()
                    .await
                    .unwrap()
                    .status(),
                StatusCode::UNAUTHORIZED
            );
            assert!(
                client
                    .get(format!("{}/orbit/v1/host", first.local_url()))
                    .bearer_auth(&descriptor.token)
                    .send()
                    .await
                    .unwrap()
                    .status()
                    .is_success()
            );
        });
        let second = start(root.path(), |_| {}).unwrap();
        let replacement: Descriptor =
            serde_json::from_slice(&std::fs::read(second.descriptor_path()).unwrap()).unwrap();
        assert_ne!(descriptor.token, replacement.token);
        first.shutdown();
        assert!(second.descriptor_path().exists());
        second.shutdown();
        assert!(!second.descriptor_path().exists());
    }
}
