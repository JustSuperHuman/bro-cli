use super::model::*;
use anyhow::{Result, bail, ensure};
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn dependency_ids(state: &State, task: &TaskView) -> Vec<String> {
    let mut ids: BTreeSet<String> = task.dependencies.iter().cloned().collect();
    ids.extend(
        state
            .tasks
            .values()
            .filter(|t| t.parent_id.as_deref() == Some(&task.id))
            .map(|t| t.id.clone()),
    );
    ids.into_iter().collect()
}

pub(super) fn blocked_by(state: &State, task: &TaskView) -> Vec<String> {
    dependency_ids(state, task)
        .into_iter()
        .filter(|id| {
            state
                .tasks
                .get(id)
                .is_none_or(|t| t.state != TaskState::Verified)
        })
        .collect()
}

pub(super) fn validate_graph(state: &State) -> Result<()> {
    fn visit(
        state: &State,
        id: &str,
        active: &mut BTreeSet<String>,
        done: &mut BTreeSet<String>,
    ) -> Result<()> {
        if done.contains(id) {
            return Ok(());
        }
        ensure!(active.insert(id.into()), "task dependency cycle at {id}");
        let task = state
            .tasks
            .get(id)
            .ok_or_else(|| anyhow::anyhow!("missing dependency {id}"))?;
        if let Some(parent) = &task.parent_id {
            let p = state
                .tasks
                .get(parent)
                .ok_or_else(|| anyhow::anyhow!("missing parent {parent}"))?;
            ensure!(
                p.intent_id == task.intent_id,
                "parent must belong to the same intent"
            );
            ensure!(p.id != task.id, "a task cannot parent itself");
        }
        for dep in dependency_ids(state, task) {
            visit(state, &dep, active, done)?;
        }
        active.remove(id);
        done.insert(id.into());
        Ok(())
    }
    let mut done = BTreeSet::new();
    for id in state.tasks.keys() {
        visit(state, id, &mut BTreeSet::new(), &mut done)?;
    }
    Ok(())
}

pub(super) fn resource_conflict(state: &State, task: &TaskView) -> Option<String> {
    for claim in &task.resources {
        for lease in state.resources.values().filter(|r| r.key == claim.key) {
            if lease.mode == ClaimMode::Exclusive || claim.mode == ClaimMode::Exclusive {
                return Some(format!(
                    "resource {} is held by attempt {} until safely reconciled",
                    claim.key, lease.attempt_id
                ));
            }
        }
    }
    None
}

pub(super) fn budget_reason(state: &State, task: &TaskView, now: i64) -> Option<String> {
    let intent = state.intents.get(&task.intent_id)?;
    for (label, budget) in [("task", &task.budget), ("intent", &intent.budget)] {
        let attempts: Vec<_> = state
            .attempts
            .values()
            .filter(|a| {
                if label == "task" {
                    a.task_id == task.id
                } else {
                    state
                        .tasks
                        .get(&a.task_id)
                        .is_some_and(|t| t.intent_id == task.intent_id)
                }
            })
            .collect();
        if attempts.len() >= budget.max_attempts as usize {
            return Some(format!("{label} attempt budget exhausted"));
        }
        if budget.deadline.is_some_and(|at| now >= at) {
            return Some(format!("{label} deadline reached"));
        }
        let tokens: u64 = attempts.iter().filter_map(|a| a.tokens).sum();
        if budget.max_tokens.is_some_and(|max| tokens >= max) {
            return Some(format!("{label} reported token budget exhausted"));
        }
        let cost: f64 = attempts.iter().filter_map(|a| a.cost_usd).sum();
        if budget.max_cost_usd.is_some_and(|max| cost >= max) {
            return Some(format!("{label} reported cost budget exhausted"));
        }
    }
    None
}

pub(super) fn capacity_reason(state: &State, task: &TaskView) -> Option<String> {
    let active: Vec<_> = state.attempts.values().filter(|a| !a.stopped).collect();
    if active.len() >= state.policy.global_limit {
        return Some("global concurrency limit reached".into());
    }
    let project = active
        .iter()
        .filter(|a| {
            state
                .tasks
                .get(&a.task_id)
                .is_some_and(|t| t.project == task.project)
        })
        .count();
    if project >= state.policy.project_limit {
        return Some("project concurrency limit reached".into());
    }
    let completion_waiting = state.tasks.values().any(|t| {
        t.lane == Lane::Completion
            && t.state == TaskState::Pending
            && t.blocker.is_none()
            && blocked_by(state, t).is_empty()
    });
    if task.lane == Lane::Work
        && completion_waiting
        && active.len()
            >= state
                .policy
                .global_limit
                .saturating_sub(state.policy.completion_reserve)
    {
        return Some("capacity reserved for completion work".into());
    }
    None
}

pub(super) fn adapter_reason(
    state: &State,
    task: &TaskView,
    adapter_id: Option<&str>,
) -> Option<String> {
    let Some(id) = adapter_id else {
        return (!task.capabilities.is_empty())
            .then(|| "task requires a capability-advertising adapter".into());
    };
    let Some(adapter) = state.adapters.get(id) else {
        return Some("adapter is not registered".into());
    };
    if !adapter.available || adapter.headroom.is_some_and(|h| h <= 0.0) {
        return Some("adapter unavailable or exhausted".into());
    }
    for capability in &task.capabilities {
        if adapter.capabilities.get(capability) != Some(&Capability::Supported) {
            return Some(format!(
                "adapter has not demonstrated capability {capability}"
            ));
        }
    }
    None
}

pub(super) fn view(state: &State, task: &TaskView) -> TaskView {
    let mut view = task.clone();
    view.blocked_by = blocked_by(state, task);
    view.next_action = match task.state {
        TaskState::Pending | TaskState::Ready => {
            if let Some(blocker) = &task.blocker {
                format!("Wait for {}: {}", blocker.wake_condition, blocker.message)
            } else if !view.blocked_by.is_empty() {
                format!("Verify dependencies: {}", view.blocked_by.join(", "))
            } else if let Some(reason) = budget_reason(state, task, crate::util::now_secs())
                .or_else(|| resource_conflict(state, task))
                .or_else(|| capacity_reason(state, task))
            {
                reason
            } else {
                view.state = TaskState::Ready;
                "Assign a capable worker".into()
            }
        }
        TaskState::Assigned => "Deliver the persisted assignment and acknowledge it".into(),
        TaskState::Running => "Await progress or a structured result".into(),
        TaskState::Blocked => task
            .blocker
            .as_ref()
            .map(|b| format!("Wait for {}: {}", b.wake_condition, b.message))
            .unwrap_or_else(|| "Resolve the recorded blocker".into()),
        TaskState::Uncertain => {
            "Reconcile the prior actor before retrying; resource claims remain held".into()
        }
        TaskState::Submitted | TaskState::Verifying => {
            "Run the required checks against the submitted digest".into()
        }
        TaskState::Verified => {
            if state.resources.values().any(|r| r.task_id == task.id) {
                "Verified; retire or isolate the worker to release its resources".into()
            } else {
                "Accepted with current evidence".into()
            }
        }
        TaskState::CancelRequested => "Prove the worker stopped or was isolated".into(),
        TaskState::Cancelled => "Cancellation reconciled".into(),
        TaskState::Failed => "Review the failure and retry within the remaining budget".into(),
        TaskState::Stopped => "Stopped without claiming completion".into(),
    };
    view
}

pub(super) fn ready(state: &State, adapter_id: Option<&str>, now: i64) -> Vec<TaskView> {
    let mut last_project: BTreeMap<&str, i64> = BTreeMap::new();
    for a in state.attempts.values() {
        if let Some(task) = state.tasks.get(&a.task_id) {
            last_project
                .entry(&task.project)
                .and_modify(|at| *at = (*at).max(a.started_at))
                .or_insert(a.started_at);
        }
    }
    let mut tasks: Vec<_> = state
        .tasks
        .values()
        .filter(|task| {
            task.state == TaskState::Pending
                && task.blocker.is_none()
                && blocked_by(state, task).is_empty()
                && resource_conflict(state, task).is_none()
                && capacity_reason(state, task).is_none()
                && budget_reason(state, task, now).is_none()
                && adapter_reason(state, task, adapter_id).is_none()
        })
        .map(|t| view(state, t))
        .collect();
    let aging = state.policy.aging_seconds.max(1) as i64;
    tasks.sort_by(|a, b| {
        let score = |t: &TaskView| {
            i64::from(t.priority).saturating_add(now.saturating_sub(t.created_at).max(0) / aging)
        };
        b.lane
            .cmp_completion(a.lane)
            .then_with(|| score(b).cmp(&score(a)))
            .then_with(|| {
                last_project
                    .get(a.project.as_str())
                    .copied()
                    .unwrap_or(0)
                    .cmp(&last_project.get(b.project.as_str()).copied().unwrap_or(0))
            })
            .then_with(|| a.created_at.cmp(&b.created_at))
            .then_with(|| a.id.cmp(&b.id))
    });
    tasks
}
trait LaneOrder {
    fn cmp_completion(self, other: Self) -> std::cmp::Ordering;
}
impl LaneOrder for Lane {
    fn cmp_completion(self, other: Self) -> std::cmp::Ordering {
        (self == Lane::Completion).cmp(&(other == Lane::Completion))
    }
}

pub(super) fn validate_claim(
    state: &State,
    task: &TaskView,
    session: &str,
    adapter: Option<&str>,
    now: i64,
) -> Result<()> {
    ensure!(
        task.state == TaskState::Pending && task.blocker.is_none(),
        "task is not ready for assignment"
    );
    ensure!(
        blocked_by(state, task).is_empty(),
        "task dependencies are not verified"
    );
    ensure!(
        !state
            .attempts
            .values()
            .any(|a| a.session_id == session && !a.stopped),
        "session already owns an unreconciled attempt"
    );
    for reason in [
        budget_reason(state, task, now),
        capacity_reason(state, task),
        resource_conflict(state, task),
        adapter_reason(state, task, adapter),
    ] {
        if let Some(reason) = reason {
            bail!("{reason}");
        }
    }
    Ok(())
}

pub(super) fn snapshot(state: &State) -> Snapshot {
    Snapshot {
        seq: state.seq,
        intents: state.intents.values().cloned().collect(),
        tasks: state.tasks.values().map(|t| view(state, t)).collect(),
        attempts: state.attempts.values().cloned().collect(),
        deliveries: state.deliveries.values().cloned().collect(),
        resources: state.resources.values().cloned().collect(),
        adapters: state.adapters.values().cloned().collect(),
        metrics: state.metrics.clone(),
        recovery: state.recovery.clone(),
        policy: state.policy.clone(),
        events: state.events.clone(),
    }
}
