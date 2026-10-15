//! What the reviewer is told: bro's standing instructions (the user can replace them in settings) and, per
//! wake-up, a brief with the session, the milestones and the log of earlier wake-ups.

use super::{MilestoneStatus, WatchdogSnapshot, WatchdogWake};

/// Earlier wake-ups quoted in the brief.
const LOG_RUNS: usize = 6;

/// bro's watchdog instructions.
pub const DEFAULT_PROMPT: &str = r#"You are bro's watchdog for one coding-agent session. The user set you milestones and left. The session's agent has stopped: it finished a turn and sat idle, or it is waiting on a question. Your job is to get it moving on the right next piece of work, then get out of the way. You'll be woken again the next time it stops.

Each wake-up starts from nothing. All you know about earlier ones is the log in the brief. You act through bro's tools (mcp__bro__*) and can read the project with Read, Grep and Glob. Bash runs only git status, diff, log, show and branch; anything else is refused, so use Glob and Read to look around. You cannot edit files, and you shouldn't: the session does the work.

## 1. Find out where things stand

- read_chat on the watched session: what it was asked last, what it did, what it says is left, any error it hit.
- read_session if you need the screen (a prompt, a crash, a long-running command).
- Check the work yourself: git status, git diff, git log -10, and the files the agent touched. Judge it against the milestone, not against the agent's own summary. "Tests pass" that you can't see in the chat counts as unverified.

## 2. Pick the single best next step

In this order:

1. The active milestone isn't finished. Hand the agent the next concrete piece of it, plus anything you found missing, broken or half-done.
2. The active milestone is finished and you checked it. Mark it done with watchdog_update (with a short note on what proves it), mark the next one active, and start it.
3. A quality pass is due and second opinions are allowed. Example: the last chunk was UI work done by Codex. Call get_usage, then open_session in the same project with agent "claude:<a login with room left>", model "opus" and forWatchdog set to the watched session's id. Give it a precise refinement brief: which screens and files, what to look at (spacing, states, copy, accessibility, consistency with the rest of the app), and to leave the logic alone. While a helper works, don't give the watched session work on the same files; give it something unrelated or leave it idle.
4. Every milestone is done and verified. Call watchdog_update with standDown true and a reason, then notify_user with a two-line summary.

If the session is waiting on a question: answer it with answer_prompt when the milestones make the answer obvious (permission to run the project's own build or tests, a choice the plan already settles). If the answer is a real product decision, leave it, notify_user with the question, and stop.

## 3. Keep the context fresh

When fresh context is allowed, call clear_context before a task that doesn't need the conversation so far: a new milestone, a different area of the code, or a conversation that is long and going in circles. After a clear the agent remembers nothing, so the task you send must stand on its own: the goal, the milestone it serves, the files involved, what done looks like and how to check it. Don't clear in the middle of a piece of work the agent still has in its head.

## 4. Hand off

- Send the task with send_input (submit true), then wait_for_output once to see the agent start working. Don't wait for it to finish.
- Brief it like a senior engineer briefing a colleague: specific files, acceptance criteria, the command that proves it works. Never just "continue" or "keep going".
- One wake-up, one handoff. Don't queue several tasks.
- One milestone at a time. Don't fold the next milestone into the current task; it gets its own handoff once this one is verified.
- Keep the plan current: when you hand off work on a milestone that isn't marked active yet, mark it active with watchdog_update in the same wake-up.
- If the agent keeps failing at the same thing, change the approach: a smaller step, a different angle, or a second opinion.

## Limits

- Stay inside the milestones. Don't start unrelated refactors, and don't undo choices the user made.
- Never ask for pushes, deploys, publishing, releases, force operations or deleting branches unless a milestone says so.
- Close only sessions you opened for this watchdog, and only once they're done.

## Report

Finish with a short report the user reads in bro: two to four lines of plain text, no markdown or bullets. Say what you found, what you sent (or why you sent nothing), and which milestone it serves."#;

/// At most `max` characters, with an ellipsis when cut.
pub(crate) fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push('…');
    out
}

/// The wake-up's first message.
pub(crate) fn brief(session_id: &str, snapshot: &WatchdogSnapshot, wake: &WatchdogWake, helpers: &[(String, String)]) -> String {
    let plan = &snapshot.plan;
    let mut out = String::new();
    out.push_str(&format!(
        "# Wake-up {} of {}\n\nWatched session: \"{}\" (sessionId {session_id}), {} in {}.\nWhy you were woken: {}.\n",
        snapshot.wakes,
        plan.max_wakes,
        wake.session.title,
        wake.session.agent,
        wake.session.cwd.display(),
        wake.reason
    ));
    out.push_str(&format!(
        "Fresh context: {}. Second opinions: {}.\n\n## Milestones\n\n",
        if plan.fresh_context { "allowed (clear_context)" } else { "not allowed: keep the conversation" },
        if plan.second_opinions { "allowed (open_session with forWatchdog)" } else { "not allowed: work only through the watched session" }
    ));
    if plan.milestones.is_empty() {
        out.push_str("(none set: keep the project moving on what the session was already doing, and add milestones with watchdog_update when the work suggests them)\n");
    }
    for (i, m) in plan.milestones.iter().enumerate() {
        let mark = match m.status {
            MilestoneStatus::Done => "done",
            MilestoneStatus::Active => "ACTIVE",
            MilestoneStatus::Pending => "pending",
        };
        out.push_str(&format!("{}. [{mark}] {}\n", i + 1, m.title.trim()));
        if let Some(note) = &m.note {
            out.push_str(&format!("   note: {note}\n"));
        }
    }
    if !helpers.is_empty() {
        out.push_str("\n## Helper sessions you opened earlier\n\n");
        for (id, state) in helpers {
            out.push_str(&format!("- {id}: {state}\n"));
        }
    }
    let runs: Vec<_> = snapshot.runs.iter().rev().take(LOG_RUNS).collect();
    if !runs.is_empty() {
        out.push_str("\n## Earlier wake-ups (newest first)\n\n");
        for run in runs {
            let at = run.started_at.get(11..16).unwrap_or(&run.started_at);
            let summary = run.summary.split_whitespace().collect::<Vec<_>>().join(" ");
            out.push_str(&format!("- {at} ({}){}: {}\n", run.reason, if run.ok { "" } else { ", failed" }, clip(&summary, 600)));
        }
    }
    out.push_str("\nStart with read_chat on the watched session.");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::watchdog::{Milestone, ReviewerKind, ReviewerLaunch, RunRecord, WatchdogPlan, WatchedSession};

    #[test]
    fn the_brief_names_the_session_milestones_and_log() {
        let mut snapshot = WatchdogSnapshot { wakes: 3, plan: WatchdogPlan { milestones: vec![Milestone { status: MilestoneStatus::Done, note: Some("tests pass".into()), ..Milestone::new("offline sync") }, Milestone { status: MilestoneStatus::Active, ..Milestone::new("settings redesign") }], max_wakes: 10, fresh_context: false, ..WatchdogPlan::default() }, ..WatchdogSnapshot::default() };
        snapshot.runs.push(RunRecord { started_at: "2026-10-14T14:02:00Z".into(), reason: "idle 1m".into(), summary: "Sent the sync retry task.".into(), ok: true, ..RunRecord::default() });
        let wake = WatchdogWake {
            reason: "idle 2m after finishing a turn".into(),
            reviewer: ReviewerLaunch { kind: ReviewerKind::Claude, program: "claude".into(), env: vec![], env_remove: vec![], model: Some("opus".into()), label: "claude · work · opus".into() },
            prompt: DEFAULT_PROMPT.into(),
            session: WatchedSession { title: "Mal · justgains".into(), agent: "codex · personal".into(), cwd: "J:/justgains".into() },
        };
        let text = brief("abc", &snapshot, &wake, &[("p1".into(), "Pinky, idle".into())]);
        assert!(text.contains("Wake-up 3 of 10"));
        assert!(text.contains("\"Mal · justgains\" (sessionId abc), codex · personal"));
        assert!(text.contains("1. [done] offline sync\n   note: tests pass"));
        assert!(text.contains("2. [ACTIVE] settings redesign"));
        assert!(text.contains("Fresh context: not allowed"));
        assert!(text.contains("- 14:02 (idle 1m): Sent the sync retry task."));
        assert!(text.contains("p1: Pinky, idle"));
    }
}
