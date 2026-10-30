use super::*;

pub(crate) async fn execute_tool(
    app: &AppState,
    name: &str,
    args: &Value,
) -> Result<String, String> {
    let value = match name {
        "coordination_snapshot" | "coordination_ready" => {
            let actor = coordinator(app)?;
            let app = app.clone();
            let args = args.clone();
            let ready = name == "coordination_ready";
            blocking(move || {
                let engine = Engine::open(app.data_root.as_path()).map_err(|e| e.to_string())?;
                let snapshot = engine.snapshot().map_err(|e| e.to_string())?;
                let tasks = if ready {
                    engine
                        .ready_tasks(args.get("adapterId").and_then(Value::as_str), now())
                        .map_err(|e| e.to_string())?
                } else {
                    snapshot.tasks.clone()
                };
                Ok(task_page(&app, &actor, &snapshot, tasks, &args))
            })
            .await?
        }
        "coordination_context" => {
            let task_id = required(args, "taskId")?;
            let actor = coordinator(app)?;
            let app = app.clone();
            blocking(move || {
                check_scope(&app, &actor, &task_sync(&app, &task_id)?)?;
                let packet = context_sync(&app, &task_id)?;
                check_scope(&app, &actor, &packet.task)?;
                serde_json::to_value(packet).map_err(|e| e.to_string())
            })
            .await?
        }
        "coordination_command" => {
            let command_id = required(args, "commandId")?;
            let mut command: Command =
                serde_json::from_value(args.get("command").cloned().ok_or("command is required.")?)
                    .map_err(|e| format!("Invalid coordination command: {e}"))?;
            if let Some(origin) = origin(args)? {
                if let Command::CreateWork { tasks, .. } | Command::CreateTasks { tasks, .. } =
                    &mut command
                {
                    for task in tasks {
                        task.contract.context.push(origin.clone());
                    }
                } else {
                    return Err("origin belongs only on task creation.".into());
                }
            }
            let actor = coordinator(app)?;
            let app = app.clone();
            serde_json::to_value(
                blocking(move || execute_sync(&app, &command_id, &actor, command)).await?,
            )
            .map_err(|e| e.to_string())?
        }
        "coordination_dispatch" => dispatch(app, args).await?,
        "coordination_ack"
        | "coordination_progress"
        | "coordination_block"
        | "coordination_checkpoint"
        | "coordination_usage" => worker_operation(app, name, args).await?,
        "coordination_submit" => submit(app, args).await?,
        "coordination_inspect" => inspect(app, args).await?,
        "coordination_verify" => verify(app, args).await?,
        "coordination_accept" => accept(app, args).await?,
        "coordination_retire_worker" => retire(app, args).await?,
        "coordination_worktree" => {
            let actor = coordinator(app)?;
            let args = args.clone();
            let app = app.clone();
            blocking(move || {
                let request: crate::coordination_worktrees::CreateRequest = serde_json::from_value(
                    args.get("request").cloned().ok_or("request is required.")?,
                )
                .map_err(|e| format!("Invalid worktree request: {e}"))?;
                check_scope(
                    &app,
                    &actor,
                    &core::TaskView {
                        project: request.repository.to_string_lossy().into_owned(),
                        ..Default::default()
                    },
                )?;
                if request.integration_owner != actor.id && actor.id != "desktop" {
                    return Err("The creating coordinator must own worktree integration.".into());
                }
                let record = if args
                    .get("reconcile")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    crate::coordination_worktrees::reconcile(
                        app.data_root.as_path(),
                        &request.task_id,
                    )?
                } else {
                    crate::coordination_worktrees::create(app.data_root.as_path(), &request)?
                };
                serde_json::to_value(record).map_err(|e| e.to_string())
            })
            .await?
        }
        "coordination_worktree_list" => {
            let actor = coordinator(app)?;
            let app = app.clone();
            blocking(move || {
                let records = crate::coordination_worktrees::list(app.data_root.as_path())?
                    .into_iter()
                    .filter(|record| {
                        check_scope(
                            &app,
                            &actor,
                            &core::TaskView {
                                project: record.repository.to_string_lossy().into_owned(),
                                ..Default::default()
                            },
                        )
                        .is_ok()
                    })
                    .collect::<Vec<_>>();
                serde_json::to_value(records).map_err(|e| e.to_string())
            })
            .await?
        }
        "coordination_memory" => {
            let actor = coordinator(app)?;
            let args = args.clone();
            let app = app.clone();
            blocking(move||{
                let scope=required(&args,"scope")?;let scope=Path::new(&scope).canonicalize().map_err(|e|e.to_string())?;
                check_scope(&app,&actor,&core::TaskView{project:scope.to_string_lossy().into_owned(),..Default::default()})?;
                let text=required(&args,"text")?;let mut sources=std::collections::BTreeMap::new();
                for path in args.get("sourcePaths").and_then(Value::as_array).into_iter().flatten(){
                    let path=path.as_str().ok_or("sourcePaths must contain file paths.")?;let path=if Path::new(path).is_absolute(){PathBuf::from(path)}else{scope.join(path)};
                    let path=path.canonicalize().map_err(|e|e.to_string())?;if !path.starts_with(&scope){return Err("Memory sources must be inside their project scope.".into());}
                    let(digest,_)=crate::coordination_evidence::hash_file(&path)?;sources.insert(path.to_string_lossy().into_owned(),digest);
                }
                if sources.is_empty(){return Err("Promoted memory requires actual sourcePaths. Hypotheses belong in a task checkpoint.".into());}
                let memory=core::Memory{id:required(&args,"memoryId")?,scope:scope.to_string_lossy().into_owned(),text,sources,valid:true,created_at:now(),..Default::default()};
                let outcome=host_execute(&app,&required(&args,"commandId")?,Command::PutMemory{memory})?;
                serde_json::to_value(outcome).map_err(|e|e.to_string())
            }).await?
        }
        _ => return Err(format!("Unknown coordination operation: {name}")),
    };
    serde_json::to_string_pretty(&value).map_err(|e| e.to_string())
}

pub(super) fn task_page(
    app: &AppState,
    actor: &Actor,
    snapshot: &Snapshot,
    tasks: Vec<core::TaskView>,
    args: &Value,
) -> Value {
    let project = args.get("project").and_then(Value::as_str);
    let state = args.get("state").and_then(Value::as_str);
    let tasks = tasks
        .into_iter()
        .filter(|task| check_scope(app, actor, task).is_ok())
        .filter(|task| {
            project
                .is_none_or(|p| bro_core::util::same_path(Path::new(p), Path::new(&task.project)))
        })
        .filter(|task| state.is_none_or(|s| s == task.state.label()))
        .collect::<Vec<_>>();
    let offset = args.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize;
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(30)
        .clamp(1, 100) as usize;
    let total = tasks.len();
    let page=tasks.into_iter().skip(offset).take(limit).map(|t|json!({"id":t.id,"intentId":t.intent_id,"title":t.title,"project":t.project,"state":t.state,"revision":t.revision,"epoch":t.epoch,"attemptId":t.attempt_id,"sessionId":t.session_id,"blockedBy":t.blocked_by,"blocker":t.blocker,"nextAction":t.next_action,"progress":t.progress,"artifactCount":t.artifacts.len(),"evidenceCount":t.evidence.len(),"updatedAt":t.updated_at})).collect::<Vec<_>>();
    json!({"seq":snapshot.seq,"tasks":page,"total":total,"offset":offset,"limit":limit,"nextOffset":if offset+limit<total{Some(offset+limit)}else{None},"metrics":snapshot.metrics,"policy":snapshot.policy,"note":"Task summaries are paginated. Use coordination_context for one task's contract and recovery details."})
}

pub(crate) async fn http_snapshot(
    axum::extract::State(app): axum::extract::State<AppState>,
    headers: axum::http::HeaderMap,
) -> axum::response::Response {
    http_operation(app, headers, "coordination_snapshot", json!({})).await
}

pub(crate) async fn http_command(
    axum::extract::State(app): axum::extract::State<AppState>,
    headers: axum::http::HeaderMap,
    axum::Json(body): axum::Json<Value>,
) -> axum::response::Response {
    http_operation(app, headers, "coordination_command", body).await
}

pub(crate) async fn http_tool(
    axum::extract::Path(operation): axum::extract::Path<String>,
    axum::extract::State(app): axum::extract::State<AppState>,
    headers: axum::http::HeaderMap,
    axum::Json(body): axum::Json<Value>,
) -> axum::response::Response {
    let operation = if operation.starts_with("coordination_") {
        operation
    } else {
        format!("coordination_{}", operation)
    };
    http_operation(app, headers, &operation, body).await
}

pub(super) async fn http_operation(
    app: AppState,
    headers: axum::http::HeaderMap,
    name: &str,
    mut body: Value,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    if let Some(token) = headers
        .get("x-bro-task-token")
        .and_then(|v| v.to_str().ok())
    {
        body["workerToken"] = json!(token);
    }
    let caller = headers
        .get("x-bro-session-id")
        .and_then(|v| v.to_str().ok());
    let scoped = body.get("workerToken").is_some();
    match crate::orchestrator::tools::execute_with_worker_scope(&app, name, &body, caller, scoped)
        .await
    {
        Ok(text) => {
            axum::Json(serde_json::from_str::<Value>(&text).unwrap_or(json!({"text":text})))
                .into_response()
        }
        Err(message) => (
            axum::http::StatusCode::BAD_REQUEST,
            axum::Json(json!({"error":{"code":"coordinationRejected","message":message}})),
        )
            .into_response(),
    }
}

pub(crate) fn definitions() -> Vec<Value> {
    use crate::orchestrator::tools::tool;
    let command = json!({"commandId":{"type":"string","description":"Stable operation ID; reuse only for the same operation."}});
    let task = json!({"commandId":{"type":"string"},"taskId":{"type":"string"}});
    let worker = json!({"commandId":{"type":"string"},"attemptId":{"type":"string"},"epoch":{"type":"integer"},"workerToken":{"type":"string","description":"Private capability from the assignment packet. Never include it in reports."}});
    let mut defs = vec![
        tool(
            "coordination_snapshot",
            "Paginated durable task summaries and metrics. Optional project/state/offset/limit(max100); nextOffset identifies more results. Use context for a task's contract. Terminal idle does not mean task acceptance.",
            json!({"project":{"type":"string"},"state":{"type":"string"},"offset":{"type":"integer"},"limit":{"type":"integer"}}),
            &[],
        ),
        tool(
            "coordination_ready",
            "Ready tasks ranked by dependency/resource/budget admission, project fairness, completion lane and priority aging. Optional adapterId filters required capabilities. Use this order for scheduling; snapshot order is not a scheduling policy.",
            json!({"adapterId":{"type":"string"},"project":{"type":"string"},"offset":{"type":"integer"},"limit":{"type":"integer"}}),
            &[],
        ),
        tool(
            "coordination_context",
            "Recovery context for a task: intent, current contract, verified dependencies, checkpoint and source-backed memory.",
            json!({"taskId":{"type":"string"}}),
            &["taskId"],
        ),
        tool(
            "coordination_command",
            "Create or steer durable work through the file-locked authority. command is a tagged object with op: createWork, createIntent, createTasks, steer, cancel, retry, wake, setPolicy or archiveIntent. CreateWork takes intent and tasks atomically. Tasks need id,title,project,workspace,contract{goal,instructions,acceptance[{id,description,kind:check|review|artifact,command,required}]},resources and dependencies. Checks require exact commands and intent.authorization.authorizedActions. Claim, delivery, result, verification and acceptance use their dedicated runtime tools.",
            {
                let mut p = command.clone();
                p["command"] = json!({"type":"object","description":"A core coordination Command tagged with op. Unknown fields are rejected."});
                p["origin"] = json!({"type":"object","description":"Exact origin JSON from the request envelope: originRequestId, replyChannel, replyHandle."});
                p
            },
            &["commandId", "command"],
        ),
        tool(
            "coordination_dispatch",
            "Atomically claim a ready task and persist delivery before typing its context into an idle worker. Requires taskId, sessionId, stable commandId. A successful paste stays dispatching until the worker acknowledges; duplicate calls never paste again.",
            {
                let mut p = task.clone();
                p["sessionId"] = json!({"type":"string"});
                p
            },
            &["commandId", "taskId", "sessionId"],
        ),
        tool(
            "coordination_ack",
            "Assigned worker: acknowledge the exact durable assignment using its private token, attemptId and epoch.",
            worker.clone(),
            &["commandId", "attemptId", "epoch", "workerToken"],
        ),
        tool(
            "coordination_inspect",
            "Coordinator: read the actual submitted artifacts and contract, and obtain a reviewToken bound to their current bytes. Review actual behavior before passing a review criterion.",
            json!({"taskId":{"type":"string"}}),
            &["taskId"],
        ),
        tool(
            "coordination_verify",
            "Coordinator: verify one required criterion against current candidate bytes. Check commands run only from the explicit authorized contract with bounded time/output; artifact checks hash real files. Review requires coordination_inspect's reviewToken, decision pass|fail and concrete findings. Records a runtime receipt and reviewable output log; workers cannot assert a passing receipt.",
            {
                let mut p = task.clone();
                p["criterionId"] = json!({"type":"string"});
                p["artifactId"] = json!({"type":"string"});
                p["timeoutSeconds"] = json!({"type":"integer"});
                p["reviewToken"] = json!({"type":"string"});
                p["decision"] = json!({"type":"string","enum":["pass","fail"]});
                p["findings"] = json!({"type":"string"});
                p
            },
            &["commandId", "taskId", "criterionId"],
        ),
        tool(
            "coordination_accept",
            "Coordinator: recapture candidate bytes and accept only current runtime evidence for every required criterion. Retained resource claims require safely retiring the worker.",
            task.clone(),
            &["commandId", "taskId"],
        ),
        tool(
            "coordination_retire_worker",
            "Stop a worker created for this coordinated task, wait for its actual host process-tree exit, then release claims. Existing user sessions and coordinators are never stopped automatically; absent or disconnected workers do not prove exit.",
            task.clone(),
            &["commandId", "taskId"],
        ),
        tool(
            "coordination_worktree",
            "Provision an optional task-owned Git worktree before createWork, preserving the user's dirty working tree. request requires taskId,repository,branch,base,integrationOwner. Returns workspace and integration resource identities. reconcile=true explicitly observes an uncertain existing provision without rerunning Git. No merge or deletion.",
            json!({"request":{"type":"object"},"reconcile":{"type":"boolean"}}),
            &["request"],
        ),
        tool(
            "coordination_worktree_list",
            "List task-owned worktrees and their provisioning state, branch/base and integration ownership in the coordinator's scope.",
            json!({}),
            &[],
        ),
        tool(
            "coordination_memory",
            "Coordinator: promote a project fact with provenance. Requires commandId,memoryId,scope,text,sourcePaths. Bro hashes actual source bytes; source changes invalidate memory. Keep unverified hypotheses in task checkpoints.",
            json!({"commandId":{"type":"string"},"memoryId":{"type":"string"},"scope":{"type":"string"},"text":{"type":"string"},"sourcePaths":{"type":"array","items":{"type":"string"}}}),
            &["commandId", "memoryId", "scope", "text", "sourcePaths"],
        ),
    ];
    for (name, description, field) in [
        (
            "coordination_progress",
            "Assigned worker: record meaningful progress or a checkpoint; liveness alone is not progress.",
            "message",
        ),
        (
            "coordination_block",
            "Assigned worker: record blocker {kind,message,wakeCondition,retryAt,requiresUser} without losing task ownership.",
            "blocker",
        ),
        (
            "coordination_checkpoint",
            "Assigned worker: save checkpoint {summary,nextSteps,sources} for compaction/replacement. IDs and revision are bound by the bridge.",
            "checkpoint",
        ),
        (
            "coordination_submit",
            "Assigned worker: submit result {summary,artifacts:[{id,kind,path}],unresolved,checkpoint}. The bridge hashes actual workspace and artifact bytes; supplied digest/check-passed claims are not trusted.",
            "result",
        ),
    ] {
        let mut p = worker.clone();
        p[field] = json!({"type":if field=="message"{"string"}else{"object"}});
        defs.push(tool(
            name,
            description,
            p,
            &["commandId", "attemptId", "epoch", "workerToken", field],
        ));
    }
    let mut usage = worker.clone();
    usage["tokens"] = json!({"type":"integer"});
    usage["costUsd"] = json!({"type":"number"});
    defs.push(tool("coordination_usage","Assigned worker: report observed token/cost usage for its attempt. Metrics remain reported usage, not an authoritative provider bill; omitted values stay unknown.",usage,&["commandId","attemptId","epoch","workerToken"]));
    defs
}
