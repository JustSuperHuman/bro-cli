//! Actions on bro's sessions that take more than one command: creating a
//! session (replaces the reference host's `wt.exe new-tab` launch) and
//! paced composed input.

use crate::model::TerminalSessionSummary;
use crate::state::{AppState, COMMAND_INPUT};
use crate::{BridgeCommand, CreateRequest};
use serde_json::Value;
use std::path::PathBuf;
use std::time::Duration;
use tokio::sync::oneshot;

/// How long bro has to answer a create request.
pub(crate) const CREATE_TIMEOUT: Duration = Duration::from_secs(8);
/// How long after bro answers the session may take to `register`.
const REGISTER_GRACE: Duration = Duration::from_secs(3);

// Composed text goes out in paced chunks: a TUI that re-renders per read needs
// room to keep up, and one huge write is what a paste-burst heuristic is most
// likely to misjudge. A long prompt still lands well under a second.
const PASTE_CHUNK_CHARS: usize = 512;
const PASTE_CHUNK_DELAY: Duration = Duration::from_millis(6);

fn string_field(body: &Value, key: &str) -> Option<String> {
    body.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

/// The create request a client body (`CreateSessionOptions`) describes.
/// A known profile fills in shell/args the client left out.
pub(crate) fn create_request(state: &AppState, body: &Value) -> CreateRequest {
    let profile_id = string_field(body, "profileId");
    let profile = profile_id.as_deref().and_then(|id| {
        state
            .inner
            .lock()
            .profiles
            .iter()
            .find(|profile| profile.id == id)
            .cloned()
    });
    let explicit_args: Option<Vec<String>> =
        body.get("args").and_then(Value::as_array).map(|args| {
            args.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        });
    let shell = string_field(body, "shell");
    let (shell, args) = match (shell, &profile) {
        (Some(shell), _) => (Some(shell), explicit_args.unwrap_or_default()),
        (None, Some(profile)) => (
            Some(profile.shell.clone()),
            explicit_args.unwrap_or_else(|| profile.args.clone()),
        ),
        (None, None) => (None, explicit_args.unwrap_or_default()),
    };
    CreateRequest {
        title: string_field(body, "title"),
        profile_id,
        shell,
        args,
        cwd: string_field(body, "cwd").map(PathBuf::from),
        project_id: string_field(body, "projectId"),
    }
}

/// Asks bro for a new session and waits until it is registered.
pub(crate) async fn launch_terminal(
    state: &AppState,
    body: Value,
) -> Result<TerminalSessionSummary, String> {
    let request = create_request(state, &body);
    let requested_title = request.title.clone();
    let requested_project = request.project_id.clone();
    let (reply, answer) = oneshot::channel();
    state.send_command(BridgeCommand::Create {
        req: request,
        reply,
    });
    let id = match tokio::time::timeout(CREATE_TIMEOUT, answer).await {
        Err(_) => return Err("bro did not create the session within 8 seconds.".into()),
        Ok(Err(_)) => return Err("bro declined to create the session.".into()),
        Ok(Ok(Err(error))) => return Err(format!("The session could not be created: {error}")),
        Ok(Ok(Ok(id))) => id,
    };
    let deadline = tokio::time::Instant::now() + REGISTER_GRACE;
    let mut session = loop {
        if let Some(session) = state.summary(&id) {
            break session;
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(format!(
                "Session {id} was created but never registered with the bridge."
            ));
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    if let Some(title) = requested_title
        && session.title != title
    {
        session = state.rename(&id, title).unwrap_or(session);
    }
    if requested_project.is_some() {
        state.set_project_id(&id, requested_project);
        session = state.summary(&id).unwrap_or(session);
    }
    Ok(session)
}

/// How long to let a paste settle before Enter. Agents buffer a paste (Claude
/// Code collapses it into a "[Pasted text]" token, Codex runs a paste-burst
/// window) and an Enter that lands inside that window becomes a newline.
pub(crate) fn submit_settle(bytes: usize) -> Duration {
    Duration::from_millis((150 + bytes as u64 / 40).min(600))
}

/// Writes `data` as paced input chunks, never splitting a character.
pub(crate) async fn write_paced(state: &AppState, id: &str, data: &str) -> Result<(), String> {
    let mut rest = data;
    while !rest.is_empty() {
        let mut end = rest.len().min(PASTE_CHUNK_CHARS);
        while !rest.is_char_boundary(end) {
            end += 1;
        }
        let (chunk, tail) = rest.split_at(end);
        state.dispatch(id, COMMAND_INPUT, chunk, 0, 0)?;
        rest = tail;
        if !rest.is_empty() {
            tokio::time::sleep(PASTE_CHUNK_DELAY).await;
        }
    }
    Ok(())
}

/// What a compose request writes: bracketed paste when the program asked
/// for it, otherwise newlines typed as Enter.
pub(crate) fn compose_payload(text: &str, bracketed: bool) -> String {
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    if text.is_empty() {
        String::new()
    } else if bracketed {
        format!("\u{1b}[200~{text}\u{1b}[201~")
    } else {
        text.replace('\n', "\r")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compose_payload_brackets_or_types() {
        assert_eq!(
            compose_payload("a\r\nb", true),
            "\u{1b}[200~a\nb\u{1b}[201~"
        );
        assert_eq!(compose_payload("a\nb", false), "a\rb");
        assert_eq!(compose_payload("", true), "");
        assert!(submit_settle(0) >= Duration::from_millis(150));
        assert_eq!(submit_settle(1_000_000), Duration::from_millis(600));
    }
}
