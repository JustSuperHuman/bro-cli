//! Turning output signals and agent-state transitions into notifications
//! (the Node host's `publishTerminalNotification` / bell settle window / prompt
//! monitor, which the reference Rust host lacked).
//!
//! * `OSC 9` / `OSC 777;notify` publish immediately (throttled per session).
//! * A bare BEL waits [`BELL_SETTLE`]; the session is then observed once more
//!   and, if the agent turned out to be asking a question, the "needs input"
//!   alert replaces the generic "Task finished".
//! * Agent `working -> idle` is "Task finished"; `-> awaiting` is
//!   "<agent> needs input" and bypasses the throttle, as in the Node host.

use crate::model::TerminalNotification;
use crate::notifications::{BELL_SETTLE, BellEvent};
use crate::state::AppState;
use std::time::Instant;

impl AppState {
    /// Handles the signals found in one output chunk.
    pub(crate) fn handle_bells(&self, session_id: &str, events: Vec<BellEvent>) {
        let mut bell = false;
        for event in events {
            match event {
                BellEvent::Osc { title, body } => {
                    self.publish_terminal_notification(session_id, "osc", title, body)
                }
                BellEvent::Bell => bell = true,
            }
        }
        if !bell {
            return;
        }
        let Some(runtime) = self.runtime.get() else {
            self.publish_terminal_notification(session_id, "bell", None, None);
            return;
        };
        let generation = self.notifications.lock().schedule_bell(session_id);
        let state = self.clone();
        let id = session_id.to_owned();
        runtime.spawn(async move {
            tokio::time::sleep(BELL_SETTLE).await;
            // A question rendered right after the bell turns this into the
            // actionable "needs input" alert, which cancels the bell.
            state.observe_now(&id);
            if state.notifications.lock().take_bell(&id, generation) {
                state.publish_terminal_notification(&id, "bell", None, None);
            }
        });
    }

    /// A throttled, session-attributed notification (`sound: "done"`).
    pub(crate) fn publish_terminal_notification(
        &self,
        session_id: &str,
        origin: &str,
        title: Option<String>,
        body: Option<String>,
    ) {
        if !self.notifications.lock().allow(session_id, Instant::now()) {
            return;
        }
        let session_title = self.summary(session_id).map(|session| session.title);
        let body = body.or_else(|| (origin == "bell").then(|| "Task finished".to_owned()));
        self.notify(
            TerminalNotification {
                id: String::new(),
                at: String::new(),
                origin: origin.into(),
                session_id: Some(session_id.to_owned()),
                title: title
                    .or_else(|| session_title.clone())
                    .or_else(|| Some("Terminal".into())),
                session_title,
                body,
                sound: Some("done".into()),
            },
            true,
        );
    }

    /// "<agent> needs input" (`sound: "attention"`), never throttled away.
    pub(crate) fn publish_needs_input(&self, session_id: &str, agent_label: &str) {
        {
            let mut center = self.notifications.lock();
            center.cancel_bell(session_id);
            center.mark(session_id, Instant::now());
        }
        let session_title = self.summary(session_id).map(|session| session.title);
        self.notify(
            TerminalNotification {
                id: String::new(),
                at: String::new(),
                origin: "api".into(),
                session_id: Some(session_id.to_owned()),
                title: Some(format!("{agent_label} needs input")),
                body: Some(format!(
                    "Open {} to answer.",
                    session_title.as_deref().unwrap_or("this terminal")
                )),
                session_title,
                sound: Some("attention".into()),
            },
            true,
        );
    }
}
