//! Spoken project names to directories: "open a session in justgains" has to
//! land in `J:\justgains`. Candidates are the bridge's projects and recents,
//! the folders bro reports ([`crate::Bridge::set_known_folders`]), and the
//! top-level folders of every drive and of the home directory.

use crate::state::AppState;
use parking_lot::Mutex;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

const SCAN_TTL: Duration = Duration::from_secs(120);
const MAX_MATCHES: usize = 8;

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FolderMatch {
    pub name: String,
    pub path: String,
    /// Where the candidate came from: project, recent, known, drive, home
    pub source: &'static str,
    pub score: u32,
}

fn leaf(path: &str) -> String {
    Path::new(path.trim_end_matches(['\\', '/']))
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

/// Letters and digits only, lowercased: "Just Gains", "just-gains" and
/// "JustGains" all compare equal.
fn squash(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// 0 = no match. Exact beats prefix beats substring beats subsequence;
/// a shorter name wins ties so "justgains" prefers `justgains` over
/// `justgains-old`.
fn score(query: &str, name: &str) -> u32 {
    let (q, n) = (squash(query), squash(name));
    if q.is_empty() || n.is_empty() {
        return 0;
    }
    let length_bonus = 50u32.saturating_sub(n.len() as u32);
    if n == q {
        return 1000;
    }
    if n.starts_with(&q) {
        return 700 + length_bonus;
    }
    if n.contains(&q) {
        return 500 + length_bonus;
    }
    if q.contains(&n) && n.len() >= 4 {
        return 300 + length_bonus;
    }
    let mut chars = n.chars();
    if q.chars().all(|c| chars.any(|m| m == c)) && q.len() >= 3 {
        return 100 + length_bonus;
    }
    0
}

fn child_dirs(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            !name.starts_with(['.', '$']) && !name.eq_ignore_ascii_case("System Volume Information")
        })
        .map(|entry| entry.path())
        .collect()
}

fn drive_roots() -> Vec<PathBuf> {
    if cfg!(windows) {
        (b'C'..=b'Z')
            .map(|letter| PathBuf::from(format!("{}:\\", letter as char)))
            .filter(|root| root.is_dir())
            .collect()
    } else {
        vec![PathBuf::from("/")]
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .filter(|home| !home.is_empty())
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

/// Top-level folders of each drive and the home directory; rescanned at most
/// every two minutes.
type Scan = Vec<(PathBuf, &'static str)>;

fn scanned() -> Scan {
    static CACHE: OnceLock<Mutex<Option<(Instant, Scan)>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(None));
    if let Some((at, dirs)) = cache.lock().as_ref()
        && at.elapsed() < SCAN_TTL
    {
        return dirs.clone();
    }
    let mut dirs: Scan = Vec::new();
    for root in drive_roots() {
        dirs.extend(child_dirs(&root).into_iter().map(|dir| (dir, "drive")));
    }
    if let Some(home) = home_dir() {
        dirs.extend(child_dirs(&home).into_iter().map(|dir| (dir, "home")));
        for nested in ["source\\repos", "src", "code", "projects", "dev"] {
            dirs.extend(child_dirs(&home.join(nested)).into_iter().map(|dir| (dir, "home")));
        }
    }
    *cache.lock() = Some((Instant::now(), dirs.clone()));
    dirs
}

/// Best matches for `query`, highest score first, one entry per directory.
/// A query that already is a directory is returned as the only match.
pub fn find(app: &AppState, query: &str) -> Vec<FolderMatch> {
    let query = query.trim().trim_matches('"');
    let direct = Path::new(query);
    if direct.is_absolute() && direct.is_dir() {
        return vec![FolderMatch {
            name: leaf(query),
            path: query.to_string(),
            source: "path",
            score: 2000,
        }];
    }
    let mut candidates: Vec<(String, String, &'static str)> = Vec::new();
    for project in app.projects() {
        candidates.push((project.name.clone(), project.cwd.clone(), "project"));
        candidates.push((leaf(&project.cwd), project.cwd, "project"));
    }
    for recent in app.recent_projects() {
        candidates.push((recent.name.clone(), recent.cwd.clone(), "recent"));
        candidates.push((leaf(&recent.cwd), recent.cwd, "recent"));
    }
    for folder in app.known_folders() {
        candidates.push((leaf(&folder), folder, "known"));
    }
    for (dir, source) in scanned() {
        let path = dir.to_string_lossy().into_owned();
        candidates.push((leaf(&path), path, source));
    }
    // Places the user has worked in rank above folders that merely exist.
    let source_bonus = |source: &str| match source {
        "project" => 30,
        "known" | "recent" => 20,
        _ => 0,
    };
    let mut matches: Vec<FolderMatch> = Vec::new();
    for (name, path, source) in candidates {
        let base = score(query, &name);
        if base == 0 {
            continue;
        }
        let total = base + source_bonus(source);
        let key = crate::projects::cwd_key(&path);
        match matches
            .iter_mut()
            .find(|found| crate::projects::cwd_key(&found.path) == key)
        {
            Some(found) if found.score >= total => {}
            Some(found) => *found = FolderMatch { name, path, source, score: total },
            None => matches.push(FolderMatch { name, path, source, score: total }),
        }
    }
    matches.retain(|found| Path::new(&found.path).is_dir());
    // Equal names on several drives (an old clone and the live one): the
    // folder touched most recently is the one being worked in.
    let modified = |path: &str| std::fs::metadata(path).and_then(|meta| meta.modified()).ok();
    matches.sort_by_cached_key(|found| {
        (std::cmp::Reverse(found.score), std::cmp::Reverse(modified(&found.path)), found.path.len())
    });
    matches.truncate(MAX_MATCHES);
    matches
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spoken_names_match_folder_spellings() {
        assert_eq!(score("justgains", "justgains"), 1000);
        assert_eq!(score("Just Gains", "JustGains"), 1000);
        assert!(score("justgains", "JustGains-Mobile") > score("justgains", "gains"));
        assert!(score("bro", "bro-cli-v2") > score("bro", "abroad"));
        assert!(score("bro cli v2", "bro-cli-v2") == 1000);
        assert_eq!(score("zzz", "justgains"), 0);
        assert!(score("amazing ads", "AmazingAds") > 0);
    }

    #[test]
    fn leaf_is_the_last_component() {
        assert_eq!(leaf(r"J:\justgains\"), "justgains");
        assert_eq!(leaf("/home/me/proj"), "proj");
    }
}
