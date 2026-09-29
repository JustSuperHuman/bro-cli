//! Description caches, one JSON file per harness keyed by session file path, with v1's
//! entry shape (`{ mtime, size, title, cwd, branch }`, plus `id`/`interactive` for
//! Codex). v2 keeps its own files (`*-v2.cache.json`): each listing prunes the entries
//! it no longer sees, so sharing v1's files would have the two versions evict each
//! other's rows on every run.
use super::describe::Described;
use crate::util::{read_json, write_json_compact};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Entry {
    pub mtime: f64,
    pub size: u64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub branch: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
    #[serde(default = "yes")]
    pub interactive: bool,
}

fn yes() -> bool {
    true
}

impl Entry {
    pub fn new(mtime: f64, size: u64, d: &Described) -> Entry {
        Entry {
            mtime,
            size,
            title: d.title.clone(),
            cwd: d.cwd.clone(),
            branch: d.branch.clone(),
            id: d.id.clone(),
            interactive: d.interactive,
        }
    }

    pub fn described(&self) -> Described {
        Described {
            id: self.id.clone(),
            title: self.title.clone(),
            cwd: self.cwd.clone(),
            branch: self.branch.clone(),
            interactive: self.interactive,
        }
    }
}

/// One harness's cache file.
pub(crate) struct Cache {
    path: PathBuf,
    pub entries: HashMap<String, Entry>,
    pub dirty: bool,
}

impl Cache {
    pub fn load(path: PathBuf) -> Cache {
        let mut entries = HashMap::new();
        if let Some(serde_json::Value::Object(map)) = read_json(&path) {
            for (k, v) in map {
                if let Ok(e) = serde_json::from_value::<Entry>(v) {
                    entries.insert(k, e);
                }
            }
        }
        Cache { path, entries, dirty: false }
    }

    /// Fresh entry for this file if its stat identity still matches.
    pub fn hit(&self, key: &str, mtime: f64, size: u64) -> Option<Described> {
        self.entries.get(key).filter(|e| e.mtime == mtime && e.size == size).map(Entry::described)
    }

    pub fn put(&mut self, key: String, entry: Entry) {
        self.entries.insert(key, entry);
        self.dirty = true;
    }

    /// Drop entries not in `live` and write (best-effort) when anything changed.
    pub fn save_pruned(mut self, live: &std::collections::HashSet<String>) {
        if !self.dirty {
            return;
        }
        self.entries.retain(|k, _| live.contains(k));
        let _ = write_json_compact(&self.path, &self.entries);
    }
}
