# SMA-852 Agent-Definition Registry Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move the agent definitions from `[[agents]]` in `agent-host.toml` into an `agent` table that
an operator manages through a REST and gRPC API, so that a new or archived agent takes effect on the
next worker poll without a restart.

**Architecture:** The core crate gets two ports (`AgentRegistry`, `RunCapabilities`) and the record
types. The service crate gets the adapters (`PgAgentRegistry`, `PgAgentResolver`,
`HelikonCapabilities`), one application service (`AgentAdmin`) that holds the field rules and the
authorization, and two thin transports over it (REST routes on `http_addr`, `AgentService` on a new
`grpc_addr`). The worker filter becomes a join on `agent` that binds the model ids of the binary.

**Tech Stack:** Rust edition 2024, tokio, axum, tonic 0.14, SeaORM 2.0.4 on Postgres 16, buf
(prost/tonic, protobuf-es, betterproto2), testcontainers, nextest.

**Spec:** `docs/superpowers/specs/2026-11-28-sma-852-agent-registry-design.md` (Gate 1 approved by Sven
on 2026-11-28). Read it with this plan. Do not re-open its decisions.

Paths use three short names:

- `core/` = `rs/crates/libs/paigasus-agent-host-core/`
- `svc/` = `rs/crates/services/paigasus-agent-host/`
- `iamcore/` = `rs/crates/libs/paigasus-iam-core/`

## Global Constraints

- Every new Rust source file starts with `// SPDX-License-Identifier: Apache-2.0` (`#` for Python).
- Edition 2024, rust-version 1.95 (workspace values).
- The workspace sets `warnings = "deny"`. A task must not add a private or `pub(crate)` item that a
  later task first uses. A `pub` item of a library module is not dead code. Each task passes
  `cargo clippy --workspace --all-targets -- -D warnings` and
  `cargo clippy -p paigasus-agent-host --all-targets --features test-model -- -D warnings` alone.
- `paigasus-agent-host-core` keeps its non-dev dependency set equal to `ALLOWED` in
  `core/src/lib.rs:56`. A core port uses core types only (spec §5).
- Hexagonal: ports in `core/src/ports.rs`, adapters in `svc/src/adapters/`, the application service
  in `svc/src/application/agents/`.
- The lock order and the fences of SMA-787 and SMA-790 do not change. `lock_live_lease` and the run
  commit do not read `agent` (spec §5).
- Every `AdminError` and `ApiError` message is fixed text that names no caller input. The caller's
  value goes only into the error metadata (spec §5, §6.6).
- Conventional commits with a workspace scope: `feat(rs): …`, `feat(contracts): …`, `test(rs): …`,
  `docs(rs): …`. Every message ends with the line
  `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Never use `--no-verify`. No `#NNN` line
  in a commit body (commitlint `footer-leading-blank`). Do not name SMA-852's parent in a title.
- Shell setup. First run `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` on its own line.
  Run git commands in a separate line. Run Rust commands from `rs/`.
- Rust checks: `cargo nextest run -p <crate> --test <name>`, `cargo fmt --check`,
  `cargo clippy --workspace --all-targets -- -D warnings`. Run every `paigasus-agent-host` test with
  `--features test-model` (the Moon `test` task sets it; `worker_sigkill_pg` and `boot_pg` need it).
- **Postgres tests.** They start a `postgres:16-alpine` container through
  `support::start_raw_postgres()` or `support::inbox::migrated_postgres()` (`svc/tests/support/`),
  which call `paigasus_test_docker::start_or_skip`. They need a running Docker daemon. Without it they
  return early and pass, and only `svc/tests/docker_preflight.rs` fails. A new Docker-backed binary
  goes into both lists of `docker_preflight.rs`, and the count there (26 today) changes with it. This
  plan adds one binary, `agent_registry_pg`, so the count is 27.
- Every helper in `svc/tests/support/` carries
  `#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]`.
- Proto changes: `buf format -w` in `contracts/`, then `moon run contracts:fmt contracts:lint
  contracts:generate --force`. The breaking check inside a worktree needs the common git dir (memory
  note "buf breaking fails inside a worktree"):
  `cd contracts && buf breaking --against "$(git rev-parse --git-common-dir)#branch=main,subdir=contracts"`.
  In the main checkout, `moon run contracts:breaking` is the same check. Never run a bare
  `buf generate`; it deletes `error_details_pb.ts`.
- Do not read line numbers from an old copy. Read the current file before an edit. The line numbers
  below are from commit `08c4b1f0`.
- A mutation proof must compile. Run it with `--no-fail-fast`. Undo it by deleting the marked change,
  not with `git checkout --`.
- New dependency: `tokio-stream` (workspace, feature `net`) moves from `[dev-dependencies]` to
  `[dependencies]` of `svc/Cargo.toml` in Task 11. It is already in `rs/Cargo.lock`. No other crate.

## Decisions this plan makes (spec §10 and gaps)

- **Error reasons.** New values in the shared range, after `STORE_UNAVAILABLE = 910`:
  `AGENT_NAME_TAKEN = 911`, `INBOX_EXISTS = 912`, `AGENT_VERSION_MISMATCH = 913`,
  `AGENT_ARCHIVED = 914`, `UNKNOWN_MODEL = 915`, `UNKNOWN_TOOL = 916`, `INVALID_AGENT_FIELD = 917`.
  `INVALID_PAGE_TOKEN` reuses the existing `INVALID_CURSOR` (35, "an opaque pagination cursor was not
  a well-formed token"): the meaning is the same. A page size out of range reuses
  `INVALID_PAGINATION` (10). The registry goes from 93 to 100 reasons.
- **Error domain.** gRPC `ErrorInfo` needs a domain, and no agent-host domain exists. Add
  `ERROR_DOMAIN_AGENTS = 3` (`"agents.paigasus.io"`) and `AGENTS_DOMAIN` in
  `rs/crates/libs/paigasus-proto/src/error.rs`.
- **HTTP metadata.** The REST error envelope gets an optional third key:
  `{"error":{"code","message","metadata":{…}}}`. The key is present only when the metadata is not
  empty. Ingress answers do not change.
- **Where the service lives.** `AgentService` goes into `agents.proto`, as the spec says. Rule 3 of
  that file's header ("each message is the `data` of one CloudEvents event") gets one sentence: the
  request and response messages of `AgentService` are not event data.
- **Page token.** Plain text `v1.<scope uuid>.<0|1>.<last uuid>`, opaque by contract. The scope is
  the org (agents) or the agent (inboxes); the flag is `include_archived` (always `0` for inboxes).
  Rows are ordered by `id` (UUIDv7 from `mint_uuid`). A token with another scope or flag, or one that
  does not parse, gives `INVALID_CURSOR`. Page size `0` or absent means 50 on both transports.
- **Lease store.** `PgLeaseStore::new` keeps its two parameters (45 call sites). `serving(Vec<AgentId>)`
  becomes `serving_models(Vec<String>)`. An empty list makes `acquire_next` return `None`.
- **By-id calls over gRPC.** A gRPC request names a PRN. `AgentAdmin` receives an `ResourceKey` with the
  PRN's org. A row whose org differs from that org is handled exactly as an unknown id (the probe).
- **Fired timer of an archived agent.** "Drops the fired timer" (spec §6.3) means: the scheduler
  sets the timer to `cancelled` in the same transaction, writes no event, and logs a warning.
- **Archived-pending gauge.** `agent_host_archived_pending_events` goes into
  `paigasus-observability/src/names.rs` as `AGENT_HOST_ARCHIVED_PENDING_EVENTS`, read in the same
  statement as the lag (one more sub-select), so `acquire_next` makes no new round trip.

## Review Focus

1. **Instructions with a NUL character.** Postgres `text` refuses U+0000. Expect `INVALID_AGENT_FIELD`
   with `field = instructions` (400), not a store error (503). Test in Task 6.
2. **A name that is unique only by case or by Unicode form.** `Triage` and `triage` are two names
   (spec: case-sensitive). Expect both creates to succeed, and the index to hold the exact bytes.
   Test in Task 5.
3. **A page token from one org replayed on another org, and a token from `include_archived=true`
   replayed with `false`.** Expect `INVALID_CURSOR` in both cases, never rows of the other scope.
   Test in Task 6.
4. **An agent PRN over gRPC whose uuid exists in another org.** Expect the same 403 (or 503 during an
   outage) as an unknown id, with one probe self-query against the nil org. Test in Task 11.
5. **An event insert that races an archive.** An insert that read the agent as active before the
   archive commits writes a `pending` row. Expect the row to stay `pending`, to be left out of the lag,
   and to be counted by the archived-pending gauge. Test in Task 9.

---

### Task 0: ADR-0028 addition and the SMA-793 condition

**Done by the coordinator, not a subagent.** No code.

- [ ] **Step 1:** Add the text of spec §6.12 to ADR-0028 in Notion, word for word. The status stays
  Proposed.
- [x] **Step 2 (done 2026-11-28, before this plan):** Add a comment on SMA-793 with the condition of spec §6.4: SMA-793 must check the
  inbox principal (membership, or the `owner` of a service account) before a run acts for it, and
  SMA-792 has the same condition.

---

### Task 1: IAM actions `ManageAgents` and `ReadAgents`, and the `agent_admin` role

**Files:**
- Modify: `iamcore/src/authz/action.rs:61` (enum), `:102-160` (`ALL`), `:163-222` (`as_wire`),
  `:239-298` (`is_write`), `:335-404` (`all_covers_every_variant`, count 55 → 57)
- Modify: `iamcore/src/authz/schema.rs:11-30` (`SCHEMA_SRC`)
- Modify: `iamcore/src/authz/roles.rs:76-78` (`STARTER_POLICY_REVISION` 8 → 9), `:84-96`
  (`STARTER_POLICY_IDS`), `:108` (`EXPECTED_STARTER_CONTENT_HASH`), `:113-238` (`ORG_ADMIN_ACTIONS`),
  `:240-242`, `:247-321` (`system_roles`), `:376-395` (`template_source`)

**Interfaces:**
- Produces: `Action::ManageAgents` (wire `"ManageAgents"`, non-restore write) and
  `Action::ReadAgents` (wire `"ReadAgents"`, read). Role key `agent_admin`, template id
  `agent_admin`, scope kinds `[NodeKind::Organization]`, actions `[ManageAgents, ReadAgents]`,
  description "Manage the agents and the agent inboxes of an organization.".
  `org_admin` gains `ReadAgents` only.

- [ ] **Step 1: Write the failing tests** in the existing test modules:

```rust
// action.rs
#[test]
fn the_agent_registry_actions_are_classified_and_round_trip() {
    assert_eq!(Action::parse("ManageAgents"), Some(Action::ManageAgents));
    assert_eq!(Action::parse("ReadAgents"), Some(Action::ReadAgents));
    assert!(Action::ManageAgents.is_write() && !Action::ManageAgents.is_restore());
    assert!(!Action::ReadAgents.is_write());
}
// all_covers_every_variant: the count becomes 57 and the message gains "+ 2 SMA-852 agent registry".

// schema.rs
#[test]
fn the_agent_registry_actions_validate_against_the_embedded_schema() { /* both names, as the
   tool-policy test at :93 does */ }

// roles.rs — rows in starter_policy_table:
// "agent_admin allows ManageAgents on its own org (SMA-852)"            -> allow
// "agent_admin allows ReadAgents on its own org (SMA-852)"              -> allow
// "agent_admin denies ManageAgents on another org (SMA-852)"            -> deny
// "org_admin allows ReadAgents on its own org (SMA-852)"                -> allow
// "org_admin denies ManageAgents on its own org (SMA-852)"              -> deny
// "org_member denies ReadAgents and ManageAgents (SMA-852)"             -> deny (two rows)
// "forbid-archived-writes denies ManageAgents on an archived org even for agent_admin (SMA-852)" -> deny
#[test]
fn agent_admin_is_organization_only() {
    assert_eq!(role("agent_admin").unwrap().scope_kinds, vec![NodeKind::Organization]);
}
```

- [ ] **Step 2: Run them and see them fail.**
  Run: `cargo nextest run -p paigasus-iam-core`
  Expected: FAIL (`no variant named ManageAgents`).

- [ ] **Step 3: Implement.** Add the two variants after `WriteInboxEvent`. Add both names to
  `SCHEMA_SRC`. Add `AGENT_ADMIN_ACTIONS`, the `agent_admin` `Role`, its `STARTER_POLICY_IDS` entry and
  its `template_source` arm. Add `Action::ReadAgents` to `ORG_ADMIN_ACTIONS`. Set the revision to 9
  and extend its doc comment ("`9`: SMA-852 added `ManageAgents` … and the `agent_admin` template").
  Run the content-hash test once, and copy the new hash from its failure text into
  `EXPECTED_STARTER_CONTENT_HASH`.

- [ ] **Step 4: Run the tests and see them pass.**
  Run: `cargo nextest run -p paigasus-iam-core` and
  `cargo nextest run -p paigasus-iam --test authz_bootstrap` (Docker).
  Expected: PASS. `authz_bootstrap` compares row counts with `system_roles().len()`, so it must pass
  unchanged.

- [ ] **Step 5: Commit.** `feat(rs): add the ManageAgents and ReadAgents IAM actions and the agent_admin role`

---

### Task 2: The contract — `AgentService`, the new error reasons and the agents domain

**Files:**
- Modify: `contracts/proto/paigasus/agents/v1/agents.proto` (header rule 3; new service and messages)
- Modify: `contracts/proto/paigasus/common/v1/error.proto` (`ErrorDomain`, seven `ErrorReason` values)
- Regenerate (do not hand-edit): `rs/crates/libs/paigasus-proto/src/generated/**`,
  `ts/packages/paigasus-proto/src/generated/**`,
  `py/packages/paigasus-proto/src/paigasus_proto/generated/**`
- Modify: `rs/crates/libs/paigasus-proto/src/lib.rs:17-23` (include `paigasus.agents.v1.tonic.rs`)
- Modify: `rs/crates/libs/paigasus-proto/src/error.rs:22-31` (`AGENTS_DOMAIN`), `EXPECTED_REASONS`,
  `:274` (93 → 100), the domain test at `:386`
- Modify: `ts/packages/paigasus-proto/src/error.test.ts:60` (93 → 100),
  `ts/packages/paigasus-sdk/src/errors/presentation.ts` (seven entries, the count comment),
  `ts/packages/paigasus-sdk/tests/presentation.test.ts:13` (93 → 100)

**Interfaces:**
- Produces (proto, package `paigasus.agents.v1`):

```proto
service AgentService {
  rpc CreateAgent(CreateAgentRequest) returns (CreateAgentResponse);
  rpc GetAgent(GetAgentRequest) returns (GetAgentResponse);
  rpc ListAgents(ListAgentsRequest) returns (ListAgentsResponse);
  rpc UpdateAgent(UpdateAgentRequest) returns (UpdateAgentResponse);
  rpc ArchiveAgent(ArchiveAgentRequest) returns (ArchiveAgentResponse);
  rpc CreateInbox(CreateInboxRequest) returns (CreateInboxResponse);
  rpc GetInbox(GetInboxRequest) returns (GetInboxResponse);
  rpc ListInboxes(ListInboxesRequest) returns (ListInboxesResponse);
}
message Agent { string agent_prn = 1; string org_prn = 2; string name = 3; string model = 4;
  string instructions = 5; repeated string tools = 6; int64 version = 7;
  google.protobuf.Timestamp create_time = 8; google.protobuf.Timestamp update_time = 9;
  google.protobuf.Timestamp archive_time = 10; }               // archive_time unset = active
message Inbox { string inbox_prn = 1; string agent_prn = 2; string principal_prn = 3;
  google.protobuf.Timestamp create_time = 4; google.protobuf.Timestamp agent_archive_time = 5; }
// CreateAgentRequest { org_prn = 1, name = 2, model = 3, instructions = 4, repeated tools = 5 }
// GetAgentRequest / ArchiveAgentRequest { agent_prn = 1 }
// ListAgentsRequest { org_prn = 1, bool include_archived = 2, uint32 page_size = 3, page_token = 4 }
// UpdateAgentRequest { agent_prn = 1, int64 version = 2, name = 3, model = 4, instructions = 5, repeated tools = 6 }
// CreateInboxRequest { agent_prn = 1, principal_prn = 2 }   GetInboxRequest { inbox_prn = 1 }
// ListInboxesRequest { agent_prn = 1, uint32 page_size = 2, page_token = 3 }
// Each XxxResponse holds `Agent agent = 1` or `Inbox inbox = 1`; the list responses hold
// `repeated Agent agents = 1` / `repeated Inbox inboxes = 1` and `string next_page_token = 2`.
```

- Produces (Rust): `paigasus_proto::paigasus::agents::v1::agent_service_server::{AgentService, AgentServiceServer}`
  and the client; `ErrorReason::{AgentNameTaken, InboxExists, AgentVersionMismatch, AgentArchived,
  UnknownModel, UnknownTool, InvalidAgentField}`; `ErrorDomain::Agents`;
  `paigasus_proto::error::AGENTS_DOMAIN: LazyLock<String>` = `"agents.paigasus.io"`.
- Each new `error.proto` value carries the comment form of the file: the kebab literal, the HTTP
  status, and "Emitted by the agent host (SMA-852)". `INBOX_EXISTS` names its metadata key `inbox`,
  `AGENT_VERSION_MISMATCH` `version`, `UNKNOWN_MODEL` `model`, `UNKNOWN_TOOL` `tool`,
  `INVALID_AGENT_FIELD` `field`.

- [ ] **Step 1: Write the failing tests.** In `paigasus-proto/src/error.rs`: add the seven kebab strings
  to `EXPECTED_REASONS`, set the count to 100, and assert
  `assert_eq!(&*crate::error::AGENTS_DOMAIN, "agents.paigasus.io")`. Set the two TS counts to 100.
- [ ] **Step 2: Run them and see them fail.**
  Run: `cargo nextest run -p paigasus-proto`. Expected: FAIL (count 93, unknown `AGENTS_DOMAIN`).
- [ ] **Step 3: Edit the two proto files**, then format and regenerate:
  `cd contracts && buf format -w`, then `moon run contracts:fmt contracts:lint contracts:generate --force`.
  Add the `include!` of `paigasus.agents.v1.tonic.rs`, `AGENTS_DOMAIN`, and the seven
  `[ErrorReason.X]: 'from-transport'` entries with one `// SMA-852` comment line.
- [ ] **Step 4: Run the checks.**
  Run: `cargo nextest run -p paigasus-proto`, `cargo build --workspace`,
  `moon run paigasus-proto-ts:test paigasus-sdk-ts:test paigasus-proto-py:test`, and the breaking check
  of Global Constraints. Expected: PASS; `buf breaking` prints nothing.
  Run: `git status --short`. Expected: only the files listed above, and no deleted
  `error_details_pb.ts`.
- [ ] **Step 5: Commit.** `feat(contracts): add the AgentService contract and the agent registry error reasons`

---

### Task 3: Core types and the `AgentRegistry` and `RunCapabilities` ports

**Files:**
- Create: `core/src/registry.rs`
- Modify: `core/src/lib.rs:7-23` (`pub mod registry;`), `:30-49` (port test)
- Modify: `core/src/ports.rs` (two traits, after `AgentResolver` at `:226-233`)

**Interfaces:**
- Produces (`core/src/registry.rs`):

```rust
pub const AGENT_NAME_MAX_CHARS: usize = 64;
pub const INSTRUCTIONS_MAX_BYTES: usize = 32_768;
pub const PAGE_SIZE_DEFAULT: u32 = 50;
pub const PAGE_SIZE_MAX: u32 = 100;
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentFields { pub name: String, pub model_id: String, pub instructions: String, pub tool_names: Vec<String> }
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRecord { pub definition: AgentDefinition, pub version: u64, pub created_at: Timestamp,
                         pub updated_at: Timestamp, pub archived_at: Option<Timestamp> }
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboxRecord { pub inbox: Inbox, pub agent_archived_at: Option<Timestamp> }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageRequest { pub size: u32, pub after: Option<Uuid> }
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page<T> { pub items: Vec<T>, pub next_after: Option<Uuid> }
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RegistryError {
    #[error("not found")] NotFound,
    #[error("agent name taken")] NameTaken,
    #[error("inbox exists")] InboxExists(InboxId),
    #[error("version mismatch")] VersionMismatch { current: u64 },
    #[error("agent archived")] Archived,
    #[error("invalid field {0}")] Invalid(&'static str),
    #[error("unknown model")] UnknownModel(String),
    #[error("unknown tool")] UnknownTool(String),
    #[error("store error")] Store(String),
}
```

- Produces (`core/src/ports.rs`):

```rust
#[async_trait]
pub trait AgentRegistry: Send + Sync {
    async fn create(&self, id: &AgentId, fields: &AgentFields) -> Result<AgentRecord, RegistryError>;
    async fn get(&self, id: Uuid) -> Result<Option<AgentRecord>, RegistryError>;
    async fn list(&self, org: Uuid, include_archived: bool, page: PageRequest) -> Result<Page<AgentRecord>, RegistryError>;
    async fn update(&self, id: &AgentId, version: u64, fields: &AgentFields) -> Result<AgentRecord, RegistryError>;
    async fn archive(&self, id: &AgentId) -> Result<AgentRecord, RegistryError>;
    async fn create_inbox(&self, id: &InboxId, agent: &AgentId, principal: &PrincipalId) -> Result<InboxRecord, RegistryError>;
    async fn get_inbox(&self, id: Uuid) -> Result<Option<InboxRecord>, RegistryError>;
    async fn list_inboxes(&self, agent: &AgentId, page: PageRequest) -> Result<Page<InboxRecord>, RegistryError>;
}
pub trait RunCapabilities: Send + Sync {
    fn has_model(&self, model: &str) -> bool;
    fn has_tool(&self, tool: &str) -> bool;
    fn model_ids(&self) -> Vec<String>;
}
```

- [ ] **Step 1: Write the failing tests.** Add `assert_port::<dyn AgentRegistry>()` and
  `assert_port::<dyn RunCapabilities>()` to `every_port_is_object_safe_send_and_sync`. In
  `registry.rs`:

```rust
#[test]
fn no_registry_error_display_holds_caller_input() {
    let texts: Vec<String> = [RegistryError::UnknownModel("secret-model".into()),
        RegistryError::UnknownTool("secret-tool".into()), RegistryError::Store("db said secret".into())]
        .iter().map(ToString::to_string).collect();
    assert_eq!(texts, ["unknown model", "unknown tool", "store error"]);
}
```

- [ ] **Step 2:** Run `cargo nextest run -p paigasus-agent-host-core`. Expected: FAIL (unresolved names).
- [ ] **Step 3:** Add the file, the module and the two traits. No new dependency
  (`the_core_crate_depends_on_no_io_crate` must still pass).
- [ ] **Step 4:** Run `cargo nextest run -p paigasus-agent-host-core`. Expected: PASS.
- [ ] **Step 5: Commit.** `feat(rs): add the agent registry and run capability ports to the agent-host core`

---

### Task 4: Migration m0006 — the `agent` table and the inbox foreign key

**Files:**
- Create: `svc/src/adapters/persistence/migration/m0006_create_agent.rs`
- Modify: `svc/src/adapters/persistence/migration/mod.rs:5-26`
- Modify: `svc/tests/support/inbox.rs:23-29` (`insert_inbox`; new `insert_agent`)
- Modify: `svc/tests/boot_pg.rs:65-81` (`the_inbox_unique_constraint_holds`: insert the agent row
  first; add a second insert without an agent that fails with SQLSTATE 23503)
- Modify: `svc/tests/schema_pg.rs:166`, `:319`, `:355` and `svc/tests/memory_pg.rs:405` (each
  `Migrator::down(Some(n))` gets `n + 1` and the `expect` text names m0006)
- Modify: `ci/images/run.sh:1145-1153` (the CTE inserts an `agent` row, then the inbox with that
  agent's org and id)

**Interfaces:**
- Produces: the SQL of spec §6.1 verbatim, as `const UP: [&str; 3]` (table, partial unique index
  `agent_org_name_active`, `inbox_agent_fk`) and `const DOWN: [&str; 2]` (drop the constraint, drop
  the table). The style of `m0005_create_session_event.rs`.
- Produces (test support):

```rust
pub async fn insert_agent(db: &DatabaseConnection, org: Uuid, agent: Uuid, model: &str, instructions: &str, tools: &[&str]);
// name = the agent uuid; ON CONFLICT (id) DO NOTHING.
pub async fn insert_inbox(db: &DatabaseConnection, org: Uuid, id: Uuid, agent: Uuid, principal: Uuid);
// unchanged signature: inserts the agent row (model "scripted", instructions "test", no tools)
// with ON CONFLICT (id) DO NOTHING, then panics if the stored agent's org_id is not `org`,
// then inserts the inbox.
```

- [ ] **Step 1: Write the failing tests** in `svc/tests/schema_pg.rs`:
  - `m0006_creates_the_agent_table_and_the_inbox_foreign_key`: an inbox insert with no agent row fails
    with `SqlErr::ForeignKeyConstraintViolation`; with the agent row of the same org it succeeds; an
    inbox whose `org_id` differs from the `org_id` of its agent row fails with 23503.
  - `m0006_down_drops_the_agent_table_and_the_constraint`: `Migrator::down(&db, Some(1))`, then an
    inbox insert with no agent succeeds and `to_regclass('agent') IS NULL`.
  - `m0006_fails_on_an_inbox_without_an_agent`: run `Migrator::up(&db, Some(5))`, insert an inbox,
    run `Migrator::up(&db, None)`; it fails, `format!("{err}")` contains `inbox_agent_fk`, and the
    `agent_host_migrations` table holds 5 rows.
  - `the_agent_name_is_unique_among_active_agents_only`: two active agents of one org with one name
    give 23505 naming `agent_org_name_active`; after `UPDATE agent SET archived_at = now()` on the
    first, the second insert succeeds.
- [ ] **Step 2:** Run `cargo nextest run -p paigasus-agent-host --features test-model --test schema_pg`.
  Expected: FAIL (no `agent` table).
- [ ] **Step 3:** Add the migration and register it. Change `insert_inbox` and add `insert_agent`.
  Update the `down` counts, `boot_pg.rs:74` and `ci/images/run.sh`.
- [ ] **Step 4:** Run the whole service suite:
  `cargo nextest run -p paigasus-agent-host --features test-model --no-fail-fast`. Expected: PASS. All
  23 binaries that call `insert_inbox` pass through the new helper. `ConfigAgentResolver` still serves
  the definitions, so no other test changes. Run `bash -n ci/images/run.sh`; expected: no output.
- [ ] **Step 5: Commit.** `feat(rs): add the agent table migration of the agent host`

---

### Task 5: `PgAgentRegistry` — the Postgres adapter of `AgentRegistry`

**Files:**
- Create: `svc/src/adapters/persistence/agent_registry.rs`
- Modify: `svc/src/adapters/persistence/mod.rs:3-13`
- Create: `svc/tests/agent_registry_pg.rs`
- Modify: `svc/tests/docker_preflight.rs:3-20` (add `agent_registry_pg`, count 26 → 27)

**Interfaces:**
- Consumes: Task 3 types and port; Task 4 schema.
- Produces:

```rust
pub struct PgAgentRegistry { db: DatabaseConnection }
impl PgAgentRegistry {
    pub fn new(db: DatabaseConnection) -> Self;
    /// The active agents, for the boot warning of Task 7.
    pub async fn active_models(&self) -> Result<Vec<(AgentId, String)>, RegistryError>;
}
impl AgentRegistry for PgAgentRegistry { /* all eight methods */ }
```

- Pinned behaviour (spec §6.3, §6.4):
  - `create`: one `INSERT … RETURNING`. SQLSTATE 23505 on `agent_org_name_active` → `NameTaken`.
  - `update`: the statement of spec §6.3. No row: read the row again in the same transaction, and
    return `NotFound` (no row in that org), `Archived` (`archived_at` set) or
    `VersionMismatch { current }`. 23505 → `NameTaken`.
  - `archive`: one transaction: `UPDATE agent SET archived_at = COALESCE(archived_at, now())
    WHERE org_id = $1 AND id = $2 RETURNING …`, then `UPDATE timer SET status = 'cancelled',
    updated_at = now() WHERE status = 'active' AND inbox_id IN (SELECT id FROM inbox WHERE org_id = $1
    AND agent_id = $2)`. A second archive returns the same `archived_at`.
  - `create_inbox`: one transaction: `SELECT archived_at FROM agent WHERE org_id = $1 AND id = $2
    FOR SHARE` (no row → `NotFound`, set → `Archived`), then
    `INSERT … ON CONFLICT (agent_id, principal_id) DO NOTHING RETURNING …` (a 23505 would abort the
    transaction). No row returned → read the existing id in the same transaction → `InboxExists(InboxId)`. An `InboxId` whose org
    differs from the agent's org → `Invalid("org")` (from `Inbox::new`'s `OrgMismatch`), with no SQL.
  - `list` / `list_inboxes`: `WHERE org_id = $1 [AND archived_at IS NULL] AND ($after IS NULL OR id >
    $after) ORDER BY id LIMIT size + 1`. `next_after` is the id of the last returned row when a
    `size + 1`-th row exists.
  - Every method that takes an `AgentId` filters on both `org_id` and `id`, so another org gives
    `NotFound`.

- [ ] **Step 1: Write the failing tests** in `svc/tests/agent_registry_pg.rs` (one Postgres per test,
  through `migrated_postgres()`):

```text
create_then_get_returns_version_1_and_no_archived_at
list_pages_by_id_and_stops_with_no_next_after          // 5 agents, size 2: 2, 2, 1; next_after None at the end
list_leaves_out_archived_agents_unless_asked
update_with_the_right_version_bumps_the_version_and_updated_at
update_with_a_wrong_version_is_version_mismatch_with_the_current_version
update_of_an_archived_agent_is_archived
update_to_a_name_of_another_active_agent_is_name_taken
archive_twice_returns_the_same_archived_at
archive_cancels_the_active_timers_of_the_agents_inboxes_only   // a timer of another agent stays active
an_archived_name_is_free_again
names_that_differ_only_by_case_are_two_names                   // Review Focus 2
create_inbox_then_get_inbox
a_second_bind_is_inbox_exists_with_the_existing_id
a_bind_to_an_archived_agent_is_archived
a_bind_with_an_inbox_id_of_another_org_is_invalid_org
an_agent_id_of_another_org_is_not_found                         // get-by-AgentId paths: update, archive, list_inboxes
get_inbox_of_an_archived_agent_sets_agent_archived_at
two_creates_with_one_name_on_two_connections_exactly_one_succeeds
two_updates_with_one_version_on_two_connections_exactly_one_succeeds
a_bind_and_an_archive_that_race_never_leave_an_inbox_after_the_archive
    // connection A: BEGIN, archive's UPDATE, hold; connection B: create_inbox blocks on FOR SHARE;
    // commit A; B returns Archived. Then the reverse order: B commits first, A succeeds.
```

  Use `support::connect_pinned` for the two connections of each race test.
- [ ] **Step 2:** Run `cargo nextest run -p paigasus-agent-host --features test-model --test agent_registry_pg`.
  Expected: FAIL (unresolved `PgAgentRegistry`).
- [ ] **Step 3:** Implement with raw `Statement::from_sql_and_values` as `inbox_repository.rs` does.
  Times cross as epoch microseconds (`persistence/time.rs`). Read the SQLSTATE with `DbErr::sql_err()`
  and the constraint name from the error text.
- [ ] **Step 4:** Run the same command, then `docker_preflight`. Expected: PASS.
- [ ] **Step 5: Commit.** `feat(rs): add the Postgres agent registry of the agent host`

---

### Task 6: `HelikonCapabilities` and the `AgentAdmin` application service

**Files:**
- Create: `svc/src/adapters/helikon/capabilities.rs`
- Modify: `svc/src/adapters/helikon/mod.rs:5-11`, `svc/src/adapters/helikon/model_registry.rs:59-79`
  (add `ids`)
- Modify: `svc/src/config.rs:95-96` (`KNOWN_TOOLS` moves to `capabilities.rs`; `config.rs` imports it
  until Task 8)
- Modify: `svc/src/adapters/http/auth.rs:40-42` (two action constants), `:170-184` (the probe takes
  the action)
- Create: `svc/src/application/agents/mod.rs`, `svc/src/application/agents/error.rs`,
  `svc/src/application/agents/page_token.rs`
- Modify: `svc/src/application/mod.rs`

**Interfaces:**
- Consumes: Task 3 ports; `adapters::iam::Iam`; `adapters::http::auth::{Caller, CallerKind}`;
  `paigasus_agent_host_core::ports::IdSource` (`adapters::id::MintIds` in production).
- Produces:

```rust
// adapters/helikon/capabilities.rs
pub const KNOWN_TOOLS: &[&str] = &[MEMORY_TOOL_NAME];
pub struct HelikonCapabilities { models: Arc<ModelRegistry> }
impl HelikonCapabilities { pub fn new(models: Arc<ModelRegistry>) -> Self; }
impl RunCapabilities for HelikonCapabilities { /* model_ids: sorted */ }
// model_registry.rs
impl ModelRegistry { pub fn ids(&self) -> Vec<String>; }

// adapters/http/auth.rs
pub const MANAGE_AGENTS_ACTION: &str = "ManageAgents";
pub const READ_AGENTS_ACTION: &str = "ReadAgents";
/// Spec §6.5 steps 3 and 4: key scope (probe on refusal), then the self-query. No owner path.
pub async fn authorize_org(iam: &dyn Iam, caller: &Caller, org: Uuid, action: &'static str) -> Result<(), ApiError>;
/// Spec §6.5 step 2: the probe against the nil org with `action`.
pub async fn deny_unknown(iam: &dyn Iam, caller: &Caller, action: &'static str) -> ApiError;
// `deny_unknown_inbox` and `authorize_write` keep their signatures and pass WRITE_INBOX_EVENT_ACTION.

// application/agents/error.rs
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdminError {
    InvalidToken, Forbidden, IamUnavailable, StoreUnavailable, Internal,
    NameTaken, InboxExists(InboxId), VersionMismatch { current: u64 }, Archived,
    UnknownModel(String), UnknownTool(String), InvalidField(&'static str),
    InvalidPagination, InvalidPageToken, InvalidRequestBody, InvalidRequestSchema, InvalidUuid,
}
impl From<ApiError> for AdminError;   // InvalidToken, Forbidden, IamUnavailable, Internal; others -> Internal
impl From<RegistryError> for AdminError;  // Store -> StoreUnavailable (logged), NotFound -> Internal (logged)

// application/agents/page_token.rs
pub fn encode(scope: Uuid, include_archived: bool, after: Uuid) -> String;   // "v1.<scope>.<0|1>.<after>"
pub fn decode(token: &str, scope: Uuid, include_archived: bool) -> Result<Option<Uuid>, AdminError>;  // "" -> Ok(None)

// application/agents/mod.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceKey { pub id: Uuid, pub org: Option<Uuid> }   // org: the PRN's org over gRPC, None over REST
pub struct AgentAdmin { registry: Arc<dyn AgentRegistry>, capabilities: Arc<dyn RunCapabilities>, iam: Arc<dyn Iam>, ids: Arc<dyn IdSource> }
impl AgentAdmin {
    pub fn new(registry: Arc<dyn AgentRegistry>, capabilities: Arc<dyn RunCapabilities>, iam: Arc<dyn Iam>, ids: Arc<dyn IdSource>) -> Self;
    pub async fn create(&self, caller: &Caller, org: Uuid, fields: AgentFields) -> Result<AgentRecord, AdminError>;
    pub async fn get(&self, caller: &Caller, agent: ResourceKey) -> Result<AgentRecord, AdminError>;
    pub async fn list(&self, caller: &Caller, org: Uuid, include_archived: bool, page_size: u32, page_token: &str) -> Result<(Vec<AgentRecord>, Option<String>), AdminError>;
    pub async fn update(&self, caller: &Caller, agent: ResourceKey, version: u64, fields: AgentFields) -> Result<AgentRecord, AdminError>;
    pub async fn archive(&self, caller: &Caller, agent: ResourceKey) -> Result<AgentRecord, AdminError>;
    pub async fn create_inbox(&self, caller: &Caller, agent: ResourceKey, principal: PrincipalId) -> Result<InboxRecord, AdminError>;
    pub async fn get_inbox(&self, caller: &Caller, inbox: ResourceKey) -> Result<InboxRecord, AdminError>;
    pub async fn list_inboxes(&self, caller: &Caller, agent: ResourceKey, page_size: u32, page_token: &str) -> Result<(Vec<InboxRecord>, Option<String>), AdminError>;
}
```

- The order in every method (spec §6.5): resolve the org (the request org, or `registry.get*` by id;
  a row whose org differs from `ResourceKey.org` counts as no row) → no row: `deny_unknown(action)` →
  `authorize_org(org, action)` → the page size and token → the field rules → the model and tools →
  the registry call. So a caller without the right gets 403 for bad fields too.
- The field rules: name 1..=64 chars, no `char::is_control` (NUL included), no outer whitespace →
  `InvalidField("name")`; instructions 1..=32768 bytes and no U+0000 → `InvalidField("instructions")`;
  a tool listed twice → `InvalidField("tools")`; then `UnknownModel(model)`, then the first
  `UnknownTool(tool)`. Page size 0 → 50; 1..=100 kept; above 100 → `InvalidPagination`.
- Each write logs at info: `action`, `principal`, `agent`, `inbox` (bind only), `version`. Never the
  instructions (spec §6.5).

- [ ] **Step 1: Write the failing tests** as in-module tests of `application/agents/mod.rs`, with an
  in-module recording `Iam` fake (the shape of `FakeIam` at `auth.rs:388-421`: it records each
  self-query as `(credential, principal PRN, action, org PRN)`) and an in-memory `AgentRegistry` fake:

```text
create_self_queries_manage_agents_on_the_request_org              // exact tuple
list_self_queries_read_agents_on_the_request_org
get_of_an_unknown_id_probes_the_nil_org_with_read_agents_and_is_forbidden
get_of_an_unknown_id_during_an_outage_is_iam_unavailable
get_with_an_agent_key_of_another_org_probes_like_an_unknown_id
a_key_scoped_to_another_org_is_forbidden_after_a_probe
a_key_scoped_to_another_org_during_an_outage_is_iam_unavailable
read_agents_without_manage_agents_is_forbidden_on_every_write    // create, update, archive, bind
a_refused_caller_gets_forbidden_also_for_invalid_fields
each_name_rule_is_invalid_field_name                              // "", 65 chars, "\u{7}", "\0", " a", "a "
the_name_limits_are_inclusive                                    // 1 and 64 chars pass
instructions_over_32768_bytes_or_empty_or_with_nul_are_invalid    // Review Focus 1
a_duplicate_tool_is_invalid_field_tools
an_unknown_model_is_unknown_model_with_the_name
an_unknown_tool_is_unknown_tool_with_the_name
page_size_0_is_50_and_101_is_invalid_pagination
a_page_token_of_another_org_or_another_filter_is_invalid_page_token   // Review Focus 3
a_garbled_page_token_is_invalid_page_token
the_next_page_token_round_trips
```

  And in `capabilities.rs`: `has_model` true only for a registered id; `has_tool("memory")` true and
  `has_tool("shell")` false; `model_ids` sorted.
- [ ] **Step 2:** Run `cargo nextest run -p paigasus-agent-host --lib`. Expected: FAIL.
- [ ] **Step 3:** Implement. Refactor `refusal_after_probe(iam, caller, org)` to take the action. The
  existing ingress tests in `auth.rs` must pass unchanged.
- [ ] **Step 4:** Run `cargo nextest run -p paigasus-agent-host --features test-model --lib` and
  `--test ingress_http`. Expected: PASS.
- [ ] **Step 5: Commit.** `feat(rs): add the AgentAdmin service with the agent field rules and authorization`

---

### Task 7: `PgAgentResolver`, the joined filter, the archived-pending gauge and the done-when test

**Files:**
- Modify: `svc/src/adapters/agents.rs` (replace `ConfigAgentResolver` with `PgAgentResolver`)
- Modify: `svc/src/adapters/persistence/lease_store/mod.rs:26-69` (`lag_select!`, `CANDIDATES_SQL`,
  `LAG_SQL`), `:138-165` (`agents` → `models`), `:178-214`
- Modify: `rs/crates/libs/paigasus-observability/src/names.rs` (`AGENT_HOST_ARCHIVED_PENDING_EVENTS`,
  in `ALL` and in `the_agent_host_worker_families_are_registered`)
- Modify: `svc/src/main.rs:20,92-105,187,316-361,366-425,534-537` (the resolver, the pool start rule,
  the boot warning, the gauge description; the lag description says "the agents that this host can
  take")
- Modify: `svc/tests/agents_pg.rs` (all of it), `svc/tests/worker_pool_pg.rs:75-164`,
  `svc/tests/lease_pg.rs`, `svc/tests/worker_pg.rs`, `svc/tests/worker_crash_pg.rs` (every
  `.serving(…)` and `ConfigAgentResolver`), `svc/tests/boot_pg.rs:289-421`,
  `svc/tests/worker_sigkill_pg.rs:115-130` (agent rows replace `[[agents]]`)
- Check, change only if needed: `svc/src/application/worker/test_fakes.rs:220-230`

**Interfaces:**
- Consumes: Task 4 helpers; Task 5 `PgAgentRegistry::active_models`; Task 6 `HelikonCapabilities`
  and `AgentAdmin`.
- Produces:

```rust
pub struct PgAgentResolver { db: DatabaseConnection }
impl PgAgentResolver { pub fn new(db: DatabaseConnection) -> Self; }
impl AgentResolver for PgAgentResolver { /* one query: inbox JOIN agent ON (org_id, agent_id), archived_at IS NULL */ }

impl PgLeaseStore { pub fn serving_models(self, models: Vec<String>) -> Self; }   // replaces `serving`
/// The boot warning (spec §6.7): one `warn!` per active agent whose model is not in `caps`.
pub async fn warn_unknown_models(registry: &PgAgentRegistry, caps: &dyn RunCapabilities) -> Result<usize, RegistryError>;
// (in adapters/persistence/agent_registry.rs)
```

- The filter (spec §6.8): a macro `active_agent_join!($alias, $models)` gives
  `JOIN agent a ON a.org_id = <alias>.org_id AND a.id = <alias>.agent_id AND a.archived_at IS NULL
  AND a.model = ANY(<models>::text[])`. `CANDIDATES_SQL`: `$1` = limit, `$2` = the model ids, alias
  `i`. `LAG_SQL`: `$1` = the model ids, alias `j`. `FOR NO KEY UPDATE OF i SKIP LOCKED` stays on `i`.
  Both statements also return `archived_pending`: the count of `pending` events of inboxes whose
  agent has `archived_at` set. `acquire_next` sets the lag gauge and the new gauge from the same row.
- `acquire(&InboxId)` keeps no filter. Its doc comment says that the resolver answers `NotFound` for
  an archived agent.
- `main.rs`: `pool_starts = config.worker.enabled`. `models.check(&agents)` and the "no [[agents]]"
  warning go. After the migration, `boot` calls `warn_unknown_models`. The resolver is
  `PgAgentResolver`; the lease store is `.serving_models(capabilities.model_ids())`.

- [ ] **Step 1: Write the failing tests.**
  - `agents_pg.rs` (rewrite): `an_inbox_of_an_active_agent_resolves_to_its_row`,
    `an_inbox_of_an_archived_agent_is_not_found`, `an_inbox_id_with_another_org_is_not_found`,
    `an_unknown_inbox_is_not_found`.
  - `worker_pool_pg.rs`: `an_inbox_of_an_archived_agent_is_not_taken_and_its_events_stay_pending`,
    `an_inbox_of_an_agent_whose_model_is_not_bound_is_not_taken`,
    `a_store_that_binds_no_model_takes_nothing` (replaces `:154`),
    `both_lag_paths_leave_out_archived_and_unbound_inboxes` (one run with a candidate row, one with
    none; the gauge equals the age of the only takeable event),
    `the_archived_pending_gauge_counts_the_pending_events_of_an_archived_agent`.
  - The done-when test, `worker_pool_pg.rs`:
    `an_agent_created_while_the_pool_runs_is_served_without_a_restart`. Start `run_worker_pool` with no
    agent row. Through `AgentAdmin` (on `PgAgentRegistry`, `HelikonCapabilities` with `scripted`, an
    allow-all `Iam` stub) create the agent and bind the inbox. Insert a pending event. Wait until the
    event is `done` (`wait_until`, 30 s). Assert one `run` row with status `completed`.
  - `boot_pg.rs`: `with_no_agents_the_pool_starts_and_takes_nothing` (replaces `:289`; the log line
    "the worker pool started" appears), `an_active_agent_with_an_unknown_model_logs_a_warning_and_boots`
    (replaces `:306`; `/readyz` 200 and the warning line), `with_an_agent_row_the_pool_serves_a_pending_event`
    (replaces `:374`). `the_s3_memory_store_with_an_agent_fails_the_boot` (`:339`) stays as it is
    until Task 8.
- [ ] **Step 2:** Run `cargo nextest run -p paigasus-agent-host --features test-model --test agents_pg
  --test worker_pool_pg`. Expected: FAIL.
- [ ] **Step 3:** Implement. Mutation proof: remove `AND a.archived_at IS NULL` from the macro and run
  `--test worker_pool_pg --no-fail-fast`; the archived-agent test must fail. Delete the marked change.
- [ ] **Step 4:** Run `cargo nextest run -p paigasus-agent-host --features test-model --no-fail-fast`
  and `cargo nextest run -p paigasus-observability`. Expected: PASS.
- [ ] **Step 5: Commit.** `feat(rs): resolve agents from the agent table and join it in the worker filter`

---

### Task 8: Config — remove `[[agents]]`, add `grpc_addr`, widen the S3 guard

**Files:**
- Modify: `svc/src/config.rs:10-12`, `:64-93` (`Config`), `:95-96`, `:405-416` (`AgentConfig` goes),
  `:529-620` (`ConfigError`), `:859-1015` (`validate`, `agent_definitions` goes), tests `:1514-1600`,
  `:2067-2170`
- Modify: `svc/src/adapters/helikon/model_registry.rs:45-79,139-148` (`UnknownModel` and `check` go;
  no caller is left)
- Modify: `svc/tests/boot_pg.rs:339-372` (the S3 boot test needs no agent now)

**Interfaces:**
- Produces:

```rust
pub struct Config {
    /* … */
    /// The gRPC listener of `AgentService` (spec §6.5). Needs `[iam]`.
    #[serde(default)]
    pub grpc_addr: Option<SocketAddr>,
    /// The removed `[[agents]]` key. Only `validate` reads it, to refuse it (spec §6.9, A5).
    #[serde(default, rename = "agents", skip_serializing)]
    pub removed_agents: Option<figment::value::Value>,
}
// New ConfigError variants, with these exact texts:
#[error("`[[agents]]` is removed; use the agent API (SMA-852)")] AgentsRemoved,
#[error("grpc_addr needs an [iam] section")] GrpcNeedsIam,
#[error("grpc_addr must use a different port than http_addr and metrics.addr")] GrpcPortClash,
// WorkerNeedsPostgresMemory: the text drops "and an [[agents]] entry exists".
```

- `validate` order: the `AgentsRemoved` check first, then the existing checks, then the two
  `grpc_addr` checks after the metrics port check. The S3 guard becomes
  `matches!(backend, S3) && self.worker.enabled`.

- [ ] **Step 1: Write the failing tests** (replace `:2121-2170`):

```text
a_toml_agents_table_fails_with_agents_removed        // the four-line [[agents]] TOML of agent_toml()
an_env_agents_key_fails_with_agents_removed          // AGENT_HOST_AGENTS='[]' through figment::Jail
grpc_addr_without_iam_fails
grpc_addr_on_the_http_port_fails
grpc_addr_on_the_metrics_port_fails
grpc_addr_with_iam_and_its_own_port_is_valid
the_worker_refuses_the_s3_memory_store                // :1586: default worker.enabled with S3 fails, no agent needed;
                                                      // worker.enabled = false with S3 passes
memory_s3_loads_from_the_environment                  // :1518: sets AGENT_HOST_WORKER__ENABLED=false
```

  Also go through every other test that builds an S3 config and expects `Ok(())`; give each
  `worker.enabled = false`.
- [ ] **Step 2:** Run `cargo nextest run -p paigasus-agent-host --lib config`. Expected: FAIL.
- [ ] **Step 3:** Implement. Remove `AgentConfig`, `agent_definitions`, `AgentIdInvalid`,
  `AgentDuplicate`, `AgentUnknownTool` and the `AgentDefinition`/`AgentId` imports.
- [ ] **Step 4:** Run `cargo nextest run -p paigasus-agent-host --features test-model --no-fail-fast`.
  Expected: PASS.
- [ ] **Step 5: Commit.** `feat(rs): remove the agents list from the agent-host config`

---

### Task 9: Archive closes the inbox to new events

**Files:**
- Modify: `core/src/ports.rs:39-53` (`InsertOutcome::AgentArchived`)
- Modify: `svc/src/adapters/persistence/inbox_repository.rs:15-19` (`INSERT_SQL`), `:107-143`
  (`insert_event_in`)
- Modify: `svc/src/application/ingest.rs:29-48` (the new arm counts nothing)
- Modify: `svc/src/adapters/http/ingress.rs:129-135`, `svc/src/adapters/http/error.rs:22-125`
  (`ApiError::AgentArchived`: 409, `agent-archived`, "the agent of this inbox is archived")
- Modify: `svc/src/adapters/nats/consumer.rs:215-221` (`AgentArchived` → `Disposition::Term` and
  `warn!`)
- Modify: `svc/src/adapters/persistence/scheduler.rs:157-162` (`Fired.duplicate: bool` becomes
  `outcome: &'static str`: `"inserted"`, `"duplicate"` or `"agent_archived"`), `:213` (the label reads
  `fired.outcome`), `:269-288` (`AgentArchived`: set the timer to `cancelled` in the same
  transaction, skip `update_fired`, `warn!` with the timer id, return `Ok(Some(Fired { outcome:
  "agent_archived", … }))`, so the pass counts the row as processed). Update the metric description
  in `main.rs:504-507`.
- Tests: `svc/tests/ingress_pg.rs`, `svc/tests/nats_source.rs`, `svc/tests/scheduler_pg.rs`,
  `svc/tests/inbox_repository_pg.rs`, `svc/tests/worker_pool_pg.rs`

**Interfaces:**
- Produces: `InsertOutcome::AgentArchived` (no fields). `INSERT_SQL` becomes
  `INSERT INTO inbox_event (…) SELECT $1, $2, … WHERE EXISTS (SELECT 1 FROM inbox i JOIN agent a ON
  a.org_id = i.org_id AND a.id = i.agent_id WHERE i.id = $2 AND a.archived_at IS NULL)
  ON CONFLICT (inbox_id, source, ce_id) DO NOTHING RETURNING id`. On no row: a stored row of the key
  gives `Duplicate` as today; no stored row gives `AgentArchived`.
- HTTP answers `AGENT_ARCHIVED` only after `authorize_write` passed (spec §6.3).

- [ ] **Step 1: Write the failing tests:**

```text
inbox_repository_pg: an_insert_into_an_inbox_of_an_archived_agent_is_agent_archived_and_writes_no_row
inbox_repository_pg: a_duplicate_key_of_an_archived_agent_stays_duplicate
ingress_pg:          an_authorized_post_to_an_archived_agents_inbox_is_409_agent_archived_and_writes_no_row
ingress_pg:          an_unauthorized_post_to_an_archived_agents_inbox_is_403
nats_source:         a_message_for_an_archived_agents_inbox_is_terminated_and_writes_no_row
scheduler_pg:        a_due_timer_of_an_archived_agent_is_cancelled_and_writes_no_event
worker_pool_pg:      an_event_inserted_before_the_archive_commits_stays_pending_and_is_counted   // Review Focus 5
http/error.rs:       the registry membership test and the envelope test list AgentArchived
```

  For the scheduler test, set `archived_at` with SQL, not through `archive`, so that the timer is still
  `active` when the scheduler fires.
- [ ] **Step 2:** Run the five test binaries with `--features test-model`. Expected: FAIL.
- [ ] **Step 3:** Implement. Every exhaustive `match` on `InsertOutcome` gets the new arm.
- [ ] **Step 4:** Run `cargo nextest run -p paigasus-agent-host --features test-model --no-fail-fast`
  and `python3 ci/error-registry/check.py --single-site`. Expected: PASS; the gate prints no offender.
- [ ] **Step 5: Commit.** `feat(rs): refuse new events for an inbox of an archived agent`

---

### Task 10: The REST routes of the agent API

**Files:**
- Create: `svc/src/adapters/http/agents/mod.rs` (router and handlers),
  `svc/src/adapters/http/agents/dto.rs` (bodies, queries, answers),
  `svc/src/adapters/http/agents/error.rs` (`AgentApiError`)
- Modify: `svc/src/adapters/http/mod.rs:6-11,34-81,107-117` (`AppState.agents`; mount the routes when
  `state.agents` is set)
- Modify: `svc/src/adapters/http/path.rs` (`AgentIdField`, `NAME = "agent"`)
- Modify: `svc/src/main.rs:391-423` (build `AgentAdmin` when `[iam]` is set)
- Modify: `ci/error-registry/check.py:109-125` (a MANIFEST row)
- Modify: `svc/tests/support/stub_iam.rs` (record each self-query as `(principal PRN, action, org PRN)`;
  new `self_query_args()`)
- Create: `svc/tests/agents_http.rs`

**Interfaces:**
- Consumes: Task 6 `AgentAdmin`, `AdminError`, `ResourceKey`; `ingress::authenticate_middleware`
  pattern (`auth::authenticate`).
- Produces:

```rust
pub struct AgentApiError(pub AdminError);
impl AgentApiError {
    /// The status, the registered code, the fixed message and the metadata (spec §6.6).
    pub fn parts(&self) -> (StatusCode, &'static str, &'static str, Vec<(&'static str, String)>);
}
impl IntoResponse for AgentApiError;   // metadata key present only when not empty
pub fn agent_routes(admin: Arc<AgentAdmin>, iam: Arc<dyn Iam>) -> Router;
pub struct AppState { /* … */ pub agents: Option<Arc<AgentAdmin>> }
```

- The routes and bodies of spec §6.5, verbatim. JSON answer of an agent:
  `{"agent","org","name","model","instructions","tools","version","created_at","updated_at","archived_at"}`
  (PRNs and RFC 3339 times; `archived_at` is `null` when active). A page:
  `{"items":[…],"next_page_token":"…"|null}`. An inbox:
  `{"inbox","agent","principal","created_at","agent_archived_at"}`.
- Codes: `invalid-token` 401, `forbidden` 403 ("the caller may not perform this action on agents"),
  `iam-unavailable` 503, `store-unavailable` 503, `internal` 500, the seven §6.6 rows, `invalid-cursor`
  400, `invalid-pagination` 400, `invalid-request-body` 400, `invalid-request-schema` 400,
  `invalid-uuid` 400. A body field of the wrong JSON type is `invalid-request-schema`; a valid JSON
  `org` or `principal` that is not a PRN of the right kind is `invalid-agent-field` with `field`
  `org` or `principal`.
- MANIFEST row: `("rs/crates/services/paigasus-agent-host/src/adapters/http/agents/error.rs", "emits",
  "every_agent_api_code_is_in_the_registry", "AgentApiError::parts() — the agent API (SMA-852)")`.

- [ ] **Step 1: Write the failing tests.** In `agents/error.rs`:
  `every_agent_api_code_is_in_the_registry` (each variant's code resolves through
  `ErrorReason::from_wire_reason` to the expected reason) and
  `no_agent_api_message_holds_caller_input` (the messages of `UnknownModel("secret")`,
  `UnknownTool("secret")`, `InboxExists(…)` do not contain `secret` or the PRN). In
  `svc/tests/agents_http.rs` (no Docker: an in-memory `AgentRegistry` in the test file and `StubIam`):

```text
for each of the 8 routes: the_self_query_is_<principal, action, org PRN>_and_both_verdicts_hold
an_api_key_of_another_org_is_403_and_503_during_an_outage
an_unknown_id_is_403_after_a_probe_against_the_nil_org_and_503_during_an_outage
an_id_of_another_org_is_403_and_503_during_an_outage
an_unauthenticated_call_reads_nothing        // a registry fake that panics on any call; 401
create_is_201_with_the_record_and_bind_is_201_with_the_inbox
patch_with_a_stale_version_is_409_with_the_current_version_in_metadata
a_second_bind_is_409_inbox_exists_with_the_inbox_prn_in_metadata
a_malformed_uuid_segment_is_400_invalid_uuid
the_routes_are_not_mounted_without_iam       // 404 not-found
```

- [ ] **Step 2:** Run `cargo nextest run -p paigasus-agent-host --features test-model --test agents_http`.
  Expected: FAIL.
- [ ] **Step 3:** Implement. Reuse `bearer_token` and `authenticate`; do not copy them. Add the
  MANIFEST row.
- [ ] **Step 4:** Run `--test agents_http --test ingress_http --lib`, then
  `python3 ci/error-registry/check.py --self-test` and `--single-site`. Expected: PASS.
- [ ] **Step 5: Commit.** `feat(rs): add the REST routes of the agent API`

---

### Task 11: The gRPC `AgentService` on `grpc_addr`

**Files:**
- Create: `svc/src/adapters/grpc/mod.rs`, `svc/src/adapters/grpc/agent_service.rs`,
  `svc/src/adapters/grpc/status.rs`
- Modify: `svc/src/adapters/mod.rs`, `svc/Cargo.toml` (`tokio-stream` to `[dependencies]`)
- Modify: `svc/src/main.rs:116-182` (bind `grpc_addr` next to `http_addr`; serve; drain), `:366-425`
  (install the admin in the gRPC slot)
- Create: `svc/tests/agents_grpc.rs`

**Interfaces:**
- Consumes: Task 2 generated server trait; Task 6 `AgentAdmin`; Task 10 `AgentApiError::parts`.
- Produces:

```rust
/// Holds the admin once boot installs it. Before that, each RPC is UNAVAILABLE with the
/// `service-migrating` reason, as the HTTP boot slot answers 503.
#[derive(Clone, Default)]
pub struct AgentGrpcSlot(Arc<OnceLock<(Arc<AgentAdmin>, Arc<dyn Iam>)>>);
impl AgentGrpcSlot { pub fn install(&self, admin: Arc<AgentAdmin>, iam: Arc<dyn Iam>) -> anyhow::Result<()>; }
impl AgentService for AgentGrpcSlot { /* 8 RPCs; read `authorization` metadata through
    `bearer_token(&request.metadata().clone().into_headers())`, then `authenticate` */ }
pub fn to_status(error: &AdminError) -> tonic::Status;  // code from AgentApiError::parts, mapped
    // through ErrorReason::from_wire_reason; ErrorInfo domain AGENTS_DOMAIN; the same metadata
```

- gRPC codes: spec §6.6 for the seven rows; `forbidden` → `PERMISSION_DENIED`; `invalid-token` →
  `UNAUTHENTICATED`; `iam-unavailable`, `store-unavailable` → `UNAVAILABLE`; `internal` → `INTERNAL`;
  every 400 → `INVALID_ARGUMENT`.
- Each RPC calls `paigasus_observability::record_grpc("AgentService", "<Rpc>", started, &result)`. The
  server has `.layer(paigasus_observability::CorrelationLayer)`, no reflection and no health service.
  It ends on the same shutdown watch as HTTP, inside `DRAIN_TIMEOUT`.
- `status.rs` uses no code literal. If its test module spells a code, add an `asserts` MANIFEST row.

- [ ] **Step 1: Write the failing tests** in `svc/tests/agents_grpc.rs`: a real tonic server on
  `127.0.0.1:0` with `AgentGrpcSlot`, an in-memory registry and `StubIam`; a generated client:

```text
create_agent_wires_to_agent_admin_and_returns_the_agent
get_agent_wires_to_agent_admin
list_agents_wires_and_returns_the_next_page_token
update_agent_wires_and_a_stale_version_is_aborted_with_the_version_metadata
archive_agent_wires_and_sets_archive_time
create_inbox_wires_and_a_second_bind_is_already_exists_with_the_inbox_metadata
get_inbox_wires_to_agent_admin
list_inboxes_wires_to_agent_admin
each_rpc_self_queries_the_expected_action_on_the_org     // StubIam::self_query_args
a_prn_whose_uuid_exists_in_another_org_is_permission_denied_after_a_nil_org_probe   // Review Focus 4
a_missing_authorization_metadata_is_unauthenticated_and_reads_nothing
before_install_every_rpc_is_unavailable_service_migrating
```

  And in `boot_pg.rs`: `a_boot_with_grpc_addr_serves_agent_service` (spawn with `AGENT_HOST_GRPC_ADDR`,
  the fake IAM of `support/fake_iam.rs`, and a `ListAgents` call that gets a verdict, not
  `UNAVAILABLE`).
- [ ] **Step 2:** Run `--test agents_grpc`. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run `cargo nextest run -p paigasus-agent-host --features test-model --no-fail-fast` and
  `python3 ci/error-registry/check.py --single-site`. Expected: PASS.
- [ ] **Step 5: Commit.** `feat(rs): serve the gRPC AgentService of the agent host`

---

### Task 12: Documentation and the rollback measurement

**Files:**
- Modify: `svc/README.md` (the API section; remove `[[agents]]`), `svc/agent-host.toml.example:32-40,85`

- [ ] **Step 1: Measure the rollback (spec §10).** Build the `main` binary in a scratch worktree, start
  it against a database that this branch migrated to m0006, and record the exit and the log line.
  Record also whether the old binary's inbox insert path still works with `inbox_agent_fk` in place.
- [ ] **Step 2: Write the README section.** Contents, each in a few sentences: the eight routes and the
  eight RPCs; a `curl` example for create and for bind; the `agent_admin` grant through `GrantRole` at
  an organization; that `org_admin` holds `ReadAgents` only; that IAM must deploy first (spec §6.5
  Rollout); that a `[[nats.channels]]` entry for a new inbox needs a restart; that
  `memory.backend = "s3"` now needs `worker.enabled = false`; that `[[agents]]` now fails the boot;
  that in a build without a model every create gives `unknown-model` until SMA-851; the rollback result
  of Step 1; the new error codes with their status.
- [ ] **Step 3:** Remove the `[[agents]]` block of the example and fix the S3 comment at `:85`. Add a
  commented `# grpc_addr = "0.0.0.0:8087"` line.
- [ ] **Step 4:** Run `cargo nextest run -p paigasus-agent-host --features test-model --test boot_pg`
  (the example file is not read by a test, so this checks only that nothing else moved).
- [ ] **Step 5: Commit.** `docs(rs): document the agent API of the agent host`

---

### Task 13: The full gate graph

- [ ] **Step 1:** `cd rs && cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings`
  and `cargo clippy -p paigasus-agent-host --all-targets --features test-model -- -D warnings`.
- [ ] **Step 2:** From the repository root, run the command of the root `CLAUDE.md` "Before you push"
  block:

```bash
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site \
  :http-extractor-envelope :input-liveness :promtool :observability-drift \
  :nats-permissions :release-parity :release-parity-py :release-parity-ts \
  :publish-metadata :version-lockstep :workflow-credentials :pyo3-stub-drift :ruff-ci \
  :next-public-free :helm-render :moon-diagnosis-exec :wasm-lockstep :test-e2e \
  --base origin/main --include-relations
```

  Expected: every task passes. On this Mac no single bash runs every gate (root `CLAUDE.md`, "This
  development Mac only"). Re-run the bash-4+ gates with `/opt/homebrew/bin/bash ci/<gate>/run.sh` and
  read those results. `:breaking` fails in a worktree for the git-dir reason; use the command of
  Global Constraints and report its result.
- [ ] **Step 3:** If a task fails, follow the diagnosis procedure of the root `CLAUDE.md` (Step 0
  first). Fix in the task that owns the file, and commit with that task's scope.
