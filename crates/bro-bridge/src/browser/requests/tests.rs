use super::*;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

fn snapshot() -> Value {
    json!({"success":true, "requests":[{
        "id":"review-one", "createdAt":1000, "broSessionId":"owner",
        "prompt":"Implement the full review.\nChange the heading to café.\n\tKeep the comments.",
        "pages":[{"id":"https://example.test/", "url":"https://example.test/", "title":"Fixture", "updatedAt":1000, "elements":[]}],
        "status":"pending"
    }]})
}

fn owners() -> HashSet<String> {
    HashSet::from(["owner".into()])
}

fn state(root: &Path, on_command: impl Fn(BridgeCommand) + Send + Sync + 'static) -> AppState {
    AppState::new(
        crate::BridgeConfig {
            port: 0,
            automatic_port: false,
            bind: "127.0.0.1".into(),
            data_root: root.to_owned(),
            web_interface: false,
        },
        "page-review-fixture".into(),
        Arc::new(on_command),
    )
}

fn register(state: &AppState, id: &str, agent: bool, status: &str, activity: &str) {
    let mut summary = crate::model::TerminalSessionSummary::native(
        id.into(),
        format!("Fixture {id}"),
        "fixture".into(),
        state.data_root.to_string_lossy().into_owned(),
        7,
        100,
        30,
    );
    summary.agent = agent.then(|| "codex".into());
    summary.status = status.into();
    summary.agent_activity = Some(activity.into());
    state.register(summary, None, None);
}

#[test]
fn bounds_utf8_request_and_prompt_and_rejects_terminal_controls() {
    let valid = snapshot()["requests"][0].clone();
    assert!(Remote::parse(&valid).is_some());
    for control in ['\0', '\u{1b}', '\u{7f}', '\u{85}', '\u{3}', '\u{8}'] {
        let mut row = valid.clone();
        row["prompt"] = json!(format!("Do this {control} now"));
        assert!(Remote::parse(&row).is_none(), "control {control:?}");
    }
    let mut row = valid.clone();
    row["prompt"] = json!("line one\r\nline two\rline three");
    assert_eq!(
        Remote::parse(&row).unwrap().prompt,
        "line one\nline two\nline three"
    );
    row["prompt"] = json!("é".repeat(MAX_PROMPT / 2 + 1));
    assert!(
        Remote::parse(&row).is_none(),
        "prompt bounds are UTF-8 bytes"
    );
    row["prompt"] = json!("x".repeat(MAX_PROMPT));
    assert!(Remote::parse(&row).is_some());
    row["pages"][0]["extra"] = json!("x".repeat(MAX_REQUEST));
    assert!(
        Remote::parse(&row).is_none(),
        "bound the entire request, including unused page data"
    );
    for (key, value) in [
        ("id", json!("bad\u{1b}id")),
        ("broSessionId", json!(" ")),
        ("status", json!("checked")),
        ("pages", json!([])),
        ("createdAt", json!(-1)),
        ("prompt", json!(" \n\t")),
    ] {
        let mut row = valid.clone();
        row[key] = value;
        assert!(Remote::parse(&row).is_none(), "invalid {key}");
    }
}

#[test]
fn durable_requests_have_no_handoff_or_tab_close_semantics_and_never_retarget() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::new(root.path());
    assert!(
        store
            .observe(Some(&snapshot()), &HashSet::new(), 100)
            .unwrap()
            .is_empty()
    );
    assert!(!root.path().join(format!("{OUTBOX}.json")).exists());
    let ready = store.observe(Some(&snapshot()), &owners(), 101).unwrap();
    assert_eq!(ready.len(), 1);
    assert_eq!(ready[0].event_id(), "page-request/review-one");
    assert!(ready[0].message().contains("café"));
    let empty = json!({"success":true,"requests":[],"tabs":[],"handoffs":[]});
    assert_eq!(
        Store::new(root.path())
            .observe(Some(&empty), &owners(), 102)
            .unwrap()
            .len(),
        1
    );
    assert!(
        store
            .observe(None, &HashSet::new(), 103)
            .unwrap()
            .is_empty(),
        "a closed owner stays pending"
    );
    assert_eq!(
        store.observe(None, &owners(), 104).unwrap().len(),
        1,
        "reopening the same owner resumes the durable request"
    );
    let mut changed = snapshot();
    changed["requests"][0]["broSessionId"] = json!("other");
    changed["requests"][0]["prompt"] = json!("replace the original");
    let ready = store
        .observe(
            Some(&changed),
            &HashSet::from(["owner".into(), "other".into()]),
            105,
        )
        .unwrap();
    assert_eq!(ready[0].session, "owner");
    assert!(!ready[0].prompt.contains("replace the original"));
}

#[test]
fn claims_are_exclusive_across_hosts_and_abandoned_claims_retry() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::new(root.path());
    store.observe(Some(&snapshot()), &owners(), 100).unwrap();
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let threads: Vec<_> = (0..2)
        .map(|_| {
            let barrier = barrier.clone();
            let path = root.path().to_owned();
            std::thread::spawn(move || {
                barrier.wait();
                Store::new(&path)
                    .claim("review-one", 100)
                    .unwrap()
                    .is_some()
            })
        })
        .collect();
    assert_eq!(
        threads
            .into_iter()
            .filter_map(|thread| thread.join().unwrap().then_some(()))
            .count(),
        1
    );
    assert!(store.claim("review-one", 144).unwrap().is_none());
    assert!(store.claim("review-one", 145).unwrap().is_some());
    store.release("review-one", false).unwrap();
    assert!(store.claim("review-one", 146).unwrap().is_some());
}

#[test]
fn a_claim_uses_fresh_delivery_state_and_serializes_one_owners_submissions() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::new(root.path());
    let mut rows = snapshot();
    let mut second = rows["requests"][0].clone();
    second["id"] = json!("review-two");
    rows["requests"].as_array_mut().unwrap().push(second);
    store.observe(Some(&rows), &owners(), 100).unwrap();
    assert!(store.claim("review-one", 100).unwrap().is_some());
    assert!(store.claim("review-two", 100).unwrap().is_none());
    store.delivered("review-one", 101).unwrap();
    store.release("review-one", false).unwrap();
    let ack_only = store.claim("review-one", 102).unwrap().unwrap();
    assert!(ack_only.handled);
    assert!(ack_only.prompt.is_empty());
    assert!(
        store.claim("review-two", 102).unwrap().is_some(),
        "an ACK retry does not block the next request"
    );
}

#[test]
fn remote_delivered_records_do_not_create_work_and_pending_records_never_expire() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::new(root.path());
    let mut delivered = snapshot();
    delivered["requests"][0]["status"] = json!("delivered");
    assert!(
        store
            .observe(Some(&delivered), &owners(), 100)
            .unwrap()
            .is_empty()
    );
    store.observe(Some(&snapshot()), &owners(), 101).unwrap();
    assert_eq!(
        store
            .observe(None, &owners(), 100 + 30 * 86_400)
            .unwrap()
            .len(),
        1
    );
    assert!(
        store
            .observe(Some(&delivered), &owners(), 100 + 30 * 86_400)
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn delivery_is_persisted_before_ack_and_failed_ack_retries_without_resubmission() {
    let root = tempfile::tempdir().unwrap();
    let seen = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let capture = seen.clone();
    let state = state(root.path(), move |command| {
        if let BridgeCommand::BrowserResume {
            id,
            event_id,
            text,
            reply,
        } = command
        {
            capture.lock().push((id, event_id, text));
            let _ = reply.send(Ok(true));
        }
    });
    register(&state, "owner", true, "running", "idle");
    let attempts = Arc::new(AtomicUsize::new(0));
    let count = attempts.clone();
    let path = root.path().to_owned();
    let app = axum::Router::new().route("/page-review", axum::routing::post(move |axum::Json(args): axum::Json<Value>| {
        let count = count.clone();
        let path = path.clone();
        async move {
            assert_eq!(args, json!({"action":"ack", "id":"review-one", "broSessionId":"owner"}));
            let persisted: Value = serde_json::from_slice(&std::fs::read(path.join(format!("{OUTBOX}.json"))).unwrap()).unwrap();
            assert_eq!(persisted["review-one"]["handled"], true, "persist before acknowledging the extension");
            assert_eq!(persisted["review-one"]["prompt"], "");
            if count.fetch_add(1, Ordering::SeqCst) == 0 {
                (axum::http::StatusCode::CONFLICT, axum::Json(json!({"success":false,"error":"fixture disconnected"})))
            } else {
                (axum::http::StatusCode::OK, axum::Json(json!({"success":true,"request":{"id":"review-one","broSessionId":"owner","status":"delivered"}})))
            }
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/page-review", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let store = Store::new(root.path());
    let entry = store
        .observe(Some(&snapshot()), &owners(), now())
        .unwrap()
        .remove(0);
    process(state.clone(), entry, endpoint.clone()).await;
    let retry = Store::new(root.path())
        .observe(Some(&snapshot()), &HashSet::new(), now())
        .unwrap()
        .remove(0);
    assert!(
        retry.handled,
        "even a now-closed session can finish its ACK"
    );
    process(state, retry, endpoint).await;
    assert!(
        store
            .observe(Some(&snapshot()), &owners(), now())
            .unwrap()
            .is_empty()
    );
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    let seen = seen.lock();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].0, "owner");
    assert_eq!(seen[0].1, "page-request/review-one");
    assert!(seen[0].2.contains("Change the heading to café."));
    server.abort();
}

#[tokio::test]
async fn busy_missing_and_exited_owners_defer_and_an_unconfirmed_delivery_stays_pending() {
    let root = tempfile::tempdir().unwrap();
    let attempts = Arc::new(AtomicUsize::new(0));
    let submitted = Arc::new(AtomicBool::new(false));
    let calls = attempts.clone();
    let result = submitted.clone();
    let state = state(root.path(), move |command| {
        if let BridgeCommand::BrowserResume { reply, .. } = command {
            calls.fetch_add(1, Ordering::SeqCst);
            let _ = reply.send(Ok(result.load(Ordering::SeqCst)));
        }
    });
    let store = Store::new(root.path());
    let entry = store
        .observe(Some(&snapshot()), &owners(), now())
        .unwrap()
        .remove(0);
    // No actual browser endpoint is involved in these deferred attempts.
    process(state.clone(), entry.clone(), "http://127.0.0.1:0".into()).await;
    register(&state, "owner", true, "running", "working");
    process(state.clone(), entry.clone(), "http://127.0.0.1:0".into()).await;
    state
        .inner
        .lock()
        .sessions
        .get_mut("owner")
        .unwrap()
        .summary
        .status = "exited".into();
    process(state.clone(), entry.clone(), "http://127.0.0.1:0".into()).await;
    assert_eq!(attempts.load(Ordering::SeqCst), 0);
    {
        let mut inner = state.inner.lock();
        let summary = &mut inner.sessions.get_mut("owner").unwrap().summary;
        summary.status = "running".into();
        summary.agent_activity = Some("idle".into());
    }
    // The real BrowserResume handler owns draft/question/empty-editor checks.
    // Its false result must never be treated as submitted or acknowledged.
    process(state, entry, "http://127.0.0.1:0".into()).await;
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    let ready = store.observe(None, &owners(), now()).unwrap();
    assert_eq!(ready.len(), 1);
    assert!(!ready[0].handled);
    assert!(!ready[0].prompt.is_empty());
}

#[tokio::test]
async fn poll_publishes_only_running_agents_and_keeps_registration_batches_under_32kib() {
    let root = tempfile::tempdir().unwrap();
    let state = state(root.path(), |_| {});
    register(&state, "owner", true, "running", "working");
    register(&state, "shell", false, "running", "idle");
    register(&state, "closed", true, "exited", "idle");
    let seen = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let capture = seen.clone();
    let app = axum::Router::new().route(
        "/page-review",
        axum::routing::post(move |axum::Json(args): axum::Json<Value>| {
            capture.lock().push(args);
            async { axum::Json(json!({"success":true,"requests":[],"targets":[]})) }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/page-review", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    poll(&state, &endpoint).await;
    {
        let seen = seen.lock();
        assert!(seen.iter().any(|args| args["action"] == "list"));
        let registration = seen
            .iter()
            .find(|args| args["action"] == "targets")
            .unwrap();
        assert_eq!(registration["targets"].as_array().unwrap().len(), 1);
        assert_eq!(registration["targets"][0]["id"], "owner");
        assert_eq!(registration["targets"][0]["title"], "Fixture owner");
        assert_eq!(
            registration["targets"][0]["folder"],
            root.path().to_string_lossy().as_ref()
        );
    }
    seen.lock().clear();
    let long = "é".repeat(1000);
    let many: Vec<_> = (0..200).map(|i| json!({"id":format!("owner-{i}"),"title":metadata(&long,512),"folder":metadata(&long,1024)})).collect();
    publish_targets(&endpoint, many).await;
    let seen = seen.lock();
    assert!(seen.len() > 1);
    assert_eq!(
        seen.iter()
            .map(|args| args["targets"].as_array().unwrap().len())
            .sum::<usize>(),
        200
    );
    assert!(
        seen.iter()
            .all(|args| serde_json::to_vec(args).unwrap().len() < 32 * 1024)
    );
    server.abort();
}
