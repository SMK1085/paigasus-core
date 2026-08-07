# SMA-471 — A real broker `EventPublisher` (NATS JetStream + CloudEvents)

**Status:** design
**Date:** 2026-08-07
**Issue:** [SMA-471](https://linear.app/smaschek/issue/SMA-471/iam-implement-a-real-broker-eventpublisher-only-the-tracing-publisher)
**Project:** Paigasus IAM
**Pairs with:** [SMA-469](https://linear.app/smaschek/issue/SMA-469/iam-outbox-retention-a-real-dead-letter-path-for-parked-events) (outbox retention + dead letters — **merged**, PR #103)
**ADR:** ADR-0016 (Notion, *Development → Architecture Decision Records*) — the broker choice, drafted alongside this spec

## 1. Problem

`EventPublisher` (`rs/crates/libs/paigasus-iam-core/src/ports.rs:351`) has exactly one
production implementation: `TracingEventPublisher`
(`rs/crates/services/paigasus-iam/src/adapters/events/tracing_publisher.rs:25`), which emits a
`tracing::info!` and returns `Ok(())` unconditionally. Every other implementation in the tree is
a test double (`CountingPublisher` in `relay_pg.rs` / `mutation_audit_e2e.rs` /
`dead_letters_pg.rs`, `FailingPublisher` in `relay_pg.rs`).

Everything *around* the publisher is real and finished:

| Piece | Where | State |
| -- | -- | -- |
| Outbox write, in the mutation's own transaction | `PgOutbox::enqueue` | done (SMA-446 B2) |
| `FOR UPDATE SKIP LOCKED` relay, multi-replica safe | `adapters/events/relay.rs` | done (SMA-446 B8/B9) |
| Attempt counting, poison parking, `last_error` | `relay.rs:155-187` | done (SMA-446 + SMA-469) |
| Retention + dead-letter inspect/replay/discard | `PgOutboxMaintainer`, `DeadLetters` | done (SMA-469) |
| Metrics + alerts | `paigasus-observability::names` | done (SMA-446 Unit 5) |
| **Delivery** | `TracingEventPublisher` | **a log line** |

So the machinery is real and the delivery is not. No process outside `paigasus-iam` can react to
an IAM event today.

### 1.1 What the relay already decided for us

The publisher is not free to pick its own semantics; the relay fixes them:

- **At-least-once.** A publish failure bumps `attempts` and the row is retried on a later tick.
- **One attempt per row per tick.** Rows are locked, published, and updated inside a single
  transaction; the loop is serial over the batch.
- **Failure is the retry signal.** `Ok(())` means `published_at` is stamped and the row is never
  looked at again. A publisher that returns `Ok(())` for a message that did not durably land is
  silent data loss — strictly worse than the tracing publisher it replaces.
- **Errors are operator-facing.** `describe_error` (`relay.rs:64`) walks the whole `source()`
  chain into the row's `last_error`, because `PublishError::Backend`'s own `Display` is the
  static string `"backend error"`. Error fidelity is load-bearing.

### 1.2 The consumer requirement

Confirmed during brainstorming: **both internal and external consumers, and latency matters.**

That rules out the cheapest options. An internal-only, fire-and-forget sink would have been
satisfied by almost anything; external subscribers mean the envelope is a public contract and
the broker needs real fan-out with independent consumer positions.

### 1.3 What this changes about failure

Today a publish cannot fail. After this change it can, and the relay's existing defaults become
load-bearing in a way they never were. This is analyzed in D8 — it is the one place where
landing a real publisher changes the behavior of code this PR does not touch.

## 2. Decisions

### D1 — NATS JetStream, via `async-nats`

The broker is NATS with JetStream enabled; the client is `async-nats` (0.50 at time of writing),
the official Rust client, which ships the JetStream API in-crate.

Alternatives considered (recorded in full in ADR-0016):

| Option | Why not |
| -- | -- |
| **Redis Streams** | Zero new infra — `redis` is already a workspace dep. But Redis is a weak durable log: `XADD MAXLEN` trims by count/size with no retention-by-age guarantee, consumer-group bookkeeping (pending-entry lists, claim/ack) is ours to operate, and there is no server-side publish deduplication. Every property D2/D3 rely on would have been hand-built. |
| **Kafka / Redpanda** | The strongest external-consumer ecosystem and long-retention story. Rejected on operational weight: a JVM-or-Redpanda broker plus its own coordination is a large first broker dependency for today's event volume, and it is slow to stand up per integration test. |
| **Managed (Pub/Sub, SNS+SQS, Service Bus)** | No infra to run, but binds `paigasus-core` — a public, self-hostable Apache-2.0 repo — to one cloud, and forces local dev and CI onto an emulator. Contradicts the repo's posture. |
| **Postgres `LISTEN`/`NOTIFY`** | Not considered viable: no durability for a disconnected consumer, an 8 kB payload cap, and no fan-out with independent positions. |

JetStream wins on the specific properties this design needs: server-side publish deduplication
(D3), durable streams with independent consumer positions, sub-millisecond fan-out, and a single
static Go binary that is trivial to run locally and in a testcontainer.

### D2 — Publishes are ack-waited, never fire-and-forget

`jetstream::Context::send_publish` returns a `PublishAckFuture`; awaiting *that* is what waits
for the server's persistence acknowledgement. Both awaits are required:

```rust
let ack = self.jetstream.send_publish(subject, publish).await?; // request accepted
let ack = ack.await?;                                            // message persisted
```

Only the second `Ok` is allowed to become `Ok(())` from `EventPublisher::publish`. Skipping it
would mark rows `published_at` for messages that never reached the stream — see §1.1.

### D3 — `Nats-Msg-Id` is the outbox row id, and that is what makes at-least-once safe

Every publish carries `Nats-Msg-Id = ev.id` (via `Publish::build().message_id(..)`). JetStream
keeps a per-stream `duplicate_window`; a second publish of the same id inside that window is not
appended, and the ack comes back with `duplicate: true`.

This closes the exact gap the issue names. The dangerous interleaving is:

1. Relay publishes event `E`. JetStream persists it.
2. The ack is lost (connection drop, timeout, replica failover).
3. The relay records a failure, `attempts += 1`, and retries `E` on a later tick.
4. Without dedup, consumers see `E` twice.

With `Nats-Msg-Id`, step 3's retry returns `duplicate: true`, the relay stamps `published_at`,
and the stream holds exactly one copy. `duplicate: true` is treated as **success**, not as a
special case — that is the whole point.

The issue asks for the publisher to be "idempotent-friendly on the consumer side (event ids are
already in the outbox rows)". Setting the CloudEvents `id` to the same UUID (D5) means a
consumer doing its own dedup keys on exactly the value JetStream keys on.

### D4 — Subject is the `EventType` wire string, verbatim

Subject = `ev.event_type.as_wire()` — `iam.principal.created`, `iam.role.granted`,
`iam.api_key.revoked`, `iam.policy.deleted`, and the four others. Stream subject filter is
`iam.>`.

Those strings are already stable by contract (`domain_event.rs:10-12`: "renaming a variant must
not change its wire string") and already namespaced under `iam.`. Reusing them means the subject
space needs no new invention, no new registry, and no second place to keep in sync — and
consumers get natural wildcard filtering (`iam.api_key.>`, `iam.>`).

**No `subject_prefix` config.** Two IAM deployments sharing one NATS is a real concern, and the
idiomatic NATS answer is accounts or JetStream domains, not prefix-mangling subjects. A prefix
would also make the public subject names deployment-dependent, which is precisely what an
external consumer must not have to care about.

### D5 — CloudEvents 1.0, JSON, structured content mode

The message body is a single JSON object with content type `application/cloudevents+json;
charset=utf-8`:

| CloudEvents attribute | Source | Notes |
| -- | -- | -- |
| `specversion` | `"1.0"` | constant |
| `id` | `ev.id` | same UUID as `Nats-Msg-Id` (D3) |
| `source` | `publisher.source` config | default `paigasus/iam`; distinguishes staging/prod/region |
| `type` | `ev.event_type.as_wire()` | same string as the subject (D4) |
| `subject` | `ev.aggregate_prn` | the CloudEvents `subject`, unrelated to the NATS subject |
| `time` | `ev.occurred_at` | RFC 3339 / ISO 8601 UTC |
| `datacontenttype` | `"application/json"` | describes `data`, not the envelope |
| `data` | `ev.payload` | passed through verbatim |
| `schemaversion` *(ext)* | `ev.schema_version` | integer |
| `actorprn` *(ext)* | `ev.actor_prn` | **omitted** when `None` |
| `correlationid` *(ext)* | `ev.correlation_id` | **omitted** when `None` |

Extension attribute names are lowercase alphanumeric with no separators, because the CloudEvents
spec requires it — hence `actorprn` and `correlationid`, not `actor_prn` / `correlation_id`.
Absent optional extensions are omitted entirely rather than serialized as `null`; the spec has
no null attribute values.

**Structured mode** (whole event in the body) rather than binary mode (attributes in NATS
headers, payload in the body): the relay hands us a complete event, consumers are polyglot, and
one JSON blob is what every CloudEvents SDK reads without NATS-specific glue. The one header we
do set is `Nats-Msg-Id`, which is broker machinery, not part of the event.

Alternatives considered:

- **Bespoke JSON envelope** mirroring the outbox row 1:1 — least code, but a contract we would
  have to invent, document, and defend to external consumers, for no gain over a standard that
  fits the data almost exactly.
- **Protobuf via `contracts/`** — matches ADR-0004's proto-first posture and would put the event
  schema behind the existing `contracts:breaking` gate. Rejected for scope: `payload` is
  free-form `serde_json::Value` today, so this means eight typed payload messages *and* a change
  to how the outbox is written. Worth revisiting; CloudEvents itself has a protobuf format, so
  this decision does not foreclose it.

**Scope boundary:** the *envelope* is the contract. The per-event-type `payload` schemas are not
put under contract by this PR; they stay exactly as the outbox already stores them.

### D6 — Stream is ensured at boot, idempotently, and boot fails if it cannot be

At startup, when `backend = "nats"`, `main.rs` connects and calls `get_or_create_stream` with the
stream config (D7), then spawns the relay as it does today.

This matches the repo's existing posture — sea-orm migrations run in-app, and audit-log
partitions are created in-app by `PgPartitionMaintainer`. It keeps dev and test self-contained:
no out-of-band provisioning step before a `cargo nextest` run.

Two properties worth stating explicitly:

- **`get_or_create_stream` does not reconcile.** It creates the stream, or fetches the existing
  one; it does not update an existing stream's config to match. That is the desired behavior
  here: the service cannot silently reshape a stream that external consumers depend on. A config
  drift between what the service wants and what exists is an operator's problem to resolve
  deliberately, and the boot log says what was found.
- **Failure is fatal.** A failed connect or a failed ensure aborts startup rather than warning
  and continuing. The non-fatal variant turns a provisioning mistake into a slow trickle of
  parked events discovered hours later; a refused boot is discovered immediately.

The service therefore needs stream-management permission on NATS, not just publish. Noted in
ADR-0016 as a consequence.

### D7 — Stream config

| Field | Value | Why |
| -- | -- | -- |
| `name` | `IAM_EVENTS` (config) | |
| `subjects` | `["iam.>"]` | D4 |
| `retention` | `Limits` | a log consumers read at their own pace, not work-queue semantics |
| `storage` | `File` | events must survive a broker restart |
| `duplicate_window` | config, default 600 s | D3; the default is derived in D9, not picked |
| `num_replicas` | 1 | single-node default; clustering is an ops concern, not a code default |
| `max_age` | not set by the service | broker-side retention is an operator decision, deliberately not owned here |

### D8 — Raise the `max_attempts` default from 5 to 60

**This is the one change to existing behavior.** It is a config default, not a relay change.

A row parks after `max_attempts` publish failures, and gets at most one attempt per tick. So the
outage a row survives is `max_attempts × poll_interval_secs`. At today's defaults that is
**5 × 5 s = 25 seconds** — meaning a routine NATS restart dead-letters the entire in-flight
backlog into the SMA-469 dead-letter surface, and an operator has to replay it by hand.

That default was written when the publisher was an infallible `tracing::info!` and could not
fail. With a real broker it becomes a footgun on day one. `max_attempts = 60` gives ≈5 minutes of
broker-outage tolerance at the default poll interval, which comfortably covers a restart or a
short partition, while still parking genuinely poison rows (a malformed row fails deterministically
and parks after 5 minutes rather than 25 seconds — an acceptable trade for not dead-lettering
healthy traffic).

Nothing about parking, replay, or the `IamOutboxEventsParked` alert changes; only how long a row
tries before it gets there.

### D9 — `duplicate_window` must exceed the full retry span, and config validates it

D3's guarantee only holds while a retry lands *inside* the dedup window. The last retry of a row
happens at most `max_attempts × poll_interval_secs` after its first attempt, so:

```
duplicate_window_secs > max_attempts × poll_interval_secs
```

D8's defaults make the right-hand side `60 × 5 s = 300 s`, so JetStream's stock 2-minute
duplicate window would **violate** the invariant. The default `duplicate_window_secs` is
therefore **600** — derived from the retry span with headroom, not inherited from JetStream — and
`IamConfig::validate` rejects any combination that violates the invariant, with a message naming
all three fields.

This is exactly the class of silent breakage the validation exists for: an operator raising
`max_attempts` during an incident would otherwise quietly re-open the double-delivery window
that D3 closed.

Note the cost side: a larger `duplicate_window` means JetStream holds more message ids in memory.
At IAM's event volume this is negligible.

### D10 — Fail fast when the connection is known-down

Before publishing, the adapter checks `client.connection_state()`; if `Disconnected`, it returns
`PublishError::Backend` immediately rather than waiting out the ack timeout.

This is not a micro-optimization — it is about how long a Postgres transaction stays open. The
relay's tick holds `FOR UPDATE` locks on the whole batch for the duration of the loop
(`relay.rs:138-189`). At `batch_size = 100` and a 2 s ack timeout, a naive adapter holds 100 row
locks for **200 seconds** during a NATS outage, and blocks shutdown for just as long. Failing
fast keeps the tick short and the transaction brief; rows still accrue `attempts` at exactly one
per tick, so D8's outage tolerance is unaffected.

The residual case is a **blackholed** (rather than refused) NATS, where the client may not yet
know it is disconnected. That is bounded by `publish_timeout_secs`. This is the same
refused-vs-blackholed distinction `adapters/redis_conn.rs` documents for Redis (SMA-473/476); no
breaker is built here, because unlike the Redis path — which is on the authz hot path — this one
is a background drain whose cost is bounded by the tick.

`async-nats` reconnects in the background on its own, so no reconnect logic is written here.

### D11 — `tracing` stays the default backend, and `TracingEventPublisher` stays

`[outbox.publisher].backend` defaults to `"tracing"`. Every existing config file, test, and
local run keeps working with no NATS available. Selecting the real publisher is an explicit
opt-in, exactly like `authn.jwks_cache.backend` and `authz.cache.backend` default to `memory`
with `redis` opt-in.

`TracingEventPublisher` is not deleted. It remains the zero-dependency local-dev sink and the
default.

### D12 — One connection site, no CI gate yet

`repo:redis-connect-single-site` exists because five adapters dial Redis. NATS has exactly one
construction site (the publisher's constructor, called once from `main.rs`), so an analogous
gate would guard nothing. Worth adding the moment a second appears; noted in §7.

## 3. The fix

### 3.1 `rs/crates/services/paigasus-iam/src/adapters/events/nats_publisher.rs` (new)

Two units, deliberately separated so the mapping is testable without a broker:

**`CloudEvent<'a>`** — a private `Serialize` struct implementing the D5 mapping. Borrows from the
`DomainEvent` where it can. `Option` extensions use `skip_serializing_if = "Option::is_none"`.
Pure; no I/O; no NATS types. Every D5 row is a unit test.

**`NatsEventPublisher`** — holds `async_nats::Client` and `jetstream::Context` (both cheap to
clone; the client multiplexes one TCP connection).

```rust
pub struct NatsEventPublisher {
    client: async_nats::Client,
    jetstream: jetstream::Context,
    source: String,
}

impl NatsEventPublisher {
    /// Connects, ensures the stream (D6), and returns the publisher. Fallible: the caller
    /// (`main.rs`) aborts boot on `Err`.
    pub async fn connect(cfg: &PublisherConfig) -> Result<Self, NatsPublisherError>;
}

#[async_trait]
impl EventPublisher for NatsEventPublisher {
    async fn publish(&self, ev: &DomainEvent) -> Result<(), PublishError> { .. }
}
```

`publish` in order: connection-state gate (D10) → build `CloudEvent` → `serde_json::to_vec` →
`send_publish` with `.message_id(ev.id)` and the content-type header → await the ack (D2) →
record metrics (§3.4) → `Ok(())`.

Every error path boxes a `thiserror` error into `PublishError::Backend` with its `source` intact,
so `describe_error` renders something an operator can act on (§1.1).

### 3.2 `config.rs`

```toml
[outbox.publisher]
backend               = "tracing"   # "tracing" | "nats"
url                   = "nats://localhost:4222"
stream                = "IAM_EVENTS"
source                = "paigasus/iam"
publish_timeout_secs  = 2
duplicate_window_secs = 600
credentials_file      = "/etc/paigasus/nats.creds"   # optional
```

- `PublisherConfig` + `PublisherBackend { Tracing, Nats }`, `#[serde(rename_all = "lowercase")]`,
  nested under `[outbox]` — field-for-field the shape of `JwksCacheConfig` / `AuthzCacheConfig`.
- `PublisherDefaults` mirrors it in the `Defaults` struct, per the existing pattern.
- `credentials_file` is a path to a NATS `.creds` (JWT + nkey seed). `url` accepts `tls://` for
  TLS, consistent with the repo's rustls-everywhere posture.
- `OutboxDefaults.max_attempts` 5 → 60 (D8).

`IamConfig::validate` additions:

1. `backend = "nats"` requires `url` — same message shape as the three existing
   `backend = "redis"` requires `redis_url` checks.
2. `publish_timeout_secs` non-zero.
3. `duplicate_window_secs` non-zero.
4. `duplicate_window_secs > max_attempts × poll_interval_secs` (D9), the message naming all three
   fields and the computed product. Computed in `u64` with `saturating_mul` — `max_attempts` is
   `u32` and the product can overflow a naive multiply.
5. `stream` and `source` non-empty.

### 3.3 `main.rs`

Inside the existing `if config.outbox.relay_enabled` block, the publisher is selected before the
relay spawns:

```rust
let publisher: Arc<dyn EventPublisher> = match config.outbox.publisher.backend {
    PublisherBackend::Tracing => Arc::new(TracingEventPublisher),
    PublisherBackend::Nats => Arc::new(NatsEventPublisher::connect(&config.outbox.publisher).await?),
};
```

The `?` is what makes D6 fail-fast. The existing `warn!` for a disabled relay is untouched.

### 3.4 `paigasus-observability` + metric registration

Three new families in `names.rs` and its `ALL` registry (the set `:observability-drift` checks):

| Metric | Type | Why |
| -- | -- | -- |
| `iam_nats_publish_duplicates_total` | counter | acks with `duplicate = true` — D3's mechanism proving itself. A rising rate means acks are being lost and the relay is retrying. |
| `iam_nats_publish_duration_seconds` | histogram | the ack round-trip is now on the critical path of a lock-holding transaction (D10). |
| `iam_nats_connected` | gauge | 0/1, set from `client.connection_state()` on each publish. |

`describe_*` registrations go beside the existing outbox ones in `main.rs`.
**`iam_nats_publish_duplicates_total` is primed at zero** when the NATS backend is selected: a
metrics-rs counter series first appears at the value of its first increment, so an unprimed
counter can never satisfy an `increase() > 0` alert on the *first* duplicate — exactly the case
an operator most wants to see. Priming applies to the counter only; the gauge and histogram do
not have this failure mode.

No new alert rules or dashboard panels in this PR — the existing outbox alerts
(`IamOutboxEventsParked`, publish-failure rate) already cover the operator-visible failure modes,
and adding panels for a broker nobody is running yet would be speculative. `:observability-drift`
only asserts the reverse direction (committed dashboards/rules reference *registered* families),
so new registered-but-unreferenced names do not red the gate.

### 3.5 `Cargo.toml`

- `async-nats = "0.50"` in `[workspace.dependencies]` with a comment in the established house
  style (why it exists, feature posture, TLS stance), and `async-nats = { workspace = true }` in
  `paigasus-iam`.
- `testcontainers-modules` gains the `nats` feature (verified present in 0.15).
- `rs/deny.toml` may need a `[licenses] exceptions` entry — `async-nats` is Apache-2.0, but its
  transitive tree must be checked with `moon run repo:deny` before the PR.

## 4. Tests

### 4.1 Envelope mapping — unit, in `nats_publisher.rs`

No container, no runtime. Serialize a hand-built `DomainEvent` and assert on the JSON:

- every D5 attribute present with the right value and the right key spelling;
- `specversion` is exactly `"1.0"`;
- `time` is RFC 3339 with a `Z`/offset, and round-trips back to the same `DateTime<Utc>`;
- `data` is the payload verbatim, including a nested-object payload;
- `actor_prn: None` ⇒ **no** `actorprn` key at all (not `null`); same for `correlationid`;
- both `Some` ⇒ both keys present;
- `id` equals the `DomainEvent.id` — the D3 tie between the CloudEvents id and `Nats-Msg-Id`;
- `type` equals `as_wire()` for all eight `EventType` variants (table-driven).

### 4.2 Config — unit, in `config.rs`

Beside the existing `[outbox]` tests:

- defaults: `backend = Tracing`, `stream = "IAM_EVENTS"`, `source = "paigasus/iam"`,
  `duplicate_window_secs = 600`, `max_attempts = 60` (D8);
- an absent `[outbox.publisher]` block is valid;
- `backend = "nats"` without `url` is rejected, with the field named;
- the D9 invariant: `duplicate_window_secs = 100`, `max_attempts = 60`, `poll_interval_secs = 5`
  is rejected and the message names all three; the boundary case
  `duplicate_window_secs == max_attempts × poll_interval_secs` is **rejected** (strict `>`), and
  one second more is accepted;
- a `max_attempts` large enough to overflow a naive `u32` multiply is rejected, not panicking.

### 4.3 Broker round-trip — integration, `tests/nats_publisher.rs`

testcontainers-modules `nats` image, JetStream enabled. One container per test module.

1. **Ensure is idempotent.** `connect` twice against the same container; both succeed; the stream
   exists once with `subjects = ["iam.>"]`.
2. **Round-trip.** Publish a `DomainEvent`; subscribe to `iam.principal.created`; assert the
   received body parses as the exact CloudEvent from §4.1, and that it arrived on the wire
   subject rather than a wildcard.
3. **Dedup is real (D3).** Publish the *same* `DomainEvent` twice. Assert the second `PublishAck`
   has `duplicate == true`, that both calls return `Ok(())` from `EventPublisher::publish`, and
   that the stream's message count is **1**. This is the guarantee the whole design rests on, so
   it is tested rather than assumed.
4. **Different ids are not deduped.** Two events differing only in `id` ⇒ stream count 2. Guards
   against a dedup window so coarse it swallows distinct events.
5. **Down broker fails, and fails quickly.** Stop the container, then publish. Assert `Err`, that
   the rendered `describe_error` chain is non-trivial (not the bare `"backend error"`), and that
   the call returns well inside a bound derived from `publish_timeout_secs` — the D10 property.
6. **Relay integration.** Drive `OutboxRelay::tick` with a `NatsEventPublisher` against real
   Postgres + real NATS: rows land in the stream, `published_at` is stamped, and a tick against a
   stopped broker leaves rows unpublished with `attempts` incremented and `last_error` populated.

### 4.4 Existing suites

`relay_pg.rs`, `mutation_audit_e2e.rs`, `dead_letters_pg.rs` and the config suite must stay green
untouched, except where they assert `max_attempts = 5` (D8).

### 4.5 Full gate

Per CLAUDE.md, per-project tasks do not run the repo-level gates, and this PR adds workspace deps:

```
moon ci :build :test :lint :fmt :deny :machete :typecheck :breaking :affected-smoke \
  :parity-corpus-drift :wasm-getrandom-free :promtool :observability-drift \
  :release-parity :release-parity-py :release-parity-ts \
  --base origin/main --include-relations
```

`:wasm-getrandom-free` matters here: `async-nats` must not leak into the wasm binding's
dependency tree. It is consumed only by `paigasus-iam`, so it should not — the gate proves it.

## 5. Documentation

- **ADR-0016** in Notion (*Development → Architecture Decision Records*), MADR-style — Status,
  Date, Context, Decision, Consequences, Alternatives considered — covering D1's option table,
  the CloudEvents choice (D5), and the consequences: a new operational dependency, a
  stream-management permission requirement (D6), and the public subject/envelope contract now
  owed to external consumers. Added to the ADR index table.
- **Rustdoc** on `nats_publisher.rs` in the established house style: why ack-waiting is mandatory
  (D2), what `Nats-Msg-Id` buys and the window it depends on (D3/D9), and the transaction-duration
  reason for the connection-state gate (D10).
- **`config.rs` doc comments** on `PublisherConfig`, including the D9 invariant spelled out and
  the D8 rationale on `max_attempts`.
- **Local dev**: `docs/dev-setup.md` gains a short note — `nats-server -js` is the whole local
  setup, and `backend = "tracing"` (the default) needs nothing at all.

## 6. Rollout, rollback, residual risk

**Rollout** is a config flip. `backend = "tracing"` is the default, so merging this changes
nothing in a running deployment until an operator sets `backend = "nats"` and supplies a `url`.

**Rollback** is the same flip in reverse. The outbox keeps accruing rows either way; nothing in
the schema changes, and there is no migration.

Residual risks:

| Risk | Mitigation |
| -- | -- |
| A NATS outage longer than `max_attempts × poll_interval_secs` still dead-letters | D8 raises tolerance to ~5 min; beyond that the SMA-469 replay surface is the recovery path, which is what it is for |
| `duplicate_window` memory growth at a 600 s window | negligible at IAM's volume; revisit if event rate grows by orders of magnitude |
| `get_or_create_stream` silently accepting a drifted existing stream | deliberate (D6); the boot log states what was found. A reconcile-or-fail check is a follow-up |
| `async-nats` 0.x API churn on upgrade | pinned via `Cargo.lock`; the §4.3 round-trip test is the regression net |

## 7. Out of scope

- **The consumer side.** Nothing subscribes in this PR. A first real consumer (e.g. the gateway
  invalidating caches on `iam.api_key.revoked` / `iam.role.revoked`) is its own issue.
- **Delivery latency below the poll interval.** Delivery is still gated by
  `poll_interval_secs` (5 s default). The post-commit nudge — waking the relay the moment a
  mutation commits, with the poll kept as a safety net — is the change that actually fixes this,
  and is deliberately deferred to its own issue rather than mixed into a publisher PR.
- **Payload schemas under contract.** D5 puts the envelope under contract, not the per-event-type
  payloads. Revisit if/when external consumers need typed payloads (the protobuf option).
- **A dev-stack compose file.** The repo has no local Postgres or Redis compose either — it runs
  on testcontainers, and `ops/observability/` is observability-specific. Adding a NATS-only
  compose would be an odd half-measure; a proper `ops/dev/` stack is its own issue.
- **NATS accounts, permissions, and TLS provisioning for external subscribers.** An ops concern.
- **Alert rules and dashboard panels for the new metrics** (§3.4).
- **A `repo:nats-connect-single-site` gate** (D12) — nothing to guard at one construction site.
- **Clustering / `num_replicas > 1`** (D7).

## 8. Acceptance criteria

1. `NatsEventPublisher` implements `EventPublisher`, publishes each `DomainEvent` as a CloudEvents
   1.0 JSON message on the subject `ev.event_type.as_wire()`, and returns `Ok(())` only after a
   JetStream persistence ack (D2).
2. Every publish carries `Nats-Msg-Id = ev.id`; a duplicate publish inside the window is acked as
   `duplicate` and leaves exactly one message in the stream, with the publisher returning
   `Ok(())` both times (D3, tested in §4.3).
3. The CloudEvents envelope matches the D5 table exactly, with `actorprn` / `correlationid`
   omitted when their source fields are `None`.
4. `[outbox.publisher]` exists with the §3.2 fields, defaults to `backend = "tracing"`, and
   `IamConfig::validate` enforces all five rules in §3.2 — including
   `duplicate_window_secs > max_attempts × poll_interval_secs`.
5. With `backend = "nats"`, boot connects and ensures the `IAM_EVENTS` stream idempotently, logs
   whether it was found or created, and **aborts startup** on failure (D6).
6. `outbox.max_attempts` defaults to 60, and the rationale is documented on the field (D8).
7. A publish against a known-disconnected client returns `Err` promptly rather than waiting out
   the ack timeout (D10), and the error's `source()` chain renders informatively through
   `describe_error`.
8. The three metrics in §3.4 are registered in `paigasus-observability::names::ALL`, described at
   startup, and emitted; `iam_nats_publish_duplicates_total` is additionally primed at zero.
9. `TracingEventPublisher` is unchanged and remains the default backend.
10. ADR-0016 is written in Notion and linked from the ADR index.
11. The full `moon ci` gate list in §4.5 is green against `origin/main`.
