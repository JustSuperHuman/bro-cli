//! Actions on bro's sessions that take more than one command: creating a
//! session (replaces the reference host's `wt.exe new-tab` launch) and
//! paced composed input.

use crate::model::TerminalSessionSummary;
use crate::state::AppState;
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
        model: string_field(body, "model"),
    }
}

/// Asks bro for a new session and waits until it is registered.
pub(crate) async fn launch_terminal(
    state: &AppState,
    body: Value,
) -> Result<TerminalSessionSummary, String> {
    let request = create_request(state, &body);
    // A mistyped folder must not quietly become bro's current project.
    if let Some(cwd) = &request.cwd {
        crate::folders::check_start_folder(cwd)?;
    }
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
    Duration::from_millis((350 + bytes as u64 / 40).min(800))
}

fn agent_submit_settle(state: &AppState, id: &str, bytes: usize) -> Duration {
    // Codex can keep its paste-burst window open after the PTY write returns.
    // A terminal write receipt is not an acknowledgement from the agent.
    if state.summary(id).is_some_and(|s| s.agent.as_deref() == Some("codex")) {
        submit_settle(bytes).max(Duration::from_millis(1500))
    } else {
        submit_settle(bytes)
    }
}

/// The callback normally queues work on bro's UI thread. Starting a paste timer when that work is queued
/// lets a busy frame collapse paste and Enter into consecutive writes. Wait for the session to take it first.
async fn write_confirmed(state: &AppState, id: &str, data: &str) -> Result<(), String> {
    let (reply, answer) = oneshot::channel();
    state.send_command(BridgeCommand::Write { id: id.into(), data: data.as_bytes().to_vec(), reply });
    tokio::time::timeout(Duration::from_secs(8), answer).await
        .map_err(|_| "The session did not confirm the input. Check the terminal before retrying.".to_string())?
        .map_err(|_| "The session closed before confirming the input.".to_string())?
}

/// One message, including its Enter, under a per-session lock. The pause starts after bro has handled the
/// paste, rather than when the bridge queued it. A bare Enter remains immediate for terminal controls.
pub(crate) async fn compose_input(state: &AppState, id: &str, data: &str, submit: bool) -> Result<(), String> {
    let lock = state.inner.lock().sessions.get(id)
        .map(|session| session.compose_lock.clone()).ok_or("Unknown terminal session.")?;
    let _guard = lock.lock().await;
    write_paced(state, id, data).await?;
    if submit {
        if !data.is_empty() {
            tokio::time::sleep(agent_submit_settle(state, id, data.len())).await;
        }
        write_confirmed(state, id, "\r").await?;
    }
    Ok(())
}

/// Validate after taking the input lock, then deliver only this question's response.
pub(crate) async fn answer_prompt(state: &AppState, id: &str, body: &Value) -> Result<(), String> {
    let lock = state.inner.lock().sessions.get(id).map(|s| s.compose_lock.clone()).ok_or("Unknown terminal session.")?;
    let _guard = lock.try_lock().map_err(|_| "An answer is already being sent. Wait for the question to refresh.")?;
    let context = state.input_context(id).ok_or("Unknown terminal session.")?;
    if context["status"] != "running" { return Err("The session has exited.".into()); }
    let data = crate::prompt::prompt_response_bytes(&context, body)?;
    let codex = context.pointer("/prompt/protocol").and_then(Value::as_str).is_some_and(|p| p.starts_with("codex-question"));
    let final_action = codex && body["action"] != "open-notes";
    if final_action {
        let mut inner = state.inner.lock();
        let session = inner.sessions.get_mut(id).ok_or("Unknown terminal session.")?;
        let prompt_id = context["prompt"]["id"].as_str().ok_or("No question is available.")?;
        if session.answered_prompt.as_deref() == Some(prompt_id) {
            return Err("An answer was already sent. Wait for the question to refresh.".into());
        }
        session.answered_prompt = Some(prompt_id.into());
    }
    if codex && body["action"] == "text" {
        let paste = data.strip_suffix('\r').unwrap_or(&data);
        write_paced(state, id, paste).await?;
        tokio::time::sleep(submit_settle(paste.len())).await;
        write_confirmed(state, id, "\r").await
    } else {
        write_confirmed(state, id, &data).await
    }
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
        write_confirmed(state, id, chunk).await?;
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
        format!("\u{1b}[200~{}\u{1b}[201~", text.replace("\u{1b}[201~", ""))
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
        assert_eq!(compose_payload("a\x1b[201~b", true), "\x1b[200~ab\x1b[201~");
        assert!(submit_settle(0) >= Duration::from_millis(350));
        assert_eq!(submit_settle(1_000_000), Duration::from_millis(800));
    }

    fn input_state() -> (tempfile::TempDir, AppState, tokio::sync::mpsc::UnboundedReceiver<BridgeCommand>) {
        let root = tempfile::tempdir().unwrap();
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let state = AppState::new(crate::BridgeConfig { data_root: root.path().into(), ..Default::default() }, String::new(),
            std::sync::Arc::new(move |command| { let _ = tx.send(command); }));
        state.register(TerminalSessionSummary::native("s".into(), "test".into(), "pwsh".into(), String::new(), 0, 100, 30), None, None);
        (root, state, rx)
    }

    #[tokio::test]
    async fn compose_waits_for_bro_before_timing_enter_and_waits_for_enter_receipt() {
        let (_root, state, mut commands) = input_state();
        let sending = tokio::spawn(async move { compose_input(&state, "s", "hello", true).await });
        let BridgeCommand::Write { data, reply, .. } = commands.recv().await.unwrap() else { panic!("paste") };
        assert_eq!(data, b"hello");
        // A busy UI can hold the paste longer than the entire old settle interval.
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert!(commands.try_recv().is_err());
        assert!(!sending.is_finished());
        let delivered = std::time::Instant::now();
        reply.send(Ok(())).unwrap();
        assert!(tokio::time::timeout(Duration::from_millis(200), commands.recv()).await.is_err());
        let BridgeCommand::Write { data, reply, .. } = commands.recv().await.unwrap() else { panic!("Enter") };
        assert!(delivered.elapsed() >= Duration::from_millis(350));
        assert_eq!(data, b"\r");
        assert!(!sending.is_finished(), "an enqueued Enter is not a receipt");
        reply.send(Ok(())).unwrap();
        assert!(sending.await.unwrap().is_ok());
    }

    #[tokio::test]
    async fn concurrent_composes_keep_each_message_with_its_enter() {
        let (_root, state, mut commands) = input_state();
        let first_state = state.clone();
        let first = tokio::spawn(async move { compose_input(&first_state, "s", "first", true).await });
        let BridgeCommand::Write { data, reply, .. } = commands.recv().await.unwrap() else { panic!("first paste") };
        assert_eq!(data, b"first");
        let second = tokio::spawn(async move { compose_input(&state, "s", "second", true).await });
        tokio::task::yield_now().await;
        assert!(commands.try_recv().is_err());
        reply.send(Ok(())).unwrap();
        for expected in [b"\r".as_slice(), b"second", b"\r"] {
            let BridgeCommand::Write { data, reply, .. } = commands.recv().await.unwrap() else { panic!("write") };
            assert_eq!(data, expected);
            reply.send(Ok(())).unwrap();
        }
        assert!(first.await.unwrap().is_ok());
        assert!(second.await.unwrap().is_ok());
    }

    #[tokio::test]
    async fn a_failed_paste_never_sends_enter_or_reports_success() {
        let (_root, state, mut commands) = input_state();
        let sending = tokio::spawn(async move { compose_input(&state, "s", "hello", true).await });
        let BridgeCommand::Write { reply, .. } = commands.recv().await.unwrap() else { panic!("paste") };
        reply.send(Err("session exited".into())).unwrap();
        assert_eq!(sending.await.unwrap(), Err("session exited".into()));
        assert!(commands.try_recv().is_err());
    }
}
