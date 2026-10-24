//! Project identity used to group the sidebar.
use crate::util::normalize_path;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ProjectKey {
    /// git root if inside a repo (walk up for `.git`), else the cwd itself.
    /// Absolute, no trailing separator.
    pub root: PathBuf,
    /// Normalized `root` for equality/hash (lowercase + forward slashes on Windows).
    pub key: String,
    /// Display name: last path component (drive letter for roots, e.g. "F:").
    pub name: String,
}

fn cache() -> &'static Mutex<HashMap<PathBuf, ProjectKey>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, ProjectKey>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// Cached per path — cheap enough to call per frame.
pub fn project_for(cwd: &Path) -> ProjectKey {
    if let Some(hit) = cache().lock().get(cwd) {
        return hit.clone();
    }
    let key = compute(cwd);
    cache().lock().insert(cwd.to_path_buf(), key.clone());
    key
}

/// Forget cached answers (e.g. after `git init` in a directory).
pub fn clear_cache() {
    cache().lock().clear();
}

/// The project key for an already-known root, without the `.git` walk.
pub fn key_for_root(root: &Path) -> ProjectKey {
    let root = trim_trailing(&normalize_path(root));
    ProjectKey { key: normalized_key(&root), name: display_name(&root), root }
}

fn compute(cwd: &Path) -> ProjectKey {
    let abs = normalize_path(cwd);
    let root = git_root(&abs).unwrap_or(abs);
    key_for_root(&root)
}

/// The repository `cwd` belongs to: [`project_for`], except that a linked worktree (`git worktree add`) resolves to
/// its main checkout, so every worktree of a repo is one repository. A submodule stays its own.
pub fn repository_for(cwd: &Path) -> ProjectKey {
    let project = project_for(cwd);
    main_checkout(&project.root).map_or(project, |root| key_for_root(&root))
}

/// Behind a linked worktree at `root`: its `.git` file names `<repo>/.git/worktrees/<name>`, whose `commondir`
/// leads back to `<repo>/.git`. A bare repository's worktrees resolve to the bare repository.
fn main_checkout(root: &Path) -> Option<PathBuf> {
    // a directory doesn't read as text: a main checkout, nothing to resolve
    let text = std::fs::read_to_string(root.join(".git")).ok()?;
    let gitdir = PathBuf::from(text.trim().strip_prefix("gitdir:")?.trim());
    let gitdir = if gitdir.is_absolute() { gitdir } else { root.join(gitdir) };
    let common = PathBuf::from(std::fs::read_to_string(gitdir.join("commondir")).ok()?.trim());
    let common = normalize_path(&if common.is_absolute() { common } else { gitdir.join(common) });
    Some(if common.file_name()? == ".git" { common.parent()?.to_path_buf() } else { common })
}

/// Nearest ancestor (inclusive) containing a `.git` file or directory.
pub fn git_root(start: &Path) -> Option<PathBuf> {
    start.ancestors().find(|dir| dir.join(".git").exists()).map(Path::to_path_buf)
}

fn trim_trailing(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    let trimmed = s.trim_end_matches(['/', '\\']);
    // Keep a bare root intact ("/" or "C:\" -> "C:").
    if trimmed.is_empty() { PathBuf::from("/") } else { PathBuf::from(trimmed) }
}

fn normalized_key(root: &Path) -> String {
    let mut s = root.to_string_lossy().replace('\\', "/");
    while s.len() > 1 && s.ends_with('/') {
        s.pop();
    }
    if cfg!(windows) { s.to_lowercase() } else { s }
}

fn display_name(root: &Path) -> String {
    if let Some(name) = root.file_name() {
        return name.to_string_lossy().into_owned();
    }
    let s = root.to_string_lossy();
    let s = s.trim_end_matches(['/', '\\']);
    if s.is_empty() { "/".into() } else { s.to_string() }
}

/// The checked-out branch of the repo at `root` (short commit id when detached; None outside git).
/// Reads `.git/HEAD` directly (worktrees' `.git` files included); cached for a few seconds, so it's cheap
/// enough to call while drawing.
pub fn git_branch(root: &Path) -> Option<String> {
    type BranchCache = Mutex<HashMap<PathBuf, (std::time::Instant, Option<String>)>>;
    static CACHE: OnceLock<BranchCache> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some((at, b)) = cache.lock().get(root)
        && at.elapsed() < std::time::Duration::from_secs(4)
    {
        return b.clone();
    }
    let b = read_branch(root);
    cache.lock().insert(root.to_path_buf(), (std::time::Instant::now(), b.clone()));
    b
}

fn read_branch(root: &Path) -> Option<String> {
    let dot = root.join(".git");
    let git_dir = if dot.is_dir() {
        dot
    } else {
        // a worktree / submodule: ".git" is a file saying "gitdir: <path>"
        let text = std::fs::read_to_string(&dot).ok()?;
        let p = PathBuf::from(text.trim().strip_prefix("gitdir:")?.trim());
        if p.is_absolute() { p } else { root.join(p) }
    };
    let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let head = head.trim();
    match head.strip_prefix("ref:") {
        Some(r) => Some(r.trim().trim_start_matches("refs/heads/").to_string()),
        None => Some(head.chars().take(7).collect()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_root_and_plain_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("MyRepo");
        let deep = repo.join("src").join("deep");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        let k = project_for(&deep);
        assert_eq!(k.name, "MyRepo");
        assert!(crate::util::same_path(&k.root, &repo));
        assert_eq!(k, project_for(&repo));

        // A worktree / submodule has a .git *file*.
        let wt = tmp.path().join("wt");
        std::fs::create_dir_all(wt.join("a")).unwrap();
        std::fs::write(wt.join(".git"), "gitdir: elsewhere").unwrap();
        assert_eq!(project_for(&wt.join("a")).name, "wt");

        let plain = tmp.path().join("plain").join("sub");
        std::fs::create_dir_all(&plain).unwrap();
        let p = project_for(&plain);
        assert_eq!(p.name, "sub");
        assert!(!p.key.ends_with('/'));
        if cfg!(windows) {
            assert_eq!(p.key, p.key.to_lowercase());
            assert!(!p.key.contains('\\'));
        }
    }

    #[test]
    fn worktrees_belong_to_their_repository_and_submodules_to_themselves() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("app");
        let admin = repo.join(".git").join("worktrees").join("feature");
        std::fs::create_dir_all(&admin).unwrap();
        std::fs::write(admin.join("commondir"), "../..\n").unwrap();
        // Claude Code's own worktrees sit inside the repo; git's anywhere
        for wt in [repo.join(".claude").join("worktrees").join("feature"), tmp.path().join("app-feature")] {
            std::fs::create_dir_all(wt.join("src")).unwrap();
            std::fs::write(wt.join(".git"), format!("gitdir: {}\n", admin.display())).unwrap();
            assert_eq!(project_for(&wt.join("src")).name, wt.file_name().unwrap().to_string_lossy(), "the sidebar keeps it apart");
            assert_eq!(repository_for(&wt.join("src")), project_for(&repo));
        }
        assert_eq!(repository_for(&repo.join("deep")), project_for(&repo));

        let sub = repo.join("vendor").join("lib");
        std::fs::create_dir_all(repo.join(".git").join("modules").join("lib")).unwrap();
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join(".git"), "gitdir: ../../.git/modules/lib\n").unwrap();
        assert_eq!(repository_for(&sub).name, "lib");

        let bare = tmp.path().join("store.git");
        let admin = bare.join("worktrees").join("main");
        std::fs::create_dir_all(&admin).unwrap();
        std::fs::write(admin.join("commondir"), "../..").unwrap();
        let wt = tmp.path().join("store-main");
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::write(wt.join(".git"), format!("gitdir: {}", admin.display())).unwrap();
        assert_eq!(repository_for(&wt), key_for_root(&bare));
    }

    #[test]
    fn drive_root_name() {
        if cfg!(windows) {
            let k = key_for_root(Path::new(r"F:\"));
            assert_eq!(k.name, "F:");
            assert_eq!(k.key, "f:");
        } else {
            assert_eq!(key_for_root(Path::new("/")).name, "/");
        }
    }

    #[test]
    fn branch_from_head_detached_and_worktree_file() {
        let d = tempfile::tempdir().unwrap();
        let repo = d.path().join("r");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::write(repo.join(".git").join("HEAD"), "ref: refs/heads/feature/x
").unwrap();
        assert_eq!(read_branch(&repo).as_deref(), Some("feature/x"));
        std::fs::write(repo.join(".git").join("HEAD"), "0123456789abcdef
").unwrap();
        assert_eq!(read_branch(&repo).as_deref(), Some("0123456"));
        let wt = d.path().join("wt");
        std::fs::create_dir_all(repo.join(".git").join("worktrees").join("wt")).unwrap();
        std::fs::write(repo.join(".git").join("worktrees").join("wt").join("HEAD"), "ref: refs/heads/wip
").unwrap();
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::write(wt.join(".git"), format!("gitdir: {}", repo.join(".git").join("worktrees").join("wt").display())).unwrap();
        assert_eq!(read_branch(&wt).as_deref(), Some("wip"));
        assert_eq!(read_branch(d.path()), None);
    }
}
