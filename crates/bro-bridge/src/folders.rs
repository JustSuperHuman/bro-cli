//! `GET /api/folders?q=` — where a remote client can start a session, the way
//! bro's own launcher offers it:
//!
//! * no query: the projects open in bro, then the folders its sessions ran in
//!   ([`crate::Bridge::set_known_folders`]), then the bridge's recent projects;
//! * a path ("F:\x", "~/code/x", "/home/me/x"): that folder when it exists,
//!   then the folders inside its parent that start with the last segment —
//!   end it with a separator to list a folder's children;
//! * anything else: names matched like spoken ones ("just gains" finds
//!   `J:\JustGains`), see [`crate::orchestrator::folders`].
//!
//! Every folder listed exists on this machine.

use crate::orchestrator::folders::{find_limited, home_dir, leaf};
use crate::state::AppState;
use serde::Serialize;
use serde_json::{Value, json};
use std::path::{MAIN_SEPARATOR, Path, PathBuf};

/// Rows a response lists at most.
const MAX_FOLDERS: usize = 40;

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Folder {
    pub name: String,
    pub path: String,
    /// project | known | recent | drive | home | typed | completion
    pub source: &'static str,
}

fn folder(path: &str, source: &'static str) -> Folder {
    Folder { name: leaf(path), path: path.to_owned(), source }
}

/// Whether the query is a path rather than a name to look up.
fn is_path(query: &str) -> bool {
    query.starts_with('~')
        || query.contains(['\\', '/'])
        || (query.len() >= 2 && query.as_bytes()[1] == b':' && query.as_bytes()[0].is_ascii_alphabetic())
}

/// `~` / `~/x` against the home directory; "C:" alone means its root.
fn expand(query: &str) -> PathBuf {
    if let Some(rest) = query.strip_prefix('~')
        && let Some(home) = home_dir()
    {
        return home.join(rest.trim_start_matches(['\\', '/']));
    }
    if query.len() == 2 && query.ends_with(':') {
        return PathBuf::from(format!("{query}{MAIN_SEPARATOR}"));
    }
    PathBuf::from(query)
}

fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// The typed folder, then its parent's children starting with the last segment
/// (case-insensitively; dot folders only when asked for), then containing it.
fn complete(query: &str) -> Vec<Folder> {
    let typed = expand(query);
    let ends_open = query.ends_with(['\\', '/']) || (query.len() == 2 && query.ends_with(':'));
    let (parent, prefix) = if ends_open {
        (typed.clone(), String::new())
    } else {
        let prefix = typed.file_name().map(|name| name.to_string_lossy().to_lowercase()).unwrap_or_default();
        (typed.parent().map(Path::to_path_buf).unwrap_or_default(), prefix)
    };
    let mut out = Vec::new();
    if !ends_open && typed.is_dir() {
        out.push(folder(&path_text(&typed), "typed"));
    }
    let Ok(entries) = std::fs::read_dir(&parent) else { return out };
    let mut starts = Vec::new();
    let mut contains = Vec::new();
    for entry in entries.flatten() {
        if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let lower = name.to_lowercase();
        if (name.starts_with(['.', '$']) && !prefix.starts_with(['.', '$'])) || name.eq_ignore_ascii_case("System Volume Information") {
            continue;
        }
        if lower == prefix && !ends_open {
            continue;
        }
        let path = path_text(&entry.path());
        if lower.starts_with(&prefix) {
            starts.push(folder(&path, "completion"));
        } else if !prefix.is_empty() && lower.contains(&prefix) {
            contains.push(folder(&path, "completion"));
        }
    }
    let by_name = |left: &Folder, right: &Folder| left.name.to_lowercase().cmp(&right.name.to_lowercase());
    starts.sort_by(by_name);
    contains.sort_by(by_name);
    out.extend(starts);
    out.extend(contains);
    out
}

/// Projects open in bro, then where its sessions ran, then remembered projects.
fn suggestions(state: &AppState) -> Vec<Folder> {
    let mut out: Vec<Folder> = Vec::new();
    let mut push = |path: &str, name: Option<&str>, source: &'static str| {
        let key = crate::projects::cwd_key(path);
        if path.trim().is_empty() || out.iter().any(|found| crate::projects::cwd_key(&found.path) == key) {
            return;
        }
        if !Path::new(path).is_dir() {
            return;
        }
        let mut entry = folder(path, source);
        if let Some(name) = name.map(str::trim).filter(|name| !name.is_empty()) {
            entry.name = name.to_owned();
        }
        out.push(entry);
    };
    let projects = state.projects();
    for project in &projects {
        push(&project.cwd, Some(&project.name), "project");
    }
    for known in state.known_folders() {
        push(&known, None, "known");
    }
    for recent in state.recent_projects() {
        push(&recent.cwd, Some(&recent.name), "recent");
    }
    out
}

/// The response body for `query` (blocking: walks the file system).
pub(crate) fn search(state: &AppState, query: &str) -> Value {
    let query = query.trim().trim_matches('"');
    let (mode, mut folders) = if query.is_empty() {
        ("suggest", suggestions(state))
    } else if is_path(query) {
        ("path", complete(query))
    } else {
        let found = find_limited(state, query, MAX_FOLDERS)
            .into_iter()
            .map(|found| Folder { name: found.name, path: found.path, source: found.source })
            .collect();
        ("search", found)
    };
    folders.truncate(MAX_FOLDERS);
    json!({
        "query": query,
        "mode": mode,
        "home": home_dir().map(|home| path_text(&home)),
        "separator": MAIN_SEPARATOR.to_string(),
        "folders": folders,
    })
}

/// Why a session cannot start in `cwd`, if it cannot.
pub(crate) fn check_start_folder(cwd: &Path) -> Result<(), String> {
    match std::fs::metadata(cwd) {
        Ok(meta) if meta.is_dir() => Ok(()),
        Ok(_) => Err(format!("{} is a file, not a folder.", cwd.display())),
        Err(_) => Err(format!("There is no folder at {} on this computer.", cwd.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_told_apart_from_names() {
        for path in ["C:", "c:\\x", "~", "~/code", "/usr", "src/app", "..\\x"] {
            assert!(is_path(path), "{path}");
        }
        for name in ["justgains", "bro cli", "c", "just.gains"] {
            assert!(!is_path(name), "{name}");
        }
    }

    #[test]
    fn completion_lists_children_matching_the_last_segment() {
        let root = tempfile::tempdir().unwrap();
        for dir in ["Alpha", "alpine", "beta", ".hidden", "zalp"] {
            std::fs::create_dir(root.path().join(dir)).unwrap();
        }
        std::fs::write(root.path().join("alfile"), "x").unwrap();
        let base = path_text(root.path());
        let names = |found: Vec<Folder>| found.into_iter().map(|f| (f.name, f.source)).collect::<Vec<_>>();

        let open = complete(&format!("{base}{MAIN_SEPARATOR}"));
        assert_eq!(names(open), [("Alpha", "completion"), ("alpine", "completion"), ("beta", "completion"), ("zalp", "completion")].map(|(n, s)| (n.to_owned(), s)));

        let al = complete(&format!("{base}{MAIN_SEPARATOR}al"));
        assert_eq!(names(al), [("Alpha", "completion"), ("alpine", "completion"), ("zalp", "completion")].map(|(n, s)| (n.to_owned(), s)));

        let exact = complete(&format!("{base}{MAIN_SEPARATOR}beta"));
        assert_eq!(names(exact), [("beta".to_owned(), "typed")]);

        let dot = complete(&format!("{base}{MAIN_SEPARATOR}.h"));
        assert_eq!(names(dot), [(".hidden".to_owned(), "completion")]);
    }

    #[test]
    fn start_folders_must_exist() {
        let root = tempfile::tempdir().unwrap();
        assert!(check_start_folder(root.path()).is_ok());
        let file = root.path().join("f.txt");
        std::fs::write(&file, "x").unwrap();
        assert!(check_start_folder(&file).unwrap_err().contains("is a file"));
        assert!(check_start_folder(&root.path().join("nope")).unwrap_err().contains("no folder"));
    }
}
