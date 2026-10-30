use super::{evidence, model::*, scheduler};
use anyhow::{Context, Result, bail, ensure};
use serde_json::json;
use std::collections::BTreeSet;

pub(super) fn identifier(id: &str) -> Result<()> {
    ensure!(
        !id.is_empty()
            && id.len() <= 200
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.:".contains(&b)),
        "invalid coordination identifier"
    );
    Ok(())
}
fn host(actor: &Actor) -> Result<()> {
    ensure!(
        actor.role == ActorRole::Host,
        "trusted host authority is required"
    );
    Ok(())
}
fn operator(actor: &Actor) -> Result<()> {
    ensure!(
        actor.role != ActorRole::Worker,
        "coordinator authority is required"
    );
    Ok(())
}
fn text(text: &str, max: usize) -> Result<()> {
    ensure!(
        !text.trim().is_empty() && text.len() <= max,
        "text must be nonempty and within its size limit"
    );
    Ok(())
}
fn id(prefix: &str) -> String {
    format!("{prefix}-{:032x}", rand::random::<u128>())
}
fn task<'a>(state: &'a State, id: &str) -> Result<&'a TaskView> {
    state.tasks.get(id).context("unknown task")
}
fn budget(b: &Budget, max: u32) -> Result<()> {
    ensure!(
        b.max_attempts > 0 && b.max_attempts <= max,
        "attempt budget must be between 1 and {max}"
    );
    ensure!(
        b.max_cost_usd.is_none_or(|v| v.is_finite() && v > 0.0)
            && b.max_tokens.is_none_or(|v| v > 0),
        "invalid cost or token budget"
    );
    Ok(())
}

fn fenced_attempt(
    state: &State,
    actor: &Actor,
    attempt_id: &str,
    epoch: u64,
    now: i64,
) -> Result<Attempt> {
    let a = state.attempts.get(attempt_id).context("unknown attempt")?;
    let t = task(state, &a.task_id)?;
    ensure!(
        actor.role == ActorRole::Host || actor.id == a.session_id,
        "attempt belongs to another worker"
    );
    ensure!(
        a.epoch == epoch
            && t.epoch == epoch
            && t.revision == a.revision
            && t.attempt_id.as_deref() == Some(attempt_id),
        "stale attempt or contract revision"
    );
    ensure!(
        !a.stopped && a.lease_until >= now,
        "attempt is stopped or its lease expired; reconcile before continuing"
    );
    ensure!(
        !matches!(
            t.state,
            TaskState::CancelRequested
                | TaskState::Cancelled
                | TaskState::Verified
                | TaskState::Uncertain
        ),
        "task no longer accepts this attempt's mutations"
    );
    Ok(a.clone())
}

fn release(state: &mut State, attempt_id: &str) {
    state
        .resources
        .retain(|_, resource| resource.attempt_id != attempt_id);
}

/// Invalidate only this task and transitive dependants/parents. Running mutation ownership survives fencing.
fn invalidate(state: &mut State, seeds: BTreeSet<String>, reason: &str, now: i64) {
    let mut affected = seeds;
    loop {
        let more: Vec<_> = state
            .tasks
            .values()
            .filter(|t| {
                !affected.contains(&t.id)
                    && scheduler::dependency_ids(state, t)
                        .iter()
                        .any(|d| affected.contains(d))
            })
            .map(|t| t.id.clone())
            .collect();
        if more.is_empty() {
            break;
        }
        affected.extend(more);
    }
    for memory in state.memory.values_mut() {
        let scoped_to_revision = affected.contains(&memory.scope);
        let source_changed = affected
            .iter()
            .filter_map(|id| state.tasks.get(id))
            .any(|t| {
                memory.sources.iter().any(|(source, digest)| {
                    t.contract
                        .sources
                        .get(source)
                        .is_some_and(|current| current != digest)
                })
            });
        if scoped_to_revision || source_changed {
            memory.valid = false;
            memory.invalidated_at = Some(now);
        }
    }
    for id in affected {
        let alive = state
            .tasks
            .get(&id)
            .and_then(|t| t.attempt_id.as_ref())
            .and_then(|a| state.attempts.get(a))
            .is_some_and(|a| !a.stopped);
        if !alive {
            if let Some(attempt_id) = state.tasks.get(&id).and_then(|t| t.attempt_id.clone()) {
                release(state, &attempt_id);
            }
        }
        if let Some(t) = state.tasks.get_mut(&id) {
            t.revision += 1;
            t.epoch += 1;
            t.updated_at = now;
            t.result = None;
            t.evidence.clear();
            t.artifacts.clear();
            t.last_error = Some(reason.into());
            t.blocker = None;
            t.cancel_reason = None;
            t.state = if alive {
                TaskState::CancelRequested
            } else {
                TaskState::Pending
            };
        }
        state.checkpoints.remove(&id);
    }
}

pub(super) fn command_name(command: &Command) -> &'static str {
    match command {
        Command::CreateWork { .. } => "workCreated",
        Command::CreateIntent { .. } => "intentCreated",
        Command::CreateTasks { .. } => "tasksCreated",
        Command::Claim { .. } => "attemptAssigned",
        Command::BeginDelivery { .. } => "dispatchStarted",
        Command::AckDelivery { .. } => "deliveryAcknowledged",
        Command::DeliveryUncertain { .. } => "deliveryUncertain",
        Command::Progress { .. } => "progress",
        Command::Block { .. } => "blocked",
        Command::Wake { .. } => "wake",
        Command::RenewAttempt { .. } => "livenessObserved",
        Command::ObserveStopped { .. } => "actorStopped",
        Command::SubmitResult { .. } => "resultSubmitted",
        Command::Verify { .. } => "verificationRecorded",
        Command::Accept { .. } => "taskAccepted",
        Command::Steer { .. } => "contractRevised",
        Command::Cancel { .. } => "cancellationRequested",
        Command::Retry { .. } => "retryRequested",
        Command::Tick => "supervisorTick",
        Command::RegisterAdapter { .. } => "adapterObserved",
        Command::RemoveAdapter { .. } => "adapterRemoved",
        Command::SetPolicy { .. } => "policyChanged",
        Command::RecordCost { .. } => "usageReported",
        Command::SaveCheckpoint { .. } => "checkpointSaved",
        Command::PutMemory { .. } => "memorySaved",
        Command::InvalidateSource { .. } => "sourceInvalidated",
        Command::ArchiveIntent { .. } => "intentArchived",
    }
}

pub(super) fn apply(
    state: &mut State,
    actor: &Actor,
    now: i64,
    command: &Command,
) -> Result<Outcome> {
    ensure!(
        serde_json::to_vec(command)?.len() <= 256 * 1024,
        "coordination command exceeds size limit"
    );
    let mut outcome = Outcome::default();
    match command {
        Command::CreateWork { intent, tasks } => {
            apply(
                state,
                actor,
                now,
                &Command::CreateIntent {
                    intent: intent.clone(),
                },
            )?;
            outcome = apply(
                state,
                actor,
                now,
                &Command::CreateTasks {
                    intent_id: intent.id.clone(),
                    tasks: tasks.clone(),
                },
            )?;
            outcome.value["intentId"] = json!(intent.id);
        }
        Command::CreateIntent { intent } => {
            operator(actor)?;
            identifier(&intent.id)?;
            text(&intent.title, 512)?;
            budget(&intent.budget, 4096)?;
            ensure!(
                !state.intents.contains_key(&intent.id),
                "intent id already exists"
            );
            ensure!(
                state.intents.len() < state.policy.max_intents,
                "intent admission limit reached; archive finished work"
            );
            ensure!(
                intent.description.len() <= 32768 && intent.constraints.len() <= 128,
                "intent is too large"
            );
            state.intents.insert(
                intent.id.clone(),
                Intent {
                    id: intent.id.clone(),
                    title: intent.title.clone(),
                    description: intent.description.clone(),
                    constraints: intent.constraints.clone(),
                    authorization: intent.authorization.clone(),
                    budget: intent.budget.clone(),
                    created_at: now,
                    updated_at: now,
                },
            );
            outcome.value = json!({"intentId":intent.id});
        }
        Command::CreateTasks { intent_id, tasks } => {
            operator(actor)?;
            ensure!(state.intents.contains_key(intent_id), "unknown intent");
            ensure!(
                !tasks.is_empty()
                    && tasks.len() <= 128
                    && state.tasks.len() + tasks.len() <= state.policy.max_tasks,
                "task admission limit reached; archive finished work"
            );
            for input in tasks {
                if let Some(parent) = input.parent_id.as_ref().and_then(|id| state.tasks.get(id)) {
                    ensure!(
                        !matches!(
                            parent.state,
                            TaskState::CancelRequested | TaskState::Cancelled
                        ),
                        "cannot add new children to a cancelled parent"
                    );
                }
                identifier(&input.id)?;
                text(&input.title, 512)?;
                text(&input.project, 4096)?;
                budget(&input.budget, 64)?;
                evidence::validate_contract(&input.contract)?;
                ensure!(
                    !state.tasks.contains_key(&input.id),
                    "task id already exists"
                );
                ensure!(
                    input.dependencies.len() <= 128
                        && input.resources.len() <= 64
                        && input.capabilities.len() <= 32,
                    "task collections exceed limits"
                );
                let mut keys = BTreeSet::new();
                for claim in &input.resources {
                    text(&claim.key, 4096)?;
                    ensure!(keys.insert(&claim.key), "duplicate resource claim");
                }
                state.tasks.insert(
                    input.id.clone(),
                    TaskView {
                        id: input.id.clone(),
                        intent_id: intent_id.clone(),
                        title: input.title.clone(),
                        project: input.project.clone(),
                        workspace: input.workspace.clone(),
                        integration_owner: input.integration_owner.clone(),
                        parent_id: input.parent_id.clone(),
                        dependencies: input.dependencies.clone(),
                        contract: input.contract.clone(),
                        priority: input.priority,
                        lane: input.lane,
                        resources: input.resources.clone(),
                        capabilities: input.capabilities.clone(),
                        budget: input.budget.clone(),
                        revision: 1,
                        created_at: now,
                        updated_at: now,
                        ..Default::default()
                    },
                );
            }
            scheduler::validate_graph(state)?;
            // Adding a new required child changes its parent's acceptance dependency set.
            let parents: BTreeSet<_> = tasks
                .iter()
                .filter_map(|t| t.parent_id.clone())
                .filter(|id| {
                    state
                        .tasks
                        .get(id)
                        .is_some_and(|t| t.state != TaskState::Pending)
                })
                .collect();
            if !parents.is_empty() {
                invalidate(state, parents, "Required child tasks were added", now);
            }
            state.intents.get_mut(intent_id).unwrap().updated_at = now;
            outcome.value = json!({"taskIds":tasks.iter().map(|t|&t.id).collect::<Vec<_>>()});
        }
        Command::Claim {
            task_id,
            session_id,
            adapter_id,
            lease_seconds,
        } => {
            operator(actor)?;
            identifier(session_id)?;
            let t = task(state, task_id)?.clone();
            scheduler::validate_claim(state, &t, session_id, adapter_id.as_deref(), now)?;
            if t.lane == Lane::Completion
                && let Some(owner) = &t.integration_owner
            {
                ensure!(
                    owner == session_id || owner == &actor.id,
                    "completion task has a different integration owner"
                );
            }
            let duration = if *lease_seconds == 0 {
                state.policy.default_lease_seconds
            } else {
                *lease_seconds
            };
            ensure!(
                (10..=86400).contains(&duration),
                "lease must be between 10 and 86400 seconds"
            );
            let attempt_id = id("attempt");
            let outbox_id = id("delivery");
            let epoch = t.epoch + 1;
            let a = Attempt {
                id: attempt_id.clone(),
                task_id: task_id.clone(),
                session_id: session_id.clone(),
                adapter_id: adapter_id.clone(),
                epoch,
                revision: t.revision,
                started_at: now,
                updated_at: now,
                lease_until: now.saturating_add(duration as i64),
                ..Default::default()
            };
            let packet = super::context::packet(state, task_id, now)?;
            let message = serde_json::to_string(
                &json!({"coordination":{"taskId":task_id,"attemptId":attempt_id,"epoch":epoch,"revision":t.revision,"outboxId":outbox_id},"context":packet}),
            )?;
            ensure!(
                message.len() <= 256 * 1024,
                "restart packet exceeds dispatch limit; reduce task context before assigning"
            );
            let delivery = Delivery {
                id: outbox_id.clone(),
                task_id: task_id.clone(),
                attempt_id: attempt_id.clone(),
                epoch,
                revision: t.revision,
                session_id: session_id.clone(),
                created_at: now,
                updated_at: now,
                message,
                ..Default::default()
            };
            for claim in &t.resources {
                state.resources.insert(
                    format!("{}:{}", attempt_id, claim.key),
                    ResourceLease {
                        key: claim.key.clone(),
                        mode: claim.mode,
                        task_id: task_id.clone(),
                        attempt_id: attempt_id.clone(),
                        epoch,
                        session_id: session_id.clone(),
                        acquired_at: now,
                    },
                );
            }
            state.metrics.attempts += 1;
            state.metrics.wait_seconds = state
                .metrics
                .wait_seconds
                .saturating_add(now.saturating_sub(t.updated_at).max(0) as u64);
            state.attempts.insert(attempt_id.clone(), a);
            state.deliveries.insert(outbox_id.clone(), delivery);
            let t = state.tasks.get_mut(task_id).unwrap();
            t.attempts += 1;
            t.epoch = epoch;
            t.state = TaskState::Assigned;
            t.attempt_id = Some(attempt_id.clone());
            t.session_id = Some(session_id.clone());
            t.updated_at = now;
            t.result = None;
            t.evidence.clear();
            t.artifacts.clear();
            t.last_error = None;
            t.blocker = None;
            t.cancel_reason = None;
            outcome.task_id = Some(task_id.clone());
            outcome.attempt_id = Some(attempt_id);
            outcome.outbox_id = Some(outbox_id);
        }
        Command::BeginDelivery {
            outbox_id,
            consumer_id,
        } => {
            operator(actor)?;
            identifier(consumer_id)?;
            let d = state
                .deliveries
                .get(outbox_id)
                .context("unknown delivery")?
                .clone();
            let t = task(state, &d.task_id)?;
            ensure!(
                d.state == DeliveryState::Pending
                    && t.state == TaskState::Assigned
                    && d.epoch == t.epoch
                    && d.revision == t.revision,
                "delivery was already claimed or is stale; reconcile instead of resending"
            );
            ensure!(
                state
                    .attempts
                    .get(&d.attempt_id)
                    .is_some_and(|a| !a.stopped && a.lease_until >= now),
                "assignment lease expired"
            );
            let d = state.deliveries.get_mut(outbox_id).unwrap();
            d.state = DeliveryState::Dispatching;
            d.consumer_id = Some(consumer_id.clone());
            d.updated_at = now;
            outcome.task_id = Some(d.task_id.clone());
            outcome.attempt_id = Some(d.attempt_id.clone());
            outcome.outbox_id = Some(outbox_id.clone());
            outcome.value = json!({"delivery":d});
        }
        Command::AckDelivery { outbox_id } => {
            host(actor)?;
            let d = state
                .deliveries
                .get(outbox_id)
                .context("unknown delivery")?
                .clone();
            let t = task(state, &d.task_id)?;
            ensure!(
                t.epoch == d.epoch
                    && t.revision == d.revision
                    && t.attempt_id.as_deref() == Some(&d.attempt_id),
                "delivery belongs to a stale attempt"
            );
            ensure!(
                matches!(
                    d.state,
                    DeliveryState::Dispatching
                        | DeliveryState::Uncertain
                        | DeliveryState::Acknowledged
                ),
                "delivery has not been dispatched"
            );
            if d.state == DeliveryState::Acknowledged {
                outcome.task_id = Some(d.task_id.clone());
                outcome.attempt_id = Some(d.attempt_id.clone());
                outcome.outbox_id = Some(outbox_id.clone());
                return Ok(outcome);
            }
            let a = state
                .attempts
                .get_mut(&d.attempt_id)
                .context("unknown attempt")?;
            ensure!(
                !a.stopped && a.lease_until >= now,
                "delivery acknowledgement arrived after lease expiry"
            );
            a.state = AttemptState::Running;
            a.updated_at = now;
            let d = state.deliveries.get_mut(outbox_id).unwrap();
            d.state = DeliveryState::Acknowledged;
            d.updated_at = now;
            let t = state.tasks.get_mut(&d.task_id).unwrap();
            t.state = TaskState::Running;
            t.updated_at = now;
            outcome.task_id = Some(d.task_id.clone());
            outcome.attempt_id = Some(d.attempt_id.clone());
            outcome.outbox_id = Some(outbox_id.clone());
        }
        Command::DeliveryUncertain { outbox_id, reason } => {
            host(actor)?;
            text(reason, 4096)?;
            let d = state
                .deliveries
                .get_mut(outbox_id)
                .context("unknown delivery")?;
            ensure!(
                matches!(
                    d.state,
                    DeliveryState::Dispatching | DeliveryState::Uncertain
                ),
                "only an in-flight delivery can be uncertain"
            );
            d.state = DeliveryState::Uncertain;
            d.note = Some(reason.clone());
            d.updated_at = now;
            let t = state.tasks.get_mut(&d.task_id).context("unknown task")?;
            if t.epoch == d.epoch {
                t.state = TaskState::Uncertain;
                t.last_error = Some(reason.clone());
                t.updated_at = now;
            }
            if let Some(a) = state.attempts.get_mut(&d.attempt_id) {
                a.state = AttemptState::Uncertain;
            }
            outcome.task_id = Some(d.task_id.clone());
            outcome.attempt_id = Some(d.attempt_id.clone());
        }
        Command::Progress {
            attempt_id,
            epoch,
            message,
            checkpoint,
        } => {
            text(message, 4096)?;
            let a = fenced_attempt(state, actor, attempt_id, *epoch, now)?;
            ensure!(
                matches!(a.state, AttemptState::Running | AttemptState::Blocked),
                "progress requires acknowledged running work"
            );
            ensure!(
                checkpoint.as_ref().is_none_or(|v| v.len() <= 32768),
                "checkpoint too large"
            );
            let a_mut = state.attempts.get_mut(attempt_id).unwrap();
            a_mut.progress = Some(message.clone());
            a_mut.checkpoint = checkpoint.clone().or(a.checkpoint);
            a_mut.updated_at = now;
            a_mut.lease_until = now.saturating_add(state.policy.default_lease_seconds as i64);
            let t = state.tasks.get_mut(&a.task_id).unwrap();
            t.progress = Some(message.clone());
            t.updated_at = now;
            outcome.task_id = Some(a.task_id);
            outcome.attempt_id = Some(attempt_id.clone());
        }
        Command::RenewAttempt {
            attempt_id,
            epoch,
            session_id,
            lease_seconds,
            observation,
        } => {
            host(actor)?;
            text(observation, 4096)?;
            let a = state
                .attempts
                .get(attempt_id)
                .context("unknown attempt")?
                .clone();
            let t = task(state, &a.task_id)?;
            ensure!(
                !a.stopped
                    && a.epoch == *epoch
                    && t.epoch == *epoch
                    && a.session_id == *session_id
                    && t.attempt_id.as_deref() == Some(attempt_id)
                    && t.revision == a.revision,
                "liveness observation belongs to a stale or stopped actor"
            );
            ensure!(
                !matches!(
                    t.state,
                    TaskState::CancelRequested
                        | TaskState::Cancelled
                        | TaskState::Stopped
                        | TaskState::Failed
                ),
                "a cancelled or revised task cannot be resumed by liveness"
            );
            let duration = if *lease_seconds == 0 {
                state.policy.default_lease_seconds
            } else {
                *lease_seconds
            };
            ensure!(
                (10..=86400).contains(&duration),
                "invalid liveness lease duration"
            );
            let acknowledged = state
                .deliveries
                .values()
                .any(|d| d.attempt_id == *attempt_id && d.state == DeliveryState::Acknowledged);
            let blocked = t.blocker.is_some();
            let a_mut = state.attempts.get_mut(attempt_id).unwrap();
            a_mut.lease_until = now.saturating_add(duration as i64);
            a_mut.liveness_at = Some(now);
            a_mut.liveness_observation = Some(observation.clone());
            if a_mut.state == AttemptState::Uncertain && acknowledged {
                a_mut.state = if blocked {
                    AttemptState::Blocked
                } else {
                    AttemptState::Running
                };
            }
            let t = state.tasks.get_mut(&a.task_id).unwrap();
            if t.state == TaskState::Uncertain && acknowledged {
                t.state = if t.blocker.is_some() {
                    TaskState::Blocked
                } else {
                    TaskState::Running
                };
                t.last_error = None;
            }
            t.updated_at = now;
            outcome.task_id = Some(a.task_id);
            outcome.attempt_id = Some(attempt_id.clone());
        }
        Command::Block {
            attempt_id,
            epoch,
            blocker,
        } => {
            let a = fenced_attempt(state, actor, attempt_id, *epoch, now)?;
            text(&blocker.message, 4096)?;
            text(&blocker.wake_condition, 512)?;
            ensure!(
                matches!(a.state, AttemptState::Running | AttemptState::Blocked),
                "a submitted result cannot regress to blocked"
            );
            let a_mut = state.attempts.get_mut(attempt_id).unwrap();
            a_mut.state = AttemptState::Blocked;
            a_mut.updated_at = now;
            let t = state.tasks.get_mut(&a.task_id).unwrap();
            t.state = TaskState::Blocked;
            t.blocker = Some(blocker.clone());
            t.updated_at = now;
            state.metrics.blocked += 1;
            outcome.task_id = Some(a.task_id);
            outcome.attempt_id = Some(attempt_id.clone());
        }
        Command::Wake { task_id, condition } => {
            operator(actor)?;
            let t = task(state, task_id)?.clone();
            ensure!(
                matches!(
                    t.state,
                    TaskState::Blocked | TaskState::Pending | TaskState::Uncertain
                ),
                "task state does not permit waking"
            );
            let Some(blocker) = t.blocker.as_ref() else {
                outcome.task_id = Some(task_id.clone());
                return Ok(outcome);
            };
            ensure!(
                &blocker.wake_condition == condition,
                "wake condition does not match the blocker"
            );
            let live = t
                .attempt_id
                .as_ref()
                .and_then(|id| state.attempts.get_mut(id));
            let next = match live {
                Some(a) if !a.stopped && a.epoch == t.epoch && a.lease_until >= now => {
                    a.state = AttemptState::Running;
                    TaskState::Running
                }
                Some(a) if !a.stopped => TaskState::Uncertain,
                _ => TaskState::Pending,
            };
            let t = state.tasks.get_mut(task_id).unwrap();
            t.blocker = None;
            t.state = next;
            t.updated_at = now;
            outcome.task_id = Some(task_id.clone());
        }
        Command::ObserveStopped {
            attempt_id,
            epoch,
            proof,
        } => {
            host(actor)?;
            text(&proof.reference, 4096)?;
            ensure!(
                proof.observed_at <= now && proof.observed_at >= 0,
                "invalid stop observation time"
            );
            let a = state
                .attempts
                .get(attempt_id)
                .context("unknown attempt")?
                .clone();
            ensure!(
                a.epoch == *epoch,
                "stop proof identifies a different ownership epoch"
            );
            if a.stopped {
                outcome.task_id = Some(a.task_id);
                outcome.attempt_id = Some(attempt_id.clone());
                return Ok(outcome);
            }
            if proof.kind == StopKind::NeverDispatched {
                ensure!(
                    state
                        .deliveries
                        .values()
                        .filter(|d| d.attempt_id == *attempt_id)
                        .all(|d| matches!(
                            d.state,
                            DeliveryState::Pending | DeliveryState::Cancelled
                        )),
                    "a dispatching actor cannot be called never dispatched"
                );
            }
            let a_mut = state.attempts.get_mut(attempt_id).unwrap();
            a_mut.stopped = true;
            a_mut.stop_proof = Some(proof.clone());
            a_mut.state = AttemptState::Stopped;
            a_mut.updated_at = now;
            let candidate_needs_stability = state.tasks.get(&a.task_id).is_some_and(|t| {
                t.attempt_id.as_deref() == Some(attempt_id)
                    && t.epoch == a.epoch
                    && t.revision == a.revision
                    && matches!(t.state, TaskState::Submitted | TaskState::Verifying)
            });
            if !candidate_needs_stability {
                release(state, attempt_id);
            }
            for d in state
                .deliveries
                .values_mut()
                .filter(|d| d.attempt_id == *attempt_id && d.state == DeliveryState::Pending)
            {
                d.state = DeliveryState::Cancelled;
                d.updated_at = now;
            }
            if let Some(t) = state.tasks.get_mut(&a.task_id)
                && t.attempt_id.as_deref() == Some(attempt_id)
            {
                t.updated_at = now;
                t.state = match t.state {
                    TaskState::CancelRequested if t.cancel_reason.is_some() => TaskState::Cancelled,
                    TaskState::CancelRequested => TaskState::Pending,
                    TaskState::Assigned if proof.kind == StopKind::NeverDispatched => {
                        TaskState::Failed
                    }
                    TaskState::Running
                    | TaskState::Blocked
                    | TaskState::Uncertain
                    | TaskState::Assigned => {
                        t.last_error = Some("Worker stopped without a verified result".into());
                        TaskState::Failed
                    }
                    other => other,
                };
            }
            outcome.task_id = Some(a.task_id);
            outcome.attempt_id = Some(attempt_id.clone());
        }
        Command::SubmitResult {
            attempt_id,
            epoch,
            result,
        } => {
            if let Some(a) = state.attempts.get(attempt_id)
                && let Some(t) = state.tasks.get(&a.task_id)
                && a.epoch == *epoch
                && t.epoch == *epoch
                && t.attempt_id.as_deref() == Some(attempt_id)
                && t.revision == a.revision
                && t.result.as_ref() == Some(result)
            {
                ensure!(
                    actor.role == ActorRole::Host || actor.id == a.session_id,
                    "attempt belongs to another worker"
                );
                outcome.task_id = Some(a.task_id.clone());
                outcome.attempt_id = Some(attempt_id.clone());
                return Ok(outcome);
            }
            let a = fenced_attempt(state, actor, attempt_id, *epoch, now)?;
            evidence::validate_result(result)?;
            ensure!(
                matches!(
                    a.state,
                    AttemptState::Running | AttemptState::Blocked | AttemptState::Submitted
                ),
                "result requires an acknowledged attempt"
            );
            let t = state.tasks.get_mut(&a.task_id).unwrap();
            if let Some(old) = &t.result {
                ensure!(
                    old == result,
                    "result already submitted; revise/retry before replacing evidence"
                );
            }
            t.result = Some(result.clone());
            t.artifacts = result.artifacts.clone();
            t.state = TaskState::Submitted;
            t.blocker = None;
            t.updated_at = now;
            let a_mut = state.attempts.get_mut(attempt_id).unwrap();
            a_mut.state = AttemptState::Submitted;
            a_mut.updated_at = now;
            outcome.task_id = Some(a.task_id);
            outcome.attempt_id = Some(attempt_id.clone());
        }
        Command::Verify {
            task_id,
            revision,
            verification,
        } => {
            host(actor)?;
            let t = task(state, task_id)?;
            if t.revision == *revision && t.evidence.iter().any(|v| v == verification) {
                outcome.task_id = Some(task_id.clone());
                outcome.attempt_id = Some(verification.attempt_id.clone());
                return Ok(outcome);
            }
            ensure!(
                t.revision == *revision
                    && matches!(t.state, TaskState::Submitted | TaskState::Verifying),
                "task candidate is stale or not awaiting verification"
            );
            evidence::validate_verification(state, t, verification, now)?;
            if let Some(existing) = t.evidence.iter().find(|v| v.id == verification.id) {
                ensure!(
                    existing == verification,
                    "verification identity is immutable"
                );
            } else {
                ensure!(
                    t.evidence.len() < 256,
                    "verification history limit reached; revise/retry for further work"
                );
                let t = state.tasks.get_mut(task_id).unwrap();
                t.evidence.push(verification.clone());
                t.state = TaskState::Verifying;
                t.updated_at = now;
                if !verification.passed {
                    state.metrics.rework += 1;
                    t.last_error = Some(format!("Criterion {} failed", verification.criterion_id));
                }
            }
            outcome.task_id = Some(task_id.clone());
            outcome.attempt_id = Some(verification.attempt_id.clone());
        }
        Command::Accept {
            task_id,
            revision,
            digest,
        } => {
            operator(actor)?;
            let t = task(state, task_id)?;
            evidence::validate_acceptance(state, t, *revision, digest, now)?;
            let attempt_id = t
                .attempt_id
                .clone()
                .context("accepted task has no attempt")?;
            ensure!(
                state.attempts.get(&attempt_id).is_some_and(|a| a.stopped),
                "retire or isolate the worker before final acceptance"
            );
            let t = state.tasks.get_mut(task_id).unwrap();
            if t.state != TaskState::Verified {
                state.metrics.accepted += 1;
            }
            t.state = TaskState::Verified;
            t.updated_at = now;
            t.last_error = None;
            outcome.task_id = Some(task_id.clone());
            release(state, &attempt_id);
        }
        Command::Steer {
            task_id,
            expected_revision,
            contract,
        } => {
            operator(actor)?;
            evidence::validate_contract(contract)?;
            let t = task(state, task_id)?;
            ensure!(
                t.revision == *expected_revision,
                "task revision changed; read it before steering"
            );
            ensure!(
                t.state != TaskState::Cancelled,
                "cancelled task cannot be steered; create a new task"
            );
            state.tasks.get_mut(task_id).unwrap().contract = contract.clone();
            invalidate(
                state,
                BTreeSet::from([task_id.clone()]),
                "Contract changed; previous evidence is stale",
                now,
            );
            state.metrics.rework += 1;
            outcome.task_id = Some(task_id.clone());
        }
        Command::Cancel { task_id, reason } => {
            operator(actor)?;
            text(reason, 4096)?;
            let target = task(state, task_id)?;
            ensure!(
                target.state != TaskState::Verified,
                "verified task must be revised explicitly, not cancelled"
            );
            let mut affected = BTreeSet::from([task_id.clone()]);
            loop {
                let children: Vec<_> = state
                    .tasks
                    .values()
                    .filter(|t| {
                        !affected.contains(&t.id)
                            && t.parent_id.as_ref().is_some_and(|id| affected.contains(id))
                    })
                    .map(|t| t.id.clone())
                    .collect();
                if children.is_empty() {
                    break;
                }
                affected.extend(children);
            }
            let mut changed = Vec::new();
            for id in affected {
                let t = state.tasks.get(&id).unwrap().clone();
                if id != *task_id && t.state.terminal() {
                    continue;
                }
                let alive = t
                    .attempt_id
                    .as_ref()
                    .and_then(|id| state.attempts.get(id))
                    .is_some_and(|a| !a.stopped);
                if t.state != TaskState::Cancelled && t.cancel_reason.is_none() {
                    state.metrics.cancellations += 1;
                }
                let t = state.tasks.get_mut(&id).unwrap();
                let note = if id == *task_id {
                    reason.clone()
                } else {
                    format!("Parent {task_id} cancelled: {reason}")
                };
                t.cancel_reason = Some(note.clone());
                t.updated_at = now;
                t.blocker = None;
                if !matches!(t.state, TaskState::CancelRequested | TaskState::Cancelled) {
                    t.epoch += 1;
                }
                t.state = if alive {
                    TaskState::CancelRequested
                } else {
                    TaskState::Cancelled
                };
                t.last_error = Some(note.clone());
                if !alive && let Some(attempt_id) = t.attempt_id.clone() {
                    release(state, &attempt_id);
                }
                for delivery in state
                    .deliveries
                    .values_mut()
                    .filter(|d| d.task_id == id && d.state == DeliveryState::Pending)
                {
                    delivery.state = DeliveryState::Cancelled;
                    delivery.updated_at = now;
                    delivery.note = Some(note.clone());
                }
                changed.push(id);
            }
            outcome.task_id = Some(task_id.clone());
            outcome.value = json!({"cancelledTaskIds":changed,"scope":"unfinishedParentTree"});
        }
        Command::Retry { task_id } => {
            operator(actor)?;
            let t = task(state, task_id)?.clone();
            ensure!(
                matches!(
                    t.state,
                    TaskState::Failed
                        | TaskState::Stopped
                        | TaskState::Submitted
                        | TaskState::Verifying
                ),
                "task is not eligible for retry"
            );
            ensure!(
                t.attempt_id
                    .as_ref()
                    .and_then(|id| state.attempts.get(id))
                    .is_none_or(|a| a.stopped),
                "previous actor must be stopped or isolated before retry"
            );
            if let Some(reason) = scheduler::budget_reason(state, &t, now) {
                bail!("{reason}");
            }
            if let Some(attempt_id) = &t.attempt_id {
                release(state, attempt_id);
            }
            let t = state.tasks.get_mut(task_id).unwrap();
            t.state = TaskState::Pending;
            t.epoch += 1;
            t.updated_at = now;
            t.result = None;
            t.evidence.clear();
            t.artifacts.clear();
            t.blocker = None;
            t.cancel_reason = None;
            state.metrics.recoveries += 1;
            outcome.task_id = Some(task_id.clone());
        }
        Command::Tick => {
            host(actor)?;
            let expired: Vec<_> = state
                .attempts
                .values()
                .filter(|a| !a.stopped && a.lease_until < now && a.state != AttemptState::Uncertain)
                .map(|a| a.id.clone())
                .collect();
            for id in expired {
                let a = state.attempts.get_mut(&id).unwrap();
                a.state = AttemptState::Uncertain;
                a.updated_at = now;
                if let Some(t) = state.tasks.get_mut(&a.task_id)
                    && t.attempt_id.as_deref() == Some(&id)
                    && !matches!(
                        t.state,
                        TaskState::Verified | TaskState::CancelRequested | TaskState::Cancelled
                    )
                {
                    if !matches!(t.state, TaskState::Submitted | TaskState::Verifying) {
                        t.state = TaskState::Uncertain;
                    }
                    t.updated_at = now;
                    t.last_error = Some(
                        "Attempt lease expired; the previous actor still owns mutation resources"
                            .into(),
                    );
                }
                for d in state
                    .deliveries
                    .values_mut()
                    .filter(|d| d.attempt_id == id && d.state == DeliveryState::Dispatching)
                {
                    d.state = DeliveryState::Uncertain;
                    d.updated_at = now;
                }
            }
            let wake: Vec<_> = state
                .tasks
                .values()
                .filter(|t| {
                    matches!(
                        t.state,
                        TaskState::Blocked | TaskState::Pending | TaskState::Uncertain
                    )
                })
                .filter_map(|t| {
                    t.blocker
                        .as_ref()
                        .filter(|b| !b.requires_user && b.retry_at.is_some_and(|at| now >= at))
                        .map(|b| (t.id.clone(), b.wake_condition.clone()))
                })
                .collect();
            for (task_id, condition) in wake {
                apply(state, actor, now, &Command::Wake { task_id, condition })?;
            }
            let deadline: Vec<_> = state
                .tasks
                .values()
                .filter(|t| !t.state.terminal() && t.cancel_reason.is_none())
                .filter(|t| {
                    t.budget.deadline.is_some_and(|at| now >= at)
                        || state
                            .intents
                            .get(&t.intent_id)
                            .is_some_and(|i| i.budget.deadline.is_some_and(|at| now >= at))
                })
                .map(|t| t.id.clone())
                .collect();
            for task_id in deadline {
                apply(
                    state,
                    actor,
                    now,
                    &Command::Cancel {
                        task_id,
                        reason: "Task or intent deadline reached".into(),
                    },
                )?;
            }
        }
        Command::RegisterAdapter { adapter } => {
            host(actor)?;
            identifier(&adapter.id)?;
            text(&adapter.harness, 64)?;
            ensure!(
                adapter.capabilities.len() <= 64
                    && adapter
                        .headroom
                        .is_none_or(|v| v.is_finite() && (0.0..=100.0).contains(&v)),
                "invalid adapter capabilities or headroom"
            );
            ensure!(
                state.adapters.contains_key(&adapter.id) || state.adapters.len() < 128,
                "adapter admission limit reached"
            );
            state.adapters.insert(adapter.id.clone(), adapter.clone());
        }
        Command::RemoveAdapter { adapter_id } => {
            host(actor)?;
            ensure!(
                !state
                    .attempts
                    .values()
                    .any(|a| a.adapter_id.as_deref() == Some(adapter_id) && !a.stopped),
                "adapter still has an unreconciled actor"
            );
            state.adapters.remove(adapter_id);
        }
        Command::SetPolicy { policy } => {
            host(actor)?;
            ensure!(
                (1..=128).contains(&policy.global_limit)
                    && (1..=128).contains(&policy.project_limit)
                    && policy.completion_reserve < policy.global_limit,
                "invalid concurrency policy"
            );
            ensure!(
                (1..=512).contains(&policy.max_tasks)
                    && (1..=128).contains(&policy.max_intents)
                    && policy.max_tasks >= state.tasks.len()
                    && policy.max_intents >= state.intents.len(),
                "invalid admission policy"
            );
            ensure!(
                (10..=86400).contains(&policy.default_lease_seconds)
                    && policy.aging_seconds > 0
                    && policy.aging_seconds <= 86400,
                "invalid lease or aging policy"
            );
            state.policy = policy.clone();
        }
        Command::RecordCost {
            attempt_id,
            tokens,
            cost_usd,
        } => {
            host(actor)?;
            ensure!(
                cost_usd.is_none_or(|v| v.is_finite() && v >= 0.0),
                "invalid reported cost"
            );
            let a = state
                .attempts
                .get_mut(attempt_id)
                .context("unknown attempt")?;
            if let Some(tokens) = tokens {
                ensure!(
                    *tokens >= a.tokens.unwrap_or(0),
                    "usage reports are cumulative and cannot decrease"
                );
                state.metrics.tokens_reported = state
                    .metrics
                    .tokens_reported
                    .saturating_add(*tokens - a.tokens.unwrap_or(0));
                a.tokens = Some(*tokens);
            }
            if let Some(cost) = cost_usd {
                ensure!(
                    *cost >= a.cost_usd.unwrap_or(0.0),
                    "cost reports are cumulative and cannot decrease"
                );
                state.metrics.cost_reported_usd += *cost - a.cost_usd.unwrap_or(0.0);
                if a.cost_usd.is_none() {
                    state.metrics.cost_reports += 1;
                }
                a.cost_usd = Some(*cost);
            }
            outcome.task_id = Some(a.task_id.clone());
            outcome.attempt_id = Some(attempt_id.clone());
        }
        Command::SaveCheckpoint { checkpoint } => {
            let a = fenced_attempt(state, actor, &checkpoint.attempt_id, checkpoint.epoch, now)?;
            ensure!(
                checkpoint.task_id == a.task_id
                    && checkpoint.revision == a.revision
                    && checkpoint.at <= now,
                "checkpoint belongs to a stale contract"
            );
            text(&checkpoint.summary, 32768)?;
            ensure!(
                checkpoint.next_steps.len() <= 64 && checkpoint.sources.len() <= 128,
                "checkpoint exceeds limits"
            );
            state
                .checkpoints
                .insert(a.task_id.clone(), checkpoint.clone());
            outcome.task_id = Some(a.task_id);
            outcome.attempt_id = Some(a.id);
        }
        Command::PutMemory { memory } => {
            host(actor)?;
            identifier(&memory.id)?;
            text(&memory.scope, 4096)?;
            text(&memory.text, 16384)?;
            ensure!(
                !memory.sources.is_empty()
                    && memory.sources.len() <= 128
                    && memory.sources.values().all(|d| evidence::valid_digest(d)),
                "memory requires source-backed content identities"
            );
            ensure!(
                state.memory.contains_key(&memory.id) || state.memory.len() < 512,
                "memory admission limit reached"
            );
            let mut memory = memory.clone();
            memory.valid = true;
            memory.created_at = now;
            memory.invalidated_at = None;
            state.memory.insert(memory.id.clone(), memory);
        }
        Command::InvalidateSource { source, digest } => {
            host(actor)?;
            text(source, 4096)?;
            ensure!(
                evidence::valid_digest(digest),
                "source requires a new content digest"
            );
            for memory in state.memory.values_mut() {
                if memory.sources.get(source).is_some_and(|d| d != digest) {
                    memory.valid = false;
                    memory.invalidated_at = Some(now);
                }
            }
            let seeds: BTreeSet<_> = state
                .tasks
                .values()
                .filter(|t| t.contract.sources.get(source).is_some_and(|d| d != digest))
                .map(|t| t.id.clone())
                .collect();
            for id in &seeds {
                state
                    .tasks
                    .get_mut(id)
                    .unwrap()
                    .contract
                    .sources
                    .insert(source.clone(), digest.clone());
            }
            invalidate(
                state,
                seeds,
                "A relevant source changed; evidence requires revalidation",
                now,
            );
        }
        Command::ArchiveIntent { intent_id } => {
            operator(actor)?;
            ensure!(state.intents.contains_key(intent_id), "unknown intent");
            let ids: BTreeSet<_> = state
                .tasks
                .values()
                .filter(|t| t.intent_id == *intent_id)
                .map(|t| t.id.clone())
                .collect();
            ensure!(
                ids.iter()
                    .all(|id| state.tasks.get(id).unwrap().state.terminal()),
                "only finished intents can be archived"
            );
            ensure!(
                !state
                    .attempts
                    .values()
                    .any(|a| ids.contains(&a.task_id) && !a.stopped),
                "retire all intent actors before archiving"
            );
            ensure!(
                !state.tasks.values().any(|t| !ids.contains(&t.id)
                    && scheduler::dependency_ids(state, t)
                        .iter()
                        .any(|id| ids.contains(id))),
                "other tasks still depend on this intent"
            );
            state.tasks.retain(|id, _| !ids.contains(id));
            state.attempts.retain(|_, a| !ids.contains(&a.task_id));
            state.deliveries.retain(|_, d| !ids.contains(&d.task_id));
            state.checkpoints.retain(|id, _| !ids.contains(id));
            state
                .memory
                .retain(|_, m| m.scope != *intent_id && !ids.contains(&m.scope));
            state.intents.remove(intent_id);
        }
    }
    Ok(outcome)
}
