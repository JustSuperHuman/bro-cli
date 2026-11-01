//! Managed runtime metadata and native controls. No audio, provider calls, or model authority.
use super::*;

pub const CAPABILITY: &str = "voice.lifecycle.v1";
const LEASE_MS: u64 = 6000;
const MAX_COMMANDS: usize = 16;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Phase { Off, Connecting, Listening, Thinking, Speaking }
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Microphone { Off, Requesting, Active, Denied, Unknown }
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Provider { Openai, Grok }
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum CredentialSource { Saved, Process, User, Machine, None }
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum RuntimeFailure { MicrophoneDenied, MicrophoneFailed, ProviderConnectionFailed }

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Preferences {
    pub provider: Provider,
    pub model: String,
    pub voice: String,
    pub volume: u8,
    pub listen_mode: String,
}
impl Preferences {
    fn valid(&self) -> bool {
        identifier(&self.model) && identifier(&self.voice) && self.volume <= 100
            && matches!(self.listen_mode.as_str(), "always" | "tap")
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum VoiceControl {
    Start {},
    Stop {},
    SetMute { muted: bool },
    ShowSettings {},
    Quit {},
    Shutdown {},
    SetPreferences { preferences: Preferences },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeStatus {
    pub runtime_id: String,
    pub pid: u32,
    pub build_id: String,
    pub renderer_ready: bool,
    pub preferences: Preferences,
    pub credential_available: bool,
    pub credential_source: CredentialSource,
    pub microphone: Microphone,
    pub phase: Phase,
    pub muted: bool,
    #[serde(default)]
    pub failure: Option<RuntimeFailure>,
}
impl RuntimeStatus {
    fn valid(&self) -> bool {
        validate_id(&self.runtime_id).is_ok() && self.pid > 0
            && self.build_id.len() == 64 && self.build_id.bytes().all(|c| c.is_ascii_hexdigit())
            && self.preferences.valid()
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Identity {
    runtime_id: String,
    pid: u32,
    build_id: String,
    #[serde(default)] process_identity: Option<String>,
}
struct Expected {
    pid: u32,
    build_id: String,
    process_identity: Option<String>,
    created: Instant,
}
#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Saved {
    #[serde(default)] intentional_stop: bool,
    #[serde(default)] runtime: Option<Identity>,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Pending {
    id: String,
    runtime_id: String,
    owner_epoch: String,
    command: VoiceControl,
    remaining_ms: u64,
    #[serde(skip)] created: Instant,
}

pub(super) struct Lifecycle {
    enabled: bool,
    path: PathBuf,
    saved: Saved,
    expected: Option<Expected>,
    status: Option<RuntimeStatus>,
    seen: Option<Instant>,
    pending: VecDeque<Pending>,
}
impl Lifecycle {
    pub(super) fn load(directory: &Path, enabled: bool) -> Result<Self> {
        let path = directory.join("managed-runtime.json");
        let saved = if enabled {
            match std::fs::read(&path) {
                Ok(bytes) if bytes.len() <= 4096 => serde_json::from_slice(&bytes).context("Invalid managed Orbit state; preserved for recovery")?,
                Ok(_) => bail!("Managed Orbit state exceeds its limit"),
                Err(e) if e.kind() == io::ErrorKind::NotFound => Saved::default(),
                Err(e) => return Err(e.into()),
            }
        } else { Saved::default() };
        Ok(Self { enabled, path, saved, expected: None, status: None, seen: None, pending: VecDeque::new() })
    }
    fn save(&self) -> Result<(), String> {
        crate::project_store::write_atomic(&self.path, &serde_json::to_vec(&self.saved).map_err(|_| "Unable to save managed Orbit state")?)
            .map_err(|_| "Unable to save managed Orbit state".into())
    }
    pub(super) fn enabled(&self) -> bool { self.enabled }
    pub(super) fn expect(&mut self, pid: u32, build: String) {
        self.expected = Some(Expected { pid, build_id: build, process_identity: bro_core::orbit_voice::managed_process_identity(pid), created: Instant::now() });
    }
    pub(super) fn expect_process(&mut self, process: &bro_core::orbit_voice::ManagedLaunch) {
        self.expected = Some(Expected { pid: process.pid, build_id: process.build_id.clone(), process_identity: Some(process.process_identity.clone()), created: Instant::now() });
    }
    pub(super) fn launch_pending(&self) -> bool { self.expected.as_ref().is_some_and(|expected| expected.created.elapsed() < Duration::from_secs(15)) }
    pub(super) fn status(&self) -> Option<RuntimeStatus> {
        self.seen.filter(|seen| seen.elapsed() < Duration::from_millis(LEASE_MS)).and(self.status.clone())
    }
    pub(super) fn stopped(&self) -> bool { self.saved.intentional_stop }
    pub(super) fn previously_owned(&self) -> bool { self.saved.runtime.is_some() }
    pub(super) fn mark_intentional_stop(&mut self) -> Result<(), String> {
        self.saved.intentional_stop = true;
        self.save()
    }
    pub(super) fn resume(&mut self) -> Result<(), String> {
        self.saved.intentional_stop = false;
        self.save()
    }
    pub(super) fn queue(&mut self, command: VoiceControl, epoch: &str) -> Result<String, String> {
        let status = self.status().ok_or("Orbit's managed runtime is not connected")?;
        if matches!(command, VoiceControl::Start {}) && (!status.renderer_ready || self.saved.intentional_stop) {
            return Err("Resume Orbit and wait for its voice controls to be ready".into());
        }
        if let VoiceControl::SetPreferences { preferences } = &command
            && !preferences.valid() { return Err("Invalid voice preferences".into()); }
        if self.pending.len() >= MAX_COMMANDS { return Err("Orbit's control queue is full".into()); }
        if matches!(command, VoiceControl::Quit {}) {
            self.saved.intentional_stop = true;
            self.save()?; // durable quit suppression before requesting a side effect
        }
        let id = uuid::Uuid::new_v4().to_string();
        self.pending.push_back(Pending { id: id.clone(), runtime_id: status.runtime_id, owner_epoch: epoch.into(), command, remaining_ms: 10_000, created: Instant::now() });
        Ok(id)
    }
    fn heartbeat(&mut self, status: RuntimeStatus, acknowledgements: &[String], epoch: &str) -> Result<Value, String> {
        if !self.enabled || !status.valid() || acknowledgements.len() > MAX_COMMANDS
            || acknowledgements.iter().any(|id| validate_id(id).is_err()) {
            return Err("Invalid managed runtime metadata".into());
        }
        let saved = self.saved.runtime.as_ref().filter(|r| r.runtime_id == status.runtime_id && r.pid == status.pid && r.build_id == status.build_id);
        let expected = self.expected.as_ref().filter(|r| r.pid == status.pid && r.build_id == status.build_id && r.created.elapsed() < Duration::from_secs(15));
        if saved.is_none() && expected.is_none() { return Err("This runtime is not owned by the selected bro workspace".into()); }
        let process_identity = bro_core::orbit_voice::managed_process_identity(status.pid);
        let known = saved.is_some_and(|r| process_identity.is_some() && r.process_identity == process_identity);
        let spawned = expected.is_some_and(|r| process_identity.is_some() && r.process_identity == process_identity);
        if !known && !spawned { return Err("The owned runtime process lifetime changed or cannot be verified".into()); }
        if !known {
            self.saved.runtime = Some(Identity { runtime_id: status.runtime_id.clone(), pid: status.pid, build_id: status.build_id.clone(), process_identity });
            self.save()?;
        }
        self.expected = None;
        // An acknowledgement is meaningful only from this runtime and owner epoch.
        self.pending.retain(|p| p.runtime_id == status.runtime_id && p.owner_epoch == epoch
            && p.created.elapsed() < Duration::from_secs(10) && !acknowledgements.contains(&p.id));
        for p in &mut self.pending { p.remaining_ms = 10_000u64.saturating_sub(p.created.elapsed().as_millis() as u64); }
        self.seen = Some(Instant::now());
        self.status = Some(status);
        Ok(json!({"schema":1,"ownerEpoch":epoch,"leaseMs":LEASE_MS,"intentionalStop":self.saved.intentional_stop,"commands":self.pending}))
    }
}
fn identifier(value: &str) -> bool {
    !value.is_empty() && value.len() <= 96 && !value.starts_with("sk-")
        && value.bytes().all(|c| c.is_ascii_alphanumeric() || b"-_.:/".contains(&c))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Heartbeat {
    lineage: String,
    owner_epoch: String,
    status: RuntimeStatus,
    acknowledgements: Vec<String>,
    #[serde(default)] intentional_stop: bool,
    #[serde(default)] runtime_stopped: bool,
}
pub(super) async fn heartbeat(State(shared): State<Arc<Shared>>, Json(body): Json<Heartbeat>) -> Result<Json<Value>, ApiError> {
    if body.lineage != shared.lineage || body.owner_epoch != shared.instance_id {
        return Err(error(StatusCode::CONFLICT, "stale_owner", "Refresh the original protected pairing descriptor."));
    }
    let reject = || error(StatusCode::CONFLICT, "runtime_rejected", "The managed runtime is not authorized or its metadata is invalid.");
    if body.runtime_stopped && (body.status.microphone != Microphone::Off || body.status.phase != Phase::Off) { return Err(reject()); }
    let mut lifecycle = shared.lifecycle.lock();
    let value = lifecycle.heartbeat(body.status, &body.acknowledgements, &shared.instance_id).map_err(|_| reject())?;
    if body.intentional_stop || body.runtime_stopped {
        lifecycle.saved.intentional_stop |= body.intentional_stop;
        if body.runtime_stopped { lifecycle.saved.runtime = None; lifecycle.expected = None; lifecycle.status = None; lifecycle.seen = None; lifecycle.pending.clear(); }
        lifecycle.save().map_err(|_| reject())?;
    }
    Ok(Json(value))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn status() -> RuntimeStatus {
        RuntimeStatus { runtime_id: "runtime-a".into(), pid: std::process::id(), build_id: "a".repeat(64), renderer_ready: true,
            preferences: Preferences { provider: Provider::Openai, model: "gpt-realtime".into(), voice: "marin".into(), volume: 70, listen_mode: "always".into() },
            credential_available: true, credential_source: CredentialSource::Process, microphone: Microphone::Off, phase: Phase::Off, muted: false, failure: None }
    }
    #[test]
    fn owner_lease_is_exclusive_and_only_contention_reports_another_owner() {
        let root = tempfile::tempdir().unwrap();
        let owner = managed_owner(root.path()).unwrap();
        assert!(managed_owner(root.path()).unwrap_err().is::<ManagedOwnerBusy>());
        drop(owner);
        let next_owner = managed_owner(root.path()).unwrap();
        let error = managed_owner(&root.path().join("missing-directory")).unwrap_err();
        assert!(!error.is::<ManagedOwnerBusy>(), "setup errors must not masquerade as a primary owner");
        drop(next_owner);
    }
    #[test]
    #[cfg(any(windows, target_os = "linux"))]
    fn matching_pid_and_build_cannot_adopt_another_process_lifetime() {
        let root = tempfile::tempdir().unwrap();
        let mut state = Lifecycle::load(root.path(), true).unwrap();
        state.expect(status().pid, "a".repeat(64));
        state.heartbeat(status(), &[], "owner-a").unwrap();
        state.saved.runtime.as_mut().unwrap().process_identity = Some("different-lifetime".into());
        state.save().unwrap();
        let mut recovered = Lifecycle::load(root.path(), true).unwrap();
        assert!(recovered.heartbeat(status(), &[], "owner-b").is_err());
        recovered.saved.runtime.as_mut().unwrap().process_identity = None;
        assert!(recovered.heartbeat(status(), &[], "owner-b").is_err(), "legacy state does not attest a process lifetime");
    }
    #[test]
    #[cfg(any(windows, target_os = "linux"))]
    fn verified_managed_discovery_recovers_a_missing_record_without_starting_voice() {
        let root = tempfile::tempdir().unwrap();
        let runtime = status();
        let mut state = Lifecycle::load(root.path(), true).unwrap();
        assert!(state.heartbeat(runtime.clone(), &[], "new-owner").is_err());
        let mut discovered = bro_core::orbit_voice::ManagedLaunch {
            pid: runtime.pid, build_id: runtime.build_id.clone(),
            process_identity: "wrong-lifetime".into(), reused: true,
        };
        state.expect_process(&discovered);
        assert!(state.heartbeat(runtime.clone(), &[], "new-owner").is_err());
        discovered.process_identity = bro_core::orbit_voice::managed_process_identity(runtime.pid).unwrap();
        state.expect_process(&discovered);
        let mut wrong_build = runtime.clone(); wrong_build.build_id = "b".repeat(64);
        assert!(state.heartbeat(wrong_build, &[], "new-owner").is_err());
        let response = state.heartbeat(runtime.clone(), &[], "new-owner").unwrap();
        assert!(response["commands"].as_array().unwrap().is_empty());
        assert_eq!(state.status().unwrap().phase, Phase::Off);
        assert!(!state.launch_pending());
        let mut recovered = Lifecycle::load(root.path(), true).unwrap();
        assert!(recovered.heartbeat(runtime, &[], "another-owner").is_ok());
    }
    #[test]
    #[cfg(any(windows, target_os = "linux"))]
    fn expired_launch_expectations_cannot_authorize_a_late_runtime() {
        let root = tempfile::tempdir().unwrap();
        let mut state = Lifecycle::load(root.path(), true).unwrap();
        state.expect(status().pid, "a".repeat(64));
        state.expected.as_mut().unwrap().created = Instant::now() - Duration::from_secs(16);
        assert!(!state.launch_pending());
        assert!(state.heartbeat(status(), &[], "owner").is_err());
    }
    #[test]
    #[cfg(any(windows, target_os = "linux"))]
    fn owned_runtime_only_commands_are_epoch_bound_and_quit_is_durable() {
        let root = tempfile::tempdir().unwrap();
        let mut state = Lifecycle::load(root.path(), true).unwrap();
        assert!(state.heartbeat(status(), &[], "owner-a").is_err());
        state.expect(status().pid, "a".repeat(64));
        assert!(state.heartbeat(status(), &[], "owner-a").is_ok());
        let id = state.queue(VoiceControl::Start {}, "owner-a").unwrap();
        assert_eq!(state.heartbeat(status(), &[], "owner-a").unwrap()["commands"].as_array().unwrap().len(), 1);
        assert!(state.heartbeat(status(), &[id], "owner-a").unwrap()["commands"].as_array().unwrap().is_empty());
        state.queue(VoiceControl::Start {}, "owner-a").unwrap();
        assert!(state.heartbeat(status(), &[], "owner-b").unwrap()["commands"].as_array().unwrap().is_empty());
        state.queue(VoiceControl::Quit {}, "owner-b").unwrap();
        let mut recovered = Lifecycle::load(root.path(), true).unwrap();
        assert!(recovered.stopped());
        assert!(recovered.heartbeat(status(), &[], "owner-c").is_ok());
        assert!(recovered.queue(VoiceControl::Start {}, "owner-c").is_err());
        recovered.resume().unwrap();
        assert!(!Lifecycle::load(root.path(), true).unwrap().stopped());
    }
    #[test]
    fn status_rejects_extra_and_secret_bearing_metadata() {
        let mut value = serde_json::to_value(status()).unwrap();
        value["token"] = json!("must-not-be-accepted");
        assert!(serde_json::from_value::<RuntimeStatus>(value).is_err());
        let mut s = status(); s.preferences.model = "sk-secret".into(); assert!(!s.valid());
        assert!(serde_json::from_value::<VoiceControl>(json!({"kind":"start","caller":"desktop"})).is_err());
    }
}
