//! One orchestrator turn: the user's message goes to the Claude Code process
//! (started on demand, resumed after restarts) with a fresh workspace
//! snapshot, and its stream fills the shared transcript until the `result`.
//! Messages sent while a turn runs wait in a short queue.

use super::{ActiveTurn, MAX_QUEUE, MessageContext, Orchestrator, claude, context};
use crate::model::iso_now;
use crate::state::AppState;
use bro_core::Harness;
use serde_json::Value;

/// A message typed in an orchestrator panel (no focus, not dictated).
pub async fn send_message(app: &AppState, text: String) -> Result<Value, (u16, String)> {
    send_message_with(app, text, MessageContext::default()).await
}

/// Appends the user's message and starts a turn, or queues it behind the
/// running one. Returns the status to show, or an HTTP status + message.
pub async fn send_message_with(
    app: &AppState,
    text: String,
    context: MessageContext,
) -> Result<Value, (u16, String)> {
    let orchestrator = app.orchestrator.clone();
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err((400, "Say something first.".into()));
    }
    {
        let mut state = orchestrator.state.lock();
        if state.turn.is_some() || state.discard_until_result {
            if state.queue.len() >= MAX_QUEUE {
                return Err((
                    409,
                    "The orchestrator is busy and has messages waiting; stop it or wait.".into(),
                ));
            }
            state.queue.push((text, context));
            state.seq += 1;
            drop(state);
            orchestrator.publish_status();
            let mut status = orchestrator.status(None, false);
            status["queued"] = Value::Bool(true);
            return Ok(status);
        }
    }
    orchestrator
        .begin_turn(app, text, context)
        .map_err(|message| (503, message))?;
    Ok(orchestrator.status(None, false))
}

impl Orchestrator {
    /// Makes sure a Claude Code process with the current settings is running.
    fn ensure_process(&self, app: &AppState) -> Result<(), String> {
        let (config, launch, resume, stale) = {
            let mut state = self.state.lock();
            let wanted = claude::fingerprint(&state.config, &state.launch);
            if state
                .process
                .as_ref()
                .is_some_and(|process| process.fingerprint == wanted)
            {
                return Ok(());
            }
            (
                state.config.clone(),
                state.launch.clone(),
                state.claude_session.clone(),
                state.process.take(),
            )
        };
        if let Some(process) = stale {
            process.kill();
        }
        // Resume only a conversation that is still on disk; a missing one
        // would make Claude Code exit at once.
        let resume = resume.filter(|id| {
            bro_core::sessions::is_uuid(id)
                && bro_core::sessions::find_by_id(Harness::Claude, id).is_some()
        });
        let generation = {
            let mut state = self.state.lock();
            state.generation += 1;
            state.generation
        };
        let orchestrator = app.orchestrator.clone();
        let process = claude::spawn(app, &orchestrator, &config, &launch, resume.as_deref(), generation)?;
        let mut state = self.state.lock();
        state.process = Some(process);
        state.process_cost = 0.0;
        if resume.is_none() {
            state.claude_session = None;
        }
        Ok(())
    }

    /// Starts a turn for `text` now.
    fn begin_turn(&self, app: &AppState, text: String, context: MessageContext) -> Result<(), String> {
        if let Err(message) = self.ensure_process(app) {
            let mut state = self.state.lock();
            state.error = Some(message.clone());
            state.seq += 1;
            drop(state);
            self.publish_status();
            return Err(message);
        }
        let turn_id = uuid::Uuid::new_v4().simple().to_string();
        let mut user = Orchestrator::new_item(&turn_id, "user", "done");
        user.text = text.clone();
        self.push_item(user);
        self.emit(super::OrchestratorUpdate::Started { text: text.clone(), tag: context.tag.clone() });
        let line = claude::user_line(&context::message_with_snapshot(app, &text, &context));
        {
            let mut state = self.state.lock();
            state.turn = Some(ActiveTurn {
                id: turn_id.clone(),
                started_at: iso_now(),
                step: "thinking".into(),
                tag: context.tag.clone(),
            });
            state.stream = claude::StreamState::default();
            state.error = None;
            state.seq += 1;
            self.persist_history_locked(&state);
        }
        let sent = self
            .state
            .lock()
            .process
            .as_ref()
            .ok_or_else(|| "Claude Code is not running.".to_string())
            .and_then(|process| process.send(line));
        if let Err(message) = sent {
            let mut failure = Orchestrator::new_item(&turn_id, "error", "error");
            failure.text = message.clone();
            self.push_item(failure);
            self.finish_turn(&turn_id, Some(message), Default::default());
            return Ok(());
        }
        self.publish_status();
        Ok(())
    }

    /// Runs the oldest queued message, if nothing else is running.
    pub(super) fn start_queued(&self, app: &AppState) {
        let next = {
            let mut state = self.state.lock();
            if state.turn.is_some() || state.discard_until_result || state.queue.is_empty() {
                return;
            }
            state.queue.remove(0)
        };
        let (text, context) = next;
        if let Err(message) = self.begin_turn(app, text, context) {
            tracing::warn!("orchestrator: queued message failed: {message}");
        }
    }
}
