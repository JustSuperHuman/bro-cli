//! The orchestrator's inbox: everything bro hands the orchestrator session goes through here. That covers
//! push-to-talk, messages from the web and phone panels (what the bridge's built-in orchestrator used to
//! answer), scheduled triggers, watchdog wake-ups and its own requests for a fresh start.
//!
//! A message is typed into the running orchestrator once it's at its prompt. With none running, bro starts one
//! (the way it last ran, else on the Claude / Codex login with the most room) and delivers when it's ready.
//! Whoever asked gets the orchestrator's answer back: the live voice reads it out, a panel shows it, a watchdog
//! logs it as its review.

use super::App;
use crate::alerts::Kind;
use crate::layout::PaneId;
use crate::pane::{Activity, Place};
use crate::services::VoiceMsg;
use bro_bridge::OrchestratorCommand;
use bro_core::Harness;
use bro_core::launch::{LaunchSpec, Permission};
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

/// …and at least this long after it starts, so the first keys don't land in its splash.
const SETTLE: Duration = Duration::from_secs(2);
/// A fresh start waits for the turn to be over: idle this long.
const TURN_OVER: Duration = Duration::from_millis(1500);
/// After `/clear`, the next message waits this long for the empty conversation.
const AFTER_CLEAR: Duration = Duration::from_secs(3);
/// A message waiting for an orchestrator that never came.
const GIVE_UP: Duration = Duration::from_secs(90);
/// An answer that hasn't come by now: the asker hears it's still going.
const ANSWER_WAIT: Duration = Duration::from_secs(240);
/// A watchdog review gets longer: it may wait on sessions and read a lot.
const REVIEW_WAIT: Duration = Duration::from_secs(20 * 60);

/// Who gets the orchestrator's answer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) enum Reply {
    None,
    /// the live voice's delegation id
    Voice(String),
    /// a panel message's answer item in the bridge transcript
    Panel(String),
    /// a watchdog wake-up: session id, generation
    Watchdog(String, u64),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) enum What {
    Control {
        text: String,
        reply: Reply,
        target: String,
        kind: String,
    },
    Say {
        text: String,
        reply: Reply,
    },
    /// `/clear` (Codex, Pi, omp: `/new`), then the handoff
    Clear {
        handoff: Option<String>,
    },
}

/// Stable identity survives queueing, context replacement, and application restart.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Request {
    pub(crate) id: String,
    pub(crate) what: What,
    pub(crate) kind: String,
}

impl Request {
    fn new(what: What) -> Self {
        let kind = match &what {
            What::Clear { .. } => "context",
            What::Control { kind, .. } => kind.as_str(),
            What::Say {
                reply: Reply::Watchdog(..),
                ..
            } => "watchdog",
            What::Say { text, .. } if text.starts_with("[schedule ") => "schedule",
            What::Say { text, .. } if text.starts_with("[fresh start") => "handoff",
            What::Say { text, .. } if text.starts_with("[task event]") => "task-event",
            What::Say { text, .. } if text.starts_with("[task action]") => "task-action",
            _ => "assignment",
        }
        .to_owned();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            what,
            kind,
        }
    }

    fn prompt(&self, text: &str) -> String {
        let (channel, handle) = match &self.what {
            What::Say {
                reply: Reply::Panel(id),
                ..
            }
            | What::Control {
                reply: Reply::Panel(id),
                ..
            } => ("panel", id.as_str()),
            What::Say {
                reply: Reply::Voice(id),
                ..
            } => ("voice", id.as_str()),
            What::Say {
                reply: Reply::Watchdog(id, _),
                ..
            } => ("watchdog", id.as_str()),
            _ => ("none", self.id.as_str()),
        };
        let origin = serde_json::json!({"originRequestId":self.id,"replyChannel":channel,"replyHandle":handle});
        format!(
            "[bro-request {} kind={}]\n{text}\n\nDelegated work origin: {origin}. Preserve this JSON object as a string in every delegated task's contract.context so its accepted outcome returns to the original channel. An initial reply acknowledges the request; accepted task outcomes are delivered separately.\nEnd your final response with [bro-response {}]. This identifies this request; it does not certify task acceptance.",
            self.id, self.kind, self.id
        )
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct ReplyStamp {
    text: String,
    at_ms: Option<i64>,
    prompt: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct SavedRoute {
    schema: u32,
    queue: Vec<Request>,
    awaiting: Vec<SavedAwaiting>,
    completed: Vec<String>,
}

impl Default for SavedRoute {
    fn default() -> Self {
        Self {
            schema: 1,
            queue: vec![],
            awaiting: vec![],
            completed: vec![],
        }
    }
}
impl SavedRoute {
    pub(super) fn validate(&self) -> Result<(), String> {
        if self.schema != 1
            || self.queue.len() > 256
            || self.awaiting.len() > 128
            || self.completed.len() > 512
        {
            return Err("unsupported or oversized saved inbox".into());
        }
        let mut ids = std::collections::HashSet::new();
        for id in self
            .queue
            .iter()
            .map(|r| &r.id)
            .chain(self.awaiting.iter().map(|a| &a.request.id))
        {
            if id.is_empty() || !ids.insert(id) {
                return Err("saved inbox has missing or duplicate request identities".into());
            }
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
struct SavedAwaiting {
    request: Request,
    reply: Reply,
    before: Option<ReplyStamp>,
    session_id: String,
    #[serde(default)]
    notified: bool,
}

/// Something delivered, waiting for the orchestrator's answer.
pub(crate) struct Awaiting {
    pub(crate) reply: Reply,
    /// the orchestrator's last reply when it was typed in (the answer is the next one)
    before: Option<ReplyStamp>,
    pub(crate) since: Instant,
    request: Request,
    session_id: String,
    notified: bool,
    dispatched: bool,
}

#[derive(Default)]
pub(crate) struct VoiceRoute {
    pub(crate) queue: Vec<(Request, Instant)>,
    pub(crate) awaiting: Vec<Awaiting>,
    /// The orchestrator is starting for the inbox: the session to keep in view when it opens.
    keep_focus: Option<Option<PaneId>>,
    /// since when the orchestrator has been idle
    pub(crate) idle_since: Option<Instant>,
    /// nothing more goes in before this (a `/clear` settling)
    pub(crate) hold_until: Option<Instant>,
    loaded: bool,
    /// A cancellation quarantines the interrupted reply until the session is ready again.
    cancelled: bool,
    persistence_error: bool,
    completed: Vec<String>,
    store: Option<super::inbox_store::Store>,
    generation: u64,
    saved_generation: u64,
    pending_dispatch: Option<(String, u64)>,
}

impl App {
    pub(crate) fn finish_inbox(&mut self) {
        if !self.voice.loaded {
            self.inbox_fail(
                "Bro closed before this request could be durably queued. It was not sent.",
            );
            return;
        }
        let required = self.save_inbox();
        if let Some(store) = &mut self.voice.store
            && let Err(error) = store.finish(required)
        {
            self.toast(Kind::Error, error);
        }
    }
    #[cfg(test)]
    pub(crate) fn test_inbox_store(
        &mut self,
        path: std::path::PathBuf,
    ) -> std::sync::mpsc::Receiver<crate::pane::Event> {
        let (tx, rx) = std::sync::mpsc::channel();
        self.persist = true;
        self.voice.loaded = false;
        self.voice.store = Some(super::inbox_store::Store::start(path, tx));
        rx
    }
    pub(crate) fn coordination_changed(
        &mut self,
        task_id: &str,
        revision: u64,
        seq: u64,
        project: &std::path::Path,
        action: &str,
    ) {
        self.restore_inbox();
        let action_key: String = action
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' {
                    c
                } else {
                    '-'
                }
            })
            .collect();
        let id = format!("task-event:{task_id}:{revision}:{seq}:{action_key}");
        if self.voice.completed.contains(&id)
            || self.voice.queue.iter().any(|(r, _)| r.id == id)
            || self.voice.awaiting.iter().any(|a| a.request.id == id)
        {
            return;
        }
        let resume = if action == "resumeReady" {
            " This timer wake does not establish useful progress. Inspect current context and the still-assigned attempt. If its existing worker is idle, send an addressed resume control to that same session with the task ID, attempt ID and epoch using send_input. Preserve its assignment and context identities; do not open a duplicate worker or create a new task. If it is busy or awaiting an answer, inspect that state and use the appropriate addressed control."
        } else {
            ""
        };
        let mut request = Request::new(What::Say {
            text: format!(
                "[task event] Task {task_id}, revision {revision}, project {}, changed: {action}. Read coordination_snapshot and coordination_context; continue eligible work and reconcile cancellation across projects. Existing goals and acceptance requirements remain in force.{resume}",
                project.display()
            ),
            reply: Reply::None,
        });
        request.id = id;
        self.send_inbox_request(request);
    }
    pub(crate) fn coordination_task_action(&mut self, task_id: &str, revision: u64, action: &str) {
        let instruction = if action == "retire" {
            "Safely retire or isolate only this task's assigned worker using coordination_retire_worker. Preserve all unrelated sessions. Do not release mutation ownership from an idle observation alone."
        } else {
            "Review this task's actual candidate using the coordination host tools. Reproduce its observable criteria and collect current evidence. Retire the actual task-owned worker before final acceptance; preserve its candidate claims through final recapture and acceptance. If an existing user session cannot be stopped without explicit direction, record that blocker clearly and preserve unrelated sessions. Accept only when all required criteria pass against the final artifact; otherwise record the next concrete action."
        };
        self.send_to_orchestrator(What::Say { text: format!("[task action] User requested {action} for task {task_id}, observed revision {revision}. Read coordination_context and current state first; reconcile any newer revision. {instruction}"), reply: Reply::None });
    }
    /// Push-to-talk: words (or a live-voice request) for the orchestrator.
    pub(crate) fn voice_message(&mut self, m: VoiceMsg) {
        if m.text.trim().is_empty() {
            if let Some(tag) = &m.tag {
                self.svc.voice_delegation_result(
                    tag,
                    "The words didn't come through; ask the user to say that again.",
                );
            }
            return;
        }
        let text = self.spoken_text(&m.text, m.focused.as_deref());
        let reply = m.tag.map(Reply::Voice).unwrap_or(Reply::None);
        self.send_to_orchestrator(What::Say { text, reply });
    }

    /// What the bridge asks of the orchestrator session: panel messages, scheduled triggers, stop, fresh start.
    pub(crate) fn orchestrator_command(&mut self, c: OrchestratorCommand) {
        match c {
            OrchestratorCommand::Control {
                id,
                text,
                request_id,
                kind,
            } => {
                self.restore_inbox();
                let Some(target) = request_id.and_then(|target| {
                    self.voice
                        .awaiting
                        .iter()
                        .find(|a| {
                            a.dispatched
                                && (a.request.id == target
                                    || matches!(&a.reply,Reply::Panel(id) if *id==target))
                        })
                        .map(|a| a.request.id.clone())
                }) else {
                    self.answer(&Reply::Panel(id),"This control does not identify an active request. Refresh the orchestrator state and send its requestId.",false);
                    return;
                };
                if !matches!(kind.as_str(), "answer" | "steer") {
                    self.answer(
                        &Reply::Panel(id),
                        "Use answer or steer for a control message.",
                        false,
                    );
                    return;
                }
                let request = Request::new(What::Control {
                    text,
                    reply: Reply::Panel(id),
                    target,
                    kind,
                });
                if self.voice.persistence_error || self.voice.queue.len() >= 128 {
                    if let What::Control { reply, .. } = request.what {
                        self.answer(
                            &reply,
                            "The inbox is unavailable or full. This control was not sent.",
                            false,
                        );
                    }
                    return;
                }
                self.voice.queue.insert(0, (request, Instant::now()));
                self.save_inbox();
                self.pump_voice();
            }
            OrchestratorCommand::Message {
                id,
                text,
                voice,
                focused,
                ..
            } => {
                let text = if voice {
                    self.spoken_text(&text, focused.as_deref())
                } else {
                    format!("[panel] {}", text.trim())
                };
                self.send_to_orchestrator(What::Say {
                    text,
                    reply: Reply::Panel(id),
                });
            }
            OrchestratorCommand::Scheduled {
                id,
                name,
                prompt,
                persist,
            } => {
                let kind = if persist { "persistent" } else { "session" };
                self.send_to_orchestrator(What::Say {
                    text: format!(
                        "[schedule \"{name}\" · {kind} trigger {id}] {}",
                        prompt.trim()
                    ),
                    reply: Reply::None,
                });
            }
            OrchestratorCommand::Cancel => {
                let mut keep = Vec::new();
                for (request, at) in std::mem::take(&mut self.voice.queue) {
                    if matches!(request.kind.as_str(), "assignment" | "answer" | "steer") {
                        if let What::Say { reply, .. } | What::Control { reply, .. } = request.what
                        {
                            self.answer(
                                &reply,
                                "The request was cancelled before delivery.",
                                false,
                            );
                        }
                    } else {
                        keep.push((request, at));
                    }
                }
                self.voice.queue = keep;
                if let Some(t) = self
                    .orchestrator_for(&bro_core::orchestrator::workspace())
                    .and_then(|id| self.panes.get_mut(&id))
                    .and_then(|p| p.as_term())
                {
                    t.send(b"\x1b");
                }
                for a in std::mem::take(&mut self.voice.awaiting) {
                    self.answer(&a.reply, "The request was cancelled. Partial work remains in the orchestrator session.", false);
                }
                self.voice.cancelled = true;
                self.voice.pending_dispatch = None;
                self.voice.idle_since = None;
                self.save_inbox();
            }
            OrchestratorCommand::ClearContext { handoff } => {
                self.send_to_orchestrator(What::Clear { handoff })
            }
        }
    }

    /// A watchdog wake-up, done by the orchestrator instead of a headless reviewer: its brief goes in a file and
    /// the orchestrator is told to act on it; its answer is the review.
    pub(crate) fn watchdog_to_orchestrator(
        &mut self,
        pane: PaneId,
        wake: bro_bridge::WatchdogWake,
        name: &str,
        manual: bool,
    ) {
        let Some(sid) = self
            .panes
            .get(&pane)
            .and_then(|p| p.as_term_ref())
            .map(|t| t.meta.sid.clone())
        else {
            return;
        };
        let reason = wake.reason.clone();
        let (generation, brief) = match self.svc.watchdog_brief(&sid, wake) {
            Ok(b) => b,
            Err(e) => {
                self.toast(
                    if manual { Kind::Info } else { Kind::Error },
                    format!("{name} · watchdog: {e}"),
                );
                return;
            }
        };
        let stem: String = sid
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .take(12)
            .collect();
        let file = bro_core::orchestrator::dir()
            .join("watchdog")
            .join(format!("{stem}-{generation}.md"));
        let written = file
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| crate::util::atomic_write(&file, brief.as_bytes()));
        if let Err(e) = written {
            self.svc.watchdog_finish(
                &sid,
                generation,
                format!("couldn't write the brief: {e}"),
                false,
            );
            return;
        }
        let text = format!(
            "[watchdog · {name} (sessionId {sid})] Wake-up: {reason}. Your reviewer brief is in {}: read it and do what it says, recording milestones with watchdog_update. End your turn with a one-line report.",
            file.display()
        );
        self.send_to_orchestrator(What::Say {
            text,
            reply: Reply::Watchdog(sid, generation),
        });
    }

    /// Queue for the orchestrator, starting one if none runs.
    fn send_to_orchestrator(&mut self, what: What) {
        self.send_inbox_request(Request::new(what));
    }

    fn send_inbox_request(&mut self, request: Request) {
        self.restore_inbox();
        if self.voice.persistence_error {
            if let What::Say { reply, .. } | What::Control { reply, .. } = request.what {
                self.answer(&reply,"The orchestrator inbox needs recovery. This request was not admitted or sent; inspect the inbox error, then retry.",false);
            }
            self.toast(
                Kind::Error,
                "Request not sent: the orchestrator inbox needs recovery.",
            );
            return;
        }
        if self.voice.queue.len() >= 128 {
            if let What::Say { reply, .. } | What::Control { reply, .. } = request.what {
                self.answer(
                    &reply,
                    "The orchestrator inbox is full. Retry after some requests finish.",
                    false,
                );
            }
            return;
        }
        self.voice.queue.push((request, Instant::now()));
        self.save_inbox();
        if self
            .orchestrator_for(&bro_core::orchestrator::workspace())
            .is_none()
            && !self.orchestrator_starting(&bro_core::orchestrator::workspace())
        {
            match self.voice_orchestrator_spec() {
                Some(spec) => {
                    self.voice.keep_focus = Some(self.focused());
                    self.launch_spec(spec, Place::Tab);
                }
                None => self.inbox_fail(
                    "No Claude or Codex login can run the orchestrator. Add one in profiles.",
                ),
            }
        }
        self.pump_voice();
    }

    /// The orchestrator to start: the one running when bro last stopped, else the last one you launched, else one
    /// on the Claude or Codex login with the most room left.
    pub(crate) fn voice_orchestrator_spec(&self) -> Option<LaunchSpec> {
        if self.persist
            && let Some(spec) = crate::orchestrator_state::load_all()
                .into_iter()
                .find(|s| bro_core::orchestrator::is_global(&s.cwd))
        {
            return Some(spec);
        }
        if let Some(r) = self.recents.iter().find(|r| r.orchestrator) {
            return Some(LaunchSpec {
                harness: r.harness,
                profile_id: r.profile_id.clone(),
                provider_id: r.provider_id.clone(),
                model: r.model.clone(),
                cwd: bro_core::orchestrator::workspace(),
                resume: None,
                permission: r.permission,
                browser: r.browser,
                extra_args: vec![],
                orchestrator: true,
            });
        }
        let profile = self.roomiest_login();
        let harness = match &profile {
            Some(id) if id.starts_with("codex:") => Harness::Codex,
            Some(_) => Harness::Claude,
            None if self.svc.is_demo()
                || cfg!(test)
                || bro_core::launch::which("claude").is_some() =>
            {
                Harness::Claude
            }
            None if bro_core::launch::which("codex").is_some() => Harness::Codex,
            None => return None,
        };
        Some(LaunchSpec {
            harness,
            profile_id: profile,
            provider_id: None,
            model: None,
            cwd: bro_core::orchestrator::workspace(),
            resume: None,
            permission: Permission::Skip,
            browser: bro_core::browser::BrowserMode::Off,
            extra_args: vec![],
            orchestrator: true,
        })
    }

    /// The signed-in Claude or Codex login (whose agent is installed) with the most left in its tighter window;
    /// an unread meter counts as half full, and Claude wins a tie.
    pub(crate) fn roomiest_login(&self) -> Option<String> {
        let st = self.svc.state();
        let installed =
            |h: &str| self.svc.is_demo() || cfg!(test) || bro_core::launch::which(h).is_some();
        let (claude, codex) = (installed("claude"), installed("codex"));
        let mut best: Option<(f64, bool, String)> = None;
        for p in
            st.profiles.ready()?.iter().filter(|p| {
                p.authenticated && ((p.is_claude() && claude) || (p.is_codex() && codex))
            })
        {
            let left = st
                .usage
                .get(&p.id)
                .and_then(|e| e.usage.as_ref())
                .and_then(|u| {
                    let d = bro_core::usage::headroom_detail(p, u);
                    [d.h5, d.wk].into_iter().flatten().reduce(f64::min)
                });
            let score = (left.unwrap_or(50.0), p.is_claude());
            if best.as_ref().is_none_or(|(l, c, _)| score > (*l, *c)) {
                best = Some((score.0, score.1, p.id.clone()));
            }
        }
        best.map(|(_, _, id)| id)
    }

    /// Deliver what's queued once the orchestrator can take it, and hand finished answers back. Runs after every
    /// batch of events.
    pub(crate) fn pump_voice(&mut self) {
        self.restore_inbox();
        if !self.voice.loaded || self.voice.persistence_error {
            return;
        }
        if self.voice.queue.is_empty() && self.voice.awaiting.is_empty() {
            return;
        }
        let Some(boss) = self.orchestrator_for(&bro_core::orchestrator::workspace()) else {
            // it didn't start (the launch failed), or never will
            let starting = self.orchestrator_starting(&bro_core::orchestrator::workspace());
            if self
                .voice
                .queue
                .iter()
                .any(|(_, at)| at.elapsed() > GIVE_UP)
                || (!starting
                    && self
                        .voice
                        .queue
                        .iter()
                        .all(|(_, at)| at.elapsed() > Duration::from_secs(1)))
            {
                self.voice.keep_focus = None;
                self.inbox_fail("The orchestrator couldn't start.");
            }
            // A hosted session can survive the controller. Keep uncertain in-flight requests so a restart
            // cannot replay their side effects or attribute their answer to the next queued request.
            return;
        };
        // it opened for the inbox: keep the session you were in on screen
        if let Some(prev) = self.voice.keep_focus.take() {
            self.start_now(boss);
            if let Some(prev) = prev.filter(|p| self.panes.contains_key(p)) {
                self.focus_pane(prev);
            }
            self.toast(
                Kind::Info,
                format!(
                    "the orchestrator is up · {} shows it",
                    self.keymap.primary(crate::keymap::Act::Orchestrator)
                ),
            );
        }
        let (activity, age, last, harness, sid) =
            match self.panes.get(&boss).and_then(|p| p.as_term_ref()) {
                Some(t) => (
                    crate::pane::Pane::activity(t),
                    t.meta.started.elapsed(),
                    last_reply(t),
                    t.meta.harness,
                    t.meta.sid.clone(),
                ),
                None => return,
            };
        let now = Instant::now();
        self.voice.idle_since = if activity == Some(Activity::Idle) {
            Some(self.voice.idle_since.unwrap_or(now))
        } else {
            None
        };
        let held = self.voice.hold_until.is_some_and(|t| now < t);
        let can_say =
            !held && !self.voice.cancelled && activity == Some(Activity::Idle) && age >= SETTLE;
        let turn_over = !held
            && age >= SETTLE
            && self
                .voice
                .idle_since
                .is_some_and(|t| t.elapsed() >= TURN_OVER);
        if self.voice.cancelled && turn_over {
            self.voice.cancelled = false;
        }
        // A changed latest reply can resolve only its matching request. Uncorrelated adapters get one
        // ordinary request at a time, and must at least expose its request marker in the latest prompt.
        let mut changed = false;
        let mut still = vec![];
        for mut a in std::mem::take(&mut self.voice.awaiting) {
            let fresh = last.as_ref().filter(|r| {
                a.before
                    .as_ref()
                    .is_none_or(|b| b.text != r.text || b.at_ms != r.at_ms)
            });
            let belongs = fresh.is_some_and(|r| response_for(r, &a.request.id));
            if a.dispatched
                && belongs
                && activity == Some(Activity::Idle)
                && a.since.elapsed() >= SETTLE
                && a.session_id == sid
            {
                let text = strip_response_marker(&fresh.unwrap().text, &a.request.id);
                self.answer(&a.reply, text.trim(), true);
                self.voice.completed.push(a.request.id.clone());
                if self.voice.completed.len() > 512 {
                    self.voice
                        .completed
                        .drain(..self.voice.completed.len() - 512);
                }
                changed = true;
            } else {
                let limit = if matches!(a.reply, Reply::Watchdog(..)) {
                    REVIEW_WAIT
                } else {
                    ANSWER_WAIT
                };
                if !a.notified && a.since.elapsed() > limit {
                    let status = "A request is still awaiting its original reply. Open the orchestrator to inspect it, or cancel before retrying.";
                    if let Reply::Voice(tag) = &a.reply {
                        self.svc.voice_delegation_progress(tag, status);
                    }
                    self.toast(Kind::Info, status);
                    a.notified = true;
                    changed = true;
                }
                still.push(a);
            }
        }
        self.voice.awaiting = still;
        if changed {
            self.save_inbox();
        }
        if let Some((id, generation)) = self.voice.pending_dispatch.clone() {
            let pending = self.voice.awaiting.iter().find(|a| a.request.id == id);
            let obsolete = pending.is_some_and(|a| match &a.request.what {
                What::Control { target, .. } => !self.voice.awaiting.iter().any(|original| {
                    original.request.id == *target
                        && original.session_id == sid
                        && original.dispatched
                }),
                _ => false,
            });
            if obsolete {
                if let Some(index) = self.voice.awaiting.iter().position(|a| a.request.id == id) {
                    let a = self.voice.awaiting.remove(index);
                    self.answer(&a.reply,"The target request finished before this control could be delivered. It was not sent.",false);
                }
                self.voice.pending_dispatch = None;
                self.save_inbox();
                return;
            }
            let allowed = pending.is_some_and(|a| match &a.request.what {
                What::Control { kind, .. } => {
                    !held
                        && age >= SETTLE
                        && if kind == "answer" {
                            activity == Some(Activity::Blocked)
                        } else {
                            matches!(activity, Some(Activity::Working | Activity::Idle))
                        }
                }
                _ => can_say,
            });
            if generation <= self.voice.saved_generation && allowed && !self.voice.cancelled {
                self.dispatch_saved_request(boss, &id, harness);
            }
            return;
        }
        if let Some((request, _)) = self.voice.queue.first().cloned()
            && let What::Control {
                reply,
                target,
                kind,
                ..
            } = &request.what
        {
            let original = self
                .voice
                .awaiting
                .iter()
                .find(|a| a.request.id == *target && a.session_id == sid && a.dispatched);
            if original.is_none() {
                self.voice.queue.remove(0);
                self.answer(
                    reply,
                    "The target request is no longer active. This control was not sent.",
                    false,
                );
                self.save_inbox();
            } else if !held
                && age >= SETTLE
                && if kind == "answer" {
                    activity == Some(Activity::Blocked)
                } else {
                    matches!(activity, Some(Activity::Working | Activity::Idle))
                }
            {
                self.voice.queue.remove(0);
                self.voice.awaiting.push(Awaiting {
                    reply: reply.clone(),
                    before: last.clone(),
                    since: now,
                    request: request.clone(),
                    session_id: sid.clone(),
                    notified: false,
                    dispatched: false,
                });
                let generation = self.save_inbox();
                self.voice.pending_dispatch = Some((request.id.clone(), generation));
                if generation <= self.voice.saved_generation {
                    self.dispatch_saved_request(boss, &request.id, harness);
                }
            }
            return;
        }
        if !self.voice.awaiting.is_empty() {
            return;
        }
        // in order: a fresh start waits for the turn to end, and what follows it waits for the fresh start
        if let Some((request, _)) = self.voice.queue.first().cloned() {
            match request.what.clone() {
                What::Say { text, reply } if can_say => {
                    self.voice.queue.remove(0);
                    self.voice.awaiting.push(Awaiting {
                        reply,
                        before: last.clone(),
                        since: now,
                        request: request.clone(),
                        session_id: sid,
                        notified: false,
                        dispatched: false,
                    });
                    // Persist dispatch intent before submitting. A crash after this point is uncertain,
                    // never an invitation to blindly resend the request after restart.
                    let generation = self.save_inbox();
                    self.voice.pending_dispatch = Some((request.id.clone(), generation));
                    if generation <= self.voice.saved_generation {
                        self.dispatch_saved_request(boss, &request.id, harness);
                    }
                    let _ = text;
                }
                What::Clear { handoff } if turn_over => {
                    self.voice.queue.remove(0);
                    self.voice.awaiting.push(Awaiting {
                        reply: Reply::None,
                        before: last,
                        since: now,
                        request: request.clone(),
                        session_id: sid,
                        notified: false,
                        dispatched: false,
                    });
                    let generation = self.save_inbox();
                    self.voice.pending_dispatch = Some((request.id.clone(), generation));
                    if generation <= self.voice.saved_generation {
                        self.dispatch_saved_request(boss, &request.id, harness);
                    }
                    let _ = handoff;
                }
                _ => {}
            }
        }
    }

    fn restore_inbox(&mut self) {
        if self.voice.loaded {
            return;
        }
        if !self.persist || self.guest {
            self.voice.loaded = true;
            return;
        }
        if self.voice.store.is_none() {
            self.voice.store = Some(super::inbox_store::Store::start(
                bro_core::orchestrator::dir().join("inbox.json"),
                self.tx.clone(),
            ));
        }
    }

    pub(crate) fn inbox_update(&mut self, update: super::inbox_store::Update) {
        use super::inbox_store::Update;
        match update {
            Update::Loaded { owner, saved }
                if self.voice.store.as_ref().is_some_and(|s| s.owner == owner) =>
            {
                self.voice.loaded = true;
                self.voice.persistence_error = false;
                self.voice.completed = saved.completed;
                let local = std::mem::take(&mut self.voice.queue);
                self.voice.queue = saved
                    .queue
                    .into_iter()
                    .map(|r| (r, Instant::now()))
                    .collect();
                for (r, at) in local {
                    if !self.voice.queue.iter().any(|(old, _)| old.id == r.id)
                        && !self.voice.completed.contains(&r.id)
                    {
                        self.voice.queue.push((r, at));
                    }
                }
                self.voice.awaiting = saved
                    .awaiting
                    .into_iter()
                    .take(128)
                    .map(|a| Awaiting {
                        reply: a.reply,
                        before: a.before,
                        since: Instant::now(),
                        request: a.request,
                        session_id: a.session_id,
                        notified: a.notified,
                        dispatched: true,
                    })
                    .collect();
                if !self.voice.awaiting.is_empty() {
                    self.toast(Kind::Info,"Orchestrator inbox recovered. Sent requests await their original reply; cancel to retry an uncertain request.");
                }
                self.save_inbox();
            }
            Update::Saved { owner, generation }
                if self.voice.store.as_ref().is_some_and(|s| s.owner == owner) =>
            {
                self.voice.saved_generation = self.voice.saved_generation.max(generation);
                self.voice.persistence_error = false;
            }
            Update::Error { owner, message }
                if self.voice.store.as_ref().is_some_and(|s| s.owner == owner) =>
            {
                self.voice.persistence_error = true;
                self.toast(Kind::Error, message);
            }
            _ => {}
        }
    }

    fn save_inbox(&mut self) -> u64 {
        if !self.persist || self.guest {
            return 0;
        }
        self.voice.generation += 1;
        if !self.voice.loaded || self.voice.persistence_error {
            return self.voice.generation;
        }
        let saved = SavedRoute {
            schema: 1,
            queue: self.voice.queue.iter().map(|(r, _)| r.clone()).collect(),
            awaiting: self
                .voice
                .awaiting
                .iter()
                .map(|a| SavedAwaiting {
                    request: a.request.clone(),
                    reply: a.reply.clone(),
                    before: a.before.clone(),
                    session_id: a.session_id.clone(),
                    notified: a.notified,
                })
                .collect(),
            completed: self.voice.completed.clone(),
        };
        if let Some(store) = &self.voice.store {
            store.save(self.voice.generation, saved);
        }
        self.voice.generation
    }

    fn dispatch_saved_request(&mut self, boss: PaneId, id: &str, harness: Option<Harness>) {
        let Some(index) = self.voice.awaiting.iter().position(|a| a.request.id == id) else {
            self.voice.pending_dispatch = None;
            return;
        };
        let request = self.voice.awaiting[index].request.clone();
        let session_id = self.voice.awaiting[index].session_id.clone();
        if !self
            .panes
            .get(&boss)
            .and_then(|p| p.as_term_ref())
            .is_some_and(|t| t.meta.sid == session_id)
        {
            return;
        }
        match request.what.clone() {
            What::Control {
                text,
                reply,
                target,
                kind,
            } => {
                if let Some(t) = self.panes.get_mut(&boss).and_then(|p| p.as_term()) {
                    let input = if kind == "steer" {
                        format!("[bro-steer {target}] {text}")
                    } else {
                        text
                    };
                    t.submit(&input);
                }
                self.voice.awaiting.remove(index);
                self.answer(&reply,"Control delivered to the active request. Its original result remains separate.",true);
                self.voice.completed.push(request.id.clone());
                if self.voice.completed.len() > 512 {
                    self.voice
                        .completed
                        .drain(..self.voice.completed.len() - 512);
                }
                self.save_inbox();
            }
            What::Say { text, .. } => {
                if let Some(t) = self.panes.get_mut(&boss).and_then(|p| p.as_term()) {
                    t.submit(&request.prompt(&text));
                }
                self.voice.awaiting[index].dispatched = true;
                self.voice.awaiting[index].since = Instant::now();
            }
            What::Clear { handoff } => {
                if let Some(t) = self.panes.get_mut(&boss).and_then(|p| p.as_term()) {
                    t.type_command(if harness == Some(Harness::Claude) {
                        "/clear"
                    } else {
                        "/new"
                    });
                }
                self.voice.awaiting.remove(index);
                if let Some(h) = handoff.filter(|h| !h.trim().is_empty()) {
                    self.voice.queue.insert(
                        0,
                        (
                            Request::new(What::Say {
                                text: format!("[fresh start · your handoff notes] {}", h.trim()),
                                reply: Reply::None,
                            }),
                            Instant::now(),
                        ),
                    );
                }
                self.voice.hold_until = Some(Instant::now() + AFTER_CLEAR);
                self.voice.idle_since = None;
                self.toast(Kind::Info, "the orchestrator started a fresh conversation");
                self.save_inbox();
            }
        }
        self.voice.pending_dispatch = None;
    }

    /// Hand an answer to whoever asked.
    fn answer(&mut self, reply: &Reply, text: &str, ok: bool) {
        match reply {
            Reply::None => {}
            Reply::Voice(tag) => self.svc.voice_delegation_result(tag, text),
            Reply::Panel(id) => self.svc.orchestrator_answer(id, text, ok),
            Reply::Watchdog(sid, generation) => {
                self.svc
                    .watchdog_finish(sid, *generation, text.to_string(), ok)
            }
        }
    }

    /// The orchestrator closed: its session-only schedule goes, and nothing waits on it any more.
    pub(crate) fn orchestrator_closed(&mut self) {
        self.svc.schedule_clear_session();
        if self.quitting {
            self.save_inbox();
            return;
        }
        for a in std::mem::take(&mut self.voice.awaiting) {
            self.answer(
                &a.reply,
                "The orchestrator was closed before it answered.",
                false,
            );
        }
        self.save_inbox();
    }

    /// Spoken words as the orchestrator reads them: marked as spoken, with the session you were looking at.
    fn spoken_text(&self, text: &str, focused: Option<&str>) -> String {
        let words = text.split_whitespace().collect::<Vec<_>>().join(" ");
        let looking = focused.and_then(|sid| {
            self.term_id(sid)
                .and_then(|id| self.panes.get(&id))
                .and_then(|p| p.as_term_ref())
                .filter(|t| !t.meta.orchestrator)
                .map(|t| (sid, t.display_name()))
        });
        match looking {
            Some((sid, name)) => {
                format!("[voice · looking at \"{name}\" (sessionId {sid})] {words}")
            }
            None => format!("[voice] {words}"),
        }
    }

    /// Drop what's queued, telling each asker why.
    fn inbox_fail(&mut self, why: &str) {
        for (request, _) in std::mem::take(&mut self.voice.queue) {
            match request.what {
                What::Say {
                    reply: Reply::None, ..
                }
                | What::Clear { .. } => self.toast(Kind::Error, format!("orchestrator: {why}")),
                What::Say { reply, .. } | What::Control { reply, .. } => {
                    self.answer(&reply, why, false)
                }
            }
        }
        self.save_inbox();
    }
}

/// The orchestrator's latest full reply, from its transcript.
fn last_reply(t: &crate::panes::term::Term) -> Option<ReplyStamp> {
    t.insights().and_then(|s| s.facts).and_then(|f| {
        f.reply.as_ref().map(|r| ReplyStamp {
            text: r.text.clone(),
            at_ms: r.at_ms,
            prompt: f.prompt.as_ref().map(|p| p.text.clone()),
        })
    })
}

fn response_for(reply: &ReplyStamp, id: &str) -> bool {
    if reply.text.contains("[bro-response ") {
        reply.text.contains(&format!("[bro-response {id}]"))
    } else {
        reply
            .prompt
            .as_ref()
            .is_some_and(|p| p.contains(&format!("[bro-request {id} ")))
    }
}

fn strip_response_marker<'a>(text: &'a str, id: &str) -> &'a str {
    text.trim_end()
        .strip_suffix(&format!("[bro-response {id}]"))
        .unwrap_or(text)
}
