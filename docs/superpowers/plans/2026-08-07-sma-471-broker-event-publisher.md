# SMA-471 — Broker `EventPublisher` (NATS JetStream) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace `TracingEventPublisher` as the production outbox sink with a NATS JetStream publisher that emits CloudEvents 1.0 JSON, deduplicated by `Nats-Msg-Id`, without changing the relay.

**Architecture:** A new `NatsEventPublisher` adapter in `paigasus-iam` implements the existing `EventPublisher` port. It connects at boot, ensures *and verifies* a JetStream stream, and publishes each `DomainEvent` as a structured CloudEvent, waiting for the persistence ack. A consecutive-failure short-circuit keeps a dead broker from holding the relay's lock-bearing transaction open. Backend selection is config (`tracing` | `nats`), defaulting to `tracing`.

**Tech Stack:** Rust edition 2024 / rust-version 1.95, `async-nats` 0.50 (JetStream), `serde_json`, `sea-orm`, `metrics`, `testcontainers-modules` (`nats`, `postgres`), `cargo nextest`, Moon.

**Spec:** `docs/superpowers/specs/2026-08-07-sma-471-broker-event-publisher-design.md` — read it before starting. Decision references below (D1–D13) point at its §2.

## Global Constraints

- Every new source file opens with `// SPDX-License-Identifier: Apache-2.0`.
- Rust crates are **edition 2024, rust-version 1.95**.
- `[workspace.lints.rust] warnings = "deny"` — **dead code is a hard compile error on the lib target.** Never add an item in one task intending to wire it in a later one; every task must leave `cargo build` clean. Items that are `pub` in a `pub mod` are public API and exempt.
- Conventional commits with a workspace scope (`feat(rs):`, `docs(rs):`). Subject must **start lowercase** and be **≤100 chars**. Never put a bare `#NNN` in the commit body — write "PR NNN" instead, or commitlint fails `footer-leading-blank` in CI.
- Prefix every shell command with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` so `moon`/`cargo-nextest`/`promtool` resolve to repo-pinned versions.
- Run Rust commands from `rs/`. Tests: `cargo nextest run --no-tests=pass`.
- Working directory is the worktree: `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-471-broker-event-publisher`. Branch: `feature/sma-471-iam-implement-a-real-broker-eventpublisher-only-the-tracing`. **Do not `cd` to the main checkout.**
- Docker-backed integration tests follow the house gating pattern: hard-fail when `CI` is set, skip with a note locally (see `tests/redis_jwks_cache.rs:23-37`).
- Metric names come from `paigasus_observability::names::` consts, never string literals at the call site.

---

## File Structure

| File | Responsibility |
| -- | -- |
| `rs/crates/services/paigasus-iam/src/config.rs` *(modify)* | `PublisherConfig`, `PublisherBackend`, defaults, six validation rules, `max_attempts` default 5→60 |
| `rs/crates/services/paigasus-iam/src/adapters/events/cloud_event.rs` *(create)* | The CloudEvents 1.0 envelope — pure `Serialize` mapping from `DomainEvent`. Public: it *is* the wire contract |
| `rs/crates/services/paigasus-iam/src/adapters/events/nats_publisher.rs` *(create)* | `NatsEventPublisher`: connect, ensure+verify stream, publish with ack, breaker, metrics |
| `rs/crates/services/paigasus-iam/src/adapters/events/mod.rs` *(modify)* | re-exports |
| `rs/crates/services/paigasus-iam/src/main.rs` *(modify)* | publisher selection **before** the first `servers.spawn`; metric descriptions |
| `rs/crates/services/paigasus-iam/src/application/dead_letters.rs` *(modify)* | `beyond_dedup_window` label on the replay counter (D4) |
| `rs/crates/libs/paigasus-observability/src/names.rs` *(modify)* | three new metric-name consts + `ALL` |
| `ops/observability/prometheus/rules/iam.rules.yml` *(modify)* | `IamOutboxPublishFailures` alert |
| `ops/observability/prometheus/rules/tests/iam.test.yml` *(modify)* | its promtool fixture + control series |
| `rs/crates/services/paigasus-iam/tests/nats_publisher.rs` *(create)* | broker round-trip, dedup, drift rejection, fast-fail, blackhole, relay integration |
| `rs/Cargo.toml`, `rs/crates/services/paigasus-iam/Cargo.toml` *(modify)* | `async-nats`, `testcontainers-modules` `nats` feature |

---

## Task 0: ADR-0016 in Notion (before any code)

CLAUDE.md: *"Significant choices get a Notion ADR before code."* This is the repo's first broker dependency.

**Files:** none (Notion only)

**Interfaces:**
- Produces: a Notion page `ADR-0016: NATS JetStream as the event broker for Paigasus IAM`, linked from the ADR index.

- [ ] **Step 1: Read the ADR index and one recent ADR for house format**

Fetch `https://app.notion.com/p/368830e8fbaa816cb411c7ee1682c175` (Architecture Decision Records) and `ADR-0015` (`395830e8fbaa8172b5bcd743d81c0bc0`) via the Notion MCP. Confirm the next free number is **0016**; if 0016 is taken, use the next free one and adjust every reference below.

- [ ] **Step 2: Create the ADR page as a child of the index**

MADR sections, in this order: **Status** (`Proposed`), **Date** (`2026-08-07`), **Context**, **Decision**, **Consequences**, **Alternatives considered**.

Content requirements — each of these must appear, sourced from the spec:
- *Context*: the outbox/relay is complete but delivery is a `tracing` line; consumers will be internal **and** external; latency matters.
- *Decision*: NATS JetStream via `async-nats`; CloudEvents 1.0 JSON structured mode; subject = `EventType` wire string; stream `IAM_EVENTS` over `iam.>`.
- *Alternatives considered*: the four-row table from spec D1 (Redis Streams, Kafka/Redpanda, managed, `LISTEN`/`NOTIFY`), each with its stated reason, plus the envelope alternatives from D6 (bespoke JSON, protobuf via `contracts/`).
- *Consequences*, all of which must be stated explicitly:
  - a new operational dependency, and the service needs **stream read + create** permission (D7);
  - delivery is **at-least-once with a best-effort dedup window, not exactly-once** (D3) — say this plainly, it is the single most important line in the ADR;
  - dead-letter replay republishes outside the window (D4);
  - CloudEvents identity is `(source, id)` but JetStream keys on `id` alone, so consumers dedup on `id` and `source` must be stable for a stream's lifetime (D6);
  - `iam.>` exposes the full authorization change graph, so subject-level NATS permissions are a deployment requirement (spec §5);
  - a single-node `File` stream ack is weaker than "persisted" implies — production wants `sync_interval: always` or `num_replicas: 3`.

- [ ] **Step 3: Add the row to the index table**

Append to the index table: `0016` | link to the new page | `Proposed` | `2026-08-07`.

- [ ] **Step 4: Report the URL**

Print the ADR URL. It is referenced from rustdoc in Task 3.

---

## Task 1: Config block, validation, and the `max_attempts` default

Pure config. No new dependencies, so `:machete` stays green.

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/config.rs`

**Interfaces:**
- Produces:
  - `pub struct PublisherConfig { pub backend: PublisherBackend, pub url: Option<String>, pub stream: String, pub source: String, pub publish_timeout_secs: u64, pub duplicate_window_secs: u64, pub max_age_secs: u64, pub credentials_file: Option<String> }`
  - `pub enum PublisherBackend { Tracing, Nats }` (`#[serde(rename_all = "lowercase")]`)
  - `OutboxConfig` gains `pub publisher: PublisherConfig` (`#[serde(default)]`)

- [ ] **Step 1: Write the failing tests**

Add to `config.rs`'s `#[cfg(test)] mod tests`, beside the existing `[outbox]` tests. Use the same `Figment`/`IamConfig::load`-style helpers the neighbouring tests use — copy their construction idiom exactly rather than inventing one.

```rust
#[test]
fn outbox_publisher_defaults_are_the_tracing_backend() {
    let cfg = load_minimal_config();
    assert_eq!(cfg.outbox.publisher.backend, PublisherBackend::Tracing);
    assert_eq!(cfg.outbox.publisher.url, None);
    assert_eq!(cfg.outbox.publisher.credentials_file, None);
    assert_eq!(cfg.outbox.publisher.stream, "IAM_EVENTS");
    assert_eq!(cfg.outbox.publisher.source, "urn:paigasus:iam");
    assert_eq!(cfg.outbox.publisher.publish_timeout_secs, 2);
    assert_eq!(cfg.outbox.publisher.duplicate_window_secs, 3_600);
    assert_eq!(cfg.outbox.publisher.max_age_secs, 604_800);
}

/// D9: raised from 5 so a routine broker restart does not dead-letter the in-flight backlog.
#[test]
fn outbox_max_attempts_defaults_to_sixty() {
    assert_eq!(load_minimal_config().outbox.max_attempts, 60);
}

#[test]
fn nats_backend_requires_a_url() {
    let err = validate_err(r#"
        [outbox.publisher]
        backend = "nats"
    "#);
    assert!(err.contains("outbox.publisher.url"), "{err}");
}

/// D10: the floor is necessary-not-sufficient, and the message must name all three fields.
#[test]
fn duplicate_window_must_exceed_the_retry_span() {
    let err = validate_err(r#"
        [outbox]
        max_attempts = 60
        poll_interval_secs = 5
        [outbox.publisher]
        backend = "nats"
        url = "nats://localhost:4222"
        duplicate_window_secs = 100
    "#);
    assert!(err.contains("duplicate_window_secs"), "{err}");
    assert!(err.contains("max_attempts"), "{err}");
    assert!(err.contains("poll_interval_secs"), "{err}");
}

/// Strict `>`: equality is REJECTED, one second more is accepted.
#[test]
fn duplicate_window_boundary_is_exclusive() {
    let at = r#"
        [outbox]
        max_attempts = 10
        poll_interval_secs = 5
        [outbox.publisher]
        backend = "nats"
        url = "nats://localhost:4222"
        duplicate_window_secs = 50
        max_age_secs = 0
    "#;
    assert!(validate_result(at).is_err(), "equality must be rejected");
    assert!(validate_result(&at.replace("duplicate_window_secs = 50", "duplicate_window_secs = 51")).is_ok());
}

/// A `u32::MAX` max_attempts must be rejected, not overflow-panic in the product.
#[test]
fn a_huge_max_attempts_is_rejected_not_panicking() {
    let err = validate_err(r#"
        [outbox]
        max_attempts = 4294967295
        poll_interval_secs = 3600
        [outbox.publisher]
        backend = "nats"
        url = "nats://localhost:4222"
        duplicate_window_secs = 3600
    "#);
    assert!(err.contains("duplicate_window_secs"), "{err}");
}

/// D10: the floor is gated on the backend — a tracing deployment must not fail boot over NATS.
#[test]
fn the_window_floor_does_not_apply_to_the_tracing_backend() {
    assert!(validate_result(r#"
        [outbox]
        max_attempts = 60
        poll_interval_secs = 5
        [outbox.publisher]
        backend = "tracing"
        duplicate_window_secs = 1
    "#).is_ok());
}

/// D8: JetStream requires duplicate_window <= max_age when max_age > 0. 0 means unlimited.
#[test]
fn max_age_must_exceed_the_duplicate_window_unless_unlimited() {
    let base = r#"
        [outbox.publisher]
        backend = "nats"
        url = "nats://localhost:4222"
        duplicate_window_secs = 3600
    "#;
    assert!(validate_result(&format!("{base}\nmax_age_secs = 1800")).is_err());
    assert!(validate_result(&format!("{base}\nmax_age_secs = 0")).is_ok(), "0 = unlimited");
    assert!(validate_result(&format!("{base}\nmax_age_secs = 7200")).is_ok());
}

#[test]
fn source_must_parse_as_a_uri() {
    let err = validate_err(r#"
        [outbox.publisher]
        backend = "nats"
        url = "nats://localhost:4222"
        source = "my prod cluster"
    "#);
    assert!(err.contains("outbox.publisher.source"), "{err}");
}

/// A config that publishes nothing while claiming a broker must not boot silently.
#[test]
fn a_disabled_relay_with_the_nats_backend_is_rejected() {
    let err = validate_err(r#"
        [outbox]
        relay_enabled = false
        [outbox.publisher]
        backend = "nats"
        url = "nats://localhost:4222"
    "#);
    assert!(err.contains("relay_enabled"), "{err}");
    assert!(err.contains("outbox.publisher.backend"), "{err}");
}

#[test]
fn zero_timeout_and_zero_window_are_rejected() {
    for field in ["publish_timeout_secs", "duplicate_window_secs"] {
        let err = validate_err(&format!(r#"
            [outbox.publisher]
            backend = "nats"
            url = "nats://localhost:4222"
            {field} = 0
        "#));
        assert!(err.contains(field), "{field}: {err}");
    }
}
```

If `load_minimal_config` / `validate_err` / `validate_result` helpers do not already exist in that test module, write them as thin wrappers over whatever the neighbouring `[outbox]` tests already do — do not restructure existing tests.

- [ ] **Step 2: Run the tests to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --lib config:: 2>&1 | tail -30
```
Expected: compile failure — `PublisherConfig` / `PublisherBackend` do not exist.

- [ ] **Step 3: Add the config types**

In `config.rs`, beside `JwksCacheConfig`/`AuthzCacheConfig` (whose shape this mirrors exactly):

```rust
/// The outbox relay's delivery sink (SMA-471). Mirrors `[authn.jwks_cache]` /
/// `[authz.cache]` field-for-field: a `backend` enum plus the connection fields the non-default
/// backend needs, all `Option` with NO default so `validate` can require them meaningfully.
///
/// Defaults to `tracing`, so an absent `[outbox.publisher]` block — and every existing config
/// file — keeps working with no broker available (SMA-471 D12).
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct PublisherConfig {
    pub backend: PublisherBackend,
    /// Required when `backend = "nats"`. May carry credentials (`nats://user:pass@host`), so it
    /// is redacted in `Debug`/`Serialize` — see the manual impls below.
    pub url: Option<String>,
    pub stream: String,
    /// CloudEvents `source`. MUST be a URI and MUST stay stable for a stream's lifetime:
    /// consumers dedup on `id` alone while CloudEvents scopes identity to `(source, id)`
    /// (SMA-471 D6).
    pub source: String,
    pub publish_timeout_secs: u64,
    /// JetStream's per-stream dedup window. A COVERAGE window, not a guarantee — see
    /// `IamConfig::validate` and SMA-471 D3/D10 for what it does and does not cover.
    pub duplicate_window_secs: u64,
    /// Stream `max_age`. `0` = unlimited (warns at startup when this service creates the
    /// stream): an unbounded `File` stream grows until the broker's disk fills.
    pub max_age_secs: u64,
    /// Path to a NATS `.creds` (JWT + nkey seed). A path, not a secret — no redaction needed.
    pub credentials_file: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PublisherBackend {
    Tracing,
    Nats,
}

impl Default for PublisherConfig {
    fn default() -> Self {
        PublisherConfig {
            backend: PublisherBackend::Tracing,
            url: None,
            stream: "IAM_EVENTS".to_string(),
            source: "urn:paigasus:iam".to_string(),
            publish_timeout_secs: 2,
            duplicate_window_secs: 3_600,
            max_age_secs: 604_800,
            credentials_file: None,
        }
    }
}
```

Add to `OutboxConfig`:

```rust
    /// The delivery sink the relay drains into — see [`PublisherConfig`].
    #[serde(default)]
    pub publisher: PublisherConfig,
```

Add `publisher: PublisherConfig` to `OutboxDefaults` and its `Default` impl (mirroring how `retention: OutboxRetentionConfig` is already carried), and change `max_attempts` in `OutboxDefaults::default()` from `5` to `60`.

- [ ] **Step 4: Redact `url` in `Debug`/`Serialize`**

`PublisherConfig` currently derives both. Replace the derives with manual impls that emit `url` as `Some("<redacted>")` when present, keeping every other field verbatim. Follow whatever `RawPepper` already does in this file for the redaction idiom and copy it; if `PartialEq`/`Eq` are needed (they are — `IamConfig` derives them), keep those derived.

Add a test:

```rust
#[test]
fn the_publisher_url_is_redacted_in_debug() {
    let cfg = PublisherConfig { url: Some("nats://user:hunter2@host:4222".to_string()), ..PublisherConfig::default() };
    let rendered = format!("{cfg:?}");
    assert!(!rendered.contains("hunter2"), "credentials leaked into Debug: {rendered}");
    assert!(rendered.contains("redacted"), "{rendered}");
}
```

- [ ] **Step 5: Add the six validation rules**

In `IamConfig::validate`, after the existing `outbox.poll_interval_secs` check:

```rust
        // SMA-471: the outbox publisher. Every rule below except (6) is gated on the `nats`
        // backend — a `tracing` deployment must never fail boot over a broker it does not run.
        if self.outbox.publisher.backend == PublisherBackend::Nats {
            let p = &self.outbox.publisher;
            if p.url.is_none() {
                return Err("outbox.publisher.backend = \"nats\" requires outbox.publisher.url".to_string());
            }
            if p.publish_timeout_secs == 0 {
                return Err("outbox.publisher.publish_timeout_secs must be at least 1".to_string());
            }
            if p.duplicate_window_secs == 0 {
                return Err("outbox.publisher.duplicate_window_secs must be at least 1".to_string());
            }
            if p.stream.is_empty() {
                return Err("outbox.publisher.stream must not be empty".to_string());
            }
            // A relative reference is legal CloudEvents but an absolute URI is RECOMMENDED, and
            // free text (a space) is not a URI-reference at all. Reject what is clearly not one.
            if p.source.is_empty() || p.source.chars().any(char::is_whitespace) {
                return Err("outbox.publisher.source must be a URI with no whitespace".to_string());
            }
            // SMA-471 D10. A FLOOR, not a guarantee: it catches the one republish gap fully
            // determined by config (an operator raising `max_attempts` past the window). It does
            // NOT cover a tick rollback, a crash-restart, or an operator dead-letter replay —
            // see the spec's D3. `saturating_mul` because `max_attempts` is u32 and the product
            // overflows a naive multiply.
            let retry_span = u64::from(self.outbox.max_attempts).saturating_mul(self.outbox.poll_interval_secs);
            if p.duplicate_window_secs <= retry_span {
                return Err(format!(
                    "outbox.publisher.duplicate_window_secs ({}) must exceed outbox.max_attempts × outbox.poll_interval_secs ({} × {} = {}) — otherwise a row's last retry falls outside JetStream's dedup window and double-delivers",
                    p.duplicate_window_secs, self.outbox.max_attempts, self.outbox.poll_interval_secs, retry_span
                ));
            }
            // SMA-471 D8: JetStream itself requires duplicate_window <= max_age when max_age > 0.
            if p.max_age_secs != 0 && p.max_age_secs <= p.duplicate_window_secs {
                return Err(format!(
                    "outbox.publisher.max_age_secs ({}) must exceed duplicate_window_secs ({}), or be 0 for unlimited",
                    p.max_age_secs, p.duplicate_window_secs
                ));
            }
        }
        // (6) Not gated: a config that names a broker but never spawns the relay publishes
        // nothing while looking correct.
        if !self.outbox.relay_enabled && self.outbox.publisher.backend == PublisherBackend::Nats {
            return Err("outbox.relay_enabled = false with outbox.publisher.backend = \"nats\" would publish nothing — set backend = \"tracing\" or enable the relay".to_string());
        }
```

- [ ] **Step 6: Run the tests to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --lib config:: 2>&1 | tail -20
```
Expected: PASS. Then `cargo clippy -p paigasus-iam -- -D warnings` clean.

- [ ] **Step 7: Fix the stale comment in the existing e2e test**

`rs/crates/services/paigasus-iam/tests/mutation_audit_e2e.rs:126` calls `OutboxRelay::new(..., 100, 5)` with a comment claiming the values mirror `OutboxConfig`'s defaults. The call still compiles; the comment is now wrong. Update it to say the values are fixed for this test and no longer track the defaults.

- [ ] **Step 8: Run the whole iam suite for regressions**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run --no-tests=pass -p paigasus-iam --lib 2>&1 | tail -20
```
Expected: PASS. Docker-backed integration tests may skip locally; that is fine.

- [ ] **Step 9: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/config.rs rs/crates/services/paigasus-iam/tests/mutation_audit_e2e.rs
git commit -m "feat(rs): add the outbox publisher config block and raise max_attempts (SMA-471)"
```

---

## Task 2: The CloudEvents envelope

Pure serde. No NATS dependency yet.

**Files:**
- Create: `rs/crates/services/paigasus-iam/src/adapters/events/cloud_event.rs`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/events/mod.rs`

**Interfaces:**
- Consumes: nothing from Task 1.
- Produces: `pub struct CloudEvent<'a>` with `pub fn from_domain_event(ev: &'a DomainEvent, source: &'a str) -> CloudEvent<'a>`, `Serialize`. **`pub` deliberately**: it is this service's public wire contract, and `warnings = "deny"` would otherwise make it dead code until Task 3.

- [ ] **Step 1: Write the failing tests**

Create `cloud_event.rs` containing only the SPDX header, the `use` lines, and this test module — the implementation comes in Step 3.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use paigasus_iam_core::EventType;
    use uuid::Uuid;

    fn sample(actor: Option<&str>, correlation: Option<Uuid>, aggregate: &str) -> DomainEvent {
        DomainEvent {
            id: Uuid::from_u128(0x1234),
            event_type: EventType::PrincipalCreated,
            schema_version: 1,
            aggregate_prn: aggregate.to_string(),
            actor_prn: actor.map(str::to_string),
            occurred_at: Utc.with_ymd_and_hms(2026, 8, 7, 12, 30, 45).unwrap(),
            payload: serde_json::json!({"kind": "user", "nested": {"a": 1}}),
            correlation_id: correlation,
        }
    }

    fn render(ev: &DomainEvent) -> serde_json::Value {
        serde_json::to_value(CloudEvent::from_domain_event(ev, "urn:paigasus:iam")).unwrap()
    }

    #[test]
    fn maps_every_required_attribute() {
        let ev = sample(Some("prn:pgs:iam:::principal/aa"), Some(Uuid::from_u128(9)), "prn:pgs:iam:::principal/bb");
        let j = render(&ev);
        assert_eq!(j["specversion"], "1.0");
        assert_eq!(j["id"], ev.id.to_string());
        assert_eq!(j["source"], "urn:paigasus:iam");
        assert_eq!(j["type"], "iam.principal.created");
        assert_eq!(j["subject"], "prn:pgs:iam:::principal/bb");
        assert_eq!(j["datacontenttype"], "application/json");
        assert_eq!(j["schemaversion"], 1);
        assert_eq!(j["actorprn"], "prn:pgs:iam:::principal/aa");
        assert_eq!(j["correlationid"], Uuid::from_u128(9).to_string());
        assert_eq!(j["data"], serde_json::json!({"kind": "user", "nested": {"a": 1}}));
    }

    #[test]
    fn time_is_rfc3339_and_round_trips() {
        let ev = sample(None, None, "prn:x");
        let rendered = render(&ev)["time"].as_str().unwrap().to_string();
        let parsed = chrono::DateTime::parse_from_rfc3339(&rendered).unwrap().with_timezone(&Utc);
        assert_eq!(parsed, ev.occurred_at);
    }

    /// CloudEvents has no null attribute values — absent optionals are OMITTED, not `null`.
    #[test]
    fn absent_optionals_are_omitted_not_null() {
        let j = render(&sample(None, None, "prn:x"));
        assert!(j.get("actorprn").is_none(), "actorprn must be absent: {j}");
        assert!(j.get("correlationid").is_none(), "correlationid must be absent: {j}");
    }

    /// CloudEvents requires `subject`, if present, to be non-empty.
    #[test]
    fn an_empty_aggregate_prn_omits_subject() {
        let j = render(&sample(None, None, ""));
        assert!(j.get("subject").is_none(), "empty subject must be omitted: {j}");
    }

    /// The CloudEvents `id` and `Nats-Msg-Id` must be the same string (SMA-471 D3/D6). Pinned
    /// so a `Display` change cannot silently break dedup.
    #[test]
    fn id_renders_as_hyphenated_lowercase() {
        let ev = sample(None, None, "prn:x");
        let rendered = render(&ev)["id"].as_str().unwrap().to_string();
        assert_eq!(rendered, "00000000-0000-0000-0000-000000001234");
        assert_eq!(rendered, ev.id.to_string(), "must match what publish uses for Nats-Msg-Id");
    }

    #[test]
    fn type_matches_the_wire_string_for_every_variant() {
        for et in [
            EventType::PrincipalCreated, EventType::PrincipalArchived,
            EventType::RoleGranted, EventType::RoleRevoked,
            EventType::ApiKeyIssued, EventType::ApiKeyRevoked,
            EventType::PolicyPut, EventType::PolicyDeleted,
        ] {
            let mut ev = sample(None, None, "prn:x");
            ev.event_type = et;
            assert_eq!(render(&ev)["type"], et.as_wire());
        }
    }

    /// SMA-471 §5: no payload may carry a secret or PII. If a future payload adds one, this
    /// reds CI instead of quietly broadcasting it to every subscriber.
    #[test]
    fn no_payload_shape_carries_a_secret_or_pii_key() {
        let payloads = [
            serde_json::json!({"principal_id": "p", "kind": "user"}),
            serde_json::json!({"key_id": "k", "prefix": "pgs_live_ab", "scope": "s", "status": "active", "expires_at": "2026-01-01T00:00:00Z"}),
            serde_json::json!({"grant_id": "g", "role_key": "admin", "scope": "prn:pgs:iam:::org/o"}),
            serde_json::json!({"policy_id": "pol", "policy_key": "starter"}),
        ];
        let banned = ["hash", "secret", "plaintext", "email", "pepper", "token", "password"];
        for payload in payloads {
            let mut ev = sample(None, None, "prn:x");
            ev.payload = payload;
            let rendered = serde_json::to_string(&CloudEvent::from_domain_event(&ev, "urn:paigasus:iam")).unwrap().to_lowercase();
            for needle in banned {
                assert!(!rendered.contains(needle), "payload leaked a `{needle}` key: {rendered}");
            }
        }
    }
}
```

- [ ] **Step 2: Run to verify it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --lib cloud_event 2>&1 | tail -20
```
Expected: compile failure — `CloudEvent` does not exist.

- [ ] **Step 3: Write the implementation**

Prepend to `cloud_event.rs` (above the test module):

```rust
// SPDX-License-Identifier: Apache-2.0

//! The CloudEvents 1.0 envelope `paigasus-iam` publishes its domain events in (SMA-471 D6).
//!
//! **Structured content mode**: the whole event is one JSON object in the message body, content
//! type `application/cloudevents+json`. Chosen over binary mode because the relay hands the
//! publisher a complete event and consumers are polyglot — one JSON blob is what every
//! CloudEvents SDK reads without NATS-specific glue.
//!
//! **This type is the public wire contract.** External consumers depend on these attribute
//! names and on `EventType`'s wire strings; changing either is a breaking change.
//!
//! **Dedup identity, deliberately narrowed.** CloudEvents scopes event identity to
//! `(source, id)`; JetStream's `Nats-Msg-Id` dedup keys on `id` alone. Consumers must therefore
//! dedup on `id`, and `source` must stay stable for the lifetime of a stream (ADR-0016).
//!
//! Extension attribute names (`schemaversion`, `actorprn`, `correlationid`) are lowercase
//! alphanumeric with no separators because the CloudEvents spec requires exactly that — hence
//! `actorprn`, not `actor_prn`.

use chrono::{DateTime, SecondsFormat, Utc};
use paigasus_iam_core::DomainEvent;
use serde::Serialize;
use uuid::Uuid;

/// One `DomainEvent` rendered as a CloudEvents 1.0 JSON envelope. Borrows from the event; build
/// it with [`CloudEvent::from_domain_event`] immediately before serializing.
#[derive(Debug, Serialize)]
pub struct CloudEvent<'a> {
    specversion: &'static str,
    id: String,
    source: &'a str,
    #[serde(rename = "type")]
    event_type: &'static str,
    /// Omitted when empty: CloudEvents requires `subject`, if present, to be non-empty.
    #[serde(skip_serializing_if = "Option::is_none")]
    subject: Option<&'a str>,
    time: String,
    datacontenttype: &'static str,
    schemaversion: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    actorprn: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    correlationid: Option<String>,
    data: &'a serde_json::Value,
}

impl<'a> CloudEvent<'a> {
    #[must_use]
    pub fn from_domain_event(ev: &'a DomainEvent, source: &'a str) -> CloudEvent<'a> {
        CloudEvent {
            specversion: "1.0",
            id: render_id(ev.id),
            source,
            event_type: ev.event_type.as_wire(),
            subject: if ev.aggregate_prn.is_empty() { None } else { Some(ev.aggregate_prn.as_str()) },
            time: render_time(ev.occurred_at),
            datacontenttype: "application/json",
            schemaversion: ev.schema_version,
            actorprn: ev.actor_prn.as_deref(),
            correlationid: ev.correlation_id.map(render_id),
            data: &ev.payload,
        }
    }
}

/// The one place a `Uuid` becomes a string for the wire. Both the CloudEvents `id` and the
/// `Nats-Msg-Id` header go through here, so they can never disagree (SMA-471 D3).
#[must_use]
pub fn render_id(id: Uuid) -> String {
    id.hyphenated().to_string()
}

/// RFC 3339 with second precision and a `Z` suffix. `occurred_at` is already microsecond-
/// truncated by the `Clock` port, but the wire format is pinned here rather than inherited.
fn render_time(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Secs, true)
}
```

In `adapters/events/mod.rs`, add `pub mod cloud_event;` and `pub use cloud_event::{CloudEvent, render_id};`.

- [ ] **Step 4: Run the tests to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --lib cloud_event 2>&1 | tail -20
cargo clippy -p paigasus-iam -- -D warnings
```
Expected: all PASS, clippy clean.

- [ ] **Step 5: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/adapters/events/
git commit -m "feat(rs): add the CloudEvents 1.0 envelope for outbox events (SMA-471)"
```

---

## Task 3: `NatsEventPublisher` — connect, ensure, verify, publish

Adds the `async-nats` dependency and consumes it in the same commit, so `:machete` stays green.

**Files:**
- Modify: `rs/Cargo.toml`, `rs/crates/services/paigasus-iam/Cargo.toml`
- Create: `rs/crates/services/paigasus-iam/src/adapters/events/nats_publisher.rs`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/events/mod.rs`
- Create: `rs/crates/services/paigasus-iam/tests/nats_publisher.rs`

**Interfaces:**
- Consumes: `PublisherConfig` (Task 1); `CloudEvent::from_domain_event`, `render_id` (Task 2).
- Produces:
  - `pub struct NatsEventPublisher`
  - `pub async fn connect(cfg: &PublisherConfig) -> Result<NatsEventPublisher, NatsPublisherError>`
  - `pub async fn publish_ack(&self, ev: &DomainEvent) -> Result<async_nats::jetstream::publish::PublishAck, NatsPublisherError>`
  - `impl EventPublisher for NatsEventPublisher`
  - `pub enum NatsPublisherError` (`thiserror`), with a `StreamConfigDrift { field, want, got }` variant and a `Disconnected` variant used by Task 4

- [ ] **Step 1: Add the dependency**

`rs/Cargo.toml`, in `[workspace.dependencies]`, in the established comment style:

```toml
# async-nats — the official NATS Rust client, and `paigasus-iam`'s outbox `EventPublisher`
# backend (ADR-0016, SMA-471). `default-features = false` for the same "minimal baseline, add
# per-crate" reason as reqwest/sea-orm/redis above: the default set drags in object-store, kv,
# websockets, service and nuid, none of which a publisher needs.
#
# TLS posture: the crate offers `ring` and `aws-lc-rs`. `ring` is selected explicitly to match
# the workspace's rustls/ring baseline — `aws-lc-rs` would link a SECOND crypto provider and
# panic at runtime with "no process-level CryptoProvider available". `nkeys` backs the
# `.creds` (JWT + nkey seed) auth the deployment config allows.
async-nats = { version = "0.50", default-features = false, features = [
  "jetstream", "ring", "server_2_10", "server_2_11", "nkeys",
] }
```

`rs/crates/services/paigasus-iam/Cargo.toml`, in `[dependencies]`:

```toml
# `adapters::events::nats_publisher` — the production outbox sink (SMA-471, ADR-0016).
async-nats = { workspace = true }
```

and in `[dev-dependencies]`, add `nats` to the existing `testcontainers-modules` feature list:

```toml
testcontainers-modules = { version = "0.15", features = ["postgres", "redis", "nats"] }
```

- [ ] **Step 2: Write the failing integration test**

Create `rs/crates/services/paigasus-iam/tests/nats_publisher.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0

//! `NatsEventPublisher` integration tests (SMA-471). Runs against an ephemeral JetStream-enabled
//! NATS in Docker, with the house gating: a missing Docker daemon is a HARD FAILURE in CI and a
//! skip on a Docker-less laptop (mirrors `tests/redis_jwks_cache.rs`).

use async_nats::jetstream;
use chrono::Utc;
use paigasus_iam::adapters::events::NatsEventPublisher;
use paigasus_iam::config::{PublisherBackend, PublisherConfig};
use paigasus_iam_core::{DomainEvent, EventPublisher, EventType};
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::ContainerAsync;
use testcontainers_modules::testcontainers::runners::AsyncRunner;
use uuid::Uuid;

/// Starts NATS **with JetStream enabled**. The stock `nats` image does NOT run with `-js`, so
/// the flag is explicit; without it every test here fails at `get_or_create_stream`.
async fn start_nats() -> Option<(ContainerAsync<Nats>, String)> {
    let node = match Nats::default().with_cmd(["-js"]).start().await {
        Ok(n) => n,
        Err(e) => {
            if std::env::var_os("CI").is_some() {
                panic!("Docker is required for the nats publisher tests in CI: {e}");
            }
            eprintln!("skipping nats_publisher: Docker unavailable ({e})");
            return None;
        }
    };
    let port = node.get_host_port_ipv4(4222).await.unwrap();
    Some((node, format!("nats://127.0.0.1:{port}")))
}

fn cfg(url: &str) -> PublisherConfig {
    PublisherConfig {
        backend: PublisherBackend::Nats,
        url: Some(url.to_string()),
        ..PublisherConfig::default()
    }
}

fn event(id: Uuid, et: EventType) -> DomainEvent {
    DomainEvent {
        id,
        event_type: et,
        schema_version: 1,
        aggregate_prn: "prn:pgs:iam:::principal/00000000-0000-0000-0000-0000000000aa".to_string(),
        actor_prn: None,
        occurred_at: Utc::now(),
        payload: serde_json::json!({"kind": "user"}),
        correlation_id: None,
    }
}

#[tokio::test]
async fn ensure_is_idempotent() {
    let Some((_node, url)) = start_nats().await else { return };
    let first = NatsEventPublisher::connect(&cfg(&url)).await.expect("first connect");
    drop(first);
    NatsEventPublisher::connect(&cfg(&url)).await.expect("second connect must adopt, not fail");

    let js = jetstream::new(async_nats::connect(&url).await.unwrap());
    let info = js.get_stream("IAM_EVENTS").await.unwrap().info().await.unwrap().clone();
    assert_eq!(info.config.subjects, vec!["iam.>".to_string()]);
}

#[tokio::test]
async fn publishes_a_cloud_event_on_the_wire_subject() {
    let Some((_node, url)) = start_nats().await else { return };
    let publisher = NatsEventPublisher::connect(&cfg(&url)).await.unwrap();

    let client = async_nats::connect(&url).await.unwrap();
    let mut sub = client.subscribe("iam.>").await.unwrap();

    let ev = event(Uuid::from_u128(1), EventType::PrincipalCreated);
    publisher.publish(&ev).await.expect("publish");

    let msg = tokio::time::timeout(std::time::Duration::from_secs(5), futures::StreamExt::next(&mut sub))
        .await.expect("no message within 5s").expect("subscription closed");
    assert_eq!(msg.subject.as_str(), "iam.principal.created");

    let body: serde_json::Value = serde_json::from_slice(&msg.payload).unwrap();
    assert_eq!(body["specversion"], "1.0");
    assert_eq!(body["id"], ev.id.to_string());
    assert_eq!(body["type"], "iam.principal.created");
    assert_eq!(body["data"], serde_json::json!({"kind": "user"}));
}

/// SMA-471 D3 — the guarantee the whole design rests on. Asserted through `publish_ack` and NOT
/// through the stream count alone: an implementation that simply swallowed the second publish
/// would also leave one message in the stream.
#[tokio::test]
async fn a_duplicate_publish_is_deduped_and_still_succeeds() {
    let Some((_node, url)) = start_nats().await else { return };
    let publisher = NatsEventPublisher::connect(&cfg(&url)).await.unwrap();
    let ev = event(Uuid::from_u128(42), EventType::RoleRevoked);

    let first = publisher.publish_ack(&ev).await.unwrap();
    assert!(!first.duplicate, "first publish must not be a duplicate");
    let second = publisher.publish_ack(&ev).await.unwrap();
    assert!(second.duplicate, "second publish of the same id must be acked as a duplicate");

    assert!(publisher.publish(&ev).await.is_ok(), "a deduped publish is SUCCESS, not an error");

    let js = jetstream::new(async_nats::connect(&url).await.unwrap());
    let info = js.get_stream("IAM_EVENTS").await.unwrap().info().await.unwrap().clone();
    assert_eq!(info.state.messages, 1, "dedup must leave exactly one message");
}

/// Guards that `message_id` is per-event rather than a constant or omitted.
#[tokio::test]
async fn distinct_ids_are_not_deduped() {
    let Some((_node, url)) = start_nats().await else { return };
    let publisher = NatsEventPublisher::connect(&cfg(&url)).await.unwrap();
    publisher.publish(&event(Uuid::from_u128(1), EventType::RoleGranted)).await.unwrap();
    publisher.publish(&event(Uuid::from_u128(2), EventType::RoleGranted)).await.unwrap();

    let js = jetstream::new(async_nats::connect(&url).await.unwrap());
    let info = js.get_stream("IAM_EVENTS").await.unwrap().info().await.unwrap().clone();
    assert_eq!(info.state.messages, 2);
}

/// SMA-471 D7: an existing stream whose `duplicate_window` is smaller than configured must be
/// REJECTED, not silently adopted — it is the field every safety claim depends on.
#[tokio::test]
async fn a_drifted_duplicate_window_is_rejected_at_connect() {
    let Some((_node, url)) = start_nats().await else { return };
    let js = jetstream::new(async_nats::connect(&url).await.unwrap());
    js.create_stream(jetstream::stream::Config {
        name: "IAM_EVENTS".to_string(),
        subjects: vec!["iam.>".to_string()],
        storage: jetstream::stream::StorageType::File,
        duplicate_window: std::time::Duration::from_secs(5),
        ..Default::default()
    }).await.unwrap();

    let err = NatsEventPublisher::connect(&cfg(&url)).await.expect_err("drifted stream must be rejected");
    let rendered = format!("{err}");
    assert!(rendered.contains("duplicate_window"), "error must name the drifted field: {rendered}");
}

/// SMA-471 D7: memory storage loses every event on a broker restart.
#[tokio::test]
async fn a_memory_storage_stream_is_rejected_at_connect() {
    let Some((_node, url)) = start_nats().await else { return };
    let js = jetstream::new(async_nats::connect(&url).await.unwrap());
    js.create_stream(jetstream::stream::Config {
        name: "IAM_EVENTS".to_string(),
        subjects: vec!["iam.>".to_string()],
        storage: jetstream::stream::StorageType::Memory,
        duplicate_window: std::time::Duration::from_secs(3_600),
        ..Default::default()
    }).await.unwrap();

    let err = NatsEventPublisher::connect(&cfg(&url)).await.expect_err("memory storage must be rejected");
    assert!(format!("{err}").contains("storage"), "{err}");
}

/// SMA-471 D2's negative case. Without this, a fire-and-forget implementation (one that drops
/// the ack future instead of awaiting it) passes the entire rest of this suite.
#[tokio::test]
async fn publishing_with_no_stream_is_an_error_not_a_silent_success() {
    let Some((_node, url)) = start_nats().await else { return };
    let publisher = NatsEventPublisher::connect(&cfg(&url)).await.unwrap();

    let js = jetstream::new(async_nats::connect(&url).await.unwrap());
    js.delete_stream("IAM_EVENTS").await.unwrap();

    let err = publisher.publish(&event(Uuid::from_u128(7), EventType::PolicyPut)).await
        .expect_err("no stream covers the subject — publish must fail");
    let rendered = describe_chain(&err);
    assert!(rendered.len() > "backend error".len(), "error chain must be informative: {rendered}");
}

/// Mirrors `relay.rs::describe_error` so the test asserts what an operator actually sees in
/// `event_outbox.last_error`.
fn describe_chain(err: &(dyn std::error::Error + 'static)) -> String {
    let mut parts = vec![err.to_string()];
    let mut source = err.source();
    while let Some(e) = source {
        parts.push(e.to_string());
        source = e.source();
    }
    parts.join(": ")
}

/// SMA-471 D3/§4.3.5: JetStream's dedup state must survive a broker restart, or D3's coverage
/// claim is narrower than the docs say. If this fails, do NOT weaken the test — narrow the claim
/// in the spec, the rustdoc and ADR-0016.
#[tokio::test]
async fn dedup_survives_a_broker_restart() {
    let Some((node, url)) = start_nats().await else { return };
    let publisher = NatsEventPublisher::connect(&cfg(&url)).await.unwrap();
    let ev = event(Uuid::from_u128(99), EventType::ApiKeyRevoked);
    publisher.publish(&ev).await.unwrap();

    node.stop().await.unwrap();
    node.start().await.unwrap();

    let publisher = NatsEventPublisher::connect(&cfg(&url)).await.unwrap();
    publisher.publish(&ev).await.unwrap();

    let js = jetstream::new(async_nats::connect(&url).await.unwrap());
    let info = js.get_stream("IAM_EVENTS").await.unwrap().info().await.unwrap().clone();
    assert_eq!(info.state.messages, 1, "dedup state must survive a restart");
}
```

Add `futures = { workspace = true }` to `paigasus-iam`'s `[dev-dependencies]` if the subscription `StreamExt::next` call needs it and it is not already there.

- [ ] **Step 3: Run to verify it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --test nats_publisher 2>&1 | tail -30
```
Expected: compile failure — `NatsEventPublisher` does not exist.

**If the container fails to start with `-js`:** the `testcontainers-modules` `Nats` module may already enable JetStream or may expect the flag differently. Inspect the module's source (`~/.cargo/registry/src/*/testcontainers-modules-0.15*/src/nats/mod.rs`) and adjust `start_nats` — this is the first open item in the spec's §7 and must be resolved here, not worked around by skipping the tests.

- [ ] **Step 4: Write the implementation**

Create `nats_publisher.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0

//! `NatsEventPublisher`: the production [`EventPublisher`] (SMA-471, ADR-0016) — the outbox
//! relay's real delivery sink, replacing `TracingEventPublisher` wherever a broker is configured.
//!
//! **Ack-waiting is mandatory, not an optimization.** `send_publish` returns a future; awaiting
//! *that* is what waits for JetStream to persist the message. The relay stamps `published_at`
//! and never revisits a row on `Ok(())`, so returning `Ok` before the ack would silently lose
//! events — strictly worse than the `tracing` publisher this replaces.
//!
//! **What `Nats-Msg-Id` does and does not cover.** Every publish carries the outbox row's id, so
//! JetStream drops a redelivery within the stream's `duplicate_window` and acks it as a
//! duplicate — which this adapter treats as success. That covers the common case: a lost ack
//! retried on a later tick. It does NOT cover a tick that published and then failed to commit
//! (the relay does the whole batch on one transaction), a crash-restart, or an operator
//! dead-letter replay hours later. The contract is therefore **at-least-once with a best-effort
//! dedup window; consumers must be idempotent** — see the spec's D3 and ADR-0016.

use std::time::Duration;

use async_nats::jetstream::{self, publish::PublishAck, stream::StorageType};
use async_trait::async_trait;
use paigasus_iam_core::{DomainEvent, EventPublisher, PublishError};

use crate::adapters::events::cloud_event::{CloudEvent, render_id};
use crate::config::PublisherConfig;

/// The stream's subject filter. Every `EventType` wire string is `iam.`-prefixed
/// (`domain_event.rs`), so one wildcard covers them all.
const SUBJECT_FILTER: &str = "iam.>";

/// Structured-mode CloudEvents content type (SMA-471 D6).
const CONTENT_TYPE: &str = "application/cloudevents+json; charset=utf-8";

#[derive(Debug, thiserror::Error)]
pub enum NatsPublisherError {
    #[error("nats connect failed")]
    Connect(#[source] async_nats::ConnectErrorKind3rdPartyPlaceholder),
    #[error("jetstream stream ensure failed")]
    Ensure(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("stream {stream} has {field} = {got}, but this service requires {want}")]
    StreamConfigDrift { stream: String, field: &'static str, want: String, got: String },
    #[error("event payload could not be serialized")]
    Serialize(#[source] serde_json::Error),
    #[error("jetstream publish failed")]
    Publish(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("nats connection is down")]
    Disconnected,
}
```

**Note on the error type:** `async_nats`'s concrete connect/publish error types must be looked up against the vendored 0.50 source (`~/.cargo/registry/src/*/async-nats-0.50*/src/`) rather than guessed. Replace the placeholder above with the real types, or box them as `Box<dyn std::error::Error + Send + Sync>` as the other variants do. **Boxing is acceptable and preferred where it keeps the `source()` chain intact** — `relay.rs::describe_error` walks that chain into `event_outbox.last_error`, so what matters is that the chain is preserved, not that the type is concrete.

Then the publisher itself:

```rust
/// The production outbox sink. Cheap to clone internals (`Client` multiplexes one TCP
/// connection and reconnects in the background), so no pooling is needed.
pub struct NatsEventPublisher {
    client: async_nats::Client,
    jetstream: jetstream::Context,
    source: String,
}

impl NatsEventPublisher {
    /// Connects, ensures the stream, and **verifies the live stream's config** (SMA-471 D7).
    ///
    /// `get_or_create_stream` creates or fetches; it does NOT reconcile an existing stream's
    /// config. That non-reconciliation is deliberate — this service must never silently reshape
    /// a stream external consumers depend on — but adoption is conditional: a stream whose
    /// `duplicate_window` is shorter than configured, or whose storage is not `File`, or whose
    /// subjects do not cover `iam.>`, fails boot rather than being adopted.
    pub async fn connect(cfg: &PublisherConfig) -> Result<NatsEventPublisher, NatsPublisherError> {
        let url = cfg.url.as_deref().expect("validate() guarantees url is Some for the nats backend");

        let mut opts = async_nats::ConnectOptions::new();
        if let Some(path) = &cfg.credentials_file {
            opts = async_nats::ConnectOptions::with_credentials_file(path.into()).await.map_err(/* Connect */ todo_map)?;
        }
        let client = opts.connect(url).await.map_err(/* Connect */ todo_map)?;

        let mut js = jetstream::new(client.clone());
        // Covers the API request AND the ack wait. A `tokio::time::timeout` around only the ack
        // await would leave the request leg unbounded (SMA-471 D11).
        js.set_timeout(Duration::from_secs(cfg.publish_timeout_secs));

        let want_window = Duration::from_secs(cfg.duplicate_window_secs);
        let stream = js.get_or_create_stream(jetstream::stream::Config {
            name: cfg.stream.clone(),
            subjects: vec![SUBJECT_FILTER.to_string()],
            retention: jetstream::stream::RetentionPolicy::Limits,
            storage: StorageType::File,
            duplicate_window: want_window,
            max_age: Duration::from_secs(cfg.max_age_secs),
            num_replicas: 1,
            ..Default::default()
        }).await.map_err(/* Ensure */ todo_map)?;

        let info = stream.cached_info();
        verify_stream(&cfg.stream, &info.config, want_window, cfg.max_age_secs)?;

        if cfg.max_age_secs == 0 {
            tracing::warn!(stream = %cfg.stream, "outbox.publisher.max_age_secs = 0 — the JetStream stream has no age limit and will grow until the broker's disk fills");
        }
        tracing::info!(stream = %cfg.stream, subjects = ?info.config.subjects, duplicate_window_secs = info.config.duplicate_window.as_secs(), "jetstream stream ready");

        Ok(NatsEventPublisher { client, jetstream: js, source: cfg.source.clone() })
    }

    /// The real publish. [`EventPublisher::publish`] delegates here and discards the ack; tests
    /// use this to assert `duplicate == true`, which the port's `Result<(), _>` cannot express.
    pub async fn publish_ack(&self, ev: &DomainEvent) -> Result<PublishAck, NatsPublisherError> {
        let body = serde_json::to_vec(&CloudEvent::from_domain_event(ev, &self.source))
            .map_err(NatsPublisherError::Serialize)?;

        let mut headers = async_nats::HeaderMap::new();
        headers.insert("Content-Type", CONTENT_TYPE);

        let publish = jetstream::publish::Publish::build()
            .payload(body.into())
            .headers(headers)
            // Same string the CloudEvents `id` renders as — both go through `render_id`.
            .message_id(render_id(ev.id));

        let ack_future = self.jetstream
            .send_publish(ev.event_type.as_wire().to_string(), publish)
            .await
            .map_err(/* Publish */ todo_map)?;
        // The SECOND await. This is what makes `Ok(())` mean "persisted" — see the module doc.
        ack_future.await.map_err(/* Publish */ todo_map)
    }
}

/// Fails when the live stream's config is weaker than what this service requires (D7).
fn verify_stream(
    name: &str,
    live: &jetstream::stream::Config,
    want_window: Duration,
    want_max_age_secs: u64,
) -> Result<(), NatsPublisherError> {
    if live.duplicate_window < want_window {
        return Err(NatsPublisherError::StreamConfigDrift {
            stream: name.to_string(),
            field: "duplicate_window",
            want: format!("{}s", want_window.as_secs()),
            got: format!("{}s", live.duplicate_window.as_secs()),
        });
    }
    if live.storage != StorageType::File {
        return Err(NatsPublisherError::StreamConfigDrift {
            stream: name.to_string(),
            field: "storage",
            want: "file".to_string(),
            got: format!("{:?}", live.storage).to_lowercase(),
        });
    }
    if !live.subjects.iter().any(|s| s == SUBJECT_FILTER) {
        return Err(NatsPublisherError::StreamConfigDrift {
            stream: name.to_string(),
            field: "subjects",
            want: SUBJECT_FILTER.to_string(),
            got: live.subjects.join(","),
        });
    }
    let live_max_age = live.max_age.as_secs();
    if live_max_age != 0 && live_max_age <= want_window.as_secs() {
        return Err(NatsPublisherError::StreamConfigDrift {
            stream: name.to_string(),
            field: "max_age",
            want: format!("> {}s or 0", want_window.as_secs()),
            got: format!("{live_max_age}s"),
        });
    }
    let _ = want_max_age_secs;
    Ok(())
}

#[async_trait]
impl EventPublisher for NatsEventPublisher {
    async fn publish(&self, ev: &DomainEvent) -> Result<(), PublishError> {
        self.publish_ack(ev)
            .await
            .map(|_ack| ())
            .map_err(|e| PublishError::from(Box::new(e) as Box<dyn std::error::Error + Send + Sync>))
    }
}
```

Replace every `todo_map` with the real error-mapping closure once the concrete `async-nats` error types are known. Drop the unused `want_max_age_secs` parameter and the `let _ =` line if it turns out not to be needed — `warnings = "deny"` will tell you.

Unit tests for `verify_stream` (no container needed) belong in this file's `#[cfg(test)] mod tests`: a matching config passes; a shorter `duplicate_window`, `Memory` storage, missing subject, and a `max_age` below the window each fail with the right `field`.

In `adapters/events/mod.rs` add `pub mod nats_publisher;` and `pub use nats_publisher::{NatsEventPublisher, NatsPublisherError};`, and update the module doc: it currently says the tracing publisher stands in "ahead of a real message-bus publisher (a later slice)" — that slice is this one.

- [ ] **Step 5: Run the tests to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --lib nats_publisher 2>&1 | tail -20
cargo nextest run -p paigasus-iam --test nats_publisher 2>&1 | tail -40
cargo clippy -p paigasus-iam -- -D warnings
```
Expected: all PASS (or a clean local skip if Docker is unavailable — but **run them at least once with Docker up** before committing; these are the tests that prove the feature).

- [ ] **Step 6: Check the dependency gates**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:deny repo:machete
cd rs && cargo tree -d 2>&1 | grep -iE "rustls|ring|aws-lc" | head -20
```
Expected: `deny`/`machete` green. The `cargo tree -d` output must show **no** `aws-lc-rs` and no duplicate `rustls` major version. If `deny` reports a license or advisory issue from `async-nats`'s tree, add the narrowest possible `rs/deny.toml` entry with a comment naming SMA-471.

- [ ] **Step 7: Commit**

```bash
git add rs/Cargo.toml rs/Cargo.lock rs/crates/services/paigasus-iam/Cargo.toml \
        rs/crates/services/paigasus-iam/src/adapters/events/ \
        rs/crates/services/paigasus-iam/tests/nats_publisher.rs rs/deny.toml
git commit -m "feat(rs): publish outbox events to nats jetstream as cloudevents (SMA-471)"
```

---

## Task 4: Bound the tick — connection-state gate and failure short-circuit

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/events/nats_publisher.rs`
- Modify: `rs/crates/services/paigasus-iam/tests/nats_publisher.rs`

**Interfaces:**
- Consumes: `NatsEventPublisher`, `NatsPublisherError::Disconnected` (Task 3).
- Produces: no new public API. Internal `Breaker` with `FAILURE_THRESHOLD: u32 = 3` and `OPEN_DURATION: Duration = 2s`, plus `NatsEventPublisher::with_breaker_durations_for_tests`.

- [ ] **Step 1: Write the failing tests**

Unit tests in `nats_publisher.rs`:

```rust
    #[test]
    fn the_breaker_opens_after_three_consecutive_failures() {
        let b = Breaker::with_durations(Duration::from_secs(2));
        assert!(b.admit(), "starts closed");
        for _ in 0..3 { b.on_failure(); }
        assert!(!b.admit(), "three consecutive failures must open it");
    }

    #[test]
    fn a_success_resets_the_failure_run() {
        let b = Breaker::with_durations(Duration::from_secs(2));
        b.on_failure();
        b.on_failure();
        b.on_success();
        b.on_failure();
        b.on_failure();
        assert!(b.admit(), "the run was broken, so two more failures must not open it");
    }

    #[test]
    fn an_open_breaker_admits_a_probe_once_the_window_elapses() {
        let b = Breaker::with_durations(Duration::from_millis(20));
        for _ in 0..3 { b.on_failure(); }
        assert!(!b.admit());
        std::thread::sleep(Duration::from_millis(40));
        assert!(b.admit(), "one probe must be admitted after the open window");
    }
```

Integration tests in `tests/nats_publisher.rs`:

```rust
/// SMA-471 D11: a stopped broker must fail via the connection-state gate, an order of magnitude
/// faster than `publish_timeout_secs` — so this test FAILS if the gate is deleted and the ack
/// timeout provides the bound instead.
#[tokio::test]
async fn a_stopped_broker_fails_fast_not_on_the_ack_timeout() {
    let Some((node, url)) = start_nats().await else { return };
    let publisher = NatsEventPublisher::connect(&cfg(&url)).await.unwrap();
    node.stop().await.unwrap();

    // Let the client observe the drop before timing the gate.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let started = std::time::Instant::now();
    let err = publisher.publish(&event(Uuid::from_u128(5), EventType::PolicyDeleted)).await.expect_err("must fail");
    let elapsed = started.elapsed();

    assert!(elapsed < std::time::Duration::from_millis(200), "expected the connection-state gate, took {elapsed:?}");
    assert!(describe_chain(&err).contains("connection is down"), "{}", describe_chain(&err));
}

/// SMA-471 D11: the relay publishes serially inside ONE lock-holding transaction, so a blackholed
/// broker must not cost `batch_size × publish_timeout_secs`. This is the test that distinguishes
/// the breaker from its absence — the stopped-container case above cannot.
#[tokio::test]
async fn a_blackholed_broker_does_not_hold_a_batch_open() {
    // A listener that accepts and never answers: SYN succeeds, so the client believes it is
    // connected and every publish runs to the ack timeout.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((sock, _)) = listener.accept().await {
            held.push(sock); // never read, never write, never close
        }
    });

    let mut c = cfg(&format!("nats://{addr}"));
    c.publish_timeout_secs = 1;
    // `connect` itself must not hang forever against a blackhole.
    let Ok(publisher) = tokio::time::timeout(std::time::Duration::from_secs(10), NatsEventPublisher::connect(&c)).await else {
        panic!("connect against a blackholed broker must not hang");
    };
    let Ok(publisher) = publisher else { return }; // connect legitimately fails: nothing more to prove

    let started = std::time::Instant::now();
    for i in 0..100u128 {
        let _ = publisher.publish(&event(Uuid::from_u128(1000 + i), EventType::RoleGranted)).await;
    }
    let elapsed = started.elapsed();
    assert!(elapsed < std::time::Duration::from_secs(20),
        "100 publishes against a blackhole took {elapsed:?}; without the breaker this is ~100s");
}
```

- [ ] **Step 2: Run to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --lib nats_publisher 2>&1 | tail -20
```
Expected: compile failure — `Breaker` does not exist.

- [ ] **Step 3: Implement the breaker and the gate**

Add to `nats_publisher.rs`:

```rust
/// Consecutive publish failures that open the breaker. Three rather than one, mirroring
/// `redis_conn.rs`'s `FAILURE_THRESHOLD`: a single blip during a reconnect must not disable the
/// sink for a whole window.
const FAILURE_THRESHOLD: u32 = 3;

/// How long an open breaker short-circuits before admitting one probe.
const OPEN_DURATION: Duration = Duration::from_secs(2);

/// A deliberately minimal consecutive-failure breaker (SMA-471 D11).
///
/// **Why this exists at all**: `OutboxRelay::tick` publishes the whole batch inside ONE
/// transaction holding `FOR UPDATE` locks. At `batch_size = 100` and a 2 s ack timeout, an
/// unbroken adapter against a blackholed broker holds 100 row locks for ~200 s, blocks
/// autovacuum, and makes SIGTERM take just as long — past a normal grace period, so the
/// orchestrator SIGKILLs mid-tick and the batch rolls back. With the breaker a bad tick costs
/// `FAILURE_THRESHOLD × publish_timeout_secs` instead.
///
/// Far simpler than `redis_conn.rs`'s `Breaker`: no half-open permit, no epoch, no metrics
/// role label. Those exist there because the Redis breaker guards eleven concurrent call sites
/// on the authz hot path; this one guards a single serial background loop, where a probe that
/// is admitted and then fails simply re-opens the window on the next `on_failure`.
#[derive(Debug)]
struct Breaker {
    open_duration: Duration,
    inner: std::sync::Mutex<BreakerInner>,
}

#[derive(Debug)]
struct BreakerInner {
    consecutive_failures: u32,
    opened_at: Option<std::time::Instant>,
}

impl Breaker {
    fn with_durations(open_duration: Duration) -> Breaker {
        Breaker { open_duration, inner: std::sync::Mutex::new(BreakerInner { consecutive_failures: 0, opened_at: None }) }
    }

    /// `true` = go ahead and dial. An open breaker admits exactly one probe per window: the
    /// `opened_at` reset means the next caller short-circuits again until this probe reports.
    fn admit(&self) -> bool {
        let mut inner = self.inner.lock().expect("breaker mutex poisoned");
        match inner.opened_at {
            None => true,
            Some(at) if at.elapsed() >= self.open_duration => {
                inner.opened_at = Some(std::time::Instant::now());
                true
            }
            Some(_) => false,
        }
    }

    fn on_success(&self) {
        let mut inner = self.inner.lock().expect("breaker mutex poisoned");
        inner.consecutive_failures = 0;
        inner.opened_at = None;
    }

    fn on_failure(&self) {
        let mut inner = self.inner.lock().expect("breaker mutex poisoned");
        inner.consecutive_failures = inner.consecutive_failures.saturating_add(1);
        if inner.consecutive_failures >= FAILURE_THRESHOLD {
            inner.opened_at = Some(std::time::Instant::now());
        }
    }
}
```

Add `breaker: Breaker` to `NatsEventPublisher`, construct it with `OPEN_DURATION` in `connect`, and add a `#[cfg(test)]`-free constructor override used by the blackhole test if a shorter window is needed there.

Gate `publish_ack` at the top:

```rust
        if !self.breaker.admit() {
            return Err(NatsPublisherError::Disconnected);
        }
        // `Pending` (a reconnect in flight) is allowed through: a reconnect typically completes
        // well inside the ack timeout, and short-circuiting it would turn every brief blip into
        // a breaker trip.
        if self.client.connection_state() == async_nats::connection::State::Disconnected {
            self.breaker.on_failure();
            return Err(NatsPublisherError::Disconnected);
        }
```

and wrap the publish result so both legs report to the breaker:

```rust
        let result = self.send_and_await_ack(ev).await;
        match &result {
            Ok(_) => self.breaker.on_success(),
            Err(_) => self.breaker.on_failure(),
        }
        result
```

Move the existing serialize + `send_publish` + ack body into a private `send_and_await_ack`.

**Verify `connection_state()` against the vendored 0.50 source** before relying on the enum path — this is the third open item in the spec's §7. If the method or the `State` enum differs, adapt; do not delete the gate.

- [ ] **Step 4: Run the tests to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --lib nats_publisher 2>&1 | tail -20
cargo nextest run -p paigasus-iam --test nats_publisher 2>&1 | tail -40
cargo clippy -p paigasus-iam -- -D warnings
```
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/adapters/events/nats_publisher.rs rs/crates/services/paigasus-iam/tests/nats_publisher.rs
git commit -m "feat(rs): short-circuit a down broker so a tick never holds row locks open (SMA-471)"
```

---

## Task 5: Metrics, priming, and the publish-failure alert

**Files:**
- Modify: `rs/crates/libs/paigasus-observability/src/names.rs`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/events/nats_publisher.rs`
- Modify: `rs/crates/services/paigasus-iam/src/main.rs` (descriptions only)
- Modify: `ops/observability/prometheus/rules/iam.rules.yml`
- Modify: `ops/observability/prometheus/rules/tests/iam.test.yml`

**Interfaces:**
- Consumes: `NatsEventPublisher` (Tasks 3–4).
- Produces: `names::IAM_NATS_PUBLISH_DUPLICATES_TOTAL`, `names::IAM_NATS_PUBLISH_DURATION_SECONDS`, `names::IAM_NATS_CONNECTED`.

- [ ] **Step 1: Add the metric names**

In `names.rs`, after the outbox dead-letter block:

```rust
// IAM NATS publisher (SMA-471)
/// Acks returned with `duplicate = true` — JetStream collapsing a relay redelivery. A rising
/// rate means publish acks are being lost and the relay is retrying. Primed at zero by
/// `NatsEventPublisher::connect` so the FIRST duplicate can satisfy an `increase() > 0` alert.
pub const IAM_NATS_PUBLISH_DUPLICATES_TOTAL: &str = "iam_nats_publish_duplicates_total";
/// Ack round-trip latency. On the critical path of a lock-holding relay transaction, so this is
/// a database-health metric as much as a broker one.
pub const IAM_NATS_PUBLISH_DURATION_SECONDS: &str = "iam_nats_publish_duration_seconds";
/// 1 when the client reports a live connection, 0 otherwise. Sampled by a BACKGROUND task, not
/// set inside `publish`: during a total outage every row eventually parks, `publish` stops being
/// called, and a publish-driven gauge would freeze exactly when it matters. Every replica sets
/// its own value, so aggregate `max by (job)` — never `sum`.
pub const IAM_NATS_CONNECTED: &str = "iam_nats_connected";
```

Add all three to the `ALL` array.

- [ ] **Step 2: Run the registry test**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-observability 2>&1 | tail -20
```
Expected: PASS (`all_names_are_unique_and_snake_case` covers the new consts).

- [ ] **Step 3: Emit the metrics**

In `nats_publisher.rs`:

- In `connect`, after the stream is verified, prime the counter and start the sampler:

```rust
        // Primed HERE, not in `describe_iam_metrics`: that runs only when `metrics.enabled`, and
        // a metrics-rs counter first appears at the value of its first increment — an unprimed
        // counter can never satisfy an `increase() > 0` alert on the FIRST duplicate. Same
        // constructor-priming pattern as `redis_conn::Breaker::with_durations`.
        counter!(names::IAM_NATS_PUBLISH_DUPLICATES_TOTAL).increment(0);

        // See IAM_NATS_CONNECTED's doc for why this cannot live inside `publish`.
        let probe = client.clone();
        tokio::spawn(async move {
            loop {
                let up = probe.connection_state() != async_nats::connection::State::Disconnected;
                gauge!(names::IAM_NATS_CONNECTED).set(if up { 1.0 } else { 0.0 });
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        });
```

- In `publish_ack`, time the round-trip and count duplicates:

```rust
        let started = std::time::Instant::now();
        let result = self.send_and_await_ack(ev).await;
        histogram!(names::IAM_NATS_PUBLISH_DURATION_SECONDS).record(started.elapsed().as_secs_f64());
        if let Ok(ack) = &result {
            if ack.duplicate {
                counter!(names::IAM_NATS_PUBLISH_DUPLICATES_TOTAL).increment(1);
            }
        }
```

- [ ] **Step 4: Describe the metrics at startup**

In `main.rs::describe_iam_metrics`, beside the existing outbox descriptions:

```rust
    describe_counter!(names::IAM_NATS_PUBLISH_DUPLICATES_TOTAL, "JetStream acks returned as duplicates — a relay redelivery collapsed by Nats-Msg-Id dedup.");
    describe_histogram!(names::IAM_NATS_PUBLISH_DURATION_SECONDS, "JetStream publish ack round-trip latency, inside the relay's lock-holding transaction.");
    describe_gauge!(names::IAM_NATS_CONNECTED, "1 when the NATS client reports a live connection, 0 otherwise. Per-replica: aggregate max by (job).");
```

Update that function's doc comment: "the 27 metric families" becomes "the 30 metric families", and add SMA-471 to its parenthetical list.

- [ ] **Step 5: Add the alert rule**

In `ops/observability/prometheus/rules/iam.rules.yml`, after `IamOutboxEventsParked`:

```yaml
      # SMA-471: with a real broker, publish failures are the earliest signal that delivery is
      # broken — earlier than parking (max_attempts × poll_interval) and earlier than backlog
      # age. The counter is already primed: relay.rs increments it by 0 on every tick, so the
      # series exists before the first failure and `increase()` can fire on it.
      - alert: IamOutboxPublishFailures
        expr: increase(iam_outbox_relay_publish_failures_total[5m]) > 0
        for: 5m
        labels: { severity: warning }
        annotations: { summary: "IAM outbox publishes are failing (broker unreachable or rejecting)" }
```

- [ ] **Step 6: Add the promtool fixture**

In `ops/observability/prometheus/rules/tests/iam.test.yml`, following the file's existing style. **A control series is required** — an all-firing fixture cannot distinguish `> 0` from `>= 0`:

```yaml
  # IamOutboxPublishFailures: increase(...[5m]) > 0 for 5m.
  # The flat control series is what makes this test discriminating: a rule written `>= 0` would
  # fire on it, and the negative expectation below would fail.
  - interval: 1m
    input_series:
      - series: 'iam_outbox_relay_publish_failures_total{job="iam", instance="a"}'
        values: '0 1 2 3 4 5 6 7 8 9 10 11'
      - series: 'iam_outbox_relay_publish_failures_total{job="iam", instance="control"}'
        values: '0+0x11'
    alert_rule_test:
      - eval_time: 2m
        alertname: IamOutboxPublishFailures
        exp_alerts: []
      - eval_time: 10m
        alertname: IamOutboxPublishFailures
        exp_alerts:
          - exp_labels: { severity: warning, job: "iam", instance: "a" }
            exp_annotations: { summary: "IAM outbox publishes are failing (broker unreachable or rejecting)" }
```

- [ ] **Step 7: Run the gates**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:promtool repo:observability-drift
cd rs && cargo nextest run -p paigasus-iam --lib 2>&1 | tail -20
cargo clippy --workspace -- -D warnings
```
Expected: all green. If `promtool` reports label mismatches on `exp_labels`, adjust to exactly the labels the expression preserves — do not delete the control series.

- [ ] **Step 8: Commit**

```bash
git add rs/crates/libs/paigasus-observability/src/names.rs \
        rs/crates/services/paigasus-iam/src/adapters/events/nats_publisher.rs \
        rs/crates/services/paigasus-iam/src/main.rs \
        ops/observability/prometheus/rules/
git commit -m "feat(rs): instrument the nats publisher and alert on outbox publish failures (SMA-471)"
```

---

## Task 6: Wire it into `main.rs` before any listener is spawned

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/main.rs`
- Modify: `rs/crates/services/paigasus-iam/tests/nats_publisher.rs`

**Interfaces:**
- Consumes: `PublisherConfig`/`PublisherBackend` (Task 1), `NatsEventPublisher::connect` (Task 3).
- Produces: nothing new; the relay is spawned with the selected `Arc<dyn EventPublisher>`.

- [ ] **Step 1: Write the failing relay integration test**

Append to `tests/nats_publisher.rs`. Use `tests/support/mod.rs::start_migrated_postgres` for the database, exactly as the other Postgres-backed suites do.

```rust
mod support;

/// End-to-end: real Postgres + real NATS through the real relay. Proves the adapter satisfies the
/// contract `OutboxRelay` actually depends on, not just the one its own unit tests assert.
#[tokio::test]
async fn the_relay_drains_rows_into_jetstream() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let Some((node, url)) = start_nats().await else { return };
    let publisher = NatsEventPublisher::connect(&cfg(&url)).await.unwrap();

    // Insert two unpublished outbox rows directly, the way `relay_pg.rs` does.
    let ids = [Uuid::from_u128(0xA1), Uuid::from_u128(0xA2)];
    for id in ids { support::insert_outbox_row(&db, id).await; }

    let relay = paigasus_iam::adapters::events::OutboxRelay::new(db.clone(), std::time::Duration::from_secs(5), 100, 60);
    let report = relay.tick(&publisher).await.unwrap();
    assert_eq!(report.drained, 2);
    assert_eq!(report.failures, 0);

    let js = jetstream::new(async_nats::connect(&url).await.unwrap());
    let info = js.get_stream("IAM_EVENTS").await.unwrap().info().await.unwrap().clone();
    assert_eq!(info.state.messages, 2);
    assert_eq!(support::unpublished_count(&db).await, 0, "published_at must be stamped");

    // A stopped broker leaves rows unpublished, with attempts and last_error recorded.
    node.stop().await.unwrap();
    for id in [Uuid::from_u128(0xB1)] { support::insert_outbox_row(&db, id).await; }
    let report = relay.tick(&publisher).await.unwrap();
    assert_eq!(report.drained, 1);
    assert_eq!(report.failures, 1);
    assert_eq!(support::unpublished_count(&db).await, 1);
    assert!(support::last_error(&db, Uuid::from_u128(0xB1)).await.is_some(), "last_error must be recorded");
}
```

Add `insert_outbox_row`, `unpublished_count` and `last_error` helpers to `tests/support/mod.rs` **only if equivalents do not already exist** — check `tests/relay_pg.rs` first and reuse or lift what it already does rather than writing a parallel set.

- [ ] **Step 2: Run to verify it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --test nats_publisher the_relay_drains 2>&1 | tail -30
```
Expected: failure — missing helpers, or `OutboxRelay` not reachable from the test crate.

- [ ] **Step 3: Move publisher construction ahead of the listeners**

In `main.rs`, immediately after `let state = AppState::new(db.clone(), &config).await?;` (`main.rs:60`) and the `db_for_*` clones, insert:

```rust
    // SMA-471: the outbox relay's delivery sink, built HERE — before the first `servers.spawn`
    // below — precisely so a broker that is unreachable at boot aborts startup with no port
    // bound. Constructing it inside the relay block (where it naturally belongs) would put the
    // `?` after the HTTP, metrics and gRPC listeners are already live, so an early return would
    // skip the graceful-shutdown `tx.send(())` and abort serving tasks mid-request.
    //
    // `validate()` rejects `relay_enabled = false` with the `nats` backend, so the disabled-relay
    // arm can only ever be the tracing publisher.
    let publisher: Arc<dyn EventPublisher> = match config.outbox.publisher.backend {
        PublisherBackend::Nats => Arc::new(NatsEventPublisher::connect(&config.outbox.publisher).await?),
        PublisherBackend::Tracing => Arc::new(TracingEventPublisher),
    };
```

Add the imports: `paigasus_iam::adapters::events::NatsEventPublisher`, `paigasus_iam::config::PublisherBackend`, `paigasus_iam_core::EventPublisher`.

Then in the existing relay block (`main.rs:198`), replace `.run(Arc::new(TracingEventPublisher), …)` with `.run(publisher, …)`, and update that block's comment — it currently calls `TracingEventPublisher` "a placeholder sink ahead of a real message-bus publisher (a later slice)", which is no longer true.

**Watch for a move error:** `publisher` is constructed outside the `if config.outbox.relay_enabled` block and moved into the spawned task inside it. If the compiler objects, clone the `Arc` at the use site.

`NatsPublisherError` must convert into whatever error type `main` returns (`anyhow::Error` via `?`); `thiserror` + `std::error::Error` gives that for free.

- [ ] **Step 4: Run the tests**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run --no-tests=pass -p paigasus-iam 2>&1 | tail -30
cargo clippy -p paigasus-iam -- -D warnings
```
Expected: PASS.

- [ ] **Step 5: Verify the boot behaviour by hand**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
# With no NATS running, a nats-backed config must fail BEFORE binding a port.
cd rs && IAM__OUTBOX__PUBLISHER__BACKEND=nats IAM__OUTBOX__PUBLISHER__URL=nats://127.0.0.1:14222 \
  cargo run -p paigasus-iam 2>&1 | tail -20
```
Expected: a startup error naming the NATS connection, and **no** "listening on" line for :8080/:9090. Use whatever env-var prefix/separator `figment` is actually configured with in `config.rs` — check `IamConfig::load` and adjust the variable names.

- [ ] **Step 6: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/main.rs rs/crates/services/paigasus-iam/tests/
git commit -m "feat(rs): select the outbox publisher at boot before any listener starts (SMA-471)"
```

---

## Task 7: Make dead-letter replay's dedup exposure measurable

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/application/dead_letters.rs`
- Modify: `rs/crates/libs/paigasus-observability/src/names.rs` (doc only)

**Interfaces:**
- Consumes: `PublisherConfig::duplicate_window_secs` is **not** plumbed here — see Step 3 for why the label is computed from a constant rather than config.
- Produces: `IAM_OUTBOX_DEAD_LETTERS_REPLAYED_TOTAL` gains a second label, `beyond_dedup_window` ∈ `{"true","false","unknown"}`.

- [ ] **Step 1: Write the failing test**

In `dead_letters.rs`'s test module (or `tests/dead_letters_pg.rs` if the counter is only observable there), using `metrics_util::debugging::DebuggingRecorder` as `redis_conn.rs`'s metric tests do:

```rust
/// SMA-471 D4: replaying a row parked longer ago than the dedup window republishes an event
/// JetStream may already hold. The label makes that exposure measurable instead of theoretical.
#[tokio::test]
async fn replaying_a_long_parked_row_is_labelled_beyond_the_dedup_window() {
    // parked_at = 3 hours ago, well past the 1-hour default window.
    let entry = replay_one_with_parked_at(Utc::now() - chrono::Duration::hours(3)).await;
    assert_eq!(label_for(&entry, "beyond_dedup_window"), "true");
}

#[tokio::test]
async fn replaying_a_freshly_parked_row_is_labelled_within_the_window() {
    let entry = replay_one_with_parked_at(Utc::now() - chrono::Duration::seconds(30)).await;
    assert_eq!(label_for(&entry, "beyond_dedup_window"), "false");
}
```

Shape the helpers to whatever the surrounding tests already use for driving `DeadLetterService::replay_one`; do not restructure them.

- [ ] **Step 2: Run to verify it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam dead_letter 2>&1 | tail -20
```
Expected: failure — the label does not exist.

- [ ] **Step 3: Add the label**

In `replay_one` (currently `dead_letters.rs:102`):

```rust
        // SMA-471 D4: replay keeps the row id, so a row parked longer ago than JetStream's
        // duplicate window republishes an event the stream may already hold. Nothing prevents
        // that — refusing the replay would remove the operator's only recovery tool — so it is
        // measured instead.
        //
        // Compared against a CONSTANT rather than `outbox.publisher.duplicate_window_secs`:
        // this application service has no publisher config, and threading it in would couple the
        // dead-letter surface to a broker it otherwise knows nothing about. The constant matches
        // the shipped default; a deployment that widens the window sees conservative labelling
        // (some `true`s that were in fact still deduped), which is the safe direction to be wrong.
        let beyond = match entry.parked_at {
            Some(at) => {
                if Utc::now().signed_duration_since(at).num_seconds() > i64::from(ASSUMED_DEDUP_WINDOW_SECS) { "true" } else { "false" }
            }
            None => "unknown",
        };
        counter!(names::IAM_OUTBOX_DEAD_LETTERS_REPLAYED_TOTAL, "scope" => "one", "beyond_dedup_window" => beyond).increment(1);
```

with, near the top of the file:

```rust
/// The `[outbox.publisher].duplicate_window_secs` default, mirrored here for D4's replay
/// labelling. See the call site for why this is a constant and not plumbed config.
const ASSUMED_DEDUP_WINDOW_SECS: u32 = 3_600;
```

In `replay_matching` (`dead_letters.rs:133`), the store returns only a row count, so per-row `parked_at` is unavailable:

```rust
        // Bulk replay returns a COUNT, not rows, so per-row `parked_at` is unavailable and the
        // window question cannot be answered per event. `"unknown"` rather than guessing — and
        // rather than changing `replay_matching_in`'s signature, which is out of scope here.
        counter!(names::IAM_OUTBOX_DEAD_LETTERS_REPLAYED_TOTAL, "scope" => "bulk", "beyond_dedup_window" => "unknown").increment(replayed);
```

Update `IAM_OUTBOX_DEAD_LETTERS_REPLAYED_TOTAL`'s doc in `names.rs` to document both labels and the closed value set.

Add a rustdoc note on `replay_one`/`replay_matching` stating the at-least-once consequence for operators.

- [ ] **Step 4: Run the tests**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run --no-tests=pass -p paigasus-iam 2>&1 | tail -20
cargo clippy --workspace -- -D warnings
```
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/application/dead_letters.rs rs/crates/libs/paigasus-observability/src/names.rs
git commit -m "feat(rs): label dead-letter replays that fall outside the dedup window (SMA-471)"
```

---

## Task 8: Documentation and the full CI gate

**Files:**
- Modify: `docs/dev-setup.md`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/events/mod.rs` (module doc)

- [ ] **Step 1: Document the local setup**

Add a short subsection to `docs/dev-setup.md` under its gotchas:

```markdown
## NATS (optional — outbox publisher, SMA-471)

The outbox publisher defaults to `backend = "tracing"` and needs nothing. To run the real
JetStream sink locally:

```bash
nats-server -js          # the whole setup; JetStream must be enabled or stream ensure fails
```

then set:

```toml
[outbox.publisher]
backend = "nats"
url     = "nats://127.0.0.1:4222"
```

The service creates and verifies the `IAM_EVENTS` stream at boot, and refuses to start if an
existing stream's `duplicate_window`, storage or subjects are weaker than configured.
Integration tests need no local server — they start their own container.
```

- [ ] **Step 2: Refresh the events module doc**

`adapters/events/mod.rs`'s doc still describes the tracing publisher as standing in "ahead of a real message-bus publisher (a later slice)" and says wiring is "a later task (B9)". Rewrite it to describe the current state: relay, CloudEvents envelope, tracing publisher (default), NATS publisher (production), with a pointer to ADR-0016.

- [ ] **Step 3: Run the complete CI gate**

Per-project Moon tasks do **not** run the repo-level gates, and this PR adds workspace deps, proto-adjacent nothing, ops rules, and new metric names — so run what CI runs:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
git fetch --no-tags origin "+refs/heads/main:refs/remotes/origin/main"
moon ci :build :test :lint :fmt :deny :machete :typecheck :breaking :affected-smoke \
  :parity-corpus-drift :wasm-getrandom-free :promtool :observability-drift \
  :release-parity :release-parity-py :release-parity-ts \
  --base origin/main --include-relations
```

Expected: all green. If Moon reports an unattributed failure, diagnose it with:

```bash
jq '.actions[] | select(.status=="failed")' .moon/cache/ciReport.json
```

`:wasm-getrandom-free` is the one to watch: it proves `async-nats` has not reached the wasm binding's dependency tree.

- [ ] **Step 4: Verify the acceptance criteria**

Walk the spec's §9 list (13 items) and confirm each against the code and a test. Any item without a test that would fail if the behaviour were removed is not done.

- [ ] **Step 5: Commit**

```bash
git add docs/dev-setup.md rs/crates/services/paigasus-iam/src/adapters/events/mod.rs
git commit -m "docs(rs): document the nats outbox publisher and its local setup (SMA-471)"
```

---

## Self-review notes

**Spec coverage.** D1→Task 3 (dep + client). D2→Task 3 (double await + §4.3.9 negative test). D3→Tasks 2–3 (`render_id` shared by `id` and `Nats-Msg-Id`; dedup + restart tests). D4→Task 7. D5→Task 3 (subject = wire string). D6→Task 2. D7→Task 3 (`verify_stream`) + Task 6 (ordering). D8→Tasks 1, 3 (`max_age`). D9→Task 1. D10→Task 1 (floor, backend-gated). D11→Task 4. D12→Task 1 (default) + Task 6 (selection). D13→no code by design. Spec §5 →Task 2 (no-secrets test) + Task 1 (`url` redaction). Spec §3.4→Task 5. Spec §6 docs→Tasks 0, 8.

**Known unknowns, each with a step that resolves rather than skips it.** `async-nats` 0.50's concrete error types and `connection_state()` signature (Task 3 Step 4, Task 4 Step 3 — read the vendored source); whether `testcontainers-modules`' `Nats` needs `-js` (Task 3 Step 3); whether JetStream dedup survives a restart (Task 3 Step 2's test — if it fails, narrow the claim in the spec/ADR rather than deleting the test); the exact figment env-var spelling (Task 6 Step 5).

**Deliberately deferred**, recorded in the spec's §8: per-row commit in the relay, `PublishError::Permanent`, the post-commit latency nudge, dashboard panels, `/readyz` NATS health, and a dev-stack compose file.

<!-- moon-diagnosis:superseded -->
> **Superseded (SMA-597).** The `ciReport.json` diagnosis advice above does not work as written:
> there is no action-level `exitCode` key, and the file carries no stdout/stderr at all. The
> measured procedure is in CLAUDE.md between the `moon-diagnosis` markers. This document is left
> otherwise unedited as a record of what was believed when it was written.
