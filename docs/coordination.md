# Bro coordination

Bro keeps durable outcomes alongside its persistent agent sessions. A task records what should be delivered, its prerequisites and acceptance evidence. A session records an agent's activity. An idle session can have unfinished work; a submitted result can still need review.

Open **Tasks · outcomes** in the sidebar, **Ctrl+Space v**, or **Alt+Shift+D**. The same task state appears in the desktop app, terminal UI and bridge. The pane reads snapshots on a background worker, keeps the last good snapshot during an error, and refreshes when state changes. Rendering does not scan the repository, hash files or make network requests.

## Define an outcome

Choose **New** and enter the outcome, a review check that describes observable success, and the project folder. **Tab** moves between fields, **Ctrl+Q** changes the quality mode, and **Ctrl+Enter** saves. Creation commits the intent and task together; a rejected contract does not leave an orphan intent.

The form creates a review criterion because a natural-language check has no exact executable command. The coordinator can add command or artifact criteria through the task tools. A command criterion must name the command that will run. A review criterion still requires an observed review operation and a recorded output identity before acceptance.

| Mode | Intended use | Completion requirement |
| --- | --- | --- |
| Quick | Small reversible work with limited impact | Focused observable evidence; applicable project instructions |
| Standard | Behavior changes and ordinary features | Current evidence for every required criterion; appropriate integration checks |
| Critical | Work whose failure can cause substantial damage | Required observable evidence plus an independent review criterion |

Quality modes do not grant additional authority or replace repository requirements. The user's existing instructions continue across retries and context replacement. For this repository, completion includes rebuilding both release binaries, installing them to the active `PATH` location, matching installed hashes to the build and checking a fresh Bro process while existing sessions keep running.

## Understand the list

The default list shows active outcomes, with work needing recovery or review first. **Finished** (`a`) includes accepted and cancelled work. The header counts queued, working, waiting and review outcomes separately from accepted or failed tasks.

| Label | Meaning | Next step |
| --- | --- | --- |
| Queued | Prerequisites, capacity, capabilities or resources may prevent assignment | Inspect the next action and prerequisite outcomes |
| Ready | The controller sees an eligible assignment | Coordinator selects a capable worker |
| Assigned | An attempt and delivery intent exist | Host dispatches and records delivery acknowledgment |
| Working | An attempt is running | Record meaningful progress, a blocker, or a structured candidate |
| Waiting | A named blocker has a wake condition | Resolve that condition; other eligible work can continue |
| Needs recovery | Dispatch, actor ownership or execution is uncertain | Reconcile the previous actor and external actions before retry |
| Needs review | A candidate was submitted | Check the actual artifact and required criteria |
| Checking | Verification is under way | Record the observed evidence |
| Accepted | The submitted artifact has current passing evidence for required criteria | Deliver the accepted outcome and required artifacts |
| Stopping | Cancellation was requested | Confirm the worker stopped or was isolated |
| Failed | An attempt failed or exhausted a limit | Inspect the issue; retry only when allowed |
| Cancelled or Stopped | Execution ended without claiming acceptance | Preserve useful partial artifacts and history |

Select a task to see its project, worker, blocker and wake condition, latest progress, acceptance criteria and candidate artifacts. **Page Up/Down** scrolls the detail. **Details** (`d`) reveals technical IDs and evidence digests. **Context** (`c`) shows the handoff checkpoint, prerequisites and valid memory, followed by recovery notices and outcome metrics. Missing provider cost data stays unknown; reported cost can cover only some sessions.

## Change or finish work

**Steer** (`s`) updates the selected contract using its observed revision. It preserves the remaining criteria, source identities and quality requirements. A concurrent revision causes an explicit conflict instead of silently overwriting another change. Relevant downstream acceptance evidence is invalidated. A running writer must reconcile its previous assignment before a replacement receives mutation ownership.

**Cancel** (`x`) records a task cancellation request. It does not immediately release a live worker's resources. **Retry** (`r`) checks budgets and ownership before reopening execution. **Review** (`v`) queues a task-specific request for the coordinator to inspect the candidate, run or observe its required verification and finish eligible work. The runtime requires the task's worker to be stopped before final acceptance; candidate resource claims remain held through the final content capture and acceptance. **Retire worker** (`z`) queues safe retirement of that task's worker. Existing user sessions require an explicit user-directed stop, and disconnected or idle sessions do not prove exit. These requests preserve unrelated sessions and do not mark the task accepted by themselves.

Task changes wake the coordinator through the bridge, including changes made from the CLI or HTTP. Wake messages carry stable task/revision/action identities and are coalesced in the inbox. The coordinator reads current state before acting, so a queued wake does not force an obsolete plan. Ordinary prompts wait until the orchestrator is ready.

## Task tools

`bro ctl` lists the currently available tools and schemas. All coordination writes use the same `bro_core::coordination::Engine`. Its commands are tagged by `op`, with camelCase JSON fields. A stable `commandId` deduplicates an identical command; reusing it with different content is rejected.

```powershell
bro ctl coordination_snapshot
bro ctl coordination_context taskId=<task-id>
bro ctl coordination_command '{"commandId":"cancel-1","command":{"op":"cancel","taskId":"task-id-from-snapshot","reason":"Cancelled by user"}}'
```

The bridge has `GET /api/coordination` and `POST /api/coordination/command`. The command envelope is `{ "commandId": "unique-command-id", "command": { "op": "..." } }`. Snapshot queries are paginated, with `offset`, `limit` (at most 100) and `nextOffset`. The native Tasks pane uses the bounded full engine snapshot, so a later page is not silently hidden.

| Tool | Purpose |
| --- | --- |
| `coordination_ready` | Eligible tasks in scheduler order; snapshot order is not dispatch order |
| `coordination_dispatch` | Claim an existing task and persist its delivery before sending it to an idle worker |
| `coordination_ack` | Worker acknowledges the exact assignment with its attempt token |
| `coordination_progress`, `coordination_block`, `coordination_checkpoint` | Meaningful progress, named blockers and recovery handoffs |
| `coordination_submit` | Submit actual artifact locations; Bro captures their content identity |
| `coordination_inspect` | Read current candidate evidence and obtain a content-bound review token |
| `coordination_verify` | Run the explicit authorized check, hash an artifact, or record concrete review findings |
| `coordination_retire_worker` | Stop the actual task-owned process and retain candidate claims for acceptance |
| `coordination_accept` | Recapture final bytes and accept current evidence after the worker is stopped |
| `coordination_worktree` | Provision or reconcile a Bro-owned worktree with explicit integration ownership |
| `coordination_memory` | Promote a project fact backed by actual source paths and content digests |

Use the running tool schema for each operation's required fields. To execute an already-created task, open a worker bound to that task ID and dispatch it; do not create a second task by repeating its instructions as a new assignment. Review decisions are model judgments with recorded findings and content identity. They complement executable checks; they do not prove the absence of every defect.

Generic coordinator commands create and revise work, record blockers or wake conditions, and manage policy. They cannot manufacture trusted host verification or stop proof. Worker mutations belong to an assigned attempt and must carry the current attempt identity and epoch. Managed CLI calls send `BRO_SESSION_ID` as `x-bro-session-id` and `BRO_TASK_TOKEN`, when present, as `x-bro-task-token`. A per-attempt token can also be supplied through the worker protocol for an already-running session. Tokens are credentials; normal tool output and the task detail do not display them.

Use task result and verification tools for completion. A worker's freeform success message is a candidate claim. The host observes checks or review operations against the submitted artifact identity. An accepted task must have passing current evidence for each required criterion, with no unresolved defects. Source and dirty-tree content identity include actual bytes; Git `HEAD` alone does not identify uncommitted changes. Relevant later edits invalidate evidence and require the affected checks again.

## Scheduling and shared resources

The scheduler considers dependencies, priority with aging, project fairness, global/project limits, a reserve for completion work, adapter availability and demonstrated capabilities. Unknown capability support is not treated as success. Attempts have configured leases and budgets; reported token or cost limits depend on provider observability. A stopped or expired process does not by itself establish task acceptance.

Resource claims use canonical resource identities and either shared-read or exclusive access. Conflicting mutation work waits. A live or uncertain writer retains its claims until the host observes its stop or isolation. Concurrent code writers should use isolated worktrees with explicit integration ownership. The shared browser profile, device, development port and installation location also need suitable ownership.

An epoch prevents a stale completion from changing current task state. It does not prevent an arbitrary shell from editing a shared file, or an uncontrolled browser from navigating. Strong mutation protection requires isolation, a tool boundary that enforces ownership, or confirmed stopping before reassignment. The controller does not provide exactly-once guarantees for arbitrary external operations. After a crash following an external side effect, inspect the destination or use its idempotency key before retrying. If the outcome cannot be established, retain the uncertainty.

## Persistence and recovery

The engine stores versioned authority under the bridge data root's `coordination/v1` directory. Mutations reload under an operating-system file lock and append checksummed journal records. Compaction writes a snapshot, and recovery preserves uncertainty instead of replacing corrupt authority with an empty task list. Admission and retention limits bound the working set. Archive finished intents when the active ledger reaches its configured limit.

Context packets contain the current task/intent, a checkpoint, dependencies and source-valid memory. Checkpoints should record constraints, observed artifacts and failed checks, followed by the next useful action. A fresh context does not imply previous checks ran again.

A delegated task preserves its request origin in a structured context entry: `originRequestId`, `replyChannel` and `replyHandle`. The initial conversational reply acknowledges the request. An accepted task produces a separate **Accepted outcome** item related to the original panel turn, with a deterministic task/revision identity. Delivery can retry after a restart without consuming a new pending request. If a voice delegation has ended, the durable accepted outcome remains inspectable in Tasks and the orchestrator transcript instead of being attributed to a later voice request.

The orchestrator inbox persists queued requests and uncertain sent requests separately from the engine. Each request has a stable identity; a final response carries its matching response marker. When an adapter has no correlated turn channel, ordinary requests are serialized. A changed latest reply can resolve only its matching request. Busy, blocked and unknown prompt states do not receive ordinary assignment text. Context clearing waits for the current answer and preserves queued work. Shutdown preserves in-flight requests; a restart waits for their original response or explicit cancellation instead of resending blindly.

Explicit phone/web controls use `messageKind: "answer"` or `"steer"` with the original pending response item's ID as `requestId`. An answer targets a blocked request; steering targets the identified active session. The control acknowledges delivery of that input and keeps the original result handle separate. Ordinary messages remain new assignments. Inbox saves run on a background worker, and dispatch waits for a durable save acknowledgment. A lifetime operating-system lock prevents overlapping hosts from loading and dispatching the same saved inbox.

## Verification scenarios

These commands run against isolated test state. They do not need a provider login or live user sessions.

```powershell
cargo test --locked -p bro-core coordination::tests
cargo test --locked -p bro-tui --bin bro inbox_
cargo test --locked -p bro-tui --bin bro tasks_view_
cargo test --locked -p bro-tui --bin bro task_form_
cargo test --locked -p bro-tui --bin bro managed_calls_forward_identity
```

The core suite covers durable lifecycle and evidence, deduplication across reopen, uncertain dispatch and torn journal recovery, corruption, stale writers, resource conflicts, graph validation, revisions, budgets, cancellation, fairness, context invalidation and multiple processes sharing authority. The inbox tests cover two simultaneous requests, unrelated and delayed replies, readiness, context clearing and cancellation. The form test submits each quality mode through the real engine so the UI cannot create an invalid criterion or orphan intent.

For a repeatable visual check, launch a fresh isolated demo process. Examples are explicitly labeled and never modify real tasks or profiles:

```powershell
$env:BRO_GUI_SCRIPT = 'wait:800;key:ctrl+space;key:v;wait:500;shot:F:/bro-cli-v2/target/tasks-wide.png;key:c;wait:300;shot:F:/bro-cli-v2/target/tasks-context.png;key:n;wait:300;shot:F:/bro-cli-v2/target/tasks-new.png;exit'
.\target\release\bro.exe gui --demo --new --gui-child
Remove-Item Env:BRO_GUI_SCRIPT
```

Inspect a wide window and a narrow split: task titles and long project paths should remain readable, controls wrap, the selected outcome remains clear, and all actions remain available by keyboard. Test an empty ledger, a bridge connection error, a long blocker and a task with multiple criteria. A connection error should preserve the last good task list and give a recovery action.

For concurrency evaluation, run A while B waits for a dependency and C awaits review. Steer A. Check that only A's revision changes, B retains its wake condition, C remains reviewable and unrelated work continues. Deliver A's old completion after reassignment and confirm rejection. Kill the controller after a dispatch intent and after a simulated external side effect; confirm that recovery does not blindly replay either. Exercise two tasks requiring the same browser/worktree resource and verify serialized or isolated mutations.

Measure accepted outcomes per hour together with acceptance quality, first useful result latency, queue/execution/review wait, user correction work, recovery success and cost per accepted result. Compare the same task mix alone and with competing work to detect interference. Repeat stochastic tasks, retain a held-out set and record model/adapter versions. Add contracts, verified acceptance, recovery, concurrency and review policy in separate ablations so improvements can be attributed. No throughput or quality gain is claimed until a local baseline and comparison have been recorded.
