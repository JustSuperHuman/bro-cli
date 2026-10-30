use super::*;

pub(super) async fn dispatch(app: &AppState, args: &Value) -> Result<Value, String> {
    let actor = coordinator(app)?;
    let command_id = required(args, "commandId")?;
    let task_id = required(args, "taskId")?;
    let session_id = crate::orchestrator::tools::resolve_session(app, args)?;
    if actor.id == session_id {
        return Err("A coordinator cannot dispatch work to its own terminal.".into());
    }
    let session = app
        .summary(&session_id)
        .ok_or("Worker session is not registered on this host.")?;
    if session.status != "running"
        || matches!(
            session.agent_activity.as_deref(),
            Some("working" | "awaiting")
        )
    {
        return Err(
            "The worker is busy, blocked or exited. Wait for readiness before claiming a new task."
                .into(),
        );
    }
    let expected_agent = session.agent.is_some();
    match crate::orchestrator::tools::wait_until_ready_for(
        app,
        &session_id,
        expected_agent,
        Duration::from_secs(4),
    )
    .await?
    {
        crate::orchestrator::tools::Readiness::Ready => {}
        state => {
            return Err(format!(
                "Worker readiness is {state:?}. No task was claimed or typed."
            ));
        }
    }
    let app2 = app.clone();
    let actor2 = actor.clone();
    let session2 = session_id.clone();
    let command2 = command_id.clone();
    let (outcome, packet, outbox_id) = blocking(move || {
        let engine = Engine::open(app2.data_root.as_path()).map_err(|e| e.to_string())?;
        let task = engine.task(&task_id).map_err(|e| e.to_string())?.ok_or("Unknown task.")?;
        check_scope(&app2, &actor2, &task)?;
        if !bro_core::util::same_path(&workspace(&task)?, Path::new(&session.cwd)) {
            return Err("Worker directory differs from the task workspace. Open the worker in that exact workspace.".into());
        }
        let adapter=adapter(&session);
        host_execute(&app2,&format!("adapter-{}-{}",session.id,uuid::Uuid::new_v4().simple()),Command::RegisterAdapter{adapter:adapter.clone()})?;
        let outcome = engine.execute(&command2, &actor2, now(), Command::Claim { task_id: task.id, session_id: session2, adapter_id: Some(adapter.id), lease_seconds: 0 }).map_err(|e| e.to_string())?;
        let outbox_id = outcome.outbox_id.clone().ok_or("Claim did not produce a delivery.")?;
        if outcome.duplicate { return Ok((outcome, None, outbox_id)); }
        let snapshot = engine.snapshot().map_err(|e| e.to_string())?;
        let attempt = snapshot.attempts.iter().find(|a| Some(&a.id) == outcome.attempt_id.as_ref()).ok_or("Assigned attempt disappeared.")?;
        let delivery = snapshot.deliveries.iter().find(|d| d.id == outbox_id).ok_or("Delivery disappeared.")?;
        let token = worker_token(&app2, attempt);
        let packet = format!("[bro-task {} attempt={} epoch={} revision={}]\nYou are assigned this durable task. First acknowledge receipt through bro ctl coordination_ack with commandId ack-{}, attemptId {}, epoch {}, workerToken (the token below). Terminal idle never marks this task complete. Report meaningful checkpoints with coordination_progress or coordination_checkpoint; submit your actual artifacts with coordination_submit. Use the exact attempt and epoch on every call. Do not claim success from your own verification statement; the bridge records actual checks and coordinator review. Preserve the user's constraints and AGENTS.md instructions.\nworkerToken={}\n{}\nExample acknowledgement JSON: {}", attempt.task_id, attempt.id, attempt.epoch, attempt.revision, attempt.id, attempt.id, attempt.epoch, token, delivery.message,
            json!({"commandId":format!("ack-{}",attempt.id),"attemptId":attempt.id,"epoch":attempt.epoch,"workerToken":token}));
        let begun = host_execute(&app2, &format!("{}-begin", command2), Command::BeginDelivery { outbox_id: outbox_id.clone(), consumer_id: format!("host-{}", std::process::id()) })?;
        if begun.duplicate { return Ok((outcome, None, outbox_id)); }
        Ok((outcome, Some(packet), outbox_id))
    }).await?;
    let Some(packet) = packet else {
        return Ok(
            json!({"outcome":outcome,"taskSent":false,"duplicate":true,"note":"This dispatch was already recorded. Inspect its delivery state; it was not sent again."}),
        );
    };
    if let Err(error) = crate::orchestrator::tools::type_text(app, &session_id, &packet, true).await
    {
        let app2 = app.clone();
        let outbox2 = outbox_id.clone();
        let why = error.clone();
        blocking(move || {
            host_execute(
                &app2,
                &format!("uncertain-{outbox2}"),
                Command::DeliveryUncertain {
                    outbox_id: outbox2,
                    reason: why,
                },
            )
        })
        .await?;
        return Err(format!(
            "Dispatch outcome is uncertain: {error}. The task remains claimed; inspect it instead of resending."
        ));
    }
    // PTY receipt proves byte delivery only. The worker's capability-bearing ack completes delivery.
    Ok(
        json!({"outcome":outcome,"taskSent":true,"acknowledged":false,"deliveryState":"dispatching","note":"Task bytes were delivered. Await the worker acknowledgement; do not resend or infer acceptance from terminal activity."}),
    )
}

pub(super) async fn worker_operation(
    app: &AppState,
    name: &str,
    args: &Value,
) -> Result<Value, String> {
    let caller = crate::orchestrator::tools::caller_session();
    let app = app.clone();
    let args = args.clone();
    let name = name.to_string();
    blocking(move || {
        let engine = Engine::open(app.data_root.as_path()).map_err(|e| e.to_string())?;
        let before = engine.snapshot().map_err(|e| e.to_string())?;
        let (actor, attempt) = worker(&app, &before, &args, caller.as_deref())?;
        let command_id = required(&args, "commandId")?;
        let outcome = if name == "coordination_ack" {
            let delivery = before
                .deliveries
                .iter()
                .find(|d| d.attempt_id == attempt.id)
                .ok_or("Attempt has no delivery.")?;
            host_execute(
                &app,
                &command_id,
                Command::AckDelivery {
                    outbox_id: delivery.id.clone(),
                },
            )?
        } else {
            let command = match name.as_str() {
                "coordination_progress" => Command::Progress {
                    attempt_id: attempt.id.clone(),
                    epoch: attempt.epoch,
                    message: required(&args, "message")?,
                    checkpoint: args
                        .get("checkpoint")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                },
                "coordination_block" => Command::Block {
                    attempt_id: attempt.id.clone(),
                    epoch: attempt.epoch,
                    blocker: serde_json::from_value(
                        args.get("blocker").cloned().ok_or("blocker is required.")?,
                    )
                    .map_err(|e| format!("Invalid blocker: {e}"))?,
                },
                "coordination_checkpoint" => {
                    let mut checkpoint: core::Checkpoint = serde_json::from_value(
                        args.get("checkpoint")
                            .cloned()
                            .ok_or("checkpoint is required.")?,
                    )
                    .map_err(|e| format!("Invalid checkpoint: {e}"))?;
                    checkpoint.attempt_id = attempt.id.clone();
                    checkpoint.task_id = attempt.task_id.clone();
                    checkpoint.epoch = attempt.epoch;
                    checkpoint.revision = attempt.revision;
                    checkpoint.at = now();
                    Command::SaveCheckpoint { checkpoint }
                }
                "coordination_usage" => Command::RecordCost {
                    attempt_id: attempt.id.clone(),
                    tokens: args.get("tokens").and_then(Value::as_u64),
                    cost_usd: args.get("costUsd").and_then(Value::as_f64),
                },
                _ => return Err("Unknown worker operation.".into()),
            };
            engine
                .execute(&command_id, &actor, now(), command)
                .map_err(|e| e.to_string())?
        };
        publish(&app, &outcome);
        wake_changed(
            &app,
            &before,
            &engine.snapshot().map_err(|e| e.to_string())?,
        );
        serde_json::to_value(outcome).map_err(|e| e.to_string())
    })
    .await
}

pub(super) async fn submit(app: &AppState, args: &Value) -> Result<Value, String> {
    let caller = crate::orchestrator::tools::caller_session();
    let app = app.clone();
    let args = args.clone();
    blocking(move || {
        let engine = Engine::open(app.data_root.as_path()).map_err(|e| e.to_string())?;
        let before = engine.snapshot().map_err(|e| e.to_string())?;
        let (actor, attempt) = worker(&app, &before, &args, caller.as_deref())?;
        let task = before
            .tasks
            .iter()
            .find(|t| t.id == attempt.task_id)
            .ok_or("Task disappeared.")?;
        let mut result: core::TaskResult =
            serde_json::from_value(args.get("result").cloned().ok_or("result is required.")?)
                .map_err(|e| format!("Invalid result: {e}"))?;
        let captured = crate::coordination_evidence::capture(&workspace(task)?, &result.artifacts)?;
        for (source, expected) in &task.contract.sources {
            if captured.sources.get(source).is_some_and(|actual|actual!=expected){
                return Err(format!("Contract source changed before submission: {source}. Stop and revise the old attempt before submitting new evidence."));
            }
        }
        result.digest = captured.digest;
        result.artifacts = captured.artifacts;
        let outcome = engine
            .execute(
                &required(&args, "commandId")?,
                &actor,
                now(),
                Command::SubmitResult {
                    attempt_id: attempt.id,
                    epoch: attempt.epoch,
                    result,
                },
            )
            .map_err(|e| e.to_string())?;
        publish(&app, &outcome);
        wake_changed(
            &app,
            &before,
            &engine.snapshot().map_err(|e| e.to_string())?,
        );
        serde_json::to_value(outcome).map_err(|e| e.to_string())
    })
    .await
}
