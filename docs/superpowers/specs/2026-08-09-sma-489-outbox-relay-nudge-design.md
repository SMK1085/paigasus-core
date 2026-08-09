# SMA-489 — Wake the outbox relay on commit

*Design, 2026-08-09. Follow-up to SMA-471 (merged as PR 112, `dc2b351`), listed in that spec's
§8 and admitted in its §1.2.*

## 1. Context

### 1.1 The gap

`OutboxRelay::run` (`rs/crates/services/paigasus-iam/src/adapters/events/relay.rs:237-250`)
sleeps first and ticks second:

```rust
loop {
    tokio::select! {
        () = tokio::time::sleep(self.poll_interval) => { self.tick_and_record(...).await; }
        () = &mut shutdown => break,
    }
}
```

So a mutation that commits just after a tick waits nearly a full `[outbox].poll_interval_secs`
— default `5` (`config.rs`, `OutboxDefaults`) — before its `DomainEvent` reaches the publisher.
p50 is ~2.5 s, p99 approaches the interval, and both degrade under backlog: a tick drains at
most `batch_size` rows, so with a deep queue the newest row may not be reached for several
intervals.

JetStream then fans that event out in sub-milliseconds. The latency requirement is currently
satisfied by the **choice of broker** and not by the **delivery path**.

### 1.2 Why lowering `poll_interval_secs` is not the fix

It trades latency for a tighter busy-loop against Postgres on every replica, and it shrinks the
retry span that `max_attempts` multiplies into outage tolerance. SMA-471 D9 deliberately raised
`max_attempts` to 60 so that `max_attempts × poll_interval_secs` ≈ 5 minutes of consecutive
publish failures survive without dead-lettering the in-flight backlog. Dropping the interval to
250 ms would collapse that span to ~15 s and quietly re-introduce the dead-lettering-on-restart
problem D9 exists to prevent.

The two knobs are coupled. This design changes neither.

### 1.3 Why the signal has to come from the database, not the process

An in-process `tokio::sync::Notify` signalled after `tx.commit()` only wakes the relay in the
**same** process. IAM is deployed multi-replica: replica B's mutation would still wait for
replica A's poll, so a share of traffic proportional to the replica count keeps the old latency
profile — and the share grows as the deployment scales, which is backwards.

Postgres `LISTEN`/`NOTIFY` fixes this at the source. A notification emitted **inside** a
transaction is buffered by the server and delivered **only if that transaction commits** — to
every listening session in the database, in any process. That property is the whole reason this
design is shaped the way it is: see D2.

### 1.4 What the relay's transaction boundary still implies

Unchanged from SMA-471 §1.3, and still load-bearing here: `OutboxRelay::tick` does the whole
batch on **one** transaction, and a tick cancelled or crashed after publishing loses the
bookkeeping but not the publish. That is why D10 refuses to race shutdown against a tick, and it
is the gap SMA-491 (per-row commit) closes separately.

## 2. Decisions

**D1 — Postgres `LISTEN`/`NOTIFY`, not an in-process `Notify`.** Cross-replica by construction
(§1.3). Cost: one direct `sqlx` dependency and a second long-lived Postgres connection per
replica. Rejected alternative — an in-process `Notify` signalled by each application service
after `tx.commit()` — is smaller but leaves the multi-replica share of traffic on the old
profile and puts the "must fire after commit, never inside" obligation on 8 separate call sites
that a future service can silently forget.

**D2 — the notification is emitted *inside* the mutation's transaction, by `PgOutbox::enqueue`.**
This looks like it violates the issue's "signal after commit, not inside the transaction"
warning. It is the opposite: that warning is about *in-process* signalling, where firing inside
the transaction wakes a tick that cannot yet see the uncommitted row. Postgres inverts the
problem — it holds the notification until commit and drops it on rollback — so emitting inside
the transaction is precisely what buys after-commit semantics, enforced by the database rather
than by convention.

Consequences, all of them good:

- `PgOutbox::enqueue` remains the only writer that changes. No application-service edits, no
  `AppState::new` signature change, no interior mutability on `SeaOrmTransaction`.
- The obligation cannot be forgotten by a future service, because a service that enqueues an
  event gets the notification for free and a service that does not, correctly, does not.
- A rolled-back mutation provably cannot nudge.

**D3 — fixed channel `iam_outbox_event`, empty payload.** The relay's tick re-queries for work
anyway, so a payload would carry nothing the tick uses. Empty is also what makes coalescing
work: Postgres collapses notifications with an identical `(channel, payload)` pair emitted within
one transaction into a single delivery, so a mutation enqueuing several events wakes the relay
once. A per-event payload would defeat that and reintroduce the burst the issue asks us to
coalesce. Fixed rather than configurable: `LISTEN` takes an identifier, notifications are scoped
to the database already, and a mistyped channel would fail silently as "no nudges".

**D4 — a failing `pg_notify` propagates and fails the mutation.** Not a swallowed warning. In
Postgres, once any statement in a transaction errors, the transaction is poisoned: every
subsequent statement fails until rollback. "Log it and carry on" is therefore not a conservative
degradation, it is a broken transaction that will fail at commit anyway with a worse error.
`enqueue` returns it like any other write failure. In practice this requires the ~8 GB async
notification queue to be full, i.e. a system already in serious trouble.

**D5 — the listener is a separate adapter and a separate task, joined to the relay by an
`Arc<tokio::sync::Notify>`.** `PgOutboxListener` owns the `sqlx` machinery and the reconnect
loop; `OutboxRelay` gains one `Arc<Notify>` parameter and stays free of `sqlx` entirely. The
seam keeps the reconnect state machine out of the drain loop and lets the relay's wakeup
behaviour be tested without Postgres (§5).

**D6 — the listener opens its own connection from `config.database_url`
(`PgListener::connect`), not a slot from SeaORM's pool (`connect_with`).** `connect_with` would
hold one pooled connection for the process lifetime, competing with request handling and with
the relay's own tick, which already holds a connection for `batch_size × publish-latency`. An
independent connection also avoids depending on type-compatibility with the exact `sqlx` version
SeaORM re-exports.

**D7 — degrade to poll-only and reconnect forever; never fatal, never wired into `/readyz`.**
A listener that cannot connect at boot logs, zeroes its gauge, and retries with capped
exponential backoff; a mid-run drop does the same. Boot never fails on it and the replica never
leaves rotation, for exactly the reason SMA-471 §8 gives for keeping NATS out of readiness: an
IAM service whose authn/authz paths do not need the broker should not be taken out of rotation
by it. The cost is stated plainly in D12 and §7.

**D8 — the poll stays, at its current interval.** It is the safety net for three cases the
notification cannot cover: a notification emitted while this replica's listener was
disconnected (Postgres does **not** queue notifications for an absent listener — they are
dropped outright), the first tick after boot, and any row left behind by a failed publish. §1.2
is why the interval itself does not move.

**D9 — after a full, clean batch, tick again immediately instead of sleeping.** A wakeup drains
at most `batch_size` (100) rows; with 500 pending, one nudge would drain 100 and then sleep 5 s,
so the latency win would evaporate exactly under the load where latency matters most — the
issue's own "degrade under backlog" complaint. The continuation condition is
`drained == batch_size && failures == 0`. The `failures == 0` half is essential, not defensive
tidiness: without it, a broken publisher plus a deep backlog becomes a hot loop re-selecting the
same failing rows every few milliseconds, hammering the broker and burning `max_attempts` in
seconds — collapsing the very retry span SMA-471 D9 was tuned to protect (§1.2).

**D10 — shutdown is checked *between* ticks, never raced *around* one.** The obvious way to make
the D9 continuation loop responsive is to `select!` the tick against `shutdown`. That cancels an
in-flight tick, rolling back a transaction whose events the publisher may already have accepted
— SMA-471 D3's unbounded-republish gap, on every graceful shutdown. Today's loop never does this
(it races only the sleep), and this design preserves that. The continuation loop instead polls
`shutdown` once, non-blockingly, between ticks:

```rust
'outer: loop {
    tokio::select! {
        () = tokio::time::sleep(self.poll_interval) => {}
        () = wake.notified()                        => {}
        () = &mut shutdown                          => break 'outer,
    }
    loop {
        let Some(report) = self.tick_and_record(publisher.as_ref()).await else { break };
        if report.drained < self.batch_size || report.failures > 0 { break }
        let mut stop = false;
        tokio::select! { biased; () = &mut shutdown => stop = true, () = std::future::ready(()) => {} }
        if stop { break 'outer }
    }
}
```

**D11 — `[outbox].wake_on_commit`, default `true`, gating only the listener task.** One bool, an
incident escape hatch that does not need a rollback, and the clean way for tests to assert the
poll-only baseline. It mirrors `relay_enabled`'s established shape: a boot-time no-spawn, not a
runtime knob on a composed service. The **writer** side is deliberately not gated —
`pg_notify` with no listener is discarded by Postgres at negligible cost, so gating it would buy
nothing and would force `PgOutbox` to stop being the stateless unit struct it is.

**D12 — three new metric families; wakeup label set primed at zero.** See §3.6. Priming matters:
a metrics-rs series first appears already at 1, so an `increase()`-based rule can never fire on
the first occurrence of an unprimed label value.

## 3. Design

### 3.1 Writer — `PgOutbox::enqueue`

After the existing insert, on the same recovered `DatabaseTransaction`:

```rust
txn.execute(Statement::from_string(
    DbBackend::Postgres,
    "SELECT pg_notify('iam_outbox_event', '')",
)).await.map_err(map_err)?;
```

`pg_notify(text, text)` rather than the `NOTIFY` utility statement: it is an ordinary function
call, so it goes through the normal prepared-statement path and takes its channel as a value
rather than requiring identifier interpolation.

`PgOutbox` stays `#[derive(Clone, Copy, Default)]` and stateless. Nothing else in the write path
moves.

### 3.2 Listener — `adapters/persistence/pg_outbox_listener.rs`

New adapter, sibling to `pg_outbox.rs` (the Postgres mechanism that emits the notification it
receives). Shape mirrors `PgOutboxMaintainer`/`PgPartitionMaintainer`: a struct with a `run`
method taking a shutdown future, spawned by the composition root.

```
PgOutboxListener::new(database_url: String, wake: Arc<Notify>)
    .run(shutdown) -> ()
```

Loop:

1. `PgListener::connect(&url)`; on error → gauge 0, log, backoff, retry.
2. `listener.listen("iam_outbox_event")`; on error → as above.
3. Gauge 1, and if this was a reconnect rather than the first connect, increment the reconnect
   counter.
4. `select!` `listener.recv()` against `shutdown`. Each `Ok(_)` → `wake.notify_one()`. An `Err`
   → gauge 0, log, back to step 1.

Backoff starts at 250 ms and doubles to a 30 s cap. Seeded small because the common case is a
brief blip; capped because past roughly the poll interval the notification adds little the poll
is not already covering, so there is no value in retrying aggressively forever.

`notify_one` is the coalescing primitive on this side: it stores at most one permit, so a burst
of notifications arriving while the relay is mid-tick yields exactly one extra tick, and a
notification arriving while no waiter is registered is not lost.

### 3.3 Relay — `adapters/events/relay.rs`

- `run` gains a `wake: Arc<Notify>` parameter and the loop shape in D10.
- `tick_and_record` changes its return type from `()` to `Option<TickReport>`: `Some(report)` on
  a successful tick, `None` on a DB-level error (already logged and counted there). `None` ends
  a continuation run so a broken database cannot hot-loop. The change is additive for its two
  existing test callers, which ignore the value.
- Each wakeup increments `iam_outbox_relay_wakeups_total` with the `source` that caused it.

Only two callers of `run` exist — `main.rs:256` and `tests/relay_pg.rs:296`.

### 3.4 Composition root — `main.rs`

The `Arc<Notify>` is created in the existing `if config.outbox.relay_enabled` block, passed to
`relay.run(...)`, and — when `config.outbox.wake_on_commit` — cloned into a second
`servers.spawn` for `PgOutboxListener::run`, on the same `rx.changed()` shutdown watch every
other background task already uses. When `wake_on_commit` is `false`, a `tracing::info!` records
that delivery is poll-only.

`AppState::new`'s signature is untouched (D2), so the ordering constraint at `main.rs:59-61` —
`AppState` is built before the publisher and relay — needs no rework.

### 3.5 Config — `config.rs`

```toml
[outbox]
wake_on_commit = true   # default
```

`OutboxConfig` gains the field, `OutboxDefaults` its default. `IamConfig::validate` is untouched:
a bool has no invalid value. Field docs explain the poll-only meaning of `false` and point at
D8's reasons the poll is still required either way.

### 3.6 Observability — `paigasus-observability::names`

Three families, each added to `ALL` (the drift test asserts committed dashboard/rule expressions
reference only registered families; registering without adding panels is fine and is what
SMA-471 did):

| Metric | Type | Notes |
|---|---|---|
| `iam_outbox_relay_wakeups_total{source}` | counter | `source` ∈ `notify` \| `poll` \| `backlog`. All three values primed at zero when the relay starts (D12). This is the family that proves the feature works in production: a healthy deployment shows `notify` dominating. |
| `iam_outbox_listener_connected` | gauge | 0/1. **Per-replica, and replicas do not agree** — the same caveat `IAM_NATS_CONNECTED` carries. `max by (job)` reports 1 while any single replica is still connected, hiding exactly the partial outage worth knowing about. Use `min by (job)`; never `sum`. |
| `iam_outbox_listener_reconnects_total` | counter | A climbing value means Postgres is churning the listener connection. |

No alert rules and no dashboard panels in this change (§8) — matching how SMA-471 scoped its own
panels out.

## 4. Error handling

| Failure | Behaviour |
|---|---|
| `pg_notify` fails inside a mutation's transaction | Propagates; the mutation fails (D4). Requires a full async-notify queue. |
| Listener cannot connect at boot | Log, gauge 0, backoff, retry forever. Boot succeeds; delivery is poll-only meanwhile (D7). |
| Listener connection drops mid-run | Same. Notifications emitted during the gap are lost outright — Postgres does not queue for an absent listener — and the poll covers them (D8). |
| Relay tick returns `DbErr` | Unchanged: logged, `ticks_total{result="error"}` incremented. Additionally now ends any continuation run (D9/§3.3). |
| Publisher fails during a continuation run | `failures > 0` ends the run; the loop returns to sleeping (D9). |
| Shutdown during a continuation run | Checked between ticks; the in-flight tick always completes (D10). |

## 5. Testing

**Without Docker** (`relay.rs` unit tests / `relay_pg.rs`'s Docker-less path):

1. `run` ticks on a notify: relay over `DatabaseConnection::Disconnected` with a 60 s poll
   interval; poke the `Notify`; assert `ticks_total{result="error"}` advanced within
   milliseconds. Proves the wakeup path with no Postgres, reusing the SMA-465
   disconnected-error-path technique.
2. `run` exits on shutdown while a notify permit is pending (no wedge).
3. The D9 continuation predicate — `drained == batch_size && failures == 0` — as a pure
   function over `TickReport`, including the `failures > 0` case that must **not** continue.

**With testcontainers Postgres** (`tests/`):

4. Enqueue inside an open transaction → a listening session receives **nothing** until commit;
   receives exactly one notification after commit. This is D2's whole claim. The listening
   session is a *different connection* from the writing one, which is also what makes it the
   cross-replica proof (§1.3): a separate replica is a separate session, and nothing in the
   mechanism distinguishes the two.
5. Rollback → no notification, ever.
6. Several events enqueued in one transaction → exactly one notification (D3 coalescing).
7. End-to-end: relay running with a 60 s poll interval and a live listener; a mutation committed
   through `PgOutbox` + the UoW is published in well under a second.
8. Backlog: seed `batch_size + N` unpublished rows, deliver one wakeup, assert all of them drain
   without an intervening poll interval (D9).
9. `wake_on_commit = false` → the poll-only baseline still drains, just on the interval.
10. Backlog **with a failing publisher**: seed `batch_size + N` rows, deliver one wakeup, assert
    exactly one tick runs — the continuation stops on `failures > 0` rather than hot-looping
    (D9). Distinct from scenario 3, which covers the predicate in isolation; this one proves the
    loop actually honours it.

**Config:** `wake_on_commit` defaults to `true` and round-trips through figment.

## 6. Files touched

| File | Change |
|---|---|
| `rs/crates/services/paigasus-iam/src/adapters/persistence/pg_outbox.rs` | Emit `pg_notify` on the caller's txn |
| `rs/crates/services/paigasus-iam/src/adapters/persistence/pg_outbox_listener.rs` | **New** — the listener adapter |
| `rs/crates/services/paigasus-iam/src/adapters/persistence/mod.rs` | Re-export |
| `rs/crates/services/paigasus-iam/src/adapters/events/relay.rs` | `wake` param, D10 loop, `tick_and_record` return type, wakeup counter |
| `rs/crates/services/paigasus-iam/src/config.rs` | `wake_on_commit` + default |
| `rs/crates/services/paigasus-iam/src/main.rs` | Create the `Arc<Notify>`, spawn the listener |
| `rs/crates/services/paigasus-iam/Cargo.toml` | Direct `sqlx` dep |
| `rs/Cargo.toml` | Workspace `sqlx` pin |
| `rs/crates/libs/paigasus-observability/src/names.rs` | Three families + `ALL` |
| `rs/crates/services/paigasus-iam/tests/` | Scenarios 4-9 |

## 7. Risks

**`sqlx` version skew.** The direct dependency must resolve to the same `0.8.6` `sea-orm 1.1.x`
already pulls, with matching runtime/TLS features (`runtime-tokio-rustls`, `postgres`, matching
the workspace's rustls-over-openssl posture). A mismatch silently duplicates `sqlx` in the tree,
inflating build time and the binary. Verified with `cargo tree` during implementation and by the
full `moon ci` gate run (`:deny`, `:machete`) before pushing.

**A dead listener is invisible except through its metrics.** That is the direct consequence of
D7's never-fatal posture, not an oversight: with no alert rule in this change, a replica whose
listener never reconnects silently serves the old 5 s latency profile. `iam_outbox_listener_connected`
is what makes it recoverable, and an alert on it is the obvious follow-up.

**A second Postgres connection path.** Until now every Postgres access in the service goes
through SeaORM's pool. This adds a second, `sqlx`-native one. SMA-471 D13 deferred an analogous
`repo:nats-connect-single-site` gate; the same argument applies here, and §8 records it.

## 8. Out of scope

- **Per-row commit in the relay** (SMA-491) and **`PublishError::Permanent`** (SMA-490). Both
  edit `relay.rs`; the issue explicitly asks for one at a time. SMA-491 is additionally better
  scoped *after* measuring with this landed — nudged ticks may shrink the stranded window enough
  that something cheaper than full per-row commit suffices.
- **Any change to `poll_interval_secs` / `max_attempts` defaults** (§1.2, SMA-471 D9).
- **Dashboard panels and alert rules** for the three new families.
- **A `repo:pg-connect-single-site` gate** (§7).
- **`/readyz` reporting listener health** (D7).
- **The consumer side** (SMA-492) — this issue blocks it for sequencing, not technically.

## 9. Acceptance criteria

1. A mutation committed on any replica causes the outbox row to be published in well under a
   second, against the default 5 s poll interval, with no change to that interval (§5.7 for the
   latency, §5.4 for the cross-session delivery that makes "any replica" true).
2. The notification is delivered if and only if the enqueuing transaction commits — a rolled-back
   mutation produces none (proven by §5.4 and §5.5).
3. Several events enqueued in one transaction produce exactly one wakeup (§5.6).
4. A wakeup with more than `batch_size` rows pending drains them without waiting a poll interval
   (§5.8).
5. A publisher failing during a backlog drain stops the continuation rather than hot-looping
   (§5.3 for the predicate, §5.10 for the loop honouring it).
6. The listener never fails boot and never takes a replica out of rotation; a connection loss
   degrades to poll-only and recovers on its own (§4).
7. Graceful shutdown never cancels an in-flight tick (D10).
8. `[outbox].wake_on_commit = false` reproduces today's poll-only behaviour exactly (§5.9).
9. `iam_outbox_relay_wakeups_total{source}` distinguishes notify-, poll- and backlog-driven
   ticks, with all three label values present from process start.
10. `moon ci :build :test :lint :fmt :deny :machete :typecheck :breaking :affected-smoke
    :parity-corpus-drift :wasm-getrandom-free :promtool :observability-drift :release-parity
    :release-parity-py :release-parity-ts --base origin/main --include-relations` passes.
