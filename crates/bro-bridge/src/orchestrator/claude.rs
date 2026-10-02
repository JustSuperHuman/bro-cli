//! The orchestrator's brain: one long-lived headless Claude Code process
//! (`claude -p --input-format stream-json --output-format stream-json`)
//! that keeps its conversation across messages and reaches bro through the
//! bridge's own MCP endpoint ([`super::mcp`]). This module spawns and
//! supervises it and turns its event stream into transcript items.

use super::{MAX_TOOL_RESULT, Orchestrator, OrchestratorConfig, ToolCall, ToolRecord, Usage, context, tools};
use crate::model::iso_now;
use crate::state::AppState;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot};

const SYSTEM_PROMPT_FILE: &str = ".orchestrator-system-prompt.md";
const MCP_CONFIG_FILE: &str = ".orchestrator-mcp.json";
/// MCP tools reach Claude as `mcp__<server>__<tool>`.
const TOOL_PREFIX: &str = "mcp__bro__";
/// Built-in tools the orchestrator may use (reading transcripts and files).
const BUILTIN_TOOLS: [&str; 3] = ["Read", "Grep", "Glob"];
/// Lines of Claude Code's stderr kept for error messages.
const STDERR_LINES: usize = 20;
/// An interrupt that produces no `result` within this long kills the process.
const INTERRUPT_GRACE: Duration = Duration::from_secs(4);

/// Session-scoped variables a parent Claude Code leaves in bro's environment;
/// inherited, they would make the child think it is nested in that session.
const INHERITED_SESSION_VARS: [&str; 11] = [
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_SESSION_ATTENDED",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
    "CLAUDE_CODE_EXECPATH",
    "CLAUDE_PID",
    "CLAUDE_EFFORT",
    "BRO_SESSION_ID",
];

/// How to start Claude Code: bro fills this in for the login the orchestrator
/// should use (see [`crate::Bridge::set_orchestrator_launch`]); the default
/// runs `claude` from `PATH` with bro's own environment.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OrchestratorLaunch {
    pub program: PathBuf,
    /// Placed before the orchestrator's own arguments.
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub env_remove: Vec<String>,
    /// Working directory; default the home directory.
    pub cwd: Option<PathBuf>,
}

impl Default for OrchestratorLaunch {
    fn default() -> Self {
        Self {
            program: PathBuf::from("claude"),
            args: Vec::new(),
            env: Vec::new(),
            env_remove: Vec::new(),
            cwd: None,
        }
    }
}

/// A running Claude Code process.
pub(super) struct Process {
    pub generation: u64,
    /// What it was started with; a change restarts it before the next message.
    pub fingerprint: String,
    input: mpsc::UnboundedSender<String>,
    kill: Option<oneshot::Sender<()>>,
}

impl Process {
    pub fn send(&self, line: String) -> Result<(), String> {
        self.input
            .send(line)
            .map_err(|_| "Claude Code is not running.".to_string())
    }

    pub fn kill(mut self) {
        if let Some(kill) = self.kill.take() {
            let _ = kill.send(());
        }
    }
}

/// Per-turn bookkeeping for mapping the stream onto transcript items.
#[derive(Default)]
pub(super) struct StreamState {
    /// API message id -> assistant item id
    messages: HashMap<String, String>,
    /// Final text gathered from `assistant` events, per assistant item.
    texts: HashMap<String, String>,
    /// Live text from partial deltas, per assistant item.
    partial: HashMap<String, String>,
    /// tool_use id -> tool item id
    tools: HashMap<String, String>,
    current: Option<String>,
    last_publish: Option<Instant>,
}

fn home_dir() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .filter(|home| !home.is_empty())
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
}

pub(super) fn fingerprint(config: &OrchestratorConfig, launch: &OrchestratorLaunch) -> String {
    serde_json::to_string(&json!([config.model, config.reasoning, launch])).unwrap_or_default()
}

/// The arguments after the launch's own prefix.
pub(super) fn arguments(
    config: &OrchestratorConfig,
    system_prompt: &str,
    mcp_config: &str,
    resume: Option<&str>,
) -> Vec<String> {
    let mut args: Vec<String> = [
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
        "--include-partial-messages",
        "--model",
        &config.model,
        "--system-prompt-file",
        system_prompt,
        "--mcp-config",
        mcp_config,
        "--strict-mcp-config",
        "--permission-mode",
        "dontAsk",
        "--tools",
        &BUILTIN_TOOLS.join(","),
        "--allowedTools",
    ]
    .iter()
    .map(|arg| arg.to_string())
    .collect();
    args.push("mcp__bro".into());
    args.extend(BUILTIN_TOOLS.iter().map(|tool| tool.to_string()));
    if config.reasoning != "off" {
        args.push("--effort".into());
        args.push(config.reasoning.clone());
    }
    if let Some(session) = resume {
        args.push("--resume".into());
        args.push(session.to_string());
    }
    args
}

/// One stream-json user message.
pub(super) fn user_line(text: &str) -> String {
    json!({
        "type": "user",
        "message": { "role": "user", "content": [{ "type": "text", "text": text }] }
    })
    .to_string()
}

pub(super) fn interrupt_line() -> String {
    json!({
        "type": "control_request",
        "request_id": uuid::Uuid::new_v4().simple().to_string(),
        "request": { "subtype": "interrupt" }
    })
    .to_string()
}

fn write_private(path: &std::path::Path, text: &str) -> Result<(), String> {
    let temporary = path.with_extension("tmp");
    std::fs::write(&temporary, text)
        .and_then(|()| std::fs::rename(&temporary, path))
        .map_err(|error| format!("Could not write {}: {error}", path.display()))
}

/// Starts Claude Code. Its stdout is parsed into `orchestrator` events tagged
/// with `generation`, so output of a process that was replaced is ignored.
pub(super) fn spawn(
    app: &AppState,
    orchestrator: &Arc<Orchestrator>,
    config: &OrchestratorConfig,
    launch: &OrchestratorLaunch,
    resume: Option<&str>,
    generation: u64,
) -> Result<Process, String> {
    let port = app.inner.lock().port;
    let data_root = app.data_root.as_ref();
    std::fs::create_dir_all(data_root).map_err(|error| error.to_string())?;
    let prompt_path = data_root.join(SYSTEM_PROMPT_FILE);
    write_private(&prompt_path, &context::system_prompt(config))?;
    let mcp_path = data_root.join(MCP_CONFIG_FILE);
    let mcp = json!({
        "mcpServers": {
            super::mcp::SERVER_NAME: {
                "type": "http",
                "url": format!("http://127.0.0.1:{port}/mcp"),
                "headers": { "Authorization": format!("Bearer {}", app.token) }
            }
        }
    });
    write_private(&mcp_path, &serde_json::to_string_pretty(&mcp).unwrap_or_default())?;

    let mut full_args = launch.args.clone();
    full_args.extend(arguments(
        config,
        &prompt_path.to_string_lossy(),
        &mcp_path.to_string_lossy(),
        resume,
    ));
    let mut command = command_for(launch, full_args);
    command
        .current_dir(launch.cwd.clone().unwrap_or_else(home_dir))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for name in INHERITED_SESSION_VARS.iter().copied().chain(launch.env_remove.iter().map(String::as_str)) {
        command.env_remove(name);
    }
    if config.reasoning == "off" {
        command.env("MAX_THINKING_TOKENS", "0");
    }
    command.envs(launch.env.iter().map(|(key, value)| (key, value)));
    #[cfg(windows)]
    command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW

    let mut child = command.spawn().map_err(|error| {
        format!(
            "Could not start Claude Code ({}): {error}. Install it or set the orchestrator's launch command.",
            launch.program.display()
        )
    })?;
    let mut stdin = child.stdin.take().ok_or("Claude Code has no stdin.")?;
    let stdout = child.stdout.take().ok_or("Claude Code has no stdout.")?;
    let stderr = child.stderr.take().ok_or("Claude Code has no stderr.")?;

    let (input, mut lines) = mpsc::unbounded_channel::<String>();
    tokio::spawn(async move {
        while let Some(line) = lines.recv().await {
            if stdin.write_all(line.as_bytes()).await.is_err()
                || stdin.write_all(b"\n").await.is_err()
                || stdin.flush().await.is_err()
            {
                break;
            }
        }
    });

    let stderr_tail = Arc::new(parking_lot::Mutex::new(Vec::<String>::new()));
    let tail_for_reader = stderr_tail.clone();
    tokio::spawn(async move {
        let mut reader = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            let mut tail = tail_for_reader.lock();
            tail.push(line);
            if tail.len() > STDERR_LINES {
                tail.remove(0);
            }
        }
    });

    let (kill, killed) = oneshot::channel::<()>();
    let for_stdout = orchestrator.clone();
    let app_for_stdout = app.clone();
    tokio::spawn(async move {
        let mut reader = BufReader::new(stdout).lines();
        let mut killed = killed;
        loop {
            tokio::select! {
                line = reader.next_line() => match line {
                    Ok(Some(line)) => {
                        if let Ok(event) = serde_json::from_str::<Value>(&line) {
                            for_stdout.on_stream_event(&app_for_stdout, generation, &event);
                        }
                    }
                    _ => break,
                },
                _ = &mut killed => {
                    let _ = child.start_kill();
                    break;
                }
            }
        }
        let status = child.wait().await.ok();
        let detail = stderr_tail.lock().join("\n");
        for_stdout.on_process_exit(generation, status.and_then(|status| status.code()), detail);
    });

    Ok(Process {
        generation,
        fingerprint: fingerprint(config, launch),
        input,
        kill: Some(kill),
    })
}

/// `launch.program` resolved like bro resolves agents (PATH, then the usual
/// install dirs; `.cmd` shims through `cmd /c` on Windows).
pub(super) fn command_for(launch: &OrchestratorLaunch, args: Vec<String>) -> tokio::process::Command {
    let (program, args) =
        bro_core::launch::program_and_args(&launch.program.to_string_lossy(), args);
    let mut command = tokio::process::Command::new(program);
    command.args(args);
    command
}

fn tool_result_text(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|block| block.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn trimmed(mut text: String) -> String {
    if text.chars().count() > MAX_TOOL_RESULT {
        text = text.chars().take(MAX_TOOL_RESULT).collect();
        text.push_str("\n…[result trimmed]");
    }
    text
}

/// Short tool name for the transcript: bro's tools lose their MCP prefix.
pub(super) fn tool_name(name: &str) -> &str {
    name.strip_prefix(TOOL_PREFIX).unwrap_or(name)
}

impl Orchestrator {
    /// Assistant item for an API message id, created on first sight.
    fn assistant_item(&self, turn_id: &str, message_id: &str) -> String {
        if let Some(item) = self.state.lock().stream.messages.get(message_id).cloned() {
            return item;
        }
        let item = self.push_item(Orchestrator::new_item(turn_id, "assistant", "streaming"));
        let mut state = self.state.lock();
        state.stream.messages.insert(message_id.to_string(), item.id.clone());
        state.stream.current = Some(item.id.clone());
        item.id
    }

    /// One line of Claude Code's stream-json output.
    pub(super) fn on_stream_event(&self, app: &AppState, generation: u64, event: &Value) {
        let kind = event.get("type").and_then(Value::as_str).unwrap_or("");
        let (turn_id, discarding) = {
            let mut state = self.state.lock();
            if state.process.as_ref().map(|process| process.generation) != Some(generation) {
                return;
            }
            if kind == "system"
                && event.get("subtype").and_then(Value::as_str) == Some("init")
                && let Some(session) = event.get("session_id").and_then(Value::as_str)
                && state.claude_session.as_deref() != Some(session)
            {
                state.claude_session = Some(session.to_string());
                self.persist_history_locked(&state);
            }
            (state.turn.as_ref().map(|turn| turn.id.clone()), state.discard_until_result)
        };
        if discarding {
            if kind == "result" {
                self.after_discarded_result(app);
            }
            return;
        }
        let Some(turn_id) = turn_id else { return };
        match kind {
            "stream_event" => self.on_partial(&turn_id, event.get("event").unwrap_or(&Value::Null)),
            "assistant" => self.on_assistant(app, &turn_id, event.get("message").unwrap_or(&Value::Null)),
            "user" => self.on_tool_results(event.get("message").unwrap_or(&Value::Null)),
            "result" => self.on_result(app, &turn_id, event),
            _ => {}
        }
    }

    fn on_partial(&self, turn_id: &str, event: &Value) {
        match event.get("type").and_then(Value::as_str) {
            Some("message_start") => {
                if let Some(id) = event.pointer("/message/id").and_then(Value::as_str) {
                    self.assistant_item(turn_id, id);
                }
            }
            Some("content_block_delta") => {
                let Some(delta) = event
                    .get("delta")
                    .filter(|delta| delta.get("type").and_then(Value::as_str) == Some("text_delta"))
                    .and_then(|delta| delta.get("text"))
                    .and_then(Value::as_str)
                else {
                    return;
                };
                let (item, text, publish) = {
                    let mut state = self.state.lock();
                    let Some(item) = state.stream.current.clone() else { return };
                    let text = {
                        let partial = state.stream.partial.entry(item.clone()).or_default();
                        partial.push_str(delta);
                        partial.clone()
                    };
                    let publish = state
                        .stream
                        .last_publish
                        .is_none_or(|at| at.elapsed() >= super::STREAM_PUBLISH_INTERVAL);
                    if publish {
                        state.stream.last_publish = Some(Instant::now());
                    }
                    (item, text, publish)
                };
                if publish {
                    self.update_item(&item, false, |entry| entry.text = text);
                }
            }
            _ => {}
        }
    }

    fn on_assistant(&self, app: &AppState, turn_id: &str, message: &Value) {
        let message_id = message
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| uuid::Uuid::new_v4().simple().to_string());
        let item = self.assistant_item(turn_id, &message_id);
        let model = message.get("model").and_then(Value::as_str).map(str::to_owned);
        let blocks = message
            .get("content")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut calls = Vec::new();
        for block in &blocks {
            match block.get("type").and_then(Value::as_str) {
                Some("text") => {
                    let text = block.get("text").and_then(Value::as_str).unwrap_or("");
                    let mut state = self.state.lock();
                    let entry = state.stream.texts.entry(item.clone()).or_default();
                    if !entry.is_empty() && !text.is_empty() {
                        entry.push_str("\n\n");
                    }
                    entry.push_str(text);
                }
                Some("tool_use") => {
                    let id = block.get("id").and_then(Value::as_str).unwrap_or("").to_string();
                    let name = tool_name(block.get("name").and_then(Value::as_str).unwrap_or("tool")).to_string();
                    let input = block.get("input").cloned().unwrap_or_else(|| json!({}));
                    calls.push(ToolCall {
                        id: id.clone(),
                        name: name.clone(),
                        arguments: input.to_string(),
                    });
                    self.set_step(&name);
                    let mut record = Orchestrator::new_item(turn_id, "tool", "streaming");
                    let summary = tools::summarize(app, &name, &input);
                    self.emit(super::OrchestratorUpdate::Step { summary: summary.clone(), tag: self.turn_tag() });
                    record.tool = Some(ToolRecord {
                        call_id: id.clone(),
                        summary,
                        name,
                        arguments: input,
                        result: String::new(),
                        ok: true,
                    });
                    let record = self.push_item(record);
                    self.state.lock().stream.tools.insert(id, record.id);
                }
                _ => {}
            }
        }
        let text = {
            let state = self.state.lock();
            state
                .stream
                .texts
                .get(&item)
                .filter(|text| !text.is_empty())
                .or_else(|| state.stream.partial.get(&item))
                .cloned()
                .unwrap_or_default()
        };
        self.update_item(&item, true, |entry| {
            entry.text = text;
            entry.tool_calls.extend(calls);
            if model.is_some() {
                entry.model = model;
            }
            entry.status = "done".into();
            entry.finished_at = Some(iso_now());
        });
        self.set_step("thinking");
    }

    fn on_tool_results(&self, message: &Value) {
        let Some(blocks) = message.get("content").and_then(Value::as_array) else { return };
        for block in blocks {
            if block.get("type").and_then(Value::as_str) != Some("tool_result") {
                continue;
            }
            let Some(item) = block
                .get("tool_use_id")
                .and_then(Value::as_str)
                .and_then(|id| self.state.lock().stream.tools.get(id).cloned())
            else {
                continue;
            };
            let ok = !block.get("is_error").and_then(Value::as_bool).unwrap_or(false);
            let result = trimmed(tool_result_text(block.get("content").unwrap_or(&Value::Null)));
            self.update_item(&item, true, |entry| {
                if let Some(tool) = entry.tool.as_mut() {
                    tool.result = result;
                    tool.ok = ok;
                }
                entry.status = if ok { "done".into() } else { "error".into() };
                entry.finished_at = Some(iso_now());
            });
        }
    }

    fn on_result(&self, app: &AppState, turn_id: &str, event: &Value) {
        let usage = event.get("usage").cloned().unwrap_or(Value::Null);
        let count = |key: &str| usage.get(key).and_then(Value::as_u64).unwrap_or(0);
        // total_cost_usd is cumulative for the process.
        let cumulative = event.get("total_cost_usd").and_then(Value::as_f64).unwrap_or(0.0);
        let cost = {
            let mut state = self.state.lock();
            let delta = (cumulative - state.process_cost).max(0.0);
            state.process_cost = cumulative;
            delta
        };
        let turn_usage = Usage {
            prompt_tokens: count("input_tokens")
                + count("cache_read_input_tokens")
                + count("cache_creation_input_tokens"),
            completion_tokens: count("output_tokens"),
            cost,
            turns: 0,
        };
        let failed = event.get("is_error").and_then(Value::as_bool).unwrap_or(false)
            || event
                .get("subtype")
                .and_then(Value::as_str)
                .is_some_and(|subtype| subtype != "success");
        let error = failed.then(|| {
            event
                .get("result")
                .and_then(Value::as_str)
                .filter(|text| !text.trim().is_empty())
                .map(str::to_owned)
                .or_else(|| {
                    event
                        .get("errors")
                        .and_then(Value::as_array)
                        .map(|errors| errors.iter().map(|e| e.as_str().map(str::to_owned).unwrap_or_else(|| e.to_string())).collect::<Vec<_>>().join("; "))
                })
                .unwrap_or_else(|| {
                    format!(
                        "Claude Code ended the turn: {}",
                        event.get("subtype").and_then(Value::as_str).unwrap_or("error")
                    )
                })
        });
        if let Some(message) = &error {
            let mut failure = Orchestrator::new_item(turn_id, "error", "error");
            failure.text = message.clone();
            failure.finished_at = Some(iso_now());
            self.push_item(failure);
        }
        self.close_streaming_items("done");
        self.finish_turn(turn_id, error, turn_usage);
        self.start_queued(app);
    }

    /// The `result` that ends an interrupted turn: the next queued message
    /// (sent after the cancel) may start now.
    fn after_discarded_result(&self, app: &AppState) {
        self.state.lock().discard_until_result = false;
        self.start_queued(app);
    }

    pub(super) fn on_process_exit(&self, generation: u64, code: Option<i32>, stderr: String) {
        let turn_id = {
            let mut state = self.state.lock();
            if state.process.as_ref().map(|process| process.generation) != Some(generation) {
                return;
            }
            state.process = None;
            state.process_cost = 0.0;
            state.discard_until_result = false;
            state.turn.as_ref().map(|turn| turn.id.clone())
        };
        let Some(turn_id) = turn_id else { return };
        let detail = stderr.trim();
        let message = format!(
            "Claude Code stopped{}{}",
            code.map(|code| format!(" (exit code {code})")).unwrap_or_default(),
            if detail.is_empty() { ".".to_string() } else { format!(": {detail}") }
        );
        let mut failure = Orchestrator::new_item(&turn_id, "error", "error");
        failure.text = message.clone();
        failure.finished_at = Some(iso_now());
        self.push_item(failure);
        self.close_streaming_items("error");
        self.finish_turn(&turn_id, Some(message), Usage::default());
        self.state.lock().queue.clear();
    }

    /// Kills a process that ignored an interrupt.
    pub(super) fn watch_interrupt(self: &Arc<Self>, generation: u64) {
        let orchestrator = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(INTERRUPT_GRACE).await;
            let stale = {
                let mut state = orchestrator.state.lock();
                if state.discard_until_result
                    && state.process.as_ref().is_some_and(|process| process.generation == generation)
                {
                    state.discard_until_result = false;
                    state.process.take()
                } else {
                    None
                }
            };
            if let Some(process) = stale {
                process.kill();
            }
        });
    }
}

#[cfg(test)]
impl Process {
    /// A process that is not running, for feeding recorded events.
    pub fn detached(generation: u64) -> (Self, mpsc::UnboundedReceiver<String>) {
        let (input, lines) = mpsc::unbounded_channel();
        (
            Self {
                generation,
                fingerprint: String::new(),
                input,
                kill: None,
            },
            lines,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BridgeConfig;
    use crate::orchestrator::OrchestratorUpdate;

    fn test_app(root: &std::path::Path) -> AppState {
        let config = BridgeConfig {
            data_root: root.to_path_buf(),
            ..BridgeConfig::default()
        };
        AppState::new(config, "token".into(), Arc::new(|_| {}))
    }

    /// A recorded turn: partial text, a bro tool call and its result, the
    /// answer, the result line.
    #[test]
    fn a_stream_json_turn_becomes_transcript_items() {
        let root = tempfile::tempdir().unwrap();
        let app = test_app(root.path());
        let orchestrator = app.orchestrator.clone();
        let updates = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let sink = updates.clone();
        orchestrator.on_update(Arc::new(move |update| sink.lock().push(update)));
        let (process, _lines) = Process::detached(7);
        {
            let mut state = orchestrator.state.lock();
            state.process = Some(process);
            state.turn = Some(super::super::ActiveTurn {
                id: "turn".into(),
                started_at: iso_now(),
                step: "thinking".into(),
                tag: None,
            });
        }
        let events = [
            json!({ "type": "system", "subtype": "init", "session_id": "0f0e0d0c-0000-4000-8000-000000000001" }),
            json!({ "type": "stream_event", "event": { "type": "message_start", "message": { "id": "m1" } } }),
            json!({ "type": "stream_event", "event": { "type": "content_block_delta", "delta": { "type": "text_delta", "text": "Opening" } } }),
            json!({ "type": "assistant", "message": { "id": "m1", "model": "claude-sonnet-5-5", "content": [{ "type": "text", "text": "Opening it." }] } }),
            json!({ "type": "assistant", "message": { "id": "m1", "content": [{ "type": "tool_use", "id": "t1", "name": "mcp__bro__list_projects", "input": {} }] } }),
            json!({ "type": "user", "message": { "content": [{ "type": "tool_result", "tool_use_id": "t1", "content": [{ "type": "text", "text": "{\"projects\":[]}" }] }] } }),
            json!({ "type": "assistant", "message": { "id": "m2", "content": [{ "type": "text", "text": "Toast is up in justgains." }] } }),
            json!({ "type": "result", "subtype": "success", "is_error": false, "total_cost_usd": 0.01, "usage": { "input_tokens": 3, "cache_read_input_tokens": 100, "output_tokens": 20 } }),
        ];
        // Output of a replaced process is ignored.
        orchestrator.on_stream_event(&app, 6, &events[6]);
        for event in &events {
            orchestrator.on_stream_event(&app, 7, event);
        }
        let status = orchestrator.status(None, true);
        let items = status["transcript"].as_array().unwrap();
        let roles: Vec<&str> = items.iter().map(|item| item["role"].as_str().unwrap()).collect();
        assert_eq!(roles, ["assistant", "tool", "assistant"]);
        assert_eq!(items[0]["text"], "Opening it.");
        assert_eq!(items[0]["toolCalls"][0]["name"], "list_projects");
        assert_eq!(items[1]["tool"]["name"], "list_projects");
        assert_eq!(items[1]["tool"]["result"], "{\"projects\":[]}");
        assert_eq!(items[1]["status"], "done");
        assert_eq!(items[2]["text"], "Toast is up in justgains.");
        assert_eq!(status["state"], "idle");
        assert_eq!(status["usage"]["promptTokens"], 103);
        assert_eq!(
            orchestrator.state.lock().claude_session.as_deref(),
            Some("0f0e0d0c-0000-4000-8000-000000000001")
        );
        let updates = updates.lock();
        assert!(matches!(&updates[0], OrchestratorUpdate::Step { summary, .. } if summary == "Listed projects"));
        assert_eq!(
            updates.last(),
            Some(&OrchestratorUpdate::Reply { text: "Toast is up in justgains.".into(), tag: None })
        );
    }

    #[test]
    fn a_dead_process_fails_the_turn_with_its_stderr() {
        let root = tempfile::tempdir().unwrap();
        let app = test_app(root.path());
        let orchestrator = app.orchestrator.clone();
        let (process, _lines) = Process::detached(1);
        {
            let mut state = orchestrator.state.lock();
            state.process = Some(process);
            state.turn = Some(super::super::ActiveTurn {
                id: "turn".into(),
                started_at: iso_now(),
                step: "thinking".into(),
                tag: None,
            });
        }
        orchestrator.on_process_exit(1, Some(1), "Not logged in".into());
        let status = orchestrator.status(None, true);
        assert_eq!(status["state"], "idle");
        assert!(status["error"].as_str().unwrap().contains("Not logged in"));
        assert_eq!(status["transcript"][0]["role"], "error");
        assert!(orchestrator.state.lock().process.is_none());
    }

    /// Real Claude Code on this machine's login: one message through the
    /// whole path (spawn, MCP tools over HTTP, stream parsing).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "runs the real claude CLI (network, uses the login)"]
    async fn real_claude_code_answers_through_bro_tools() {
        let root = tempfile::tempdir().unwrap();
        let bridge = crate::Bridge::start_with(
            BridgeConfig {
                port: 0,
                automatic_port: false,
                bind: "127.0.0.1".into(),
                data_root: root.path().join("bridge"),
                web_interface: false,
            },
            crate::StartOptions { import_legacy_token: false },
            Box::new(|_| {}),
        )
        .unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel();
        bridge.on_orchestrator_update(Box::new(move |update| {
            let _ = tx.send(update);
        }));
        let started = Instant::now();
        bridge
            .orchestrator_send("Call list_sessions, then tell me how many sessions are open, in one sentence.".into())
            .unwrap();
        let mut steps = Vec::new();
        let reply = loop {
            match tokio::time::timeout(Duration::from_secs(90), rx.recv()).await.unwrap().unwrap() {
                OrchestratorUpdate::Step { summary, .. } => steps.push(summary),
                OrchestratorUpdate::Reply { text, .. } => break text,
                OrchestratorUpdate::Failed { message, .. } => panic!("failed: {message}"),
                OrchestratorUpdate::Started { .. } => {}
            }
        };
        eprintln!("reply after {:.1}s: {reply} (steps: {steps:?})", started.elapsed().as_secs_f32());
        assert!(steps.iter().any(|step| step == "Listed every terminal"), "{steps:?}");
        bridge.shutdown();
    }

    /// Real Claude Code handling a dictated command end to end, with a stand-in
    /// for bro's PTY side: the project must resolve on this machine and the
    /// session must get a squad name. Set BRO_TEST_PROJECT to a folder name
    /// that exists here (default "justgains").
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "runs the real claude CLI (network, uses the login)"]
    async fn real_claude_code_opens_a_named_session_from_speech() {
        use crate::{BridgeCommand, SessionMeta};
        use std::sync::OnceLock;
        let project = std::env::var("BRO_TEST_PROJECT").unwrap_or_else(|_| "justgains".into());
        let root = tempfile::tempdir().unwrap();
        let slot: Arc<OnceLock<crate::Bridge>> = Arc::new(OnceLock::new());
        let created = Arc::new(parking_lot::Mutex::new(Vec::<(Option<String>, Option<PathBuf>)>::new()));
        let (slot_cb, created_cb) = (slot.clone(), created.clone());
        let bridge = crate::Bridge::start_with(
            BridgeConfig {
                port: 0,
                automatic_port: false,
                bind: "127.0.0.1".into(),
                data_root: root.path().join("bridge"),
                web_interface: false,
            },
            crate::StartOptions { import_legacy_token: false },
            Box::new(move |command| {
                if let BridgeCommand::Create { req, reply } = command {
                    created_cb.lock().push((req.title.clone(), req.cwd.clone()));
                    let id = uuid::Uuid::new_v4().to_string();
                    slot_cb.get().unwrap().register(SessionMeta {
                        id: id.clone(),
                        title: "claude".into(),
                        shell: "claude".into(),
                        args: vec![],
                        cwd: req.cwd.clone(),
                        project: None,
                        pid: None,
                        cols: 100,
                        rows: 30,
                        agent: None,
                    });
                    let _ = reply.send(Ok(id));
                }
            }),
        )
        .unwrap();
        let _ = slot.set(bridge.clone());
        let (tx, mut rx) = mpsc::unbounded_channel();
        bridge.on_orchestrator_update(Box::new(move |update| {
            let _ = tx.send(update);
        }));
        let started = Instant::now();
        // As speech recognition would deliver it.
        bridge.orchestrator_send(format!("open a session in {project}")).unwrap();
        let reply = loop {
            match tokio::time::timeout(Duration::from_secs(120), rx.recv()).await.unwrap().unwrap() {
                OrchestratorUpdate::Reply { text, .. } => break text,
                OrchestratorUpdate::Failed { message, .. } => panic!("failed: {message}"),
                _ => {}
            }
        };
        let created = created.lock().clone();
        eprintln!("reply after {:.1}s: {reply}\ncreated: {created:?}", started.elapsed().as_secs_f32());
        let (title, cwd) = created.first().expect("a session was created").clone();
        assert_eq!(title.as_deref(), Some(format!("Mal · {project}").as_str()));
        let cwd = cwd.expect("cwd");
        assert!(
            cwd.file_name().unwrap().to_string_lossy().eq_ignore_ascii_case(&project),
            "{cwd:?}"
        );
        assert!(reply.contains("Mal"), "the reply names the session: {reply}");
        bridge.shutdown();
    }

    #[test]
    fn arguments_run_headless_with_bro_tools_only() {
        let config = OrchestratorConfig::default();
        let args = arguments(&config, "prompt.md", "mcp.json", Some("abc"));
        let joined = args.join(" ");
        assert!(joined.starts_with("-p --input-format stream-json --output-format stream-json"));
        assert!(joined.contains("--model sonnet"));
        assert!(joined.contains("--strict-mcp-config"));
        assert!(joined.contains("--allowedTools mcp__bro Read Grep Glob"));
        assert!(joined.ends_with("--resume abc"));
        assert!(!joined.contains("--effort"), "thinking off uses MAX_THINKING_TOKENS, not effort");
        let high = OrchestratorConfig { reasoning: "high".into(), ..config };
        assert!(arguments(&high, "p", "m", None).join(" ").contains("--effort high"));
    }

    #[test]
    fn stream_lines_are_single_json_objects() {
        let line = user_line("open a session\nin justgains");
        assert!(!line.contains('\n'));
        let parsed: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(parsed["message"]["content"][0]["text"], "open a session\nin justgains");
        assert_eq!(serde_json::from_str::<Value>(&interrupt_line()).unwrap()["request"]["subtype"], "interrupt");
        assert_eq!(tool_name("mcp__bro__open_session"), "open_session");
        assert_eq!(tool_name("Read"), "Read");
    }

    /// Real Claude Code routing dictation: prompts for the focused agent go
    /// into it verbatim; instructions for Hugh don't touch it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "runs the real claude CLI (network, uses the login)"]
    async fn real_claude_code_routes_dictation_by_focus() {
        use crate::orchestrator::MessageContext;
        use crate::{BridgeCommand, SessionMeta};
        let root = tempfile::tempdir().unwrap();
        let typed = Arc::new(parking_lot::Mutex::new(String::new()));
        let creates = Arc::new(parking_lot::Mutex::new(0usize));
        let (typed_cb, creates_cb) = (typed.clone(), creates.clone());
        let bridge = crate::Bridge::start_with(
            BridgeConfig {
                port: 0,
                automatic_port: false,
                bind: "127.0.0.1".into(),
                data_root: root.path().join("bridge"),
                web_interface: false,
            },
            crate::StartOptions { import_legacy_token: false },
            Box::new(move |command| match command {
                BridgeCommand::Input { id, data } if id == "mal" => {
                    typed_cb.lock().push_str(&String::from_utf8_lossy(&data))
                }
                // Declined: only whether Hugh tried matters here.
                BridgeCommand::Create { reply, .. } => {
                    *creates_cb.lock() += 1;
                    let _ = reply.send(Err(anyhow::anyhow!("test host opens no sessions")));
                }
                _ => {}
            }),
        )
        .unwrap();
        bridge.register(SessionMeta {
            id: "mal".into(),
            title: "Mal · justgains".into(),
            shell: "claude".into(),
            args: vec![],
            cwd: Some(PathBuf::from(r"J:\justgains")),
            project: None,
            pid: None,
            cols: 100,
            rows: 30,
            agent: Some("claude".into()),
        });
        bridge.output("mal", "> \r\n".as_bytes());
        let (tx, mut rx) = mpsc::unbounded_channel();
        bridge.on_orchestrator_update(Box::new(move |update| {
            let _ = tx.send(update);
        }));
        let cases = [
            ("add a loading spinner to the workout screen while it syncs", true),
            ("hey Hugh, what's Mal working on right now?", false),
            ("why is the login test failing", true),
            ("open a session in bro cli v2", false),
        ];
        for (words, is_prompt) in cases {
            typed.lock().clear();
            let creates_before = *creates.lock();
            let started = Instant::now();
            bridge
                .orchestrator_send_with(
                    words.into(),
                    MessageContext { voice: true, focused_session: Some("mal".into()), tag: None },
                )
                .unwrap();
            let reply = loop {
                match tokio::time::timeout(Duration::from_secs(120), rx.recv()).await.unwrap().unwrap() {
                    OrchestratorUpdate::Reply { text, .. } => break text,
                    OrchestratorUpdate::Failed { message, .. } => break format!("FAILED: {message}"),
                    _ => {}
                }
            };
            let sent = typed.lock().clone();
            eprintln!(
                "[{:.1}s] {words:?}\n   reply: {reply}\n   typed into Mal: {sent:?}",
                started.elapsed().as_secs_f32()
            );
            if is_prompt {
                let key_word = words.split_whitespace().find(|w| w.len() > 6).unwrap();
                assert!(sent.contains(key_word), "{words:?} should go to Mal verbatim, got {sent:?}");
            } else {
                assert!(sent.is_empty(), "{words:?} is for Hugh, but Mal got {sent:?}");
            }
            if words.starts_with("open") {
                assert!(*creates.lock() > creates_before, "Hugh should have tried to open a session");
            }
        }
        bridge.shutdown();
    }
}
