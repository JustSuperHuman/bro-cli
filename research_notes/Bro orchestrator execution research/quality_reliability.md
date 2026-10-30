# Bro task quality and reliability

Research date: 2026-10-29. Source versions and publication dates are recorded below. This is research and a proposed design for Bro; it does not claim that these mechanisms are implemented or that Bro has achieved measured gains. The repository audit supplies the implementation findings.

## Which failures should Bro explicitly prevent

### Takeaway

A useful orchestrator must distinguish a worker finishing its turn from the user's task being accepted. Coordination, task state, and verification deserve separate failure handling.

### Cited Findings

- MAST identifies 14 failure modes across system design, inter-agent misalignment, and task verification. Its final paper reports 1,642 annotated traces from seven frameworks, with the taxonomy developed through expert analysis of 150 traces. Relevant modes include repetition, lost history, task derailment, ignored information, premature termination, missing verification, and incorrect verification. The dataset covers coding, math, and general tasks; its frequencies should not be treated as Bro's failure distribution. First submitted 2025-03-17, version 3 dated 2025-10-26. [MAST paper](https://arxiv.org/html/2503.13657v3)
- MAST's intervention studies show that clearer prompts and specialized roles have inconsistent benefits across models and arrangements. Verification flaws can also appear in apparently successful runs. Additional agents or instructions therefore need evaluation against actual outcomes. [MAST paper](https://arxiv.org/html/2503.13657v3)
- The latest scaling study is version 3, dated 2026-04-08. It updates the earlier version behind Google's January 2026 blog; use the architecture researcher's notes for current experiment counts and results. Its architecture comparison supports conditioning coordination on task structure. [Scaling agent systems version 3](https://arxiv.org/html/2512.08296v3)
- Anthropic's 2025-11-26 harness experiments describe agents attempting too much within one context window, leaving confusing partial implementations, and declaring completion after seeing partial progress. Their approach uses incremental features, persistent progress artifacts, and explicit end-to-end verification. Compaction alone did not solve continuity. [Long-running agent harnesses](https://www.anthropic.com/research/effective-harnesses-for-long-running-agents)

### Inferences

Proposed Bro failure classes should make the next action clear:

| Failure class | Observable evidence | Appropriate response |
| --- | --- | --- |
| Worker unavailable | Session exited, transport broken, process identity changed | Reconcile artifacts and attempt identity, then resume or replace |
| Task blocked | Named dependency, missing input, unavailable credential or resource | Record blocker and wake condition; release unrelated capacity |
| Task stalled | Repeated failed tool/result signatures without a new hypothesis or acceptance progress | Inspect, narrow scope, change strategy, or request missing information |
| Result incomplete | Worker says finished but a required artifact or criterion is absent | Return a specific gap to the worker |
| Result incorrect | Reproduction, state check, or reviewer finds a defect | Reopen the task with evidence and bounded repair scope |
| Integration failure | Individually accepted changes conflict in the combined artifact | Assign integration owner and rerun affected checks on combined revision |
| Control conflict | Two workers hold incompatible access to a file tree, browser, device, port, or deployment | Enforce ownership or serialize access |
| Stale result | Completion refers to an old attempt or outdated task revision | Retain it in history; reject its authority to change current task state |

An output heartbeat establishes activity. A healthy process establishes liveness. Useful progress needs a task-specific signal: an artifact produced, a failing case reproduced, an acceptance criterion newly satisfied, a blocker removed, or a documented hypothesis change. Reading and thinking can be necessary progress, so silence or unchanged files should trigger investigation according to the task type, not automatic termination.

Preserve both a durable user goal and separate execution tasks. A worker may complete a subtask while the goal still needs review, integration, installation, or delivery. A goal can have partial results without being complete.

### Gaps

The literature does not establish a universal ideal team size, stall timeout, retry count, or review depth. Bro needs measurements by task class and agent adapter. A research taxonomy cannot establish the prevalence of any failure in Bro without local traces.

## What evidence should count as completion

### Takeaway

Define acceptance before delegation and bind verification to the delivered artifact. Give simple tasks a small contract; increase the checks when the change can break important behavior.

### Cited Findings

- Anthropic separates a transcript from the outcome: the agent's claim that an action succeeded differs from the resulting environment state. Its evaluation guidance combines code-based checks, model rubrics, and human judgment; model graders require human calibration. It recommends clear task criteria and reference solutions. Published 2026-01-09. [Agent evaluation guidance](https://www.anthropic.com/research/demystifying-evals-for-ai-agents)
- The original MT-Bench judge study documents position, verbosity, and self-enhancement biases alongside useful agreement with human preferences. This concerns conversational judgment; it does not establish that a model can certify program correctness. NeurIPS 2023. [Judging LLM judges](https://papers.neurips.cc/paper_files/paper/2023/hash/91f18a1287b398d378ef22505bf41832-Abstract-Datasets_and_Benchmarks.html)
- Anthropic's application-building experiments found that unit checks or HTTP requests could miss failures in the complete user flow; explicit browser-based checks improved end-to-end verification in that setting. This is a practical engineering report, not a universal quantified effect. [Long-running agent harnesses](https://www.anthropic.com/research/effective-harnesses-for-long-running-agents)

### Inferences

Proposed task contract:

```text
task_id, goal_id, task_revision
requested_outcome, scope, exclusions
acceptance_criteria[]
dependencies[], owner, allowed_resources[]
artifact_location, base_revision
verification_plan, quality_mode
deadline/budget when supplied or configured
```

Proposed result contract:

```text
task_id, task_revision, attempt_id, ownership_epoch
result_kind: candidate | blocked | failed | cancelled
artifact_locations[], artifact_revision_or_digest
criteria_results[]: criterion_id, evidence_ref, verifier, outcome
checks[]: command_or_action, cwd, environment_id, exit_status, output_ref
known_gaps[], integration_notes, external_action_receipts[]
```

The worker submits a candidate. A controller validates identity, required fields, artifact availability, and the applicable checks before recording acceptance. The controller can delegate assessment but should own the state transition. Typed JSON prevents missing fields; it cannot prove that those fields are true.

Acceptance evidence should identify the actual content, not merely a branch name: commit plus dirty-tree digest, content manifest, build digest, or other stable artifact identity. Record the test environment and commands. A later relevant edit invalidates affected evidence. Rechecking after integration is necessary when independently correct changes interact. The dependency relation determines the minimal recheck scope.

Checks should protect the user's intended behavior. An agent-generated assertion that mirrors the implementation is weak evidence; a regression reproduction, existing independent test, observed final state, or representative user flow is stronger. No test suite proves the absence of every defect. A reviewer can assess ambiguity, design quality, and omissions while executable checks establish their particular conditions.

| Proposed quality mode | Suitable work | Minimum acceptance evidence | Additional review |
| --- | --- | --- | --- |
| Quick | Read-only answers; small reversible text/config edits with limited impact | Requested artifact exists; content/scope inspected; applicable local instructions followed | Owner inspection; add a check only when it verifies a material concern |
| Standard | Behavior changes, ordinary features, multi-file refactors | Concrete acceptance cases; appropriate existing checks; artifact identity; affected integration checks | Separate review for substantive changes; UI flow observation when behavior is visual/interactive |
| Critical | Auth, migrations, destructive data operations, shared infrastructure, external release actions | Standard evidence plus failure/recovery cases, rollback or reconciliation plan, and observed action receipts | Independent review with relevant expertise; user approval only where authorization actually requires it |

These are proposed defaults, not a new approval policy. The user's instructions and repository rules govern. An already-authorized release should reach its requested destination; an uncertain side effect requires reconciliation. A low-impact change should not acquire a large ceremony simply because an orchestrator is involved.

For independent review, give the reviewer the task contract, diff/artifact and raw evidence. Ask them to reproduce the riskiest claim and name any unsatisfied criterion. Avoid anchoring them with a persuasive worker summary before they inspect the artifact. Another model may add perspective, but model diversity does not guarantee independent errors. Use one review-and-repair cycle by default only as an initial configurable policy; allow further work when new defects justify it, bounded by the actual budget.

Research tasks can use claim-to-source checks and citation verification. UI tasks can require screenshots plus an observed interaction outcome. Data tasks can check row counts, totals, formulas, or exported output.

Bro's own repository gives a concrete completion contract: after every repository change, build both Windows release binaries with `cargo build --locked --release -p bro-tui --bins`, install both to the active location on `PATH`, verify installed hashes match the build, and check a fresh Bro process while preserving existing sessions. A passing test report alone does not satisfy that contract. Acceptance should ingest actual repository instructions and inherited authorization, including this delivery requirement, without repeatedly asking for actions the user has already authorized. [Repository instructions](../../AGENTS.md)

### Gaps

Public evidence does not establish an automatic reviewer that can certify every task type. Review quality, false rejection, missed defects, and review cost need their own calibration. Bro also needs an explicit way to represent accepted exceptions without silently weakening the user's criteria.

## How should Bro recover and coordinate safely

### Takeaway

Persist control decisions before dispatch and reconcile uncertain actions after restart. Make task identity, attempt identity, and ownership separate concepts.

### Cited Findings

- Temporal records events in history so workflows can recover after crashes. Its activity documentation distinguishes completion observed once from execution that may occur multiple times. A crash after the operation but before reporting success can cause a retry. Idempotency keys must be enforced by the receiving operation. [Temporal event history](https://docs.temporal.io/workflow-execution/event), [Temporal activity definition](https://docs.temporal.io/activity-definition)
- Temporal separates queue-wait, execution-attempt, overall execution, and heartbeat timeouts. Long-running activities can checkpoint progress through heartbeats; cancellation delivery is cooperative and depends on heartbeats in that activity model. These semantics describe Temporal and are design references for Bro. [Activity failure detection](https://docs.temporal.io/encyclopedia/detecting-activity-failures)
- LangGraph's checkpointer documentation explicitly distinguishes exit-only, asynchronous and synchronous persistence. Exit-only mode cannot recover intermediate state after a process crash; asynchronous persistence has a crash window; synchronous writes complete before continuation. Replaying from a checkpoint may rerun subsequent API calls. [LangGraph checkpointers](https://docs.langchain.com/oss/python/langgraph/checkpointers)
- Amazon describes retries worsening overload, retry amplification across layers, and the need for idempotency when retrying side effects. It recommends selecting an appropriate retry layer and adding backoff with jitter. Published 2019-12-05. [Amazon retry design](https://builder.aws.com/content/3EumjoZascWd1oZiEgL8ORlv3qE/timeouts-retries-and-backoff-with-jitter)
- A lease can expire while its old holder is merely paused. Kleppmann's locking analysis explains monotonically increasing fencing tokens: the resource must reject stale writes. A timeout alone does not prove the old process is dead. Published 2016-02-08. [Fencing stale workers](https://martin.kleppmann.com/2016/02/08/how-to-do-distributed-locking.html)

### Inferences

Proposed durable state belongs in a local transactional control store with an append-only event journal. Bro need not adopt Temporal or LangGraph to borrow these properties. Evaluate the existing persistence design before choosing a storage engine.

1. Record assignment, task revision, owner, and attempt before sending a prompt. Commit a pending dispatch record in the same transaction. A dispatcher retries delivery using a stable command ID; the receiving adapter deduplicates when it can.
2. Track command delivery separately from task execution. A transport acknowledgment establishes delivery, not correct task completion. A send that times out is an uncertain delivery, not proof that nothing happened.
3. Maintain a monotonic ownership epoch. Completion and progress messages carry task ID, revision, attempt ID and epoch. The controller accepts state changes only from the current assignment. Keep stale messages for diagnosis without letting them close the task.
4. Model a worker session separately from an attempt. A session can outlive many tasks; a task can survive many sessions. Store real session/process identity and adapter resume metadata without treating PID reuse as continuity.
5. On restart, reload goals and pending decisions, inspect live sessions and artifact state, and reconcile commands with uncertain outcomes. Resume from the last trustworthy checkpoint, not an imagined transcript. Avoid automatically spawning duplicate workers for tasks whose original session is still active.
6. Keep queue deadlines, attempt deadlines, lease/heartbeat policy and total task budgets distinct. Use event-driven wakeups for completions, dependency changes, user input, resource availability and timers. Poll only as a recovery mechanism when an adapter cannot provide events.
7. Retry transient transport or provider errors with bounded backoff and jitter, honoring retry-after signals. A failed acceptance check calls for a repair plan. A missing permission or input calls for a blocker. Repeatedly replaying the same prompt should not be the common recovery path.
8. Persist progress checkpoints at meaningful boundaries: discovered constraints, affected files, artifact identities, passed/failed checks, current hypothesis, named blocker and exact next action. Store raw logs/artifacts separately; summaries must link to evidence.
9. Represent cancellation as requested, acknowledged and stopped. Stop downstream dispatch immediately. Let an active worker reach a safe checkpoint where possible; escalate to process interruption according to policy. Preserve partial artifacts and reconcile any external actions already started.

Resource coordination needs explicit enforcement. A result epoch protects controller state; it cannot prevent an old shell from modifying a shared directory. Prefer isolated worktrees for concurrent code writers and a designated integration owner. Treat the shared browser profile, mobile device, clipboard, development port and install directory as resources with scoped leases. A dedicated browser tab reduces interference only when the operations do not affect profile-wide state. Where tools bypass the lease mechanism, serialize mutation access or stop the old writer before reassignment. Reclaiming a lease is not sufficient proof that an uncontrolled writer has stopped.

For an external side effect, persist an action intent with a stable action ID and target identity, then record its receipt/outcome. If the process dies between those steps, query the destination by idempotency key or observed state before any retry. If the destination offers neither a key nor a reliable lookup, classify the outcome as uncertain and resolve that uncertainty through available evidence or user input. Arbitrary browser actions cannot acquire exactly-once guarantees from a local event journal.

### Gaps

Adapters may lack reliable completion events, cancellation acknowledgment or idempotent prompt delivery. Bro should report adapter capability explicitly and use reconciliation for weaker adapters. A task budget can be exact only for resources Bro can observe and control; token/cost usage from opaque external sessions may be incomplete and must be labeled accordingly.

## How can Bro prove multitasking got better

### Takeaway

Measure accepted outcomes under realistic concurrency and injected failures. Keep quality, cost, and responsiveness visible together.

### Cited Findings

- The ICML 2026 reliability paper evaluates 15 models on GAIA and tau-bench, proposing 12 metrics across consistency, robustness, predictability and safety. It finds only small reliability gains accompanying capability gains. The final version is dated 2026-06-02; earlier drafts used 14 models. The study uses one scaffold per benchmark, temperature zero, and LLM-based safety judgment, so it does not quantify Bro or current coding-agent reliability. [Reliability paper version 3](https://arxiv.org/html/2602.16666v3), [Published paper](https://proceedings.mlr.press/v306/rabanser26a.html)
- Tau-bench grades resulting database state against a goal and introduces pass^k for repeated-trial reliability. It concerns simulated retail/airline interactions, not code integration. Submitted 2024-06-17. [Tau-bench](https://arxiv.org/abs/2406.12045)
- AI Agents That Matter argues for joint accuracy/cost evaluation, controlled holdouts and reproducible environments. Submitted 2024-07-01. [Cost-aware agent evaluation](https://arxiv.org/html/2407.01502v1)
- Anthropic recommends repeated trials and isolated environments, warns that shared state can both distort outcomes and correlate failures, and suggests beginning with 20-50 representative cases rather than delaying evaluation. Pass@k asks whether any attempt succeeds; pass^k asks whether all repeated trials succeed. [Agent evaluation guidance](https://www.anthropic.com/research/demystifying-evals-for-ai-agents)

### Inferences

Build a Bro-specific suite from observed tasks and incidents, with a reference outcome and an independent checker for each case. The following are proposed scenarios and invariants; they are not performance claims.

| Scenario | Injection or setup | Acceptance checks |
| --- | --- | --- |
| Three tasks with user steering | A runs; B waits for a named dependency; C waits for review. User changes A's requirement. | Only A's revision changes. A receives the new requirement. B retains its blocker and wake condition. C still receives review. A's obsolete result cannot be accepted. No goal disappears during steering. |
| Idle worker with wrong output | Worker reports done and becomes idle; required behavior fails. | Candidate is checked, rejection includes a concrete failing criterion, task remains open for repair, and UI distinguishes worker idle from task accepted. |
| Delayed old completion | Reassign task after lease loss, then deliver old worker's completion. | Old epoch/attempt cannot mutate active state. Event remains inspectable. New attempt's evidence is still required. |
| Duplicate completion | Deliver the same result event repeatedly. | Exactly one acceptance transition and one downstream dispatch occur; repeated event is deduplicated. |
| Stale review evidence | Change relevant files after checks pass but before acceptance. | Check artifact identity mismatches; affected evidence is invalidated and rerun. |
| Browser conflict | Two tasks need the same profile/device or navigation context. | Exclusive actions serialize; ownership is visible; one task's navigation cannot overwrite the other's active operation. |
| Worktree conflict | Concurrent tasks touch the same file or integration target. | Writers are isolated or serialized; integration identifies conflicting changes; combined artifact passes affected criteria. |
| Restart after external action | Crash after remote action succeeds but before receipt is persisted. | Restart queries destination or uses idempotency; no blind duplicate action. Unresolvable outcome is marked uncertain. |
| Restart with live worker | Kill Bro controller while external worker remains active. | Reconciliation reattaches or diagnoses existing worker; no automatic duplicate agent performs the same mutation. |
| Silent worker and long build | One session stops responding; another runs a legitimately long build. | Supervisor distinguishes missing liveness from acknowledged active operation; recovery avoids killing useful work based only on no terminal output. |
| Provider rate limit | Concurrent workers receive retry-after/rate-limit failures. | Shared backoff prevents a retry storm; work with other available resources continues; budget includes retry usage. |
| User cancels during integration | Cancel while a task holds shared resources and a child is active. | No new downstream work starts. Cancellation acknowledgment and actual stopping are separate. Partial changes and uncertain side effects remain visible. |
| Starvation | Repeated small high-priority requests arrive while an older task remains ready. | Configured aging or fairness eventually schedules the older task; interactive messages remain responsive. |
| Context replacement | Resume a task in a fresh session with only persisted checkpoint and artifacts. | Worker reconstructs scope, acceptance criteria, known failures and next action without falsely claiming earlier checks were rerun. |

Track these measures by task type, difficulty and quality mode:

- Accepted tasks per wall-clock hour, where acceptance is independently checked and large tasks are not rewarded for artificial splitting. Report task-mix strata alongside aggregate counts.
- Time from request to first useful artifact, to candidate, to accepted integrated result, and to final delivery. Separate queue time, execution time, dependency wait and review wait. Report medians and tail latency.
- False-completion rate: reported completions later rejected for unmet criteria. Also record reopen rate after acceptance and escaped defect severity.
- User correction work: number of corrective interventions, explicit repair time when measurable, and sampled user effort ratings. Do not infer effort simply from token count.
- Outcome consistency across repeated trials, fault-recovery success, and recovery latency. Inspect cases where the same mean success rate hides different failure distributions.
- Cost per accepted result, including failed attempts and review. Record tokens, tool calls, provider usage and local resource utilization only when observable; retain unknowns.
- Multitasking interference: compare a task's accepted latency and success when run alone versus with the same competing workload. Track resource conflict, lost/duplicate event, stale result rejection, and unnecessary wakeup counts.
- Review effectiveness: defects caught, false rejections, defects escaped, and review cost. Sample both accepted and rejected outputs for human calibration.

Use paired task sets and controlled environments for baseline-versus-change comparisons. Repeat nondeterministic cases, keep model/adapter versions and budgets fixed where possible, and report uncertainty. Do not require identical reasoning paths when multiple correct approaches exist; judge the outcome and important execution constraints.

Run ablations in this order: current orchestrator; structured task/result contracts; verified acceptance; event-driven supervision and recovery; adaptive concurrency; review policy. Add one mechanism at a time to locate gains and costs. Compare equivalent compute/cost budgets as well as normal product settings. Preserve a held-out task set so prompt tuning does not merely learn the suite.

Start with historical failures plus the control scenarios above. Then add representative bug fixes, UI changes, multi-file features, read-only research and release/install work. Synthetic events should exercise the real controller state machine; fake agent adapters can make races and restarts deterministic. End-to-end agent trials then test whether the prompts and adapters produce the expected behavior. Keep infrastructure failures distinct from model failures in the report, while still counting their user-visible impact.

### Gaps

There are no Bro baseline measurements in this research. Proposed dashboards, tests, and modes need implementation and a recorded baseline before any throughput or quality improvement can be claimed. Small samples can reveal major failures; precise rates and small improvements need enough trials for the effect and variability involved.

## What should be built first

### Takeaway

Make task truth observable and enforce acceptance before increasing autonomy. Recovery and scheduling become easier once task identity and evidence have stable meanings.

### Cited Findings

The sequencing below is a design proposal drawn from the distinct concerns established by [MAST](https://arxiv.org/html/2503.13657v3), [Temporal's execution semantics](https://docs.temporal.io/activity-definition), and [cost-aware evaluation](https://arxiv.org/html/2407.01502v1). Those sources do not prescribe Bro's implementation order.

### Inferences

1. Add task identity, task revisions, explicit candidate/accepted/blocked states, and criterion-linked evidence. Give the UI an honest view of what is running, what is waiting, and what remains unverified.
2. Add a durable assignment/event protocol with attempt IDs, deduplication, ownership epochs and restart reconciliation. Specify mutation ownership at the same time, so stale workers cannot silently retain write access.
3. Add a minimal verification runner and review routing by quality mode. Bind every acceptance to artifact identity and keep integration as an explicit responsibility.
4. Add progress-aware supervision, named wake conditions and bounded retry/repair policy. Protect responsive user steering without discarding background goals.
5. Add resource-aware scheduling and adaptive concurrency only after the earlier invariants have a fault-injection suite. Optimize accepted outcomes and user effort, with cost and latency alongside them.

An optional later step is learning which agent, prompt and quality mode fit each task class from recorded outcomes. Start with explicit capability metadata and simple policies; a learned scheduler needs enough reliable outcome data to justify its complexity.

### Gaps

The repository audit must map this order onto Bro's existing mechanisms, including features already present. Estimates of engineering effort and migration risk need that code-level analysis; this literature review does not provide measured estimates.
