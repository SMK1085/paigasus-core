# SMA-786: M0 — agent-host skeleton: crates, `agents/v1` contracts, local run

Linear: SMA-786 (project "Paigasus Agent Host"). Blocked by SMA-785 (ADR-0028, merged in PR 493).
Blocks SMA-787 (event inbox), SMA-789 (memory store), SMA-794 (egress proxy).

Status: revised after the second adversarial challenge (2026-11-20). Waiting for Gate 1.

**ADR-0028 source in this repo.** Every "ADR (x)" reference below points to section 4 of
`docs/superpowers/specs/2026-11-18-sma-785-agent-host-adr-design.md`. That spec holds the full ADR
text, and the Notion page names it as its source. Read it for the exact fields, states and rules.

## 1. Intent

The agent host gets an empty service that can be deployed. Then the primitives of the later issues
(inbox, timers, memory, outbox) go into a real crate with a real database, real health probes and a
real image. They do not have to make that home first.

ADR-0028 (Notion, Proposed) fixes the domain model. This issue builds only the frame around it.

## 2. Goals, non-goals, anti-goals

**Goals**

- G1. Two new crates: the pure domain crate `paigasus-agent-host-core` and the hexagonal service
  `paigasus-agent-host`.
- G2. The service boots against Postgres, runs its migrations under a lock, and answers `/healthz`,
  `/readyz` and `/metrics`.
- G3. A developer starts it locally with one command: `moon run paigasus-agent-host-rs:dev`.
- G4. CI builds the service image and smoke-tests it against Postgres, on both architectures.
- G5. `contracts/proto/paigasus/agents/v1` exists, with generated Rust, Python and TypeScript code.
- G6. Typed PRNs for `agent`, `inbox` and `run` under the service `agents`.
- G7. `paigasus-helikon` is a crates.io dependency of the service, with one real use. So the
  supply-chain gates (`:deny`, `:osv`, `:machete`) and the CI disk budget see its dependency tree
  now and not in SMA-790. M0 brings in only the Helikon core and the Tokio runner. These bring in
  no TLS stack. The provider crates use `reqwest` 0.13 (Helikon `Cargo.toml:64`) with rustls and
  `aws-lc-rs` (Helikon `Cargo.toml:38-40` and the `rustls` feature in each provider
  `Cargo.toml`). That conflict arrives in SMA-790. The new `deny.toml` bans (§6.12) will red it
  there. This is a known risk for SMA-790.
- G8. The full gate graph (root `CLAUDE.md`, "Before you push") is green.

**Non-goals**

- No inbox ingress, worker, scheduler, outbox, dispatcher or memory adapter. These are SMA-787 to
  SMA-795. M0 defines their ports only, and the ports are provisional (§6.2).
- No Helm chart entry and no kind install. Sven decided "chart later" (2026-11-19). The chart
  needs a console per zone, and the agents console (SMA-799) does not exist yet.
- No release chain and no image publish. `ci/images/chains.toml`, `CHAIN_APPROVALS` and
  `release.yml` do not change (Sven, 2026-11-19, §6.10). The chart issue adds the chain.
- No `GET /v1/service-info` route. `service_info.proto:34-39` forbids a service-info route that is
  less protected than the other routes, and M0 has no authentication. SMA-787 adds the route
  behind authentication. The service slug is fixed now as `agents` (§6.7).
- No CloudEvents library crate. It moves to SMA-787, which owns ingress and is the first code that
  parses an envelope.
- No `InboxEvent` proto message. It moves to the first issue that writes or reads it (§6.4).
- No change to `ts/tooling/dev-stack.ts`. SMA-799 can add the agent host to it.
- No `Capability` enum value in `service_info.proto`. The chart issue adds it with the slug
  `agents`.
- No kernel grammar change. The PRN parser has no registry of services or resource types.

**Anti-goals** (the design fails, even if it works, when one of these is true)

- A1. The core crate depends on SeaORM, axum, Helikon or any IO crate.
- A2. Production code calls `Migrator::up` without the advisory lock.
- A3. A port, a value object or a proto message contradicts ADR-0028, so that a later issue must
  break it.
- A4. A gate is bypassed or a check is weakened to make this branch green.
- A5. An `agents/v1` message carries a trust-bearing field that ADR-0028 (e) says the host sets
  (`source`, `id`, `actorprn`, `threadid`), or repeats a CloudEvents envelope attribute.
- A6. The agent host can change or break the migration state of another service in the same
  database.
- A7. Anything in M0 can publish an image or a crate.

## 3. Constraints

- Rust edition 2024, `rust-version = 1.95`, `[lints] workspace = true`, `publish = false`, an
  SPDX header on every source file.
- Both crates start at version `0.0.0`. The IAM and gateway services moved to `0.1.0` only at
  their first image release (`rs/release-plz.toml:205-212`).
- `rs/Cargo.toml` `members` uses literal paths only (SMA-663, gate A11).
- Request extractors use the house `Envelope*` types (`repo:http-extractor-envelope`).
- Docker-gated tests use `paigasus_test_docker::start_or_skip` (`repo:iam-docker-policy-single-site`).
- Logging only through `paigasus-logging`. Metrics only through `paigasus-observability`.
- Generated proto code is committed (ADR-0004). `buf lint` is `STANDARD`. `buf breaking` uses the
  FILE category (`contracts/buf.yaml:15-33`): a message on `main` can never be deleted, and a
  field can be deleted only with its number reserved. So every message in M0 is permanent.
- `paigasus-proto` is a published family (crates.io, PyPI, npm). The next release ships the new
  package. The builder reads rule C1 in `.github/CLAUDE.md` and picks the commit type of the
  contracts commit accordingly.
- The deferred tool calls (SMA-803) and the turn-boundary checkpoint (SMA-804) are merged on
  Helikon `main` and ship in the same release as the jiff fix (§6.9). M0 does not use them. The
  owner issues (SMA-790, SMA-795) use them.
- The workspace must not link `aws-lc-rs`, `aws-lc-sys`, `openssl-sys` or `native-tls`
  (`rs/crates/services/paigasus-gateway/Cargo.toml:37-44`). M0 makes this a gate: `rs/deny.toml`
  gets `[bans] deny` entries for these four crates (§6.12).

## 4. Approaches considered (for the local run)

| | Approach | Trade-off |
|---|---|---|
| **A (chosen)** | A Moon `dev` task in the service crate starts a throwaway Postgres container and `cargo run` | One container. Same pattern as the IAM dev story (`cargo run` plus env). It does not exercise the image; the CI image smoke does. |
| B | A Docker Compose file in `ops/agent-host/` with Postgres and the built image | Proves the image locally, but every code change needs a slow image build. A second local-stack style that the chart replaces later. |
| C | Extend `ts/tooling/dev-stack.ts` | That tool runs `next dev` for the consoles against fakes. A Rust service and a Postgres mix two jobs. Better done by SMA-799. |

Sven picked A on 2026-11-19.

## 5. What Sven decided (2026-11-19)

- M0 proves a local run against Postgres and an image that CI builds. The chart comes later.
- The local run is approach A.
- The image gets a build-and-smoke path with **no release chain** (§6.10). This replaces an
  earlier "full chain now" decision. The adversarial challenge showed that a chain without a chart
  image block turns `repo:helm-render` row 8a red (`ci/helm-render/helm_render.py:819-857`), and
  that one approval of any other release could publish the empty image
  (`ci/actionlint/release_guard.py:73-81`).
- The crate name stays `paigasus-agent-host`. The service slug, the future chart zone and the PRN
  service are `agents`. When the chart issue adds the chain (key `agent-host`, from
  `release_plan.py`), it adds a key-to-zone map (`agent-host` -> `agents`) in `helm_render.py`.
- The three Helikon changes (SMA-803, SMA-804, SMA-805, Helikon ADR-15) and the jiff fix
  (SMA-818) are merged on Helikon `main`. The release commit SMK1085/paigasus-helikon#350 is also
  on `main`. crates.io has `paigasus-helikon` 0.6.12 and `paigasus-helikon-core` 0.6.0, with
  `jiff ^0.2.46` (sparse index, checked 2026-11-20). M0 depends on that release (§6.9).

## 6. Design

### 6.1 Crate layout

```
rs/crates/libs/paigasus-agent-host-core/     pure domain, version 0.0.0
  Cargo.toml  moon.yml (id paigasus-agent-host-core-rs, layer library)
  src/lib.rs
  src/ids.rs          PRN, UUID and string newtypes (§6.3)
  src/inbox.rs        Inbox, InboxEvent, EventStatus
  src/timer.rs        Timer, TimerKind
  src/run.rs          Run, RunStatus, RunOutcome, RunHandle, LeaseToken
  src/outbox.rs       OutboxMessage, OutboxPayload
  src/deferral.rs     PendingToolCall, PendingToolCallState
  src/agent.rs        AgentDefinition, ResolvedAgent
  src/ports.rs        the ports (§6.2)
  src/error.rs        DomainError

rs/crates/services/paigasus-agent-host/      service, version 0.0.0
  Cargo.toml  moon.yml (id paigasus-agent-host-rs, layer application)
  CHANGELOG.md  README.md  agent-host.toml.example
  scripts/dev.sh
  src/main.rs         composition root (§6.5)
  src/lib.rs
  src/config.rs       figment config (§6.6)
  src/adapters/boot.rs        boot router while migrating (copied from IAM)
  src/adapters/http/          router, health, error envelope
  src/adapters/persistence/   migration/, migration_lock.rs, entities/
  src/adapters/helikon/       AgentDefinition -> Helikon LlmAgent (§6.9)
  tests/              boot_pg.rs, migration_isolation_pg.rs, docker_preflight.rs, guard tests
```

The template for the image and the simple boot is `paigasus-gateway`. The template for the boot
router, SeaORM, migrations and Postgres tests is `paigasus-iam`.

### 6.2 Domain crate: value objects and ports

The domain crate depends only on `paigasus-kernel`, `uuid`, `jiff`, `thiserror`, `async-trait`,
`serde` and `serde_json`. This is the `paigasus-iam-core` set
(`rs/crates/libs/paigasus-iam-core/Cargo.toml`), with `jiff` in place of `chrono`.

**Time type (the coordinator's call).** The core crate uses `jiff::Timestamp` for every time
value. The public API of Helikon uses `jiff`, and `rs/deny.toml:32-38` plans to move our crates off
`chrono` (SMA-784). The persistence adapter converts at the SeaORM boundary.

**Guard for anti-goal A1.** A unit test in the core crate uses `include_str!` on its own
`Cargo.toml` and asserts the dependency set above (pattern:
`iam/src/adapters/persistence/migration_lock.rs:255-262`).

**Value objects.** Each one holds the fields that M0 needs to state the ADR shape. Later issues add
fields. They do not rename or re-type these.

- `Inbox`: `InboxId`, `AgentId`, `PrincipalId`, `created_at`. `Inbox::new` rejects an agent whose
  org differs from the org of the inbox id, and returns a typed `DomainError`. The `org_id` column
  of the table (§6.8) is the org of both.
- `InboxEvent`: `EventId`, `InboxId`, the CloudEvents `source`, `ce_id` (the CloudEvents `id`, a
  string) and `ce_type`, optional `ThreadId`, `received_at`, status (`Pending | Done | Dead`),
  `attempts`, `not_before`, optional `claimed_run: RunId`, `data: serde_json::Value`. `EventId` is
  the host's own UUID. `ce_id` is the dedup key part from the sender.
- `Timer`: `TimerId`, `InboxId`, kind (`FollowUp { due_at } | Schedule { cron, tz }`), optional
  `ThreadId`, `fire_seq`. `Timer::fire_event_id()` returns `timer:<timer_id>:<fire_seq>` (ADR (l)).
- `Run`: `RunId`, `InboxId`, `LeaseToken`, the claimed `EventId`s, `RunStatus`, optional
  `final_output`.
- `RunStatus`: `Running | Completed | Deferred | Stopped | Failed`. There is no `Discarded`
  status. ADR (b) says that nothing commits for a lost lease, so no code writes it. A discarded run
  is the `Err(CommitError::LeaseLost)` result in memory, and its row stays `Running` until the
  claimable predicate frees its events (ADR (b), claim step 4).
- `OutboxMessage`: `OutboxMessageId`, `InboxId`, `ThreadId`, `RunId`, `payload: OutboxPayload`,
  `created_at`. `OutboxPayload` is `#[non_exhaustive]`. It has one variant now: `Message { text }`.
  SMA-795 adds `ApprovalRequest { .. }`. That is an additive variant, not a re-type.
- `PendingToolCall`: `DeferralId`, `CallId`, tool name, the raw args (`serde_json::Value`), the
  prompt, `handoff_path: Vec<String>`, `InboxId`, `RunId`, `expires_at` and
  `PendingToolCallState`. The merged Helikon API fixes the first six fields
  (`paigasus-helikon-core/src/deferral.rs:12-19`, §6.9). The host adds the inbox and the run that
  deferred the call, because ADR (g) resumes that inbox's thread and a stale run must not resolve
  it. The host also adds `expires_at` and the state. The args hash, the canonical args text and the approval wire
  format are not in M0 (§6.4, §7).
- `PendingToolCallState`: `Open -> Approved | Denied | Expired | Superseded`, then each of those
  four `-> Resolved` (ADR (g), "States"). Methods do the transitions. Every other transition
  returns an error.
- `AgentDefinition`: `AgentId`, name, model id, instructions, tool names. No memory scope (SMA-789)
  and no mid-run policy (ADR (h)) yet.
- `ResolvedAgent { definition: AgentDefinition, principal: PrincipalId }`.

**Ports.** All are `async_trait`, `Send + Sync`. **All ports are provisional.** M0 fixes each
port's name and responsibility. The owner issue may change its methods and signatures. It must
keep the responsibility and agree with ADR-0028 (anti-goal A3).

| Port | Responsibility | Owner | Used | M0 sketch |
|---|---|---|---|---|
| `InboxRepository` | inbox rows and event inserts with dedup | SMA-787 | Outside a run: ingress and console | `get(InboxId)`, `insert_event(InboxEvent) -> Inserted \| Duplicate { payload_matches: bool }` |
| `TimerRepository` | timer rows | SMA-788 | Outside a run: scheduler and console. A run adds or cancels a timer only in the buffer of the `RunCommitter`, which writes it in the one commit | `insert(Timer)`, `cancel(TimerId)` |
| `Scheduler` | turn due timers into inbox events (ADR (l)) | SMA-788 | Outside a run | `fire_due(limit) -> usize` |
| `MemoryStore` | the agent's directory-like memory | SMA-789 | Inside a run: reads go direct, writes go to the buffer and the `RunCommitter` writes them in the one commit. Outside a run: console reads and writes | `read`, `write`, `list` on a path |
| `Outbox` | the read side of the durable outbound messages | SMA-792 | Outside a run: the dispatcher. A run adds a message only in the buffer of the `RunCommitter` | `next_batch(limit) -> Vec<OutboxMessage>`, `mark_sent(OutboxMessageId)` |
| `AgentResolver` | inbox to agent definition and principal | SMA-790 | Inside a run: read only, at the start | `resolve(InboxId) -> ResolvedAgent` |
| `RunCommitter` | the one commit transaction of ADR (b) | SMA-790 | Inside a run: the only writer | `commit(RunHandle, RunOutcome) -> Result<Committed, CommitError>` |

Supporting types:

- `LeaseToken(i64)`: ADR (c) stores the token as `BIGINT`.
- `RunHandle { inbox: InboxId, run: RunId, token: LeaseToken }`. The commit locks
  `inbox_lease WHERE inbox_id = $1` and updates `run WHERE id = $run` (ADR (b) steps 1 and 2). A
  token alone finds neither row.
- `RunOutcome`: `Completed { final_output: Option<String> } | Deferred { calls:
  Vec<PendingToolCall> } | Stopped { reason: String } | Failed { error: String }` (ADR (b),
  "Outcomes"). `Failed` is a deterministic failure only. There is no `Transient` case. A transient
  failure does not commit: the run rolls back, and the lease expires or the worker releases it.
  SMA-790 designs the release call and the backoff.
- `Committed { run: RunId }`. `CommitError::{LeaseLost, Store(String)}`. A database error is
  `Store`, which ADR (b) classifies as transient. It is never mapped to `LeaseLost`. SMA-790 must
  add an "outcome unknown" case to `CommitError` (see §7), so that a retry that finds zero rows is
  not counted as a lost lease (ADR (o) metric). M0 keeps the port provisional.
- How the committer receives the buffered changes (session events, outbox messages, timers,
  deferrals, memory writes, folded events: ADR (b) steps 3 to 8) is SMA-790's design. M0 does not
  fix it.
- How a transient failure releases the lease and writes its `not_before` backoff after the
  rollback is also SMA-790's design.

M0 has no adapter for any port. The ports are public items of a library, so no dead-code warning
fires.

### 6.3 PRNs and IDs

The kernel already parses `prn:pgs:agents::<org>:agent/<uuid>`. The cedar mapping already gives
`Pgs::Agents::Agent`, `Pgs::Agents::Inbox` and `Pgs::Agents::Run`. So the kernel does not change.
The issue text says "in `paigasus-kernel`". This design puts the typed wrappers in the core crate,
because the kernel holds only the grammar, and the IAM types follow the same rule
(`paigasus-iam-core/src/value.rs`).

PRN newtypes in `paigasus-agent-host-core::ids`:

- `AgentId`, `InboxId`, `RunId`: each wraps a `Prn` built with
  `Prn::build("agents", "", Some(org), "<type>", uuid)`. `new(org, uuid)`, `parse(&str)`,
  `as_prn()`, `uuid()`, `org()`. `parse` rejects a different service or type, a non-empty region,
  and an empty org.
- `PrincipalId`: an IAM principal PRN, `prn:pgs:iam:::principal/<uuid>`, with no region and **no
  org** (`paigasus-iam-core/src/value.rs:127-132`). `parse` rejects any other service or type, and
  a non-empty region or org. It does not depend on `paigasus-iam-core`, so the core crate stays
  free of IAM. The org of an inbox comes from the inbox PRN, not from the principal.

UUID newtypes (ADR-0028 names no PRN for them): `ThreadId`, `EventId`, `TimerId`,
`OutboxMessageId`.

String newtypes: `CallId` and `DeferralId`. The model assigns the `CallId`. Helikon makes the
`DeferralId` (UUID v4 text, `paigasus-helikon-core/src/deferral.rs:92`), and it is the key that
resumes a deferred call. Each `parse` rejects an empty string, a NUL character, leading or trailing
whitespace, and input over the maximum length (pattern: `ExternalSubject::parse`,
`paigasus-iam-core/src/value.rs:96-103`). The maximum is 256 characters for `CallId` and 64
characters for `DeferralId`. `DeferralId::parse` does not require a UUID form, because the Helikon
API fixes only a string.

Note for SMA-787: ADR (e) says that ingress drops a `threadid` that does not belong to the inbox.
A `threadid` that is not a UUID fails `ThreadId::parse`. Ingress then drops it and starts a fresh
thread. It does not reject the event.

### 6.4 Contracts: `agents/v1`

One file, `contracts/proto/paigasus/agents/v1/agents.proto`, `package paigasus.agents.v1`. No
`service`, so no tonic output. The file comment says:

- The package is `paigasus.agents.v1`. Field numbers and names are permanent. New fields can be
  added.
- JSON `data` uses the proto3 JSON lowerCamelCase field names.
- Each message is the `data` of one CloudEvents event (ADR (e)). The comment gives each message's
  direction and CloudEvents `type`.
- **Envelope rule.** A message never repeats a CloudEvents envelope attribute (`id`, `source`,
  `type`, `time`, `subject`) or an extension (`threadid`, `actorprn`, `correlationid`,
  `schemaversion`). The envelope is the only place for those values. The host sets the
  trust-bearing ones (anti-goal A5).

One message. Field names follow the house rule: a field that holds a PRN is `<thing>_prn`
(`contracts/proto/paigasus/iam/v1/iam.proto:32`).

| Message | Direction | CloudEvents `type` | Fields |
|---|---|---|---|
| `OutboxMessage` | out | `agents.outbox.message` | `run_prn`, `text` |

- **Envelope values for a host-emitted event.** `source` is the fixed URN `urn:paigasus:agents`.
  IAM does the same with `urn:paigasus:iam` (`iam/src/config.rs:808`). `subject` is the inbox PRN.
  IAM sets `subject` in the same way (`iam/src/adapters/events/cloud_event.rs:56`). So the
  message has no `inbox_prn`. `run_prn` is not an envelope attribute, so it stays in the message.
- **`ApprovalRequest`, `ApprovalDecision` and the `APPROVAL_DECISION_KIND_*` enum are not in M0.**
  They move to SMA-795, as `InboxEvent` did. The merged Helikon API (SMA-803) does not match the
  earlier draft. Its resume key is `deferral_id`, a UUID v4 that Helikon makes
  (`paigasus-helikon-core/src/deferral.rs:92`). The `call_id` is assigned by the model. The agent
  is `handoff_path: Vec<String>`. In addition, three questions need one design together with the
  published API: the raw args, the secret redaction, and the RFC 8785 rule for large numbers
  (§7, "Handed to owner issues"). `buf breaking` makes a published message permanent, so M0 must
  not publish a message that is probably wrong.
- `InboxEvent` is not in M0. ADR (e) says that an inbox event is a CloudEvent whose `data` is
  either an `agents/v1` message or a raw external payload. A separate `InboxEvent` message would
  repeat the envelope. The first issue that needs a proto for an inbox event (SMA-787 or SMA-799)
  designs it.
- The Rust bindings have no proto3 JSON mapping (`contracts/buf.gen.yaml:8-38` has no serde or
  pbjson option). ADR (e) puts the proto3 JSON of a message in `data`. SMA-787 decides the Rust
  JSON strategy. M0 uses only string fields, so it needs no well-known type.

Registration points: the Rust module tree in `rs/crates/libs/paigasus-proto/src/lib.rs`, the TS
entry `src/agents.ts` with a `./agents` subpath export (§6.12), the generated trees in all three
languages (`moon run contracts:generate --force`), and one smoke test per language (pattern: the
`health` smoke tests). No `Auditable` derive, because no message embeds an `audit` field.

### 6.5 Service boot sequence

`main.rs` copies the IAM shape (`rs/crates/services/paigasus-iam/src/main.rs`):

1. `paigasus_observability::health::dispatch(args)`: `serve` or `healthcheck --path <path>` (the
   image has no shell, so the binary probes itself; `paigasus-observability/src/health.rs:19`,
   `:51-58`).
2. `Config::load()`, then `validate()`.
3. `paigasus_logging::init("paigasus-agent-host", &config.log_level)`.
4. If `metrics.enabled`, `paigasus_observability::init("paigasus-agent-host")` (the gateway form).
5. Register the shutdown signal once, for both boot phases (`iam/src/main.rs:256-279`).
6. Connect to Postgres first (`iam/src/main.rs:118`). Then bind the HTTP listener (and the metrics
   listener when `metrics.addr` is set; `iam/src/main.rs:134`). This is the order of IAM. While the
   migrations run, `/healthz` answers 200, `/readyz` answers 503 `{"status":"migrating"}`, and
   other paths answer 503 `service-migrating` in the error envelope (IAM `adapters/boot.rs`). A
   shutdown signal during the migration stops the boot.
7. When metrics are enabled, start the Prometheus upkeep task: `handle.run_upkeep()` every 5
   seconds, on the shared shutdown watch (`iam/src/main.rs:197-227`). Start it with the binds, not
   after the migration, so that histogram decay also runs during a long migration.
8. Run `migrate_under_lock(db, config.migration.lock_wait())`.
9. Swap in the real router.
10. Graceful shutdown with the bounded drains (`iam/src/main.rs:374-382`).

A guard test asserts that `main.rs` calls `migrate_under_lock`, as IAM's
`the_composition_root_still_migrates_under_the_lock` does (anti-goal A2). The migration-lock
module is copied from IAM, with its own advisory-lock key and the migration table of §6.8. The
advisory-lock key must differ from the two IAM keys, 5_580_559 and 5_580_467
(`iam/src/adapters/persistence/migration_lock.rs:24-26`). A unit test asserts this.

### 6.6 Config

figment: `Serialized::defaults` < `Toml::file("agent-host.toml")` <
`Env::prefixed("AGENT_HOST_").split("__")`.

| Key | Default | Note |
|---|---|---|
| `http_addr` | `0.0.0.0:8086` | The default is for the container. IAM uses 8080/9090/9091, the gateway 8088. No other file in the repo uses 8086 (checked 2026-11-19). `scripts/dev.sh` sets `AGENT_HOST_HTTP_ADDR=127.0.0.1:<port>` (§6.11) |
| `database_url` | none (required) | redacted in `Debug` |
| `log_level` | `info` | |
| `metrics.enabled` | `true` | |
| `metrics.addr` | unset | unset merges `/metrics` on `http_addr`; when set, it must use a different PORT from `http_addr`, and `validate()` checks this (IAM: `iam/src/config.rs:1654-1659`) |
| `migration.lock_wait_secs` | the IAM default | range 1 to 3600, as IAM (`iam/src/config.rs:1663-1666`) |

### 6.7 HTTP surface

- `GET /healthz`: 200 always.
- `GET /readyz`: 200 when a database ping succeeds, else 503. Outside the correlation and metrics
  layers.
- `GET /metrics`: Prometheus text.
- Any other path: 404 in IAM's error envelope `{"error":{"code","message"}}`
  (`iam/src/adapters/boot.rs:225-228`). The gateway uses a different, OpenAI-shaped envelope; M0
  does not copy it.
- The HTTP metrics layer uses the prefix `agent_host` (a Prometheus name cannot contain `-`). It
  makes `agent_host_http_requests_total` and two more families
  (`rs/crates/libs/paigasus-observability/src/http.rs:49-57`). The three names go into
  `paigasus_observability::names` with `describe_*` lines, as the gateway does
  (`gateway/src/main.rs:223-234`), so `repo:observability-drift` sees them. The drift test keeps
  only the prefixes `iam_` and `gateway_` today (`paigasus-observability/tests/drift.rs:42`). So M0
  adds `agent_host_` to that filter and to the doc comments (§6.12).
- The service slug is `agents`. M0 uses it nowhere on the wire, because `/v1/service-info` moves to
  SMA-787. The slug is fixed here so that SMA-787, the chart and the console agree.
- **Error registry.** The boot router spells `service-migrating`, and the 404 handler spells
  `not-found`. Both are declared codes (`contracts/proto/paigasus/common/v1/error.proto`). So
  `repo:error-code-single-site` needs an `emits` row in `ci/error-registry/check.py` for each file
  that spells one, with a membership test that calls `ErrorReason::from_wire_reason`
  (IAM precedent: `iam/src/adapters/boot.rs:204`, `:308-318`). The service therefore depends on
  `paigasus-proto` (at least as a dev-dependency), with its Moon edges (§6.12).

### 6.8 Migrations and database isolation

The agent host has its own migration table: `MigratorTrait::migration_table_name()` returns
`agent_host_migrations`, and the copied `applied_count` reads the same name (IAM reads the fixed
name `public.seaql_migrations`, `iam/src/adapters/persistence/migration_lock.rs:98-114`). Reason:
SeaORM returns an error for every applied version that a migrator does not know
(`sea-orm-migration-2.0.4/src/migrator/exec.rs:74-83`). With a shared `seaql_migrations` table,
the first agent-host migration in a shared database would stop the next IAM boot (anti-goal A6).

The copied `applied_count` uses the unqualified name `to_regclass('agent_host_migrations')`. The
IAM copy reads `public.seaql_migrations` (`migration_lock.rs:102`). An unqualified name follows the
`search_path` of the connection.

The table names (`inbox`, later `run`, `thread`, `timer`) are generic. So the supported
deployment gives the agent host **its own database**. A separate schema in a shared database is
**not supported** in M0. The README and `agent-host.toml.example` state this.

`m0001_create_inbox`. ADR (d) says an inbox is created explicitly, one per (agent, principal).

```sql
CREATE TABLE inbox (
  id            uuid PRIMARY KEY,
  org_id        uuid NOT NULL,
  agent_id      uuid NOT NULL,
  principal_id  uuid NOT NULL,
  created_at    timestamptz NOT NULL DEFAULT now(),
  UNIQUE (agent_id, principal_id)
);
```

The `org_id` column is the org of the inbox PRN and of the agent PRN (`Inbox::new` checks that
they match, §6.2). The table stores the principal UUID, not the PRN text, so a case or format variant of the same
principal cannot get past the unique constraint. The PRN is rebuilt with `PrincipalId::from_uuid`.

Each later issue adds its own tables (`inbox_event` and `inbox_lease` in SMA-787, `timer` in
SMA-788, and so on).

Postgres tests (Docker-gated, `*_pg.rs`):

- `boot_pg.rs`: boot, `m0001` applied once, a second boot applies nothing, the unique constraint
  holds, `/readyz` 200. A 503 `service-migrating` answer while the lock is held by another
  connection. Lock contention, copied from IAM's test.
- `migration_isolation_pg.rs`: a database that already holds `public.seaql_migrations` with a
  version the agent host does not know. The agent host boots, and that table is unchanged.

### 6.9 Helikon: the first consumer

- Dependency: `paigasus-helikon = { version = "=0.6.12", default-features = false, features =
  ["runtime-tokio"] }` in the service only (anti-goal A1). The exact pin has a comment in
  `Cargo.toml`: a facade bump needs the re-check of this section, because the facade re-exports the
  core, and one facade patch release already moved the core from 0.5 to 0.6. The release
  SMK1085/paigasus-helikon#350 published facade 0.6.12 with `paigasus-helikon-core` 0.6.0. The
  facade requires core `^0.6.0`, and the core requires `jiff ^0.2.46` (sparse index, checked
  2026-11-20).
- The release is on crates.io, so the dependency does not block the work. The builder never uses
  a git or path dependency on Helikon.
- Facts verified on 2026-11-18 against 0.6.11 and core 0.5.21. They were checked again on
  2026-11-20 against Helikon `main` commit b2989b72 (SMA-803 is commit 8fcceedf). The builder
  re-checks each fact against the published release, because core 0.6.0 has breaking changes (the
  `AgentEvent` variants moved). The release commit on Helikon `main` is 7ed91e7e (#350). The facade has the feature
  `runtime-tokio` and re-exports the core as `paigasus_helikon::core`. `LlmAgent::builder::<Ctx>()`
  exists (`paigasus-helikon-core/src/agent.rs:377`). The builder is typestate: `.name(..)` and
  `.model(..)` are required, `.instructions(..)` is optional (`agent_builder.rs`). The core has no
  exported mock model, so the test defines a minimal type that implements the core `Model` trait.
- The merged deferred-call API (SMA-803), which SMA-790 and SMA-795 use: `PendingToolCall` holds
  `deferral_id: String`, `call_id`, `tool`, `args: serde_json::Value`, `prompt` and
  `handoff_path: Vec<String>` (`paigasus-helikon-core/src/deferral.rs:12-19`). Helikon makes the
  `deferral_id` as UUID v4 text (`deferral.rs:92`).
- One real use: `adapters/helikon::build_agent<M: Model + 'static>(def: &AgentDefinition, model: M)
  -> LlmAgent<(), M, String>`. `LlmAgent` has the generics `<Ctx, M, T = String>`
  (`agent.rs:312`). M0 picks `Ctx = ()`. The bound `Ctx: Send + Sync + 'static` holds for `()`, and
  `LlmAgentBuilder::build` returns `LlmAgent<Ctx, M, T>` (`agent_builder.rs:356`). The builder
  checks that this signature compiles against the published release. The function maps the name
  and the instructions onto `LlmAgent::builder::<()>()`. Tool names map later (SMA-790), because M0
  has no tool registry. A unit test builds an agent from a definition with the test model and
  checks the name and the instructions.
- If the published API differs from these facts, the builder stops and reports. The builder does
  not wrap Helikon in a speculative abstraction.

**Lockfile effect. The builder measures it and reports it in the PR.**

- The new core requires `jiff ^0.2.46` (SMA-818 removed the old `=0.2.28` pin). `rs/Cargo.lock`
  holds jiff 0.2.37, so the add moves the whole workspace (IAM and the gateway included) UP to
  jiff 0.2.46 or later. No package may move down. jiff 1.x does not satisfy `^0.2.46`, so the lock
  can hold a second, 1.x copy only if another crate asks for it.
- The builder diffs `rs/Cargo.lock`, lists every package that changes version (all upward), runs `:osv` and
  `:deny`, and confirms that the facade resolves to the new core. If any package moves down, the
  builder stops and reports.
- No dependabot `ignore` for jiff. The earlier plan for one is withdrawn.
- After the add, `rs/Cargo.lock` holds no `aws-lc-rs`, `aws-lc-sys`, `openssl-sys` or
  `native-tls`. If it does, the builder stops and reports.
- A new license or advisory gets a `deny.toml` entry with a reason. It never gets a broad wildcard
  (anti-goal A4).

### 6.10 Container image: build and smoke, no release chain

- The existing parameterised `rs/Dockerfile` with `--build-arg BIN=paigasus-agent-host`. No new
  Dockerfile.
- The path is `ci/images/run.sh build agent-host`, then `ci/images/run.sh smoke agent-host`. It
  needs `crate_for` to answer `agent-host` (`paigasus-agent-host`, `run.sh:66-72`) and one
  `kind_for_key` row, `agent-host` as `cargo` (`run.sh:77-83`). It also needs an `agent-host)` case
  in `smoke()` and a `cleanup` entry for its containers. A bare `build`, a bare `smoke` and `all`
  keep their `iam` and `gateway` lists. M0 adds **no** `build-oci`, `load-oci` or `rehearse` support
  for `agent-host`. So no OCI archive of the agent host exists, and anti-goal A7 holds by
  construction: a publish path starts from an OCI archive that `build-oci` wrote, and the agent
  host has none. The image version label comes from the crate `Cargo.toml` (`0.0.0`). The chart issue
  adds the chain and the archive support.
- `smoke_agent_host`, after `smoke_iam`: its own Postgres container name (not the shared
  `$PG_NAME`), `POSTGRES_16_ALPINE_DIGEST`, reached by hostname, then the image with
  `AGENT_HOST_DATABASE_URL`; expect `/readyz` 200 and the in-image `healthcheck` exit 0. Negative
  row, as the gateway smoke does (`run.sh:1065-1079`): stop Postgres, then expect `/readyz` 503 and
  `healthcheck --path /readyz` exit exactly 1.
- `.github/workflows/images.yml`: a separate step, after the step "Smoke each service on its own"
  (`:166-169`), runs `ci/images/run.sh build agent-host` and then `ci/images/run.sh smoke
  agent-host`. The step "Build both images as OCI archives" (`:150-153`), the step "Load each
  archive and check its identity" (`:160-164`), the SBOM loop (`:179-187`) and every publish path
  do not change, because the image is never published. The publish paths never see the key.
- `release.yml`, `chains.toml`, `release_guard.py` and `helm_render.py` do not change.
- **Budget.** The images job builds every service in one leg, with a 60-minute timeout, on a disk
  that has overflowed before (`images.yml:95-97`). The builder runs
  `gh workflow run images.yml --ref <branch>` (a PR runs amd64 only, `images.yml:101`), on both
  architectures, and records `df -h /` and the job time before and after. Stop rule: if the leg
  takes more than 45 minutes or free disk falls below 3 GB, move the agent host to its own matrix
  job.

### 6.11 Local run: `moon run paigasus-agent-host-rs:dev`

A `dev` task in the service `moon.yml`. It never runs in CI. The worked example for its shape is
the gateway `e2e-bin` task (`rs/crates/services/paigasus-gateway/moon.yml:70-86`). The builder
uses the moon 2.5.3 task options for a persistent, uncached, local-only task, and checks the
result with `moon tasks`. The builder does not use the moon 1.x key `local: true` unless moon
2.5.3 accepts it.

Inputs, so that `repo:affected-smoke` A8 and A10 stay green (`cargo_moon_parity.py:194-198`,
`:256-259`, `:569-576`): `@group(sources)`, `Cargo.toml`, `@group(upstreams)`, `/rs/Cargo.lock`,
`/rs/.cargo/config.toml`, and the script itself.

The script `scripts/dev.sh`:

1. Labels its Postgres container with the PID of the owning script (for example
   `paigasus.dev=agent-host` and `paigasus.dev.pid=<pid>`). It first removes only the containers
   with the label whose PID is no longer alive. A second developer run on the same machine keeps
   its own container. An `EXIT` trap does not run on SIGKILL.
2. Starts `postgres:16-alpine` at the pinned digest on a free host port bound to `127.0.0.1`, with
   a unique name, and waits for `pg_isready`. The script holds its own copy of the digest from
   `POSTGRES_16_ALPINE_DIGEST` in `ci/images/run.sh`, with a comment that names that source. It
   does not source `ci/images/run.sh`. No drift gate: the script is local only, and a stale digest
   still starts a correct Postgres 16 (my call).
3. Runs `cargo run --locked -p paigasus-agent-host` with `AGENT_HOST_DATABASE_URL` and
   `AGENT_HOST_HTTP_ADDR=127.0.0.1:<port>` set. The default `0.0.0.0:8086` of §6.6 is for the
   container only.
4. Removes the container on exit (an `EXIT` trap).
5. A `--check` mode runs `cargo build --locked` first, before its timeout starts. It then exits 0
   after `/readyz` answers 200, and 1 on a timeout. CI does not run it. It lets a developer, or a
   later issue, check that the script still works.

The script runs under `/bin/bash` 3.2 and bash 5: no here-string, no `mapfile`, no associative
array, no pipe into a reader that can exit early (`repo:actionlint` check 13), and
`PROTO_REPORTER=text` exported if it captures proto output. The crate README documents it.

### 6.12 Gate and registration points

The builder registers the two crates in every place below. To find a miss, the builder also greps
the whole repo for `paigasus-gateway` and `paigasus-gateway-core` and checks each hit.

- `rs/Cargo.toml`: `members` (literal) and `[workspace.dependencies]` (the core crate with
  `version = "0.0.0"`).
- Both `moon.yml` files: `dependsOn` for every `{ workspace = true }` in-tree dependency
  (`paigasus-kernel-rs`, `paigasus-proto-rs` and its derive crate, `paigasus-observability-rs`,
  `paigasus-logging-rs`, `paigasus-test-docker-rs`, the core crate), `fileGroups.upstreams` with
  the full transitive closure (strict equality, A6), `test: deps: ['^:build']`, and
  `options.mutex: 'heavy-integration'` on the service test task.
- `ci/affected-graph/run.sh`: the `lockfile->all-lint` set (every crate), the `kernel->bindings`
  set, the contracts and derive-crate cases (`:299`, `:311`), and every other case whose closure
  the crates join.
- `rs/release-plz.toml`: a `[[package]]` with `release = false`, `publish = false`,
  `git_release_enable = false` for each crate. The comment at `:216-217` says "all eleven" crates
  carry `publish = false`. With the two new crates the count is thirteen. M0 updates the comment.
- `rs/deny.toml`: the `[bans]` table (`:68-70`) holds `multiple-versions = "warn"` and `wildcards =
  "deny"`, and no `deny` list. M0 adds a `deny` list to it with one entry for each of `aws-lc-rs`,
  `aws-lc-sys`, `openssl-sys` and `native-tls`, in the form `{ crate = "<name>", reason = "<why>"
  }`. The reason names the TLS constraint of §3. This makes the constraint a gate. The builder
  checks that `cargo deny check bans` passes before and after the Helikon dependency. The
  provider crates of Helikon will red it in SMA-790 (G7).
- `rs/crates/libs/paigasus-observability/tests/drift.rs`: add the prefix `agent_host_` to the
  filter (`:42`) and to the doc comments (`:14`, `:36`). Without this change the gate does not see
  the agent host metric names.
- `ts/packages/paigasus-proto`: add `src/agents.ts` and an `./agents` subpath export in
  `package.json`, following `./iam` (`package.json:11-14`, `src/iam.ts`, and the test in
  `src/index.test.ts:44-60`). The root barrel does not export `iam`, and it does not export
  `agents`. The Python package has no per-service module (only the generated tree under
  `src/paigasus_proto/generated/paigasus/<service>/v1` and one smoke test per topic in `tests/`),
  so Python needs only the generated tree and a smoke test.
- `rs/.config/nextest.toml`: two override blocks for `package(=paigasus-agent-host)`, copied from
  the gateway blocks (`:124-136`): `docker_preflight` with `retries = 1`, and the `*_pg` binaries
  in the `docker-containers` group with the shared retry budget.
- `repo:iam-docker-policy-single-site` and `repo:redis-connect-single-site`: add the new service
  `src` and `tests` directories. No new `repo:*` gate, so the target list in the root `CLAUDE.md`
  and in `ci.yml` does not change.
- `repo:error-code-single-site`: the `emits` rows of §6.7.
- `repo:observability-drift`: the three metric names of §6.7, and the filter change below.
- `repo:machete`: no allowlist entry. Every dependency has a use.
- `repo:deny`, `repo:osv`: §6.9.
- `repo:http-extractor-envelope`: scans `services/*/src/adapters/http/**`, so it covers the new
  service with no change. M0 has no body extractor.
- `repo:publish-metadata` and `repo:version-lockstep` do not apply to `publish = false` crates.
- `repo:helm-render`: no change, because no chain is added.
- `ci/images/run.sh`, `images.yml`: §6.10.
- CODEOWNERS is Moon-generated with one owner line. No change.

## 7. Left to the builder

- Exact field types inside the domain value objects, within §6.2.
- The advisory-lock key value.
- The README text and the `agent-host.toml.example` comments.
- The minimal test `Model` for §6.9.
- The moon 2.5.3 option keys for the `dev` task (§6.11).
- Small-scale names (module and function names) that this document does not fix.

### Handed to owner issues

**SMA-795 (approvals).** It owns `ApprovalRequest`, `ApprovalDecision` and the
`APPROVAL_DECISION_KIND_*` enum (§6.4). Its design follows these rules:

- (a) The unique key of a deferred call is `deferral_id`, not `call_id` (see the ADR corrections
  below).
- (b) RFC 8785 serializes a number as an IEEE 754 double. So a number that does not survive a
  round trip through `f64` cannot be bound by the hash. Rule: such a call cannot be deferred. The
  host denies it and states the reason.
- (c) Helikon redacts secrets in the `PendingToolCall` that it returns. The raw args exist only in
  `SessionEvent::ToolDeferred`. An event that the host fans out on NATS must not carry the raw
  args. It carries `args_hash` and a redacted display copy. The hash covers the raw args, which
  only the host holds.

**SMA-790 (run and commit).**

- `CommitError` needs an "outcome unknown" case. A retry after a lost COMMIT acknowledgement can
  find zero rows. The host must not count that as a lost lease (ADR (o) metric).
- ADR (b) lists a transient outcome. The M0 `RunOutcome` has none (§6.2). A transient failure
  rolls back, and SMA-790 designs the lease release and the backoff.
- The provider crates of Helikon use `reqwest` 0.13 with rustls and `aws-lc-rs`. The new
  `deny.toml` bans (§6.12) will red the first build that adds them. This is a known risk (G7).

### Corrections to ADR-0028

The coordinator updates the ADR text after Gate 1. The builder does not change the ADR.

- ADR (g): the unique key of a pending tool call must be `deferral_id`, not `call_id`. Helikon
  makes the `deferral_id` (UUID v4). The model assigns the `call_id`. This needs a correction note
  on ADR-0028.
- ADR (d): the ADR says that the org of an inbox comes from the principal PRN. An IAM principal PRN
  has no org (`paigasus-iam-core/src/value.rs:127-132`). The org comes from the inbox PRN (§6.3).

### Coordinator's calls (answered)

- **Merge order.** The Helikon release is on crates.io (§6.9), so nothing blocks the merge.
- **Commit type.** To publish `OutboxMessage { run_prn, text }` before SMA-787 is acceptable. The
  builder follows rule C1 (§3).
- **Shared schema.** A separate schema in a shared database is not supported in M0 (§6.8).
- **Time type.** The core crate uses `jiff` (§6.2).

## 8. Done when

- `moon run paigasus-agent-host-rs:dev` boots the service locally, `/readyz` returns 200, and the
  `inbox` table exists. `scripts/dev.sh --check` exits 0.
- `boot_pg.rs` and `migration_isolation_pg.rs` (Docker-gated) pass in CI.
- `images.yml`, run by `workflow_dispatch` on the branch, builds the image and `smoke_agent_host`
  passes on amd64 and arm64. The PR records the job time and free disk before and after.
- `agents/v1` code is generated and committed in all three languages, and the codegen-drift step
  is green.
- The PR lists every `rs/Cargo.lock` version change from §6.9 (all upward).
- The unit-level checks pass: the PRN parse rejects; the `PendingToolCallState` transitions; the
  `Timer::fire_event_id` format; the two error-registry membership tests (§6.7); the
  `migrate_under_lock` guard test (§6.5); the Helikon adapter test (§6.9); the per-language proto
  smoke tests (§6.4); the advisory-lock key test (§6.5); the A1 `Cargo.toml` test (§6.2); and the
  `Inbox::new` org check (§6.2).
- The full gate graph from the root `CLAUDE.md` is green, run as CI runs it.
