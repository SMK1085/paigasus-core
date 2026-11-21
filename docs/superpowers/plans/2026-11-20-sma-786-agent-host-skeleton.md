# SMA-786 Agent-Host Skeleton Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the pure domain crate `paigasus-agent-host-core`, the hexagonal service
`paigasus-agent-host` (Postgres boot, migrations under a lock, `/healthz` `/readyz` `/metrics`), the
`agents/v1` contracts, a CI image build and smoke, and a one-command local run.

**Architecture:** The core crate holds value objects, typed IDs and provisional ports, and has no
IO dependency. The service copies the IAM boot shape (boot router while migrating, advisory-lock
migrations) and the gateway image and metrics shape. Helikon enters as a pinned crates.io
dependency with one real use. No release chain, no chart and no publish path.

**Tech Stack:** Rust 2024 (rust-version 1.95), axum 0.8, SeaORM 2 / sea-orm-migration 2.0.4,
figment 0.10, jiff 0.2, `paigasus-helikon` =0.6.12 (core 0.6.0), buf (prost, betterproto2,
protobuf-es), Moon 2.5.3, Docker (testcontainers, `ci/images/run.sh`).

**Spec:** `docs/superpowers/specs/2026-11-19-sma-786-agent-host-skeleton-design.md` (approved,
commit cfe73984). ADR text: `docs/superpowers/specs/2026-11-18-sma-785-agent-host-adr-design.md` §4.
Read both before you start a task. "§x" below means a section of the SMA-786 spec.

## Global Constraints

- Both new crates: edition 2024, `rust-version = 1.95` (workspace keys), `[lints] workspace = true`,
  `publish = false`, version `0.0.0`.
- Every new source file opens with `// SPDX-License-Identifier: Apache-2.0` (`#` for Python and
  shell, after the shebang line).
- `rs/Cargo.toml` `members` entries are literal paths. No `*`, `?` or `[` (A11).
- `warnings = deny`: never add an item in one task that a later task uses. Each task adds only
  what it uses or tests. A dependency that no code uses reds `repo:machete`.
- The core crate depends only on `paigasus-kernel`, `uuid`, `jiff`, `thiserror`, `async-trait`,
  `serde`, `serde_json` (anti-goal A1).
- Every time value in the core crate is `jiff::Timestamp`.
- Production code never calls `Migrator::up` directly. It calls `migrate_under_lock` (A2).
- Migration table name: `agent_host_migrations`. Advisory-lock key: `5_580_786` (not 5_580_559,
  not 5_580_467).
- HTTP metric prefix `agent_host`. Error envelope `{"error":{"code":"…","message":"…"}}`.
- Default `http_addr` `0.0.0.0:8086`. Env prefix `AGENT_HOST_`, nested split `__`. Config file
  `agent-host.toml`.
- Helikon: `paigasus-helikon = { version = "=0.6.12", default-features = false, features =
  ["runtime-tokio"] }`, in the service only. Never a git or path dependency.
- The workspace must not link `aws-lc-rs`, `aws-lc-sys`, `openssl-sys` or `native-tls`.
- No change to `ci/images/chains.toml`, `CHAIN_APPROVALS`, `release.yml`,
  `ci/actionlint/release_guard.py`, `ci/helm-render/helm_render.py`, the chart, or
  `ts/tooling/dev-stack.ts`. No `build-oci`, `load-oci` or `rehearse` support for `agent-host`.
- No new `repo:*` gate. The `ci-targets` command in the root `CLAUDE.md` and `ci.yml` stay as they are.
- Docker-gated tests start containers only through `paigasus_test_docker::start_or_skip`.
- Logging only through `paigasus-logging`. Metrics only through `paigasus-observability`.
- Commits: conventional, workspace scope, body lines of 100 characters or fewer, no line in the
  body that starts with `#` followed by a number, never `--no-verify`. End every message with
  the line `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Do not write the file name of moon's CI report (the JSON file under `.moon/cache`) in any new
  tracked file. `repo:actionlint` check 12 reds a file that names it without a marker.
- Shell: run every Rust command from `rs/`. For Moon, first run
  `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` and `export PROTO_REPORTER=text`.
- Do not install software on the host. Use `docker run ubuntu:24.04` if a tool is missing.

## Review Focus

1. **A multi-byte ID at the length limit.** A `CallId` of 256 two-byte characters is valid; 257 is
   not. The limit counts characters, not bytes. Test in Task 1.
2. **`metrics.addr` on the HTTP port but a different IP** (`127.0.0.1:8086` against
   `0.0.0.0:8086`). The listeners collide on the port, so `validate()` must reject it. Test in
   Task 5.
3. **SIGTERM while the boot waits for the migration lock.** The process must exit promptly with
   status 0. It must not hang until the lock wait ends. Test in Task 7.
4. **Two spellings of one principal PRN** (upper-case UUID). The parse must never yield a
   different principal than `PrincipalId::from_uuid`. Test in Task 1.
5. **Two `dev.sh` runs at the same time on one machine.** The second run must not remove the
   first run's container. Check in Task 10.

---

## File map

Core crate `rs/crates/libs/paigasus-agent-host-core/`:
`Cargo.toml`, `moon.yml`, `src/lib.rs`, `src/error.rs`, `src/ids.rs`, `src/inbox.rs`,
`src/timer.rs`, `src/run.rs`, `src/outbox.rs`, `src/deferral.rs`, `src/agent.rs`, `src/ports.rs`.

Service crate `rs/crates/services/paigasus-agent-host/`:
`Cargo.toml`, `moon.yml`, `CHANGELOG.md`, `README.md`, `agent-host.toml.example`,
`scripts/dev.sh`, `src/main.rs`, `src/lib.rs`, `src/config.rs`, `src/adapters/mod.rs`,
`src/adapters/boot.rs`, `src/adapters/http/mod.rs`, `src/adapters/http/error.rs`,
`src/adapters/persistence/mod.rs`, `src/adapters/persistence/migration/mod.rs`,
`src/adapters/persistence/migration/m0001_create_inbox.rs`,
`src/adapters/persistence/migration_lock.rs`, `src/adapters/helikon.rs`,
`tests/support/mod.rs`, `tests/boot_pg.rs`, `tests/migration_isolation_pg.rs`,
`tests/docker_preflight.rs`.

Contracts: `contracts/proto/paigasus/agents/v1/agents.proto`; generated
`rs/crates/libs/paigasus-proto/src/generated/paigasus/agents/v1/paigasus.agents.v1.rs`,
`py/packages/paigasus-proto/src/paigasus_proto/generated/paigasus/agents/v1/__init__.py`,
`ts/packages/paigasus-proto/src/generated/paigasus/agents/v1/agents_pb.ts`; registration
`rs/crates/libs/paigasus-proto/src/lib.rs`, `ts/packages/paigasus-proto/src/agents.ts`,
`ts/packages/paigasus-proto/package.json`; smoke tests
`rs/crates/libs/paigasus-proto/tests/agents_smoke.rs`,
`py/packages/paigasus-proto/tests/test_agents_smoke.py`,
`ts/packages/paigasus-proto/src/agents.test.ts`.

Modified elsewhere: `rs/Cargo.toml`, `rs/Cargo.lock`, `rs/release-plz.toml`, `rs/deny.toml`,
`rs/.config/nextest.toml`, `rs/crates/libs/paigasus-observability/src/names.rs`,
`rs/crates/libs/paigasus-observability/tests/drift.rs`, `ci/affected-graph/run.sh`,
`ci/error-registry/check.py`, `moon.yml` (root), `ci/images/run.sh`,
`.github/workflows/images.yml`.

**Decision on `src/adapters/persistence/entities/` (§6.1).** M0 does not create it. No M0 code
reads or writes the `inbox` table through SeaORM. An entity would need a time type at the SeaORM
boundary (the workspace `sea-orm` has `with-chrono`, not jiff), and that choice belongs to the
first adapter (SMA-787). The tests read the table with raw SQL.

---

### Task 1: Core crate, typed IDs and the A1 guard

**Files:**
- Create: `rs/crates/libs/paigasus-agent-host-core/{Cargo.toml,moon.yml,src/lib.rs,src/error.rs,src/ids.rs}`
- Modify: `rs/Cargo.toml` (`members`, `[workspace.dependencies]`), `rs/Cargo.lock`,
  `rs/release-plz.toml`, `ci/affected-graph/run.sh`

**Interfaces:**
- Produces (module `ids`; all derive `Debug, Clone, PartialEq, Eq, Hash`):
  - `AgentId`, `InboxId`, `RunId`: `new(org: Uuid, id: Uuid) -> Self`,
    `parse(raw: &str) -> Result<Self, DomainError>`, `as_prn(&self) -> &Prn`, `uuid(&self) -> Uuid`,
    `org(&self) -> Uuid`. Built with `Prn::build("agents", "", Some(org), "<agent|inbox|run>", id)`
    (`rs/crates/libs/paigasus-kernel/src/resource_name.rs`).
  - `PrincipalId`: `from_uuid(Uuid) -> Self`, `parse(&str) -> Result<Self, DomainError>`,
    `as_prn(&self) -> &Prn`, `uuid(&self) -> Uuid`. Form `prn:pgs:iam:::principal/<uuid>`.
  - `ThreadId`, `EventId`, `TimerId`, `OutboxMessageId` (also `Copy`): `from_uuid(Uuid) -> Self`,
    `uuid(&self) -> Uuid`, `parse(&str) -> Result<Self, DomainError>`.
  - `CallId`, `DeferralId`: `parse(&str) -> Result<Self, DomainError>`, `as_str(&self) -> &str`.
    `pub const CALL_ID_MAX_CHARS: usize = 256; pub const DEFERRAL_ID_MAX_CHARS: usize = 64;`
- Produces (module `error`): `#[derive(Debug, thiserror::Error, PartialEq, Eq)] pub enum DomainError`
  with at least `InvalidId { kind: &'static str }`. Tasks 2 adds variants.

- [ ] **Step 1: Write the crate files and register the crate.**
  - `Cargo.toml`: copy the `[package]` block of `rs/crates/libs/paigasus-iam-core/Cargo.toml:1-8`
    with the new name. `[dependencies]`: `paigasus-kernel`, `uuid`, `thiserror`, all
    `{ workspace = true }`. No other dependency yet.
  - `moon.yml`: copy `rs/crates/libs/paigasus-iam-core/moon.yml:1-22` (id
    `paigasus-agent-host-core-rs`, layer `library`, `dependsOn: ['paigasus-kernel-rs']`, the two
    kernel `upstreams` lines, `build` and `test` with `deps: ['^:build']`). Do not copy the
    SMA-774 test inputs.
  - `rs/Cargo.toml`: add `"crates/libs/paigasus-agent-host-core",` as the first `crates/libs/`
    entry of `members` (alphabetical). Add
    `paigasus-agent-host-core = { path = "crates/libs/paigasus-agent-host-core", version = "0.0.0" }`
    next to `paigasus-gateway-core` (`rs/Cargo.toml:204`), with the same comment style.
  - `rs/release-plz.toml`: add a `[[package]]` block `name = "paigasus-agent-host-core"`,
    `release = false`, `publish = false`, `git_release_enable = false` (the form at
    `rs/release-plz.toml:259-263`). Change "all eleven" (`:216-217`) and "first of these eleven"
    (`:221-222`) to "twelve".

- [ ] **Step 2: Write the failing tests in `src/ids.rs` and `src/lib.rs`.**

```rust
// ids.rs tests (u(n) = Uuid::from_u128(n))
agent_id_round_trips_through_its_prn          // AgentId::parse(&AgentId::new(u(1),u(2)).as_prn().canonical()) == Ok(same); org()==u(1); uuid()==u(2)
agent_id_prn_has_the_agents_service_and_no_region // canonical() == "prn:pgs:agents::<u(1)>:agent/<u(2)>"
id_parse_rejects_another_service                 // AgentId::parse("prn:pgs:iam::<org>:agent/<id>") is Err
id_parse_rejects_another_type                    // InboxId::parse(&AgentId::new(..).as_prn().canonical()) is Err
id_parse_rejects_a_region                        // "prn:pgs:agents:eu-1:<org>:run/<id>" is Err
id_parse_rejects_an_empty_org                    // "prn:pgs:agents:::inbox/<id>" is Err
principal_id_rejects_an_org_or_a_region          // "prn:pgs:iam::<org>:principal/<id>" and "prn:pgs:iam:eu-1::principal/<id>" are Err
principal_id_rejects_another_type                // "prn:pgs:iam:::user/<id>" is Err
principal_id_spellings_never_name_another_principal // Review Focus 4: for the upper-case form of from_uuid(u)'s canonical text: if parse is Ok, it == from_uuid(u)
thread_id_parse_rejects_a_non_uuid               // ThreadId::parse("not-a-uuid") is Err (§6.3 note for SMA-787)
call_id_rejects_empty_nul_and_outer_whitespace   // "", "a\0b", " a", "a " all Err
call_id_limit_counts_characters_not_bytes        // Review Focus 1: "é".repeat(256) Ok, "é".repeat(257) Err
deferral_id_limit_is_64_characters               // "a".repeat(64) Ok, 65 Err; a non-UUID "x" is Ok
```

```rust
// lib.rs test: the A1 guard (pattern: include_str! guard at
// rs/crates/services/paigasus-iam/src/adapters/persistence/migration_lock.rs:255-262)
#[test]
fn the_core_crate_depends_on_no_io_crate() {
    // Parse the [dependencies] table of include_str!("../Cargo.toml"): the key of each entry line.
    // Assert every key is in ALLOWED and that [dependencies] is not empty.
    const ALLOWED: &[&str] = &["paigasus-kernel", "uuid", "jiff", "thiserror", "async-trait", "serde", "serde_json"];
}
```

The guard asserts a SUBSET, not equality. The dependencies arrive over Tasks 1 to 3, and
`repo:machete` forbids a dependency with no use. After Task 3 the set equals `ALLOWED`.

- [ ] **Step 3: Run the tests and see them fail.**
  Run: `cd rs && cargo nextest run --locked -p paigasus-agent-host-core`
  Expected: compile errors for the missing types. (Run `cargo build -p paigasus-agent-host-core`
  once without `--locked` first: the new crate changes `rs/Cargo.lock`.)

- [ ] **Step 4: Implement `ids.rs` and `error.rs`.** The string newtypes copy the checks of
  `ExternalSubject::parse` (`rs/crates/libs/paigasus-iam-core/src/value.rs:96-103`). The PRN
  newtypes check `service()`, `resource_type()`, `region().is_empty()` and `org().is_some()` on
  the parsed `Prn`. `new` uses `expect` on `Prn::build`, as `PrincipalId::from_uuid` does
  (`value.rs:127-132`). Use a declarative macro for the three PRN newtypes if you want one.

- [ ] **Step 5: Run the tests and see them pass.**
  Run: `cd rs && cargo nextest run --locked -p paigasus-agent-host-core && cargo clippy --locked -p paigasus-agent-host-core --all-targets -- -D warnings && cargo fmt --check`
  Expected: all pass, no warning.

- [ ] **Step 6: Update the affected-graph expectations.**
  Run: `/bin/bash ci/affected-graph/run.sh` (system bash 3.2, not Homebrew bash).
  Expected first: FAIL rows. Add exactly the new IDs that each FAIL row names. Expected rows:
  `lockfile->all-lint` and `lockfile->all-lint-ci` (`paigasus-agent-host-core-rs:lint`),
  `kernel->bindings` (`paigasus-agent-host-core-rs`), `kernel->consumer-tasks`
  (`paigasus-agent-host-core-rs:build`, `:test`, `:lint`). Keep the list order style of each row.
  Run again. Expected: no FAIL row. If a row needs an ID that is not a new-crate ID, stop and report.

- [ ] **Step 7: Commit.**

```bash
git add rs/crates/libs/paigasus-agent-host-core rs/Cargo.toml rs/Cargo.lock rs/release-plz.toml ci/affected-graph/run.sh
git commit -m "feat(rs): add the paigasus-agent-host-core crate with typed agent IDs" -m "SMA-786 §6.1, §6.3. PRN, UUID and string newtypes, DomainError, and the A1 dependency guard." -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: Core value objects

**Files:**
- Create: `rs/crates/libs/paigasus-agent-host-core/src/{inbox.rs,timer.rs,run.rs,outbox.rs,deferral.rs,agent.rs}`
- Modify: `src/lib.rs`, `src/error.rs`, the crate `Cargo.toml`, `rs/Cargo.toml`
  (`[workspace.dependencies]`), `rs/Cargo.lock`

**Interfaces:**
- Consumes: Task 1 IDs and `DomainError`.
- Produces (field names are fixed; later issues add fields, never rename):
  - `inbox`: `Inbox { id: InboxId, agent: AgentId, principal: PrincipalId, created_at: Timestamp }`
    with `Inbox::new(id, agent, principal, created_at) -> Result<Inbox, DomainError>` and
    `org(&self) -> Uuid`. `EventStatus { Pending, Done, Dead }`. `InboxEvent { id: EventId,
    inbox: InboxId, source: String, ce_id: String, ce_type: String, thread: Option<ThreadId>,
    received_at: Timestamp, status: EventStatus, attempts: u32, not_before: Timestamp,
    claimed_run: Option<RunId>, data: serde_json::Value }`.
  - `timer`: `TimerKind { FollowUp { due_at: Timestamp }, Schedule { cron: String, tz: String } }`,
    `Timer { id: TimerId, inbox: InboxId, kind: TimerKind, thread: Option<ThreadId>, fire_seq: u64 }`,
    `Timer::fire_event_id(&self) -> String`.
  - `run`: `LeaseToken(pub i64)`, `RunStatus { Running, Completed, Deferred, Stopped, Failed }`,
    `Run { id: RunId, inbox: InboxId, token: LeaseToken, events: Vec<EventId>, status: RunStatus,
    final_output: Option<String> }`, `RunHandle { inbox: InboxId, run: RunId, token: LeaseToken }`,
    `RunOutcome { Completed { final_output: Option<String> }, Deferred { calls: Vec<PendingToolCall> },
    Stopped { reason: String }, Failed { error: String } }`, `Committed { run: RunId }`,
    `#[derive(thiserror::Error)] CommitError { LeaseLost, Store(String) }`.
  - `outbox`: `#[non_exhaustive] OutboxPayload { Message { text: String } }` with
    `#[derive(Serialize, Deserialize)] #[serde(tag = "kind", rename_all = "snake_case")]`;
    `OutboxMessage { id: OutboxMessageId, inbox: InboxId, thread: ThreadId, run: RunId,
    payload: OutboxPayload, created_at: Timestamp }`.
  - `deferral`: `#[derive(Serialize, Deserialize)] #[serde(rename_all = "snake_case")]
    PendingToolCallState { Open, Approved, Denied, Expired, Superseded, Resolved }` with
    `approve`, `deny`, `expire`, `supersede`, `resolve`, each `(self) -> Result<Self, DomainError>`.
    `PendingToolCall { deferral_id: DeferralId, call_id: CallId, tool: String,
    args: serde_json::Value, prompt: String, handoff_path: Vec<String>, inbox: InboxId,
    run: RunId, expires_at: Timestamp, state: PendingToolCallState }`.
  - `agent`: `AgentDefinition { id: AgentId, name: String, model_id: String,
    instructions: String, tool_names: Vec<String> }`, `ResolvedAgent { definition:
    AgentDefinition, principal: PrincipalId }`.
  - `DomainError` gains `OrgMismatch { inbox_org: Uuid, agent_org: Uuid }` and
    `InvalidTransition { from: PendingToolCallState, to: PendingToolCallState }`.
  - No `Discarded` status and no `Transient` outcome (§6.2).

- [ ] **Step 1: Add the dependencies.** In `rs/Cargo.toml` `[workspace.dependencies]` add
  `jiff = "0.2"` with a comment: "core time type (SMA-786 §6.2); Helikon core needs >= 0.2.46,
  which Task 8 brings in". jiff is not a workspace dependency today (only transitive, 0.2.37 in
  `rs/Cargo.lock`). In the crate `Cargo.toml` add `jiff`, `serde`, `serde_json`, all
  `{ workspace = true }`. Do not move the locked jiff version in this task.

- [ ] **Step 2: Write the failing tests.**

```rust
inbox_new_rejects_an_agent_of_another_org   // InboxId::new(u(1),..) + AgentId::new(u(9),..) -> Err(DomainError::OrgMismatch{inbox_org:u(1),agent_org:u(9)})
inbox_new_takes_its_org_from_the_inbox_prn  // same org -> Ok; inbox.org() == u(1)
timer_fire_event_id_is_deterministic        // TimerId u(1), fire_seq 7 -> "timer:00000000-0000-0000-0000-000000000001:7"
pending_tool_call_state_allows_only_the_adr_transitions
    // Table over all 6 states x 5 methods. Ok exactly for: Open->{approve,deny,expire,supersede};
    // {Approved,Denied,Expired,Superseded}->resolve. Every other cell is Err(InvalidTransition{..}).
pending_tool_call_state_serializes_in_snake_case // serde_json::to_string(&Open) == "\"open\""
outbox_payload_message_serializes_with_a_kind_tag // {"kind":"message","text":"hi"}
```

- [ ] **Step 3: Run and see them fail.** Run: `cd rs && cargo nextest run --locked -p paigasus-agent-host-core`. Expected: compile errors.
- [ ] **Step 4: Implement the six modules.** Re-export them from `lib.rs` as `pub mod`.
- [ ] **Step 5: Run and see them pass.** Same command as Task 1 Step 5. Also run
  `git diff rs/Cargo.lock`. Expected: only the new crate's dependency edges change, and jiff stays at 0.2.37.
- [ ] **Step 6: Commit.** `git add` the crate, `rs/Cargo.toml`, `rs/Cargo.lock`. Message:
  `feat(rs): add the agent-host domain value objects` with a body that names §6.2 and the
  co-author line.

---

### Task 3: Core ports

**Files:**
- Create: `rs/crates/libs/paigasus-agent-host-core/src/ports.rs`
- Modify: `src/lib.rs`, the crate `Cargo.toml`, `rs/Cargo.lock`

**Interfaces:**
- Consumes: Tasks 1 and 2.
- Produces (all `#[async_trait] pub trait X: Send + Sync`, all provisional, doc comment on each
  trait names its owner issue and "provisional: the owner may change the methods, not the
  responsibility" per §6.2):
  - `#[derive(Debug, thiserror::Error)] pub enum RepositoryError { NotFound, Store(String) }`
  - `pub enum InsertOutcome { Inserted, Duplicate { payload_matches: bool } }`
  - `InboxRepository` (SMA-787): `get(&self, id: &InboxId) -> Result<Option<Inbox>, RepositoryError>`;
    `insert_event(&self, event: InboxEvent) -> Result<InsertOutcome, RepositoryError>`.
  - `TimerRepository` (SMA-788): `insert(&self, timer: Timer) -> Result<(), RepositoryError>`;
    `cancel(&self, id: &TimerId) -> Result<(), RepositoryError>`.
  - `Scheduler` (SMA-788): `fire_due(&self, limit: u32) -> Result<usize, RepositoryError>`.
  - `MemoryStore` (SMA-789): `read(&self, agent: &AgentId, path: &str) -> Result<Option<String>, RepositoryError>`;
    `write(&self, agent: &AgentId, path: &str, content: String) -> Result<(), RepositoryError>`;
    `list(&self, agent: &AgentId, prefix: &str) -> Result<Vec<String>, RepositoryError>`.
  - `Outbox` (SMA-792): `next_batch(&self, limit: u32) -> Result<Vec<OutboxMessage>, RepositoryError>`;
    `mark_sent(&self, id: &OutboxMessageId) -> Result<(), RepositoryError>`.
  - `AgentResolver` (SMA-790): `resolve(&self, inbox: &InboxId) -> Result<ResolvedAgent, RepositoryError>`.
  - `RunCommitter` (SMA-790): `commit(&self, handle: RunHandle, outcome: RunOutcome) -> Result<Committed, CommitError>`.
    Its doc comment states: a database error is `Store`, never `LeaseLost`; SMA-790 adds an
    "outcome unknown" case; the buffered-change shape is SMA-790's design (§6.2, §7).

- [ ] **Step 1: Write the failing test.**

```rust
#[test]
fn every_port_is_object_safe_send_and_sync() {
    fn assert_port<T: ?Sized + Send + Sync>() {}
    assert_port::<dyn InboxRepository>(); assert_port::<dyn TimerRepository>();
    assert_port::<dyn Scheduler>(); assert_port::<dyn MemoryStore>(); assert_port::<dyn Outbox>();
    assert_port::<dyn AgentResolver>(); assert_port::<dyn RunCommitter>();
}
```

- [ ] **Step 2: Run and see it fail** (compile error). Command as in Task 2.
- [ ] **Step 3: Add `async-trait = { workspace = true }` and implement `ports.rs`.** No adapter. The
  A1 guard now sees all seven allowed names.
- [ ] **Step 4: Run and see it pass.** Same command as Task 1 Step 5.
- [ ] **Step 5: Commit.** Message `feat(rs): add the provisional agent-host ports`, body names
  §6.2, the owner issues, and the co-author line.

---

### Task 4: `agents/v1` contracts

**Files:**
- Create: `contracts/proto/paigasus/agents/v1/agents.proto`,
  `rs/crates/libs/paigasus-proto/tests/agents_smoke.rs`,
  `py/packages/paigasus-proto/tests/test_agents_smoke.py`,
  `ts/packages/paigasus-proto/src/agents.ts`, `ts/packages/paigasus-proto/src/agents.test.ts`
- Generated (commit them): the three trees named in the file map.
- Modify: `rs/crates/libs/paigasus-proto/src/lib.rs:16-45`, `ts/packages/paigasus-proto/package.json:11-14`

**Interfaces:**
- Produces: Rust `paigasus_proto::paigasus::agents::v1::OutboxMessage { run_prn: String, text: String }`;
  Python `paigasus_proto.generated.paigasus.agents.v1.OutboxMessage`; TS subpath
  `@paigasus/proto/agents` exporting `OutboxMessageSchema`.

- [ ] **Step 1: Write the proto.** Header as `contracts/proto/paigasus/common/v1/actor.proto`
  (SPDX line, `syntax = "proto3";`, `package paigasus.agents.v1;`). No `service`, no `option`.
  One message:

```proto
message OutboxMessage {
  string run_prn = 1;
  string text = 2;
}
```

  The file comment holds the four points of §6.4 (package and permanence; proto3 JSON
  lowerCamelCase; each message is the `data` of one CloudEvent, with direction and type; the
  envelope rule with the full list `id`, `source`, `type`, `time`, `subject`, `threadid`,
  `actorprn`, `correlationid`, `schemaversion`). The message comment gives: direction out,
  CloudEvents `type` `agents.outbox.message`, envelope `source` `urn:paigasus:agents`, `subject`
  = the inbox PRN, so the message has no `inbox_prn`. Do not add `ApprovalRequest`,
  `ApprovalDecision`, an `APPROVAL_DECISION_KIND_*` enum or `InboxEvent` (§6.4).
  Field naming: `<thing>_prn`, as `org_prn` at `contracts/proto/paigasus/iam/v1/iam.proto:60`.

- [ ] **Step 2: Format, lint, generate.**
  Run: `cd contracts && buf format -w && cd .. && moon run contracts:fmt contracts:lint contracts:breaking && moon run contracts:generate --force`
  Expected: all pass. `git status` shows the three new generated trees and no other generated
  change. If `ts/.../generated/google/rpc/error_details_pb.ts` is deleted (a BSR rate limit),
  restore it with `git checkout -- <path>` and run generate again later.

- [ ] **Step 3: Register and write the smoke tests (they fail before registration).**
  - Rust `src/lib.rs`: add `pub mod agents { pub mod v1 { #![allow(clippy::all, warnings)]
    include!("generated/paigasus/agents/v1/paigasus.agents.v1.rs"); } }` inside `pub mod paigasus`,
    as the `gateway` block. No tonic include.
  - Rust test `agents_smoke.rs` (pattern `tests/health_smoke.rs`):
    `outbox_message_carries_run_prn_and_text`: build `OutboxMessage { run_prn: "prn:pgs:agents::<org>:run/<id>".into(), text: "hi".into() }` and assert both fields.
  - Python `test_agents_smoke.py` (pattern `tests/test_health_smoke.py`):
    `test_outbox_message_carries_run_prn_and_text`: `OutboxMessage(run_prn="p", text="hi")`, assert both.
  - TS `agents.ts`: SPDX header, a short comment that the root barrel does not export it (as
    `src/iam.ts`), and `export * from './generated/paigasus/agents/v1/agents_pb';`.
  - TS `package.json` `exports`: add `"./agents": "./src/agents.ts"` after `"./iam"`.
  - TS test `agents.test.ts` (pattern `src/health.test.ts` and `src/index.test.ts:44-60`):
    `it('creates an OutboxMessage')`: `create(OutboxMessageSchema, { runPrn: 'p', text: 'hi' })`, assert fields;
    `it('has the agents type name')`: `OutboxMessageSchema.typeName === 'paigasus.agents.v1.OutboxMessage'`;
    `it('is not in the root barrel')`: `'OutboxMessageSchema' in (await import('./index.js'))` is false.

- [ ] **Step 4: Run the three smoke tests and see them pass.**
  Run: `cd rs && cargo nextest run --locked -p paigasus-proto && cd .. && moon run paigasus-proto-py:test paigasus-proto-ts:test paigasus-proto-ts:typecheck ts:fmt`
  Expected: PASS. (Use `moon query projects` to confirm the exact project IDs if one is unknown.)

- [ ] **Step 5: Commit. The type is `feat(contracts)`.** Reason (`.github/CLAUDE.md:118-128`): a
  new message in the published `paigasus-proto` family is a new public API. Rule C1 says a
  non-releasing commit would still ship with the next releasing commit, so a non-releasing type
  would not hold the package back; it would only hide a feature and give it a patch level. `feat`
  with scope `contracts` is a releasing commit and gives the correct minor level.

```bash
git add contracts/proto/paigasus/agents rs/crates/libs/paigasus-proto py/packages/paigasus-proto ts/packages/paigasus-proto
git commit -m "feat(contracts): add the agents/v1 package with OutboxMessage" -m "SMA-786 §6.4. One host-emitted message, generated in Rust, Python and TypeScript.
A TS ./agents subpath, and one smoke test per language." -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: Service crate, config and migrations under a lock

**Files:**
- Create: `rs/crates/services/paigasus-agent-host/{Cargo.toml,moon.yml,CHANGELOG.md,README.md,agent-host.toml.example}`,
  `src/lib.rs`, `src/config.rs`, `src/adapters/mod.rs`, `src/adapters/persistence/mod.rs`,
  `src/adapters/persistence/migration/mod.rs`, `src/adapters/persistence/migration/m0001_create_inbox.rs`,
  `src/adapters/persistence/migration_lock.rs`
- Modify: `rs/Cargo.toml` (`members`), `rs/Cargo.lock`, `rs/release-plz.toml`, `ci/affected-graph/run.sh`

**Interfaces:**
- Produces:
  - `config::Config { http_addr: SocketAddr, database_url: RedactedUrl, log_level: String,
    metrics: MetricsConfig, migration: MigrationConfig }`, `MetricsConfig { enabled: bool,
    addr: Option<SocketAddr> }`, `MigrationConfig { lock_wait_secs: u64 }` with
    `lock_wait(&self) -> Duration`. `Config::figment() -> Figment`,
    `Config::load() -> Result<Config, figment::Error>`, `Config::validate(&self) -> Result<(), ConfigError>`.
    `RedactedUrl::as_str(&self) -> &str`.
  - `adapters::persistence::{Migrator, MIGRATION_TABLE}` with
    `pub const MIGRATION_TABLE: &str = "agent_host_migrations";`.
  - `adapters::persistence::migration_lock::{MIGRATION_LOCK_KEY, migrate_under_lock,
    MigrationLockOutcome, MigrationLockError}` with `pub const MIGRATION_LOCK_KEY: i64 = 5_580_786;`
    and `pub async fn migrate_under_lock(db: &DatabaseConnection, wait: Duration) -> Result<MigrationLockOutcome, MigrationLockError>`.

- [ ] **Step 1: Write the crate files and register the crate.**
  - `Cargo.toml`: `[package]` as `rs/crates/services/paigasus-gateway/Cargo.toml:1-16` but name
    `paigasus-agent-host`, version `0.0.0`, `[lib] name = "paigasus_agent_host"`. No `[[bin]]` yet.
    Dependencies (each used in this task): `sea-orm`, `sea-orm-migration`, `async-trait`,
    `figment`, `serde`, `thiserror`, `tracing`, `tokio` (features `time`), all
    `{ workspace = true }`. Dev: `figment = { workspace = true, features = ["test"] }`.
  - `moon.yml`: id `paigasus-agent-host-rs`, layer `application`, language `rust`, empty
    `dependsOn`, `fileGroups.upstreams: []`, `build` and `test` with `deps: ['^:build']`, and
    `test` with `options: { mutex: 'heavy-integration' }` (as `paigasus-gateway/moon.yml:68-69`).
  - `rs/Cargo.toml`: add `"crates/services/paigasus-agent-host",` first among `crates/services/`.
  - `rs/release-plz.toml`: a `[[package]]` block for `paigasus-agent-host` with the three keys
    false; change "twelve" to "thirteen".
  - `CHANGELOG.md`: copy the header form of `rs/crates/services/paigasus-gateway/CHANGELOG.md`
    with no entries. `README.md`: purpose, the config table of §6.6, and the sentence "The agent
    host needs its own database. A separate schema in a shared database is not supported." (§6.8).
    `agent-host.toml.example`: every key of §6.6 with its default, commented, and the same
    database sentence.

- [ ] **Step 2: Write the failing tests.**

```rust
// config.rs (figment::Jail, as the IAM config tests)
defaults_bind_the_container_port            // with AGENT_HOST_DATABASE_URL set only: http_addr == "0.0.0.0:8086", log_level "info", metrics.enabled, metrics.addr None, lock_wait_secs == 120
database_url_is_required                    // no AGENT_HOST_DATABASE_URL -> Config::load() is Err
debug_redacts_the_database_url              // format!("{config:?}") does not contain the password "s3cret"
the_toml_file_and_nested_env_keys_are_read  // agent-host.toml sets log_level; AGENT_HOST_METRICS__ADDR="127.0.0.1:9100" -> metrics.addr Some
metrics_addr_on_the_http_port_is_rejected   // Review Focus 2: http 0.0.0.0:8086, metrics 127.0.0.1:8086 -> validate() Err
lock_wait_outside_1_to_3600_is_rejected     // 0 and 3601 Err; 1 and 3600 Ok
// migration_lock.rs
the_lock_key_differs_from_both_iam_keys     // MIGRATION_LOCK_KEY != 5_580_559 && != 5_580_467 (IAM MIGRATION_LOCK_KEY and AUDIT_PARTITION_LOCK_KEY)
the_migrator_uses_its_own_table             // Migrator::migration_table_name() renders as MIGRATION_TABLE
// plus the four next_poll tests, copied with their names from IAM migration_lock.rs
```

- [ ] **Step 3: Run and see them fail.** Run: `cd rs && cargo build -p paigasus-agent-host && cargo nextest run --locked -p paigasus-agent-host`. Expected: compile errors.

- [ ] **Step 4: Implement.**
  - `config.rs`: copy the structure of `rs/crates/services/paigasus-iam/src/config.rs`:
    `RedactedUrl` (`:22`, `:77`, `:95`), the figment chain (`:1219-1231`) with
    `Toml::file("agent-host.toml")` and `Env::prefixed("AGENT_HOST_").split("__")`, the port check
    (`:1654-1659`, compare `.port()` only), the range check (`:1663-1666`), default 120 (`:862`).
  - `migration_lock.rs`: copy `rs/crates/services/paigasus-iam/src/adapters/persistence/migration_lock.rs`
    whole. Changes: the key; `applied_count` uses `to_regclass('agent_host_migrations')`
    (unqualified, §6.8) and `FROM agent_host_migrations`, both built from `MIGRATION_TABLE`; the
    composition-root guard test is NOT copied here (Task 6 adds it, when `main.rs` exists).
  - `migration/mod.rs`: `pub struct Migrator;` with `#[async_trait] impl MigratorTrait` that lists
    `m0001_create_inbox::Migration` and overrides `fn migration_table_name() -> DynIden` to return
    `MIGRATION_TABLE` as an identifier.
  - `m0001_create_inbox.rs`: create the table of §6.8 exactly (`id`, `org_id`, `agent_id`,
    `principal_id` as `uuid NOT NULL`, `created_at timestamptz NOT NULL DEFAULT now()`,
    `UNIQUE (agent_id, principal_id)`). `down` drops it.

- [ ] **Step 5: Run and see them pass.** Run: `cd rs && cargo nextest run --locked -p paigasus-agent-host && cargo clippy --locked -p paigasus-agent-host --all-targets -- -D warnings && cargo fmt --check`. Expected: PASS.

- [ ] **Step 6: Update the affected-graph expectations.** As Task 1 Step 6. Expected rows:
  `lockfile->all-lint` and `-ci` (`paigasus-agent-host-rs:lint`). Run until no FAIL row.

- [ ] **Step 7: Commit.** `feat(rs): add the paigasus-agent-host service crate with locked migrations`,
  body names §6.6, §6.8, the lock key and the table name, then the co-author line.

---

### Task 6: HTTP surface, boot router, metrics and the composition root

**Files:**
- Create: `src/main.rs`, `src/adapters/boot.rs`, `src/adapters/http/mod.rs`, `src/adapters/http/error.rs`
- Modify: the service `Cargo.toml` and `moon.yml`, `src/lib.rs`, `src/adapters/mod.rs`,
  `src/adapters/persistence/migration_lock.rs` (the guard test),
  `rs/crates/libs/paigasus-observability/src/names.rs`,
  `rs/crates/libs/paigasus-observability/tests/drift.rs:14,36,42`,
  `ci/error-registry/check.py` (`MANIFEST`, near `:118-120`), `ci/affected-graph/run.sh`, `rs/Cargo.lock`

**Interfaces:**
- Consumes: Task 5 `Config`, `migrate_under_lock`, `MIGRATION_LOCK_KEY`.
- Produces:
  - `adapters::http::AppState { db: DatabaseConnection }` (`Clone`),
    `adapters::http::app_router(state: AppState, metrics: Option<Router>) -> Router`.
  - `adapters::http::error::not_found() -> Response` (404, code `not-found`).
  - `adapters::boot::{BootSlot, boot_http_router}`: `BootSlot::new() -> Self`,
    `BootSlot::install(&self, app: Router) -> Result<(), AlreadyInstalled>`,
    `boot_http_router(slot: BootSlot, metrics: Option<Router>) -> Router`.
  - `paigasus_observability::names::{AGENT_HOST_HTTP_REQUESTS_TOTAL,
    AGENT_HOST_HTTP_REQUEST_DURATION_SECONDS, AGENT_HOST_HTTP_INFLIGHT_REQUESTS}`, in `ALL`.
  - Binary `paigasus-agent-host` (`[[bin]] path = "src/main.rs"`).

- [ ] **Step 1: Dependencies and Moon edges.** Add `axum`, `paigasus-observability`, `metrics`,
  `serde_json`, `anyhow` (`{ workspace = true }`), `paigasus-logging = { path =
  "../../libs/paigasus-logging", version = "0.0.0" }` (the gateway form), and tokio features
  `rt-multi-thread`, `macros`, `net`, `signal`, `time`, `sync`. Dev: `paigasus-proto`, `tower`
  (`util`, for `oneshot`), `tokio` with `test-util` if needed. `moon.yml`: `dependsOn`
  `paigasus-logging-rs`, `paigasus-observability-rs`, `paigasus-proto-rs`; `upstreams` = the
  transitive closure, in the form of `paigasus-gateway/moon.yml:28-49`: kernel, logging,
  observability, proto-derive, proto (each `src/**/*` plus `Cargo.toml`). Do not add
  `paigasus-proto-derive-rs` to `dependsOn`: it is not a direct dependency (the gateway does not
  list it either). A6 decides the exact `upstreams` set.

- [ ] **Step 2: Write the failing tests.**

```rust
// http/mod.rs (tower::ServiceExt::oneshot on the router; DatabaseConnection::default() is Disconnected)
healthz_is_200_always
readyz_is_503_when_the_database_ping_fails        // AppState { db: DatabaseConnection::default() }
an_unknown_path_is_404_in_the_error_envelope       // body == {"error":{"code":"not-found","message":<non-empty>}}
// http/error.rs
the_not_found_code_is_in_the_registry              // ErrorReason::from_wire_reason("not-found") == Some(ErrorReason::NotFound)
// boot.rs
while_migrating_healthz_is_200_and_readyz_503      // body {"status":"migrating"}
while_migrating_another_path_is_503_service_migrating // {"error":{"code":"service-migrating",..}}
after_install_the_app_router_answers               // install(app_router(..)); unknown path -> 404 not-found
a_second_install_is_rejected                       // Err(AlreadyInstalled)
the_migrating_code_is_in_the_registry              // from_wire_reason("service-migrating") == Some(ErrorReason::ServiceMigrating)
// migration_lock.rs (copy of IAM's test at rs/crates/services/paigasus-iam/src/adapters/persistence/migration_lock.rs:256)
the_composition_root_still_migrates_under_the_lock // include_str!("../../main.rs") minus `//` lines: contains "migrate_under_lock(", contains "config.migration.lock_wait()", no "Migrator::up"
```

```rust
// rs/crates/libs/paigasus-observability/tests/drift.rs
#[test]
fn metric_idents_sees_the_agent_host_prefix() {
    assert_eq!(metric_idents("rate(agent_host_http_requests_total[5m])"), vec!["agent_host_http_requests_total".to_string()]);
}
```

- [ ] **Step 3: Run and see them fail.** Run: `cd rs && cargo nextest run --locked -p paigasus-agent-host -p paigasus-observability`. Expected: compile errors, and the drift test fails on the prefix.

- [ ] **Step 4: Implement.**
  - `boot.rs`: copy the HTTP half of `rs/crates/services/paigasus-iam/src/adapters/boot.rs`
    (`AlreadyInstalled`, `BootSlot` over `Arc<OnceLock<Router>>`, `boot_http_router`, the
    migrating fallback with the `MIGRATING` constant at `:204` and the envelope at `:225-228`).
    Drop the gRPC half, `HealthReporter` and `AppState` from the copy.
  - `http/mod.rs`: `/healthz`, `/readyz` (`db.ping()`; 200 `{"status":"ok"}` or 503), the
    `not_found` fallback, then `paigasus_observability::http_metrics_layer("agent_host")` and
    the correlation layer over the app routes. `/healthz` and `/readyz` stay outside the
    correlation and metrics layers (§6.7). Take the layer order from
    `rs/crates/services/paigasus-gateway/src/adapters/http/mod.rs:91-120`. Merge the metrics router
    when `metrics.addr` is unset.
  - `names.rs`: three `pub const AGENT_HOST_*` with the names that `http.rs:42-60` derives from
    the prefix; add them to `ALL`.
  - `drift.rs`: add `|| token.starts_with("agent_host_")` at `:42`, and name `agent_host_` in the
    doc comments at `:14` and `:36`.
  - `main.rs`: the boot sequence of §6.5, steps 1 to 10, copied from
    `rs/crates/services/paigasus-iam/src/main.rs` (connect `:118`, binds `:134` and `:146-147`,
    upkeep `:197-227`, the one shutdown signal `:256-279`, the drains `:374-382`) and the
    healthcheck dispatch from `rs/crates/services/paigasus-gateway/src/main.rs`. Service name
    `"paigasus-agent-host"` for `paigasus_logging::init` and `paigasus_observability::init`. A
    `describe_agent_host_metrics()` with one `describe_*` line per name (as `gateway/src/main.rs:219-240`).
  - `check.py`: two `emits` rows, form `:118-120`:
    `("rs/crates/services/paigasus-agent-host/src/adapters/boot.rs", "emits", "the_migrating_code_is_in_the_registry", "MIGRATING — the boot router's 503 while migrations run")` and
    `("rs/crates/services/paigasus-agent-host/src/adapters/http/error.rs", "emits", "the_not_found_code_is_in_the_registry", "not_found() — the 404 fallback")`.
    Spell each code only in its own file.

- [ ] **Step 5: Run and see them pass.**
  Run: `cd rs && cargo nextest run --locked -p paigasus-agent-host -p paigasus-observability && cargo clippy --locked -p paigasus-agent-host -p paigasus-observability --all-targets -- -D warnings && cargo fmt --check && cd .. && moon run repo:error-code-single-site repo:observability-drift repo:http-extractor-envelope`
  Expected: PASS.

- [ ] **Step 6: Update the affected-graph expectations.** As Task 1 Step 6. Expected new rows:
  `contracts->proto` and `proto-derive->proto` (`paigasus-agent-host-rs`), `kernel->bindings`
  (`paigasus-agent-host-rs`, through observability), `proto->svc-info-deep` and
  `proto->svc-info-ci` (`paigasus-agent-host-rs:build`, `:test`, `:lint`), `kernel->consumer-tasks`
  (the same three). Add only what a FAIL row names.

- [ ] **Step 7: Commit.** `feat(rs): boot the agent host with health probes, metrics and the boot router`,
  body names §6.5, §6.7, the two registry rows and the drift prefix, then the co-author line.

---

### Task 7: Postgres tests and the Docker registration points

**Files:**
- Create: `tests/support/mod.rs`, `tests/boot_pg.rs`, `tests/migration_isolation_pg.rs`, `tests/docker_preflight.rs`
- Modify: the service `Cargo.toml` and `moon.yml`, `rs/.config/nextest.toml` (after `:136`),
  `moon.yml` (root: `:542`, `:548`, `:549`, `:566-569`, `:623`, `:629`, `:645-646`),
  `ci/affected-graph/run.sh`, `rs/Cargo.lock`

**Interfaces:**
- Consumes: the binary from Task 6 (`env!("CARGO_BIN_EXE_paigasus-agent-host")`),
  `MIGRATION_LOCK_KEY`, `MIGRATION_TABLE`.
- Produces: `tests/support/mod.rs` with `start_raw_postgres() -> Option<(ContainerAsync<Postgres>, String /*url*/)>`
  through `paigasus_test_docker::start_or_skip(Postgres::default().with_tag("16-alpine"), "<what>")`
  (form: `rs/crates/services/paigasus-iam/tests/support/mod.rs:84`), and helpers
  `spawn_service(url: &str) -> (Child, u16)`, `http_status(port, path) -> Option<(u16, String)>`
  (copy `paigasus-iam/tests/boot_lifecycle_pg.rs:110-114`), `connect_pinned(url)` (`:100`).

- [ ] **Step 1: Dev dependencies and Moon edge.** Add `paigasus-test-docker = { workspace = true }`,
  `testcontainers = "0.27"`, `testcontainers-modules = { version = "0.15", features = ["postgres"] }`,
  `reqwest = { workspace = true }` (the IAM pins). `moon.yml`: `dependsOn` gains
  `paigasus-test-docker-rs`; `upstreams` gains its two lines.

- [ ] **Step 2: Write the tests.** They spawn the built binary, as IAM's `boot_lifecycle_pg.rs`
  does, because the boot order lives in `main.rs`. Each spawn sets `AGENT_HOST_DATABASE_URL`,
  `AGENT_HOST_HTTP_ADDR=127.0.0.1:<free port>`, `AGENT_HOST_MIGRATION__LOCK_WAIT_SECS`.

```rust
// docker_preflight.rs (copy rs/crates/services/paigasus-gateway/tests/docker_preflight.rs; Postgres in place of Redis)
docker_backed_suites_can_actually_run      // unless skip_docker(): start_raw_postgres().await.is_some()
// boot_pg.rs
a_fresh_database_boots_and_applies_m0001_once // /readyz 200; SELECT count(*) FROM agent_host_migrations == 1; to_regclass('inbox') IS NOT NULL
a_second_boot_applies_nothing              // spawn again on the same DB: count stays 1, /readyz 200
the_inbox_unique_constraint_holds          // two INSERTs with the same (agent_id, principal_id) -> second fails with SQLSTATE 23505
a_held_lock_reports_migrating              // hold pg_advisory_lock(5_580_786) on a pinned connection; /healthz 200,
                                           // /readyz 503 {"status":"migrating"}, /v1/x 503 code "service-migrating";
                                           // release the lock -> /readyz 200, /v1/x 404 "not-found"
sigterm_during_the_lock_wait_exits_promptly // Review Focus 3: hold the lock, wait for /healthz, `kill -TERM <pid>`
                                           // (std::process::Command); the child exits with status 0 within 15 s
metrics_count_requests_with_the_agent_host_prefix // after one GET /v1/x: a SAMPLE line (not # HELP/# TYPE) that starts
                                           // with "agent_host_http_requests_total{" has a value >= 1
// lock contention: copy two_concurrent_migrators_converge_without_deadlocking from
// rs/crates/services/paigasus-iam/tests/migration_lock_pg.rs:61, against this crate's Migrator and table
// migration_isolation_pg.rs (anti-goal A6)
a_foreign_seaql_migrations_table_is_left_alone // pre-create public.seaql_migrations (the SeaORM default columns)
                                           // with version "m9999_other_service"; boot; /readyz 200;
                                           // that table still holds exactly that one row
```

- [ ] **Step 3: Run with Docker up and see them pass.**
  Run: `cd rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run --locked -p paigasus-agent-host`
  Expected: PASS. Prove that the isolation test bites: temporarily remove the
  `migration_table_name` override, run `--test migration_isolation_pg`, see it FAIL (SeaORM
  "Migration file of version 'm9999_other_service' is missing" or a changed foreign table),
  then restore the override and re-run to PASS. Do not commit the mutation.

- [ ] **Step 4: Registration points.**
  - `rs/.config/nextest.toml`: after `:136`, copy the two gateway blocks (`:122-136`) with
    `package(=paigasus-agent-host)`: `binary(docker_preflight)` with `retries = 1`; and
    `binary(docker_preflight) or binary(boot_pg) or binary(migration_isolation_pg)` with the
    shared retry budget and `test-group = 'docker-containers'`.
  - Root `moon.yml`: add `services/paigasus-agent-host/src services/paigasus-agent-host/tests` to
    the three `repo:redis-connect-single-site` directory lists (`:542`, `:548`, `:549`) and its
    inputs (`:566-569`); add `services/paigasus-agent-host/tests` to the two
    `repo:iam-docker-policy-single-site` lists (`:623`, `:629`) and its inputs (`:645-646`).
  - Affected graph: expected new row `test-docker->services` (`paigasus-agent-host-rs`). As Task 1 Step 6.

- [ ] **Step 5: Run the gates.**
  Run: `moon run repo:redis-connect-single-site repo:iam-docker-policy-single-site repo:input-liveness paigasus-agent-host-rs:test`
  Expected: PASS. Then run `/opt/homebrew/bin/bash ci/actionlint/run.sh`; read its pipe preflight
  line first. If it exits rc 2 with a "small" pipe message, record that and use the Linux
  container route instead of claiming a verdict.

- [ ] **Step 6: Commit.** `test(rs): prove the agent-host boot, lock and table isolation against Postgres`,
  body names §6.8 and the registration points, then the co-author line.

---

### Task 8: Helikon dependency, the TLS ban and the first use

**Files:**
- Create: `rs/crates/services/paigasus-agent-host/src/adapters/helikon.rs`
- Modify: `rs/deny.toml:68-70`, the service `Cargo.toml` and `moon.yml`, `src/adapters/mod.rs`,
  `rs/Cargo.lock`, `ci/affected-graph/run.sh`

**Interfaces:**
- Consumes: `paigasus_agent_host_core::agent::AgentDefinition` (Task 2).
- Produces: `pub fn build_agent<M: Model + 'static>(def: &AgentDefinition, model: M) -> LlmAgent<(), M, String>`
  with `use paigasus_helikon::core::{LlmAgent, Model};` (the facade re-exports the core as
  `core`; it does not re-export `LlmAgent` or `Model` at its root).

- [ ] **Step 1: The ban, before the dependency.** In `rs/deny.toml` `[bans]` add:

```toml
deny = [
  { crate = "aws-lc-rs", reason = "ring-only workspace: a second rustls provider panics at first TLS use (SMA-786 §3)" },
  { crate = "aws-lc-sys", reason = "ring-only workspace (SMA-786 §3)" },
  { crate = "openssl-sys", reason = "rustls-only workspace: no system OpenSSL (SMA-786 §3)" },
  { crate = "native-tls", reason = "rustls-only workspace (SMA-786 §3)" },
]
```

  Run: `moon run repo:deny --force` (it runs `cargo deny check`, which includes `bans`).
  Expected: PASS. Record the output for the PR.

- [ ] **Step 2: Record the lock before.** Run: `cp rs/Cargo.lock "$SCRATCH/Cargo.lock.before"`
  (any directory outside the repo).

- [ ] **Step 3: Add the dependencies.** Service `Cargo.toml`:

```toml
# Exact pin (SMA-786 §6.9): the facade re-exports the core, and one facade PATCH release
# already moved the core from 0.5 to 0.6. A facade bump re-checks §6.9 of the spec.
paigasus-helikon = { version = "=0.6.12", default-features = false, features = ["runtime-tokio"] }
paigasus-agent-host-core = { workspace = true }
```

  Dev: `async-trait = { workspace = true }` if the test model needs it (the core `Model` trait
  is `#[async_trait]`). `moon.yml`: `dependsOn` gains `paigasus-agent-host-core-rs`; `upstreams`
  gains its two lines. Run `cd rs && cargo update -p paigasus-helikon` only if `cargo build`
  needs it. The jiff floor `^0.2.46` of Helikon core moves the workspace jiff up.

- [ ] **Step 4: Re-check the Helikon facts against the published crate.** The local registry
  held no 0.6.x source when this plan was written; the facts below come from Helikon commit
  7ed91e7e. Read `~/.cargo/registry/src/*/paigasus-helikon-core-0.6.0/` now and confirm:
  `LlmAgent<Ctx, M, T = String>` (`agent.rs:312`), `LlmAgent::builder::<Ctx>()` (`:377`),
  `.name(impl Into<String>)` and `.model(M)` required, `.instructions(impl Instructions<Ctx>)`
  optional, `build(self) -> LlmAgent<Ctx, M, T>` infallible (`agent_builder.rs:356`), the `Model`
  trait (`model.rs:49-90`: `invoke` and `capabilities` required), `PendingToolCall` fields
  (`deferral.rs:12-19`), `new_deferral_id` UUID v4 (`deferral.rs:92`). Also confirm the facade
  `Cargo.toml` requires core `^0.6.0`. **If any fact differs, stop and report. Do not wrap Helikon
  in an abstraction.**

- [ ] **Step 5: Write the failing test** in `helikon.rs`:

```rust
struct NoModel; // test-only Model: invoke -> Err(ModelError::Unavailable), capabilities -> ModelCapabilities::default()
#[test]
fn build_agent_maps_the_name_and_the_instructions() {
    // def: name "triage", instructions "Be brief.", model_id "m", tool_names ["t"]
    // agent = build_agent(&def, NoModel)
    // assert_eq!(Agent::name(&agent), "triage")
    // ctx = RunContext::<()>::new(Arc::new(()), Arc::new(MemorySession::new()), HookRegistry::new(), TracerHandle::default(), CancellationToken::new())
    // assert_eq!(agent.instructions.render(&ctx), "Be brief.")
}
```

  (There is no instructions accessor; the field is public and `render` needs a `RunContext`. All
  these types are at the core root.) Run: `cd rs && cargo nextest run --locked -p paigasus-agent-host helikon`. Expected: FAIL (compile error).

- [ ] **Step 6: Implement `build_agent`.** `LlmAgent::builder::<()>().name(def.name.clone()).model(model).instructions(def.instructions.clone()).build()`. Tool names are not mapped (SMA-790). Run the test: PASS.

- [ ] **Step 7: Measure the lockfile effect (§6.9).**
  - `diff` the saved lock against `rs/Cargo.lock`. List every package whose version changed.
    **Every change must be upward. If one moves down, stop and report.**
  - Confirm jiff is at 0.2.46 or later, and a 1.x copy exists only if another crate asks for it.
  - Confirm `paigasus-helikon` 0.6.12 resolves `paigasus-helikon-core` 0.6.x.
  - `grep -E '^name = "(aws-lc-rs|aws-lc-sys|openssl-sys|native-tls)"' rs/Cargo.lock` prints
    nothing. If it prints a line, stop and report.
  - Run `moon run repo:deny repo:osv repo:machete`. Expected: PASS. A new license or advisory
    gets a `deny.toml` entry with a reason, never a wildcard (A4).
  - Save the list of version changes for the PR body (Task 11).

- [ ] **Step 8: Affected graph.** As Task 1 Step 6. Probably no new row (the service already
  joins every kernel row). Then run the service checks of Task 1 Step 5 for both crates.

- [ ] **Step 9: Commit.** `feat(rs): add paigasus-helikon as the agent host's first runtime dependency`,
  body: §6.9, the deny bans, "jiff moves up to <version>", no other text that starts with `#`,
  then the co-author line.

---

### Task 9: Image build and smoke, no release chain

**Files:**
- Modify: `ci/images/run.sh` (`crate_for` `:66-72`, `kind_for_key` `:77-83`, run-scoped names
  `:870-875`, `cleanup()` `:876`, a new `smoke_agent_host` after `smoke_iam` `:1083`, `smoke()`
  `:1129-1142`, the usage text near `:2590`), `.github/workflows/images.yml` (after `:166-169`)

**Interfaces:**
- Consumes: the image built by the existing `rs/Dockerfile` with `--build-arg BIN=paigasus-agent-host`.
- Produces: `ci/images/run.sh build agent-host` and `ci/images/run.sh smoke agent-host`.

- [ ] **Step 1: Edit `run.sh`.**
  - `crate_for`: `agent-host) echo paigasus-agent-host ;;`. `kind_for_key`: add `agent-host` to
    the `cargo` arm.
  - Names: `AH_NAME=smoke-agent-host-$RUN_ID` and `AH_PG_NAME=smoke-ah-pg-$RUN_ID` (its own
    Postgres, not `$PG_NAME`). Add both to `cleanup()`'s `docker rm -f` list.
  - `smoke_agent_host`: start `postgres@$POSTGRES_16_ALPINE_DIGEST` (`:59`) as `$AH_PG_NAME` on
    `$NET`; start the image with `AGENT_HOST_DATABASE_URL=postgres://…@$AH_PG_NAME:5432/…`
    (hostname, not IP); `expect_status "agent-host /readyz" … 200`; in-image `healthcheck` exit 0.
    Negative row, copied from the gateway (`:1065-1079`): `docker stop $AH_PG_NAME`, then
    `/readyz` 503 and `healthcheck --path /readyz` exit exactly 1.
  - `smoke()`: an `agent-host)` case: `assert_fresh paigasus-agent-host:dev; smoke_agent_host; assert_uid "$AH_NAME"`.
  - Do not change the bare `build`, bare `smoke`, `all`, `build-oci`, `load-oci`, `rehearse`
    lists. Add `agent-host` to the usage text for `build` and `smoke` only.

- [ ] **Step 2: Run locally (Docker up).**
  Run: `ci/images/run.sh build agent-host && ci/images/run.sh smoke agent-host`
  Expected: `SMOKE OK (agent-host)`. Then prove the negative row bites: run
  `ci/images/run.sh build-oci agent-host out` and expect a non-zero exit (no OCI support, A7).

- [ ] **Step 3: Edit `images.yml`.** A new step after "Smoke each service on its own":

```yaml
      - name: Build and smoke the agent host (no release chain, SMA-786)
        run: |
          ci/images/run.sh build agent-host
          ci/images/run.sh smoke agent-host
```

  Do not change the OCI build, load, SBOM or publish steps.

- [ ] **Step 4: Run the workflow gates.** Run `moon run repo:workflow-credentials repo:input-liveness`
  and `/opt/homebrew/bin/bash ci/actionlint/run.sh` (pipe preflight first, as Task 7 Step 5), and
  `bash ci/images/console-selftest.sh` (it reads `images.yml`). Expected: PASS.

- [ ] **Step 5: Commit.** `feat(ci): build and smoke the agent-host image with no release chain`,
  body names §6.10 and "no build-oci support, so no publish path exists", then the co-author line.

---

### Task 10: Local run with `moon run paigasus-agent-host-rs:dev`

**Files:**
- Create: `rs/crates/services/paigasus-agent-host/scripts/dev.sh` (mode 755)
- Modify: the service `moon.yml`, `README.md`

**Interfaces:**
- Consumes: the binary and `AGENT_HOST_*` config.
- Produces: Moon task `paigasus-agent-host-rs:dev`; `scripts/dev.sh [--check]`.

- [ ] **Step 1: Write `scripts/dev.sh`.** `#!/usr/bin/env bash`, SPDX line, `set -euo pipefail`.
  Behaviour per §6.11:
  1. Remove only containers with label `paigasus.dev=agent-host` whose `paigasus.dev.pid` is not a
     live process (`kill -0`). Leave the others.
  2. Start `postgres:16-alpine@sha256:<digest>` with the labels `paigasus.dev=agent-host` and
     `paigasus.dev.pid=$$`, a unique name, `-p 127.0.0.1::5432`; read the host port with
     `docker port`; wait for `pg_isready` (bounded). A comment names the digest source:
     `POSTGRES_16_ALPINE_DIGEST` in `ci/images/run.sh:59`. Do not source that file.
  3. `trap` on `EXIT` removes the container.
  4. Default mode: `cargo run --locked -p paigasus-agent-host` from `rs/`, with
     `AGENT_HOST_DATABASE_URL` and `AGENT_HOST_HTTP_ADDR=127.0.0.1:<free port>`.
  5. `--check`: `cargo build --locked -p paigasus-agent-host` first; then start the binary, poll
     `/readyz` for at most 60 s, exit 0 on 200, exit 1 on timeout; stop the binary.
  Shell rules: runs under `/bin/bash` 3.2 and bash 5. No here-string, no `mapfile`, no
  `declare -A`, no `local -n`, no pipe into `grep -q`, `grep -m`, `head` or `awk … exit`
  (`repo:actionlint` check 13). Use `sed -n 1p` in place of `head -1`.

- [ ] **Step 2: Add the Moon task.**

```yaml
  dev:
    command: 'bash scripts/dev.sh'
    deps: ['^:build']
    inputs:
      - '@group(sources)'
      - 'Cargo.toml'
      - '@group(upstreams)'
      - '/rs/Cargo.lock'
      - '/rs/.cargo/config.toml'
      - 'scripts/dev.sh'
    options:
      cache: false
      persistent: true
      runInCI: false
```

  Shape from `paigasus-gateway/moon.yml:70-86`; `runInCI: false` precedent at root `moon.yml:15`;
  `persistent: true` at `.moon/templates/python/moon.yml:11`. Do not use `local: true`.
  Run: `moon tasks paigasus-agent-host-rs`. Expected: `dev` is listed with no error.

- [ ] **Step 3: Verify the done-when items of §8.**
  - `rs/crates/services/paigasus-agent-host/scripts/dev.sh --check` under `/bin/bash` and under
    `/opt/homebrew/bin/bash`. Expected: exit 0 both times.
  - `moon run paigasus-agent-host-rs:dev`; in a second shell `curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:<port>/readyz`
    prints `200`, and `docker exec <container> psql -U postgres -tc "select to_regclass('inbox')"`
    prints `inbox`. Stop with Ctrl-C; `docker ps --filter label=paigasus.dev=agent-host` is empty.
  - Review Focus 5: start two `dev.sh --check` runs at the same time. Expected: both exit 0, and
    neither run's log shows that it removed the other's container.

- [ ] **Step 4: README.** Add a "Local run" section: the command, what it starts, `--check`, the
  label scheme, and that a SIGKILL leaves a container that the next run removes.

- [ ] **Step 5: Gates.** Run `/bin/bash ci/affected-graph/run.sh` (A8: `--locked` in the script;
  A10: `/rs/.cargo/config.toml` in the inputs). If a lockfile row now lists
  `paigasus-agent-host-rs:dev`, add it only if the row is a strict set that names it. Run
  `/opt/homebrew/bin/bash ci/actionlint/run.sh` (check 13 scans every tracked `*.sh`).
  Expected: PASS.

- [ ] **Step 6: Commit.** `feat(rs): start the agent host locally with one moon command`, body
  names §6.11, then the co-author line.

---

### Task 11: Registration sweep, the full gate graph and the image budget

**Files:**
- Modify: only where the sweep finds a miss.

- [ ] **Step 1: Sweep.** Run
  `git grep -n -e 'paigasus-gateway' -e 'paigasus-gateway-core' -- ':!docs/superpowers' ':!**/CHANGELOG.md' ':!rs/Cargo.lock'`.
  For each file, decide "applies to the agent host in M0" or "does not" (chart, release chain,
  console, SDK, release scripts, runbooks do not apply). Write the list of files you checked
  and the decision for each into the PR notes. Fix a real miss in a `fix(rs)` or `fix(ci)` commit.

- [ ] **Step 2: Run the full gate graph as CI runs it.** The command is the `ci-targets` block of
  the root `CLAUDE.md`, copied without its markers:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site \
  :http-extractor-envelope :input-liveness :promtool :observability-drift \
  :nats-permissions :release-parity :release-parity-py :release-parity-ts \
  :publish-metadata :version-lockstep :workflow-credentials :pyo3-stub-drift :ruff-ci \
  :next-public-free :helm-render :moon-diagnosis-exec :wasm-lockstep :test-e2e \
  --base origin/main --include-relations
```

  No single local bash runs every gate (root `CLAUDE.md`, "This development Mac only"). Run the
  graph with one bash, then re-run the gates that need the other bash directly:
  `/bin/bash ci/affected-graph/run.sh` (3.2); `/opt/homebrew/bin/bash` for `ci/ruff/run.sh`,
  `ci/next-public/run.sh`, `ci/publish-metadata/run.sh`, `ops/nats/check-subjects.sh` plus the
  nats nextest half, and `ci/actionlint/run.sh` (only with a healthy pipe preflight). Read those
  results in place of the `moon ci` verdict for them. For a failure, follow the root `CLAUDE.md`
  diagnosis procedure, Step 0 first. Expected: all green. If a gate fails for a host reason
  (bash version, small pipe), record it with the evidence; never weaken a gate (A4).

- [ ] **Step 3: Image budget (COORDINATOR-RUN).** The coordinator pushes the branch and runs
  `gh workflow run images.yml --ref feature/sma-786-agent-host-skeleton`. A `workflow_dispatch`
  run builds both architectures (`images.yml:101`). Record for amd64 and arm64: the job time,
  and `df -h /` free disk before and after, against the last `main` run. Stop rule (§6.10): a leg
  over 45 minutes, or free disk below 3 GB, moves the agent host to its own matrix job in a
  follow-up commit on this branch. `smoke_agent_host` must pass on both legs.

- [ ] **Step 4: PR notes.** Hand the coordinator: the `rs/Cargo.lock` version-change list from
  Task 8 Step 7 (all upward), the `cargo deny check bans` result before and after Helikon, the
  Task 7 mutation result, the Task 10 local-run evidence, the sweep list, and the gate results
  with the bash used for each.

---

## Spec coverage

| Spec item | Task |
|---|---|
| G1, §6.1 | 1, 5 |
| G2, §6.5, §6.7, §6.8 | 5, 6, 7 |
| G3, §6.11 | 10 |
| G4, §6.10 | 9, 11 |
| G5, §6.4 | 4 |
| G6, §6.3 | 1 |
| G7, §6.9 | 8 |
| G8 | 11 |
| §6.2 value objects, ports, A1 guard | 1, 2, 3 |
| §6.6 config | 5 |
| §6.12 registration points | 1, 5, 6, 7, 8, 9, 11 |
| §7 builder items (field types, lock key, README, test model, moon keys, names) | 2, 5, 8, 10 |
| §8 done-when | 4, 6, 7, 8, 9, 10, 11 |

Not builder tasks: the §7 items handed to SMA-787, SMA-790 and SMA-795, and the ADR-0028
corrections (the coordinator owns them).
