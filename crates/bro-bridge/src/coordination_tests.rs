use super::*;
use std::sync::{Arc, OnceLock};

struct Harness {
    _data: tempfile::TempDir,
    workspace: tempfile::TempDir,
    app: AppState,
    writes: Arc<parking_lot::Mutex<Vec<String>>>,
    wakes: Arc<parking_lot::Mutex<Vec<(String, u64, String)>>>,
}

fn powershell_check(script: &str) -> String {
    if cfg!(windows) {
        format!(
            "\"{}\\System32\\WindowsPowerShell\\v1.0\\powershell.exe\" -NoProfile -File {script}",
            std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into())
        )
    } else {
        format!("pwsh -NoProfile -File {script}")
    }
}

impl Harness {
    fn new() -> Self {
        let data = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        std::fs::write(workspace.path().join("artifact.txt"), "delivered content").unwrap();
        let writes = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let output = writes.clone();
        let wakes = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let wake_sink = wakes.clone();
        let slot = Arc::new(OnceLock::<AppState>::new());
        let on_command = slot.clone();
        let app = AppState::new(
            crate::BridgeConfig {
                data_root: data.path().into(),
                ..Default::default()
            },
            "test-private-bridge-token".into(),
            Arc::new(move |command| match command {
                crate::BridgeCommand::Write { data, reply, .. } => {
                    output
                        .lock()
                        .push(String::from_utf8_lossy(&data).into_owned());
                    let _ = reply.send(Ok(()));
                }
                crate::BridgeCommand::Kill { id } => {
                    let state = on_command.get().unwrap();
                    state.exit(&id, Some(0));
                    state.unregister(&id);
                }
                crate::BridgeCommand::CoordinationWake {
                    task_id,
                    seq,
                    action,
                    ..
                } => wake_sink.lock().push((task_id, seq, action)),
                _ => {}
            }),
        );
        slot.set(app.clone()).ok();
        let mut summary = crate::model::TerminalSessionSummary::native(
            "worker".into(),
            "Worker".into(),
            "claude".into(),
            workspace.path().to_string_lossy().into_owned(),
            9281,
            100,
            30,
        );
        summary.agent = Some("claude".into());
        summary.agent_activity = Some("idle".into());
        app.register(summary, None, crate::agents::Agent::parse("claude"));
        {
            let mut inner = app.inner.lock();
            let session = inner.sessions.get_mut("worker").unwrap();
            session.seq = 1;
            session.summary.agent = Some("claude".into());
            session.summary.agent_activity = Some("idle".into());
        }
        Self {
            _data: data,
            workspace,
            app,
            writes,
            wakes,
        }
    }
    fn create(&self, check: Option<&str>) {
        self.create_authorized(check, true);
    }
    fn create_authorized(&self, check: Option<&str>, authorized: bool) {
        let criterion = match check {
            Some(command) => core::Criterion {
                id: "criterion".into(),
                description: "Runtime check succeeds".into(),
                command: Some(command.into()),
                ..Default::default()
            },
            None => core::Criterion {
                id: "criterion".into(),
                description: "Inspect delivered artifact".into(),
                kind: core::CriterionKind::Artifact,
                ..Default::default()
            },
        };
        let intent = core::NewIntent {
            id: "intent".into(),
            title: "Outcome".into(),
            authorization: core::Authorization {
                scope: self.workspace.path().to_string_lossy().into_owned(),
                source: "Test user authorized this check".into(),
                authorized_actions: if authorized {
                    vec!["runChecks".into()]
                } else {
                    vec!["readFiles".into()]
                },
                recorded_at: now(),
            },
            ..Default::default()
        };
        let task = core::NewTask {
            id: "task".into(),
            title: "Deliver result".into(),
            project: self.workspace.path().to_string_lossy().into_owned(),
            integration_owner: Some("desktop".into()),
            contract: core::TaskContract {
                goal: "Deliver requested artifact".into(),
                acceptance: vec![criterion],
                ..Default::default()
            },
            ..Default::default()
        };
        execute_sync(
            &self.app,
            "create",
            &Actor {
                id: "desktop".into(),
                role: ActorRole::Coordinator,
            },
            Command::CreateWork {
                intent,
                tasks: vec![task],
            },
        )
        .unwrap();
    }
    async fn dispatch(&self) -> Value {
        serde_json::from_str(
            &execute_tool(
                &self.app,
                "coordination_dispatch",
                &json!({"commandId":"dispatch","taskId":"task","sessionId":"worker"}),
            )
            .await
            .unwrap(),
        )
        .unwrap()
    }
    fn credentials(&self) -> Value {
        let snapshot = snapshot_sync(&self.app).unwrap();
        let attempt = &snapshot.attempts[0];
        json!({"commandId":"ack","attemptId":attempt.id,"epoch":attempt.epoch,"workerToken":worker_token(&self.app,attempt)})
    }
    async fn ack_submit(&self) {
        let mut args = self.credentials();
        execute_tool(&self.app, "coordination_ack", &args)
            .await
            .unwrap();
        args["commandId"] = json!("submit");
        args["result"] = json!({"summary":"Artifact ready","artifacts":[{"id":"artifact","kind":"report","path":"artifact.txt"}]});
        execute_tool(&self.app, "coordination_submit", &args)
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn durable_worker_lifecycle_requires_ack_real_evidence_and_actual_exit() {
    let h = Harness::new();
    h.create(None);
    record_owned_worker(&h.app, "worker", "task").await.unwrap();
    let dispatched = h.dispatch().await;
    assert_eq!(dispatched["acknowledged"], false);
    assert_eq!(
        snapshot_sync(&h.app).unwrap().tasks[0].state,
        core::TaskState::Assigned
    );
    let writes = h.writes.lock().len();
    let duplicate = h.dispatch().await;
    assert_eq!(duplicate["duplicate"], true);
    assert_eq!(h.writes.lock().len(), writes);
    h.ack_submit().await;
    assert!(
        execute_tool(
            &h.app,
            "coordination_accept",
            &json!({"commandId":"accept-early","taskId":"task"})
        )
        .await
        .is_err()
    );
    execute_tool(&h.app,"coordination_verify",&json!({"commandId":"verify","taskId":"task","criterionId":"criterion","artifactId":"artifact"})).await.unwrap();
    let retired: Value = serde_json::from_str(
        &execute_tool(
            &h.app,
            "coordination_retire_worker",
            &json!({"commandId":"retire","taskId":"task"}),
        )
        .await
        .unwrap(),
    )
    .unwrap();
    assert_eq!(retired["candidateHold"], true);
    assert!(
        h.app.summary("worker").is_none(),
        "exit then unregister still proves exit"
    );
    execute_tool(
        &h.app,
        "coordination_accept",
        &json!({"commandId":"accept","taskId":"task"}),
    )
    .await
    .unwrap();
    let snapshot = snapshot_sync(&h.app).unwrap();
    assert_eq!(snapshot.tasks[0].state, core::TaskState::Verified);
    assert!(snapshot.resources.is_empty());
    assert!(
        !serde_json::to_string(&snapshot)
            .unwrap()
            .contains(&h.credentials()["workerToken"].as_str().unwrap().to_owned())
    );
}

#[tokio::test]
async fn stale_artifact_never_produces_trusted_receipt() {
    let h = Harness::new();
    h.create(None);
    h.dispatch().await;
    h.ack_submit().await;
    std::fs::write(
        h.workspace.path().join("artifact.txt"),
        "changed after submission",
    )
    .unwrap();
    assert!(execute_tool(&h.app,"coordination_verify",&json!({"commandId":"verify","taskId":"task","criterionId":"criterion","artifactId":"artifact"})).await.is_err());
    assert!(snapshot_sync(&h.app).unwrap().tasks[0].evidence.is_empty());
}

#[tokio::test]
async fn worker_token_cannot_elevate_or_target_another_session() {
    let h = Harness::new();
    h.create(None);
    h.dispatch().await;
    let mut args = h.credentials();
    args["workerToken"] = json!("wrong");
    assert!(
        execute_tool(&h.app, "coordination_ack", &args)
            .await
            .is_err()
    );
    assert!(
        crate::orchestrator::tools::execute_with_worker_scope(
            &h.app,
            "coordination_snapshot",
            &json!({}),
            None,
            true
        )
        .await
        .is_err()
    );
    assert!(
        execute_sync(
            &h.app,
            "forge",
            &Actor {
                id: "desktop".into(),
                role: ActorRole::Host
            },
            Command::Tick
        )
        .is_err()
    );
    assert!(
        execute_sync(
            &h.app,
            "forge",
            &Actor {
                id: "desktop".into(),
                role: ActorRole::Coordinator
            },
            Command::Verify {
                task_id: "task".into(),
                revision: 1,
                verification: Default::default()
            }
        )
        .is_err()
    );
}

#[tokio::test]
async fn timeout_and_project_self_target_do_not_type() {
    let h = Harness::new();
    {
        let mut inner = h.app.inner.lock();
        let session = inner.sessions.get_mut("worker").unwrap();
        session.summary.kind = Some("orchestrator".into());
        session.summary.agent_activity = Some("working".into());
    }
    assert_eq!(
        crate::orchestrator::tools::wait_until_ready_for(
            &h.app,
            "worker",
            true,
            Duration::from_millis(20)
        )
        .await
        .unwrap(),
        crate::orchestrator::tools::Readiness::TimedOut
    );
    assert!(
        crate::orchestrator::tools::execute_for_session(
            &h.app,
            "send_input",
            &json!({"sessionId":"worker","text":"new assignment"}),
            Some("worker")
        )
        .await
        .is_err()
    );
    assert!(h.writes.lock().is_empty());
}

#[tokio::test]
async fn existing_user_session_cannot_be_retired_automatically() {
    let h = Harness::new();
    h.create(None);
    h.dispatch().await;
    assert!(
        execute_tool(
            &h.app,
            "coordination_retire_worker",
            &json!({"commandId":"retire","taskId":"task"})
        )
        .await
        .unwrap_err()
        .contains("existing user session")
    );
    assert!(h.app.summary("worker").is_some());
    assert!(!snapshot_sync(&h.app).unwrap().resources.is_empty());
}

#[tokio::test]
async fn cancel_retire_uses_original_attempt_epoch() {
    let h = Harness::new();
    h.create(None);
    record_owned_worker(&h.app, "worker", "task").await.unwrap();
    h.dispatch().await;
    execute_sync(
        &h.app,
        "cancel",
        &Actor {
            id: "desktop".into(),
            role: ActorRole::Coordinator,
        },
        Command::Cancel {
            task_id: "task".into(),
            reason: "User cancelled".into(),
        },
    )
    .unwrap();
    execute_tool(
        &h.app,
        "coordination_retire_worker",
        &json!({"commandId":"retire","taskId":"task"}),
    )
    .await
    .unwrap();
    let snapshot = snapshot_sync(&h.app).unwrap();
    assert_eq!(snapshot.tasks[0].state, core::TaskState::Cancelled);
    assert!(snapshot.resources.is_empty());
}

#[test]
fn graph_scope_checks_parent_intent_and_archive() {
    let h = Harness::new();
    h.create(None);
    let another = tempfile::tempdir().unwrap();
    let mut session = crate::model::TerminalSessionSummary::native(
        "coordinator".into(),
        "Project".into(),
        "codex".into(),
        another.path().to_string_lossy().into_owned(),
        22,
        80,
        24,
    );
    session.kind = Some("orchestrator".into());
    h.app.register(session, None, None);
    let actor = Actor {
        id: "coordinator".into(),
        role: ActorRole::Coordinator,
    };
    assert!(
        execute_sync(
            &h.app,
            "archive-other",
            &actor,
            Command::ArchiveIntent {
                intent_id: "intent".into()
            }
        )
        .is_err()
    );
    let child = core::NewTask {
        id: "child".into(),
        project: another.path().to_string_lossy().into_owned(),
        parent_id: Some("task".into()),
        ..Default::default()
    };
    assert!(
        execute_sync(
            &h.app,
            "child-other",
            &actor,
            Command::CreateTasks {
                intent_id: "intent".into(),
                tasks: vec![child]
            }
        )
        .is_err()
    );
}

#[tokio::test]
async fn verification_retry_after_restart_never_runs_check_twice() {
    let h = Harness::new();
    let counter = h._data.path().join("check-count.txt");
    let script = format!(
        "$p = '{}'\n$n = if (Test-Path -LiteralPath $p) {{ [int](Get-Content -LiteralPath $p) }} else {{ 0 }}\nSet-Content -LiteralPath $p -Value ($n + 1)\nWrite-Output 'verified'\n",
        counter.to_string_lossy().replace('\'', "''")
    );
    std::fs::write(h.workspace.path().join("verify.ps1"), script).unwrap();
    h.create(Some(&powershell_check("verify.ps1")));
    h.dispatch().await;
    h.ack_submit().await;
    let args = json!({"commandId":"actual-check","taskId":"task","criterionId":"criterion","timeoutSeconds":30});
    let first: Value = serde_json::from_str(
        &execute_tool(&h.app, "coordination_verify", &args)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(first["verification"]["passed"], true);
    assert_eq!(std::fs::read_to_string(&counter).unwrap().trim(), "1");
    let reopened = AppState::new(
        (*h.app.config).clone(),
        (*h.app.token).clone(),
        Arc::new(|_| {}),
    );
    let repeated: Value = serde_json::from_str(
        &execute_tool(&reopened, "coordination_verify", &args)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(first, repeated);
    assert_eq!(std::fs::read_to_string(&counter).unwrap().trim(), "1");
    let mut changed = args.clone();
    changed["timeoutSeconds"] = json!(31);
    assert!(
        execute_tool(&reopened, "coordination_verify", &changed)
            .await
            .unwrap_err()
            .contains("different verification")
    );
}

#[tokio::test]
async fn unauthorized_check_fails_preflight_without_reserving_operation() {
    let h = Harness::new();
    std::fs::write(
        h.workspace.path().join("verify.ps1"),
        "Write-Output 'verified'",
    )
    .unwrap();
    h.create_authorized(Some("pwsh -NoProfile -File verify.ps1"), false);
    h.dispatch().await;
    h.ack_submit().await;
    assert!(
        execute_tool(
            &h.app,
            "coordination_verify",
            &json!({"commandId":"unauthorized-check","taskId":"task","criterionId":"criterion"})
        )
        .await
        .unwrap_err()
        .contains("authorize")
    );
    let path = h
        .app
        .data_root
        .join("coordination")
        .join("verification-operations")
        .join(format!(
            "{}.json",
            core::digest_bytes(b"unauthorized-check")
        ));
    assert!(!path.exists());
    assert!(snapshot_sync(&h.app).unwrap().tasks[0].evidence.is_empty());
}

#[tokio::test]
async fn supervised_recovery_never_treats_missing_other_host_session_as_stopped() {
    let h = Harness::new();
    h.create(None);
    h.dispatch().await;
    execute_tool(&h.app, "coordination_ack", &h.credentials())
        .await
        .unwrap();
    let before = snapshot_sync(&h.app).unwrap();
    let other = AppState::new(
        (*h.app.config).clone(),
        (*h.app.token).clone(),
        Arc::new(|_| {}),
    );
    let after = supervision::reconcile_at(&other, Some(&before), now() + 400).unwrap();
    assert!(!after.attempts[0].stopped);
    assert!(!after.resources.is_empty());
    assert_eq!(after.tasks[0].state, core::TaskState::Uncertain);
    let restored = supervision::reconcile_at(&h.app, Some(&after), now() + 400).unwrap();
    assert_eq!(restored.attempts.len(), 1);
    assert!(!restored.attempts[0].stopped);
    assert_eq!(restored.tasks[0].state, core::TaskState::Running);
    assert_eq!(restored.attempts[0].id, before.attempts[0].id);
}

#[tokio::test]
async fn supervisor_reconciles_exit_after_unregister_and_skips_noop_ticks() {
    let h = Harness::new();
    h.create(None);
    record_owned_worker(&h.app, "worker", "task").await.unwrap();
    h.dispatch().await;
    h.ack_submit().await;
    h.app.exit("worker", Some(0));
    h.app.unregister("worker");
    let after = reconcile_once(&h.app, None).unwrap();
    assert!(after.attempts[0].stopped);
    assert!(after.resources.iter().any(|r| r.task_id == "task"));
    let cleaned = reconcile_once(&h.app, Some(&after)).unwrap();
    let idle = reconcile_once(&h.app, Some(&cleaned)).unwrap();
    assert_eq!(
        cleaned.seq, idle.seq,
        "quiet supervisor passes must not append journal events"
    );
}

#[tokio::test]
async fn timer_resume_is_addressed_and_does_not_replace_the_attempt() {
    let h = Harness::new();
    h.create(None);
    h.dispatch().await;
    let mut args = h.credentials();
    execute_tool(&h.app, "coordination_ack", &args)
        .await
        .unwrap();
    args["commandId"] = json!("block-timer");
    args["blocker"] = json!({"kind":"timer","message":"Wait for retry window","wakeCondition":"retry-window","retryAt":now()+2});
    execute_tool(&h.app, "coordination_block", &args)
        .await
        .unwrap();
    let blocked = snapshot_sync(&h.app).unwrap();
    h.wakes.lock().clear();
    let resumed = supervision::reconcile_at(&h.app, Some(&blocked), now() + 3).unwrap();
    assert_eq!(resumed.attempts.len(), 1);
    assert_eq!(resumed.tasks[0].attempt_id, blocked.tasks[0].attempt_id);
    assert!(
        h.wakes
            .lock()
            .iter()
            .any(|(_, _, action)| action == "resumeReady")
    );
}

#[tokio::test]
async fn compact_task_query_is_paginated_and_omits_delivery_payloads() {
    let h = Harness::new();
    h.create(None);
    h.dispatch().await;
    let query: Value = serde_json::from_str(
        &execute_tool(&h.app, "coordination_snapshot", &json!({"limit":1}))
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(query["total"], 1);
    assert_eq!(query["tasks"].as_array().unwrap().len(), 1);
    assert!(query["tasks"][0].get("contract").is_none());
    assert!(query.get("deliveries").is_none());
    let empty: Value = serde_json::from_str(
        &execute_tool(
            &h.app,
            "coordination_snapshot",
            &json!({"limit":1,"offset":1}),
        )
        .await
        .unwrap(),
    )
    .unwrap();
    assert!(empty["tasks"].as_array().unwrap().is_empty());
    assert_eq!(empty["total"], 1);
}

#[tokio::test]
async fn accepted_result_delivery_survives_restart_without_a_second_panel_item() {
    let h = Harness::new();
    h.create(None);
    record_owned_worker(&h.app, "worker", "task").await.unwrap();
    h.dispatch().await;
    h.ack_submit().await;
    execute_tool(&h.app,"coordination_verify",&json!({"commandId":"verify","taskId":"task","criterionId":"criterion","artifactId":"artifact"})).await.unwrap();
    execute_tool(
        &h.app,
        "coordination_retire_worker",
        &json!({"commandId":"retire","taskId":"task"}),
    )
    .await
    .unwrap();
    execute_tool(
        &h.app,
        "coordination_accept",
        &json!({"commandId":"accept","taskId":"task"}),
    )
    .await
    .unwrap();
    let count = |app: &AppState| {
        app.orchestrator.status(None, true)["transcript"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|item| item["id"] == "task-result:task:1")
            .count()
    };
    assert_eq!(count(&h.app), 1);
    let restarted = AppState::new(
        (*h.app.config).clone(),
        (*h.app.token).clone(),
        Arc::new(|_| {}),
    );
    supervision::deliver_accepted(&restarted, &snapshot_sync(&restarted).unwrap()).unwrap();
    assert_eq!(count(&restarted), 1);
}

#[tokio::test]
async fn original_panel_request_gets_a_distinct_durable_final_result() {
    let h = Harness::new();
    h.app.orchestrator.set_routed(true);
    h.app.orchestrator.route(
        &h.app,
        "Produce the report".into(),
        crate::MessageContext::default(),
    );
    let panel = h.app.orchestrator.status(None, true)["transcript"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let message = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"coordination_command","arguments":{"commandId":"origin-create","origin":{"originRequestId":"request-1","replyChannel":"panel","replyHandle":panel},"command":{"op":"createWork","intent":{"id":"intent","title":"Report"},"tasks":[{"id":"task","title":"Report","project":h.workspace.path(),"integrationOwner":"desktop","contract":{"goal":"Produce report","acceptance":[{"id":"criterion","description":"Report artifact exists","kind":"artifact"}]}}]}}}});
    let answer = crate::orchestrator::mcp::handle_message(&h.app, &message)
        .await
        .unwrap();
    assert_eq!(answer["result"]["isError"], false, "{answer}");
    h.app
        .orchestrator
        .answer(&panel, "Started the requested report.", true);
    record_owned_worker(&h.app, "worker", "task").await.unwrap();
    h.dispatch().await;
    h.ack_submit().await;
    execute_tool(&h.app,"coordination_verify",&json!({"commandId":"verify","taskId":"task","criterionId":"criterion","artifactId":"artifact"})).await.unwrap();
    execute_tool(
        &h.app,
        "coordination_retire_worker",
        &json!({"commandId":"retire","taskId":"task"}),
    )
    .await
    .unwrap();
    execute_tool(
        &h.app,
        "coordination_accept",
        &json!({"commandId":"accept","taskId":"task"}),
    )
    .await
    .unwrap();
    let status = h.app.orchestrator.status(None, true);
    let items = status["transcript"].as_array().unwrap();
    let initial = items.iter().find(|item| item["id"] == panel).unwrap();
    let final_item = items
        .iter()
        .find(|item| item["id"] == "task-result:task:1")
        .unwrap();
    assert_eq!(initial["text"], "Started the requested report.");
    assert_eq!(final_item["originRequestId"], panel);
    assert_eq!(initial["turnId"], final_item["turnId"]);
}

#[tokio::test]
async fn pending_panel_handle_is_restored_and_answered_after_restart() {
    let h = Harness::new();
    h.app.orchestrator.set_routed(true);
    h.app.orchestrator.route(
        &h.app,
        "Keep working".into(),
        crate::MessageContext {
            tag: Some("original-tag".into()),
            ..Default::default()
        },
    );
    let id = h.app.orchestrator.status(None, true)["transcript"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let restarted = AppState::new(
        (*h.app.config).clone(),
        (*h.app.token).clone(),
        Arc::new(|_| {}),
    );
    restarted
        .orchestrator
        .answer(&id, "Recovered final reply", true);
    let status = restarted.orchestrator.status(None, true);
    assert_eq!(
        status["transcript"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["id"] == id)
            .unwrap()["text"],
        "Recovered final reply"
    );
}

#[tokio::test]
async fn context_omits_and_invalidates_changed_source_memory() {
    let h = Harness::new();
    h.create(None);
    let path = h.workspace.path().join("artifact.txt");
    let (digest, _) = crate::coordination_evidence::hash_file(&path).unwrap();
    let mut task = task_sync(&h.app, "task").unwrap();
    task.contract
        .sources
        .insert(path.to_string_lossy().into_owned(), digest);
    execute_sync(
        &h.app,
        "source-contract",
        &Actor {
            id: "desktop".into(),
            role: ActorRole::Coordinator,
        },
        Command::Steer {
            task_id: "task".into(),
            expected_revision: task.revision,
            contract: task.contract,
        },
    )
    .unwrap();
    execute_tool(&h.app,"coordination_memory",&json!({"commandId":"remember","memoryId":"fact","scope":h.workspace.path(),"text":"Report content was observed","sourcePaths":[path]})).await.unwrap();
    assert_eq!(context_sync(&h.app, "task").unwrap().memory.len(), 1);
    std::fs::write(&path, "Source changed").unwrap();
    assert!(context_sync(&h.app, "task").unwrap().memory.is_empty());
}

#[tokio::test]
async fn intent_only_deadline_is_enforced_without_cancelling_other_intents() {
    let h = Harness::new();
    let at = now();
    let make = |id: &str| core::NewTask {
        id: id.into(),
        title: id.into(),
        project: h.workspace.path().to_string_lossy().into_owned(),
        integration_owner: Some("desktop".into()),
        contract: core::TaskContract {
            goal: id.into(),
            acceptance: vec![core::Criterion {
                id: "criterion".into(),
                description: "Artifact".into(),
                kind: core::CriterionKind::Artifact,
                ..Default::default()
            }],
            ..Default::default()
        },
        ..Default::default()
    };
    let actor = Actor {
        id: "desktop".into(),
        role: ActorRole::Coordinator,
    };
    execute_sync(
        &h.app,
        "deadline-work",
        &actor,
        Command::CreateWork {
            intent: core::NewIntent {
                id: "deadline-intent".into(),
                title: "Deadline".into(),
                budget: core::Budget {
                    deadline: Some(at + 10),
                    ..Default::default()
                },
                ..Default::default()
            },
            tasks: vec![make("deadline-task")],
        },
    )
    .unwrap();
    execute_sync(
        &h.app,
        "unrelated-work",
        &actor,
        Command::CreateWork {
            intent: core::NewIntent {
                id: "other-intent".into(),
                title: "Other".into(),
                ..Default::default()
            },
            tasks: vec![make("other-task")],
        },
    )
    .unwrap();
    let after = supervision::reconcile_at(&h.app, None, at + 11).unwrap();
    let expired = after
        .tasks
        .iter()
        .find(|task| task.id == "deadline-task")
        .unwrap();
    assert!(matches!(
        expired.state,
        core::TaskState::Failed | core::TaskState::Cancelled
    ));
    assert_eq!(
        after
            .tasks
            .iter()
            .find(|task| task.id == "other-task")
            .unwrap()
            .state,
        core::TaskState::Ready
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "Starts a real logged-in Codex CLI and spends provider usage. Run explicitly for the native worker protocol smoke."]
async fn native_codex_worker_protocol_smoke() {
    use std::process::Stdio;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let h = Harness::new();
    h.create(None);
    h.dispatch().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server_app = h.app.clone();
    let server = tokio::spawn(async move { crate::server::serve(listener, server_app).await });
    let ack = h.credentials();
    let mut submit = ack.clone();
    submit["commandId"] = json!("native-submit");
    submit["result"] = json!({"summary":"Native Codex completed the isolated task protocol","artifacts":[{"id":"artifact","kind":"report","path":"artifact.txt"}]});
    let script = h._data.path().join("native-protocol.ps1");
    let script_text = format!(
        "$ErrorActionPreference = 'Stop'\nInvoke-RestMethod -Method Post -Uri 'http://127.0.0.1:{port}/api/coordination/ack' -ContentType 'application/json' -Body @'\n{}\n'@ | ConvertTo-Json -Depth 8\nInvoke-RestMethod -Method Post -Uri 'http://127.0.0.1:{port}/api/coordination/submit' -ContentType 'application/json' -Body @'\n{}\n'@ | ConvertTo-Json -Depth 8\n",
        ack, submit
    );
    std::fs::write(&script, script_text).unwrap();
    let prompt = format!(
        "You are the worker for an isolated Bro integration test, not the user's live project. The requested artifact artifact.txt already exists. Read it, then run this exact PowerShell script once to acknowledge your assignment and submit that real artifact to the isolated bridge: pwsh -NoProfile -File \"{}\" . Do not edit files or use a different Bro endpoint. The script contains this attempt's private capability; do not print or copy its source into your reply. After successful execution, finish with the line 'native protocol complete'.",
        script.display()
    );
    let args = vec![
        "exec".into(),
        "--json".into(),
        "--skip-git-repo-check".into(),
        "--dangerously-bypass-approvals-and-sandbox".into(),
        "--cd".into(),
        h.workspace.path().to_string_lossy().into_owned(),
        "-".into(),
    ];
    let (program, args) = bro_core::launch::program_and_args("codex", args);
    let mut command = tokio::process::Command::new(program);
    command
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut tree = bro_core::proc_tree::ProcessTree::new().unwrap();
    tree.configure_command(command.as_std_mut());
    let mut child = command
        .spawn()
        .expect("A logged-in Codex CLI must be installed for this explicit smoke");
    tree.attach(child.id().unwrap()).unwrap();
    h.app
        .inner
        .lock()
        .sessions
        .get_mut("worker")
        .unwrap()
        .summary
        .pid = child.id();
    record_owned_worker(&h.app, "worker", "task").await.unwrap();
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(prompt.as_bytes()).await.unwrap();
    stdin.shutdown().await.unwrap();
    drop(stdin);
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let output = tokio::spawn(async move {
        let mut bytes = Vec::new();
        stdout
            .take(8 * 1024 * 1024)
            .read_to_end(&mut bytes)
            .await
            .unwrap();
        bytes
    });
    let errors = tokio::spawn(async move {
        let mut bytes = Vec::new();
        stderr
            .take(1024 * 1024)
            .read_to_end(&mut bytes)
            .await
            .unwrap();
        bytes
    });
    let status = tokio::time::timeout(Duration::from_secs(180), child.wait()).await;
    tree.terminate();
    if status.is_err() {
        let _ = child.kill().await;
    }
    let status = status
        .expect("Native Codex worker exceeded the bounded 180 second smoke timeout")
        .unwrap();
    let out = output.await.unwrap();
    let err = errors.await.unwrap();
    std::fs::write(h._data.path().join("native-output.jsonl"), out).unwrap();
    std::fs::write(h._data.path().join("native-stderr.log"), err).unwrap();
    assert!(
        status.success(),
        "Native Codex process failed with {:?}",
        status.code()
    );
    let snapshot = snapshot_sync(&h.app).unwrap();
    assert_eq!(
        snapshot.tasks[0].state,
        core::TaskState::Submitted,
        "native CLI must actually ack and submit through the live HTTP endpoint"
    );
    h.app.exit("worker", status.code());
    h.app.unregister("worker");
    execute_tool(&h.app,"coordination_verify",&json!({"commandId":"native-verify","taskId":"task","criterionId":"criterion","artifactId":"artifact"})).await.unwrap();
    execute_tool(
        &h.app,
        "coordination_retire_worker",
        &json!({"commandId":"native-retire","taskId":"task"}),
    )
    .await
    .unwrap();
    execute_tool(
        &h.app,
        "coordination_accept",
        &json!({"commandId":"native-accept","taskId":"task"}),
    )
    .await
    .unwrap();
    assert_eq!(
        snapshot_sync(&h.app).unwrap().tasks[0].state,
        core::TaskState::Verified
    );
    let _ = h.app.shutdown.send(true);
    let _ = server.await;
}
