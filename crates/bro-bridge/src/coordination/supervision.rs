use super::*;

pub(crate) fn spawn_supervisor(app: &AppState) {
    let app = app.clone();
    tokio::spawn(async move {
        let mut shutdown = app.shutdown.subscribe();
        let mut previous: Option<Snapshot> = None;
        loop {
            if *app.shutdown.borrow() {
                break;
            }
            let app2 = app.clone();
            let before = previous.clone();
            let reconciled = blocking(move || reconcile_once(&app2, before.as_ref())).await;
            match reconciled {
                Ok(snapshot) => previous = Some(snapshot),
                Err(error) => tracing::warn!("coordination supervisor: {error}"),
            };
            tokio::select! {_=tokio::time::sleep(Duration::from_secs(10))=>{},_=shutdown.changed()=>break}
        }
    });
}

pub(super) fn reconcile_once(
    app: &AppState,
    before: Option<&Snapshot>,
) -> Result<Snapshot, String> {
    reconcile_at(app, before, now())
}

pub(super) fn reconcile_at(
    app: &AppState,
    before: Option<&Snapshot>,
    observed_at: i64,
) -> Result<Snapshot, String> {
    let host_execute =
        |app: &AppState, id: &str, command: Command| host_execute_at(app, id, observed_at, command);
    let engine = Engine::open(app.data_root.as_path()).map_err(|e| e.to_string())?;
    let initial = engine.snapshot().map_err(|e| e.to_string())?;
    let sessions = app.summaries();
    for session in &sessions {
        if session.status != "running" {
            continue;
        }
        let candidate = adapter(session);
        if initial
            .adapters
            .iter()
            .find(|a| a.id == candidate.id)
            .is_none_or(|old| {
                old.available != candidate.available
                    || old.harness != candidate.harness
                    || old.capabilities != candidate.capabilities
            })
        {
            host_execute(
                &app,
                &format!(
                    "observe-adapter-{}-{}",
                    session.id,
                    uuid::Uuid::new_v4().simple()
                ),
                Command::RegisterAdapter { adapter: candidate },
            )?;
        }
    }
    let exits = app.coordination_exits.lock().clone();
    for observed in &initial.adapters {
        if let Some(id) = observed.id.strip_prefix("session-") {
            if exits.contains_key(id)
                && !initial
                    .attempts
                    .iter()
                    .any(|a| a.adapter_id.as_deref() == Some(&observed.id) && !a.stopped)
            {
                let _ = host_execute(
                    &app,
                    &format!("remove-adapter-{}", observed.id),
                    Command::RemoveAdapter {
                        adapter_id: observed.id.clone(),
                    },
                );
            }
        }
    }
    for attempt in initial.attempts.iter().filter(|a| !a.stopped) {
        if let Some((pid, created, at)) = exits.get(&attempt.session_id) {
            let provenance = std::fs::read(owned_worker_path(&app, &attempt.session_id))
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
            let matches = provenance.is_some_and(|value| {
                value["taskId"] == attempt.task_id
                    && value["pid"] == json!(pid)
                    && value["createdAt"] == *created
            }) || sessions
                .iter()
                .any(|s| s.id == attempt.session_id && s.pid == *pid && s.created_at == *created);
            if matches {
                let _ = host_execute(
                    &app,
                    &format!("exit-{}-{}", attempt.id, at),
                    Command::ObserveStopped {
                        attempt_id: attempt.id.clone(),
                        epoch: attempt.epoch,
                        proof: core::StopProof {
                            kind: core::StopKind::ProcessExited,
                            reference: format!(
                                "host process-tree exit session {} pid {:?} created {}",
                                attempt.session_id, pid, created
                            ),
                            observed_at: *at,
                        },
                    },
                );
            }
        } else if let Some(session) = sessions
            .iter()
            .find(|s| s.id == attempt.session_id && s.status == "running" && s.pid.is_some())
        {
            // A registered, host-owned process is liveness, not useful progress. Never infer death from absence.
            if attempt.lease_until < observed_at + 90 {
                let _ = host_execute(
                    &app,
                    &format!("renew-{}-{}", attempt.id, observed_at),
                    Command::RenewAttempt {
                        attempt_id: attempt.id.clone(),
                        epoch: attempt.epoch,
                        session_id: session.id.clone(),
                        lease_seconds: 0,
                        observation: format!(
                            "host registry owns running process {:?} created {}",
                            session.pid, session.created_at
                        ),
                    },
                );
            }
        }
    }
    let current = engine.snapshot().map_err(|e| e.to_string())?;
    let due = current.attempts.iter().any(|a| {
        !a.stopped && a.lease_until < observed_at && a.state != core::AttemptState::Uncertain
    }) || current.tasks.iter().any(|t| {
        !matches!(
            t.state,
            core::TaskState::Verified
                | core::TaskState::Cancelled
                | core::TaskState::Stopped
                | core::TaskState::Failed
                | core::TaskState::CancelRequested
        ) && (t.budget.deadline.is_some_and(|at| at <= observed_at)
            || current
                .intents
                .iter()
                .find(|i| i.id == t.intent_id)
                .is_some_and(|i| i.budget.deadline.is_some_and(|at| at <= observed_at))
            || t.blocker.as_ref().is_some_and(|b| {
                !b.requires_user && b.retry_at.is_some_and(|at| at <= observed_at)
            }))
    });
    if due {
        host_execute(
            &app,
            &format!("tick-{}-{}", std::process::id(), observed_at),
            Command::Tick,
        )?;
    }
    let after = engine.snapshot().map_err(|e| e.to_string())?;
    deliver_accepted(&app, &after)?;
    for task in &after.tasks {
        if !matches!(
            task.state,
            core::TaskState::Running | core::TaskState::Assigned
        ) {
            continue;
        }
        let progress = after.events.iter().rev().find(|e| {
            e.task_id.as_deref() == Some(&task.id)
                && matches!(
                    e.kind.as_str(),
                    "progress" | "checkpointSaved" | "deliveryAcknowledged" | "attemptAssigned"
                )
        });
        let at = progress.map(|e| e.at).unwrap_or(task.created_at);
        let seq = progress.map(|e| e.seq).unwrap_or(0);
        if observed_at.saturating_sub(at) >= 600 {
            wake(&app, task, seq, "inspectProgress");
        }
    }
    wake_changed(&app, before.unwrap_or(&Snapshot::default()), &after);
    Ok(after)
}

pub(super) fn deliver_accepted(app: &AppState, snapshot: &Snapshot) -> Result<(), String> {
    use std::io::Write;
    let directory = app.data_root.join("coordination").join("result-deliveries");
    std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(directory.join("authority.lock"))
        .map_err(|e| e.to_string())?;
    lock.lock().map_err(|e| e.to_string())?;
    for task in snapshot
        .tasks
        .iter()
        .filter(|t| t.state == core::TaskState::Verified)
    {
        let Some(result) = &task.result else { continue };
        let id = format!("task-result-{}-{}", task.id, task.revision);
        let path = directory.join(format!("{}.json", core::digest_bytes(id.as_bytes())));
        std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
        let origin = task
            .contract
            .context
            .iter()
            .filter_map(|text| serde_json::from_str::<Value>(text).ok())
            .find_map(|value| {
                value
                    .get("broRequestOrigin")
                    .cloned()
                    .or_else(|| value.get("originRequestId").is_some().then_some(value))
            })
            .map(serde_json::from_value::<RequestOrigin>)
            .transpose()
            .map_err(|e| e.to_string())?;
        let text = format!("Task complete: {}. {}", task.title, result.summary);
        let mut record = json!({"id":id,"taskId":task.id,"revision":task.revision,"digest":result.digest,"origin":origin,"text":text,"state":"pending"});
        match std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
        {
            Ok(mut file) => {
                file.write_all(&serde_json::to_vec(&record).map_err(|e| e.to_string())?)
                    .and_then(|_| file.sync_all())
                    .map_err(|e| e.to_string())?;
                #[cfg(unix)]
                std::fs::File::open(&directory)
                    .and_then(|dir| dir.sync_all())
                    .map_err(|e| e.to_string())?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                record = serde_json::from_slice(&std::fs::read(&path).map_err(|e| e.to_string())?)
                    .map_err(|e| format!("Result delivery record needs reconciliation: {e}"))?;
            }
            Err(error) => return Err(error.to_string()),
        }
        if record["digest"] != result.digest {
            return Err(
                "Accepted task delivery content differs from its persisted receipt.".into(),
            );
        }
        if record["state"] == "delivered" {
            continue;
        }
        let handle = origin
            .as_ref()
            .and_then(|origin| origin.reply_handle.as_deref())
            .or_else(|| origin.as_ref().map(|origin| origin.request_id.as_str()))
            .unwrap_or("");
        // The transcript uses a deterministic item id, so replay after a crash cannot append twice.
        if !app
            .orchestrator
            .task_completion(handle, &task.id, task.revision, &text)
        {
            return Err("Accepted result remains pending because its durable panel history could not be written. Delivery will retry.".into());
        }
        app.notify(
            crate::model::TerminalNotification {
                id: id.clone(),
                at: crate::model::iso_now(),
                origin: "coordination".into(),
                session_id: None,
                session_title: None,
                title: Some("Task complete".into()),
                body: Some(format!(
                    "{} Open Tasks for the accepted artifacts and verification.",
                    task.title
                )),
                sound: None,
            },
            false,
        );
        record["state"] = json!("delivered");
        record["deliveredAt"] = json!(now());
        crate::project_store::write_atomic(
            &path,
            &serde_json::to_vec(&record).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}
