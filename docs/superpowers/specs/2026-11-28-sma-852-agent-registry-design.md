# SMA-852 — Agent-definition registry

- Linear: SMA-852. Project: Paigasus Agent Host. Related: SMA-790 (the worker, Done), SMA-799 (the
  agents console, later), SMA-793 (acting for a principal, later).
- ADR: ADR-0028 (Notion). This issue adds one decision to it (§6.12).
- Builds on: the SMA-790 spec (`2026-11-22-sma-790-worker-run-loop-design.md`, §3 N2, §6.1, §6.2,
  §11), the SMA-785 ADR spec (`2026-11-18-sma-785-agent-host-adr-design.md`, decisions (d) and
  (k)) and the SMA-787 spec (ingress authentication, `WriteInboxEvent`, §6.1).
- Status: revised after the builder check and the Stage 2 challenge (§13). Waiting for Gate 1.

## 1. Intent

Today an operator writes agent definitions into `[[agents]]` in `agent-host.toml`. The service
reads that list one time at boot. `ConfigAgentResolver` serves it, and `PgLeaseStore::serving()`
binds the configured `(org_id, agent_id)` pairs into the inbox filter. So a new agent needs a
restart.

This issue moves the definitions into a table in the agent-host database. An operator manages
them through an API. The same API binds an agent to a principal, and that makes the inbox. The
worker finds a new agent on its next poll. No restart is necessary.

## 2. Goals

- G1. An `agent` table in the agent-host database, with a migration.
- G2. An operator API, as gRPC (`paigasus.agents.v1.AgentService`) and as REST routes on the
  same application service. It creates, reads, lists, updates and archives agents. It creates,
  reads and lists the inboxes of an agent.
- G3. Authentication and authorization for every API call, with the same rules as ingress (key
  scope, self-query, probe on refusal) and two new IAM actions (§6.5).
- G4. `PgAgentResolver`, an `AgentResolver` adapter that reads the table. It replaces
  `ConfigAgentResolver`.
- G5. The inbox filter of the worker (`CANDIDATES_SQL`, `LAG_SQL`) joins the `agent` table. It
  takes only an inbox of an active agent whose model this binary has. An agent that an operator
  adds or archives takes effect on the next poll.
- G6. The model name and the tool names are checked when the operator writes the definition.
- G7. An archived agent gets no new events, and its active timers are cancelled (§6.3).
- G8. `[[agents]]` is removed from the config. A config that still has it fails the boot with a
  clear message.

## 3. Non-goals

- N1. An IAM identity for each agent (a `ServiceAccount` per agent). Sven chose on 2026-11-28 that
  "the principal the agent acts for" is the inbox principal (reading (a)). An agent identity of its
  own is a later issue, with SMA-793 and SMA-794 (ADR-0028 (k)).
- N2. Definitions in IAM. Sven chose the agent-host table on 2026-11-28 (§7).
- N3. A check that the principal of a new inbox exists in IAM, or belongs to the org (§6.4,
  accepted risk).
- N4. Delete of an agent or of an inbox, and restore of an archived agent.
- N5. The agents console (SMA-799). It will use this API.
- N6. A change history table of agent definitions. Each write is logged (§6.5).
- N7. New per-agent settings (for example a mid-run policy per agent). The fields stay the fields
  of `[[agents]]` today.
- N8. Host partitioning. Every agent host serves every active agent, as one host serves every
  configured agent today.
- N9. A change to the agents of an org that IAM archived. `forbid-archived-writes` refuses
  `ManageAgents` on it. Its runs continue as today.

## 4. Anti-goals

- A1. An inbox of an archived agent, or of no agent, that the worker takes. Events that go to
  `dead` because an agent was archived, or because this binary does not have its model (SMA-790
  A7).
- A2. An agent that one org can read, change or bind through a call that is authorized only for a
  different org. This includes an API key that is scoped to a different org.
- A3. An in-memory copy of the agent list in the worker or the lease store. The registry must not
  need a reload, a cache or a notification to take effect. (The list of model ids that the binary
  has is a constant of the binary, not a copy of the registry.)
- A4. A definition that the service accepts with a model or a tool that it cannot run.
- A5. A config key `[[agents]]` that the service reads silently and then ignores.
- A6. An API answer that tells a caller without the right whether an agent or an inbox exists,
  also during an IAM outage.

## 5. Constraints

- `paigasus-agent-host-core` must not depend on Helikon or on sea-orm (the dependency guard in its
  `lib.rs`). A new core port uses core types only.
- Migrations use sea-orm-migration with raw SQL through `execute_unprepared`, and the
  `agent_host_migrations` table. The next migration is `m0006`. It has a `down`.
- The lock order and the fences of SMA-787 and SMA-790 do not change. The filter change is in the
  candidate query and the lag query only. `lock_live_lease` and the run commit do not read the
  `agent` table.
- The IAM role rules: a new non-restore write action joins the generated `forbid-archived-writes`
  list, so `STARTER_POLICY_REVISION` goes from 8 to 9, and `EXPECTED_STARTER_CONTENT_HASH`
  changes (`paigasus-iam-core/src/authz/roles.rs`). The Cedar schema (`schema.rs`) gets the two
  actions.
- The error contract: every `ApiError` has a fixed message that names no caller input
  (`adapters/http/error.rs`). Every code is an `ErrorReason` in `common/v1/error.proto`. Every new
  emit site goes on the `MANIFEST` of `ci/error-registry/check.py` (`repo:error-code-single-site`).
- A proto change needs `buf format -w`, and `contracts:breaking` must stay green. The change adds a
  service, messages and new `ErrorReason` values.
- The agent host already has an IAM client (`adapters/iam/client.rs`) with
  `introspect_api_key`, `introspect_token` and `is_authorized_self`. The API uses these. It needs
  no new IAM RPC.
- The production `ModelRegistry` is empty until SMA-851 (`main.rs:293-301`). So, in a production
  build, every create fails with `UNKNOWN_MODEL` until SMA-851. Only the `test-model` build has a
  model (`scripted`). This is the same as today, where boot fails on an unknown model.

## 6. Design

### 6.1 The `agent` table (m0006)

```sql
CREATE TABLE agent (
  id           uuid        NOT NULL,
  org_id       uuid        NOT NULL,
  name         text        NOT NULL,
  model        text        NOT NULL,
  instructions text        NOT NULL,
  tools        text[]      NOT NULL DEFAULT '{}',
  version      bigint      NOT NULL DEFAULT 1,
  created_at   timestamptz NOT NULL DEFAULT now(),
  updated_at   timestamptz NOT NULL DEFAULT now(),
  archived_at  timestamptz,
  PRIMARY KEY (id),
  UNIQUE (org_id, id)
);
CREATE UNIQUE INDEX agent_org_name_active ON agent (org_id, name) WHERE archived_at IS NULL;
ALTER TABLE inbox
  ADD CONSTRAINT inbox_agent_fk FOREIGN KEY (org_id, agent_id) REFERENCES agent (org_id, id);
```

- The server makes the uuid. The agent PRN is `prn:pgs:agents::<org>:agent/<uuid>` (`AgentId`), as
  today.
- `name` is unique among the active agents of one org, case-sensitive. An archived agent frees its
  name. Name rules: 1 to 64 characters, no control characters (NUL included), and no outer
  whitespace, in the style of `string_id!` (`ids.rs:168-171`).
- `instructions`: 1 to 32768 bytes. `tools`: no duplicates.
- The foreign key makes every inbox belong to an agent of the same org. The domain rule
  `id.org == agent.org` of `Inbox::new` stays.
- The migration runs in one transaction under the migration advisory lock. It fails if an `inbox`
  row has no `agent` row, and the Postgres error names `inbox_agent_fk`. No deployed environment
  holds agent-host data today (the service is pre-release, and the Helm chart has no agent host).
  So there is no backfill from `[[agents]]`.
- `down` drops the constraint and the table.

### 6.2 Ports and the application service

- A new port `AgentRegistry` in `paigasus-agent-host-core` (next to `AgentResolver` in
  `ports.rs`): `create`, `get`, `list`, `update`, `archive` for agents, and `create_inbox`,
  `get_inbox`, `list_inboxes`. It uses core types only. A list takes a page size (1 to 100,
  default 50) and an opaque page token, and returns the next token.
- A new port `RunCapabilities` in core: `has_model(&str) -> bool`, `has_tool(&str) -> bool`,
  `model_ids() -> Vec<String>`. The adapter is in `adapters/helikon/`, over `ModelRegistry` and
  `MEMORY_TOOL_NAME`. `KNOWN_TOOLS` moves there from `config.rs`.
- `AgentDefinition` keeps its fields. A new `AgentRecord` holds the definition plus `version`,
  `created_at`, `updated_at` and `archived_at`.
- Port errors: `NotFound`, `NameTaken`, `InboxExists(InboxId)`, `VersionMismatch`, `Archived`,
  `Invalid(field)`, `UnknownModel(name)`, `UnknownTool(name)`, `Store`.
- An application service `AgentAdmin` in the service crate holds the rules: the field limits,
  `RunCapabilities` for the model and the tools, and the authorization of §6.5. The gRPC service
  and the REST routes both call `AgentAdmin`. They do not call the port directly.

### 6.3 Update and archive

- An update replaces `name`, `model`, `instructions` and `tools`. The id and the org never change.
  The caller sends the `version` that it read. The statement is
  `UPDATE agent SET … , version = version + 1, updated_at = now() WHERE id = $1 AND version = $v
  AND archived_at IS NULL`. No row: the service reads the row again and returns `NotFound`,
  `Archived` or `VersionMismatch`. A name that another active agent of the org has gives
  `NameTaken` (from the unique index, not from a check before the write).
- A run uses the definition that the resolver read at the start of the run. A run in flight does
  not see an update. The next run sees it.
- Archive, in one transaction:
  1. sets `archived_at` (a second archive changes nothing and succeeds);
  2. cancels the active timers of the inboxes of the agent.
- After an archive:
  - the filter does not take the inboxes of the agent;
  - a run in flight ends normally;
  - a claim that resolves the agent after the archive gets `NotFound`. That is a transient failure
    (SMA-790 §6.2). The lease can serve other threads of the same inbox up to
    `worker.max_runs_per_lease` before it is released, so more than one claim can see the archive.
    Each claimed event loses at most one attempt. The next take does not choose the inbox.
  - **No new events.** The event insert of HTTP ingress, NATS ingress and the timer scheduler
    (`scheduler.rs:269`) writes the event only when the agent of the inbox is active, in the same
    statement (`INSERT … SELECT … WHERE EXISTS (… archived_at IS NULL)`). HTTP ingress then answers
    `AGENT_ARCHIVED` (409), after the authorization, so a caller without the right learns nothing.
    NATS ingress terminates the message (`Disposition::Term`) with a warning. The scheduler drops
    the fired timer with a warning.
- There is no restore (N4). The events that were pending at the archive wait. An operator removes
  them with SQL. A new gauge `agent_host_archived_pending_events` (provisional name, SMA-798 owns
  the names) counts them, so they do not disappear from monitoring. The lag metric counts only
  the inboxes that the filter can take.
- `GetAgent` and `GetInbox` return an archived agent, or an inbox of one, with `archived_at` set.
  `ListAgents` leaves out archived agents unless the request sets `include_archived`.
  `ListInboxes` of an archived agent returns its inboxes.

### 6.4 The inbox binding

- `CreateInbox(agent, principal)` makes the inbox row and returns the `InboxId`
  (`prn:pgs:agents::<org>:inbox/<uuid>`). The server makes the uuid.
- The bind locks the agent row `FOR SHARE` and checks `archived_at IS NULL` in the same
  transaction. So a bind cannot commit after an archive of the same agent. (The foreign key check
  alone takes `FOR KEY SHARE`, which does not conflict with the archive update.) An archived agent
  gives `Archived`.
- A second bind for the same agent and principal gives `InboxExists` with the id of the existing
  inbox in the `ErrorInfo` metadata. This uses the existing `UNIQUE (agent_id, principal_id)`.
- The principal is a `PrincipalId` PRN. The service parses it and does not ask IAM about it.
- **Accepted risk (Sven, 2026-11-28).** A `PrincipalId` has no org, and the service does not check
  the principal. So a holder of `ManageAgents` on org A can bind an org-A agent to a principal of
  another org, or to a principal of org A with more rights. Today this gives that principal only
  the ingress owner right and a memory scope in org A. No run acts with the authority of the
  principal. SMA-793 must add the check before a run acts for the principal (ADR-0028 (k)). A
  comment on SMA-793 records this condition. SMA-792 (the outbox) sends agent output to the inbox
  principal, so the same condition applies to it.
- After the bind, the operator connects a channel to the inbox. HTTP ingress works at once. A
  `[[nats.channels]]` entry needs the inbox to exist at boot (`bind_channels`,
  `nats/consumer.rs:101-108`), and the API is mounted only after that check. So a NATS channel for
  a new inbox needs one more restart. The README states this. A dynamic NATS binding is not in
  scope.

### 6.5 The API, authentication and authorization

- **gRPC.** A new `service AgentService` in `contracts/proto/paigasus/agents/v1/agents.proto`:
  `CreateAgent`, `GetAgent`, `ListAgents`, `UpdateAgent`, `ArchiveAgent`, `CreateInbox`,
  `GetInbox`, `ListInboxes`. Errors use the `google.rpc` status details as IAM does.
- **REST.** On the existing `http_addr` router, next to ingress, in the style of
  `paigasus-iam/src/adapters/http/service_accounts.rs`:

  | Route | Call | Body and answer |
  |---|---|---|
  | `POST /v1/agents` | create | body: `org`, `name`, `model`, `instructions`, `tools`. 201 with the record |
  | `GET /v1/agents?org=<org PRN>&include_archived=&page_size=&page_token=` | list | 200 with a page |
  | `GET /v1/agents/{agent}` | get | 200 with the record |
  | `PATCH /v1/agents/{agent}` | update | body: `version` and all four fields (a full replace of those fields). 200 with the record |
  | `DELETE /v1/agents/{agent}` | archive | 200 with the archived record |
  | `POST /v1/agents/{agent}/inboxes` | bind | body: `principal`. 201 with the inbox |
  | `GET /v1/agents/{agent}/inboxes?page_size=&page_token=` | list inboxes | 200 with a page |
  | `GET /v1/inboxes/{inbox}` | get inbox | 200 with the inbox |

  `{agent}` and `{inbox}` are uuids. The page token encodes the org, the filter and the position.
  A token used with a different org or filter gives `INVALID_PAGE_TOKEN` (400).
- **The gRPC listener.** A new optional key `grpc_addr`. With `[iam]` set, the service serves
  `AgentService` there. `grpc_addr` without `[iam]` fails `validate`. Its port must differ from
  `http_addr` and `metrics.addr`. It binds at the same boot step as the HTTP listener, uses the
  same correlation-id and metrics layers, and drains with the same `DRAIN_TIMEOUT`. It follows
  `http_addr`: the service does not terminate TLS, and the ingress or the mesh in front of it
  does. It has no reflection service and no gRPC health service.
- **Authentication.** The same path as ingress (`http/auth.rs`): an IAM API-key introspect, then an
  OIDC introspect. No database read occurs before it. The gRPC service reads the
  `authorization` metadata. The code is shared, not copied.
- **Authorization.** Two new IAM actions:
  - `ManageAgents`, a non-restore write: create, update, archive and bind.
  - `ReadAgents`, a read: get and list agents and inboxes.

  The steps for every call, in this order, in the style of `authorize_write`
  (`auth.rs:130-160`):
  1. Find the target org. For create and list, it is the `org` of the request. For a call by id,
     the service reads the row. An unknown id has no org.
  2. An unknown id gives the probe answer of `deny_unknown_inbox`: a self-query against the nil
     org with the action of the route, then `FORBIDDEN` (403), or `IAM_UNAVAILABLE` (503) during
     an outage.
  3. The key scope: an API key must be scoped to Root or to exactly the target org
     (`key_scope_allows`). Out of scope gives the probe answer of `refusal_after_probe` with the
     action of the route.
  4. The self-query: `is_authorized_self(caller principal, action, org PRN)`. There is no owner
     shortcut. Denied gives `FORBIDDEN`.

  So an unknown id, an id of another org, a key of another org and a missing right all give the
  same answer (403), also during an IAM outage (503 for all) (A6). A caller with `ReadAgents` and
  no `ManageAgents` gets 403 on a write.
- **Roles.** A new role template `agent_admin` holds `ManageAgents` and `ReadAgents`, at an
  Organization. This follows SMA-787 §6.1, which kept `WriteInboxEvent` off `org_admin` in a
  dedicated `agent_event_writer` template: an agent runs for every member's inbox, so a rewrite of
  its instructions, or a bind to oneself (which passes the owner path of ingress), is a separate
  right. `org_admin` gets `ReadAgents` only. An `org_admin` can grant `agent_admin` through
  `GrantRole`, and that grant is audited. `STARTER_POLICY_REVISION` goes to 9.
- **Rollout.** The IAM change and the agent-host change go in one PR. IAM must deploy first: an
  agent host that asks an IAM of revision 8 about `ManageAgents` gets `InvalidAction`, which maps
  to 503 (`auth.rs:293-304`). IAM reconciles the stored starter policies on the revision bump
  (`reconcile.rs`).
- **Logs.** Each write logs, at info level, the action, the caller principal, the agent id, the
  inbox id (for a bind) and the new version. It does not log the instructions.

### 6.6 The error contract

New `ErrorReason` values in `common/v1/error.proto`, each with a fixed message:

| Reason | HTTP | gRPC | `ErrorInfo` metadata |
|---|---|---|---|
| `AGENT_NAME_TAKEN` | 409 | `ALREADY_EXISTS` | — |
| `INBOX_EXISTS` | 409 | `ALREADY_EXISTS` | `inbox` (the existing inbox PRN) |
| `AGENT_VERSION_MISMATCH` | 409 | `ABORTED` | `version` (the current version) |
| `AGENT_ARCHIVED` | 409 | `FAILED_PRECONDITION` | — |
| `UNKNOWN_MODEL` | 400 | `INVALID_ARGUMENT` | `model` |
| `UNKNOWN_TOOL` | 400 | `INVALID_ARGUMENT` | `tool` |
| `INVALID_AGENT_FIELD` | 400 | `INVALID_ARGUMENT` | `field` (the field name only) |
| `INVALID_PAGE_TOKEN` | 400 | `INVALID_ARGUMENT` | — |

The existing `FORBIDDEN`, `INVALID_TOKEN`, `IAM_UNAVAILABLE`, `INVALID_REQUEST_BODY` and
`INVALID_REQUEST_SCHEMA` are used as in ingress. The caller's value goes only into the metadata,
never into the message. Each new emit site goes on the `MANIFEST` of
`ci/error-registry/check.py`. If an existing reason already has the meaning of a row, the builder
uses it and says so in the plan.

### 6.7 The model and tool checks

- `AgentAdmin` checks on create and update that `RunCapabilities::has_model(&model)` is true, and
  that `has_tool` is true for every tool (only `memory` in v1).
- The registry is built from the binary, so a model can disappear in a later release, or be
  missing on one host in a mixed-version rollout. The filter therefore binds the model ids of the
  binary: `AND a.model = ANY($n::text[])`. An inbox of an agent whose model this binary does not
  have is not taken. Its events wait. They do not go to `dead` (A1).
- At boot, the service reads the active agents and logs one warning for each agent whose model is
  not in the registry. Boot does not fail.
- The runner check in `HelikonAgentRunner` (`runner.rs:48-55`) stays as a guard for a race.

### 6.8 The resolver and the filter

- `PgAgentResolver::resolve(&InboxId)` runs one query: the inbox row joined to the active agent
  row on `(org_id, agent_id)`. No row gives `NotFound` (transient, as today).
- `CANDIDATES_SQL`, `LAG_SQL` and the `lag_select!` macro all change together. They replace the
  `unnest($2::uuid[], $3::uuid[])` filter with
  `JOIN agent a ON a.org_id = <alias>.org_id AND a.id = <alias>.agent_id AND a.archived_at IS NULL
  AND a.model = ANY($n::text[])`. The candidate query uses the alias `i`, and the lag sub-select
  uses `j` (`lease_store/mod.rs:37`, `:62`). So the join text is a macro that takes the alias.
- The `FOR NO KEY UPDATE OF i SKIP LOCKED` stays on `i` only. It must not lock `agent` rows. So an
  archive never waits for a worker.
- `PgLeaseStore::serving()` and its `agents` field go away. In their place, the lease store takes
  the model ids at construction. `acquire_next` returns `None` early only when the model list is
  empty.
- `acquire(&InboxId)` (`lease_store/mod.rs:212`) takes a named inbox and does not use the filter.
  This stays. Its callers then meet the resolver, which answers `NotFound` for an archived agent.
  The code comment says so.
- The pool starts when `worker.enabled` is true. It no longer needs a configured agent. With no
  agent, a worker makes one poll query per interval, as an idle pool does today (SMA-790 G1).
- The S3 boot guard (`config.rs`, line 980) refuses `memory.backend = "s3"` when
  `worker.enabled` is true. The condition "and `[[agents]]` is not empty" goes away, because an
  agent can now arrive at run time. `worker.enabled` is true by default (`config.rs:802-807`), so
  an S3 config must now set `worker.enabled = false`. The README states this.

### 6.9 Config

- `Config.agents`, `AgentConfig`, `agent_definitions()` and the errors `AgentIdInvalid`,
  `AgentDuplicate` and `AgentUnknownTool` go away.
- A config that has a top-level `agents` key (from the file or from `AGENT_HOST_AGENTS*`) fails
  `validate` with a new error: "`[[agents]]` is removed; use the agent API (SMA-852)".
- New key `grpc_addr` (optional, no default), checked in `validate` (§6.5).
- `agent-host.toml.example` and `README.md` lose `[[agents]]`. They get a short section on the API,
  with a `curl` example for create and bind, the `agent_admin` grant, the NATS restart note and the
  S3 note.

### 6.10 The change inventory

The builder must change these. The list comes from the challenge, and the builder extends it.

- `tests/support/inbox.rs` (`insert_inbox`, used by about 20 test binaries): it also inserts the
  agent row (name = the uuid, `ON CONFLICT (id) DO NOTHING`), and it panics when the agent exists in
  another org.
- `boot_pg.rs:74`: an inbox insert without an agent now fails with SQLSTATE 23503, not 23505.
- `ci/images/run.sh:1145-1153`: the image smoke inserts an inbox with a random org and agent. It
  must insert the agent row first. `images.yml` is not a required check, so a miss reds `main`.
- Tests whose expectation inverts: `boot_pg.rs:289`, `:306`, `:339`; `config.rs:1518-1528` (S3
  with the default `worker.enabled`); `worker_pool_pg.rs:136-164`; all of `agents_pg.rs`.
- `.serving()` callers: `worker_pool_pg.rs`, `lease_pg.rs`, `worker_crash_pg.rs`, `main.rs`.
- `[[agents]]` TOML: `boot_pg.rs`, `worker_sigkill_pg.rs:124`, `agent-host.toml.example`,
  `README.md`, the `config.rs` unit tests (lines 2067-2170).
- `schema_pg.rs:166` and every other `Migrator::down(Some(n))` step count, which move with m0006.
- `application/worker/test_fakes.rs` (the fake resolver).
- IAM: `action.rs` (the enum, the wire string, the count test), `schema.rs` (`SCHEMA_SRC` and its
  validation test), `roles.rs` (the template, the revision, `EXPECTED_STARTER_CONTENT_HASH`, the
  role tests).

### 6.11 Tests

Each test must fail when the code it targets is removed.

- Migration: m0006 runs on an empty database, and `down` reverses it. It fails on a database with
  an inbox that has no agent, and the error names `inbox_agent_fk`.
- `PgAgentRegistry` (Postgres): create, get, list with pages, `include_archived`, update with the
  right and the wrong version, update of an archived agent, archive twice, a name that is free
  again after an archive, bind, a second bind (`InboxExists` with the existing id), a bind to an
  archived agent, an org mismatch, and a page token used with another org.
- Concurrency (Postgres, two connections): two creates with the same name, exactly one succeeds;
  two updates with the same version, exactly one succeeds; a bind and an archive that race, the
  bind either commits before the archive or gets `Archived`.
- Archive: the active timers of the agent's inboxes are cancelled. HTTP ingress answers
  `AGENT_ARCHIVED`, NATS ingress terminates, a fired timer is dropped, and no event row is
  written in each case.
- `PgAgentResolver`: an active agent, an archived agent (`NotFound`), an inbox of another org.
- The filter: an inbox of an archived agent is not taken, and its events stay `pending`. An inbox
  of an agent whose model is not in the bound list is not taken. Both lag paths leave both inboxes
  out. The gauge counts the pending events of the archived agent.
- **The done-when test** (`worker_pool_pg.rs`): start the pool with no agent. Create the agent and
  the inbox through `AgentAdmin` while the pool runs, insert an event, and check that a worker
  runs it without a restart.
- `AgentAdmin`: an unknown model, an unknown tool, each field limit, each name rule.
- Authorization (a recording fake `Iam`; the existing `FakeIam` is at `auth.rs:388-421`): for each route and each RPC, the
  test checks the exact self-query (principal, action, org PRN) and both verdicts. It also checks
  an API key scoped to another org (403), an unknown id (403 after a probe against the nil org),
  an id of another org (403), and an IAM outage for each of these (503). An unauthenticated call
  makes no database read (a store fake that fails on any call).
- IAM: role tests for `agent_admin` (allows both actions on its org, denies on another org),
  `org_admin` (allows `ReadAgents`, denies `ManageAgents`), `org_member` (denies both),
  `forbid-archived-writes` (denies `ManageAgents` on an archived org), the action count, the schema
  validation and the content hash.
- Config: a config with `[[agents]]` fails with the new error. `grpc_addr` with the port of
  `http_addr` fails. `grpc_addr` without `[iam]` fails.
- Boot (`boot_pg.rs`, `worker_sigkill_pg.rs`): replace the `[[agents]]` TOML with agent rows. A boot
  with an active agent whose model is unknown logs the warning and starts.
- gRPC: one test per RPC through a real tonic server on a loopback port, so the wiring of
  `AgentService` to `AgentAdmin` is tested, not only `AgentAdmin`.

### 6.12 The ADR-0028 addition

Before the code, the plan adds to ADR-0028 in Notion (status stays Proposed): "Agent definitions
live in the agent-host database and are managed through `AgentService`. IAM authorizes with
`ManageAgents` and `ReadAgents` on the org, held by the `agent_admin` template. The inbox
principal is the principal that the agent acts for. The service does not check that principal yet;
SMA-793 must check it before a run acts for it (SMA-852)."

## 7. Approaches considered

- **Where the definitions live.**
  1. A table in the agent-host database, and the API in the agent host (chosen by Sven on
     2026-11-28). The filter becomes a join, so no cache and no reload exist. The model check is
     at write time, because `ModelRegistry` is in the same process. Cost: the agent host serves
     gRPC for the first time.
  2. The definitions in IAM, fetched by the agent host with a cache and NATS change events, like
     the gateway tool policies. The "no restart" then depends on the cache refresh. IAM does not
     know the models, so a bad model shows only at run time. Two services change. Rejected.
- **The principal the agent acts for.** (a) The inbox principal, the link that exists today (chosen
  by Sven on 2026-11-28). (b) An IAM identity per agent, for example a `ServiceAccount`. It touches
  ADR-0028 (k), which SMA-793 and SMA-794 own. Deferred (N1).
- **How the operator adds an agent.** Through an HTTP and gRPC API (Sven, 2026-11-28). SQL alone
  and a CLI were the alternatives.
- **The principal check on bind.** (A) Accept the risk for v1 (chosen by Sven on 2026-11-28).
  (B) Check membership through `ListMemberships`, and the `owner` of a service account. Deferred to
  SMA-793.

## 8. Decided by Sven

- 2026-11-28: an operator adds agents through an HTTP and gRPC API.
- 2026-11-28: the principal the agent acts for is the inbox principal (reading (a)).
- 2026-11-28: Option 1, the agent-host table ("for now").
- 2026-11-28: accept the unchecked inbox principal for v1 (§6.4). SMA-793 adds the check.

## 9. Calls made by the coordinator (check these)

- The API also binds an agent to a principal and makes the inbox (§6.4).
- Two IAM actions (`ManageAgents`, `ReadAgents`), like the tool-policy pairs, not one action per
  RPC.
- **A dedicated `agent_admin` template holds both actions. `org_admin` gets `ReadAgents` only**
  (§6.5). This follows the SMA-787 precedent for `WriteInboxEvent`.
- Every refusal and every unknown id gives 403, also for a call by id (A6), as ingress does.
- Optimistic concurrency with a `version` column on update. PATCH replaces all four fields.
- Archive only. No delete and no restore. Archive cancels the timers, and ingress refuses new
  events. The pending events wait, with a gauge.
- The filter binds the model ids of the binary, so an agent with a missing model waits.
- The migration adds a foreign key from `inbox` to `agent` and fails on old inbox rows.
- An unknown model at boot is a warning, not a boot failure.
- `[[agents]]` fails the boot. There is no import of an old list.
- The S3 guard applies whenever the worker is enabled.
- `grpc_addr` is optional, has no TLS of its own, and needs `[iam]`.
- `name` can change on update.
- A NATS channel for a new inbox still needs a restart.
- IAM deploys before the agent host.

## 10. Left to the builder

- The module layout of `AgentAdmin`, the gRPC service and the REST routes.
- The page-token encoding, within §6.5.
- The exact proto message fields, within §6.5.
- How the shared authentication code is factored between ingress and the API.
- Indexes beyond those in §6.1.
- Whether an existing `ErrorReason` already covers a row of §6.6.
- The rollback behaviour of an older binary on a database with m0006 applied. The builder measures
  it and writes it in the README.

## 11. Residual risks

- The unchecked inbox principal (§6.4).
- Every create fails in a production build until SMA-851 adds a model (§5).
- An `org_admin` can grant itself `agent_admin`. That is an audited, explicit step.

## 12. Open questions answered by the coordinator

- The agents of an IAM-archived org keep running (N9).
- No host partitioning (N8).
- The page token is bound to the org and the filter (§6.5).
- SMA-792 has the same principal condition as SMA-793 (§6.4).

## 13. Challenge changelog (Stage 2, 2026-11-28)

Builder check (before the challenge): folded in the order of authentication, the archived-agent
reads, the gRPC TLS rule, the migration data rule, the lag scope, the mutable name and the IAM
rollout. The principal check went to Sven (option A).

The spec-challenger verdict: APPROVE WITH CHANGES. Folded in:

- BLOCKER, no API-key scope check: added step 3 of §6.5 and its tests.
- MAJOR, an unknown id as an existence oracle: the probe pattern, and 403 for every refusal (A6).
- MAJOR, the model check uses an adapter type, and a missing model sends events to `dead`: the
  `RunCapabilities` port, and the model ids in the filter (§6.7). §5 now states that production
  creates fail until SMA-851.
- MAJOR, the unbounded backlog of an archived agent: archive cancels timers, ingress refuses new
  events, and a gauge counts the pending events.
- MAJOR, `ManageAgents` on `org_admin`: the `agent_admin` template (a coordinator call, §9).
- MAJOR, the missing error contract: §6.6.
- MAJOR, the code and tests that the foreign key breaks: §6.10.
- MAJOR, tests that do not fail when their target is removed: the recording fake, the lag tests,
  the concurrency tests, the no-read-before-authentication test, the IAM hash and schema tests.
- MINOR, all folded in: the bind and archive race (`FOR SHARE`), the update predicate, "one attempt
  per claimed event", the S3 default, the NATS restart, `grpc_addr` without `[iam]` and its wiring,
  the REST shape and the name rules, `acquire(&InboxId)`, the alias macro, the rollout order and
  `down`, and the write log.

Not folded in: none. The four questions are answered in §12.
