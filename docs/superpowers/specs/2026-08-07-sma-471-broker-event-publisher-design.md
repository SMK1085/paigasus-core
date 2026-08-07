# SMA-471 — A real broker `EventPublisher` (NATS JetStream + CloudEvents)

**Status:** design (revised after adversarial review)
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
- **One attempt per row per tick — at most.** Rows are locked, published, and updated inside a
  single transaction; the loop is serial over the batch. "At most" matters: see §1.3.
- **Failure is the retry signal.** `Ok(())` means `published_at` is stamped and the row is never
  looked at again. A publisher that returns `Ok(())` for a message that did not durably land is
  silent data loss — strictly worse than the tracing publisher it replaces.
- **Errors are operator-facing.** `describe_error` (`relay.rs:64`) walks the whole `source()`
  chain into the row's `last_error`, because `PublishError::Backend`'s own `Display` is the
  static string `"backend error"`. Error fidelity is load-bearing.

### 1.2 The consumer requirement

Confirmed during brainstorming: **both internal and external consumers, and latency matters.**

That rules out the cheapest options. An internal-only, fire-and-forget sink would have been
satisfied by almost anything; external subscribers mean the envelope is a public contract and the
broker needs real fan-out with independent consumer positions.

**Honest caveat on latency:** this PR does *not* deliver low latency. Delivery remains gated by
`poll_interval_secs` (5 s default, worse under backlog), because the change that actually fixes
it — waking the relay the moment a mutation commits — is a relay change and is deliberately out
of scope (§7). The latency requirement is what justified rejecting the cheap brokers; it is met
by the *choice*, not yet by the *delivery path*. The follow-up is tracked, not forgotten.

### 1.3 The relay's transaction boundary is the fact that shapes this design

`OutboxRelay::tick` (`relay.rs:137-210`) does the whole batch on **one** transaction: `begin` →
`SELECT … FOR UPDATE SKIP LOCKED LIMIT batch_size` → for each row, `publish` then `update` →
`commit`. Three consequences drive most decisions below:

1. **Every publish is a network round-trip inside a lock-holding write transaction.** Tick
   duration is `batch_size × per-publish latency`, and the row locks plus one pool connection are
   held for all of it.
2. **A tick that fails after publishing loses the bookkeeping, not the publish.** If `update` or
   `commit` fails — or the process is killed mid-tick — messages already accepted by JetStream
   are rolled back on the DB side with `attempts` unchanged. Those rows will be republished
   whenever the service recovers, with **no bound** on the delay.
3. **Rows are drained FIFO.** `ORDER BY id` over UUIDv7 ids (`IdGenerator::new_event_id`,
   `ports.rs:176`) is time-ordering. A row is attempted once per tick only if it is within the
   first `batch_size` unpublished, unparked rows — so a stuck head of the queue starves
   everything behind it.

None of this is introduced by this PR. All of it becomes *observable* for the first time when the
publisher can actually fail.

### 1.4 What this changes about failure

Today a publish cannot fail. After this change it can, and existing defaults become load-bearing
in ways they never were (D8), while the dedup story has to survive §1.3's realities (D3/D9).

## 2. Decisions

### D1 — NATS JetStream, via `async-nats`

The broker is NATS with JetStream enabled; the client is `async-nats` 0.50 (verified on
crates.io: Apache-2.0, `rust-version` 1.88, JetStream in-crate behind the default-on `jetstream`
feature).

Alternatives considered (recorded in full in ADR-0016):

| Option | Why not |
| -- | -- |
| **Redis Streams** | Zero new infra — `redis` is already a workspace dep. But Redis is a weak durable log: `XADD MAXLEN` trims by count/size with no retention-by-age guarantee, consumer-group bookkeeping (pending-entry lists, claim/ack) is ours to operate, and there is no server-side publish deduplication. Every property D3 relies on would have been hand-built. |
| **Kafka / Redpanda** | The strongest external-consumer ecosystem and long-retention story. Rejected on operational weight: a JVM-or-Redpanda broker plus its own coordination is a large first broker dependency for today's event volume, and it is slow to stand up per integration test. |
| **Managed (Pub/Sub, SNS+SQS, Service Bus)** | No infra to run, but binds `paigasus-core` — a public, self-hostable Apache-2.0 repo — to one cloud, and forces local dev and CI onto an emulator. Contradicts the repo's posture. |
| **Postgres `LISTEN`/`NOTIFY`** | Not viable: no durability for a disconnected consumer, an 8 kB payload cap, no fan-out with independent positions. |

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

Only the second `Ok` may become `Ok(())` from `EventPublisher::publish`. Skipping it would mark
rows `published_at` for messages that never reached the stream — see §1.1. §4.3 tests the
negative case, because a fire-and-forget implementation passes every happy-path test.

### D3 — `Nats-Msg-Id` gives *coverage* against duplicate delivery, not a guarantee

Every publish carries `Nats-Msg-Id = ev.id.to_string()` (hyphenated lowercase, pinned by test).
JetStream keeps a per-stream `duplicate_window`; a second publish of the same id inside that
window is not appended, and the ack returns `duplicate: true`, which the publisher treats as
**success**.

The interleaving this defends against:

1. Relay publishes event `E`; JetStream persists it.
2. The ack is lost (connection drop, timeout, leader change).
3. The relay records a failure, `attempts += 1`, and retries `E` later.
4. Without dedup, consumers see `E` twice.

**It is essential to be precise about what this does not cover.** The first draft of this spec
claimed the invariant `duplicate_window > max_attempts × poll_interval_secs` made at-least-once
"safe". That is wrong: it measures the wrong span, and it is violated in exactly the failure
modes the design exists for. The real quantity is *the maximum wall-clock gap between a publish
that reached JetStream and a later republish of that same row*, which is bounded by none of the
config values individually. Enumerated:

| Gap source | Bound |
| -- | -- |
| Normal retry cadence | `poll_interval_secs` per attempt |
| Tick duration under a slow/blackholed broker | `batch_size × per-publish cost` (D10 bounds this) |
| **Tick rollback** — publish landed, `update`/`commit` failed or process killed (§1.3.2) | **unbounded** — a DB outage or crash-loop republishes on recovery, minutes to hours later |
| **FIFO starvation** — a replayed backlog of low-id rows re-injected at the head (§1.3.3) | `ceil(N / batch_size) × max_attempts` ticks |
| **Dead-letter replay** (D4) | operator-chosen; hours to days |

So: dedup covers the common case (a lost ack retried on the next tick or few) and does not cover
crash-recovery or operator replay. The honest framing, which the docs and ADR must carry, is
**at-least-once delivery with a best-effort dedup window; consumers must be idempotent.** That
is the standard transactional-outbox contract, and the issue already anticipates it ("the
publisher needs to be idempotent-friendly on the consumer side").

Closing the remaining gaps properly means committing `published_at` per row rather than per
batch — a relay change, out of scope (§7), and recorded as the follow-up that would upgrade this
from "mostly deduped" to "deduped".

### D4 — Dead-letter replay is explicitly outside the dedup window, and says so

`REPLAY_ONE_SQL` (`pg_dead_letters.rs:45`) sets `parked = false, attempts = 0, parked_at = NULL`
and **keeps the row's `id`**. §6 names replay as the recovery path for an outage longer than the
retry budget — and the most likely reason a row parks under a real broker is exactly "the ack was
lost, then the broker stayed down long enough to exhaust attempts". Replaying that row hours
later republishes an event JetStream already holds, far outside any sane `duplicate_window`.

Two fixes were considered and rejected:

- **Mint a fresh row id on replay.** Rejected: the CloudEvents `id` would change for the same
  logical event, so a consumer's own dedup key changes too — trading a duplicate for an
  undetectable duplicate.
- **Refuse replay past `parked_at + duplicate_window`.** Rejected: it removes the operator's only
  recovery tool precisely when they need it.

So the fix is transparency, not prevention:

- The dead-letter list surface reports each row's parked age, so an operator can see they are
  past the window.
- `iam_outbox_dead_letters_replayed_total` gains a `beyond_dedup_window` label (`"true"` /
  `"false"`), making the exposure measurable rather than theoretical.
- The rustdoc on the replay path and the ADR both state it plainly.

### D5 — Subject is the `EventType` wire string, verbatim

Subject = `ev.event_type.as_wire()` — `iam.principal.created`, `iam.role.granted`,
`iam.api_key.revoked`, `iam.policy.deleted`, and the four others. Stream subject filter is
`iam.>`.

Those strings are already stable by contract (`domain_event.rs:10-12`: "renaming a variant must
not change its wire string") and already namespaced under `iam.`. Reusing them means the subject
space needs no new invention and no second place to keep in sync, and consumers get natural
wildcard filtering.

**No `subject_prefix` config.** Two IAM deployments sharing one NATS is a real concern, and the
idiomatic NATS answer is accounts or JetStream domains, not prefix-mangling subjects. A prefix
would also make the public subject names deployment-dependent, which is what an external consumer
must not have to care about.

### D6 — CloudEvents 1.0, JSON, structured content mode

Body is a single JSON object, content type `application/cloudevents+json; charset=utf-8`:

| CloudEvents attribute | Source | Notes |
| -- | -- | -- |
| `specversion` | `"1.0"` | constant |
| `id` | `ev.id` | same UUID as `Nats-Msg-Id` (D3) |
| `source` | `publisher.source` config | absolute URI, default `urn:paigasus:iam` — see below |
| `type` | `ev.event_type.as_wire()` | same string as the subject (D5) |
| `subject` | `ev.aggregate_prn` | CloudEvents `subject`; omitted if empty |
| `time` | `ev.occurred_at` | RFC 3339 UTC |
| `datacontenttype` | `"application/json"` | describes `data`, not the envelope |
| `data` | `ev.payload` | verbatim |
| `schemaversion` *(ext)* | `ev.schema_version` | integer |
| `actorprn` *(ext)* | `ev.actor_prn` | **omitted** when `None` |
| `correlationid` *(ext)* | `ev.correlation_id` | **omitted** when `None` |

Extension names are lowercase alphanumeric with no separators because the CloudEvents spec
requires it — hence `actorprn`, `correlationid`, `schemaversion` (all ≤ 20 chars, satisfying the
SHOULD). Absent optional extensions are omitted entirely, never serialized as `null`; the spec
has no null attribute values. `subject` is required-if-present to be non-empty, so an empty
`aggregate_prn` is skipped rather than emitted as `""`.

`source` defaults to the absolute URI `urn:paigasus:iam` rather than the path-like
`paigasus/iam`. A relative reference is legal but the spec RECOMMENDS an absolute URI, and a
free-text value (`"my prod cluster"`) would not be a URI-reference at all — so `validate` parses
it rather than only checking non-empty.

**Dedup identity mismatch, stated deliberately.** CloudEvents defines `id` as unique within the
scope of the *producer*, i.e. the identity is `(source, id)`. JetStream keys on `Nats-Msg-Id` =
`id` alone. Since `source` is per-deployment configurable, a consumer following CloudEvents to
the letter and the broker following D3 key on different things. The resolution — documented for
consumers and in the ADR — is: **dedup on `id` alone, and `source` MUST be stable for the
lifetime of a stream.** Changing `source` on a live stream is a breaking operational act.

**Structured mode** (whole event in the body) rather than binary mode (attributes in NATS
headers): the relay hands us a complete event, consumers are polyglot, and one JSON blob is what
every CloudEvents SDK reads without NATS-specific glue. The only header set is `Nats-Msg-Id`,
which is broker machinery, not part of the event.

Alternatives: a **bespoke JSON envelope** (least code, but a contract we invent and then defend
to external consumers for no gain), and **protobuf via `contracts/`** (matches ADR-0004 and the
`contracts:breaking` gate, but `payload` is free-form `serde_json::Value` today, so it means
eight typed payload messages *and* a change to how the outbox is written). CloudEvents has a
protobuf format, so this does not foreclose the latter.

**Scope boundary:** the *envelope* is the contract; the per-event-type `payload` schemas are not
put under contract by this PR.

### D7 — Stream is ensured at boot, and its live config is *verified*, not just logged

At startup, when `backend = "nats"`, the service connects and calls `get_or_create_stream`.

`get_or_create_stream` does `STREAM.INFO` and only creates on a 404 — it **does not reconcile**
an existing stream's config. The first draft treated that as sufficient ("a config drift is an
operator's problem; the boot log says what was found"). That is wrong, because the drifted field
could be `duplicate_window`, which is the field every safety claim in D3 depends on. A stream
pre-created with JetStream's stock 2-minute window — or `duplicate_window: 0`, or
`storage: Memory` — would be silently adopted and look completely healthy.

So after ensuring, the service reads `cached_info()` and **hard-fails boot** unless:

- `duplicate_window >= publisher.duplicate_window_secs`,
- `storage == File`,
- the live `subjects` cover `iam.>`,
- `max_age` is either 0 (unlimited) or `> duplicate_window` (JetStream's own constraint).

Non-reconciliation stays the right behavior — the service must not silently reshape a stream
external consumers depend on — but adoption is now conditional rather than blind. The boot log
states what was found either way, at `info`.

The service therefore needs stream-read and stream-create permission on NATS. Noted in ADR-0016.

**Failure is fatal**, and the connect+ensure happens **before the first `servers.spawn`**, right
after `AppState::new` (`main.rs:60`). This matters: the relay block sits at `main.rs:198`, after
the HTTP (`:87`), metrics (`:103`), gRPC (`:119`) and upkeep (`:139`) listeners are already
spawned, so a `?` there would return with ports bound and possibly requests served, bypassing the
graceful-shutdown `tx.send(())` at `:345` and aborting live listeners. Connecting first makes
"aborts startup" true rather than aspirational.

### D8 — Stream config

| Field | Value | Why |
| -- | -- | -- |
| `name` | `IAM_EVENTS` (config) | |
| `subjects` | `["iam.>"]` | D5 |
| `retention` | `Limits` | a log consumers read at their own pace, not work-queue semantics |
| `storage` | `File` | events must survive a broker restart |
| `duplicate_window` | config, default 3600 s | D10 |
| `max_age` | config, default 7 days | see below |
| `num_replicas` | 1 | single-node default; clustering is an ops concern |

`max_age` is **not** left unset. An unset `max_age` on a `File` stream that the service itself
created means the stream grows until the broker's disk fills, and — because D7 does not reconcile
— an operator's later retention decision has to be applied out-of-band to a stream they did not
create. The default is 7 days, aligned with `outbox.retention.published_days`, so the broker's
retention and the outbox's retention tell the same story. `0` means unlimited and is accepted,
but the service emits a startup `warn!` naming the field when it *creates* a stream with no age
limit.

JetStream requires `duplicate_window <= max_age` when `max_age > 0`; `validate` enforces it.

### D9 — Raise the `max_attempts` default from 5 to 60, and state the head-of-line cost

**This is the one change to existing behavior.** It is a config default, not a relay change.

A row parks after `max_attempts` publish failures, at most one per tick, so the outage a row
survives is `max_attempts × poll_interval_secs`. At today's defaults that is **5 × 5 s = 25
seconds** — a routine NATS restart dead-letters the entire in-flight backlog into the SMA-469
dead-letter surface, and an operator replays it by hand (which D4 shows is itself a
duplicate-delivery event). That default was written when the publisher was an infallible
`tracing::info!`; with a real broker it is a day-one footgun.

`max_attempts = 60` gives ≈5 minutes of broker-outage tolerance at the default poll interval.

**The cost, stated plainly:** a *permanently* failing row — a payload over NATS's 1 MB
`max_payload`, an invalid subject — now burns 60 attempts over 5 minutes instead of 5 over 25
seconds. And because the relay is FIFO (§1.3.3), `batch_size` or more such rows **head-of-line
block every healthy event behind them** for 5 minutes rather than 25 seconds. This is a real 12×
regression in the poison-row case, accepted because permanent failures are believed impossible
today (all eight payload shapes are small fixed objects — §5) while transient broker
unavailability is certain.

The proper fix is a `PublishError::Permanent` variant that the relay parks on immediately instead
of retrying. That requires changing both `paigasus-iam-core` and the relay, so it is a follow-up
(§7). §4.3 adds a guard test that an oversized payload produces an actionable error string, so
the day it becomes possible it is diagnosable.

### D10 — `duplicate_window` default is a coverage window, and validation enforces a floor

Given D3, the window is chosen to *cover* realistic republish gaps, not derived from a formula
that pretends to guarantee anything. Default: **3600 s**, which comfortably covers a poll-cadence
retry, a slow tick, a service restart, and a short DB blip.

`IamConfig::validate` still enforces a floor, relabelled honestly:

```
duplicate_window_secs > max_attempts × poll_interval_secs
```

This is a **necessary but not sufficient** condition — it catches the one gap that *is* fully
determined by config (an operator raising `max_attempts` during an incident past the window), and
the error message says so. Computed in `u64` with `saturating_mul`, since `max_attempts` is `u32`
and the product overflows a naive multiply.

The check runs **only when `backend = "nats"`**. Firing it under `backend = "tracing"` would fail
a deployment's boot over a broker it does not run.

Cost note: a larger window means JetStream holds more message ids in memory. Negligible at IAM's
volume.

### D11 — Bound the *tick*, not just the publish

The first draft gated on `client.connection_state()` (fail immediately when `Disconnected`) and
called the blackholed case "bounded by `publish_timeout_secs`". That is not good enough: at
`batch_size = 100` and a 2 s timeout, a blackholed broker holds 100 `FOR UPDATE` row locks and a
pool connection for **200 seconds**, blocks autovacuum on `event_outbox`, and — because
`OutboxRelay::run`'s `tokio::select!` runs the tick body to completion and `main.rs` sets no
shutdown deadline — makes SIGTERM take up to 200 s. That is past a normal
`terminationGracePeriodSeconds`, so the orchestrator SIGKILLs mid-tick, rolling the batch back
and feeding exactly the unbounded-republish gap in D3.

Calling a number unacceptable and then bounding it by the same number is not a fix. The adapter
therefore carries **both**:

1. **Connection-state gate.** `Disconnected` → return `Err` immediately without dialing.
   `Pending` (a reconnect in flight) is treated as connected and allowed to proceed to the
   timeout, since a reconnect typically completes in well under the ack timeout.
2. **Consecutive-failure short-circuit**, modelled directly on `redis_conn.rs`'s `Breaker` (the
   prior art this design already cites). After N consecutive publish failures the adapter returns
   `Err` without dialing until an open window elapses, then admits one probe. A tick against a
   blackholed broker therefore costs `N × publish_timeout_secs` and not
   `batch_size × publish_timeout_secs` — at `N = 3` and a 2 s timeout, ~6 s instead of 200 s.

Rows still accrue `attempts` at exactly one per tick, so D9's outage tolerance is unaffected;
only the tick's cost changes. `async-nats` reconnects in the background on its own, so no
reconnect logic is written here.

`publish_timeout_secs` is applied as `jetstream::Context::set_timeout`, which covers the API
request *and* the ack wait — named explicitly because a `tokio::time::timeout` wrapped around
only the ack await would leave the request leg unbounded.

### D12 — `tracing` stays the default backend, and `TracingEventPublisher` stays

`[outbox.publisher].backend` defaults to `"tracing"`. Every existing config file, test, and local
run keeps working with no NATS available. Selecting the real publisher is an explicit opt-in,
exactly like `authn.jwks_cache.backend` and `authz.cache.backend` default to `memory`.

`TracingEventPublisher` is not deleted; it remains the zero-dependency local-dev sink.

### D13 — One connection site, no CI gate yet

`repo:redis-connect-single-site` exists because five adapters dial Redis. NATS has exactly one
construction site, so an analogous gate would guard nothing. Worth adding the moment a second
appears; noted in §7.

## 3. The fix

### 3.1 `rs/crates/services/paigasus-iam/src/adapters/events/nats_publisher.rs` (new)

**`CloudEvent<'a>`** — a private `Serialize` struct implementing the D6 mapping. Borrows where it
can; `Option` extensions use `skip_serializing_if = "Option::is_none"`, and `subject` skips on
empty. Pure, no I/O, no NATS types — every D6 row is a unit test.

**`NatsEventPublisher`** — holds `async_nats::Client`, `jetstream::Context`, the `source`, and
the D11 breaker state.

```rust
pub struct NatsEventPublisher { /* client, jetstream, source, breaker */ }

impl NatsEventPublisher {
    /// Connects, ensures the stream, and VERIFIES its live config (D7). Fallible: the caller
    /// aborts boot on `Err`.
    pub async fn connect(cfg: &PublisherConfig) -> Result<Self, NatsPublisherError>;

    /// The real publish, returning the ack. `publish` delegates to this and discards the ack;
    /// tests use it to assert `duplicate == true` (D3), which the `EventPublisher` signature
    /// cannot express.
    pub async fn publish_ack(&self, ev: &DomainEvent) -> Result<PublishAck, NatsPublisherError>;
}
```

`publish_ack` in order: breaker + connection-state gate (D11) → build `CloudEvent` →
`serde_json::to_vec` → `send_publish` with `.message_id(ev.id.to_string())` and the content-type
header → await the ack (D2) → record metrics → return.

Note `.message_id` takes `impl Into<String>`, which `Uuid` does not implement — hence the
explicit `to_string()`, whose hyphenated-lowercase rendering is pinned by a test so it can never
drift from the CloudEvents `id`.

Every error path boxes a `thiserror` error into `PublishError::Backend` with its `source` intact,
so `describe_error` renders something actionable (§1.1). Variants are distinguishable enough that
a test can assert "this was the connection-state gate", not merely "an error".

A small background task sampling `connection_state()` on an interval maintains the
`iam_nats_connected` gauge — see §3.4 for why it cannot be set from `publish`.

### 3.2 `config.rs`

```toml
[outbox.publisher]
backend               = "tracing"   # "tracing" | "nats"
url                   = "nats://localhost:4222"   # Option; REQUIRED when backend = "nats"
stream                = "IAM_EVENTS"
source                = "urn:paigasus:iam"
publish_timeout_secs  = 2
duplicate_window_secs = 3600
max_age_secs          = 604800      # 7 days; 0 = unlimited (warns)
credentials_file      = "/etc/paigasus/nats.creds"   # Option
```

`url` and `credentials_file` are `Option<String>` with **no default** — matching
`JwksCacheConfig`/`AuthzCacheConfig` exactly, which is what D12 claims to mirror. (The first
draft showed `url` with a concrete default *and* required it under `backend = "nats"`, which are
mutually exclusive; the `Option` shape is the one that makes the validation rule meaningful.)

`PublisherConfig` + `PublisherBackend { Tracing, Nats }` with
`#[serde(rename_all = "lowercase")]`, nested under `[outbox]`; `PublisherDefaults` mirrors it in
`Defaults`. `OutboxDefaults.max_attempts` 5 → 60 (D9).

`IamConfig::validate` additions — **all gated on `backend = "nats"`** except where noted:

1. `backend = "nats"` requires `url` (same message shape as the three existing `redis_url`
   checks).
2. `publish_timeout_secs`, `duplicate_window_secs` non-zero.
3. `duplicate_window_secs > max_attempts × poll_interval_secs` (D10), `saturating_mul`, message
   naming all three fields and the product, and stating it is a floor rather than a guarantee.
4. `max_age_secs == 0 || max_age_secs > duplicate_window_secs` (D8, JetStream's constraint).
5. `stream` non-empty; `source` parses as a URI (D6).
6. `relay_enabled = false` with `backend = "nats"` is rejected — otherwise nothing connects,
   nothing ensures the stream, boot succeeds, and the only signal is a generic `warn!` about
   undrained rows. Silent no-op configs are worse than a rejected boot.

### 3.3 `main.rs`

Publisher construction moves **before the first `servers.spawn`** (D7), right after
`AppState::new`:

```rust
let publisher: Arc<dyn EventPublisher> = match (&config.outbox.relay_enabled, &config.outbox.publisher.backend) {
    (true, PublisherBackend::Nats) => Arc::new(NatsEventPublisher::connect(&config.outbox.publisher).await?),
    _ => Arc::new(TracingEventPublisher),
};
```

The existing `if config.outbox.relay_enabled` block then spawns the relay with this handle. The
`warn!` for a disabled relay is untouched.

### 3.4 `paigasus-observability` + metrics

Three new families in `names.rs` and its `ALL` registry:

| Metric | Type | Why |
| -- | -- | -- |
| `iam_nats_publish_duplicates_total` | counter | acks with `duplicate = true` — D3's mechanism proving itself, and a rising rate means acks are being lost |
| `iam_nats_publish_duration_seconds` | histogram | the ack round-trip is on the critical path of a lock-holding transaction (§1.3.1) |
| `iam_nats_connected` | gauge | 0/1, per replica |

Plus a label on an existing counter: `iam_outbox_dead_letters_replayed_total{beyond_dedup_window}`
(D4).

**`iam_nats_connected` is sampled by a background task, not set inside `publish`.** Setting it
per-publish would freeze it at its last value exactly when it matters — during a total outage
every row eventually parks, `drained` goes to 0, `publish` is never called, and the one metric
that says "IAM cannot reach NATS" stops updating. Documented as per-replica (`max by (job)`),
mirroring `IAM_OUTBOX_PARKED_ROWS`.

**`iam_nats_publish_duplicates_total` is primed at zero in `NatsEventPublisher::connect`**, not
in `describe_iam_metrics` — the latter runs only when `metrics.enabled`, and
`Breaker::with_durations` (`redis_conn.rs:334`) already establishes constructor-priming as the
pattern. A metrics-rs counter first appears at the value of its first increment, so an unprimed
counter can never satisfy an `increase() > 0` alert on the *first* duplicate.

**One new alert rule.** The first draft justified shipping none by citing an existing
"publish-failure rate" alert — **that alert does not exist**; `ops/observability/prometheus/rules/iam.rules.yml`
has rules on parked, backlog age, ticks, retention and dead-letter backlog, but nothing on
`iam_outbox_relay_publish_failures_total`. With a real broker that gap matters, and closing it is
free: `relay.rs:204` already increments the counter by 0 every tick, so the series is primed
without any new code. Adding `increase(iam_outbox_relay_publish_failures_total[5m]) > 0` plus its
`promtool` fixture (including the control series the fixture convention requires) is part of this
PR. Dashboard panels for the three new metrics remain out of scope (§7).

`describe_iam_metrics`'s doc comment ("the 27 metric families", `main.rs:361`) becomes 30.

### 3.5 `Cargo.toml`

`async-nats` is added with `default-features = false` and an explicit feature list. Its default
set is much wider than needed (`object-store`, `kv`, `websockets`, `service`, `nuid`), and while
the default TLS backend is `ring` — matching the workspace's rustls/ring posture, verified, so
there is no latent second-crypto-provider panic — the workspace comments legislate explicit
minimal features, and `aws-lc-rs` is an available opt-in that must never be selected. Features
taken: `jetstream`, `ring`, `server_2_10`/`server_2_11`, `nkeys` (for `.creds`). Comment in house
style covering the TLS stance explicitly.

`testcontainers-modules` gains the `nats` feature (verified present in 0.15). `rs/deny.toml` may
need `[licenses] exceptions` for the transitive tree — checked with `moon run repo:deny` before
the PR.

## 4. Tests

### 4.1 Envelope mapping — unit, in `nats_publisher.rs`

No container, no runtime:

- every D6 attribute present with the right value and key spelling; `specversion` exactly `"1.0"`;
- `time` is RFC 3339 and round-trips to the same `DateTime<Utc>`;
- `data` is the payload verbatim, including a nested-object payload;
- `actor_prn: None` ⇒ **no** `actorprn` key at all (not `null`); same for `correlationid`; both
  `Some` ⇒ both present; empty `aggregate_prn` ⇒ no `subject` key;
- `id` equals `DomainEvent.id`, and `Nats-Msg-Id` equals its hyphenated-lowercase rendering —
  the D3/D6 tie, pinned so a `Display` change cannot silently break dedup;
- `type` equals `as_wire()` for all eight `EventType` variants (table-driven);
- **no-secrets regression (§5):** serialize one event per `EventType` with a representative
  payload and assert no key matches `hash|secret|plaintext|email|pepper|token`.

### 4.2 Config — unit, in `config.rs`

- defaults: `backend = Tracing`, `stream = "IAM_EVENTS"`, `source = "urn:paigasus:iam"`,
  `duplicate_window_secs = 3600`, `max_age_secs = 604800`, `max_attempts = 60` (D9);
- absent `[outbox.publisher]` is valid;
- `backend = "nats"` without `url` rejected, field named;
- D10 floor: `duplicate_window_secs = 100, max_attempts = 60, poll_interval_secs = 5` rejected
  naming all three; boundary `duplicate_window_secs == product` **rejected** (strict `>`), one
  more accepted; a `max_attempts` large enough to overflow a naive `u32` multiply is rejected,
  not panicking;
- the same violating combination under `backend = "tracing"` is **accepted** (D10's gating);
- `max_age_secs` between 1 and `duplicate_window_secs` rejected; `0` accepted;
- non-URI `source` rejected;
- `relay_enabled = false` + `backend = "nats"` rejected.

### 4.3 Broker round-trip — integration, `tests/nats_publisher.rs`

testcontainers-modules `nats`. **JetStream must be explicitly enabled** — the `nats-server` image
does not run with `-js` by default, so the module's default command needs verifying and very
likely `ImageExt::with_cmd(["-js"])`; every test here fails at `get_or_create_stream` otherwise.
This is the first thing to confirm in implementation.

1. **Ensure is idempotent.** `connect` twice; both succeed; stream exists once with
   `subjects = ["iam.>"]`.
2. **Round-trip.** Publish; subscribe to `iam.>`; assert `msg.subject == "iam.principal.created"`
   and the body parses as the exact §4.1 CloudEvent.
3. **Dedup (D3).** Publish the same `DomainEvent` twice via `publish_ack`. Second ack has
   `duplicate == true`; both `EventPublisher::publish` calls return `Ok(())`; stream message
   count is **1**. Asserted through `publish_ack` rather than stream-count alone, because a naive
   implementation that swallows the second publish also satisfies a count of 1.
4. **`message_id` is per-event.** Two events differing only in `id` ⇒ stream count 2. (Guards
   against a constant or omitted `message_id`, not against window duration.)
5. **Dedup survives a broker restart.** Publish, restart the container, publish the same event ⇒
   stream count stays 1. JetStream's dedup state must be rebuilt from the persisted stream, and
   "replica failover" is one of the ack-loss causes D3 names — if this fails, D3's coverage claim
   needs narrowing in the docs and the ADR.
6. **Stream-config drift is rejected (D7).** Pre-create `IAM_EVENTS` with a 5 s
   `duplicate_window`; `connect` returns `Err` naming the field. Same for `storage: Memory`.
7. **Down broker fails fast (D11).** Stop the container, publish. Assert `Err`, that
   `elapsed < 200 ms` — an order of magnitude below `publish_timeout_secs`, so the test fails if
   the connection-state gate is deleted and the ack timeout provides the bound — and that the
   error names the connection-state variant, not a timeout.
8. **A blackholed broker does not hold the tick open (D11).** A TCP listener that accepts and
   never answers. Drive one `OutboxRelay::tick` at `batch_size = 100` and assert it completes in
   well under `batch_size × publish_timeout_secs`. This is the test that distinguishes the
   breaker from its absence; the stopped-container case cannot.
9. **D2's negative case.** Delete the stream, then publish ⇒ `Err` with an informative chain, and
   the outbox row keeps `published_at IS NULL`. Without this, a fire-and-forget implementation
   passes the entire suite.
10. **Oversized payload is diagnosable (D9).** A payload past `max_payload` produces an error
    string naming the size problem, not a bare "backend error".
11. **Relay integration.** `OutboxRelay::tick` with a `NatsEventPublisher` against real Postgres
    + real NATS: rows land in the stream and `published_at` is stamped; a tick against a stopped
    broker leaves rows unpublished with `attempts` incremented and `last_error` populated.

### 4.4 Existing suites

`relay_pg.rs`, `mutation_audit_e2e.rs`, `dead_letters_pg.rs` and the config suite stay green.
Two known touch-points: config assertions on `max_attempts = 5` (D9), and
`mutation_audit_e2e.rs:126`'s `OutboxRelay::new(..., 100, 5)` whose comment claims it mirrors
`OutboxConfig`'s defaults — the call still compiles, but the comment goes stale and must be
updated.

### 4.5 Full gate

```
moon ci :build :test :lint :fmt :deny :machete :typecheck :breaking :affected-smoke \
  :parity-corpus-drift :wasm-getrandom-free :promtool :observability-drift \
  :release-parity :release-parity-py :release-parity-ts \
  --base origin/main --include-relations
```

`:wasm-getrandom-free` matters: `async-nats` must not reach the wasm binding's dependency tree.
`:promtool` covers the new alert rule and its fixture.

## 5. What leaves the trust boundary

This is an IAM service and D5 publishes on a flat `iam.>` namespace that any subscriber on the
account can wildcard, so what is in the payloads is a security question, not a formality.

Audited, per `EventType`:

| Event | Payload fields | Verdict |
| -- | -- | -- |
| `principal.created` / `archived` | `principal_id`, `kind` | no PII — notably **no email** |
| `api_key.issued` / `revoked` | `key_id`, `prefix`, `scope`, `status`, `expires_at` | `prefix` is the display prefix, **not** token material; no hash, no pepper |
| `role.granted` / `revoked` | `grant_id`, `role_key`, `scope` | identifiers only |
| `policy.put` / `deleted` | policy identifiers | identifiers only |

No secret or PII field is present in any payload today. §4.1 adds a regression test asserting
that, so a future payload change that adds an email or a hash reds CI rather than quietly
broadcasting it.

**What a subscriber can nonetheless infer** is significant and must be stated rather than
discovered: with `subject = aggregate_prn` and the `actorprn` extension, a subscriber sees *who
granted whom which role on which org/project, in real time* — the full authorization change
graph. That is inherent in publishing IAM events at all, but it means **subject-level NATS
permissions are a deployment requirement, not an optimization**, and `iam.>` must not be readable
by every account tenant. Recorded as an ADR-0016 consequence.

**Credentials in config.** `url` may carry credentials (`nats://user:pass@host`). `IamConfig`
derives `Serialize`/`Debug`, and config is logged at startup, so the `url` field is redacted in
those impls the way `RawPepper` already is. `credentials_file` holds a path, not a secret, and
needs no redaction — but a missing or unreadable file at boot is fatal with an error naming the
path.

## 6. Documentation

- **ADR-0016** in Notion (*Development → Architecture Decision Records*), MADR-style — Status,
  Date, Context, Decision, Consequences, Alternatives — carrying D1's option table, the
  CloudEvents choice, and the consequences: a new operational dependency, stream-management
  permission (D7), **at-least-once-with-a-dedup-window rather than exactly-once (D3)**, the
  replay exposure (D4), the `(source, id)` vs `id` dedup-identity resolution (D6), the
  subject-permission requirement (§5), and a note that production wants `sync_interval: always`
  or `num_replicas: 3` since a single-node `File` ack is weaker durability than "persisted"
  suggests. Added to the ADR index table.
- **Rustdoc** on `nats_publisher.rs` in house style: why ack-waiting is mandatory (D2), what
  `Nats-Msg-Id` does and does **not** cover (D3), and the transaction-duration reason for the
  breaker (D11).
- **`config.rs` doc comments** spelling out D10's floor-not-guarantee framing and D9's rationale.
- **Dead-letter replay docs** carry D4's warning.
- **`docs/dev-setup.md`**: `nats-server -js` is the whole local setup; the default `tracing`
  backend needs nothing.

## 7. Rollout, rollback, residual risk

Rollout is a config flip; `backend = "tracing"` is the default, so merging changes nothing until
an operator opts in. Rollback is the same flip. No schema change, no migration.

| Risk | Mitigation |
| -- | -- |
| Republish outside the dedup window after a crash or DB outage (D3) | documented as at-least-once; per-row commit is the follow-up that closes it |
| Operator replay republishing a delivered event (D4) | parked age surfaced, `beyond_dedup_window` label, documented |
| Outage longer than `max_attempts × poll_interval_secs` still parks | D9 raises tolerance to ~5 min; beyond that the SMA-469 replay surface is the recovery path |
| Poison row head-of-line blocking for 5 min instead of 25 s (D9) | accepted; `PublishError::Permanent` is the follow-up |
| `async-nats` 0.x API churn | pinned via `Cargo.lock`; §4.3 is the regression net |

**Open items to confirm during implementation** (each has a test that fails loudly if the
assumption is wrong): whether `testcontainers-modules`' `nats` module enables JetStream (§4.3);
whether JetStream dedup state survives a restart (§4.3.5); whether
`Client::connection_state()` exists in 0.50 with the semantics D11 assumes; whether `.creds` is
re-read on reconnect, which matters because NATS user JWTs expire and a rotated file may be
needed after an outage; and whether an existing stream with an overlapping subject in the same
account makes `get_or_create_stream` fail with an actionable message.

## 8. Out of scope

- **The consumer side.** Nothing subscribes in this PR.
- **Latency below the poll interval** — the post-commit relay nudge (§1.2). *Follow-up issue.*
- **Per-row commit in the relay**, which would close D3's unbounded-republish gap. *Follow-up.*
- **`PublishError::Permanent` + immediate parking** for deterministic failures (D9). *Follow-up.*
- **Payload schemas under contract** (the protobuf option, D6).
- **A dev-stack compose file.** The repo has no local Postgres or Redis compose either — it runs
  on testcontainers, and `ops/observability/` is observability-specific. A proper `ops/dev/`
  stack is its own issue.
- **NATS accounts, permissions and TLS provisioning.** §5 states the requirement; implementing it
  is an ops concern.
- **Dashboard panels** for the three new metrics (the alert rule *is* in scope, §3.4).
- **`/readyz` reporting NATS health.** Today a permanently broken connection yields a service that
  reports ready while every event parks; detection is via `iam_nats_connected` +
  `IamOutboxBacklogAgeHigh` (~10 min) + the new publish-failure alert (~5 min). Wiring NATS into
  readiness is a deliberate follow-up — it would make a broker outage take the service out of
  rotation, which is the wrong trade for an IAM service whose authn/authz paths do not need NATS.
- **A `repo:nats-connect-single-site` gate** (D13); **clustering / `num_replicas > 1`** (D8).

## 9. Acceptance criteria

1. `NatsEventPublisher` implements `EventPublisher`, publishes each `DomainEvent` as a CloudEvents
   1.0 JSON message on the subject `ev.event_type.as_wire()`, and returns `Ok(())` only after a
   JetStream persistence ack (D2), proven by the §4.3.9 negative test.
2. Every publish carries `Nats-Msg-Id = ev.id`; a duplicate publish inside the window is acked as
   `duplicate` and leaves exactly one message in the stream, with `publish` returning `Ok(())`
   both times — asserted via `publish_ack`, not stream count alone (§4.3.3).
3. The CloudEvents envelope matches the D6 table exactly, with `actorprn` / `correlationid` /
   `subject` omitted when their sources are absent or empty, and `source` validated as a URI.
4. `[outbox.publisher]` exists with the §3.2 fields, `url`/`credentials_file` are `Option` with no
   default, `backend` defaults to `"tracing"`, and `validate` enforces all six rules — including
   the D10 floor **only** under `backend = "nats"`, and rejecting `relay_enabled = false` +
   `backend = "nats"`.
5. With `backend = "nats"`, boot connects, ensures `IAM_EVENTS` idempotently, **verifies the live
   stream's `duplicate_window`/`storage`/`subjects`/`max_age` and fails on drift** (D7), logs what
   was found, and does all of this **before any listener is spawned**, so a failure aborts startup
   with no port bound.
6. `outbox.max_attempts` defaults to 60, and both the rationale and the head-of-line-blocking cost
   are documented on the field (D9).
7. A publish against a known-disconnected client returns `Err` in under 200 ms naming the
   connection-state cause (D11), and one tick at `batch_size = 100` against a **blackholed** broker
   completes in well under `batch_size × publish_timeout_secs` (§4.3.8).
8. The three metrics in §3.4 are registered in `names::ALL`, described at startup, and emitted;
   `iam_nats_publish_duplicates_total` is primed at zero in the constructor;
   `iam_nats_connected` is maintained by a background sampler, not by `publish`;
   `iam_outbox_dead_letters_replayed_total` carries the `beyond_dedup_window` label.
9. A `iam_outbox_relay_publish_failures_total` alert rule plus its `promtool` fixture (with a
   control series) are added and green under `:promtool`.
10. §4.1's no-secrets regression test passes for all eight `EventType` payload shapes, and `url`
    is redacted in `IamConfig`'s `Debug`/`Serialize`.
11. `TracingEventPublisher` is unchanged and remains the default backend.
12. ADR-0016 is written in Notion, linked from the index, and states the at-least-once contract
    rather than implying exactly-once.
13. The full `moon ci` gate list in §4.5 is green against `origin/main`.
