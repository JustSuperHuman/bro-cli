//! Slop Collector (beta): finds what the tools your agents run leave behind, and cleans it up.
//!
//! Two kinds of slop:
//!
//! - **Programs left running.** Remotion, Playwright and Puppeteer start browsers with no window (headless) to do
//!   their work. When a render or test is cut short (a tool timeout, Ctrl+C), the browser can keep running with
//!   nothing left to drive it, holding gigabytes until its session closes, and a session can stay open for days.
//! - **Temp files.** The same tools make a throwaway browser profile or render folder in the temp folder for every
//!   run, and delete it when the run ends normally. A run that's cut short leaves it behind. See [`temp`].
//!
//! Every process a session starts sits in its process tree (a Job Object on Windows, see [`crate::proc_tree`]), so
//! the collector sees each one however it was started. It looks every [`EVERY`] and groups what it finds:
//!
//! - **detached**: a process whose parent has exited, with everything under it. Only these can be slop: anything
//!   still under the session's program (or under any other live process) is in use by definition.
//! - **unattended**: a detached group led by an automated browser (headless, or driven over DevTools), a Remotion
//!   helper or a browser driver (chromedriver). Its script is gone, so nothing can drive it: it's unused from the
//!   moment the collector sees it.
//! - **quiet**: a detached group with one of those further down (a hung render script and the browser under it).
//!   It's unused once it stops using the CPU; until then it's working, and only listed as running on its own.
//! - with `anything_idle`, every other detached group too, once it has gone quiet.
//!
//! A group is closed once it has been unused for `wait_mins`. Use starts that clock over: a window on screen, or a
//! connection from outside the group to a port one of its processes listens on (a DevTools client, your browser on a
//! dev server). Never closed: a group with a window, a group with a server in it (unless the server only feeds the
//! group's own browser, as a Remotion render's does; Remotion Studio always counts as a server), the session's own
//! program. A process is ended only while its tree still holds it and only if it is the very process that was seen
//! (its start time matches), so a reused PID is never touched; whatever it started since the look goes with it.
//!
//! What it cleaned up is kept in `~/.bro/v2-slop.json`: running totals and the latest cleanups.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

mod temp;
#[cfg(windows)]
mod windows;

#[cfg(test)]
mod tests;

/// How long a leftover may sit unused, in minutes (the setting steps through these).
pub const WAIT_CHOICES: [u64; 6] = [2, 5, 10, 15, 30, 60];
/// How often the collector looks while it's on.
pub const EVERY: std::time::Duration = std::time::Duration::from_secs(30);
/// ...and while it's clearing temp folders, so a big backlog doesn't take an hour.
pub const SOON: std::time::Duration = std::time::Duration::from_millis(500);
/// Cleanups the ledger keeps.
const RECENT: usize = 25;
/// The CPU a group may use between two looks and still count as quiet: 2% of one core.
const QUIET_SHARE: f64 = 0.02;
/// ...but never less than this (two looks close together).
const QUIET_FLOOR_MS: u64 = 250;
/// A temp cleanup this soon after a program's cleanup is that program's profile: it joins that cleanup.
const JOIN_MS: u64 = 120_000;

/// `[slop]` in `~/.bro/v2.toml`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SlopSettings {
    /// Beta: off until you turn it on.
    pub on: bool,
    /// Clean up once a leftover has waited (true), or only list it until you say (false).
    pub collect: bool,
    /// How long a leftover sits unused before it's closed, in minutes.
    pub wait_mins: u64,
    /// Count every detached process that has gone quiet, not only what renders and tests leave. Servers and
    /// anything with a window are still left alone.
    pub anything_idle: bool,
}

impl Default for SlopSettings {
    fn default() -> Self {
        Self { on: false, collect: true, wait_mins: 10, anything_idle: false }
    }
}

impl SlopSettings {
    fn wait_ms(&self) -> u64 {
        self.wait_mins.max(1) * 60_000
    }
}

/// This platform can look inside a session's process tree.
pub fn supported() -> bool {
    cfg!(windows)
}

/// Why something counts as a leftover.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Reason {
    /// a browser or helper whose script is gone: nothing can drive it
    #[default]
    Unattended,
    /// it has done next to nothing for the wait
    Quiet,
}

/// What the collector found on its last look, and what it has cleaned up so far.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Report {
    pub on: bool,
    /// Cleans up on its own (false: lists what it finds until you say).
    pub collect: bool,
    pub anything_idle: bool,
    pub wait_mins: u64,
    /// This platform can look inside sessions.
    pub supported: bool,
    /// Unix ms of the last look (0: none yet).
    pub looked_ms: u64,
    /// Everything the sessions run: how many sessions, processes and how much memory (private bytes).
    pub sessions: u32,
    pub processes: u32,
    pub memory: u64,
    /// Programs left running, biggest first.
    pub leftovers: Vec<Leftover>,
    /// Temp folders left behind.
    pub temp: TempFiles,
    /// Detached processes it leaves to you: still working, or not a kind it closes.
    pub others: Others,
    pub ledger: Ledger,
}

impl Report {
    /// The memory the leftovers hold.
    pub fn leftover_memory(&self) -> u64 {
        self.leftovers.iter().map(|l| l.memory).sum()
    }

    /// Something to clean up now: a program, or temp files.
    pub fn anything(&self) -> bool {
        !self.leftovers.is_empty() || self.temp.folders > 0
    }
}

/// A group of processes left running.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Leftover {
    /// `pid:start` of its first process.
    pub key: String,
    /// The session it's in (the window's id) and what that session is called.
    pub session: String,
    pub session_label: String,
    /// In plain words: "Chrome from Remotion", "Video encoder from Remotion", "vite". Empty from a server before it
    /// had them: use `what`.
    pub title: String,
    /// What it is exactly: "Remotion's headless Chrome", "Remotion's ffmpeg", "node · vite".
    pub what: String,
    /// The folder it came from, when its path says ("film" for `film/node_modules/.remotion/...`).
    pub project: Option<String>,
    pub processes: u32,
    pub memory: u64,
    pub reason: Reason,
    /// How long it has sat unused.
    pub waited_secs: u64,
    /// When it will be closed (None: it won't be on its own, the collector only lists).
    pub due_secs: Option<u64>,
    /// It used CPU since the last look.
    pub busy: bool,
}

impl Leftover {
    /// What to call it.
    pub fn name(&self) -> &str {
        if self.title.is_empty() { &self.what } else { &self.title }
    }
}

/// Temp folders the tools left behind, waiting to be cleared.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct TempFiles {
    pub folders: u32,
    pub bytes: u64,
    /// Whose they are, most first: "Remotion", "Puppeteer", "Playwright".
    pub tools: Vec<String>,
    /// It's clearing them now.
    pub clearing: bool,
}

/// Detached processes the collector leaves to you.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Others {
    pub groups: u32,
    pub processes: u32,
    pub memory: u64,
    /// A few of them by name, biggest first.
    pub names: Vec<String>,
}

/// What the collector has cleaned up, kept across restarts.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Ledger {
    /// Unix ms it started counting.
    pub since_ms: u64,
    pub cleanups: u64,
    pub processes: u64,
    pub memory: u64,
    /// Disk it cleared, in temp folders.
    pub disk: u64,
    /// Numbers each cleanup, so a window can tell which ones are new.
    pub seq: u64,
    /// Newest first.
    pub recent: Vec<Cleanup>,
}

/// One cleanup: a program the collector closed (with the temp folder it left, when it cleared that too), or temp
/// folders it cleared.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Cleanup {
    pub seq: u64,
    pub at_ms: u64,
    pub session_label: String,
    /// See [`Leftover::title`] and [`Leftover::what`]. Temp folders: "Temp files", and whose they were ("Remotion
    /// and Puppeteer").
    pub title: String,
    pub what: String,
    pub project: Option<String>,
    pub processes: u32,
    pub memory: u64,
    pub reason: Reason,
    pub waited_secs: u64,
    /// Someone asked for it ("Clean up now", `bro slop collect`) rather than the wait running out.
    pub by_hand: bool,
    /// Temp folders cleared, and the disk they held.
    pub folders: u32,
    pub disk: u64,
}

impl Cleanup {
    /// What to call it.
    pub fn name(&self) -> &str {
        if self.title.is_empty() { &self.what } else { &self.title }
    }

    /// Only temp folders: no program.
    pub fn temp_only(&self) -> bool {
        self.processes == 0 && self.folders > 0
    }
}

/// A session to look through.
pub struct Target {
    /// The window's id for the session.
    pub session: String,
    /// What the session is called.
    pub label: String,
    pub scope: crate::proc_tree::Scope,
}

/// A session as the decisions see it.
struct Who<'a> {
    session: &'a str,
    label: &'a str,
    root: Option<u32>,
}

/// One process, as a look found it.
#[derive(Clone, Debug, Default)]
pub(crate) struct Proc {
    pub pid: u32,
    pub ppid: u32,
    /// When it started: opaque, but comparable (a FILETIME on Windows).
    pub started: u64,
    pub cpu_ms: u64,
    /// Private bytes.
    pub mem: u64,
    /// File name, lower case ("chrome.exe").
    pub exe: String,
    pub path: String,
    /// Read for the processes that need one (browsers, script runners); empty otherwise.
    pub cmd: String,
    /// Its parent is alive outside the session's tree: something else still has it.
    pub held: bool,
}

/// What the system says around a set of processes: which have a window on screen, which listen on a port, and who
/// is connected to those ports.
#[derive(Default, Debug)]
pub(crate) struct Around {
    pub windows: HashSet<u32>,
    pub listening: HashMap<u32, Vec<u16>>,
    /// pid → the processes connected to the ports it listens on (0: one bro can't place, on another machine).
    pub clients: HashMap<u32, Vec<u32>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Browser,
    Helper,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Guard {
    /// a window on screen
    Window,
    /// a server it doesn't only run for its own browser
    Serving,
    /// something outside it is connected to it
    InUse,
}

/// A detached process and everything under it.
#[derive(Debug)]
struct Group {
    key: (u32, u64),
    /// Indexes into the session's processes, its first process first.
    members: Vec<usize>,
    kind: Kind,
    /// Its first process is the browser (or helper, or driver) itself: nothing can drive it.
    unattended: bool,
    title: String,
    what: String,
    project: Option<String>,
    cpu_ms: u64,
    mem: u64,
    guard: Option<Guard>,
    /// The temp profiles its browsers run on (folder names, lower case).
    profiles: Vec<String>,
}

/// What the collector remembers about a group between looks.
struct Track {
    /// Unused since (an unattended group).
    first_ms: u64,
    /// Quiet since.
    quiet_ms: u64,
    looked_ms: u64,
    cpu_ms: u64,
    members: HashSet<(u32, u64)>,
}

/// Temp folders cleared over a run of looks, recorded as one cleanup when the run ends.
#[derive(Default)]
struct Clearing {
    folders: u32,
    bytes: u64,
    by_hand: bool,
}

/// The collector: run [`Collector::look`] every [`EVERY`] while it's on ([`SOON`] while [`Collector::clearing`]).
pub struct Collector {
    tracks: HashMap<(u32, u64), Track>,
    /// (path, command line) per process, read once.
    cache: HashMap<(u32, u64), (String, String)>,
    ledger: Ledger,
    path: Option<PathBuf>,
    temp: Option<temp::Sweeper>,
    clearing: Option<Clearing>,
    /// Profiles the browsers it just closed ran on: theirs to clear without the usual wait.
    released: HashSet<String>,
}

impl Collector {
    /// A collector keeping its ledger at `path` (None: in memory).
    pub fn new(path: Option<PathBuf>) -> Collector {
        let mut ledger: Ledger = path.as_ref().and_then(|p| std::fs::read(p).ok()).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        if ledger.since_ms == 0 {
            ledger.since_ms = crate::util::now_ms().max(0) as u64;
        }
        Collector { tracks: HashMap::new(), cache: HashMap::new(), ledger, path, temp: None, clearing: None, released: HashSet::new() }
    }

    /// Also clear what the tools leave in `dir`, the temp folder (see [`temp`]).
    pub fn with_temp(mut self, dir: PathBuf) -> Collector {
        self.temp = Some(temp::Sweeper::new(dir));
        self
    }

    /// It's partway through clearing temp folders: look again [`SOON`].
    pub fn clearing(&self) -> bool {
        self.clearing.is_some() && self.temp.as_ref().is_some_and(|t| t.pending() > 0)
    }

    /// What it says while it isn't looking (off): the settings and the ledger.
    pub fn idle_report(&mut self, s: &SlopSettings) -> Report {
        self.tracks.clear();
        self.cache.clear();
        self.released.clear();
        self.finish_clearing(crate::util::now_ms().max(0) as u64);
        if let Some(t) = self.temp.as_mut() {
            t.forget();
        }
        Report { on: s.on, collect: s.collect, anything_idle: s.anything_idle, wait_mins: s.wait_mins, supported: supported(), ledger: self.ledger.clone(), ..Report::default() }
    }

    /// Look through `targets`, clean up what's due (everything listed, `by_hand`) and say what it found.
    pub fn look(&mut self, targets: &[Target], s: &SlopSettings, now_ms: u64, by_hand: bool) -> Report {
        let procs = self.snapshot(targets);
        let who: Vec<Who> = targets.iter().map(|t| Who { session: &t.session, label: &t.label, root: t.scope.root() }).collect();
        let around = if procs.iter().zip(&who).any(|(p, w)| !detached(p, w.root).is_empty()) { around() } else { Around::default() };
        let mut report = self.decide(&who, &procs, &around, s, now_ms, by_hand, &mut |t, members| end_group(&targets[t].scope, members));
        if self.temp.is_some() {
            if !self.released.is_empty() {
                // a browser lets go of its profile a moment after it's ended
                std::thread::sleep(std::time::Duration::from_millis(400));
            }
            self.sweep(&mut report, s, now_ms, by_hand, &temp_in_use);
        }
        report
    }

    #[cfg(windows)]
    fn snapshot(&mut self, targets: &[Target]) -> Vec<Vec<Proc>> {
        let all = windows::snapshot(targets, &mut self.cache);
        let live: HashSet<(u32, u64)> = all.iter().flatten().map(|p| (p.pid, p.started)).collect();
        self.cache.retain(|k, _| live.contains(k));
        all
    }

    #[cfg(not(windows))]
    fn snapshot(&mut self, targets: &[Target]) -> Vec<Vec<Proc>> {
        targets.iter().map(|_| vec![]).collect()
    }

    /// The decisions, apart from the system: group, guard, time, close (`end` ends a group's processes and says how
    /// many it ended) and report.
    #[allow(clippy::too_many_arguments)]
    fn decide(&mut self, targets: &[Who], procs: &[Vec<Proc>], around: &Around, s: &SlopSettings, now_ms: u64, by_hand: bool, end: &mut dyn FnMut(usize, &[&Proc]) -> u32) -> Report {
        let mut report = Report {
            on: s.on,
            collect: s.collect,
            anything_idle: s.anything_idle,
            wait_mins: s.wait_mins,
            supported: supported(),
            looked_ms: now_ms,
            sessions: targets.len() as u32,
            ..Report::default()
        };
        let wait = s.wait_ms();
        let mut seen = HashSet::new();
        let mut others: Vec<(u64, String)> = vec![];
        let mut ended_any = false;
        for (ti, (target, list)) in targets.iter().zip(procs.iter()).enumerate() {
            report.processes += list.len() as u32;
            report.memory += list.iter().map(|p| p.mem).sum::<u64>();
            for members in detached(list, target.root) {
                let g = classify(list, members, around);
                seen.insert(g.key);
                let keys: HashSet<(u32, u64)> = g.members.iter().map(|&i| (list[i].pid, list[i].started)).collect();
                let track = self.tracks.entry(g.key).or_insert_with(|| Track { first_ms: now_ms, quiet_ms: now_ms, looked_ms: now_ms, cpu_ms: g.cpu_ms, members: keys.clone() });
                let elapsed = now_ms.saturating_sub(track.looked_ms);
                let budget = ((elapsed as f64 * QUIET_SHARE) as u64).max(QUIET_FLOOR_MS);
                let grew = keys.iter().any(|k| !track.members.contains(k));
                let busy = elapsed > 0 && (g.cpu_ms.saturating_sub(track.cpu_ms) > budget || grew);
                if busy {
                    track.quiet_ms = now_ms;
                }
                if g.guard.is_some() {
                    // in use (or a server): unused starts over once that ends
                    track.first_ms = now_ms;
                    track.quiet_ms = now_ms;
                }
                track.looked_ms = now_ms;
                track.cpu_ms = g.cpu_ms;
                track.members = keys;
                if g.guard.is_some() {
                    continue;
                }
                let counts = matches!(g.kind, Kind::Browser | Kind::Helper) || s.anything_idle;
                // something that's working, or that it hasn't watched long enough to tell, isn't a leftover yet
                let settled = g.unattended || (elapsed > 0 && !busy);
                if !counts || !settled {
                    others.push((g.mem, g.title.clone()));
                    report.others.groups += 1;
                    report.others.processes += g.members.len() as u32;
                    report.others.memory += g.mem;
                    continue;
                }
                let reason = if g.unattended { Reason::Unattended } else { Reason::Quiet };
                let since = if g.unattended { track.first_ms } else { track.quiet_ms };
                let waited = now_ms.saturating_sub(since);
                let ran_out = s.collect && waited >= wait;
                if ran_out || by_hand {
                    let group: Vec<&Proc> = g.members.iter().map(|&i| &list[i]).collect();
                    let ended = end(ti, &group);
                    if ended > 0 {
                        ended_any = true;
                        self.tracks.remove(&g.key);
                        self.released.extend(g.profiles.iter().cloned());
                        self.record(Cleanup {
                            at_ms: now_ms,
                            session_label: target.label.to_string(),
                            title: g.title.clone(),
                            what: g.what.clone(),
                            project: g.project.clone(),
                            processes: ended,
                            memory: g.mem,
                            reason,
                            waited_secs: waited / 1000,
                            by_hand: !ran_out,
                            ..Cleanup::default()
                        });
                        continue;
                    }
                }
                report.leftovers.push(Leftover {
                    key: format!("{}:{}", g.key.0, g.key.1),
                    session: target.session.to_string(),
                    session_label: target.label.to_string(),
                    title: g.title,
                    what: g.what,
                    project: g.project,
                    processes: g.members.len() as u32,
                    memory: g.mem,
                    reason,
                    waited_secs: waited / 1000,
                    due_secs: s.collect.then(|| wait.saturating_sub(waited) / 1000),
                    busy,
                });
            }
        }
        self.tracks.retain(|k, _| seen.contains(k));
        if ended_any {
            self.save();
        }
        report.leftovers.sort_by_key(|l| std::cmp::Reverse(l.memory));
        others.sort_by_key(|o| std::cmp::Reverse(o.0));
        let mut names: Vec<String> = vec![];
        for (_, n) in others {
            if !names.contains(&n) && names.len() < 3 {
                names.push(n);
            }
        }
        report.others.names = names;
        report.ledger = self.ledger.clone();
        report
    }

    /// The temp folder: list what's left there (every few minutes, now when asked or when a browser it closed let go of
    /// its profile), clear it when that's on (or asked for, or already under way) and say what's left.
    fn sweep(&mut self, report: &mut Report, s: &SlopSettings, now_ms: u64, by_hand: bool, in_use: &dyn Fn() -> temp::InUse) {
        let Some(t) = self.temp.as_mut() else { return };
        let released = std::mem::take(&mut self.released);
        if by_hand || !released.is_empty() || t.due(now_ms) {
            t.list(std::time::SystemTime::now(), now_ms, &in_use(), &released);
        }
        if !released.is_empty() {
            // anything still locked a moment ago: try again at the next look
            t.soon();
        }
        let go = s.collect || by_hand || self.clearing.as_ref().is_some_and(|c| c.by_hand);
        if go && t.pending() > 0 {
            let (folders, bytes) = t.clear(if by_hand { temp::BY_HAND } else { temp::BUDGET });
            let c = self.clearing.get_or_insert_with(Clearing::default);
            c.folders += folders;
            c.bytes += bytes;
            c.by_hand |= by_hand;
        }
        let pending = t.pending();
        report.temp = t.summary();
        report.temp.clearing = go && pending > 0;
        if pending == 0 {
            self.finish_clearing(now_ms);
        }
        report.ledger = self.ledger.clone();
    }

    /// A run of temp clearing is over: one cleanup for all of it, or, right after a program's cleanup, part of that
    /// one (it's the profile the program left).
    fn finish_clearing(&mut self, now_ms: u64) {
        let Some(c) = self.clearing.take().filter(|c| c.folders > 0) else { return };
        self.ledger.disk += c.bytes;
        match self.ledger.recent.first_mut() {
            Some(last) if last.processes > 0 && last.folders == 0 && now_ms.saturating_sub(last.at_ms) <= JOIN_MS && c.folders <= 2 => {
                last.folders = c.folders;
                last.disk = c.bytes;
            }
            _ => {
                // whose they were: "Remotion and Puppeteer"
                let what = and_list(&self.temp.as_mut().map(|t| t.take_tools()).unwrap_or_default());
                self.ledger.seq += 1;
                self.ledger.cleanups += 1;
                self.ledger.recent.insert(
                    0,
                    Cleanup { seq: self.ledger.seq, at_ms: now_ms, title: "Temp files".into(), what, folders: c.folders, disk: c.bytes, by_hand: c.by_hand, ..Cleanup::default() },
                );
                self.ledger.recent.truncate(RECENT);
            }
        }
        self.save();
    }

    fn record(&mut self, mut c: Cleanup) {
        self.ledger.seq += 1;
        self.ledger.cleanups += 1;
        self.ledger.processes += c.processes as u64;
        self.ledger.memory += c.memory;
        c.seq = self.ledger.seq;
        self.ledger.recent.insert(0, c);
        self.ledger.recent.truncate(RECENT);
    }

    fn save(&self) {
        if let Some(p) = &self.path
            && let Ok(bytes) = serde_json::to_vec_pretty(&self.ledger)
        {
            let _ = crate::util::atomic_write(p, &bytes);
        }
    }
}

#[cfg(windows)]
fn around() -> Around {
    windows::around()
}

#[cfg(not(windows))]
fn around() -> Around {
    Around::default()
}

#[cfg(windows)]
fn end_group(scope: &crate::proc_tree::Scope, members: &[&Proc]) -> u32 {
    windows::end_group(scope, members)
}

#[cfg(not(windows))]
fn end_group(scope: &crate::proc_tree::Scope, members: &[&Proc]) -> u32 {
    members.iter().filter(|p| scope.end(p.pid, p.started)).count() as u32
}

#[cfg(windows)]
fn temp_in_use() -> temp::InUse {
    windows::temp_in_use()
}

#[cfg(not(windows))]
fn temp_in_use() -> temp::InUse {
    // nothing here says who holds a folder: count everything as held
    temp::InUse { unknown: true, ..temp::InUse::default() }
}

/// Each detached process (its parent gone, or a stranger now holding its parent's PID) with everything under it,
/// its first process first. The session's own program, and anything whose parent is alive elsewhere, is never one.
fn detached(procs: &[Proc], root: Option<u32>) -> Vec<Vec<usize>> {
    let by_pid: HashMap<u32, usize> = procs.iter().enumerate().map(|(i, p)| (p.pid, i)).collect();
    let parent = |i: usize| by_pid.get(&procs[i].ppid).copied().filter(|&j| j != i && procs[j].started <= procs[i].started);
    let mut kids: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..procs.len() {
        if let Some(j) = parent(i) {
            kids.entry(j).or_default().push(i);
        }
    }
    let mut out = vec![];
    for (i, p) in procs.iter().enumerate() {
        if Some(p.pid) == root || p.held || parent(i).is_some() {
            continue;
        }
        let (mut members, mut stack, mut seen) = (vec![], vec![i], HashSet::new());
        while let Some(k) = stack.pop() {
            if seen.insert(k) {
                members.push(k);
                stack.extend(kids.get(&k).into_iter().flatten().rev());
            }
        }
        out.push(members);
    }
    out
}

/// What a detached group is, what to call it and what keeps it.
fn classify(procs: &[Proc], members: Vec<usize>, around: &Around) -> Group {
    let lead = members[0];
    let inside: HashSet<usize> = members.iter().copied().collect();
    let by_pid: HashMap<u32, usize> = members.iter().map(|&i| (procs[i].pid, i)).collect();
    // a browser's own process: its parent isn't the same browser (renderers, the GPU process and the rest are)
    let main = |i: usize| {
        browser(&procs[i]).is_some()
            && !by_pid.get(&procs[i].ppid).is_some_and(|&j| j != i && inside.contains(&j) && procs[j].exe == procs[i].exe)
            && !procs[i].cmd.contains("--type=")
            && !procs[i].cmd.contains("-contentproc")
    };
    let automated_main = members.iter().copied().find(|&i| main(i) && automated(&procs[i]));
    let helper_at = members.iter().copied().find(|&i| helper(&procs[i]) || driver(&procs[i]).is_some());
    let kind = match (automated_main, helper_at) {
        (Some(_), _) => Kind::Browser,
        (None, Some(_)) => Kind::Helper,
        _ => Kind::Other,
    };
    let unattended = automated_main == Some(lead) || helper(&procs[lead]) || driver(&procs[lead]).is_some();
    let tool = members.iter().find_map(|&i| tool(&procs[i]));
    let from = tool.unwrap_or("a script");
    let (title, what, project) = if let Some(b) = automated_main {
        let p = &procs[b];
        let name = browser(p).unwrap_or("Browser");
        let headless = p.exe.contains("headless") || p.cmd.to_ascii_lowercase().contains("headless");
        let what = match tool {
            Some(t) => format!("{t}'s {}{name}", if headless { "headless " } else { "" }),
            None => format!("{} {name}", if headless { "headless" } else { "automated" }),
        };
        (format!("{name} from {from}"), what, project_of(p).or_else(|| members.iter().find_map(|&i| project_of(&procs[i]))))
    } else if let Some(h) = helper_at {
        let p = &procs[h];
        let stem = stem(&p.exe);
        let (title, what) = match driver(p) {
            Some(product) => (format!("{product} from {from}"), stem),
            None if stem.starts_with("remotion") => ("Video helper from Remotion".to_string(), "Remotion's compositor".to_string()),
            None if matches!(stem.as_str(), "ffmpeg" | "ffprobe") => ("Video encoder from Remotion".to_string(), format!("Remotion's {stem}")),
            None => (format!("{stem} from Remotion"), format!("Remotion's {stem}")),
        };
        (title, what, project_of(p).or_else(|| members.iter().find_map(|&i| project_of(&procs[i]))))
    } else {
        // the biggest process that isn't a shell or a console host says what the group is
        let named = members.iter().copied().filter(|&i| !plumbing(&procs[i].exe)).max_by_key(|&i| procs[i].mem).unwrap_or(lead);
        (short_label(&procs[named]), label(&procs[named]), members.iter().find_map(|&i| project_of(&procs[i])))
    };
    let listens = |p: &Proc| around.listening.get(&p.pid).is_some_and(|ports| !ports.is_empty());
    let guard = if members.iter().any(|&i| around.windows.contains(&procs[i].pid)) {
        Some(Guard::Window)
    } else if members.iter().any(|&i| {
        let p = &procs[i];
        // a render's own server, feeding its own browser, isn't one you use
        listens(p) && browser(p).is_none() && driver(p).is_none() && (kind == Kind::Other || studio(p))
    }) {
        Some(Guard::Serving)
    } else if members.iter().any(|&i| around.clients.get(&procs[i].pid).is_some_and(|peers| peers.iter().any(|&c| c == 0 || !by_pid.contains_key(&c)))) {
        Some(Guard::InUse)
    } else {
        None
    };
    let mut profiles: Vec<String> = members.iter().filter_map(|&i| temp::profile_of(&procs[i].cmd)).collect();
    profiles.sort();
    profiles.dedup();
    Group {
        key: (procs[lead].pid, procs[lead].started),
        cpu_ms: members.iter().map(|&i| procs[i].cpu_ms).sum(),
        mem: members.iter().map(|&i| procs[i].mem).sum(),
        members,
        kind,
        unattended,
        title,
        what,
        project,
        guard,
        profiles,
    }
}

/// The browser an executable is (any of its processes).
fn browser(p: &Proc) -> Option<&'static str> {
    let exe = p.exe.trim_end_matches(".exe");
    Some(match exe {
        "chrome" | "google-chrome" | "chrome-headless-shell" => "Chrome",
        "headless_shell" | "chromium" | "chromium-browser" => "Chromium",
        "msedge" | "microsoft-edge" => "Edge",
        "brave" => "Brave",
        "firefox" => "Firefox",
        _ => return None,
    })
}

/// A browser driver (Selenium's, WebdriverIO's): it only runs for a script.
fn driver(p: &Proc) -> Option<&'static str> {
    Some(match p.exe.trim_end_matches(".exe") {
        "chromedriver" => "ChromeDriver",
        "msedgedriver" => "EdgeDriver",
        "geckodriver" => "GeckoDriver",
        _ => return None,
    })
}

/// Started by a script rather than by you: headless, driven over DevTools, or from a tool's own download.
fn automated(p: &Proc) -> bool {
    let cmd = p.cmd.to_ascii_lowercase();
    p.exe.contains("headless")
        || ["--headless", "--enable-automation", "--remote-debugging-pipe", "--remote-debugging-port", "-juggler", "-marionette", " -headless"].iter().any(|m| cmd.contains(m))
        || tool(p).is_some()
}

/// One of Remotion's helpers (its compositor, its ffmpeg): useless once Remotion is gone.
fn helper(p: &Proc) -> bool {
    let path = p.path.to_ascii_lowercase().replace('\\', "/");
    browser(p).is_none() && path.contains("/@remotion/")
}

/// Remotion Studio: a server you use, whatever it runs under it.
fn studio(p: &Proc) -> bool {
    let hay = format!("{} {}", p.path, p.cmd).to_ascii_lowercase();
    hay.contains("remotion") && (hay.contains(" studio") || hay.contains(" preview"))
}

/// The tool a process belongs to, by where it lives or how it was started.
fn tool(p: &Proc) -> Option<&'static str> {
    let hay = format!("{} {}", p.path, p.cmd).to_ascii_lowercase();
    [("remotion", "Remotion"), ("playwright", "Playwright"), ("puppeteer", "Puppeteer"), ("cypress", "Cypress"), ("selenium", "Selenium"), ("webdriverio", "WebdriverIO")]
        .into_iter()
        .find(|(k, _)| hay.contains(k))
        .map(|(_, v)| v)
}

/// Shells and console hosts: they carry a group, they aren't what it is.
fn plumbing(exe: &str) -> bool {
    matches!(exe.trim_end_matches(".exe"), "cmd" | "bash" | "sh" | "zsh" | "pwsh" | "powershell" | "conhost" | "openconsole" | "npm" | "npx")
}

/// A process's file name without its extension.
fn stem(exe: &str) -> String {
    exe.rsplit_once('.').map_or(exe, |(s, _)| s).to_string()
}

fn runner(name: &str) -> bool {
    matches!(name, "node" | "bun" | "deno" | "python" | "pythonw" | "py" | "python3" | "ruby")
}

/// The script a runner runs.
fn script(p: &Proc) -> Option<String> {
    args(&p.cmd).into_iter().skip(1).find(|a| !a.starts_with('-')).map(|s| script_name(&s))
}

/// What a process is, exactly: its name, and for a script runner the script ("node · vite").
fn label(p: &Proc) -> String {
    let name = stem(&p.exe);
    match runner(&name).then(|| script(p)).flatten() {
        Some(script) => format!("{name} · {script}"),
        None => name,
    }
}

/// What to call a process: the script a runner runs ("vite"), else its name.
fn short_label(p: &Proc) -> String {
    let name = stem(&p.exe);
    runner(&name).then(|| script(p)).flatten().unwrap_or(name)
}

/// A script's name: its package when it lives in node_modules (`vite`, `@remotion/cli`), else its file's name (its
/// folder's, for an `index.js`).
fn script_name(path: &str) -> String {
    let norm = path.replace('\\', "/");
    if let Some(at) = norm.rfind("node_modules/") {
        let mut parts = norm[at + "node_modules/".len()..].split('/').filter(|s| !s.is_empty());
        if let Some(first) = parts.next() {
            if first.starts_with('@')
                && let Some(second) = parts.next()
            {
                return format!("{first}/{second}");
            }
            if first != ".bin" {
                return first.to_string();
            }
        }
    }
    let mut parts = norm.rsplit('/').filter(|s| !s.is_empty());
    let file = parts.next().unwrap_or(&norm);
    let stem = file.split('.').next().unwrap_or(file);
    match stem {
        "index" | "main" | "cli" => parts.next().map_or(stem, |d| d).to_string(),
        _ => stem.to_string(),
    }
}

/// The folder a process came from, when its path (or its script's) runs through one's node_modules.
fn project_of(p: &Proc) -> Option<String> {
    let hay = format!("{} {}", p.path, p.cmd).replace('\\', "/");
    hay.match_indices("/node_modules/").find_map(|(at, _)| {
        let name = hay[..at].rsplit(['/', ' ', '"']).next()?.trim_matches('"');
        // global installs and npx's cache aren't projects
        let global = matches!(name.to_ascii_lowercase().as_str(), "nodejs" | "node" | "lib" | "npm" | "pnpm" | ".pnpm" | "_npx" | "npm-cache" | "node_global" | "bun" | ".bun");
        let hash = name.len() >= 12 && name.chars().all(|c| c.is_ascii_hexdigit());
        (!name.is_empty() && !name.ends_with(':') && !global && !hash).then(|| name.to_string())
    })
}

/// A command line's arguments, quotes honoured (Windows' rules, near enough).
fn args(cmd: &str) -> Vec<String> {
    let (mut out, mut cur, mut quoted) = (vec![], String::new(), false);
    for c in cmd.chars() {
        match c {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// "a", "a and b", "a, b and c".
pub fn and_list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// "40 s", "6 min", "1 h 20 min", "2 days".
pub fn duration(secs: u64) -> String {
    match secs {
        s if s < 60 => format!("{s} s"),
        s if s < 3600 => format!("{} min", s / 60),
        s if s < 86_400 => match (s / 3600, s % 3600 / 60) {
            (h, 0) => format!("{h} h"),
            (h, m) => format!("{h} h {m} min"),
        },
        s => match s / 86_400 {
            1 => "a day".into(),
            d => format!("{d} days"),
        },
    }
}

/// "6.7 GB", "312 MB", "0 MB".
pub fn bytes(n: u64) -> String {
    const GB: f64 = 1024.0 * 1024.0 * 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    let f = n as f64;
    if n == 0 {
        "0 MB".into()
    } else if f >= GB {
        let v = f / GB;
        if v >= 100.0 { format!("{v:.0} GB") } else { format!("{v:.1} GB") }
    } else if f >= MB {
        format!("{:.0} MB", f / MB)
    } else {
        format!("{:.0} KB", (f / 1024.0).max(1.0))
    }
}
