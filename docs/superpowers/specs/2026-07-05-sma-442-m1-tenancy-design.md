# SMA-442: M1 Tenancy (organizations → teams → projects) — Design

- **Issue:** [SMA-442](https://linear.app/smaschek/issue/SMA-442) · Epic M1 of 6 · IAM v1 vertical slice
- **Date:** 2026-07-05
- **Status:** Challenged + revised (Stage 2 folded in; GATE 1 pending)
- **Governing ADRs:** ADR-0014 (tenancy hierarchy & PRN scheme, incl. 2026-06-30 amendment),
  ADR-0005 (kernel-first), ADR-0013 (Cedar), ADR-0004 (proto contracts)
- **Precedents:** `docs/superpowers/specs/2026-07-04-sma-441-paigasus-iam-walking-skeleton-design.md`
  (M0 skeleton), `docs/superpowers/specs/2026-06-30-sma-448-kernel-prn-design.md` (PRN primitive)

## 1. Context & goal

`paigasus-iam` has the M0 walking skeleton (axum + tonic health, SeaORM `principal`/`user`,
figment config). M1 adds the tenancy spine: **Organization → Team → Project** nodes with
stable PRNs minted via `paigasus-kernel`, a **Membership** relation attaching principals to
tenancy nodes, full CRUD/lifecycle over HTTP + gRPC, SeaORM persistence + migration, and
unit + integration tests. These nodes become the Cedar entity hierarchy in M3.

Acceptance criteria (from the issue):

1. Can create an org → team → project hierarchy and attach a principal via `Membership`.
2. PRNs are stable and unique; tenancy nodes are queryable and form a parent/child chain.

## 2. Decisions (settled during brainstorm)

| # | Decision | Choice |
|---|----------|--------|
| D1 | Archive semantics | **Derived status**: each node stores only its own `status`; *effective* status = own status ∨ any ancestor archived, computed on read (bounded 2-join, fixed-depth chain). Restore is lossless. |
| D2 | Naming fields | **`slug` + `name`**: `slug` lowercase URL-safe token, unique within parent scope, mutable; `name` free-text display, no uniqueness. |
| D3 | Membership levels | **Any node + org invariant**: membership may target org, team, or project; team/project membership requires an existing org membership in that org. |
| D4 | Storage model | **Three tables** (`organization`, `team`, `project`) with real FKs + `membership` with three nullable FKs and an exactly-one CHECK. |
| D5 | Membership identity | Plain **UUIDv7 primary key, no PRN** — a membership is a relationship, never referenced by policies/keys/budgets. |
| D6 | ADR | **No new ADR.** ADR-0014 already covers tenancy + PRN; flip it Proposed → Accepted after GATE 1 (Notion edit, outside this repo). |

Decisions added by the adversarial challenge (Stage 2):

| # | Decision | Choice |
|---|----------|--------|
| D7 | Conflict attribution | Every unique constraint/index gets a deterministic name (§4); adapters map violations to domain codes by **constraint name**, never by PG detail text (PII rule). |
| D8 | Guard enforcement | Invariant guards run **inside the write transaction** under READ COMMITTED with `SELECT … FOR SHARE` locks on the rows they read (§3.4). |
| D9 | Principals in M1 | Wire the existing (M0-built, unwired) `CreateUser` use case behind a minimal `POST /v1/users` so AC-1 is exercisable end-to-end through the API; OIDC/JIT identity remains M2. |
| D10 | Archive mechanics | `archive`/`restore` act on the node's **own** status only and are idempotent on it; guards evaluate **effective** status; `restore` is always permitted (§5.1 rule 1 truth table). |

## 3. Domain model (`rs/crates/libs/paigasus-iam-core`, pure)

New modules: `tenancy.rs` (entities), extensions to `value.rs`, `ports.rs`.

### 3.1 Value objects

- `Slug` — `parse(&str) -> Result<Slug, DomainError>`: lowercase ASCII `[a-z0-9]` plus inner
  hyphens; no leading/trailing/double hyphen; 1–64 chars. (Deliberately more permissive than
  the kernel PRN label grammar — slugs never enter PRNs or Cedar.)
- Node name — validated in entity constructors: trimmed, non-empty, ≤ 256 Unicode scalar
  values (`chars()` count).
- `OrganizationId(Prn)`, `TeamId(Prn)`, `ProjectId(Prn)` — same shape as `PrincipalId`:
  `from_prn`, `uuid()`, `prn()`, `canonical()`. Constructors verify `service == "iam"` and the
  expected `resource_type` (`organization` / `team` / `project`), and org-scoped ids verify the
  org slot is present (orgs: absent).
- `NodeStatus { Active, Archived }` — `as_str()` (`"active"` / `"archived"`), `parse`.
- `TenancyNodeRef { Organization(OrganizationId), Team(TeamId), Project(ProjectId) }` —
  conversion to/from `Prn` (by `resource_type`); used by Membership and the APIs.

### 3.2 Entities

```text
Organization { id: OrganizationId, slug: Slug, name: String, status: NodeStatus,
               created_at, updated_at }
Team         { id: TeamId, org_id: OrganizationId, slug, name, status, created_at, updated_at }
Project      { id: ProjectId, team_id: TeamId, org_id: OrganizationId, slug, name, status,
               created_at, updated_at }
Membership   { id: Uuid, principal_id: PrincipalId, node: TenancyNodeRef, created_at }
```

`Project.org_id` is denormalized (always the owning team's org) so org-scope queries and the
membership invariant check don't need an extra join; the DB makes disagreement impossible
(§4). Timestamps are µs-truncated `DateTime<Utc>` from the `Clock` port (M0 convention).

**Implementation note:** the entity shapes above are the logical model; in the implemented
code, `Team.org_id`/`Project.org_id` are not separate struct fields. They live inside the
typed ids (`TeamId`/`ProjectId` are PRN-derived and embed the org UUID in the PRN's org slot),
exposed via the `org_uuid()` accessor. The physical `org_id` columns described in §4 exist
only in persistence, and the composite FK (`fk_project_team … REFERENCES team (id, org_id)`)
makes them incapable of disagreeing with the owning team's org.

### 3.3 PRN minting

Via the existing kernel surface (no kernel changes):

```text
org:      Prn::build("iam", "", None,           "organization", uuid7)
team:     Prn::build("iam", "", Some(org_uuid), "team",         uuid7)
project:  Prn::build("iam", "", Some(org_uuid), "project",      uuid7)
```

`IdGenerator` port grows: `new_organization_id()`, `new_team_id(org: Uuid)`,
`new_project_id(org: Uuid)`, `new_membership_id() -> Uuid`. `KernelIdGenerator` implements
them exactly like `new_principal_id` (SystemTime ms + 10 random bytes → `mint_uuid7`).

### 3.4 Repository ports (per-aggregate, `#[async_trait]`, M0 style)

- `OrganizationRepository` — `create(&Organization, &Team)` (**one transaction**: org +
  auto-provisioned default team, ADR-0014), `find(&OrganizationId)`, `rename`, `set_status`,
  `list(limit, offset)`. (No `find_by_slug` — slug uniqueness is enforced by the DB unique
  index, surfaced as a typed conflict on insert/rename.)
- `TeamRepository` — `create`, `find`, `rename`, `set_status`, `list_by_org(org, limit, offset)`.
- `ProjectRepository` — `create`, `find`, `rename`, `set_status`, `list_by_team(team, limit, offset)`.
- `MembershipRepository` — `attach(&Membership)`, `detach(id)` /
  `detach_org_cascade(principal, org)`, `find(id)`,
  `list_by_principal(principal, limit, offset)`, `list_by_node(node, limit, offset)`,
  `org_membership_exists(principal, org)`.
- `PrincipalRepository` — grows `exists(&PrincipalId) -> Result<bool, RepositoryError>`
  (needed by attach; the M0 surface is otherwise untouched).

**Transactional guard enforcement (D8).** Atomicity alone is not isolation: under Postgres
READ COMMITTED, a concurrent `archive` UPDATE does not conflict with a plain INSERT, so a
naive check-then-insert is TOCTOU-racy. Therefore `create` (team/project), `attach`, and
`rename` run their guards **inside the write transaction** and take `SELECT … FOR SHARE`
row locks on every row the guard reads: the ancestor chain (org, and team for projects),
and — for team/project attach — the principal's org-membership row. A concurrent `archive`
(a row UPDATE) blocks on those locks, so a guard cannot be invalidated mid-flight. Guard
violations surface as typed repository errors — `RepositoryError` grows `NotFound` and
`Precondition(code)` variants alongside M0's `Conflict`/`Backend` — which the services map
1:1 onto `TenancyError` codes. Isolation stays READ COMMITTED (no SERIALIZABLE retry loops).

**Effective status is a core rule, not a SQL detail.** `paigasus-iam-core` owns the pure
function `NodeStatus::effective(own: NodeStatus, ancestors: &[NodeStatus]) -> NodeStatus`;
the application-layer guards and the in-memory test fakes both call it. Read queries compute
the same value in SQL (bounded ancestor join) for efficiency; an integration parity test
asserts the SQL result equals the core function across the full archived/active matrix (§7).

**Conflict attribution (D7).** Every unique constraint/index has a deterministic name (§4).
The persistence adapter maps `SqlErr::UniqueConstraintViolation` to a domain code by matching
the **constraint name** (names are never PII; PG detail text is still discarded, M0 rule):
`uq_organization_slug` / `uq_team_org_slug` / `uq_project_team_slug` → `slug-conflict`;
`uq_membership_*` → `duplicate-membership`; the user email key → `email-conflict`;
`uq_*_prn` → `Backend` (a UUIDv7 collision is astronomically unlikely — internal error, not a
client conflict). Residual FK violations (races the in-transaction checks didn't see) map
defensively to `NotFound`.

Reads that return nodes return them **with effective status** (`(entity, NodeStatus)` or a
small read struct). Membership reads that return `node_prn` join the node table
(`organization`/`team`/`project`) to fetch the stored `prn` — the row itself only carries the
FK. Node lists order by `created_at, id` (§5.1 rule 9).

## 4. Persistence (`paigasus-iam/src/adapters/persistence/`, migration `m0002_create_tenancy`)

```sql
organization (id uuid PK, prn text, slug text, name text,
              status text, created_at timestamptz, updated_at timestamptz,
              CONSTRAINT uq_organization_prn  UNIQUE (prn),
              CONSTRAINT uq_organization_slug UNIQUE (slug))

team         (id uuid PK, prn text, org_id uuid FK→organization ON DELETE CASCADE,
              slug text, name text, status text, created_at, updated_at,
              CONSTRAINT uq_team_prn      UNIQUE (prn),
              CONSTRAINT uq_team_org_slug UNIQUE (org_id, slug),
              CONSTRAINT uq_team_id_org   UNIQUE (id, org_id))  -- backs project's composite FK

project      (id uuid PK, prn text, team_id uuid, org_id uuid,
              slug text, name text, status text, created_at, updated_at,
              CONSTRAINT uq_project_prn       UNIQUE (prn),
              CONSTRAINT uq_project_team_slug UNIQUE (team_id, slug),
              CONSTRAINT fk_project_team FOREIGN KEY (team_id, org_id)
                REFERENCES team (id, org_id) ON DELETE CASCADE)
                                  -- org_id can never disagree with the team's org

membership   (id uuid PK,
              principal_id uuid FK→principal ON DELETE CASCADE,
              org_id     uuid NULL FK→organization ON DELETE CASCADE,
              team_id    uuid NULL FK→team         ON DELETE CASCADE,
              project_id uuid NULL FK→project      ON DELETE CASCADE,
              created_at timestamptz,
              CONSTRAINT ck_membership_one_target
                CHECK (num_nonnulls(org_id, team_id, project_id) = 1))

-- membership uniqueness: three partial unique indexes (avoids the Postgres
-- NULLs-are-distinct trap in a composite UNIQUE):
CREATE UNIQUE INDEX uq_membership_principal_org     ON membership (principal_id, org_id)
  WHERE org_id IS NOT NULL;
CREATE UNIQUE INDEX uq_membership_principal_team    ON membership (principal_id, team_id)
  WHERE team_id IS NOT NULL;
CREATE UNIQUE INDEX uq_membership_principal_project ON membership (principal_id, project_id)
  WHERE project_id IS NOT NULL;
-- plus plain indexes on each FK column for list queries
```

**Constraint names are load-bearing:** the D7 error mapping matches on them, so renaming one
is a behavior change, not cosmetics. SeaORM entities mirror M0 style (`DeriveEntityModel`,
persistence-only, never derived on core types); repos map entity ↔ domain and `map_err`
SeaORM errors as in `pg_repository.rs` (extended per §3.4). Ids native `uuid` (minted
app-side), PRN/slug/status `text`, timestamps `timestamptz`. CHECK constraints and partial
indexes use raw SQL in the migration where sea-query lacks first-class support — Postgres-only
(`num_nonnulls`, partial indexes) is acceptable: the service targets Postgres exclusively
(M0 precedent). The migration ships an explicit `down()` dropping in FK order
(membership → project → team → organization), matching `m0001`. Registered as
`m0002_create_tenancy` in `Migrator`.

## 5. Application layer (`paigasus-iam/src/application/`)

Grouped **per-aggregate services** (deviation from M0's one-struct-per-use-case, justified by
~18 operations sharing the same 2–4 deps; same generic-DI-by-value pattern, no `Arc<dyn>`):

- `OrganizationService<R, I, C>` — `create(slug, name)` (mints org PRN + default team
  `slug="default"`, `name="Default"`; returns both), `rename(id, new_slug?, new_name?)`,
  `archive(id)`, `restore(id)`, `get(id)`, `list(page)`.
- `TeamService<...>` — `create(org_id, slug, name)`, `rename`, `archive`, `restore`, `get`,
  `list_by_org`.
- `ProjectService<...>` — `create(team_id, slug, name)`, same lifecycle, `list_by_team`.
- `MembershipService<...>` — `attach(principal_prn, node_prn)`, `detach(id)`,
  `list(principal | node)`.

### 5.1 Lifecycle & invariant rules

1. **Own vs effective status (D10).** `archive` sets and `restore` clears the node's **own**
   status only; both are idempotent on own status (no-ops return current state and do not
   advance `updated_at`). Guards evaluate **effective** status via the pure core function
   (§3.4). `archive` and `restore` are *always* permitted regardless of ancestor state — so
   restoring an org never silently un-archives a team whose own flag was set while the org
   was archived. Truth table (team under an org):

   | org own | team own | team effective | rename / child-create / attach on team | `archive(team)` | `restore(team)` |
   |---------|----------|----------------|----------------------------------------|-----------------|-----------------|
   | active   | active   | active   | allowed                     | sets own=archived | no-op |
   | active   | archived | archived | rejected (`node-archived`)  | no-op             | sets own=active |
   | archived | active   | archived | rejected (`node-archived`)  | sets own=archived | no-op |
   | archived | archived | archived | rejected (`node-archived`)  | no-op             | sets own=active (still effectively archived) |

2. **Archived-parent guard:** creating a team/project under, or attaching a membership to, an
   effectively archived node → `parent-archived` / `node-archived` (409 / `FailedPrecondition`),
   enforced in-transaction with row locks (§3.4, D8).
3. **Rename** requires at least one of `new_slug` / `new_name` (else `nothing-to-rename`) and
   is rejected when the node is effectively archived. `updated_at` advances on rename and on
   own-status *changes* (archive/restore that actually flip the flag).
4. **Org-membership invariant (D3):** attach to team/project requires an existing org
   membership for that principal in the node's org — checked inside the attach transaction
   with a `FOR SHARE` lock on the org-membership row (§3.4).
5. **Cascade detach:** detaching an **org** membership also removes that principal's
   team/project memberships in that org (GitHub model), in one transaction. Detaching a
   team/project membership removes only itself. Detaching a nonexistent membership id →
   `not-found` (404), not an idempotent no-op.
6. **Uniqueness:** duplicate slug in scope or duplicate membership → conflict (409 /
   `AlreadyExists`), attributed via constraint names (D7). Org slugs are globally unique;
   team slugs unique per org; project slugs unique per team.
7. **Default team is not special** after creation: it can be renamed/archived like any team.
   Note the consequence: its `default` slug occupies the org's team-slug space, so a
   user-created team with `slug = "default"` conflicts unless the default team was renamed.
8. **Attach resolution:** `principal_prn` / `node_prn` are resolved by their `resource_id`
   UUID against the DB **inside the attach transaction** (principal existence via the new
   `exists` port method). The client-supplied PRN must equal the stored canonical PRN —
   otherwise `prn-mismatch` (400 / `InvalidArgument`). The org slot of a client-supplied PRN
   is **never trusted**: the invariant's org comes from the persisted node row (a forged org
   slot would otherwise bypass D3). Missing principal or node → `not-found`.
9. **List semantics:** lists include archived nodes (callers see both `status` and
   `effective_status`); stable order `ORDER BY created_at, id`; `limit` defaults to 50,
   max 200; `limit < 1`, `limit > 200`, or `offset < 0` → `invalid-pagination`
   (400 / `InvalidArgument`).

### 5.2 Domain errors → transport mapping

New `TenancyError` enum with stable kebab-case codes (`slug-conflict`,
`duplicate-membership`, `email-conflict`, `parent-archived`, `node-archived`, `not-found`,
`missing-org-membership`, `invalid-slug`, `invalid-name`, `invalid-prn`, `prn-mismatch`,
`invalid-pagination`, `nothing-to-rename`). Conflict codes are attributed via constraint
names (D7, §3.4); one shared mapping module (`adapters/http` and `adapters/grpc` both
consume it):

| Class | HTTP | gRPC |
|-------|------|------|
| validation (`invalid-*`, `prn-mismatch`, `nothing-to-rename`) | 400 | `InvalidArgument` |
| `not-found` (incl. defensive FK-violation mapping, §3.4) | 404 | `NotFound` |
| duplicates (`slug-conflict`, `duplicate-membership`, `email-conflict`) | 409 | `AlreadyExists` |
| state guards (`parent-archived`, `node-archived`, `missing-org-membership`) | 409 | `FailedPrecondition` |
| backend (incl. `uq_*_prn` violations) | 500 | `Internal` |

HTTP error body: `{"error": {"code": "<kebab>", "message": "<human>"}}`. Secrets/PII never
echoed; PG messages never surfaced (M0 rule).

## 6. Wire surface

### 6.1 Proto (`contracts/proto/paigasus/iam/v1/iam.proto`)

First real IAM RPCs. Messages `Organization`, `Team` (`org_prn`), `Project` (`team_prn` +
`org_prn`) — each also carrying `prn`, `slug`, `name`, `NodeStatus status`,
`NodeStatus effective_status`, `paigasus.common.v1.AuditMetadata audit` (timestamps
populated, actor fields empty until M2) — and `Membership { id, principal_prn, node_prn,
audit }` (memberships are immutable: `modified_at` is set equal to `created_at`, per the
shared AuditMetadata contract), `NodeStatus` enum
(`NODE_STATUS_UNSPECIFIED/ACTIVE/ARCHIVED`), request/response pairs, and:

```proto
service TenancyService {
  // org / team / project × Create, Get, List, Rename, Archive, Restore  (18 RPCs)
  // + AttachMembership, DetachMembership, ListMemberships               (3 RPCs)
}
```

Notable shapes: `CreateOrganizationResponse { Organization organization = 1; Team
default_team = 2; }`; `ListMembershipsRequest` filters via a `oneof { string principal_prn;
string node_prn; }` (unset oneof → `InvalidArgument`). Lists paginate with `limit` + `offset`
per §5.1 rule 9 (`ListTeams(org_prn)`, `ListProjects(team_prn)`). The membership node
reference is a single PRN string; `resource_type` conveys the node type. The existing
placeholder `ServiceInfo` message stays untouched (buf breaking is additive-only here).

gRPC speaks **PRNs** throughout (the canonical cross-service reference), while HTTP paths use
**UUIDs** (§6.2) — the asymmetry is intentional and lossless: the UUID *is* the PRN's
`resource_id`, so translation is trivial in both directions.

Codegen wiring: add the `iam.v1.tonic.rs` include to `paigasus-proto/src/lib.rs` (mirroring
`gateway::v1`), regenerate via `moon run contracts:generate`, commit generated output
(drift-gated).

### 6.2 HTTP (axum) — establishes the `/v1` prefix

Path ids are **UUIDs** (PRNs contain `:` and `/`); PRNs appear in bodies/responses.

```text
POST /v1/organizations                      GET /v1/organizations?limit&offset
GET  /v1/organizations/{id}                 PATCH /v1/organizations/{id}     (rename)
POST /v1/organizations/{id}/archive         POST /v1/organizations/{id}/restore
POST /v1/organizations/{id}/teams           GET /v1/organizations/{id}/teams
GET  /v1/teams/{id}                         PATCH /v1/teams/{id} (+ /archive /restore)
POST /v1/teams/{id}/projects                GET /v1/teams/{id}/projects
GET  /v1/projects/{id}                      PATCH /v1/projects/{id} (+ /archive /restore)
POST /v1/memberships                        DELETE /v1/memberships/{id}
GET  /v1/memberships?principal=<prn>|node=<prn>
POST /v1/users                              (minimal principal creation, D9 — see below)
```

Handlers are thin: deserialize → call application service → map result/error. `/healthz`,
`/readyz` stay at root. `AppState` grows the concrete service instances (type-aliased
composition in `main.rs`).

**`POST /v1/users` (D9):** wires the *existing, currently-unwired* M0 `CreateUser` use case
(body: `email`, `display_name`, optional `locale`/`timezone`; 201 with the principal PRN;
duplicate email → 409 `email-conflict`). Without it a running M1 service has zero principals
and `AttachMembership` is dead code in production — AC-1 must be exercisable end-to-end
through the API. HTTP-only (no proto RPC): full identity management including gRPC surface,
OIDC and JIT provisioning stays in M2. List/query semantics for memberships follow §5.1
rule 9 (pagination, `not-found` on deleting a nonexistent membership).

### 6.3 gRPC (tonic)

`TenancyService` server implementation in `adapters/grpc.rs` (or a `grpc/` module if it
outgrows one file), added alongside tonic-health on the existing server; same shutdown +
timeout wiring as M0.

## 7. Testing

- **Core unit tests:** `Slug` parsing table, typed-id constructor rejections (wrong
  service/resource-type/org-slot), `TenancyNodeRef` ↔ PRN round-trip,
  `NodeStatus::effective` over the full §5.1 truth table, and service tests with in-memory
  fakes (which call the same `effective` function) covering every rule in §5.1: invariant,
  guards, cascade detach, default team, idempotent archive/restore,
  restore-under-archived-ancestor, rename validation, pagination validation, PRN-mismatch
  rejection.
- **Integration tests (testcontainers,** reusing `start_migrated_postgres`**):**
  1. chain round-trip: create org (default team returned) → team → project; parent/child
     queries; PRN stability/uniqueness (AC 1 + 2);
  2. membership: attach org/team/project with invariant, duplicate → conflict, org detach
     cascades;
  3. derived status: archive org → children report `effective_status=archived`, restore, and
     write-guards enforced;
  4. constraint tests: slug conflicts, duplicate memberships, and composite-FK integrity hit
     the real named constraints and map to the correct domain codes (D7);
  5. SQL-vs-core effective-status parity across the full own/ancestor matrix (§3.4);
  6. HTTP round-trip via axum `oneshot` incl. `POST /v1/users` → `POST /v1/memberships`
     (AC-1 end-to-end through the API) and the 400/404/409 error paths;
  7. gRPC `TenancyService` smoke test on an ephemeral port (per `grpc_health.rs`).
- Docker gate behavior unchanged (CI hard-fails, local skips).

## 8. Build / CI wiring

- `paigasus-iam/Cargo.toml`: add `paigasus-proto` (+ `serde` derives for HTTP DTOs if not
  already pulled); `moon.yml dependsOn`: add `paigasus-proto-rs`.
- `ci/affected-graph/run.sh`: the `contracts->proto` strict-equality expected set must gain
  `paigasus-iam-rs` (default-deny guard, SMA-409/429 precedent). `kernel->bindings` set is
  untouched (no new kernel dependents).
- No new workspace deps expected → no `deny.toml` / machete changes. No Windows-reserved
  filenames in the new module set.
- Pre-push: full graph `moon ci :build :test :lint :fmt :deny :machete :typecheck :breaking
  :affected-smoke :parity-corpus-drift :wasm-getrandom-free --base origin/main
  --include-relations`.

## 9. Out of scope (deferred, stated deliberately)

- Roles, role grants, Cedar schema/policies/evaluation (M3) — Membership carries **no role**.
- Authentication / actor context (M2) — endpoints are unauthenticated pre-release admin APIs;
  audit actor fields stay empty. **Deployment constraint (explicit):** until M2, the service
  must not be internet-exposed — dev/ops environments only (no authn, no rate limiting, only
  axum's default body limits).
- Full identity management (OIDC, JIT provisioning, user update/delete/get, gRPC user
  surface) — M2. M1 wires only the minimal existing `CreateUser` behind `POST /v1/users` (D9).
- Service accounts & API keys (M4); audit log & outbox events (M5/M6).
- Project/team re-parenting (PRNs already immutable-by-design for it), hard delete,
  Redis caching, Py/TS client wrappers.
- Kernel changes — none needed; PRN surface from SMA-448 suffices.
- New ADR — ADR-0014 governs; flip to Accepted post-GATE 1.
