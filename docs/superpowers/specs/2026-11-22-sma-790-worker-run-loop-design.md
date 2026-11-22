# SMA-790 — Worker run loop: claim, run, commit

- Linear: SMA-790. Project: Paigasus Agent Host.
- Blocked by: SMA-787, SMA-789, SMA-805, SMA-843 (all Done; SMA-843 is PR 522). Blocks: SMA-791,
  SMA-792, SMA-793, SMA-796, SMA-798.
- ADR: ADR-0028 (Notion), decisions (a), (b), (c), (f), (i), (m), (n) and (o). This issue owns the
  amendment of (b) in §6.13.
- Builds on: the SMA-787 spec (`2026-11-20-sma-787-event-inbox-design.md`), the SMA-789 spec
  (`2026-11-20-sma-789-memory-store-design.md`, §6.10 and §6.12) and the SMA-843 spec
  (`2026-11-21-sma-843-memory-dir-guard-design.md`, §6.1 and §6.6).
- Status: revised after the builder check and the Stage 2 challenge (§12). Waiting for Gate 1.

## 1. Intent

The agent host has an inbox, a lease, a claim and a memory store, but nothing runs an agent. This
issue adds the worker. A worker takes the lease of an inbox, claims a batch of events, makes one
Helikon run from them, and records the result in one transaction. The result must stay correct
when a worker dies at any step.

This issue proves the spine with a scripted model. Sven chose this on 2026-11-22 (option A): a real
model provider and an agent-definition registry are follow-up issues. So this issue adds no
provider crate and does not touch the `aws-lc-rs` ban in `deny.toml` (SMA-786 G7).

## 2. Goals

- G1. A worker pool that serves many inboxes with a fixed number of worker tasks, and that gives
  every busy inbox a turn. With no due events, a worker makes no model call. Its only cost is one
  poll query per interval.
- G2. An `AgentResolver` adapter that maps an inbox to an agent definition, and a `ModelRegistry`
  that maps a model id to a Helikon `Model`.
- G3. `PgRunSession`, a Helikon `Session` that buffers the run's events and writes them in the run
  commit. The Helikon sessions testkit `=0.1.2` passes against it.
- G4. The run start: the run-start memory text and the rendered events form the user turn.
- G5. A per-event screen. A flagged event goes to `dead`, and the rest of the batch runs.
- G6. A `RunCommitter` adapter that writes ADR-0028 (b) as one transaction.
- G7. A failure classification. A transient failure rolls back and backs off. A deterministic
  failure commits the `run` row and the memory changes. After `max_attempts`, the events go to
  `dead`.
- G8. The `run` row records the final output, the error and the token usage.
- G9. Crash tests: the ADR (o) matrix in-process, and one SIGKILL test on the real binary. Each test
  fails when the fence it targets is removed.

## 3. Non-goals

- N1. A real model provider, and the `deny.toml` work for it. Follow-up issue.
- N2. An agent-definition table or an IAM call for agent definitions. Follow-up issue.
- N3. Real screening detectors. SMA-754 has no code yet. Follow-up issue after SMA-754.
- N4. The outbox and the send tool (SMA-792), timers (SMA-788), deferral (SMA-795), compaction and
  the memory-flush turn (SMA-791), and the `fold` and `interrupt` mid-run policies. The boot check
  that refuses `fold` and `interrupt` stays.
- N5. The "message / silent" run outcome of the Linear scope. It needs the outbox. SMA-792 adds it
  (§6.10).
- N6. `LISTEN`/`NOTIFY`. Polling only in v1.
- N7. Metric names are provisional. SMA-798 owns the final names.
- N8. A byte limit for each claimed batch. The claim belongs to SMA-787. A follow-up can add it.

## 4. Anti-goals

- A1. A model call, a tool call or a memory write that commits under a lost lease.
- A2. An open transaction or a held connection across the model loop.
- A3. Event, tool or memory content in the system role (ADR (n)).
- A4. A screened event that reaches the model.
- A5. A database error in the run commit that the lost-lease metric counts. (The existing
  heartbeat counts a store error after the local deadline as `operation="deadline"`. This issue
  does not change that.)
- A6. One poison event that sends healthy batchmates to `dead` because of the screen. (A crash still
  counts an attempt for the whole batch, per SMA-787 §8.4.1. This issue does not change that.)
- A7. Events that go to `dead` because no agent serves their inbox, or because of a graceful
  shutdown.

## 5. Constraints

- Helikon stays at `paigasus-helikon = "=0.6.12"` (core 0.6.0, `runtime-tokio` 0.1.26). The run uses
  `TokioRunner::run`, never `run_streamed` (ADR (a)).
- `TokioRunner` calls `session.snapshot()` and `session.events(None)` once in `load_and_record`.
  It calls `session.append()` once in `finalize`, when the agent started, also on a failed,
  cancelled or timed-out run. It does not call `append` when `load_and_record` or `agent.run`
  returns `Err`. An `append` error is only logged (no deferral in v1). So `PgRunSession::append`
  must not fail (it only buffers).
- The input guardrails of Helikon see the user text of the whole conversation and can only stop the
  whole run. So the screen of G5 is a host step, not a Helikon guardrail.
- The `LeaseKeeper` owns the only `LeaseHandle`, behind a mutex. Every claim, commit, screen-out,
  back-off and release goes through it (SMA-787 §8.1).
- `paigasus-agent-host-core` must not depend on Helikon (the A1 dependency guard in its `lib.rs`).
  Every type in a core port is a core type. The service converts from the Helikon types.
- All correctness times use the database clock. A fence must not check the lease expiry in the
  statement that waits for the lease row lock. Lock the lease row first, by inbox and token. Then
  check the expiry in a new statement in the same transaction. Postgres evaluates the predicate of
  a locking statement before the wait. It evaluates the predicate again only if the other
  transaction changed the row. `clock_timestamp()` in that predicate does not help.
- Task 15 found this defect. The SMA-787 heartbeat and the first claim had it, and the run commit
  did too. `lock_live_lease` in `lease_store/mod.rs` fixes all three.
- Lock order in the run commit: the lease row first, then the memory scope row (SMA-789 §6.12).
- Call `apply_in` when `MemoryCommitSet::is_empty()` is false, never on `changes.is_empty()`. A set
  with guards and no change still goes to `apply_in` (SMA-843 §6.6).

## 6. Design

### 6.1 Worker pool

- When `worker.enabled` is true and `[[agents]]` is not empty, the pool starts
  `worker.concurrency` tasks (default 4, range 1 to 64). When `[[agents]]` is empty, the pool does
  not start, and boot logs one warning. Ingress still stores events.
- The pool has its own shutdown `CancellationToken`.
- Each task runs this loop:
  1. `acquire_next(holder, ttl)`. On `None`, sleep `worker.poll_interval_ms` (default 1000, range
     100 to 60000, with ±10 % jitter) and try again.
  2. Start a `LeaseKeeper` on the handle.
  3. `keeper.claim(&claim_config)`. On `None`, go to step 6.
  4. Run the claim (§6.3 to §6.8).
  5. If the pool shutdown token is cancelled, go to step 6. If this lease served
     `worker.max_runs_per_lease` runs (default 8, range 1 to 1000), go to step 6. Otherwise go to
     step 3. The keeper rotates the token on each claim after the first.
  6. `keeper.release()`.
- `holder` is `<hostname>/<pid>/<task index>`.
- **Candidates.** `acquire_next` takes only an inbox that a configured agent serves:
  `CANDIDATES_SQL` gets `AND (i.org_id, i.agent_id) IN (<the configured agents>)`, bound as two
  arrays. So an inbox with no configured agent is never taken, and its events wait (A7).
- **Shutdown.** The pool cancels its shutdown token. The worker passes a run token to Helikon that
  is a child of both the keeper's cancellation and the pool shutdown token. A run in flight then
  ends with `Cancelled`. The worker decides why from `keeper.is_lost()` (new) and the shutdown
  token, never from the `RunError`:
  - lost → discard, write nothing (§6.7);
  - shutdown and not lost → `keeper.release_after_shutdown()` (§6.9);
  - neither → transient.
  The drain time is the existing 10 s `DRAIN_TIMEOUT`. A task that does not end in that time is
  aborted, and its lease expires.
- **Errors and panics.** A worker task never ends on an error. It logs the error, drops its keeper
  (with no `release`), sleeps one poll interval and goes back to step 1. The pool supervises its
  tasks: when a task panics, the pool starts a new task with the same index and counts the restart.
- **Pool sizes.** The main database pool gets `max_connections = 2 × worker.concurrency + 8`. The
  lease store gets its own small pool (`worker.concurrency + 1` connections) for the take, the
  heartbeat and the release, so a busy main pool cannot delay a heartbeat. `PgLeaseStore` already
  takes its own `DatabaseConnection`.

### 6.2 Agent resolution and the model

- `AgentResolver::resolve(&InboxId)` keeps its port in `paigasus-agent-host-core`. The worker calls
  it after the claim and before the screen. The adapter `ConfigAgentResolver` reads the inbox row
  (agent id, principal id) and looks up the agent in an `[[agents]]` list in the config file:

  ```toml
  [[agents]]
  id = "prn:pgs:agents::<org>:agent/<uuid>"
  name = "triage"
  model = "scripted"
  instructions = "You triage incoming issues."
  tools = ["memory"]
  ```

- Boot parses each `id` as an `AgentId` (org and uuid). An id that does not parse, or a duplicate
  id, fails the boot. The resolver matches the full `AgentId`: the inbox's org (from the `InboxId`)
  and its `agent_id` uuid together. A uuid alone never matches, so an entry of one org cannot serve
  an inbox of another org.
- `AgentDefinition` gains nothing new. The memory scope is `(agent, principal)` of the inbox, as in
  SMA-789.
- `tools` accepts only `memory` in v1. Boot fails on an unknown tool name. Only the top-level agent
  gets the `memory` tool (SMA-789 §6.9).
- `ModelRegistry` lives in the service crate, because `Model` is a Helikon type.
  `model(&str) -> Option<SharedModel>`. `SharedModel` is a newtype around `Arc<dyn Model>` that
  implements `Model` by forwarding, because `LlmAgent<Ctx, M, T>` needs a sized `M`. The production
  registry is empty. Boot fails when an `[[agents]]` entry names a model that the registry does not
  have.
- Because of the candidate filter (§6.1), "agent not configured" can only occur in a race with a
  config change. It is a transient failure, so it writes no memory.
- Tests and the `test-model` cargo feature register a `scripted` model (§6.11).

### 6.3 Screening

- After the claim and the resolve, the worker calls
  `EventScreen::screen(&InboxEvent) -> Result<ScreenVerdict, ScreenError>` (async) for each event.
  `ScreenVerdict` is `Pass` or `Flag { detector: String, reason: String }`.
- The port lives in `paigasus-agent-host-core`. The default adapter `AllowAll` passes every event.
  Tests use a double that flags chosen events.
- If at least one event is flagged, the worker calls `keeper.screen_out(flagged)` (§6.9). It is one
  short transaction, fenced like the run commit:
  - Each flagged event: `status = 'dead'`, `last_error = 'screened: <detector>: <reason>'`, only
    `WHERE claimed_run_id = $run AND status = 'pending'`.
  - If every event of the claim is flagged: `run.status = 'screened'`, `finished_at = now()`. No
    model call follows. The worker goes back to the claim step. The thread is not updated (no
    `last_run_at`, no `successor_id`), because no run happened on it. A fresh thread that the claim
    inserted stays with `last_run_at` null.
  - Otherwise the run stays `running`, and the run continues with the events that passed.
- A screen error is a transient failure of the run. Its back-off covers every claimed event,
  because no event passed.
- The `run_event` rows keep all claimed events, also the screened ones.
- Every screened event increases the existing dead counter, and the new screened counter.

### 6.4 Run start and the user turn

- The worker builds one `MemoryWorkspace` for the run, shared as
  `Arc<tokio::sync::Mutex<MemoryWorkspace>>` by the `memory` tool and the commit.
- It calls `run_start_context` and `render_run_start(items, nonce)`. The nonce comes from
  `run_nonce()`. A memory store error here is a transient failure before the model loop (SMA-789
  §6.10).
- A new event renderer `render::events(&[InboxEvent], nonce)` in `paigasus-agent-host-core` uses the
  `untrusted` helpers with kind `event` and the **same random run nonce**. Each event is one block:
  `open_tag("event", nonce, [("n", <index>), ("type", …), ("source", …), ("time", …)])`, then
  `escape("event", <data as JSON text>, nonce)`, then `close_tag("event", nonce)`. `time` is the
  CloudEvents `time`, or `received_at` when the event has no `time`. Absent `data` renders as
  `null`. No other attribute is shown (ADR (m)). Events are in claim order.
- The user turn is one `Item` with the user role and two text parts. Part 1 is the run-start memory
  text. Part 2 is the rendered events. If no memory file exists, part 1 is absent.
- The memory text must not go into the session (SMA-789 §6.10). But Helikon records the input
  message into the session. So the session write function (§6.6) removes every text part of a
  `UserMessage` that starts with `<memory-<nonce>` of this run. The nonce is random per run, and the
  event renderer escapes that prefix, so an event cannot forge the part.
- The system role carries only the agent's `instructions` (ADR (n)).

### 6.5 The Helikon run

- The `RunContext` gets the `PgRunSession`, the run token of §6.1, and no deferral
  (`with_deferred_tool_calls(false)`, so a `Defer` is a deny).
- `RunConfig`: `max_turns` from `run.max_turns` (default 16, range 1 to 256), `timeout` from
  `run.timeout_secs` (default 300, range 1 to 3600). `run.timeout_secs` has no bound from the lease
  TTL, because the keeper heartbeats during the run.
- The worker calls `TokioRunner.run(&agent, ctx, input, config)` and maps the result in §6.7.

### 6.6 `PgRunSession`

- One instance per run. It holds the thread id, a pool handle and a `Mutex<Vec<SessionEvent>>`
  buffer.
- `snapshot()` reads the committed events of the thread (`session_event`, ordered by `sequence`)
  plus the buffer and projects them with Helikon's `project`.
- `events(since)` returns committed events plus the buffer. Positions start at 0 and `since` is
  exclusive, over the combined list (the testkit needs this).
- `append(events)` adds to the buffer and never fails.
- `take_buffer()` gives the buffer to the commit and leaves it empty.
- A fresh thread has no committed events.
- **Limit for SMA-791.** Helikon's `project` turns a `Compacted` event into a system-role item, but
  ADR (f) and (n) need a user-role item. v1 writes no `Compacted` event. SMA-791 must replace the
  projection when it adds compaction.
- New table `session_event` (migration `m0004`):

  ```sql
  session_event (
    inbox_id  uuid   NOT NULL,
    thread_id uuid   NOT NULL,
    sequence  bigint NOT NULL,
    run_id    uuid   NOT NULL,
    kind      text   NOT NULL,
    payload   jsonb  NOT NULL,   -- the whole SessionEvent, with its nanosecond ts
    PRIMARY KEY (thread_id, sequence),
    FOREIGN KEY (thread_id, inbox_id) REFERENCES thread (id, inbox_id),
    FOREIGN KEY (run_id, inbox_id)    REFERENCES run (id, inbox_id)
  )
  ```

- **The session write function** `write_session(txn, inbox, thread, run, nonce, events)` is the one
  function that writes rows. The run commit calls it. It applies the memory filter of §6.4, replaces
  U+0000 in every string, and numbers the rows from `COALESCE(max(sequence), -1) + 1`. The primary
  key makes a second writer fail and roll back instead of writing a duplicate number.
- The commit takes `thread_id` from the `RETURNING thread_id` of the `run` update (§6.8 step 2),
  never from the record.
- The testkit runs two setups (the Linear comment of 2026-11-20): `PgRunSession` as it is, and a test
  wrapper whose `append` calls `write_session` in its own transaction, under a per-thread
  `pg_advisory_xact_lock`, and reads back through a new `PgRunSession`. The dev-dependency is
  `paigasus-helikon-sessions-testkit = "=0.1.2"`.

### 6.7 Outcomes and failure classes

The service crate maps every end of a run to one class:

| Class | Source | Commit |
|---|---|---|
| Completed | `Ok`, `StopReason::Completed` | full commit (§6.8) |
| Stopped | `Ok`, `StopReason::Stopped` (not reachable in v1: no checkpoint) | full commit, `status = 'stopped'` |
| Deterministic | `Ok`, `StopReason::Deferred` (not reachable: deferral off); `RunError::{MaxIterations, Timeout, InvalidResume}`; `AgentError::{MaxTurnsExceeded, MaxIterationsExceeded, Guardrail, HookDenied, InvalidStructuredOutput, MaxAgentDepthExceeded, MaxHandoffsExceeded, Tool, InvalidInjection, NotImplemented, Incomplete}`; `ModelError::{ContextLengthExceeded, Refused}`; a commit error with SQLSTATE `22P05` or `22021` | failure commit (§6.8) |
| Transient | `ModelError::{RateLimited, Unavailable, Transport, Other}`; `AgentError::{Session, Other}`; `RunError::{SessionWrite, Other}` (a session read error and an empty failure slot reach the worker as `RunError::Other`); a memory store error at run start; a screen error; a resolver error; "agent not configured"; a commit `MemoryRejected`; a commit `Store` error after its retries; `Cancelled` with no lost lease and no shutdown; every other variant (wildcard) | rollback, then back-off (§6.9) |
| Discarded | a lost lease (`keeper.is_lost()`), before or at the commit | nothing |
| Shutdown | `Cancelled` with the pool shutdown token cancelled and the lease held | `release_after_shutdown` (§6.9) |

- The Helikon error enums are `#[non_exhaustive]`, so the match needs a wildcard arm. The wildcard
  is transient: a transient failure commits nothing, so an unknown error cannot write memory, and
  `max_attempts` still ends the retries. A unit test pins the class of every variant named in the
  table.
- `ContextLengthExceeded` also closes the thread in the failure commit (`thread.closed = true`). The
  next claim then starts a fresh thread, so one long thread cannot send all new events to `dead`
  until compaction exists (SMA-791).
- The token usage of a failed run is not known (Helikon gives it only on the result). The columns
  stay null.
- **A deterministic failure that repeats** (from the SMA-789 Gate 1 decision): each attempt commits
  its own memory changes. A long task that times out keeps its progress notes, and the next attempt
  reads them at run start and continues. The writes are not replays: the next attempt reads fresh
  memory, and every write carries an `Expected` version. The tool side effects of a failed attempt
  can repeat. ADR (b) already accepts at-least-once tools per attempt. `max_attempts` keeps its
  SMA-787 default of 5.
- **The session of a failed attempt** (ADR (b)): the session buffer of a failed or discarded attempt
  is dropped. The next attempt renders the same events again, on a session that does not contain the
  failed attempt. Only a full commit writes session rows.
- **Discarded:** when the lease is lost, the worker writes nothing. The run row stays `running`. Its
  events are free for the next claim, because the token changed (SMA-787 §8.4).

### 6.8 The run commit

`RunCommitter` keeps its responsibility, with new methods (SMA-786 §6.2 allows a signature change).
It lives in `paigasus-agent-host-core` and uses core types only:

```rust
trait RunCommitter {
    async fn commit(&self, h: &mut LeaseHandle, r: RunRecord) -> Result<Committed, CommitError>;
    async fn screen_out(&self, h: &mut LeaseHandle, s: ScreenOut) -> Result<ScreenResult, CommitError>;
    async fn back_off(&self, h: &mut LeaseHandle, b: BackOff) -> Result<(), CommitError>;
    async fn release_after_shutdown(&self, h: LeaseHandle) -> Result<(), CommitError>;
}
```

- `RunRecord`: the claim (run id, token, ids of the passed events), the class (`Completed`,
  `Stopped`, `Deterministic`) with its output, error text and `RunUsage`, the session rows as
  `Vec<SessionRow { kind: String, payload: serde_json::Value }>`, the run nonce, a
  `close_thread` flag, and the `MemoryCommitSet`. `RunUsage` is a core struct (input, output,
  cached input and reasoning tokens). The service converts from Helikon `SessionEvent` and
  `TokenUsage`.
- `LeaseKeeper::start` gets an `Arc<dyn RunCommitter>`. The keeper calls each method under its
  mutex. Every COMMIT goes through `commit_or_poison`. The keeper calls `record_complete` after a
  successful `commit`, `back_off`, and a `screen_out` that flags every event.
- `CommitError` becomes `{ LeaseLost, OutcomeUnknown, MemoryRejected(MemoryApplyError),
  Store(String) }`. `OutcomeUnknown` is a failed COMMIT statement or a dropped future: the keeper
  poisons the handle and cancels the token, and the worker drops the keeper. Only the `Display` text
  of `MemoryRejected` goes into `run.error`, `last_error` and the logs, so the `path` field does not
  leave the process (SMA-789 §6.13). The lost-lease metric counts only `LeaseLost`.
- *Replaced:* `ClaimRepository::complete`, `keeper.complete`, `CompleteError` and
  `lease_store/complete.rs` are removed in the same change. Their tests (`complete_pg.rs`, the
  `complete` steps of `lease_property_pg.rs`, `lease_crash_pg.rs` and `keeper_pg.rs`) move to
  `commit` with a `Completed` record. `RunHandle` and `RunOutcome` in `run.rs` are replaced by
  `RunRecord` and `RunClass`. One path to `done` is easier to prove than two.
- Every write uses only the events that passed the screen (except the back-off after a screen
  error, §6.3).

**Full commit (Completed, Stopped).** One transaction with `SET LOCAL lock_timeout = '5s'`:

1. Lock the lease row `FOR UPDATE`. Check `token = $t AND expires_at > clock_timestamp()`.
2. `UPDATE run SET status, finished_at, final_output, input_tokens, output_tokens,
   cached_input_tokens, reasoning_tokens WHERE id = $run AND status = 'running' AND
   lease_token = $t RETURNING thread_id`. Zero rows means "already committed or lease lost". Roll
   back with `LeaseLost`.
3. `write_session` (§6.6).
4. Update the thread (`last_run_at`, and the predecessor's `successor_id` where it is empty), as the
   SMA-787 `complete` does.
5. Check that the scope of the `MemoryCommitSet` is `(agent, principal)` of the inbox row. If the
   set is not empty: `apply_in`. On `Conflict` or `CapExceeded`, roll back the whole transaction
   and return `MemoryRejected`. The run is then transient.
6. `UPDATE inbox_event SET status = 'done' WHERE id = ANY($passed) AND claimed_run_id = $run AND
   status = 'pending'`.
7. COMMIT.

A `Store` error before COMMIT retries the whole transaction up to 2 times, with no new model call.
Step 2 makes a retry harmless. A COMMIT error is `OutcomeUnknown` and is never retried.

**Failure commit (Deterministic).** This is the ADR-0028 (b) amendment that Sven decided at SMA-789
Gate 1: the memory changes commit, and the session rows are dropped. One transaction: steps 1 and
2 with `status = 'failed'`, `error_class = 'deterministic'`, `error = <text>`; `thread.closed = true`
when `close_thread` is set; step 5 (a memory refusal here rolls back, and the run becomes
transient); the back-off update of §6.9 for the passed events; then COMMIT.

**U+0000.** The service replaces U+0000 in `final_output`, `error` and every session string before
the write. A remaining SQLSTATE `22P05` or `22021` is deterministic, so the model does not run again
for a write that cannot pass.

### 6.9 Back-off, screen-out, shutdown and `dead`

- **`back_off`** runs after a transient failure, after the rollback (or when no transaction was
  opened). One short transaction, fenced like step 1: `run.status = 'failed'`,
  `error_class = 'transient'`, `error`; and for the target events
  `not_before = clock_timestamp() + delay`, `last_error = <text>`. After a failed `back_off`, the
  worker drops the keeper and does not call `release`. The events then become free by lease
  expiry, with no back-off.
- **Delay.** `delay = min(base × 2^(n−1) × jitter, max)`, with saturating math, where `n` is the
  event's `attempts` (counted at the claim) and `jitter` is in [0.8, 1.2]. The jitter is applied
  before the cap, so the delay never exceeds `max`. For `ModelError::RateLimited { retry_after_ms }`,
  the delay is `max(delay, retry_after_ms)`, also capped. Defaults: `retry.base_delay_secs` 30
  (range 1 to 3600) and `retry.max_delay_secs` 1800 (range 1 to 86400, and at least the base). With
  5 attempts, the retries cover about 7.5 minutes. The deterministic failure uses the same delay.
- **`screen_out`** is §6.3. It returns `ScreenResult::{AllScreened, Continue}`. A failed COMMIT is
  `OutcomeUnknown`, and the worker drops the keeper.
- **`release_after_shutdown`** is one transaction: lock the lease row `FOR UPDATE` and check the
  token and the expiry; if the check fails, change nothing. Otherwise set the open run to
  `discarded`, decrement `attempts` for the passed events
  `WHERE claimed_run_id = $run AND status = 'pending' AND attempts > 0`, and expire the lease. A
  deploy is not a failure, so it must not move events toward `dead` (A7). Plain `release` keeps its
  SMA-787 behaviour, with no decrement. A crash or a lost lease keeps the attempt (SMA-787 §8.4.1).
- **`dead`.** The claim of SMA-787 moves an event with `attempts >= max_attempts` to `dead`. This
  issue keeps that rule, and the `last_error` of the last failure stays in place: `DEAD_SQL` in
  `lease_store/claim.rs` changes to `last_error = COALESCE(last_error, $4)`. The assertion in
  `claim_pg.rs` that expects `DEAD_REASON` keeps its case (an event with no earlier error), and a
  new case checks that an earlier error stays.

### 6.10 The `run` row

Migration `m0004` adds to `run`: `final_output text`, `error text`, `error_class text`,
`input_tokens bigint`, `output_tokens bigint`, `cached_input_tokens bigint`,
`reasoning_tokens bigint`. Status values become `running`, `completed`, `stopped`, `failed`,
`screened`, `discarded` (and later `deferred`). There is still no CHECK, because later issues add
values. `RunStatus` in the core gains `Screened` and `Discarded`. The table name stays `run` (the
ADR name). The trigger events are in `run_event`, and the session id is `run.thread_id`. SMA-792
adds the "message / silent" outcome from the outbox rows of the run.

### 6.11 Tests

Every test below must fail when the behaviour it targets is removed. The plan records one mutation
for each crash and fence test, and the result of that mutation.

- **Unit tests** in the core: the event renderer (event data that contains `</event-<nonce>>` or
  `<memory-<nonce>` cannot close or forge a block; absent `time` and `data`), the delay (n = 1, the
  cap, `retry_after`, n = 100 with no overflow), the config ranges at both ends
  (`each_range_is_enforced_at_both_ends`).
- **Unit tests** in the service: the class of every variant in §6.7, the memory filter and the NUL
  replacement of `write_session`.
- **Sessions testkit**: `run_all` against both setups of §6.6.
- **Postgres tests** (`tests/*_pg.rs`, Docker-gated as today), one for each path:
  - full commit; failure commit with memory and back-off together; a memory `Conflict` at commit
    (rollback, then transient); a commit with guards and no change (SMA-843); the scope check;
  - a commit `Store` retry; `OutcomeUnknown` poisons the handle;
  - the memory filter in the real commit path (a run with a memory file; no `session_event.payload`
    contains `<memory-` or the file content);
  - `screen_out` (partial and all), and a failed COMMIT of `screen_out` and of `back_off`;
  - `back_off` and the `dead` move with the kept `last_error`;
  - `release_after_shutdown` refunds the attempt; a stale worker A whose lease expired, after B
    claimed again, changes no attempt of B;
  - the candidate filter: an inbox with no configured agent is never taken;
  - `max_runs_per_lease` releases a busy inbox, and a second inbox gets a turn.
- **Crash matrix, in-process.** One `FaultPoint` port (a trait with a no-op default) serves these
  tests and the binary barrier. The in-process double waits on a pending future at the chosen stage,
  and the test aborts the worker task. The double never panics (a panic inside a transaction drop
  can abort the process). Stages (ADR (o)):
  1. after a tool call, before the commit;
  2. inside the commit, before COMMIT;
  3. after the commit, before the release;
  4. a heartbeat partition: the test holds the lease row lock while worker A is inside its commit,
     so that A's step-1 check runs after the database expiry.
  Workers A and B run different scripts: A writes `/a.md`, B writes `/b.md`. After the take-over the
  test checks: `/a.md` does not exist (except at stage 3, where A committed); every
  `session_event.run_id` is a run of the committed worker; `attempts` is 2; A's run row is still
  `running` (stages 1, 2, 4); and the events are `done` once. Then the test calls
  `RunCommitter::commit` with A's record through a raw handle and asserts `LeaseLost` and no changed
  row in `run`, `inbox_event`, `session_event` and `memory_file`. Mutation: remove the token and
  expiry check of step 1, and the test must fail.
- **SIGKILL test on the real binary.** The `test-model` cargo feature registers the `scripted` model
  from a script file named by an env variable, and connects the `FaultPoint` barrier: at a named
  stage the worker creates a file and waits. The test posts a webhook event through the HTTP ingress
  (with the existing fake IAM), waits for the barrier file after the memory tool call, sends
  SIGKILL, starts a second process, and checks that the event is `done`, the memory file of the
  second process exists once, the first process's file does not, and the session events are
  persisted. This is the "Done when" test of the Linear issue.
  - The agent-host Moon `test` task passes `--features test-model`. The test target fails, not
    skips, when the feature is off.
  - `#[cfg(all(feature = "test-model", not(debug_assertions)))] compile_error!(…)` stops a release
    build with the feature.

### 6.12 Configuration

New keys, with defaults and ranges, all checked in `validate`:

| Key | Default | Range |
|---|---|---|
| `worker.concurrency` | 4 | 1 to 64 |
| `worker.poll_interval_ms` | 1000 | 100 to 60000 |
| `worker.max_runs_per_lease` | 8 | 1 to 1000 |
| `run.max_turns` | 16 | 1 to 256 |
| `run.timeout_secs` | 300 | 1 to 3600 |
| `retry.base_delay_secs` | 30 | 1 to 3600 |
| `retry.max_delay_secs` | 1800 | 1 to 86400, and ≥ base |
| `[[agents]]` | empty | §6.2 |

### 6.13 The ADR-0028 amendment

Before the code, the plan updates ADR-0028 in Notion (status stays Proposed) with:

1. (b): a deterministic failure also commits the memory change set; the session rows are dropped.
2. (b): a transient failure writes a `failed` run row and the back-off in a second short
   transaction after the rollback.
3. (b): the new run status `screened`, and the per-event screen before the model loop.
4. (b) and (c): a graceful shutdown gives the attempt back; a crash still counts it.
5. (b): `RunCommitter` has the four methods of §6.8.

### 6.14 Metrics (provisional, SMA-798 owns the names)

`agent_host_runs_total{class}`, `agent_host_run_duration_seconds`,
`agent_host_events_screened_total`, `agent_host_commit_memory_conflicts_total` (SMA-789),
`agent_host_commit_outcome_unknown_total`, `agent_host_worker_restarts_total`,
`agent_host_inbox_lag_seconds` (the age of the oldest due event, read in the poll query), and
`agent_host_event_attempts` (a histogram, observed when an event goes to `done` or `dead`).

## 7. Approaches considered

- **Commit path.** (1) The keeper calls `RunCommitter` under its mutex (chosen). It reuses the
  fencing, the poisoning and the token rotation of SMA-787. (2) A committer outside the keeper
  that takes the token from the caller: it needs a second copy of the handle state and breaks the
  single-owner rule. (3) Helikon's `PostgresSession`: it writes outside the run transaction, uses
  sqlx with its own migration table, and ADR (b) rejects it.
- **Screening.** (1) A host step per event before the run (chosen): only the flagged event goes to
  `dead`. (2) A Helikon input guardrail: a tripwire fails the whole run, and the retries send the
  whole batch to `dead`. Sven chose (1) on 2026-11-22.
- **Crash tests.** (1) In-process fault points only: fast, the full matrix, but no real process
  crash. (2) SIGKILL only: real, slow, and test code in the binary. (3) Both (chosen by Sven on
  2026-11-22): the matrix in-process, and one SIGKILL test on the real binary.
- **Scope.** (A) A proven spine with a scripted model (chosen by Sven on 2026-11-22). (B) Also a real
  provider and an agent table: much larger, and blocked by the `deny.toml` provider question.

## 8. Decided by Sven

- 2026-11-20 (SMA-789 Gate 1): memory changes commit on a deterministic failure. This issue amends
  ADR-0028 (b) in Notion and implements it.
- 2026-11-21 (SMA-843): a guard stays armed until the commit, so a commit-time `Conflict` can come
  from a file another writer added during the run. No re-check at later commands.
- 2026-11-22: scope A; screen per event and dead-letter only the flagged event; crash tests in
  both forms.
- 2026-11-22 (Gate 1): spec approved. Accepted risk for v1: a memory file that makes every run
  fail deterministically sends every later event of that inbox to `dead`. This change adds no
  guard against it.

## 9. Calls made by the coordinator (check these)

- The wildcard of the error match is transient.
- A graceful shutdown refunds the attempt (§6.9).
- No pool start and no inbox take without a configured agent (§6.1).
- `ContextLengthExceeded` closes the thread.
- Retry defaults: base 30 s, cap 30 min, `max_attempts` 5 (about 7.5 minutes of retries).
- `DEAD_SQL` keeps the last error (`COALESCE`).
- `max_runs_per_lease` = 8.
- A separate lease-store pool.
- `ClaimRepository::complete` is removed in this change.

## 10. Left to the builder

- Index choices on `session_event` and `run` beyond the primary keys.
- The exact jitter source and the exact error text formats.
- The module layout inside `application/` and `adapters/`.

## 11. Follow-up issues (made at Gate 1, 2026-11-22)

- SMA-851: a real model provider, with a resolution of the `aws-lc-rs` ban.
- SMA-852: an agent-definition registry (table or IAM), replacing `[[agents]]` in the config file.
- SMA-853: connect the SMA-754 detectors to `EventScreen`.

## 12. Challenge changelog (Stage 2, 2026-11-22)

Verdict of the challenger: APPROVE WITH CHANGES.

Folded in:

- BLOCKER — the shutdown refund used `release`, which cannot tell a shutdown from other ends and
  checks no lease row: now `release_after_shutdown` with a lease-row fence, a pool shutdown token
  and `keeper.is_lost()` (§6.1, §6.9), and a stale-worker test.
- BLOCKER — the SMA-843 API was not in the base: the branch now includes PR 522; SMA-843 is in
  "Blocked by"; a guard-only commit test is added.
- MAJOR — Helikon types in the core port: core-owned `SessionRow` and `RunUsage`; the class mapping
  moved to the service (§5, §6.8).
- MAJOR — the default config sent events to `dead`: candidate filter and no pool without agents
  (§6.1, A7).
- MAJOR — one busy inbox could hold a worker: `max_runs_per_lease` (§6.1).
- MAJOR — transient retries ended in about 75 s: larger defaults and `retry_after` (§6.9).
- MAJOR — `screen_out` and `back_off` had no contract: four `RunCommitter` methods, poisoning and
  `record_complete` rules (§6.8, §6.9).
- MAJOR — the crash tests passed with the fence removed: different scripts, the stale-commit check,
  the stage-4 lock, a mutation for each test, the feature in the Moon task (§6.11).
- MAJOR — untested decisions: a Pg test for each (§6.11).
- MAJOR — the heartbeat could wait for a pool connection: pool sizes and a lease-store pool (§6.1).
- MAJOR — the event tag: the run nonce, index as an attribute, `time` and `data` fallbacks (§6.4).
- MINOR — all items folded in: the id example, the sequence base, U+0000, the composite keys, the
  delay overflow and the jitter order, `SharedModel`, the class table gaps, the config ranges,
  `MemoryRejected` and the scope check, the commit retry and `lock_timeout`, one `FaultPoint` port
  with no panic and the `compile_error!`, the panic restart, `ContextLengthExceeded`, the metrics,
  the async `Result` screen, the shared `write_session`.
- MINOR — `Compacted` projects as a system item: recorded as a limit for SMA-791, not changed now
  (§6.6). v1 writes no `Compacted` event.

Not folded in:

- The batch byte limit (part of the `ContextLengthExceeded` item): the claim is SMA-787's, and the
  thread close already stops the repeat. Recorded as N8.
- A separate transient budget (an alternative in the retry item): it needs a new column and an ADR
  change to the attempt rule. The larger defaults solve the measured problem.

Answered from the challenge questions:

- The ADR amendment comes before the code (§6.13).
- `AgentResolver::resolve` runs after the claim and before the screen (§6.2).
- Open for Gate 1: a memory file that makes every run fail deterministically (see the Gate 1
  message).
