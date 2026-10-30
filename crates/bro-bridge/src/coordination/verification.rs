use super::*;

pub(super) fn current_candidate(
    app: &AppState,
    actor: &Actor,
    task_id: &str,
) -> Result<(Engine, core::TaskView, core::TaskResult, PathBuf), String> {
    let engine = Engine::open(app.data_root.as_path()).map_err(|e| e.to_string())?;
    let task = engine
        .task(task_id)
        .map_err(|e| e.to_string())?
        .ok_or("Unknown task.")?;
    check_scope(app, actor, &task)?;
    let result = task
        .result
        .clone()
        .ok_or("The worker has not submitted a structured result.")?;
    let cwd = workspace(&task)?;
    let actual = crate::coordination_evidence::capture(&cwd, &result.artifacts)?;
    if actual.digest != result.digest || actual.artifacts != result.artifacts {
        return Err("Candidate content changed after submission. Stop the previous worker, then revise or retry the task and submit a new candidate; old evidence cannot accept these bytes.".into());
    }
    for (path, expected) in &task.contract.sources {
        let source = if Path::new(path).is_absolute() {
            PathBuf::from(path)
        } else {
            cwd.join(path)
        };
        let (actual, _) = crate::coordination_evidence::hash_file(&source)?;
        if &actual != expected {
            host_execute(
                app,
                &format!("source-{}-{}", core::digest_bytes(path.as_bytes()), actual),
                Command::InvalidateSource {
                    source: path.clone(),
                    digest: actual,
                },
            )?;
            return Err(format!(
                "Contract source changed: {path}. Its evidence and dependent memory were invalidated. Stop any old worker, revise or retry the affected task, and verify the new candidate."
            ));
        }
    }
    Ok((engine, task, result, cwd))
}

pub(super) fn review_token(
    app: &AppState,
    actor: &Actor,
    task: &core::TaskView,
    result: &core::TaskResult,
) -> String {
    core::digest_bytes(
        format!(
            "bro-review-v1\0{}\0{}\0{}\0{}\0{}",
            app.token, actor.id, task.id, task.revision, result.digest
        )
        .as_bytes(),
    )
}

pub(super) async fn inspect(app: &AppState, args: &Value) -> Result<Value, String> {
    let actor = coordinator(app)?;
    let task_id = required(args, "taskId")?;
    let app = app.clone();
    blocking(move||{
        let (_,task,result,cwd)=current_candidate(&app,&actor,&task_id)?;
        let mut artifacts=Vec::new();
        for artifact in &result.artifacts {
            let path=Path::new(&artifact.path); let path=if path.is_absolute(){path.to_owned()}else{cwd.join(path)};
            use std::io::Read;
            let mut bytes=Vec::new();std::fs::File::open(&path).and_then(|file|file.take(12000).read_to_end(&mut bytes)).map_err(|e|format!("Cannot inspect {}: {e}",path.display()))?;
            let preview=String::from_utf8_lossy(&bytes).to_string();
            artifacts.push(json!({"artifact":artifact,"preview":preview,"previewTruncated":artifact.bytes>12000}));
        }
        Ok(json!({"task":task,"result":result,"artifacts":artifacts,"reviewToken":review_token(&app,&actor,&task,&result),"note":"Review the acceptance criteria and actual behavior. The token binds your judgment to these candidate bytes; it does not establish correctness."}))
    }).await
}

pub(super) async fn verify(app: &AppState, args: &Value) -> Result<Value, String> {
    let actor = coordinator(app)?;
    let command_id = required(args, "commandId")?;
    let task_id = required(args, "taskId")?;
    let criterion_id = required(args, "criterionId")?;
    let args = args.clone();
    let app = app.clone();
    blocking(move||{
        let (engine,task,result,cwd)=current_candidate(&app,&actor,&task_id)?;
        let criterion=task.contract.acceptance.iter().find(|c|c.id==criterion_id).ok_or("Unknown criterionId.")?;
        let attempt_id=task.attempt_id.clone().ok_or("Candidate has no attempt.")?;
        // Validate the entire requested operation before reserving its side-effect identity.
        match criterion.kind{
            core::CriterionKind::Check=>{
                let packet=engine.context(&task.id,now()).map_err(|e|e.to_string())?;
                let command=criterion.command.as_deref().ok_or("Check has no explicit command.")?;
                if !packet.intent.authorization.authorized_actions.iter().any(|a|a=="runChecks"||a==&format!("command:{command}")){return Err("The intent does not authorize this exact check. Record its applicable inherited authorization first.".into());}
            }
            core::CriterionKind::Artifact=>{let id=required(&args,"artifactId")?;if !result.artifacts.iter().any(|a|a.id==id){return Err("Artifact was not submitted.".into());}}
            core::CriterionKind::Review=>{
                if criterion.independent&&task.session_id.as_deref()==Some(actor.id.as_str()){return Err("This criterion requires an independent reviewer.".into());}
                if required(&args,"reviewToken")?!=review_token(&app,&actor,&task,&result){return Err("Inspect this current candidate before recording the review.".into());}
                if !matches!(required(&args,"decision")?.as_str(),"pass"|"fail"){return Err("decision must be pass or fail.".into());}
                if required(&args,"findings")?.len()>16000{return Err("Review findings are too long.".into());}
            }
        }
        let operation_path=app.data_root.join("coordination").join("verification-operations").join(format!("{}.json",core::digest_bytes(command_id.as_bytes())));
        std::fs::create_dir_all(operation_path.parent().unwrap()).map_err(|e|e.to_string())?;
        let fingerprint=core::digest_bytes(&serde_json::to_vec(&json!({"task":task.id,"revision":task.revision,"digest":result.digest,"criterion":criterion_id,"actor":actor.id,"request":args})).map_err(|e|e.to_string())?);
        match std::fs::OpenOptions::new().create_new(true).write(true).open(&operation_path){
            Ok(mut file)=>{use std::io::Write;file.write_all(serde_json::to_string(&json!({"fingerprint":fingerprint,"state":"started"})).unwrap().as_bytes()).map_err(|e|e.to_string())?;file.sync_all().map_err(|e|e.to_string())?;#[cfg(unix)]std::fs::File::open(operation_path.parent().unwrap()).and_then(|dir|dir.sync_all()).map_err(|e|e.to_string())?;}
            Err(e) if e.kind()==std::io::ErrorKind::AlreadyExists=>{
                let saved:Value=serde_json::from_slice(&std::fs::read(&operation_path).map_err(|e|e.to_string())?).map_err(|e|e.to_string())?;
                if saved["fingerprint"]!=fingerprint{return Err("commandId was already used for a different verification operation.".into());}
                if let Some(value)=saved.get("result"){
                    let path=value.get("outputPath").and_then(Value::as_str).ok_or("Recorded verification output path is missing.")?;
                    let(digest,_)=crate::coordination_evidence::hash_file(Path::new(path))?;
                    if value.pointer("/verification/outputDigest").and_then(Value::as_str)!=Some(&digest){return Err("Saved verifier output changed. The previous receipt cannot be replayed.".into());}
                    return Ok(value.clone());
                }
                return Err("This verifier operation already started and its completion is uncertain. It was not rerun. Inspect its evidence/output before creating a new verification operation.".into());
            }
            Err(e)=>return Err(e.to_string()),
        }
        let mut receipt=core::Verification{id:format!("verify-{}",core::digest_bytes(command_id.as_bytes())[..24].to_string()),criterion_id:criterion_id.clone(),attempt_id,epoch:task.epoch,revision:task.revision,digest:result.digest.clone(),verifier:actor.id.clone(),finished_at:now(),..Default::default()};
        let output=match criterion.kind {
            core::CriterionKind::Check=>{
                let packet=engine.context(&task.id,now()).map_err(|e|e.to_string())?;
                let command=criterion.command.as_deref().ok_or("Check has no explicit command.")?;
                if !packet.intent.authorization.authorized_actions.iter().any(|a|a=="runChecks"||a==&format!("command:{command}")) {
                    return Err("This intent does not authorize the required command. Record the user's applicable authorization in its contract before executing checks.".into());
                }
                let timeout=args.get("timeoutSeconds").and_then(Value::as_u64).unwrap_or(120).clamp(1,600);
                let checked=crate::coordination_evidence::run_check(&cwd,command,Duration::from_secs(timeout))?;
                receipt.passed=checked.passed; receipt.exit_code=Some(checked.exit_code); receipt.command=Some(command.to_string());receipt.output_digest=checked.digest; checked.output
            }
            core::CriterionKind::Artifact=>{
                let artifact_id=required(&args,"artifactId")?;
                let artifact=result.artifacts.iter().find(|a|a.id==artifact_id).ok_or("Artifact was not submitted.")?;
                receipt.passed=true; receipt.artifact_id=Some(artifact_id);
                serde_json::to_vec(artifact).map_err(|e|e.to_string())?
            }
            core::CriterionKind::Review=>{
                if criterion.independent&&task.session_id.as_deref()==Some(actor.id.as_str()){return Err("This criterion requires a reviewer independent of the worker.".into());}
                if required(&args,"reviewToken")?!=review_token(&app,&actor,&task,&result){return Err("Inspect this candidate with coordination_inspect before recording a review; the review token must match its revision and content.".into());}
                let decision=required(&args,"decision")?;
                if !matches!(decision.as_str(),"pass"|"fail"){return Err("decision must be pass or fail.".into());}
                let findings=required(&args,"findings")?;
                if findings.len()>16000{return Err("Review findings are too long.".into());}
                receipt.passed=decision=="pass"; receipt.command=Some(format!("coordinator review of {} at revision {}",result.digest,task.revision));
                format!("Reviewer: {}\nCriterion: {}\nDecision: {}\n{}",actor.id,criterion.description,decision,findings).into_bytes()
            }
        };
        let after=crate::coordination_evidence::capture(&cwd,&result.artifacts)?;
        if after.digest!=result.digest||after.artifacts!=result.artifacts{return Err("Candidate changed during verification. No passing receipt was recorded; submit the new candidate.".into());}
        receipt.finished_at=now();if receipt.output_digest.is_empty(){receipt.output_digest=core::digest_bytes(&output);}
        let log=app.data_root.join("coordination").join("evidence").join(format!("{}.log",receipt.id));
        std::fs::create_dir_all(log.parent().unwrap()).map_err(|e|e.to_string())?;
        crate::project_store::write_atomic(&log,&output).map_err(|e|e.to_string())?;
        receipt.output_path=Some(log.to_string_lossy().into_owned());
        let outcome=host_execute(&app,&command_id,Command::Verify{task_id:task.id.clone(),revision:task.revision,verification:receipt.clone()})?;
        let value=json!({"outcome":outcome,"verification":receipt,"outputPath":log});
        crate::project_store::write_atomic(&operation_path,&serde_json::to_vec(&json!({"fingerprint":fingerprint,"state":"completed","result":value})).map_err(|e|e.to_string())?).map_err(|e|e.to_string())?;
        Ok(value)
    }).await
}

pub(super) async fn accept(app: &AppState, args: &Value) -> Result<Value, String> {
    let actor = coordinator(app)?;
    let task_id = required(args, "taskId")?;
    let command_id = required(args, "commandId")?;
    let app = app.clone();
    blocking(move||{
        let (engine,task,result,_)=current_candidate(&app,&actor,&task_id)?;
        let before=engine.snapshot().map_err(|e|e.to_string())?;
        let attempt=before.attempts.iter().find(|a|Some(&a.id)==task.attempt_id.as_ref()).ok_or("Task attempt is missing.")?;
        if !attempt.stopped{return Err("Stop the task-owned worker with coordination_retire_worker before accepting mutable workspace content. Existing user sessions require an explicit user-directed stop; terminal idle is not a stop proof.".into());}
        let outcome=engine.execute(&command_id,&actor,now(),Command::Accept{task_id:task.id,revision:task.revision,digest:result.digest}).map_err(|e|e.to_string())?;
        publish(&app,&outcome);wake_changed(&app,&before,&engine.snapshot().map_err(|e|e.to_string())?);
        deliver_accepted(&app,&engine.snapshot().map_err(|e|e.to_string())?)?;
        Ok(json!({"outcome":outcome,"note":"Result accepted against runtime evidence. Resource claims remain held until the assigned worker is proven stopped."}))
    }).await
}
