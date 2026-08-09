# SMA-489 — Wake the outbox relay on commit

*Design, 2026-08-09. Follow-up to SMA-471 (merged as PR 112, `dc2b351`), listed in that spec's
§8 and admitted in its §1.2. Revised after adversarial review — see §10 for what changed and why.*

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
retry span that `max_attempts` multiplies into outage tolerance. SMA-471 D9 raised
`max_attempts` to 60 so that `max_attempts × poll_interval_secs` ≈ 5 minutes of consecutive
publish failures survive without dead-lettering the in-flight backlog.

**That product is not merely a tuning note — it is enforced config.** `IamConfig::validate`
(`config.rs:1019-1024`) rejects any configuration where
`duplicate_window_secs <= max_attempts × poll_interval_secs`, because a row whose last retry
falls outside JetStream's dedup window double-delivers (SMA-471 D10). Anything that makes rows
retry *faster than the poll interval* voids that check while leaving it passing — the validation
would still compute the same product, and the product would no longer describe reality.

This is the single sharpest constraint on the design. D13 exists entirely to respect it.

### 1.3 Why the signal has to come from the database, not the process

An in-process `tokio::sync::Notify` signalled after `tx.commit()` only wakes the relay in the
**same** process. IAM is deployed multi-replica: replica B's mutation would still wait for
replica A's poll, so a share of traffic proportional to the replica count keeps the old latency
profile — and the share grows as the deployment scales, which is backwards.

Postgres `LISTEN`/`NOTIFY` fixes this at the source. A notification emitted **inside** a
transaction is buffered by the server and delivered **only if that transaction commits** — to
every listening session in the database, in any process.

### 1.4 What the relay's transaction boundary still implies

Unchanged from SMA-471 §1.3 and still load-bearing: `OutboxRelay::tick` does the whole batch on
**one** transaction, and a tick cancelled or crashed after publishing loses the bookkeeping but
not the publish. That is why D10 refuses to race shutdown against a tick, and it is the gap
SMA-491 (per-row commit) closes separately.

Note also that **global FIFO is already best-effort today**, before this change: with
`FOR UPDATE SKIP LOCKED` and N replicas, replica B skips the rows replica A has locked and can
finish publishing a later batch first. SMA-471 §1.3.3 describes `ORDER BY id` as a *consequence*
of the query, not as an ordering guarantee offered to consumers. D13 relies on this.

### 1.5 Deployment assumption

`database_url` points at **direct Postgres or a session-mode pooler**. Transaction- and
statement-mode poolers (PgBouncer's defaults) do not support `LISTEN`, and the failure is
silent and total: `pg_notify` still succeeds on the writer side while the listener receives
nothing forever, with every metric in §3.6 reporting healthy. D6 and §3.6 make this both
configurable and detectable rather than assumed.

Design point: **< 10 mutations/s across 2-3 replicas.** D14's debounce default is set from this
and is a backstop, not a load-bearing throttle.

## 2. Decisions

**D1 — Postgres `LISTEN`/`NOTIFY`, not an in-process `Notify`.** Cross-replica by construction
(§1.3). Rejected alternative — an in-process `Notify` signalled by each application service after
`tx.commit()` — is smaller but leaves the multi-replica share of traffic on the old profile and
puts the "must fire after commit, never inside" obligation on 11 separate call sites that a
future service can silently forget.

**D2 — the notification is emitted *inside* the mutation's transaction, by `PgOutbox::enqueue`.**
This looks like it violates the issue's "signal after commit, not inside the transaction"
warning. It is the opposite: that warning is about *in-process* signalling, where firing inside
the transaction wakes a tick that cannot yet see the uncommitted row. Postgres inverts the
problem — it holds the notification until commit and drops it on rollback — so emitting inside
the transaction is precisely what buys after-commit semantics.

This also holds across savepoints: `Transaction::savepoint` (`uow.rs:108`) has no production call
site today, and if it gained one, a `ROLLBACK TO SAVEPOINT` would discard the row and its
notification together, because `enqueue` emits both on the same transaction handle.

Consequences:

- `PgOutbox::enqueue` remains the only writer that changes. No application-service edits, no
  `AppState::new` signature change, no interior mutability on `SeaOrmTransaction`.
- A rolled-back mutation provably cannot nudge.
- **Bounded, not universal.** The obligation cannot be forgotten *by a service that enqueues
  through `PgOutbox`*. It says nothing about rows arriving another way: a raw-SQL backfill, an
  operator `INSERT`, a future second `Outbox` implementation, or — concretely today — the
  SMA-469 dead-letter replay path (`pg_dead_letters.rs`'s `REPLAY_ONE_SQL`, which clears
  `parked`/`attempts` without notifying). **Replayed dead letters wait for the poll.** That is
  acceptable — replay is an operator action, not a latency-sensitive path — but it is not
  "cannot be forgotten", and this spec does not claim it is.
- **Not free.** It is a second statement inside the mutation's transaction: one extra round-trip
  per enqueued event, inside the lock-holding window, plus Postgres's global `NotifyQueueLock`
  taken at commit for any transaction that queued a notification. Negligible at §1.5's volume,
  but real, and the commit-path lock is a known contention point at high write rates.

**D3 — fixed channel `iam_outbox_event`, empty payload.** The relay's tick re-queries for work,
so a payload would carry nothing the tick uses. Two further reasons to keep it empty:

- **Security.** NOTIFY channels are database-wide and unprivileged: any session on the same
  database can `LISTEN` on the channel. An empty payload means eavesdropping reveals only that
  *some* IAM mutation occurred, never which principal or which event type.
- **Coalescing, stated honestly.** Postgres says it *may* collapse identical `(channel, payload)`
  notifications emitted within one transaction; this is an optimization, not a contract, and it
  is weakened across subtransaction boundaries. It is also **not load-bearing today** — all 11
  production `outbox.enqueue` call sites (`roles.rs:246,296`, `policies.rs:148,198`,
  `api_keys.rs:278,344`, `service_accounts.rs:168,223`, `create_user.rs:127`,
  `bootstrap_admin.rs:190`, `system_retirement.rs:278`) enqueue exactly one event per
  transaction. The coalescing this design actually depends on is `Notify::notify_one`'s
  single-permit semantics on the *consumer* side (D5), which is a Rust-level guarantee. Tests
  assert *at most one extra relay tick*, never "exactly one notification" (§5.6).

Fixed rather than configurable: `LISTEN` takes an identifier, notifications are already scoped to
the database, and a mistyped channel would fail silently as "no nudges". Keep the name lowercase
— sqlx emits `LISTEN "iam_outbox_event"` (quoted, case-preserving) while `pg_notify` takes the
channel as a *value*; the two agree only because the name has no uppercase.

**D4 — a full notification queue fails the mutation at COMMIT, and the writer therefore needs its
own off switch.** PostgreSQL's documented behaviour: "If this queue becomes full, transactions
calling NOTIFY will **fail at commit**." Not at the `SELECT pg_notify(...)` statement. So:

- The error surfaces from `SeaOrmTransaction::commit` (`uow.rs:104-106`) as an opaque
  `RepositoryError::Backend`, with no attribution to the notify. §4 records this.
- The documented cause is a session that executed `LISTEN` and then stopped consuming — which is
  exactly this design's listener if its TCP connection goes half-open (D15).
- **Blast radius: every IAM mutation fails at commit.** A feature whose entire purpose is shaving
  2.5 s off event latency must not be able to cause a write outage without an escape hatch, which
  is why D11 gates the *writer* as well as the listener.

*Considered and rejected:* wrapping the notify in a `SAVEPOINT` so a failure could be absorbed.
It does not work — the failure occurs at commit of the outer transaction, long after the
savepoint has been released, so there is nothing left to roll back to.

**D5 — the listener is a separate adapter and a separate task, joined to the relay by an
`Arc<tokio::sync::Notify>`.** `PgOutboxListener` owns the sqlx machinery and the liveness state
machine; `OutboxRelay` gains one `Arc<Notify>` parameter and stays free of sqlx. `notify_one`
stores at most one permit, so a burst arriving mid-tick yields exactly one extra tick and a
notification arriving with no waiter registered is not lost.

**D6 — no new dependency: use `sea_orm::sqlx`.** `sea-orm-1.1.20/src/lib.rs:519` is
`pub use sqlx;`, and the workspace already enables `sqlx-postgres` (`rs/Cargo.toml:116-118`),
which turns on `sqlx/postgres`. So `sea_orm::sqlx::postgres::PgListener` is available today at a
version that cannot skew from SeaORM's by construction. No direct `sqlx` dependency, no workspace
pin, no `:deny`/`:machete` churn.

The listener still opens its **own** connection via `PgListener::connect(url)` rather than taking
a slot from SeaORM's pool: `connect_with` would hold a pooled connection for the process
lifetime, competing with request handling and with the relay's own tick, which already holds one
for `batch_size × publish-latency`. (`PgListener::connect` builds its own internal 1-connection
pool; that is fine and is not SeaORM's.)

The URL is `[outbox].listen_database_url` when set, else `database_url` — a seam for a future
deployment that fronts Postgres with a transaction-mode pooler (§1.5) without having to move the
main connection.

**D7 — degrade to poll-only; never fail boot, never wired into `/readyz`.** A listener that
cannot connect logs, zeroes its gauge, and retries with capped backoff. Boot never fails on it
and the replica never leaves rotation, for the reason SMA-471 §8 gives for keeping NATS out of
readiness: an IAM service whose authn/authz paths do not need the broker should not be taken out
of rotation by it.

**Important asymmetry, and the reason D15 exists:** an *absent* listener is harmless (poll covers
it), but a *wedged* listener — connected as far as Postgres is concerned, not consuming — is
actively dangerous, because it is the documented way to fill the notification queue and trigger
D4's commit failures. "Degrade gracefully" is therefore not enough on its own; the listener must
also be able to detect that it has stopped receiving.

**D8 — the poll stays, at its current interval.** Safety net for three cases the notification
cannot cover: a notification emitted while this replica's listener was disconnected (Postgres
does **not** queue for an absent listener — they are dropped outright), the first tick after
boot, and rows that arrived without a nudge (D2's bounded-scope note). §1.2 is why the interval
itself does not move.

**D9 — after a full batch that made progress, tick again immediately instead of sleeping.**
A wakeup drains at most `batch_size` (100) rows; with 500 pending, one nudge would drain 100 and
then sleep 5 s, so the latency win would evaporate under exactly the load where latency matters.

The continuation predicate is `drained == batch_size && drained > failures` — **progress, not
perfection.** The naive `failures == 0` is wrong: a single malformed row (the case
`row_to_domain_event`, `relay.rs:95-109`, exists to handle) sits at a fixed FIFO position and
reappears in every batch until it parks 60 attempts later, so `failures > 0` would be true on
every tick and the continuation would be dead exactly when a deep backlog most needs draining.
`drained > failures` still stops dead when *every* publish fails — the hot-loop that would
otherwise hammer the broker and burn `max_attempts` in seconds — while surviving a poison row.

**D10 — shutdown is checked *between* ticks, never raced *around* one.** The obvious way to make
the continuation loop responsive is to `select!` the tick against `shutdown`. That cancels an
in-flight tick, rolling back a transaction whose events the publisher may already have accepted —
SMA-471 D3's unbounded-republish gap, on every graceful shutdown. Today's loop never does this
(it races only the sleep) and this design preserves that.

**The poll deadline is absolute, not a per-iteration sleep.** A `sleep(self.poll_interval)`
constructed *inside* the `select!` restarts on every outer iteration — including every nudged
one — so at any commit rate above one per interval (≈0.2/s at the 5 s default, far below §1.5's
design point) the poll arm would never fire at all. Since `TickMode::All` is the only path that
selects `attempts > 0` rows, a row that failed once would then never be retried and never
parked: it would simply sit unpublished forever. Silently, too — `oldest_unpublished_age_seconds`
is derived from each tick's own row set, so `Fresh` ticks keep overwriting it while ignoring the
stuck rows. The deadline is therefore hoisted out of the loop and only advanced when the poll arm
actually fires.

**Arm order is load-bearing too, and for a second reason.** `biased` takes the *first ready* arm,
so ordering `notify` ahead of `poll` reintroduces the same starvation at a higher traffic rate: a
saturating nudge stream keeps a permit permanently ready, and the overdue poll deadline is never
even polled (measured: 1295 notify ticks, 0 poll ticks). That rate is ~5 commits/s — inside
§1.5's design point, not outside it. The order must be **shutdown → poll → notify**.
This costs nudge latency nothing, because `sleep_until` is `Pending` at every instant except its
deadline, so `notify` still wins every other poll of the `select!`.

Rejected alternative: an `if Instant::now() >= next_poll` pre-check before the `select!`. It
bypasses the shutdown arm entirely, and since a `"poll"` source also skips the debounce, a
permanently-overdue deadline could spin without ever observing shutdown.

```rust
let mut next_poll = tokio::time::Instant::now() + self.poll_interval;
'outer: loop {
    let source = tokio::select! {
        biased;                                        // shutdown wins a tie, deterministically
        () = &mut shutdown                       => break 'outer,
        () = tokio::time::sleep_until(next_poll) => {
            next_poll = tokio::time::Instant::now() + self.poll_interval;
            WakeSource::Poll
        }
        () = wake.notified()                     => WakeSource::Notify,
    };
    let mut mode = TickMode::from(source);             // D13
    loop {
        let report = match self.tick_and_record(publisher.as_ref(), mode).await {
            Ok(r) => r,
            Err(_) => break,                           // already logged + counted; never hot-loop
        };
        if report.drained < self.batch_size || report.drained <= report.failures { break }
        // Poll shutdown WITHOUT cancelling anything, then continue draining.
        let stopping = std::future::poll_fn(|cx| {
            std::task::Poll::Ready(shutdown.as_mut().poll(cx).is_ready())
        }).await;
        if stopping { break 'outer }
        mode = TickMode::Fresh;                        // continuation is a nudged tick (D13)
    }
}
```

`biased` on the outer select matters: without it, a ready permit and a ready shutdown are chosen
at random, making AC7 and §5.2 nondeterministic and permitting one extra tick after shutdown.
Biasing costs nothing here because the tick is not inside the select.

**Soundness note, documented because it is one restructuring away from being violated:**
`S: Future<Output = ()>` is not `FusedFuture`, and polling a completed future is a contract
violation. This shape is sound only because *every* path that observes `shutdown` as ready breaks
out of the loop immediately. Anyone restructuring this must preserve that, or switch the parameter
to a `CancellationToken`/`watch::Receiver`, which are poll-after-ready safe.

**D11 — `[outbox].wake_on_commit`, default `true`, gates BOTH the listener and the writer.**
The earlier draft gated only the listener, on the reasoning that `pg_notify` with no listener is
nearly free. D4 refutes that: the writer is the half that can cause a write outage, so an escape
hatch that cannot switch it off is not an escape hatch. `PgOutbox` gains a `notify: bool`
constructor parameter (still `Copy`, still stateless in every meaningful sense) set from config at
composition time.

`wake_on_commit = false` therefore restores today's *wakeup* behaviour exactly: no notify
statement, no listener. It does **not** restore today's *drain* behaviour — D9's backlog
continuation is a change to the drain loop that is independent of the flag and stays active.
AC8 and §5.9 say so rather than over-claiming.

**D12 — five new metric families; wakeup label set primed at zero; one increment per tick.**
See §3.6. Priming matters: a metrics-rs series first appears already at 1, so an `increase()`-based
rule can never fire on the first occurrence of an unprimed label value.

**D13 — nudged ticks drain only never-attempted rows; retries stay on the poll tick.**
This is the decision that keeps §1.2 true. `tick` increments `attempts` once per tick for every
row it locks, and nothing throttles how often a nudged tick happens — so without this, a failing
row's `attempts` would burn at the *commit rate* rather than at `poll_interval_secs`. At even
2 mutations/s a row would reach `max_attempts = 60` in ~30 s instead of ~5 min, dead-lettering
the in-flight backlog on a routine NATS restart and voiding `config.rs:1019`'s dedup-floor
validation while leaving it passing.

So the tick takes a mode:

| Mode | Selection | Used by |
|---|---|---|
| `TickMode::All` | `published_at IS NULL AND parked = false` (today's query) | the poll tick |
| `TickMode::Fresh` | the same, plus `attempts = 0` | notify- and backlog-driven ticks |

Retry cadence is then provably unchanged: a row that has failed once is invisible to every
nudged tick and is retried only by the poll, exactly as today. (A row may burn one attempt on the
nudged tick that first tried it, so the worst-case span is `(max_attempts - 1) × poll_interval`
rather than `max_attempts × poll_interval` — a 1.7% reduction on the default, far inside the
strict inequality `config.rs:1019` already enforces.)

**Cost, stated plainly:** a nudged tick can publish a newer row ahead of an older failing one.
Global FIFO is already best-effort across replicas (§1.4) and SMA-471 §1.3.3 describes ordering
as a consequence rather than a guarantee, so this narrows an ordering property that was never
promised. **Bonus:** it also means fresh events are no longer head-of-line blocked behind a
poison row on the nudge path — a partial, free mitigation of the wart SMA-490 exists to fix.

**D14 — `[outbox].wake_debounce_ms`, default `200`, with jitter.** `Notify::notify_one` stores a
permit, so under sustained write traffic there is always one pending and the relay would tick
back-to-back with zero idle — `BEGIN; SELECT … FOR UPDATE SKIP LOCKED; COMMIT;` as fast as
Postgres answers. NOTIFY broadcasts to every listening session, so R commits/s × N replicas
produces R×N wakeups and `SKIP LOCKED` makes N-1 of those ticks do wasted work. §1.2 rejects a
250 ms poll (4 tx/s/replica) as a busy-loop; shipping an unbounded tick rate would be worse.

After any notify- or backlog-driven tick the relay waits `wake_debounce_ms` (± up to 25% jitter,
so N replicas do not converge on the same instant) before honouring another nudge. At §1.5's
design point the debounce is essentially never reached; it is a backstop that bounds the worst
case to ~5 ticks/s/replica.

The poll's *cadence* is unaffected — its deadline is absolute (D10), so a debounce cannot delay
it beyond the interval. One edge does exist: a poll-driven tick that continues into a backlog
drain is a backlog tick by the time it ends, so it pays one debounce before the loop re-arms.
200 ms against a 5 s interval, and the poll deadline it re-arms against has not moved. Noise,
recorded rather than engineered around.

**D15 — listener liveness is driven by `try_recv`, not by `recv` errors, plus keepalives and a
watchdog.** The earlier draft's reconnect loop would not have worked. `PgListener` sets
`eager_reconnect: true` by default (`sqlx-postgres-0.8.6/src/listener.rs:74`) and `try_recv`
(line 256) catches `ConnectionAborted`/`UnexpectedEof`/`TimedOut`/`BrokenPipe`, drops the
connection, calls `connect_if_needed()` — which re-issues the `LISTEN` — and loops (lines
285-299). `recv()` loops on `try_recv`, so for the ordinary blip it **never returns `Err`**:
`iam_outbox_listener_connected` would have sat at 1 and `..._reconnects_total` at 0 straight
through a real reconnect, in exactly the scenario §7 says the gauge exists for.

So:

- Call `eager_reconnect(false)` and use `try_recv()`. `Ok(None)` then means "the connection was
  lost and re-established, notifications may have been missed" — which is precisely the signal
  the gauge and the reconnect counter should be driven from.
- Set **TCP keepalives** on the listener's connect options — this is the mechanism that actually
  heals a wedge. sqlx sets none and `try_recv` has no read timeout, so a silently-dropped
  connection leaves Postgres believing the session is alive and LISTENing: the half-open case that
  fills the notification queue and triggers D4. With `keepalives_idle = 30s`,
  `keepalives_interval = 10s`, `keepalives_retries = 3`, a dead peer surfaces as an error from
  `try_recv` within ~60 s and the existing reconnect path handles it. Well inside the time the
  8 GB queue needs to fill.
- Add an **observability-only watchdog**: if no notification has arrived within
  `max(60s, 3 × poll_interval)`, log a warning. It deliberately does **not** force a reconnect.
  A forced reconnect on silence would be wrong — silence is the normal state of a quiet system
  (no mutations ⇒ no notifications), so it would churn a connection every watchdog period on an
  idle deployment while proving nothing. Keepalives already cover the case the watchdog would
  otherwise be guessing at; the warning exists so an operator correlating "no notifications" with
  §1.5's pooler failure has a log line as well as a metric.

Note also that `PgListener::connect` uses a default 30 s pool acquire timeout, so a genuinely
unreachable Postgres takes ~30 s to surface — the backoff is sized against that (§3.2), not
against an instant failure.

## 3. Design

### 3.1 Writer — `PgOutbox::enqueue`

`PgOutbox::new(notify: bool)` (D11). When `notify`, after the existing insert and on the same
recovered `DatabaseTransaction`:

```rust
txn.execute(Statement::from_string(
    DbBackend::Postgres,
    "SELECT pg_notify('iam_outbox_event', '')",
)).await.map_err(map_err)?;
```

`pg_notify(text, text)` over the `NOTIFY` utility statement because it takes its channel as a
value rather than as an identifier needing interpolation. (Note: `Statement::from_string` is a
raw unparameterised statement, so this is *not* a prepared-statement argument — the channel is a
compile-time constant either way, and both forms are equally safe here.)

### 3.2 Listener — `adapters/persistence/pg_outbox_listener.rs`

New adapter, sibling to `pg_outbox.rs`. Shape mirrors `PgOutboxMaintainer`/`PgPartitionMaintainer`:
a struct with a `run` method taking a shutdown future, spawned by the composition root.

```
PgOutboxListener::new(url: String, wake: Arc<Notify>, watchdog: Duration)
    .run(shutdown) -> ()
```

Loop:

1. Connect: `PgListener::connect(&url)` with TCP keepalives set and `eager_reconnect(false)`
   (D15). On error → gauge 0, log, backoff, retry.
2. `listener.listen("iam_outbox_event")`. On error → as above.
3. Gauge 1; if this was a reconnect rather than first connect, increment
   `iam_outbox_listener_reconnects_total`.
4. `select!` `listener.try_recv()` against `shutdown` and the watchdog timer.
   - `Ok(Some(_))` → `iam_outbox_listener_notifications_total` += 1; `wake.notify_one()`.
   - `Ok(None)` → reconnected under us; gauge 1 via step 3's counter path, continue.
   - `Err(_)` → gauge 0, log, back to step 1.
   - watchdog elapsed with no notification → `warn!` only, no reconnect (D15).

Connection construction, satisfying D6 and D15 together: build `PgConnectOptions` from the URL,
set the keepalives, open a **private** `PgPool` with `max_connections(1)`, then
`PgListener::connect_with(&pool)`. That pool is the listener's own — it is emphatically not
SeaORM's, so D6's "does not consume a request-serving slot" still holds — and going through
`connect_with` is the only way to supply connect options, which `PgListener::connect(url)` does
not accept.

Backoff starts at 250 ms and doubles to a 30 s cap. **Shutdown is raced against the backoff
sleep and against the connect/listen attempts, not only against `try_recv`** — otherwise a
replica whose Postgres is unreachable could take ~60 s (30 s backoff + 30 s acquire timeout) to
honour SIGTERM, and SMA-471 D11 already flagged exceeding `terminationGracePeriodSeconds` as a
real problem for this service.

**The connection URL must never appear in a log line.** `IamConfig.database_url` is not redacted
in `IamConfig`'s derived `Debug`/`Serialize` (unlike `PublisherConfig::url` and `RawPepper`), so
the listener logs the error only, never the target — mirroring SMA-471 §5.

### 3.3 Relay — `adapters/events/relay.rs`

- `run` gains `wake: Arc<Notify>` and the loop shape in D10.
- `tick` gains a `TickMode` (D13). To avoid churning its eight existing test call sites, `tick`
  keeps its current signature and delegates to a new `tick_with(publisher, mode)`;
  `tick(publisher)` == `tick_with(publisher, TickMode::All)`.
- `tick_and_record` returns `Result<TickReport, DbErr>` rather than `()` — it still logs and
  counts the error itself; returning `tick`'s own `Result` loses nothing and reads better than
  an `Option` that conflates every DB failure. Additive for its two existing callers.
- Each tick increments `iam_outbox_relay_wakeups_total` with the `source` that caused it.

Only two callers of `run` exist — `main.rs:256` and `tests/relay_pg.rs:296`.

### 3.4 Composition root — `main.rs`

The `Arc<Notify>` is created in the existing `if config.outbox.relay_enabled` block, passed to
`relay.run(...)`, and — when `wake_on_commit` — cloned into a second `servers.spawn` for
`PgOutboxListener::run`, on the same `rx.changed()` shutdown watch every other background task
uses.

`relay_enabled = false` means **no listener either** (it would be notifying nobody); the existing
`warn!` covers it. `IamConfig::validate` gains a warning — not a rejection — for
`relay_enabled = false` with `wake_on_commit = true`, mirroring the existing
`relay_enabled = false` + `backend = "nats"` handling at `config.rs:1035`.

`AppState::new`'s signature is untouched (D2), so the ordering constraint at `main.rs:59-61`
needs no rework. `describe_iam_metrics` (`main.rs:403`) gains five `describe_*!` calls and its
doc comment's family count moves 32 → 37.

### 3.5 Config — `config.rs`

```toml
[outbox]
wake_on_commit      = true    # default; gates BOTH the pg_notify writer and the listener
wake_debounce_ms    = 200     # default; validated non-zero
listen_database_url = "..."   # optional; falls back to database_url
```

Field docs state the §1.5 session-mode requirement on `listen_database_url`, the poll-only
meaning of `wake_on_commit = false` **and what it does not turn off** (D11), and D14's rationale
for the debounce.

### 3.6 Observability — `paigasus-observability::names`

Five families, each added to `ALL` and to `describe_iam_metrics`:

| Metric | Type | Notes |
|---|---|---|
| `iam_outbox_relay_wakeups_total{source}` | counter | `source` ∈ `notify` \| `poll` \| `backlog`. **One increment per tick**, so `sum without (source)(wakeups_total) == sum without (result)(ticks_total)` is an invariant §5.11 asserts. All three values primed at zero (D12). |
| `iam_outbox_publish_lag_seconds` | histogram | `now − occurred_at` at publish time. **This is what proves AC1 in production** — nothing else does. `iam_outbox_oldest_unpublished_age_seconds` cannot: it is overwritten to 0 on every empty tick (`relay.rs:207`), and D14's higher tick rate makes it noisier still. The existing `paigasus-observability` buckets already span 5 ms - 10 s. |
| `iam_outbox_listener_notifications_total` | counter | Without it, `wakeups_total{source="notify"} == 0` cannot distinguish "a pooler ate LISTEN / Postgres never notified" (§1.5) from "the relay never observed the permit". |
| `iam_outbox_listener_connected` | gauge | 0/1. **Per-replica, and replicas do not agree** — the same caveat `IAM_NATS_CONNECTED` carries (`names.rs:159-162`). `max by (job)` reports 1 while any single replica is connected, hiding the partial outage worth knowing about. Use `min by (job)`; never `sum`. |
| `iam_outbox_listener_reconnects_total` | counter | Driven by D15's `Ok(None)` path, not by `recv` errors. |

One alert rule **is** in scope, because §1.5's silent-pooler failure is otherwise undetectable:
`iam_outbox_listener_notifications_total` flat at zero while
`iam_outbox_relay_drained_total` is advancing means rows are being written and drained but no
notification ever arrives. Dashboard panels remain out of scope (§8).

`docs/ops/RUNBOOK-observability.md` §2.1/§2.2 gains the five families (`main.rs:401` says the
descriptions mirror it) plus a note to check `pg_notification_queue_usage()` when mutations start
failing at commit (D4).

## 4. Error handling

| Failure | Behaviour |
|---|---|
| Notification queue full | The mutation **fails at COMMIT** (D4), surfacing as an opaque `RepositoryError::Backend` from `SeaOrmTransaction::commit`. Mitigated by D15 (prevent the wedge that causes it) and recoverable by `wake_on_commit = false` (D11). |
| `pg_notify` statement itself errors | Propagates from `enqueue`; the transaction is poisoned and would fail at commit regardless. Not the queue-full path. |
| Listener cannot connect at boot | Log (never the URL), gauge 0, backoff, retry forever. Boot succeeds; delivery is poll-only meanwhile (D7). |
| Listener connection drops | sqlx reconnects internally; surfaced as `Ok(None)` → gauge/counter updated (D15). Notifications during the gap are lost outright — Postgres does not queue for an absent listener — and the poll covers them (D8). |
| Listener wedges (half-open TCP) | TCP keepalives surface it as a `try_recv` error within ~60 s and the reconnect path handles it (D15). This is the case that would otherwise fill the queue and trigger row 1. The watchdog only warns; it never forces a reconnect, because silence is normal on a quiet system. |
| Relay tick returns `DbErr` | Unchanged: logged, `ticks_total{result="error"}` incremented. Additionally ends any continuation run (D9). |
| Publisher fails during a continuation run | Continues while `drained > failures`; stops when no row in the batch succeeded (D9). |
| Shutdown during a continuation run | Checked between ticks; the in-flight tick always completes (D10). |

## 5. Testing

**Without Docker:**

1. `run` ticks on a notify: relay over `DatabaseConnection::Disconnected`, 60 s poll interval;
   poke the `Notify`; assert `ticks_total{result="error"}` advanced within milliseconds. Proves
   the wakeup path with no Postgres, reusing the SMA-465 disconnected-error-path technique.
2. `run` exits promptly on shutdown when a notify permit is already pending, and runs **no**
   further tick (the `biased` guarantee, D10).
3. The D9 continuation predicate over `TickReport`, table-driven: full+all-succeed → continue;
   full+all-fail → stop; **full+mixed → continue** (the case that discriminates
   `drained > failures` from the rejected `failures == 0`); partial → stop.
4. D14 debounce: N rapid notifications produce a tick count bounded by
   `elapsed / wake_debounce_ms`, not by N.

**With testcontainers Postgres:**

5. Enqueue inside an open transaction → a **second connection** listening receives nothing until
   commit, then exactly one notification. This is D2's whole claim, and because the listener is a
   different session it is also the cross-replica proof (§1.3) — a separate replica is a separate
   session and the mechanism does not distinguish them.
6. Rollback → no notification ever.
7. **D13 retry metering, the AC that matters most:** seed a failing row, deliver N wakeups inside
   one poll interval, assert `attempts` advanced by **1**, not N.
8. End-to-end latency: relay running with a 60 s poll interval and a live listener; a mutation
   committed through `PgOutbox` + the UoW is published with p99 < 250 ms.
9. Backlog: seed `batch_size + N` rows **by direct entity insert** (`relay_pg.rs:61`'s `seed_row`
   helper) — *not* through `PgOutbox::enqueue`, which would emit a notification per row and make
   the test pass with the continuation loop deleted — deliver one wakeup, assert all drain
   without an intervening poll interval.
10. Backlog with a failing publisher: assert the continuation stops rather than hot-looping.
11. Metric invariant: `sum(wakeups_total) == sum(ticks_total)` after a mixed run (§3.6).
12. Listener: (a) bad URL at boot → `run` neither returns nor panics, gauge stays 0;
    (b) `pg_terminate_backend` on the listener's pid → gauge 0→1, `reconnects_total` incremented,
    and a post-reconnect notification still wakes the relay.
13. AC7: a publisher blocking on a barrier; signal shutdown mid-tick, release the barrier, assert
    `published_at` was still stamped — i.e. the tick's transaction committed rather than being
    cancelled.
14. `wake_on_commit = false` → no notification is emitted at all (D11 gates the writer), and the
    relay still drains on the interval.

**Config:** `wake_on_commit` defaults `true`, `wake_debounce_ms` defaults `200` and is validated
non-zero, `listen_database_url` falls back to `database_url`, and the
`relay_enabled = false` + `wake_on_commit = true` warning fires.

## 6. Files touched

| File | Change |
|---|---|
| `.../adapters/persistence/pg_outbox.rs` | `new(notify: bool)`; emit `pg_notify` on the caller's txn |
| `.../adapters/persistence/pg_outbox_listener.rs` | **New** — listener adapter |
| `.../adapters/persistence/mod.rs` | Re-export |
| `.../adapters/events/relay.rs` | `wake` param, D10 loop, `TickMode`, `tick_with`, `tick_and_record` return type, wakeup counter, lag histogram |
| `.../adapters/http/mod.rs` | Five `PgOutbox::new()` call sites gain the flag (lines 347, 460, 482, 619, 639) |
| `.../src/config.rs` | Three fields, defaults, validation, warning |
| `.../src/main.rs` | `Arc<Notify>`, spawn listener, 5 × `describe_*!`, family count 32 → 37 |
| `.../tests/relay_pg.rs` | `run` caller at line 296; scenarios above |
| `.../tests/` | New listener + notify integration tests |
| `rs/crates/libs/paigasus-observability/src/names.rs` | Five families + `ALL` |
| `ops/observability/prometheus/rules/` | The notifications-flat-at-zero alert (§3.6) |
| `docs/ops/RUNBOOK-observability.md` | §2.1/§2.2 entries + `pg_notification_queue_usage()` note |

No `Cargo.toml` or `Cargo.lock` change — D6.

## 7. Risks

**A silent pooler swap.** §1.5 assumes direct/session-mode Postgres. If that ever changes, the
feature dies silently. `listen_database_url` is the seam, and §3.6's alert is the detector; both
exist specifically because this failure reports healthy on every other signal.

**Notification-queue exhaustion causes a write outage** (D4). The mitigations are preventative
(D15's keepalives and watchdog) plus a real off switch (D11). The runbook entry is what makes it
diagnosable, since the error itself is an opaque backend error at commit.

**Connection budget.** Each replica adds a second long-lived Postgres backend on top of SeaORM's
pool, scaling with replica count against a default `max_connections = 100`. At §1.5's 2-3
replicas this is noise; it is worth stating before someone scales to 30.

**Ordering narrows on the nudge path** (D13). Accepted, and argued in D13 — but it is the one
behavioural property this change takes away, so it belongs here rather than only in a decision.

## 8. Out of scope

- **Per-row commit in the relay** (SMA-491) and **`PublishError::Permanent`** (SMA-490). Both
  edit `relay.rs`; the issue explicitly asks for one at a time. SMA-491 is additionally better
  scoped *after* measuring with this landed.
- **Any change to `poll_interval_secs` / `max_attempts` defaults** (§1.2).
- **Dashboard panels** for the five new families (the one alert rule *is* in scope, §3.6).
- **`/readyz` reporting listener health** (D7).
- **A `repo:pg-connect-single-site` gate** — the analogue of SMA-471 D13's deferred
  `repo:nats-connect-single-site`. This change introduces the service's second Postgres
  connection path, so the gate becomes worth having; it is not built here.
- **The consumer side** (SMA-492) — this issue blocks it for sequencing, not technically.

## 9. Acceptance criteria

1. A mutation committed on any replica is published with **p99 < 250 ms** against the default 5 s
   poll interval, with no change to that interval (§5.8 for latency, §5.5 for the cross-session
   delivery that makes "any replica" true).
2. The notification is delivered if and only if the enqueuing transaction commits (§5.5, §5.6).
3. A nudge causes **at most one extra relay tick**, asserted at the relay rather than at the
   notification stream (§5.11, D3).
4. A wakeup with more than `batch_size` rows pending drains them without waiting a poll interval
   (§5.9).
5. A publisher failing on *every* row stops the continuation; a *poison row alongside healthy
   rows* does not (§5.3, §5.10).
6. **A failing row's `attempts` advances at most once per `poll_interval_secs` regardless of
   wakeup rate** (§5.7) — so `max_attempts × poll_interval_secs` still describes reality and
   `config.rs:1019`'s dedup-floor validation remains meaningful.
7. The relay's tick rate is bounded by `wake_debounce_ms` under sustained write traffic (§5.4).
8. The listener never fails boot and never takes a replica out of rotation; a killed backend
   recovers on its own with the gauge and reconnect counter both moving (§5.12).
9. Graceful shutdown never cancels an in-flight tick (§5.13), and never runs an extra tick when a
   permit is pending (§5.2).
10. `wake_on_commit = false` emits no notification at all and restores today's wakeup behaviour;
    D9's continuation remains active and the spec says so (§5.14, D11).
11. `iam_outbox_publish_lag_seconds` and `iam_outbox_listener_notifications_total` exist, so AC1
    and §1.5's silent-pooler failure are both observable in production (§3.6).
12. `moon ci :build :test :lint :fmt :deny :machete :typecheck :breaking :affected-smoke
    :parity-corpus-drift :wasm-getrandom-free :promtool :observability-drift :release-parity
    :release-parity-py :release-parity-ts --base origin/main --include-relations` passes.

## 10. Revision log — what the adversarial review changed

| Finding | Disposition |
|---|---|
| Nudged ticks are unmetered retries; collapses SMA-471 D9's budget and voids `config.rs:1019` | **Folded — D13.** The most serious finding; verified against `config.rs:1019`. |
| No floor on tick rate — ships the busy-loop §1.2 rejects | **Folded — D14** (debounce + jitter). |
| D4 wrong: queue-full fails at COMMIT, not at the statement; writer ungated → write outage | **Folded — D4 rewritten, D11 now gates the writer.** Verified against Postgres docs. The reviewer's savepoint suggestion was **rejected**: the failure is at outer commit, after the savepoint is gone. |
| sqlx `PgListener` self-reconnects; gauge/counter would never move | **Folded — D15.** Verified in `sqlx-postgres-0.8.6/src/listener.rs:74,256,285-299`. |
| `failures == 0` predicate dies on one poison row | **Folded — D9** now `drained > failures`. |
| Half-open TCP wedge, no keepalive or watchdog | **Folded — D15.** |
| Pooler incompatibility unmentioned | **Folded — §1.5, `listen_database_url`, §3.6 alert.** Confirmed with Sven: direct/session-mode. |
| `sea_orm::sqlx` already re-exported; no new dep needed | **Folded — D6 rewritten.** Verified at `sea-orm-1.1.20/src/lib.rs:519`. Removed the dependency, the workspace pin and the whole version-skew risk. |
| Listener cannot shut down promptly while backing off | **Folded — §3.2.** |
| Metrics insufficient: no lag, no notifications counter | **Folded — §3.6**, two families added. |
| `wakeups_total` semantics ambiguous vs `ticks_total` | **Folded — one increment per tick**, invariant asserted in §5.11. |
| AC8 over-claimed "exactly" | **Folded — AC10** now enumerates what the flag does not turn off. |
| Listener untested; AC6/AC7 cited prose | **Folded — §5.12, §5.13.** |
| D3 coalescing stated as guarantee, not permission; 8 call sites is really 11 | **Folded — D3 rewritten**, tests assert at the relay. |
| "cannot be forgotten" / "for free" over-claimed; dead-letter replay does not notify | **Folded — D2** now bounds both claims. |
| `pg_notify` vs `NOTIFY` rationale wrong (not a prepared statement) | **Folded — §3.1** corrected; choice retained on other grounds. |
| Outer `select!` should be `biased`; poll-after-ready invariant undocumented | **Folded — D10.** |
| `tick_and_record -> Option` lossy | **Folded — returns `Result<TickReport, DbErr>`.** |
| §6 incomplete; §5.8 seeding would self-defeat; credential redaction; `relay_enabled`×`wake_on_commit`; AC1 unfalsifiable; security section; connection budget | **All folded** — §6, §5.9, §3.2, §3.4, AC1, D3, §7. |
| Two-relay test proving cross-replica wake + no double publish | **Not folded.** §5.5 proves cross-*session* delivery and `SKIP LOCKED` is already covered by `relay_pg.rs` scenario 3; a two-relay test would re-prove existing coverage at meaningful harness cost. Recorded as a known limit of the test set. |
| Move the notify into `SeaOrmTransaction::commit` instead | **Not folded.** It would need interior mutability on `SeaOrmTransaction` and would fire *after* commit — losing exactly the atomicity D2 is built on. The per-event round-trip cost is now stated in D2 instead. |
