//! One wake-up: a fresh headless reviewer (`claude -p` or `codex exec`) on the login bro picked, wired to
//! the bridge's own MCP endpoint. Its tool calls become the wake-up's steps and its last message the report.

use super::{ReviewerKind, WatchdogSnapshot, WatchdogWake, prompt};
use crate::orchestrator::claude::INHERITED_SESSION_VARS;
use crate::orchestrator::tools;
use crate::state::AppState;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::oneshot;

/// A reviewer that hasn't finished by now is stopped.
const RUN_TIMEOUT: Duration = Duration::from_secs(20 * 60);
/// Lines of stderr kept for the error message.
const STDERR_LINES: usize = 12;
/// Read-only git the Claude reviewer may run (`--permission-mode dontAsk` refuses everything else).
const GIT_READS: [&str; 5] = ["Bash(git status:*)", "Bash(git diff:*)", "Bash(git log:*)", "Bash(git show:*)", "Bash(git branch:*)"];
const BUILTIN_TOOLS: [&str; 4] = ["Read", "Grep", "Glob", "Bash"];

/// Claims the next wake-up of `session_id` and starts its reviewer on the bridge runtime.
pub(crate) fn spawn(app: &AppState, session_id: &str, wake: WatchdogWake) -> Result<(), String> {
    let runtime = app.runtime.get().cloned().ok_or("the bridge is not running")?;
    let (kill, killed) = oneshot::channel();
    let (generation, snapshot) = app.watchdogs.begin(session_id, &wake.reason, &wake.reviewer.label, kill)?;
    let app = app.clone();
    let session_id = session_id.to_string();
    runtime.spawn(async move {
        let (summary, ok) = match review(&app, &session_id, generation, &snapshot, &wake, killed).await {
            Ok(text) => (text, true),
            Err(error) => (error, false),
        };
        app.watchdogs.finish(&session_id, generation, summary, ok);
    });
    Ok(())
}

/// A wake-up for bro's orchestrator session instead of a headless reviewer: claims it like [`spawn`] (the
/// watchdog shows it running) and returns its generation and the whole brief, instructions first.
pub(crate) fn brief_for_orchestrator(app: &AppState, session_id: &str, wake: WatchdogWake) -> Result<(u64, String), String> {
    // stopping the watchdog ends the wake-up on bro's side; nothing to kill here
    let (kill, _) = oneshot::channel();
    let (generation, snapshot) = app.watchdogs.begin(session_id, &wake.reason, &wake.reviewer.label, kill)?;
    let helpers: Vec<(String, String)> = snapshot.helpers.iter().map(|h| (h.clone(), helper_state(app, h))).collect();
    let brief = prompt::brief(session_id, &snapshot, &wake, &helpers);
    Ok((generation, format!("{}\n\n---\n\n{brief}", wake.prompt)))
}

/// `claude -p` arguments: stream-json out, bro's MCP tools plus read-only project access, nothing else.
pub(crate) fn claude_args(prompt_file: &str, mcp_file: &str, model: Option<&str>) -> Vec<String> {
    let mut args: Vec<String> = ["-p", "--output-format", "stream-json", "--verbose", "--system-prompt-file", prompt_file, "--mcp-config", mcp_file, "--strict-mcp-config", "--permission-mode", "dontAsk"]
        .iter()
        .map(|a| a.to_string())
        .collect();
    if let Some(model) = model.filter(|m| !m.trim().is_empty()) {
        args.extend(["--model".into(), model.to_string()]);
    }
    args.extend(["--tools".into(), BUILTIN_TOOLS.join(",")]);
    // variadic: last, so it swallows nothing (the prompt arrives on stdin)
    args.push("--allowedTools".into());
    args.push("mcp__bro".into());
    args.extend(["Read", "Grep", "Glob"].iter().map(|t| t.to_string()));
    args.extend(GIT_READS.iter().map(|t| t.to_string()));
    args
}

/// `codex exec` arguments: JSON events, a read-only sandbox, bro's MCP server, the prompt from stdin.
pub(crate) fn codex_args(cwd: &Path, mcp_url: &str, model: Option<&str>) -> Vec<String> {
    let mut args: Vec<String> = vec!["exec".into(), "--json".into(), "--skip-git-repo-check".into(), "-s".into(), "read-only".into(), "-C".into(), cwd.to_string_lossy().into_owned()];
    args.extend(["-c".into(), format!("mcp_servers.bro.url={}", serde_json::to_string(mcp_url).unwrap_or_default())]);
    // exec can't ask anyone: without this, Codex's approval review rejects every bro tool call
    args.extend(["-c".into(), "mcp_servers.bro.default_tools_approval_mode=\"approve\"".into()]);
    if let Some(model) = model.filter(|m| !m.trim().is_empty()) {
        args.extend(["-m".into(), model.to_string()]);
    }
    args.push("-".into());
    args
}

fn home_dir() -> PathBuf {
    std::env::var_os("USERPROFILE").filter(|h| !h.is_empty()).or_else(|| std::env::var_os("HOME")).map(PathBuf::from).unwrap_or_else(std::env::temp_dir)
}

/// A file name part from a session id.
fn safe(id: &str) -> String {
    id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').take(48).collect()
}

fn write_atomic(path: &Path, text: &str) -> Result<(), String> {
    let temporary = path.with_extension("tmp");
    std::fs::write(&temporary, text).and_then(|()| std::fs::rename(&temporary, path)).map_err(|e| format!("couldn't write {}: {e}", path.display()))
}

/// "Pinky · justgains, claude, idle" for the brief.
fn helper_state(app: &AppState, id: &str) -> String {
    match app.session_view(id) {
        None => "closed".into(),
        Some(view) => format!("{}, {}, {}", view.summary.title, view.summary.agent.as_deref().unwrap_or("shell"), view.summary.agent_activity.as_deref().unwrap_or("idle")),
    }
}

async fn review(app: &AppState, session_id: &str, generation: u64, snapshot: &WatchdogSnapshot, wake: &WatchdogWake, mut killed: oneshot::Receiver<()>) -> Result<String, String> {
    let helpers: Vec<(String, String)> = snapshot.helpers.iter().map(|h| (h.clone(), helper_state(app, h))).collect();
    let brief = prompt::brief(session_id, snapshot, wake, &helpers);
    let dir = app.data_root.join("watchdog");
    std::fs::create_dir_all(&dir).map_err(|e| format!("couldn't create {}: {e}", dir.display()))?;
    let port = app.inner.lock().port;
    let mcp_url = format!("http://127.0.0.1:{port}/mcp");
    let model = wake.reviewer.model.as_deref();
    let (args, input) = match wake.reviewer.kind {
        ReviewerKind::Claude => {
            let prompt_path = dir.join(format!("{}-prompt.md", safe(session_id)));
            write_atomic(&prompt_path, &wake.prompt)?;
            let mcp_path = dir.join(format!("{}-mcp.json", safe(session_id)));
            let mcp = json!({ "mcpServers": { crate::orchestrator::mcp::SERVER_NAME: { "type": "http", "url": mcp_url, "headers": { "Authorization": format!("Bearer {}", app.token) } } } });
            write_atomic(&mcp_path, &serde_json::to_string_pretty(&mcp).unwrap_or_default())?;
            (claude_args(&prompt_path.to_string_lossy(), &mcp_path.to_string_lossy(), model), brief)
        }
        // codex exec has no system prompt of its own: the instructions lead the message
        ReviewerKind::Codex => (codex_args(&wake.session.cwd, &mcp_url, model), format!("{}\n\n---\n\n{brief}", wake.prompt)),
    };
    let (program, args) = bro_core::launch::program_and_args(&wake.reviewer.program, args);
    let mut command = tokio::process::Command::new(&program);
    command
        .args(&args)
        .current_dir(if wake.session.cwd.is_dir() { wake.session.cwd.clone() } else { home_dir() })
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for name in INHERITED_SESSION_VARS.iter().copied().chain(wake.reviewer.env_remove.iter().map(String::as_str)) {
        command.env_remove(name);
    }
    command.envs(wake.reviewer.env.iter().map(|(k, v)| (k, v)));
    let mut tree = bro_core::proc_tree::ProcessTree::new().map_err(|e| format!("reviewer process owner: {e}"))?;
    tree.configure_command(command.as_std_mut());
    let mut child = command.spawn().map_err(|e| format!("couldn't start {} ({program}): {e}", wake.reviewer.label))?;
    tree.attach(child.id().ok_or("the reviewer has no process id")?).map_err(|e| format!("reviewer process ownership: {e}"))?;
    let mut stdin = child.stdin.take().ok_or("the reviewer has no stdin")?;
    let stdout = child.stdout.take().ok_or("the reviewer has no stdout")?;
    let stderr = child.stderr.take().ok_or("the reviewer has no stderr")?;
    tokio::spawn(async move {
        let _ = stdin.write_all(input.as_bytes()).await;
        let _ = stdin.shutdown().await;
    });
    let tail = Arc::new(parking_lot::Mutex::new(Vec::<String>::new()));
    let tail_writer = tail.clone();
    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let mut tail = tail_writer.lock();
            tail.push(line);
            if tail.len() > STDERR_LINES {
                tail.remove(0);
            }
        }
    });

    let mut parsed = Parsed::default();
    let mut lines = BufReader::new(stdout).lines();
    let deadline = tokio::time::sleep(RUN_TIMEOUT);
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            line = lines.next_line() => match line {
                Ok(Some(line)) => {
                    let Ok(event) = serde_json::from_str::<Value>(&line) else { continue };
                    let steps = match wake.reviewer.kind {
                        ReviewerKind::Claude => parsed.claude(app, &event),
                        ReviewerKind::Codex => parsed.codex(app, &event),
                    };
                    for step in steps {
                        app.watchdogs.step(session_id, generation, step);
                    }
                }
                _ => break,
            },
            _ = &mut killed => {
                tree.terminate();
                let _ = child.kill().await;
                return Err("stopped before it finished".into());
            }
            _ = &mut deadline => {
                tree.terminate();
                let _ = child.kill().await;
                return Err(format!("the reviewer took longer than {} minutes and was stopped", RUN_TIMEOUT.as_secs() / 60));
            }
        }
    }
    let status = child.wait().await.ok();
    tree.terminate();
    let stderr = tail.lock().join(" ").trim().to_string();
    if let Some(error) = parsed.error {
        return Err(error);
    }
    // every bro tool call refused (an approval policy, a dead endpoint): the reviewer never reached the
    // session, so this wake-up failed whatever its report says, and bro retries instead of going quiet
    if parsed.bro_calls > 0 && parsed.bro_failed == parsed.bro_calls {
        let report = parsed.text.as_deref().map(str::trim).filter(|t| !t.is_empty()).map(|t| format!(": {}", prompt::clip(t, 300))).unwrap_or_default();
        return Err(format!("{} couldn't use bro's tools (all {} calls were refused){report}", wake.reviewer.label, parsed.bro_calls));
    }
    match parsed.text.filter(|t| !t.trim().is_empty()) {
        Some(text) => Ok(text.trim().to_string()),
        None if status.is_some_and(|s| s.success()) => Ok("The reviewer finished without a report.".into()),
        None => Err(format!(
            "{} stopped{}{}",
            wake.reviewer.label,
            status.and_then(|s| s.code()).map(|c| format!(" (exit code {c})")).unwrap_or_default(),
            if stderr.is_empty() { String::new() } else { format!(": {}", prompt::clip(&stderr, 400)) }
        )),
    }
}

/// What the reviewer's event stream said so far.
#[derive(Default, Debug)]
pub(crate) struct Parsed {
    /// The last thing it wrote: its report.
    pub text: Option<String>,
    pub error: Option<String>,
    /// bro tool calls finished, and how many of them came back refused or failed
    pub bro_calls: usize,
    pub bro_failed: usize,
    /// Claude's ids for its bro tool calls, to match their results
    bro_ids: Vec<String>,
}

impl Parsed {
    /// One `claude -p --output-format stream-json` line; returns the tool calls it started.
    pub(crate) fn claude(&mut self, app: &AppState, event: &Value) -> Vec<String> {
        let mut steps = vec![];
        match event.get("type").and_then(Value::as_str) {
            Some("assistant") => {
                for block in event.pointer("/message/content").and_then(Value::as_array).into_iter().flatten() {
                    match block.get("type").and_then(Value::as_str) {
                        Some("text") => {
                            if let Some(text) = block.get("text").and_then(Value::as_str).filter(|t| !t.trim().is_empty()) {
                                self.text = Some(text.to_string());
                            }
                        }
                        Some("tool_use") => {
                            let name = block.get("name").and_then(Value::as_str).unwrap_or("tool");
                            if name.starts_with("mcp__bro__")
                                && let Some(id) = block.get("id").and_then(Value::as_str)
                            {
                                self.bro_ids.push(id.to_string());
                            }
                            steps.push(describe(app, name, block.get("input").unwrap_or(&Value::Null)));
                        }
                        _ => {}
                    }
                }
            }
            Some("user") => {
                for block in event.pointer("/message/content").and_then(Value::as_array).into_iter().flatten() {
                    let id = block.get("tool_use_id").and_then(Value::as_str).unwrap_or_default();
                    if block.get("type").and_then(Value::as_str) == Some("tool_result") && self.bro_ids.iter().any(|b| b == id) {
                        self.bro_calls += 1;
                        if block.get("is_error").and_then(Value::as_bool).unwrap_or(false) {
                            self.bro_failed += 1;
                        }
                    }
                }
            }
            Some("result") => {
                let text = event.get("result").and_then(Value::as_str).filter(|t| !t.trim().is_empty());
                let failed = event.get("is_error").and_then(Value::as_bool).unwrap_or(false) || event.get("subtype").and_then(Value::as_str).is_some_and(|s| s != "success");
                if failed {
                    self.error = Some(text.map(str::to_owned).unwrap_or_else(|| format!("the reviewer ended early ({})", event.get("subtype").and_then(Value::as_str).unwrap_or("error"))));
                } else if let Some(text) = text {
                    self.text = Some(text.to_string());
                }
            }
            _ => {}
        }
        steps
    }

    /// One `codex exec --json` line.
    pub(crate) fn codex(&mut self, app: &AppState, event: &Value) -> Vec<String> {
        let item = event.get("item").unwrap_or(&Value::Null);
        let item_type = item.get("type").and_then(Value::as_str).unwrap_or("");
        match (event.get("type").and_then(Value::as_str), item_type) {
            (Some("item.started"), "mcp_tool_call") => {
                let tool = item.get("tool").and_then(Value::as_str).unwrap_or("tool");
                let args = match item.get("arguments") {
                    Some(Value::String(text)) => serde_json::from_str(text).unwrap_or(Value::Null),
                    Some(other) => other.clone(),
                    None => Value::Null,
                };
                vec![tools::summarize(app, tool, &args)]
            }
            (Some("item.started"), "command_execution") => {
                vec![format!("Ran {}", prompt::clip(item.get("command").and_then(Value::as_str).unwrap_or("a command"), 60))]
            }
            (Some("item.completed"), "mcp_tool_call") => {
                if item.get("server").and_then(Value::as_str).is_none_or(|s| s == "bro") {
                    self.bro_calls += 1;
                    if item.get("status").and_then(Value::as_str) == Some("failed") || item.get("error").is_some_and(|e| !e.is_null()) {
                        self.bro_failed += 1;
                    }
                }
                vec![]
            }
            (Some("item.completed"), "agent_message") => {
                if let Some(text) = item.get("text").and_then(Value::as_str).filter(|t| !t.trim().is_empty()) {
                    self.text = Some(text.to_string());
                }
                vec![]
            }
            (Some("turn.failed"), _) => {
                self.error = Some(event.pointer("/error/message").and_then(Value::as_str).unwrap_or("the reviewer's turn failed").to_string());
                vec![]
            }
            (Some("error"), _) => {
                if let Some(message) = event.get("message").and_then(Value::as_str) {
                    self.error = Some(message.to_string());
                }
                vec![]
            }
            _ => vec![],
        }
    }
}

/// One line for a tool call: bro's tools as the orchestrator words them, the built-ins by what they read.
fn describe(app: &AppState, name: &str, input: &Value) -> String {
    let arg = |key: &str| input.get(key).and_then(Value::as_str).unwrap_or_default().to_string();
    let file = |path: String| Path::new(&path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or(path);
    match name.strip_prefix("mcp__bro__") {
        Some(tool) => tools::summarize(app, tool, input),
        None => match name {
            "Read" => format!("Read {}", file(arg("file_path"))),
            "Grep" => format!("Searched for \"{}\"", prompt::clip(&arg("pattern"), 40)),
            "Glob" => format!("Listed {}", arg("pattern")),
            "Bash" => format!("Ran {}", prompt::clip(&arg("command"), 60)),
            other => other.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BridgeConfig;

    fn app(root: &Path) -> AppState {
        AppState::new(BridgeConfig { data_root: root.to_path_buf(), ..BridgeConfig::default() }, "token".into(), Arc::new(|_| {}))
    }

    /// A real reviewer on this machine's Claude login, against a stand-in session that looks idle at its
    /// prompt: it must read the session through bro's tools and hand it a task.
    #[test]
    #[ignore = "runs the real claude CLI (network, uses the login)"]
    fn a_real_reviewer_hands_an_idle_session_its_next_task() {
        real_reviewer(ReviewerLaunch { kind: ReviewerKind::Claude, program: "claude".into(), env: vec![], env_remove: vec![], model: Some("sonnet".into()), label: "claude · local · sonnet".into() });
    }

    #[test]
    #[ignore = "runs the real codex CLI (network, uses the login)"]
    fn a_real_codex_reviewer_hands_an_idle_session_its_next_task() {
        real_reviewer(ReviewerLaunch { kind: ReviewerKind::Codex, program: "codex".into(), env: vec![], env_remove: vec![], model: None, label: "codex · local".into() });
    }

    use crate::watchdog::ReviewerLaunch;

    fn real_reviewer(reviewer: ReviewerLaunch) {
        use crate::watchdog::{Milestone, WatchdogPlan, WatchdogUpdate, WatchedSession};
        use crate::{BridgeCommand, SessionMeta};
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("app");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(project.join("README.md"), "# app\nA todo list CLI. `todo add <text>` works; `todo done <n>` is not written yet.\n").unwrap();
        let typed = Arc::new(parking_lot::Mutex::new(String::new()));
        let typed_cb = typed.clone();
        let bridge = crate::Bridge::start_with(
            BridgeConfig { port: 0, automatic_port: false, bind: "127.0.0.1".into(), data_root: root.path().join("bridge"), web_interface: false },
            crate::StartOptions { import_legacy_token: false },
            Box::new(move |command| {
                if let BridgeCommand::Input { id, data } = command
                    && id == "mal"
                {
                    typed_cb.lock().push_str(&String::from_utf8_lossy(&data));
                }
            }),
        )
        .unwrap();
        bridge.register(SessionMeta { id: "mal".into(), title: "Mal · app".into(), shell: "claude".into(), args: vec![], cwd: Some(project.clone()), project: None, pid: None, cols: 100, rows: 30, agent: Some("claude".into()), kind: None });
        bridge.output("mal", "⏺ Added `todo add`. It saves to todo.json.\r\n\r\n> \r\n".as_bytes());
        let (tx, rx) = std::sync::mpsc::channel();
        bridge.on_watchdog_update(Box::new(move |u| {
            let _ = tx.send(u);
        }));
        bridge.watchdog_start("mal", WatchdogPlan { milestones: vec![Milestone::new("`todo done <n>` marks item n done and saves it"), Milestone::new("README documents every command")], max_wakes: 3, ..WatchdogPlan::default() });
        let started = std::time::Instant::now();
        bridge
            .watchdog_wake("mal", crate::watchdog::WatchdogWake {
                reason: "it finished a turn and sat idle 1 min".into(),
                reviewer,
                prompt: crate::watchdog::DEFAULT_PROMPT.into(),
                session: WatchedSession { title: "Mal · app".into(), agent: "claude · local".into(), cwd: project },
            })
            .unwrap();
        let mut steps = vec![];
        let (summary, ok) = loop {
            match rx.recv_timeout(Duration::from_secs(300)).expect("the reviewer finishes") {
                WatchdogUpdate::Step { summary, .. } => steps.push(summary),
                WatchdogUpdate::Finished { summary, ok, .. } => break (summary, ok),
                _ => {}
            }
        };
        eprintln!("after {:.0}s ok={ok}\nsteps: {steps:#?}\nreport: {summary}\ntyped into Mal: {:?}", started.elapsed().as_secs_f32(), typed.lock());
        assert!(ok, "{summary}");
        assert!(steps.iter().any(|s| s.contains("Mal")), "it looked at the session: {steps:?}");
        assert!(typed.lock().to_lowercase().contains("done"), "it handed Mal the todo done task: {:?}", typed.lock());
        bridge.shutdown();
    }

    #[test]
    fn claude_runs_headless_with_bro_tools_and_read_only_git() {
        let joined = claude_args("p.md", "m.json", Some("opus")).join(" ");
        assert!(joined.starts_with("-p --output-format stream-json --verbose --system-prompt-file p.md --mcp-config m.json --strict-mcp-config --permission-mode dontAsk --model opus"));
        assert!(joined.contains("--tools Read,Grep,Glob,Bash"));
        assert!(joined.ends_with("--allowedTools mcp__bro Read Grep Glob Bash(git status:*) Bash(git diff:*) Bash(git log:*) Bash(git show:*) Bash(git branch:*)"));
        let codex = codex_args(Path::new("J:/app"), "http://127.0.0.1:10001/mcp", None);
        assert_eq!(codex.first().map(String::as_str), Some("exec"));
        assert!(codex.contains(&"mcp_servers.bro.url=\"http://127.0.0.1:10001/mcp\"".to_string()));
        assert!(codex.contains(&"mcp_servers.bro.default_tools_approval_mode=\"approve\"".to_string()), "exec can't answer approval prompts");
        assert_eq!(codex.last().map(String::as_str), Some("-"));
    }

    #[test]
    fn streams_become_steps_and_a_report() {
        let root = tempfile::tempdir().unwrap();
        let app = app(root.path());
        let mut p = Parsed::default();
        let steps = p.claude(&app, &json!({ "type": "assistant", "message": { "content": [
            { "type": "text", "text": "Checking the chat." },
            { "type": "tool_use", "name": "mcp__bro__read_chat", "input": { "sessionId": "x" } },
            { "type": "tool_use", "name": "Read", "input": { "file_path": "F:/app/src/sync.ts" } },
            { "type": "tool_use", "name": "Bash", "input": { "command": "git diff --stat" } }
        ] } }));
        assert_eq!(steps, vec!["Read x's chat".to_string(), "Read sync.ts".into(), "Ran git diff --stat".into()]);
        p.claude(&app, &json!({ "type": "result", "subtype": "success", "is_error": false, "result": "Sent milestone 2's first task." }));
        assert_eq!(p.text.as_deref(), Some("Sent milestone 2's first task."));
        assert!(p.error.is_none());
        let mut c = Parsed::default();
        let steps = c.codex(&app, &json!({ "type": "item.started", "item": { "type": "mcp_tool_call", "server": "bro", "tool": "send_input", "arguments": "{\"sessionId\":\"x\",\"text\":\"Fix the retry\"}" } }));
        assert_eq!(steps, vec!["Typed into x: Fix the retry".to_string()]);
        c.codex(&app, &json!({ "type": "item.completed", "item": { "type": "agent_message", "text": "Done." } }));
        assert_eq!(c.text.as_deref(), Some("Done."));
        c.codex(&app, &json!({ "type": "turn.failed", "error": { "message": "usage limit" } }));
        assert_eq!(c.error.as_deref(), Some("usage limit"));
    }

    #[test]
    fn refused_bro_calls_are_counted() {
        let root = tempfile::tempdir().unwrap();
        let app = app(root.path());
        let mut c = Parsed::default();
        let refused = json!({ "type": "item.completed", "item": { "type": "mcp_tool_call", "server": "bro", "tool": "read_chat", "error": { "message": "MCP tool call requires approval, but approval policy is never" }, "status": "failed" } });
        c.codex(&app, &refused);
        c.codex(&app, &json!({ "type": "item.completed", "item": { "type": "mcp_tool_call", "server": "other", "tool": "x", "error": { "message": "no" }, "status": "failed" } }));
        assert_eq!((c.bro_calls, c.bro_failed), (1, 1), "only bro's own tools count");
        c.codex(&app, &json!({ "type": "item.completed", "item": { "type": "mcp_tool_call", "server": "bro", "tool": "list_sessions", "error": null, "status": "completed" } }));
        assert_eq!((c.bro_calls, c.bro_failed), (2, 1));

        let mut p = Parsed::default();
        p.claude(&app, &json!({ "type": "assistant", "message": { "content": [
            { "type": "tool_use", "id": "t1", "name": "mcp__bro__read_chat", "input": {} },
            { "type": "tool_use", "id": "t2", "name": "Read", "input": { "file_path": "a.rs" } }
        ] } }));
        p.claude(&app, &json!({ "type": "user", "message": { "content": [
            { "type": "tool_result", "tool_use_id": "t1", "is_error": true, "content": "denied" },
            { "type": "tool_result", "tool_use_id": "t2", "is_error": true, "content": "denied" }
        ] } }));
        assert_eq!((p.bro_calls, p.bro_failed), (1, 1));
    }
}
