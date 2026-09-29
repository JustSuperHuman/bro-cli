// Backs `@file` completion for remote clients. Claude Code and Codex both take
// `@relative/path` references, but a phone has no view of the host filesystem —
// so the host indexes the session's working directory and answers fuzzy
// queries against it. Port of the Node host's `server/file-search.ts`.

use parking_lot::Mutex;
use serde::Serialize;
use std::collections::{BTreeSet, HashMap};
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

const CACHE_TTL: Duration = Duration::from_secs(45);
const MAX_ENTRIES: usize = 20_000;
const MAX_WALK_DEPTH: usize = 7;
const GIT_TIMEOUT: Duration = Duration::from_secs(5);
const GIT_MAX_BUFFER: usize = 16 * 1024 * 1024;

// Directories a repository walk should never descend into: build output and
// dependency trees dwarf the source and are never what an `@` mention means.
const SKIP_DIRECTORIES: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    ".idea",
    ".vs",
    ".vscode",
    ".next",
    ".nuxt",
    ".expo",
    ".gradle",
    ".venv",
    "__pycache__",
    "node_modules",
    "bower_components",
    "vendor",
    "dist",
    "build",
    "out",
    "target",
    "bin",
    "obj",
    "packages",
    "coverage",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum FileKind {
    File,
    Dir,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct FileHit {
    /// Relative to the session cwd, forward-slashed (both agents accept that).
    pub path: String,
    pub name: String,
    pub dir: String,
    pub kind: FileKind,
}

struct IndexEntry {
    hit: FileHit,
    lower: String,
    lower_name: String,
    depth: i64,
}

fn make_entry(relative: &str, kind: FileKind) -> IndexEntry {
    let posix = relative.replace('\\', "/");
    let normalized = posix.strip_prefix("./").unwrap_or(&posix).to_string();
    let (dir, name) = match normalized.rfind('/') {
        Some(slash) => (
            normalized[..slash].to_string(),
            normalized[slash + 1..].to_string(),
        ),
        None => (String::new(), normalized.clone()),
    };
    IndexEntry {
        lower: normalized.to_lowercase(),
        lower_name: name.to_lowercase(),
        depth: normalized.split('/').count() as i64,
        hit: FileHit {
            path: normalized,
            name,
            dir,
            kind,
        },
    }
}

#[cfg(windows)]
fn hide_window(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn hide_window(_command: &mut Command) {}

/// `git ls-files` for tracked + untracked-but-not-ignored files, bounded by a
/// timeout and an output cap. `None` when this is not a repository (or git is
/// missing), which falls back to a directory walk.
fn git_list_files(cwd: &str) -> Option<Vec<String>> {
    let mut command = Command::new("git");
    command
        .args([
            "-C",
            cwd,
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    hide_window(&mut command);
    let mut child = command.spawn().ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut buffer = Vec::new();
        let _ = (&mut stdout)
            .take(GIT_MAX_BUFFER as u64 + 1)
            .read_to_end(&mut buffer);
        buffer
    });

    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < GIT_TIMEOUT => {
                std::thread::sleep(Duration::from_millis(20))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    let buffer = reader.join().ok()?;
    if !status.success() || buffer.len() > GIT_MAX_BUFFER {
        return None;
    }
    Some(
        String::from_utf8_lossy(&buffer)
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect(),
    )
}

fn walk_directory(cwd: &Path) -> Vec<(String, FileKind)> {
    fn walk(directory: &Path, relative: &str, depth: usize, found: &mut Vec<(String, FileKind)>) {
        if depth > MAX_WALK_DEPTH || found.len() >= MAX_ENTRIES {
            return;
        }
        let Ok(entries) = std::fs::read_dir(directory) else {
            return;
        };
        for entry in entries.flatten() {
            if found.len() >= MAX_ENTRIES {
                return;
            }
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let name = entry.file_name().to_string_lossy().into_owned();
            let child = if relative.is_empty() {
                name.clone()
            } else {
                format!("{relative}/{name}")
            };
            if kind.is_dir() {
                if SKIP_DIRECTORIES.contains(&name.to_lowercase().as_str()) {
                    continue;
                }
                found.push((child.clone(), FileKind::Dir));
                walk(&entry.path(), &child, depth + 1, found);
            } else if kind.is_file() {
                found.push((child, FileKind::File));
            }
        }
    }

    let mut found = Vec::new();
    walk(cwd, "", 0, &mut found);
    found
}

fn build_index(cwd: &str) -> Vec<IndexEntry> {
    if let Some(tracked) = git_list_files(cwd).filter(|files| !files.is_empty()) {
        let mut entries: Vec<IndexEntry> = tracked
            .iter()
            .take(MAX_ENTRIES)
            .map(|relative| make_entry(relative, FileKind::File))
            .collect();
        // git lists files only; the directories they live in are just as
        // mentionable, so derive them.
        let mut directories = BTreeSet::new();
        for entry in &entries {
            let mut prefix = String::new();
            let segments: Vec<&str> = entry.hit.path.split('/').collect();
            for segment in &segments[..segments.len().saturating_sub(1)] {
                if !prefix.is_empty() {
                    prefix.push('/');
                }
                prefix.push_str(segment);
                directories.insert(prefix.clone());
            }
        }
        entries.extend(
            directories
                .iter()
                .map(|directory| make_entry(directory, FileKind::Dir)),
        );
        return entries;
    }

    walk_directory(Path::new(cwd))
        .into_iter()
        .map(|(relative, kind)| make_entry(&relative, kind))
        .collect()
}

fn get_index(cwd: &str) -> Arc<Vec<IndexEntry>> {
    type Cache = Mutex<HashMap<String, (Instant, Arc<Vec<IndexEntry>>)>>;
    static CACHE: OnceLock<Cache> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some((at, entries)) = cache.lock().get(cwd)
        && at.elapsed() < CACHE_TTL
    {
        return entries.clone();
    }
    let entries = Arc::new(build_index(cwd));
    let mut cache = cache.lock();
    cache.retain(|_, (at, _)| at.elapsed() < CACHE_TTL);
    cache.insert(cwd.to_string(), (Instant::now(), entries.clone()));
    entries
}

/// Ordered subsequence match, scored so tight runs beat scattered letters.
fn subsequence_score(haystack: &str, needle: &str) -> Option<i64> {
    let mut cursor = 0;
    let mut gaps: i64 = 0;
    let mut previous: Option<usize> = None;
    for character in needle.chars() {
        let found = cursor + haystack[cursor..].find(character)?;
        if let Some(previous) = previous {
            gaps += haystack[previous..found].chars().count() as i64 - 1;
        }
        previous = Some(found);
        cursor = found + character.len_utf8();
    }
    Some(300 - gaps.min(200))
}

fn char_index(text: &str, byte_index: usize) -> i64 {
    text[..byte_index].chars().count() as i64
}

fn score_entry(entry: &IndexEntry, query: &str) -> Option<i64> {
    let name_len = entry.lower_name.chars().count() as i64;
    let path_len = entry.lower.chars().count() as i64;
    if query.is_empty() {
        return Some(100 - entry.depth);
    }
    if entry.lower_name.starts_with(query) {
        return Some(1000 - name_len - entry.depth * 2);
    }
    if entry.lower.starts_with(query) {
        return Some(900 - path_len - entry.depth);
    }
    if let Some(index) = entry.lower_name.find(query) {
        return Some(800 - char_index(&entry.lower_name, index) * 3 - name_len - entry.depth * 2);
    }
    if let Some(index) = entry.lower.find(query) {
        return Some(650 - char_index(&entry.lower, index) - entry.depth * 2);
    }
    subsequence_score(&entry.lower, query).map(|score| score - entry.depth * 2)
}

fn rank(entries: &[IndexEntry], raw_query: &str, limit: usize) -> Vec<FileHit> {
    let trimmed = raw_query.trim();
    let query = trimmed
        .strip_prefix('@')
        .unwrap_or(trimmed)
        .replace('\\', "/")
        .to_lowercase();

    let mut ranked: Vec<(&IndexEntry, i64)> = entries
        .iter()
        .filter_map(|entry| score_entry(entry, &query).map(|score| (entry, score)))
        .collect();
    ranked.sort_by(|(a, score_a), (b, score_b)| {
        score_b
            .cmp(score_a)
            .then_with(|| a.hit.path.len().cmp(&b.hit.path.len()))
            .then_with(|| a.hit.path.cmp(&b.hit.path))
    });
    ranked
        .into_iter()
        .take(limit)
        .map(|(entry, _)| entry.hit.clone())
        .collect()
}

/// Fuzzy lookup under `cwd`, best matches first. Blocking: call off the async
/// runtime.
pub(crate) fn search_files(cwd: &str, query: &str, limit: usize) -> Vec<FileHit> {
    if cwd.is_empty() {
        return Vec::new();
    }
    rank(&get_index(cwd), query, limit)
}

/// The Node route's `limit` handling: 1..=100, default 30.
pub(crate) fn clamp_limit(raw: Option<&str>) -> usize {
    raw.and_then(|value| value.trim().parse::<f64>().ok())
        .filter(|value| value.is_finite())
        .map(|value| value.floor().clamp(1.0, 100.0) as usize)
        .unwrap_or(30)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn entries(paths: &[(&str, FileKind)]) -> Vec<IndexEntry> {
        paths
            .iter()
            .map(|(path, kind)| make_entry(path, *kind))
            .collect()
    }

    #[test]
    fn make_entry_splits_path() {
        let entry = make_entry("./src\\lib/api.ts", FileKind::File);
        assert_eq!(entry.hit.path, "src/lib/api.ts");
        assert_eq!(entry.hit.name, "api.ts");
        assert_eq!(entry.hit.dir, "src/lib");
        assert_eq!(entry.depth, 3);
    }

    #[test]
    fn ranks_name_prefix_above_substring_and_fuzzy() {
        let index = entries(&[
            ("docs/composer-notes.md", FileKind::File),
            ("src/components/Composer.tsx", FileKind::File),
            ("src/lib/composerApi.ts", FileKind::File),
            ("scripts/compile-many-other-things.sh", FileKind::File),
        ]);
        let hits = rank(&index, "@Compo", 10);
        // Both are name-prefix hits; the shorter name wins, as in Node.
        assert_eq!(hits[0].path, "src/components/Composer.tsx");
        assert_eq!(hits[1].path, "src/lib/composerApi.ts");
        assert!(
            hits.iter()
                .any(|hit| hit.path == "scripts/compile-many-other-things.sh")
        );
        assert!(rank(&index, "zzz", 10).is_empty());
        assert_eq!(rank(&index, "", 2).len(), 2);
    }

    #[test]
    fn subsequence_prefers_tight_runs() {
        assert!(
            subsequence_score("abcdef", "abc").unwrap()
                > subsequence_score("axbxcx", "abc").unwrap()
        );
        assert!(subsequence_score("abc", "cba").is_none());
    }

    #[test]
    fn walk_skips_dependency_directories() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("src/nested")).unwrap();
        fs::create_dir_all(root.path().join("node_modules/pkg")).unwrap();
        fs::write(root.path().join("src/nested/main.rs"), "").unwrap();
        fs::write(root.path().join("node_modules/pkg/index.js"), "").unwrap();
        fs::write(root.path().join("README.md"), "").unwrap();

        let found = walk_directory(root.path());
        let paths: Vec<_> = found.iter().map(|(path, _)| path.as_str()).collect();
        assert!(paths.contains(&"src/nested/main.rs"));
        assert!(paths.contains(&"src/nested"));
        assert!(paths.contains(&"README.md"));
        assert!(!paths.iter().any(|path| path.contains("node_modules")));

        // Not a git repository, so the search falls back to that walk.
        let hits = search_files(&root.path().to_string_lossy(), "main", 5);
        assert_eq!(hits[0].path, "src/nested/main.rs");
        assert_eq!(hits[0].kind, FileKind::File);
        let json = serde_json::to_value(&hits[0]).unwrap();
        assert_eq!(json["kind"], "file");
        assert_eq!(json["dir"], "src/nested");
    }

    #[test]
    fn limit_matches_node_route() {
        assert_eq!(clamp_limit(None), 30);
        assert_eq!(clamp_limit(Some("abc")), 30);
        assert_eq!(clamp_limit(Some("0")), 1);
        assert_eq!(clamp_limit(Some("40")), 40);
        assert_eq!(clamp_limit(Some("500")), 100);
    }
}
