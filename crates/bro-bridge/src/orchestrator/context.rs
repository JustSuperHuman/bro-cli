//! What the orchestrator knows about the workspace before it answers: every
//! terminal tab, where it is, what is running in it and what it is doing,
//! rendered into every message (and on demand through the
//! `list_sessions` tool).

use super::{MessageContext, OrchestratorConfig};
use crate::agents;
use crate::session::SessionView;
use crate::state::AppState;
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// Screen lines quoted per session in the snapshot.
const TAIL_LINES: usize = 12;
/// Fewer lines each once the workspace is crowded, so the prompt stays bounded.
const CROWDED_TAIL_LINES: usize = 4;
const CROWDED_AT: usize = 12;
const TAIL_WIDTH: usize = 160;

pub fn idle_seconds(updated_at: &str) -> Option<i64> {
    let at = DateTime::parse_from_rfc3339(updated_at).ok()?;
    Some((Utc::now() - at.with_timezone(&Utc)).num_seconds().max(0))
}

fn describe_idle(seconds: Option<i64>) -> String {
    match seconds {
        None => "unknown".into(),
        Some(s) if s < 5 => "just now".into(),
        Some(s) if s < 60 => format!("{s}s ago"),
        Some(s) if s < 3600 => format!("{}m ago", s / 60),
        Some(s) => format!("{}h ago", s / 3600),
    }
}

fn read_first_line(path: &Path) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    content.lines().next().map(|line| line.trim().to_string())
}

/// Branch name (or a short detached hash) for the repository containing
/// `cwd`, read straight from `.git/HEAD` so it costs one stat per tab.
pub fn git_branch(cwd: &str) -> Option<String> {
    let mut directory = PathBuf::from(cwd.trim());
    if !directory.is_dir() {
        return None;
    }
    for _ in 0..40 {
        let marker = directory.join(".git");
        let git_dir = if marker.is_dir() {
            Some(marker)
        } else if marker.is_file() {
            read_first_line(&marker)
                .and_then(|line| {
                    line.strip_prefix("gitdir:")
                        .map(|rest| rest.trim().to_string())
                })
                .map(|target| {
                    let target = PathBuf::from(target);
                    if target.is_absolute() {
                        target
                    } else {
                        directory.join(target)
                    }
                })
        } else {
            None
        };
        if let Some(git_dir) = git_dir {
            let head = read_first_line(&git_dir.join("HEAD"))?;
            return Some(match head.strip_prefix("ref:") {
                Some(reference) => reference
                    .trim()
                    .trim_start_matches("refs/heads/")
                    .to_string(),
                None => head.chars().take(8).collect(),
            });
        }
        if !directory.pop() {
            break;
        }
    }
    None
}

/// The last `lines` meaningful screen rows, right-trimmed and clipped.
pub fn screen_tail(text: &str, lines: usize, width: usize) -> Vec<String> {
    let mut rows: Vec<String> = text
        .lines()
        .map(|line| line.trim_end().chars().take(width).collect::<String>())
        .collect();
    while rows.last().is_some_and(|row| row.trim().is_empty()) {
        rows.pop();
    }
    let start = rows.len().saturating_sub(lines);
    rows.drain(..start);
    rows
}

fn shell_name(shell: &str) -> String {
    let trimmed = shell.trim().trim_matches('"');
    let first = trimmed.split_whitespace().next().unwrap_or(trimmed);
    first
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(first)
        .trim_end_matches(".exe")
        .to_string()
}

fn project_name(app: &AppState, project_id: Option<&str>) -> Option<String> {
    let id = project_id?;
    app.projects()
        .into_iter()
        .find(|project| project.id == id)
        .map(|project| project.name)
}

fn agent_state(view: &SessionView) -> (Option<String>, Option<String>, String) {
    let agent = view
        .summary
        .agent
        .as_deref()
        .and_then(agents::Agent::parse)
        .map(|agent| agent.label().to_string());
    let activity = view.summary.agent_activity.clone();
    let note = match (agent.as_deref(), activity.as_deref()) {
        (None, _) => "shell".to_string(),
        (Some(label), Some("working")) => format!("{label}, working on a turn"),
        (Some(label), Some("awaiting")) => {
            let title = view
                .prompt
                .as_ref()
                .and_then(|prompt| prompt.get("title"))
                .and_then(Value::as_str)
                .unwrap_or("a question");
            format!("{label}, WAITING FOR AN ANSWER: {title}")
        }
        (Some(label), _) => format!("{label}, idle at its prompt"),
    };
    (agent, activity, note)
}

/// One record per tab, in creation order, for the tool and the snapshot.
pub fn session_records(app: &AppState, tail_lines: usize) -> Vec<Value> {
    let mut views = app.session_views();
    views.sort_by(|left, right| left.summary.created_at.cmp(&right.summary.created_at));
    views
        .iter()
        .map(|view| {
            let (agent, activity, note) = agent_state(view);
            let idle = idle_seconds(&view.summary.updated_at);
            let tail = screen_tail(&view.screen, tail_lines, TAIL_WIDTH);
            json!({
                "id": view.summary.id,
                "title": view.summary.title,
                "cwd": view.summary.cwd,
                "gitBranch": git_branch(&view.summary.cwd),
                "project": project_name(app, view.summary.project_id.as_deref()),
                "shell": shell_name(&view.summary.shell),
                "status": view.summary.status,
                "agent": agent,
                "agentActivity": activity,
                "summary": note,
                "prompt": view.prompt,
                "lastOutput": describe_idle(idle),
                "idleSeconds": idle,
                "screenTail": tail
            })
        })
        .collect()
}

/// The workspace block of the system prompt.
pub fn workspace_snapshot(app: &AppState) -> String {
    let count = app.session_views().len();
    let tail_lines = if count > CROWDED_AT {
        CROWDED_TAIL_LINES
    } else {
        TAIL_LINES
    };
    let records = session_records(app, tail_lines);
    let mut out = String::new();
    out.push_str(&format!(
        "## Workspace right now ({} at {})\n",
        match records.len() {
            0 => "no terminals open".to_string(),
            1 => "1 terminal".to_string(),
            n => format!("{n} terminals"),
        },
        Utc::now().format("%Y-%m-%d %H:%M UTC")
    ));
    if records.is_empty() {
        out.push_str("There are no terminal sessions. Offer to open one with create_session.\n");
        return out;
    }
    for (index, record) in records.iter().enumerate() {
        let field = |key: &str| {
            record
                .get(key)
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        };
        let mut line = format!(
            "{}. \"{}\" — id {} — {}",
            index + 1,
            field("title"),
            field("id"),
            field("cwd")
        );
        if let Some(branch) = record.get("gitBranch").and_then(Value::as_str) {
            line.push_str(&format!(" (git {branch})"));
        }
        if let Some(project) = record.get("project").and_then(Value::as_str) {
            line.push_str(&format!(" — project \"{project}\""));
        }
        line.push_str(&format!(
            " — {} — {} — last output {}",
            field("shell"),
            field("summary"),
            field("lastOutput")
        ));
        if field("status") == "exited" {
            line.push_str(" — EXITED");
        }
        out.push_str(&line);
        out.push('\n');
        if let Some(tail) = record.get("screenTail").and_then(Value::as_array)
            && !tail.is_empty()
        {
            out.push_str("   screen:\n");
            for row in tail.iter().filter_map(Value::as_str) {
                out.push_str("   │ ");
                out.push_str(row);
                out.push('\n');
            }
        }
    }
    out
}

/// The orchestrator's standing instructions (Claude Code's `--system-prompt`).
/// The live workspace travels with each message instead
/// ([`message_with_snapshot`]), so it is never stale.
pub fn system_prompt(config: &OrchestratorConfig) -> String {
    let mut prompt = String::new();
    prompt.push_str(concat!(
        "You are Hugh, the orchestrator built into bro, the user's agentic terminal workspace. You see every terminal ",
        "session bro runs on this machine: shells and coding agents (Claude Code, Codex, Pi, omp). Your job is to tell ",
        "the user what every session is doing, spot the ones that are stuck, failing or waiting on them, open new ",
        "sessions, hand them tasks, chain work between them, and answer questions about their chats, all through your ",
        "bro tools.\n\n",
        "## Voice\n",
        "- Dictated messages arrive as <dictated>...</dictated>, transcribed by speech recognition. Expect mis-hearings: ",
        "match names and projects to the closest real ones (\"toast\"/\"tost\" -> Toast, \"just gains\" -> justgains) ",
        "and act on the most plausible reading. Ask only when two readings would do different, hard-to-undo things.\n",
        "- Your reply is spoken aloud and shown as a toast, so it must be tiny: at most about 8 words, one clause, plain ",
        "text. State the outcome only. No explanations, no restating the request, no \"I\" preambles, no offers or ",
        "follow-up questions (\"Want me to...?\"), no pleasantries. Good: \"Opened Toast in justgains.\" \"Sent to Mal.\" ",
        "\"Mal is idle.\" \"Toast is running the tests.\" \"Nothing is stuck.\" \"Pinky's build failed: missing env var.\" ",
        "Go longer only when the user explicitly asks for detail or a summary, and even then keep it to two short ",
        "sentences. Ask a question back only when you truly cannot act, in five words or fewer.\n\n",
        "## Routing a dictated message: prompt for the focused pane, or instruction for you\n",
        "Each dictated message comes with a <focus> line: the session the user was looking at when they started speaking. ",
        "Decide first, then do exactly one of these:\n",
        "- PROMPT: the focused session runs a coding agent (Claude Code, Codex, Pi, omp) and the words read like something ",
        "the user would type to that agent: a task, a question about its code or work, feedback or a correction (\"add a ",
        "loading spinner\", \"why is this test failing?\", \"no, use the other endpoint\", \"looks good, commit it\"). Call ",
        "send_input right away with the focused sessionId, submit=true, and the user's own words: fix obvious ",
        "mis-transcriptions and drop filler (um, uh), but never rephrase, summarize or add to them. Use no other tool ",
        "first and don't wait for output. Reply only \"Sent to <name>.\" (add \"queued behind its current turn\" when that ",
        "agent is working; it still takes the message).\n",
        "- INSTRUCTION: the words are about bro and its sessions rather than the focused agent's work: opening, closing, ",
        "focusing or naming sessions; a different session by name (\"tell Toast...\", \"ask Pinky...\"); status or history ",
        "questions (\"what is Mal doing\", \"what did Toast change\", \"is anything stuck\"); chaining (\"when... then...\"); ",
        "or words addressed to you (\"Hugh, ...\"). Handle it with your tools as usual.\n",
        "- Explicit cues win: starting with \"Hugh\" or \"hey Hugh\" means INSTRUCTION; \"tell it\", \"type\", \"send this\" or ",
        "\"prompt\" mean PROMPT to the focused session (drop the cue words).\n",
        "- The focused session is a plain shell: INSTRUCTION, unless the user dictates a literal command (\"run git ",
        "status\" -> send_input \"git status\"). No focused session: INSTRUCTION.\n",
        "- Still unsure: PROMPT if the focused agent is idle and the words concern code or its project, otherwise ",
        "INSTRUCTION.\n",
        "Messages without a <dictated> tag were typed into your panel and are always instructions for you.\n\n",
        "## Sessions and names\n",
        "- Sessions you open get squad names: Mal, Pinky, Toast, Minty, Lilac, Sky, Bitty (titles look like ",
        "\"Toast · justgains\"). The user refers to sessions by these names; every tool's sessionId accepts the name.\n",
        "- \"Open a session in X\" means open_session with project X (a claude session unless another agent is named). ",
        "If a task is given in the same breath, pass it as task. Reply with just the name and project: \"Opened Toast in justgains.\"\n",
        "- To talk to an agent session: send_input with submit=true. Don't type into a session whose agent is mid-turn ",
        "unless the user asked you to interrupt it.\n",
        "- Chaining (\"when Mal is done, have Pinky review it\"): wait_for_output on the first session until its agent is ",
        "idle (call it again while it reports still working), read what it produced (read_chat or read_session), then ",
        "send the follow-up to the next session with the relevant context included. When the chain is done, say so in a few words.\n",
        "- Questions about what a session did or said (\"what did Toast change?\", \"why did the build fail?\"): use ",
        "read_chat first (the full conversation), read_session for the live screen. For older chats, list_chats then ",
        "read_chat with a chatId. Read, Grep and Glob work on transcript files and project files when you need more.\n",
        "- A session marked WAITING FOR AN ANSWER is blocked on a question; tell the user what it asks and offer to answer ",
        "it with answer_prompt (answer yourself only when the user said what to choose).\n\n",
        "## Safety\n",
        "- These are the user's real terminals. Read before you type. Never send destructive commands (deleting files, ",
        "resetting git state, killing processes, force pushes) unless the user explicitly asked for exactly that.\n",
        "- Ask before closing a session that looks busy. Opening, naming and focusing sessions needs no confirmation.\n",
        "- After changing something, verify with the tools rather than assuming, and report what you saw.\n\n",
        "Each user message arrives with a <workspace> block: a live snapshot of every session taken as it was sent. ",
        "Answer status questions from it directly; call read_session / read_chat for more.\n"
    ));
    prompt.push_str(&format!("\nYou are running as Claude Code model `{}`.\n", config.model));
    prompt
}

/// One line on the pane the user was looking at.
fn focus_line(app: &AppState, focused: Option<&str>) -> String {
    let Some(view) = focused.and_then(|id| app.session_view(id)) else {
        return "No session is focused (the user is not looking at a session).".into();
    };
    let (_, _, note) = agent_state(&view);
    format!(
        "The user is looking at \"{}\" (sessionId {}) in {}: {}.",
        view.summary.title, view.summary.id, view.summary.cwd, note
    )
}

/// A user message as Claude receives it: the live workspace, the focused
/// pane, then the words (tagged when dictated).
pub fn message_with_snapshot(app: &AppState, text: &str, context: &MessageContext) -> String {
    let mut out = String::from("<workspace>\n");
    let projects = app.projects();
    if !projects.is_empty() {
        out.push_str("## Projects\n");
        for project in projects.iter().take(40) {
            out.push_str(&format!("- {} — {}\n", project.name, project.cwd));
        }
        out.push('\n');
    }
    out.push_str(&workspace_snapshot(app));
    out.push_str("</workspace>\n");
    if context.voice {
        out.push_str(&format!(
            "<focus>{}</focus>\n<dictated>{text}</dictated>",
            focus_line(app, context.focused_session.as_deref())
        ));
    } else {
        out.push('\n');
        out.push_str(text);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screen_tail_drops_trailing_blank_rows_and_clips_width() {
        let tail = screen_tail("one\ntwo   \n\n\n", 5, 2);
        assert_eq!(tail, vec!["on".to_string(), "tw".to_string()]);
    }

    #[test]
    fn git_branch_reads_head_from_a_parent_directory() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join(".git")).unwrap();
        std::fs::write(root.path().join(".git/HEAD"), "ref: refs/heads/feature/x\n").unwrap();
        let nested = root.path().join("src/deep");
        std::fs::create_dir_all(&nested).unwrap();
        assert_eq!(
            git_branch(nested.to_str().unwrap()).as_deref(),
            Some("feature/x")
        );
        assert_eq!(git_branch("Z:\\definitely\\missing"), None);
    }
}
