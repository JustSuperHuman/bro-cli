use super::*;
use std::{collections::BTreeMap, fs};

fn host() -> Actor {
    Actor {
        id: "host".into(),
        role: ActorRole::Host,
    }
}
fn coordinator() -> Actor {
    Actor {
        id: "coordinator".into(),
        role: ActorRole::Coordinator,
    }
}
fn worker(id: &str) -> Actor {
    Actor {
        id: id.into(),
        role: ActorRole::Worker,
    }
}
fn run(engine: &Engine, command: Command) -> Outcome {
    engine
        .execute(
            &format!("command-{:032x}", rand::random::<u128>()),
            &host(),
            100,
            command,
        )
        .unwrap()
}
fn check() -> Criterion {
    Criterion {
        id: "check".into(),
        description: "The required check passes".into(),
        command: Some("cargo check".into()),
        ..Default::default()
    }
}
fn input(id: &str) -> NewTask {
    NewTask {
        id: id.into(),
        title: id.into(),
        project: "project-a".into(),
        contract: TaskContract {
            goal: "Finish the change".into(),
            acceptance: vec![check()],
            ..Default::default()
        },
        ..Default::default()
    }
}
fn intent() -> NewIntent {
    NewIntent {
        id: "intent".into(),
        title: "Deliver the goal".into(),
        ..Default::default()
    }
}
fn setup(tasks: Vec<NewTask>) -> (tempfile::TempDir, Engine) {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    run(
        &engine,
        Command::CreateWork {
            intent: intent(),
            tasks,
        },
    );
    (dir, engine)
}
fn assign(engine: &Engine, task_id: &str, session: &str) -> Outcome {
    run(
        engine,
        Command::Claim {
            task_id: task_id.into(),
            session_id: session.into(),
            adapter_id: None,
            lease_seconds: 30,
        },
    )
}
fn dispatch(engine: &Engine, assigned: &Outcome) {
    let outbox_id = assigned.outbox_id.clone().unwrap();
    run(
        engine,
        Command::BeginDelivery {
            outbox_id: outbox_id.clone(),
            consumer_id: "bridge".into(),
        },
    );
    run(engine, Command::AckDelivery { outbox_id });
}
fn epoch(engine: &Engine, task: &str) -> u64 {
    engine.task(task).unwrap().unwrap().epoch
}
fn result() -> TaskResult {
    TaskResult {
        summary: "The change is implemented".into(),
        digest: digest_bytes(b"dirty content including untracked files"),
        ..Default::default()
    }
}
fn submit(engine: &Engine, assigned: &Outcome, task_id: &str) {
    run(
        engine,
        Command::SubmitResult {
            attempt_id: assigned.attempt_id.clone().unwrap(),
            epoch: epoch(engine, task_id),
            result: result(),
        },
    );
}
fn verification(engine: &Engine, assigned: &Outcome, task_id: &str) -> Verification {
    let t = engine.task(task_id).unwrap().unwrap();
    Verification {
        id: format!("verification-{task_id}"),
        criterion_id: "check".into(),
        attempt_id: assigned.attempt_id.clone().unwrap(),
        epoch: t.epoch,
        revision: t.revision,
        digest: result().digest,
        passed: true,
        exit_code: Some(0),
        command: Some("cargo check".into()),
        verifier: "independent-check-process".into(),
        output_digest: digest_bytes(b"actual verifier stdout"),
        finished_at: 100,
        ..Default::default()
    }
}
fn verify_accept(engine: &Engine, assigned: &Outcome, task_id: &str) {
    stop(engine, assigned, task_id);
    let t = engine.task(task_id).unwrap().unwrap();
    run(
        engine,
        Command::Verify {
            task_id: task_id.into(),
            revision: t.revision,
            verification: verification(engine, assigned, task_id),
        },
    );
    run(
        engine,
        Command::Accept {
            task_id: task_id.into(),
            revision: t.revision,
            digest: result().digest,
        },
    );
}
fn stop(engine: &Engine, assigned: &Outcome, task_id: &str) {
    let a = engine
        .snapshot()
        .unwrap()
        .attempts
        .into_iter()
        .find(|a| Some(&a.id) == assigned.attempt_id.as_ref())
        .unwrap();
    run(
        engine,
        Command::ObserveStopped {
            attempt_id: a.id,
            epoch: a.epoch,
            proof: StopProof {
                kind: StopKind::ProcessExited,
                reference: format!("observed process exit for {task_id}"),
                observed_at: 100,
            },
        },
    );
}

#[test]
fn durable_lifecycle_verifies_evidence_then_releases_resources_only_on_stop() {
    let mut first = input("first");
    first.resources.push(ResourceClaim {
        key: "repo:a".into(),
        mode: ClaimMode::Exclusive,
    });
    let mut next = input("next");
    next.dependencies.push("first".into());
    next.resources = first.resources.clone();
    let (dir, engine) = setup(vec![first, next]);
    let assigned = assign(&engine, "first", "worker1");
    dispatch(&engine, &assigned);
    submit(&engine, &assigned, "first");
    assert!(
        engine
            .execute(
                "premature",
                &coordinator(),
                100,
                Command::Accept {
                    task_id: "first".into(),
                    revision: 1,
                    digest: result().digest
                }
            )
            .is_err()
    );
    verify_accept(&engine, &assigned, "first");
    assert_eq!(
        engine.task("first").unwrap().unwrap().state,
        TaskState::Verified
    );
    assert_eq!(
        Engine::open(dir.path())
            .unwrap()
            .task("first")
            .unwrap()
            .unwrap()
            .state,
        TaskState::Verified
    );
    assert_eq!(engine.ready_tasks(None, 100).unwrap()[0].id, "next");
    assert_eq!(engine.snapshot().unwrap().metrics.accepted, 1);
}

#[test]
fn semantic_duplicates_cannot_reopen_accepted_or_submitted_work() {
    let (_dir, engine) = setup(vec![input("task")]);
    let assigned = assign(&engine, "task", "worker");
    dispatch(&engine, &assigned);
    submit(&engine, &assigned, "task");
    run(
        &engine,
        Command::AckDelivery {
            outbox_id: assigned.outbox_id.clone().unwrap(),
        },
    );
    assert_eq!(
        engine.task("task").unwrap().unwrap().state,
        TaskState::Submitted
    );
    verify_accept(&engine, &assigned, "task");
    run(
        &engine,
        Command::AckDelivery {
            outbox_id: assigned.outbox_id.clone().unwrap(),
        },
    );
    submit(&engine, &assigned, "task");
    run(
        &engine,
        Command::Verify {
            task_id: "task".into(),
            revision: 1,
            verification: verification(&engine, &assigned, "task"),
        },
    );
    assert_eq!(
        engine.task("task").unwrap().unwrap().state,
        TaskState::Verified
    );
    assert_eq!(engine.snapshot().unwrap().metrics.accepted, 1);
    assert!(
        engine
            .execute(
                "block-after-submit",
                &worker("worker"),
                100,
                Command::Block {
                    attempt_id: assigned.attempt_id.unwrap(),
                    epoch: 1,
                    blocker: Blocker {
                        message: "question".into(),
                        wake_condition: "answer".into(),
                        ..Default::default()
                    }
                }
            )
            .is_err()
    );
}

#[test]
fn commands_remain_idempotent_after_reopen_and_reject_key_reuse() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    let command = Command::CreateWork {
        intent: intent(),
        tasks: vec![input("one")],
    };
    let first = engine
        .execute("stable-command", &coordinator(), 100, command.clone())
        .unwrap();
    let duplicate = Engine::open(dir.path())
        .unwrap()
        .execute("stable-command", &coordinator(), 101, command)
        .unwrap();
    assert!(duplicate.duplicate);
    assert_eq!(first.seq, duplicate.seq);
    assert!(
        engine
            .execute("stable-command", &host(), 101, Command::Tick)
            .is_err()
    );
    assert_eq!(engine.snapshot().unwrap().tasks.len(), 1);
}

#[test]
fn uncertain_dispatch_is_not_replayed_after_restart_or_torn_tail() {
    use std::io::Write;
    let (dir, engine) = setup(vec![input("task")]);
    let assigned = assign(&engine, "task", "worker");
    let command = Command::BeginDelivery {
        outbox_id: assigned.outbox_id.clone().unwrap(),
        consumer_id: "bridge1".into(),
    };
    engine
        .execute("begin-once", &host(), 100, command.clone())
        .unwrap();
    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(engine.storage_path().join("journal.jsonl"))
        .unwrap();
    file.write_all(b"{\"incomplete\":").unwrap();
    file.sync_all().unwrap();
    drop(file);
    let restored = Engine::open(dir.path()).unwrap();
    assert!(
        restored
            .execute("begin-once", &host(), 100, command.clone())
            .unwrap()
            .duplicate
    );
    assert!(
        restored
            .execute("begin-twice", &host(), 100, command)
            .is_err()
    );
    let snapshot = restored.snapshot().unwrap();
    assert_eq!(snapshot.deliveries[0].state, DeliveryState::Dispatching);
    assert_eq!(
        snapshot
            .recovery
            .iter()
            .filter(|r| r.kind == "uncommittedTail")
            .count(),
        1
    );
    assert!(
        snapshot.recovery[0]
            .preserved_path
            .as_ref()
            .is_some_and(|p| std::path::Path::new(p).exists())
    );
}

#[test]
fn corrupt_committed_record_is_an_error_and_never_an_empty_workspace() {
    use std::io::Write;
    let (dir, engine) = setup(vec![input("task")]);
    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(engine.storage_path().join("journal.jsonl"))
        .unwrap();
    file.write_all(b"{\"not\":\"a committed transaction\"}\n")
        .unwrap();
    drop(file);
    assert!(Engine::open(dir.path()).is_err());
}

#[test]
fn expired_writer_keeps_conflicts_fenced_until_host_proves_stop() {
    let mut one = input("one");
    one.resources.push(ResourceClaim {
        key: "device:phone".into(),
        mode: ClaimMode::Exclusive,
    });
    let mut two = input("two");
    two.resources = one.resources.clone();
    let (_dir, engine) = setup(vec![one, two]);
    let assigned = assign(&engine, "one", "old");
    dispatch(&engine, &assigned);
    engine
        .execute("expired", &host(), 131, Command::Tick)
        .unwrap();
    assert_eq!(
        engine.task("one").unwrap().unwrap().state,
        TaskState::Uncertain
    );
    assert!(
        engine
            .execute(
                "stale-result",
                &worker("old"),
                131,
                Command::SubmitResult {
                    attempt_id: assigned.attempt_id.clone().unwrap(),
                    epoch: 1,
                    result: result()
                }
            )
            .is_err()
    );
    assert!(
        engine
            .execute(
                "second-writer",
                &coordinator(),
                131,
                Command::Claim {
                    task_id: "two".into(),
                    session_id: "new".into(),
                    adapter_id: None,
                    lease_seconds: 30
                }
            )
            .is_err()
    );
    stop(&engine, &assigned, "one");
    assert!(engine.snapshot().unwrap().resources.is_empty());
    assign(&engine, "two", "new");
}

#[test]
fn quiet_live_worker_can_be_reconciled_after_controller_restart_without_redispatch() {
    let (dir, engine) = setup(vec![input("task")]);
    let assigned = assign(&engine, "task", "quiet-build");
    dispatch(&engine, &assigned);
    engine
        .execute("expired", &host(), 131, Command::Tick)
        .unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    let renew = Command::RenewAttempt {
        attempt_id: assigned.attempt_id.clone().unwrap(),
        epoch: 1,
        session_id: "quiet-build".into(),
        lease_seconds: 300,
        observation: "same host session and PID still running the quiet compiler".into(),
    };
    assert!(
        engine
            .execute(
                "worker-self-renew",
                &worker("quiet-build"),
                140,
                renew.clone()
            )
            .is_err()
    );
    engine
        .execute("observed-live", &host(), 140, renew.clone())
        .unwrap();
    assert_eq!(
        engine.task("task").unwrap().unwrap().state,
        TaskState::Running
    );
    assert_eq!(engine.snapshot().unwrap().deliveries.len(), 1);
    engine
        .execute(
            "completed-after-recovery",
            &worker("quiet-build"),
            141,
            Command::SubmitResult {
                attempt_id: assigned.attempt_id.clone().unwrap(),
                epoch: 1,
                result: result(),
            },
        )
        .unwrap();
    run(
        &engine,
        Command::Steer {
            task_id: "task".into(),
            expected_revision: 1,
            contract: input("task").contract,
        },
    );
    assert!(
        engine
            .execute("renew-after-steer", &host(), 142, renew)
            .is_err()
    );
}

#[test]
fn atomic_multi_resource_claims_and_shared_reads() {
    let read = ResourceClaim {
        key: "repo:shared".into(),
        mode: ClaimMode::Read,
    };
    let mut one = input("one");
    one.resources = vec![read.clone()];
    let mut two = input("two");
    two.resources = vec![read];
    let mut write = input("write");
    write.resources = vec![
        ResourceClaim {
            key: "other:free".into(),
            mode: ClaimMode::Exclusive,
        },
        ResourceClaim {
            key: "repo:shared".into(),
            mode: ClaimMode::Exclusive,
        },
    ];
    let (_dir, engine) = setup(vec![one, two, write]);
    assign(&engine, "one", "reader1");
    assign(&engine, "two", "reader2");
    assert!(
        engine
            .execute(
                "write",
                &coordinator(),
                100,
                Command::Claim {
                    task_id: "write".into(),
                    session_id: "writer".into(),
                    adapter_id: None,
                    lease_seconds: 30
                }
            )
            .is_err()
    );
    let snapshot = engine.snapshot().unwrap();
    assert_eq!(snapshot.resources.len(), 2);
    assert!(snapshot.resources.iter().all(|r| r.key == "repo:shared"));
}

#[test]
fn invalid_graph_and_contract_are_atomic_and_do_not_leave_orphans() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    let mut one = input("one");
    one.dependencies = vec!["missing".into()];
    assert!(
        engine
            .execute(
                "missing",
                &coordinator(),
                100,
                Command::CreateWork {
                    intent: intent(),
                    tasks: vec![one]
                }
            )
            .is_err()
    );
    assert!(engine.snapshot().unwrap().intents.is_empty());
    let mut one = input("one");
    one.dependencies.push("two".into());
    let mut two = input("two");
    two.dependencies.push("one".into());
    assert!(
        engine
            .execute(
                "cycle",
                &coordinator(),
                100,
                Command::CreateWork {
                    intent: intent(),
                    tasks: vec![one, two]
                }
            )
            .is_err()
    );
    let mut empty = input("empty");
    empty.contract.acceptance.clear();
    assert!(
        engine
            .execute(
                "vacuous",
                &coordinator(),
                100,
                Command::CreateWork {
                    intent: intent(),
                    tasks: vec![empty]
                }
            )
            .is_err()
    );
    assert!(engine.snapshot().unwrap().tasks.is_empty());
}

#[test]
fn adding_required_child_invalidates_accepted_parent_and_dependants_only() {
    let parent = input("parent");
    let mut dependent = input("dependent");
    dependent.dependencies.push("parent".into());
    let (_dir, engine) = setup(vec![parent, dependent, input("unrelated")]);
    let assigned = assign(&engine, "parent", "worker");
    dispatch(&engine, &assigned);
    submit(&engine, &assigned, "parent");
    verify_accept(&engine, &assigned, "parent");
    stop(&engine, &assigned, "parent");
    let old_unrelated = engine.task("unrelated").unwrap().unwrap().revision;
    let mut child = input("child");
    child.parent_id = Some("parent".into());
    run(
        &engine,
        Command::CreateTasks {
            intent_id: "intent".into(),
            tasks: vec![child],
        },
    );
    let parent = engine.task("parent").unwrap().unwrap();
    assert_eq!(parent.state, TaskState::Pending);
    assert_eq!(parent.revision, 2);
    assert!(parent.evidence.is_empty());
    assert_eq!(engine.task("dependent").unwrap().unwrap().revision, 2);
    assert_eq!(
        engine.task("unrelated").unwrap().unwrap().revision,
        old_unrelated
    );
    assert_eq!(parent.blocked_by, vec!["child"]);
}

#[test]
fn evidence_requires_trusted_actual_matching_checks_and_independence() {
    let mut task = input("task");
    task.contract.acceptance[0].independent = true;
    let (_dir, engine) = setup(vec![task]);
    let assigned = assign(&engine, "task", "worker");
    dispatch(&engine, &assigned);
    submit(&engine, &assigned, "task");
    let good = verification(&engine, &assigned, "task");
    assert!(
        engine
            .execute(
                "worker-claim",
                &worker("worker"),
                100,
                Command::Verify {
                    task_id: "task".into(),
                    revision: 1,
                    verification: good.clone()
                }
            )
            .is_err()
    );
    for (id, bad) in [
        (
            "bad-exit",
            Verification {
                exit_code: Some(1),
                ..good.clone()
            },
        ),
        (
            "bad-digest",
            Verification {
                digest: digest_bytes(b"old content"),
                ..good.clone()
            },
        ),
        (
            "bad-criterion",
            Verification {
                criterion_id: "unknown".into(),
                ..good.clone()
            },
        ),
        (
            "bad-self",
            Verification {
                verifier: "worker".into(),
                ..good.clone()
            },
        ),
        (
            "bad-command",
            Verification {
                command: Some("echo passed".into()),
                ..good.clone()
            },
        ),
    ] {
        assert!(
            engine
                .execute(
                    id,
                    &host(),
                    100,
                    Command::Verify {
                        task_id: "task".into(),
                        revision: 1,
                        verification: bad
                    }
                )
                .is_err(),
            "{id}"
        );
    }
    run(
        &engine,
        Command::Verify {
            task_id: "task".into(),
            revision: 1,
            verification: good.clone(),
        },
    );
    let mut overwritten = good;
    overwritten.passed = false;
    overwritten.exit_code = Some(1);
    assert!(
        engine
            .execute(
                "overwrite-proof",
                &host(),
                100,
                Command::Verify {
                    task_id: "task".into(),
                    revision: 1,
                    verification: overwritten
                }
            )
            .is_err()
    );
}

#[test]
fn default_intent_can_finish_more_than_three_tasks_and_explicit_caps_hold() {
    let tasks = (0..5).map(|n| input(&format!("task{n}"))).collect();
    let (_dir, engine) = setup(tasks);
    for n in 0..5 {
        let task = format!("task{n}");
        let a = assign(&engine, &task, "worker");
        dispatch(&engine, &a);
        submit(&engine, &a, &task);
        verify_accept(&engine, &a, &task);
        stop(&engine, &a, &task);
    }
    assert_eq!(engine.snapshot().unwrap().metrics.accepted, 5);
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    let mut cap = intent();
    cap.budget.max_attempts = 1;
    run(
        &engine,
        Command::CreateWork {
            intent: cap,
            tasks: vec![input("one"), input("two")],
        },
    );
    let a = assign(&engine, "one", "worker");
    stop(&engine, &a, "one");
    let two = engine.task("two").unwrap().unwrap();
    assert_eq!(two.state, TaskState::Pending);
    assert!(two.next_action.contains("intent attempt budget"));
    assert!(engine.ready_tasks(None, 100).unwrap().is_empty());
}

#[test]
fn cancellations_do_not_release_live_actor_and_old_tokens_stay_fenced() {
    let mut task = input("task");
    task.resources.push(ResourceClaim {
        key: "db:migration".into(),
        mode: ClaimMode::Exclusive,
    });
    let (_dir, engine) = setup(vec![task]);
    let a = assign(&engine, "task", "worker");
    dispatch(&engine, &a);
    run(
        &engine,
        Command::Cancel {
            task_id: "task".into(),
            reason: "User cancelled".into(),
        },
    );
    assert_eq!(
        engine.task("task").unwrap().unwrap().state,
        TaskState::CancelRequested
    );
    assert_eq!(engine.snapshot().unwrap().resources.len(), 1);
    assert!(
        engine
            .execute(
                "late",
                &worker("worker"),
                100,
                Command::SubmitResult {
                    attempt_id: a.attempt_id.clone().unwrap(),
                    epoch: 1,
                    result: result()
                }
            )
            .is_err()
    );
    stop(&engine, &a, "task");
    assert_eq!(
        engine.task("task").unwrap().unwrap().state,
        TaskState::Cancelled
    );
    assert!(engine.snapshot().unwrap().resources.is_empty());
}

#[test]
fn fairness_capability_and_completion_reserve_are_explicit() {
    let mut old = input("old");
    old.project = "old-project".into();
    old.priority = 0;
    let (_dir, engine) = setup(vec![old]);
    let mut new = input("new");
    new.priority = 5;
    new.project = "new-project".into();
    engine
        .execute(
            "later-task",
            &host(),
            1000,
            Command::CreateTasks {
                intent_id: "intent".into(),
                tasks: vec![new],
            },
        )
        .unwrap();
    assert_eq!(
        engine.ready_tasks(None, 1000).unwrap()[0].id,
        "old",
        "aging overtakes small priority differences"
    );
    let mut review = input("review");
    review.lane = Lane::Completion;
    review.capabilities = vec!["screenshots".into()];
    run(
        &engine,
        Command::CreateTasks {
            intent_id: "intent".into(),
            tasks: vec![review],
        },
    );
    let mut adapter = Adapter {
        id: "adapter".into(),
        harness: "codex".into(),
        available: true,
        ..Default::default()
    };
    run(
        &engine,
        Command::RegisterAdapter {
            adapter: adapter.clone(),
        },
    );
    assert!(
        engine
            .ready_tasks(Some("adapter"), 1000)
            .unwrap()
            .iter()
            .all(|t| t.id != "review")
    );
    adapter
        .capabilities
        .insert("screenshots".into(), Capability::Supported);
    run(&engine, Command::RegisterAdapter { adapter });
    assert_eq!(
        engine.ready_tasks(Some("adapter"), 1000).unwrap()[0].id,
        "review"
    );
    run(
        &engine,
        Command::SetPolicy {
            policy: Policy {
                global_limit: 2,
                project_limit: 2,
                completion_reserve: 1,
                ..Default::default()
            },
        },
    );
    assign(&engine, "old", "worker");
    assert!(
        engine
            .execute(
                "use-reserved-slot",
                &coordinator(),
                100,
                Command::Claim {
                    task_id: "new".into(),
                    session_id: "other".into(),
                    adapter_id: None,
                    lease_seconds: 30
                }
            )
            .is_err()
    );
}

#[test]
fn checkpoints_and_source_backed_memory_recover_and_invalidate() {
    let mut task = input("task");
    let digest = digest_bytes(b"source contents");
    task.contract
        .sources
        .insert("file:a".into(), digest.clone());
    let (dir, engine) = setup(vec![task]);
    let a = assign(&engine, "task", "worker");
    dispatch(&engine, &a);
    let memory = Memory {
        id: "fact".into(),
        scope: "project-a".into(),
        text: "Observed source-backed fact".into(),
        sources: BTreeMap::from([("file:a".into(), digest.clone())]),
        ..Default::default()
    };
    assert!(
        engine
            .execute(
                "untrusted-fact",
                &worker("worker"),
                100,
                Command::PutMemory {
                    memory: memory.clone()
                }
            )
            .is_err()
    );
    run(&engine, Command::PutMemory { memory });
    run(
        &engine,
        Command::SaveCheckpoint {
            checkpoint: Checkpoint {
                task_id: "task".into(),
                attempt_id: a.attempt_id.clone().unwrap(),
                epoch: 1,
                revision: 1,
                summary: "Compiler is running".into(),
                sources: BTreeMap::from([("file:a".into(), digest)]),
                at: 100,
                ..Default::default()
            },
        },
    );
    let restored = Engine::open(dir.path()).unwrap();
    assert!(restored.context("task", 101).unwrap().checkpoint.is_some());
    assert_eq!(restored.context("task", 101).unwrap().memory.len(), 1);
    run(
        &engine,
        Command::InvalidateSource {
            source: "file:a".into(),
            digest: digest_bytes(b"edited dirty contents"),
        },
    );
    let packet = restored.context("task", 102).unwrap();
    assert!(packet.checkpoint.is_none());
    assert!(packet.memory.is_empty());
    assert_eq!(packet.task.revision, 2);
}

#[test]
fn compaction_can_replay_old_journal_after_snapshot_commit() {
    let (_dir, engine) = setup(vec![input("task")]);
    let original = fs::read(engine.storage_path().join("journal.jsonl")).unwrap();
    for n in 0..63 {
        engine
            .execute(&format!("tick-{n}"), &host(), 100, Command::Tick)
            .unwrap();
    }
    assert!(engine.storage_path().join("snapshot.json").exists());
    fs::write(engine.storage_path().join("journal.jsonl"), original).unwrap();
    assert_eq!(engine.snapshot().unwrap().tasks.len(), 1);
    assert_eq!(engine.snapshot().unwrap().seq, 64);
}

#[test]
fn oversized_input_is_rejected_without_poisoning_store() {
    let (dir, engine) = setup(vec![input("task")]);
    let mut contract = input("task").contract;
    contract.context = vec!["x".repeat(65536); 3];
    assert!(
        engine
            .execute(
                "oversized",
                &coordinator(),
                100,
                Command::Steer {
                    task_id: "task".into(),
                    expected_revision: 1,
                    contract
                }
            )
            .is_err()
    );
    assert_eq!(
        Engine::open(dir.path())
            .unwrap()
            .task("task")
            .unwrap()
            .unwrap()
            .revision,
        1
    );
}

#[test]
fn changed_dirty_or_untracked_bytes_change_manifest_identity() {
    let file = ContentEntry {
        path: "untracked.txt".into(),
        sha256: digest_bytes(b"one"),
        bytes: 3,
        deleted: false,
    };
    let original = digest_manifest(&[file.clone()]).unwrap();
    assert_ne!(
        original,
        digest_manifest(&[ContentEntry {
            sha256: digest_bytes(b"two"),
            ..file.clone()
        }])
        .unwrap()
    );
    assert_ne!(
        original,
        digest_manifest(&[ContentEntry {
            deleted: true,
            ..file
        }])
        .unwrap()
    );
}

#[test]
fn contract_revision_invalidates_task_scoped_memory_even_when_source_bytes_match() {
    let mut task = input("task");
    let digest = digest_bytes(b"source");
    task.contract
        .sources
        .insert("file:a".into(), digest.clone());
    let (_dir, engine) = setup(vec![task]);
    run(
        &engine,
        Command::PutMemory {
            memory: Memory {
                id: "decision".into(),
                scope: "task".into(),
                text: "The old contract's conclusion".into(),
                sources: BTreeMap::from([("file:a".into(), digest)]),
                ..Default::default()
            },
        },
    );
    assert_eq!(engine.context("task", 100).unwrap().memory.len(), 1);
    let mut contract = engine.task("task").unwrap().unwrap().contract;
    contract.goal = "A revised goal".into();
    run(
        &engine,
        Command::Steer {
            task_id: "task".into(),
            expected_revision: 1,
            contract,
        },
    );
    assert!(engine.context("task", 100).unwrap().memory.is_empty());
}

#[test]
fn stopped_candidate_holds_mutation_claim_through_final_acceptance() {
    let mut first = input("first");
    first.resources.push(ResourceClaim {
        key: "repo:stable".into(),
        mode: ClaimMode::Exclusive,
    });
    let mut second = input("second");
    second.resources = first.resources.clone();
    let (_dir, engine) = setup(vec![first, second]);
    let a = assign(&engine, "first", "worker");
    dispatch(&engine, &a);
    submit(&engine, &a, "first");
    run(
        &engine,
        Command::Verify {
            task_id: "first".into(),
            revision: 1,
            verification: verification(&engine, &a, "first"),
        },
    );
    assert!(
        engine
            .execute(
                "accept-alive",
                &coordinator(),
                100,
                Command::Accept {
                    task_id: "first".into(),
                    revision: 1,
                    digest: result().digest
                }
            )
            .is_err()
    );
    stop(&engine, &a, "first");
    assert_eq!(
        engine.snapshot().unwrap().resources.len(),
        1,
        "stopped candidate remains stable until accept or explicit retry/cancel"
    );
    assert!(
        engine
            .execute(
                "second-before-accept",
                &coordinator(),
                100,
                Command::Claim {
                    task_id: "second".into(),
                    session_id: "other".into(),
                    adapter_id: None,
                    lease_seconds: 30
                }
            )
            .is_err()
    );
    run(
        &engine,
        Command::Accept {
            task_id: "first".into(),
            revision: 1,
            digest: result().digest,
        },
    );
    assert!(engine.snapshot().unwrap().resources.is_empty());
}

#[test]
fn cancelled_timed_blocker_cannot_be_reawakened_by_supervisor() {
    let (_dir, engine) = setup(vec![input("task")]);
    let a = assign(&engine, "task", "worker");
    dispatch(&engine, &a);
    run(
        &engine,
        Command::Block {
            attempt_id: a.attempt_id.clone().unwrap(),
            epoch: 1,
            blocker: Blocker {
                kind: "rateLimit".into(),
                message: "Wait for reset".into(),
                wake_condition: "reset".into(),
                retry_at: Some(150),
                ..Default::default()
            },
        },
    );
    run(
        &engine,
        Command::Cancel {
            task_id: "task".into(),
            reason: "User cancelled the goal".into(),
        },
    );
    engine
        .execute("tick-after-cancel", &host(), 151, Command::Tick)
        .unwrap();
    assert_eq!(
        engine.task("task").unwrap().unwrap().state,
        TaskState::CancelRequested
    );
    stop(&engine, &a, "task");
    engine
        .execute("tick-after-stop", &host(), 152, Command::Tick)
        .unwrap();
    assert_eq!(
        engine.task("task").unwrap().unwrap().state,
        TaskState::Cancelled
    );
    assert!(engine.ready_tasks(None, 152).unwrap().is_empty());
}

#[test]
fn retired_adapter_can_be_removed_without_erasing_a_live_actor() {
    let (_dir, engine) = setup(vec![input("task")]);
    run(
        &engine,
        Command::RegisterAdapter {
            adapter: Adapter {
                id: "worker-adapter".into(),
                harness: "codex".into(),
                available: true,
                ..Default::default()
            },
        },
    );
    let a = run(
        &engine,
        Command::Claim {
            task_id: "task".into(),
            session_id: "worker".into(),
            adapter_id: Some("worker-adapter".into()),
            lease_seconds: 30,
        },
    );
    assert!(
        engine
            .execute(
                "remove-live",
                &host(),
                100,
                Command::RemoveAdapter {
                    adapter_id: "worker-adapter".into()
                }
            )
            .is_err()
    );
    stop(&engine, &a, "task");
    run(
        &engine,
        Command::RemoveAdapter {
            adapter_id: "worker-adapter".into(),
        },
    );
    assert!(engine.snapshot().unwrap().adapters.is_empty());
}

#[test]
fn cancelling_parent_stops_unfinished_descendants_and_preserves_verified_outputs() {
    let parent = input("parent");
    let mut queued = input("queued");
    queued.parent_id = Some("parent".into());
    let mut live = input("live");
    live.parent_id = Some("parent".into());
    live.resources.push(ResourceClaim {
        key: "workspace:live".into(),
        mode: ClaimMode::Exclusive,
    });
    let mut verified = input("verified");
    verified.parent_id = Some("parent".into());
    let mut grandchild = input("grandchild");
    grandchild.parent_id = Some("queued".into());
    let mut dependent = input("dependent");
    dependent.dependencies.push("parent".into());
    let (_dir, engine) = setup(vec![
        parent,
        queued,
        live,
        verified,
        grandchild,
        dependent,
        input("unrelated"),
    ]);
    let finished = assign(&engine, "verified", "finished-worker");
    dispatch(&engine, &finished);
    submit(&engine, &finished, "verified");
    verify_accept(&engine, &finished, "verified");
    let before_verified = engine.task("verified").unwrap().unwrap();
    let running = assign(&engine, "live", "running-worker");
    dispatch(&engine, &running);
    run(
        &engine,
        Command::Block {
            attempt_id: running.attempt_id.clone().unwrap(),
            epoch: epoch(&engine, "live"),
            blocker: Blocker {
                kind: "rateLimit".into(),
                message: "Wait for a reset".into(),
                wake_condition: "reset".into(),
                retry_at: Some(150),
                ..Default::default()
            },
        },
    );
    let cancelled = run(
        &engine,
        Command::Cancel {
            task_id: "parent".into(),
            reason: "User cancelled the outcome".into(),
        },
    );
    assert_eq!(cancelled.value["scope"], "unfinishedParentTree");
    for id in ["parent", "queued", "grandchild"] {
        assert_eq!(
            engine.task(id).unwrap().unwrap().state,
            TaskState::Cancelled
        );
        assert!(
            engine
                .execute(
                    &format!("claim-after-cancel-{id}"),
                    &coordinator(),
                    100,
                    Command::Claim {
                        task_id: id.into(),
                        session_id: "replacement".into(),
                        adapter_id: None,
                        lease_seconds: 30
                    }
                )
                .is_err()
        );
    }
    let live = engine.task("live").unwrap().unwrap();
    assert_eq!(live.state, TaskState::CancelRequested);
    assert!(live.blocker.is_none());
    assert_eq!(
        engine.snapshot().unwrap().resources.len(),
        1,
        "live mutation ownership remains fenced until trusted stop"
    );
    assert!(
        engine
            .execute(
                "cancelled-child-result",
                &worker("running-worker"),
                100,
                Command::SubmitResult {
                    attempt_id: running.attempt_id.clone().unwrap(),
                    epoch: 1,
                    result: result()
                }
            )
            .is_err()
    );
    engine
        .execute("cancelled-child-timer", &host(), 151, Command::Tick)
        .unwrap();
    assert_eq!(
        engine.task("live").unwrap().unwrap().state,
        TaskState::CancelRequested
    );
    assert_eq!(
        engine.task("verified").unwrap().unwrap(),
        before_verified,
        "verified result/evidence/identity is retained"
    );
    assert!(
        engine
            .task("dependent")
            .unwrap()
            .unwrap()
            .blocked_by
            .contains(&"parent".into())
    );
    let ready = engine.ready_tasks(None, 100).unwrap();
    assert_eq!(
        ready.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
        vec!["unrelated"]
    );
    assign(&engine, "unrelated", "unrelated-worker");
    let mut new_child = input("too-late");
    new_child.parent_id = Some("parent".into());
    assert!(
        engine
            .execute(
                "new-cancelled-child",
                &coordinator(),
                100,
                Command::CreateTasks {
                    intent_id: "intent".into(),
                    tasks: vec![new_child]
                }
            )
            .is_err()
    );
    stop(&engine, &running, "live");
    assert_eq!(
        engine.task("live").unwrap().unwrap().state,
        TaskState::Cancelled
    );
    assert!(engine.snapshot().unwrap().resources.is_empty());
}

#[test]
fn separate_process_authority_worker() {
    let Ok(root) = std::env::var("BRO_COORDINATION_TEST_ROOT") else {
        return;
    };
    let index = std::env::var("BRO_COORDINATION_TEST_INDEX").unwrap();
    let engine = Engine::open(root).unwrap();
    for n in 0..4 {
        let mut i = intent();
        i.id = format!("intent-{index}-{n}");
        engine
            .execute(
                &format!("create-{index}-{n}"),
                &coordinator(),
                100,
                Command::CreateIntent { intent: i },
            )
            .unwrap();
    }
}

#[test]
fn independent_processes_reload_under_one_os_authority_lock() {
    let dir = tempfile::tempdir().unwrap();
    let exe = std::env::current_exe().unwrap();
    let children: Vec<_> = (0..3)
        .map(|n| {
            std::process::Command::new(&exe)
                .args([
                    "--exact",
                    "coordination::tests::separate_process_authority_worker",
                    "--nocapture",
                ])
                .env("BRO_COORDINATION_TEST_ROOT", dir.path())
                .env("BRO_COORDINATION_TEST_INDEX", n.to_string())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    for child in children {
        let out = child.wait_with_output().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    assert_eq!(
        Engine::open(dir.path())
            .unwrap()
            .snapshot()
            .unwrap()
            .intents
            .len(),
        12
    );
}
