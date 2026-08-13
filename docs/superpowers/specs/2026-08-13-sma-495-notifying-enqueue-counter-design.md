# SMA-495 — Count notifying enqueues so the nudge-absent alert cannot false-positive on a replay

Linear: [SMA-495](https://linear.app/smaschek/issue/SMA-495/iam-count-committed-notifying-enqueues-so-the-nudge-absent-alert)
Follow-up from [SMA-489](https://linear.app/smaschek/issue/SMA-489) (merged as PR 113, `ed25c6d`).
Related: [SMA-469](https://linear.app/smaschek/issue/SMA-469) (dead-letter replay).

## 1. Context

### 1.1 The gap

`IamOutboxNotificationsAbsent` (`ops/observability/prometheus/rules/iam.rules.yml`) exists to catch
one specific silent failure: a transaction- or statement-mode connection pooler swallowing
`LISTEN`. The writer's `pg_notify` still succeeds, the relay keeps working off its
`poll_interval_secs` fallback, and nothing else in the catalog looks wrong. The alert fires on
"no notification has arrived for 30 minutes *while notifying work is happening*":

```promql
(sum by (job, instance) (increase(iam_outbox_listener_notifications_total[30m])) == 0)
  and
(sum by (job, instance) (increase(iam_outbox_relay_drained_total[30m])) > 0)
```

The right-hand term is a **stand-in** for "notifying work is happening". It is not the same thing.
`iam_outbox_relay_drained_total` counts every row the relay drains, including rows that never
produced a notification — most importantly SMA-469 dead-letter replays, whose `REPLAY_ONE_SQL`
clears `parked`/`attempts` with a direct `UPDATE` and emits no `pg_notify`. SMA-489's own design
says this in D2: *"Replayed dead letters wait for the poll."* The alert and the design disagree.

### 1.2 The failure mode

An operator replaying dead letters during a quiet period — no ordinary mutations for 30 minutes —
satisfies both terms with a perfectly healthy listener, and is told `LISTEN` is broken. Narrow, but
a maintenance window is exactly when a replay happens. It is currently mitigated by documentation
only: `docs/ops/RUNBOOK-observability.md` names an in-progress replay as cause #3, a benign case to
rule out first. Honest, but it still costs an operator a diversion during an incident.

### 1.3 What the alert actually needs

Three distinct preconditions, of which the shipped expression tests only the last two — and the
second only by proxy:

1. **A notification was emitted** in the window. Nothing in the expression tests this today;
   `drained_total` is the proxy, and §1.1 is the story of that proxy being wrong.
2. **This replica was alive and doing outbox work** for the window. `increase(drained[30m]) > 0`
   scoped `by (job, instance)` tests it, and does so well — see D4.
3. **This replica heard nothing.** The left-hand term.

The change adds a term for (1). It does **not** touch (2) or (3). That framing is what keeps the
diff small and every piece of SMA-489's reasoning intact.

## 2. Decisions

### D1 — A dedicated, unlabelled counter on the `pg_notify` path

`iam_outbox_notifying_enqueues_total`, incremented once per `PgOutbox::enqueue` call that executed
`pg_notify`.

Rejected: folding it into an `iam_outbox_enqueues_total` with a `notified="true"|"false"` label.
That also yields total enqueue volume and makes `wake_on_commit = false` visible as a `"false"`
series rather than an absent one, but the alert's control would then depend on a label filter that
reads as correct when it is missing — a dropped `{notified="true"}` silently counts non-notifying
enqueues too. Volume signals already exist (`drained`/`published`), so the extra reach is YAGNI
against a load-bearing filter.

Rejected: `iam_outbox_notify_emitted_total`. The rest of the family is framed in outbox rows
(`..._relay_drained_total`, `..._dead_letters_replayed_total`), and this counter is incremented per
row enqueued, so the row framing is the accurate one. It would in fact be a *worse* name: see D1a.

#### D1a — This counter is NOT 1:1 comparable with the listener counter

PostgreSQL collapses notifications with an identical channel **and** payload within one
transaction: *"If the same channel name is signaled multiple times with identical payload strings
within the same transaction, only one instance of the notification event is delivered."* Here the
channel is the constant `WAKE_CHANNEL` and the payload is deliberately empty (`pg_outbox.rs:67`,
`:79-83`, SMA-489 D3). So a transaction enqueuing N events increments this counter **N times** and
delivers **exactly one** notification.

That is harmless for this alert, which only asks `> 0` of the counter and `== 0` of the listener —
neither is a rate comparison. It is a trap for anyone who later builds a delivery-loss ratio from
the two, so the registered doc must state it outright rather than leave "notifying enqueues" and
"notifications" looking interchangeable. This also settles D1's second rejection: naming the counter
after the `NOTIFY` would actively assert the false equivalence.

### D2 — Counted pre-commit, and the doc says so

The outbox has no post-commit hook: `PgOutbox::enqueue` writes to a transaction it *recovers* via
`recover_txn`, never one it owns, so there is no point in this adapter that runs after `COMMIT`.
The counter therefore counts **attempted** notifying enqueues, and can only ever *over*-count
delivered notifications, never under-count.

The gap this opens is a window where every mutation rolls back: the counter climbs while nothing
commits, nothing drains, and no notification is delivered. It is closed for free by precondition (2)
of §1.3 — the `drained` term that is **already in the shipped expression** — so no new machinery is
needed to absorb it. See D3.

Rejected: threading a deferred-increment slot through the `Transaction` core port so
`SeaOrmTransaction::commit` flushes it — the shape the dead-letter replay/discard counters use. It
would make the counter exact, but it changes a core port for one alert control signal to fix a case
the existing expression already handles.

### D3 — Add one term; change nothing else

```promql
(sum by (job, instance) (increase(iam_outbox_listener_notifications_total[30m])) == 0)
  and (sum by (job, instance) (increase(iam_outbox_relay_drained_total[30m])) > 0)
  and on (job) (sum by (job) (increase(iam_outbox_notifying_enqueues_total[30m])) > 0)
```

The first two terms are **byte-identical to what ships today**. The third is new. `drained_total`
is neither removed nor re-aggregated: it stops being the evidence for "a notification was emitted"
and is left doing the job it was always actually doing — proving this replica was alive and
draining. `and` is left-associative and returns the left-hand elements with their labels intact, so
the result carries `{job, instance}` and the annotation's `{{ $labels.instance }}` still resolves.

| scenario | notifications | drained (per-instance) | enqueues (per-job) | result |
|---|---|---|---|---|
| dead-letter replay only, quiet period | flat | climbing | **0** | **silent** — the SMA-495 fix |
| every mutation rolls back for 30m | flat | **0** | climbing | silent |
| pooler eats `LISTEN` | flat | climbing | climbing | **fires** |
| one wedged replica among healthy ones | flat *on it* | climbing *on it* | climbing (job) | **fires**, names it |
| replica born mid-window, quiet since | flat | **0** | climbing (job) | silent |
| idle deployment | flat | 0 | 0 | silent |
| rules deployed ahead of the binary | flat | climbing | *absent* | silent — see D7 |

The fifth row is why `drained` stays `by (job, instance)`. A replica created mid-window gets a
**new `instance` label**, hence a fresh series whose `increase(notifications[30m])` is legitimately
0 from birth. Had the controls been job-scoped, a burst of notifying work on its *neighbours*
before it existed would license an alert against it, and `for: 15m` would elapse well inside the
~29-minute condition window — a new false page on healthy infrastructure, on every scale-up or
rolling deploy that lands in a lull. Its own `drained` being 0 is what excludes it, and that is
exactly the protection the shipped expression already provides. (A same-host restart is not
affected: Prometheus sees a counter reset on a continuous series, not a new one.)

This also preserves the shipped `for: 15m` rationale verbatim (`iam.rules.yml:57-62`), which bounds
the post-restart transient at "~2 minutes" *because* the control is per-instance. Re-aggregating it
would have silently invalidated that paragraph.

### D4 — Why the new term is `by (job)` while the other two stay `by (job, instance)`

`NOTIFY` is **broadcast** to every listening session, so on a healthy deployment every replica's
`listener_notifications_total` climbs independently. That is what makes the left-hand term correct
per instance: a single replica whose `LISTEN` is wedged has a flat counter while its neighbours
climb, and `sum by (job)` there would let the healthy replicas mask it (SMA-489's masked-replica
reasoning, unchanged).

A *notifying enqueue*, by contrast, increments on whichever replica **served the mutation**. Scoping
that term `by (job, instance)` would mean a replica taking no writes in the window — load-balancer
skew, or a low-traffic period where one replica gets everything — could never alert, however wedged
its listener is, even while draining every row late off the poll. Scoping it `by (job)` matches what
it means: *the deployment emitted a notification this window that this instance should have heard.*

`and on (job)` matches on the shared label; set operators need no `group_left` (and forbid it), and
`sum by (job)` yields exactly one right-hand series per job, so the many-to-one is unambiguous.

The two scopings are not in tension, because the terms answer different questions. "Did a
notification get emitted anywhere in this deployment?" is a job-level fact. "Was *this* replica
alive, working, and deaf?" is an instance-level one.

**Assumption:** every replica scraped under one `job` shares a Postgres instance, and therefore one
`NOTIFY` broadcast domain. If a job ever spans shards, regions, or databases, replica A's enqueues
would license an alert against replica B. This holds for every deployment shape IAM supports today;
it is recorded here because the job-scoped term is where it would break.

### D5 — Primed at zero in `main.rs`, gated on config

`counter!(names::IAM_OUTBOX_NOTIFYING_ENQUEUES_TOTAL).increment(0)` in `main.rs`, adjacent to
`describe_iam_metrics()`, gated on `config.outbox.wake_on_commit`.

*Why prime:* a metrics-rs series first appears already at its first increment's value, and
`increase()` baselines on that first sample, so an unprimed counter's *first* notifying enqueue
cannot satisfy `> 0` — the control would be blind during exactly the first window after a deploy.
This follows `iam_outbox_relay_wakeups_total` and `iam_nats_publish_duplicates_total`, which prime
for the same reason.

*Why not in `PgOutbox::new`:* `PgOutbox` is a `#[derive(Clone, Copy)]` value type with a trivial
constructor, built at five composition-root sites (`adapters/http/mod.rs:347,460,482,619,639`) and
~9 test sites. Priming there puts a process-global side effect in a value constructor, and makes
the prime depend on DI ordering rather than configuration — `main.rs` happens to call
`paigasus_observability::init` (line 47) before `AppState::new` (line 69), but `tests/metrics.rs:29-30`
builds `AppState` *before* `init`, so the prime would land on a no-op recorder there. Every sibling
prime in this codebase lives at a once-per-process lifecycle site (`pg_outbox_listener.rs:123-125`,
`NatsEventPublisher::connect`, `Generations::from_connection`); this follows them.

The gate means the series exists **iff `[outbox].wake_on_commit = true`**, which is the meaningful
statement: *this replica is configured to nudge.*

### D6 — Replay stays un-nudged

`REPLAY_ONE_SQL` is not changed to emit `pg_notify`. Replayed dead letters waiting for the poll is
SMA-489 D2's deliberate design, and this change depends on it: a replay that nudged would produce a
real notification, which is a different (also correct) way to avoid the false positive, but it is a
behaviour change to the dead-letter path and out of scope here.

### D7 — Deploy the binary before the rules

`ops/` and the IAM binary ship on different cadences. Until at least one replica per job runs the
new binary with `wake_on_commit = true`, `sum by (job) (increase(iam_outbox_notifying_enqueues_total[30m]))`
is an **empty vector**, `and on (job)` matches nothing, and the alert is structurally silent — the
same shape as the `memory`-backend trap the fixture already pins for `IamAuthzRedisCacheBypassed`
(`iam.test.yml:379-399`). A binary rollback re-arms that silence indefinitely, with no signal.

The constraint is therefore: **binary first, rules second; on rollback, roll the rule back too.**
This is accepted rather than engineered around — a fallback such as `or on (job) (older expression)`
would make the rule permanently carry the bug it is fixing. It is pinned by a fixture block
(§3.5) so the silence is a documented, tested property rather than a surprise, and named in the
RUNBOOK alongside the existing `wake_on_commit = false` structural-silence note.

### D8 — Rejected: subtract replays from drains, no new metric

```promql
(sum by (job) (increase(iam_outbox_relay_drained_total[30m]))
 - sum by (job) (increase(iam_outbox_dead_letters_replayed_total[30m]))) > 0
```

Zero code, no rules/binary coupling. Rejected as unsound rather than merely inelegant: the two
counters are incremented at different times (replays post-commit at the API call, drains when the
relay later picks the row up), so a replay near a window edge subtracts a row that has not been
drained inside it and can drive the expression negative on healthy traffic. A row that re-parks and
is replayed again double-subtracts against a single drain. And `scope="bulk"` increments by row
count while `scope="one"` increments per call, so the two are not even in the same units without a
label filter. An arithmetic identity between two counters that were never designed to be reconciled
is a worse foundation than one counter that means what the alert asks.

## 3. Design

### 3.1 `paigasus-observability::names`

Add `IAM_OUTBOX_NOTIFYING_ENQUEUES_TOTAL = "iam_outbox_notifying_enqueues_total"`, adjacent to
`IAM_OUTBOX_LISTENER_NOTIFICATIONS_TOTAL` — it is that counter's write-side twin, not a relay
metric — and add it to `ALL`. The `ALL` entry is mandatory: `tests/drift.rs` extracts every
`iam_`-prefixed token from the committed rules and asserts each resolves, so the rule change reds
`:observability-drift` without it.

Its doc comment states, in this order:

1. One increment per `PgOutbox::enqueue` that executed `pg_notify`.
2. **Not 1:1 with `iam_outbox_listener_notifications_total`** (D1a): N enqueues in one transaction
   give N increments but exactly one delivered notification, because Postgres collapses duplicate
   channel+payload within a transaction. Do not build a ratio from the pair.
3. **Pre-commit** (D2): a rolled-back mutation increments it while delivering no notification and
   draining no row. `IamOutboxNotificationsAbsent` absorbs this via its `drained` term.
4. A dead-letter replay gives **no** increment (D6) — the property the alert depends on.
5. Primed at zero iff `[outbox].wake_on_commit = true` (D5).
6. `[outbox].relay_enabled = false` with `wake_on_commit = true` is accepted config (`main.rs:38-40`
   warns about it) and is **not** gated here: such a deployment emits and primes this counter while
   running no relay and no listener. The alert stays silent there anyway, because the listener
   series is absent.

### 3.2 `adapters/persistence/pg_outbox.rs`

One increment, inside the existing `if self.notify {}` block, **after** the `pg_notify` statement's
`?` — a failed execute must not count. The comment at the site cross-references the alert and
restates the pre-commit caveat, so the increment is not silently relocated later. No change to
`PgOutbox::new` (D5).

### 3.3 `main.rs`

- A `describe_counter!` for the new name, in the outbox block alongside its siblings.
- The D5 prime, gated on `config.outbox.wake_on_commit`.
- `describe_iam_metrics`'s doc comment says "the **37** metric families" (`main.rs:436`) — becomes
  38, and the SMA-489 clause gains this counter.

These three places — `names.rs`, `describe_counter!`, the increment site — have nothing mechanically
linking them, and `drift.rs` only checks that names *resolve*. SMA-489 desynced them four separate
times; keeping them consistent is an explicit acceptance criterion.

### 3.4 `ops/observability/prometheus/rules/iam.rules.yml`

The expression from D3 — one appended conjunct. The block comment's claim that the right-hand term
proves "delivery is actually happening" is the one statement that changes: it must now say that
`drained` proves *this replica was alive and working* while the new term proves *a notification was
emitted*, and why the new one alone is job-scoped (D4). The `by (job, instance)` masked-replica
paragraph and the `for: 15m` paragraph stay **unmodified** — D3 deliberately preserves both.

The `description` annotation cites `iam_outbox_relay_drained_total` as the evidence that rows "ARE
flowing"; it is rewritten around the enqueue counter. The RUNBOOK cross-reference and the "if only
SOME replicas are alerting" triage line stay.

### 3.5 `ops/observability/prometheus/rules/tests/iam.test.yml`

Every new series carries an explicit `job` label — several existing blocks write unlabelled series
(`iam.test.yml:38-39`, `:72-73`), and an unlabelled enqueues series gives `sum by (job)` an empty
`job`, so `and on (job)` matches nothing and the block goes silent for a reason no reader will
guess.

| block | notifications | drained | enqueues | expect | what it guards |
|---|---|---|---|---|---|
| block 1, `job="iam"` (existing) | flat | climbing | **`0+5x40`** | fires at 35m | the alert still fires; `for: 15m` via the `eval_time: 10m` empty assertion |
| block 1, `job="iam-healthy"` (existing) | climbing | climbing | **`0+5x40`** | silent | the `== 0` → `>= 0` mutant. **Must climb** — see below |
| block 1, `job="iam-idle"` (existing) | flat | flat | **flat** | silent | idle silence |
| masked replica, `job="iam"` (existing) | flat on `wedged` | climbing on both | **on `healthy` only** | fires, names `wedged` | per-instance left term — and, free, D4 |
| **replay-only** (new) | flat | climbing | **flat** | silent at 35m | delete the enqueues term → fires. *This is SMA-495.* |
| **pre-deploy** (new) | flat | climbing | **series absent** | silent at 35m | pins D7's accepted silence |

Three details that are load-bearing rather than stylistic:

- **`iam-healthy`'s enqueues series must climb.** With no enqueues series for that job, a `>= 0`
  mutant makes its left-hand term true, but `sum by (job)` over an absent series is empty,
  `and on (job)` drops it, no alert is produced, and **the mutant passes**. `iam-healthy` is the
  guard the fixture comment at `iam.test.yml:91-94` calls MANDATORY; adding the series only to
  `job="iam"` would silently retire it while AC4 claimed otherwise.
- **`iam`'s enqueues series must be `0+5x40`, climbing from t=0.** The `eval_time: 10m` empty
  assertion is the only thing pinning `for: 15m`, and it discriminates only because the condition
  is true from t=1m. A series that starts climbing later destroys that guard without failing
  anything.
- **The `iam-idle` comment must be rewritten** (`iam.test.yml:96-104`). It states that "ONLY the
  right-hand `(drained[30m]) > 0` term keeps it silent", hand-verified. After the change a flat
  enqueues series keeps `iam-idle` silent too, so deleting the drained clause no longer flips it —
  the comment would claim a discriminating power it no longer has, which is precisely how the next
  reviewer gets talked into deleting a real guard. Its replacement points at the rollback-storm
  case as the drained clause's remaining justification.

There is deliberately **no rollback-storm block**. The drained term is unchanged by this work and
already carries its own coverage; the case is documented in D2/D3 as the reason it is retained.

New blocks assert at `eval_time: 35m`, matching the existing blocks (`iam.test.yml:130`) — a bare
`exp_alerts: []` at an eval time inside `for: 15m` passes vacuously and would not reproduce §4.1's
deletion proof.

### 3.6 `docs/ops/RUNBOOK-observability.md`

- **§2.2 metric table** — a row for the new counter carrying D1a, D2 and D6.
- **§4 alert table** (line 220) — the expression row, verbatim.
- **§4 alert section, line 654** — "Both terms aggregate `by (job, instance)`" is now wrong; there
  are three terms and one is job-scoped.
- **§4 cause #3** — currently documents the replay false positive and tells the operator to check
  `increase(iam_outbox_dead_letters_replayed_total[30m])` against the drained rows. That triage step
  is now **obsolete**: a replay cannot satisfy the enqueues control. Replaced by a note that the
  evidence term is now the enqueue counter, plus D2's caveat.
- **§4 `wake_on_commit = false` paragraph** (lines 680-684) — gains a second structural-silence
  reason (the enqueue counter is also never registered), and a D7 note that the alert is likewise
  silent if the rules ship ahead of the binary.

## 4. Testing

### 4.1 promtool (`repo:promtool`)

The blocks of §3.5. Each is verified by hand to flip SUCCESS → FAILED when the thing it guards is
mutated, and back on restore — the same manual proof the existing `iam-idle` comment records. The
mutation list: delete the enqueues conjunct (replay-only block must fire); change `== 0` to `>= 0`
(`iam-healthy` must fire); scope the enqueues term `by (job, instance)` (masked-replica block must
stop firing); shorten `for:` (block 1's `eval_time: 10m` must fire). A block that merely passes is
not a guard.

### 4.2 Rust — `rs/crates/services/paigasus-iam/tests/relay_nudge_pg.rs`

Docker-gated (`support::start_migrated_postgres` returns `None` and the test returns early without
a container), matching every `_pg` suite.

**These assertions must be difference-based, not absence-based.** `support::sum_metric_from`
(`tests/support/mod.rs:751-759`) sums the parsed sample lines for a family; an **absent** family
sums to `0.0` identically to one present at zero. So `assert_eq!(sum_metric_from(&out, name), 0.0)`
passes with the entire feature deleted. (This is a *second* vacuity trap on top of the `# TYPE`
one that helper's doc already records — the helper defeats the name-grep form, not this one.) Every
test below therefore establishes a nonzero baseline through a `notify = true` enqueue in the same
process, so a deleted increment fails the baseline assertion.

Each test calls `paigasus_observability::init` **before** its first enqueue. Note this buys no
isolation on its own: `init` is a `OnceLock` and its `service` argument is used only for a log line
(`paigasus-observability/src/lib.rs:22-40`), so a second call returns a clone over the same
registry. Isolation comes entirely from nextest's process-per-test (`.moon/tasks/rust.yml:22-23`);
these assertions are invalid under a bare `cargo test`, the same assumption `support/mod.rs:747-749`
already records.

1. **Wired** — recorder installed, one enqueue through `PgOutbox::new(true)`, committed; the counter
   reads exactly `1.0`. Fails at `0.0` if the increment is deleted.
2. **Gated** — extends `wake_on_commit_false_emits_no_notification` (`relay_nudge_pg.rs:530-552`),
   which installs no recorder today. Install one, enqueue **once** through `PgOutbox::new(true)`,
   then enqueue through `PgOutbox::new(false)`; the counter must still read exactly `1.0`. An
   increment that ignored the flag reads `2.0`; a deleted increment reads `0.0`. Absence proves
   nothing here, so the baseline is what makes the assertion real.
3. **Replay-inert** — recorder installed; one `notify = true` enqueue establishes the baseline and
   is drained. Then seed a parked row, `PgDeadLetters::replay_in`, commit, and run a relay tick. The
   counter must be **unchanged** across the replay while the tick's `drained` moves. Asserting both
   halves is the point: a counter that did not move because the replay did nothing would prove
   nothing.

`relay_nudge_pg.rs` has `seed_row` but no parked-row seeder; `dead_letters_pg.rs::seed_parked` is
private to that file. Test 3 adds a local parked-row helper in `relay_nudge_pg.rs`, following that
file's existing precedent of copying `seed_row` from `relay_pg.rs`.

Not tested: that a rolled-back mutation still increments the counter (D2's caveat). It is true, it
is documented, and asserting it would freeze behaviour that should be free to change if the counter
ever becomes post-commit accurate. Also not tested: D5's prime — it is a one-line `increment(0)` in
`main.rs`, which no integration test constructs; it is verified by inspection.

### 4.3 Gates

Per-project Moon tasks do not run the repo-level gates. Before pushing:

```
moon ci :build :test :lint :fmt :deny :machete :typecheck :breaking :affected-smoke \
  :parity-corpus-drift :wasm-getrandom-free :redis-connect-single-site :promtool \
  :observability-drift :nats-permissions :release-parity :release-parity-py :release-parity-ts \
  --base origin/main --include-relations
```

`:promtool` and `:observability-drift` are the two that can red on this change specifically. No new
dependencies, so `:deny`/`:machete` need no waivers.

## 5. Files touched

| file | change |
|---|---|
| `rs/crates/libs/paigasus-observability/src/names.rs` | new const + doc (§3.1) + `ALL` entry |
| `rs/crates/services/paigasus-iam/src/adapters/persistence/pg_outbox.rs` | one increment |
| `rs/crates/services/paigasus-iam/src/main.rs` | `describe_counter!`, D5 prime, 37 → 38 in the doc comment |
| `rs/crates/services/paigasus-iam/tests/relay_nudge_pg.rs` | three assertions + a parked-row helper (§4.2) |
| `ops/observability/prometheus/rules/iam.rules.yml` | one conjunct, block comment, annotation |
| `ops/observability/prometheus/rules/tests/iam.test.yml` | 3 blocks amended, `iam-idle` comment rewritten, 2 blocks added |
| `docs/ops/RUNBOOK-observability.md` | §2.2 row; §4 table line 220; line 654; cause #3; lines 680-684 |

## 6. Risks

- **Fixture annotation drift.** The `description` annotation is rewritten and duplicated verbatim in
  every `exp_annotations` in the fixture. A mismatch fails `:promtool` loudly — a nuisance rather
  than a hazard, but the most likely cause of a red first run.
- **Three-place doc desync.** §3.3's hazard. Mitigated by discipline and AC1; nothing mechanical
  checks it.
- **Silence between deploys.** D7. Bounded by deploy ordering and pinned by a fixture block, but it
  is a real window in which this alert cannot fire.
- **`and on (job)` is new to this rules file.** No other rule here uses a set operator with an `on`
  modifier. The masked-replica block (which fails if the term is per-instance) and the replay-only
  block (which fails if the term is absent) together pin its behaviour.
- **A residual blind spot, unchanged by this work.** `drained{job,instance} == 0` silences the alert,
  and `IamOutboxRelayStalled` (`rate(iam_outbox_relay_ticks_total[10m]) == 0`, no `result` filter)
  does **not** fire for a relay that ticks while erroring on every tick. A pooler eating `LISTEN`
  concurrently with a fully-erroring relay is therefore invisible here. This is the status quo, not
  a regression — the term is unchanged — and `IamOutboxPublishFailures` covers the erroring half.
  Recorded so it is not rediscovered as new.

## 7. Out of scope

- A Grafana panel for the new counter. The issue does not ask, and no other outbox panel changes.
- Making the counter post-commit accurate (D2's rejected alternative).
- Emitting `pg_notify` from `REPLAY_ONE_SQL` (D6).
- Any change to `iam_outbox_relay_drained_total`, in the code or in how the rule aggregates it.
- Closing the erroring-relay blind spot in §6 (would need a `result`-filtered `IamOutboxRelayStalled`,
  which is its own change to a critical alert).

## 8. Acceptance criteria

1. `iam_outbox_notifying_enqueues_total` is registered in `names::ALL`, described via
   `describe_counter!` in `main.rs`, and incremented in `PgOutbox::enqueue` on the `notify` path
   after a successful `pg_notify`; the three texts agree, and all state D1a's non-comparability and
   D2's pre-commit caveat.
2. The counter is primed at zero in `main.rs` iff `[outbox].wake_on_commit = true`, and `PgOutbox`'s
   constructor is unchanged.
3. `IamOutboxNotificationsAbsent` gains exactly one conjunct. The existing two terms — including
   `drained_total` at `sum by (job, instance)` — are byte-identical to what ships today, and the
   block comment's `for: 15m` and masked-replica paragraphs are unmodified.
4. The promtool suite contains a replay-only block and a pre-deploy block; block 1's `iam-healthy`
   carries a **climbing** enqueues series and `iam`'s starts at t=0; the `iam-idle` comment no longer
   claims the drained term is the sole reason for its silence; and §4.1's four mutations each turn
   the suite red.
5. `relay_nudge_pg.rs` asserts the counter is wired, gated by `wake_on_commit`, and inert across a
   dead-letter replay that demonstrably drains — each against a nonzero in-process baseline, so no
   assertion can be satisfied by an absent metric family.
6. The RUNBOOK no longer lists a dead-letter replay as a false positive to rule out, documents the
   pre-commit caveat and D7's deploy ordering, and its "both terms aggregate `by (job, instance)`"
   line is corrected.
7. The full `moon ci` gate list of §4.3 passes.

## 9. Revision log — what the adversarial review changed

- **The controls are no longer both job-scoped.** The first draft re-aggregated `drained_total` to
  `by (job)`. That created a new false page: a replica born mid-window gets a fresh `instance`
  series with `increase(notifications[30m]) == 0` from birth, and job-scoped controls would license
  an alert against it from its neighbours' earlier traffic — on every scale-up landing in a lull. It
  also silently invalidated the shipped `for: 15m` rationale. Reverting `drained` to per-instance
  fixes both, shrinks the diff, and moots the objection that the retained conjunct adds a silence
  path (it is unchanged from today). §6 records the residual blind spot instead.
- **D1a added.** The first draft asserted N enqueues in one transaction and N notifications were
  "equally true". Postgres collapses duplicate channel+payload within a transaction, and this
  payload is always empty — so it is N to one. The registered doc now warns against the ratio.
- **The prime moved from `PgOutbox::new` to `main.rs`** (D5) — a `Copy` value constructor should not
  mutate a process-global registry, and `tests/metrics.rs:29-30` builds `AppState` before `init`, so
  the constructor-sited prime was ordering-dependent rather than config-dependent.
- **§4.2's assertions were vacuous.** `sum_metric_from` returns `0.0` for an absent family, so every
  `== 0.0` assertion passed with the feature deleted — and the test being extended installs no
  recorder at all. All three are now difference-based against an in-process baseline. §4.2's claim
  that per-test recorder *names* buy isolation was also wrong and is removed.
- **§3.5 was under-specified per job**, and the minimal reading retired the MANDATORY `iam-healthy`
  mutation guard. Series are now specified per job, with the reasons.
- **D7 and D8 added** — a deploy-ordering constraint for a silence the first draft did not mention,
  and an explicit rejection of the zero-code `drained − replayed` alternative.
