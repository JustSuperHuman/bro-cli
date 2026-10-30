use super::model::*;
use anyhow::{Context, Result};

pub(super) fn packet(state: &State, task_id: &str, now: i64) -> Result<ContextPacket> {
    let task = state.tasks.get(task_id).context("unknown task")?;
    let intent = state
        .intents
        .get(&task.intent_id)
        .context("task has no intent")?;
    let checkpoint = state
        .checkpoints
        .get(task_id)
        .filter(|c| c.revision == task.revision && c.epoch == task.epoch)
        .cloned();
    let mut memory: Vec<_> = state
        .memory
        .values()
        .filter(|m| {
            m.valid && (m.scope == task.project || m.scope == task.intent_id || m.scope == task.id)
        })
        .filter(|m| {
            m.sources
                .iter()
                .all(|(source, digest)| task.contract.sources.get(source) == Some(digest))
        })
        .cloned()
        .collect();
    memory.sort_by_key(|m| std::cmp::Reverse(m.created_at));
    memory.truncate(4);
    let dependencies = super::scheduler::dependency_ids(state, task)
        .iter()
        .filter_map(|id| state.tasks.get(id))
        .map(|t| TaskView {
            id: t.id.clone(),
            intent_id: t.intent_id.clone(),
            title: t.title.clone(),
            project: t.project.clone(),
            state: t.state,
            revision: t.revision,
            epoch: t.epoch,
            result: t.result.as_ref().map(|r| TaskResult {
                digest: r.digest.clone(),
                summary: r.summary.chars().take(512).collect(),
                ..Default::default()
            }),
            ..Default::default()
        })
        .collect();
    Ok(ContextPacket {
        task: super::scheduler::view(state, task),
        intent: intent.clone(),
        checkpoint,
        memory,
        dependencies,
        generated_at: now,
    })
}
