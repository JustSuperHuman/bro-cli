# Agent architectures for Bro orchestration

Research accessed on 29 October 2026. This note covers transferable execution patterns and their limits. Bro's code audit is a separate input; the proposals below assume its existing root and project coordinators remain in place. The observations come from primary sources. Proposed Bro behavior is identified under Inferences.

## Which orchestration patterns improve throughput and when do additional agents hurt?

### Takeaway

Bro should preserve central ownership of each user outcome and choose delegation according to task independence and expected verification value. A durable execution layer around the current hierarchy is a stronger starting point than adding more hierarchy or prescribing a fixed team for every request.

### Cited findings

- Anthropic distinguishes code-directed workflows from model-directed agents. Its December 2024 guidance recommends starting with the simplest workable design, using parallel work for independent subtasks or distinct evaluation perspectives, and an orchestrator-worker pattern when decomposition must be discovered dynamically. The page now marks some tooling discussion as historical. [Building effective agents, published 19 December 2024](https://www.anthropic.com/research/building-effective-agents)

- Official OpenAI documentation distinguishes handoffs, which transfer conversational ownership, from agents used as tools, which leave the manager responsible for the answer. It recommends splitting when instructions, tools, policy, or output contracts materially differ. A specialist should have a narrow job; a different agent name alone does not justify delegation. [Orchestration and handoffs, live documentation](https://developers.openai.com/api/docs/guides/agents/orchestration)

- Anthropic reported a 90.2% improvement for an Opus 4 lead with Sonnet 4 workers over a single Opus 4 agent on its internal research evaluation. It also reported roughly 15 times the token consumption of chat interactions for multi-agent systems. The comparison concerns breadth-oriented research, and the article explicitly cautions that tightly dependent coding tasks have less parallelism. The same article describes synchronous worker batches as an information-flow bottleneck and warns that asynchronous coordination introduces consistency and error-propagation problems. [Multi-agent research system, published 13 June 2025](https://www.anthropic.com/research/multi-agent-research-system)

- The verified April 2026 v3 of Kim et al. evaluates 260 configurations across six benchmarks with standardized tools, prompts, and compute. Mean multi-agent improvement across the study is -0.3%, with wide variation: Finance Centralized improves by 80.8%, while PlanCraft Independent falls by 70%. Its fitted model has configuration-level cross-validated R-squared of 0.373, or 0.413 with a task-grounded capability metric. The paper finds diminishing benefits as single-agent performance increases and warns about coordination in tool-heavy settings. These are task- and harness-dependent findings, not production scheduling constants. [Towards a Science of Scaling Agent Systems, v3, revised 8 April 2026](https://arxiv.org/html/2512.08296v3)

### Inferences

Bro should support several execution shapes within the same task model:

| Task shape | Suggested execution | Why it fits |
| --- | --- | --- |
| Small fix in one coupled code path | One worker, proportionate verification | Delegation can cost more context and integration work than it saves. |
| Research across independent sources | Several bounded investigations, central synthesis | Separate contexts increase coverage without concurrent writes. |
| Unclear bug cause | Parallel investigations with distinct hypotheses | Workers should return reproducible observations and attempts to disprove their hypotheses. |
| Independent implementation areas | Separate worktrees or explicit file ownership, then one integration owner | Parallel effort has value only if the pieces can be integrated coherently. |
| Shared API or architecture change | Agree the interface first, then release dependent work | Work against unstable assumptions causes avoidable rework. |
| Subjective or difficult deliverable | Builder plus an independently briefed evaluator | Extra review is justified where it can detect material failures. |
| Release or environment mutation | A controlled workflow with observable preconditions and receipts | Runtime state should determine whether the next step is allowed. |

The coordinator should propose a task graph whose edges express real dependencies. The runtime should validate graph consistency and schedule only ready tasks. A plan can evolve when discoveries warrant it; its revision must be recorded so workers and reviewers know which requirements remain current.

A useful delegation decision considers four costs: worker setup, context transfer, shared-resource contention, and integration. These should be compared with expected parallel work and verification benefit. The first version can use a small set of transparent rules. Bro should learn better estimates from its own traces before adopting a prediction model.

Avoid universal roles such as always launching a planner, implementer, test writer, reviewer, and integrator. A review may require a second perspective; a one-line mechanical fix generally does not require an entire team. A specialist's contract, available tools, or expertise should explain its existence.

Central ownership should mean accountability for a result, not mandatory mediation of every useful fact. Workers can send targeted messages to another worker when a dependency changes. Record the message under the relevant task and notify the coordinator of material changes. Broad broadcasts and all-to-all debate should be reserved for cases where their information value can be explained.

### Gaps

The research does not determine an optimal worker count, model mix, or concurrency limit for Bro. Its CLI engines, hardware, model subscriptions, repository sizes, and task distribution differ from the studied systems. Run Bro-specific comparisons before claiming a speed or quality gain.

The authors' repository references a later journal publication whose page was inaccessible. The frozen v3 above is the inspected source. Google's January 2026 blog describes an earlier 180-configuration version; avoid mixing its coefficients with v3. Neither v3's approximate 45% saturation threshold nor its 87% within-domain selection result is an unconditional Bro scheduling rule. [Version history](https://arxiv.org/abs/2512.08296)

## What should Bro guarantee in its execution runtime and context system?

### Takeaway

Persist intent, task state, and events outside any individual agent process. Treat a CLI session as an execution resource that can continue, fail, or be replaced while the user's objective and verified progress remain recoverable.

### Cited findings

- Anthropic's April 2026 Managed Agents architecture separates the durable session event log, replaceable harness, and execution environment. A restarted harness reloads the session log and resumes from recorded events. The context window is a selected view of history; the durable log remains available for later retrieval. The article describes failures in a coupled design as difficult to distinguish: an event-stream interruption, a failed container, and a stuck harness could look alike. [Scaling Managed Agents, published 8 April 2026](https://www.anthropic.com/research/managed-agents)

- LangGraph distinguishes thread-scoped checkpoints from cross-thread stores. Its checkpointer persists state at super-step boundaries and stores successful node writes within a parallel step, allowing failed peers to resume without recomputing those completed nodes. Its documented durability settings trade speed against loss exposure: exit-only persistence cannot recover intermediate state after a crash; asynchronous persistence has a crash window; synchronous persistence commits before continuing. Explicit replay runs post-checkpoint nodes again, including API calls. [Persistence](https://docs.langchain.com/oss/python/langgraph/persistence), [Checkpointers, live documentation](https://docs.langchain.com/oss/python/langgraph/checkpointers)

- Temporal recommends idempotent activities because execution may happen multiple times. A worker can complete an external action and crash before recording completion; a retry can repeat that action. An idempotency key must be enforced by the destination service. Activity execution also needs timeouts and, for longer operations, heartbeats. Durable orchestration does not itself make arbitrary external actions exactly once. [Activity definition, live documentation](https://docs.temporal.io/activity-definition)

- Temporal separates priority from fairness. Priority orders queued tasks by urgency; fairness uses weighted virtual queues to prevent one workload from dominating dispatch within a priority level. Its documentation explicitly notes that fair dispatch does not account for tasks already executing on workers. [Task queue priority and fairness, live documentation](https://docs.temporal.io/develop/task-queue-priority-fairness)

- Anthropic's context-engineering guidance treats context as a finite attention budget. It recommends compaction, external structured notes, and isolated subagent contexts for different task shapes. It describes keeping decisions and unresolved issues while dropping redundant outputs, and retrieving information just in time through references. [Effective context engineering, published 29 September 2025](https://www.anthropic.com/research/effective-context-engineering-for-ai-agents)

### Inferences

#### Separate durable records by purpose

The following is a proposed Bro data model, not a claim about its current implementation:

| Record | What it owns | What should survive replacement |
| --- | --- | --- |
| Intent | User objective, acceptance criteria, constraints, authorization scope, steering history | Every worker and coordinator restart |
| Task | Bounded deliverable, dependencies, owner, resource requirements, contract revision | Retries and worker changes |
| Attempt | One execution of a task on a particular worker, timestamps, budget, evidence references | The historical record, even after replacement |
| Session | Connection to a provider process and its native continuation handle | Provider-dependent; intent must not depend on this surviving |
| Event | A recorded observation or transition, with sequence and causation | Crash recovery and audit |
| Artifact | Patch, report, screenshot, test log, build receipt, or other deliverable | Verification and handoff |
| Verification | Which criterion was checked against which artifact revision, by what method | A reproducible account of why completion was accepted |

Intent is the durable answer to “what did the user ask us to finish?” Tasks are a revisable plan. Attempts are disposable execution history. A failed attempt should not erase the task, and a new user message should not silently replace the original intent.

Represent task states such as ready, leased, running, waiting, submitted, verifying, needs work, and accepted explicitly. Waiting needs a reason: dependency, resource, provider backoff, external state, or user information. A running worker and an unresolved user outcome are different facts.

The runtime should enforce atomic task claims, dependency readiness, state-transition validity, and evidence requirements for acceptance. Agents should propose decomposition, implementation approaches, and semantic judgments. Prompts can request these behaviors, but the runtime must reject duplicate ownership or an invalid completion transition regardless of what the model says.

#### Use durable events to supervise work

Normalize worker events into a small vocabulary: assignment accepted, meaningful progress, tool active, needs input, submitted, failed, cancelled, and connection lost. Attach an intent ID, task ID, attempt ID, plan revision, monotonic sequence, and causation ID. Provider-specific raw data can remain linked as a diagnostic artifact.

Record state before dispatching a consequential command. Give dispatches stable IDs; acknowledge them explicitly and reconcile uncertain outcomes before resending. Delivery acknowledgments, agent acceptance, and completed work should be separate milestones.

A supervisor should wake on a relevant event, durable timer, or new user message. It should not need to repeatedly read complete terminal transcripts to discover that a worker finished. A fallback poll can reconcile missing provider events, with the result marked as inferred where appropriate.

Heartbeat and progress are separate. A process can be alive while repeating an unproductive loop; an active compiler may produce no output for a while. Supervision should consider active tool execution, changed artifacts, test results, and repeated identical failures. Recovery actions can escalate from a status request to a changed approach or replacement attempt, under bounded retry and cost policies.

On coordinator restart, load the event cursor, restore pending tasks, inspect actual worker processes, and reconcile in-flight actions. A disconnected stream should not cause an immediate duplicate implementation attempt. Older attempts should be fenced from submitting acceptance for a task already reassigned.

Fencing a task record does not stop a still-running process from editing files or manipulating a browser. Bro needs to stop or isolate the old actor before granting an exclusive resource to a replacement. This is a runtime boundary, not something an ownership prompt can guarantee.

#### Schedule scarce resources as well as agent slots

Task readiness should require both accepted dependencies and available resources. Candidate resource keys include a repository integration branch, a worktree, the global install target, one release build slot, a mobile device, and a browser tab or authenticated browser context. File ownership helps with source edits; it cannot isolate a shared database, port, device, or deployment target.

Use leases with owner identity and expiration, together with a recovery procedure. Expiration should trigger reconciliation rather than automatic concurrent reuse. Acquire multiple resources in a consistent order or request them atomically to avoid circular waits. Release the browser while a task is doing unrelated local computation.

Start with project-level fairness and explicit limits on simultaneous expensive actions. A new urgent request can move ahead in the ready queue; interrupting an active write or build needs a distinct safe-pause capability. Priority aging can prevent background tasks from waiting indefinitely. Per-project work-in-progress limits should include review and integration queues so the system cannot create more unfinished patches than it can verify.

Reserve capacity for verification when it is on the completion path. A saturated worker pool should not leave completed implementations waiting indefinitely for the one step that makes them useful. Whether to keep a dedicated review slot or temporarily borrow capacity is an engineering choice to measure.

#### Compile context for each attempt

A proposed handoff packet should include the user's outcome and latest steering; task deliverable and non-goals; acceptance criteria; dependencies and selected decisions; relevant files and artifact references; workspace and base revision; already attempted approaches; current blockers; authorized actions; and the next observable checkpoint.

Keep the packet concise and link to complete logs. Every critical assertion should have a provenance reference: a source file at a revision, an observed tool result, or an explicit user instruction. A narrative such as “all tests pass” without the tested revision should not become durable project knowledge.

Separate verified project memory from unverified observations and working hypotheses. Scope facts to a repository revision or environment where possible. When the branch, dependency lockfile, or related subsystem changes, mark affected memory for revalidation. Store stable conventions and known setup steps only after they have been observed; keep user preferences separate from inferred technical facts.

Before compaction or worker replacement, save a checkpoint packet plus references to incomplete artifacts. A replacement worker should be able to locate its workspace, recover the relevant intent and task contract, see what already passed, and identify the next step without rereading the entire conversation. Preserve full event history for retrieval when a summary omitted something later needed.

#### Route through capability-reporting adapters

Retain Bro's current CLI sessions and add a small provider adapter boundary. Useful reported capabilities include structured event output, native resume, mid-turn steering, cancellation, approval events, context usage, tool access, worktree support, and reliable usage reporting. Unknown should be a supported value; a terminal heuristic is not equivalent to a provider's explicit completion signal.

Select workers first by required capability and available environment, then by observed task-class outcomes and cost. A worker without the needed browser connection should not receive a browser verification task merely because it is idle. A reviewer needs access to the actual artifact and runtime it must inspect. Routing should preserve user-selected engines or model constraints.

An in-process SDK can make structured events, result schemas, and nested calls easier to integrate. Moving every worker to one SDK would also require Bro to own more agent-loop behavior and provider-specific integration. Those are architectural tradeoffs, not evidence that current CLI sessions must be replaced. Add an SDK adapter when a measured task needs a capability the CLI adapter cannot supply reliably.

### Gaps

No cited source establishes that an external workflow service is necessary for a local Rust desktop application. Bro can adopt durable log, lease, and recovery principles in its own storage and process model. Temporal and LangGraph provide design references, not a mandate to add a server dependency.

Bro's actual provider event formats, continuation semantics, and cancellation guarantees need adapter-level inspection and contract tests. The code audit should identify which of the proposed boundaries already exist. Lease duration, checkpoint frequency, and retry limits need workload measurements.

## How can orchestration improve final quality without unbounded review or busywork?

### Takeaway

Define the observable result before execution, verify the submitted artifact at a known revision, and retain a clear owner for integration. Use additional evaluators where they detect meaningful failures, and make their judgments inspectable.

### Cited findings

- Anthropic's November 2025 long-running harness uses an initializer, a structured feature list, incremental implementation, and progress artifacts. It observes premature completion and incomplete verification even when an agent runs unit tests or makes HTTP requests. Explicit end-to-end interaction testing improved results in its web-app experiment. The article does not establish that one universal multi-agent arrangement works best. [Effective harnesses, published 26 November 2025](https://www.anthropic.com/research/effective-harnesses-for-long-running-agents)

- Its March 2026 follow-up uses a planner, generator, and evaluator. Generator and evaluator first agree on a testable contract; the evaluator exercises the running application. Early evaluators found legitimate bugs but excused them, so the author tuned their criteria against his judgments. The initial full harness example was over twenty times more expensive than the solo run. Later experiments removed the sprint structure as model capability improved and moved evaluation later, retaining extra review where the generator still struggled. These are illustrative application builds, not a controlled claim that every task should use this pipeline. [Harness design, published 24 March 2026](https://www.anthropic.com/research/harness-design-long-running-apps)

- In the February 2026 compiler experiment, independent failing tests made parallel work easy. A monolithic Linux build caused sixteen agents to repeatedly fix the same bottleneck and overwrite one another; the author introduced a known-good compiler oracle to partition investigation. He reports that most harness effort went into tests, environment, and feedback; concise summaries with complete logs elsewhere reduced context pollution. He also added stricter integration checks after new work repeatedly broke existing behavior. [Building a C compiler, published 5 February 2026](https://www.anthropic.com/research/building-c-compiler)

- Claude Code's current team documentation includes dependency-aware shared tasks, atomic claiming through file locking, and hooks that can reject task completion. It advises distinct file ownership. It also lists limitations: in-process teammates are not restored by resume, task status can lag, and a lead may declare completion early. A shared task list and an agent lead therefore still need supervision. [Agent teams, live documentation](https://code.claude.com/docs/en/agent-teams)

- Official OpenAI tracing documentation records sessions, turns, and spans for agent work, generations, and tool calls. Spans carry outcome, timing, and recorded inputs or results. It distinguishes live events from traces built after a turn, and notes that missing usage is unknown, not zero. [Tracing, live documentation](https://developers.openai.com/api/docs/guides/agents-api/tracing)

### Inferences

#### Make completion a verified state

At task creation, record concrete criteria that the user would recognize as success. For implementation, connect each criterion to a runnable behavior or an inspectable artifact. “Implement settings” is insufficient; “a changed preference survives reopening the app” can be checked.

Workers submit a result package: artifact locations, change summary, base and final revisions, acceptance evidence, remaining issues, and integration instructions. Submitted work moves into verification. A worker's final message or idle status should not mark the parent intent complete.

Verification evidence should identify the command or interaction, environment, exit status or observed result, artifact revision, time, and verifier. A passing test result from an earlier revision does not verify a later merged patch. Verification can be reused only when its assumptions and relevant inputs remain valid.

Scale checks to the change. Mechanical edits may need direct inspection; behavior changes need targeted checks; cross-module integration needs combined-artifact verification; UI changes need interaction and layout checks; installation changes need a fresh installed process. Repository instructions remain part of the contract. Avoid tests that merely repeat the implementation or review loops that produce no actionable finding.

Choose acceptance methods by artifact. Research work needs claims tied to retrieved sources, coverage of the actual question, and a synthesis that distinguishes evidence from inference. Writing needs an audience and a reviewable final artifact. UI work needs rendered and interaction evidence, including the central user workflow. Release work needs evidence from the installed or deployed state, linked to the delivered build. Route to a worker that can inspect that artifact: source retrieval for research, rendering and browser access for UI, and the relevant environment access for release verification.

For subjective quality, define a rubric tied to the product and keep its judgments separate from mechanical correctness. A design reviewer should assess actual hierarchy, interaction, and visual consistency using the rendered result. A polished screenshot does not establish that the main workflow functions.

Review findings should state the failure, a reproducer or artifact reference, its consequence, and the criterion it violates. Builders should receive consolidated feedback. If a reviewer changes a criterion after seeing the work, record the contract revision and distinguish a genuine missed requirement from a newly requested improvement.

#### Assign integration explicitly

One integration owner should assemble accepted pieces, resolve interface mismatches, run required combined checks, and package the outcome. Isolated worker checks cannot establish that the assembled product works. The integration owner should avoid silently rewriting every worker result; recurring integration failures are feedback about decomposition and contracts.

Parallel implementation should use worktrees where tasks would otherwise compete over source state. Assign ownership of shared schema, dependency files, generated assets, and release metadata explicitly. An integration queue should reflect dependencies and artifact freshness. Merge conflicts and failed combined checks are states to resolve, not reasons to lose accepted worker evidence.

#### Make user steering addressable

Tag new user input to an intent and, when known, the affected task. Preserve the original objective unless the user cancels or replaces it. A request for status should produce a status response while work continues. A constraint change should increment the plan or contract revision and invalidate only affected queued work and verification.

Show a compact status built from durable records: current objective, accepted criteria, active work, waits and their reasons, next checkpoint, and material findings. A user should be able to tell the difference between “worker is processing,” “waiting for a device,” “implementation submitted,” and “verified complete.”

Pause, cancel, reassign, and retry should have explicit scope. Cancelling one intent should not terminate an unrelated project coordinator or its tasks. A paused task should retain resumable artifacts and release resources it can safely release. Use user input only when a needed decision is missing; routine recoverable errors belong in supervision.

#### Measure quality-adjusted completion

Compare changes using accepted user outcomes, not worker count, token volume, or tool activity. Useful measurements include time to verified completion, rework after acceptance, first-attempt acceptance, integration failure rate, duplicate work, time blocked by resource, queued review time, context reconstruction time, and cost per accepted outcome.

Create a small task set from representative Bro usage, including interrupted work and concurrent projects. Repeat where agent variance matters. Record the provider version, model, instructions, task contract, and workspace revision so the comparison remains interpretable. An improvement that reduces elapsed time but raises escaped failures should be visible as a tradeoff.

Candidate acceptance scenarios for the first implementation slices:

1. Two workers try to claim the same task. Exactly one attempt receives ownership, and the other gets a structured conflict result.
2. A coordinator restarts while an implementation worker keeps running. Bro restores the intent and observes the existing attempt without launching a duplicate.
3. A worker submits code and goes idle, but one required behavior has no verification evidence. The parent remains unresolved and schedules the missing check.
4. Two tasks request the same browser tab or install target. One waits with a visible reason; no concurrent actor uses the exclusive resource.
5. An old worker returns after reassignment. Its result is preserved as history, but it cannot accept the current task or overwrite current ownership.
6. The user changes a requirement while independent tasks run. Affected tasks receive the new revision; unaffected work proceeds.
7. One worker in a parallel group fails. Accepted outputs from its peers remain available, and only the failed dependency chain needs recovery.
8. A task resumes after compaction or replacement. Its packet identifies the correct workspace, current contract, verified progress, failed approaches, and next step.
9. Every worker slot is occupied and implementations are waiting for verification. The scheduler admits completion work under its stated policy instead of indefinitely starting new implementation.
10. An external action may have succeeded before the connection failed. Bro reconciles its destination or idempotency receipt before deciding whether to retry.

These scenarios should be checked at the runtime boundary. A prompt that asks agents to behave correctly does not establish the property.

### Gaps

The sources do not establish a universal cost threshold for review, ideal number of repair rounds, or proof that model diversity always improves evaluation. Bro should observe reviewer usefulness by task class and revise its policy as worker capabilities change.

The architectural proposal has not been implemented or benchmarked in this research pass. Its acceptance scenarios describe intended guarantees. They should become targeted runtime tests when the relevant implementation is built.
