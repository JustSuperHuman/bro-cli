//! The orchestrator outlives bro: while one runs, how it was launched is kept in
//! `~/.bro/v2-orchestrator.json`, and the next bro to start (a reopen, or another instance) starts it again
//! with the same harness, login, model, permissions and browser. Closing it yourself forgets it; quitting bro
//! (or bro dying) does not.

use bro_core::launch::LaunchSpec;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Saved {
    pub spec: LaunchSpec,
    /// unix seconds it was started
    pub at: i64,
}

pub fn path() -> PathBuf {
    crate::util::bro_dir().join("v2-orchestrator.json")
}

/// The orchestrator to bring back, if one was running when bro last stopped.
pub fn load() -> Option<LaunchSpec> {
    let saved: Saved = serde_json::from_str(&std::fs::read_to_string(path()).ok()?).ok()?;
    saved.spec.orchestrator.then_some(saved.spec)
}

/// An orchestrator is running with `spec`.
pub fn save(spec: &LaunchSpec) {
    let saved = Saved { spec: spec.clone(), at: crate::util::now_secs() };
    if let Ok(body) = serde_json::to_vec_pretty(&saved) {
        let _ = crate::util::atomic_write(&path(), &body);
    }
}

/// You closed it: the next bro starts without one.
pub fn clear() {
    let _ = std::fs::remove_file(path());
}
