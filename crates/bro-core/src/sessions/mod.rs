//! Past agent sessions on disk, for the sidebar's "resume" lists.
//! Claude: `<profile>/projects/<encoded>/<uuid>.jsonl` (v1 `describeTranscript` rules).
//! Codex: `<home>/sessions/YYYY/MM/DD/rollout-*.jsonl` (interactive only). Pi/omp: their
//! session dirs if present (`~/.pi/agent/sessions` or `$PI_CODING_AGENT_DIR/sessions`,
//! `~/.omp/agent/sessions`).
//!
//! Descriptions are cached per harness in `~/.bro/*-v2.cache.json` (v1's entry format,
//! separate files so v1 and v2 don't prune each other's entries — see [`cache`]).
mod cache;
pub mod describe;
mod stage;

pub use stage::{cleanup_staged, stage_for_profile};

use crate::util::{read_head, system_time_ms};
use crate::{Harness, paths, profiles, projects::ProjectKey};
use cache::{Cache, Entry};
use describe::Described;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: String,
    pub harness: Harness,
    /// Owning profile id ("claude:work"); None for pi/omp
    pub profile_id: Option<String>,
    pub path: PathBuf,
    pub cwd: Option<PathBuf>,
    pub project: Option<ProjectKey>,
    /// First real user prompt / summary, single line
    pub title: String,
    pub branch: Option<String>,
    pub modified: SystemTime,
    pub size: u64,
}

#[derive(Debug, Clone, Default)]
pub struct ListOpts {
    /// Max sessions described overall (newest first). 0 = default (400).
    pub limit: usize,
    /// Restrict to these harnesses (empty = all)
    pub harnesses: Vec<Harness>,
}

/// v1 `MAX_SESSIONS`.
pub const DEFAULT_LIMIT: usize = 400;
/// Files smaller than this hold no prompt (v1).
pub const MIN_SIZE: u64 = 512;
const CLAUDE_HEAD: usize = 128 * 1024;
const CODEX_HEAD: usize = 512 * 1024;
const PI_HEAD: usize = 128 * 1024;
const CODEX_MAX_DEPTH: usize = 4;
const PI_MAX_DEPTH: usize = 3;
const WORKERS: usize = 8;

/// Where one harness login keeps its session files.
#[derive(Debug, Clone)]
struct Source {
    harness: Harness,
    profile_id: Option<String>,
    root: PathBuf,
}

/// A session file found by the (cheap) stat pass.
#[derive(Debug, Clone)]
struct Stat {
    harness: Harness,
    profile_id: Option<String>,
    path: PathBuf,
    /// Claude: the encoded project directory name (sessions in it share a cwd)
    project_dir: Option<String>,
    modified: SystemTime,
    mtime_ms: f64,
    size: u64,
}

/// `~/.pi/agent/sessions` (or under `$PI_CODING_AGENT_DIR`).
pub fn pi_sessions_dir() -> PathBuf {
    paths::pi_agent_dir().join("sessions")
}

/// `~/.omp/agent/sessions`.
pub fn omp_sessions_dir() -> PathBuf {
    paths::omp_agent_dir().join("sessions")
}

fn sources(harnesses: &[Harness]) -> Vec<Source> {
    let want = |h: Harness| harnesses.is_empty() || harnesses.contains(&h);
    let mut out = Vec::new();
    if want(Harness::Claude) {
        out.extend(profiles::list_claude().into_iter().map(|p| Source {
            harness: Harness::Claude,
            profile_id: Some(p.id),
            root: p.dir.join("projects"),
        }));
    }
    if want(Harness::Codex) {
        out.extend(profiles::list_codex().into_iter().map(|p| Source {
            harness: Harness::Codex,
            profile_id: Some(p.id),
            root: p.dir.join("sessions"),
        }));
    }
    if want(Harness::Pi) {
        out.push(Source { harness: Harness::Pi, profile_id: None, root: pi_sessions_dir() });
    }
    if want(Harness::Omp) {
        out.push(Source { harness: Harness::Omp, profile_id: None, root: omp_sessions_dir() });
    }
    out
}

fn stat_file(src: &Source, path: PathBuf, project_dir: Option<String>) -> Option<Stat> {
    let meta = std::fs::metadata(&path).ok()?;
    if !meta.is_file() || meta.len() < MIN_SIZE {
        return None;
    }
    let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
    Some(Stat {
        harness: src.harness,
        profile_id: src.profile_id.clone(),
        path,
        project_dir,
        modified,
        mtime_ms: system_time_ms(modified),
        size: meta.len(),
    })
}

fn is_jsonl(p: &Path) -> bool {
    p.extension().is_some_and(|e| e == "jsonl")
}

/// Every `.jsonl` under `dir`, at most `depth` directories deep.
fn walk_jsonl(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_dir() {
            if depth > 0 {
                walk_jsonl(&path, depth - 1, out);
            }
        } else if is_jsonl(&path) {
            out.push(path);
        }
    }
}

fn stat_source(src: &Source) -> Vec<Stat> {
    let mut out = Vec::new();
    match src.harness {
        Harness::Claude => {
            let Ok(dirs) = std::fs::read_dir(&src.root) else { return out };
            for d in dirs.flatten() {
                if !d.file_type().is_ok_and(|t| t.is_dir()) {
                    continue;
                }
                let name = d.file_name().to_string_lossy().into_owned();
                let Ok(files) = std::fs::read_dir(d.path()) else { continue };
                for f in files.flatten() {
                    let path = f.path();
                    if is_jsonl(&path) {
                        out.extend(stat_file(src, path, Some(name.clone())));
                    }
                }
            }
        }
        Harness::Codex | Harness::Pi | Harness::Omp => {
            let depth = if src.harness == Harness::Codex { CODEX_MAX_DEPTH } else { PI_MAX_DEPTH };
            let mut files = Vec::new();
            walk_jsonl(&src.root, depth, &mut files);
            out.extend(files.into_iter().filter_map(|p| stat_file(src, p, None)));
        }
    }
    out
}

fn cache_file(h: Harness) -> PathBuf {
    let name = match h {
        Harness::Claude => "sessions-v2.cache.json",
        Harness::Codex => "codex-sessions-v2.cache.json",
        Harness::Pi => "pi-sessions-v2.cache.json",
        Harness::Omp => "omp-sessions-v2.cache.json",
    };
    paths::bro_dir().join(name)
}

/// Read and describe one session file (no cache).
pub fn describe_file(harness: Harness, path: &Path) -> Described {
    match harness {
        Harness::Claude => describe::describe_transcript(&read_head(path, CLAUDE_HEAD)),
        Harness::Codex => describe::describe_rollout(&read_head(path, CODEX_HEAD)),
        Harness::Pi | Harness::Omp => describe::describe_pi(&read_head(path, PI_HEAD)),
    }
}

/// Describe `paths` on a few worker threads, preserving order.
fn describe_parallel(jobs: &[(Harness, PathBuf)]) -> Vec<Described> {
    if jobs.is_empty() {
        return Vec::new();
    }
    let chunk = jobs.len().div_ceil(WORKERS).max(1);
    std::thread::scope(|scope| {
        let handles: Vec<_> = jobs
            .chunks(chunk)
            .map(|part| scope.spawn(move || part.iter().map(|(h, p)| describe_file(*h, p)).collect::<Vec<_>>()))
            .collect();
        handles.into_iter().flat_map(|h| h.join().unwrap_or_default()).collect()
    })
}

fn file_stem(p: &Path) -> String {
    p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

fn session_id(harness: Harness, stat_path: &Path, d: &Described) -> String {
    match harness {
        Harness::Claude => file_stem(stat_path),
        Harness::Codex => d.id.clone(),
        // Pi names files `<timestamp>_<id>.jsonl`; the header id is authoritative.
        Harness::Pi | Harness::Omp if !d.id.is_empty() => d.id.clone(),
        Harness::Pi | Harness::Omp => {
            let stem = file_stem(stat_path);
            stem.rsplit_once('_').map(|(_, id)| id.to_string()).unwrap_or(stem)
        }
    }
}

fn to_info(stat: &Stat, d: &Described, cwd_fallback: Option<&String>) -> SessionInfo {
    let cwd = Some(d.cwd.clone()).filter(|c| !c.is_empty()).or_else(|| cwd_fallback.cloned()).map(PathBuf::from);
    SessionInfo {
        id: session_id(stat.harness, &stat.path, d),
        harness: stat.harness,
        profile_id: stat.profile_id.clone(),
        path: stat.path.clone(),
        project: cwd.as_deref().map(crate::projects::project_for),
        cwd,
        title: d.title.clone(),
        branch: Some(d.branch.clone()).filter(|b| !b.is_empty()),
        modified: stat.modified,
        size: stat.size,
    }
}

fn keep(stat: &Stat, d: &Described) -> bool {
    !d.title.is_empty() && (stat.harness != Harness::Codex || (d.interactive && !d.id.is_empty()))
}

/// Newest first. Uses on-disk caches; blocking.
pub fn list(opts: &ListOpts) -> Vec<SessionInfo> {
    let limit = if opts.limit == 0 { DEFAULT_LIMIT } else { opts.limit };
    let mut stats: Vec<Stat> = sources(&opts.harnesses).iter().flat_map(stat_source).collect();
    stats.sort_by_key(|s| std::cmp::Reverse(s.modified));
    stats.truncate(limit);

    let mut caches: HashMap<Harness, Cache> = HashMap::new();
    let mut described: Vec<Option<Described>> = vec![None; stats.len()];
    let mut misses: Vec<(usize, Harness, PathBuf)> = Vec::new();
    for (i, s) in stats.iter().enumerate() {
        let cache = caches.entry(s.harness).or_insert_with(|| Cache::load(cache_file(s.harness)));
        let key = s.path.to_string_lossy().into_owned();
        match cache.hit(&key, s.mtime_ms, s.size) {
            Some(d) => described[i] = Some(d),
            None => misses.push((i, s.harness, s.path.clone())),
        }
    }
    let jobs: Vec<(Harness, PathBuf)> = misses.iter().map(|(_, h, p)| (*h, p.clone())).collect();
    for ((i, h, path), d) in misses.iter().zip(describe_parallel(&jobs)) {
        let s = &stats[*i];
        if let Some(cache) = caches.get_mut(h) {
            cache.put(path.to_string_lossy().into_owned(), Entry::new(s.mtime_ms, s.size, &d));
        }
        described[*i] = Some(d);
    }
    for (h, cache) in caches {
        let live: HashSet<String> =
            stats.iter().filter(|s| s.harness == h).map(|s| s.path.to_string_lossy().into_owned()).collect();
        cache.save_pruned(&live);
    }

    // Sessions in one Claude project dir share a cwd; one that recorded it covers the
    // rest (older transcripts predate the cwd field).
    let mut cwd_by_project: HashMap<String, String> = HashMap::new();
    for (s, d) in stats.iter().zip(&described) {
        if let (Some(dir), Some(d)) = (&s.project_dir, d)
            && !d.cwd.is_empty()
        {
            cwd_by_project.entry(dir.clone()).or_insert_with(|| d.cwd.clone());
        }
    }

    stats
        .iter()
        .zip(described)
        .filter_map(|(s, d)| {
            let d = d?;
            keep(s, &d).then(|| to_info(s, &d, s.project_dir.as_ref().and_then(|p| cwd_by_project.get(p))))
        })
        .collect()
}

/// Locate one session by id without listing everything (checks each login's session
/// dir directly). Title may be empty for an abandoned start.
pub fn find_by_id(harness: Harness, id: &str) -> Option<SessionInfo> {
    find_among(harness, id, None)
}

/// Like [`find_by_id`] but only inside one login's directory (`profile_id` such as
/// "claude:work"); for Pi/omp pass `None`.
pub fn find_in_profile(harness: Harness, id: &str, profile_id: Option<&str>) -> Option<SessionInfo> {
    find_among(harness, id, Some(profile_id))
}

fn find_among(harness: Harness, id: &str, only: Option<Option<&str>>) -> Option<SessionInfo> {
    if id.is_empty() || id.contains(['/', '\\']) {
        return None;
    }
    let all = sources(&[harness]);
    let wanted = all.into_iter().filter(|s| only.is_none_or(|o| s.profile_id.as_deref() == o));
    for src in wanted {
        let found: Option<(PathBuf, Option<String>)> = match harness {
            Harness::Claude => std::fs::read_dir(&src.root).ok().and_then(|dirs| {
                dirs.flatten().find_map(|d| {
                    let p = d.path().join(format!("{id}.jsonl"));
                    p.is_file().then(|| (p, Some(d.file_name().to_string_lossy().into_owned())))
                })
            }),
            _ => {
                let depth = if harness == Harness::Codex { CODEX_MAX_DEPTH } else { PI_MAX_DEPTH };
                let mut files = Vec::new();
                walk_jsonl(&src.root, depth, &mut files);
                files.into_iter().find(|p| file_stem(p).contains(id)).map(|p| (p, None))
            }
        };
        let Some((path, project_dir)) = found else { continue };
        let meta = std::fs::metadata(&path).ok()?;
        let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        let stat = Stat {
            harness,
            profile_id: src.profile_id.clone(),
            path: path.clone(),
            project_dir,
            modified,
            mtime_ms: system_time_ms(modified),
            size: meta.len(),
        };
        let d = describe_file(harness, &path);
        return Some(to_info(&stat, &d, None));
    }
    None
}

/// `8-4-4-4-12` hex — the only session ids bro will build paths from.
pub fn is_uuid(id: &str) -> bool {
    let parts: Vec<&str> = id.split('-').collect();
    parts.len() == 5
        && parts.iter().zip([8, 4, 4, 4, 12]).all(|(p, n)| p.len() == n && p.chars().all(|c| c.is_ascii_hexdigit()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::test_env::sandbox;
    use serde_json::json;

    pub(crate) fn write_claude_session(dir: &Path, project: &str, id: &str, lines: &[serde_json::Value]) -> PathBuf {
        let p = dir.join("projects").join(project).join(format!("{id}.jsonl"));
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        let mut body: String = lines.iter().map(|l| format!("{l}\n")).collect();
        body.push_str(&format!("{}\n", json!({"type": "assistant", "pad": "x".repeat(600)})));
        std::fs::write(&p, body).unwrap();
        p
    }

    #[test]
    fn lists_claude_codex_and_pi_sessions() {
        let sb = sandbox();
        let acct = paths::claude_accounts_dir().join("work");
        let id1 = "11111111-1111-4111-8111-111111111111";
        let id2 = "22222222-2222-4222-8222-222222222222";
        write_claude_session(&acct, "F--proj", id1, &[json!({"type": "user", "cwd": "F:\\proj", "message": {"content": "First prompt"}})]);
        // Same project dir, no cwd recorded → inherits.
        write_claude_session(&acct, "F--proj", id2, &[json!({"type": "user", "message": {"content": "Second"}})]);
        // Only noise → dropped.
        write_claude_session(&acct, "F--proj", "33333333-3333-4333-8333-333333333333", &[json!({"type": "user", "message": {"content": "<system-reminder>x"}})]);
        // Tiny file ignored.
        std::fs::write(acct.join("projects").join("F--proj").join("tiny.jsonl"), "{}").unwrap();

        let rollout = sb.home().join(".codex").join("sessions").join("2026").join("09").join("29");
        std::fs::create_dir_all(&rollout).unwrap();
        let cid = "01a0ed53-d984-7ad0-badc-5b7024210c1a";
        let lines = [
            json!({"type": "session_meta", "payload": {"id": cid, "cwd": "F:\\z4", "originator": "codex-tui"}}),
            json!({"type": "response_item", "payload": {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "Codex task"}]}}),
            json!({"type": "pad", "x": "y".repeat(600)}),
        ];
        std::fs::write(rollout.join(format!("rollout-2026-09-29T09-21-32-{cid}.jsonl")), lines.iter().map(|l| format!("{l}\n")).collect::<String>()).unwrap();

        let pi_dir = sb.home().join(".pi").join("agent").join("sessions").join("--w--");
        std::fs::create_dir_all(&pi_dir).unwrap();
        let pi_lines = [
            json!({"type": "session", "id": "pi-1", "cwd": "/w"}),
            json!({"type": "message", "message": {"role": "user", "content": [{"type": "text", "text": "Pi prompt"}]}}),
            json!({"type": "pad", "x": "z".repeat(600)}),
        ];
        std::fs::write(pi_dir.join("2026_pi-1.jsonl"), pi_lines.iter().map(|l| format!("{l}\n")).collect::<String>()).unwrap();

        let all = list(&ListOpts::default());
        let titles: HashSet<&str> = all.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(titles, HashSet::from(["First prompt", "Second", "Codex task", "Pi prompt"]));
        let second = all.iter().find(|s| s.title == "Second").unwrap();
        assert_eq!(second.id, id2);
        assert_eq!(second.profile_id.as_deref(), Some("claude:work"));
        assert_eq!(second.cwd.as_deref(), Some(Path::new("F:\\proj")));
        assert_eq!(second.project.as_ref().unwrap().name, if cfg!(windows) { "proj" } else { "F:\\proj" });
        let codex = all.iter().find(|s| s.harness == Harness::Codex).unwrap();
        assert_eq!(codex.id, cid);
        assert_eq!(codex.profile_id.as_deref(), Some("codex:local"));
        let pi = all.iter().find(|s| s.harness == Harness::Pi).unwrap();
        assert_eq!((pi.id.as_str(), pi.profile_id.as_ref()), ("pi-1", None));

        // Cached second pass returns the same, and the v2 cache file exists.
        assert!(sb.bro().join("sessions-v2.cache.json").exists());
        assert_eq!(list(&ListOpts::default()).len(), 4);
        let only_codex = list(&ListOpts { limit: 0, harnesses: vec![Harness::Codex] });
        assert_eq!(only_codex.len(), 1);
        assert_eq!(list(&ListOpts { limit: 1, harnesses: vec![] }).len(), 1);

        let found = find_by_id(Harness::Claude, id1).unwrap();
        assert_eq!(found.title, "First prompt");
        assert!(find_by_id(Harness::Codex, cid).is_some());
        assert!(find_by_id(Harness::Claude, "nope").is_none());
    }

    #[test]
    fn uuid_check() {
        assert!(is_uuid("01a0ed53-d984-7ad0-badc-5b7024210c1a"));
        assert!(!is_uuid("../../etc"));
        assert!(!is_uuid("01a0ed53-d984-7ad0-badc-5b7024210c1"));
    }
}

#[cfg(test)]
mod real_home {
    /// Manual smoke test against the real machine (read-only apart from the v2 cache in
    /// `$BRO_DIR`): `cargo test -p bro-core real_home -- --ignored --nocapture`.
    #[test]
    #[ignore = "reads the real home directory"]
    fn real_home_smoke() {
        let t = std::time::Instant::now();
        let all = super::list(&super::ListOpts::default());
        println!("{} sessions in {:?}", all.len(), t.elapsed());
        for s in all.iter().take(15) {
            println!("{:?} {:?} {:?} {}", s.harness, s.profile_id, s.project.as_ref().map(|p| &p.name), s.title);
        }
        let t = std::time::Instant::now();
        let again = super::list(&super::ListOpts::default());
        println!("cached: {} in {:?}", again.len(), t.elapsed());
        for p in crate::profiles::list() {
            println!("{} auth={} plan={:?} tier={:?} id={:?} email={:?}", p.id, p.authenticated, p.plan, p.tier, p.identity.is_some(), p.email.is_some());
        }
    }
}
