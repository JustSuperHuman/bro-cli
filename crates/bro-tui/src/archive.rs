//! Archived sessions: earlier sessions you've hidden from the sidebar. Only bro's view changes — the
//! transcripts stay on disk and can still be resumed from the "show archived" list. Persisted in
//! `~/.bro/v2-archive.json` (atomic writes, on a background thread).

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::PathBuf;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Archive {
    /// session ids (Claude / Codex / pi / omp ids are unique enough across harnesses)
    pub ids: BTreeSet<String>,
    /// batches archived this run, newest last, for undo
    #[serde(skip)]
    undo: Vec<Vec<String>>,
}

pub fn path() -> PathBuf {
    crate::util::bro_dir().join("v2-archive.json")
}

impl Archive {
    /// Load (missing or broken file = empty).
    pub fn load() -> Archive {
        std::fs::read_to_string(path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    pub fn contains(&self, id: &str) -> bool {
        self.ids.contains(id)
    }

    /// Archive a batch (one session, or a whole project's). Returns how many were newly archived.
    pub fn add(&mut self, ids: impl IntoIterator<Item = String>) -> usize {
        let batch: Vec<String> = ids.into_iter().filter(|id| self.ids.insert(id.clone())).collect();
        let n = batch.len();
        if n > 0 {
            self.undo.push(batch);
        }
        n
    }

    /// Bring sessions back.
    pub fn remove(&mut self, ids: &[String]) -> usize {
        ids.iter().filter(|id| self.ids.remove(*id)).count()
    }

    /// Undo the most recent batch. Returns how many came back.
    pub fn undo(&mut self) -> usize {
        match self.undo.pop() {
            Some(batch) => self.remove(&batch),
            None => 0,
        }
    }

    /// Save on a background thread (no-op when `enabled` is false, e.g. demo and tests).
    pub fn save(&self, enabled: bool) {
        if !enabled {
            return;
        }
        let ids = self.ids.clone();
        std::thread::spawn(move || {
            if let Ok(s) = serde_json::to_vec_pretty(&Archive { ids, undo: vec![] }) {
                let _ = crate::util::atomic_write(&path(), &s);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_undo_and_round_trip() {
        let mut a = Archive::default();
        assert_eq!(a.add(["a".to_string()]), 1);
        assert_eq!(a.add(["a".to_string(), "b".to_string(), "c".to_string()]), 2, "a was already archived");
        assert!(a.contains("b"));
        assert_eq!(a.undo(), 2);
        assert!(a.contains("a") && !a.contains("b") && !a.contains("c"));
        assert_eq!(a.undo(), 1);
        assert_eq!(a.undo(), 0);
        a.add(["x".to_string()]);
        let back: Archive = serde_json::from_str(&serde_json::to_string(&a).unwrap()).unwrap();
        assert!(back.contains("x"));
    }
}
