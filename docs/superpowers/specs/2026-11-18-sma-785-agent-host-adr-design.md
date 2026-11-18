# SMA-785: ADR-0028 — always-on agent host: durability, worker model, event envelope

Linear: SMA-785 (project "Paigasus Agent Host"). Blocks SMA-786 (M0 skeleton). Every later issue in
the project (SMA-787 to SMA-799) builds on the decisions below.

Status: revision 2, after the builder check and the adversarial challenge. Approved by Sven at
Gate 1 on 2026-11-18, with the approver proposal in (g) and the per-run commit in (b).
Sven chose: session policy A (resume by thread), defer option 1 (change Helikon), mid-run
approach A (a turn-boundary checkpoint in Helikon). Every other choice below is marked as Claude's
call where it is not in the Linear issue.

This file is the source text of the Notion page **ADR-0028**
(https://app.notion.com/p/41c830e8fbaa810fbf43d7912042d5ca). The Helikon entry is Helikon ADR-15
(https://app.notion.com/p/41c830e8fbaa816687cce856eadd0174). The Notion page is the deliverable.
The scope of SMA-785 is this ADR, a Helikon decision entry, and the project description. It
builds no code.

## 1. Intent

Helikon gives us the agent loop. It does not give us what sits around the loop for an agent that
runs unattended: events arrive while nobody watches, the agent wakes itself up, and it decides when
to speak. The project builds that layer as a service in `paigasus-core`, following Anthropic's
"Building always-on agents" article.

This ADR fixes the load-bearing choices before any code, so that the skeleton (SMA-786) and the
primitives (SMA-787 onward) do not each make them again in a different way.

## 2. Goals, non-goals, anti-goals

**Goals.**

- One durability model for the inbox, timers, leases, runs, threads, sessions and the outbox.
- **At most one committed run per lease token**, provable from the data. A run that lost its lease
  is cancelled at once and never commits.
- One event envelope for every event that enters or leaves the host.
- A rule that says, for every run, whether it resumes a session or starts a fresh one.
- A deferred tool call that ends the run cleanly and resumes later.
- A safe way for an event that arrives during a run to join that run, or to stop it.
- A clear line between the open core and `paigasus-cloud`.

**Non-goals.**

- Multi-agent coordination across inboxes (out of scope for v1, per the project description).
- Exactly-once delivery. The host is at-least-once with deduplication, like ADR-0016.
- A Temporal adapter. The ADR keeps it possible. It does not build it.
- A decision on how an agent holds a principal's access. See decision (k).

**Anti-goals.** The design fails, even if it works, if:

- a self-hoster needs a Temporal cluster to run the core;
- untrusted content reaches the model in the system role (section 4 (n) lists every path);
- a crashed run sends half a message, or loses an event;
- two runs commit for the same inbox under the same lease;
- a party other than an authorised approver can approve a deferred call.

## 3. Context (for the ADR page)

- **The article's model.** Every trigger goes into a durable inbox. One worker per inbox merges
  the pending events into one run. Timers live in a separate store and never start runs. A
  scheduler turns due timers into inbox events. A run ends in one of three ways: send a message,
  send nothing, or defer a tool call for approval.
- **IAM already uses most of this pattern.** ADR-0016: a Postgres transactional outbox
  (`event_outbox`, with `attempts` and `parked`), a relay that drains with
  `FOR UPDATE SKIP LOCKED`, NATS JetStream, and a CloudEvents 1.0 JSON envelope
  (`rs/crates/services/paigasus-iam/src/adapters/events/cloud_event.rs`, extensions
  `schemaversion`, `actorprn`, `correlationid`). IAM uses SeaORM 2 with `sea-orm-migration`.
- **Helikon is a separate public repo** (`paigasus-helikon`, published to crates.io). The host
  depends on the published crates. Facts that this ADR builds on (verified in the code):
  - Sessions are an append-only `SessionEvent` log behind the `Session` trait (`append`, `events`,
    `snapshot`). A session id is a plain `String`.
  - `TokioRunner` calls `snapshot()` once at run start and `append()` once at finalize, also on a
    failed, cancelled or timed-out run. Persistence is best-effort: an `append` error is logged,
    not returned. `run_streamed` does not finalize on a natural end without a terminal event.
  - `SessionRecorder::drain` writes a synthetic result for every unmatched `ToolCalled`. It records
    the run input and four `AgentEvent` kinds only. Items that the loop adds to its own
    conversation are not recorded.
  - `PermissionDecision` is `Allow | Deny | AskUser | Replace`. Guard rules can return `Ask`. Both
    `AskUser` and guard `Ask` block the loop on an `ApprovalHandler` future. `ApprovalOutcome` is
    only `Allow | Deny`. `LoopState::NeedsApproval` exists and returns `NotImplemented`.
  - `PreToolUse` hooks run before authorisation and can replace the args. `Replace` can too.
  - The only mid-run control is a `CancellationToken`. Input guardrails run once, on the seed
    input. A hook can return `InjectSystemMessage`. Nothing can inject a user-role message.
  - `CompactingSession` compacts inside `append`, with a model call that nothing can cancel. Its
    summary is a `SessionEvent::Compacted`, which `project()` renders as `Item::System`.
  - `TemporalRunner` (runtime-temporal 0.5.3, Temporal Rust SDK 1.0) is implemented. Its v0
    rejects hooks, guardrails and handoffs.
  - `sessions-testkit` (the `Session` conformance suite) has `publish = false`.
  - Helikon ADR-6: durability is a seam (`Runner`), not built in.
- **IAM has no delegation mechanism.** A service cannot act for a principal today.

## 4. Decision

### (a) Durability substrate: Postgres is the source of truth

- The host's Postgres database holds every durable record: `inbox`, `inbox_event`, `inbox_lease`,
  `run`, `thread`, `session_event`, `pending_tool_call`, `timer`, `outbox_message`, and the
  Postgres memory store (SMA-789).
- NATS is an ingress source (a JetStream consumer that writes inbox rows) and the fan-out for
  outbox events. It is never the source of truth for the inbox.
- Redis is optional, for caches only. No correctness depends on it.
- A `Scheduler` port in `paigasus-agent-host-core` turns due timers into inbox events (decision
  (l)). The core ships a Postgres adapter. A Temporal adapter can implement the same port later.
- The core always runs Helikon's `TokioRunner`, through `Runner::run`, not `run_streamed`. The host
  does not wrap the loop in Temporal. Reason: `TemporalRunner` v0 rejects hooks, guardrails and
  handoffs, and the permission layer (SMA-793) needs hooks and guardrails.
- Times that decide correctness (lease expiry, due timers, backoff) use the database clock
  (`now()`), never the worker clock.

### (b) Run lifecycle and the atomic commit

**Claim transaction** (short, before the model loop):

1. Take the lease (decision (c)).
2. Select the claimable events of one thread (decision (f)).
3. Insert a `run` row: status `running`, the lease token, the claimed event ids.
4. Set `claimed_run_id` on those events and increment their `attempts`. Events keep the status
   `pending`. So a crash also counts as an attempt, and an expired lease frees the events with no
   reaper: an event whose claiming run is not `running` under a live lease is claimable again.

No transaction or database connection stays open across the model loop.

**Commit transaction** (one, at the end of the run), through a `RunCommitter` port in the domain
(`commit(RunOutcome, LeaseToken) -> Result<Committed, LeaseLost>`), so the domain does not see
SeaORM:

1. `UPDATE run SET status = <outcome> WHERE id = $run AND status = 'running'`, joined with a check
   that the lease token still matches. Zero rows means "already committed, or lease lost". Roll
   back and discard. This also makes a retried commit (a lost COMMIT acknowledgement) harmless.
2. Append the run's session events (`PgRunSession`, below).
3. Insert or update the `thread` row (`last_run_at`).
4. Insert outbox messages. Insert new timers. Cancel timers the run cancelled (for example an
   approval expiry).
5. Insert or resolve `pending_tool_call` rows (decision (g)).
6. Write the buffered memory changes of the Postgres memory adapter.
7. Mark the claimed and folded inbox events `done`.

**Outcomes.** `completed`, `deferred`, `stopped`, `failed`, `discarded`.

- `failed`: the runner returned an error. The host classifies it:
  - **transient** (provider 429 or 5xx, a database error): roll back. The events get a
    `not_before` backoff and stay `pending`.
  - **deterministic** (max turns, a guardrail tripwire, invalid structured output, a provider 4xx,
    a timeout): commit only the `run` row with the error. The partial session events are
    discarded, so a malformed session cannot poison the thread. The events stay `pending`.
  - After `max_attempts`, the events move to `dead`. The console shows dead events and can replay
    them (SMA-799).
- `discarded`: the lease was lost. Nothing commits.
- Every run has a maximum wall time (`RunConfig.timeout`).
- `final_output` is never delivered to the user. Only the send tool (SMA-792) sends messages. The
  host stores `final_output` on the `run` row.

**`PgRunSession` contract.** The host implements Helikon's `Session` trait itself, because
`sessions-postgres` writes through its own pool and its own transaction.

- `snapshot()` reads the committed events of the thread's session and projects them (with the
  compaction rule in (f)).
- `append()` only buffers in memory. It never fails.
- `events()` and `snapshot()` return the committed events plus the buffer.
- The commit writes the buffer. If the run is discarded, the buffer is dropped.
- The host runs Helikon's `sessions-testkit` against `PgRunSession`. Helikon publishes the testkit
  with the release that carries (g) and (h).

**Side-effect guarantee, stated as it is.** A run commits once, at the end. If a worker crashes
at turn 10, the next attempt runs turns 1 to 10 again. The model can make new `call_id`s and
different args. So a side-effect tool is **at-least-once per run attempt, with possibly different
args**. A tool that must not repeat must defer (decision (g)) or keep its own idempotency check on
the business key. Messages to the user always go through the outbox, so they are never sent twice.
A per-turn commit would bound the repeat to one turn. It is not chosen for v1 (section 6).

The S3 memory adapter cannot join the transaction. Its writes are at-least-once and can survive a
discarded run. The Postgres memory adapter is the default.

### (c) Worker model: one lease per inbox

- `inbox_lease (inbox_id PK, holder, token BIGINT, expires_at)`. Lease rows are never deleted, so a
  token never repeats.
- **Take the lease.** `INSERT … ON CONFLICT (inbox_id) DO UPDATE SET token = inbox_lease.token + 1,
  holder = …, expires_at = now() + ttl WHERE inbox_lease.expires_at < now() RETURNING token`. This
  also works for a new inbox that has no lease row. The candidate inbox is selected with the
  claimable predicate of (f), with `FOR UPDATE OF inbox_lease SKIP LOCKED` when the query joins
  `inbox_event`.
- **Heartbeat.** `UPDATE inbox_lease SET expires_at = now() + ttl WHERE inbox_id = $1 AND token =
  $t`. `lease_ttl` is at least three heartbeat intervals.
- **Lease lost** means: no successful heartbeat before `expires_at`. Zero rows updated and a
  database timeout both count, once `expires_at` has passed. On a lost lease the worker cancels the
  run **at once** with the `CancellationToken` and discards it. A cancel inside a tool call is no
  worse than the re-run that follows.
- **Interrupt.** The run stops at the turn boundary (decision (h)), commits, keeps the lease, and
  claims again at once.
- One worker process serves many inboxes at the same time, up to a concurrency limit.

### (d) Inbox granularity, tenancy and routing

- One inbox per (agent, principal). Split only when parallelism demands it (revisit trigger).
- The `inbox` row holds the principal PRN. The org comes from the PRN (ADR-0014). Every query is
  scoped by the inbox.
- An inbox is created explicitly, when an agent is bound to a principal. It is never created by
  the first event.
- **Routing.** HTTP ingress names the inbox in the path (`POST /v1/inboxes/{inbox}/events`,
  SMA-787). Every other channel (a NATS subject, a webhook, an email address) has a channel
  configuration that maps it to exactly one inbox. An event that names no principal (for example
  an org-wide webhook) is routed by its channel configuration. A channel that cannot name one inbox
  is rejected at configuration time.

### (e) Event envelope and ingress trust

- Every inbox event, outbox message and approval reply is a CloudEvents 1.0 event in JSON
  structured mode (`application/cloudevents+json`), wire-compatible with IAM's `cloud_event.rs`
  (microsecond `time`, hyphenated `id`).
- Extensions: `schemaversion`, `actorprn`, `correlationid` (as in IAM), and a new `threadid`.
- `data` holds the proto3 JSON mapping of the `agents/v1` message that `type` names. Events from
  outside (a raw webhook) carry their own JSON in `data`. `type` for events the host emits starts
  with `agents.` (for example `agents.outbox.message`).
- **Dedup.** A unique constraint on (`inbox_id`, `source`, `id`). A duplicate insert is a success
  with no new row, and the handler returns `202`. A duplicate whose payload differs is logged and
  counted. `done` rows are kept for a retention period, and that period is the dedup window.
- **The host, not the caller, sets the trust-bearing fields:**
  - `source` comes from the authenticated channel configuration.
  - `actorprn` is the authenticated caller, set by the host.
  - A raw webhook without a CloudEvents `id` gets an `id` from the provider's delivery id. If there
    is none, the `id` is a hash of the content.
  - A `threadid` is kept only if that thread belongs to the same inbox. Otherwise the host drops
    it, and the event starts a fresh thread.
- Each channel configuration names how its caller is authenticated (an IAM API key, a webhook
  signature, a verified email sender).
- The envelope code moves into a shared library crate (decision (j)).

### (f) Session policy: one run serves one thread

- A **thread** is a sequence of runs on one Helikon session. The thread id is the Helikon session
  id. `thread (id, inbox_id, created_at, last_run_at, closed)`.
- Every outbox message carries the `threadid` of the run that sent it. A user reply carries it back
  (the channel adapter maps it, for example from email `In-Reply-To`). An approval reply carries
  the `threadid` of the deferred run.
- **A thread is live** when it exists, is not `closed`, and its `last_run_at` is within the thread
  TTL. A thread with an open deferred call is always live. An operator can close a thread.
- **The claimable predicate.** An event is claimable when its status is `pending`, `not_before`
  has passed, it is not claimed by a `running` run under a live lease, and one of these is true:
  - its thread has no open deferred call;
  - it **resolves** that thread's open call: an approval reply that matches an open `call_id`, the
    call's expiry timer event, or a user message on that thread (see (g), "superseded").

  The predicate is used both to select an inbox and to claim events. The open-deferral state is a
  column on `thread`, written by the run commit, and indexed. So an inbox whose only pending events
  wait on an approval is idle and is not selected again.
- **Claim.** Take the oldest claimable event of the inbox (by `received_at`).
  - If it has a live `threadid`, claim the claimable events of that thread, in `received_at`
    order, up to the batch limit. The run resumes that thread.
  - If it has no `threadid`, or its thread is not live, claim the claimable events that have no
    live thread, up to the batch limit. The run starts a fresh session under a new thread.
  - Events of other threads stay pending for the next claim. So several messages in one thread
    still produce one reply, and no run mixes threads.
- A timer event with a `threadid` follows the same rule. If its thread is not live, it starts a
  fresh thread.
- **Compaction is a host step, not `CompactingSession`.** After a commit, under the same lease,
  if the thread's token estimate is over a threshold and the thread has no open deferred call:
  1. Run a memory-flush turn, so that facts that must persist go to memory (SMA-791).
  2. Write a summary as a `Compacted` event, made by a separate model call that the host can
     cancel.
  3. `PgRunSession::snapshot()` projects a `Compacted` event as a delimited **user-role** item and
     drops the events before it. It never projects it as a system item.

### (g) Deferred tool calls

**Helikon changes** (on `#[non_exhaustive]` types; compile-compatible for callers):

- `PermissionDecision::Defer { prompt }` and `ApprovalOutcome::Defer`, so that a policy, a guard
  `Ask` and an `AskUser` can all defer. The host's `ApprovalHandler` returns `Defer`. One deferral
  answers every ask point of one call.
- The loop uses the existing `LoopState::NeedsApproval`. When a turn holds deferred calls, the
  loop runs the turn's other allowed calls, records their results, and ends the run at the turn
  boundary.
- `RunResult` gets a stop reason: `Completed | Deferred { calls: Vec<PendingToolCall> } |
  Stopped { reason }`. It also reaches `run_streamed` consumers and agrees with
  `AgentEvent::is_terminal`.
- `PendingToolCall` and a new `SessionEvent::ToolDeferred` hold the `call_id`, the tool, the
  **effective args** (after `PreToolUse` and `Replace`), and the active agent (for a handoff).
  `SessionRecorder::drain` does not write a synthetic result for a deferred call.
- A resume entry point takes resolutions (`call_id` → `Approved | Denied { reason }`). It writes
  the results of **all** resolved calls before any new input. On `Approved` it runs the call with
  the effective args, on the recorded agent:
  - It does not fire `PreToolUse` again.
  - It runs the deny rules, guard `Deny`, the mode and the policy again.
  - `Defer`, `AskUser` and guard `Ask` count as answered by this approval.
  - Any `Deny` wins.
  - A `Replace` that changes the args denies the call.
  - A tool that the agent no longer has is denied with "tool no longer available".
- A `Defer` inside an `AgentAsTool` sub-run (depth > 0) is a `Deny` in v1. Its session is
  discarded, so there is nothing to resume.
- A runner that does not support `Defer` (for example `TemporalRunner` v0) treats it as `Deny`.
- Core's own matches without a wildcard (`SessionEvent::kind`, `ts`, `project`, `tool_exec.rs`)
  change with the new variants. Once a `tool_deferred` event is stored, the host cannot roll back
  below that Helikon version, because an older reader cannot parse the row.

**Host side** (SMA-795):

- The run commits with outcome `deferred`, inserts one `pending_tool_call` row per call (state
  `open`), sets the thread's open-deferral column, inserts one expiry timer per call, and releases
  the lease.
- One `ApprovalRequest` per call goes through the outbox. It shows the effective args. It carries
  the `threadid`, the `call_id`, a hash of the effective args (RFC 8785 canonical JSON), and an
  approval token (below).
- The thread resumes only when **every** open call of the deferred turn has a resolution. The
  first resolution per `call_id` wins (a unique constraint). A later reply is marked `done` with
  the outcome `stale`. The resolving commit cancels the expiry timer.
- **States.** `open → approved | denied | expired | superseded → resolved`.
- **Expiry.** The expiry timer resolves the call as `Denied { reason: "approval expired" }`.
- **Superseded.** A user message on a thread with open calls resolves every open call as
  `Denied { reason: "superseded by a new user message" }`. The run then continues with that
  message after the denial results. So "cancel that" works at once, and a thread never waits for
  hours.
- **Who may approve** (accepted at Gate 1): only the inbox principal in v1. The reply endpoint
  authenticates the approver through IAM and checks that the approver PRN is the inbox principal.
  The approval token is single-use and unguessable: an HMAC over `inbox_id`, `thread_id`,
  `call_id`, the args hash and the expiry. An email link carries the token, opens a confirmation
  page, and the approver signs in there. The token alone does not approve. Delegates and admin
  approvers come later, with (k).

### (h) Mid-run events: a turn-boundary checkpoint

**Helikon change:**

- An optional callback on `RunContext`, called at every **turn boundary** of the top-level loop
  and of handoff children: after all tool results of a turn are recorded, before the next model
  call. It is never called inside a tool call, in an `AgentAsTool` sub-run, or at a boundary where
  a call was deferred.
- It returns `Continue | Inject(Vec<ContentPart>) | Stop { reason }`.
- `Inject` content is user content. The loop emits a new `AgentEvent` for it, and
  `SessionRecorder` records it as a `UserMessage`. So the thread history contains it.
- The loop runs the input guardrails on injected content. A tripwire ends the run with `Stop`.
- `Stop` ends the run at the boundary with stop reason `Stopped`. No tool call is half done.

**Host side:**

- At each boundary, one indexed query reads new claimable events **of the running thread**. The
  host claims them under the same lease and the same run.
- Each agent definition has a mid-run policy per event type: `queue` (the default), `fold` or
  `interrupt`. Approval replies and expiry timer events always use `queue`. Events of other
  threads are never read at a boundary.
  - `queue`: the event stays pending for the next run.
  - `fold`: the host returns `Inject`. The event is marked `done` in the run commit. If the run is
    discarded, the event is claimable again.
  - `interrupt`: the host returns `Stop`, commits the run as `stopped`, keeps the lease, and starts
    a new run on the same thread with the new events.
- An operator cancel from the console (SMA-799) also returns `Stop`. The run commits as `stopped`
  and keeps its work so far.
- An `interrupt` that arrives during the last model call does not stop that turn's outbox
  messages. v1 has no extra check before the commit. This is a known limit.
- A `TemporalRunner` would need the same checkpoint later, as a workflow signal. That is outside
  this ADR.

### (i) Open-core boundary

Under ADR-0006 (a published-artifact dependency edge):

- **Core** (`paigasus-core`, Apache-2.0): the inbox, timers, worker, outbox and dispatcher; the
  webhook and email channels; the Postgres and S3 memory adapters; the Postgres `Scheduler`
  adapter; the agents console zone.
- **`paigasus-cloud`**: managed channel apps (Slack, Teams), hosted dreaming, and any managed
  Temporal adapter. They use the published crates and the `agents/v1` protos. They never patch
  the core.

### (j) Crate layout

The project description's layout stands, with these additions:

- A new library crate for the CloudEvents envelope (the name is the builder's call, for example
  `paigasus-cloudevents`). It owns parse, validate and build, with extension attributes per
  service, and keeps IAM's wire bytes unchanged. IAM moves to it in a later issue. This ADR does
  not require that move.
- `paigasus-agent-host` depends on the published `paigasus-helikon` crates at a version that has
  (g) and (h), and on the published `sessions-testkit` as a dev-dependency.
- Domain types in `paigasus-agent-host-core`: the `RunCommitter` and `Scheduler` ports, and
  `PendingToolCall` as an entity with the states in (g).

### (k) Acting for a principal: named, not decided

IAM has no delegation or token-exchange mechanism. An inbox records which principal it acts for.
How a run carries that principal's access is the subject of SMA-793 (permissions) and SMA-794
(the credential-injecting egress proxy). This ADR does not decide it. The approver rule in (g) does
not depend on it.

### (l) The `Scheduler` port contract

- Follow-ups (one-shot) and schedules (recurring, cron plus an IANA time zone) both exist in v1
  (SMA-788).
- A fire produces an inbox event with a **deterministic id** `timer:<timer_id>:<fire_seq>` and a
  fixed `source`. The dedup constraint then makes a double fire harmless, for any adapter.
- The Postgres adapter selects due timers with `FOR UPDATE SKIP LOCKED`, compared with the database
  `now()`. It inserts the event and marks the follow-up done (or advances the schedule) in one
  transaction.
- A run commit can create and cancel timers.

### (m) Event-to-input rendering

One renderer turns an inbox event into model input, for the first batch and for `Inject`:

- The output is user-role content, never system-role.
- Each event is a delimited block with its `type`, `source`, `time` and `data`. The delimiters mark
  the content as untrusted data, not as instructions.
- `actorprn`, `correlationid` and other internal attributes are not shown to the model.

### (n) Paths into the system role

| Path | Rule |
|---|---|
| Agent definition (system prompt, tool descriptions) | Operator-authored. The only system-role content. |
| Helikon hooks with `InjectSystemMessage` | The host never uses them with event, tool or memory content. |
| `Compacted` | Projected as user-role by `PgRunSession` (decision (f)). |
| Memory loaded at run start (SMA-789, SMA-791) | Loaded as user-role content or as tool results, never system-role. |

### (o) Observability and crash tests

- Metrics: lost leases, discarded runs, attempts per event, dead events, open deferrals and their
  age, inbox lag (oldest claimable event age). SMA-798 owns the names.
- A crash-injection test matrix for SMA-790: after a tool call and before the commit; during the
  commit; after the commit and before the lease release; during a heartbeat partition.
- A `LISTEN`/`NOTIFY` nudge, if added later, stays outside the run commit transaction. In IAM, a
  full notification queue fails the COMMIT.

## 5. Consequences

**Positive.**

- A self-hoster runs the core on Postgres and NATS. Redis is optional. Temporal is not needed.
- A run that lost its lease never commits, and it stops at once.
- A run's session events, messages, timers, deferrals, memory changes and inbox acks commit
  together, or not at all.
- One run serves one thread, so provider message order and the thread history stay correct.
- A deferred call is a first-class outcome, bound to the args the approver saw, and only the inbox
  principal can approve it.
- Untrusted content never enters the system role, on any of the paths in (n).
- One envelope across IAM and the agent host. Trace links (SMA-798) can use the CloudEvents `id`.

**Negative and risks.**

- Two Helikon changes, (g) and (h), and the testkit publication come before SMA-795 and before the
  `fold` and `interrupt` policies. A delay in Helikon delays those issues.
- After a deferral is stored, the host cannot roll back below the Helikon version that has (g).
- The host owns `PgRunSession` and host-side compaction. They must stay correct against Helikon's
  session semantics. The testkit reduces this risk.
- A crash repeats the whole run, so a side-effect tool can run again with different args. Tool
  authors must know this.
- Polling Postgres for due timers and new events adds load. At high volume a nudge may be needed.
- The S3 memory adapter is at-least-once and can keep writes of a discarded run.
- An `interrupt` cannot stop the outbox messages of the turn that is in flight.

## 6. Alternatives considered

- **Temporal as the durability substrate.** Not chosen. Self-hosters would need a Temporal
  cluster. `TemporalRunner` v0 rejects hooks, guardrails and handoffs. The `Scheduler` port keeps
  it possible.
- **NATS JetStream as the inbox.** Not chosen. The dedup window is time-limited, a work-queue
  stream gives no relational view for the console or for the claimable predicate, and timers and
  memory still need Postgres. NATS stays an ingress source.
- **Redis Streams as the inbox.** Not chosen. Not durable enough to be the source of truth, and
  ADR-0016 already rejected it for events.
- **Postgres advisory locks instead of a lease row.** Not chosen. A session-level lock is lost
  with the connection, gives no fencing token, and does not work well behind a pooler.
- **One inbox per agent, or per thread.** Not chosen. Per agent mixes principals. Per thread
  loses one lease per principal.
- **A bespoke envelope, or binary protobuf.** Not chosen. ADR-0016 already uses CloudEvents JSON.
  Protobuf stays an upgrade path.
- **One run for a batch with several threads.** Not chosen. It puts user messages between tool
  results, which providers reject, and it sends replies on the wrong thread.
- **A per-turn commit.** Not chosen for v1. It would bound a crash to one turn. But Helikon
  persists only at finalize, so it needs a third Helikon change, and it makes outbox messages of
  earlier turns visible before the run ends. Revisit trigger.
- **Helikon's `CompactingSession`.** Not chosen. It projects the summary as system-role, compacts
  inside `append` before the commit, and makes a model call that nothing can cancel.
- **Session: one long session per inbox.** Not chosen. Unrelated events share one context and
  compaction loses detail more often.
- **Session: always fresh, except approval replies.** Not chosen. A normal reply loses the
  conversation unless memory holds it.
- **Defer: deny now, re-run later and let the model retry.** Not chosen. It depends on the model
  sending the same call again, which is not verified.
- **Defer: block in the process until the answer comes.** Not chosen. It holds the lease and a
  worker slot for hours.
- **Approval by possession of a link token alone.** Not chosen. A forwarded email would approve a
  dangerous call.
- **Mid-run: interrupt only, through `CancellationToken`.** Not chosen. A cancel can land inside a
  tool call, and a side-effect tool can then run twice.
- **Mid-run: a hook with `InjectSystemMessage`.** Not chosen. It puts external content into the
  system role.

## 7. Revisit triggers

- Temporal's Rust SDK and `TemporalRunner` support hooks, guardrails and handoffs.
- A principal needs parallel runs (split the inbox).
- Postgres polling load becomes measurable (add a nudge, outside the commit).
- Repeated side effects after crashes become a real problem (per-turn commit).
- Helikon gets a native user-message injection API (re-check (h)).
- Approval by delegates or admins is needed (extend (g), with (k)).

## 8. Left to the builder

- Table columns, indexes, and SeaORM entity shapes beyond what (b), (c), (f) and (g) name.
- Default values: heartbeat interval, lease TTL (at least three heartbeats), batch limit, worker
  concurrency, thread TTL, approval expiry, timer poll interval, `max_attempts`, backoff curve,
  maximum run wall time, compaction threshold, dedup retention.
- The name of the CloudEvents library crate.
- The exact Helikon API names for (g) and (h), within the semantics above.
- NATS subject names for agent-host events, under the existing `ops/nats/` conventions.
- The delimiter format of the renderer in (m).

## 9. Done when

- ADR-0028 exists in Notion → Development → Architecture Decision Records, status Proposed, with
  a row in the index table.
- A Helikon "Decisions (ADRs)" entry, status Proposed, records changes (g) and (h) and the testkit
  publication.
- The project description shows the crate-layout additions in decision (j).
