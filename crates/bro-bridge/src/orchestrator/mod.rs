//! The Orchestrator ("Hugh"): a chat agent built into the terminal host that
//! watches every tab and drives them through tools. Its brain is one
//! long-lived headless Claude Code process ([`claude`]) on the user's own
//! login, which reaches the tools through the bridge's MCP endpoint
//! ([`mcp`]). It keeps one shared transcript that the web, native and mobile
//! panels all render, and takes messages from any of them or from bro's
//! push-to-talk voice input.

mod cast;
mod chats;
mod claude;
mod context;
mod folders;
pub(crate) mod mcp;
mod tools;
mod turn;

pub use claude::OrchestratorLaunch;
pub use turn::{send_message, send_message_with};

use crate::model::{ServerEvent, iso_now};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::sync::broadcast;

pub const DEFAULT_MODEL: &str = "sonnet";
/// Model aliases Claude Code accepts, offered by the settings panel.
pub const MODELS: [(&str, &str); 4] = [
    ("sonnet", "Sonnet (fast, recommended)"),
    ("haiku", "Haiku (fastest)"),
    ("opus", "Opus (strongest)"),
    ("fable", "Fable"),
];

const CONFIG_FILE: &str = ".terminal-web-orchestrator.json";
const HISTORY_FILE: &str = ".terminal-web-orchestrator-history.json";
/// Transcript items kept in memory and on disk.
const MAX_ITEMS: usize = 400;
/// Longest a single streamed tool result may be.
const MAX_TOOL_RESULT: usize = 24_000;
/// Minimum gap between streamed transcript broadcasts for one item.
const STREAM_PUBLISH_INTERVAL: Duration = Duration::from_millis(80);
/// Messages that may wait while a turn runs.
const MAX_QUEUE: usize = 8;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct OrchestratorConfig {
    /// Claude Code model alias or full id.
    pub model: String,
    /// `off` (no extended thinking; fastest), `low`, `medium` or `high`.
    pub reasoning: String,
}

impl Default for OrchestratorConfig {
    fn default() -> Self {
        Self {
            model: DEFAULT_MODEL.into(),
            reasoning: "off".into(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// Raw JSON text of the arguments.
    pub arguments: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ToolRecord {
    pub call_id: String,
    pub name: String,
    pub arguments: Value,
    pub summary: String,
    pub result: String,
    pub ok: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptItem {
    pub id: String,
    /// Bumped on every change to this item.
    pub rev: u64,
    /// Global sequence at the item's last change; clients poll with `since`.
    pub seq: u64,
    pub turn_id: String,
    /// `user`, `assistant`, `tool` or `error`.
    pub role: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<ToolRecord>,
    /// `streaming`, `done`, `cancelled` or `error`.
    pub status: String,
    pub at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Usage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub cost: f64,
    pub turns: u64,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct History {
    items: Vec<TranscriptItem>,
    usage: Usage,
    /// Claude Code session to `--resume` after a restart.
    claude_session: Option<String>,
}

struct ActiveTurn {
    id: String,
    started_at: String,
    step: String,
    /// The sender's tag for this message ([`MessageContext::tag`]).
    tag: Option<String>,
}

struct State {
    config: OrchestratorConfig,
    launch: OrchestratorLaunch,
    items: Vec<TranscriptItem>,
    usage: Usage,
    seq: u64,
    turn: Option<ActiveTurn>,
    error: Option<String>,
    claude_session: Option<String>,
    process: Option<claude::Process>,
    generation: u64,
    /// The process's cumulative `total_cost_usd` at its last result.
    process_cost: f64,
    /// Set by cancel: events until the interrupted turn's `result` are dropped.
    discard_until_result: bool,
    stream: claude::StreamState,
    /// Messages sent while a turn was running, oldest first.
    queue: Vec<(String, MessageContext)>,
}

/// Where a message came from, so Hugh can tell a prompt meant for the pane
/// the user is looking at from an instruction meant for Hugh.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MessageContext {
    /// Dictated through push-to-talk (and so transcribed, possibly misheard).
    pub voice: bool,
    /// The session focused in bro when the user started speaking.
    pub focused_session: Option<String>,
    /// Echoed on every [`OrchestratorUpdate`] of this message's turn, so the
    /// sender can match the answer to the question (a voice delegation id).
    pub tag: Option<String>,
}

/// Progress of a turn, for bro's own UI (voice toasts, status line).
#[derive(Clone, Debug, PartialEq)]
/// `tag` is the sender's [`MessageContext::tag`] for the message the turn answers.
pub enum OrchestratorUpdate {
    /// A message started a turn.
    Started { text: String, tag: Option<String> },
    /// A tool call began: its one-line description ("Opened a session in justgains").
    Step { summary: String, tag: Option<String> },
    /// The turn ended; `text` is the final answer.
    Reply { text: String, tag: Option<String> },
    Failed { message: String, tag: Option<String> },
}

pub type UpdateSink = std::sync::Arc<dyn Fn(OrchestratorUpdate) + Send + Sync>;

pub struct Orchestrator {
    state: Mutex<State>,
    data_root: PathBuf,
    events: broadcast::Sender<ServerEvent>,
    sinks: parking_lot::RwLock<Vec<UpdateSink>>,
}

fn read_json<T: for<'de> Deserialize<'de> + Default>(path: &Path) -> T {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn write_json<T: Serialize>(path: &Path, value: &T) {
    let Ok(bytes) = serde_json::to_vec_pretty(value) else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let temporary = path.with_extension("json.tmp");
    if std::fs::write(&temporary, bytes).is_ok() {
        let _ = std::fs::rename(&temporary, path);
    }
}

/// Configs written by the OpenRouter-era orchestrator carry an OpenRouter
/// model id; anything that is not a Claude model falls back to the default.
fn migrate(mut config: OrchestratorConfig) -> OrchestratorConfig {
    if let Some(bare) = config.model.strip_prefix("anthropic/") {
        config.model = bare.to_string();
    }
    if config.model.contains('/') || config.model.trim().is_empty() {
        config.model = DEFAULT_MODEL.into();
    }
    if !matches!(config.reasoning.as_str(), "off" | "low" | "medium" | "high") {
        config.reasoning = "off".into();
    }
    config
}

fn public_config(config: &OrchestratorConfig) -> Value {
    json!({
        "provider": "claude-code",
        "model": config.model,
        "reasoning": config.reasoning,
        // The panel's key fields: Claude Code uses the user's own login.
        "keySource": "login",
        "keyPreview": Value::Null,
        "name": cast::COACH,
        "defaults": { "model": DEFAULT_MODEL, "reasoning": "off" }
    })
}

impl Orchestrator {
    pub fn new(data_root: &Path, events: broadcast::Sender<ServerEvent>) -> Self {
        let config = migrate(read_json(&data_root.join(CONFIG_FILE)));
        let history: History = read_json(&data_root.join(HISTORY_FILE));
        let mut items = history.items;
        // A turn that was streaming when the host went away never finished.
        for item in &mut items {
            if item.status == "streaming" {
                item.status = "cancelled".into();
            }
        }
        let seq = items.iter().map(|item| item.seq).max().unwrap_or(0);
        Self {
            state: Mutex::new(State {
                config,
                launch: OrchestratorLaunch::default(),
                items,
                usage: history.usage,
                seq,
                turn: None,
                error: None,
                claude_session: history.claude_session,
                process: None,
                generation: 0,
                process_cost: 0.0,
                discard_until_result: false,
                stream: claude::StreamState::default(),
                queue: Vec::new(),
            }),
            data_root: data_root.to_path_buf(),
            events,
            sinks: parking_lot::RwLock::new(Vec::new()),
        }
    }

    pub fn on_update(&self, sink: UpdateSink) {
        self.sinks.write().push(sink);
    }

    /// A message that could not even start a turn.
    pub(crate) fn emit_failure(&self, message: String, tag: Option<String>) {
        self.emit(OrchestratorUpdate::Failed { message, tag });
    }

    fn turn_tag(&self) -> Option<String> {
        self.state.lock().turn.as_ref().and_then(|turn| turn.tag.clone())
    }

    fn emit(&self, update: OrchestratorUpdate) {
        let sinks = self.sinks.read().clone();
        for sink in sinks {
            sink(update.clone());
        }
    }

    fn publish(&self, value: Value) {
        let _ = self.events.send(ServerEvent::global(value));
    }

    fn persist_config(&self, config: &OrchestratorConfig) {
        write_json(&self.data_root.join(CONFIG_FILE), config);
    }

    fn persist_history_locked(&self, state: &State) {
        write_json(
            &self.data_root.join(HISTORY_FILE),
            &History {
                items: state.items.clone(),
                usage: state.usage.clone(),
                claude_session: state.claude_session.clone(),
            },
        );
    }

    pub fn public_config(&self) -> Value {
        public_config(&self.state.lock().config)
    }

    /// How the Claude Code process is started; takes effect on the next message.
    pub fn set_launch(&self, launch: OrchestratorLaunch) {
        self.state.lock().launch = launch;
    }

    fn status_locked(state: &State, since: Option<u64>, include_transcript: bool) -> Value {
        let running = state.turn.is_some();
        let mut status = json!({
            "state": if running { "running" } else { "idle" },
            "seq": state.seq,
            "config": public_config(&state.config),
            "error": state.error,
            "usage": state.usage,
            "itemCount": state.items.len(),
            "queued": state.queue.len(),
            "activeTurn": state.turn.as_ref().map(|turn| json!({
                "id": turn.id,
                "startedAt": turn.started_at,
                "step": turn.step
            }))
        });
        if include_transcript {
            let items: Vec<&TranscriptItem> = match since {
                Some(since) => state.items.iter().filter(|item| item.seq > since).collect(),
                None => state.items.iter().collect(),
            };
            status["transcript"] = json!(items);
            status["partial"] = json!(since.is_some());
        }
        status
    }

    pub fn status(&self, since: Option<u64>, include_transcript: bool) -> Value {
        Self::status_locked(&self.state.lock(), since, include_transcript)
    }

    fn publish_status(&self) {
        let status = self.status(None, false);
        self.publish(json!({ "type": "orchestrator", "orchestrator": status }));
    }

    /// Accepts `model` and `reasoning`. Fields of the retired OpenRouter
    /// settings (provider, baseUrl, apiKey, keyEnv) are ignored so older
    /// panels keep working.
    pub fn update_config(&self, patch: &Value) -> Result<Value, String> {
        let text = |key: &str| {
            patch
                .get(key)
                .and_then(Value::as_str)
                .map(str::trim)
                .map(str::to_owned)
        };
        let config = {
            let mut state = self.state.lock();
            let mut config = state.config.clone();
            if let Some(model) = text("model") {
                if model.is_empty() {
                    return Err("A model is required (sonnet, haiku, opus or a full id).".into());
                }
                config.model = model;
            }
            if let Some(reasoning) = text("reasoning") {
                if !matches!(reasoning.as_str(), "off" | "low" | "medium" | "high") {
                    return Err("reasoning must be off, low, medium or high.".into());
                }
                config.reasoning = reasoning;
            }
            let config = migrate(config);
            if state.config != config {
                state.config = config.clone();
                state.error = None;
                state.seq += 1;
            }
            config
        };
        self.persist_config(&config);
        self.publish_status();
        Ok(public_config(&config))
    }

    /// Marks every streaming item of the transcript `status`.
    fn close_streaming_items(&self, status: &str) {
        let changed = {
            let mut state = self.state.lock();
            state.seq += 1;
            let seq = state.seq;
            let now = iso_now();
            let mut changed = Vec::new();
            for item in state
                .items
                .iter_mut()
                .filter(|item| item.status == "streaming")
            {
                item.status = status.into();
                item.finished_at = Some(now.clone());
                item.rev += 1;
                item.seq = seq;
                changed.push(item.clone());
            }
            changed
        };
        for item in changed {
            self.publish_item(&item);
        }
    }

    /// Ends the running turn, if any, and drops queued messages. Claude Code
    /// is interrupted (and killed if it ignores that); its conversation
    /// survives either way.
    pub fn cancel(self: &std::sync::Arc<Self>) -> bool {
        let generation = {
            let mut state = self.state.lock();
            let Some(_) = state.turn.take() else {
                return false;
            };
            state.queue.clear();
            state.seq += 1;
            let generation = state.process.as_ref().map(|process| {
                let _ = process.send(claude::interrupt_line());
                process.generation
            });
            state.discard_until_result = generation.is_some();
            state.stream = claude::StreamState::default();
            generation
        };
        if let Some(generation) = generation
            && tokio::runtime::Handle::try_current().is_ok()
        {
            self.watch_interrupt(generation);
        }
        self.close_streaming_items("cancelled");
        {
            let state = self.state.lock();
            self.persist_history_locked(&state);
        }
        self.publish_status();
        true
    }

    /// Clears the transcript and starts Claude Code afresh (a new conversation).
    pub fn clear(self: &std::sync::Arc<Self>) {
        self.cancel();
        let process = {
            let mut state = self.state.lock();
            state.items.clear();
            state.error = None;
            state.claude_session = None;
            state.discard_until_result = false;
            state.seq += 1;
            self.persist_history_locked(&state);
            state.process.take()
        };
        if let Some(process) = process {
            process.kill();
        }
        let seq = self.state.lock().seq;
        self.publish(json!({ "type": "orchestrator_reset", "seq": seq }));
        self.publish_status();
    }

    /// Stops Claude Code (bridge shutdown).
    pub fn shutdown(&self) {
        if let Some(process) = self.state.lock().process.take() {
            process.kill();
        }
    }

    fn publish_item(&self, item: &TranscriptItem) {
        self.publish(json!({ "type": "orchestrator_item", "item": item, "seq": item.seq }));
    }

    fn push_item(&self, mut item: TranscriptItem) -> TranscriptItem {
        {
            let mut state = self.state.lock();
            state.seq += 1;
            item.seq = state.seq;
            state.items.push(item.clone());
            if state.items.len() > MAX_ITEMS {
                let excess = state.items.len() - MAX_ITEMS;
                state.items.drain(..excess);
            }
        }
        self.publish_item(&item);
        item
    }

    /// Applies `update` to one item, bumps its revision and broadcasts it.
    fn update_item(&self, id: &str, persist: bool, update: impl FnOnce(&mut TranscriptItem)) {
        let item = {
            let mut state = self.state.lock();
            state.seq += 1;
            let seq = state.seq;
            let Some(item) = state.items.iter_mut().find(|item| item.id == id) else {
                return;
            };
            update(item);
            item.rev += 1;
            item.seq = seq;
            let snapshot = item.clone();
            if persist {
                self.persist_history_locked(&state);
            }
            snapshot
        };
        self.publish_item(&item);
    }

    fn set_step(&self, step: &str) {
        let mut state = self.state.lock();
        if let Some(turn) = state.turn.as_mut() {
            turn.step = step.to_string();
        }
    }

    fn finish_turn(&self, turn_id: &str, error: Option<String>, usage: Usage) {
        let update = match &error {
            Some(message) => OrchestratorUpdate::Failed { message: message.clone(), tag: self.turn_tag() },
            None => OrchestratorUpdate::Reply {
                tag: self.turn_tag(),
                text: self
                    .state
                    .lock()
                    .items
                    .iter()
                    .rev()
                    .find(|item| item.turn_id == turn_id && item.role == "assistant" && !item.text.trim().is_empty())
                    .map(|item| item.text.clone())
                    .unwrap_or_default(),
            },
        };
        self.emit(update);
        {
            let mut state = self.state.lock();
            if state.turn.as_ref().is_some_and(|turn| turn.id == turn_id) {
                state.turn = None;
            }
            state.stream = claude::StreamState::default();
            state.error = error;
            state.usage.prompt_tokens += usage.prompt_tokens;
            state.usage.completion_tokens += usage.completion_tokens;
            state.usage.cost += usage.cost;
            state.usage.turns += 1;
            state.seq += 1;
            self.persist_history_locked(&state);
        }
        self.publish_status();
    }

    fn new_item(turn_id: &str, role: &str, status: &str) -> TranscriptItem {
        TranscriptItem {
            id: uuid::Uuid::new_v4().simple().to_string(),
            rev: 1,
            seq: 0,
            turn_id: turn_id.to_string(),
            role: role.to_string(),
            text: String::new(),
            reasoning: None,
            tool_calls: Vec::new(),
            tool: None,
            status: status.to_string(),
            at: iso_now(),
            finished_at: None,
            model: None,
        }
    }

    /// The models the settings panel offers (Claude Code aliases).
    pub fn models(&self) -> Value {
        let current = self.state.lock().config.model.clone();
        let models: Vec<Value> = MODELS
            .iter()
            .map(|(id, name)| {
                json!({
                    "id": id, "name": name, "provider": "anthropic", "description": "",
                    "contextLength": 0, "promptPrice": 0.0, "completionPrice": 0.0,
                    "tools": true, "reasoning": true, "created": 0
                })
            })
            .collect();
        json!({
            "fetchedAt": iso_now(),
            "source": "claude-code",
            "recommended": models,
            "models": models,
            "current": current,
            "error": Value::Null
        })
    }

    /// Checks that Claude Code starts (`claude --version`).
    pub async fn test_connection(&self) -> Value {
        let launch = self.state.lock().launch.clone();
        let mut args = launch.args.clone();
        args.push("--version".into());
        let mut command = claude::command_for(&launch, args);
        command
            .envs(launch.env.iter().map(|(key, value)| (key, value)))
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        match tokio::time::timeout(Duration::from_secs(20), command.output()).await {
            Ok(Ok(output)) if output.status.success() => json!({
                "ok": true,
                "message": format!("Claude Code {} is ready.", String::from_utf8_lossy(&output.stdout).trim())
            }),
            Ok(Ok(output)) => json!({
                "ok": false,
                "message": format!("Claude Code failed: {}", String::from_utf8_lossy(&output.stderr).trim())
            }),
            Ok(Err(error)) => json!({
                "ok": false,
                "message": format!("Could not start {}: {error}", launch.program.display())
            }),
            Err(_) => json!({ "ok": false, "message": "Claude Code did not answer within 20 seconds." }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openrouter_era_configs_migrate_to_claude_code() {
        let old = OrchestratorConfig {
            model: "anthropic/claude-sonnet-5".into(),
            reasoning: "low".into(),
        };
        assert_eq!(migrate(old).model, "claude-sonnet-5");
        let foreign = OrchestratorConfig {
            model: "openai/gpt-5".into(),
            reasoning: "max".into(),
        };
        let migrated = migrate(foreign);
        assert_eq!(migrated.model, DEFAULT_MODEL);
        assert_eq!(migrated.reasoning, "off");
    }

    #[test]
    fn config_updates_validate_and_ignore_retired_fields() {
        let root = tempfile::tempdir().unwrap();
        let (events, _) = broadcast::channel(8);
        let orchestrator = Orchestrator::new(root.path(), events);
        let updated = orchestrator
            .update_config(&json!({ "provider": "openrouter", "baseUrl": "x", "model": "haiku", "reasoning": "off" }))
            .unwrap();
        assert_eq!(updated["provider"], "claude-code");
        assert_eq!(updated["model"], "haiku");
        assert!(orchestrator.update_config(&json!({ "reasoning": "max" })).is_err());
        assert!(orchestrator.update_config(&json!({ "model": "" })).is_err());
        let persisted: OrchestratorConfig = read_json(&root.path().join(CONFIG_FILE));
        assert_eq!(persisted.model, "haiku");
    }

    #[test]
    fn transcript_survives_a_restart_with_streaming_items_cancelled() {
        let root = tempfile::tempdir().unwrap();
        let (events, _) = broadcast::channel(8);
        let orchestrator = Orchestrator::new(root.path(), events.clone());
        let mut item = Orchestrator::new_item("turn", "assistant", "streaming");
        item.text = "half an answer".into();
        orchestrator.push_item(item);
        {
            let mut state = orchestrator.state.lock();
            state.items[0].seq = 5;
            state.claude_session = Some("session-1".into());
            orchestrator.persist_history_locked(&state);
        }

        let reloaded = Orchestrator::new(root.path(), events);
        let status = reloaded.status(None, true);
        assert_eq!(status["transcript"][0]["status"], "cancelled");
        assert_eq!(status["transcript"][0]["text"], "half an answer");
        assert_eq!(reloaded.state.lock().claude_session.as_deref(), Some("session-1"));
        let partial = reloaded.status(Some(5), true);
        assert_eq!(partial["transcript"].as_array().unwrap().len(), 0);
        assert_eq!(partial["partial"], true);
    }
}
