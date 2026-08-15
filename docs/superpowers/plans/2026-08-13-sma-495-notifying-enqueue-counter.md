# SMA-495 Notifying-Enqueue Counter Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `iam_outbox_notifying_enqueues_total`, incremented on `PgOutbox::enqueue`'s `pg_notify` path, and gate `IamOutboxNotificationsAbsent` on it — so an SMA-469 dead-letter replay can no longer satisfy the alert with a perfectly healthy listener.

**Architecture:** One new counter, one new conjunct. The alert gains a third term (`and on (job) …`) that proves *a notification was emitted*; the two terms that ship today — the per-instance listener term and the per-instance `drained_total` term — are left **byte-identical**, which is what preserves SMA-489's `for: 15m` and masked-replica reasoning and keeps a mid-window-born replica from false-paging.

**Tech Stack:** Rust (edition 2024, rust-version 1.95), `metrics` / `metrics-exporter-prometheus`, SeaORM, Postgres `LISTEN`/`NOTIFY`, Prometheus + `promtool test rules`, Moon 2.3.2.

**Spec:** `docs/superpowers/specs/2026-08-13-sma-495-notifying-enqueue-counter-design.md`

## Global Constraints

- Every source file opens with `// SPDX-License-Identifier: Apache-2.0` (`#` for Python, `#` for YAML). All files touched here already have one — do not add a second.
- Conventional commits with a workspace scope: `feat(rs):`, `docs(rs):`, `test(rs):`. **Subject must start lowercase and be ≤100 chars.** Never put a bare `#NNN` in the commit body (commitlint reads it as a footer and fails `footer-leading-blank`) — write "owner/repo PR NNN".
- `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` before every `moon`/`cargo`/`promtool` command. Shims FIRST — the order is load-bearing.
- Rust: `cargo fmt` clean, `cargo clippy --all-targets -- -D warnings` clean. **Dead code is a hard compile error on the lib target**, so never add a symbol in one task and wire it in a later one.
- The three doc sites for a metric — `names.rs`, `describe_counter!` in `main.rs`, the increment site — have nothing mechanically linking them. SMA-489 desynced them four separate times. Keep them consistent within a single task.
- Postgres collapses notifications with identical channel + payload inside one transaction. The payload here is always empty, so **N enqueues in one transaction deliver exactly ONE notification**. Never write text implying the new counter is 1:1 with `iam_outbox_listener_notifications_total`.
- `support::sum_metric_from` returns `0.0` for an **absent** metric family exactly as for a zero one. **Never assert `== 0.0` as the sole proof of anything** — always establish a nonzero baseline in the same process first.
- Do not bypass the lefthook `commit-msg` hook with `--no-verify`.

---

## File Structure

| file | responsibility after this change |
|---|---|
| `rs/crates/libs/paigasus-observability/src/names.rs` | owns the metric name constant, its registered doc, and the `ALL` entry the drift gate checks |
| `rs/crates/services/paigasus-iam/src/adapters/persistence/pg_outbox.rs` | the single increment site, on the `notify` path only |
| `rs/crates/services/paigasus-iam/src/main.rs` | `describe_counter!` help text + the config-gated zero-prime |
| `rs/crates/services/paigasus-iam/tests/relay_nudge_pg.rs` | proves the counter is wired, gated by `wake_on_commit`, and inert across a replay |
| `ops/observability/prometheus/rules/iam.rules.yml` | the alert expression and the reasoning comment |
| `ops/observability/prometheus/rules/tests/iam.test.yml` | the promtool guards, one per mutation the rule must not survive |
| `docs/ops/RUNBOOK-observability.md` | operator-facing metric row, expression row, and the now-obsolete replay triage step |

Task order is **Rust first, ops second**. The rule references a metric name, so `:observability-drift` only goes green once `names.rs` carries it — Task 1 must land before Task 4.

---

### Task 1: Register the metric and increment it

**Files:**
- Modify: `rs/crates/libs/paigasus-observability/src/names.rs` (add const near `IAM_OUTBOX_LISTENER_NOTIFICATIONS_TOTAL` at line 141; add `ALL` entry near line 245)
- Modify: `rs/crates/services/paigasus-iam/src/adapters/persistence/pg_outbox.rs:89-92`
- Modify: `rs/crates/services/paigasus-iam/src/main.rs` (`describe_iam_metrics` doc at line 436; new `describe_counter!` after the `IAM_OUTBOX_LISTENER_RECONNECTS_TOTAL` block ~line 553; prime near line 52)
- Test: `rs/crates/services/paigasus-iam/tests/relay_nudge_pg.rs` (new test)

**Interfaces:**
- Consumes: nothing.
- Produces: `paigasus_observability::names::IAM_OUTBOX_NOTIFYING_ENQUEUES_TOTAL: &str = "iam_outbox_notifying_enqueues_total"`. Tasks 2, 3 and 4 all reference this exact string.

- [ ] **Step 1: Write the failing test**

Append to `rs/crates/services/paigasus-iam/tests/relay_nudge_pg.rs`, after the `wake_on_commit_false_emits_no_notification` test (it ends at line 552):

```rust
// --- SMA-495: the notifying-enqueue counter ---------------------------------------------------

/// SMA-495 AC1: an enqueue that emitted `pg_notify` is counted.
///
/// The recorder is installed BEFORE the first enqueue — `counter!` against no installed recorder
/// is a silent no-op, so an `init` after the fact would render an empty exposition and this would
/// fail for the wrong reason.
///
/// Asserted as an exact `1.0`, never as "not absent": `support::sum_metric_from` sums the parsed
/// sample lines and returns `0.0` for a family that does not exist at all, identically to one
/// present at zero — so an absence-based assertion here would pass with the increment deleted.
#[tokio::test]
async fn a_notifying_enqueue_is_counted() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let handle = paigasus_observability::init("test-iam-notifying-enqueue");

    let uow = SeaOrmUnitOfWork::new(db.clone());
    let outbox = PgOutbox::new(true);
    let tx = uow.begin().await.expect("begin");
    outbox.enqueue(&*tx, &sample_event()).await.expect("enqueue");
    tx.commit().await.expect("commit");

    assert_eq!(
        support::sum_metric_from(&handle.render(), "iam_outbox_notifying_enqueues_total"),
        1.0,
        "a committed notifying enqueue must increment iam_outbox_notifying_enqueues_total"
    );
}
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --test relay_nudge_pg a_notifying_enqueue_is_counted
```

Expected: **FAIL**, `assertion `left == right` failed: left: 0.0, right: 1.0`. (If Docker is unavailable the test returns early and reports as passed — that is a false green. Start Docker before continuing; this task cannot be verified without it.)

- [ ] **Step 3: Add the name constant and its registered doc**

In `rs/crates/libs/paigasus-observability/src/names.rs`, insert immediately **after** the `IAM_OUTBOX_LISTENER_NOTIFICATIONS_TOTAL` declaration (line 141) and before the `IAM_OUTBOX_LISTENER_CONNECTED` doc comment:

```rust
/// Enqueues that emitted a `pg_notify` — the write-side twin of
/// [`IAM_OUTBOX_LISTENER_NOTIFICATIONS_TOTAL`], and the control term
/// `IamOutboxNotificationsAbsent` gates on (SMA-495). It answers "was a nudge emitted at all in
/// this window", which `IAM_OUTBOX_RELAY_DRAINED_TOTAL` only ever approximated: a drain counts
/// every row the relay processes, including SMA-469 dead-letter replays, whose `REPLAY_ONE_SQL`
/// un-parks a row with a direct `UPDATE` and emits NO notification (SMA-489 D2). A replay during a
/// quiet period therefore used to satisfy that alert with a perfectly healthy listener.
///
/// **NOT 1:1 with [`IAM_OUTBOX_LISTENER_NOTIFICATIONS_TOTAL`] — do not build a ratio from the
/// pair.** Postgres collapses notifications carrying an identical channel AND payload within one
/// transaction, and this payload is always empty (SMA-489 D3), so a transaction enqueuing N events
/// increments this counter N times while delivering exactly ONE notification. The alert is
/// unaffected: it asks only `> 0` of this counter and `== 0` of the listener's, never a rate
/// comparison.
///
/// **Counted pre-commit.** The outbox writes on a transaction it RECOVERS rather than owns, so
/// there is no post-commit hook to count from; this counts *attempted* notifying enqueues and can
/// only ever over-count delivered notifications, never under-count. A rolled-back mutation
/// increments it while delivering no notification and draining no row —
/// `IamOutboxNotificationsAbsent` absorbs that through its separate `drained` term, which is why
/// that term is retained rather than replaced.
///
/// Primed at zero in `main.rs` iff `[outbox].wake_on_commit = true`, so the series means "this
/// replica is configured to nudge" and an `increase()` control can fire on the very first
/// enqueue. `[outbox].relay_enabled = false` does NOT gate it: that deployment emits and primes
/// this counter while running no relay and no listener, and the alert stays silent there anyway
/// because the listener series is absent.
pub const IAM_OUTBOX_NOTIFYING_ENQUEUES_TOTAL: &str = "iam_outbox_notifying_enqueues_total";
```

Then add the entry to `ALL`, immediately after `IAM_OUTBOX_LISTENER_NOTIFICATIONS_TOTAL,` (line 245):

```rust
    IAM_OUTBOX_NOTIFYING_ENQUEUES_TOTAL,
```

This `ALL` entry is **mandatory**: `paigasus-observability/tests/drift.rs` extracts every `iam_`-prefixed token from the committed rule files and asserts each resolves, so Task 4 reds `:observability-drift` without it.

- [ ] **Step 4: Add the increment**

In `rs/crates/services/paigasus-iam/src/adapters/persistence/pg_outbox.rs`, add to the imports (after the `use paigasus_iam_core::…` line):

```rust
use metrics::counter;
use paigasus_observability::names;
```

Then replace the body of the `if self.notify {` block's tail — the existing statement at lines 89-91 — so the increment follows it:

```rust
            txn.execute(Statement::from_string(DbBackend::Postgres, format!("SELECT pg_notify('{WAKE_CHANNEL}', '')")))
                .await
                .map_err(map_err)?;
            // SMA-495. AFTER the `?`, so a `pg_notify` that failed to execute is never counted.
            // This is the control term `IamOutboxNotificationsAbsent` gates on: it means "a nudge
            // was emitted", which `iam_outbox_relay_drained_total` only approximated — a drain
            // also counts SMA-469 dead-letter replays, which emit no notification at all.
            //
            // Counted PRE-COMMIT: there is no post-commit hook on a recovered transaction, so a
            // rolled-back mutation increments this while delivering nothing. That is absorbed by
            // the alert's separate `drained` term. Do NOT move this increment out of the `notify`
            // branch or below the transaction boundary without re-reading that rule.
            counter!(names::IAM_OUTBOX_NOTIFYING_ENQUEUES_TOTAL).increment(1);
```

Verify `paigasus-observability` and `metrics` are already dependencies of `paigasus-iam` (they are — `pg_audit_log.rs:130` and `pg_outbox_listener.rs:124` both use this exact pair). No `Cargo.toml` change, so no `:deny`/`:machete` waiver is needed.

- [ ] **Step 5: Add the describe and the prime**

In `rs/crates/services/paigasus-iam/src/main.rs`, update the `describe_iam_metrics` doc comment at line 436 — `the 37 metric families` becomes `the 38 metric families`, and extend the SMA-489 clause:

```rust
/// SMA-489 commit-nudge/listener families and the SMA-495 notifying-enqueue family), via the
```

Add the `describe_counter!` immediately after the `IAM_OUTBOX_LISTENER_RECONNECTS_TOTAL` block:

```rust
    describe_counter!(
        names::IAM_OUTBOX_NOTIFYING_ENQUEUES_TOTAL,
        "Enqueues that emitted a pg_notify — the write-side twin of iam_outbox_listener_notifications_total and the control IamOutboxNotificationsAbsent gates on. NOT 1:1 with it: Postgres collapses identical channel+payload notifications within a transaction, so N enqueues in one transaction give N increments but ONE notification. Counted pre-commit, so a rolled-back mutation increments it without delivering anything. A dead-letter replay increments it not at all."
    );
```

Add the prime inside the existing `if metrics_handle.is_some() {` block (line 52), which currently holds only `describe_iam_metrics();`:

```rust
    if metrics_handle.is_some() {
        describe_iam_metrics();
        // SMA-495 / SMA-489 D12 priming. A metrics-rs series first appears already at its first
        // increment's VALUE, and `increase()` baselines on that first sample — so without this an
        // `increase(...) > 0` control could never fire on a replica's first notifying enqueue,
        // blinding IamOutboxNotificationsAbsent for exactly the first window after a deploy.
        //
        // Gated on the config, NOT sited in `PgOutbox::new`: that is a `Copy` value type built at
        // five composition-root sites, and priming there would put a process-global side effect in
        // a value constructor AND make the prime depend on DI ordering rather than configuration
        // (`tests/metrics.rs` builds `AppState` before installing a recorder). Gated here, the
        // series exists iff this replica is configured to nudge.
        if config.outbox.wake_on_commit {
            metrics::counter!(names::IAM_OUTBOX_NOTIFYING_ENQUEUES_TOTAL).increment(0);
        }
    }
```

- [ ] **Step 6: Run the test to verify it passes**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --test relay_nudge_pg a_notifying_enqueue_is_counted
```

Expected: **PASS**.

- [ ] **Step 7: Verify the increment is actually load-bearing**

Comment out the `counter!(…).increment(1);` line added in Step 4, re-run the command from Step 6, and confirm it now **FAILS** with `left: 0.0, right: 1.0`. Restore the line and confirm it passes again. A test that cannot fail is not a test — and `sum_metric_from`'s `0.0`-for-absent behaviour makes this class of assertion easy to get wrong.

- [ ] **Step 8: Check the workspace still builds clean**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings
```

Expected: no output from `fmt`, no warnings from `clippy`.

- [ ] **Step 9: Commit**

```bash
git add rs/crates/libs/paigasus-observability/src/names.rs \
        rs/crates/services/paigasus-iam/src/adapters/persistence/pg_outbox.rs \
        rs/crates/services/paigasus-iam/src/main.rs \
        rs/crates/services/paigasus-iam/tests/relay_nudge_pg.rs
git commit -m "feat(rs): count outbox enqueues that emitted a commit nudge (SMA-495)"
```

---

### Task 2: Prove the counter is gated by `wake_on_commit`

**Files:**
- Modify: `rs/crates/services/paigasus-iam/tests/relay_nudge_pg.rs:531-552` (the existing `wake_on_commit_false_emits_no_notification` test)

**Interfaces:**
- Consumes: `names::IAM_OUTBOX_NOTIFYING_ENQUEUES_TOTAL` (Task 1), referenced as the literal `"iam_outbox_notifying_enqueues_total"` in the test.
- Produces: nothing.

The existing test installs **no recorder at all**, so every `counter!` in it is a silent no-op. Adding a bare `assert_eq!(…, 0.0)` there would pass with the whole feature deleted — twice over. The fix is to establish a nonzero baseline through a `notify = true` enqueue in the same process, so the assertion is a *difference*.

- [ ] **Step 1: Rewrite the test to assert by difference**

Replace the whole body of `wake_on_commit_false_emits_no_notification` (lines 531-552) with:

```rust
#[tokio::test]
async fn wake_on_commit_false_emits_no_notification() {
    let Some((pg, db)) = support::start_migrated_postgres().await else { return };
    let url = support::connection_url(&pg).await;
    let handle = paigasus_observability::init("test-iam-wake-on-commit-false");

    let mut listener = PgListener::connect(&url).await.expect("listener connects");
    listener.listen("iam_outbox_event").await.expect("listen");

    let uow = SeaOrmUnitOfWork::new(db.clone());

    // SMA-495. A BASELINE through the notifying writer, before the gated one. `sum_metric_from`
    // returns 0.0 for a family that does not exist at all, so a bare `assert_eq!(counter, 0.0)`
    // below would be satisfied by the counter never having been registered — i.e. it would pass
    // with the whole feature deleted. Counting from 1 makes the assertion prove a DIFFERENCE:
    // an increment that ignored `notify` reads 2.0, a deleted increment reads 0.0, and only the
    // correct behaviour reads 1.0.
    let notifying = PgOutbox::new(true);
    let tx = uow.begin().await.expect("begin");
    notifying.enqueue(&*tx, &sample_event()).await.expect("enqueue");
    tx.commit().await.expect("commit");
    listener.recv().await.expect("the notifying writer's own notification");

    let outbox = PgOutbox::new(false); // the escape hatch
    let tx = uow.begin().await.expect("begin");
    outbox.enqueue(&*tx, &sample_event()).await.expect("enqueue");
    tx.commit().await.expect("commit");

    assert!(
        tokio::time::timeout(Duration::from_millis(750), listener.recv()).await.is_err(),
        "wake_on_commit = false still emitted a notification — the writer is not gated"
    );

    assert_eq!(
        support::sum_metric_from(&handle.render(), "iam_outbox_notifying_enqueues_total"),
        1.0,
        "wake_on_commit = false must not increment the notifying-enqueue counter (only the \
         baseline enqueue may be counted)"
    );

    // ...and the row is still there for the poll to drain. Two rows now: the baseline and this one.
    let rows = event_outbox::Entity::find().all(&db).await.expect("query");
    assert_eq!(rows.len(), 2, "the outbox row itself must be written regardless of the flag");
}
```

Note the two deliberate changes to pre-existing assertions: the baseline enqueue consumes one notification off the listener before the timeout assertion (otherwise the timeout would see the *baseline's* notification and fail), and the row count moves from 1 to 2.

- [ ] **Step 2: Run the test to verify it passes**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --test relay_nudge_pg wake_on_commit_false_emits_no_notification
```

Expected: **PASS**.

- [ ] **Step 3: Verify the gate is load-bearing**

In `pg_outbox.rs`, temporarily move the `counter!(…).increment(1);` line **outside** the `if self.notify {}` block (to just before `Ok(())`). Re-run Step 2's command.

Expected: **FAIL** with `left: 2.0, right: 1.0` — the gated enqueue was counted. Restore the line inside the block and confirm PASS.

- [ ] **Step 4: Commit**

```bash
git add rs/crates/services/paigasus-iam/tests/relay_nudge_pg.rs
git commit -m "test(rs): prove wake_on_commit gates the notifying-enqueue counter (SMA-495)"
```

---

### Task 3: Prove a dead-letter replay does not increment the counter

**Files:**
- Modify: `rs/crates/services/paigasus-iam/tests/relay_nudge_pg.rs` (new helper + new test; extend the `use` block at lines 42-50)

**Interfaces:**
- Consumes: `names::IAM_OUTBOX_NOTIFYING_ENQUEUES_TOTAL` (Task 1); `PgDeadLetters::new(DatabaseConnection)` and `DeadLetters::replay_in(&dyn Transaction, Uuid) -> Result<Option<DeadLetterEntry>, _>` from `paigasus_iam::adapters::persistence` / `paigasus_iam_core`; `OutboxRelay::new(db, poll, batch, max_attempts).tick(&dyn EventPublisher)` returning a report with a `.drained` field.
- Produces: nothing.

This is the premise the whole alert change rests on: SMA-489 D2's *"Replayed dead letters wait for the poll"*. Asserted in code, not only as a synthetic promtool series.

- [ ] **Step 1: Extend the imports**

In `rs/crates/services/paigasus-iam/tests/relay_nudge_pg.rs`, change these two `use` lines (lines 45 and 46):

```rust
use paigasus_iam::adapters::persistence::{PgDeadLetters, PgOutbox, PgOutboxListener, SeaOrmUnitOfWork};
use paigasus_iam_core::{DeadLetters, DomainEvent, EventPublisher, EventType, Outbox, PublishError, UnitOfWork};
```

- [ ] **Step 2: Write the failing test**

Append to `rs/crates/services/paigasus-iam/tests/relay_nudge_pg.rs`, after the Task 1 test:

```rust
/// Inserts one PARKED `event_outbox` row — a dead letter awaiting an operator. Local rather than
/// shared: `dead_letters_pg.rs::seed_parked` is private to that file, and this file already sets
/// the precedent of copying a seeder in (`seed_row`, from `relay_pg.rs`).
async fn seed_parked_row(db: &DatabaseConnection, id: Uuid) -> event_outbox::Model {
    event_outbox::ActiveModel {
        id: Set(id),
        occurred_at: Set(Utc::now()),
        event_type: Set(EventType::PrincipalCreated.as_wire().to_string()),
        schema_version: Set(1),
        aggregate_prn: Set(format!("prn:pgs:iam:::principal/{id}")),
        actor_prn: Set(None),
        payload: Set(serde_json::json!({"kind": "user"}).to_string()),
        correlation_id: Set(None),
        published_at: Set(None),
        attempts: Set(5),
        parked: Set(true),
        parked_at: Set(Some(Utc::now())),
        last_error: Set(Some("backend error: transport closed".to_string())),
    }
    .insert(db)
    .await
    .expect("seed parked event_outbox row")
}

/// SMA-495: a dead-letter replay must NOT increment the notifying-enqueue counter.
///
/// This is the premise `IamOutboxNotificationsAbsent` now rests on. `REPLAY_ONE_SQL` un-parks a
/// row with a direct `UPDATE` and emits no `pg_notify` (SMA-489 D2, "replayed dead letters wait
/// for the poll"), so a replay is drainable work that produced no nudge — exactly the shape that
/// used to false-positive the alert.
///
/// BOTH halves are asserted. A counter that stayed put because the replay did nothing would prove
/// nothing, so the relay tick must be shown to actually drain the replayed row. And the counter is
/// compared against a nonzero BASELINE rather than against zero: `sum_metric_from` returns 0.0 for
/// an absent family, so `assert_eq!(counter, 0.0)` would pass with the feature deleted.
#[tokio::test]
async fn a_dead_letter_replay_is_not_a_notifying_enqueue() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let handle = paigasus_observability::init("test-iam-replay-not-notifying");

    let uow = SeaOrmUnitOfWork::new(db.clone());

    // Baseline: one real notifying enqueue, so the family exists with a known nonzero value.
    let outbox = PgOutbox::new(true);
    let tx = uow.begin().await.expect("begin");
    outbox.enqueue(&*tx, &sample_event()).await.expect("enqueue");
    tx.commit().await.expect("commit");

    let publisher = Arc::new(CountingPublisher::default());
    let relay = OutboxRelay::new(db.clone(), Duration::from_secs(60), 100, 5);
    relay.tick(publisher.as_ref()).await.expect("drain the baseline row");

    let baseline = support::sum_metric_from(&handle.render(), "iam_outbox_notifying_enqueues_total");
    assert_eq!(baseline, 1.0, "the baseline enqueue must be counted, or this test proves nothing");

    // Now the replay: park a row, return it to the live queue, and let the relay pick it up.
    let parked = Uuid::from_u128(0xdead_1e77e7);
    seed_parked_row(&db, parked).await;
    let dead = PgDeadLetters::new(db.clone());
    let tx = uow.begin().await.expect("begin");
    dead.replay_in(&*tx, parked).await.expect("replay").expect("the parked row must be returned");
    tx.commit().await.expect("commit");

    let report = relay.tick(publisher.as_ref()).await.expect("tick after replay");
    assert_eq!(report.drained, 1, "the replayed row must be visible to the relay's poll");

    assert_eq!(
        support::sum_metric_from(&handle.render(), "iam_outbox_notifying_enqueues_total"),
        baseline,
        "a dead-letter replay must not increment the notifying-enqueue counter — the whole point \
         of SMA-495 is that replayed rows are drainable work that emitted no nudge"
    );
}
```

- [ ] **Step 3: Run the test**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --test relay_nudge_pg a_dead_letter_replay_is_not_a_notifying_enqueue
```

Expected: **PASS** on the first run — Task 1's increment is already correctly scoped to the `notify` path, so this test characterises existing behaviour rather than driving new code. If it fails on `report.drained`, the seeded row is not being picked up; check that `parked` was actually cleared by `replay_in`.

- [ ] **Step 4: Verify the test can fail**

In `pg_outbox.rs`, temporarily add a second `counter!(names::IAM_OUTBOX_NOTIFYING_ENQUEUES_TOTAL).increment(1);` inside `PgDeadLetters`' replay path is *not* possible without touching another file — so instead verify the weaker but sufficient property: temporarily change the baseline assertion to `assert_eq!(baseline, 2.0, …)` and confirm the test **FAILS**. Restore it. This proves the render/parse path is live rather than silently returning zeros.

- [ ] **Step 5: Run the whole file to check nothing regressed**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --test relay_nudge_pg
```

Expected: all tests **PASS** (Task 2's rewritten test included).

- [ ] **Step 6: Commit**

```bash
git add rs/crates/services/paigasus-iam/tests/relay_nudge_pg.rs
git commit -m "test(rs): prove a dead-letter replay emits no commit nudge (SMA-495)"
```

---

### Task 4: Gate the alert on the new counter

**Files:**
- Modify: `ops/observability/prometheus/rules/iam.rules.yml:34-70` (the `IamOutboxNotificationsAbsent` comment block, expression and annotation)
- Modify: `ops/observability/prometheus/rules/tests/iam.test.yml:84-159` (both existing blocks) and append two new blocks

**Interfaces:**
- Consumes: `iam_outbox_notifying_enqueues_total` (Task 1). This task **reds `:observability-drift` if Task 1 has not landed.**
- Produces: nothing.

- [ ] **Step 1: Replace the expression and its annotation**

In `ops/observability/prometheus/rules/iam.rules.yml`, replace the single-line `expr:` and the `annotations:` line of `IamOutboxNotificationsAbsent` with:

```yaml
      - alert: IamOutboxNotificationsAbsent
        expr: (sum by (job, instance) (increase(iam_outbox_listener_notifications_total[30m])) == 0) and (sum by (job, instance) (increase(iam_outbox_relay_drained_total[30m])) > 0) and on (job) (sum by (job) (increase(iam_outbox_notifying_enqueues_total[30m])) > 0)
        for: 15m
        labels: { severity: warning }
        annotations: { summary: "IAM outbox commit nudges are not arriving (LISTEN likely blocked by a pooler)", description: "No iam_outbox_listener_notifications_total notification has arrived on {{ $labels.job }}/{{ $labels.instance }} in the last 30 minutes, even though iam_outbox_notifying_enqueues_total shows nudges being emitted across this deployment and iam_outbox_relay_drained_total shows this replica draining rows in that same window — rows ARE flowing, but never via the LISTEN/NOTIFY nudge. Delivery has silently fallen back to [outbox].poll_interval_secs (~5s), which is correct but not what this deployment is configured for. Most likely cause: a transaction- or statement-mode connection pooler in front of Postgres (PgBouncer's defaults do not support LISTEN) — the writer's pg_notify still succeeds, so nothing else looks wrong. If only SOME replicas are alerting, the pooler is not the cause — look at that instance's own listener (iam_outbox_listener_connected, iam_outbox_listener_reconnects_total). See RUNBOOK section 4." }
```

The first two conjuncts must stay **byte-identical** to what shipped. Diff them character by character before moving on — re-aggregating `drained` to `by (job)` is the specific mistake this design rejected (spec D3).

- [ ] **Step 2: Add the SMA-495 paragraph to the block comment**

In the same file, insert this paragraph into the `IamOutboxNotificationsAbsent` comment block, immediately **after** the opening paragraph (the one ending `…there is nothing to drain.`) and before the `BOTH terms aggregate` paragraph:

```yaml
      # SMA-495. There are THREE terms, and the two control terms prove different things. The
      # `drained` term proves THIS REPLICA was alive and doing outbox work — that is all it ever
      # proved, and it is scoped per-instance for that reason. It does NOT prove a nudge was
      # emitted: a drain counts every row the relay processes, including SMA-469 dead-letter
      # replays, whose `REPLAY_ONE_SQL` un-parks a row with a direct UPDATE and emits no
      # `pg_notify` at all (SMA-489 D2, "replayed dead letters wait for the poll"). An operator
      # replaying dead letters during a quiet period therefore used to satisfy this alert with a
      # perfectly healthy listener. `iam_outbox_notifying_enqueues_total` is the term that proves
      # a nudge was emitted, and it is the ONLY term aggregated `by (job)`.
      #
      # That asymmetry is deliberate. `NOTIFY` is broadcast to every listening session, so
      # "did anyone emit a nudge" is a DEPLOYMENT-level fact — but a notifying enqueue increments
      # on whichever replica served the MUTATION, so scoping it per-instance would mean a replica
      # taking no writes this window could never alert, however wedged its listener is, even while
      # draining every row late off the poll. Conversely the `drained` term must NOT become
      # `by (job)`: a replica born mid-window gets a NEW `instance` label, hence a fresh series
      # whose notification counter is legitimately flat from birth, and job-scoped controls would
      # license an alert against it from its neighbours' earlier traffic — a false page on every
      # scale-up that lands in a lull. Its own `drained` being 0 is what excludes it.
      #
      # `and on (job)` matches on the shared label; set operators take no `group_left` (and forbid
      # it), and `sum by (job)` yields exactly one right-hand series per job, so the many-to-one is
      # unambiguous. The result carries the LEFT side's labels, so `{{ $labels.instance }}` in the
      # annotation still names the wedged replica.
      #
      # This assumes every replica under one `job` shares a Postgres instance, and therefore one
      # `NOTIFY` broadcast domain. If a job ever spans shards or regions, replica A's enqueues
      # would license an alert against replica B.
      #
      # NOTE the deploy ordering this creates: until at least one replica per job runs a binary
      # emitting `iam_outbox_notifying_enqueues_total`, that term is an EMPTY vector, `and on (job)`
      # matches nothing, and this alert is structurally silent. Ship the binary before these rules,
      # and roll the rules back with the binary.
```

Leave the `BOTH terms aggregate by (job, instance)` paragraph and the `for: 15m` paragraph **unmodified** — both remain true of the two terms they describe, and the `for: 15m` reasoning depends on `drained` still being per-instance.

- [ ] **Step 3: Update the two existing fixture blocks**

In `ops/observability/prometheus/rules/tests/iam.test.yml`, add an enqueues series to each job in the first block's `input_series` (after line 123):

```yaml
      - series: 'iam_outbox_notifying_enqueues_total{job="iam",instance="a"}'
        values: '0+5x40' # nudges ARE being emitted — the control that makes the alert fire
      - series: 'iam_outbox_notifying_enqueues_total{job="iam-healthy",instance="b"}'
        values: '0+5x40' # MUST climb — see the comment above
      - series: 'iam_outbox_notifying_enqueues_total{job="iam-idle",instance="c"}'
        values: '0+0x40' # control: nothing to notify about
```

`iam-healthy`'s series **must climb**, and `iam`'s **must start at t=0** with `0+5x40`. Both are load-bearing, for reasons Step 4 records in the file.

In the masked-replica block, add **only** a `healthy` series (after line 153):

```yaml
      - series: 'iam_outbox_notifying_enqueues_total{job="iam",instance="healthy:8080"}'
        values: '0+5x40' # only the healthy replica serves writes — `wedged` must STILL alert
```

Then update both blocks' `exp_annotations` `description` strings to match Step 1's rewritten text verbatim, substituting the block's own `job`/`instance` for the `{{ $labels.* }}` templates (`iam/a` in the first block, `iam/wedged:8080` in the second).

- [ ] **Step 4: Rewrite the `iam-idle` comment and document the new series**

Replace the `iam-idle` paragraph (`iam.test.yml:96-104`) with:

```yaml
  # `iam-idle` is a SECOND, independently necessary control, added after the first review round:
  # every one of its counters is flat at zero — a deployment with nothing to notify about because
  # nothing is being written. The left-hand `== 0` term is true for it (same as the real `iam`
  # target), so ONLY the two control terms keep it silent, and it must NEVER appear in exp_alerts.
  #
  # NOTE (SMA-495): this block no longer discriminates WHICH control keeps `iam-idle` silent —
  # both its drained and its enqueues series are flat, so deleting either clause leaves it silent.
  # The `replay only` block below is what pins the enqueues clause; the drained clause's own
  # justification is the rollback case documented on the rule (a window where every mutation rolls
  # back climbs enqueues while draining nothing). Do not read this block as proof of either.
  #
  # The `iam-healthy` enqueues series MUST climb (`0+5x40`). With no series for that job, a rule
  # regressed from `== 0` to `>= 0` makes its left-hand term true, but `sum by (job)` over an
  # ABSENT series is empty, `and on (job)` drops it, no alert is produced — and the mutant passes,
  # silently retiring the guard this block exists for.
  #
  # The `iam` enqueues series MUST start climbing at t=0 (`0+5x40`). The `eval_time: 10m` empty
  # assertion below is the only thing pinning `for: 15m`, and it discriminates only because the
  # condition is true from t=1m; a series that starts later destroys that guard without failing
  # anything.
```

- [ ] **Step 5: Append the two new blocks**

Append after the masked-replica block (after line 159):

```yaml
  # IamOutboxNotificationsAbsent (SMA-495): REPLAY ONLY — the false positive this change exists to
  # remove, and the block that pins the enqueues control. An operator replays dead letters during a
  # quiet period: rows drain (SMA-469's `REPLAY_ONE_SQL` un-parks them) but NO nudge was ever
  # emitted, because replay writes a direct UPDATE with no `pg_notify` (SMA-489 D2). Before
  # SMA-495 both terms held and this paged with a perfectly healthy listener. Delete the
  # `iam_outbox_notifying_enqueues_total` conjunct from the rule and this block FIRES — that is
  # the proof it is a guard and not just a passing series.
  - interval: 1m
    input_series:
      - series: 'iam_outbox_listener_notifications_total{job="iam",instance="replay:8080"}'
        values: '0+0x40' # flat: nothing nudged, because nothing was mutated
      - series: 'iam_outbox_relay_drained_total{job="iam",instance="replay:8080"}'
        values: '0+5x40' # replayed rows ARE draining
      - series: 'iam_outbox_notifying_enqueues_total{job="iam",instance="replay:8080"}'
        values: '0+0x40' # ...but no enqueue emitted a nudge
    alert_rule_test:
      - eval_time: 35m
        alertname: IamOutboxNotificationsAbsent
        exp_alerts: []

  # IamOutboxNotificationsAbsent (SMA-495): the PRE-DEPLOY / ROLLBACK state. `ops/` and the IAM
  # binary ship on different cadences, so until a replica runs a binary emitting the new counter
  # the series does not exist at all — `sum by (job)` over it is an EMPTY vector, `and on (job)`
  # matches nothing, and this alert is structurally SILENT even though the left-hand and drained
  # terms both hold. That is accepted rather than engineered around (an `or` fallback would make
  # the rule permanently carry the bug it fixes), so it is pinned here as a KNOWN property rather
  # than left to be rediscovered as a regression. Deploy the binary first; roll the rules back
  # with it.
  - interval: 1m
    input_series:
      - series: 'iam_outbox_listener_notifications_total{job="iam",instance="predeploy:8080"}'
        values: '0+0x40'
      - series: 'iam_outbox_relay_drained_total{job="iam",instance="predeploy:8080"}'
        values: '0+5x40'
      # deliberately NO iam_outbox_notifying_enqueues_total series
    alert_rule_test:
      - eval_time: 35m
        alertname: IamOutboxNotificationsAbsent
        exp_alerts: []
```

- [ ] **Step 6: Run promtool and the drift gate**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:promtool repo:observability-drift
```

Expected: both **pass**. If `:observability-drift` fails with an unknown metric name, Task 1's `ALL` entry is missing.

- [ ] **Step 7: Prove all four guards (the step this task exists for)**

Apply each mutation to `iam.rules.yml`, run `moon run repo:promtool`, confirm the stated failure, then **restore before the next one**:

| mutation | expected failure |
|---|---|
| delete the `and on (job) (…notifying_enqueues…)` conjunct | the **replay-only** block fires |
| `== 0` → `>= 0` in the first term | **`iam-healthy`** fires |
| the enqueues term → `sum by (job, instance)` with plain `and` | the **masked-replica** block stops firing |
| `for: 15m` → `for: 0m` | block 1's **`eval_time: 10m`** assertion fires |

If any mutation leaves the suite green, that guard does not exist — fix the fixture before continuing. Record the confirmed results in the commit body.

- [ ] **Step 8: Commit**

```bash
git add ops/observability/prometheus/rules/iam.rules.yml \
        ops/observability/prometheus/rules/tests/iam.test.yml
git commit -m "feat(ops): gate the nudge-absent alert on notifying enqueues (SMA-495)"
```

---

### Task 5: Update the operator documentation

**Files:**
- Modify: `docs/ops/RUNBOOK-observability.md` — §2.2 table (after line 116), §4 table (line 220), line 654, cause #3 (~lines 668-680), the `wake_on_commit = false` paragraph (~lines 680-684)

**Interfaces:**
- Consumes: the final expression from Task 4 (copy it verbatim).
- Produces: nothing.

- [ ] **Step 1: Add the §2.2 metric-table row**

Insert immediately after the `iam_outbox_listener_notifications_total` row (line 116):

```markdown
| `iam_outbox_notifying_enqueues_total` | counter | — | SMA-495: enqueues that emitted a `pg_notify` — the write-side twin of `iam_outbox_listener_notifications_total`, and the control term `IamOutboxNotificationsAbsent` gates on. **Not 1:1 with the listener counter — do not build a delivery-loss ratio from the pair.** Postgres collapses notifications carrying an identical channel *and* payload within one transaction, and this payload is always empty, so a transaction enqueuing N events increments this N times while delivering exactly **one** notification. **Counted pre-commit**: the outbox writes on a transaction it recovers rather than owns, so there is no post-commit hook — a rolled-back mutation increments this while delivering no notification and draining no row (the alert absorbs that through its separate `drained` term, which is why that term is retained). A **dead-letter replay increments it not at all**, which is the property that makes the alert immune to a replay. Primed at zero iff `[outbox].wake_on_commit = true`, so the series existing means "this replica is configured to nudge"; `[outbox].relay_enabled = false` does not gate it. |
```

- [ ] **Step 2: Update the §4 alert-table row**

Replace line 220 with the expression exactly as committed in Task 4:

```markdown
| `IamOutboxNotificationsAbsent` | `(sum by (job, instance) (increase(iam_outbox_listener_notifications_total[30m])) == 0) and (sum by (job, instance) (increase(iam_outbox_relay_drained_total[30m])) > 0) and on (job) (sum by (job) (increase(iam_outbox_notifying_enqueues_total[30m])) > 0)` for 15m | warning |
```

- [ ] **Step 3: Correct the aggregation sentence at line 654**

Replace:

```markdown
Both terms aggregate `by (job, instance)`, so this fires **per replica**. Start by checking how
many replicas are alerting — that alone splits the two causes below.
```

with:

```markdown
There are three terms. The two that describe *this replica* — the listener term and the `drained`
term — aggregate `by (job, instance)`, so this fires **per replica**. The third,
`iam_outbox_notifying_enqueues_total`, aggregates `by (job)`: a notifying enqueue lands on whichever
replica served the mutation, so "was a nudge emitted at all" is a deployment-level question, not a
per-replica one. Start by checking how many replicas are alerting — that alone splits the two causes
below.
```

- [ ] **Step 4: Replace the obsolete cause #3**

Replace the whole of cause #3 (the paragraph beginning `3. **A dead-letter replay is draining during a quiet period**` and running to `…once ordinary mutations do.`) with:

```markdown
3. ~~A dead-letter replay draining during a quiet period.~~ **No longer possible (SMA-495).** This
   used to be this alert's one false positive: not every drained row was ever notified about, because
   a replay (SMA-469, `POST /v1/outbox/dead-letters/…/replay`) returns parked rows to the live queue
   with a direct `UPDATE` that emits **no** `pg_notify`, so replayed rows wait for the poll by
   design. The evidence term is now `iam_outbox_notifying_enqueues_total`, which a replay does not
   increment, so a replay cannot satisfy this alert however quiet the deployment is. There is
   nothing to rule out here any more.

   The counterpart caveat: that counter is incremented **pre-commit** (the outbox has no post-commit
   hook), so a window in which every mutation *rolls back* climbs it while delivering nothing. The
   `iam_outbox_relay_drained_total` term is retained precisely to absorb that — nothing commits, so
   nothing drains, and the alert stays silent. See also the full-notification-queue note below,
   which is one way to reach exactly that state.
```

- [ ] **Step 5: Extend the `wake_on_commit = false` paragraph**

Append to the paragraph beginning `` `[outbox].wake_on_commit = false` is **not** a possible cause ``, before its closing parenthetical:

```markdown
Since SMA-495 there is a second structural reason: with the flag off the writer never emits a
notification, so `iam_outbox_notifying_enqueues_total` is never registered either, and the alert's
third term is empty as well.

**One deploy-ordering caveat.** `ops/` and the IAM binary ship separately. Until at least one
replica per job runs a binary emitting `iam_outbox_notifying_enqueues_total`, that term is an empty
vector and this alert is silent — correct once the binary lands, but a blind window if the rules go
first. Deploy the binary before these rules, and roll the rules back together with it.
```

- [ ] **Step 6: Check the rendered document**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
grep -n "iam_outbox_notifying_enqueues_total" docs/ops/RUNBOOK-observability.md
grep -n "Both terms aggregate" docs/ops/RUNBOOK-observability.md
```

Expected: the first prints four or more hits (§2.2 row, §4 table row, line-654 paragraph, cause #3, `wake_on_commit` paragraph); the second prints **nothing**.

- [ ] **Step 7: Commit**

```bash
git add docs/ops/RUNBOOK-observability.md
git commit -m "docs(ops): document the notifying-enqueue counter and retire the replay caveat (SMA-495)"
```

---

### Task 6: Run the full CI gate graph

**Files:** none modified (fix-forward only if something reds).

- [ ] **Step 1: Run the whole graph the way CI does**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon ci :build :test :lint :fmt :deny :machete :typecheck :breaking :affected-smoke \
  :parity-corpus-drift :wasm-getrandom-free :redis-connect-single-site :promtool \
  :observability-drift :nats-permissions :release-parity :release-parity-py :release-parity-ts \
  --base origin/main --include-relations
```

Per-project Moon tasks do **not** run these repo-level gates — this is the only step that proves the change is CI-clean.

- [ ] **Step 2: If Moon reports an unattributed failure, find it**

Moon's summary can report "1 failed" without naming the task. Read the report directly:

```bash
jq '.actions[] | select(.status=="failed") | {target, status}' .moon/cache/ciReport.json
```

- [ ] **Step 3: Confirm the working tree is clean and the branch is ready**

```bash
git status --short
git log --oneline origin/main..HEAD
```

Expected: no modified tracked files; five commits (Tasks 1-5). `.claude/settings.json` and `.entire/` are pre-existing untracked files — leave them alone.

---

## Self-Review

**Spec coverage.** D1/D1a → Task 1 Step 3 (const doc) and Step 5 (describe text). D2 → Task 1 Steps 3-4 comments. D3 (the expression) → Task 4 Step 1, with the "byte-identical" warning. D4 (the asymmetry) → Task 4 Step 2. D5 (prime in `main.rs`, config-gated) → Task 1 Step 5. D6 (replay stays un-nudged) → nothing to implement; asserted by Task 3. D7 (deploy ordering) → Task 4 Step 5's pre-deploy block, Task 4 Step 2's comment, Task 5 Step 5. D8 → rejected alternative, no task needed. §3.1's six doc points → Task 1 Step 3. §3.5's three load-bearing fixture details → Task 4 Steps 3-4. §4.1's four mutations → Task 4 Step 7. §4.2's three tests → Tasks 1-3. §4.3 gates → Task 6. All seven ACs are covered; AC2's "primed by inspection" needs no test by design.

**Placeholder scan.** No TBD/TODO. Every code step carries the literal text to write. Task 3 Step 4 states a weaker verification than Tasks 1-2 and says so explicitly rather than gesturing at "verify appropriately".

**Type consistency.** `IAM_OUTBOX_NOTIFYING_ENQUEUES_TOTAL` and the literal `"iam_outbox_notifying_enqueues_total"` agree across Tasks 1-5. `PgOutbox::new(bool)`, `SeaOrmUnitOfWork::new(DatabaseConnection)`, `uow.begin()`, `outbox.enqueue(&*tx, &DomainEvent)`, `PgDeadLetters::new(db)`, `dead.replay_in(&*tx, Uuid) -> Result<Option<_>, _>`, `OutboxRelay::new(db, Duration, usize, u32).tick(&dyn EventPublisher) -> Result<report, _>` with `.drained`, and `support::sum_metric_from(&str, &str) -> f64` all match the signatures read from the existing tests. Task 3's `DeadLetters` trait import is required for `replay_in` to resolve and is added in its Step 1.

**One ordering constraint made explicit:** Task 4 reds `:observability-drift` unless Task 1 has landed. Tasks 2, 3 and 5 are independent of each other.
