# SMA-444: M3 Authorization (Cedar policy engine) — Design

- **Issue:** [SMA-444](https://linear.app/smaschek/issue/SMA-444) · Epic M3 of 6 · IAM v1 vertical slice
- **Date:** 2026-07-07
- **Status:** Approved (GATE 1 passed 2026-07-07; challenge folded in; ServiceAccount cut, ListOrganizations platform-only)
- **Governing ADRs:** ADR-0013 (Cedar embedded for authorization — implemented by this epic),
  ADR-0014 (tenancy hierarchy & PRN scheme → Cedar entity hierarchy), ADR-0015 (Authenticator
  port; the `AuthContext` this epic authorizes comes from it), ADR-0005 (kernel-first),
  ADR-0004 (proto contracts), ADR-0006 (open-core boundary; managed Cedar path).
- **Precedents:** `docs/superpowers/specs/2026-07-06-sma-443-m2-authentication-design.md` (M2 authn),
  `docs/superpowers/specs/2026-07-05-sma-442-m1-tenancy-design.md` (M1 tenancy),
  `docs/superpowers/specs/2026-06-30-sma-448-kernel-prn-design.md` (PRN + `to_cedar_uid`).

## 1. Context & goal

M1 shipped the tenancy spine (org → team → project + memberships); M2 added authentication
(who you are). M3 adds **authorization** (what you may do): an embedded **Cedar** policy engine
answering `is_authorized(principal, action, resource, context) -> Decision`, **default-deny**,
behind a new `Authorizer` port. Per ADR-0013 the engine (`cedar-policy` crate) is embedded
in-process in the pure `paigasus-iam-core` libs crate; the I/O of loading policies and entity
slices stays in the service behind ports.

The kernel already provides `paigasus_kernel::cedar::to_cedar_uid` (PRN → Cedar entity uid),
so the entity-naming contract is fixed and shared across services. M3 builds the schema, the
starter policy set, the roles-as-grants model, the decision path over HTTP + gRPC + a `tower`
authorization middleware, the entity-slice + decision caches with invalidation, and
determining-policy diagnostics feeding an audit port.

**Acceptance criteria (from the issue):**

1. `is_authorized` enforces the starter policy set; **a role grant changes effective access**.
2. **A denial names its determining policy.**
3. **A policy change takes effect within the cache-TTL bound.**

Scope posture chosen at brainstorm: **full scope in one PR** (all six issue bullets, including
Redis entity-slice + decision caching with active invalidation), matching M1's single-large-PR
rhythm. §17 lists the concrete decomposition into commits/tasks so "one PR" stays coherent.

## 2. Decisions (settled during brainstorm; refined by the Stage 2 challenge — see §18)

| # | Decision | Choice |
|---|----------|--------|
| D1 | Engine placement | Embed **`cedar-policy` 4.11.x** (Apache-2.0) in the pure `paigasus-iam-core` behind the `Authorizer` port. Cedar evaluation is pure (no I/O); policy/entity loading stays in the service via ports. |
| D2 | Roles & grants representation | **Template-linked policies** (approach A). Each system role is a Cedar policy **template** `permit(principal == ?principal, action in [<role actions>], resource in ?resource)`; a `RoleGrant(principal, role, scope-node)` materializes as one **template-linked policy** (id `grant:<uuid>`). Grant = add a linked policy; revoke = remove it. Scope flows down the tenancy hierarchy via `resource in ?resource`. Rejected: synthetic role-group entities (B) and generated concrete-policy strings (C). |
| D3 | Cedar principal type | **A single `Pgs::Iam::Principal` type** keyed by the `principal` PRN (`prn:pgs:iam:::principal/<uuid>`) via `to_cedar_uid` unchanged, with `kind` (`user`/`service_account`) as an **attribute** — because every authenticated identity in the codebase is a `principal` PRN and `AuthContext` carries no `kind` (challenge B1). Grants, requests, `GrantRoleRequest.principal_prn`, and memberships all key on the same `Principal` uid. |
| D4 | Hierarchy root | A singleton `Pgs::Iam::Root::"paigasus"` sits above all orgs (`Organization in Root`). Unscoped actions (`CreateOrganization`, `PutPolicy`, `DeletePolicy`, `ListPolicies`, `ListOrganizations`, platform-level `GrantRole`) authorize against `Root`; a `platform_admin` grant at `Root` covers everything. `Root` is **synthetic** (not a DB row): the slice loader injects the `Root` entity + the `Organization→Root` edge into every slice (§7). |
| D5 | ServiceAccount | **Cut from M3** (GATE 1 — challenge M8: M2 auth is OIDC-bearer-only, so an SA can't authenticate in M3). Principals are **user-only** in M3; the single `Pgs::Iam::Principal` type still carries a `kind` attribute, so re-adding `service_account` (with its credential path) later is cheap. No `service_account` table, `CreateServiceAccount`, or `ServiceAccountService` in this epic. |
| D6 | Policy authoring | **Full policy CRUD API** (`PutPolicy`/`DeletePolicy`/`ListPolicies`) with **write-time schema validation** and its own authorization (Root-scoped). **System policies/roles are immutable** (`system` flag): the CRUD API rejects edits to them and boot-reconcile is compare-and-warn (challenge M5). Roles are system-defined/seeded (no custom-role API in M3). |
| D7 | Enforcement scope | **Retrofit authorization onto the existing tenancy/membership routes** (HTTP + gRPC), fine-grained per-operation, plus a reusable coarse `tower` `AuthzLayer` for the M5 gateway. Gated by `authz.enforce_tenancy` (default `true`). Adds a `Forbidden` error class → HTTP 403 / gRPC `PermissionDenied` (challenge M9). |
| D8 | Org ownership | `CreateOrganization` requires `platform_admin` (against `Root`) **and** atomically seeds an `org_admin` `RoleGrant` for the creating principal on the new org, in the **same transaction** as org + default-team creation (ADR-0014). |
| D9 | Cold-start | `authz.bootstrap_admins` is a list of **`(issuer, subject)`** identities (not principal PRNs — principals are minted server-side at JIT, challenge M4). When such an identity first provisions, the `platform_admin` (Root) grant is materialized in the same txn as provisioning. Boot logs a loud warning when no bootstrap admin is configured (lockout risk). |
| D10 | Audit | Determining-policy diagnostics feed an **`AuditSink` port**; default impl `TracingAuditSink`. The persistent audit store is M5 (SMA-446); M3 ships the port + tracing sink only. |
| D11 | Caches & generations | **Two generation counters** (`policy_gen`, `entity_gen`) plus TTLs. The in-memory policy snapshot (authoritative) reloads on `policy_gen` advance or TTL; decision- and slice-cache keys **embed both generations** so a policy/grant/tenancy change orphans stale entries without any SCAN/DEL (challenge M1/M2). `authz.cache.backend = memory|redis`, mirroring M2's `jwks_cache`. Decisions **never depend on Redis being up**. |
| D12 | Cache-outage posture | Entity-slice/decision caches **fail open to Postgres** (bypass on Redis error → still a real default-deny evaluation). Never fail open on the *decision*. (Contrast M2's JWKS cache, which fails closed because Redis is its only key source; here Postgres is the source of truth.) |
| D13 | ADR | Implements the existing **ADR-0013** (Cedar). Flip its status Proposed → Accepted at GATE 1. No new ADR unless the challenge surfaces one. |

## 3. Cedar model

### 3.1 Entities & schema (`authz/schema.rs`, embedded `const &str`, parsed once via `OnceLock`)

Cedar namespace `Pgs::Iam` (matching `to_cedar_uid`'s `Pgs::<Service>::<Type>` output):

- **Principal:** a single type `Pgs::Iam::Principal` (D3), keyed by the `principal` PRN's uuid.
  Attributes: `kind` (String: `user` in M3; forward-compatible with `service_account`, deferred
  per D5), `status` (String: `active`).
- **Resources (memberOf hierarchy):** `Pgs::Iam::Root`, `Pgs::Iam::Organization`,
  `Pgs::Iam::Team`, `Pgs::Iam::Project`. Parent edges: `Organization in Root`, `Team in
  Organization`, `Project in Team`. Attribute: `effective_status` (String: `active|archived`)
  so policies can `forbid` writes on an archived subtree.
- **Roles** are **not** entity types (D2 — roles are templates, not groups).
- **Actions** (`action` declarations with `appliesTo` principal/resource types) — the IAM
  catalog, one per operation:
  - Tenancy reads: `GetOrganization`, `ListOrganizations`, `GetTeam`, `ListTeams`,
    `GetProject`, `ListProjects`, `ListMemberships`.
  - Tenancy writes: `CreateOrganization`, `RenameOrganization`, `ArchiveOrganization`,
    `RestoreOrganization`, `CreateTeam`, `RenameTeam`, `ArchiveTeam`, `RestoreTeam`,
    `CreateProject`, `RenameProject`, `ArchiveProject`, `RestoreProject`,
    `AttachMembership`, `DetachMembership`.
  - Authz management: `PutPolicy`, `DeletePolicy`, `ListPolicies`, `GrantRole`,
    `RevokeRole`, `ListRoleGrants`.
  - **`IsAuthorized` is *not* a Cedar action** (challenge Q3): the *RPC-level* access control on
    the check itself (may the caller ask?) is the §9.2 self/admin rule, not a policy-gated
    action. The `action` field of an `IsAuthorized` request is any catalog action above.

The schema is **validated at build/test time** (a unit test parses it) and every starter policy
is validation-checked against it.

### 3.2 Starter policy set (`authz/roles.rs`, code-defined; `system`-flagged; reconciled at boot)

**Base policies** (`policy.kind = static`, `system = true`):

- `forbid-archived-writes`: `forbid(principal, action in [<write actions>], resource) when {
  resource has effective_status && resource.effective_status == "archived" };` — belt-and-braces
  over M1's in-txn guards.

**Role templates** (one `policy` row each, `kind = template`, `system = true`, referenced by a
`role` row):

| Role key | Template actions (⊆ catalog) | Grantable at | Confers `GrantRole`? |
|----------|------------------------------|--------------|----------------------|
| `platform_admin` | all actions (`permit(principal == ?principal, action, resource in ?resource)`) | `Root` | yes (any scope) |
| `org_admin` | all org-scoped tenancy + membership + `GrantRole`/`RevokeRole`/`ListRoleGrants` | `Organization` | yes (within its org subtree) |
| `org_member` | org/team/project **reads** | `Organization` | no |
| `team_admin` | team + project writes/reads, membership within team | `Team` | yes (within its team subtree) |
| `team_member` | team/project reads | `Team` | no |
| `project_admin` | project writes/reads within the project | `Project` | yes (within the project) |
| `project_member` | project reads | `Project` | no |

A grant binds `?principal` (the grantee's `Principal` uid) and `?resource` (the scope node's
Cedar uid). Because `Project in Team in Org in Root`, an `org_admin` grant satisfies `resource in
?resource` for the org and every team/project under it.

**Anti-escalation invariant (challenge Q1):** `GrantRole`/`RevokeRole` authorize against the
**grant's scope node** (§9.4), and a role is only grantable where the actor holds `GrantRole`
authority. So an `org_admin` on O can grant only roles scoped within O's subtree and can **never**
grant `platform_admin` (that requires `GrantRole` at `Root`, i.e. platform_admin). No role
confers more than its own actions.

### 3.3 Decision path (pure, `authz/engine.rs`)

`PolicyEngine::decide(policies, entities, request) -> Decision`:

1. Build a `cedar_policy::Request` from `(principal uid, action uid, resource uid, context)`.
2. `cedar_policy::Authorizer::is_authorized(request, policies, entities)`.
3. Map `Response`: `decision()` → `Effect::{Allow,Deny}`; `diagnostics().reason()` (policy ids)
   → `determining_policies: Vec<String>`. On `Deny` with an empty reason set → the marker
   `"default-deny (no matching permit)"` so AC2 always names *something*.
4. `diagnostics().errors()` non-empty → `AuthzError::Evaluation` (fail closed: treated as Deny,
   logged; never Allow).

## 4. Domain model (`rs/crates/libs/paigasus-iam-core/src/authz/`, pure)

### 4.1 Value objects & entities (`authz/model.rs`)

- `Action` (enum, `authz/action.rs`): the catalog; `as_wire`/`parse`/`to_cedar_uid`; a
  `is_write()` classification (used by `forbid-archived-writes` and the retrofit map).
- `AccessRequest { principal: Prn, action: Action, resource: Prn, context: RequestContext }`.
- `RequestContext`: a small typed map (`BTreeMap<String, ContextValue>`, `ContextValue =
  String|Long|Bool`). **Kept because it is the PARC/ADR signature and the wire contract**, but
  the v1 starter policies do not consume it (challenge MINOR); the tenancy middleware passes an
  empty context.
- `Decision { effect: Effect, determining_policies: Vec<String> }`.
- `Role { key: String, template_id: String, scope_kinds: Vec<NodeKind>, description: String,
  system: bool }`.
- `RoleGrant { id: Uuid, principal: PrincipalId, role_key: String, scope: GrantScope,
  linked_policy_id: String, created_at }` where `GrantScope = Root | Node(TenancyNodeRef)`.
- `RoleGrantRef { scope: GrantScope, role_key: String }` — the structured introspection shape
  (challenge M10), replacing the `Vec<Prn>` overload.
- `PolicyDocument { policy_id, kind: PolicyKind (Static|Template), source, description, system:
  bool, created_at, updated_at }`.
- `EntitySlice { entities: Vec<SliceEntity> }` — Root + resource + ancestor chain + principal, in
  a transport-neutral shape the adapter converts into `cedar_policy::Entities`.

### 4.2 Error taxonomy

- Core `AuthzError` (`authz/model.rs`): `PolicyParse` / `SchemaValidation` (write-time) ·
  `TemplateLink` · `Evaluation` (Cedar internal errors → fail closed) · `UnknownRole` ·
  `InvalidScope` (role not grantable at that node kind) · `SystemImmutable` (edit of a `system`
  policy/role) · `Backend`. Source-preserving; never leaks raw backend text.
- Service application error: add a **`Forbidden`** class to the existing `ErrorClass`/`ApiError`
  funnels (challenge M9) — `application/error.rs`, `adapters/http/error.rs`, and the gRPC
  `status_to_grpc` map — rendering **HTTP 403** / gRPC **`PermissionDenied`**. This is new: the
  M1/M2 funnels have no 403 today.

### 4.3 Ports (`authz/ports.rs`, `#[async_trait]`)

- `Authorizer { async fn is_authorized(&self, req: &AccessRequest) -> Result<Decision, AuthzError>; }`
- `PolicyStore { list_all_in_txn, put [validates + rejects system rows], delete [rejects system],
  policy_gen, bump_policy_gen }`.
- `RoleGrantStore { grant, revoke, list_by_principal, list_by_scope }` (grant/revoke bump
  `policy_gen`).
- `EntitySliceLoader { load(resource, principal) -> EntitySlice }` (injects Root).
- `DecisionCache { get, put }` (key embeds `policy_gen` + `entity_gen`).
- `AuditSink { record(&AuthzDecisionEvent) }`.

`assert_object_safe` test extended to cover the new trait objects.

## 5. Cedar engine adapter & pure core

`cedar-policy` is a **normal dependency of `paigasus-iam-core`**. It is *not* in the wasm/binding
tree (bindings depend on `paigasus-kernel`, not `paigasus-iam-core`), so the
`repo:wasm-getrandom-free` gate is unaffected. Engine, schema, action catalog, model,
template-linking, and write-time validation live in the pure core and are unit-testable with no
DB/Redis.

## 6. Persistence (`services/paigasus-iam`)

### 6.1 Migration `m0004_create_authz`

Follows the m0002 convention (named constraints via raw SQL where the D7 error-mapping needs
stable names). Tables:

- `policy(policy_id TEXT PK, kind TEXT NOT NULL CHECK (kind IN ('static','template')),
  source TEXT NOT NULL, description TEXT, system BOOLEAN NOT NULL DEFAULT false, created_at,
  updated_at)`.
- `role(key TEXT PK, template_id TEXT NOT NULL FK→policy(policy_id), scope_kinds TEXT NOT NULL,
  description TEXT, system BOOLEAN NOT NULL DEFAULT false, created_at)`. `scope_kinds` is a
  **JSON array** of node kinds (e.g. `["organization"]`) — encoding pinned (challenge MINOR);
  roles are code-defined, the column is the persisted/introspectable form.
- `role_grant(id UUID PK, principal_id UUID FK→principal(id) ON DELETE CASCADE, role_key TEXT
  FK→role(key), scope_kind TEXT NOT NULL CHECK (scope_kind IN ('root','organization','team',
  'project')), scope_node_prn TEXT NOT NULL, scope_org_id/scope_team_id/scope_project_id UUID
  NULL FK→ respective node ON DELETE CASCADE, linked_policy_id TEXT NOT NULL UNIQUE, created_at)`,
  `UNIQUE(principal_id, role_key, scope_node_prn)`. A `CHECK` ties `scope_kind` to exactly the
  matching `scope_*_id` being non-null (all NULL for `root`), so a Root/platform grant is
  first-class rather than a NULL-count hack.

Registered in `migration/mod.rs` after `m0003`.

### 6.2 Adapters

- `PgPolicyStore` (`PolicyStore`): CRUD over `policy`; `put` first calls core `validate_policy`
  against the schema and **rejects `system` rows** (returns `SystemImmutable`); `policy_gen`
  reads / `bump_policy_gen` `INCR`s the counter (Redis for the `redis` backend, an in-process
  `AtomicU64` for `memory`).
- `PgRoleGrantStore` (`RoleGrantStore`): `grant` inserts a `role_grant` row **and** registers the
  template-linked policy in one txn, then bumps `policy_gen`; `revoke` removes both + bumps.
- `PgEntitySliceLoader`: loads a node + its ancestors (reusing M1's node reads), the principal,
  and **injects the synthetic `Root` entity + `Organization→Root` edge** (challenge MINOR). The
  policy/grant snapshot load reads all policies + grants in **one repeatable-read txn** to avoid a
  torn snapshot (challenge MINOR).
- Tenancy write adapters (`set_status`, create, rename) additionally **bump `entity_gen`** so the
  decision/slice caches see the change (challenge M2).

## 7. Caching architecture (`adapters/authz/`)

Two generations, both cheap counters (Redis `INCR` for the `redis` backend; in-process
`AtomicU64` for `memory`):

- **`policy_gen`** — bumped on any policy CRUD or role grant/revoke.
- **`entity_gen`** — bumped on any tenancy mutation (create/rename/`set_status` of org/team/
  project) that can change a slice or `effective_status`.

Components:

- **Policy snapshot** (`policy_snapshot.rs`): `ArcSwap<CompiledPolicies { policy_set, schema,
  gen }>`, built by loading all `policy` rows + all `role_grant` linked policies (one
  repeatable-read txn) and compiling. **Authoritative** — decisions evaluate against it even if
  Redis is down. A single background reload task (spawned once in `main`, wired to the shutdown
  watch channel) reloads when `store.policy_gen() > snapshot.gen` or `policy_cache_ttl_secs`
  elapsed; additionally, a decision that observes `policy_gen > snapshot.gen` triggers a
  synchronous reload before deciding, so a **grant is visible on the same replica immediately**
  (AC1) and cross-replica within the poll interval + TTL.
- **Entity-slice cache** (`entity_cache.rs`, Redis, key `iam:authz:slice:<entity_gen>:<resource
  -prn>`): the `entity_gen` in the key means a tenancy change **orphans** old slices with no
  SCAN/DEL and no failed-invalidation retry problem; TTL `slice_cache_ttl_secs` GCs them.
- **Decision cache** (`decision_cache.rs`, Redis, key `iam:authz:dec:<policy_gen>:<entity_gen>:
  <blake3(principal,action,resource,context)>`): any policy/grant (`policy_gen`) or tenancy
  (`entity_gen`) change orphans stale entries; TTL `decision_cache_ttl_secs` is a GC backstop.
- **`CedarAuthorizer`** (`cedar_authorizer.rs`, `Authorizer`): decision-cache get → on miss, load
  slice (cache→Postgres) + snapshot policy set → `engine.decide` → `AuditSink.record` →
  decision-cache put → return. Redis errors on the accelerator caches are logged and **bypassed**
  (D12). It is **`Arc`-shared in `AppState`** (like M2's `WiredAuthenticator`) so every `AppState`
  clone shares one snapshot/reload-task/Redis handle, not a duplicate (challenge MINOR).

`memory` backend keeps generations/decisions in-process (single-replica; TTL still bounds AC3;
gen still gives immediate AC1). `redis` adds cross-replica invalidation.

## 8. Application layer (`services/paigasus-iam/src/application/`)

- `authorize.rs` — thin use case over the `Authorizer` port used by the middleware, the
  `IsAuthorized` RPC/route, and internally by other use cases.
- `roles.rs` — `GrantRole`/`RevokeRole`/`ListRoleGrants` (validates role exists, scope kind
  allowed, principal exists; authorizes the actor against the **grant's scope node**, §3.2).
- `policies.rs` — `PutPolicy`/`DeletePolicy`/`ListPolicies` (write-time validation; rejects
  `system` rows; authorizes against `Root`).
- `organizations.rs` (extend) — `CreateOrganization` authorizes `platform_admin` against `Root`,
  then in one txn creates org + default team **and** the seeded `org_admin` grant for the actor
  (D8), then bumps `policy_gen`.
- `bootstrap.rs` — at boot, reconcile the starter policies/roles (**compare-and-warn**, not blind
  upsert — challenge M5); on JIT-provision, if the `(issuer, subject)` matches
  `authz.bootstrap_admins`, seed the `platform_admin` grant in the provisioning txn (D9). Warn
  loudly at boot if `bootstrap_admins` is empty.

## 9. Wire surface

### 9.1 Proto (`contracts/proto/paigasus/iam/v1/iam.proto`)

Add the `AuthorizationService` (replacing the reserved placeholder comment). `IsAuthorizedResponse.determining_policies` is populated only for self/admin callers
(§9.2); other callers get `allowed` + a generic `reason` with `determining_policies` empty
(challenge M6).

```proto
message IsAuthorizedRequest { string principal_prn = 1; string action = 2; string resource_prn = 3; map<string,string> context = 4; }
message IsAuthorizedResponse { bool allowed = 1; repeated string determining_policies = 2; string reason = 3; }
message Policy { string policy_id = 1; string kind = 2; string source = 3; string description = 4; bool system = 5; }
message PutPolicyRequest { Policy policy = 1; }          // upsert; validated; rejects system ids
message PutPolicyResponse { Policy policy = 1; }
message DeletePolicyRequest { string policy_id = 1; }
message DeletePolicyResponse {}
message ListPoliciesRequest { uint32 limit = 1; uint64 offset = 2; }
message ListPoliciesResponse { repeated Policy policies = 1; }
message RoleGrant { string id = 1; string principal_prn = 2; string role_key = 3; string scope_prn = 4; }
message GrantRoleRequest { string principal_prn = 1; string role_key = 2; string scope_prn = 3; }
message GrantRoleResponse { RoleGrant grant = 1; }
message RevokeRoleRequest { string id = 1; }
message RevokeRoleResponse {}
message ListRoleGrantsRequest { string principal_prn = 1; uint32 limit = 2; uint64 offset = 3; }
message ListRoleGrantsResponse { repeated RoleGrant grants = 1; }

service AuthorizationService {
  rpc IsAuthorized(IsAuthorizedRequest) returns (IsAuthorizedResponse);
  rpc PutPolicy(PutPolicyRequest) returns (PutPolicyResponse);
  rpc DeletePolicy(DeletePolicyRequest) returns (DeletePolicyResponse);
  rpc ListPolicies(ListPoliciesRequest) returns (ListPoliciesResponse);
  rpc GrantRole(GrantRoleRequest) returns (GrantRoleResponse);
  rpc RevokeRole(RevokeRoleRequest) returns (RevokeRoleResponse);
  rpc ListRoleGrants(ListRoleGrantsRequest) returns (ListRoleGrantsResponse);
}
```

**Introspect role field (challenge M10):** the reserved `IntrospectResponse.role_group_prns`
(field 7, `repeated string`, always empty until now) is **reserved** and replaced by a structured
`repeated RoleGrantRef role_grants = 8; message RoleGrantRef { string scope_prn = 1; string
role_key = 2; }`, so we don't overload `Vec<Prn>` (`PrincipalContext.role_groups` in
`authn.rs` becomes `Vec<RoleGrantRef>`, and the gRPC converter is updated). Reserving-and-adding
keeps the `:breaking` gate green (the old field number/name is retired, not repurposed).

### 9.2 HTTP

- `POST /v1/authz/is-authorized` → `IsAuthorizedResponse`.
- `POST/GET/DELETE /v1/authz/policies` (+ `/{policy_id}`); `GET` lists (Root-scoped).
- `POST /v1/authz/role-grants` (grant), `GET` (list by principal), `DELETE /{id}` (revoke).

**`IsAuthorized` exposure (challenge M6, resolves old §16.1):** **self-only by default** — the
`principal_prn` in the request must equal the caller's `AuthContext` principal, unless the caller
holds a read/admin grant on the target resource, in which case third-party queries are allowed.
`determining_policies` (which can contain `grant:<uuid>` ids) are returned **only** to self/admin
callers; other callers get allow/deny + a generic reason. Management routes are behind bearer
auth (M2) **and** authorized (`PutPolicy`→Root, `GrantRole`→scope node).

### 9.3 gRPC

`AuthorizationServiceServer` added to `grpc::router`; the existing `AuthLayer` bearer enforcement
covers it (none of its RPCs are `:path`-exempt).

### 9.4 Authorization middleware & tenancy retrofit

- A reusable coarse **`AuthzLayer`** (`adapters/http/authz_middleware.rs`; a `tower` layer /
  `from_fn_with_state`) taking an `Action` + a resource extractor — shipped and unit-tested; the
  form the M5 gateway reuses.
- **Fine-grained enforcement** on the tenancy routes: each handler, after the M2 `AuthContext` is
  established, calls `authorize(ctx.principal, <action>, <resource-prn>, empty-context)` and
  returns **403 / `PermissionDenied`** on deny. Gated by `authz.enforce_tenancy`.

Action → resource binding (writes on the node; creates on the parent; reads coarse-gated on the
parent/Root; **lists are coarse-gated, not per-item filtered in v1** — challenge M3):

| Operation(s) | Action | Resource authorized |
|---|---|---|
| CreateOrganization | `CreateOrganization` | `Root` |
| ListOrganizations | `ListOrganizations` | `Root` (**platform-only in v1** — posture change, see GATE 1) |
| Get/Rename/Archive/Restore Organization | resp. | the org |
| CreateTeam / ListTeams | resp. | the parent org |
| Get/Rename/Archive/Restore Team | resp. | the team |
| CreateProject / ListProjects | resp. | the parent team |
| Get/Rename/Archive/Restore Project | resp. | the project |
| Attach/Detach/List Membership | resp. | the target tenancy node |
| PutPolicy/DeletePolicy/ListPolicies | resp. | `Root` |
| GrantRole/RevokeRole | resp. | the grant's **scope node** (anti-escalation) |
| ListRoleGrants | `ListRoleGrants` | self, or the principal's org for an `org_admin`; else `Root` |

**Test-migration cost (challenge M9):** every M1/M2 tenancy integration test now needs (1) the
acting principal provisioned (a first authenticated call JIT-provisions it), then (2) a seeded
grant at the right scope, in order. A `support::seed_platform_admin(&principal)` /
`seed_org_admin(&principal, &org)` helper is added; reads that now require grants are updated. The
suites are migrated **within this PR** (default `enforce_tenancy = true`).

## 10. Diagnostics → audit

`CedarAuthorizer` records an `AuthzDecisionEvent { principal_prn, action, resource_prn, effect,
determining_policies, ts }` to the `AuditSink` on every decision. `TracingAuditSink` emits a
structured event (`info` allow / `warn` deny) with the determining policy ids — never token/claim
material. The persistent audit store + event stream is M5 (SMA-446).

## 11. Configuration (`config.rs`, figment) — new `[authz]` block

```toml
[authz]
enforce_tenancy = true            # gate the retrofit; default true
policy_cache_ttl_secs = 30        # AC3 staleness bound
slice_cache_ttl_secs = 60
decision_cache_ttl_secs = 30
refresh_interval_secs = 1         # background snapshot-gen poll (cross-replica AC1 bound)

[authz.cache]
backend = "memory"                # or "redis" (mirrors authn.jwks_cache)
# redis_url = "redis://..."       # required iff backend = "redis"

# Cold-start platform admins, keyed by OIDC identity (NOT principal PRN — challenge M4):
[[authz.bootstrap_admins]]
issuer = "https://idp.example.com/realms/acme"
subject = "abc-123"
```

`IamConfig::validate` extends: `redis` cache backend requires `redis_url`; TTLs ≥ 1; each
`bootstrap_admins` entry has a valid `https` issuer + non-empty subject. Empty `bootstrap_admins`
is allowed but logged as a lockout warning at boot.

## 12. Testing

- **Cedar policy test suite in CI** (pure, `paigasus-iam-core`): a table of `(principal, action,
  resource, context) → expect Allow/Deny` over the starter policy set + representative linked
  grants; plus a test parsing the schema and **validating every starter policy** against it. Fast,
  no DB/Redis — the issue's "Cedar policy test suite in CI" deliverable.
- **Unit tests:** engine `decide` (allow/deny/default-deny/eval-error), template linking, action
  catalog round-trip, generation/key logic, slice assembly (incl. Root injection), config validate,
  the anti-escalation `GrantRole` invariant, `SystemImmutable` rejection.
- **Integration (testcontainers Postgres + Redis)** — the three ACs over HTTP + gRPC:
  - **AC1:** provision principal P; `is_authorized(P, CreateProject, org/…)` → deny; grant
    `org_admin` on the org; same call → allow (immediate via `policy_gen` bump + synchronous
    reload).
  - **AC2:** a denied decision lists its determining policy id (or the default-deny marker); an
    allowed decision lists the `grant:<uuid>` linked policy; a non-self caller gets it redacted.
  - **AC3:** `PutPolicy` edits a policy; with a short `policy_cache_ttl_secs`, the change is
    observed within the TTL bound (and immediately via the generation counter).
  - Retrofit: a representative tenancy write is 403 without a grant, 200 with one; archive bumps
    `entity_gen` and a stale allow is not served.
- **Redis-specific:** cross-replica invalidation (two `CedarAuthorizer`s sharing one Redis; a
  grant via one is visible to the other within `refresh_interval_secs`); cache-outage
  fail-open-to-Postgres (D12).

## 13. Build / CI wiring

- `paigasus-iam-core/Cargo.toml`: add `cedar-policy = "4"`. `paigasus-iam/Cargo.toml`: add
  `blake3` (decision-cache key hashing); reuse `redis`, `sea-orm`, `serde_json`.
- **`rs/deny.toml`:** `cedar-policy` is Apache-2.0 (allowed). Its transitive tree may pull a
  license not yet on the allow-list or a new advisory → add scoped `[licenses] exceptions` /
  dev-only `[advisories] ignore` as needed (per the repo-gates note). Confirm during the plan.
- **`ci/affected-graph/run.sh`:** no new crate (authz lives in existing `paigasus-iam-core-rs` +
  `paigasus-iam-rs`), so the strict-equality sets are unchanged; verify the `paigasus-iam-core`
  edit case still matches.
- **Proto codegen:** `buf generate` regenerates Rust/Py/TS for the new services; run
  `parity-corpus-drift` / codegen-drift; the generated `paigasus-proto` Rust is committed.
- Run the full graph before pushing: `moon ci :build :test :lint :fmt :deny :machete :typecheck
  :breaking :affected-smoke :parity-corpus-drift :wasm-getrandom-free --base origin/main
  --include-relations`. `:breaking` — adding services/messages + reserving-and-adding the
  introspect field is additive/non-breaking; confirm.

## 14. Out of scope (deferred)

- **Persistent audit store / event stream** — M5 (SMA-446). M3 ships the `AuditSink` port +
  `TracingAuditSink` only.
- **ServiceAccount entirely** — deferred (GATE 1, D5). The single `Pgs::Iam::Principal` type keeps
  a `kind` attribute so re-adding `service_account` + its credential path later is cheap.
- **Gateway-wide enforcement** — M5. M3 enforces within IAM's own surface + ships `AuthzLayer`.
- **Custom (user-defined) roles** — roles are system-seeded; policy CRUD ships, role creation does
  not.
- **Per-principal policy slicing** — M3 compiles the full policy set into the snapshot; slicing is
  a scale optimization (see §16.1).
- **Authorization-filtered list results** — v1 lists are coarse-gated, not per-item filtered.
- **Cedar-WASM in-process TS path** — future (ADR-0013/ADR-0005).

## 15. AC traceability

| AC | Mechanism | Test |
|----|-----------|------|
| 1 — role grant changes effective access | template-linked policies (D2); `policy_gen` bump + synchronous snapshot reload | AC1 integration (HTTP + gRPC) |
| 2 — denial names its determining policy | `Response.diagnostics().reason()` → `determining_policies`; default-deny marker (§3.3); redacted for non-self | AC2 integration + engine unit tests |
| 3 — policy change within cache-TTL | policy snapshot TTL (`policy_cache_ttl_secs`) + `policy_gen` counter | AC3 integration (short TTL) |

## 16. Remaining open questions & risks (for GATE 1 / plan)

Resolved at GATE 1: **ServiceAccount cut from M3** (D5); **`ListOrganizations` is platform-only in
v1** (§9.4). Remaining:

1. **Policy-set scale (challenge Q2):** the snapshot recompiles *all* linked grants on every
   grant/revoke. Fine at v1 scale (thousands of grants); `policy_cache_ttl_secs` must exceed the
   full-recompile latency. Per-principal slicing (§14) is the escape hatch. Confirm expected order
   of magnitude during the plan.

## 17. Decomposition (keeps "one PR" coherent)

Ordered, each independently reviewable/testable: (1) core `authz` module — schema, action catalog,
engine, model, ports, `Principal` type (with `kind` attr), unit + Cedar policy tests; (2) proto
`AuthorizationService` + `RoleGrantRef` + codegen; (3) migration `m0004` + Pg adapters
(policy/role/role_grant) + generations; (4) caches + `CedarAuthorizer` + `AppState` wiring + reload
task; (5) application use cases + bootstrap/reconcile + `Forbidden` error class; (6) HTTP + gRPC
surfaces (is-authorized, policy CRUD, role grants); (7) `AuthzLayer` + tenancy retrofit + test-suite
migration; (8) integration tests for the 3 ACs + Redis cross-replica; (9) `deny.toml`/CI wiring.

## 18. Change log — Stage 2 adversarial challenge

Folded in: **B1** (single `Pgs::Iam::Principal` type, D3) · **M1/M2** (two-generation cache model,
D11/§7) · **M3** (list resource bindings + platform-only `ListOrganizations`, §9.4) · **M4**
(bootstrap by `(issuer,subject)`, D9/§11) · **M5** (immutable system policies, D6/§6) · **M6**
(`IsAuthorized` self-only + redaction, §9.2) · **M7** (mgmt resource-binding table + CreateSA→org,
§9.4) · **M9** (`Forbidden`/403 error class + test-migration cost, §4.2/§9.4) · **M10** (structured
`RoleGrantRef`, §9.1) · minors (Arc-shared authorizer + single reload task, single-txn snapshot
load, Root injection, `scope_kinds` JSON, drop `IsAuthorized` from Cedar catalog, context kept as
contract-only). Resolved at GATE 1: **M8** — ServiceAccount **cut** from M3 (D5); `ListOrganizations` platform-only
(§9.4). Kept as a noted risk: policy-set scale (challenge Q2, §16.1).
