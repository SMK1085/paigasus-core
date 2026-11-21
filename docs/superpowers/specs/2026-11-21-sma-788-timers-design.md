# SMA-788: Timers, follow-ups, schedules and the scheduler loop

- Linear: SMA-788 (project Paigasus Agent Host). Blocks SMA-795 and SMA-796.
- Stacked on PR 518 (SMA-787, the event inbox). That PR is not merged. This branch rebases onto
  `main` after it merges.
- Decision source: ADR-0028 (Proposed), the agent-host ADR spec
  `2026-11-18-sma-785-agent-host-adr-design.md` decision (l), and the Anthropic article
  "Building always-on agents" (the project's source).
- Reviews: a builder check and two adversarial challenges (both: approve with changes). Their
  findings are in this version. §16 lists the changes.

## 1. Intent

An always-on agent must act without a new message from a person. It must check on a reply
tomorrow, or send a report every morning. A timer is the way an agent wakes itself.

The article and ADR-0028 fix the shape. A timer lives in its own store, apart from the inbox. A
timer never starts a run. A scheduler polls the store, updates each due timer, and adds an event
to the inbox. The run that follows is a normal run of an inbox event.

## 2. Decisions made with Sven

| # | Decision | Who |
|---|---|---|
| S1 | Stack this branch on PR 518. Rebase onto `main` after PR 518 merges. | Sven |
| S2 | Caps on timers, with the values in `agent-host.toml` (option C). | Sven |
| S3 | Cron evaluation with `croner` 4.0.3 and its `jiff` feature (option A). | Sven |
| S4 | The fire event id is `timer:<timer_id>:<fire_at>`, not `timer:<timer_id>:<fire_seq>`. | Claude's proposal, shown to Sven. He did not object |
| S5 | A creation budget against a self-wake loop, not a fire budget (§9.2). | Sven, at GATE 1 |
| S6 | Vixie cron behaviour on a fall-back day: interval patterns also fire in the repeated hour, fixed-hour patterns fire once (§6.2, R3). | Sven, at GATE 1 |
| S7 | A follow-up after the thread TTL wakes on a fresh session. The note carries the context. No ADR (f) change (§10.2). | Sven, at GATE 1 |

Every other decision in this document is Claude's call. §12 lists them.

## 3. Goals, non-goals, anti-goals

### Goals

- G1. A `timer` table, separate from the inbox tables.
- G2. A follow-up: a one-shot timer that the agent sets for itself. It fires once and then has
  the status `done`.
- G3. A schedule: a recurring timer with a cron expression and an IANA time zone. After each fire,
  the scheduler moves `next_fire_at` to the next slot.
- G4. Fires that the service missed while it was down fire once on recovery, not once per missed
  slot.
- G5. A scheduler loop. It polls on a fixed interval, claims due timers with
  `FOR UPDATE OF t SKIP LOCKED`, advances them, and writes an inbox event in the same
  transaction.
- G6. Four Helikon tools: `schedule_follow_up`, `create_schedule`, `list_timers` and
  `cancel_timer`. Each tool works only on the inbox of its run.
- G7. A deterministic fire event id, so the inbox deduplicates a double fire.
- G8. Caps and a creation budget that stop an agent from waking itself too often (S2, S5).

### Done when

A daily schedule fires once per day across a scheduler restart and a DST change. An
agent-created follow-up wakes the agent at the set time. "Wakes the agent" means that a pending
event is in the agent's inbox at the set time. The worker that runs it comes in SMA-790. §14.1
names the tests that prove each part.

### Non-goals

- NG1. The worker and the `RunCommitter` (SMA-790). This branch supplies the timer part of the
  commit as a function that the committer calls, with a written contract (§8).
- NG2. Waking the worker when an event arrives. The article says the scheduler wakes the worker.
  The worker does not exist yet, and the SMA-790 worker polls. A nudge (for example `NOTIFY`) is
  a later change.
- NG3. A console or HTTP API for timers. Only the run commit creates and cancels timers in this
  branch.
- NG4. Approval-expiry timers (SMA-795) and dreaming schedules (SMA-796). This branch prepares
  for them with the `origin` column and the per-origin `source` (§6, §11.2).
- NG5. Removal of old `done`, `cancelled` and `failed` rows. Retention is a later change. That
  change must keep `origin = 'agent'` rows for at least 24 hours after `created_at`, because the
  creation budget counts them (§9.2).
- NG6. A Temporal adapter for the `Scheduler` port.
- NG7. Telling the agent or the principal that a timer went to `failed`. A metric and a warning
  log show it. This is an accepted limit of v1.

### Anti-goals

- AG1. A timer starts a run directly, or the scheduler calls the worker.
- AG2. One schedule fires N times after N missed slots.
- AG3. A daily schedule fires twice, or not at all, on a DST change day.
- AG4. An agent reads, changes or cancels a timer of another inbox, or learns that such a timer
  exists.
- AG5. The scheduler fires a timer by the worker clock. The database clock decides what is due.
- AG6. One bad timer row stops the scheduler from firing the other timers.
- AG7. A scheduler pass makes ingress to an inbox wait.
- AG8. A short database incident ends a healthy timer for ever.

## 4. Constraints

- C1. The database clock decides correctness (ADR spec, lines 106-107). "Due" means
  `next_fire_at <= now()` in Postgres. A transaction that can wait on a lock uses
  `clock_timestamp()`.
- C2. The event insert and the timer update are in one transaction (decision (l)).
- C3. The inbox dedup key is `UNIQUE (inbox_id, source, ce_id)` (SMA-787, `m0003`). The host sets
  `source`. Later issues make trust decisions on `source`, never on `type` alone (SMA-787 spec,
  line 158). The NATS consumer does not check the reserved `agents.` type prefix, so a type is
  never a trust signal.
- C4. A run adds or cancels a timer only in a run buffer, and the run commit writes it (M0 port
  docs, `ports.rs`). The memory tool (SMA-789) uses the same pattern with `MemoryWorkspace` and
  `PgMemoryStore::apply_in`.
- C5. The ports in `paigasus-agent-host-core` are provisional. The owner (SMA-788) may change
  the methods, not the responsibility.
- C6. Rust edition 2024, SeaORM 2 with `sea-orm-migration`, raw SQL migrations, the `jiff` time
  crate, SPDX headers. The workspace denies warnings.
- C7. **The runtime image has no system time zone data.** The chisel cut in `rs/Dockerfile`
  holds `base-files`, `libc6_libs`, `libgcc-s1_libs` and `ca-certificates_data`, and no
  `tzdata`. On Linux, `jiff`'s global database reads `/usr/share/zoneinfo` first.
- C8. **`tzdb-zoneinfo` cannot be turned off.** It is a default feature of `jiff` 0.2.46, and
  `paigasus-helikon-core` 0.6.0 depends on `jiff` with default features. Cargo feature
  unification turns it on in the host binary.
- C9. The `data` of an `agents.*` event is the proto3 JSON mapping of an `agents/v1` message
  (ADR (e)). `contracts/proto/paigasus/agents/v1/agents.proto` rules: lowerCamelCase JSON names
  (rule 2), and a message never repeats an envelope attribute (rule 4).

## 5. Approaches (cron evaluation)

The repo had no cron crate.

| Option | Trade-off |
|---|---|
| **A. `croner` 4.0.3, feature `jiff`** (chosen) | MIT licence. The most widely used of the three. It works with `jiff` directly, with no `chrono` conversion. |
| B. `jiff-cron` 0.3.0 | Built for `jiff`. Younger, and fewer projects use it. |
| C. Own evaluator on `jiff` | Full control. A few hundred lines of new code, each with tests. |

The `cron` crate was dropped: it works only with `chrono` and would also need `chrono-tz`.

**How `croner` is used.** `croner` only matches civil times. It searches with
`jiff::civil::DateTime`, which it supports (`CronDateTime` for `jiff::civil::DateTime`), so no
zone is involved. Our code does the resolution to instants and the DST rules (§6.2). The
dependency is `croner = { version = "4.0.3", default-features = false, features = ["jiff"] }`, so
`chrono` is not pulled in. `croner` adds `derive_builder` and `strum` to the dependency graph. The
first plan task proves the civil matches for the patterns in §14 and checks the new dependency
tree against `repo:deny`, `repo:osv` and `repo:machete`. If either fails, the builder stops and
reports to the coordinator. The builder does not change to option B or C without Sven's
decision.

### 5.1 Time zone data

- Add the `jiff` feature `tzdb-bundle-always`. Do not try to turn `tzdb-zoneinfo` off (C8).
- Timer code never uses `jiff`'s global database (`jiff::tz::db()`, `Timestamp::in_tz`,
  `Zoned::now`, and similar). A `TzResolver` port gives a `TimeZone` for a name. Its adapter uses
  an explicit `TimeZoneDatabase::bundled()` handle. `CronSchedule::parse` takes the
  `&dyn TzResolver` and resolves the zone itself. `next_after` uses the zone that `parse` stored.
- So every host (the Mac, CI and the container) uses the same pinned tzdb, and a tzdb update is a
  visible dependency bump of `jiff-tzdb`.
- Measured: the image smoke (`ci/images/run.sh`, `smoke agent-host`) seeds a due `0 9 * * *`
  schedule in `Europe/Berlin`. It checks that the agent host resolves the zone and fires. After
  one poll interval it checks that exactly one event exists. That exact check is skipped from
  08:58 to 09:01 Berlin time, when a second fire is correct.
- The required check is a unit test: `TimeZoneDatabase::bundled().get("Europe/Berlin")` succeeds.
  It fails without the feature on every host. The image smoke test (`build + smoke`) resolves
  `Europe/Berlin` in the container as a second control.

## 6. Domain (`paigasus-agent-host-core`)

### 6.1 Types

The M0 `Timer` type changes as follows.

- `Timer { id: TimerId, inbox: InboxId, kind: TimerKind, thread: Option<ThreadId>, note: Note,
  origin: TimerOrigin, status: TimerStatus, scheduled_at: Timestamp, next_fire_at: Timestamp }`.
  `scheduled_at` is the scheduled instant of the pending firing: the due time of a follow-up, or
  the instant of the slot of a schedule. `next_fire_at` is the due time. It equals `scheduled_at`,
  except during a back-off (§11.1), when it is later. The constructor keeps
  `next_fire_at >= scheduled_at`.
- `TimerKind::FollowUp` (the scheduled instant is `scheduled_at`) or
  `TimerKind::Schedule { schedule: CronSchedule, next_slot: Slot }`, where
  `Slot { civil: civil::DateTime, second: bool }` is the civil slot that `scheduled_at` resolves,
  with the occurrence flag of R3 (§6.2).
- `fire_seq` is removed (S4).
- `TimerStatus`: `Active`, `Done`, `Cancelled`, `Failed`.
- `TimerOrigin`: `Agent` (made by an agent tool) or `System` (made by the host, for example the
  SMA-795 expiry timer). Only `Agent` timers count against the caps and the budget.
- `Note`: the text that the agent writes for its future self, at most 1000 characters, no NUL.
  The fire event carries it, so the agent knows why it woke. The note is untrusted model text.
  It is never written to a log or to the `failure` column. The SMA-790 renderer shows it as data
  from the agent's past run, not as an instruction from the host (the same rule as SMA-789
  §6.13).
- `CronSchedule`: a validated value object that holds the cron text, the IANA zone name, the
  resolved `TimeZone` and the parsed `croner` pattern. It is made only through
  `CronSchedule::parse(cron, tz_name, &dyn TzResolver)`. `PartialEq` and `Eq` compare the cron
  text and the zone name only.
- The `Timer` constructor **rounds `scheduled_at` and `next_fire_at` up** to the next whole second, so a follow-up
  never fires early and every path (the tools and SMA-795) gives one text form of `fire_at`.
- `TimerView`: the read model for `list_timers`. It is `Timer` plus `fire_count`,
  `last_fired_at` and `created_at`.

### 6.2 The next slot and the DST rules

A slot is a **civil time**: a wall-clock date and time in the schedule's zone, with no offset.
The cron pattern matches civil times. An instant is a point on the UTC time line. The design
keeps the two apart, so the DST rules are our code and not the crate's behaviour.

The stored state of a schedule is the pair (`next_slot`, `scheduled_at`). The walk starts from
the stored civil slot, not from the instant. So a tzdb rule change between two fires cannot move
the start of the walk and cause an extra fire.

`CronSchedule::next_after(fired_slot: Slot, now: Timestamp, bound: u32)
-> Result<(Slot, Timestamp), WalkError>` (the R3 occurrence step runs before step 3):

1. Take `c` = `fired_slot`. For a first slot, `c` is the civil time of the creation time.
2. If `c` is more than 25 hours before the civil time of `now`, set `c` to that civil time minus
   25 hours. A long outage then does not make the walk visit every missed slot.
3. Find the next civil slot strictly after `c` with `croner`.
4. Resolve it to an instant with R1 and R2.
5. If that instant is not after `now`, set `c` to this slot and go back to step 3. Missed slots
   are skipped (G4).
6. Return the civil slot and its instant.

`bound` limits the loop of steps 3 to 5. The production value comes from a constant, and tests
pass a low value. An overrun is `WalkError::BoundExceeded`. A `croner` error (including its
year-5000 search limit) is `WalkError::Cron`. There is no "no further slot" case: a five-field
pattern always has one, and the old R5 is removed.

The rules:

- R1. **Spring forward (a gap).** A civil slot that does not exist on that day (for example 02:30
  in `Europe/Berlin` on the last Sunday of March) resolves to the first valid instant after the
  gap (03:00 local). The builder gets that instant from the zone's transition data in `jiff`.
  `jiff`'s `compatible` disambiguation gives 03:30, so it is not correct here.
- R2. **Fall back (an overlap).** A civil slot that occurs twice on that day resolves to its first
  occurrence (the earlier instant).
- R3. **The repeated hour (S6, Vixie cron behaviour).** A pattern is an **interval pattern** when
  its hour field is `*` or a `*/n` step. Every other pattern is a **fixed-hour pattern**. The
  `@hourly` alias is an interval pattern; the other aliases are fixed-hour patterns.
  - A fixed-hour pattern fires each civil slot once, at its first occurrence (R2). `30 1 * * *`
    fires once on a fall-back day.
  - An interval pattern fires a civil slot in the overlap twice: at its first occurrence and at
    its second occurrence. Berlin's repeated hour is 02:00–02:59 (03:00 CEST goes back to
    02:00 CET). So `0 * * * *` fires at 02:00 CEST and at 02:00 CET in Berlin, and keeps a real
    gap of 1 hour. `*/15 * * * *` fires all eight slots of the repeated hour, in instant order.
    New York's repeated hour is 01:00–01:59.
  - The walk state therefore has an occurrence flag (`first` or `second`) next to the civil slot.
    The walk uses the **fold rule** for an interval pattern in an overlap: it first walks every
    slot of the overlap with `first`, then goes back to the first slot at or after the start of
    the overlap with `second`, walks those, and then leaves the overlap. So the fires come in
    instant order. Plan Task 3 holds the exact steps. A fixed-hour pattern never uses `second`.
  - The event id stays `timer:<timer_id>:<fire_at>`. The two occurrences have different instants,
    so they have different ids.
- R4. A slot that was skipped because its instant was not after `now` (step 5) does not fire.
  This applies, for example, when the scheduler recovers at the second 01:15 of a fall-back day.
  Several civil slots can resolve to one instant in a gap (for example 02:00 and 03:00 of
  `0 * * * *` on a spring day). That instant fires once.

**Cron syntax.** A pre-check runs on each field, after `croner`'s month and weekday names are
replaced, because `croner` always accepts its extensions and has no option to refuse them.

- Accepted: the five-field form (minute, hour, day of month, month, day of week); numbers,
  ranges, lists and steps; month and weekday names; the aliases `@hourly`, `@daily`, `@weekly`,
  `@monthly`, `@yearly` and `@annually`.
- Refused: a seconds or year field (`croner`'s `Seconds::Disallowed`); the extensions `L`, `W`,
  `#` and `?`; the `+` prefix of the day-of-week field; and a pattern that restricts both the day
  of month and the day of week. The last rule removes the OR-or-AND question: Vixie cron and
  `croner` give different results for `0 9 */2 * MON`, and no rule in between is safe.
- A test pins each rule. The tool refusal text names the refused part.

### 6.3 The fire plan

A pure function decides each fire. It takes the timer and the database `now` and returns a plan.

`plan_fire(timer, now, bound) -> Result<FirePlan, PlanError>`, with
`FirePlan { fire_at, next: Next }`, where `Next` is `Done` or
`Advance { slot: Slot, at: Timestamp }`.

- `fire_at` is the stored `scheduled_at`. For a schedule that missed slots, this is the first
  missed slot. So a repeated fire of the same row and slot gives the same `fire_at`, also after a
  rollback, a retry and a back-off. A back-off moves only `next_fire_at`, so the event id and
  `TimerFired.fireAt` always carry the scheduled slot and never the retry time.
- For a follow-up, `next` is `Done`.
- For a schedule, `next` is `Advance(next_after(next_slot, now, bound))`. `next_after` starts
  from the fired slot, not from `now`.
- A late fire (for example after a three-day outage) fires once. Its event carries `fire_at`, and
  the envelope `time` is the actual fire time, so the agent can see the delay.

`Timer::fire_event_id(fire_at) -> String` gives `timer:<timer_id uuid>:<fire_at>`. `fire_at` is
written in RFC 3339 UTC with a `Z` suffix and whole seconds. A test pins the exact text.

## 7. Storage (`paigasus-agent-host`)

### 7.1 Migration `m0004_create_timer`

```sql
CREATE TABLE timer (
  id             uuid PRIMARY KEY,
  inbox_id       uuid NOT NULL REFERENCES inbox (id),
  kind           text NOT NULL CHECK (kind IN ('follow_up', 'schedule')),
  origin         text NOT NULL CHECK (origin IN ('agent', 'system')),
  status         text NOT NULL CHECK (status IN ('active', 'done', 'cancelled', 'failed')),
  thread_id      uuid,
  cron           text,
  tz             text,
  next_slot      timestamp,              -- civil time, no zone; schedules only
  next_slot_second boolean,              -- second occurrence in an overlap (R3); schedules only
  note           text NOT NULL CHECK (char_length(note) <= 1000),
  scheduled_at   timestamptz NOT NULL,   -- the scheduled instant of the pending firing (the event time)
  next_fire_at   timestamptz NOT NULL,   -- the due time; later than scheduled_at only during a back-off
  last_fired_at  timestamptz,
  fire_count     bigint NOT NULL DEFAULT 0,
  fire_attempts  integer NOT NULL DEFAULT 0,
  failure        text CHECK (char_length(failure) <= 500),
  created_at     timestamptz NOT NULL DEFAULT now(),
  updated_at     timestamptz NOT NULL DEFAULT now(),
  FOREIGN KEY (thread_id, inbox_id) REFERENCES thread (id, inbox_id),
  CHECK ((cron IS NULL) = (tz IS NULL)),
  CHECK ((cron IS NULL) = (next_slot IS NULL)),
  CHECK ((cron IS NULL) = (next_slot_second IS NULL)),
  CHECK ((kind = 'schedule') = (cron IS NOT NULL)),
  CHECK (isfinite(scheduled_at)),
  CHECK (isfinite(next_fire_at)),
  CHECK (next_fire_at >= scheduled_at)
);
CREATE INDEX timer_due ON timer (next_fire_at) WHERE status = 'active';
CREATE INDEX timer_inbox_active ON timer (inbox_id) WHERE status = 'active';
CREATE INDEX timer_inbox_agent_created ON timer (inbox_id, created_at) WHERE origin = 'agent';
```

- `fire_attempts` counts fire attempts that failed with a permanent-looking error (§11.1). A
  successful fire sets it to 0.
- `scheduled_at` is the instant that the event carries (§6.3). A fire that advances a schedule
  sets `scheduled_at` and `next_fire_at` to the instant of the new slot.
- The `down` is `DROP TABLE timer`.
- Two existing tests count migration steps back from the newest: `tests/schema_pg.rs:164`
  (`down(Some(1))` for m0003) and `tests/memory_pg.rs:346` (`down(Some(2))` for m0002). Both
  become one step larger.
- Rollback, measured: a binary built before m0004 refuses to start on a database that has m0004
  applied. The migration run fails with `Migration file of version '<version>' is missing, this
  migration has been applied but its file is missing`. The test used a foreign version row, not a
  real old binary. State this result in the PR.

### 7.2 A transactional inbox insert

`PgInboxRepository` runs its insert only on the pool. C2 needs it inside the scheduler's
transaction. The builder moves the SMA-787 insert into one function that takes any SeaORM
`ConnectionTrait` (a pool or a `DatabaseTransaction`) and returns the same `InsertOutcome`.
`PgInboxRepository::insert_event` calls that function, so the SQL exists once. The SMA-787 tests
must stay green.

### 7.3 The timer store

`PgTimerStore` implements the `TimerRepository` port. The port is read-only outside a run:

- `list_active(inbox, origin) -> Vec<TimerView>`: the active timers of one inbox and origin, by
  `next_fire_at`.
- `get(inbox, id) -> Option<TimerView>`: one timer, only if it belongs to that inbox.
- `active_agent_ids(inbox) -> Vec<TimerId>`: the ids of the active `agent` timers, for the count
  cap (§10.3).
- `count_agent_created_since(inbox, since: Timestamp) -> u32`: for the budget (§9.2).

The M0 `insert` and `cancel` methods are removed. No caller exists outside a run in v1 (NG3), and
a write path with no cap and no lease fence would break §8. Every query has
`WHERE inbox_id = $1` (AG4). The port docs of `Scheduler::fire_due` change to "processed", not
"fired" (§11.1).

## 8. The commit contract for SMA-790

The tools collect changes in a run buffer (§10.1). `TimerWorkspace::seal()` gives:

```text
TimerChanges {
  inserts: Vec<Timer>,                       // each carries its inbox and origin
  cancels: Vec<(TimerId, TimerOrigin)>,
}
```

The adapter exposes `PgTimerStore::apply_in(txn: &DatabaseTransaction, inbox: &InboxId, caps:
&TimerCaps, changes) -> Result<ApplyReport, ApplyError>`. It has the shape of
`PgMemoryStore::apply_in`.

- **Where it runs.** Inside the run transaction, after ADR (b) step 1 (the `inbox_lease` row,
  `FOR UPDATE`, with the token check), at READ COMMITTED. That lock is the fence: a run that lost
  its lease cannot write timers. `apply_in` also takes
  `SELECT 1 FROM inbox_lease WHERE inbox_id = $1 FOR UPDATE` itself. In the run commit the lock
  is already held, so this costs nothing. It protects a future caller that forgets the lock. It
  never locks the `inbox` row. When that `SELECT` returns no row, the inbox has no lease row. This
  is a bug, so `apply_in` fails with `ApplyError::LeaseMissing` and writes nothing.
- **Thread rows.** The thread row of a run already exists: SMA-787 inserts it at claim
  (`lease_store/claim.rs`). So the composite thread foreign key holds.
- **Savepoint.** It opens a savepoint. On an error it rolls back to the savepoint and leaves the
  caller's transaction usable.
- **Checks.**
  - A missing `inbox_lease` row is `ApplyError::LeaseMissing`, a bug (see the first item above).
  - An insert whose inbox is not `inbox`, or whose status is not `Active`, is
    `ApplyError::InboxMismatch` or `ApplyError::InvalidInsert`. Both are bugs, never model
    behaviour.
  - An `Agent` cancel changes only a row of `inbox` with `origin = 'agent'`. A `System` cancel
    changes only a row of `inbox` with `origin = 'system'`. A cancel of an id in another inbox
    changes nothing and reports `NotActive`.
  - **The count cap.** It counts the active `agent` timers of the inbox before and after the
    change. It refuses only when the count after the change is over `max_active_per_inbox`
    **and** larger than before, as `MemoryCaps::check` does: `ApplyError::CapExceeded { before,
    after, limit }`.
  - **The budget.** Only when the change set has at least one `agent` insert, it counts `agent`
    rows with `created_at > statement_timestamp() - interval '24 hours'` plus the inserts. The
    insert sets `created_at = statement_timestamp()` explicitly, so a timer counts for the full 24
    hours. The default `now()` is the start of the outer transaction. Over
    `max_agent_timers_created_per_day` is `ApplyError::BudgetExceeded { created, limit }`. A run
    with no insert is never refused for the budget.
- **Cancel outcomes.** `ApplyReport` holds, for each cancel, `Cancelled` or `NotActive` (the
  timer fired or was cancelled after the tool call, or the id is not in this inbox). `NotActive`
  is logged at `info` and counted. The agent is not told: the fire event in the inbox tells it
  that the timer fired.

**Recommended handling in SMA-790** (SMA-790 decides):

| Result | Recommended SMA-790 action | Why |
|---|---|---|
| `Ok(report)` | Commit the run. | — |
| `CapExceeded`, `BudgetExceeded` | Roll back the whole run as transient, and let the next attempt run. | The same rule as SMA-789 §6.12. The next attempt reads fresh state, and the tool check gives the model the refusal text. The early tool checks keep this rare. |
| `InboxMismatch`, `InvalidInsert`, `LeaseMissing` | Log at `error`, and roll back the whole run. | A bug. A silent commit would hide it. |
| A database error | Roll back the whole run and retry it. | Transient. |
| A database error that names a failed savepoint rollback | Roll back the whole run. Do not use the transaction again. | The state of the transaction is unknown. |

**Run outcomes.** Timer changes commit only with a run outcome that finishes its claimed events
(`completed`, `deferred`, `stopped`). They never commit with `failed`. This differs from SMA-789
Q3, where memory changes commit on a deterministic failure. The reason: ADR (b) keeps the claimed
events `pending` after `failed`, so the next attempt runs the same events again. A memory write
repeats onto the same file, but a timer insert gets a new `TimerId` each time. With
`max_attempts = 5`, one intent would otherwise make five follow-ups, or five daily schedules.

In this branch, the tests call `apply_in` inside a test transaction that holds an `inbox_lease`
row lock, as SMA-790 will.

## 9. Configuration, caps and budget

### 9.1 The `[timers]` section

A new `[timers]` section in `config.rs` and `agent-host.toml.example`, with the existing
pattern (defaults in `Config::figment()`, range checks in `validate()`):

| Key | Default | Range | Meaning |
|---|---|---|---|
| `enabled` | `true` | — | Run the scheduler loop. |
| `poll_interval_secs` | `5` | 1 to 300 | The time between two passes. |
| `pass_limit` | `100` | 1 to 1000 | The most rows that one pass processes (§11.1). |
| `statement_timeout_ms` | `5000` | 100 to 60000 | `SET LOCAL statement_timeout` for each fire transaction. |
| `min_schedule_interval_secs` | `900` | 60 to 86400 | The smallest allowed civil gap between two slots (§10.3). |
| `max_active_per_inbox` | `50` | 1 to 1000 | The most active `agent` timers per inbox. |
| `max_horizon_days` | `366` | 1 to 366 | The latest allowed follow-up, and the window for the first slot of a schedule. |
| `max_fire_attempts` | `10` | 1 to 1000 | Failed fire attempts before a timer goes to `failed` (§11.1). |

`enabled = false` stops only the loop. The tools still work, so timers wait until a replica with
the loop runs.

### 9.2 A creation budget against a self-wake loop (S5)

The caps of §9.1 do not stop a loop of follow-ups. A fired follow-up is `done` and leaves the
count, so each run could set a follow-up in 1 second, and the next run could do the same. Each
wake is a model run. Sven chose a creation budget:

| Key | Default | Range | Meaning |
|---|---|---|---|
| `min_follow_up_delay_secs` | `60` | 1 to 86400 | The shortest allowed follow-up, from the time of the tool call. |
| `max_agent_timers_created_per_day` | `100` | 1 to 10000 | The most `agent` timers that one inbox can create in a rolling 24 hours, any status. |

- The tool checks the budget with `count_agent_created_since(inbox, now - 24 h)` plus the
  buffered inserts. `apply_in` checks it again (§8), only when the change set has an insert.
- A cancelled or fired timer still counts, so cancel-and-create does not reset the budget.
- The budget is never enforced at fire time. A schedule that exists always fires.
- Sven rejected a fire budget (at most N fires per hour, enforced at fire time), because a
  dropped fire breaks a schedule with no notice.

The schedule load stays bounded by §9.1: at most `max_active_per_inbox` schedules, each at most
once per `min_schedule_interval_secs`. With the defaults that is 4800 wakes per day per inbox in
the worst case. An operator lowers the two values to lower that bound. The bound depends on
`min_schedule_interval_secs`: when an operator lowers it, the bound rises (50 schedules at 60 s
is 72000 fires a day). An operator must change the two values together.

## 10. The agent tools

### 10.1 The run buffer

`TimerWorkspace` is made per run. It gets the run's `InboxId`, its `ThreadId`, the caps, a store
handle, a `TzResolver`, an id generator port (the tools do not call the `mint_uuid` adapter), and
a clock port.

- **The clock.** The clock gives the database `now` at run start plus the monotonic time since
  run start. So a tool call at minute 15 of a run computes `in_seconds` and the "in the future"
  check from the time of the call, not from the run start.
- The workspace holds buffered inserts and cancels, like `MemoryWorkspace`. It is sealed at the
  end of the run. A sealed workspace refuses every command.

### 10.2 The tools

All four tools follow the `MemoryTool` pattern: `impl Tool<Ctx>`, a hand-written JSON schema,
and scope through the constructor. **Every refusal is `Ok(ToolOutput)` text for the model,
argument errors included**, as `MemoryTool` does. For example, both `at` and `in_seconds`, or
neither, give a refusal text.

| Tool | Arguments | Effect | Result text |
|---|---|---|---|
| `schedule_follow_up` | `at` (RFC 3339 with offset) or `in_seconds`, exactly one; `note`; `new_thread` (bool, default false) | `Write` | The new timer id and the due time in UTC |
| `create_schedule` | `cron`, `tz`, `note` | `Write` | The new timer id and the next three slots, in the zone and in UTC |
| `list_timers` | none | `ReadOnly` (`ToolEffect::ReadOnly`) | The active `agent` timers of the inbox: stored, plus buffered inserts, minus buffered cancels |
| `cancel_timer` | `timer_id` | `Write` | Confirmation, or a refusal |

- A follow-up gets the run's thread. With `new_thread: true` it gets no thread. A schedule gets
  no thread, so each fire follows the ADR (f) rule for an event with no thread.
- **The thread TTL (S7).** The claim starts a fresh session when the thread's `last_run_at` is
  older than `claim.thread_ttl_secs` (default 86400 s, SMA-787 §8.4 step 6). So with the
  defaults, a follow-up for "tomorrow" wakes the agent with no conversation history, only the
  note. Sven accepted this. The `schedule_follow_up` description and its result text tell the
  model that the conversation can be gone when the timer fires, and that the note must hold the
  context it needs. When the due time is later than `claim.thread_ttl_secs` from now, the result
  text says so directly. An operator who wants longer threads raises `claim.thread_ttl_secs`.
  ADR (f) does not change.
- `list_timers` shows the due time (`next_fire_at`). After a back-off, this is the retry time.
  The `fireAt` of the fire event is the scheduled slot (`scheduled_at`), not the retry time.
- `list_timers` shows only `agent` timers. `system` timers are host business.
- `cancel_timer` gives the same refusal text for an unknown id, an id of another inbox and a
  `system` timer (AG4). It refuses a `done`, `cancelled` or `failed` timer with a clear text.
- A cancel of a timer that this run inserted removes it from the buffer.
- Tool-call metrics and warning logs follow the memory tool. A log never holds the note.

### 10.3 Validation and caps

The tool refuses, with a text that names the limit and the value:

- a follow-up due time that is not in the future (by the workspace clock), sooner than
  `min_follow_up_delay_secs`, or later than `max_horizon_days`;
- an invalid cron text, a refused syntax element (§6.2), or an unknown IANA zone;
- a schedule whose smallest **civil** gap between two consecutive civil slots is below
  `min_schedule_interval_secs`. The walk measures gaps on the civil slot sequence, before the
  DST resolution, so a DST change does not make a normal schedule fail. The actual gap between
  two fires can differ from the civil gap by up to one DST step. The walk covers the next 366
  days and stops at the first gap below the minimum, so an every-minute pattern fails at once.
  Measured in a debug build (Task 6 and Task 16): `create_schedule("* * * * *", "UTC")` at a
  minimum of 60 s took 1.24 s before the early exit and about 150 microseconds after it. The
  early exit is at the five-field floor of 60 s: a minimum of 60 s or less passes with no walk.
  For a larger minimum the walk runs until the first gap below the minimum, or until the window
  ends;
- a schedule with no slot in the next `max_horizon_days`. The range of `max_horizon_days` ends at
  366, the window of the civil gap check. So the check always sees every slot that can be the
  first slot. A longer horizon would leave a gap: a slot after day 366 (for example `0 0,12 29 2 *`
  with the next 29 February in two years) would be accepted with no gap check. The cap closes
  this (D2). A test pins the refusal;
- a create that makes the active `agent` timers more than `max_active_per_inbox`. The count is a
  fresh read of `active_agent_ids`, minus the ids of buffered cancels that are in that set, plus
  the buffered inserts. A buffered cancel of a timer that is no longer active is not subtracted;
- a create over the creation budget (§9.2);
- a note over 1000 characters, or with a NUL.

## 11. The scheduler

### 11.1 One pass

`PgScheduler` implements `Scheduler::fire_due(limit)`. A pass is a sequence of short
transactions, **one per due row**, up to `limit` rows. One row cannot affect another, and no
savepoints are needed.

For each row, one transaction:

1. `SET LOCAL statement_timeout = <statement_timeout_ms>`, then `SELECT now()` gives `now`.
2. Select one due row:

   ```sql
   SELECT t.…, i.org_id
     FROM timer t JOIN inbox i ON i.id = t.inbox_id
    WHERE t.status = 'active' AND t.next_fire_at <= now()
    ORDER BY t.next_fire_at
    LIMIT 1
      FOR UPDATE OF t SKIP LOCKED
   ```

   The join gives the org for `InboxId`. `OF t` locks only the timer row. A lock on `inbox` would
   block ingress (AG7). No row: the pass ends.
3. Build the `Timer` (its zone through the `TzResolver`) and call `plan_fire(timer, now, bound)`
   inside `catch_unwind`. The release profile must keep `panic = "unwind"`; the builder checks it.
4. Insert the inbox event with the transactional insert (§7.2). `Inserted` and `Duplicate` both
   count as success. A `Duplicate` adds to a metric. A `Duplicate` with `payload_matches = false`
   also logs a warning, as `ingest()` does. The payload holds no per-attempt field (§11.2), so a
   real double fire has matching payloads.
5. Update the row: `last_fired_at = now`, `fire_count + 1`, `fire_attempts = 0`,
   `updated_at = now`, and either `status = 'done'` or the new `next_slot`. On Advance,
   `scheduled_at` and `next_fire_at` both become the new slot instant.
6. Commit.

**Errors.** Every error path rolls the transaction back explicitly. Then:

- **Permanent-looking errors.** A row that cannot be read (for example an unknown zone or pattern
  after a rolling deploy to an older replica), a `PlanError`, a panic, or a database error of
  SQLSTATE class 22 (data exception) or 23 (integrity constraint). In a new short transaction,
  the row gets `fire_attempts + 1` and `next_fire_at = now + backoff` (exponential from the poll
  interval, at most 1 hour). The back-off moves only the due time: `scheduled_at` does not
  change. When `fire_attempts` reaches `max_fire_attempts`, the row gets
  `status = 'failed'` and `failure = <reason>` instead. The reason has at most 500 characters,
  no control characters, and never the note. A metric is counted and a warning is logged with the
  timer id. The pass goes on with the next row (AG6).
  The back-off UPDATE matches the row only while `status = 'active'` and `fire_attempts`,
  `fire_count` and `next_fire_at` still have the values that the fire read. Another replica can
  fire the row between the read and the back-off. That fire resets `fire_attempts` to 0 and moves
  `next_fire_at`, so a stale back-off must not match it. No row matched means that another worker
  handled the row, and `fire_due` still counts it as processed.
- **Transient errors.** Every other database error (for example a statement timeout, a deadlock
  `40P01` or a lost connection). The row stays as it is. The pass stops and the loop counts the
  error (AG8). The next pass retries.
- If the backoff update itself fails, the row stays as it is and the pass stops. The backoff
  moves `next_fire_at`, so one bad row does not stay first in every later pass.

`fire_due` returns the number of rows it processed, backed-off and failed rows included.

The scheduler does not check whether the timer's thread is closed or has a successor. The
worker applies the thread rule of the ADR (line 292) to the event, as for every other event.

### 11.2 The event

The event is a full CloudEvents structured envelope, built with `paigasus-cloudevents`:

- `id` = `ce_id` = `timer:<timer_id>:<fire_at>`. `CloudEvent::new` takes only a UUID, so this
  branch adds a constructor that takes a text id and runs the same `check_text("id", …,
  ID_MAX_CHARS)` (§15). The id has 63 characters, under the limit of 256.
- `source` depends on the origin: `urn:paigasus:agents:timer:agent` or
  `urn:paigasus:agents:timer:system`. Only the scheduler writes these values (C3). They are
  separate from the `urn:paigasus:agents:http:` and `urn:paigasus:agents:nats:` prefixes that
  ingress and the NATS config enforce. So SMA-795 can trust an expiry event on `source` alone.
- `type` = `agents.timer.fired`. The prefix `agents.` is reserved for host events, and HTTP
  ingress refuses it (`RESERVED_TYPE_PREFIX`, SMA-787). This follows the IAM pattern (no version
  suffix).
- `time` = the transaction's `now`. It is the actual fire time.
- `threadid` = the timer's thread, if it has one. The SMA-787 insert drops a thread that is not
  in the inbox, and the event then starts a fresh thread.
- `datacontenttype` = `application/json`. `schemaversion` = `1`, as an integer extension. The
  envelope has no `actorprn` and no `subject`.
- `data` = the proto3 JSON of a new `agents/v1` message (C9):

  ```proto
  // Direction: host to agent (inbox). CloudEvents type: agents.timer.fired.
  message TimerFired {
    string timer_id = 1;
    TimerKind kind = 2;        // FOLLOW_UP or SCHEDULE
    TimerOrigin origin = 3;    // AGENT or SYSTEM
    google.protobuf.Timestamp fire_at = 4;
    string note = 5;
    optional string cron = 6;
    optional string tz = 7;
  }
  ```

  The JSON names are `timerId`, `fireAt` and so on. There is no `firedAt`: the envelope `time`
  holds it (rule 4). The builder follows the enum rules of `agents.proto`. The Rust side checks `data` against the
  `TimerFired` descriptor in `FILE_DESCRIPTOR_SET`.
- The inbox event `id` is a new UUID v7 (the existing id adapter).
- Tests pin both `source` strings, the `type`, the `id` text, and parse the built event back
  into `TimerFired`.

### 11.3 The loop

The loop follows the metrics upkeep task in `main.rs`: a `tokio::time::interval` of
`poll_interval_secs`, with `MissedTickBehavior::Delay`, and a `select!` on the shutdown `watch`
channel.

- When a pass processes exactly `pass_limit` rows, the loop starts the next pass at once, until a
  pass processes fewer. It reads the shutdown channel before each row, so a drain does not delay
  a shutdown.
- An error is logged and counted, and the loop waits for the next tick.
- The loop task must not end on an error. A panic in the pure code is caught per row (§11.1).
  `main.rs` treats an ended task as fatal, so an ended loop stops the process.
- The loop is spawned in the `JoinSet` after boot, so the existing drain stops it.

Several replicas can run the loop. `SKIP LOCKED` keeps them off the same rows.

### 11.4 Wiring in this branch

- `main.rs` and `boot.rs` build the `TzResolver` and `PgScheduler`, and spawn the
  loop when `[timers] enabled` is true. `PgTimerStore` has no caller in the binary before SMA-790,
  so `main.rs` does not build it (an unused value is a hard error).
- The four tools and `TimerWorkspace` exist and have tests, but nothing registers them for a run.
  No run exists until SMA-790. SMA-790 builds the workspace per run, registers the tools and
  calls `apply_in` in its commit. This is the same state as the memory tool after SMA-789.
- `tests/boot_pg.rs` proves the wiring: a boot with a due row and `poll_interval_secs = 1` gives a
  pending event; a boot with `enabled = false` gives no event.

## 12. Claude's calls (not decided by Sven)

1. The `origin` column, and one `source` per origin.
2. A follow-up wakes on the run's thread by default. A schedule has no thread.
3. The `note` field, at most 1000 characters, carried in the fire event, never logged.
4. The default values in §9.1 and §9.2. Sven agreed to caps and a budget, not to these numbers.
5. A missed-slot recovery fires once, with the first missed slot as `fire_at`.
6. One short transaction per due row, with no savepoints.
7. Error classes: permanent-looking errors back off and then fail; transient errors leave the row
   as it is and stop the pass.
8. No lock on the `inbox` row, in `apply_in` and in the due query (`OF t`).
9. A pass that is full starts the next pass at once.
10. No new error codes: the tools answer the model with text, and there is no HTTP route.
11. `fire_at` in RFC 3339 with whole seconds, rounded up in the `Timer` constructor.
12. The civil-time model of §6.2, with the civil slot stored next to the instant. A gap slot fires
    at the end of the gap (03:00), not one gap length later (03:30).
13. If `croner` fails the first task, the builder stops and reports.
14. A cancel that finds the timer already fired is logged and counted. The agent is not told.
15. `fire_due` counts every processed row, so the drain does not stop early.
16. The tools are built and tested, but nothing registers them until SMA-790 (§11.4).
17. The bundled tzdb through an explicit handle and a `TzResolver` port (§5.1).
18. The M0 `insert` and `cancel` port methods are removed (§7.3).
19. The commit contract of §8: rollback for cap, budget and bug errors, as SMA-789 does.
20. Timer changes do not commit with a `failed` run outcome (§8), unlike memory changes.
21. The cron syntax rules, including the refusal of patterns that restrict both day fields.
22. The gap check in civil time (§10.3).
23. A `failed` timer is not reported to the agent or the principal in v1 (NG7).
24. The `TimerFired` proto message is owned by this branch.

## 13. Observability

New metric names in `paigasus-observability/src/names.rs` (and its drift list), described in
`describe_agent_host_metrics()`:

- `agent_host_timer_fired_total{kind, origin, outcome}`, where `outcome` is `inserted` or
  `duplicate`.
- `agent_host_timer_fire_errors_total{class}`, where `class` is `permanent` or `transient`.
- `agent_host_timer_failed_total{kind, origin}`: rows set to `failed`.
- `agent_host_timer_cancel_not_active_total`: buffered cancels that found the timer not active.
- `agent_host_scheduler_pass_duration_seconds`: a histogram.
- `agent_host_scheduler_lag_seconds`: a gauge, set **after** each pass: the database `now` minus
  the oldest due `next_fire_at` that is still active (0 when nothing is due). It is the remaining
  backlog, and the signal that fires happen at the set time.
- `agent_host_scheduler_last_pass_timestamp_seconds`: a gauge, the time of the last pass that
  finished. It shows a hung loop, which the lag gauge cannot show.
- `agent_host_timer_tool_calls_total{tool, outcome}`, where `outcome` is `ok` or `refused`.

## 14. Tests

### 14.1 The "done when" proof

The database clock cannot be moved, so the DST and outage proof is in the pure layer, where
`now` is a parameter. The Postgres tests prove the atomicity, the event and the restart.

- **DST, pure.** Simulate the poll loop: call `plan_fire` at each `next_fire_at` plus a small
  delta, and also with coarse steps of several hours, over a year that holds both DST changes.
  Zones: `Europe/Berlin`, `America/New_York`, `Australia/Sydney` (southern hemisphere),
  `Australia/Lord_Howe` (30-minute shift) and `America/Santiago` (change at midnight).
  - `0 9 * * *`: exactly one fire per local day, each at 09:00 local.
  - `30 2 * * *` in Berlin: one fire per day, at 03:00 local on the spring day (R1) and at the
    first 02:30 on the fall day (R2).
  - `30 1 * * *` in New York: one fire on the fall day, at the first 01:30.
  - `0 * * * *` on a fall-back day: two fires in the repeated hour, at 02:00 CEST and 02:00 CET in Berlin,
    with different event ids (R3). On a spring day: one fire at 03:00 for the gap (R4).
  - `*/15 * * * *` on a fall-back day: all eight slots of the repeated hour fire (R3).
  - `@hourly` is an interval pattern, and `@daily` is a fixed-hour pattern (R3).
  - A recovery between the two occurrences of an interval slot fires the second occurrence only
    if its instant is after `now`.
  - A recovery at the second 01:15 of a fall-back day does not fire the 01:30 slot again (R4).
- **Outage across a DST change, pure.** A daily 09:00 Berlin schedule with a stored slot three
  days before the spring change, and `now` one hour after the change. Assert one fire,
  `fire_at` = the stored slot, the event id text, and the next slot 09:00 CEST on the same local day as `now`
  (2026-03-29).
- **tzdb rule change, pure.** Two `TzResolver` fakes with different rules for one zone. The walk
  from the stored civil slot gives no extra fire.
- **Restart, Postgres.** A daily schedule whose stored slot is in the past. One pass gives exactly
  one event with the exact `ce_id`, and the row advances once. The expected `next_fire_at` is
  computed from the database `now` with an independent `jiff` call.
- **Crash between insert and commit, Postgres.** Instance A runs a pass that stops after the event
  insert and before the commit (a test hook). The test ends A's backend with
  `pg_terminate_backend`, so the row lock goes away. Instance B, as the restarted scheduler, runs
  a pass: exactly one event, with the same `ce_id` that A would have written. The row advances
  once.
- **Follow-up wakes the agent, Postgres.** Make a workspace, wait some seconds, then call
  `schedule_follow_up` with `in_seconds` at the minimum the test config allows. Seal, then
  `apply_in` in a transaction that holds the lease-row lock. Run the real loop with a 1-second
  interval. Assert from database times: the event's `received_at` is not before `next_fire_at`,
  and `received_at - next_fire_at` is less than two poll intervals. Wait for the event with a
  longer deadline. The event is pending, on the run's thread, and the timer is `done`. The binary
  gets its own nextest block, copied from `ingress_latency_pg` (`threads-required =
  'num-test-threads'`, `retries = 0`).

### 14.2 Other tests

- **`SKIP LOCKED`.** Hold one due row with `SELECT … FOR UPDATE` in a second transaction. A pass
  returns at once and fires the other due rows. The test must fail when `SKIP LOCKED` is removed;
  the builder shows this once (a mutation that compiles).
- **No ingress wait.** While a fire transaction is open, an inbox event insert for the same inbox
  finishes at once (AG7).
- **Double fire.** Reset a row's slot to one that already fired, then run a pass: the insert
  gives `Duplicate` with `payload_matches = true`, no second event, and the row advances.
- **Missed slots.** A schedule that missed N slots fires once, with the first missed slot as
  `fire_at`. A missed walk over a year of a 15-minute schedule stays fast (step 2 of §6.2).
- **Bad rows.** A row with an unknown zone backs off, then goes to `failed` after
  `max_fire_attempts`. A walk with a low `bound` overruns and is classed permanent. A row with
  `fire_count = 9223372036854775807` fails its update with `bigint out of range` (class 22). In
  each case the other due rows fire.
- **Transient errors.** A statement timeout leaves the row active and unchanged, and stops the
  pass.
- **Isolation.** A tool of inbox X cannot list, get or cancel a timer of inbox Y, and the refusal
  text is the same as for an unknown id and for a `system` timer. `apply_in` refuses an insert
  for another inbox, and an `apply_in` cancel with the id of a timer in another inbox leaves that
  row active.
- **Commit contract.** `CapExceeded` at the limit and one past it. An inbox over a lowered cap can
  commit a run that adds no timer. A savepoint rollback leaves the outer transaction usable.
  `NotActive` for a cancel after a fire. A `System` cancel does not touch an `agent` row, and the
  reverse.
- **Creation budget.** At the limit and one past it, in the tool and in `apply_in`. A cancelled
  timer still counts. A row older than 24 hours does not count. A change set with no insert is
  not refused after the budget is lowered. A schedule over the budget still fires.
- **Caps and validation.** Each limit in §10.3, at the limit and one past it. The count cap does
  not subtract a buffered cancel of a timer that is no longer active. Tests set their own
  `[timers]` values where the defaults do not fit. The civil gap check accepts `0 */2 * * *` in
  Berlin at a 7200-second minimum, and accepts `0 9 * * *` at 86400.
- **Clock.** A tool call after a wait uses the call time, not the run start. A due time with a
  fraction of a second is rounded up.
- **Cron syntax.** Each accepted and refused element of §6.2, including `JUL` and `WED` (names
  that hold `L` and `W`) and a pattern that restricts both day fields.
- **Event.** Both `source` strings, the `type`, the `id` text, and a parse of the built event into
  `TimerFired`.
- **Transactional insert.** The SMA-787 inbox tests stay green after the change of §7.2.
- **Migrations.** m0004 up and down. The two step counts of §7.1 are updated.
- **Boot.** The two `boot_pg.rs` cases of §11.4.
- **Config.** Defaults, and each range check.
- **tzdb.** The unit test of §5.1, and the image smoke check.
- New Postgres test binaries are added to the agent-host filter in `rs/.config/nextest.toml`.
  Race and timing binaries get their own block before that filter.

## 15. Changes to earlier documents and shared code

- ADR-0028 decision (l), in Notion: the fire event id becomes `timer:<timer_id>:<fire_at>`, and
  the `source` depends on the origin. If Sven chooses S7-b, also decision (f). The builder writes
  these changes in the spec only, and Sven updates the Notion ADR.
- `ports.rs`: the `TimerRepository` methods change as in §7.3, and the `Scheduler::fire_due` docs
  as in §11.1 (C5).
- `timer.rs`: the `Timer` type changes as in §6.1, and `fire_seq` is removed.
- `paigasus-cloudevents`: a constructor with a text id and the same length check (§11.2).
- `contracts/proto/paigasus/agents/v1/agents.proto`: the `TimerFired` message and its enums. The
  plan includes the codegen, `contracts:generate` drift, the Python and TypeScript bindings, and
  `buf breaking`.
- The workspace `jiff` feature `tzdb-bundle-always` (§5.1).

## 16. Changes after the reviews

From the builder check: the civil-time model (§6.2) closes a double fire on a fall-back day.

From the first adversarial challenge: the image has no time zone data (C7); the commit contract
(§8); the run clock (§10.1); the event `type` and the per-origin `source` (§11.2); `OF t` in the
due query; the civil gap check; stronger "done when" tests; the text-id constructor; and the
minor items. Sven chose the creation budget (S5).

From the second adversarial challenge (approve with changes), all folded in:

- The tzdb fix used a feature that Cargo unification turns on again (C8). Now: an explicit
  bundled handle through a `TzResolver` port, and a unit test as the required check (§5.1).
- The Postgres DST test could not cross a DST change, because of the 25-hour clamp and the real
  database clock. The DST and outage proof is now pure; Postgres proves atomicity (§14.1).
- Timer changes no longer commit with a `failed` run, because a retry would duplicate them (§8).
- The commit-time refusals now roll back the run, as SMA-789 §6.12 does (§8).
- The per-row savepoints are replaced by one short transaction per row, with SQLSTATE classes, a
  backoff and `max_fire_attempts`. A transient error no longer ends a timer (§11.1, AG8).
- `TimerFired` is a proto message with lowerCamelCase names and no `firedAt` (§11.2, C9).
- The thread TTL limit of a follow-up is now open question S7 (§10.2).
- `boot_pg.rs` proves the loop wiring (§11.4).
- Minor items: the count-cap fix for stale cancels; the budget only for change sets with an
  insert; rounding up instead of truncation; a cron pre-check, because `croner` cannot refuse
  its extensions; R5 removed; the stored civil slot against a tzdb rule change; a backoff for a
  rolling deploy; the lag gauge after the pass, a last-pass gauge and a statement timeout;
  `statement_timestamp()` for the budget bound; the lease-row lock inside `apply_in`; the port gaps;
  testable bound and crash tests; the timing-test isolation; retention for the budget window
  (NG5); the index in the SQL block; the cross-inbox cancel test.
- Decided by Sven after the PR review (D1): the row stores the scheduled instant in `scheduled_at`.
  A back-off moves only `next_fire_at`, so `fireAt` and the event id carry the scheduled slot
  (§6.1, §6.3, §7.1, §11.1).
- Decided by Sven at the second GATE 1: S6 (Vixie cron behaviour in the repeated hour, §6.2 R3)
  and S7 (a follow-up after the thread TTL uses the note, §10.2). Sven approved the spec.

## 17. Left to the builder

- The exact `croner` options and the pre-check code for the cron syntax of §6.2.
- The module layout inside the two crates.
- The production `bound` of the walk in §6.2.
- The exact backoff curve in §11.1.
- How the crash test stops a pass between the insert and the commit (§14.1).
