//! `GET /api/folders?q=` — where a remote client can start a session, the way
//! bro's own launcher offers it:
//!
//! * no query: the projects open in bro, then the folders its sessions ran in
//!   ([`crate::Bridge::set_known_folders`]), then the bridge's recent projects;
//! * a path ("F:\x", "~/code/x", "/home/me/x"): that folder when it exists,
//!   then the folders inside its parent that start with the last segment —
//!   end it with a separator to list a folder's children (all of them, up to
//!   [`MAX_CHILDREN`]), the way a phone browses the computer: the answer says
//!   which folder that is (`in`), its `parent` and how many it holds (`total`);
//! * anything else: names matched like spoken ones ("just gains" finds
//!   `J:\JustGains`), see [`crate::orchestrator::folders`].
//!
//! Every answer names the places to start browsing from (`roots`: the drives,
//! or `/`). Every folder listed exists on this machine.

use crate::orchestrator::folders::{drive_roots, find_limited, home_dir, leaf};
use crate::state::AppState;
use serde::Serialize;
use serde_json::{Value, json};
use std::path::{MAIN_SEPARATOR, Path, PathBuf};

/// Rows a response lists at most.
const MAX_FOLDERS: usize = 40;
/// Rows a folder's listing (a path ending in a separator) holds at most: every child of any folder people browse.
const MAX_CHILDREN: usize = 1000;

#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Folder {
    pub name: String,
    pub path: String,
    /// project | known | recent | drive | home | typed | completion
    pub source: &'static str,
    /// A suggestion's icon, hue and tone, as for projects ([`crate::project_icons`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon_tone: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hue: Option<u16>,
}

fn folder(path: &str, source: &'static str) -> Folder {
    Folder { name: leaf(path), path: path.to_owned(), source, ..Folder::default() }
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

/// What a path answers: the folders, the folder they're in, and how many matched before the cap.
struct Listing {
    folders: Vec<Folder>,
    /// The folder whose children are listed, when it exists.
    dir: Option<PathBuf>,
    total: usize,
}

/// The typed folder, then its parent's children starting with the last segment
/// (case-insensitively; dot folders only when asked for), then containing it.
/// Ending in a separator lists every child of that folder.
fn complete(query: &str) -> Listing {
    let typed = expand(query);
    let ends_open = query.ends_with(['\\', '/']) || (query.len() == 2 && query.ends_with(':'));
    let (parent, prefix) = if ends_open {
        // "F:\code\" is the folder "F:\code" (a root keeps its separator)
        (typed.components().collect::<PathBuf>(), String::new())
    } else {
        let prefix = typed.file_name().map(|name| name.to_string_lossy().to_lowercase()).unwrap_or_default();
        (typed.parent().map(Path::to_path_buf).unwrap_or_default(), prefix)
    };
    let mut out = Vec::new();
    if !ends_open && typed.is_dir() {
        out.push(folder(&path_text(&typed), "typed"));
    }
    let Ok(entries) = std::fs::read_dir(&parent) else { return Listing { folders: out, dir: None, total: 0 } };
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
    let total = starts.len() + contains.len();
    out.extend(starts);
    out.extend(contains);
    out.truncate(if ends_open { MAX_CHILDREN } else { MAX_FOLDERS });
    Listing { folders: out, dir: Some(parent), total }
}

/// Where browsing the computer starts: each drive (Windows), else `/`. Looked up again after a minute (a drive
/// plugged in shows up).
fn roots() -> Vec<Folder> {
    use parking_lot::Mutex;
    use std::sync::OnceLock;
    use std::time::{Duration, Instant};
    static CACHE: OnceLock<Mutex<Option<(Instant, Vec<Folder>)>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Mutex::default);
    if let Some((at, roots)) = cache.lock().as_ref()
        && at.elapsed() < Duration::from_secs(60)
    {
        return roots.clone();
    }
    let found: Vec<Folder> = drive_roots()
        .iter()
        .map(|root| {
            let path = path_text(root);
            let name = path.trim_end_matches(['\\', '/']).to_owned();
            Folder { name: if name.is_empty() { path.clone() } else { name }, path, source: "drive", ..Folder::default() }
        })
        .collect();
    *cache.lock() = Some((Instant::now(), found.clone()));
    found
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
    // each with the icon bro's window shows for it, else its letter's hue
    for entry in &mut out {
        let look = state.icons.look(&entry.path);
        (entry.icon, entry.icon_tone, entry.hue) = (look.icon, look.icon_tone, look.hue);
    }
    out
}

/// The response body for `query` (blocking: walks the file system).
pub(crate) fn search(state: &AppState, query: &str) -> Value {
    let query = query.trim().trim_matches('"');
    let mut listed: Option<(PathBuf, usize)> = None;
    let (mode, mut folders) = if query.is_empty() {
        ("suggest", suggestions(state))
    } else if is_path(query) {
        let listing = complete(query);
        listed = listing.dir.filter(|dir| dir.is_dir()).map(|dir| (dir, listing.total));
        ("path", listing.folders)
    } else {
        let found = find_limited(state, query, MAX_FOLDERS)
            .into_iter()
            .map(|found| Folder { name: found.name, path: found.path, source: found.source, ..Folder::default() })
            .collect();
        ("search", found)
    };
    if mode != "path" {
        folders.truncate(MAX_FOLDERS);
    }
    let mut body = json!({
        "query": query,
        "mode": mode,
        "home": home_dir().map(|home| path_text(&home)),
        "separator": MAIN_SEPARATOR.to_string(),
        "folders": folders,
        "roots": roots(),
    });
    if let Some((dir, total)) = listed {
        let path = path_text(&dir);
        let name = leaf(&path);
        body["in"] = json!({ "name": name.trim_end_matches(['\\', '/']), "path": path });
        body["parent"] = json!(dir.parent().map(path_text));
        body["total"] = json!(total);
    }
    body
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
        let names = |found: Listing| found.folders.into_iter().map(|f| (f.name, f.source)).collect::<Vec<_>>();

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
    fn a_folder_lists_every_child_and_a_prefix_only_the_first_few() {
        let root = tempfile::tempdir().unwrap();
        for n in 0..(MAX_FOLDERS + 25) {
            std::fs::create_dir(root.path().join(format!("app-{n:03}"))).unwrap();
        }
        let base = path_text(root.path());
        let open = complete(&format!("{base}{MAIN_SEPARATOR}"));
        assert_eq!(open.folders.len(), MAX_FOLDERS + 25, "browsing a folder shows all of it");
        assert_eq!(open.total, MAX_FOLDERS + 25);
        assert_eq!(open.dir.as_deref(), Some(root.path()));
        let typed = complete(&format!("{base}{MAIN_SEPARATOR}app"));
        assert_eq!(typed.folders.len(), MAX_FOLDERS, "completing a name stays short");
        assert_eq!(typed.total, MAX_FOLDERS + 25);
    }

    #[test]
    fn roots_are_where_browsing_starts() {
        let found = roots();
        assert!(!found.is_empty());
        assert!(found.iter().all(|root| Path::new(&root.path).is_dir() && !root.name.is_empty()), "{found:?}");
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
