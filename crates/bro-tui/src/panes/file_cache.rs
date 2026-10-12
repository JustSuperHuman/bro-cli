//! Which path-like words on a terminal screen are real files. Rendering asks the cache (never the disk); unknown
//! and stale candidates are resolved in a batch on a worker thread (`bro_bridge::file_preview::resolve_path`:
//! cwd, project root, git `a/`/`b/` prefixes, a unique suffix in the project index), which wakes the pane
//! when it's done. A confirmed file keeps its link while it is re-checked, so links don't flicker.
//!
//! Unlike the bridge (remote clients stay inside the workspace), bro runs on your own machine: absolute paths
//! anywhere on disk link too, folders link (a click opens them in the system file manager), and a bare
//! `name.png` is also looked for in the folders other absolute paths on screen pointed at.

use crate::pane::Waker;
use bro_bridge::file_links::FileLinkMatch;
use bro_bridge::file_preview::{Kind, ResolveRoots, Resolved, resolve_path};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long an answer is trusted before it is checked again (files appear when agents write them).
const FOUND_TTL: Duration = Duration::from_secs(30);
const MISSING_TTL: Duration = Duration::from_secs(8);
/// Candidates resolved per worker batch, and entries kept before the cache starts over.
const BATCH: usize = 200;
const MAX_ENTRIES: usize = 4096;
/// Folders remembered from absolute paths, for bare file names printed after them.
const MAX_DIRS: usize = 8;

/// [`resolve_path`], then (outside the session's roots) an absolute or `~/` path straight from the disk.
/// Blocking.
pub fn resolve_local(roots: &ResolveRoots, raw: &str) -> Result<Resolved, String> {
    let confined = resolve_path(roots, raw);
    if confined.is_ok() {
        return confined;
    }
    let text = raw.trim();
    // network shares can hang a lookup
    if text.starts_with(r"\\") || text.starts_with("//") || text.chars().any(char::is_control) {
        return confined;
    }
    let path = match text.strip_prefix("~/").or_else(|| text.strip_prefix("~\\")) {
        Some(rest) => bro_core::util::home_dir().join(rest),
        None => PathBuf::from(text),
    };
    if !path.is_absolute() {
        return confined;
    }
    on_disk(&path).ok_or(()).or(confined)
}

fn on_disk(path: &Path) -> Option<Resolved> {
    let canonical = crate::util::strip_verbatim(path.canonicalize().ok()?);
    let kind = if canonical.is_dir() { Kind::Dir } else { Kind::File };
    Some(Resolved { relative: canonical.to_string_lossy().replace('\\', "/"), path: canonical, kind })
}

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
    /// Newest first.
    dirs: Vec<PathBuf>,
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

    /// Resolve `paths` now (the worker's body; tests call it directly). Absolute paths go first so a bare name
    /// in the same batch can be found in the folder they name; `dirs` collects those folders.
    fn resolve_batch(roots: &ResolveRoots, dirs: &mut Vec<PathBuf>, paths: &[String]) -> Vec<(String, Option<PathBuf>)> {
        let mut order: Vec<&String> = paths.iter().collect();
        order.sort_by_key(|p| !Path::new(p.as_str()).is_absolute());
        order
            .into_iter()
            .map(|path| {
                let found = crate::services::guard("file_links::resolve", || resolve_local(roots, path).ok().or_else(|| Self::near(dirs, path)))
                    .ok()
                    .flatten();
                if let Some(r) = &found
                    && Path::new(path).is_absolute()
                {
                    let dir = if r.kind == Kind::Dir { Some(r.path.clone()) } else { r.path.parent().map(Path::to_path_buf) };
                    if let Some(dir) = dir {
                        dirs.retain(|d| *d != dir);
                        dirs.insert(0, dir);
                        dirs.truncate(MAX_DIRS);
                    }
                }
                (path.clone(), found.map(|r| r.path))
            })
            .collect()
    }

    /// A plain relative name inside one of the remembered folders.
    fn near(dirs: &[PathBuf], path: &str) -> Option<Resolved> {
        let relative = Path::new(path);
        if relative.is_absolute() || relative.has_root() || relative.components().any(|c| !matches!(c, Component::Normal(_))) {
            return None;
        }
        dirs.iter().find_map(|dir| on_disk(&dir.join(relative)))
    }

    fn store(shared: &Mutex<Shared>, dirs: Vec<PathBuf>, results: Vec<(String, Option<PathBuf>)>) {
        let mut s = shared.lock();
        s.dirs = dirs;
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
        let (queue, mut dirs) = {
            let mut s = self.shared.lock();
            (std::mem::take(&mut s.queue), s.dirs.clone())
        };
        let results = Self::resolve_batch(&self.roots, &mut dirs, &queue);
        Self::store(&self.shared, dirs, results);
    }
}

impl Lookup<'_> {
    /// The link target for a confirmed file or folder (with the candidate's line/column), queueing unknown or stale ones.
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
        let mut dirs = self.shared.dirs.clone();
        self.shared.busy = true;
        let (roots, shared) = (self.cache.roots.clone(), self.cache.shared.clone());
        drop(self);
        std::thread::spawn(move || {
            let results = FileCache::resolve_batch(&roots, &mut dirs, &batch);
            FileCache::store(&shared, dirs, results);
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
        assert!(l.get(&candidate("src", None)).is_some(), "folders link too");
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

    #[test]
    fn paths_outside_the_workspace_link_and_bare_names_find_their_folder() {
        let (session, elsewhere) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let folder = elsewhere.path().join("Zapier");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("banner-1x1.png"), b"png").unwrap();
        let cache = FileCache::new(ResolveRoots { cwd: session.path().to_path_buf(), project: None });
        let text = format!("Other candidates are in {}{} (files 2026*). It stays at banner-1x1.png.", folder.display(), std::path::MAIN_SEPARATOR);
        let found = bro_bridge::file_links::find_file_links(&text);
        assert_eq!(found.len(), 2, "{found:?}");
        // queued bare name first: absolute paths still resolve first within a batch
        for m in found.iter().rev() {
            drop(cache.lookup().get(m));
        }
        cache.resolve_queued_now();
        let mut l = cache.lookup();
        let dir = l.get(&found[0]).expect("an absolute folder outside the session links");
        assert!(super::super::links::parse_file_uri(&dir).unwrap().0.ends_with("Zapier"));
        let file = l.get(&found[1]).expect("a bare name next to it links");
        assert!(super::super::links::parse_file_uri(&file).unwrap().0.ends_with("banner-1x1.png"));
        drop(l);
        assert!(FileCache::near(&[folder], "../Zapier/banner-1x1.png").is_none(), "no climbing out of a folder");
    }
}
