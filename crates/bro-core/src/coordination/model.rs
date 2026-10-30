use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ActorRole {
    Host,
    Coordinator,
    Worker,
}

/// The bridge authenticates this identity. A JSON argument must never grant Host authority.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Actor {
    pub id: String,
    pub role: ActorRole,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TaskState {
    #[default]
    Pending,
    Ready,
    Assigned,
    Running,
    Blocked,
    Uncertain,
    Submitted,
    Verifying,
    Verified,
    CancelRequested,
    Cancelled,
    Failed,
    Stopped,
}
impl TaskState {
    pub fn terminal(self) -> bool {
        matches!(self, Self::Verified | Self::Cancelled | Self::Stopped)
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Ready => "ready",
            Self::Assigned => "assigned",
            Self::Running => "running",
            Self::Blocked => "blocked",
            Self::Uncertain => "uncertain",
            Self::Submitted => "submitted",
            Self::Verifying => "verifying",
            Self::Verified => "verified",
            Self::CancelRequested => "cancellation requested",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
            Self::Stopped => "stopped",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Lane {
    #[default]
    Work,
    Completion,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CriterionKind {
    #[default]
    Check,
    Review,
    Artifact,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum QualityMode {
    Quick,
    #[default]
    Standard,
    Critical,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Criterion {
    pub id: String,
    pub description: String,
    pub kind: CriterionKind,
    pub required: bool,
    pub command: Option<String>,
    pub independent: bool,
}
impl Default for Criterion {
    fn default() -> Self {
        Self {
            id: String::new(),
            description: String::new(),
            kind: CriterionKind::Check,
            required: true,
            command: None,
            independent: false,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TaskContract {
    pub quality: QualityMode,
    pub goal: String,
    pub instructions: String,
    pub constraints: Vec<String>,
    pub acceptance: Vec<Criterion>,
    /// Source identities and their content digests; relevant edits invalidate this contract's evidence.
    pub sources: BTreeMap<String, String>,
    pub context: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ClaimMode {
    Read,
    #[default]
    Exclusive,
}

/// Keys are canonical, global resource identities (for example repo:<canonical-path>).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ResourceClaim {
    pub key: String,
    pub mode: ClaimMode,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Budget {
    pub max_attempts: u32,
    pub deadline: Option<i64>,
    pub max_tokens: Option<u64>,
    pub max_cost_usd: Option<f64>,
}
impl Default for Budget {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            deadline: None,
            max_tokens: None,
            max_cost_usd: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct NewIntent {
    pub id: String,
    pub title: String,
    pub description: String,
    pub constraints: Vec<String>,
    pub budget: Budget,
    pub authorization: Authorization,
}
impl Default for NewIntent {
    fn default() -> Self {
        Self {
            id: String::new(),
            title: String::new(),
            description: String::new(),
            constraints: vec![],
            budget: Budget {
                max_attempts: 128,
                ..Budget::default()
            },
            authorization: Authorization::default(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Authorization {
    pub scope: String,
    pub source: String,
    pub authorized_actions: Vec<String>,
    pub recorded_at: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Intent {
    pub id: String,
    pub title: String,
    pub description: String,
    pub constraints: Vec<String>,
    pub budget: Budget,
    pub authorization: Authorization,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct NewTask {
    pub id: String,
    pub title: String,
    pub project: String,
    pub workspace: Option<String>,
    pub integration_owner: Option<String>,
    pub parent_id: Option<String>,
    pub dependencies: Vec<String>,
    pub contract: TaskContract,
    pub priority: i32,
    pub lane: Lane,
    pub resources: Vec<ResourceClaim>,
    pub capabilities: Vec<String>,
    pub budget: Budget,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Blocker {
    pub kind: String,
    pub message: String,
    pub wake_condition: String,
    pub retry_at: Option<i64>,
    pub requires_user: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AttemptState {
    #[default]
    Assigned,
    Running,
    Blocked,
    Uncertain,
    Submitted,
    Stopped,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Attempt {
    pub id: String,
    pub task_id: String,
    pub session_id: String,
    pub adapter_id: Option<String>,
    pub epoch: u64,
    pub revision: u64,
    pub state: AttemptState,
    pub started_at: i64,
    pub updated_at: i64,
    pub lease_until: i64,
    pub stopped: bool,
    pub stop_proof: Option<StopProof>,
    pub checkpoint: Option<String>,
    pub progress: Option<String>,
    pub tokens: Option<u64>,
    pub cost_usd: Option<f64>,
    pub liveness_observation: Option<String>,
    pub liveness_at: Option<i64>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StopKind {
    #[default]
    ProcessExited,
    Isolated,
    NeverDispatched,
}

/// Only Host can attest this. "Idle" is not a stop proof.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StopProof {
    pub kind: StopKind,
    pub reference: String,
    pub observed_at: i64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DeliveryState {
    #[default]
    Pending,
    Dispatching,
    Acknowledged,
    Uncertain,
    Cancelled,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Delivery {
    pub id: String,
    pub task_id: String,
    pub attempt_id: String,
    pub epoch: u64,
    pub revision: u64,
    pub session_id: String,
    pub state: DeliveryState,
    pub consumer_id: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub message: String,
    pub note: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ResourceLease {
    pub key: String,
    pub mode: ClaimMode,
    pub task_id: String,
    pub attempt_id: String,
    pub epoch: u64,
    pub session_id: String,
    pub acquired_at: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Artifact {
    pub id: String,
    pub kind: String,
    pub path: String,
    pub sha256: String,
    pub bytes: u64,
    pub source_digest: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TaskResult {
    pub summary: String,
    pub digest: String,
    pub artifacts: Vec<Artifact>,
    pub unresolved: Vec<String>,
    pub checkpoint: Option<String>,
}

/// A trusted execution receipt, submitted by Host after running/observing the verifier.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Verification {
    pub id: String,
    pub criterion_id: String,
    pub attempt_id: String,
    pub epoch: u64,
    pub revision: u64,
    pub digest: String,
    pub passed: bool,
    pub exit_code: Option<i32>,
    pub command: Option<String>,
    pub verifier: String,
    pub output_digest: String,
    pub artifact_id: Option<String>,
    pub finished_at: i64,
    pub output_path: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TaskView {
    pub id: String,
    pub intent_id: String,
    pub title: String,
    pub project: String,
    pub workspace: Option<String>,
    pub integration_owner: Option<String>,
    pub state: TaskState,
    pub revision: u64,
    pub epoch: u64,
    pub session_id: Option<String>,
    pub attempt_id: Option<String>,
    pub parent_id: Option<String>,
    pub dependencies: Vec<String>,
    pub blocked_by: Vec<String>,
    pub blocker: Option<Blocker>,
    pub next_action: String,
    pub priority: i32,
    pub lane: Lane,
    pub progress: Option<String>,
    pub evidence: Vec<Verification>,
    pub artifacts: Vec<Artifact>,
    pub last_error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub contract: TaskContract,
    pub resources: Vec<ResourceClaim>,
    pub capabilities: Vec<String>,
    pub budget: Budget,
    pub result: Option<TaskResult>,
    pub attempts: u32,
    pub cancel_reason: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Capability {
    Supported,
    Unsupported,
    #[default]
    Unknown,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Adapter {
    pub id: String,
    pub harness: String,
    pub login: Option<String>,
    pub capabilities: BTreeMap<String, Capability>,
    pub available: bool,
    pub headroom: Option<f64>,
    pub observed_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Policy {
    pub global_limit: usize,
    pub project_limit: usize,
    pub completion_reserve: usize,
    pub aging_seconds: u64,
    pub default_lease_seconds: u64,
    /// Admission bounds fail explicitly. Finished intents can be archived.
    pub max_tasks: usize,
    pub max_intents: usize,
}
impl Default for Policy {
    fn default() -> Self {
        Self {
            global_limit: 8,
            project_limit: 3,
            completion_reserve: 1,
            aging_seconds: 60,
            default_lease_seconds: 300,
            max_tasks: 256,
            max_intents: 64,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Metrics {
    pub accepted: u64,
    pub attempts: u64,
    pub rework: u64,
    pub recoveries: u64,
    pub cancellations: u64,
    pub blocked: u64,
    pub wait_seconds: u64,
    pub tokens_reported: u64,
    pub cost_reported_usd: f64,
    pub cost_reports: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Event {
    pub seq: u64,
    pub at: i64,
    pub actor: String,
    pub kind: String,
    pub task_id: Option<String>,
    pub attempt_id: Option<String>,
    pub detail: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RecoveryNotice {
    pub at: i64,
    pub kind: String,
    pub detail: String,
    pub preserved_path: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Checkpoint {
    pub task_id: String,
    pub attempt_id: String,
    pub epoch: u64,
    pub revision: u64,
    pub summary: String,
    pub next_steps: Vec<String>,
    pub sources: BTreeMap<String, String>,
    pub at: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Memory {
    pub id: String,
    pub scope: String,
    pub text: String,
    pub sources: BTreeMap<String, String>,
    pub valid: bool,
    pub created_at: i64,
    pub invalidated_at: Option<i64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ContextPacket {
    pub task: TaskView,
    pub intent: Intent,
    pub checkpoint: Option<Checkpoint>,
    pub memory: Vec<Memory>,
    pub dependencies: Vec<TaskView>,
    pub generated_at: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Snapshot {
    pub seq: u64,
    pub intents: Vec<Intent>,
    pub tasks: Vec<TaskView>,
    pub attempts: Vec<Attempt>,
    pub deliveries: Vec<Delivery>,
    pub resources: Vec<ResourceLease>,
    pub adapters: Vec<Adapter>,
    pub metrics: Metrics,
    pub recovery: Vec<RecoveryNotice>,
    pub policy: Policy,
    pub events: Vec<Event>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Outcome {
    pub seq: u64,
    pub duplicate: bool,
    pub task_id: Option<String>,
    pub attempt_id: Option<String>,
    pub outbox_id: Option<String>,
    pub value: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "op",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Command {
    CreateWork {
        intent: NewIntent,
        tasks: Vec<NewTask>,
    },
    CreateIntent {
        intent: NewIntent,
    },
    CreateTasks {
        intent_id: String,
        tasks: Vec<NewTask>,
    },
    Claim {
        task_id: String,
        session_id: String,
        adapter_id: Option<String>,
        lease_seconds: u64,
    },
    BeginDelivery {
        outbox_id: String,
        consumer_id: String,
    },
    AckDelivery {
        outbox_id: String,
    },
    DeliveryUncertain {
        outbox_id: String,
        reason: String,
    },
    Progress {
        attempt_id: String,
        epoch: u64,
        message: String,
        checkpoint: Option<String>,
    },
    RenewAttempt {
        attempt_id: String,
        epoch: u64,
        session_id: String,
        lease_seconds: u64,
        observation: String,
    },
    Block {
        attempt_id: String,
        epoch: u64,
        blocker: Blocker,
    },
    Wake {
        task_id: String,
        condition: String,
    },
    ObserveStopped {
        attempt_id: String,
        epoch: u64,
        proof: StopProof,
    },
    SubmitResult {
        attempt_id: String,
        epoch: u64,
        result: TaskResult,
    },
    Verify {
        task_id: String,
        revision: u64,
        verification: Verification,
    },
    Accept {
        task_id: String,
        revision: u64,
        digest: String,
    },
    Steer {
        task_id: String,
        expected_revision: u64,
        contract: TaskContract,
    },
    /// Cancels this task and unfinished descendants linked by parent_id. Verified outputs survive.
    /// Dependency-only neighbours remain blocked; live actors retain claims until trusted stop.
    Cancel {
        task_id: String,
        reason: String,
    },
    Retry {
        task_id: String,
    },
    Tick,
    RegisterAdapter {
        adapter: Adapter,
    },
    RemoveAdapter {
        adapter_id: String,
    },
    SetPolicy {
        policy: Policy,
    },
    RecordCost {
        attempt_id: String,
        tokens: Option<u64>,
        cost_usd: Option<f64>,
    },
    SaveCheckpoint {
        checkpoint: Checkpoint,
    },
    PutMemory {
        memory: Memory,
    },
    InvalidateSource {
        source: String,
        digest: String,
    },
    ArchiveIntent {
        intent_id: String,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(super) struct Receipt {
    pub fingerprint: String,
    pub outcome: Outcome,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(super) struct State {
    pub seq: u64,
    pub retained_bytes: u64,
    pub intents: BTreeMap<String, Intent>,
    pub tasks: BTreeMap<String, TaskView>,
    pub attempts: BTreeMap<String, Attempt>,
    pub deliveries: BTreeMap<String, Delivery>,
    pub resources: BTreeMap<String, ResourceLease>,
    pub adapters: BTreeMap<String, Adapter>,
    pub checkpoints: BTreeMap<String, Checkpoint>,
    pub memory: BTreeMap<String, Memory>,
    pub receipts: BTreeMap<String, Receipt>,
    pub metrics: Metrics,
    pub policy: Policy,
    pub events: Vec<Event>,
    pub recovery: Vec<RecoveryNotice>,
}
