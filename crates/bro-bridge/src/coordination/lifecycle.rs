use super::*;

pub(super) fn owned_worker_path(app: &AppState, session: &str) -> PathBuf {
    app.data_root
        .join("coordination")
        .join("workers")
        .join(format!("{}.json", core::digest_bytes(session.as_bytes())))
}

pub(crate) async fn record_owned_worker(
    app: &AppState,
    session: &str,
    task_id: &str,
) -> Result<(), String> {
    let session = app.summary(session).ok_or("Worker is not registered.")?;
    let task_id = task_id.to_owned();
    let app = app.clone();
    blocking(move||{
        let path=owned_worker_path(&app,&session.id);std::fs::create_dir_all(path.parent().unwrap()).map_err(|e|e.to_string())?;
        let body=json!({"sessionId":session.id,"taskId":task_id,"pid":session.pid,"createdAt":session.created_at,"hostPid":std::process::id()});
        crate::project_store::write_atomic(&path,&serde_json::to_vec(&body).map_err(|e|e.to_string())?).map_err(|e|e.to_string())
    }).await
}

/// Backward-compatible open_session(task) is an actual durable delegation.
pub(crate) async fn prepare_assignment(
    app: &AppState,
    cwd: &str,
    args: &Value,
) -> Result<Option<core::TaskView>, String> {
    let actor = coordinator(app)?;
    if let Some(task_id) = args.get("taskId").and_then(Value::as_str) {
        let app = app.clone();
        let task_id = task_id.to_owned();
        return blocking(move || {
            let task = task_sync(&app, &task_id)?;
            check_scope(&app, &actor, &task)?;
            Ok(Some(task))
        })
        .await;
    }
    let Some(instructions) = args
        .get("task")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .map(str::to_owned)
    else {
        return Ok(None);
    };
    let app = app.clone();
    let cwd = cwd.to_owned();
    let args = args.clone();
    blocking(move||{
        let canonical=Path::new(&cwd).canonicalize().map_err(|e|format!("Project directory unavailable: {e}"))?;
        let project=canonical.to_string_lossy().into_owned();
        let title=instructions.lines().next().unwrap_or("Delegated work").chars().take(100).collect::<String>();
        let task_id=format!("task-{}",uuid::Uuid::new_v4().simple());let intent_id=format!("intent-{}",uuid::Uuid::new_v4().simple());
        let mut contract:core::TaskContract=match args.get("taskContract"){
            Some(value)=>serde_json::from_value(value.clone()).map_err(|e|format!("Invalid taskContract: {e}"))?,
            None=>core::TaskContract{goal:title.clone(),instructions:instructions.clone(),acceptance:vec![core::Criterion{id:"outcome-review".into(),description:"The requested outcome is present, applicable repository instructions were satisfied, and the actual artifact or behavior was reviewed. Identify tests, build/install/deployment receipts and remaining limitations where required.".into(),kind:core::CriterionKind::Review,..Default::default()}],..Default::default()},
        };
        if contract.instructions.trim().is_empty(){contract.instructions=instructions.clone();}
        if let Some(origin)=origin(&args)?{contract.context.push(origin);}
        let mut policies=Vec::new();let mut parents=canonical.ancestors().collect::<Vec<_>>();parents.reverse();
        for dir in parents{let path=dir.join("AGENTS.md");if path.is_file(){
            let (digest,bytes)=crate::coordination_evidence::hash_file(&path)?;
            if bytes>65536{return Err(format!("Repository instructions are too large to capture: {}",path.display()));}
            let key=path.to_string_lossy().into_owned();contract.sources.insert(key.clone(),digest);policies.push(key);
        }}
        if !policies.is_empty(){contract.context.push(format!("Read and satisfy repository instructions at: {}",policies.join(", ")));}
        let read_only=args.get("readOnly").and_then(Value::as_bool).unwrap_or(false);
        let task=core::NewTask{id:task_id.clone(),title,project:project.clone(),workspace:Some(project.clone()),integration_owner:Some(actor.id.clone()),contract,resources:vec![core::ResourceClaim{key:format!("workspace:{}",bro_core::util::path_key(&canonical)),mode:if read_only{core::ClaimMode::Read}else{core::ClaimMode::Exclusive}}],..Default::default()};
        let authorization=core::Authorization{scope:project,source:"User-directed delegation through open_session; coordinator retains inherited authorization limits.".into(),authorized_actions:if read_only{vec!["readFiles".into(),"runChecks".into()]}else{vec!["readFiles".into(),"writeFiles".into(),"runChecks".into()]},recorded_at:now()};
        let intent=core::NewIntent{id:intent_id,title:task.title.clone(),description:instructions,authorization,..Default::default()};
        execute_sync(&app,&format!("create-{task_id}"),&actor,Command::CreateWork{intent,tasks:vec![task]})?;
        Ok(Some(task_sync(&app,&task_id)?))
    }).await
}

pub(super) async fn retire(app: &AppState, args: &Value) -> Result<Value, String> {
    let actor = coordinator(app)?;
    let task_id = required(args, "taskId")?;
    let command_id = required(args, "commandId")?;
    let app2 = app.clone();
    let (task,session)=blocking(move||{
        let task=task_sync(&app2,&task_id)?;check_scope(&app2,&actor,&task)?;
        let id=task.session_id.as_deref().ok_or("Task has no assigned worker.")?;
        let provenance:Value=serde_json::from_slice(&std::fs::read(owned_worker_path(&app2,id)).map_err(|_|"This is an existing user session, not a task-created worker. It will not be stopped automatically.")?).map_err(|e|e.to_string())?;
        let session=match app2.summary(id){
            Some(session)=>session,
            None=>{
                let observed=app2.coordination_exits.lock().get(id).cloned().ok_or("Worker is absent on this host. Absence does not prove exit; claims remain held.")?;
                if provenance["pid"]!=json!(observed.0)||provenance["createdAt"]!=observed.1{return Err("Exit observation does not identify this task-created process.".into());}
                let mut session=crate::model::TerminalSessionSummary::native(id.into(),id.into(),String::new(),task.project.clone(),observed.0.ok_or("Exit observation lacks process identity.")?,80,24);
                session.created_at=observed.1;session.status="exited".into();session
            }
        };
        if session.kind.as_deref()==Some("orchestrator"){return Err("Coordinators cannot be retired as task-owned workers.".into());}
        if provenance["taskId"]!=task.id||provenance["pid"]!=json!(session.pid)||provenance["createdAt"]!=session.created_at||session.pid.is_none(){return Err("Worker provenance differs from this task's host-owned process. Refusing to stop it.".into());}
        Ok((task,session))
    }).await?;
    if session.status == "running" {
        app.dispatch(&session.id, crate::state::COMMAND_KILL, "", 0, 0)?;
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        if app
            .coordination_exits
            .lock()
            .get(&session.id)
            .is_some_and(|(pid, created, _)| *pid == session.pid && *created == session.created_at)
        {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            return Err("Stop requested, but no matching host exit observation arrived. Claims remain held; inspect the owning host.".into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let app = app.clone();
    blocking(move||{
        let snapshot=snapshot_sync(&app)?;let attempt_id=task.attempt_id.ok_or("Task has no attempt.")?;
        let attempt=snapshot.attempts.iter().find(|a|a.id==attempt_id).ok_or("Task attempt is missing.")?;
        let outcome=host_execute(&app,&command_id,Command::ObserveStopped{attempt_id,epoch:attempt.epoch,proof:core::StopProof{kind:core::StopKind::ProcessExited,reference:format!("host process-tree exit for session {} pid {:?} created {}",session.id,session.pid,session.created_at),observed_at:now()}})?;
        let snapshot=snapshot_sync(&app)?;let held=snapshot.resources.iter().any(|r|r.task_id==task.id);
        Ok(json!({"outcome":outcome,"workerStopped":true,"resourcesReleased":!held,"candidateHold":held}))
    }).await
}
