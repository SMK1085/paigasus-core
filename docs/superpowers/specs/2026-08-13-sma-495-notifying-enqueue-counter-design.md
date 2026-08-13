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

Two distinct preconditions, which `drained_total` alone conflates:

1. **A notification was emitted** in the window — otherwise there was nothing for the listener to
   hear, and silence proves nothing.
2. **Something actually committed** — otherwise a notification was *attempted* but never delivered
   (Postgres discards a buffered notification on rollback), and silence again proves nothing.

`drained_total` is decent evidence for (2) and no evidence for (1). A counter on the `pg_notify`
path is exact evidence for (1) and — because it is incremented before commit — no evidence for (2).
Neither alone is sufficient; the design uses both.

## 2. Decisions

### D1 — A dedicated, unlabelled counter on the `pg_notify` path

`iam_outbox_notifying_enqueues_total`, incremented once per `PgOutbox::enqueue` call that executed
`pg_notify`.

Rejected: folding it into an `iam_outbox_enqueues_total` with a `notified="true"|"false"` label.
That also yields total enqueue volume and makes `wake_on_commit = false` visible as a `"false"`
series rather than an absent one, but the alert's control would then depend on a label filter that
reads as correct when it is missing — a dropped `{notified="true"}` silently counts non-notifying
enqueues too. Volume signals already exist (`drained`/`published`), so the extra reach is YAGNI
against a load-bearing filter. A separate counter cannot be misread that way.

Rejected: `iam_outbox_notify_emitted_total`. More literally accurate — it counts `NOTIFY`
executions — but the rest of the family is framed in outbox rows (`..._relay_drained_total`,
`..._dead_letters_replayed_total`), and the two only differ when one transaction enqueues several
events, where "N notifying enqueues" and "N notifications" are equally true.

### D2 — Counted pre-commit, and the doc says so

The outbox has no post-commit hook: `PgOutbox::enqueue` writes to a transaction it *recovers* via
`recover_txn`, never one it owns, so there is no point in this adapter that runs after `COMMIT`.
The counter therefore counts **attempted** notifying enqueues.

Rejected: threading a deferred-increment slot through the `Transaction` core port so
`SeaOrmTransaction::commit` flushes it — the shape the dead-letter replay/discard counters use.
That would make the counter exact, but it changes a core port for one alert control signal, and D3
below removes the need: the rollback case the imprecision opens is closed by the second control
term rather than by extra machinery.

This imprecision is stated in the registered doc, not glossed. The direction of the error matters
and is worth spelling out: the counter can only **over**-count relative to delivered notifications,
never under-count.

### D3 — Two control terms, both scoped `by (job)`

```promql
(sum by (job, instance) (increase(iam_outbox_listener_notifications_total[30m])) == 0)
  and on (job) (sum by (job) (increase(iam_outbox_notifying_enqueues_total[30m])) > 0)
  and on (job) (sum by (job) (increase(iam_outbox_relay_drained_total[30m])) > 0)
```

`drained_total` is **not** removed. It stops being the evidence and becomes the corroborator for
precondition (2) of §1.3. Behaviour across the four cases that matter:

| scenario | notifications | enqueues | drained | result |
|---|---|---|---|---|
| dead-letter replay only, quiet period | flat | **0** | climbing | **silent** — the SMA-495 fix |
| every mutation rolls back for 30m | flat | climbing | **0** | **silent** — no new false positive |
| pooler eats `LISTEN` | flat | climbing | climbing | **fires** |
| one wedged replica, serves no writes | flat *on it* | climbing (job) | climbing (job) | **fires**, names it |
| idle deployment | flat | 0 | 0 | silent |

`drained{job} == 0` can only blind the alert if the relay is not draining at all, which
`IamOutboxRelayStalled` already pages on at critical severity — so the added conjunct is not a new
way to go quiet during the outage this alert exists for.

### D4 — Why the controls are `by (job)` and the left-hand term stays `by (job, instance)`

This is the decision that keeps SMA-489's per-replica detection intact, and it is not symmetric
with the expression it replaces.

`NOTIFY` is **broadcast** to every listening session, so on a healthy deployment every replica's
`listener_notifications_total` climbs independently. That is what makes the left-hand term correct
per instance: a single replica whose `LISTEN` is wedged has a flat counter while its neighbours
climb, and `sum by (job)` there would let the healthy replicas mask it (SMA-489's masked-replica
reasoning, unchanged).

A *notifying enqueue*, by contrast, increments on whichever replica **served the mutation**. Scoping
that term `by (job, instance)` would mean a replica taking no writes in the window — load-balancer
skew, or simply a low-traffic period where one replica gets everything — has a zero control term and
can **never** alert, however wedged its listener is. That is a detection regression against exactly
the case SMA-489 added `by (job, instance)` for.

Scoping the controls `by (job)` matches what they mean: *the deployment emitted a notification this
window that this instance should have heard.* `and on (job)` matches on the shared label; set
operators need no `group_left`, and the result carries the left-hand side's `{job, instance}`, so
the alert still names the wedged replica. Applying the same reasoning to `drained_total` removes its
own (pre-existing, milder) replica skew for free.

### D5 — Primed at zero on the notify path

`counter!(...).increment(0)` in `PgOutbox::new` when `notify == true`. A metrics-rs series first
appears already at its first increment's value, and `increase()` baselines on that first sample, so
an unprimed counter's *first* notifying enqueue cannot satisfy `> 0` — the control would be blind
during exactly the first window after a deploy. This follows `iam_outbox_relay_wakeups_total` and
`iam_nats_publish_duplicates_total`, which prime for the same reason.

`main.rs` installs the recorder (`paigasus_observability::init`, line 47) before `AppState::new`
(line 69), so the prime reaches a live recorder. `AppState::new` constructs five `PgOutbox` values;
`increment(0)` is idempotent, so the repetition is harmless. Priming only when `notify == true`
gives the series a useful meaning: *this replica is configured to nudge.*

### D6 — Replay stays un-nudged

`REPLAY_ONE_SQL` is not changed to emit `pg_notify`. Replayed dead letters waiting for the poll is
SMA-489 D2's deliberate design, and this change depends on it: a replay that nudged would produce a
real notification, which is a different (also correct) way to avoid the false positive, but it is a
behaviour change to the dead-letter path and out of scope here.

## 3. Design

### 3.1 `paigasus-observability::names`

Add `IAM_OUTBOX_NOTIFYING_ENQUEUES_TOTAL = "iam_outbox_notifying_enqueues_total"`, adjacent to
`IAM_OUTBOX_LISTENER_NOTIFICATIONS_TOTAL` — it is that counter's write-side twin, not a relay
metric — and add it to `ALL`. The `ALL` entry is mandatory: `tests/drift.rs` extracts every
`iam_`-prefixed token from the committed rules and asserts each resolves, so the rule change reds
`:observability-drift` without it.

Its doc comment states: one increment per enqueue that executed `pg_notify`; **pre-commit**, so a
rolled-back mutation increments it while delivering no notification and draining no row (D2); N
events in one transaction give N increments; a dead-letter replay gives **none**, which is the
property `IamOutboxNotificationsAbsent` depends on (D6); and primed at zero (D5).

### 3.2 `adapters/persistence/pg_outbox.rs`

Inside the existing `if self.notify {}` block, **after** the `pg_notify` statement's `?` — a failed
execute must not count. The comment at the site cross-references the alert and restates the
pre-commit caveat, so the increment is not silently moved later.

`PgOutbox::new` gains the D5 prime, gated on `notify`.

### 3.3 `main.rs`

A `describe_counter!` for the new name, in the outbox block alongside its siblings, with help text
consistent with §3.1. These three places — `names.rs`, `describe_counter!`, the increment site —
have nothing mechanically linking them, and `drift.rs` only checks that names *resolve*. SMA-489
desynced them four separate times; keeping them consistent is an explicit acceptance criterion.

### 3.4 `ops/observability/prometheus/rules/iam.rules.yml`

The expression from D3. The block comment is revised rather than appended to: the existing text
explains why the right-hand term is mandatory and why both terms are `by (job, instance)`, and both
claims change. It must now explain what each of the two controls proves, why they are `by (job)`
(D4), and why the left-hand term is not.

The `description` annotation cites `iam_outbox_relay_drained_total` as the evidence that rows "ARE
flowing"; it is rewritten around the enqueue counter. The RUNBOOK cross-reference and the
"if only SOME replicas are alerting" triage line stay.

### 3.5 `ops/observability/prometheus/rules/tests/iam.test.yml`

Both existing blocks need the new series, and each control term gets a block that turns the suite
**red if that term is deleted** — the discipline the existing `iam-idle` control already applies.

| block | notifications | enqueues | drained | expect | what it guards |
|---|---|---|---|---|---|
| `iam` / `iam-healthy` / `iam-idle` (existing) | as today | added | as today | unchanged | `== 0` vs `>= 0`; idle silence; `for: 15m` |
| masked replica, `wedged` + `healthy` (existing) | as today | **on `healthy` only** | as today | fires, names `wedged` | per-instance left-hand term — and, for free, D4: `wedged` served no writes yet must still alert |
| **replay-only** (new) | flat | **flat** | climbing | silent | delete the enqueues control → this block fires. *This is SMA-495.* |
| **rollback-storm** (new) | flat | climbing | **flat** | silent | delete the drained control → this block fires |

Putting the enqueues series on `healthy` only in the masked-replica block is deliberate: it makes
that block simultaneously the masked-replica guard and the D4 guard, with no extra series.

The `eval_time: 10m` empty assertion pinning `for: 15m` is retained. Every `exp_annotations` block
must match the rewritten `description` verbatim.

### 3.6 `docs/ops/RUNBOOK-observability.md`

Three edits:

- **§2.2 metric table** — a row for the new counter, carrying the pre-commit caveat and the
  replay-inertness.
- **§4 alert table** — the expression row, updated verbatim.
- **§4 `IamOutboxNotificationsAbsent` section** — cause #3 currently documents the replay false
  positive and tells the operator to check
  `increase(iam_outbox_dead_letters_replayed_total[30m])` against the drained rows. That triage step
  is now **obsolete**: a replay cannot satisfy the enqueues control, so the alert is structurally
  silent for it. Replaced by a short note that the evidence term is now the enqueue counter, plus
  the D2 caveat: a window in which every mutation rolls back climbs the enqueue counter, which is why
  `drained` is retained as a second control. The existing full-`pg_notify`-queue paragraph — which
  *is* a total-rollback signature — stays and now reads consistently with it.

## 4. Testing

### 4.1 promtool (`repo:promtool`)

The four blocks of §3.5. Each new block is verified by hand to flip SUCCESS → FAILED when its
corresponding control term is removed from the rule, and back on restore — the same manual proof
the existing `iam-idle` control's comment records. That proof is what distinguishes a real guard
from a block that merely happens to pass.

### 4.2 Rust — `rs/crates/services/paigasus-iam/tests/relay_nudge_pg.rs`

Docker-gated (`support::start_migrated_postgres` returns `None` and the test returns early without
a container), matching every `_pg` suite. All assertions parse the value via
`support::sum_metric_from` rather than `contains()`: `render()` always emits a `# TYPE <name>` line,
so a name-contains assertion passes with the increment deleted.

**These are exact-value assertions on a process-global registry.** `paigasus_observability::init`
installs the recorder in a `OnceLock`, so every test sharing a process shares one registry — and
D5's prime means merely *constructing* a `PgOutbox::new(true)` moves the series into existence. The
suite is run under `cargo nextest`, which executes each test in its own process, so the assertions
below are isolated. They are **not** isolated under a bare `cargo test`, where the `== 0` gated
assertion would see priming from a sibling test in the same binary. Each test therefore calls
`paigasus_observability::init` with its own recorder name and renders through its own handle, as
`a_killed_listener_backend_reconnects_and_still_delivers` already does.

1. **Wired** — a committed notifying enqueue increments the counter to `1`.
2. **Gated** — extends `wake_on_commit_false_emits_no_notification`: under `PgOutbox::new(false)`
   the counter stays `0`. This is what makes the alert's control correct when the writer is switched
   off, rather than merely silent by accident.
3. **Replay-inert** — seed a parked row, `PgDeadLetters::replay_in`, commit; the counter stays `0`
   while a relay tick reports `drained == 1`. Asserting both halves is the point: a counter that
   stayed `0` because the replay did nothing would prove nothing.

Not tested: that a rolled-back mutation still increments the counter (D2's caveat). It is true, it
is documented, and asserting it would freeze behaviour that should be free to change if the counter
ever becomes post-commit accurate.

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
| `rs/crates/libs/paigasus-observability/src/names.rs` | new const + doc + `ALL` entry |
| `rs/crates/services/paigasus-iam/src/adapters/persistence/pg_outbox.rs` | increment + prime |
| `rs/crates/services/paigasus-iam/src/main.rs` | `describe_counter!` |
| `rs/crates/services/paigasus-iam/tests/relay_nudge_pg.rs` | three assertions (§4.2) |
| `ops/observability/prometheus/rules/iam.rules.yml` | expression, block comment, annotation |
| `ops/observability/prometheus/rules/tests/iam.test.yml` | two blocks updated, two added |
| `docs/ops/RUNBOOK-observability.md` | §2.2 row, §4 table row, §4 alert section |

## 6. Risks

- **Fixture annotation drift.** The `description` annotation is rewritten and is duplicated verbatim
  in every `exp_annotations` in the fixture. A mismatch fails `:promtool` loudly, so this is a
  nuisance rather than a hazard — but it is the most likely single cause of a red first run.
- **Three-place doc desync.** §3.3's hazard. Mitigated only by discipline and an acceptance
  criterion; nothing mechanical checks it.
- **`and on (job)` is new to this rules file.** No other rule here uses a set operator with an `on`
  modifier. The masked-replica and replay-only fixture blocks together pin its behaviour, so a
  mistake in the matching semantics cannot pass silently.

## 7. Out of scope

- A Grafana panel for the new counter. The issue does not ask, and no other outbox panel changes.
- Making the counter post-commit accurate (D2's rejected alternative).
- Emitting `pg_notify` from `REPLAY_ONE_SQL` (D6).
- Any change to `iam_outbox_relay_drained_total`'s own semantics — only how the rule aggregates it.

## 8. Acceptance criteria

1. `iam_outbox_notifying_enqueues_total` is registered in `names::ALL`, described via
   `describe_counter!` in `main.rs`, and incremented in `PgOutbox::enqueue` on the `notify` path
   after a successful `pg_notify`; the three texts agree with each other.
2. The counter is primed at zero when a `PgOutbox` is constructed with `notify == true`, and — in a
   process where no `PgOutbox` was constructed with `notify == true` — is absent from `/metrics`
   entirely rather than present at zero.
3. `IamOutboxNotificationsAbsent` uses the D3 expression; the left-hand term remains
   `sum by (job, instance)` and both controls are `sum by (job)`.
4. The promtool suite contains a replay-only block and a rollback-storm block, each proven to fail
   when its control term is removed, and the pre-existing blocks still assert what they asserted
   before.
5. `relay_nudge_pg.rs` asserts the counter is wired, gated by `wake_on_commit`, and inert across a
   dead-letter replay that demonstrably drains.
6. The RUNBOOK's `IamOutboxNotificationsAbsent` section no longer lists a dead-letter replay as a
   false positive to rule out, and documents the pre-commit caveat instead.
7. The full `moon ci` gate list of §4.3 passes.
