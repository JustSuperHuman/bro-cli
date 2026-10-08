//! Which path-like words on a terminal screen are real files. Rendering asks the cache (never the disk); unknown
//! and stale candidates are resolved in a batch on a worker thread (`bro_bridge::file_preview::resolve_path`:
//! cwd, project root, git `a/`/`b/` prefixes, a unique suffix in the project index), which wakes the pane
//! when it's done. A confirmed file keeps its link while it is re-checked, so links don't flicker.

use crate::pane::Waker;
use bro_bridge::file_links::FileLinkMatch;
use bro_bridge::file_preview::{Kind, ResolveRoots, resolve_path};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long an answer is trusted before it is checked again (files appear when agents write them).
const FOUND_TTL: Duration = Duration::from_secs(30);
const MISSING_TTL: Duration = Duration::from_secs(8);
/// Candidates resolved per worker batch, and entries kept before the cache starts over.
const BATCH: usize = 200;
const MAX_ENTRIES: usize = 4096;

#[derive(Default)]
struct Entry {
    /// The canonical file, when the candidate is one.
    file: Option<PathBuf>,
    checked: Option<Instant>,
    queued: bool,
}

#[derive(Default)]
struct Shared {
    entries: HashMap<String, Entry>,
    queue: Vec<String>,
    busy: bool,
}

/// One per terminal pane (its roots don't change).
pub struct FileCache {
    roots: ResolveRoots,
    shared: Arc<Mutex<Shared>>,
}

/// A locked view for one render: answer lookups, then [`Lookup::finish`] starts a worker for what's unknown.
pub struct Lookup<'a> {
    cache: &'a FileCache,
    shared: parking_lot::MutexGuard<'a, Shared>,
}

impl FileCache {
    pub fn new(roots: ResolveRoots) -> FileCache {
        FileCache { roots, shared: Arc::default() }
    }

    pub fn roots(&self) -> &ResolveRoots {
        &self.roots
    }

    pub fn lookup(&self) -> Lookup<'_> {
        Lookup { cache: self, shared: self.shared.lock() }
    }

    /// Resolve `paths` now (the worker's body; tests call it directly).
    fn resolve_batch(roots: &ResolveRoots, paths: &[String]) -> Vec<(String, Option<PathBuf>)> {
        paths
            .iter()
            .map(|path| {
                let file = crate::services::guard("file_links::resolve", || resolve_path(roots, path))
                    .ok()
                    .and_then(Result::ok)
                    .filter(|r| r.kind == Kind::File)
                    .map(|r| r.path);
                (path.clone(), file)
            })
            .collect()
    }

    fn store(shared: &Mutex<Shared>, results: Vec<(String, Option<PathBuf>)>) {
        let mut s = shared.lock();
        let now = Instant::now();
        for (path, file) in results {
            let e = s.entries.entry(path).or_default();
            e.file = file;
            e.checked = Some(now);
            e.queued = false;
        }
        s.busy = false;
    }

    /// Tests: resolve everything queued on this thread.
    #[cfg(test)]
    pub fn resolve_queued_now(&self) {
        let queue = std::mem::take(&mut self.shared.lock().queue);
        let results = Self::resolve_batch(&self.roots, &queue);
        Self::store(&self.shared, results);
    }
}

impl Lookup<'_> {
    /// The link target for a confirmed file (with the candidate's line/column), queueing unknown or stale ones.
    pub fn get(&mut self, m: &FileLinkMatch) -> Option<Arc<str>> {
        if self.shared.entries.len() >= MAX_ENTRIES && !self.shared.busy {
            self.shared.entries.clear();
            self.shared.queue.clear();
        }
        let s = &mut *self.shared;
        let e = s.entries.entry(m.path.clone()).or_default();
        let ttl = if e.file.is_some() { FOUND_TTL } else { MISSING_TTL };
        if !e.queued && e.checked.is_none_or(|at| at.elapsed() >= ttl) {
            e.queued = true;
            s.queue.push(m.path.clone());
        }
        let file = e.file.as_deref()?;
        super::links::file_uri(file, m.line, m.column).map(Arc::from)
    }

    /// Resolve what's queued on a worker; it wakes the pane when the answers are in.
    pub fn finish(mut self, waker: Option<Waker>) {
        if self.shared.busy || self.shared.queue.is_empty() || cfg!(test) {
            return;
        }
        let Some(waker) = waker else { return };
        let take = self.shared.queue.len().min(BATCH);
        let batch: Vec<String> = self.shared.queue.drain(..take).collect();
        self.shared.busy = true;
        let (roots, shared) = (self.cache.roots.clone(), self.cache.shared.clone());
        drop(self);
        std::thread::spawn(move || {
            let results = FileCache::resolve_batch(&roots, &batch);
            FileCache::store(&shared, results);
            // re-render with the answers (and pick up whatever queued meanwhile)
            waker.wake();
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(path: &str, line: Option<u32>) -> FileLinkMatch {
        FileLinkMatch { start: 0, end: path.len(), path: path.into(), line, column: None, label: path.into() }
    }

    #[test]
    fn unknown_candidates_queue_and_only_real_files_link() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/app.rs"), "fn main() {}\n").unwrap();
        let cache = FileCache::new(ResolveRoots { cwd: dir.path().to_path_buf(), project: None });
        let mut l = cache.lookup();
        assert!(l.get(&candidate("src/app.rs", Some(3))).is_none(), "unknown until resolved");
        assert!(l.get(&candidate("src/missing.rs", None)).is_none());
        assert!(l.get(&candidate("src", None)).is_none());
        // asking twice queues once
        assert!(l.get(&candidate("src/app.rs", Some(3))).is_none());
        drop(l);
        assert_eq!(cache.shared.lock().queue.len(), 3);
        cache.resolve_queued_now();
        let mut l = cache.lookup();
        let uri = l.get(&candidate("src/app.rs", Some(3))).expect("a file links");
        let (path, line, _) = super::super::links::parse_file_uri(&uri).unwrap();
        assert!(path.ends_with("app.rs"));
        assert_eq!(line, Some(3));
        assert!(l.get(&candidate("src/missing.rs", None)).is_none());
        assert!(l.get(&candidate("src", None)).is_none(), "directories aren't linked");
        drop(l);
        assert!(cache.shared.lock().queue.is_empty(), "fresh answers aren't re-queued");
    }

    #[test]
    fn stale_files_keep_their_link_while_rechecked() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("README.md"), "# hi\n").unwrap();
        let cache = FileCache::new(ResolveRoots { cwd: dir.path().to_path_buf(), project: None });
        drop(cache.lookup().get(&candidate("README.md", None)));
        cache.resolve_queued_now();
        cache.shared.lock().entries.get_mut("README.md").unwrap().checked = Some(Instant::now() - FOUND_TTL);
        assert!(cache.lookup().get(&candidate("README.md", None)).is_some(), "still linked while re-checking");
        assert_eq!(cache.shared.lock().queue, vec!["README.md".to_string()]);
    }
}
