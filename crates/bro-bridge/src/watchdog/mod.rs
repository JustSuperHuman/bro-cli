//! Watchdogs: keep one session working toward the user's milestones while they are away.
//!
//! bro decides *when* a watchdog wakes (its session stopped and sat idle, or waits on a question) and *who*
//! reviews (a login with usage left). The bridge keeps each watchdog's plan and log and runs the review: a
//! fresh headless agent ([`run`]) that reads the session and the project through the tools at `/mcp`, then
//! hands the session its next task, clears its context, opens a second-opinion session, or stands down.
//! The reviewer reports back through the `watchdog_update` tool and its final answer.

mod prompt;
pub(crate) mod run;

pub use prompt::DEFAULT_PROMPT;

use crate::model::iso_now;
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

/// Wake-ups kept in a watchdog's log.
const MAX_RUNS: usize = 30;
/// Tool steps kept per wake-up.
const MAX_STEPS: usize = 40;

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MilestoneStatus {
    #[default]
    Pending,
    /// The one the session is on now.
    Active,
    Done,
}

impl MilestoneStatus {
    pub fn parse(text: &str) -> Option<MilestoneStatus> {
        match text.trim().to_ascii_lowercase().as_str() {
            "pending" | "todo" | "next" => Some(MilestoneStatus::Pending),
            "active" | "current" | "in_progress" | "in-progress" | "working" => Some(MilestoneStatus::Active),
            "done" | "complete" | "completed" | "finished" => Some(MilestoneStatus::Done),
            _ => None,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            MilestoneStatus::Pending => "pending",
            MilestoneStatus::Active => "active",
            MilestoneStatus::Done => "done",
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Milestone {
    pub title: String,
    pub status: MilestoneStatus,
    /// The reviewer's latest word on it ("tests pass, docs missing").
    pub note: Option<String>,
}

impl Milestone {
    pub fn new(title: impl Into<String>) -> Milestone {
        Milestone { title: title.into(), ..Milestone::default() }
    }
}

/// What the user asked the watchdog for.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct WatchdogPlan {
    pub milestones: Vec<Milestone>,
    /// The reviewer may clear the session's context before a new task.
    pub fresh_context: bool,
    /// The reviewer may open a second-opinion session on another agent (an Opus pass over Codex UI work).
    pub second_opinions: bool,
    pub max_wakes: u32,
    /// Idle seconds after a stop before bro wakes the watchdog (bro's trigger reads it).
    pub idle_secs: u64,
}

impl Default for WatchdogPlan {
    fn default() -> Self {
        Self { milestones: vec![], fresh_context: true, second_opinions: true, max_wakes: 25, idle_secs: 60 }
    }
}

impl WatchdogPlan {
    /// "2/5": milestones done of all.
    pub fn progress(&self) -> (usize, usize) {
        (self.milestones.iter().filter(|m| m.status == MilestoneStatus::Done).count(), self.milestones.len())
    }
}

/// One wake-up, finished.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct RunRecord {
    pub started_at: String,
    pub finished_at: String,
    /// "claude · work · opus"
    pub reviewer: String,
    /// Why it woke ("idle 1m after finishing a turn").
    pub reason: String,
    /// The reviewer's report.
    pub summary: String,
    pub ok: bool,
    /// One line per tool call ("Read Mal's chat", "Typed into Mal: …").
    pub steps: Vec<String>,
}

/// The wake-up in progress.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Running {
    pub started_at: String,
    pub reviewer: String,
    pub reason: String,
    pub steps: Vec<String>,
}

/// Everything about one session's watchdog.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct WatchdogSnapshot {
    pub session_id: String,
    pub plan: WatchdogPlan,
    pub started_at: String,
    /// Wake-ups so far (finished and running).
    pub wakes: u32,
    pub paused: bool,
    /// Why it stood down, once it has (milestones done, out of wake-ups, the reviewer said so).
    pub finished: Option<String>,
    /// Why observation stopped. This is separate from accepting a durable task.
    pub stop_reason: Option<WatchdogStopReason>,
    /// Legacy milestone notes never constitute runtime task acceptance.
    pub accepted: bool,
    pub running: Option<Running>,
    /// Newest last.
    pub runs: Vec<RunRecord>,
    /// Sessions the reviewer opened for this watchdog (second opinions, refinement passes).
    pub helpers: Vec<String>,
    /// Bumped on every change.
    pub revision: u64,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum WatchdogStopReason {
    ReviewerStopped,
    WakeBudgetExhausted,
}

impl WatchdogSnapshot {
    /// Can be woken now.
    pub fn wakeable(&self) -> bool {
        self.running.is_none() && !self.paused && self.finished.is_none() && self.wakes < self.plan.max_wakes
    }
}

/// Which CLI reviews.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ReviewerKind {
    /// `claude -p` with bro's MCP tools.
    Claude,
    /// `codex exec --json` with bro's MCP tools.
    Codex,
}

/// How to start the reviewer: bro fills it in for the login it picked.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ReviewerLaunch {
    pub kind: ReviewerKind,
    /// `claude` / `codex`, resolved like bro resolves agents.
    pub program: String,
    pub env: Vec<(String, String)>,
    pub env_remove: Vec<String>,
    pub model: Option<String>,
    /// "claude · work · opus", for the log.
    pub label: String,
}

/// The session being watched, as bro describes it to the reviewer.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct WatchedSession {
    pub title: String,
    /// "claude · work · opus"
    pub agent: String,
    pub cwd: PathBuf,
}

/// One wake-up request.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct WatchdogWake {
    pub reason: String,
    pub reviewer: ReviewerLaunch,
    /// The reviewer's system prompt (the user's from settings, else [`DEFAULT_PROMPT`]).
    pub prompt: String,
    pub session: WatchedSession,
}

/// What changed, for bro's UI. Runs on a bridge thread; don't block.
#[derive(Clone, Debug, PartialEq)]
pub enum WatchdogUpdate {
    /// Plan, state or log changed; `snapshot` is None once the watchdog is gone.
    Changed { session_id: String, snapshot: Option<Box<WatchdogSnapshot>> },
    Started { session_id: String, reviewer: String, reason: String },
    Step { session_id: String, summary: String },
    Finished { session_id: String, summary: String, ok: bool },
    /// The reviewer stood the watchdog down.
    StoodDown { session_id: String, why: String },
}

pub(crate) type UpdateSink = Arc<dyn Fn(WatchdogUpdate) + Send + Sync>;

struct Entry {
    snapshot: WatchdogSnapshot,
    /// Ends the running reviewer.
    kill: Option<tokio::sync::oneshot::Sender<()>>,
    generation: u64,
}

/// Every watchdog, keyed by the watched session's id.
#[derive(Default)]
pub(crate) struct Watchdogs {
    entries: Mutex<HashMap<String, Entry>>,
    sinks: RwLock<Vec<UpdateSink>>,
    generation: Mutex<u64>,
}

impl Watchdogs {
    pub fn on_update(&self, sink: UpdateSink) {
        self.sinks.write().push(sink);
    }

    fn emit(&self, update: WatchdogUpdate) {
        let sinks = self.sinks.read().clone();
        for sink in sinks {
            sink(update.clone());
        }
    }

    fn changed(&self, session_id: &str) {
        let snapshot = self.get(session_id).map(Box::new);
        self.emit(WatchdogUpdate::Changed { session_id: session_id.to_string(), snapshot });
    }

    /// Change one watchdog and announce it; None when there is none.
    fn edit<R>(&self, session_id: &str, f: impl FnOnce(&mut Entry) -> R) -> Option<R> {
        let out = {
            let mut entries = self.entries.lock();
            let entry = entries.get_mut(session_id)?;
            let out = f(entry);
            entry.snapshot.revision += 1;
            out
        };
        self.changed(session_id);
        Some(out)
    }

    /// A new watchdog (replacing one the session had).
    pub fn start(&self, session_id: &str, plan: WatchdogPlan) -> WatchdogSnapshot {
        let snapshot = WatchdogSnapshot { session_id: session_id.to_string(), plan, started_at: iso_now(), revision: 1, ..WatchdogSnapshot::default() };
        let old = self.entries.lock().insert(session_id.to_string(), Entry { snapshot: snapshot.clone(), kill: None, generation: 0 });
        if let Some(kill) = old.and_then(|old| old.kill) {
            let _ = kill.send(());
        }
        self.changed(session_id);
        snapshot
    }

    /// New milestones and options; the log and wake count stay. Milestones the user kept keep their status.
    pub fn set_plan(&self, session_id: &str, plan: WatchdogPlan) -> bool {
        self.edit(session_id, |entry| {
            let s = &mut entry.snapshot;
            // a finished watchdog given more to do (or more wake-ups) watches again
            if s.finished.is_some() && (plan.milestones.iter().any(|m| m.status != MilestoneStatus::Done) || plan.max_wakes > s.wakes) {
                s.finished = None;
                s.stop_reason = None;
            }
            s.plan = plan;
        })
        .is_some()
    }

    pub fn stop(&self, session_id: &str) -> bool {
        let Some(entry) = self.entries.lock().remove(session_id) else { return false };
        if let Some(kill) = entry.kill {
            let _ = kill.send(());
        }
        self.changed(session_id);
        true
    }

    pub fn set_paused(&self, session_id: &str, paused: bool) -> bool {
        self.edit(session_id, |entry| entry.snapshot.paused = paused).is_some()
    }

    pub fn get(&self, session_id: &str) -> Option<WatchdogSnapshot> {
        self.entries.lock().get(session_id).map(|entry| entry.snapshot.clone())
    }

    pub fn all(&self) -> Vec<WatchdogSnapshot> {
        let mut all: Vec<WatchdogSnapshot> = self.entries.lock().values().map(|entry| entry.snapshot.clone()).collect();
        all.sort_by(|a, b| a.started_at.cmp(&b.started_at));
        all
    }

    /// Claims the next wake-up: returns its generation and the snapshot it starts from, or why it can't.
    pub fn begin(&self, session_id: &str, reason: &str, reviewer: &str, kill: tokio::sync::oneshot::Sender<()>) -> Result<(u64, WatchdogSnapshot), String> {
        let generation = {
            let mut next = self.generation.lock();
            *next += 1;
            *next
        };
        let snapshot = {
            let mut entries = self.entries.lock();
            let entry = entries.get_mut(session_id).ok_or("This session has no watchdog.")?;
            let s = &mut entry.snapshot;
            if s.running.is_some() {
                return Err("The watchdog is already reviewing.".into());
            }
            if s.paused {
                return Err("The watchdog is paused.".into());
            }
            if let Some(why) = &s.finished {
                return Err(format!("The watchdog stood down: {why}"));
            }
            if s.wakes >= s.plan.max_wakes {
                s.finished = Some(format!("used all {} wake-ups", s.plan.max_wakes));
                s.stop_reason = Some(WatchdogStopReason::WakeBudgetExhausted);
                return Err("The watchdog has used all its wake-ups.".into());
            }
            s.wakes += 1;
            s.running = Some(Running { started_at: iso_now(), reviewer: reviewer.to_string(), reason: reason.to_string(), steps: vec![] });
            s.revision += 1;
            entry.kill = Some(kill);
            entry.generation = generation;
            s.clone()
        };
        self.emit(WatchdogUpdate::Started { session_id: session_id.to_string(), reviewer: reviewer.to_string(), reason: reason.to_string() });
        self.changed(session_id);
        Ok((generation, snapshot))
    }

    /// A tool call of the running wake-up.
    pub fn step(&self, session_id: &str, generation: u64, summary: String) {
        let ok = self
            .edit(session_id, |entry| {
                let current = entry.generation == generation;
                if let Some(running) = entry.snapshot.running.as_mut().filter(|_| current) {
                    running.steps.push(summary.clone());
                    if running.steps.len() > MAX_STEPS {
                        running.steps.remove(0);
                    }
                }
                current
            })
            .unwrap_or(false);
        if ok {
            self.emit(WatchdogUpdate::Step { session_id: session_id.to_string(), summary });
        }
    }

    /// The wake-up ended: log it.
    pub fn finish(&self, session_id: &str, generation: u64, summary: String, ok: bool) {
        let logged = self
            .edit(session_id, |entry| {
                if entry.generation != generation {
                    return false;
                }
                entry.kill = None;
                let s = &mut entry.snapshot;
                let Some(running) = s.running.take() else { return false };
                s.runs.push(RunRecord { started_at: running.started_at, finished_at: iso_now(), reviewer: running.reviewer, reason: running.reason, summary: summary.clone(), ok, steps: running.steps });
                if s.runs.len() > MAX_RUNS {
                    s.runs.remove(0);
                }
                if s.finished.is_none() && s.wakes >= s.plan.max_wakes {
                    s.finished = Some(format!("used all {} wake-ups", s.plan.max_wakes));
                    s.stop_reason = Some(WatchdogStopReason::WakeBudgetExhausted);
                }
                true
            })
            .unwrap_or(false);
        if logged {
            self.emit(WatchdogUpdate::Finished { session_id: session_id.to_string(), summary, ok });
        }
    }

    pub fn add_helper(&self, session_id: &str, helper: &str) -> bool {
        self.edit(session_id, |entry| {
            if !entry.snapshot.helpers.iter().any(|h| h == helper) {
                entry.snapshot.helpers.push(helper.to_string());
            }
        })
        .is_some()
    }

    /// The `watchdog_update` tool: milestone statuses and notes, new milestones, standing down.
    pub fn apply(&self, session_id: &str, args: &Value) -> Result<Value, String> {
        let mut stood_down = None;
        let result = self.edit(session_id, |entry| -> Result<Value, String> {
            let s = &mut entry.snapshot;
            let mut changes = vec![];
            for change in args.get("milestones").and_then(Value::as_array).into_iter().flatten() {
                let number = change.get("number").and_then(Value::as_u64).ok_or("Each milestone change needs its number (1-based).")? as usize;
                let total = s.plan.milestones.len();
                let milestone = s.plan.milestones.get_mut(number.wrapping_sub(1)).ok_or_else(|| format!("There is no milestone {number}; the plan has {total}."))?;
                if let Some(status) = change.get("status").and_then(Value::as_str) {
                    milestone.status = MilestoneStatus::parse(status).ok_or_else(|| format!("Unknown status \"{status}\": use pending, active or done."))?;
                }
                if let Some(note) = change.get("note").and_then(Value::as_str) {
                    let note = note.trim();
                    milestone.note = (!note.is_empty()).then(|| note.to_string());
                }
                changes.push(format!("{number} {}", milestone.status.label()));
            }
            // one milestone is active at a time: the newest one marked active wins
            if let Some(active) = s.plan.milestones.iter().rposition(|m| m.status == MilestoneStatus::Active) {
                for (i, m) in s.plan.milestones.iter_mut().enumerate() {
                    if i != active && m.status == MilestoneStatus::Active {
                        m.status = MilestoneStatus::Pending;
                    }
                }
            }
            for title in args.get("add").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
                if !title.trim().is_empty() {
                    s.plan.milestones.push(Milestone::new(title.trim()));
                    changes.push(format!("added \"{}\"", title.trim()));
                }
            }
            if args.get("standDown").and_then(Value::as_bool).unwrap_or(false) {
                let why = args.get("reason").and_then(Value::as_str).map(str::trim).filter(|r| !r.is_empty()).unwrap_or("the reviewer found nothing left to do").to_string();
                s.finished = Some(why.clone());
                s.stop_reason = Some(WatchdogStopReason::ReviewerStopped);
                stood_down = Some(why);
                changes.push("stood down".into());
            }
            let (done, total) = s.plan.progress();
            Ok(json!({ "changes": changes, "progress": format!("{done}/{total} done"), "milestones": s.plan.milestones, "stopReason": s.stop_reason, "accepted": false }))
        });
        let result = result.ok_or("This session has no watchdog.")??;
        if let Some(why) = stood_down {
            self.emit(WatchdogUpdate::StoodDown { session_id: session_id.to_string(), why });
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(titles: &[&str]) -> WatchdogPlan {
        WatchdogPlan { milestones: titles.iter().map(|t| Milestone::new(*t)).collect(), max_wakes: 2, ..WatchdogPlan::default() }
    }

    #[test]
    fn wakes_are_claimed_one_at_a_time_and_budgeted() {
        let dogs = Watchdogs::default();
        dogs.start("s", plan(&["a"]));
        let (kill, _) = tokio::sync::oneshot::channel();
        let (generation, snapshot) = dogs.begin("s", "idle", "claude · opus", kill).unwrap();
        assert_eq!(snapshot.wakes, 1);
        let (kill, _) = tokio::sync::oneshot::channel();
        assert!(dogs.begin("s", "idle", "claude", kill).unwrap_err().contains("already"));
        dogs.step("s", generation, "Read Mal's chat".into());
        dogs.step("s", generation + 99, "stale".into());
        dogs.finish("s", generation, "sent milestone 1".into(), true);
        let s = dogs.get("s").unwrap();
        assert!(s.running.is_none());
        assert_eq!(s.runs[0].steps, vec!["Read Mal's chat".to_string()]);
        let (kill, _) = tokio::sync::oneshot::channel();
        let (generation, _) = dogs.begin("s", "idle", "claude", kill).unwrap();
        dogs.finish("s", generation, "again".into(), true);
        let s = dogs.get("s").unwrap();
        assert_eq!(s.finished.as_deref(), Some("used all 2 wake-ups"));
        assert!(!s.wakeable());
        // more wake-ups bring it back
        assert!(dogs.set_plan("s", WatchdogPlan { max_wakes: 5, ..s.plan.clone() }));
        assert!(dogs.get("s").unwrap().wakeable());
    }

    #[test]
    fn the_reviewer_updates_milestones_and_stands_down() {
        let dogs = Watchdogs::default();
        let updates = Arc::new(Mutex::new(vec![]));
        let sink = updates.clone();
        dogs.on_update(Arc::new(move |u| sink.lock().push(u)));
        dogs.start("s", plan(&["sync", "settings"]));
        let out = dogs.apply("s", &json!({ "milestones": [{ "number": 1, "status": "done", "note": "tests pass" }, { "number": 2, "status": "active" }], "add": ["docs"] })).unwrap();
        assert_eq!(out["progress"], "1/3 done");
        let s = dogs.get("s").unwrap();
        assert_eq!(s.plan.milestones[0].note.as_deref(), Some("tests pass"));
        assert_eq!(s.plan.milestones[1].status, MilestoneStatus::Active);
        assert_eq!(s.plan.milestones[2].title, "docs");
        dogs.apply("s", &json!({ "milestones": [{ "number": 3, "status": "active" }] })).unwrap();
        assert_eq!(dogs.get("s").unwrap().plan.milestones[1].status, MilestoneStatus::Pending, "one active at a time");
        assert!(dogs.apply("s", &json!({ "milestones": [{ "number": 9, "status": "done" }] })).is_err());
        assert!(dogs.apply("s", &json!({ "milestones": [{ "number": 1, "status": "maybe" }] })).is_err());
        dogs.apply("s", &json!({ "standDown": true, "reason": "every milestone verified" })).unwrap();
        assert_eq!(dogs.get("s").unwrap().finished.as_deref(), Some("every milestone verified"));
        assert!(updates.lock().iter().any(|u| matches!(u, WatchdogUpdate::StoodDown { why, .. } if why == "every milestone verified")));
        assert!(dogs.apply("other", &json!({})).is_err());
        assert!(dogs.stop("s"));
        assert!(dogs.get("s").is_none());
    }
}
