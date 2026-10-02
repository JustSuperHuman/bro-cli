//! The orchestrator's tools over the terminal host: read, drive, open and
//! close sessions, find projects and read chat transcripts. Definitions are in
//! MCP form (served by [`super::mcp`]); every tool runs in process against the
//! same session registry the web and native clients use.

use super::{cast, chats, context, folders};
use crate::BridgeCommand;
use crate::commands::{compose_payload, launch_terminal, submit_settle, write_paced};
use crate::model::TerminalNotification;
use crate::prompt;
use crate::state::{AppState, COMMAND_INPUT, COMMAND_KILL};
use serde_json::{Value, json};
use std::time::{Duration, Instant};

const MAX_READ_LINES: usize = 600;
const MAX_WAIT_SECONDS: u64 = 45;
/// How long open_session waits for a new agent to reach its prompt before
/// typing the task.
const READY_TIMEOUT: Duration = Duration::from_secs(45);

fn tool(name: &str, description: &str, properties: Value, required: &[&str]) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": {
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": false
        }
    })
}

pub fn definitions() -> Vec<Value> {
    vec![
        tool(
            "list_sessions",
            "Fresh snapshot of every terminal tab: id, title, directory, git branch, project, shell, which AI agent is running and what it is doing, any question it is waiting on, and the last screen lines. Use when the workspace may have changed since the conversation started.",
            json!({}),
            &[],
        ),
        tool(
            "read_session",
            "Read a tab's screen and recent scrollback as plain text (newest lines last). Use it to see what a tab is doing in more detail than the snapshot shows.",
            json!({
                "sessionId": { "type": "string", "description": "Session id from the workspace list." },
                "lines": { "type": "integer", "description": "Trailing lines to return (default 120, max 600)." }
            }),
            &["sessionId"],
        ),
        tool(
            "send_input",
            "Type text into a tab, optionally pressing Enter afterwards. sessionId may also be the tab's name (e.g. \"Toast\"). Works for shells and for AI agent TUIs (Claude Code, Codex): to talk to an agent, send the message with submit=true, then call wait_for_output. Multi-line text is delivered as one bracketed paste when the program supports it.",
            json!({
                "sessionId": { "type": "string" },
                "text": { "type": "string", "description": "What to type." },
                "submit": { "type": "boolean", "description": "Press Enter after the text (default true)." }
            }),
            &["sessionId", "text"],
        ),
        tool(
            "send_keys",
            "Press named keys in a tab, for menus and TUIs. Keys: enter, tab, shift+tab, esc, space, backspace, up, down, left, right, home, end, pageup, pagedown, ctrl+c, ctrl+d, ctrl+l, ctrl+r, ctrl+u, ctrl+z, or a single character.",
            json!({
                "sessionId": { "type": "string" },
                "keys": { "type": "array", "items": { "type": "string" }, "description": "Keys pressed in order." }
            }),
            &["sessionId", "keys"],
        ),
        tool(
            "answer_prompt",
            "Answer the question an agent tab is currently waiting on (the snapshot lists its options). Choose an option by its number/key or label, or cancel it.",
            json!({
                "sessionId": { "type": "string" },
                "option": { "type": "string", "description": "Option key (e.g. \"1\", \"y\") or its label. Omit when cancelling." },
                "cancel": { "type": "boolean", "description": "Dismiss the prompt instead of answering (default false)." }
            }),
            &["sessionId"],
        ),
        tool(
            "wait_for_output",
            "Wait until a tab prints new output and goes quiet again (or until the timeout), then return its latest screen lines. Call this after send_input so you report what actually happened.",
            json!({
                "sessionId": { "type": "string" },
                "timeoutSeconds": { "type": "integer", "description": "Maximum wait (default 15, max 45)." },
                "lines": { "type": "integer", "description": "Trailing lines to return (default 60)." }
            }),
            &["sessionId"],
        ),
        tool(
            "open_session",
            "Open a new agent session in a project and optionally give it a task. The project may be spoken loosely (\"justgains\", \"bro cli\") or be a path; it is resolved like find_project. The session gets the next free squad name (Mal, Pinky, Toast, Minty, Lilac, Sky, Bitty) unless you pass one, so the user can refer to it by name. With a task, waits for the agent to reach its prompt and sends it. Prefer this over create_session.",
            json!({
                "project": { "type": "string", "description": "Project name or directory path." },
                "task": { "type": "string", "description": "First message to send the agent (optional)." },
                "agent": { "type": "string", "description": "claude (default), codex, pi, or shell. A login-specific id like claude:work also works." },
                "name": { "type": "string", "description": "Session name; default is the next free squad name." },
                "focus": { "type": "boolean", "description": "Bring it to the front in bro (default false; true when the user wants to see it)." }
            }),
            &["project"],
        ),
        tool(
            "find_project",
            "Resolve a spoken project name to directories on this machine (bro's projects, recent folders, and top-level folders of each drive and the home directory). Returns the best matches with scores.",
            json!({ "query": { "type": "string" } }),
            &["query"],
        ),
        tool(
            "focus_session",
            "Bring a tab to the front in bro's window.",
            json!({ "sessionId": { "type": "string" } }),
            &["sessionId"],
        ),
        tool(
            "list_chats",
            "List past and current Claude Code / Codex chats saved on disk, newest first: chatId, title (first prompt), directory, project, last activity, transcript path.",
            json!({
                "project": { "type": "string", "description": "Only chats whose project or directory contains this." },
                "query": { "type": "string", "description": "Only chats whose title contains this." },
                "limit": { "type": "integer", "description": "Default 20, max 100." }
            }),
            &[],
        ),
        tool(
            "read_chat",
            "Read a chat's conversation (user and agent messages, tool calls as one-line markers) from its transcript. Pass sessionId for a live tab's current chat, or chatId from list_chats. Use this to answer questions about what an agent did or said; it sees far more than read_session.",
            json!({
                "sessionId": { "type": "string", "description": "A live tab (id or name)." },
                "chatId": { "type": "string", "description": "A chat id from list_chats." },
                "messages": { "type": "integer", "description": "Newest messages to return (default 40, max 400)." }
            }),
            &[],
        ),
        tool(
            "create_session",
            "Open a new terminal session in bro, optionally in a directory and with a profile (shell, claude, codex, pi, omp).",
            json!({
                "title": { "type": "string", "description": "Tab title." },
                "cwd": { "type": "string", "description": "Starting directory." },
                "profileId": { "type": "string", "description": "Shell/agent profile id from the host's profiles (pwsh, cmd, claude, codex, ...)." }
            }),
            &[],
        ),
        tool(
            "close_session",
            "Close a tab (ends its process). Confirm with the user first when the tab looks busy.",
            json!({ "sessionId": { "type": "string" } }),
            &["sessionId"],
        ),
        tool(
            "rename_session",
            "Rename a tab.",
            json!({ "sessionId": { "type": "string" }, "title": { "type": "string" } }),
            &["sessionId", "title"],
        ),
        tool(
            "list_projects",
            "List the project groupings (name + directory) tabs are organized under, and recently used directories.",
            json!({}),
            &[],
        ),
        tool(
            "notify_user",
            "Send a notification (sound + toast) to the user's connected devices, e.g. when something they asked you to watch finishes.",
            json!({
                "title": { "type": "string" },
                "body": { "type": "string" }
            }),
            &["title"],
        ),
    ]
}

fn key_sequence(key: &str) -> Option<String> {
    let sequence = match key.to_ascii_lowercase().as_str() {
        "enter" | "return" => "\r",
        "tab" => "\t",
        "shift+tab" => "\x1b[Z",
        "esc" | "escape" => "\x1b",
        "space" => " ",
        "backspace" => "\x7f",
        "delete" => "\x1b[3~",
        "up" => "\x1b[A",
        "down" => "\x1b[B",
        "right" => "\x1b[C",
        "left" => "\x1b[D",
        "home" => "\x1b[H",
        "end" => "\x1b[F",
        "pageup" => "\x1b[5~",
        "pagedown" => "\x1b[6~",
        "ctrl+c" => "\x03",
        "ctrl+d" => "\x04",
        "ctrl+l" => "\x0c",
        "ctrl+r" => "\x12",
        "ctrl+u" => "\x15",
        "ctrl+z" => "\x1a",
        other => {
            let mut characters = other.chars();
            let single = characters.next()?;
            if characters.next().is_some() {
                return None;
            }
            return Some(single.to_string());
        }
    };
    Some(sequence.to_string())
}

fn string_arg(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn integer_arg(args: &Value, key: &str) -> Option<u64> {
    args.get(key).and_then(|value| {
        value
            .as_u64()
            .or_else(|| value.as_f64().map(|number| number.max(0.0) as u64))
            .or_else(|| value.as_str().and_then(|text| text.trim().parse().ok()))
    })
}

/// Resolves the tab a tool call names. Exact ids first; then an id prefix
/// or a title match, so a model that paraphrased still lands on the tab.
fn resolve_session(app: &AppState, args: &Value) -> Result<String, String> {
    let requested = string_arg(args, "sessionId").ok_or("sessionId is required.")?;
    if app.summary(&requested).is_some() {
        return Ok(requested);
    }
    let wanted = requested.to_lowercase();
    let summaries = app.summaries();
    let by_prefix: Vec<_> = summaries
        .iter()
        .filter(|session| session.id.to_lowercase().starts_with(&wanted))
        .collect();
    if by_prefix.len() == 1 {
        return Ok(by_prefix[0].id.clone());
    }
    let by_title: Vec<_> = summaries
        .iter()
        .filter(|session| session.title.to_lowercase() == wanted)
        .collect();
    if by_title.len() == 1 {
        return Ok(by_title[0].id.clone());
    }
    // A spoken name ("toast") against "Toast · justgains"; the newest wins
    // when an older tab carries the same name.
    let mut by_name: Vec<_> = summaries
        .iter()
        .filter(|session| cast::name_of(&session.title).to_lowercase() == wanted)
        .collect();
    by_name.sort_by(|left, right| right.created_at.cmp(&left.created_at));
    if let Some(session) = by_name.first() {
        return Ok(session.id.clone());
    }
    Err(format!(
        "No session matches \"{requested}\". Call list_sessions for current ids."
    ))
}

fn session_label(app: &AppState, session_id: &str) -> String {
    app.summary(session_id)
        .map(|session| session.title)
        .unwrap_or_else(|| session_id.chars().take(8).collect())
}

/// One-line description for the transcript card.
pub fn summarize(app: &AppState, name: &str, args: &Value) -> String {
    let target = || {
        string_arg(args, "sessionId")
            .map(|id| session_label(app, &id))
            .unwrap_or_else(|| "a session".into())
    };
    match name {
        "list_sessions" => "Listed every terminal".into(),
        "read_session" => format!("Read {}", target()),
        "send_input" => {
            let text = string_arg(args, "text").unwrap_or_default();
            let preview: String = text
                .lines()
                .next()
                .unwrap_or_default()
                .chars()
                .take(60)
                .collect();
            format!(
                "Typed into {}: {preview}{}",
                target(),
                if text.len() > preview.len() {
                    "…"
                } else {
                    ""
                }
            )
        }
        "send_keys" => format!(
            "Pressed {} in {}",
            args.get("keys")
                .and_then(Value::as_array)
                .map(|keys| keys
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", "))
                .unwrap_or_default(),
            target()
        ),
        "answer_prompt" => format!("Answered the prompt in {}", target()),
        "wait_for_output" => format!("Waited for {}", target()),
        "open_session" => format!(
            "Opened a session in {}",
            string_arg(args, "project").unwrap_or_else(|| "a project".into())
        ),
        "find_project" => format!(
            "Looked up \"{}\"",
            string_arg(args, "query").unwrap_or_default()
        ),
        "focus_session" => format!("Brought {} to the front", target()),
        "list_chats" => "Listed chats".into(),
        "read_chat" => match string_arg(args, "chatId") {
            Some(chat) => format!("Read chat {}", chat.chars().take(8).collect::<String>()),
            None => format!("Read {}'s chat", target()),
        },
        "create_session" => format!(
            "Opened a terminal{}",
            string_arg(args, "cwd")
                .map(|cwd| format!(" in {cwd}"))
                .unwrap_or_default()
        ),
        "close_session" => format!("Closed {}", target()),
        "rename_session" => format!(
            "Renamed {} to \"{}\"",
            target(),
            string_arg(args, "title").unwrap_or_default()
        ),
        "list_projects" => "Listed projects".into(),
        "notify_user" => format!(
            "Notified you: {}",
            string_arg(args, "title").unwrap_or_default()
        ),
        other => other.to_string(),
    }
}

async fn wait_for_output(
    app: &AppState,
    session_id: &str,
    timeout: Duration,
    lines: usize,
) -> String {
    let started = Instant::now();
    let initial = app.session_seq(session_id).unwrap_or(0);
    let quiet_window = Duration::from_millis(900);
    let mut last_change = None::<Instant>;
    let mut last_seq = initial;
    let mut outcome = "timed out with no new output";
    while started.elapsed() < timeout {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let Some(seq) = app.session_seq(session_id) else {
            outcome = "the session closed";
            break;
        };
        if seq != last_seq {
            last_seq = seq;
            last_change = Some(Instant::now());
        }
        if let Some(changed) = last_change {
            let view = app.session_view(session_id);
            let activity = view
                .as_ref()
                .and_then(|view| view.summary.agent_activity.clone());
            let settled = changed.elapsed() >= quiet_window;
            let agent_done = activity
                .as_deref()
                .is_some_and(|activity| activity != "working");
            if settled && (activity.is_none() || agent_done) {
                outcome = "output settled";
                break;
            }
            if settled && changed.elapsed() >= Duration::from_secs(6) {
                // Still working: hand back what is on screen so the model can
                // decide whether to keep waiting.
                outcome = "still working (returning the current screen)";
                break;
            }
        }
    }
    let text = app.session_text(session_id, lines).unwrap_or_default();
    let view = app.session_view(session_id);
    let state = view
        .as_ref()
        .map(|view| {
            let agent = view.summary.agent.clone().unwrap_or_else(|| "shell".into());
            let activity = view.summary.agent_activity.clone().unwrap_or_default();
            let prompt = view
                .prompt
                .as_ref()
                .and_then(|prompt| prompt.get("title"))
                .and_then(Value::as_str)
                .map(|title| format!(", waiting on: {title}"))
                .unwrap_or_default();
            format!("{agent} {activity}{prompt}").trim().to_string()
        })
        .unwrap_or_default();
    format!(
        "[{outcome} after {:.1}s; state: {state}]\n{text}",
        started.elapsed().as_secs_f32()
    )
}

pub async fn execute(app: &AppState, name: &str, args: &Value) -> Result<String, String> {
    match name {
        "list_sessions" => Ok(
            serde_json::to_string_pretty(&context::session_records(app, 8))
                .unwrap_or_else(|_| "[]".into()),
        ),
        "read_session" => {
            let id = resolve_session(app, args)?;
            let lines = integer_arg(args, "lines")
                .unwrap_or(120)
                .clamp(1, MAX_READ_LINES as u64) as usize;
            let text = app
                .session_text(&id, lines)
                .ok_or("The session is no longer available.")?;
            let view = app.session_view(&id);
            let header = view
                .map(|view| {
                    format!(
                        "[{} — {} — {}{}]",
                        view.summary.title,
                        view.summary.cwd,
                        view.summary.agent.as_deref().unwrap_or("shell"),
                        view.summary
                            .agent_activity
                            .as_deref()
                            .map(|activity| format!(" {activity}"))
                            .unwrap_or_default()
                    )
                })
                .unwrap_or_default();
            Ok(if text.trim().is_empty() {
                format!("{header}\n(the session has not printed anything yet)")
            } else {
                format!("{header}\n{text}")
            })
        }
        "send_input" => {
            let id = resolve_session(app, args)?;
            let text = args
                .get("text")
                .and_then(Value::as_str)
                .ok_or("text is required.")?
                .replace("\r\n", "\n");
            let submit = args.get("submit").and_then(Value::as_bool).unwrap_or(true);
            type_text(app, &id, &text, submit).await?;
            Ok(format!(
                "Sent {} characters to \"{}\"{}.",
                text.chars().count(),
                session_label(app, &id),
                if submit { " and pressed Enter" } else { "" }
            ))
        }
        "send_keys" => {
            let id = resolve_session(app, args)?;
            let keys: Vec<String> = args
                .get("keys")
                .and_then(Value::as_array)
                .map(|keys| {
                    keys.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default();
            if keys.is_empty() {
                return Err("keys must be a non-empty array.".into());
            }
            for key in &keys {
                let sequence =
                    key_sequence(key).ok_or_else(|| format!("Unknown key \"{key}\"."))?;
                app.dispatch(&id, COMMAND_INPUT, &sequence, 0, 0)?;
                tokio::time::sleep(Duration::from_millis(60)).await;
            }
            Ok(format!(
                "Pressed {} in \"{}\".",
                keys.join(", "),
                session_label(app, &id)
            ))
        }
        "answer_prompt" => {
            let id = resolve_session(app, args)?;
            let context = app
                .input_context(&id)
                .ok_or("The session is no longer available.")?;
            let prompt = context
                .get("prompt")
                .filter(|prompt| !prompt.is_null())
                .ok_or("That tab is not waiting on a question right now.")?;
            let body = if args.get("cancel").and_then(Value::as_bool).unwrap_or(false) {
                json!({ "promptId": prompt["id"], "action": "cancel" })
            } else {
                let wanted = string_arg(args, "option")
                    .ok_or("option is required unless cancel is true.")?;
                let lower = wanted.to_lowercase();
                let option = prompt
                    .get("options")
                    .and_then(Value::as_array)
                    .and_then(|options| {
                        options.iter().find(|option| {
                            option
                                .get("key")
                                .and_then(Value::as_str)
                                .map(str::to_lowercase)
                                == Some(lower.clone())
                                || option
                                    .get("id")
                                    .and_then(Value::as_str)
                                    .map(str::to_lowercase)
                                    == Some(lower.clone())
                                || option
                                    .get("label")
                                    .and_then(Value::as_str)
                                    .map(str::to_lowercase)
                                    == Some(lower.clone())
                                || option
                                    .get("label")
                                    .and_then(Value::as_str)
                                    .is_some_and(|label| label.to_lowercase().starts_with(&lower))
                        })
                    })
                    .ok_or_else(|| {
                        format!(
                            "No option matches \"{wanted}\"; the prompt offers: {}",
                            prompt["options"]
                        )
                    })?;
                json!({ "promptId": prompt["id"], "action": "select", "optionId": option["id"] })
            };
            let data = prompt::prompt_response_bytes(&context, &body)?;
            app.dispatch(&id, COMMAND_INPUT, &data, 0, 0)?;
            Ok(format!(
                "Answered the prompt in \"{}\".",
                session_label(app, &id)
            ))
        }
        "wait_for_output" => {
            let id = resolve_session(app, args)?;
            let timeout = Duration::from_secs(
                integer_arg(args, "timeoutSeconds")
                    .unwrap_or(15)
                    .clamp(1, MAX_WAIT_SECONDS),
            );
            let lines = integer_arg(args, "lines")
                .unwrap_or(60)
                .clamp(1, MAX_READ_LINES as u64) as usize;
            Ok(wait_for_output(app, &id, timeout, lines).await)
        }
        "create_session" => {
            let body = json!({
                "title": string_arg(args, "title"),
                "cwd": string_arg(args, "cwd"),
                "profileId": string_arg(args, "profileId")
            });
            let session = launch_terminal(app, body).await?;
            Ok(serde_json::to_string_pretty(&json!({
                "id": session.id,
                "title": session.title,
                "cwd": session.cwd,
                "shell": session.shell
            }))
            .unwrap_or_default())
        }
        "open_session" => open_session(app, args).await,
        "find_project" => {
            let query = string_arg(args, "query").ok_or("query is required.")?;
            let matches = find_folders(app, &query).await;
            Ok(serde_json::to_string_pretty(&json!({ "matches": matches })).unwrap_or_default())
        }
        "focus_session" => {
            let id = resolve_session(app, args)?;
            app.send_command(BridgeCommand::Focus { id: id.clone() });
            Ok(format!("\"{}\" is in front.", session_label(app, &id)))
        }
        "list_chats" => {
            let project = string_arg(args, "project");
            let query = string_arg(args, "query");
            let limit = integer_arg(args, "limit").unwrap_or(20).clamp(1, 100) as usize;
            let listed = tokio::task::spawn_blocking(move || {
                chats::list(project.as_deref(), query.as_deref(), limit)
            })
            .await
            .map_err(|error| error.to_string())?;
            Ok(serde_json::to_string_pretty(&listed).unwrap_or_default())
        }
        "read_chat" => {
            let session = match string_arg(args, "sessionId") {
                Some(_) => Some(resolve_session(app, args)?),
                None => None,
            };
            let chat = string_arg(args, "chatId");
            let messages = integer_arg(args, "messages");
            let app = app.clone();
            tokio::task::spawn_blocking(move || {
                chats::read(&app, session.as_deref(), chat.as_deref(), messages)
            })
            .await
            .map_err(|error| error.to_string())?
        }
        "close_session" => {
            let id = resolve_session(app, args)?;
            let label = session_label(app, &id);
            app.dispatch(&id, COMMAND_KILL, "", 0, 0)?;
            Ok(format!("Closed \"{label}\"."))
        }
        "rename_session" => {
            let id = resolve_session(app, args)?;
            let title = string_arg(args, "title").ok_or("title is required.")?;
            app.rename(&id, title.clone())
                .ok_or("The session is no longer available.")?;
            // bro's own sidebar keeps the name only when told directly.
            app.send_command(BridgeCommand::Rename {
                id,
                title: title.clone(),
            });
            Ok(format!("Renamed the tab to \"{title}\"."))
        }
        "list_projects" => Ok(serde_json::to_string_pretty(&json!({
            "projects": app.projects(),
            "recent": app.recent_projects()
        }))
        .unwrap_or_default()),
        "notify_user" => {
            let title = string_arg(args, "title").ok_or("title is required.")?;
            app.notify(
                TerminalNotification {
                    id: String::new(),
                    at: String::new(),
                    // The clients' origin union is api | bell | osc.
                    origin: "api".into(),
                    session_id: None,
                    session_title: None,
                    title: Some(title),
                    body: string_arg(args, "body"),
                    sound: Some("done".into()),
                },
                true,
            );
            Ok("Notification sent.".into())
        }
        other => Err(format!("Unknown tool \"{other}\".")),
    }
}

/// Types `text` into a tab the way a person pasting would: one bracketed
/// paste when the program supports it, paced, then Enter once the paste has
/// settled (an Enter inside an agent's paste window becomes a newline).
async fn type_text(app: &AppState, id: &str, text: &str, submit: bool) -> Result<(), String> {
    let view = app
        .session_view(id)
        .ok_or("The session is no longer available.")?;
    // A confident agent detection implies paste support even when this
    // mirror never saw the mode switch (host restarts leave the screen model
    // empty).
    let paste = view.bracketed_paste || view.summary.agent.is_some();
    let payload = compose_payload(text, paste);
    if !payload.is_empty() {
        write_paced(app, id, &payload).await?;
    }
    if submit {
        tokio::time::sleep(submit_settle(payload.len())).await;
        app.dispatch(id, COMMAND_INPUT, "\r", 0, 0)?;
    }
    Ok(())
}

async fn find_folders(app: &AppState, query: &str) -> Vec<folders::FolderMatch> {
    let app = app.clone();
    let query = query.to_string();
    tokio::task::spawn_blocking(move || folders::find(&app, &query))
        .await
        .unwrap_or_default()
}

/// Waits until a freshly started tab can take a message: its agent was
/// detected and sits idle at the prompt, or (a shell) its output settled.
/// Returns the question the tab is blocked on, if it is.
async fn wait_until_ready(app: &AppState, id: &str) -> Result<Option<String>, String> {
    let started = Instant::now();
    let mut last_seq = 0;
    let mut last_change = Instant::now();
    while started.elapsed() < READY_TIMEOUT {
        tokio::time::sleep(Duration::from_millis(250)).await;
        let view = app
            .session_view(id)
            .ok_or("The session closed while starting.")?;
        let seq = app.session_seq(id).unwrap_or(0);
        if seq != last_seq {
            last_seq = seq;
            last_change = Instant::now();
        }
        let quiet = last_seq > 0 && last_change.elapsed() >= Duration::from_millis(900);
        match view.summary.agent_activity.as_deref() {
            Some("awaiting") => {
                let question = view
                    .prompt
                    .as_ref()
                    .and_then(|prompt| prompt.get("title"))
                    .and_then(Value::as_str)
                    .unwrap_or("a question")
                    .to_string();
                return Ok(Some(question));
            }
            Some("working") => {}
            _ if view.summary.agent.is_some() && quiet => return Ok(None),
            // Agents take a moment to be detected; a plain shell never is.
            _ if quiet && started.elapsed() >= Duration::from_secs(6) => return Ok(None),
            _ => {}
        }
    }
    Ok(None)
}

async fn open_session(app: &AppState, args: &Value) -> Result<String, String> {
    let query = string_arg(args, "project").ok_or("project is required.")?;
    let matches = find_folders(app, &query).await;
    let best = matches.first().ok_or_else(|| {
        format!("No folder on this machine matches \"{query}\". Ask the user for the path.")
    })?;
    if let Some(second) = matches.get(1)
        && second.score == best.score
        && best.score < 1000
    {
        return Err(format!(
            "\"{query}\" is ambiguous; ask which one: {}",
            matches
                .iter()
                .take_while(|found| found.score == best.score)
                .map(|found| found.path.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    let agent = string_arg(args, "agent").unwrap_or_else(|| "claude".into());
    let name = string_arg(args, "name").unwrap_or_else(|| {
        let titles: Vec<String> = app.summaries().into_iter().map(|s| s.title).collect();
        cast::next_name(titles.iter().map(String::as_str))
    });
    let title = cast::title(&name, Some(&best.name));
    let session = launch_terminal(
        app,
        json!({ "title": title, "cwd": best.path, "profileId": agent }),
    )
    .await?;
    let id = session.id.clone();
    if args.get("focus").and_then(Value::as_bool).unwrap_or(false) {
        app.send_command(BridgeCommand::Focus { id: id.clone() });
    }
    let mut outcome = json!({
        "id": id,
        "name": name,
        "title": title,
        "cwd": best.path,
        "agent": agent,
    });
    // Same-named folders elsewhere (the most recently used one was picked).
    let others: Vec<&str> = matches
        .iter()
        .skip(1)
        .filter(|found| found.score == best.score)
        .map(|found| found.path.as_str())
        .collect();
    if !others.is_empty() {
        outcome["alsoFound"] = json!(others);
    }
    if let Some(task) = string_arg(args, "task") {
        match wait_until_ready(app, &id).await? {
            Some(question) => {
                outcome["taskSent"] = json!(false);
                outcome["note"] = json!(format!(
                    "The session is waiting on \"{question}\"; answer it with answer_prompt, then send the task with send_input."
                ));
            }
            None => {
                type_text(app, &id, &task, true).await?;
                outcome["taskSent"] = json!(true);
            }
        }
    }
    Ok(serde_json::to_string_pretty(&outcome).unwrap_or_default())
}
