//! The background tasks a session leaves running while it goes on (or waits on you): Claude Code's backgrounded
//! commands (`run_in_background`, or a command you sent to the background), its Monitors and its workflows. Read
//! with the rest of its conversation ([`super::conversation::Conversation::tasks`]):
//!
//! - one starts when its call's result names a task (`backgroundTaskId` for a command, `taskId` for a Monitor or a
//!   workflow), and is called what the call was ("Re-render the three ads", "Watching the EAS builds");
//! - it ends with a `<task-notification>` that has a `<status>` (a Monitor's events have none: it watches on, its
//!   newest event kept), with a `TaskStop` of it, or when a Monitor's timeout runs out;
//! - none outlives the session's process: one started before it is over ([`Task::live`]).

use super::subagents::SubagentState;
use serde_json::Value;

/// A conversation remembers at most this many tasks (the newest).
pub const REMEMBER_MAX: usize = 100;

/// What sort of background task it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum TaskKind {
    /// a command running in the background
    #[default]
    Command,
    /// a Monitor: a command watched for what it prints
    Watch,
    /// a workflow of agents
    Workflow,
}

/// One background task, as far as the session's transcript says.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Task {
    /// Claude Code's id for it ("bzzk17aji")
    pub id: String,
    /// the call that started it
    pub call: String,
    pub kind: TaskKind,
    /// what it is, in a few words: the call's own ("Re-render the three ads")
    pub title: String,
    /// what it runs, in full (a command)
    pub detail: Option<String>,
    pub state: SubagentState,
    pub started_ms: Option<i64>,
    pub ended_ms: Option<i64>,
    /// when it ends on its own (a Monitor's timeout)
    pub expires_ms: Option<i64>,
    /// the file its output goes to
    pub output: Option<String>,
    /// the newest it said: a Monitor's latest event, how a command ended ("exit code 1")
    pub news: Option<String>,
}

impl Task {
    /// At work at `now_ms` in a session whose process started at `since_ms`: running as the transcript says, short
    /// of its timeout, and started by this process (a task doesn't outlive the process that ran it).
    pub fn live(&self, now_ms: i64, since_ms: Option<i64>) -> bool {
        self.state.running()
            && self.expires_ms.is_none_or(|e| now_ms < e)
            && since_ms.zip(self.started_ms).is_none_or(|(since, start)| start + 1000 >= since)
    }

    /// How long it ran, or has run so far, in ms.
    pub fn elapsed_ms(&self, now_ms: i64) -> Option<i64> {
        let start = self.started_ms?;
        let end = if self.state.running() { self.expires_ms.map_or(now_ms, |e| e.min(now_ms)) } else { self.ended_ms.unwrap_or(now_ms) };
        Some((end - start).max(0))
    }
}

/// The task a call's result says it started (`structured` its `toolUseResult`, `text` its words): its id and sort,
/// the file its output goes to, and when a Monitor gives up on its own (ms from now).
pub(crate) fn started(structured: Option<&Value>, text: &str) -> Option<(String, TaskKind, Option<String>, Option<i64>)> {
    let s = structured?;
    let str_of = |k: &str| s.get(k).and_then(Value::as_str).map(str::trim).filter(|v| !v.is_empty());
    if let Some(id) = str_of("backgroundTaskId") {
        let output = str_of("backgroundOutputPath").map(str::to_string).or_else(|| output_path(text));
        return Some((id.to_string(), TaskKind::Command, output, None));
    }
    let id = str_of("taskId")?;
    // a workflow names its run; a watch says how long it keeps watching
    if s.get("runId").is_some() || text.trim_start().starts_with("Workflow launched") {
        return Some((id.to_string(), TaskKind::Workflow, None, None));
    }
    let lasts = (s.get("persistent").and_then(Value::as_bool) != Some(true)).then(|| s.get("timeoutMs").and_then(Value::as_i64)).flatten().filter(|ms| *ms > 0);
    Some((id.to_string(), TaskKind::Watch, None, lasts))
}

/// "Output is being written to: C:\…\b1.output." in what a backgrounded command said back.
fn output_path(text: &str) -> Option<String> {
    let (_, rest) = text.split_once("Output is being written to: ")?;
    let path = rest.split(['\n', '\r']).next()?.trim();
    let path = path.split_once(". ").map_or(path, |(p, _)| p).trim_end_matches('.').trim();
    (!path.is_empty()).then(|| path.to_string())
}

/// The task a `TaskStop` (or an older `KillShell`) stopped, from what it said back.
pub(crate) fn stopped(structured: Option<&Value>, input: Option<&Value>) -> Option<String> {
    let id = |v: Option<&Value>, k: &str| v.and_then(|v| v.get(k)).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    id(structured, "task_id").or_else(|| id(structured, "shell_id")).or_else(|| id(input, "task_id")).or_else(|| id(input, "shell_id")).or_else(|| id(input, "bash_id"))
}

/// What a notification's summary adds to how a command went: "exit code 1" of
/// `Background command "Build" failed with exit code 1` (or `completed (exit code 0)`).
pub(crate) fn outcome(summary: &str) -> Option<String> {
    let at = summary.rfind("exit code ")? + "exit code ".len();
    let code: String = summary[at..].chars().take_while(|c| c.is_ascii_digit() || *c == '-').collect();
    (!code.is_empty()).then(|| format!("exit code {code}"))
}

/// A watch's event that says it stopped watching ("[Monitor timed out …]", "[Monitor stopped]").
pub(crate) fn watch_over(text: &str) -> bool {
    ["[Monitor timed out", "[Monitor expired", "[Monitor stopped"].iter().any(|m| text.contains(m))
}

/// The newest line of a watch's event, as its news.
pub(crate) fn event_line(text: &str) -> Option<String> {
    let line = text.lines().map(str::trim).rfind(|l| !l.is_empty())?;
    let cut: String = line.chars().take(120).collect();
    Some(cut)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_result_says_which_task_it_started() {
        let bash = json!({"stdout":"","stderr":"","interrupted":false,"backgroundTaskId":"bzzk17aji"});
        let said = "Command running in background with ID: bzzk17aji. Output is being written to: C:\\Temp\\tasks\\bzzk17aji.output. You will be notified when it completes.";
        assert_eq!(started(Some(&bash), said), Some(("bzzk17aji".into(), TaskKind::Command, Some("C:\\Temp\\tasks\\bzzk17aji.output".into()), None)));
        let watch = json!({"taskId":"bwen2xvol","timeoutMs":1_800_000,"persistent":false});
        assert_eq!(started(Some(&watch), "Monitor started (task bwen2xvol, …)"), Some(("bwen2xvol".into(), TaskKind::Watch, None, Some(1_800_000))));
        let forever = json!({"taskId":"bw2","timeoutMs":1_800_000,"persistent":true});
        assert_eq!(started(Some(&forever), "").map(|t| t.3), Some(None), "a persistent watch has no timeout");
        let flow = json!({"status":"launched","taskId":"w9","runId":"wf_abc123"});
        assert_eq!(started(Some(&flow), "Workflow launched in background. Task ID: w9").map(|t| t.1), Some(TaskKind::Workflow));
        assert_eq!(started(Some(&json!({"stdout":"hi"})), "hi"), None, "a command that ran to its end started nothing");
        assert_eq!(started(None, said), None);
    }

    #[test]
    fn a_stop_names_its_task_and_a_summary_how_it_went() {
        assert_eq!(stopped(Some(&json!({"message":"Successfully stopped task: b1","task_id":"b1","task_type":"local_bash"})), None).as_deref(), Some("b1"));
        assert_eq!(stopped(None, Some(&json!({"shell_id":"s2"}))).as_deref(), Some("s2"));
        assert_eq!(outcome("Background command \"Build\" failed with exit code 1").as_deref(), Some("exit code 1"));
        assert_eq!(outcome("Background command \"Build\" completed (exit code 0)").as_deref(), Some("exit code 0"));
        assert_eq!(outcome("Agent \"x\" finished"), None);
        assert!(watch_over("[Monitor timed out. Re-arm it only if …]"));
        assert_eq!(event_line("sheet ready: a.png\nsheet ready: b.png\n").as_deref(), Some("sheet ready: b.png"));
    }

    #[test]
    fn a_task_is_live_while_it_runs_in_this_process_short_of_its_timeout() {
        let t = Task { id: "b1".into(), started_ms: Some(10_000), expires_ms: Some(70_000), ..Task::default() };
        assert!(t.live(20_000, Some(5_000)));
        assert!(!t.live(80_000, Some(5_000)), "past its timeout");
        assert!(!t.live(20_000, Some(60_000)), "started before this process");
        assert_eq!(t.elapsed_ms(90_000), Some(60_000), "a watch that timed out ran until then");
        let done = Task { state: SubagentState::Done, ended_ms: Some(15_000), ..t };
        assert!(!done.live(12_000, None));
        assert_eq!(done.elapsed_ms(90_000), Some(5_000));
    }
}
