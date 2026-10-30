use super::model::*;
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

pub fn digest_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentEntry {
    pub path: String,
    pub sha256: String,
    pub bytes: u64,
    pub deleted: bool,
}

/// Identity includes relative names, actual bytes' digests, additions and deletions.
/// Build the manifest outside the authority lock, from the full relevant dirty/untracked tree.
pub fn digest_manifest(entries: &[ContentEntry]) -> Result<String> {
    let mut entries = entries.to_vec();
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    let mut names = BTreeSet::new();
    for entry in &entries {
        ensure!(
            !entry.path.is_empty() && names.insert(&entry.path),
            "manifest paths must be nonempty and unique"
        );
        ensure!(
            entry.deleted || valid_digest(&entry.sha256),
            "manifest requires SHA-256 of actual file content"
        );
    }
    Ok(digest_bytes(&serde_json::to_vec(&entries)?))
}

pub(super) fn valid_digest(digest: &str) -> bool {
    digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_hexdigit())
}

pub(super) fn validate_contract(contract: &TaskContract) -> Result<()> {
    ensure!(
        !contract.goal.trim().is_empty(),
        "task requires an explicit goal"
    );
    ensure!(
        contract.goal.len() <= 8192 && contract.instructions.len() <= 32768,
        "task contract is too large"
    );
    ensure!(
        contract.acceptance.len() <= 64 && contract.acceptance.iter().any(|c| c.required),
        "at least one required observable acceptance criterion is needed"
    );
    ensure!(
        contract.constraints.len() <= 64
            && contract.context.len() <= 64
            && contract.sources.len() <= 128,
        "contract collections exceed limits"
    );
    ensure!(
        contract
            .constraints
            .iter()
            .chain(&contract.context)
            .all(|s| s.len() <= 4096)
            && contract
                .sources
                .keys()
                .all(|s| !s.is_empty() && s.len() <= 4096)
            && serde_json::to_vec(contract)?.len() <= 64 * 1024,
        "contract retained data exceeds limits"
    );
    let mut ids = BTreeSet::new();
    for c in &contract.acceptance {
        super::transition::identifier(&c.id)?;
        ensure!(ids.insert(&c.id), "duplicate criterion id");
        ensure!(
            !c.description.trim().is_empty() && c.description.len() <= 4096,
            "criterion needs a bounded description"
        );
        if c.kind == CriterionKind::Check {
            ensure!(
                c.command
                    .as_ref()
                    .is_some_and(|s| !s.trim().is_empty() && s.len() <= 8192),
                "check criterion requires its exact command"
            );
        }
    }
    if contract.quality == QualityMode::Critical {
        ensure!(
            contract
                .acceptance
                .iter()
                .any(|c| c.required && c.kind == CriterionKind::Review && c.independent),
            "critical work requires an independent review criterion"
        );
    }
    for digest in contract.sources.values() {
        ensure!(
            valid_digest(digest),
            "source identities require content digests"
        );
    }
    Ok(())
}

pub(super) fn validate_result(result: &TaskResult) -> Result<()> {
    ensure!(
        !result.summary.trim().is_empty() && result.summary.len() <= 16384,
        "result needs a bounded summary"
    );
    ensure!(
        valid_digest(&result.digest),
        "result requires an actual content SHA-256 identity"
    );
    ensure!(
        result.artifacts.len() <= 128 && result.unresolved.len() <= 64,
        "result collections exceed limits"
    );
    ensure!(
        serde_json::to_vec(result)?.len() <= 64 * 1024,
        "result retained data exceeds limits"
    );
    let mut ids = BTreeSet::new();
    for artifact in &result.artifacts {
        super::transition::identifier(&artifact.id)?;
        ensure!(ids.insert(&artifact.id), "duplicate artifact id");
        ensure!(
            valid_digest(&artifact.sha256) && artifact.source_digest == result.digest,
            "artifact must identify its bytes and the submitted content"
        );
        ensure!(
            !artifact.path.is_empty() && artifact.path.len() <= 4096,
            "artifact requires a bounded location"
        );
    }
    Ok(())
}

pub(super) fn validate_verification(
    state: &State,
    task: &TaskView,
    v: &Verification,
    now: i64,
) -> Result<()> {
    super::transition::identifier(&v.id)?;
    let result = task
        .result
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("no submitted result to verify"))?;
    let attempt = state
        .attempts
        .get(&v.attempt_id)
        .ok_or_else(|| anyhow::anyhow!("unknown verification attempt"))?;
    ensure!(
        task.attempt_id.as_deref() == Some(&v.attempt_id)
            && task.epoch == v.epoch
            && attempt.epoch == v.epoch,
        "verification attempt is stale"
    );
    ensure!(
        v.revision == task.revision
            && attempt.revision == task.revision
            && v.digest == result.digest,
        "verification content or contract is stale"
    );
    ensure!(
        !v.verifier.trim().is_empty() && valid_digest(&v.output_digest),
        "trusted verification needs verifier identity and actual output digest"
    );
    ensure!(
        v.finished_at >= attempt.started_at && v.finished_at <= now,
        "invalid verifier execution time"
    );
    let criterion = task
        .contract
        .acceptance
        .iter()
        .find(|c| c.id == v.criterion_id)
        .ok_or_else(|| anyhow::anyhow!("verification names an unknown criterion"))?;
    if criterion.independent {
        ensure!(
            v.verifier != attempt.session_id,
            "criterion requires a verifier independent of its worker"
        );
    }
    match criterion.kind {
        CriterionKind::Check => {
            ensure!(
                v.command.as_ref() == criterion.command.as_ref(),
                "verification command differs from the required check"
            );
            ensure!(
                v.exit_code.is_some(),
                "check verification requires the actual process exit code"
            );
            ensure!(
                !v.passed || v.exit_code == Some(0),
                "a nonzero check cannot pass"
            );
        }
        CriterionKind::Artifact => {
            let artifact_id = v
                .artifact_id
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("artifact verification needs artifact identity"))?;
            ensure!(
                result.artifacts.iter().any(|a| &a.id == artifact_id),
                "verification artifact was not submitted"
            );
        }
        CriterionKind::Review => {
            ensure!(
                v.command.as_ref().is_some_and(|s| !s.trim().is_empty()),
                "review requires an observed review operation"
            );
        }
    }
    Ok(())
}

pub(super) fn validate_acceptance(
    state: &State,
    task: &TaskView,
    revision: u64,
    digest: &str,
    now: i64,
) -> Result<()> {
    ensure!(
        matches!(
            task.state,
            TaskState::Submitted | TaskState::Verifying | TaskState::Verified
        ),
        "task has no candidate eligible for acceptance"
    );
    ensure!(
        task.revision == revision && super::scheduler::blocked_by(state, task).is_empty(),
        "contract or dependency evidence is not current"
    );
    validate_contract(&task.contract)?;
    let result = task
        .result
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("task has no structured result"))?;
    ensure!(
        result.digest == digest && result.unresolved.is_empty(),
        "content changed or unresolved defects remain"
    );
    for criterion in task.contract.acceptance.iter().filter(|c| c.required) {
        let v = task
            .evidence
            .iter()
            .filter(|v| v.criterion_id == criterion.id)
            .max_by_key(|v| v.finished_at)
            .ok_or_else(|| {
                anyhow::anyhow!("required criterion {} lacks trusted evidence", criterion.id)
            })?;
        validate_verification(state, task, v, now)?;
        ensure!(v.passed, "required criterion {} did not pass", criterion.id);
    }
    Ok(())
}
