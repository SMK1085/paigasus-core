# M3 Authorization (Cedar policy engine) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Embed the Cedar policy engine in `paigasus-iam` so `is_authorized(principal, action, resource, context)` enforces a starter policy set (default-deny) over HTTP + gRPC + a `tower` middleware, with role-grants as template-linked policies, entity-slice + decision caching with generation-based invalidation, and determining-policy diagnostics → an audit port.

**Architecture:** Pure Cedar engine + schema + model + ports live in `paigasus-iam-core` (`authz` module); the service provides Postgres stores, Redis caches, a `CedarAuthorizer` adapter, application use cases, and HTTP/gRPC surfaces. Roles are Cedar policy *templates*; a `RoleGrant` is a template-linked policy (`grant:<uuid>`). An authoritative in-memory policy snapshot (reloaded on a `policy_gen` counter + TTL) plus generation-keyed Redis decision/slice caches give AC1 (immediate on grant) and AC3 (change within TTL); Cedar's `diagnostics().reason()` gives AC2.

**Tech Stack:** Rust (edition 2024, rust-version 1.95), `cedar-policy` 4.11.x, `sea-orm` 1 (Postgres), `redis` 1, `axum` 0.8, `tonic` 0.14, `blake3`, `figment`, `testcontainers`. Design spec: `docs/superpowers/specs/2026-07-07-sma-444-m3-authorization-cedar-design.md`.

## Global Constraints

- **Edition 2024 + rust-version 1.95** on every crate (already the workspace default; do not add per-crate overrides).
- **SPDX header** on every new source file: `// SPDX-License-Identifier: Apache-2.0` (first line).
- **Hexagonal split (ADR-0005):** ports + pure domain in `paigasus-iam-core`; no `sea-orm`/`axum`/`tonic`/`redis`/`cedar-policy` I/O in the core except `cedar-policy` itself, which is a **pure** eval dependency (allowed in the core). No backend error text leaks through core error types.
- **Generic DI by value** for application services (`Service<Repo, Ids, Clock, …>`), never `Arc<dyn>` — mirror `MembershipService` / `AuthenticateToken`.
- **Migrations** use `sea-orm-migration`; create UNIQUE/CHECK constraints with **explicit names via raw `execute_unprepared` SQL** (mirror `m0002_create_tenancy.rs`) so the D7 error-mapping matches.
- **`cedar-policy` must not enter the wasm/binding tree.** It is only in `paigasus-iam-core` + `paigasus-iam`; bindings depend on `paigasus-kernel`. `moon run repo:wasm-getrandom-free` must stay green.
- **PATH for tooling:** every shell step prefixes `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` so moon/cargo/buf/nextest resolve to the pinned versions.
- **Commits are SSH-signed via 1Password.** Use `--no-verify` in this worktree (lefthook's `commit-msg` commitlint hook is not installed here; CI's parity gate validates messages). Conventional-commit subjects, scope `rs` (or `contracts` for the proto-only task); **no `#NNN` in the body** (Linear auto-links by branch).
- **Cedar entity naming** is `paigasus_kernel::cedar::to_cedar_uid` (already shipped): `principal` PRN → `Pgs::Iam::Principal::"<uuid>"`, tenancy PRNs → `Pgs::Iam::{Organization,Team,Project}::"<uuid>"`. A single `Pgs::Iam::Principal` Cedar type (kind is an attribute). Do **not** introduce `User`/`ServiceAccount` Cedar types.
- **ServiceAccount is out of scope** (GATE 1): principals are user-only; keep the `kind` attribute forward-compatible but add no `service_account` table/RPC.
- **Run the full gate graph before the final push** (not just per-project tasks): `moon ci :build :test :lint :fmt :deny :machete :typecheck :breaking :affected-smoke :parity-corpus-drift :wasm-getrandom-free --base origin/main --include-relations`.

---

## File Structure

**`rs/crates/libs/paigasus-iam-core/` (pure core)**
- `Cargo.toml` — add `cedar-policy`.
- `src/authz/mod.rs` — module root + re-exports.
- `src/authz/schema.rs` — embedded Cedar schema string + `schema()` (parsed once) + `validate_policy`.
- `src/authz/action.rs` — `Action` catalog enum.
- `src/authz/model.rs` — `AccessRequest`, `Decision`, `Effect`, `AuthzError`, `Role`, `RoleGrant`, `GrantScope`, `RoleGrantRef`, `PolicyDocument`, `PolicyKind`, `NodeKind`, `RequestContext`, `ContextValue`, `EntitySlice`, `SliceEntity`, `AuthzDecisionEvent`.
- `src/authz/engine.rs` — `PolicyEngine` (Cedar wrapper): `decide`, `link_grant`, `compile`.
- `src/authz/ports.rs` — `Authorizer`, `PolicyStore`, `RoleGrantStore`, `EntitySliceLoader`, `DecisionCache`, `AuditSink`.
- `src/authz/roles.rs` — starter policy/role definitions + the Cedar policy test suite.
- `src/lib.rs` — `pub mod authz;` + re-exports.
- `src/authn.rs` — change `PrincipalContext.role_groups: Vec<Prn>` → `Vec<RoleGrantRef>`.

**`contracts/proto/paigasus/iam/v1/iam.proto`** — `AuthorizationService` + messages + `RoleGrantRef`; reserve `role_group_prns`, add `role_grants`.

**`rs/crates/services/paigasus-iam/` (adapters + application)**
- `Cargo.toml` — add `blake3`.
- `src/adapters/persistence/migration/m0004_create_authz.rs` (+ register in `migration/mod.rs`).
- `src/adapters/persistence/entities/{policy,role,role_grant}.rs` (+ `entities/mod.rs`).
- `src/adapters/persistence/{pg_policies,pg_role_grants,pg_entity_slice}.rs` (+ `persistence/mod.rs` exports).
- `src/adapters/authz/{mod,generation,policy_snapshot,entity_cache,decision_cache,cedar_authorizer,audit}.rs`.
- `src/application/{authorize,roles,policies,bootstrap}.rs` (+ `application/mod.rs`); extend `application/organizations.rs`, `application/error.rs`.
- `src/adapters/http/{authz,authz_middleware}.rs` + extend `adapters/http/{mod,dto,error}.rs` and each tenancy handler.
- `src/adapters/grpc/{authz}.rs` + extend `adapters/grpc/{mod,convert}.rs` and `tenancy.rs`.
- `src/config.rs` — `[authz]` block + validate.
- `src/main.rs` — spawn the snapshot reload task; seed bootstrap.
- `tests/{authz_decision,authz_policies,authz_roles,authz_retrofit,authz_redis}.rs` + `tests/support/mod.rs` grant helpers.

---

## Phase A — Core `authz` module (`paigasus-iam-core`)

### Task 1: Cedar dependency + schema

**Files:**
- Modify: `rs/crates/libs/paigasus-iam-core/Cargo.toml`
- Create: `rs/crates/libs/paigasus-iam-core/src/authz/mod.rs`, `src/authz/schema.rs`
- Modify: `rs/crates/libs/paigasus-iam-core/src/lib.rs`

**Interfaces:**
- Produces: `authz::schema::schema() -> &'static cedar_policy::Schema`; `authz::schema::validate_policy(src: &str) -> Result<(), authz::model::AuthzError>` (defined here but `AuthzError` lands in Task 3 — for Task 1 return `Result<(), String>` and switch to `AuthzError` in Task 3); `authz::schema::SCHEMA_SRC: &str`.

- [ ] **Step 1: Add the dependency.** In `Cargo.toml` `[dependencies]` add (alphabetical, with a justifying comment like the existing entries):
```toml
# cedar-policy — embedded authorization engine (ADR-0013). Pure evaluation (no I/O), so it
# lives in the pure core. NOT in the wasm/binding tree (bindings depend on paigasus-kernel),
# so `repo:wasm-getrandom-free` is unaffected.
cedar-policy = "4"
```
- [ ] **Step 2: Write the failing test** in `src/authz/schema.rs`:
```rust
// SPDX-License-Identifier: Apache-2.0
//! Embedded Cedar schema (ADR-0013) + write-time policy validation. Parsed once.

use cedar_policy::{Schema, Policy};
use std::str::FromStr;
use std::sync::OnceLock;

/// Cedar schema (human syntax). One `Principal` type (kind as an attribute); the tenancy
/// nodes form the resource hierarchy with a synthetic `Root` at the top.
pub const SCHEMA_SRC: &str = r#"
namespace Pgs::Iam {
  entity Root;
  entity Organization in [Root] { effective_status: String };
  entity Team in [Organization] { effective_status: String };
  entity Project in [Team] { effective_status: String };
  entity Principal { kind: String, status: String };

  action GetOrganization, ListOrganizations, GetTeam, ListTeams, GetProject, ListProjects,
         ListMemberships, CreateOrganization, RenameOrganization, ArchiveOrganization,
         RestoreOrganization, CreateTeam, RenameTeam, ArchiveTeam, RestoreTeam, CreateProject,
         RenameProject, ArchiveProject, RestoreProject, AttachMembership, DetachMembership,
         PutPolicy, DeletePolicy, ListPolicies, GrantRole, RevokeRole, ListRoleGrants
    appliesTo { principal: [Principal], resource: [Root, Organization, Team, Project] };
}
"#;

pub fn schema() -> &'static Schema {
    static SCHEMA: OnceLock<Schema> = OnceLock::new();
    SCHEMA.get_or_init(|| Schema::from_str(SCHEMA_SRC).expect("embedded Cedar schema is valid"))
}

/// Validate a policy's syntax + schema conformance at write time.
pub fn validate_policy(src: &str) -> Result<(), String> {
    use cedar_policy::{Validator, PolicySet, ValidationMode};
    let pset = PolicySet::from_str(src).map_err(|e| e.to_string())?;
    let result = Validator::new(schema().clone()).validate(&pset, ValidationMode::default());
    if result.validation_passed() { Ok(()) } else { Err(format!("{:?}", result)) }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn schema_parses() { let _ = schema(); }
    #[test]
    fn a_wellformed_permit_validates() {
        assert!(validate_policy(r#"permit(principal, action == Pgs::Iam::Action::"GetOrganization", resource);"#).is_ok());
    }
    #[test]
    fn a_malformed_policy_is_rejected() {
        assert!(validate_policy("permit(this is not cedar);").is_err());
    }
}
```
- [ ] **Step 3: Wire the module.** `src/authz/mod.rs`:
```rust
// SPDX-License-Identifier: Apache-2.0
//! Cedar authorization: schema, engine, model, ports, starter policies (ADR-0013).
pub mod schema;
```
Add `pub mod authz;` to `src/lib.rs` (after `pub mod authn;`).
- [ ] **Step 4: Run** `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cd rs && cargo test -p paigasus-iam-core authz::schema`. Expected: PASS (3 tests). If the schema string fails to parse, fix the Cedar human-syntax (check the `cedar-policy` 4.11 schema grammar — `entity X in [Y] { attr: T };` and `action A, B appliesTo { principal: [...], resource: [...] };`).
- [ ] **Step 5: Commit** `git add -A && git commit --no-verify -m "feat(rs): embed Cedar schema in paigasus-iam-core (SMA-444)"`.

> **Note for the implementer:** verify the exact `cedar-policy` 4.11 API names during Step 4 (`Schema::from_str`, `Validator::new`, `PolicySet::from_str`, `ValidationMode`, `validate`, `validation_passed`). If a name differs in the pinned version, use the crate's docs; the shapes above match the 4.x public API.

### Task 2: Action catalog (`action.rs`)

**Files:** Create `rs/crates/libs/paigasus-iam-core/src/authz/action.rs`; modify `authz/mod.rs`.

**Interfaces:**
- Produces: `enum Action` (one variant per schema action, `PascalCase`); `Action::as_wire(&self) -> &'static str` (the exact schema action name, e.g. `"GetOrganization"`); `Action::parse(&str) -> Option<Action>`; `Action::cedar_uid(&self) -> cedar_policy::EntityUid` (`Pgs::Iam::Action::"<wire>"`); `Action::is_write(&self) -> bool` (true for all Create/Rename/Archive/Restore/Attach/Detach/Put/Delete/Grant/Revoke).

- [ ] **Step 1: Failing test** in `action.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::Action;
    #[test]
    fn wire_roundtrip_all_variants() {
        for a in Action::ALL {
            assert_eq!(Action::parse(a.as_wire()), Some(*a), "{}", a.as_wire());
            assert_eq!(a.cedar_uid().type_name().to_string(), "Pgs::Iam::Action");
        }
    }
    #[test]
    fn write_classification() {
        assert!(Action::CreateTeam.is_write());
        assert!(!Action::GetTeam.is_write());
    }
}
```
- [ ] **Step 2:** Implement `Action` with a `pub const ALL: &[Action]` slice, `as_wire`/`parse` (a single match table — keep the wire string identical to the schema action name), `is_write`, and `cedar_uid` (`EntityUid::from_type_name_and_id` or `format!("Pgs::Iam::Action::\"{}\"", self.as_wire()).parse()`). Add SPDX header + `pub mod action;` to `mod.rs`.
- [ ] **Step 3: Run** `cargo test -p paigasus-iam-core authz::action`. Expected: PASS.
- [ ] **Step 4: Commit** `feat(rs): add IAM Cedar action catalog (SMA-444)`.

### Task 3: authz domain model (`model.rs`)

**Files:** Create `authz/model.rs`; modify `authz/mod.rs`; (retrofit Task 1's `validate_policy` return type to `Result<(), AuthzError>`).

**Interfaces (Produces — later tasks depend on these exact shapes):**
```rust
pub enum Effect { Allow, Deny }
pub struct Decision { pub effect: Effect, pub determining_policies: Vec<String> }
pub enum ContextValue { Str(String), Long(i64), Bool(bool) }
pub struct RequestContext(pub std::collections::BTreeMap<String, ContextValue>); // empty() ctor
pub struct AccessRequest { pub principal: Prn, pub action: Action, pub resource: Prn, pub context: RequestContext }
pub enum NodeKind { Root, Organization, Team, Project }
pub enum GrantScope { Root, Node(TenancyNodeRef) }   // canonical_prn(&self) -> String; kind(&self) -> NodeKind
pub struct Role { pub key: String, pub template_id: String, pub scope_kinds: Vec<NodeKind>, pub description: String, pub system: bool }
pub struct RoleGrant { pub id: Uuid, pub principal: PrincipalId, pub role_key: String, pub scope: GrantScope, pub linked_policy_id: String, pub created_at: DateTime<Utc> }
pub struct RoleGrantRef { pub scope_prn: String, pub role_key: String }
pub enum PolicyKind { Static, Template }
pub struct PolicyDocument { pub policy_id: String, pub kind: PolicyKind, pub source: String, pub description: String, pub system: bool, pub created_at: DateTime<Utc>, pub updated_at: DateTime<Utc> }
pub struct SliceEntity { pub uid: (String, String), pub parents: Vec<(String, String)>, pub attrs: BTreeMap<String, ContextValue> }
pub struct EntitySlice { pub entities: Vec<SliceEntity> }
pub struct AuthzDecisionEvent { pub principal_prn: String, pub action: String, pub resource_prn: String, pub effect: Effect, pub determining_policies: Vec<String>, pub at: DateTime<Utc> }
pub enum AuthzError { PolicyParse(String), SchemaValidation(String), TemplateLink(String), Evaluation(String), UnknownRole(String), InvalidScope(String), SystemImmutable(String), Backend(Box<dyn std::error::Error + Send + Sync>) }  // #[derive(thiserror::Error)]
```
- [ ] **Step 1: Failing test** — a `RoleGrantRef` round-trip + `GrantScope::canonical_prn` for Root (`prn:pgs:iam:::root/paigasus` or the agreed Root uid — see note) and for a node (delegates to `TenancyNodeRef::canonical`). Assert `AuthzError` `Display` for each variant is non-empty and leaks no `Backend` inner text beyond the wrapped source.
- [ ] **Step 2:** Implement the types (derive `Debug, Clone, PartialEq, Eq` where sensible; `AuthzError` derives `thiserror::Error`). Add `RequestContext::empty()`. `GrantScope::Root.canonical_prn()` returns the constant Root uid id (`"paigasus"`); expose `pub const ROOT_ENTITY: (&str, &str) = ("Pgs::Iam::Root", "paigasus");`. Switch Task 1's `validate_policy` to `Result<(), AuthzError>` (`AuthzError::PolicyParse`/`SchemaValidation`).
- [ ] **Step 3: Run** `cargo test -p paigasus-iam-core authz::model`. Expected: PASS.
- [ ] **Step 4: Commit** `feat(rs): add authz domain model + error taxonomy (SMA-444)`.

### Task 4: Cedar engine (`engine.rs`)

**Files:** Create `authz/engine.rs`; modify `authz/mod.rs`.

**Interfaces:**
- Consumes: `schema()`, `Action`, `EntitySlice`, `AccessRequest`, `Decision`, `AuthzError`, `to_cedar_uid`.
- Produces:
  - `PolicyEngine::compile(policies: &[PolicyDocument], grants: &[RoleGrant]) -> Result<CompiledPolicies, AuthzError>` where `CompiledPolicies { pub policy_set: cedar_policy::PolicySet, gen: u64 }` (gen set by caller/Task 12; default 0 here).
  - `PolicyEngine::decide(policies: &cedar_policy::PolicySet, slice: &EntitySlice, req: &AccessRequest) -> Decision` (never returns `Err`; a Cedar eval error maps to `Effect::Deny` + a `determining_policies = ["evaluation-error"]` marker and is logged by the caller — keep pure here, return the marker).
  - `link_grant(pset: &mut PolicySet, template_id: &str, grant: &RoleGrant) -> Result<(), AuthzError>` (`pset.link(PolicyId(template_id), PolicyId("grant:<uuid>"), {?principal, ?resource})`).
  - `DEFAULT_DENY_MARKER: &str = "default-deny (no matching permit)"`.

- [ ] **Step 1: Failing tests** (pure, no DB) covering: (a) an allow — a template `permit(principal == ?principal, action, resource in ?resource)` linked for `(principal P, org O)`, a slice with `Principal P` + `Project → Team → Org O → Root`, request `(P, CreateProject, project)` ⇒ `Allow` and `determining_policies == ["grant:<uuid>"]`; (b) default-deny — no linked policy ⇒ `Deny` and `determining_policies == [DEFAULT_DENY_MARKER]`; (c) forbid-archived — a `forbid(... when resource.effective_status == "archived")` base policy + an org grant + an archived project ⇒ `Deny` naming the forbid id.
- [ ] **Step 2:** Implement. Build `cedar_policy::Entities` from `EntitySlice` (`Entity::new` per `SliceEntity`, string uids parsed via `EntityUid::from_str`, parents as a `HashSet<EntityUid>`, attrs as `RestrictedExpression`). Build `Request::new(principal_uid, action.cedar_uid(), resource_uid, cedar_context, Some(schema()))`. Call `Authorizer::new().is_authorized(&req, pset, &entities)`. Map `response.decision()` + `response.diagnostics().reason()` (a `PolicyId` iterator → `to_string()`); empty reason on `Deny` → `[DEFAULT_DENY_MARKER]`. `link_grant` uses `pset.link(...)` with `to_cedar_uid` on the principal + scope PRNs.
- [ ] **Step 3: Run** `cargo test -p paigasus-iam-core authz::engine`. Expected: PASS.
- [ ] **Step 4: Commit** `feat(rs): add Cedar decision engine + template linking (SMA-444)`.

> **Implementer note:** the `cedar-policy` 4.x request/entities/response API (`Request::new`, `Entity::new`, `Entities::from_entities`, `Authorizer::is_authorized`, `Response::decision`, `Diagnostics::reason`) may have small signature differences vs. this sketch (e.g. `Request::new` returning `Result`, schema arg optional). Follow the pinned crate's rustdoc; keep the mapping semantics above.

### Task 5: authz ports (`ports.rs`)

**Files:** Create `authz/ports.rs`; modify `authz/mod.rs`, `lib.rs` (re-export the ports + key model types).

**Interfaces (Produces):**
```rust
#[async_trait] pub trait Authorizer: Send + Sync { async fn is_authorized(&self, req: &AccessRequest) -> Result<Decision, AuthzError>; }
#[async_trait] pub trait PolicyStore: Send + Sync {
    async fn list_all(&self) -> Result<Vec<PolicyDocument>, AuthzError>;      // one repeatable-read txn (impl detail)
    async fn put(&self, doc: &PolicyDocument) -> Result<(), AuthzError>;       // validates; rejects system
    async fn delete(&self, policy_id: &str) -> Result<(), AuthzError>;         // rejects system
    async fn policy_gen(&self) -> Result<u64, AuthzError>;
    async fn bump_policy_gen(&self) -> Result<u64, AuthzError>;
}
#[async_trait] pub trait RoleGrantStore: Send + Sync {
    async fn grant(&self, g: &RoleGrant) -> Result<(), AuthzError>;            // inserts row + bumps policy_gen
    async fn revoke(&self, id: Uuid) -> Result<(), AuthzError>;
    async fn list_all(&self) -> Result<Vec<RoleGrant>, AuthzError>;
    async fn list_by_principal(&self, p: &PrincipalId) -> Result<Vec<RoleGrant>, AuthzError>;
}
#[async_trait] pub trait EntitySliceLoader: Send + Sync {
    async fn load(&self, resource: &Prn, principal: &Prn) -> Result<EntitySlice, AuthzError>;
    async fn entity_gen(&self) -> Result<u64, AuthzError>;
}
#[async_trait] pub trait DecisionCache: Send + Sync {
    async fn get(&self, key: &str) -> Option<Decision>;
    async fn put(&self, key: &str, decision: &Decision);
}
#[async_trait] pub trait AuditSink: Send + Sync { async fn record(&self, ev: &AuthzDecisionEvent); }
```
- [ ] **Step 1:** Add an `assert_object_safe(_: &dyn Authorizer, _: &dyn PolicyStore, …)` compile test (mirror `ports.rs`'s existing one).
- [ ] **Step 2:** Implement the trait definitions (all `#[async_trait]`). Re-export from `lib.rs`: `pub use authz::{...}` (ports + `Action`, `Decision`, `Effect`, `AccessRequest`, `AuthzError`, `Role`, `RoleGrant`, `GrantScope`, `RoleGrantRef`, `PolicyDocument`, `RequestContext`).
- [ ] **Step 3: Run** `cargo test -p paigasus-iam-core authz`. Expected: PASS (compiles).
- [ ] **Step 4: Commit** `feat(rs): add authz ports (SMA-444)`.

### Task 6: Starter policy set + roles + Cedar policy test suite (`roles.rs`)

**Files:** Create `authz/roles.rs`; modify `authz/mod.rs`, `lib.rs`.

**Interfaces:**
- Produces: `starter_policies() -> Vec<PolicyDocument>` (the base `forbid-archived-writes` static policy + one `template` per role, all `system = true`); `system_roles() -> Vec<Role>` (7 roles with `template_id`, `scope_kinds`, `system=true`); `role(key: &str) -> Option<Role>`.

- [ ] **Step 1: Write the Cedar policy test suite** (the issue's CI deliverable) — a table-driven test that, for each `(principal, kind, action, resource, expected)` case, compiles `starter_policies()` + a set of representative linked `RoleGrant`s via `PolicyEngine::compile`, builds a slice, and asserts `decide().effect`. Cover: default-deny for an ungranted principal; `platform_admin`@Root allows `CreateOrganization`; `org_admin`@org allows `CreateProject` on a project under it and denies on another org; `org_member`@org allows `GetProject` but denies `RenameProject`; `forbid-archived-writes` denies a write on an archived project even with `org_admin`. Also a test that **every** `starter_policies()` entry passes `validate_policy`.
```rust
// sketch of one row
Case { principal: p1, kind: "user", grants: &[grant("org_admin", org_o)], action: Action::CreateProject, resource: project_under(org_o), expect: Effect::Allow },
```
- [ ] **Step 2:** Implement `starter_policies()` + `system_roles()`. Template source example (org_admin):
```
@id("tpl_org_admin")
permit(
  principal == ?principal,
  action in [Pgs::Iam::Action::"CreateTeam", Pgs::Iam::Action::"RenameTeam", /* …org-scoped set… */
             Pgs::Iam::Action::"GrantRole", Pgs::Iam::Action::"RevokeRole", Pgs::Iam::Action::"ListRoleGrants"],
  resource in ?resource
);
```
`platform_admin` template omits the `action in [...]` clause (all actions). `forbid-archived-writes`:
```
@id("forbid-archived-writes")
forbid(principal, action in [/* all write actions via Action::ALL.filter(is_write) */], resource)
when { resource has effective_status && resource.effective_status == "archived" };
```
Keep the action lists generated from `Action::ALL` where possible to avoid drift.
- [ ] **Step 3: Run** `cargo test -p paigasus-iam-core authz::roles`. Expected: PASS. This is the **Cedar policy test suite** — it must run in CI via `cargo nextest`/`moon run paigasus-iam-core-rs:test`.
- [ ] **Step 4: Commit** `feat(rs): add starter Cedar policies, roles + CI policy test suite (SMA-444)`.

### Task 7: `PrincipalContext.role_groups` retype (core)

**Files:** Modify `rs/crates/libs/paigasus-iam-core/src/authn.rs`.

- [ ] **Step 1:** Change `PrincipalContext.role_groups: Vec<Prn>` → `pub role_grants: Vec<RoleGrantRef>` (import from `authz::model`). Update the field's constructors/tests in `authn.rs`. Grep the workspace for `role_groups` and note every consumer (the gRPC converter in Task 20/18 handles the service side).
- [ ] **Step 2: Run** `cargo build -p paigasus-iam-core`. Expected: compiles (service won't yet — that's later tasks).
- [ ] **Step 3: Commit** `refactor(rs): PrincipalContext carries structured role grants (SMA-444)`.

---

## Phase B — Proto

### Task 8: `AuthorizationService` proto + codegen

**Files:** Modify `contracts/proto/paigasus/iam/v1/iam.proto`; regenerate `rs/crates/libs/paigasus-proto/src/generated/...` (+ py/ts) via buf.

- [ ] **Step 1:** Edit `iam.proto`: (a) in `IntrospectResponse`, replace `repeated string role_group_prns = 7;` with `reserved 7; reserved "role_group_prns"; repeated RoleGrantRef role_grants = 8;`; add `message RoleGrantRef { string scope_prn = 1; string role_key = 2; }`. (b) Add the `AuthorizationService` block + messages exactly as in spec §9.1 (IsAuthorized/PutPolicy/DeletePolicy/ListPolicies/GrantRole/RevokeRole/ListRoleGrants + Policy/RoleGrant messages). Update the file header comment (remove the "AuthorizationService … M4/M5" reservation note; it's landing now). **Do not** add `ServiceAccountService`.
- [ ] **Step 2: Run** `export PATH=... && moon run contracts:build` (or `buf generate` per the contracts task) to regenerate. Then `cargo build -p paigasus-proto`.
- [ ] **Step 3:** Commit the generated Rust (`paigasus-proto/src/generated/...`) + proto. `git add contracts/ rs/crates/libs/paigasus-proto/`.
- [ ] **Step 4: Run** `moon run :parity-corpus-drift` (codegen-drift) locally if available; expected: no drift (committed == generated).
- [ ] **Step 5: Commit** `feat(contracts): add IAM AuthorizationService + RoleGrantRef (SMA-444)`.

> **Implementer note:** confirm the buf codegen command from `contracts/` and that the generated Rust path matches `paigasus.iam.v1.rs`. Follow the SMA-389 proto-build wiring. Reserving field 7 + adding field 8 is additive — verify `moon run :breaking` stays green.

---

## Phase C — Persistence (`paigasus-iam` service)

### Task 9: Migration `m0004_create_authz` + entities

**Files:** Create `migration/m0004_create_authz.rs`; modify `migration/mod.rs`; create `persistence/entities/{policy,role,role_grant}.rs` + register in `entities/mod.rs`.

- [ ] **Step 1: Write the round-trip test** (extend `tests/roundtrip.rs` or a new `tests/authz_schema.rs`, testcontainers Postgres): run `Migrator::up`, then INSERT + SELECT a `policy`, `role`, `role_grant` row; assert the `scope_kind`/`scope_*_id` CHECK rejects a row with a mismatched combination.
- [ ] **Step 2: Implement `m0004`** mirroring `m0002_create_tenancy.rs` (DeriveIden enums; `create_table`; named UNIQUE/CHECK via `execute_unprepared`). Tables exactly per spec §6.1: `policy(policy_id TEXT PK, kind, source, description, system BOOL, created_at, updated_at)`; `role(key TEXT PK, template_id FK→policy, scope_kinds TEXT /* JSON */, description, system, created_at)`; `role_grant(id UUID PK, principal_id FK→principal CASCADE, role_key FK→role, scope_kind TEXT, scope_node_prn TEXT, scope_org_id/scope_team_id/scope_project_id UUID NULL FK→node CASCADE, linked_policy_id TEXT UNIQUE, created_at)` with `UNIQUE(principal_id, role_key, scope_node_prn)` and a `CHECK` matching `scope_kind` to the non-null FK (all NULL for `root`). Register `Box::new(m0004_create_authz::Migration)` in `migration/mod.rs`.
- [ ] **Step 3:** SeaORM `Entity` structs for the three tables (mirror `entities/membership.rs`).
- [ ] **Step 4: Run** `cargo nextest run -p paigasus-iam authz_schema --no-tests=pass` (needs Docker). Expected: PASS.
- [ ] **Step 5: Commit** `feat(rs): add authz Postgres schema (policy/role/role_grant) (SMA-444)`.

### Task 10: `PgPolicyStore` (+ generation abstraction)

**Files:** Create `adapters/authz/generation.rs`, `persistence/pg_policies.rs`; modify `persistence/mod.rs`, `adapters/authz/mod.rs`.

**Interfaces:**
- Produces: `Generations` — an enum/struct abstracting the two counters over `memory` (in-proc `Arc<AtomicU64>`) and `redis` (INCR/GET on `iam:authz:policy_gen` / `iam:authz:entity_gen`), with `policy_gen()/bump_policy_gen()/entity_gen()/bump_entity_gen()`. `PgPolicyStore::new(db, generations)`.

- [ ] **Step 1: Failing test** (testcontainers): seed a `system` policy; `put` on it → `Err(AuthzError::SystemImmutable)`; `put` a non-system valid policy → Ok + `policy_gen` increments; `put` an invalid policy source → `Err(SchemaValidation)`; `delete` a system policy → `Err(SystemImmutable)`.
- [ ] **Step 2:** Implement `Generations` (both backends) and `PgPolicyStore` (`list_all` in a repeatable-read txn; `put` calls `validate_policy` then upserts, rejecting `system=true` rows; `delete` rejects system; `policy_gen`/`bump_policy_gen` delegate to `Generations`). Map SeaORM `DbErr` → `AuthzError::Backend`.
- [ ] **Step 3: Run** the test (Docker). Expected: PASS.
- [ ] **Step 4: Commit** `feat(rs): add PgPolicyStore + generation counters (SMA-444)`.

### Task 11: `PgRoleGrantStore`

**Files:** Create `persistence/pg_role_grants.rs`; modify `persistence/mod.rs`.

- [ ] **Step 1: Failing test** (testcontainers): `grant` inserts a `role_grant` row for a seeded principal + org scope, bumps `policy_gen`; `list_by_principal` returns it; `revoke` deletes it + bumps; a duplicate `(principal, role, scope)` grant → `AuthzError::Backend`/conflict.
- [ ] **Step 2:** Implement, mirroring `pg_memberships.rs` transaction style. `grant` computes `linked_policy_id = format!("grant:{}", g.id)`, writes the row, and bumps `policy_gen` (the linked *policy* itself is materialized at snapshot-compile time from the grant rows — Task 12 — so the store only persists the grant + bumps the gen).
- [ ] **Step 3: Run** (Docker). Expected: PASS.
- [ ] **Step 4: Commit** `feat(rs): add PgRoleGrantStore (SMA-444)`.

### Task 12: `PgEntitySliceLoader` + tenancy `entity_gen` bumps

**Files:** Create `persistence/pg_entity_slice.rs`; modify `persistence/mod.rs` and the tenancy write adapters (`pg_organizations.rs`, `pg_teams.rs`, `pg_projects.rs`) to bump `entity_gen`.

**Interfaces:**
- Produces: `PgEntitySliceLoader::new(db, generations)`; `load` returns an `EntitySlice` containing the `Root` singleton (`ROOT_ENTITY`), the resource entity + its ancestor chain (each with `effective_status`), and the `Principal` entity (`kind`, `status`). `entity_gen()` delegates to `Generations`.

- [ ] **Step 1: Failing test** (testcontainers): create org→team→project; `load(project_prn, principal_prn)` returns a slice whose entities include `Root`, the project (parent = team), team (parent = org), org (parent = Root), and the principal; archiving the org and re-`load`ing reflects `effective_status = "archived"` on the subtree; a tenancy `set_status`/create bumps `entity_gen`.
- [ ] **Step 2:** Implement `load` (reuse M1 node reads for the node + `NodeView` effective status; construct `SliceEntity`s via `to_cedar_uid`; inject `ROOT_ENTITY` and the org→Root parent). Add `generations.bump_entity_gen()` calls to the tenancy `create`/`rename`/`set_status` adapters (or wrap them). Principal `kind` = `"user"`.
- [ ] **Step 3: Run** (Docker). Expected: PASS.
- [ ] **Step 4: Commit** `feat(rs): add entity-slice loader + entity_gen invalidation (SMA-444)`.

---

## Phase D — Caches + authorizer

### Task 13: Policy snapshot

**Files:** Create `adapters/authz/policy_snapshot.rs`; modify `adapters/authz/mod.rs`. Add `arc-swap` to `Cargo.toml` (or use `Arc<RwLock<Arc<CompiledPolicies>>>` to avoid a new dep — prefer the latter to keep the dep surface small).

**Interfaces:**
- Produces: `PolicySnapshot { current() -> Arc<CompiledPolicies>, reload_if_stale(&self) -> Result<(), AuthzError>, spawn_reload(self: Arc<Self>, ttl, poll, shutdown) -> JoinHandle }`. Built from `Arc<dyn PolicyStore>` + `Arc<dyn RoleGrantStore>` + `PolicyEngine`.

- [ ] **Step 1: Failing test** (unit, in-memory fakes for the stores): initial snapshot compiles base policies; after a fake grant + `bump_policy_gen`, `reload_if_stale` recompiles and the new `grant:<uuid>` linked policy is present; without a bump, `reload_if_stale` is a no-op.
- [ ] **Step 2:** Implement: hold `Arc<RwLock<Arc<CompiledPolicies>>>` + the store handles. `reload_if_stale` compares `store.policy_gen()` to the snapshot's gen; on advance (or forced), it loads `list_all` policies + grants in one pass, `PolicyEngine::compile`s, and swaps. `spawn_reload` loops on `tokio::select!(sleep(poll), shutdown)` calling `reload_if_stale`, and also honors the TTL as a max staleness.
- [ ] **Step 3: Run** `cargo test -p paigasus-iam authz::policy_snapshot`. Expected: PASS.
- [ ] **Step 4: Commit** `feat(rs): add authz policy snapshot with generation reload (SMA-444)`.

### Task 14: Decision + entity-slice caches

**Files:** Create `adapters/authz/decision_cache.rs`, `adapters/authz/entity_cache.rs`; modify `adapters/authz/mod.rs`.

**Interfaces:**
- Produces: `RedisDecisionCache`/`MemoryDecisionCache` (both `DecisionCache`); a `decision_key(policy_gen, entity_gen, req) -> String` = `format!("iam:authz:dec:{policy_gen}:{entity_gen}:{}", blake3::hash(...))`; `SliceCache` similarly keyed `iam:authz:slice:<entity_gen>:<resource-prn>` wrapping an `EntitySliceLoader`.

- [ ] **Step 1: Failing test:** `decision_key` changes when any of policy_gen/entity_gen/principal/action/resource/context changes; the memory cache get/put round-trips; a Redis error in the redis cache is swallowed (`get` → `None`, `put` → no panic) — fail-open (D12).
- [ ] **Step 2:** Implement. Redis via `ConnectionManager` (mirror `oidc/redis_cache.rs`), values `serde_json`. `blake3` for the hash. Both caches degrade to bypass on Redis error (log + `None`).
- [ ] **Step 3: Run** `cargo test -p paigasus-iam authz::decision_cache authz::entity_cache`. Expected: PASS.
- [ ] **Step 4: Commit** `feat(rs): add generation-keyed decision + slice caches (SMA-444)`.

### Task 15: `CedarAuthorizer` + `TracingAuditSink` + `AppState` wiring

**Files:** Create `adapters/authz/cedar_authorizer.rs`, `adapters/authz/audit.rs`; modify `adapters/http/mod.rs` (`AppState`), `main.rs`.

**Interfaces:**
- Produces: `CedarAuthorizer` (implements `Authorizer`): composes `PolicySnapshot` + `SliceCache` + `DecisionCache` + `PolicyEngine` + `Arc<dyn AuditSink>`. Held as `Arc<CedarAuthorizer>` in `AppState.authz` (shared across clones, like `WiredAuthenticator`). `TracingAuditSink` (implements `AuditSink`).

- [ ] **Step 1: Failing test** (unit, in-memory stores/caches): `is_authorized` returns `Deny` (default) for an ungranted principal and records an event to a capturing `AuditSink`; after a grant + snapshot reload, returns `Allow` naming `grant:<uuid>`; a cached decision short-circuits the engine (assert via a counting fake).
- [ ] **Step 2:** Implement `is_authorized`: build `decision_key` from current gens → cache `get` → on hit return; on miss, `snapshot.reload_if_stale()` (synchronous — AC1), load slice via `SliceCache`, `engine.decide`, `audit.record`, cache `put`, return. Add `authz: Arc<CedarAuthorizer>` to `AppState`; construct it in `AppState::new` from a wired `Generations` (chosen by `authz.cache.backend`) + the Pg stores. In `main.rs`, after `AppState::new`, `snapshot.clone().spawn_reload(ttl, poll, shutdown_rx)` and push the handle into the server `JoinSet`.
- [ ] **Step 3: Run** `cargo test -p paigasus-iam authz::cedar_authorizer`. Expected: PASS.
- [ ] **Step 4: Commit** `feat(rs): add CedarAuthorizer + audit sink + AppState wiring (SMA-444)`.

---

## Phase E — Application + errors

### Task 16: `Forbidden` error class

**Files:** Modify `application/error.rs`, `adapters/http/error.rs`, `adapters/grpc/convert.rs`.

- [ ] **Step 1: Failing test:** an application `Forbidden` maps to HTTP `403` with the shared error body shape, and to gRPC `Status::permission_denied`.
- [ ] **Step 2:** Add a `Forbidden` variant/`ErrorClass` to the app error, `403 FORBIDDEN` in the HTTP funnel, and `permission_denied` in `status_to_grpc`. Keep the body/`WWW-Authenticate` conventions consistent with the M2 funnel (403 has no challenge header).
- [ ] **Step 3: Run** `cargo test -p paigasus-iam error`. Expected: PASS.
- [ ] **Step 4: Commit** `feat(rs): add Forbidden (403 / PermissionDenied) error class (SMA-444)`.

### Task 17: Application use cases (authorize, roles, policies, bootstrap) + org-owner seed

**Files:** Create `application/{authorize,roles,policies,bootstrap}.rs`; modify `application/mod.rs`, `application/organizations.rs`, `application/authenticate_token.rs`.

**Interfaces:**
- Produces: `Authorize` (wraps `Arc<dyn Authorizer>` + the self/admin exposure rule); `RoleService` (`grant`/`revoke`/`list` — validates role, scope kind, principal; authorizes actor against the grant's scope node); `PolicyService` (`put`/`delete`/`list` — Root-authorized, rejects system); `bootstrap::reconcile_starter(store, roles)` (compare-and-warn) + `bootstrap::maybe_seed_admin(issuer, subject, principal, txn)`.

- [ ] **Step 1: Failing tests** (in-memory fakes): `RoleService::grant` rejects an unknown role (`UnknownRole`), a role at a disallowed scope kind (`InvalidScope`), and requires the actor to be authorized for `GrantRole` at the scope node; `Authorize` self-check allows a principal to query itself and redacts `determining_policies` for a non-self, non-admin third-party query; `reconcile_starter` upserts missing system policies and warns (does not overwrite silently) on a drifted one; `CreateOrganization` seeds an `org_admin` grant for the actor in the same op.
- [ ] **Step 2:** Implement. `CreateOrganization` (in `organizations.rs`): authorize `platform_admin`@Root, then one txn: create org + default team (existing) + `RoleGrantStore.grant(org_admin, actor, org)`, then `bump_policy_gen`. `authenticate_token.rs`: on JIT-provision, call `bootstrap::maybe_seed_admin`. Roles/policies services authorize via the injected `Authorize`.
- [ ] **Step 3: Run** `cargo test -p paigasus-iam application::{roles,policies,authorize,bootstrap}`. Expected: PASS.
- [ ] **Step 4: Commit** `feat(rs): add authz application use cases + seeded org owner + bootstrap (SMA-444)`.

---

## Phase F — Wire surfaces + retrofit

### Task 18: HTTP authz routes

**Files:** Create `adapters/http/authz.rs`; modify `adapters/http/{mod,dto}.rs`.

- [ ] **Step 1: Failing test** (`tests/authz_policies.rs` / `tests/authz_decision.rs`, oneshot or testcontainers): `POST /v1/authz/is-authorized` self-query returns `{allowed, determining_policies, reason}`; `POST /v1/authz/policies` as a non-admin → 403; as platform_admin → 200; `POST /v1/authz/role-grants` grants and `DELETE` revokes.
- [ ] **Step 2:** Implement handlers + DTOs (mirror `http/memberships.rs` + `http/authn.rs`). Mount under the protected `/v1` sub-router. Enforce the self/admin exposure rule for `is-authorized`; redact `determining_policies` for non-self/non-admin.
- [ ] **Step 3: Run** (Docker where needed). Expected: PASS.
- [ ] **Step 4: Commit** `feat(rs): add HTTP authz surface (is-authorized, policies, role-grants) (SMA-444)`.

### Task 19: gRPC `AuthorizationService`

**Files:** Create `adapters/grpc/authz.rs`; modify `adapters/grpc/{mod,convert}.rs`.

- [ ] **Step 1: Failing test** (`tests/grpc_authz.rs`, mirror `grpc_tenancy.rs`): the seven RPCs round-trip; `IsAuthorized` self-query works; management RPCs enforce authz; the introspect converter emits `role_grants` (`RoleGrantRef`).
- [ ] **Step 2:** Implement `AuthzGrpc` (mirror `grpc/tenancy.rs` + `grpc/authn.rs`); add `AuthorizationServiceServer::new(...)` to `grpc::router`; update `convert.rs` for `RoleGrantRef` and the `role_grants` field (fixes the Task 7 retype on the service side).
- [ ] **Step 3: Run** (Docker). Expected: PASS.
- [ ] **Step 4: Commit** `feat(rs): add gRPC AuthorizationService (SMA-444)`.

### Task 20: `AuthzLayer` + tenancy retrofit + test-suite migration

**Files:** Create `adapters/http/authz_middleware.rs`; modify each tenancy HTTP handler (`http/{organizations,teams,projects,memberships}.rs`), each gRPC tenancy method (`grpc/tenancy.rs`), `tests/support/mod.rs`, and the existing tenancy tests.

- [ ] **Step 1: Failing test** (`tests/authz_retrofit.rs`): a tenancy write (e.g. `POST /v1/organizations/.../teams`) returns **403** for an authenticated-but-ungranted principal and **200** after `seed_org_admin`; a write on an archived node is denied even with a grant (`forbid-archived-writes`).
- [ ] **Step 2:** Implement the reusable coarse `AuthzLayer` (unit-tested) + add a per-handler `authorize(ctx.principal, <action>, <resource-prn>, RequestContext::empty())` call at the top of each tenancy handler (HTTP + gRPC), returning `Forbidden` on deny, per the §9.4 action→resource table. Gate on `authz.enforce_tenancy`. Add `support::seed_platform_admin(&principal)` / `seed_org_admin(&principal, &org)` helpers.
- [ ] **Step 3: Migrate existing tenancy tests:** for every M1/M2 test that drives an enforced route, (a) ensure the acting principal is provisioned (first authenticated call), (b) seed the needed grant before the action. Update `tests/{http_tenancy,tenancy_*,grpc_tenancy,http_memberships,tenancy_memberships}.rs` accordingly.
- [ ] **Step 4: Run** `cargo nextest run -p paigasus-iam --no-tests=pass` (Docker). Expected: PASS (all migrated).
- [ ] **Step 5: Commit** `feat(rs): enforce Cedar authz on tenancy routes + AuthzLayer (SMA-444)`.

---

## Phase G — Config, integration, CI

### Task 21: `[authz]` config + validation

**Files:** Modify `src/config.rs`, `src/main.rs`.

- [ ] **Step 1: Failing tests** (figment `Jail`, mirror the existing config tests): defaults land; `cache.backend = "redis"` without `redis_url` fails `validate`; a TTL of `0` fails; a `bootstrap_admins` entry with a non-https issuer or empty subject fails; empty `bootstrap_admins` is Ok (but `main` logs a warning — assert via a unit on the warn path or just that validate passes).
- [ ] **Step 2:** Add `AuthzConfig` (mirror `AuthnConfig`/`JwksCacheConfig`) with `enforce_tenancy`, TTLs, `refresh_interval_secs`, `cache { backend, redis_url }`, `bootstrap_admins: Vec<BootstrapAdmin { issuer, subject }>`. Extend `IamConfig` + `Defaults` + `validate`. In `main`, warn if `bootstrap_admins` is empty.
- [ ] **Step 3: Run** `cargo test -p paigasus-iam config`. Expected: PASS.
- [ ] **Step 4: Commit** `feat(rs): add [authz] config + validation (SMA-444)`.

### Task 22: AC integration tests + Redis cross-replica

**Files:** Create `tests/authz_decision.rs` (ACs), `tests/authz_redis.rs`; modify `tests/support/mod.rs`.

- [ ] **Step 1 (AC1):** provision P; `is_authorized(P, CreateProject, project-under-O)` → deny; `GrantRole(org_admin, P, O)`; same call → **allow** (same replica, immediate). Assert over both HTTP and gRPC.
- [ ] **Step 2 (AC2):** a denied decision names its determining policy (or `DEFAULT_DENY_MARKER`); an allowed decision names `grant:<uuid>`; a non-self caller receives it redacted.
- [ ] **Step 3 (AC3):** with `policy_cache_ttl_secs = 1`, `PutPolicy` a change; assert the new effect is observed within the TTL bound (poll ≤ ttl+slack) and immediately with the gen counter.
- [ ] **Step 4 (Redis):** two `CedarAuthorizer`s sharing one Redis (testcontainer) — a grant via one is visible to the other within `refresh_interval_secs`; kill Redis mid-test and assert decisions still evaluate (fail-open-to-Postgres, still default-deny where unGranted).
- [ ] **Step 5: Run** `cargo nextest run -p paigasus-iam authz_ --no-tests=pass` (Docker). Expected: PASS.
- [ ] **Step 6: Commit** `test(rs): AC1-AC3 + Redis cross-replica authz integration tests (SMA-444)`.

### Task 23: `deny.toml` / CI wiring + full gate run

**Files:** Modify `rs/deny.toml` (if needed), `rs/crates/services/paigasus-iam/Cargo.toml` (machete allowlist if a dep is consumed only later).

- [ ] **Step 1: Run the full graph:** `export PATH=... && cd rs && cargo build --workspace && cargo clippy --workspace -- -D warnings && cargo fmt --check`, then from the repo root `moon ci :build :test :lint :fmt :deny :machete :typecheck :breaking :affected-smoke :parity-corpus-drift :wasm-getrandom-free --base origin/main --include-relations`.
- [ ] **Step 2:** Resolve `:deny` findings: `cedar-policy`'s transitive tree may introduce a license not on the allow-list → add a scoped `[licenses] exceptions` entry (name + license), or a new advisory → a justified dev-only `[advisories] ignore`. Resolve `:machete` (unused-dep) if `blake3`/`arc-swap` land a task before their first use (temporary `[package.metadata.cargo-machete] ignored`, pruned once consumed). Confirm `:wasm-getrandom-free` and `:affected-smoke` stay green (no new crate, so the strict-equality set is unchanged).
- [ ] **Step 3: Commit** `build(rs): deny/machete waivers for cedar-policy tree (SMA-444)` (only if changes were needed).

---

## Self-Review

**Spec coverage:** §3 schema/model/engine → Tasks 1–6; §4 ports/errors → Tasks 3,5,16; §6 migration/adapters → Tasks 9–12; §7 caches → Tasks 13–15; §8 application → Task 17; §9 proto/HTTP/gRPC/retrofit → Tasks 8,18,19,20; §10 audit → Task 15; §11 config → Task 21; §12 tests → Tasks 6,22 (Cedar policy suite = Task 6); §13 CI → Task 23; AC1/2/3 → Task 22; the three ACs each have a dedicated integration step.

**Gaps folded in:** `PrincipalContext` retype (Task 7) has both a core step (7) and a service-side converter fix (Task 19 Step 2) — flagged so the service build breaks only transiently between them. The Cedar policy **test suite in CI** is Task 6 Step 3 (pure, runs under the normal test gate).

**Type consistency:** `Generations`, `PolicySnapshot`, `CompiledPolicies`, `decision_key`, `ROOT_ENTITY`, `DEFAULT_DENY_MARKER`, `RoleGrantRef`, `AuthzError` variants, and the port signatures are named identically across the tasks that produce and consume them. `Action::as_wire` strings equal the schema action names (Task 1/2) and the wire `action` field (Task 18/19).

**Implementer caution (repeated where relevant):** the exact `cedar-policy` 4.11 API surface (`Schema`, `Validator`, `PolicySet::link`, `Request::new`, `Entities`, `Response::diagnostics().reason()`) must be confirmed against the pinned crate's rustdoc during Tasks 1/4/13 — the plan's shapes match the 4.x public API but small signature deltas are possible.
