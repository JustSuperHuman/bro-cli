//! What changes in the workspace while a call is live, as short `[bro]` updates.
//!
//! One watcher runs while any call is up. It diffs the session registry (agent activity is detected
//! by the bridge itself, so this works whatever bro's insight settings are) and turns transitions
//! into [`Notice`]s: an agent finished a turn, an agent needs the user, a session opened or closed.
//! Hugh's answers to a call's `ask_orchestrator` come back through the orchestrator's update sink,
//! addressed to that call. Each call decides what to say out loud ([`Notice::speak_for`]).

use super::{Announce, Call, Input, brain};
use crate::OrchestratorUpdate;
use crate::state::AppState;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

const POLL: Duration = Duration::from_millis(700);
/// A finished turn waits this long for its recap, and must still be idle afterwards (agents flicker
/// between tool calls).
const FINISH_SETTLE: Duration = Duration::from_millis(2500);
/// A question waits this long for its options to render.
const QUESTION_SETTLE: Duration = Duration::from_millis(900);
/// The watcher stops after this long without calls.
const IDLE_STOP: Duration = Duration::from_secs(60);
const RECENT: usize = 60;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Finished,
    Input,
    Opened,
    Closed,
    Orchestrator,
    /// A slow tool this call started came back.
    Tool,
    /// What the user has open changed.
    Focus,
}

#[derive(Clone, Debug)]
pub(crate) struct Notice {
    pub kind: Kind,
    pub text: String,
    pub session_id: Option<String>,
    /// Only for this call (its own orchestrator answer, its own slow tool).
    pub call: Option<String>,
}

impl Notice {
    pub fn new(kind: Kind, text: impl Into<String>, session_id: Option<String>) -> Self {
        Self { kind, text: text.into(), session_id, call: None }
    }

    /// Whether this call says it out loud, or only takes it in as context.
    pub fn speak_for(&self, call: &Call) -> bool {
        let announce = *call.announce.lock();
        let watched = self.session_id.as_deref().is_some_and(|id| call.watches(id));
        match self.kind {
            Kind::Orchestrator | Kind::Tool => true,
            Kind::Input => announce != Announce::Off || watched,
            Kind::Finished => match announce {
                Announce::All => true,
                Announce::Asked | Announce::Off => watched,
            },
            Kind::Opened | Kind::Closed | Kind::Focus => false,
        }
    }
}

/// Newest last: "14:02 Toast finished: …".
pub(crate) fn recent(app: &AppState, limit: usize) -> Vec<String> {
    let events = app.voice.events.lock();
    events.iter().skip(events.len().saturating_sub(limit)).cloned().collect()
}

fn remember(app: &AppState, notice: &Notice) {
    if notice.kind == Kind::Focus {
        return;
    }
    let mut events = app.voice.events.lock();
    events.push_back(format!("{} {}", chrono::Local::now().format("%H:%M"), notice.text));
    while events.len() > RECENT {
        events.pop_front();
    }
}

/// Hands a notice to the calls it is for.
pub(crate) fn deliver(app: &AppState, notice: Notice) {
    remember(app, &notice);
    for call in app.voice.calls() {
        if notice.call.as_deref().is_none_or(|id| id == call.id) {
            call.send(Input::Notice(notice.clone()));
        }
    }
}

#[derive(Clone)]
struct Mark {
    title: String,
    activity: Option<String>,
    exited: bool,
    agent: bool,
}

/// Starts the watcher (and, once, the orchestrator sink) if it isn't running.
pub(crate) fn ensure_running(app: &AppState) {
    if !app.voice.sink_registered.swap(true, Ordering::AcqRel) {
        let weak = app.clone();
        app.orchestrator.on_update(Arc::new(move |update| orchestrator_update(&weak, update)));
    }
    if app.voice.feed_running.swap(true, Ordering::AcqRel) {
        return;
    }
    let app = app.clone();
    tokio::spawn(async move {
        watch(&app).await;
        app.voice.feed_running.store(false, Ordering::Release);
    });
}

fn orchestrator_update(app: &AppState, update: OrchestratorUpdate) {
    let (tag, notice) = match update {
        OrchestratorUpdate::Reply { text, tag } => (tag, Some(format!("Hugh answered: {}", brain::clip(&text, 1200)))),
        OrchestratorUpdate::Failed { message, tag } => (tag, Some(format!("Hugh couldn't finish: {}", brain::clip(&message, 400)))),
        OrchestratorUpdate::Step { summary, tag } => {
            if let Some(call) = tag.as_deref().and_then(|tag| call_for_tag(app, tag)) {
                super::publish(app, &call.id, serde_json::json!({"event":"progress","text":summary}));
            }
            (None, None)
        }
        _ => (None, None),
    };
    let (Some(tag), Some(text)) = (tag, notice) else { return };
    if let Some(call) = call_for_tag(app, &tag) {
        let mut notice = Notice::new(Kind::Orchestrator, text, None);
        notice.call = Some(call.id.clone());
        deliver(app, notice);
    }
}

fn call_for_tag(app: &AppState, tag: &str) -> Option<Arc<Call>> {
    let rest = tag.strip_prefix("voice:")?;
    let (id, _) = rest.rsplit_once(':')?;
    app.voice.call(id)
}

fn marks(app: &AppState) -> HashMap<String, Mark> {
    app.summaries()
        .into_iter()
        .map(|summary| {
            (
                summary.id.clone(),
                Mark {
                    title: summary.title,
                    activity: summary.agent_activity,
                    exited: summary.status == "exited",
                    agent: summary.agent.is_some(),
                },
            )
        })
        .collect()
}

async fn watch(app: &AppState) {
    let mut known = marks(app);
    // session -> (what to check, when)
    let mut due: HashMap<String, (Kind, Instant)> = HashMap::new();
    let mut quiet_since: Option<Instant> = None;
    let mut shutdown = app.shutdown.subscribe();
    loop {
        tokio::select! {
            _ = tokio::time::sleep(POLL) => {}
            _ = shutdown.changed() => return,
        }
        if app.voice.calls().is_empty() {
            let since = *quiet_since.get_or_insert_with(Instant::now);
            if since.elapsed() >= IDLE_STOP {
                return;
            }
            known = marks(app);
            due.clear();
            continue;
        }
        quiet_since = None;
        let now = Instant::now();
        let current = marks(app);
        for (id, mark) in &current {
            match known.get(id) {
                None => {
                    if mark.agent || !mark.title.is_empty() {
                        deliver(app, Notice::new(Kind::Opened, format!("{} opened.", mark.title), Some(id.clone())));
                    }
                }
                Some(before) => {
                    if mark.exited && !before.exited {
                        deliver(app, Notice::new(Kind::Closed, format!("{} exited.", mark.title), Some(id.clone())));
                        due.remove(id);
                        continue;
                    }
                    let was = before.activity.as_deref();
                    let is = mark.activity.as_deref();
                    if was != is {
                        match is {
                            Some("awaiting") => {
                                due.insert(id.clone(), (Kind::Input, now + QUESTION_SETTLE));
                            }
                            Some("idle") if was == Some("working") => {
                                due.insert(id.clone(), (Kind::Finished, now + FINISH_SETTLE));
                            }
                            Some("working") => {
                                // still going: a pending "finished" was a flicker
                                if due.get(id).is_some_and(|(kind, _)| *kind == Kind::Finished) {
                                    due.remove(id);
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        for (id, mark) in &known {
            if !current.contains_key(id) {
                deliver(app, Notice::new(Kind::Closed, format!("{} closed.", mark.title), Some(id.clone())));
                due.remove(id);
            }
        }
        let ready: Vec<(String, Kind)> = due
            .iter()
            .filter(|(_, (_, at))| *at <= now)
            .map(|(id, (kind, _))| (id.clone(), *kind))
            .collect();
        for (id, kind) in ready {
            due.remove(&id);
            if let Some(notice) = settled(app, &id, kind) {
                deliver(app, notice);
            }
        }
        known = current;
    }
}

/// Prompt and footer lines of agent TUIs, which say nothing about the work.
fn is_chrome(line: &str) -> bool {
    let line = line.trim();
    line.chars().filter(|c| c.is_alphanumeric()).count() < 3
        || line.starts_with('❯')
        || line.starts_with('>')
        || line.contains("for shortcuts")
        || line.contains("esc to interrupt")
        || line.contains("shift+tab")
        || line.starts_with('─')
        || line.starts_with('╭')
        || line.starts_with('╰')
}

/// The notice once a transition held: what the agent asks, or what its turn amounted to.
fn settled(app: &AppState, id: &str, kind: Kind) -> Option<Notice> {
    let view = app.session_view(id)?;
    let summary = &view.summary;
    match kind {
        Kind::Input => {
            if summary.agent_activity.as_deref() != Some("awaiting") {
                return None;
            }
            let question = brain::prompt_text(view.prompt.as_ref())
                .or_else(|| app.blocked_reason(id))
                .unwrap_or_else(|| "a question on its screen".into());
            Some(Notice::new(Kind::Input, format!("{} needs the user: {}", summary.title, brain::clip(&question, 500)), Some(id.into())))
        }
        Kind::Finished => {
            if summary.agent_activity.as_deref() != Some("idle") {
                return None;
            }
            let what = app
                .finished_summary(id)
                .or_else(|| {
                    let tail: Vec<String> = crate::orchestrator::context::screen_tail(&view.screen, 12, 200)
                        .into_iter()
                        .filter(|line| !is_chrome(line))
                        .collect();
                    let tail = &tail[tail.len().saturating_sub(5)..];
                    (!tail.is_empty()).then(|| format!("its screen ends with: {}", tail.join(" / ")))
                })
                .unwrap_or_else(|| "no summary yet (read_chat has its answer)".into());
            Some(Notice::new(Kind::Finished, format!("{} finished its turn: {}", summary.title, brain::clip(&what, 700)), Some(id.into())))
        }
        _ => None,
    }
}
