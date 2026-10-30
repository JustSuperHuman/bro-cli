//! Orchestrators outlive bro: each scope's launch configuration is kept in
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

/// Preferred configuration for the workspace inbox. Project restores use [`load_all`].
#[cfg(test)]
pub fn load() -> Option<LaunchSpec> {
    let all = load_all();
    all.iter().find(|s| bro_core::orchestrator::is_global(&s.cwd)).cloned().or_else(|| all.into_iter().next())
}

pub fn load_all() -> Vec<LaunchSpec> {
    std::fs::read_to_string(path()).ok().map(|body| decode(&body)).unwrap_or_default()
}

fn decode(body: &str) -> Vec<LaunchSpec> {
    // Older Bro versions saved one object. Its workspace still identifies the root Orchestrator.
    let saved = serde_json::from_str::<Vec<Saved>>(body).unwrap_or_else(|_| {
        serde_json::from_str::<Saved>(body).map(|mut s| {
            s.spec.cwd = bro_core::orchestrator::workspace();
            vec![s]
        }).unwrap_or_default()
    });
    saved.into_iter().filter(|s| s.spec.orchestrator).map(|s| s.spec).collect()
}

fn write(specs: Vec<LaunchSpec>) {
    let saved: Vec<Saved> = specs.into_iter().map(|spec| Saved { spec, at: crate::util::now_secs() }).collect();
    if let Ok(body) = serde_json::to_vec_pretty(&saved) {
        let _ = crate::util::atomic_write(&path(), &body);
    }
}

/// An orchestrator is running with `spec`.
pub fn save(spec: &LaunchSpec) {
    let mut specs = load_all();
    let key = bro_core::orchestrator::scope_key(&spec.cwd);
    specs.retain(|s| bro_core::orchestrator::scope_key(&s.cwd) != key);
    specs.push(spec.clone());
    write(specs);
}

/// Closing or failing one coordinator must leave other scopes available for restore.
pub fn forget(cwd: &std::path::Path) {
    let key = bro_core::orchestrator::scope_key(cwd);
    let specs = load_all().into_iter().filter(|s| bro_core::orchestrator::scope_key(&s.cwd) != key).collect();
    write(specs);
}

/// You closed it: the next bro starts without one.
#[cfg(test)]
pub fn clear() {
    let _ = std::fs::remove_file(path());
}
