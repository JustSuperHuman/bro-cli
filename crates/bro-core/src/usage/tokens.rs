//! Token totals per login, read from disk (v1 `bro tokens` / `bro profiles`, without ccusage).
//!
//! * Claude keeps its own all-time counter in `<dir>/stats-cache.json`: `modelUsage` is the lifetime
//!   per-model tally, `dailyModelTokens` a short per-day window. That's the lifetime figure.
//! * Both CLIs keep logs: Claude transcripts (`<dir>/projects/**/*.jsonl`, one line per content block,
//!   deduped by message id + request id) and Codex rollouts (`<dir>/sessions/**`, `archived_sessions`,
//!   `token_count` events). They give the rolling 24h / 7d / 30d windows, and Codex's lifetime — a floor,
//!   since only retained logs count.
//!
//! Logs run to gigabytes, so each file's hourly buckets are cached in `~/.bro/token-scan.json` keyed by
//! path + size + mtime; only new or grown files are read again. Claude files untouched for 32 days are
//! skipped outright (they can't reach the 30d window, and Claude's counter covers lifetime).

use crate::paths;
use crate::util::{parse_iso_secs, path_key, write_json_compact};
use chrono::{Local, NaiveDate, TimeZone};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const HOUR: i64 = 3600;
const DAY: i64 = 24 * HOUR;
const CACHE_VERSION: u32 = 1;
/// Claude logs older than this can't reach the 30-day window.
const CLAUDE_LOOKBACK: i64 = 32 * DAY;

/// Tokens by kind. `total()` is what v1's reports show (input + output + cache).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tokens {
    /// uncached input
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

impl Tokens {
    pub fn total(&self) -> u64 {
        self.input + self.output + self.cache_read + self.cache_write
    }
    pub fn add(&mut self, o: &Tokens) {
        self.input += o.input;
        self.output += o.output;
        self.cache_read += o.cache_read;
        self.cache_write += o.cache_write;
    }
}

/// Claude's own counter (`stats-cache.json`).
#[derive(Debug, Clone, Default)]
pub struct ClaudeCounter {
    /// all-time input + output + cache, every model
    pub lifetime: u64,
    /// per local date, from `dailyModelTokens`
    pub daily: BTreeMap<NaiveDate, u64>,
    pub sessions: Option<u64>,
    /// `lastComputedDate`: the counter is current up to this day
    pub through: Option<NaiveDate>,
}

impl ClaudeCounter {
    /// Tokens on the local dates `from..=to`.
    pub fn between(&self, from: NaiveDate, to: NaiveDate) -> u64 {
        self.daily.range(from..=to).map(|(_, n)| n).sum()
    }
}

/// Read a Claude login's `stats-cache.json`; None when Claude hasn't written one yet.
pub fn claude_counter(dir: &Path) -> Option<ClaudeCounter> {
    let v = crate::util::read_json(&dir.join("stats-cache.json"))?;
    let n = |v: &serde_json::Value| v.as_f64().filter(|n| n.is_finite() && *n > 0.0).map(|n| n as u64).unwrap_or(0);
    const LIFETIME: [&str; 4] = ["inputTokens", "outputTokens", "cacheReadInputTokens", "cacheCreationInputTokens"];
    let lifetime = v.get("modelUsage").and_then(|m| m.as_object()).map(|m| m.values().map(|u| LIFETIME.iter().map(|k| n(&u[*k])).sum::<u64>()).sum()).unwrap_or(0);
    let mut daily = BTreeMap::new();
    for day in v.get("dailyModelTokens").and_then(|d| d.as_array()).into_iter().flatten() {
        let Some(date) = day.get("date").and_then(|d| d.as_str()).and_then(|d| d.parse::<NaiveDate>().ok()) else { continue };
        let sum: u64 = day.get("tokensByModel").and_then(|m| m.as_object()).map(|m| m.values().map(n).sum()).unwrap_or(0);
        *daily.entry(date).or_insert(0) += sum;
    }
    Some(ClaudeCounter {
        lifetime,
        daily,
        sessions: v.get("totalSessions").and_then(|s| s.as_u64()),
        through: v.get("lastComputedDate").and_then(|d| d.as_str()).and_then(|d| d.parse().ok()),
    })
}

/// Hourly token buckets read from one login's logs.
#[derive(Debug, Clone, Default)]
pub struct LogTotals {
    /// hour start (unix seconds, UTC) → tokens
    pub hours: BTreeMap<i64, Tokens>,
    /// log files that went into it
    pub files: usize,
}

impl LogTotals {
    /// Everything since `from` (unix seconds), at hour granularity.
    pub fn since(&self, from: i64) -> Tokens {
        let mut t = Tokens::default();
        for v in self.hours.range(from - from.rem_euclid(HOUR)..).map(|(_, v)| v) {
            t.add(v);
        }
        t
    }
    /// Every retained log.
    pub fn all(&self) -> Tokens {
        self.since(i64::MIN / 2)
    }
    /// Totals per local date.
    pub fn daily(&self) -> BTreeMap<NaiveDate, u64> {
        let mut out = BTreeMap::new();
        for (h, t) in &self.hours {
            *out.entry(local_date(*h)).or_insert(0) += t.total();
        }
        out
    }
    /// The last hour with any tokens.
    pub fn last(&self) -> Option<i64> {
        self.hours.keys().next_back().copied()
    }
}

/// The local calendar date of a unix time.
pub fn local_date(secs: i64) -> NaiveDate {
    Local.timestamp_opt(secs, 0).single().map(|d| d.date_naive()).unwrap_or_default()
}

/// One login to scan.
#[derive(Debug, Clone)]
pub struct Login {
    pub claude: bool,
    pub dir: PathBuf,
}

/// Bytes still to read (files the cache can't answer) and bytes read so far.
#[derive(Debug, Default)]
pub struct Progress {
    pub total: AtomicU64,
    pub done: AtomicU64,
}

#[derive(Default, Serialize, Deserialize)]
struct Cache {
    version: u32,
    files: HashMap<String, FileEntry>,
}

#[derive(Clone, Serialize, Deserialize)]
struct FileEntry {
    size: u64,
    mtime: i64,
    /// (hour, input, output, cache_read, cache_write)
    hours: Vec<(i64, u64, u64, u64, u64)>,
}

fn cache_path() -> PathBuf {
    paths::bro_dir().join("token-scan.json")
}

fn load_cache() -> Cache {
    std::fs::read(cache_path())
        .ok()
        .and_then(|b| serde_json::from_slice::<Cache>(&b).ok())
        .filter(|c| c.version == CACHE_VERSION)
        .unwrap_or_default()
}

/// Every `.jsonl` under `root`: (path, size, mtime secs).
fn walk(root: &Path, out: &mut Vec<(PathBuf, u64, i64)>) {
    let Ok(entries) = std::fs::read_dir(root) else { return };
    for e in entries.flatten() {
        let Ok(ft) = e.file_type() else { continue };
        let path = e.path();
        if ft.is_dir() {
            walk(&path, out);
        } else if ft.is_file() && path.extension().is_some_and(|x| x == "jsonl")
            && let Ok(md) = e.metadata()
        {
            let mtime = md.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64).unwrap_or(0);
            out.push((path, md.len(), mtime));
        }
    }
}

fn log_files(login: &Login, now: i64) -> Vec<(PathBuf, u64, i64)> {
    let mut out = vec![];
    if login.claude {
        walk(&login.dir.join("projects"), &mut out);
        out.retain(|(_, _, mtime)| *mtime >= now - CLAUDE_LOOKBACK);
    } else {
        walk(&login.dir.join("sessions"), &mut out);
        walk(&login.dir.join("archived_sessions"), &mut out);
    }
    out
}

/// Scan every login's logs; files unchanged since the last scan come from the cache. Blocking — run it
/// off the UI thread. Results are in `logins` order.
pub fn scan(logins: &[Login], progress: &Progress) -> Vec<LogTotals> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let cache = load_cache();
    // (login index, path, size, mtime, cache key)
    let mut jobs = vec![];
    let mut fresh: HashMap<String, FileEntry> = HashMap::new();
    let mut owner: Vec<(usize, String)> = vec![];
    let mut seen = HashSet::new();
    for (i, login) in logins.iter().enumerate() {
        for (path, size, mtime) in log_files(login, now) {
            let key = path_key(&path);
            if !seen.insert(key.clone()) {
                continue;
            }
            owner.push((i, key.clone()));
            match cache.files.get(&key) {
                Some(e) if e.size == size && e.mtime == mtime => {
                    fresh.insert(key, e.clone());
                }
                _ => jobs.push((login.claude, path, size, mtime, key)),
            }
        }
    }
    drop(cache);
    progress.total.store(jobs.iter().map(|j| j.2).sum(), Ordering::Relaxed);

    let next = std::sync::atomic::AtomicUsize::new(0);
    let read: Mutex<Vec<(String, FileEntry)>> = Mutex::new(vec![]);
    let workers = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(1, 8).min(jobs.len().max(1));
    std::thread::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| {
                while let Some((claude, path, size, mtime, key)) = jobs.get(next.fetch_add(1, Ordering::Relaxed)) {
                    let hours = if *claude { read_claude(path, progress) } else { read_codex(path, progress) };
                    let hours = hours.into_iter().map(|(h, t)| (h, t.input, t.output, t.cache_read, t.cache_write)).collect();
                    read.lock().unwrap().push((key.clone(), FileEntry { size: *size, mtime: *mtime, hours }));
                }
            });
        }
    });
    let read = read.into_inner().unwrap();
    let changed = !read.is_empty() || fresh.len() != owner.len();
    fresh.extend(read);

    let mut out: Vec<LogTotals> = logins.iter().map(|_| LogTotals::default()).collect();
    for (i, key) in &owner {
        let Some(e) = fresh.get(key) else { continue };
        let t = &mut out[*i];
        t.files += 1;
        for (h, input, output, cache_read, cache_write) in &e.hours {
            t.hours.entry(*h).or_default().add(&Tokens { input: *input, output: *output, cache_read: *cache_read, cache_write: *cache_write });
        }
    }
    if changed || !cache_path().exists() {
        let _ = write_json_compact(&cache_path(), &Cache { version: CACHE_VERSION, files: fresh });
    }
    out
}

/// Feed `path` line by line to `each`, skipping lines without `needle`; counts bytes into `progress`.
fn lines(path: &Path, needle: &[u8], progress: &Progress, mut each: impl FnMut(&[u8])) {
    let Ok(f) = std::fs::File::open(path) else { return };
    let finder = memchr::memmem::Finder::new(needle);
    let mut r = BufReader::with_capacity(1 << 18, f);
    let mut buf = Vec::with_capacity(1 << 16);
    let mut pending = 0u64;
    loop {
        buf.clear();
        match r.read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                pending += n as u64;
                if pending >= 1 << 20 {
                    progress.done.fetch_add(pending, Ordering::Relaxed);
                    pending = 0;
                }
            }
        }
        if finder.find(&buf).is_some() {
            each(&buf);
        }
    }
    progress.done.fetch_add(pending, Ordering::Relaxed);
}

fn hour_of(ts: Option<&str>) -> Option<i64> {
    let s = parse_iso_secs(ts?)?;
    Some(s - s.rem_euclid(HOUR))
}

#[derive(Deserialize)]
struct ClaudeLine {
    timestamp: Option<String>,
    #[serde(rename = "requestId")]
    request_id: Option<String>,
    message: Option<ClaudeMsg>,
}

#[derive(Deserialize)]
struct ClaudeMsg {
    id: Option<String>,
    usage: Option<ClaudeUsage>,
}

#[derive(Deserialize)]
struct ClaudeUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
}

/// A Claude transcript: each assistant message's usage, once (it's repeated on every content-block line).
fn read_claude(path: &Path, progress: &Progress) -> BTreeMap<i64, Tokens> {
    let mut out: BTreeMap<i64, Tokens> = BTreeMap::new();
    let mut seen = HashSet::new();
    lines(path, b"\"usage\"", progress, |line| {
        let Ok(l) = serde_json::from_slice::<ClaudeLine>(line) else { return };
        let Some(msg) = l.message else { return };
        let Some(u) = msg.usage else { return };
        if let Some(id) = &msg.id
            && !seen.insert(format!("{id}:{}", l.request_id.as_deref().unwrap_or("")))
        {
            return;
        }
        let Some(h) = hour_of(l.timestamp.as_deref()) else { return };
        out.entry(h).or_default().add(&Tokens { input: u.input_tokens, output: u.output_tokens, cache_read: u.cache_read_input_tokens, cache_write: u.cache_creation_input_tokens });
    });
    out.retain(|_, t| t.total() > 0);
    out
}

#[derive(Deserialize)]
struct CodexLine {
    timestamp: Option<String>,
    payload: Option<CodexPayload>,
}

#[derive(Deserialize)]
struct CodexPayload {
    #[serde(rename = "type")]
    kind: Option<String>,
    info: Option<CodexInfo>,
}

#[derive(Deserialize)]
struct CodexInfo {
    total_token_usage: Option<CodexUsage>,
    last_token_usage: Option<CodexUsage>,
}

#[derive(Deserialize, Clone, Copy, PartialEq, Eq, Default)]
struct CodexUsage {
    /// includes the cached part
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    cached_input_tokens: u64,
    /// includes reasoning
    #[serde(default)]
    output_tokens: u64,
}

impl CodexUsage {
    fn minus(self, o: CodexUsage) -> CodexUsage {
        CodexUsage {
            input_tokens: self.input_tokens.saturating_sub(o.input_tokens),
            cached_input_tokens: self.cached_input_tokens.saturating_sub(o.cached_input_tokens),
            output_tokens: self.output_tokens.saturating_sub(o.output_tokens),
        }
    }
    fn tokens(self) -> Tokens {
        let cached = self.cached_input_tokens.min(self.input_tokens);
        Tokens { input: self.input_tokens - cached, output: self.output_tokens, cache_read: cached, cache_write: 0 }
    }
}

/// A Codex rollout: each turn's `last_token_usage` (or the step in the running total), skipping the
/// repeats Codex writes when the total didn't move.
fn read_codex(path: &Path, progress: &Progress) -> BTreeMap<i64, Tokens> {
    let mut out: BTreeMap<i64, Tokens> = BTreeMap::new();
    let mut prev: Option<CodexUsage> = None;
    lines(path, b"\"token_count\"", progress, |line| {
        let Ok(l) = serde_json::from_slice::<CodexLine>(line) else { return };
        let Some(p) = l.payload.filter(|p| p.kind.as_deref() == Some("token_count")) else { return };
        let Some(info) = p.info else { return };
        let step = match (info.total_token_usage, prev) {
            (Some(total), Some(before)) if total == before => return,
            (total, before) => match (info.last_token_usage, total) {
                (Some(last), _) => last,
                (None, Some(total)) => total.minus(before.unwrap_or_default()),
                (None, None) => return,
            },
        };
        if info.total_token_usage.is_some() {
            prev = info.total_token_usage;
        }
        let Some(h) = hour_of(l.timestamp.as_deref()) else { return };
        out.entry(h).or_default().add(&step.tokens());
    });
    out.retain(|_, t| t.total() > 0);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::test_env::sandbox;

    fn write(path: &Path, lines: &[serde_json::Value]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
        std::fs::write(path, text).unwrap();
    }

    fn iso(secs: i64) -> String {
        chrono::DateTime::from_timestamp(secs, 0).unwrap().to_rfc3339()
    }

    fn claude_line(ts: i64, id: &str, req: &str, input: u64, output: u64) -> serde_json::Value {
        serde_json::json!({ "type": "assistant", "timestamp": iso(ts), "requestId": req,
            "message": { "id": id, "role": "assistant", "content": [{ "type": "text", "text": "hi" }],
                "usage": { "input_tokens": input, "output_tokens": output, "cache_read_input_tokens": 100, "cache_creation_input_tokens": 10 } } })
    }

    fn codex_line(ts: i64, total: (u64, u64, u64), last: (u64, u64, u64)) -> serde_json::Value {
        let u = |(i, c, o): (u64, u64, u64)| serde_json::json!({ "input_tokens": i, "cached_input_tokens": c, "output_tokens": o, "reasoning_output_tokens": 0, "total_tokens": i + o });
        serde_json::json!({ "timestamp": iso(ts), "type": "event_msg", "payload": { "type": "token_count", "info": { "total_token_usage": u(total), "last_token_usage": u(last) } } })
    }

    #[test]
    fn claude_counter_reads_lifetime_and_days() {
        let sb = sandbox();
        let dir = sb.home().join(".claude");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("stats-cache.json"), serde_json::json!({
            "lastComputedDate": "2026-10-02", "totalSessions": 7,
            "modelUsage": { "a": { "inputTokens": 1, "outputTokens": 2, "cacheReadInputTokens": 3, "cacheCreationInputTokens": 4, "costUSD": 99 }, "b": { "inputTokens": 10 } },
            "dailyModelTokens": [{ "date": "2026-09-01", "tokensByModel": { "a": 5 } }, { "date": "2026-10-01", "tokensByModel": { "a": 6, "b": 7 } }]
        }).to_string()).unwrap();
        let c = claude_counter(&dir).unwrap();
        assert_eq!(c.lifetime, 20);
        assert_eq!(c.sessions, Some(7));
        let d = |s: &str| s.parse::<NaiveDate>().unwrap();
        assert_eq!(c.between(d("2026-09-03"), d("2026-10-02")), 13);
        assert_eq!(c.through, Some(d("2026-10-02")));
        assert!(claude_counter(&sb.home().join("nope")).is_none());
    }

    #[test]
    fn scans_both_logs_dedupes_and_caches() {
        let sb = sandbox();
        let now = crate::util::now_secs();
        let claude = sb.home().join(".claude");
        let codex = sb.home().join(".codex");
        // one message spread over two content-block lines, a second message, and noise
        write(&claude.join("projects").join("p").join("s.jsonl"), &[
            serde_json::json!({ "type": "user", "message": { "role": "user", "content": "usage" } }),
            claude_line(now - 60, "m1", "r1", 5, 50),
            claude_line(now - 60, "m1", "r1", 5, 50),
            claude_line(now - 3 * DAY, "m2", "r2", 1, 9),
        ]);
        // sub-agent transcripts sit deeper
        write(&claude.join("projects").join("p").join("s").join("subagents").join("a.jsonl"), &[claude_line(now - 120, "m3", "r3", 0, 1)]);
        write(&codex.join("sessions").join("2026").join("10").join("02").join("rollout-a.jsonl"), &[
            codex_line(now - 10 * DAY, (100, 40, 10), (100, 40, 10)),
            codex_line(now - 10 * DAY, (100, 40, 10), (100, 40, 10)), // repeat: total didn't move
            codex_line(now - 30, (300, 140, 30), (200, 100, 20)),
        ]);
        write(&codex.join("archived_sessions").join("rollout-b.jsonl"), &[codex_line(now - 400 * DAY, (7, 0, 3), (7, 0, 3))]);
        let logins = [Login { claude: true, dir: claude.clone() }, Login { claude: false, dir: codex.clone() }];

        let p = Progress::default();
        let r = scan(&logins, &p);
        assert!(p.total.load(Ordering::Relaxed) > 0);
        assert_eq!(p.done.load(Ordering::Relaxed), p.total.load(Ordering::Relaxed));
        let c = &r[0];
        assert_eq!(c.files, 2);
        // m1 once (5+50+100+10) + m3 (0+1+110); m2 is outside 24h
        assert_eq!(c.since(now - DAY).total(), 165 + 111);
        assert_eq!(c.since(now - 7 * DAY).total(), 276 + 120);
        let x = &r[1];
        assert_eq!(x.all(), Tokens { input: 60 + 100 + 7, output: 10 + 20 + 3, cache_read: 40 + 100, cache_write: 0 });
        assert_eq!(x.since(now - DAY).total(), 220);
        assert_eq!(x.since(now - 30 * DAY).total(), 330);
        assert!(x.last().unwrap() > now - 2 * HOUR);

        // second scan: nothing to read again
        let p2 = Progress::default();
        let again = scan(&logins, &p2);
        assert_eq!(p2.total.load(Ordering::Relaxed), 0);
        assert_eq!(again[1].all(), x.all());
        assert_eq!(again[0].since(now - DAY), c.since(now - DAY));

        // a grown file is read again
        let f = codex.join("archived_sessions").join("rollout-b.jsonl");
        write(&f, &[codex_line(now - 400 * DAY, (7, 0, 3), (7, 0, 3)), codex_line(now - 399 * DAY, (17, 0, 4), (10, 0, 1))]);
        let p3 = Progress::default();
        let grown = scan(&logins, &p3);
        assert_eq!(p3.total.load(Ordering::Relaxed), std::fs::metadata(&f).unwrap().len());
        assert_eq!(grown[1].all().total(), x.all().total() + 11);
    }

    #[test]
    fn old_claude_logs_are_skipped_and_missing_dirs_are_empty() {
        let sb = sandbox();
        let claude = sb.home().join(".claude");
        let f = claude.join("projects").join("p").join("old.jsonl");
        write(&f, &[claude_line(crate::util::now_secs() - 90 * DAY, "m", "r", 1, 1)]);
        let old = SystemTime::now() - std::time::Duration::from_secs(60 * DAY as u64);
        std::fs::File::options().write(true).open(&f).unwrap().set_modified(old).unwrap();
        let r = scan(&[Login { claude: true, dir: claude }, Login { claude: false, dir: sb.home().join("missing") }], &Progress::default());
        assert_eq!(r[0].files, 0);
        assert_eq!(r[1].all().total(), 0);
    }
}
