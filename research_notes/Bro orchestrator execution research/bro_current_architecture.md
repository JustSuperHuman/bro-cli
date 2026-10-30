# Bro current orchestrator architecture

Audit date: 29 October 2026. Evidence is the current working tree, which already contained user edits. This was a static audit: no live session was controlled and no tests, build, or benchmarks were run. Proposed failure scenarios below are deductions from code, except where an existing test directly encodes the behavior.

## What can Bro already do

### Takeaway

Bro already has a useful coordination shell: a root Orchestrator, project coordinators, workers in several CLI harnesses, transcript inspection, persistent timers, and watchdog reviews. The main opportunity is to give work a durable identity and an enforced completion contract across those components.

### Cited findings

- The current source supports one coordinator per project and a workspace coordinator. The root resolves projects and uses `open_orchestrator`; project coordinators reuse their workers. Scoped prompt files prevent concurrent launches from overwriting instructions. New project coordinators inherit the source harness, login, provider, model, permissions, and browser, and reuse a running coordinator. This hierarchy is already implemented and should be preserved. [Scoped instructions](F:/bro-cli-v2/crates/bro-core/src/orchestrator.rs:104), [scope-specific files](F:/bro-cli-v2/crates/bro-core/src/orchestrator.rs:302), [inheritance and deduplication](F:/bro-cli-v2/crates/bro-tui/src/app/sessions.rs:398), [bridge API](F:/bro-cli-v2/crates/bro-bridge/src/orchestrator/tools.rs:1191).
- Orchestrators can list, read, name, focus, open, close, and send input or keys to sessions; answer detected questions; inspect chat transcripts; inspect usage; schedule future prompts; clear worker or their own context; and record watchdog milestones. Claude and Codex use MCP, with `bro ctl` as a fallback; Pi and omp use the CLI route. [Tool reference](F:/bro-cli-v2/crates/bro-core/src/orchestrator.rs:146), [harness access](F:/bro-cli-v2/crates/bro-core/src/orchestrator.rs:55), [tool schemas](F:/bro-cli-v2/crates/bro-bridge/src/orchestrator/tools.rs:34).
- A worker's assignment is optional free text in `open_session.task`, sent through terminal paste. Its schema has no Bro task ID, dependencies, acceptance checks, resource claims, timeout policy, or result schema. The `taskSent` flag records attempted delivery, not worker acknowledgement or successful completion. [Open schema](F:/bro-cli-v2/crates/bro-bridge/src/orchestrator/tools.rs:91), [task delivery](F:/bro-cli-v2/crates/bro-bridge/src/orchestrator/tools.rs:1174), [terminal typing](F:/bro-cli-v2/crates/bro-bridge/src/orchestrator/tools.rs:1031).
- The standing prompt already asks for independent parallel work, ideally in separate projects or worktrees, with complete briefs and verification. These are instructions to the model. The inspected `open_session` schema accepts a folder, but has no managed worktree lifecycle or resource reservation. Existing transcript labels understand Claude's `EnterWorktree`/`ExitWorktree` tools, and repository identity groups worktrees, so worktree awareness exists. [Playbook](F:/bro-cli-v2/crates/bro-core/src/orchestrator.rs:175), [session schema](F:/bro-cli-v2/crates/bro-bridge/src/orchestrator/tools.rs:91), [transcript labels](F:/bro-cli-v2/crates/bro-core/src/sessions/conversation.rs:1115), [repository-aware peers](F:/bro-cli-v2/crates/bro-core/src/peers.rs:12).
- Watchdogs already hold milestones with pending/active/done states and evidence notes, limit wake count, record reviewer runs, coordinate helper sessions, and retry failed reviews with backoff. The trigger waits when the watched session or helpers are busy and can wake on a question. Default plan is 25 wakes and 60 seconds of idle time. These are existing bounded supervision mechanisms. [Plan](F:/bro-cli-v2/crates/bro-bridge/src/watchdog/mod.rs:55), [wake claims and limits](F:/bro-cli-v2/crates/bro-bridge/src/watchdog/mod.rs:302), [retry and helper logic](F:/bro-cli-v2/crates/bro-tui/src/watchdog/mod.rs:74), [app trigger](F:/bro-cli-v2/crates/bro-tui/src/app/watchdogs.rs:164).
- There is already an event backbone: the bridge has a broadcast channel, terminal output events, session observations, activity transitions, and watchdog update sinks. The orchestration wait tools still poll session state. A durable task supervisor can consume the existing events instead of requiring a replacement terminal host. [Broadcast channel](F:/bro-cli-v2/crates/bro-bridge/src/state.rs:56), [observations](F:/bro-cli-v2/crates/bro-bridge/src/state.rs:202), [watchdog sinks](F:/bro-cli-v2/crates/bro-bridge/src/watchdog/mod.rs:223), [polling wait](F:/bro-cli-v2/crates/bro-bridge/src/orchestrator/tools.rs:967).
- Timers support session and persistent schedules. Persistent occurrences use exclusive claim files to choose a single firing Bro process. This is timer occurrence deduplication, not proof of successful execution. [Schedule storage](F:/bro-cli-v2/crates/bro-bridge/src/schedule.rs:120), [occurrence claims](F:/bro-cli-v2/crates/bro-bridge/src/schedule.rs:195), [delivery](F:/bro-cli-v2/crates/bro-bridge/src/schedule.rs:252).
- Session survival already exists: the host stores live/lost session metadata for recovery, and the UI restores each orchestrator's saved launch configuration. The bridge also saves conversation items, aggregate usage, and a Claude resume ID. Those persisted structures are not a task ledger. [Host recovery](F:/bro-cli-v2/crates/bro-host/src/store.rs:1), [orchestrator state](F:/bro-cli-v2/crates/bro-tui/src/orchestrator_state.rs:10), [restoration](F:/bro-cli-v2/crates/bro-tui/src/app/sessions.rs:365), [conversation history](F:/bro-cli-v2/crates/bro-bridge/src/orchestrator/mod.rs:122).
- Claude agent teams expose tasks, owners, and `blocked_by` dependencies in Bro. That module explicitly reads Claude's team files without writing them. It is a provider-specific read model, not Bro's cross-harness task scheduler. Keep this display and map it into any common task view where possible. [Team reader](F:/bro-cli-v2/crates/bro-core/src/agent_teams.rs:1), [task fields](F:/bro-cli-v2/crates/bro-core/src/agent_teams.rs:67), [dependency view](F:/bro-cli-v2/crates/bro-core/src/agent_teams.rs:151).

### Inferences

- The best architectural direction is a small, deterministic task/attempt/result runtime around Bro's existing CLI sessions. The LLM proposes decomposition, assignments, and repairs; code owns task state, delivery, dependencies, budgets, and evidence requirements.
- A session should remain a reusable execution context. It can perform several attempts over its lifetime and should not be synonymous with one task.

### Gaps

- No runtime failure frequency, completion rate, token overhead, or concurrency benefit was measured. This audit supports design priorities, not performance promises.

## Where can multitasking lose work or misreport completion

### Takeaway

Several API outcomes describe terminal quietness or input delivery as if they were work outcomes. Inbox answer attribution and explicit readiness are the first correctness fixes because more concurrent work makes these ambiguities more costly.

### Cited findings

| Static observation | Inferred consequence and precise limit | Source |
|---|---|---|
| `pump_voice` allows `can_say` for both Idle and Working, then drains successive `Say` entries. Each awaiting request stores only the previous latest reply and an elapsed timer. | Several requests may be submitted during one turn. When the next fresh reply appears, multiple waiting request records can receive that same text. This has not been reproduced against a live harness. | [Delivery](F:/bro-cli-v2/crates/bro-tui/src/app/voice.rs:246), [attribution](F:/bro-cli-v2/crates/bro-tui/src/app/voice.rs:282) |
| `wait_until_ready` returns `Ok(None)` after its 45-second loop, and `open_session` interprets `None` as permission to type. An undetected but quiet program also qualifies after six seconds. | A slow launch can be reported as taskSent even without a positive ready signal. The worker may still accept it later, but the API does not distinguish the uncertainty. | [Readiness](F:/bro-cli-v2/crates/bro-bridge/src/orchestrator/tools.rs:1079), [delivery](F:/bro-cli-v2/crates/bro-bridge/src/orchestrator/tools.rs:1174) |
| `wait_for_sessions` considers a missing session settled, and a present session settled when activity is anything except working after 1.5 seconds of quiet. The outcome says a tab/every tab finished. The returned per-session state separately says closed or awaiting. | Callers can confuse an ended turn, a question, or a closed worker with successful task completion. This is an outcome-contract ambiguity, not a claim that the function itself marks a task done. | [Settled predicate](F:/bro-cli-v2/crates/bro-bridge/src/orchestrator/tools.rs:974), [outcome](F:/bro-cli-v2/crates/bro-bridge/src/orchestrator/tools.rs:989), [detailed states](F:/bro-cli-v2/crates/bro-bridge/src/orchestrator/tools.rs:1004) |
| `watchdog_update` can mark a milestone done with no evidence field, and standDown sets finished without checking milestones. | Completion is dependent on reviewer discipline. A paused or stopped workflow needs a distinct outcome so an allowed early stop does not masquerade as verified completion. | [Update logic](F:/bro-cli-v2/crates/bro-bridge/src/watchdog/mod.rs:389) |
| `send_input` calls `type_text` immediately; no busy-state check or durable worker inbox exists in that branch. The prompt says to queue ordinary messages until idle. | Correct busy-session behavior depends on the caller and harness queue behavior. Add explicit assignment/steering semantics; do not block authorized steering. | [Send branch](F:/bro-cli-v2/crates/bro-bridge/src/orchestrator/tools.rs:598), [prompt rule](F:/bro-cli-v2/crates/bro-core/src/orchestrator.rs:232) |
| `resolve_worker` refuses only an orchestrator in the global workspace. It does not identify the caller's own project coordinator. | A project coordinator can target its own session through this generic API despite the prompt's self-target prohibition. This is a static guard gap; source-aware caller identity is needed while retaining authorized cross-coordinator messages. | [Guard](F:/bro-cli-v2/crates/bro-bridge/src/orchestrator/tools.rs:380), [scope instructions](F:/bro-cli-v2/crates/bro-core/src/orchestrator.rs:121) |
| The UI inbox uses in-memory vectors. Bridge history persists items and usage, but reconstructs its queue empty; watchdog entries are also held in memory. | Session survival alone cannot reconstruct pending assignments, request attribution, review state, or retry policy after the owning app dies. | [UI inbox](F:/bro-cli-v2/crates/bro-tui/src/app/voice.rs:62), [bridge history](F:/bro-cli-v2/crates/bro-bridge/src/orchestrator/mod.rs:122), [queue reconstruction](F:/bro-cli-v2/crates/bro-bridge/src/orchestrator/mod.rs:246), [watchdog registry](F:/bro-cli-v2/crates/bro-bridge/src/watchdog/mod.rs:215) |
| Schedule `due` claims and advances an occurrence before sending a command into the UI inbox. Occurrences more than 60 seconds late are skipped. | A crash between claim and durable delivery can lose a scheduled operation. The current behavior suits best-effort reminders; required work needs acknowledgement and an explicit catch-up policy. | [Due policy](F:/bro-cli-v2/crates/bro-bridge/src/schedule.rs:215), [command send](F:/bro-cli-v2/crates/bro-bridge/src/schedule.rs:264) |

### Inferences

- Normal request delivery should create a durable envelope with request ID, target scope, task/attempt ID where relevant, delivery sequence, and acknowledgement state. Match the response to the envelope. A compatibility fallback can allow only one uncorrelated request in flight per coordinator.
- Keep `wait_for_sessions` as an observation tool. Give its outcomes neutral wording such as `state_changed` and `timed_out`, and use a separate task-result API for verified success.
- Readiness should return a typed outcome: ready, blocked on prompt, exited, or timed out. A timeout should leave task delivery pending and inspectable.

### Gaps

- The UI inbox behavior above was not exercised with simultaneous panel, voice, schedule, and watchdog inputs. Harnesses may internally queue terminal messages, but that does not by itself solve Bro's response correlation.
- Current raw terminal escape/input APIs are useful for people and recovery. They should remain available while structured task operations become the standard path.

## What should be built first and where

### Takeaway

Start with delivery and result correctness, then add persistence and scheduling. Bro's current event channel, launchers, and watchdog logic are suitable integration points.

### Cited findings

The proposals below use the existing [tool dispatcher](F:/bro-cli-v2/crates/bro-bridge/src/orchestrator/tools.rs:562), [UI inbox](F:/bro-cli-v2/crates/bro-tui/src/app/voice.rs:134), [bridge event channel](F:/bro-cli-v2/crates/bro-bridge/src/state.rs:56), [host persistence](F:/bro-cli-v2/crates/bro-host/src/store.rs:15), and [watchdog states](F:/bro-cli-v2/crates/bro-bridge/src/watchdog/mod.rs:27). They are design recommendations, not implemented features.

| Order | Proposed change | Dependencies and integration | Acceptance scenario and risk |
|---|---|---|---|
| 1 | Fix inbox correlation and explicit launch readiness. | Add request envelopes around `VoiceRoute`, preserve origin IDs through terminal delivery, and use typed readiness in tools.rs. Begin with one ordinary request in flight where a harness cannot identify turns. | Send two different panel requests while a coordinator works; each receives its own result after restart. A slow worker stays pending, and no task is typed after readiness timeout. Risk: harnesses expose different queue/turn behavior. |
| 2 | Introduce shared Task, Attempt, Result, and Artifact records. | Add a provider-neutral domain module near bro-core orchestration code and a bridge store/service. IDs belong to work, separately from session IDs. Store goal, scope, acceptance checks, dependencies, revision, owner, status, and remaining budget. | A worker turn ends or closes without Result: task remains unverified. Reusing a worker for a second task cannot overwrite the first task's history. Risk: avoid duplicating Claude's task IDs; retain provenance and adapters. |
| 3 | Persist an append-only event journal plus current projections and durable inbox/outbox. | Task store becomes the source of truth; commit intent before terminal dispatch. Reconcile recovered host sessions with attempts. A local transactional database is a candidate, not a mandated dependency. | Crash after dispatch, before acknowledgement, and after result submission: recover without blind duplicate external effects. Use idempotency keys for operations with reliable deduplication and require reconciliation where delivery remains uncertain. Risk: do not promise exactly-once arbitrary shell effects. |
| 4 | Build an event-driven supervisor with leases and bounded recovery. | Consume session/activity/exit events and typed task events; adapt the existing watchdog retry and wake budget. Persist heartbeats/checkpoints and distinguish waiting on tools, questions, rate limits, and actual stalls. | A stalled worker is noticed without an LLM polling loop. An expired lease cannot allow a late worker to overwrite a newer attempt. Risk: quiet long-running tools need progress signals; inactivity is not failure. |
| 5 | Schedule ready tasks from a dependency graph with explicit resource claims. | Build on Tasks and the journal. Use conservative dependency edges, per-project work limits, estimated task cost, login headroom, and resource names for repo writes, lockfiles, ports, migration targets, browsers, and builds. Reserve capacity for review/integration. | Independent research runs together; two writers sharing one folder cannot both own it. A database migration or release build has one owner. Risk: excessive locking can erase the benefit of parallelism. |
| 6 | Manage worktrees and integration as task resources. | Add worktree create/reuse/cleanup operations and a serialized integration lane; attach branch/base revision and untracked-change metadata to results. Keep people’s existing dirty trees untouched. | Two independent code tasks build on separate worktrees, integrate serially, and run combined checks on the merged revision. Cleanup preserves sessions and uncommitted artifacts. Risk: generated files, caches, and external services still need separate claims. |
| 7 | Enforce quality profiles before a task can become verified. | Acceptance checks are declared before execution. Store exit status, command, revision/diff hash, artifact links, reviewer verdict, and unresolved defects. Use a separate reviewer for consequential changes and a UI evidence profile for visible changes. | A worker saying tests pass cannot close the task without matching evidence. Editing after the test invalidates affected evidence. Both child tasks passing still leaves the parent waiting for integration checks. Risk: evidence should be proportional to task impact. |
| 8 | Add task context packages and checkpointed memory. | Build packages from the task contract, current plan, user constraints, relevant files, decisions, and artifact references. Keep authoritative state outside conversation text. Generate `clear_own_context` handoffs from the ledger. | A coordinator with a fresh conversation can reconstruct active work and promises without reading every transcript. Risk: stale summaries require revision/provenance fields; untrusted source text must stay data. |
| 9 | Add measured routing and budget allocation. | Extend cached usage selection with task class, observed success, review cost, and recovery cost. Enforce per-goal attempt/time/token ceilings and reserve resources for finishing work. | A depleted login stops receiving new heavy tasks; its active task is checkpointed and reconciled before reassignment. Risk: cached usage is not current billing truth; use confidence/freshness and conservative fallback. |
| 10 | Provide one goal view and an evaluation harness. | Project current task graph, blockers, next ready actions, evidence, cost, and attempt lineage into the UI; make a replay corpus from anonymized local runs and deterministic fault cases. | User can see which task prevents completion and why. Compare verified throughput, recovery success, merge conflicts, rework, and user interventions against today's baseline. Risk: avoid making a busy terminal or many agents a success metric. |

### Inferences

- The first useful vertical slice is: submit goal → create task → assign attempt → acknowledge delivery → return structured result → run acceptance checks → verify → notify. It can use one worker initially and still establish the right contract for parallelism.
- A parent task should become verified only after its required children and the parent-level integration criteria are verified. Cancelled, abandoned, failed, and blocked are separate terminal or waiting outcomes.
- Hierarchy should remain shallow by default. Existing root/project coordinators provide ownership boundaries; task graphs handle most within-project dependencies without extra manager agents.
- Prompt improvements can happen alongside this work: ask for explicit assumptions, file ownership, acceptance checks, stop conditions, and evidence. The existing playbook already contains the basic brief/verify/review loop, so stronger enforcement has more value than restating it.

### Gaps

- A precise database choice, default concurrency, scheduling formula, and model policy need implementation experiments. This audit does not establish useful fixed numbers.

## What existing tests establish and what needs regression coverage

### Takeaway

Bro has tests for important orchestration primitives, including the new hierarchy. The highest-value new tests target attribution, recovery, and the distinction between a stopped worker and a finished task.

### Cited findings

- Existing tests exercise project coordinator creation/delegation, inherited configuration and live model choice, deduplication, scope-specific state restoration, and guest restrictions. They should remain requirements through any runtime redesign. [Bridge hierarchy test](F:/bro-cli-v2/crates/bro-bridge/src/tests.rs:1203), [UI hierarchy tests](F:/bro-cli-v2/crates/bro-tui/src/app/tests.rs:4096), [inheritance](F:/bro-cli-v2/crates/bro-tui/src/app/tests.rs:4140), [restoration coverage](F:/bro-cli-v2/crates/bro-tui/src/app/tests.rs:4341).
- The self-target test registers a global coordinator without a project directory; it checks send_input, keys, close, and wait denial. It does not establish caller-aware protection for a project coordinator. [Existing test](F:/bro-cli-v2/crates/bro-bridge/src/tests.rs:1239).
- Inbox tests cover one voice result, one panel awaiting record, schedule delivery, cancellation, launch failure, and ordered context clearing. The context-clear test intentionally allows the handoff and a later status message to be sent during the same pump once the hold expires. These tests do not prove concurrent request-to-reply attribution. [Voice result](F:/bro-cli-v2/crates/bro-tui/src/app/tests.rs:4457), [panel and schedules](F:/bro-cli-v2/crates/bro-tui/src/app/tests.rs:4552), [context clear order](F:/bro-cli-v2/crates/bro-tui/src/app/tests.rs:4569).
- Existing wait coverage checks idle sessions and expects the phrase `every tab finished`. That validates the present observation contract, not verified task success. [Wait test](F:/bro-cli-v2/crates/bro-bridge/src/tests.rs:1974).
- The watchdog test marks one milestone done, adds a third milestone, changes active state, then accepts standDown with other work unfinished. This confirms that standDown is currently a general stop operation, despite wording that mentions verified completion. Keep general stopping available and distinguish its reason/state from completion. [Watchdog test](F:/bro-cli-v2/crates/bro-bridge/src/watchdog/mod.rs:474).

### Inferences

Proposed focused regression scenarios:

1. Queue two panel messages plus a watchdog wake while the coordinator is working. Complete only one request. Assert that only its response handle is fulfilled; the other two remain pending. Repeat across restart and context reset.
2. Keep a newly opened agent continuously working beyond the ready timeout. Assert an explicit timeout, `taskSent=false`, no terminal paste, and a recoverable pending assignment. Add separate blocked-on-question and exited-at-boot cases.
3. Wait on a worker that asks a question and another that exits. Assert that observation results preserve awaiting/closed, and neither can satisfy the task completion predicate. A normal idle worker without a structured Result also remains unverified.
4. Try to complete a watchdog/task with pending milestones, missing check artifacts, failed checks, or evidence for an old revision. Assert rejection of verified completion. An explicit stop remains allowed and visible as stopped.
5. Have a project coordinator target itself, the root, another coordinator, and a worker. Assert caller-aware self-denial with intended delegation paths retained.
6. Crash the supervisor at each dispatch/ack/result boundary; replay duplicate and stale attempt events; expire a lease; test that late results cannot rewrite the current attempt.
7. Reserve a repo write, shared port, or integration slot from two tasks; only one should receive the conflicting claim. Two read-only tasks should still run together.

### Gaps

- Test presence was established by reading source. No test suite was executed, so pass status is unknown.

## How to rebuild and install after this research

### Takeaway

The parent must build both release binaries and replace the copies on the active PATH without stopping existing sessions. The repository installer already demonstrates the Windows rename approach.

### Cited findings

- AGENTS.md requires `cargo build --locked --release -p bro-tui --bins`, installing both `bro.exe` and `bro-app.exe`, matching installed hashes to the build, and checking a fresh process while existing sessions keep running. [Repository instructions](F:/bro-cli-v2/AGENTS.md:4).
- The inherited `BRO_BIN` and `where.exe bro` resolve to `C:\Users\james\AppData\Local\Programs\bro\bro.exe`; `where.exe bro-app` resolves to the sibling `bro-app.exe`. This is an observed environment fact from the audit, not a hard-coded destination recommendation. The installation documentation gives that folder as the Windows default. [Install destination](F:/bro-cli-v2/docs/install.md:36).
- `install.ps1` stages a sibling `.<name>.new`, renames the current executable to an `.old` backup because a running Windows executable cannot be overwritten, then renames the staged file into place. If an old backup is locked, it uses a PID-specific backup name. [Installer implementation](F:/bro-cli-v2/install.ps1:68).
- `bro --version` starts a fresh process and exits successfully through the version branch. `bro-app` forwards arguments to `bro gui --gui-child`, so a graphical smoke check must account for launcher behavior. [Version branch](F:/bro-cli-v2/crates/bro-tui/src/main.rs:112), [desktop launcher](F:/bro-cli-v2/crates/bro-tui/src/bin/bro-app.rs:52).

### Inferences

- Parent sequence: build once after all research artifacts are finished, resolve active executable paths again, stage each build, verify staged hashes, perform the sibling rename swap, compare installed/build SHA-256 hashes, run installed `bro --version`, and inspect a fresh graphical process only if needed. Keep existing PIDs and sessions alive. Preserve backups if locked; no server restart is needed for file replacement.

### Gaps

- This research agent performed no build, install, process launch, or runtime smoke check; the parent owns those required steps.
