use super::feed::{Kind, Notice};
use super::realtime::{Conductor, Effect};
use super::tools::{self, Effect as ToolEffect, Gate, Verdict, consent};
use super::*;
use crate::BridgeConfig;
use std::time::{Duration, Instant};

fn app(root: &std::path::Path) -> AppState {
    AppState::new(BridgeConfig { data_root: root.to_path_buf(), ..BridgeConfig::default() }, "token".into(), Arc::new(|_| {}))
}

fn call() -> (Arc<Call>, mpsc::UnboundedReceiver<Input>) {
    let (inbox, rx) = mpsc::unbounded_channel();
    let call = Arc::new(Call {
        id: "rtc_test".into(),
        client: "web".into(),
        started_at: iso_now(),
        voice: "marin".into(),
        focused: Mutex::new(None),
        announce: Mutex::new(Announce::All),
        watched: Mutex::new(Vec::new()),
        state: Mutex::new("listening"),
        ended: AtomicBool::new(false),
        inbox,
        asks: AtomicU64::new(0),
    });
    (call, rx)
}

fn sends(effects: &[Effect]) -> Vec<Value> {
    effects.iter().filter_map(|effect| if let Effect::Send(value) = effect { Some(value.clone()) } else { None }).collect()
}

fn types(effects: &[Effect]) -> Vec<String> {
    sends(effects).iter().map(|value| value["type"].as_str().unwrap_or("").to_owned()).collect()
}

fn function_call(response: &str, call_id: &str, name: &str, args: Value) -> Value {
    json!({"type":"response.output_item.done","response_id":response,
        "item":{"type":"function_call","call_id":call_id,"name":name,"arguments":args.to_string()}})
}

#[test]
fn the_catalog_is_every_bro_tool_the_voice_needs_and_nothing_orchestrator_internal() {
    let catalog = tools::catalog();
    let names: Vec<&str> = catalog.iter().map(|(name, _, _)| name.as_str()).collect();
    for name in ["list_sessions", "read_session", "read_chat", "send_input", "answer_prompt", "open_session", "close_session", "get_usage",
        "message_agent", "stop_agent", "ask_orchestrator", "workspace_brief", "confirm_action", "end_call"] {
        assert!(names.contains(&name), "{name} is missing");
    }
    for internal in ["watchdog_update", "clear_own_context", "open_orchestrator", "wait_for_output", "wait_for_sessions", "coordination_accept"] {
        assert!(!names.contains(&internal), "{internal} must not reach the voice");
    }
    let mut unique = names.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), names.len(), "tool names are unique");
    let open = &catalog.iter().find(|(name, _, _)| name == "open_session").unwrap().1;
    for dropped in ["taskContract", "origin", "forWatchdog", "taskId", "readOnly"] {
        assert!(open["inputSchema"]["properties"].get(dropped).is_none());
    }
    for tool in tools::realtime_tools() {
        assert_eq!(tool["type"], "function");
        assert_eq!(tool["parameters"]["type"], "object");
        let name = tool["name"].as_str().unwrap();
        assert!(name.len() <= 64 && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
        assert!(tool["description"].as_str().is_some_and(|d| !d.is_empty()));
    }
    assert_eq!(tools::effect_of("list_sessions", &json!({})), Some(ToolEffect::Read));
    assert_eq!(tools::effect_of("message_agent", &json!({})), Some(ToolEffect::Act));
    assert_eq!(tools::effect_of("close_all_sessions", &json!({})), Some(ToolEffect::Confirm));
    assert_eq!(tools::effect_of("send_keys", &json!({"keys":["esc"]})), Some(ToolEffect::Act));
    assert_eq!(tools::effect_of("send_keys", &json!({"keys":["down","Ctrl+C"]})), Some(ToolEffect::Confirm));
    assert_eq!(tools::effect_of("rm_rf", &json!({})), None);
}

#[test]
fn consent_reads_a_clear_yes_and_any_no() {
    for yes in ["Yes.", "yeah do it", "Sure, go ahead", "OK", "yep close it", "Confirm."] {
        assert_eq!(consent(yes), Some(true), "{yes}");
    }
    for no in ["No.", "nope", "wait", "don't", "yes, wait, no", "not yet", "leave it", "Cancel that"] {
        assert_eq!(consent(no), Some(false), "{no}");
    }
    for unclear in ["what?", "which one is Toast", "", "hmm"] {
        assert_eq!(consent(unclear), None, "{unclear}");
    }
}

#[test]
fn only_the_users_own_words_after_the_question_release_a_parked_action() {
    let start = Instant::now();
    let mut gate = Gate::default();
    gate.heard("yes", start); // said before anything was parked: doesn't count
    let id = gate.park("close_session", &json!({"sessionId":"toast"}), "Close Toast".into(), start + Duration::from_secs(1));
    assert!(matches!(gate.confirm(&id, start + Duration::from_secs(2)), Verdict::NotYet(_)));
    gate.heard("which one?", start + Duration::from_secs(3));
    assert!(matches!(gate.confirm(&id, start + Duration::from_secs(4)), Verdict::NotYet(_)));
    gate.heard("Yeah, do it.", start + Duration::from_secs(5));
    match gate.confirm(&id, start + Duration::from_secs(6)) {
        Verdict::Go(parked) => assert_eq!(parked.name, "close_session"),
        other => panic!("{other:?}"),
    }
    assert_eq!(gate.confirm(&id, start + Duration::from_secs(7)), Verdict::Unknown, "released once");

    let id = gate.park("clear_context", &json!({}), "Clear Pinky".into(), start + Duration::from_secs(10));
    gate.heard("no, leave it", start + Duration::from_secs(11));
    assert!(matches!(gate.confirm(&id, start + Duration::from_secs(12)), Verdict::Declined(_)));

    let id = gate.park("close_session", &json!({}), "Close Mal".into(), start + Duration::from_secs(20));
    gate.heard("yes", start + Duration::from_secs(21));
    assert_eq!(gate.confirm(&id, start + Duration::from_secs(20) + tools::CONFIRM_TTL), Verdict::Unknown, "expired");
}

#[test]
fn a_yes_just_before_the_action_counts_only_when_the_user_asked_for_it() {
    let start = Instant::now();
    // "close Toast" -> the model asks first -> "yes, do it" -> only then the tool call.
    let mut gate = Gate::default();
    gate.heard("Great. OK, close toast.", start);
    gate.heard("Yes, do it.", start + Duration::from_secs(8));
    let id = gate.park("close_session", &json!({"sessionId":"toast"}), "Close Toast".into(), start + Duration::from_secs(10));
    assert!(matches!(gate.confirm(&id, start + Duration::from_secs(11)), Verdict::Go(_)));

    // A yes to something else doesn't let the model close a session nobody asked to close.
    let mut gate = Gate::default();
    gate.heard("tell Toast to send the release email", start);
    gate.heard("yes", start + Duration::from_secs(8));
    let id = gate.park("close_session", &json!({"sessionId":"toast"}), "Close Toast".into(), start + Duration::from_secs(10));
    assert!(matches!(gate.confirm(&id, start + Duration::from_secs(11)), Verdict::NotYet(_)), "\"send\" is not \"end\"");

    // A stale yes doesn't either.
    let mut gate = Gate::default();
    gate.heard("close Pinky", start);
    gate.heard("yes", start + Duration::from_secs(1));
    let id = gate.park("close_session", &json!({}), "Close Pinky".into(), start + Duration::from_secs(40));
    assert!(matches!(gate.confirm(&id, start + Duration::from_secs(41)), Verdict::NotYet(_)));
}

#[test]
fn announcements_follow_the_calls_setting() {
    let (call, _rx) = call();
    let finished = Notice::new(Kind::Finished, "Toast finished", Some("toast".into()));
    let question = Notice::new(Kind::Input, "Toast needs the user", Some("toast".into()));
    let opened = Notice::new(Kind::Opened, "Mal opened", Some("mal".into()));
    assert!(finished.speak_for(&call) && question.speak_for(&call) && !opened.speak_for(&call));
    *call.announce.lock() = Announce::Asked;
    assert!(!finished.speak_for(&call) && question.speak_for(&call));
    call.watch("toast");
    assert!(finished.speak_for(&call));
    *call.announce.lock() = Announce::Off;
    let pinky = Notice::new(Kind::Finished, "Pinky finished", Some("pinky".into()));
    assert!(!pinky.speak_for(&call) && finished.speak_for(&call), "a session the user talked to still reports back");
    assert_eq!(Announce::parse("Quiet"), Some(Announce::Off));
}

#[test]
fn a_tool_call_runs_and_the_model_continues_once_its_output_is_in() {
    let root = tempfile::tempdir().unwrap();
    let app = app(root.path());
    let (call, _rx) = call();
    let now = Instant::now();
    let mut conductor = Conductor::new(now);
    conductor.on_server(&app, &call, &json!({"type":"response.created","response":{"id":"r1"}}), now);
    let effects = conductor.on_server(&app, &call, &function_call("r1", "c1", "list_projects", json!({})), now);
    assert!(effects.iter().any(|e| matches!(e, Effect::Run { call_id: Some(id), name, .. } if id == "c1" && name == "list_projects")));
    let effects = conductor.on_server(&app, &call, &json!({"type":"response.done","response":{"id":"r1","status":"completed"}}), now);
    assert!(!types(&effects).contains(&"response.create".to_owned()), "waits for the tool");
    let effects = conductor.on_input(&app, &call, Input::ToolDone { call_id: "c1".into(), output: json!({"ok":true,"result":"[]"}) }, now);
    let sent = sends(&effects);
    assert_eq!(sent[0]["item"]["type"], "function_call_output");
    assert_eq!(sent[0]["item"]["call_id"], "c1");
    assert_eq!(sent[1]["type"], "response.create");
    assert!(sent[1].get("response").is_none(), "a continuation keeps the session's tools and instructions");
}

#[test]
fn destructive_tools_wait_for_a_spoken_yes_and_never_run_on_the_models_word() {
    let root = tempfile::tempdir().unwrap();
    let app = app(root.path());
    let (call, _rx) = call();
    let now = Instant::now();
    let mut conductor = Conductor::new(now);
    conductor.on_server(&app, &call, &json!({"type":"response.created","response":{"id":"r1"}}), now);
    let effects = conductor.on_server(&app, &call, &function_call("r1", "c1", "close_session", json!({"sessionId":"toast"})), now);
    assert!(!effects.iter().any(|e| matches!(e, Effect::Run { .. })), "nothing runs yet");
    let output: Value = serde_json::from_str(sends(&effects)[0]["item"]["output"].as_str().unwrap()).unwrap();
    assert_eq!(output["needsConfirmation"], true);
    let action = output["actionId"].as_str().unwrap().to_owned();
    assert!(effects.iter().any(|e| matches!(e, Effect::Ui(v) if v["event"] == "confirm")));
    let effects = conductor.on_server(&app, &call, &json!({"type":"response.done","response":{"id":"r1"}}), now);
    assert!(types(&effects).contains(&"response.create".to_owned()), "the model asks the user");

    // The model claims the user agreed; bro heard nothing.
    let later = now + Duration::from_secs(2);
    conductor.on_server(&app, &call, &json!({"type":"response.created","response":{"id":"r2"}}), later);
    conductor.on_server(&app, &call, &json!({"type":"response.done","response":{"id":"r2"}}), later);
    let effects = conductor.on_server(&app, &call, &function_call("r3", "c2", "confirm_action", json!({"actionId":action})), later);
    assert!(!effects.iter().any(|e| matches!(e, Effect::Run { .. })));
    let output: Value = serde_json::from_str(sends(&effects)[0]["item"]["output"].as_str().unwrap()).unwrap();
    assert_eq!(output["ok"], false);

    let heard = later + Duration::from_secs(1);
    conductor.on_server(&app, &call, &json!({"type":"conversation.item.input_audio_transcription.completed","item_id":"u1","transcript":"Yes, close it."}), heard);
    let effects = conductor.on_server(&app, &call, &function_call("r4", "c3", "confirm_action", json!({"actionId":action})), heard);
    assert!(effects.iter().any(|e| matches!(e, Effect::Run { call_id: Some(id), name, args } if id == "c3" && name == "close_session" && args["sessionId"] == "toast")));
}

#[test]
fn a_tap_on_the_card_runs_the_parked_action_and_tells_the_model() {
    let root = tempfile::tempdir().unwrap();
    let app = app(root.path());
    let (call, _rx) = call();
    let now = Instant::now();
    let mut conductor = Conductor::new(now);
    let effects = conductor.on_server(&app, &call, &function_call("r1", "c1", "clear_context", json!({"sessionId":"pinky"})), now);
    let output: Value = serde_json::from_str(sends(&effects)[0]["item"]["output"].as_str().unwrap()).unwrap();
    let action = output["actionId"].as_str().unwrap().to_owned();
    let effects = conductor.on_input(&app, &call, Input::Confirm { action_id: action.clone(), approve: true }, now);
    assert!(effects.iter().any(|e| matches!(e, Effect::Run { call_id: None, name, .. } if name == "clear_context")));
    let effects = conductor.on_input(&app, &call, Input::Confirm { action_id: action, approve: true }, now);
    assert!(!effects.iter().any(|e| matches!(e, Effect::Run { .. })), "a second tap does nothing");
}

#[test]
fn updates_wait_for_a_quiet_line_and_are_spoken_without_tools() {
    let root = tempfile::tempdir().unwrap();
    let app = app(root.path());
    let (call, _rx) = call();
    let start = Instant::now();
    let mut conductor = Conductor::new(start);
    conductor.on_server(&app, &call, &json!({"type":"input_audio_buffer.speech_started"}), start);
    conductor.on_input(&app, &call, Input::Ready, start);
    let effects = conductor.on_input(&app, &call, Input::Notice(Notice::new(Kind::Finished, "Toast finished its turn: tests pass", Some("toast".into()))), start);
    assert!(sends(&effects).is_empty(), "never over the user");
    let later = start + Duration::from_secs(1);
    conductor.on_server(&app, &call, &json!({"type":"input_audio_buffer.speech_stopped"}), later);
    assert!(sends(&conductor.tick(&app, &call, later + Duration::from_millis(200))).is_empty(), "the user may go on");
    let effects = conductor.tick(&app, &call, later + Duration::from_secs(3));
    let sent = sends(&effects);
    assert_eq!(sent.len(), 2, "{sent:?}");
    assert_eq!(sent[0]["item"]["role"], "system");
    let text = sent[0]["item"]["content"][0]["text"].as_str().unwrap();
    assert!(text.starts_with("[bro] Toast finished its turn: tests pass"), "{text}");
    assert_eq!(sent[1]["type"], "response.create");
    assert_eq!(sent[1]["response"]["tool_choice"], "none", "an update can't make the model act");
    assert!(sent[1]["response"].get("instructions").is_none(), "the session's instructions (and digest) stay in force");
}

#[test]
fn background_updates_are_batched_into_context_without_a_reply() {
    let root = tempfile::tempdir().unwrap();
    let app = app(root.path());
    let (call, _rx) = call();
    let start = Instant::now();
    let mut conductor = Conductor::new(start);
    conductor.on_server(&app, &call, &json!({"type":"input_audio_buffer.speech_stopped"}), start); // no greeting
    conductor.on_input(&app, &call, Input::Notice(Notice::new(Kind::Opened, "Mal opened.", Some("mal".into()))), start);
    conductor.on_input(&app, &call, Input::Notice(Notice::new(Kind::Closed, "Sky closed.", Some("sky".into()))), start);
    assert!(sends(&conductor.tick(&app, &call, start + Duration::from_secs(3))).is_empty(), "batched");
    let effects = conductor.tick(&app, &call, start + Duration::from_secs(9));
    assert_eq!(types(&effects), vec!["conversation.item.create"]);
    let text = sends(&effects)[0]["item"]["content"][0]["text"].as_str().unwrap().to_owned();
    assert!(text.contains("[bro, background] Mal opened.") && text.contains("[bro, background] Sky closed."), "{text}");
}

#[test]
fn the_greeting_waits_for_the_clients_audio_and_keeps_the_digest() {
    let root = tempfile::tempdir().unwrap();
    let app = app(root.path());
    let (call, _rx) = call();
    let start = Instant::now();
    let mut conductor = Conductor::new(start);
    assert!(sends(&conductor.tick(&app, &call, start + Duration::from_secs(1))).is_empty());
    let effects = conductor.on_input(&app, &call, Input::Ready, start + Duration::from_secs(1));
    let sent = sends(&effects);
    assert_eq!(sent[0]["item"]["role"], "system");
    assert_eq!(sent[1]["response"]["metadata"]["bro"], "greeting");
    assert!(sent[1]["response"].get("instructions").is_none());
}

#[test]
fn the_digest_lists_waiting_agents_first_with_their_question() {
    let root = tempfile::tempdir().unwrap();
    let app = app(root.path());
    assert!(brain::digest(&app).contains("No sessions are open"));
    let prompt = json!({"title":"Allow Bash: npm publish?","options":[{"key":"1","label":"Yes"},{"key":"2","label":"No"}]});
    assert_eq!(brain::prompt_text(Some(&prompt)).unwrap(), "Allow Bash: npm publish? Options: 1 Yes, 2 No");
    let instructions = brain::instructions(&app, None, "phone");
    assert!(instructions.contains("# Right now") && instructions.contains("their phone"));
    assert_eq!(brain::clip("a  b\nc", 10), "a b c");
    assert!(brain::clip_keep_lines(&"line\n".repeat(100), 50).starts_with('…'));
}

// ----- the HTTP + sideband loop, against a stand-in for the provider --------------------------------

mod stand_in {
    use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
    use axum::extract::{Query, State};
    use axum::response::IntoResponse;
    use axum::routing::{any, post};
    use axum::Router;
    use futures_util::{SinkExt, StreamExt};
    use parking_lot::Mutex;
    use serde_json::{Value, json};
    use std::collections::HashMap;
    use std::sync::Arc;

    #[derive(Default)]
    pub struct Seen {
        pub calls: Mutex<Vec<(String, Value, Option<String>)>>,
        pub sideband: Mutex<Vec<Value>>,
        pub hangups: Mutex<Vec<String>>,
        pub attached: Mutex<Vec<(String, Option<String>)>>,
    }

    async fn calls(State(seen): State<Arc<Seen>>, headers: axum::http::HeaderMap, mut form: axum::extract::Multipart) -> impl IntoResponse {
        let (mut sdp, mut session) = (String::new(), Value::Null);
        while let Ok(Some(field)) = form.next_field().await {
            let name = field.name().unwrap_or("").to_owned();
            let text = field.text().await.unwrap_or_default();
            if name == "sdp" { sdp = text } else if name == "session" { session = serde_json::from_str(&text).unwrap_or(Value::Null) }
        }
        let auth = headers.get("authorization").and_then(|v| v.to_str().ok()).map(str::to_owned);
        seen.calls.lock().push((sdp, session, auth));
        ([("location", "/v1/realtime/calls/rtc_standin")], "v=0\r\no=- 1 1 IN IP4 0.0.0.0\r\ns=answer\r\n")
    }

    async fn hangup(State(seen): State<Arc<Seen>>, axum::extract::Path(id): axum::extract::Path<String>) -> impl IntoResponse {
        seen.hangups.lock().push(id);
        "{}"
    }

    async fn sideband(State(seen): State<Arc<Seen>>, Query(query): Query<HashMap<String, String>>, headers: axum::http::HeaderMap, upgrade: WebSocketUpgrade) -> impl IntoResponse {
        let auth = headers.get("authorization").and_then(|v| v.to_str().ok()).map(str::to_owned);
        seen.attached.lock().push((query.get("call_id").cloned().unwrap_or_default(), auth));
        upgrade.on_upgrade(move |socket| script(socket, seen))
    }

    /// Plays the model: one turn that calls a read tool, then waits for bro's answer.
    async fn script(socket: WebSocket, seen: Arc<Seen>) {
        let (mut tx, mut rx) = socket.split();
        let send = |value: Value| Message::Text(value.to_string().into());
        let _ = tx.send(send(json!({"type":"input_audio_buffer.speech_started"}))).await;
        let _ = tx.send(send(json!({"type":"input_audio_buffer.speech_stopped"}))).await;
        let _ = tx.send(send(json!({"type":"conversation.item.input_audio_transcription.completed","item_id":"u1","transcript":"what projects do I have"}))).await;
        let _ = tx.send(send(json!({"type":"response.created","response":{"id":"r1"}}))).await;
        let _ = tx.send(send(json!({"type":"response.output_item.done","response_id":"r1","item":{"type":"function_call","call_id":"c1","name":"list_projects","arguments":"{}"}}))).await;
        let _ = tx.send(send(json!({"type":"response.done","response":{"id":"r1","status":"completed"}}))).await;
        while let Some(Ok(message)) = rx.next().await {
            if let Message::Text(text) = message
                && let Ok(value) = serde_json::from_str::<Value>(&text)
            {
                seen.sideband.lock().push(value);
            }
        }
    }

    pub async fn start() -> (String, Arc<Seen>) {
        let seen = Arc::new(Seen::default());
        let router = Router::new()
            .route("/v1/realtime/calls", post(calls))
            .route("/v1/realtime/calls/{id}/hangup", post(hangup))
            .route("/v1/realtime", any(sideband))
            .with_state(seen.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        (format!("http://{address}/v1"), seen)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_call_runs_end_to_end_through_bro_with_the_key_kept_on_this_machine() {
    let (base, seen) = stand_in::start().await;
    let root = tempfile::tempdir().unwrap();
    let bridge = crate::Bridge::start(
        BridgeConfig { port: 0, automatic_port: false, bind: "127.0.0.1".into(), data_root: root.path().join("bridge"), web_interface: false },
        Box::new(|_| {}),
    )
    .unwrap();
    // A key in a variable only this test sets.
    let variable = format!("BRO_TEST_VOICE_KEY_{}", std::process::id());
    // SAFETY: a variable name unique to this test process; nothing else reads it.
    unsafe { std::env::set_var(&variable, "sk-test-standin") };
    bridge.set_voice_config(VoiceConfig { api_key_env: variable.clone(), api_base: base, ..VoiceConfig::default() });
    let url = bridge.local_url().unwrap();
    let client = reqwest::Client::new();

    let status: Value = client.get(format!("{url}/api/voice")).send().await.unwrap().json().await.unwrap();
    assert_eq!(status["available"], true);
    let page = client.get(format!("{url}/voice")).send().await.unwrap();
    assert!(page.text().await.unwrap().contains("Start talking"));

    let bad = client.post(format!("{url}/api/voice/calls")).json(&json!({"sdp":"hello"})).send().await.unwrap();
    assert_eq!(bad.status(), 400);
    let started = client
        .post(format!("{url}/api/voice/calls"))
        .json(&json!({"sdp":"v=0\r\no=- 0 0 IN IP4 127.0.0.1\r\ns=offer\r\n","voice":"cedar","client":"phone"}))
        .send()
        .await
        .unwrap();
    assert_eq!(started.status(), 200);
    let started: Value = started.json().await.unwrap();
    assert_eq!(started["callId"], "rtc_standin");
    assert!(started["sdp"].as_str().unwrap().starts_with("v=0"));
    assert!(started.to_string().find("sk-test").is_none(), "the key never reaches the client");
    {
        let calls = seen.calls.lock();
        let (sdp, session, auth) = &calls[0];
        assert!(sdp.contains("s=offer"));
        assert_eq!(auth.as_deref(), Some("Bearer sk-test-standin"));
        assert_eq!(session["type"], "realtime");
        assert_eq!(session["audio"]["output"]["voice"], "cedar");
        assert!(session["instructions"].as_str().unwrap().contains("You are bro"));
        assert!(session["tools"].as_array().unwrap().len() > 20);
    }

    // The scripted turn calls list_projects; bro answers it on the sideband and lets the model go on.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let done = {
            let sideband = seen.sideband.lock();
            sideband.iter().any(|e| e["type"] == "conversation.item.create" && e["item"]["call_id"] == "c1")
                && sideband.iter().any(|e| e["type"] == "response.create" && e.get("response").is_none())
        };
        if done { break; }
        assert!(tokio::time::Instant::now() < deadline, "bro never answered the tool call: {:?}", seen.sideband.lock());
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(seen.attached.lock()[0], ("rtc_standin".to_owned(), Some("Bearer sk-test-standin".to_owned())));
    let status: Value = client.get(format!("{url}/api/voice")).send().await.unwrap().json().await.unwrap();
    assert_eq!(status["calls"][0]["id"], "rtc_standin");
    assert_eq!(status["calls"][0]["client"], "phone");

    let hung = client.post(format!("{url}/api/voice/calls/rtc_standin/hangup")).send().await.unwrap();
    assert_eq!(hung.status(), 200);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while seen.hangups.lock().is_empty() {
        assert!(tokio::time::Instant::now() < deadline, "bro didn't end the call upstream");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let status: Value = client.get(format!("{url}/api/voice")).send().await.unwrap().json().await.unwrap();
    assert!(status["calls"].as_array().unwrap().is_empty());
    // SAFETY: as above.
    unsafe { std::env::remove_var(&variable) };
    bridge.shutdown();
}
