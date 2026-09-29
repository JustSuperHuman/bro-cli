//! The projects you've opened in bro (the sidebar's list), in your order. Persisted in
//! `~/.bro/v2-projects.json` (atomic writes, on a background thread). Opening or closing a project never
//! touches the folder itself.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct OpenProjects {
    pub roots: Vec<PathBuf>,
}

pub fn path() -> PathBuf {
    crate::util::bro_dir().join("v2-projects.json")
}

fn same(a: &Path, b: &Path) -> bool {
    if cfg!(windows) { a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase() } else { a == b }
}

impl OpenProjects {
    /// Load (missing or broken file = empty).
    pub fn load() -> OpenProjects {
        std::fs::read_to_string(path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    pub fn contains(&self, root: &Path) -> bool {
        self.roots.iter().any(|r| same(r, root))
    }

    /// Add at the end; false when it was already open.
    pub fn add(&mut self, root: PathBuf) -> bool {
        if self.contains(&root) {
            return false;
        }
        self.roots.push(root);
        true
    }

    pub fn remove(&mut self, root: &Path) -> bool {
        let n = self.roots.len();
        self.roots.retain(|r| !same(r, root));
        self.roots.len() != n
    }

    /// Save on a background thread (no-op when `enabled` is false, e.g. demo and tests).
    pub fn save(&self, enabled: bool) {
        if !enabled {
            return;
        }
        let me = self.clone();
        std::thread::spawn(move || {
            if let Ok(s) = serde_json::to_vec_pretty(&me) {
                let _ = crate::util::atomic_write(&path(), &s);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_remove_dedup() {
        let mut p = OpenProjects::default();
        assert!(p.add(PathBuf::from("/a")));
        assert!(!p.add(PathBuf::from("/a")));
        assert!(p.add(PathBuf::from("/b")));
        assert!(p.remove(Path::new("/a")));
        assert_eq!(p.roots, vec![PathBuf::from("/b")]);
        let back: OpenProjects = serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        assert!(back.contains(Path::new("/b")));
    }
}
