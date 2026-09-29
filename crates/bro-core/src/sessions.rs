//! Past agent sessions on disk, for the sidebar's "resume" lists.
//! Claude: `<profile>/projects/<encoded>/<uuid>.jsonl` (v1 `describeTranscript` rules,
//! cache `~/.bro/sessions.cache.json`). Codex: `<home>/sessions/YYYY/MM/DD/rollout-*.jsonl`
//! (interactive only, cache `~/.bro/codex-sessions.cache.json`). Pi/omp: their session
//! dirs if present (`~/.pi/agent/sessions`, `~/.omp/agent/sessions`).
use crate::{Harness, projects::ProjectKey};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::SystemTime;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: String,
    pub harness: Harness,
    /// Owning profile id ("claude:work"); None for pi/omp
    pub profile_id: Option<String>,
    pub path: PathBuf,
    pub cwd: Option<PathBuf>,
    pub project: Option<ProjectKey>,
    /// First real user prompt / summary, single line
    pub title: String,
    pub branch: Option<String>,
    pub modified: SystemTime,
    pub size: u64,
}

#[derive(Debug, Clone, Default)]
pub struct ListOpts {
    /// Max sessions described overall (newest first). 0 = default (400).
    pub limit: usize,
    /// Restrict to these harnesses (empty = all)
    pub harnesses: Vec<Harness>,
}

/// Newest first. Uses on-disk caches; blocking.
pub fn list(opts: &ListOpts) -> Vec<SessionInfo> { todo!() }

/// Copy a session (and its sidecar files) from its owner profile into `target_profile_id`
/// so it can be resumed there with fork semantics (v1 `stageSessionForProfile` /
/// codex `stageFiles`). Returns the files created so the caller can clean up.
pub fn stage_for_profile(session: &SessionInfo, target_profile_id: &str) -> anyhow::Result<Vec<PathBuf>> { todo!() }
