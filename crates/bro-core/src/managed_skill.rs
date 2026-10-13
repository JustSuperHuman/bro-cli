//! Agent skills bro downloads once into `~/.bro/<name>-skill` and copies into the Claude and Codex
//! homes it launches (JustTools, JustWatch). A copy bro wrote carries a marker file, so it can be
//! refreshed and removed again; a skill of the same name bro didn't write is never touched.
use crate::paths;
use std::path::{Path, PathBuf};

/// Marks a skill directory bro wrote.
const MARKER: &str = ".bro-managed";

/// `~/.bro/<name>-skill`: the downloaded skill.
pub fn cache_dir(name: &str) -> PathBuf {
    paths::bro_dir().join(format!("{name}-skill"))
}

pub fn skill_dir(home: &Path, name: &str) -> PathBuf {
    home.join("skills").join(name)
}

/// `home` holds a `name` skill bro didn't write (yours, e.g. from `npx skills add`).
pub fn has_own(home: &Path, name: &str) -> bool {
    let dir = skill_dir(home, name);
    std::fs::symlink_metadata(&dir).is_ok() && !dir.join(MARKER).is_file()
}

/// Copies the cached `files` of skill `name` under `home` (a no-op when unchanged). False when
/// nothing is cached yet, or `home` has a skill of that name of its own, which is left as it is.
pub fn write(home: &Path, name: &str, files: &[&str]) -> std::io::Result<bool> {
    let cache = cache_dir(name);
    if !cache.join(files[0]).is_file() || has_own(home, name) {
        return Ok(false);
    }
    let dir = skill_dir(home, name);
    for file in files {
        let Ok(body) = std::fs::read_to_string(cache.join(file)) else { continue };
        crate::browser::write_if_changed(&dir.join(file), &body)?;
    }
    crate::browser::write_if_changed(&dir.join(MARKER), "written by bro; turn it off in bro's settings\n")?;
    Ok(true)
}

/// Removes the copy bro wrote under `home`. A link is skipped (what it points at is removed
/// where it really lives); your own skill is never removed.
pub fn remove(home: &Path, name: &str) {
    let dir = skill_dir(home, name);
    let linked = std::fs::symlink_metadata(&dir).is_ok_and(|m| m.file_type().is_symlink());
    if !linked && dir.join(MARKER).is_file() {
        let _ = std::fs::remove_dir_all(&dir);
    }
}

fn subdirs(dir: PathBuf) -> impl Iterator<Item = PathBuf> {
    std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.is_dir())
}

/// Takes skill `name` back out of every Claude and Codex home bro launches.
pub fn remove_everywhere(name: &str) {
    let homes = [paths::claude_local_dir(), paths::codex_local_dir()].into_iter().chain(subdirs(paths::claude_accounts_dir())).chain(subdirs(paths::codex_profiles_dir()));
    for home in homes {
        remove(&home, name);
    }
}
