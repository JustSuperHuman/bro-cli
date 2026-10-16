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
use std::time::{Duration, Instant};

/// A fresh orchestrator gets this long to reach its prompt before a message is typed in anyway.
const BOOT_WAIT: Duration = Duration::from_secs(30);
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
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Reply {
    None,
    /// the live voice's delegation id
    Voice(String),
    /// a panel message's answer item in the bridge transcript
    Panel(String),
    /// a watchdog wake-up: session id, generation
    Watchdog(String, u64),
}

#[derive(Clone, Debug)]
pub(crate) enum What {
    Say { text: String, reply: Reply },
    /// `/clear` (Codex, Pi, omp: `/new`), then the handoff
    Clear { handoff: Option<String> },
}

/// Something delivered, waiting for the orchestrator's answer.
pub(crate) struct Awaiting {
    pub(crate) reply: Reply,
    /// the orchestrator's last reply when it was typed in (the answer is the next one)
    before: Option<String>,
    pub(crate) since: Instant,
}

#[derive(Default)]
pub(crate) struct VoiceRoute {
    pub(crate) queue: Vec<(What, Instant)>,
    pub(crate) awaiting: Vec<Awaiting>,
    /// The orchestrator is starting for the inbox: the session to keep in view when it opens.
    keep_focus: Option<Option<PaneId>>,
    /// since when the orchestrator has been idle
    pub(crate) idle_since: Option<Instant>,
    /// nothing more goes in before this (a `/clear` settling)
    pub(crate) hold_until: Option<Instant>,
}

impl App {
    /// Push-to-talk: words (or a live-voice request) for the orchestrator.
    pub(crate) fn voice_message(&mut self, m: VoiceMsg) {
        if m.text.trim().is_empty() {
            if let Some(tag) = &m.tag {
                self.svc.voice_delegation_result(tag, "The words didn't come through; ask the user to say that again.");
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
            OrchestratorCommand::Message { id, text, voice, focused, .. } => {
                let text = if voice { self.spoken_text(&text, focused.as_deref()) } else { format!("[panel] {}", text.trim()) };
                self.send_to_orchestrator(What::Say { text, reply: Reply::Panel(id) });
            }
            OrchestratorCommand::Scheduled { id, name, prompt, persist } => {
                let kind = if persist { "persistent" } else { "session" };
                self.send_to_orchestrator(What::Say { text: format!("[schedule \"{name}\" · {kind} trigger {id}] {}", prompt.trim()), reply: Reply::None });
            }
            OrchestratorCommand::Cancel => {
                if let Some(t) = self.orchestrator_pane().and_then(|id| self.panes.get_mut(&id)).and_then(|p| p.as_term()) {
                    t.send(b"\x1b");
                }
            }
            OrchestratorCommand::ClearContext { handoff } => self.send_to_orchestrator(What::Clear { handoff }),
        }
    }

    /// A watchdog wake-up, done by the orchestrator instead of a headless reviewer: its brief goes in a file and
    /// the orchestrator is told to act on it; its answer is the review.
    pub(crate) fn watchdog_to_orchestrator(&mut self, pane: PaneId, wake: bro_bridge::WatchdogWake, name: &str, manual: bool) {
        let Some(sid) = self.panes.get(&pane).and_then(|p| p.as_term_ref()).map(|t| t.meta.sid.clone()) else { return };
        let reason = wake.reason.clone();
        let (generation, brief) = match self.svc.watchdog_brief(&sid, wake) {
            Ok(b) => b,
            Err(e) => {
                self.toast(if manual { Kind::Info } else { Kind::Error }, format!("{name} · watchdog: {e}"));
                return;
            }
        };
        let stem: String = sid.chars().filter(|c| c.is_ascii_alphanumeric()).take(12).collect();
        let file = bro_core::orchestrator::dir().join("watchdog").join(format!("{stem}-{generation}.md"));
        let written = file.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| crate::util::atomic_write(&file, brief.as_bytes()));
        if let Err(e) = written {
            self.svc.watchdog_finish(&sid, generation, format!("couldn't write the brief: {e}"), false);
            return;
        }
        let text = format!(
            "[watchdog · {name} (sessionId {sid})] Wake-up: {reason}. Your reviewer brief is in {}: read it and do what it says, recording milestones with watchdog_update. End your turn with a one-line report.",
            file.display()
        );
        self.send_to_orchestrator(What::Say { text, reply: Reply::Watchdog(sid, generation) });
    }

    /// Queue for the orchestrator, starting one if none runs.
    fn send_to_orchestrator(&mut self, what: What) {
        self.voice.queue.push((what, Instant::now()));
        if self.orchestrator_pane().is_none() && self.orchestrator_pending.is_none() {
            match self.voice_orchestrator_spec() {
                Some(spec) => {
                    self.voice.keep_focus = Some(self.focused());
                    self.launch_spec(spec, Place::Tab);
                }
                None => self.inbox_fail("No Claude or Codex login can run the orchestrator. Add one in profiles."),
            }
        }
        self.pump_voice();
    }

    /// The orchestrator to start: the one running when bro last stopped, else the last one you launched, else one
    /// on the Claude or Codex login with the most room left.
    pub(crate) fn voice_orchestrator_spec(&self) -> Option<LaunchSpec> {
        if self.persist
            && let Some(spec) = crate::orchestrator_state::load()
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
            None if self.svc.is_demo() || cfg!(test) || bro_core::launch::which("claude").is_some() => Harness::Claude,
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
        let installed = |h: &str| self.svc.is_demo() || cfg!(test) || bro_core::launch::which(h).is_some();
        let (claude, codex) = (installed("claude"), installed("codex"));
        let mut best: Option<(f64, bool, String)> = None;
        for p in st.profiles.ready()?.iter().filter(|p| p.authenticated && ((p.is_claude() && claude) || (p.is_codex() && codex))) {
            let left = st.usage.get(&p.id).and_then(|e| e.usage.as_ref()).and_then(|u| {
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
        if self.voice.queue.is_empty() && self.voice.awaiting.is_empty() {
            return;
        }
        let Some(boss) = self.orchestrator_pane() else {
            // it didn't start (the launch failed), or never will
            let starting = self.orchestrator_pending.is_some();
            if self.voice.queue.iter().any(|(_, at)| at.elapsed() > GIVE_UP) || (!starting && self.voice.queue.iter().all(|(_, at)| at.elapsed() > Duration::from_secs(1))) {
                self.voice.keep_focus = None;
                self.inbox_fail("The orchestrator couldn't start.");
            }
            for a in std::mem::take(&mut self.voice.awaiting) {
                self.answer(&a.reply, "The orchestrator was closed before it answered.", false);
            }
            return;
        };
        // it opened for the inbox: keep the session you were in on screen
        if let Some(prev) = self.voice.keep_focus.take() {
            self.start_now(boss);
            if let Some(prev) = prev.filter(|p| self.panes.contains_key(p)) {
                self.focus_pane(prev);
            }
            self.toast(Kind::Info, format!("the orchestrator is up · {} shows it", self.keymap.primary(crate::keymap::Act::Orchestrator)));
        }
        let (activity, age, last, harness) = match self.panes.get(&boss).and_then(|p| p.as_term_ref()) {
            Some(t) => (crate::pane::Pane::activity(t), t.meta.started.elapsed(), last_reply(t), t.meta.harness),
            None => return,
        };
        let now = Instant::now();
        self.voice.idle_since = if activity == Some(Activity::Idle) { Some(self.voice.idle_since.unwrap_or(now)) } else { None };
        let held = self.voice.hold_until.is_some_and(|t| now < t);
        let can_say = !held
            && match activity {
                Some(Activity::Idle | Activity::Working) => age >= SETTLE,
                Some(Activity::Blocked) => false,
                None => age >= BOOT_WAIT,
            };
        let turn_over = !held && age >= SETTLE && self.voice.idle_since.is_some_and(|t| t.elapsed() >= TURN_OVER);
        // in order: a fresh start waits for the turn to end, and what follows it waits for the fresh start
        while let Some((what, _)) = self.voice.queue.first().cloned() {
            match what {
                What::Say { text, reply } if can_say => {
                    if let Some(t) = self.panes.get_mut(&boss).and_then(|p| p.as_term()) {
                        t.submit(&text);
                    }
                    if reply != Reply::None {
                        self.voice.awaiting.push(Awaiting { reply, before: last.clone(), since: now });
                    }
                    self.voice.queue.remove(0);
                }
                What::Clear { handoff } if turn_over => {
                    let command = if harness == Some(Harness::Claude) { "/clear" } else { "/new" };
                    if let Some(t) = self.panes.get_mut(&boss).and_then(|p| p.as_term()) {
                        t.type_command(command);
                    }
                    self.voice.queue.remove(0);
                    if let Some(h) = handoff.filter(|h| !h.trim().is_empty()) {
                        self.voice.queue.insert(0, (What::Say { text: format!("[fresh start · your handoff notes] {}", h.trim()), reply: Reply::None }, now));
                    }
                    self.voice.hold_until = Some(now + AFTER_CLEAR);
                    self.voice.idle_since = None;
                    self.toast(Kind::Info, "the orchestrator started a fresh conversation");
                    break;
                }
                _ => break,
            }
        }
        // an answer is the orchestrator's next reply once it's idle again
        let idle = activity == Some(Activity::Idle);
        let mut still = vec![];
        for a in std::mem::take(&mut self.voice.awaiting) {
            let fresh = last.as_ref().filter(|r| Some(*r) != a.before.as_ref());
            let limit = if matches!(a.reply, Reply::Watchdog(..)) { REVIEW_WAIT } else { ANSWER_WAIT };
            match fresh {
                Some(r) if idle && a.since.elapsed() >= SETTLE => self.answer(&a.reply, r, true),
                _ if a.since.elapsed() > limit => self.answer(&a.reply, "Still working on it; the orchestrator's session has the details.", false),
                _ => still.push(a),
            }
        }
        self.voice.awaiting = still;
    }

    /// Hand an answer to whoever asked.
    fn answer(&mut self, reply: &Reply, text: &str, ok: bool) {
        match reply {
            Reply::None => {}
            Reply::Voice(tag) => self.svc.voice_delegation_result(tag, text),
            Reply::Panel(id) => self.svc.orchestrator_answer(id, text, ok),
            Reply::Watchdog(sid, generation) => self.svc.watchdog_finish(sid, *generation, text.to_string(), ok),
        }
    }

    /// The orchestrator closed: its session-only schedule goes, and nothing waits on it any more.
    pub(crate) fn orchestrator_closed(&mut self) {
        self.svc.schedule_clear_session();
        for a in std::mem::take(&mut self.voice.awaiting) {
            self.answer(&a.reply, "The orchestrator was closed before it answered.", false);
        }
    }

    /// Spoken words as the orchestrator reads them: marked as spoken, with the session you were looking at.
    fn spoken_text(&self, text: &str, focused: Option<&str>) -> String {
        let words = text.split_whitespace().collect::<Vec<_>>().join(" ");
        let looking = focused.and_then(|sid| {
            self.term_id(sid).and_then(|id| self.panes.get(&id)).and_then(|p| p.as_term_ref()).filter(|t| !t.meta.orchestrator).map(|t| (sid, t.display_name()))
        });
        match looking {
            Some((sid, name)) => format!("[voice · looking at \"{name}\" (sessionId {sid})] {words}"),
            None => format!("[voice] {words}"),
        }
    }

    /// Drop what's queued, telling each asker why.
    fn inbox_fail(&mut self, why: &str) {
        for (what, _) in std::mem::take(&mut self.voice.queue) {
            match what {
                What::Say { reply: Reply::None, .. } | What::Clear { .. } => self.toast(Kind::Error, format!("orchestrator: {why}")),
                What::Say { reply, .. } => self.answer(&reply, why, false),
            }
        }
    }
}

/// The orchestrator's latest full reply, from its transcript.
fn last_reply(t: &crate::panes::term::Term) -> Option<String> {
    t.insights().and_then(|s| s.facts).and_then(|f| f.reply.as_ref().map(|r| r.text.clone()))
}
