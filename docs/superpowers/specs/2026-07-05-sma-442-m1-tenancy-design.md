# SMA-442: M1 Tenancy (organizations → teams → projects) — Design

- **Issue:** [SMA-442](https://linear.app/smaschek/issue/SMA-442) · Epic M1 of 6 · IAM v1 vertical slice
- **Date:** 2026-07-05
- **Status:** Approved design (GATE 1 pending)
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

## 3. Domain model (`rs/crates/libs/paigasus-iam-core`, pure)

New modules: `tenancy.rs` (entities), extensions to `value.rs`, `ports.rs`.

### 3.1 Value objects

- `Slug` — `parse(&str) -> Result<Slug, DomainError>`: lowercase ASCII `[a-z0-9]` plus inner
  hyphens; no leading/trailing/double hyphen; 1–64 chars. (Deliberately more permissive than
  the kernel PRN label grammar — slugs never enter PRNs or Cedar.)
- Node name — validated in entity constructors: trimmed, non-empty, ≤ 256 chars.
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
  index, surfaced as `Conflict` on insert/rename.)
- `TeamRepository` — `create`, `find`, `rename`, `set_status`, `list_by_org(org, limit, offset)`.
- `ProjectRepository` — `create`, `find`, `rename`, `set_status`, `list_by_team(team, limit, offset)`.
- `MembershipRepository` — `attach(&Membership)` (checks + inserts in one transaction),
  `detach(id)` / `detach_org_cascade(principal, org)`, `find(id)`,
  `list_by_principal(principal)`, `list_by_node(node)`, `org_membership_exists(principal, org)`.

Reads that return nodes return them **with effective status**: repo methods yield
`(entity, effective_status: NodeStatus)` (or a small read struct), computed in SQL by joining
the ancestor chain. Reuses `RepositoryError { Conflict, Backend }`; conflict detail is carried
as a domain error code, not raw PG text (PII rule from M0).

## 4. Persistence (`paigasus-iam/src/adapters/persistence/`, migration `m0002_create_tenancy`)

```sql
organization (id uuid PK, prn text UNIQUE, slug text UNIQUE, name text,
              status text, created_at timestamptz, updated_at timestamptz)

team         (id uuid PK, prn text UNIQUE, org_id uuid FK→organization ON DELETE CASCADE,
              slug text, name text, status text, created_at, updated_at,
              UNIQUE (org_id, slug),
              UNIQUE (id, org_id))              -- backs project's composite FK

project      (id uuid PK, prn text UNIQUE, team_id uuid, org_id uuid,
              slug text, name text, status text, created_at, updated_at,
              UNIQUE (team_id, slug),
              FOREIGN KEY (team_id, org_id) REFERENCES team (id, org_id)
                ON DELETE CASCADE)              -- org_id can never disagree with the team's org

membership   (id uuid PK,
              principal_id uuid FK→principal ON DELETE CASCADE,
              org_id     uuid NULL FK→organization ON DELETE CASCADE,
              team_id    uuid NULL FK→team         ON DELETE CASCADE,
              project_id uuid NULL FK→project      ON DELETE CASCADE,
              created_at timestamptz,
              CHECK (num_nonnulls(org_id, team_id, project_id) = 1))

-- membership uniqueness: three partial unique indexes (avoids the Postgres
-- NULLs-are-distinct trap in a composite UNIQUE):
UNIQUE (principal_id, org_id)     WHERE org_id     IS NOT NULL
UNIQUE (principal_id, team_id)    WHERE team_id    IS NOT NULL
UNIQUE (principal_id, project_id) WHERE project_id IS NOT NULL
-- plus plain indexes on each FK column for list queries
```

SeaORM entities mirror M0 style (`DeriveEntityModel`, persistence-only, never derived on core
types); repos map entity ↔ domain and `map_err` SeaORM errors as in `pg_repository.rs`.
Ids native `uuid` (minted app-side), PRN/slug/status `text`, timestamps `timestamptz`.
CHECK constraints and partial indexes are expressed with raw SQL in the migration where
sea-query lacks first-class support. Registered as `m0002_create_tenancy` in `Migrator`.

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

1. **Archived parent guard:** creating a team/project under, or attaching a membership to, an
   *effectively archived* node → `ParentArchived` (409 / `FailedPrecondition`).
2. **Archived nodes are read-only except `restore`.** Rename/child-create/attach on an
   effectively archived node is rejected. `archive` is **idempotent** (no-op on archived).
3. **Restore clears only the node's own flag.** If an ancestor remains archived, the response
   still reports `effective_status = archived`.
4. **Org-membership invariant (D3):** attach to team/project requires an existing org
   membership for that principal in the node's org — checked inside the attach transaction.
5. **Cascade detach:** detaching an **org** membership also removes that principal's
   team/project memberships in that org (GitHub model), in one transaction. Detaching a
   team/project membership removes only itself.
6. **Uniqueness:** duplicate slug in scope or duplicate membership → `Conflict` (409 /
   `AlreadyExists`). Org slugs are globally unique; team slugs unique per org; project slugs
   unique per team.
7. **Default team is not special** after creation: it can be renamed/archived like any team.
8. Rename requires at least one of `new_slug` / `new_name`.
9. The membership `principal_prn` must resolve to an existing principal; `node_prn` must
   resolve to an existing node of its stated `resource_type`.

### 5.2 Domain errors → transport mapping

New `TenancyError` enum with stable kebab-case codes (`slug-conflict`,
`duplicate-membership`, `parent-archived`, `node-archived`, `not-found`,
`missing-org-membership`, `invalid-slug`, `invalid-name`, `invalid-prn`, `nothing-to-rename`).
One shared mapping module (`adapters/http` and `adapters/grpc` both consume it):

| Class | HTTP | gRPC |
|-------|------|------|
| validation (`invalid-*`, `nothing-to-rename`) | 400 | `InvalidArgument` |
| `not-found` | 404 | `NotFound` |
| duplicates (`slug-conflict`, `duplicate-membership`) | 409 | `AlreadyExists` |
| state guards (`parent-archived`, `node-archived`, `missing-org-membership`) | 409 | `FailedPrecondition` |
| backend | 500 | `Internal` |

HTTP error body: `{"error": {"code": "<kebab>", "message": "<human>"}}`. Secrets/PII never
echoed; PG messages never surfaced (M0 rule).

## 6. Wire surface

### 6.1 Proto (`contracts/proto/paigasus/iam/v1/iam.proto`)

First real IAM RPCs. Messages `Organization`, `Team`, `Project` (fields: `prn`, parent PRN(s),
`slug`, `name`, `NodeStatus status`, `NodeStatus effective_status`,
`paigasus.common.v1.AuditMetadata audit` — timestamps populated, actor fields empty until M2),
`Membership { id, principal_prn, node_prn, audit }` (membership audit sets `created_at` only —
memberships are immutable, there is no `updated_at`), `NodeStatus` enum
(`NODE_STATUS_UNSPECIFIED/ACTIVE/ARCHIVED`), request/response pairs, and:

```proto
service TenancyService {
  // org / team / project × Create, Get, List, Rename, Archive, Restore  (18 RPCs)
  // + AttachMembership, DetachMembership, ListMemberships               (3 RPCs)
}
```

Lists paginate with `limit` + `offset` (`ListTeams(org_prn)`, `ListProjects(team_prn)`,
`ListMemberships(principal_prn | node_prn)` — exactly one filter). The membership node
reference is a single PRN string; `resource_type` conveys the node type. The existing
placeholder `ServiceInfo` message stays untouched (buf breaking is additive-only here).

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
```

Handlers are thin: deserialize → call application service → map result/error. `/healthz`,
`/readyz` stay at root. `AppState` grows the concrete service instances (type-aliased
composition in `main.rs`); `CreateUser` stays unwired (M2).

### 6.3 gRPC (tonic)

`TenancyService` server implementation in `adapters/grpc.rs` (or a `grpc/` module if it
outgrows one file), added alongside tonic-health on the existing server; same shutdown +
timeout wiring as M0.

## 7. Testing

- **Core unit tests:** `Slug` parsing table, typed-id constructor rejections (wrong
  service/resource-type/org-slot), `TenancyNodeRef` ↔ PRN round-trip, and service tests with
  in-memory fakes covering every rule in §5.1 (invariant, guards, cascade detach, default
  team, idempotent archive, restore-under-archived-ancestor, rename validation).
- **Integration tests (testcontainers,** reusing `start_migrated_postgres`**):**
  1. chain round-trip: create org (default team returned) → team → project; parent/child
     queries; PRN stability/uniqueness (AC 1 + 2);
  2. membership: attach org/team/project with invariant, duplicate → conflict, org detach
     cascades;
  3. derived status: archive org → children report `effective_status=archived`, restore, and
     write-guards enforced;
  4. constraint tests: slug conflicts and composite-FK integrity hit real PG errors;
  5. HTTP round-trip via axum `oneshot` (status codes incl. 400/404/409 paths);
  6. gRPC `TenancyService` smoke test on an ephemeral port (per `grpc_health.rs`).
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
  audit actor fields stay empty.
- Service accounts & API keys (M4); audit log & outbox events (M5/M6).
- Project/team re-parenting (PRNs already immutable-by-design for it), hard delete,
  Redis caching, Py/TS client wrappers.
- Kernel changes — none needed; PRN surface from SMA-448 suffices.
- New ADR — ADR-0014 governs; flip to Accepted post-GATE 1.
