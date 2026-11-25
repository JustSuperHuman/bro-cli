//! The voice's tools: bro's orchestrator tool set, reworded for speech, plus voice-native ones.
//!
//! Three kinds of effect:
//! - [`Effect::Read`] looks; never changes anything.
//! - [`Effect::Act`] does what the user just asked (message, answer, open, focus, schedule…). Only
//!   responses to the user's own speech can call tools at all: bro's proactive announcements are
//!   created with `tool_choice: "none"`.
//! - [`Effect::Confirm`] is hard to undo (closing sessions, clearing context, killing a process).
//!   The first call parks the action and returns a question; it runs only after [`Gate`] sees the
//!   user's own transcribed "yes" after that point, or a tap on the confirmation card. The model
//!   can't produce either.

use super::{Announce, Call, brain, feed};
use crate::orchestrator::MessageContext;
use crate::orchestrator::tools as bro;
use crate::state::{AppState, COMMAND_INPUT};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Longest tool result the model reads (characters).
const RESULT_CHARS: usize = 12_000;
/// A parked action the user didn't confirm in time is dropped.
pub(crate) const CONFIRM_TTL: Duration = Duration::from_secs(120);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Effect {
    Read,
    Act,
    Confirm,
}

/// bro tools the voice uses as they are, with the wording a voice model needs.
const BRO_TOOLS: &[(&str, Effect, &str)] = &[
    ("list_sessions", Effect::Read, "Fresh details on every session: agent, project, branch, what it's doing, any question it waits on, last screen lines. The [bro] updates and workspace_brief usually suffice; use this for detail."),
    ("read_session", Effect::Read, "Read a session's screen and recent scrollback (newest last) to see exactly what it printed. sessionId may be its name (\"Toast\")."),
    ("read_chat", Effect::Read, "Read an agent's conversation (user and agent messages, tool calls as one-liners): what it was asked, what it did and said. Pass sessionId (name or id) for a live session, or chatId from list_chats. Best way to answer \"what did Toast do?\"."),
    ("list_chats", Effect::Read, "Past and current Claude Code / Codex chats saved on disk, newest first; filter by project or title text."),
    ("list_projects", Effect::Read, "The projects sessions are grouped under, and recently used folders."),
    ("find_project", Effect::Read, "Resolve a spoken project name to folders on this computer, best match first."),
    ("get_usage", Effect::Read, "Subscription usage for every Claude and Codex login: 5-hour and weekly meters, resets, which are exhausted, and bro's pick for big and small tasks."),
    ("schedule_list", Effect::Read, "Every scheduled prompt for bro's orchestrator: when it next runs."),
    ("coordination_snapshot", Effect::Read, "Durable tasks bro's orchestrator is tracking: their state, progress and next action."),
    ("send_input", Effect::Act, "Type raw text into a session, e.g. a shell command; submit presses Enter (default true). To talk to a coding agent use message_agent instead."),
    ("send_keys", Effect::Act, "Press named keys in a session for menus and TUIs: enter, tab, shift+tab, esc, space, backspace, up, down, left, right, home, end, pageup, pagedown, ctrl+c, ctrl+d, ctrl+l, ctrl+r, ctrl+u, ctrl+z, or one character. ctrl+c, ctrl+d and ctrl+z need the user's yes."),
    ("answer_prompt", Effect::Act, "Answer the question an agent is waiting on with the option the user chose (its number/key or label), or cancel it. Only with the user's own choice."),
    ("open_session", Effect::Act, "Start a new agent session in a project, optionally with its first task in the user's words. The project may be spoken loosely (\"justgains\", \"bro cli\"). It gets the next free squad name (Mal, Pinky, Toast…) so the user can refer to it. Say the name you got."),
    ("create_session", Effect::Act, "Open a plain terminal (shell or a profile like pwsh, claude, codex), optionally in a folder."),
    ("focus_session", Effect::Act, "Bring a session to the front in bro's window on the computer."),
    ("show_project", Effect::Act, "Show one project's sessions side by side in bro's window, or every session when project is omitted."),
    ("rename_session", Effect::Act, "Rename a session."),
    ("schedule_add", Effect::Act, "Schedule a prompt for bro's orchestrator to act on later, once or repeating (in \"10m\", at \"17:30\", every \"1h\", daily \"09:00\")."),
    ("schedule_remove", Effect::Act, "Remove a scheduled prompt by id or name."),
    ("notify_user", Effect::Act, "Send a notification (sound and toast) to the user's devices, e.g. a reminder they asked for."),
    ("close_session", Effect::Confirm, "Close a session and end its process. Call it as soon as the user asks: bro holds it until they confirm."),
    ("close_all_sessions", Effect::Confirm, "Close every session, or one project's, keeping any listed in except; busy ones only with includeBusy. Call it as soon as the user asks: bro holds it until they confirm."),
    ("clear_context", Effect::Confirm, "Start an agent on a fresh conversation (it forgets everything so far); follow with a self-contained task. Call it as soon as the user asks: bro holds it until they confirm."),
];

/// Arguments the voice never needs (task contracts, watchdog plumbing).
const DROPPED_PROPERTIES: &[(&str, &[&str])] = &[("open_session", &["taskId", "taskContract", "origin", "forWatchdog", "readOnly"])];

fn custom_tools() -> Vec<(Value, Effect)> {
    let tool = |name: &str, description: &str, properties: Value, required: &[&str]| bro::tool(name, description, properties, required);
    vec![
        (tool("workspace_brief", "The whole workspace in a few lines: every session, what it's doing, who waits on the user, plus the latest updates. Use when the user asks what's going on.", json!({}), &[]), Effect::Read),
        (tool("recent_events", "What happened lately: agents that finished or asked something, sessions opened or closed, Hugh's answers, newest last.", json!({"limit":{"type":"integer","description":"Default 12, max 40."}}), &[]), Effect::Read),
        (tool("message_agent", "Say something to a coding agent in a session, as the user: a task, a question, feedback, a correction. Works while it is busy (it reads it next). Use the user's own words. bro tells you when the agent answers.", json!({"sessionId":{"type":"string","description":"Session name (\"Toast\") or id."},"message":{"type":"string","description":"The user's words for the agent."}}), &["sessionId","message"]), Effect::Act),
        (tool("stop_agent", "Interrupt what an agent is doing right now (like pressing Esc). It keeps its conversation; send it a new message to continue.", json!({"sessionId":{"type":"string"}}), &["sessionId"]), Effect::Act),
        (tool("watch_session", "Tell the user when this session finishes its current work or needs them, even if announcements are limited.", json!({"sessionId":{"type":"string"}}), &["sessionId"]), Effect::Act),
        (tool("ask_orchestrator", "Hand a bigger job to Hugh, bro's orchestrator agent, in the user's words: multi-step work, coordinating several agents, anything that needs planning. Hugh has every bro tool. Returns at once; you'll hear Hugh's answer when it comes.", json!({"request":{"type":"string","description":"The user's request, faithfully."}}), &["request"]), Effect::Act),
        (tool("set_announcements", "How much bro says unprompted: all (every agent that finishes or needs input), asked (only agents the user talked to, plus questions), off (nothing; updates still reach you).", json!({"mode":{"type":"string","enum":["all","asked","off"]}}), &["mode"]), Effect::Act),
        (tool("confirm_action", "Carry out an action that needed the user's yes, after they said yes. bro checks their actual words.", json!({"actionId":{"type":"string"}}), &["actionId"]), Effect::Act),
        (tool("cancel_action", "Drop an action that was waiting for the user's yes, when they say no.", json!({"actionId":{"type":"string"}}), &["actionId"]), Effect::Act),
        (tool("end_call", "Hang up after you say goodbye, when the user is done (\"that's all\", \"bye\").", json!({}), &[]), Effect::Act),
    ]
}

/// Name, definition and effect of every voice tool.
pub(crate) fn catalog() -> Vec<(String, Value, Effect)> {
    let mut definitions = bro::definitions();
    let mut out = Vec::new();
    for (name, effect, description) in BRO_TOOLS {
        let Some(index) = definitions.iter().position(|definition| definition["name"] == *name) else { continue };
        let mut definition = definitions.swap_remove(index);
        definition["description"] = json!(description);
        if let Some((_, dropped)) = DROPPED_PROPERTIES.iter().find(|(tool, _)| tool == name)
            && let Some(properties) = definition["inputSchema"]["properties"].as_object_mut()
        {
            for property in *dropped {
                properties.remove(*property);
            }
        }
        out.push(((*name).to_owned(), definition, *effect));
    }
    for (definition, effect) in custom_tools() {
        let name = definition["name"].as_str().unwrap_or_default().to_owned();
        out.push((name, definition, effect));
    }
    out
}

/// The catalog in the Realtime API's function-tool shape.
pub(crate) fn realtime_tools() -> Vec<Value> {
    catalog()
        .into_iter()
        .map(|(name, definition, _)| {
            json!({
                "type": "function",
                "name": name,
                "description": definition["description"],
                "parameters": definition["inputSchema"],
            })
        })
        .collect()
}

pub(crate) fn effect_of(name: &str, args: &Value) -> Option<Effect> {
    let effect = catalog().into_iter().find(|(tool, _, _)| tool == name).map(|(_, _, effect)| effect)?;
    if name == "send_keys" && interrupting_keys(args) {
        return Some(Effect::Confirm);
    }
    Some(effect)
}

fn interrupting_keys(args: &Value) -> bool {
    args.get("keys").and_then(Value::as_array).is_some_and(|keys| {
        keys.iter().filter_map(Value::as_str).any(|key| matches!(key.trim().to_ascii_lowercase().as_str(), "ctrl+c" | "ctrl+d" | "ctrl+z"))
    })
}

fn arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str).map(str::trim).filter(|value| !value.is_empty())
}

/// What a tool call does, in words for activity rows, confirmation cards and the model.
pub(crate) fn describe(app: &AppState, name: &str, args: &Value) -> String {
    let target = || {
        arg(args, "sessionId")
            .and_then(|id| bro::resolve_session(app, &json!({"sessionId": id})).ok())
            .and_then(|id| app.summary(&id))
    };
    let session = || {
        target()
            .map(|summary| crate::orchestrator::cast::name_of(&summary.title).to_owned())
            .unwrap_or_else(|| arg(args, "sessionId").unwrap_or("that session").to_owned())
    };
    // Hard-to-undo actions say when they would cut work short.
    let busy = || {
        if target().is_some_and(|summary| summary.agent_activity.as_deref() == Some("working")) { " (it's working right now)" } else { "" }
    };
    match name {
        "close_session" => format!("Close {}{}", session(), busy()),
        "clear_context" => format!("Clear {}'s conversation", session()),
        "send_keys" => format!("Press {} in {}{}", args["keys"].as_array().map(|keys| keys.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")).unwrap_or_default(), session(), busy()),
        "close_all_sessions" => match arg(args, "project") {
            Some(project) => format!("Close every session in {project}{}", if args["includeBusy"] == true { ", busy ones too" } else { "" }),
            None => format!("Close every session{}", if args["includeBusy"] == true { ", busy ones too" } else { "" }),
        },
        "message_agent" => format!("Told {}: “{}”", session(), brain::clip(arg(args, "message").unwrap_or(""), 90)),
        "stop_agent" => format!("Stopped {}", session()),
        "watch_session" => format!("Watching {}", session()),
        "ask_orchestrator" => format!("Asked Hugh: “{}”", brain::clip(arg(args, "request").unwrap_or(""), 90)),
        "workspace_brief" => "Looked over the workspace".into(),
        "recent_events" => "Checked the latest updates".into(),
        "set_announcements" => format!("Announcements: {}", arg(args, "mode").unwrap_or("all")),
        "read_session" => format!("Read {}'s screen", session()),
        "read_chat" => match arg(args, "chatId") {
            Some(_) => "Read a past chat".into(),
            None => format!("Read {}'s chat", session()),
        },
        "answer_prompt" => match (arg(args, "option"), args["cancel"] == true) {
            (_, true) => format!("Dismissed {}'s question", session()),
            (Some(option), _) => format!("Answered {}: {option}", session()),
            _ => format!("Answered {}", session()),
        },
        other => bro::summarize(app, other, args),
    }
}

/// Runs one allowed tool. `Err` text goes to the model as the reason.
pub(crate) async fn execute(app: &AppState, call: &Call, name: &str, args: &Value) -> Result<String, String> {
    match name {
        "workspace_brief" => {
            let mut text = brain::digest(app);
            if let Some(line) = brain::focus_line(app, call.focused.lock().as_deref()) {
                text.push_str(&format!("\n{line}\n"));
            }
            let events = feed::recent(app, 8);
            if !events.is_empty() {
                text.push_str("\n## Latest updates\n");
                for event in events {
                    text.push_str(&format!("- {event}\n"));
                }
            }
            Ok(text)
        }
        "recent_events" => {
            let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(12).clamp(1, 40) as usize;
            let events = feed::recent(app, limit);
            Ok(if events.is_empty() { "Nothing new since bro started listening.".into() } else { events.join("\n") })
        }
        "message_agent" => message_agent(app, call, args).await,
        "stop_agent" => {
            let id = bro::resolve_session(app, args)?;
            let summary = app.summary(&id).ok_or("That session closed.")?;
            if summary.agent.is_none() {
                return Err(format!("{} isn't running an agent; to stop a shell command press ctrl+c with send_keys.", summary.title));
            }
            app.dispatch(&id, COMMAND_INPUT, "\x1b", 0, 0)?;
            call.watch(&id);
            Ok(format!("Interrupted {}. It keeps its conversation.", summary.title))
        }
        "watch_session" => {
            let id = bro::resolve_session(app, args)?;
            call.watch(&id);
            let title = app.summary(&id).map(|summary| summary.title).unwrap_or(id);
            Ok(format!("Watching {title}; you'll hear when it finishes or needs the user."))
        }
        "ask_orchestrator" => {
            let request = arg(args, "request").ok_or("request is required.")?.to_owned();
            let number = call.asks.fetch_add(1, std::sync::atomic::Ordering::AcqRel) + 1;
            let context = MessageContext {
                voice: true,
                focused_session: call.focused.lock().clone(),
                tag: Some(format!("voice:{}:{number}", call.id)),
            };
            crate::orchestrator::send_message_with(app, request, context).await.map_err(|(_, message)| message)?;
            Ok("Hugh has it. You'll hear his answer when it's ready.".into())
        }
        "set_announcements" => {
            let mode = arg(args, "mode").and_then(Announce::parse).ok_or("mode is all, asked or off.")?;
            *call.announce.lock() = mode;
            super::publish(app, &call.id, json!({"event":"announce","announce":mode.as_str()}));
            Ok(format!("Announcements: {}.", mode.as_str()))
        }
        "read_session" => {
            let mut args = args.clone();
            if args.get("lines").is_none() {
                args["lines"] = json!(60);
            }
            bro::execute(app, name, &args).await
        }
        "read_chat" => {
            let mut args = args.clone();
            if args.get("messages").is_none() {
                args["messages"] = json!(16);
            }
            bro::execute(app, name, &args).await
        }
        _ => bro::execute(app, name, args).await,
    }
}

async fn message_agent(app: &AppState, call: &Call, args: &Value) -> Result<String, String> {
    let id = bro::resolve_session(app, args)?;
    let message = arg(args, "message").ok_or("message is required.")?.replace("\r\n", "\n");
    let summary = app.summary(&id).ok_or("That session closed.")?;
    if summary.kind.as_deref() == Some("orchestrator") {
        return Err("That's Hugh, bro's orchestrator: use ask_orchestrator for him.".into());
    }
    if summary.agent.is_none() {
        return Err(format!("{} is a plain terminal, not an agent. Use send_input to type a command there.", summary.title));
    }
    if summary.status == "exited" {
        return Err(format!("{} has exited.", summary.title));
    }
    if summary.agent_activity.as_deref() == Some("awaiting") {
        let question = app.session_view(&id).and_then(|view| brain::prompt_text(view.prompt.as_ref()));
        return Err(format!(
            "{} is waiting on a question{}. Answer it with answer_prompt first, or cancel it.",
            summary.title,
            question.map(|q| format!(": {q}")).unwrap_or_default()
        ));
    }
    let busy = summary.agent_activity.as_deref() == Some("working");
    bro::type_text(app, &id, &message, true).await?;
    call.watch(&id);
    Ok(if busy {
        format!("Sent to {}. It's mid-turn and will read it next; you'll hear when it answers.", summary.title)
    } else {
        format!("Sent to {}. You'll hear when it answers.", summary.title)
    })
}

/// A tool's result as the model reads it.
pub(crate) fn output(result: Result<String, String>) -> Value {
    match result {
        Ok(text) => json!({"ok": true, "result": brain::clip_keep_lines(&text, RESULT_CHARS)}),
        Err(error) => json!({"ok": false, "error": brain::clip(&error, 600)}),
    }
}

// ----- the confirmation gate -------------------------------------------------------------------

/// An action waiting for the user's yes.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Parked {
    pub name: String,
    pub args: Value,
    pub summary: String,
    pub at: Instant,
}

/// Parked actions and the user's own words, which alone can release them.
#[derive(Default)]
pub(crate) struct Gate {
    parked: HashMap<String, Parked>,
    next: u32,
    /// Completed transcriptions of the user's speech, oldest first.
    heard: Vec<(Instant, String)>,
}

#[derive(Debug, PartialEq)]
pub(crate) enum Verdict {
    Go(Parked),
    NotYet(&'static str),
    Declined(Parked),
    Unknown,
}

impl Gate {
    pub fn park(&mut self, name: &str, args: &Value, summary: String, now: Instant) -> String {
        self.parked.retain(|_, parked| now.duration_since(parked.at) < CONFIRM_TTL);
        self.next += 1;
        let id = format!("a{}", self.next);
        self.parked.insert(id.clone(), Parked { name: name.into(), args: args.clone(), summary, at: now });
        id
    }

    pub fn heard(&mut self, text: &str, now: Instant) {
        let text = text.trim();
        if !text.is_empty() {
            self.heard.push((now, text.to_owned()));
            if self.heard.len() > 64 {
                self.heard.remove(0);
            }
        }
    }

    /// The model asks to run a parked action. The user's latest words must be a clear yes, said
    /// after it was parked, or (when the model asked before calling the tool) just before, by a
    /// user whose own recent words asked for this kind of action.
    pub fn confirm(&mut self, id: &str, now: Instant) -> Verdict {
        let Some(parked) = self.parked.get(id).cloned() else { return Verdict::Unknown };
        if now.duration_since(parked.at) >= CONFIRM_TTL {
            self.parked.remove(id);
            return Verdict::Unknown;
        }
        let Some((last_at, last)) = self.heard.last().cloned() else {
            return Verdict::NotYet("The user hasn't answered yet. Ask them, then wait.");
        };
        let answered_after = last_at > parked.at;
        let answered_just_before = !answered_after
            && parked.at.duration_since(last_at) <= ANSWER_BEFORE
            && self.asked_for(&parked.name, parked.at);
        if !answered_after && !answered_just_before {
            return Verdict::NotYet("The user hasn't answered yet. Ask them, then wait.");
        }
        match consent(&last) {
            Some(true) => {
                self.parked.remove(id);
                Verdict::Go(parked)
            }
            Some(false) => {
                self.parked.remove(id);
                Verdict::Declined(parked)
            }
            None => Verdict::NotYet("The user's last words weren't a clear yes. Ask again plainly."),
        }
    }

    /// A tap on the card, or the model dropping it.
    pub fn take(&mut self, id: &str) -> Option<Parked> {
        self.parked.remove(id)
    }

    /// The user's own words shortly before `at` asked for this kind of action.
    fn asked_for(&self, name: &str, at: Instant) -> bool {
        let words: &[&str] = match name {
            "close_session" | "close_all_sessions" => &["close", "kill", "shut", "end", "quit", "get rid", "clean up", "remove"],
            "clear_context" => &["clear", "fresh", "reset", "forget", "new conversation", "start over", "wipe"],
            "send_keys" => &["stop", "kill", "cancel", "interrupt", "control", "ctrl", "exit", "quit", "abort", "suspend"],
            _ => &[],
        };
        self.heard
            .iter()
            .filter(|(heard, _)| *heard <= at && at.duration_since(*heard) <= INTENT_WINDOW)
            .any(|(_, text)| {
                let text = spaced_words(text);
                words.iter().any(|word| text.contains(&format!(" {word} ")))
            })
    }
}

/// A yes said this long before the action was parked still counts (the model asked first).
const ANSWER_BEFORE: Duration = Duration::from_secs(20);
/// How far back the user's own request for the action may be.
const INTENT_WINDOW: Duration = Duration::from_secs(90);

/// " lowercase words joined by single spaces ", so phrases match on word boundaries.
fn spaced_words(text: &str) -> String {
    let lower = text.to_lowercase().replace(['’', '‘'], "'");
    let words: Vec<&str> = lower.split(|c: char| !c.is_alphanumeric() && c != '\'').filter(|word| !word.is_empty()).collect();
    format!(" {} ", words.join(" "))
}

/// `Some(true)` for a clear yes, `Some(false)` for a no, `None` otherwise.
pub(crate) fn consent(text: &str) -> Option<bool> {
    let joined = spaced_words(text);
    const NO: &[&str] = &[" no ", " nope ", " nah ", " don't ", " dont ", " do not ", " cancel ", " wait ", " stop ", " hold on ", " never mind ", " nevermind ", " not yet ", " keep it ", " leave it "];
    const YES: &[&str] = &[" yes ", " yeah ", " yep ", " yup ", " sure ", " ok ", " okay ", " do it ", " go ahead ", " confirm ", " confirmed ", " please do ", " affirmative ", " correct ", " absolutely ", " definitely ", " go for it ", " close it ", " kill it ", " clear it ", " that's right ", " right "];
    if NO.iter().any(|no| joined.contains(no)) {
        return Some(false);
    }
    if YES.iter().any(|yes| joined.contains(yes)) {
        return Some(true);
    }
    None
}
