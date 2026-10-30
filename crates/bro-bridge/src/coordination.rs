//! Runtime boundary for durable tasks. Blocking persistence and content checks run off the UI thread.

use crate::state::AppState;
use bro_core::coordination::{
    self as core, Actor, ActorRole, Command, ContextPacket, Engine, Outcome, Snapshot,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[cfg(test)]
#[path = "coordination_tests.rs"]
mod tests;

fn required(args: &Value, field: &str) -> Result<String, String> {
    args.get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| format!("{field} is required."))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RequestOrigin {
    #[serde(rename = "originRequestId", alias = "requestId")]
    request_id: String,
    reply_channel: String,
    reply_handle: Option<String>,
}

fn origin(args: &Value) -> Result<Option<String>, String> {
    let Some(value) = args.get("origin") else {
        return Ok(None);
    };
    let origin: RequestOrigin = serde_json::from_value(value.clone())
        .map_err(|e| format!("Invalid request origin: {e}"))?;
    if origin.request_id.is_empty()
        || origin.request_id.len() > 256
        || origin.reply_handle.as_ref().is_some_and(|s| s.len() > 256)
        || !matches!(
            origin.reply_channel.as_str(),
            "panel" | "voice" | "native" | "watchdog" | "none"
        )
    {
        return Err("Origin requires a bounded originRequestId and explicit replyChannel.".into());
    }
    serde_json::to_string(&json!({"broRequestOrigin":origin}))
        .map(Some)
        .map_err(|e| e.to_string())
}

pub(crate) fn forward_origin(args:&Value,task:&str)->Result<String,String>{
    let Some(encoded)=origin(args)?else{return Ok(task.to_owned())};
    let envelope:Value=serde_json::from_str(&encoded).map_err(|e|e.to_string())?;
    Ok(format!("[bro-request-origin {}]\nPreserve this original request origin when creating or delegating durable tasks. A new local request envelope does not replace it.\n{task}",envelope["broRequestOrigin"]))
}

fn coordinator(app: &AppState) -> Result<Actor, String> {
    if crate::orchestrator::tools::worker_scoped() {
        return Err("An assigned worker capability cannot perform coordinator operations.".into());
    }
    let id = crate::orchestrator::tools::caller_session().unwrap_or_else(|| "desktop".into());
    if id != "desktop" && !crate::orchestrator::tools::is_orchestrator(app, &id) {
        return Err("This operation belongs to a coordinator. Assigned workers can acknowledge, report progress, save checkpoints and submit their own result.".into());
    }
    Ok(Actor {
        id,
        role: ActorRole::Coordinator,
    })
}

fn check_scope(app: &AppState, actor: &Actor, task: &core::TaskView) -> Result<(), String> {
    if let Some(session) = app.summary(&actor.id) {
        let cwd = Path::new(&session.cwd);
        if !bro_core::orchestrator::is_global(cwd)
            && !bro_core::util::same_path(cwd, Path::new(&task.project))
        {
            return Err("This task belongs to another project. Route it through that project's coordinator.".into());
        }
    }
    Ok(())
}

fn task_sync(app: &AppState, task_id: &str) -> Result<core::TaskView, String> {
    Engine::open(app.data_root.as_path())
        .and_then(|engine| engine.task(task_id))
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "Unknown task. Refresh coordination_snapshot.".into())
}

fn workspace(task: &core::TaskView) -> Result<PathBuf, String> {
    let path = Path::new(task.workspace.as_deref().unwrap_or(&task.project));
    path.canonicalize()
        .map_err(|e| format!("Task workspace is unavailable: {e}"))
}

fn host_execute(app: &AppState, command_id: &str, command: Command) -> Result<Outcome, String> {
    host_execute_at(app, command_id, now(), command)
}

fn host_execute_at(
    app: &AppState,
    command_id: &str,
    at: i64,
    command: Command,
) -> Result<Outcome, String> {
    let outcome = Engine::open(app.data_root.as_path())
        .and_then(|engine| engine.execute(command_id, &host(), at, command))
        .map_err(|e| e.to_string())?;
    publish(app, &outcome);
    Ok(outcome)
}

fn adapter(session: &crate::model::TerminalSessionSummary) -> core::Adapter {
    let mut capabilities = std::collections::BTreeMap::new();
    capabilities.insert("terminalInput".into(), core::Capability::Supported);
    capabilities.insert("structuredEvents".into(), core::Capability::Unknown);
    capabilities.insert("midTurnSteering".into(), core::Capability::Unknown);
    capabilities.insert("browser".into(), core::Capability::Unknown);
    capabilities.insert("filesystemIsolation".into(), core::Capability::Unsupported);
    capabilities.insert(
        "nativeResume".into(),
        if matches!(session.agent.as_deref(), Some("claude" | "codex")) {
            core::Capability::Supported
        } else {
            core::Capability::Unknown
        },
    );
    core::Adapter {
        id: format!("session-{}", session.id),
        harness: session.agent.clone().unwrap_or_else(|| "shell".into()),
        capabilities,
        available: session.status == "running",
        observed_at: now(),
        ..Default::default()
    }
}

fn worker_token(app: &AppState, attempt: &core::Attempt) -> String {
    // Bridge token is durable and outside public task state. Plain capabilities appear only in assignment packets.
    core::digest_bytes(
        format!(
            "bro-task-v1\0{}\0{}\0{}\0{}\0{}\0{}",
            app.token,
            attempt.id,
            attempt.session_id,
            attempt.epoch,
            attempt.revision,
            attempt.started_at
        )
        .as_bytes(),
    )
}

fn worker(
    app: &AppState,
    snapshot: &Snapshot,
    args: &Value,
    caller: Option<&str>,
) -> Result<(Actor, core::Attempt), String> {
    let attempt_id = required(args, "attemptId")?;
    let attempt = snapshot
        .attempts
        .iter()
        .find(|a| a.id == attempt_id)
        .cloned()
        .ok_or("Unknown attempt.")?;
    let epoch = args
        .get("epoch")
        .and_then(Value::as_u64)
        .ok_or("epoch is required.")?;
    let token = required(args, "workerToken")?;
    let expected = worker_token(app, &attempt);
    let same = token.len() == expected.len()
        && token
            .bytes()
            .zip(expected.bytes())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0;
    if !same || epoch != attempt.epoch || caller.is_some_and(|id| id != attempt.session_id) {
        return Err("The worker capability, session or attempt epoch does not match. Read the current assignment; do not retry an older result.".into());
    }
    Ok((
        Actor {
            id: attempt.session_id.clone(),
            role: ActorRole::Worker,
        },
        attempt,
    ))
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}
fn host() -> Actor {
    Actor {
        id: "bridge-runtime".into(),
        role: ActorRole::Host,
    }
}

pub(crate) fn snapshot_sync(app: &AppState) -> Result<Snapshot, String> {
    Engine::open(app.data_root.as_path())
        .and_then(|engine| engine.snapshot())
        .map_err(|e| e.to_string())
}

pub(crate) fn context_sync(app: &AppState, task: &str) -> Result<ContextPacket, String> {
    let engine = Engine::open(app.data_root.as_path()).map_err(|e| e.to_string())?;
    let mut packet = engine.context(task, now()).map_err(|e| e.to_string())?;
    let mut fresh = Vec::new();
    let mut invalidated = false;
    for memory in packet.memory {
        let mut valid = true;
        for (source, expected) in &memory.sources {
            match crate::coordination_evidence::hash_file(Path::new(source)) {
                Ok((digest, _)) if &digest == expected => {}
                Ok((digest, _)) => {
                    valid = false;
                    invalidated = true;
                    host_execute(
                        app,
                        &format!(
                            "memory-source-{}-{digest}",
                            core::digest_bytes(source.as_bytes())
                        ),
                        Command::InvalidateSource {
                            source: source.clone(),
                            digest,
                        },
                    )?;
                }
                Err(_) => valid = false,
            }
        }
        if valid {
            fresh.push(memory);
        }
    }
    if invalidated {
        packet = engine.context(task, now()).map_err(|e| e.to_string())?;
        packet
            .memory
            .retain(|memory| fresh.iter().any(|saved| saved.id == memory.id));
    } else {
        packet.memory = fresh;
    }
    Ok(packet)
}

/// In-process coordinator actions use the same file-locked authority as HTTP/MCP.
/// Trusted evidence is produced only by this module's observed verification operations.
pub(crate) fn execute_sync(
    app: &AppState,
    command_id: &str,
    actor: &Actor,
    mut command: Command,
) -> Result<Outcome, String> {
    if actor.role == ActorRole::Host
        || matches!(
            command,
            Command::Verify { .. }
                | Command::ObserveStopped { .. }
                | Command::RegisterAdapter { .. }
                | Command::BeginDelivery { .. }
                | Command::AckDelivery { .. }
                | Command::DeliveryUncertain { .. }
                | Command::PutMemory { .. }
                | Command::Accept { .. }
                | Command::Claim { .. }
                | Command::RenewAttempt { .. }
                | Command::RemoveAdapter { .. }
                | Command::InvalidateSource { .. }
        )
    {
        return Err("Use the bridge's dispatch, verification or retirement operation for runtime evidence. A caller cannot assert Host authority.".into());
    }
    if actor.role != ActorRole::Coordinator {
        return Err(
            "Worker mutations require the assignment capability through the worker tools.".into(),
        );
    }
    let engine = Engine::open(app.data_root.as_path()).map_err(|e| e.to_string())?;
    let before = engine.snapshot().map_err(|e| e.to_string())?;
    validate_graph_scope(app, actor, &before, &command)?;
    for task in &before.tasks {
        let target = serde_json::to_value(&command)
            .ok()
            .and_then(|v| v.get("taskId").and_then(Value::as_str).map(str::to_owned));
        if target.as_deref() == Some(task.id.as_str()) {
            check_scope(app, actor, task)?;
        }
    }
    match &mut command {
        Command::CreateTasks { tasks, .. } | Command::CreateWork { tasks, .. } => {
            for task in tasks {
                normalize_task(app, task)?;
                let scope = core::TaskView {
                    project: task.project.clone(),
                    ..Default::default()
                };
                check_scope(app, actor, &scope)?;
            }
        }
        Command::SubmitResult { .. }
        | Command::Progress { .. }
        | Command::Block { .. }
        | Command::SaveCheckpoint { .. }
        | Command::RecordCost { .. } => {
            return Err("Use the assigned worker's authenticated task operation.".into());
        }
        Command::Steer {
            task_id, contract, ..
        } => {
            let task = before
                .tasks
                .iter()
                .find(|task| task.id == *task_id)
                .ok_or("Unknown task.")?;
            normalize_sources(&mut contract.sources, &workspace(task)?)?;
        }
        _ => {}
    }
    let outcome = engine
        .execute(command_id, actor, now(), command)
        .map_err(|e| e.to_string())?;
    publish(app, &outcome);
    wake_changed(app, &before, &engine.snapshot().map_err(|e| e.to_string())?);
    Ok(outcome)
}

fn validate_graph_scope(
    app: &AppState,
    actor: &Actor,
    snapshot: &Snapshot,
    command: &Command,
) -> Result<(), String> {
    let value = serde_json::to_value(command).map_err(|e| e.to_string())?;
    if let Some(intent) = value.get("intentId").and_then(Value::as_str) {
        for task in snapshot
            .tasks
            .iter()
            .filter(|task| task.intent_id == intent)
        {
            check_scope(app, actor, task)?;
        }
    }
    if let Some(tasks) = value.get("tasks").and_then(Value::as_array) {
        for task in tasks {
            for id in task
                .get("dependencies")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .chain(task.get("parentId").and_then(Value::as_str))
            {
                if let Some(referenced) = snapshot.tasks.iter().find(|t| t.id == id) {
                    check_scope(app, actor, referenced)?;
                }
            }
        }
    }
    if matches!(command, Command::Cancel { .. } | Command::Steer { .. }) {
        if let Some(target) = value.get("taskId").and_then(Value::as_str) {
            let mut affected = std::collections::BTreeSet::from([target.to_owned()]);
            loop {
                let previous = affected.len();
                for task in &snapshot.tasks {
                    if task
                        .parent_id
                        .as_ref()
                        .is_some_and(|id| affected.contains(id))
                        || task.dependencies.iter().any(|id| affected.contains(id))
                    {
                        affected.insert(task.id.clone());
                    }
                    if affected.contains(&task.id) {
                        if let Some(parent) = &task.parent_id {
                            affected.insert(parent.clone());
                        }
                    }
                }
                if affected.len() == previous {
                    break;
                }
            }
            for task in snapshot
                .tasks
                .iter()
                .filter(|task| affected.contains(&task.id))
            {
                check_scope(app, actor, task)?;
            }
        }
    }
    Ok(())
}

fn normalize_task(app: &AppState, task: &mut core::NewTask) -> Result<(), String> {
    let project = Path::new(&task.project)
        .canonicalize()
        .map_err(|e| format!("Task project must be an existing directory: {e}"))?;
    let cwd = Path::new(task.workspace.as_deref().unwrap_or(&task.project))
        .canonicalize()
        .map_err(|e| format!("Task workspace is unavailable: {e}"))?;
    task.project = project.to_string_lossy().into_owned();
    task.workspace = Some(cwd.to_string_lossy().into_owned());
    normalize_sources(&mut task.contract.sources, &cwd)?;
    let managed = app.data_root.join("coordination").join("workspaces");
    if cwd.starts_with(&managed) {
        let record = crate::coordination_worktrees::load(app.data_root.as_path(), &task.id)?;
        crate::coordination_worktrees::inspect(&record)?;
        if !bro_core::util::same_path(&cwd, &record.path)
            || task.integration_owner.as_deref() != Some(&record.integration_owner)
        {
            return Err(
                "Managed worktree ownership differs from this task or integration owner.".into(),
            );
        }
    }
    if task
        .integration_owner
        .as_ref()
        .is_none_or(|owner| owner.trim().is_empty())
    {
        return Err(
            "Task requires an explicit integrationOwner responsible for the final artifact.".into(),
        );
    }
    for claim in &mut task.resources {
        for prefix in ["workspace:", "repo:", "integration:"] {
            if let Some(value) = claim.key.strip_prefix(prefix) {
                let path = Path::new(value)
                    .canonicalize()
                    .map_err(|e| format!("Resource path must exist: {e}"))?;
                claim.key = format!("{prefix}{}", bro_core::util::path_key(&path));
                break;
            }
        }
    }
    if !task
        .resources
        .iter()
        .any(|claim| claim.key.starts_with("workspace:"))
    {
        task.resources.push(core::ResourceClaim {
            key: format!("workspace:{}", bro_core::util::path_key(&cwd)),
            mode: core::ClaimMode::Exclusive,
        });
    }
    for dir in cwd.ancestors() {
        let path = dir.join("AGENTS.md");
        if path.is_file() {
            let (digest, _) = crate::coordination_evidence::hash_file(&path)?;
            task.contract
                .sources
                .entry(path.to_string_lossy().into_owned())
                .or_insert(digest);
        }
    }
    Ok(())
}

fn normalize_sources(
    sources: &mut std::collections::BTreeMap<String, String>,
    cwd: &Path,
) -> Result<(), String> {
    let old = std::mem::take(sources);
    for (source, digest) in old {
        let path = if Path::new(&source).is_absolute() {
            PathBuf::from(&source)
        } else {
            cwd.join(&source)
        };
        let path = path.canonicalize().map_err(|e| {
            format!("Contract source must be an accessible local file: {source}: {e}")
        })?;
        let key = path.to_string_lossy().into_owned();
        if sources
            .insert(key, digest.clone())
            .is_some_and(|previous| previous != digest)
        {
            return Err("Contract source aliases name different content identities.".into());
        }
    }
    Ok(())
}

fn wake_changed(app: &AppState, before: &Snapshot, after: &Snapshot) {
    for task in &after.tasks {
        let changed = before
            .tasks
            .iter()
            .find(|t| t.id == task.id)
            .is_none_or(|old| old.revision != task.revision || old.state != task.state);
        let resume = task.state == core::TaskState::Running
            && task.blocker.is_none()
            && before
                .tasks
                .iter()
                .find(|old| old.id == task.id)
                .is_some_and(|old| old.blocker.is_some());
        if changed
            && (resume
                || matches!(
                    task.state,
                    core::TaskState::Ready
                        | core::TaskState::Submitted
                        | core::TaskState::Verified
                        | core::TaskState::Blocked
                        | core::TaskState::Uncertain
                        | core::TaskState::CancelRequested
                        | core::TaskState::Failed
                ))
        {
            let seq = after
                .events
                .iter()
                .rev()
                .find(|e| {
                    e.task_id.as_deref() == Some(&task.id)
                        && !matches!(
                            e.kind.as_str(),
                            "progress"
                                | "livenessObserved"
                                | "usageReported"
                                | "checkpointSaved"
                                | "verificationRecorded"
                        )
                })
                .map(|e| e.seq)
                .or_else(|| {
                    after
                        .events
                        .iter()
                        .find(|e| {
                            matches!(e.kind.as_str(), "workCreated" | "tasksCreated")
                                && e.at >= task.created_at
                        })
                        .map(|e| e.seq)
                })
                .unwrap_or(0);
            wake(
                app,
                task,
                seq,
                if resume {
                    "resumeReady"
                } else {
                    task.state.label()
                },
            );
        }
    }
}

fn wake(app: &AppState, task: &core::TaskView, seq: u64, action: &str) {
    let key = (task.revision, seq, action.to_owned());
    {
        let mut emitted = app.coordination_wakes.lock();
        if emitted.get(&task.id) == Some(&key) {
            return;
        }
        emitted.insert(task.id.clone(), key);
    }
    app.send_command(crate::BridgeCommand::CoordinationWake {
        task_id: task.id.clone(),
        revision: task.revision,
        seq,
        project: PathBuf::from(&task.project),
        action: action.into(),
    });
}

fn publish(app: &AppState, outcome: &Outcome) {
    app.publish(crate::model::ServerEvent::global(json!({
        "type": "coordination", "seq": outcome.seq, "taskId": outcome.task_id,
        "attemptId": outcome.attempt_id, "duplicate": outcome.duplicate,
    })));
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| format!("Coordination operation interrupted: {e}"))?
}

mod dispatch;
mod lifecycle;
mod supervision;
mod tools;
mod verification;

use dispatch::{dispatch, submit, worker_operation};
use lifecycle::{owned_worker_path, retire};
pub(crate) use lifecycle::{prepare_assignment, record_owned_worker};
use supervision::deliver_accepted;
#[cfg(test)]
use supervision::reconcile_once;
pub(crate) use supervision::spawn_supervisor;
pub(crate) use tools::{definitions, execute_tool, http_command, http_snapshot, http_tool};
use verification::{accept, inspect, verify};
