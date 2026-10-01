# SMA-646 Service-account owner and API-key scope: stored-PRN check — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `CreateServiceAccount`, `ListServiceAccounts` and `IssueApiKey` load the named tenancy node,
authorize against its STORED PRN, and refuse a caller PRN that differs from the stored one with
`TenancyError::PrnMismatch`, so no answer or event echoes a forged node PRN.

**Architecture:** One new shared helper, `TenancyNodes::resolve_and_authorize`, in
`application/tenancy_nodes.rs`. It holds the three tenancy repositories and does load → authorize
against the stored PRN → compare canonical strings → warn on a mismatch → return the stored node.
`ServiceAccountService` (`create`, `list`) and `ApiKeyService` (`issue`, new step 4 after D15) call
it. Both transports use these services, so no transport changes.

**Tech Stack:** Rust 2024 (rust-version 1.95), tokio, `tracing`, tonic (gRPC), axum (HTTP),
SeaORM + Postgres (testcontainers), cargo-nextest, Moon.

**Spec:** `docs/superpowers/specs/2026-09-28-sma-646-service-account-stored-owner-design.md`
(APPROVED 2026-09-28, decisions A1–A6 in its §0). Read the spec before each task. The spec is the
authority for every behaviour row (B1–B7, K1–K6) and acceptance criterion (AC 1–16).

**Worktree:** `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-646-service-account-stored-owner`,
branch `feature/sma-646-service-account-stored-owner`. Start every shell command with
`cd <worktree> &&` and `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`. Below, `IAM`
means `rs/crates/services/paigasus-iam`.

## Global Constraints

- Every new source file opens with `// SPDX-License-Identifier: Apache-2.0`.
- Edition 2024, rust-version 1.95. `warnings = "deny"`: an unused binding or item fails the build.
- No port change: `ServiceAccountRepository`, `ApiKeyRepository` and every `paigasus-iam-core` trait stay as they are.
- No transport change. `Page::new` stays in the transports, before the service call (spec §2.2, B7). Do not move it.
- `RoleService` stays unchanged (spec §2.1 D).
- `owner_resource_prn`, `node_resource_prn` and `grant_scope_resource_prn` keep their callers. Do not delete them.
- Do not write a registry reason code (for example the kebab-case code of `TenancyError::PrnMismatch`) as a quoted string literal anywhere in `application/tenancy_nodes.rs`, `application/service_accounts.rs` or `application/api_keys.rs`, test modules and comments included (`repo:error-code-single-site`). Assert on `TenancyError::PrnMismatch`. Integration tests under `IAM/tests/` may use the string codes, as `tests/grpc_tenancy.rs` does.
- The mismatch warning has exactly the fields of `warn_prn_mismatch` (`adapters/grpc/tenancy.rs:223-225`): `rpc`, `actor`, `requested_prn`, `stored_prn`.
- Conventional commits with scope `rs`, for example `fix(rs): … (SMA-646)`. End every commit message with the line `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Do not put a `#NNN` line or a `token: value` line in the commit body (commitlint `footer-leading-blank`).
- Never use `--no-verify`, `--no-gpg-sign`, `git commit --amend`, `git reset` or a force push. Do not use `git checkout -- <file>` to undo a mutation.
- Before each commit, `git -C <worktree> branch --show-current` must print `feature/sma-646-service-account-stored-owner`.
- A filtered Docker test run needs `PAIGASUS_REQUIRE_DOCKER=1`. Without it, an unreachable daemon makes the Docker tests pass silently (`rs/CLAUDE.md`).
- Do not install host software. Do not leave background jobs running.

## Review Focus

These five inputs are not named by a spec test, but a real caller can send them. Each line names
the expected behaviour and the task that pins it.

1. **An archived owner or scope node.** The helper must not add a status gate: an archived node resolves like an active one, and the service keeps today's behaviour for it. Pinned by `an_archived_node_still_resolves` (Task 1).
2. **A correct PRN with upper-case uuids over HTTP.** `POST /v1/service-accounts` must answer 201 and echo the stored lower-case PRN. The spec pins this only over gRPC. Pinned in `http_forged_owner_prn_is_refused` (Task 5).
3. **An org-level grant and a team scope in the same organization.** A caller with `org_admin` at organization A must still issue a key scoped to a team inside A (Cedar `resource in ?resource`). K4 must not block it. Pinned by the positive control of `a_forged_scope_prn_never_issues_an_api_key` (Task 4).
4. **A forged scope from a caller who fails the owner check or D15.** The answer must stay `forbidden`, never `prn-mismatch` (AC 15, K5). Pinned by `a_forged_scope_never_outranks_the_owner_or_d15_checks` (Task 3).
5. **The project arm.** A forged org slot on a PROJECT owner or scope must answer `PrnMismatch`, as the team arm does. Pinned by the `project, wrong org slot` row of `forged_variants` (Tasks 2 and 3).

---

## File map

| file | responsibility | task |
|---|---|---|
| `IAM/src/application/tenancy_nodes.rs` (new) | `TenancyNodes` and `resolve_and_authorize`, the one place of the order rule | 1 |
| `IAM/src/application/mod.rs` | register `tenancy_nodes` | 1 |
| `IAM/src/application/fakes.rs` | seeding helpers: `seed_org`, `seed_team`, `seed_project`, `store_with_orgs`, `tenancy_nodes`, `forged_variants` | 1 |
| `IAM/src/application/service_accounts.rs` | `nodes` dep, `create`/`list` call the helper, docs, U1–U8 | 2 |
| `IAM/src/application/api_keys.rs` | `nodes` dep, `issue` step 4, docs, U9–U14 | 3 |
| `IAM/src/adapters/http/mod.rs` | rename the three repo handles, build one `TenancyNodes`, wire both services | 2, 3 |
| `IAM/tests/support/mod.rs` | `seed_team_ref`, `prn_with_org`, `prn_with_region`, `prn_upper_uuids` | 4 |
| `IAM/tests/api_keys_grpc.rs` | I1, I3 | 4 |
| `IAM/tests/http_service_accounts.rs` | I2, I4 | 5 |

---

### Task 1: The shared `TenancyNodes` helper and the fake-store seeding helpers

**Files:**
- Create: `IAM/src/application/tenancy_nodes.rs`
- Modify: `IAM/src/application/mod.rs` (add one line)
- Modify: `IAM/src/application/fakes.rs` (add helpers after the `impl ProjectRepository for InMemoryProjects` block, before `fn node_lookup`, about line 497)

**Interfaces:**
- Consumes: `Authorize::check(&self, actor: &Prn, action: Action, resource: &Prn) -> Result<(), TenancyError>`; `OrganizationRepository::find(Uuid)`, `TeamRepository::find(Uuid)`, `ProjectRepository::find(Uuid)`, each `-> Result<Option<NodeView<_>>, RepositoryError>`.
- Produces:
  - `pub struct TenancyNodes { pub orgs: Arc<dyn OrganizationRepository>, pub teams: Arc<dyn TeamRepository>, pub projects: Arc<dyn ProjectRepository> }` (`#[derive(Clone)]`)
  - `pub(crate) async fn resolve_and_authorize(&self, authorize: &Authorize, actor: &Prn, action: Action, claimed: &TenancyNodeRef, operation: &'static str) -> Result<TenancyNodeRef, TenancyError>`
  - In `fakes.rs`: `seed_org(&TenancyStore, u128) -> OrganizationId`, `seed_team(&TenancyStore, &OrganizationId, u128) -> TeamId`, `seed_project(&TenancyStore, &TeamId, u128) -> ProjectId`, `store_with_orgs(impl IntoIterator<Item = u128>) -> TenancyStore`, `tenancy_nodes(&TenancyStore) -> TenancyNodes`, `FORGED_REGION: &str`, `forged_variants(&OrganizationId, &TeamId, &ProjectId) -> Vec<(&'static str, TenancyNodeRef)>`.

- [ ] **Step 1: Add the seeding helpers to `fakes.rs`**

Add this import near the top of `fakes.rs`, after the `use paigasus_kernel::Prn;` line:

```rust
use crate::application::tenancy_nodes::TenancyNodes;
```

Insert this block before `fn node_lookup` (about line 497). `Organization`, `OrganizationId`,
`Project`, `ProjectId`, `Team`, `TeamId`, `Slug`, `TenancyNodeRef`, `DateTime`, `Utc`, `Uuid` and
`Arc` are already imported.

```rust
// ---- SMA-646: stored tenancy nodes for the stored-PRN tests ------------------------------

/// SMA-646: inserts an ACTIVE organization with uuid `n` into `store` and returns its id. The
/// stored canonical PRN is `OrganizationId::from_uuid(n).canonical()`, which is the F1
/// invariant of the real `prn` column (spec §2.4).
pub fn seed_org(store: &TenancyStore, n: u128) -> OrganizationId {
    let id = OrganizationId::from_uuid(Uuid::from_u128(n));
    let stamp = test_stamp(DateTime::<Utc>::UNIX_EPOCH, 1);
    let org = Organization::new(id.clone(), Slug::parse(&format!("org-{n}")).unwrap(), "Org", &stamp).unwrap();
    store.orgs.lock().unwrap().insert(id.uuid(), org);
    id
}

/// SMA-646: inserts an ACTIVE team with uuid `n` under `org` and returns its id.
pub fn seed_team(store: &TenancyStore, org: &OrganizationId, n: u128) -> TeamId {
    let id = TeamId::from_parts(org.uuid(), Uuid::from_u128(n));
    let stamp = test_stamp(DateTime::<Utc>::UNIX_EPOCH, 1);
    let team = Team::new(id.clone(), Slug::parse(&format!("team-{n}")).unwrap(), "Team", &stamp).unwrap();
    store.teams.lock().unwrap().insert(id.uuid(), team);
    id
}

/// SMA-646: inserts an ACTIVE project with uuid `n` under `team` and returns its id.
pub fn seed_project(store: &TenancyStore, team: &TeamId, n: u128) -> ProjectId {
    let id = ProjectId::from_parts(team.org_uuid(), Uuid::from_u128(n));
    let stamp = test_stamp(DateTime::<Utc>::UNIX_EPOCH, 1);
    let project = Project::new(id.clone(), team.clone(), Slug::parse(&format!("project-{n}")).unwrap(), "Project", &stamp).unwrap();
    store.projects.lock().unwrap().insert(id.uuid(), project);
    id
}

/// SMA-646: a fresh store that holds one organization per uuid in `ns`.
pub fn store_with_orgs(ns: impl IntoIterator<Item = u128>) -> TenancyStore {
    let store = TenancyStore::default();
    for n in ns {
        seed_org(&store, n);
    }
    store
}

/// SMA-646: the `TenancyNodes` value the services take, over the three in-memory fakes of
/// ONE shared store.
pub fn tenancy_nodes(store: &TenancyStore) -> TenancyNodes {
    TenancyNodes {
        orgs: Arc::new(InMemoryOrgs(store.clone())),
        teams: Arc::new(InMemoryTeams(store.clone())),
        projects: Arc::new(InMemoryProjects(store.clone())),
    }
}

/// SMA-646: a syntactically valid region. `Prn::parse` accepts it; the stored PRN has none.
pub const FORGED_REGION: &str = "eu-west-1";

/// SMA-646: the four forged shapes of spec §4.1 U1. Each names a REAL node's uuid with a slot
/// that the stored PRN does not have. The uuid case is not forgeable (`Prn` stores uuids).
pub fn forged_variants(org: &OrganizationId, team: &TeamId, project: &ProjectId) -> Vec<(&'static str, TenancyNodeRef)> {
    let wrong_org = Uuid::from_u128(0xF0F0);
    vec![
        ("team, wrong org slot", TenancyNodeRef::Team(TeamId::from_parts(wrong_org, team.uuid()))),
        ("project, wrong org slot", TenancyNodeRef::Project(ProjectId::from_parts(wrong_org, project.uuid()))),
        (
            "organization, forged region",
            TenancyNodeRef::Organization(OrganizationId::from_prn(Prn::build("iam", FORGED_REGION, None, "organization", org.uuid()).unwrap()).unwrap()),
        ),
        (
            "team, forged region",
            TenancyNodeRef::Team(TeamId::from_prn(Prn::build("iam", FORGED_REGION, Some(team.org_uuid()), "team", team.uuid()).unwrap()).unwrap()),
        ),
    ]
}
```

- [ ] **Step 2: Register the module**

In `IAM/src/application/mod.rs`, add `pub mod tenancy_nodes;` after `pub mod teams;` (keep the list sorted).

- [ ] **Step 3: Write the helper module with its failing tests first**

Create `IAM/src/application/tenancy_nodes.rs` with the struct, the red-step method body below,
and the test module. Then run the tests (Step 4). Then replace the red-step body with the body
of Step 5. The red-step body names every parameter and `node_prn`, so `warnings = "deny"` does
not stop the build with `unused_variables` or `dead_code`:

```rust
        // SMA-646 red step: replaced in Step 5.
        let _ = (&self.orgs, &self.teams, &self.projects, authorize, actor, action, claimed, operation, node_prn);
        Err(TenancyError::Internal)
```

```rust
// SPDX-License-Identifier: Apache-2.0

//! `TenancyNodes`: the stored-PRN check for a caller-supplied tenancy node (SMA-646).
//!
//! `ServiceAccountService::create`/`list` (the owner) and `ApiKeyService::issue` (the key's
//! scope) take a `TenancyNodeRef` that the transport parsed from the caller's PRN.
//! `TenancyNodeRef::from_prn` checks the shape only, not the org-slot VALUE and not the region.
//! So a caller can name a real node's uuid with a forged org slot or region. Before SMA-646,
//! these services echoed that forged PRN in their answers and in the `iam.api_key.issued`
//! payload, while a later read showed the stored PRN.
//!
//! [`TenancyNodes::resolve_and_authorize`] is the one place of the rule, in this order:
//!
//! 1. Load the node by uuid (`find`). An unknown uuid is `TenancyError::NotFound`.
//! 2. Authorize `action` against the STORED PRN, not the caller's. So the decision audit, the
//!    trace and the decision-cache key never carry a forged PRN.
//! 3. Compare the caller's canonical PRN with the stored canonical PRN. A difference writes one
//!    `tracing::warn!` and answers `TenancyError::PrnMismatch`.
//! 4. Return the STORED node, so the write and the answer use what the load read.
//!
//! **Authorize before compare is load-bearing** (SMA-645 §3): if the compare ran first, the
//! difference between a mismatch and `Forbidden` would tell an ungranted caller which
//! organization owns a node. **Load before authorize** costs one existence oracle (`NotFound`
//! against `Forbidden` for an ungranted caller). The tenancy surfaces accept the same cost
//! (`adapters/grpc/tenancy.rs`, `load_org_checked`), and Sven accepted it here (spec A4).
//!
//! **The compare is sound outside the write transaction** because of the F1 invariant (SMA-645
//! §3.1): a node's `prn` column is written once, from `canonical()`, with an empty region, and no
//! code moves a node to another parent. A future "move a node" feature breaks this and must
//! revisit this helper.
//!
//! **The warning line.** A refused request writes no row, no outbox event and no denial row, so
//! the log line is the only trace of a probe with a forged PRN (SMA-643 D4). Its fields are the
//! fields of `warn_prn_mismatch` in `adapters/grpc/tenancy.rs`. SMA-649 chose NO log line for its
//! PRINCIPAL guards, to match the node guard of the same RPC. Here the guarded value IS a tenancy
//! node, so the node-RPC rule applies.
//!
//! `RoleService::resolve_scope` is a similar lookup with the other order (authorize the caller's
//! PRN, then compare) and no log line. It stays unchanged (spec §2.1 D, §7 follow-up).

use crate::application::authorize::Authorize;
use crate::application::error::TenancyError;
use paigasus_iam_core::{Action, OrganizationRepository, ProjectRepository, TeamRepository, TenancyNodeRef};
use paigasus_kernel::Prn;
use std::sync::Arc;

/// Read access to the three tenancy repositories, for the stored-PRN check. Only `find` is
/// used. No narrower read-only port exists, and `RoleService` takes the same three.
#[derive(Clone)]
pub struct TenancyNodes {
    pub orgs: Arc<dyn OrganizationRepository>,
    pub teams: Arc<dyn TeamRepository>,
    pub projects: Arc<dyn ProjectRepository>,
}

/// The PRN a tenancy node represents as an authorization resource.
fn node_prn(node: &TenancyNodeRef) -> &Prn {
    match node {
        TenancyNodeRef::Organization(id) => id.prn(),
        TenancyNodeRef::Team(id) => id.prn(),
        TenancyNodeRef::Project(id) => id.prn(),
    }
}

impl TenancyNodes {
    /// Loads `claimed`'s node, authorizes `action` for `actor` against the STORED PRN, refuses a
    /// caller PRN whose canonical form differs from the stored one, and returns the STORED node.
    /// `operation` is the RPC name for the warning line. Module docs give the order and why.
    pub(crate) async fn resolve_and_authorize(&self, authorize: &Authorize, actor: &Prn, action: Action, claimed: &TenancyNodeRef, operation: &'static str) -> Result<TenancyNodeRef, TenancyError> {
        // SMA-646 red step: replaced in Step 5.
        let _ = (&self.orgs, &self.teams, &self.projects, authorize, actor, action, claimed, operation, node_prn);
        Err(TenancyError::Internal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::fakes::{FakeAuthorizer, TenancyStore, seed_org, seed_team, tenancy_nodes};
    use paigasus_iam_core::{NodeStatus, TeamId};
    use uuid::Uuid;

    fn actor() -> Prn {
        Prn::build("iam", "", None, "principal", Uuid::from_u128(1)).unwrap()
    }

    /// The correct PRN resolves to the STORED node, authorized against the stored PRN.
    #[tokio::test]
    async fn a_correct_prn_returns_the_stored_node() {
        let store = TenancyStore::default();
        let org = seed_org(&store, 10);
        let team = seed_team(&store, &org, 11);
        let fake = FakeAuthorizer::default();
        fake.allow(Action::CreateServiceAccount, team.prn());
        let authorize = Authorize::new(Arc::new(fake));

        let got = tenancy_nodes(&store)
            .resolve_and_authorize(&authorize, &actor(), Action::CreateServiceAccount, &TenancyNodeRef::Team(team.clone()), "CreateServiceAccount")
            .await
            .unwrap();
        assert_eq!(got, TenancyNodeRef::Team(team));
    }

    /// Review Focus 1: the helper adds no status gate. An archived node resolves like an active
    /// one; the service keeps its own behaviour for it.
    #[tokio::test]
    async fn an_archived_node_still_resolves() {
        let store = TenancyStore::default();
        let org = seed_org(&store, 20);
        store.orgs.lock().unwrap().get_mut(&org.uuid()).unwrap().status = NodeStatus::Archived;
        let fake = FakeAuthorizer::default();
        fake.allow(Action::ListServiceAccounts, org.prn());
        let authorize = Authorize::new(Arc::new(fake));

        let got = tenancy_nodes(&store)
            .resolve_and_authorize(&authorize, &actor(), Action::ListServiceAccounts, &TenancyNodeRef::Organization(org.clone()), "ListServiceAccounts")
            .await
            .unwrap();
        assert_eq!(got, TenancyNodeRef::Organization(org));
    }

    /// An unknown uuid is `NotFound` before any authorization (a default-deny authorizer).
    #[tokio::test]
    async fn an_unknown_node_is_not_found() {
        let store = TenancyStore::default();
        let authorize = Authorize::new(Arc::new(FakeAuthorizer::default()));
        let unknown = TenancyNodeRef::Team(TeamId::from_parts(Uuid::from_u128(30), Uuid::from_u128(31)));

        let err = tenancy_nodes(&store).resolve_and_authorize(&authorize, &actor(), Action::CreateServiceAccount, &unknown, "CreateServiceAccount").await.unwrap_err();
        assert_eq!(err, TenancyError::NotFound);
    }
}
```

- [ ] **Step 4: Run the tests to see them fail**

Run: `cd <worktree>/rs && cargo nextest run -p paigasus-iam --lib -E 'test(/tenancy_nodes::tests/)'`
Expected: 3 FAIL, each an assertion on `Err(Internal)`. A compile error is not a red test: fix
the build first.

- [ ] **Step 5: Write the helper body**

Replace the three red-step lines (the comment, the `let _ = …;` line and `Err(TenancyError::Internal)`) with:

```rust
        let stored = match claimed {
            TenancyNodeRef::Organization(id) => TenancyNodeRef::Organization(self.orgs.find(id.uuid()).await?.ok_or(TenancyError::NotFound)?.node.id),
            TenancyNodeRef::Team(id) => TenancyNodeRef::Team(self.teams.find(id.uuid()).await?.ok_or(TenancyError::NotFound)?.node.id),
            TenancyNodeRef::Project(id) => TenancyNodeRef::Project(self.projects.find(id.uuid()).await?.ok_or(TenancyError::NotFound)?.node.id),
        };
        authorize.check(actor, action, node_prn(&stored)).await?;
        let requested = claimed.canonical();
        let stored_canonical = stored.canonical();
        if requested != stored_canonical {
            tracing::warn!(rpc = %operation, actor = %actor.canonical(), requested_prn = %requested, stored_prn = %stored_canonical, "refused a request: the request prn does not match the stored tenancy node");
            return Err(TenancyError::PrnMismatch);
        }
        Ok(stored)
```

- [ ] **Step 6: Run the tests to see them pass, then lint**

Run: `cd <worktree>/rs && cargo nextest run -p paigasus-iam --lib -E 'test(/tenancy_nodes::tests/)'`
Expected: 3 PASS.

Run: `cd <worktree>/rs && cargo fmt -p paigasus-iam && cargo clippy -p paigasus-iam --all-targets --locked -- -D warnings`
Expected: no warning. `TenancyNodes` is `pub` in a `pub mod`, so it is not dead code. If clippy
reports `resolve_and_authorize` as unused, it is because no service calls it yet: in that case
add `#[cfg_attr(not(test), allow(dead_code))]` on the method now and remove it in Task 2 Step 6.

- [ ] **Step 7: Commit**

```bash
cd <worktree> && git branch --show-current   # must print feature/sma-646-service-account-stored-owner
git add rs/crates/services/paigasus-iam/src/application/tenancy_nodes.rs rs/crates/services/paigasus-iam/src/application/mod.rs rs/crates/services/paigasus-iam/src/application/fakes.rs
git commit -m "fix(rs): add the TenancyNodes stored-PRN helper (SMA-646)

Load the node, authorize against the stored PRN, compare, warn on a
mismatch, and return the stored node. Add fake-store seeding helpers.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: `ServiceAccountService::create` and `list` use the helper

**Files:**
- Modify: `IAM/src/application/service_accounts.rs` (struct, Deps, `new`, `create`, `list`, docs, tests)
- Modify: `IAM/src/adapters/http/mod.rs:519-545` (the three handles) and `:702-711` (the `ServiceAccountServiceDeps` literal)

**Interfaces:**
- Consumes: `TenancyNodes`, `resolve_and_authorize` (Task 1); `fakes::{seed_org, seed_team, seed_project, store_with_orgs, tenancy_nodes, forged_variants, TenancyStore}`.
- Produces: `ServiceAccountServiceDeps { …, pub nodes: TenancyNodes, … }`. A local `tenancy_nodes: TenancyNodes` value in `AppState::new` (`adapters/http/mod.rs`) that Task 3 clones.

- [ ] **Step 1: Add the `nodes` dependency (no behaviour change yet)**

In `service_accounts.rs`:
- Add `use crate::application::tenancy_nodes::TenancyNodes;` after the `use crate::application::pagination::Page;` line.
- Add the field `nodes: TenancyNodes,` to `ServiceAccountService` after `authorize: Authorize,`.
- Add `pub nodes: TenancyNodes,` to `ServiceAccountServiceDeps` after `pub authorize: Authorize,`, with this doc line above it: `/// SMA-646: loads the owner node for the stored-PRN check of `create`/`list`.`
- Add `nodes: deps.nodes,` to `new` after `authorize: deps.authorize,`.

In `adapters/http/mod.rs`:
- Add `use crate::application::tenancy_nodes::TenancyNodes;` next to the other `crate::application::…` imports.
- Replace the comment at lines 519-523 and the three `let role_orgs/role_teams/role_projects` lines (524-526) with:

```rust
        // Read access to the tenancy repos, independent of `orgs`/`teams`/`projects` above
        // (those are wrapped in `OrganizationService`/etc., not exposed as bare repos) — cheap
        // fresh instances, `DatabaseConnection` clones an `Arc`-backed pool handle. Three users:
        // `RoleService::resolve_scope` (SMA-444 FIX 2), and, through ONE shared `TenancyNodes`,
        // the stored-PRN check of `ServiceAccountService::create`/`list` and
        // `ApiKeyService::issue` (SMA-646).
        let tenancy_orgs: Arc<dyn OrganizationRepository> = Arc::new(PgOrganizationRepository::new(db.clone(), gens.clone()));
        let tenancy_teams: Arc<dyn TeamRepository> = Arc::new(PgTeamRepository::new(db.clone(), gens.clone()));
        let tenancy_projects: Arc<dyn ProjectRepository> = Arc::new(PgProjectRepository::new(db.clone(), gens.clone()));
        let tenancy_nodes = TenancyNodes {
            orgs: tenancy_orgs.clone(),
            teams: tenancy_teams.clone(),
            projects: tenancy_projects.clone(),
        };
```

- In the `RoleServiceDeps` literal (lines 542-544) write `orgs: tenancy_orgs,`, `teams: tenancy_teams,`, `projects: tenancy_projects,`.
- In the `ServiceAccountServiceDeps` literal (line 702) add `nodes: tenancy_nodes.clone(),` after `authorize: authorize.clone(),`. (Task 3 adds the second user of `tenancy_nodes`.)
- Run `grep -rn "role_orgs\|role_teams\|role_projects" rs/crates/services/paigasus-iam/src` from the worktree. Expected: no match.

- [ ] **Step 2: Update the unit-test harness so every existing test keeps a stored owner**

In the `#[cfg(test)] mod tests` of `service_accounts.rs`:

- Change the `fakes` import to:
```rust
    use crate::application::fakes::{
        FakeAuthorizer, FakeOutbox, FakeUnitOfWork, FixedClock, InMemoryApiKeys, InMemoryServiceAccounts, SeqIds, TenancyStore, forged_variants, seed_org, seed_project, seed_team, store_with_orgs,
        tenancy_nodes,
    };
    use crate::log_capture::capture_logs;
```
- Add `ProjectId, TeamId` to the `paigasus_iam_core::{…}` test import.
- Add `store: TenancyStore,` to `struct ServiceWithFakes`.
- In `new_service_with_fakes`, add before `let svc = …`:
```rust
        // SMA-646: `create`/`list` now load the owner node. Every `owner_org(n)` with n in
        // 1..=32 names a STORED organization, so the pre-SMA-646 tests keep their meaning.
        // `owner_org(404)` stays unseeded for the not-found test.
        let store = store_with_orgs(1..=32);
```
  add `nodes: tenancy_nodes(&store),` to the `ServiceAccountServiceDeps` literal, and return `ServiceWithFakes { svc, repo, keys, cache, outbox, store }`.
- In `archive_never_evicts_the_cache_when_the_mutation_rolls_back` (the second Deps literal, line 494), add `nodes: tenancy_nodes(&store_with_orgs([7])),`.
- `archive_disables_and_evicts_keys` destructures `ServiceWithFakes { svc, repo, keys, cache, outbox }`: add `..` at the end of that pattern.
- Add this helper after `missing_id`:
```rust
    /// SMA-646: seeds org 100 -> team 101 -> project 102 into `store`.
    fn seed_chain(store: &TenancyStore) -> (OrganizationId, TeamId, ProjectId) {
        let org = seed_org(store, 100);
        let team = seed_team(store, &org, 101);
        let project = seed_project(store, &team, 102);
        (org, team, project)
    }

    /// SMA-646: the three STORED nodes of `seed_chain` as owners.
    fn stored_owners(org: &OrganizationId, team: &TeamId, project: &ProjectId) -> [TenancyNodeRef; 3] {
        [TenancyNodeRef::Organization(org.clone()), TenancyNodeRef::Team(team.clone()), TenancyNodeRef::Project(project.clone())]
    }
```

- [ ] **Step 3: Write the failing tests U1–U8**

Add these tests at the end of the test module. Each granted test allows the STORED canonical only,
never the forged one (spec §4.1: `FakeAuthorizer` is not Cedar; this is what lets mutation m3 red U1).

```rust
    /// SMA-646 U1 (B1, AC 1): a forged org slot or region on an EXISTING owner answers
    /// `PrnMismatch`. Nothing is written and no event is enqueued.
    #[tokio::test]
    async fn create_refuses_a_forged_owner() {
        let fake = FakeAuthorizer::default();
        let ServiceWithFakes { svc, repo, outbox, store, .. } = new_service_with_fakes(fake.clone());
        let (org, team, project) = seed_chain(&store);
        for node in stored_owners(&org, &team, &project) {
            fake.allow(Action::CreateServiceAccount, &owner_resource_prn(&node));
        }
        let actor = actor_prn(1);

        for (label, forged) in forged_variants(&org, &team, &project) {
            let err = svc.create(&actor, forged, "ci-bot").await.unwrap_err();
            assert_eq!(err, TenancyError::PrnMismatch, "{label}");
        }
        assert!(repo.accounts.lock().unwrap().is_empty(), "a refused create must not persist anything");
        assert!(outbox.0.lock().unwrap().is_empty(), "a refused create must not enqueue an event");
    }

    /// SMA-646 U2 (AC 3): the correct PRN succeeds and the record carries the STORED owner.
    #[tokio::test]
    async fn create_returns_the_stored_owner() {
        let fake = FakeAuthorizer::default();
        let ServiceWithFakes { svc, store, .. } = new_service_with_fakes(fake.clone());
        let (org, team, project) = seed_chain(&store);
        let actor = actor_prn(1);
        for (i, node) in stored_owners(&org, &team, &project).into_iter().enumerate() {
            fake.allow(Action::CreateServiceAccount, &owner_resource_prn(&node));
            let record = svc.create(&actor, node.clone(), &format!("bot-{i}")).await.unwrap();
            assert_eq!(record.account.owner.canonical(), node.canonical());
        }
    }

    /// SMA-646 U3 (B2, AC 2): a forged owner on List answers `PrnMismatch`.
    #[tokio::test]
    async fn list_refuses_a_forged_owner() {
        let fake = FakeAuthorizer::default();
        let ServiceWithFakes { svc, store, .. } = new_service_with_fakes(fake.clone());
        let (org, team, project) = seed_chain(&store);
        for node in stored_owners(&org, &team, &project) {
            fake.allow(Action::ListServiceAccounts, &owner_resource_prn(&node));
        }
        let actor = actor_prn(1);

        for (label, forged) in forged_variants(&org, &team, &project) {
            let err = svc.list(&actor, &forged, Page::new(None, None).unwrap()).await.unwrap_err();
            assert_eq!(err, TenancyError::PrnMismatch, "{label}");
        }
    }

    /// SMA-646 U4: the correct PRN lists the owner's accounts, each with the STORED owner.
    #[tokio::test]
    async fn list_returns_accounts_for_the_correct_owner() {
        let fake = FakeAuthorizer::default();
        let ServiceWithFakes { svc, store, .. } = new_service_with_fakes(fake.clone());
        let (_org, team, _project) = seed_chain(&store);
        let owner = TenancyNodeRef::Team(team.clone());
        fake.allow(Action::CreateServiceAccount, &owner_resource_prn(&owner));
        fake.allow(Action::ListServiceAccounts, &owner_resource_prn(&owner));
        let actor = actor_prn(1);
        svc.create(&actor, owner.clone(), "one").await.unwrap();
        svc.create(&actor, owner.clone(), "two").await.unwrap();

        let listed = svc.list(&actor, &owner, Page::new(None, None).unwrap()).await.unwrap();
        assert_eq!(listed.len(), 2);
        for record in &listed {
            assert_eq!(record.account.owner.canonical(), team.canonical());
        }
    }

    /// SMA-646 U5 (B6): with a default-deny authorizer, Create AND List answer `Forbidden` for a
    /// forged and a correct PRN alike. This proves the ORDER (authorize before compare) only;
    /// the "alike" property for real decisions rests on Cedar (I1).
    #[tokio::test]
    async fn an_ungranted_caller_cannot_tell_a_forged_owner_from_a_correct_one() {
        let ServiceWithFakes { svc, store, .. } = new_service_with_fakes(FakeAuthorizer::default());
        let (_org, team, _project) = seed_chain(&store);
        let correct = TenancyNodeRef::Team(team.clone());
        let forged = TenancyNodeRef::Team(TeamId::from_parts(Uuid::from_u128(0xF0F0), team.uuid()));
        let actor = actor_prn(1);

        for (label, node) in [("correct", correct), ("forged", forged)] {
            assert_eq!(svc.create(&actor, node.clone(), "ci-bot").await.unwrap_err(), TenancyError::Forbidden, "create {label}");
            assert_eq!(svc.list(&actor, &node, Page::new(None, None).unwrap()).await.unwrap_err(), TenancyError::Forbidden, "list {label}");
        }
    }

    /// SMA-646 U6 (B3, AC 5): a forged owner outranks an invalid name. The control (correct PRN,
    /// same name) answers the name error, so this test cannot pass vacuously.
    #[tokio::test]
    async fn a_forged_owner_outranks_an_invalid_name() {
        let fake = FakeAuthorizer::default();
        let ServiceWithFakes { svc, store, .. } = new_service_with_fakes(fake.clone());
        let (_org, team, _project) = seed_chain(&store);
        let correct = TenancyNodeRef::Team(team.clone());
        fake.allow(Action::CreateServiceAccount, &owner_resource_prn(&correct));
        let forged = TenancyNodeRef::Team(TeamId::from_parts(Uuid::from_u128(0xF0F0), team.uuid()));
        let actor = actor_prn(1);

        assert_eq!(svc.create(&actor, forged, "   ").await.unwrap_err(), TenancyError::PrnMismatch);
        assert!(matches!(svc.create(&actor, correct, "   ").await.unwrap_err(), TenancyError::InvalidName(_)));
    }

    /// SMA-646 U7 (B5, AC 7): an unknown owner uuid answers `NotFound` on Create and List, also
    /// for a default-deny authorizer (the load runs first).
    #[tokio::test]
    async fn an_unknown_owner_is_not_found() {
        let (svc, ..) = new_service(FakeAuthorizer::default());
        let actor = actor_prn(1);
        let unknown_team = TenancyNodeRef::Team(TeamId::from_parts(Uuid::from_u128(1), Uuid::from_u128(4040)));

        for node in [owner_org(404), unknown_team] {
            assert_eq!(svc.create(&actor, node.clone(), "ci-bot").await.unwrap_err(), TenancyError::NotFound);
            assert_eq!(svc.list(&actor, &node, Page::new(None, None).unwrap()).await.unwrap_err(), TenancyError::NotFound);
        }
    }

    /// SMA-646 U8 (AC 8): each refused mismatch writes exactly one warning line, with the RPC,
    /// the requested PRN and the stored PRN.
    #[tokio::test]
    async fn a_forged_owner_logs_one_warning() {
        let (logs, _guard) = capture_logs();
        let fake = FakeAuthorizer::default();
        let ServiceWithFakes { svc, store, .. } = new_service_with_fakes(fake.clone());
        let (_org, team, _project) = seed_chain(&store);
        let correct = TenancyNodeRef::Team(team.clone());
        fake.allow(Action::CreateServiceAccount, &owner_resource_prn(&correct));
        fake.allow(Action::ListServiceAccounts, &owner_resource_prn(&correct));
        let forged = TenancyNodeRef::Team(TeamId::from_parts(Uuid::from_u128(0xF0F0), team.uuid()));
        let actor = actor_prn(1);

        let _ = svc.create(&actor, forged.clone(), "ci-bot").await.unwrap_err();
        let _ = svc.list(&actor, &forged, Page::new(None, None).unwrap()).await.unwrap_err();

        let text = logs.text();
        assert_eq!(text.matches("requested_prn=").count(), 2, "one warning per refused call: {text}");
        assert_eq!(text.matches("rpc=CreateServiceAccount").count(), 1, "{text}");
        assert_eq!(text.matches("rpc=ListServiceAccounts").count(), 1, "{text}");
        assert!(text.contains(&format!("requested_prn={}", forged.canonical())), "{text}");
        assert!(text.contains(&format!("stored_prn={}", correct.canonical())), "{text}");
        assert!(text.contains("WARN"), "{text}");
    }
```

- [ ] **Step 4: Run the new tests to see them fail**

Run: `cd <worktree>/rs && cargo nextest run -p paigasus-iam --lib --no-fail-fast -E 'test(/service_accounts::tests/)'`
Expected: U1, U3, U6, U7, U8 FAIL (today's code echoes or answers `Forbidden`/`InvalidName`/`Ok`).
U2, U4, U5 pass already (their behaviour does not change). Every pre-existing test PASSES.

- [ ] **Step 5: Call the helper in `create` and `list`**

In `create`, replace the first line
`self.authorize.check(actor, Action::CreateServiceAccount, &owner_resource_prn(&owner)).await?;` with:

```rust
        let owner = self.nodes.resolve_and_authorize(&self.authorize, actor, Action::CreateServiceAccount, &owner, "CreateServiceAccount").await?;
```

In `list`, replace the body with:

```rust
        let owner = self.nodes.resolve_and_authorize(&self.authorize, actor, Action::ListServiceAccounts, owner, "ListServiceAccounts").await?;
        Ok(self.repo.list_by_owner(&owner, page.limit, page.offset).await?)
```

- [ ] **Step 6: Update the docs**

- Module doc, line 4-5: replace "Mirrors `RoleService`'s DI + authorize pattern (`application/roles.rs:\n//! 78-204`): every method authorizes BEFORE mutating/reading." with "Every method authorizes BEFORE mutating/reading." (the range is stale; spec §3.1).
- Add this paragraph to the module doc, after the first paragraph:
```rust
//!
//! **SMA-646: `create` and `list` confirm the owner PRN against storage.** The caller's
//! `owner` comes from a parsed wire PRN, and `TenancyNodeRef::from_prn` does not check the
//! org-slot value or the region. Both methods call
//! [`TenancyNodes::resolve_and_authorize`](crate::application::tenancy_nodes::TenancyNodes):
//! load the owner node (unknown -> `NotFound`), authorize against its STORED PRN, refuse a
//! differing caller PRN with `PrnMismatch` (one warning line), then use the STORED node for the
//! write, the list filter and the answer. The F1 invariant (a node's `prn` column never changes)
//! makes the compare sound outside the transaction. `get`/`archive` take the SA's own principal
//! PRN and authorize against the stored owner of the SA row, so they need no such check.
```
- Replace the `create` doc comment with:
```rust
    /// Creates a service account owned by `owner`. SMA-646 order: load `owner`'s node
    /// (`NotFound` if absent), authorize `Action::CreateServiceAccount` against its STORED PRN
    /// BEFORE minting anything, then refuse a caller PRN that differs from the stored one
    /// (`PrnMismatch`, module docs). Authorize before compare keeps a mismatch from telling an
    /// ungranted caller which organization owns the node. `name` validation
    /// (`ServiceAccount::new`) runs after that, so a forged owner outranks an invalid name
    /// (SMA-645 B2). The record carries the STORED owner. The returned `status` is `Active`
    /// WITHOUT a re-query — a freshly created SA's principal is minted `Active` right above.
    /// SMA-446 Slice B Task B7 (module docs — OUTBOX-ONLY): the principal+SA insert and its
    /// `iam.principal.created` event share ONE UoW transaction; a duplicate-name-per-owner
    /// unique-violation inside `create_in` rolls the whole unit of work back before the event is
    /// ever enqueued.
```
- Replace the `list` doc comment with:
```rust
    /// Lists service accounts owned by `owner`, `ORDER BY created_at, id` (rule 9, delegated to
    /// the repo). SMA-646: loads `owner`'s node (`NotFound` if absent), authorizes
    /// `Action::ListServiceAccounts` against its STORED PRN, refuses a differing caller PRN
    /// (`PrnMismatch`), and filters on the STORED node. Before SMA-646 a forged owner PRN
    /// listed the real owner's accounts, each labelled with the forged PRN. The `Page` is built
    /// in the transport, so `InvalidPagination` still outranks a forged owner (spec B7).
```
- If Task 1 Step 6 added `#[cfg_attr(not(test), allow(dead_code))]` on `resolve_and_authorize`, remove it now.
- Read the module doc of `adapters/grpc/service_accounts.rs` (lines 1-29). If it claims anything about owner validation, correct it. On 906de46c it claims nothing, so no edit is expected.

- [ ] **Step 7: Run the tests to see them pass**

Run: `cd <worktree>/rs && cargo nextest run -p paigasus-iam --lib --no-fail-fast -E 'test(/service_accounts::tests|tenancy_nodes::tests/)'`
Expected: all PASS.

Run: `cd <worktree>/rs && cargo fmt -p paigasus-iam && cargo clippy -p paigasus-iam --all-targets --locked -- -D warnings`
Expected: no warning.

Run: `cd <worktree> && grep -n '"[a-z]*-[a-z-]*"' rs/crates/services/paigasus-iam/src/application/service_accounts.rs rs/crates/services/paigasus-iam/src/application/tenancy_nodes.rs`
Expected: no registry reason code in quotes (`"ci-bot"` style names are fine; check each hit
against `TenancyError`'s codes in `application/error.rs`).

- [ ] **Step 8: Commit**

```bash
cd <worktree> && git branch --show-current   # must print feature/sma-646-service-account-stored-owner
git add rs/crates/services/paigasus-iam/src/application/service_accounts.rs rs/crates/services/paigasus-iam/src/adapters/http/mod.rs
git commit -m "fix(rs): confirm the owner PRN of CreateServiceAccount and ListServiceAccounts against storage (SMA-646)

Load the owner node, authorize against its stored PRN, refuse a forged
org slot or region with prn-mismatch, and answer with the stored owner.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: `ApiKeyService::issue` confirms the scope PRN (step 4 after D15)

**Files:**
- Modify: `IAM/src/application/api_keys.rs` (struct, Deps, `new`, `issue`, docs, tests)
- Modify: `IAM/src/adapters/http/mod.rs` (the `ApiKeyServiceDeps` literal, about line 722, and the `ServiceAccountServiceDeps` line from Task 2)

**Interfaces:**
- Consumes: `TenancyNodes` (Task 1); the local `tenancy_nodes` in `AppState::new` (Task 2); `fakes::{seed_org, seed_team, seed_project, store_with_orgs, tenancy_nodes, forged_variants, TenancyStore}`.
- Produces: `ApiKeyServiceDeps { …, pub nodes: TenancyNodes, … }`.

- [ ] **Step 1: Add the `nodes` dependency (no behaviour change yet)**

In `api_keys.rs`:
- Add `use crate::application::tenancy_nodes::TenancyNodes;` after the `use crate::application::pagination::Page;` line.
- Add `nodes: TenancyNodes,` to `ApiKeyService` after `authorize: Authorize,`.
- Add to `ApiKeyServiceDeps` after `pub authorize: Authorize,`:
```rust
    /// SMA-646: loads the key's scope node for the stored-PRN check of `issue`.
    pub nodes: TenancyNodes,
```
- Add `nodes: deps.nodes,` to `new` after `authorize: deps.authorize,`.

In `adapters/http/mod.rs`:
- In the `ApiKeyServiceDeps` literal add `nodes: tenancy_nodes,` after `authorize: authorize.clone(),`.
- The `ServiceAccountServiceDeps` literal keeps `nodes: tenancy_nodes.clone(),`. Both services now share one value.

In the `api_keys.rs` test module:
- Change the `fakes` import to:
```rust
    use crate::application::fakes::{
        FakeAuditLog, FakeAuthorizer, FakeOutbox, FakeSecretHasher, FakeUnitOfWork, FixedClock, InMemoryApiKeys, InMemoryRoleGrants, InMemoryServiceAccounts, SeqIds, SeqKeyEntropy, TenancyStore,
        forged_variants, seed_org, seed_project, seed_team, store_with_orgs, tenancy_nodes,
    };
    use crate::log_capture::capture_logs;
```
- Add `ProjectId, TeamId` to the `paigasus_iam_core::{…}` test import.
- Add `store: TenancyStore,` to `struct ServiceWithFakes`.
- In `new_service_with_fakes`, add before `let svc = …`:
```rust
        // SMA-646: `issue` now loads the key's scope node. Every `owner_org(n)` with n in
        // 1..=32 names a STORED organization, so the pre-SMA-646 tests, which scope each key to
        // the SA's owner `owner_org(1)` and allow `IssueApiKey` there, keep their meaning.
        let store = store_with_orgs(1..=32);
```
  add `nodes: tenancy_nodes(&store),` to the Deps literal, and add `store,` to the returned `ServiceWithFakes { … }`.
- In `revoke_never_evicts_cache_when_the_mutation_rolls_back` (the second Deps literal, line 793) add `nodes: tenancy_nodes(&store_with_orgs([1])),`.
- Add this helper after `seed_role_grant`:
```rust
    /// SMA-646: seeds org 100 -> team 101 -> project 102 into `store`.
    fn seed_chain(store: &TenancyStore) -> (OrganizationId, TeamId, ProjectId) {
        let org = seed_org(store, 100);
        let team = seed_team(store, &org, 101);
        let project = seed_project(store, &team, 102);
        (org, team, project)
    }
```

- [ ] **Step 2: Write the failing tests U9–U14**

Each test seeds the SA with the STORED org 100 as owner. The granted tests allow `IssueApiKey` at
the stored owner and at each STORED scope node only.

```rust
    /// SMA-646 U9 (K1, AC 11): a forged scope on an EXISTING node answers `PrnMismatch`. No key,
    /// no outbox event and no audit entry is written.
    #[tokio::test]
    async fn issue_refuses_a_forged_scope() {
        let fake = FakeAuthorizer::default();
        let ServiceWithFakes {
            svc, keys, service_accounts, outbox, audit, store, ..
        } = new_service_with_fakes(fake.clone());
        let (org, team, project) = seed_chain(&store);
        let owner = TenancyNodeRef::Organization(org.clone());
        for node in [owner.clone(), TenancyNodeRef::Team(team.clone()), TenancyNodeRef::Project(project.clone())] {
            fake.allow(Action::IssueApiKey, &node_resource_prn(&node));
        }
        let sa_id = seed_service_account(&service_accounts, owner, 1000);
        let actor = actor_prn(1);

        for (label, forged) in forged_variants(&org, &team, &project) {
            let err = svc.issue(&actor, &sa_id, forged, None, Vec::new(), Vec::new()).await.unwrap_err();
            assert_eq!(err, TenancyError::PrnMismatch, "{label}");
        }
        assert!(keys.0.lock().unwrap().is_empty(), "a refused issue must not persist a key");
        assert!(outbox.0.lock().unwrap().is_empty(), "a refused issue must not enqueue an event");
        assert!(audit.0.lock().unwrap().is_empty(), "a refused issue must not record an audit entry");
    }

    /// SMA-646 U10 (K2, AC 12): the correct scope succeeds, and the key and the event payload
    /// carry the STORED scope.
    #[tokio::test]
    async fn issue_returns_the_stored_scope() {
        let fake = FakeAuthorizer::default();
        let ServiceWithFakes { svc, service_accounts, outbox, store, .. } = new_service_with_fakes(fake.clone());
        let (org, team, _project) = seed_chain(&store);
        let owner = TenancyNodeRef::Organization(org);
        let scope = TenancyNodeRef::Team(team.clone());
        fake.allow(Action::IssueApiKey, &node_resource_prn(&owner));
        fake.allow(Action::IssueApiKey, &node_resource_prn(&scope));
        let sa_id = seed_service_account(&service_accounts, owner, 1001);

        let new_key = svc.issue(&actor_prn(1), &sa_id, scope, None, Vec::new(), Vec::new()).await.unwrap();
        assert_eq!(new_key.key.scope.canonical(), team.canonical());
        let events = outbox.0.lock().unwrap();
        assert_eq!(events[0].payload["scope"], serde_json::json!(team.canonical()));
    }

    /// SMA-646 U11 (K4, AC 14): the caller may issue at the SA owner but has no `IssueApiKey` at
    /// the scope node. Issue answers `Forbidden` for a forged and a correct scope alike.
    #[tokio::test]
    async fn a_caller_without_issue_at_the_scope_cannot_tell_a_forged_scope_from_a_correct_one() {
        let fake = FakeAuthorizer::default();
        let ServiceWithFakes { svc, keys, service_accounts, store, .. } = new_service_with_fakes(fake.clone());
        let (org, team, _project) = seed_chain(&store);
        let owner = TenancyNodeRef::Organization(org);
        fake.allow(Action::IssueApiKey, &node_resource_prn(&owner));
        // Deliberately NOT allowed: `IssueApiKey` at `team`.
        let sa_id = seed_service_account(&service_accounts, owner, 1002);
        let correct = TenancyNodeRef::Team(team.clone());
        let forged = TenancyNodeRef::Team(TeamId::from_parts(Uuid::from_u128(0xF0F0), team.uuid()));

        for (label, scope) in [("correct", correct), ("forged", forged)] {
            let err = svc.issue(&actor_prn(1), &sa_id, scope, None, Vec::new(), Vec::new()).await.unwrap_err();
            assert_eq!(err, TenancyError::Forbidden, "{label}");
        }
        assert!(keys.0.lock().unwrap().is_empty());
    }

    /// SMA-646 U12 (K3, AC 13): an unknown scope uuid answers `NotFound`, and no key is written.
    #[tokio::test]
    async fn issue_with_an_unknown_scope_is_not_found() {
        let fake = FakeAuthorizer::default();
        let ServiceWithFakes { svc, keys, service_accounts, store, .. } = new_service_with_fakes(fake.clone());
        let (org, _team, _project) = seed_chain(&store);
        let owner = TenancyNodeRef::Organization(org.clone());
        fake.allow(Action::IssueApiKey, &node_resource_prn(&owner));
        let sa_id = seed_service_account(&service_accounts, owner, 1003);
        let unknown = TenancyNodeRef::Team(TeamId::from_parts(org.uuid(), Uuid::from_u128(4040)));

        let err = svc.issue(&actor_prn(1), &sa_id, unknown, None, Vec::new(), Vec::new()).await.unwrap_err();
        assert_eq!(err, TenancyError::NotFound);
        assert!(keys.0.lock().unwrap().is_empty(), "no key may be written for an unknown scope");
    }

    /// SMA-646 U13 (AC 8): one forged Issue writes exactly one warning line.
    #[tokio::test]
    async fn a_forged_scope_logs_one_warning() {
        let (logs, _guard) = capture_logs();
        let fake = FakeAuthorizer::default();
        let ServiceWithFakes { svc, service_accounts, store, .. } = new_service_with_fakes(fake.clone());
        let (org, team, _project) = seed_chain(&store);
        let owner = TenancyNodeRef::Organization(org);
        let correct = TenancyNodeRef::Team(team.clone());
        fake.allow(Action::IssueApiKey, &node_resource_prn(&owner));
        fake.allow(Action::IssueApiKey, &node_resource_prn(&correct));
        let sa_id = seed_service_account(&service_accounts, owner, 1004);
        let forged = TenancyNodeRef::Team(TeamId::from_parts(Uuid::from_u128(0xF0F0), team.uuid()));

        let _ = svc.issue(&actor_prn(1), &sa_id, forged.clone(), None, Vec::new(), Vec::new()).await.unwrap_err();

        let text = logs.text();
        assert_eq!(text.matches("requested_prn=").count(), 1, "{text}");
        assert_eq!(text.matches("rpc=IssueApiKey").count(), 1, "{text}");
        assert!(text.contains(&format!("requested_prn={}", forged.canonical())), "{text}");
        assert!(text.contains(&format!("stored_prn={}", correct.canonical())), "{text}");
    }

    /// SMA-646 U14 (K5, AC 15, Review Focus 4): the scope check runs AFTER the owner check and
    /// D15. A caller who fails either one gets `Forbidden` for a forged scope, never a mismatch.
    #[tokio::test]
    async fn a_forged_scope_never_outranks_the_owner_or_d15_checks() {
        let fake = FakeAuthorizer::default();
        let ServiceWithFakes { svc, service_accounts, grants, store, .. } = new_service_with_fakes(fake.clone());
        let (org, team, _project) = seed_chain(&store);
        let owner = TenancyNodeRef::Organization(org);
        let correct = TenancyNodeRef::Team(team.clone());
        let forged = TenancyNodeRef::Team(TeamId::from_parts(Uuid::from_u128(0xF0F0), team.uuid()));
        // Allowed at the scope only, NOT at the SA owner.
        fake.allow(Action::IssueApiKey, &node_resource_prn(&correct));
        let sa_owner_denied = seed_service_account(&service_accounts, owner.clone(), 1005);
        assert_eq!(
            svc.issue(&actor_prn(1), &sa_owner_denied, forged.clone(), None, Vec::new(), Vec::new()).await.unwrap_err(),
            TenancyError::Forbidden,
            "owner check first"
        );

        // Now allowed at the owner too, but the SA holds a grant the caller cannot grant (D15).
        fake.allow(Action::IssueApiKey, &node_resource_prn(&owner));
        let sa_d15_denied = seed_service_account(&service_accounts, owner, 1006);
        seed_role_grant(&grants, 1906, &sa_d15_denied, "org_admin", GrantScope::Node(owner_org(2)));
        assert_eq!(svc.issue(&actor_prn(1), &sa_d15_denied, forged, None, Vec::new(), Vec::new()).await.unwrap_err(), TenancyError::Forbidden, "D15 second");
    }
```

Note: `seed_service_account` names every SA `ci-bot`. The fake repo keys by uuid and never
checks the name on a direct insert, so two SAs per owner are fine here.

- [ ] **Step 3: Run the new tests to see them fail**

Run: `cd <worktree>/rs && cargo nextest run -p paigasus-iam --lib --no-fail-fast -E 'test(/api_keys::tests/)'`
Expected: U9, U11, U12, U13 FAIL (today `issue` accepts any scope). U10 and U14 pass already.
Every pre-existing test PASSES.

- [ ] **Step 4: Add step 4 to `issue`**

In `issue`, after the D15 `for grant in &sa_grants { … }` loop and before `let id = self.ids.new_api_key_id();`, add:

```rust
        // SMA-646 step 4 (module docs): load the scope node, authorize `IssueApiKey` against its
        // STORED PRN, refuse a forged scope PRN, and use the STORED node from here on — before
        // any id is minted or secret generated.
        let scope = self.nodes.resolve_and_authorize(&self.authorize, actor, Action::IssueApiKey, &scope, "IssueApiKey").await?;
```

`ApiKey { scope, … }` and the payload `"scope": key.scope.canonical()` now use the resolved
value through the shadowed binding. Do not change them.

- [ ] **Step 5: Update the docs**

- Module doc, line 4-5: replace the stale "(`application/roles.rs:78-204`,\n//! `application/service_accounts.rs:63-143`)" with nothing, so the sentence reads "Mirrors `RoleService`/`ServiceAccountService`'s DI + authorize pattern: every method authorizes BEFORE mutating/reading."
- Add this paragraph to the module doc after the D15 paragraph:
```rust
//!
//! **SMA-646: `issue` confirms the key's scope PRN against storage (step 4).** The caller's
//! `scope` comes from a parsed wire PRN with an unchecked org slot and region. After the owner
//! check and D15, `issue` calls
//! [`TenancyNodes::resolve_and_authorize`](crate::application::tenancy_nodes::TenancyNodes):
//! load the scope node (unknown -> `NotFound`), authorize `Action::IssueApiKey` against its
//! STORED PRN, refuse a differing caller PRN with `PrnMismatch` (one warning line), and build the
//! key and the `iam.api_key.issued` payload from the STORED node. The authorization at the scope
//! is NEW in SMA-646 (spec K4, §8 Q5): without it, a caller who may issue keys for SOME service
//! account could label a key with a node outside their authority, and could read a mismatch
//! against success as an answer to "which organization owns this node". A caller granted at an
//! organization passes for a team or project inside it (Cedar `resource in ?resource`).
```
- In the `issue` doc comment, after item 3 of the numbered list, add:
```rust
    /// 4. SMA-646: the key's `scope` must name an existing node (`NotFound`), `actor` must be
    ///    authorized for `Action::IssueApiKey` AT that node's STORED PRN (`Forbidden`), and the
    ///    caller's scope PRN must equal the stored one (`PrnMismatch`). The key carries the
    ///    STORED scope. This runs after steps 2 and 3, so a caller who fails them still gets
    ///    `Forbidden` for a forged scope (spec K5).
```
  and change "Only once both authorization checks fully pass" to "Only once all four checks pass".

- [ ] **Step 6: Run the tests to see them pass**

Run: `cd <worktree>/rs && cargo nextest run -p paigasus-iam --lib --no-fail-fast`
Expected: all PASS (the whole lib, so any other constructor or fixture break shows here).

Run: `cd <worktree>/rs && cargo fmt -p paigasus-iam && cargo clippy -p paigasus-iam --all-targets --locked -- -D warnings`
Expected: no warning.

Run: `cd <worktree> && grep -rn "ServiceAccountServiceDeps {\|ApiKeyServiceDeps {" rs/crates/services/paigasus-iam`
Expected: every literal (four in `application/`, two in `adapters/http/mod.rs`, none in `tests/`) has a `nodes:` field. The build already proves it; this is the spec §3.4 re-grep.

- [ ] **Step 7: Commit**

```bash
cd <worktree> && git branch --show-current   # must print feature/sma-646-service-account-stored-owner
git add rs/crates/services/paigasus-iam/src/application/api_keys.rs rs/crates/services/paigasus-iam/src/adapters/http/mod.rs
git commit -m "fix(rs): confirm the scope PRN of IssueApiKey against storage (SMA-646)

After the owner and D15 checks, load the scope node, authorize IssueApiKey
against its stored PRN, refuse a forged scope with prn-mismatch, and build
the key and the issued event from the stored node.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: gRPC integration tests I1 and I3 (Docker Postgres, real Cedar)

**Files:**
- Modify: `IAM/tests/support/mod.rs` (add four helpers after `seed_org_ref`, about line 818)
- Modify: `IAM/tests/api_keys_grpc.rs` (imports, two local helpers, two tests)

**Interfaces:**
- Consumes: `support::{start_migrated_postgres, start_mock_idp, test_config, provision, provision_platform_admin, seed_org_admin, seed_org_ref, grpc_bearer}`; `AppState::new`; the service changes of Tasks 2 and 3.
- Produces (in `support`): `seed_team_ref(&DatabaseConnection, &TenancyNodeRef) -> TenancyNodeRef`, `prn_with_org(&str, &str) -> String`, `prn_with_region(&str, &str) -> String`, `prn_upper_uuids(&str) -> String`. Task 5 uses them.

These tests run after the fix, so they are not red-first. Task 6 proves that they bite (m1, m2, m7).

- [ ] **Step 1: Add the support helpers**

After `seed_org_ref` in `tests/support/mod.rs`, add:

```rust
/// SMA-646: seeds a bare `team` row under `org` via raw SQL, with `prn = TeamId::canonical()`
/// (the F1 invariant the stored-PRN check relies on), and returns a `TenancyNodeRef` naming it.
/// The slug comes from the minted uuid, so repeat calls never collide.
#[allow(dead_code)]
pub async fn seed_team_ref(db: &DatabaseConnection, org: &TenancyNodeRef) -> TenancyNodeRef {
    let org_uuid = org.resource_uuid();
    let id = KernelIdGenerator.new_team_id(org_uuid);
    let uuid = id.uuid();
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!(
            r#"INSERT INTO "team" (id, org_id, prn, slug, name, status, created_at, updated_at)
               VALUES ('{uuid}', '{org_uuid}', '{prn}', 'team-{slug}', 'Test Team', 'active', now(), now())"#,
            prn = id.canonical(),
            slug = uuid.simple(),
        ),
        [],
    ))
    .await
    .unwrap();
    TenancyNodeRef::Team(id)
}

/// SMA-646: splits a canonical PRN into `prn`, `pgs`, service, region, org, `type/uuid`.
fn prn_six_fields(prn: &str) -> Vec<&str> {
    let fields: Vec<&str> = prn.splitn(6, ':').collect();
    assert_eq!(fields.len(), 6, "a canonical prn has six fields: {prn}");
    fields
}

/// SMA-646: replaces the organization slot of `prn` (a forged-slot fixture).
#[allow(dead_code)]
pub fn prn_with_org(prn: &str, org: &str) -> String {
    let f = prn_six_fields(prn);
    format!("prn:pgs:{}:{}:{}:{}", f[2], f[3], org, f[5])
}

/// SMA-646: replaces the region slot of `prn` (a forged-region fixture).
#[allow(dead_code)]
pub fn prn_with_region(prn: &str, region: &str) -> String {
    let f = prn_six_fields(prn);
    format!("prn:pgs:{}:{}:{}:{}", f[2], region, f[4], f[5])
}

/// SMA-646: upper-cases the org slot and the resource uuid of a CORRECT prn. `Prn::parse`
/// normalises uuid case, so the answer must still carry the stored lower-case PRN.
#[allow(dead_code)]
pub fn prn_upper_uuids(prn: &str) -> String {
    let f = prn_six_fields(prn);
    let (kind, uuid) = f[5].split_once('/').expect("the last prn field is type/uuid");
    format!("prn:pgs:{}:{}:{}:{}/{}", f[2], f[3], f[4].to_uppercase(), kind, uuid.to_uppercase())
}
```

If `prn_six_fields` is reported unused in a test binary that uses none of the three callers, add
`#[allow(dead_code)]` on it too. `KernelIdGenerator`, `IdGenerator`, `Statement`, `DbBackend` and
`TenancyNodeRef` are already imported in `support/mod.rs` (used by `seed_org_ref`); check with the
compiler.

- [ ] **Step 2: Add imports and local helpers to `api_keys_grpc.rs`**

Add to the imports:

```rust
use paigasus_iam::adapters::persistence::entities::{api_key, audit_log, event_outbox, service_account};
use paigasus_iam_core::{EventType, TeamId};
use paigasus_kernel::Prn;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter};
use uuid::Uuid;
```

Add after `authed`:

```rust
/// Reads `ErrorInfo.reason` off a `tonic::Status`. Every IAM status carries one (SMA-504).
fn reason(err: &tonic::Status) -> String {
    let details = tonic_types::StatusExt::get_error_details(err);
    details.error_info().expect("every IAM status carries ErrorInfo").reason.clone()
}

/// SMA-646: `iam.principal.created` outbox rows whose payload names `name`. Filtered by type and
/// payload, never a total (spec §4.2).
async fn created_events(db: &DatabaseConnection, name: &str) -> usize {
    event_outbox::Entity::find()
        .filter(event_outbox::Column::EventType.eq(EventType::PrincipalCreated.as_wire()))
        .all(db)
        .await
        .expect("read event_outbox")
        .into_iter()
        .filter(|row| serde_json::from_str::<serde_json::Value>(&row.payload).expect("payload is json")["name"] == name)
        .count()
}

/// SMA-646: `service_account` rows with this name.
async fn accounts_named(db: &DatabaseConnection, name: &str) -> u64 {
    service_account::Entity::find().filter(service_account::Column::Name.eq(name)).count(db).await.expect("count service_account")
}

/// SMA-646: `iam.api_key.issued` outbox rows for one service account.
async fn issued_events(db: &DatabaseConnection, sa_prn: &str) -> u64 {
    event_outbox::Entity::find()
        .filter(event_outbox::Column::EventType.eq(EventType::ApiKeyIssued.as_wire()))
        .filter(event_outbox::Column::AggregatePrn.eq(sa_prn))
        .count(db)
        .await
        .expect("count event_outbox")
}

/// SMA-646: `IssueApiKey` audit rows. Their `resource_prn` is the SA's stored owner.
async fn issue_audits(db: &DatabaseConnection, owner_prn: &str) -> u64 {
    audit_log::Entity::find()
        .filter(audit_log::Column::Action.eq("IssueApiKey"))
        .filter(audit_log::Column::ResourcePrn.eq(owner_prn))
        .count(db)
        .await
        .expect("count audit_log")
}

/// SMA-646: `api_key` rows of one service account.
async fn keys_of(db: &DatabaseConnection, sa_prn: &str) -> u64 {
    let sa_uuid = Prn::parse(sa_prn).expect("sa prn").resource_id();
    api_key::Entity::find().filter(api_key::Column::ServiceAccountId.eq(sa_uuid)).count(db).await.expect("count api_key")
}
```

Check that `serde_json`, `sea-orm`, `uuid`, `paigasus-kernel` and `tonic-types` are reachable from
this test binary (they are used by `tests/grpc_tenancy.rs`, so they are). If an import is unused
after both tests exist, delete it.

- [ ] **Step 3: Write I1**

```rust
/// SMA-646 I1 (spec §4.2, AC 1-4, 7): `CreateServiceAccount`/`ListServiceAccounts` confirm the
/// owner PRN against storage, against real Postgres and real Cedar.
#[tokio::test]
async fn a_forged_owner_prn_never_creates_a_service_account() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let state = AppState::new(db.clone(), &support::test_config(&idp)).await.unwrap();
    let admin = idp.bearer("i1-admin", Some("i1-admin@example.com"), "paigasus", 3600);
    support::provision_platform_admin(&state, &admin).await;
    let stranger = idp.bearer("i1-stranger", Some("i1-stranger@example.com"), "paigasus", 3600);
    support::provision(&state, &stranger).await;
    let org = support::seed_org_ref(&state.db).await;
    let team = support::seed_team_ref(&state.db, &org).await;
    let (addr, server) = spawn_server(state).await;
    let mut client = ServiceAccountServiceClient::new(channel(addr).await);

    let absent_org = Uuid::from_u128(0x0f46).as_hyphenated().to_string();
    let forged_slot = support::prn_with_org(&team.canonical(), &absent_org);
    let forged_region = support::prn_with_region(&team.canonical(), "eu-west-1");
    let create = |owner_prn: String, name: &str| CreateServiceAccountRequest { owner_prn, name: name.to_string() };
    let list = |owner_prn: String| ListServiceAccountsRequest { owner_prn, limit: 0, offset: 0 };

    // B1: a forged org slot and a forged region answer prn-mismatch; nothing is written.
    for (label, forged) in [("org slot", forged_slot.clone()), ("region", forged_region.clone())] {
        let err = client.create_service_account(authed(create(forged.clone(), "i1-forged"), &admin)).await.unwrap_err();
        assert_eq!(err.code(), Code::InvalidArgument, "create {label}: {err:?}");
        assert_eq!(reason(&err), "prn-mismatch", "create {label}");
        // B2: List with the forged PRN answers the same.
        let err = client.list_service_accounts(authed(list(forged), &admin)).await.unwrap_err();
        assert_eq!(err.code(), Code::InvalidArgument, "list {label}: {err:?}");
        assert_eq!(reason(&err), "prn-mismatch", "list {label}");
    }
    assert_eq!(accounts_named(&db, "i1-forged").await, 0, "a refused create must write no service_account row");
    assert_eq!(created_events(&db, "i1-forged").await, 0, "a refused create must enqueue no iam.principal.created event");
    let listed = client.list_service_accounts(authed(list(team.canonical()), &admin)).await.unwrap().into_inner().service_accounts;
    assert!(listed.iter().all(|sa| sa.name != "i1-forged"), "{listed:?}");

    // B5: an unknown team uuid answers not-found.
    let unknown = TeamId::from_parts(org.resource_uuid(), Uuid::from_u128(0x0f47)).canonical();
    let err = client.create_service_account(authed(create(unknown, "i1-unknown"), &admin)).await.unwrap_err();
    assert_eq!(err.code(), Code::NotFound, "{err:?}");

    // B6 / AC 4: an ungranted caller gets permission-denied for the forged and the correct PRN
    // alike, on Create and List (real Cedar).
    for (label, prn) in [("correct", team.canonical()), ("forged", forged_slot.clone())] {
        let err = client.create_service_account(authed(create(prn.clone(), "i1-stranger"), &stranger)).await.unwrap_err();
        assert_eq!(err.code(), Code::PermissionDenied, "create {label}: {err:?}");
        assert_eq!(reason(&err), "forbidden", "create {label}");
        let err = client.list_service_accounts(authed(list(prn), &stranger)).await.unwrap_err();
        assert_eq!(err.code(), Code::PermissionDenied, "list {label}: {err:?}");
        assert_eq!(reason(&err), "forbidden", "list {label}");
    }

    // Positive control (AC 3): the correct PRN with upper-cased uuids creates the account, the
    // filtered outbox count moves by one, and Create, Get and List agree on the STORED owner.
    let created = client
        .create_service_account(authed(create(support::prn_upper_uuids(&team.canonical()), "i1-ok"), &admin))
        .await
        .unwrap()
        .into_inner()
        .service_account
        .expect("service_account");
    assert_eq!(created.owner_prn, team.canonical());
    assert_eq!(created_events(&db, "i1-ok").await, 1, "the positive control must be visible to the filtered count");
    let got = client
        .get_service_account(authed(GetServiceAccountRequest { prn: created.prn.clone() }, &admin))
        .await
        .unwrap()
        .into_inner()
        .service_account
        .expect("service_account");
    assert_eq!(got.owner_prn, created.owner_prn);
    let listed = client.list_service_accounts(authed(list(team.canonical()), &admin)).await.unwrap().into_inner().service_accounts;
    let entry = listed.iter().find(|sa| sa.prn == created.prn).expect("the created account is listed");
    assert_eq!(entry.owner_prn, created.owner_prn);

    server.abort();
}
```

- [ ] **Step 4: Write I3**

```rust
/// SMA-646 I3 (spec §4.2, AC 11-14): `IssueApiKey` confirms the scope PRN against storage, and
/// authorizes `IssueApiKey` at the stored scope (K4), against real Postgres and real Cedar.
#[tokio::test]
async fn a_forged_scope_prn_never_issues_an_api_key() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let state = AppState::new(db.clone(), &support::test_config(&idp)).await.unwrap();
    let admin = idp.bearer("i3-admin", Some("i3-admin@example.com"), "paigasus", 3600);
    support::provision_platform_admin(&state, &admin).await;
    let org_a = support::seed_org_ref(&state.db).await;
    let team_a = support::seed_team_ref(&state.db, &org_a).await;
    let org_b = support::seed_org_ref(&state.db).await;
    let team_b = support::seed_team_ref(&state.db, &org_b).await;
    // The actor holds org_admin at organization A only.
    let actor = idp.bearer("i3-actor", Some("i3-actor@example.com"), "paigasus", 3600);
    let actor_prn = support::provision(&state, &actor).await;
    support::seed_org_admin(&state, &actor_prn, &org_a.canonical()).await;
    let (addr, server) = spawn_server(state).await;
    let mut client = ServiceAccountServiceClient::new(channel(addr).await);

    // The SA is owned by team A (created by the admin with the correct PRN). It has no grants,
    // so D15 is empty and the only checks are the owner check and the new scope check.
    let sa = client
        .create_service_account(authed(CreateServiceAccountRequest { owner_prn: team_a.canonical(), name: "i3-bot".to_string() }, &admin))
        .await
        .unwrap()
        .into_inner()
        .service_account
        .expect("service_account");
    let issue = |scope_prn: String| IssueApiKeyRequest {
        service_account_prn: sa.prn.clone(),
        scope_prn,
        expires_at: None,
        scope_actions: Vec::new(),
        scope_roles: Vec::new(),
    };
    let events_before = issued_events(&db, &sa.prn).await;
    let audits_before = issue_audits(&db, &team_a.canonical()).await;

    // K1: a forged org slot and a forged region answer prn-mismatch; nothing is written.
    let absent_org = Uuid::from_u128(0x0f48).as_hyphenated().to_string();
    for (label, forged) in [
        ("org slot", support::prn_with_org(&team_a.canonical(), &absent_org)),
        ("region", support::prn_with_region(&team_a.canonical(), "eu-west-1")),
    ] {
        let err = client.issue_api_key(authed(issue(forged), &actor)).await.unwrap_err();
        assert_eq!(err.code(), Code::InvalidArgument, "{label}: {err:?}");
        assert_eq!(reason(&err), "prn-mismatch", "{label}");
    }
    assert_eq!(keys_of(&db, &sa.prn).await, 0, "a refused issue must write no api_key row");
    assert_eq!(issued_events(&db, &sa.prn).await, events_before, "a refused issue must enqueue no iam.api_key.issued event");
    assert_eq!(issue_audits(&db, &team_a.canonical()).await, audits_before, "a refused issue must write no IssueApiKey audit row");

    // K3: an unknown team uuid as scope answers not-found.
    let unknown = TeamId::from_parts(org_a.resource_uuid(), Uuid::from_u128(0x0f49)).canonical();
    let err = client.issue_api_key(authed(issue(unknown), &actor)).await.unwrap_err();
    assert_eq!(err.code(), Code::NotFound, "{err:?}");

    // K4: a scope node in organization B, where the actor has no IssueApiKey, answers
    // permission-denied for the correct and the forged PRN alike.
    let org_a_slot = org_a.resource_uuid().as_hyphenated().to_string();
    for (label, scope) in [("correct", team_b.canonical()), ("forged", support::prn_with_org(&team_b.canonical(), &org_a_slot))] {
        let err = client.issue_api_key(authed(issue(scope), &actor)).await.unwrap_err();
        assert_eq!(err.code(), Code::PermissionDenied, "{label}: {err:?}");
        assert_eq!(reason(&err), "forbidden", "{label}");
    }
    assert_eq!(keys_of(&db, &sa.prn).await, 0);

    // Positive control (AC 12, Review Focus 3): the actor's org-level grant covers team A as a
    // scope (Cedar `resource in`). Upper-cased uuids still succeed, the answer carries the
    // STORED scope, the filtered outbox count moves by one, and ListApiKeys agrees.
    let issued = client.issue_api_key(authed(issue(support::prn_upper_uuids(&team_a.canonical())), &actor)).await.unwrap().into_inner();
    let key = issued.api_key.expect("api_key");
    assert_eq!(key.scope_prn, team_a.canonical());
    assert_eq!(issued_events(&db, &sa.prn).await, events_before + 1, "the positive control must be visible to the filtered count");
    let listed = client
        .list_api_keys(authed(ListApiKeysRequest { service_account_prn: sa.prn.clone(), limit: 0, offset: 0 }, &actor))
        .await
        .unwrap()
        .into_inner()
        .api_keys;
    let entry = listed.iter().find(|k| k.id == key.id).expect("the issued key is listed");
    assert_eq!(entry.scope_prn, key.scope_prn);

    server.abort();
}
```

- [ ] **Step 5: Run the two tests**

Run: `cd <worktree>/rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --no-fail-fast --test api_keys_grpc`
Expected: all 5 tests PASS (3 existing, I1, I3). If Docker is not reachable, the run panics; start
Docker and run again. Do not accept a skip.

If a positive-control assertion fails because the gRPC `ListServiceAccounts`/`ListApiKeys` DTO
field names differ, read `contracts/` proto names for the generated struct and fix the test, not
the service.

- [ ] **Step 6: Lint and commit**

Run: `cd <worktree>/rs && cargo fmt -p paigasus-iam && cargo clippy -p paigasus-iam --all-targets --locked -- -D warnings`
Expected: no warning.

```bash
cd <worktree> && git branch --show-current   # must print feature/sma-646-service-account-stored-owner
git add rs/crates/services/paigasus-iam/tests/support/mod.rs rs/crates/services/paigasus-iam/tests/api_keys_grpc.rs
git commit -m "test(rs): pin the stored owner and scope PRN over gRPC (SMA-646)

Forged owner and scope PRNs answer prn-mismatch and write nothing; an
ungranted caller sees forbidden for both; a correct PRN with upper-case
uuids answers the stored PRN.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: HTTP integration tests I2 and I4

**Files:**
- Modify: `IAM/tests/http_service_accounts.rs`

**Interfaces:**
- Consumes: `support::{app_with_state, provision_platform_admin, seed_org_ref, seed_team_ref, prn_with_org, prn_with_region, prn_upper_uuids, send}` (Task 4 adds the last four).
- Produces: nothing for later tasks.

- [ ] **Step 1: Extend the import line**

Change `use support::{app_with_state, provision, provision_platform_admin, seed_org_ref, send, send_raw};` to:

```rust
use support::{app_with_state, prn_upper_uuids, prn_with_org, prn_with_region, provision, provision_platform_admin, seed_org_ref, seed_team_ref, send, send_raw};
```

- [ ] **Step 2: Write I2**

```rust
/// SMA-646 I2 (spec §4.2, AC 1-3): `POST /v1/service-accounts` and `GET /v1/service-accounts`
/// refuse a forged owner PRN. Before SMA-646 the GET answered 200 with the forged PRN on each
/// row (B2); this is the only test that reproduces that relabelling over HTTP.
#[tokio::test]
async fn http_forged_owner_prn_is_refused() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, state, idp) = app_with_state(db).await;
    let token = idp.bearer("i2-admin", Some("i2-admin@example.com"), "paigasus", 3600);
    provision_platform_admin(&state, &token).await;
    let org = seed_org_ref(&state.db).await;
    let team = seed_team_ref(&state.db, &org).await;

    // A real account under the team, so a relabelling List would have a row to relabel.
    let (status, real) = send(&app, "POST", "/v1/service-accounts", Some(json!({ "owner_prn": team.canonical(), "name": "i2-real" })), Some(token.as_str())).await;
    assert_eq!(status, StatusCode::CREATED, "{real}");

    let absent_org = "00000000-0000-0000-0000-000000000f50";
    for (label, forged) in [("org slot", prn_with_org(&team.canonical(), absent_org)), ("region", prn_with_region(&team.canonical(), "eu-west-1"))] {
        let (status, err) = send(&app, "POST", "/v1/service-accounts", Some(json!({ "owner_prn": forged, "name": "i2-forged" })), Some(token.as_str())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "create {label}: {err}");
        assert_eq!(err["error"]["code"], "prn-mismatch", "create {label}");

        let (status, err) = send(&app, "GET", &format!("/v1/service-accounts?owner_prn={forged}"), None, Some(token.as_str())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "list {label}: {err}");
        assert_eq!(err["error"]["code"], "prn-mismatch", "list {label}");
    }

    // Review Focus 2: the correct PRN with upper-cased uuids succeeds over HTTP too, and the
    // answer carries the STORED lower-case PRN.
    let (status, ok) = send(&app, "POST", "/v1/service-accounts", Some(json!({ "owner_prn": prn_upper_uuids(&team.canonical()), "name": "i2-ok" })), Some(token.as_str())).await;
    assert_eq!(status, StatusCode::CREATED, "{ok}");
    assert_eq!(ok["owner_prn"], team.canonical());

    let (status, listed) = send(&app, "GET", &format!("/v1/service-accounts?owner_prn={}", team.canonical()), None, Some(token.as_str())).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    let items = listed.as_array().expect("list is a json array");
    assert_eq!(items.len(), 2, "i2-real and i2-ok only: {listed}");
    assert!(items.iter().all(|sa| sa["owner_prn"] == team.canonical()), "{listed}");
}
```

- [ ] **Step 3: Write I4**

```rust
/// SMA-646 I4 (spec §4.2, AC 11): `POST /v1/service-accounts/{id}/api-keys` refuses a forged
/// scope PRN, and no key is listed afterwards.
#[tokio::test]
async fn http_forged_scope_prn_is_refused() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, state, idp) = app_with_state(db).await;
    let token = idp.bearer("i4-admin", Some("i4-admin@example.com"), "paigasus", 3600);
    provision_platform_admin(&state, &token).await;
    let org = seed_org_ref(&state.db).await;
    let team = seed_team_ref(&state.db, &org).await;

    let (status, created) = send(&app, "POST", "/v1/service-accounts", Some(json!({ "owner_prn": team.canonical(), "name": "i4-bot" })), Some(token.as_str())).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let sa_id = created["prn"].as_str().unwrap().rsplit('/').next().unwrap().to_string();

    let absent_org = "00000000-0000-0000-0000-000000000f51";
    for (label, forged) in [("org slot", prn_with_org(&team.canonical(), absent_org)), ("region", prn_with_region(&team.canonical(), "eu-west-1"))] {
        let (status, err) = send(&app, "POST", &format!("/v1/service-accounts/{sa_id}/api-keys"), Some(json!({ "scope_prn": forged })), Some(token.as_str())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{label}: {err}");
        assert_eq!(err["error"]["code"], "prn-mismatch", "{label}");
    }

    let (status, listed) = send(&app, "GET", &format!("/v1/service-accounts/{sa_id}/api-keys"), None, Some(token.as_str())).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert!(listed.as_array().expect("list is a json array").is_empty(), "a refused issue must leave no key: {listed}");
}
```

- [ ] **Step 4: Run, lint and commit**

Run: `cd <worktree>/rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --no-fail-fast --test http_service_accounts`
Expected: all PASS, I2 and I4 included.

Run: `cd <worktree>/rs && cargo fmt -p paigasus-iam && cargo clippy -p paigasus-iam --all-targets --locked -- -D warnings`
Expected: no warning.

```bash
cd <worktree> && git branch --show-current   # must print feature/sma-646-service-account-stored-owner
git add rs/crates/services/paigasus-iam/tests/http_service_accounts.rs
git commit -m "test(rs): pin the stored owner and scope PRN over HTTP (SMA-646)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: Mutation proof (spec §4.3, AC 10) — no commit

Sven authorized these runs on 2026-09-28. For each row: make the exact edit with the Edit tool,
run the named tests with `--no-fail-fast`, record which tests went red, revert the exact edit with
the Edit tool, and confirm `git -C <worktree> diff --stat` is empty. Never use `git checkout --`.
Never commit a mutation. A rustc error means the mutation proved nothing: fix the mutation so
that it compiles (use `let _ = …;` for a discarded result) and run it again.

If the permission system refuses a mutation run, restore the file, and write the exact edit and
the expected red tests under "Mutation proof pending" in the PR-body notes. Then continue.

Commands:
- Unit: `cd <worktree>/rs && cargo nextest run -p paigasus-iam --lib --no-fail-fast -E 'test(/service_accounts::tests|api_keys::tests|tenancy_nodes::tests/)'`
- Integration: `cd <worktree>/rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --no-fail-fast --test api_keys_grpc --test http_service_accounts`

| # | exact edit | run | must go red (at least) |
|---|---|---|---|
| m1 | `create`: replace `let owner = self.nodes.resolve_and_authorize(&self.authorize, actor, Action::CreateServiceAccount, &owner, "CreateServiceAccount").await?;` with `self.authorize.check(actor, Action::CreateServiceAccount, &owner_resource_prn(&owner)).await?;` | unit + integration | U1, U7, I1, I2 |
| m2 | `list`: replace the helper line with `self.authorize.check(actor, Action::ListServiceAccounts, &owner_resource_prn(owner)).await?;` and change `list_by_owner(&owner, …)` to `list_by_owner(owner, …)` | unit + integration | U3, U7, I1, I2 |
| m3 | helper: change `authorize.check(actor, action, node_prn(&stored))` to `authorize.check(actor, action, node_prn(claimed))` | unit | U1, U9 |
| m4 | helper: move the three lines `let requested = …; let stored_canonical = …; if requested != stored_canonical { … }` above `authorize.check(…)` | unit | U5, U11 |
| m5 | `create`: delete the helper line at the top; after `let sa = ServiceAccount::new(id, owner, name, now)?;` change that line to `let sa = ServiceAccount::new(id, owner.clone(), name, now)?;` and add `let _ = self.nodes.resolve_and_authorize(&self.authorize, actor, Action::CreateServiceAccount, &owner, "CreateServiceAccount").await?;` below it | unit | U6 |
| m6 | helper: delete the `tracing::warn!(…);` line | unit | U8, U13 |
| m7 | `issue`: delete the step-4 line `let scope = self.nodes.resolve_and_authorize(…).await?;` (and its comment) | unit + integration | U9, U11, U12, I3, I4 |
| m8 | `issue`: delete the step-4 line; after `tx.commit().await?;` add `let _ = self.nodes.resolve_and_authorize(&self.authorize, actor, Action::IssueApiKey, &key.scope, "IssueApiKey").await?;` | unit | U9 (a key and an event exist) |
| m9 | helper: delete the line `authorize.check(actor, action, node_prn(&stored)).await?;` | unit | U5, U11 |

- [ ] **Step 1: Run m1 to m9 one at a time, as the table says.** After each revert, `git diff --stat` must show nothing.
- [ ] **Step 2: Record the result.** Write a table "mutation → red tests" into the notes for the PR body (not into a repo file). Every row must have at least one red test (AC 10). If a row stays green, stop: the test set has a gap. Add the missing assertion to the owning task's test, commit it as `test(rs): …`, then re-run the WHOLE battery (every row, not only the one that failed).
- [ ] **Step 3: Record the equivalent mutants** (spec §4.3) in the same notes, as "not run, equivalent by construction": return `claimed.clone()` after the compare; pass the caller's `owner` to `list_by_owner`; build the `ApiKey` with the caller's `scope`. They stay a code-review item (AC 6).
- [ ] **Step 4: Final check.** `git -C <worktree> status --short` shows no modified tracked file.

---

### Task 7: Full verification and the gate graph

**Files:** none changed, unless a gate finds a defect.

- [ ] **Step 1: The whole crate with Docker**

Run: `cd <worktree>/rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --profile iam`
Expected: all PASS. The spec §2.6 names `tests/api_key_auth.rs` and `tests/authz_acceptance.rs`:
each key issue there scopes to the SA's owner, where the actor holds `IssueApiKey`, so no change is
expected. If one fails with `forbidden` at an `IssueApiKey` call, the scope is outside the actor's
authority (K4): change the test's scope to the SA's owner and record why in the commit body. Do
not weaken the service.

- [ ] **Step 2: Workspace lint**

Run: `cd <worktree>/rs && cargo fmt --check && cargo clippy --workspace --all-targets --locked -- -D warnings`
Expected: no output from `fmt --check`, no warning from clippy.

- [ ] **Step 3: The registry-code gate**

Run: `cd <worktree> && moon run repo:error-code-single-site`
Expected: PASS. A red names a quoted registry code in one of the three `application/` files: replace
it with `TenancyError::…` or a backticked code.

- [ ] **Step 4: The full gate graph (root `CLAUDE.md`, "Before you push")**

Run the `moon ci … --base origin/main --include-relations` command between the `ci-targets`
markers of the root `CLAUDE.md`, with the proto shims first on `PATH`. On this Mac no single bash
runs every gate (root `CLAUDE.md`, "This development Mac only"). Read a gate that fails with an
empty stdout and a `declare`/`mapfile` stderr, or an rc 2 pipe-capacity message, as a host
artifact: re-run that gate directly under the right bash, or in a `docker run ubuntu:24.04`
container when the host is in the 512-byte small-pipe state (SMA-612). For any other unattributed
failure, follow the `moon-diagnosis` procedure of the root `CLAUDE.md`, starting with its Step 0.

Expected: every gate this change can reach is green. This change touches only `rs/`, so the
affected set is the `paigasus-iam` tasks plus the `repo:*` gates that select on everything.

- [ ] **Step 5: Linear description**

Read SMA-646 in Linear. If its description has no "Scope extended on 2026-09-28" section, append
one that says: `IssueApiKey`'s forged scope echo is folded into this issue (decision A5); the scope
is now confirmed against storage and `IssueApiKey` is authorized at the stored scope node (K4).
Do not change the issue state.

- [ ] **Step 6: Final state**

Run: `cd <worktree> && git status --short && git log --oneline origin/main..HEAD`
Expected: a clean tree (the untracked `.claude/` directory is not part of this change), and the
commits of Tasks 1–5 on top of the spec commit.

---

## Self-review record

- **Spec coverage.** §2.2/§3.1 helper → Task 1. §2.2 `create`/`list`, §2.3 B1–B7, U1–U8 → Task 2 (B7 is unchanged by construction: `Page::new` stays in the transports). §2.5/§3.2 K1–K6, U9–U13 → Task 3, plus U14 for AC 15. §3.3/§3.4 wiring → Tasks 2 and 3. §4.2 I1/I3 → Task 4, I2/I4 → Task 5. §4.3 m1–m9 and the equivalent mutants → Task 6. §4.4 local run, the gate graph, and the Linear note of A5 → Task 7. AC 6 (the stored node is what the records carry) is a code-review item: the code returns and uses `stored`.
- **Stale spec facts found while planning.** `api_keys.rs` has no `test_scope()` helper; the unit tests pass `owner_org(1)` as the scope, which is the SA's owner, so their only change is the seeded store. The `"scope"` payload line is 258 in this worktree, not 260. Neither changes the design.
- **Placeholder scan.** No TBD and no `todo!`. The one stub body is the deliberate red step of Task 1 (it compiles under `warnings = "deny"`).
- **Type consistency.** `TenancyNodes`, `resolve_and_authorize(&Authorize, &Prn, Action, &TenancyNodeRef, &'static str) -> Result<TenancyNodeRef, TenancyError>`, `tenancy_nodes(&TenancyStore)`, `forged_variants(&OrganizationId, &TeamId, &ProjectId)`, `seed_team_ref(&DatabaseConnection, &TenancyNodeRef)`, `prn_with_org`, `prn_with_region`, `prn_upper_uuids` are used with the same names and types in every task.
- **Review Focus.** Five lines, each with its test in the owning task.
