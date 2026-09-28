# SMA-649 — Confirm a principal PRN against storage (ListMemberships, GrantRole and ListRoleGrants) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `ListMemberships` with a principal filter, `GrantRole`, and `ListRoleGrants` with a principal filter refuse a principal PRN whose region or organization slot differs from the stored principal PRN (`prn-mismatch`), and answer `not-found` for an unknown principal uuid.

**Architecture:** `ListMemberships` goes through ONE guarded port method for both filters: `MembershipKindQuery::list_of_kind` takes the full `PrincipalId` and an `Option<PrincipalKind>`. The Postgres adapter confirms the principal PRN in the repository (`principal_list_uuid`, the twin of `node_list_sql`). `RoleService::grant` confirms the principal PRN in the application layer (`resolve_principal`, the twin of `resolve_scope`), through a new `PrincipalRepository` dependency. `RoleService::list` calls the same `resolve_principal` for a principal filter (spec §4.8, extended on 2026-09-28).

**Tech Stack:** Rust (edition 2024, rust-version 1.95), SeaORM over Postgres, tonic (gRPC), axum (HTTP), `cargo nextest`, Docker-backed Postgres tests (`tests/support`).

**Spec:** `docs/superpowers/specs/2026-09-27-sma-649-list-memberships-principal-prn-design.md` (approved 2026-09-27, with changes in its §0; extended on 2026-09-28 by Q6). Read it before you start a task. Section numbers below (§2.2, §4.7, §4.8, T1–T7, M1–M7, AC1–AC19) point into it.

## Global Constraints

- Work only in `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-649-list-memberships-principal-prn` on branch `feature/sma-649-list-memberships-principal-prn`. Start every shell command with `cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-649-list-memberships-principal-prn && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" &&`.
- Before each commit, `git branch --show-current` must print `feature/sma-649-list-memberships-principal-prn`.
- Never use `--no-verify`, `--no-gpg-sign`, `git commit --amend`, `git reset`, `git checkout -- <file>` or a force push. Do not start background jobs. Do not install host software.
- Conventional commits with a workspace scope (`fix(rs): …`, `test(rs): …`, `docs(rs): …`). End each commit message with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Do not put a `#NNN` or a `token: value` line in the commit body (commitlint `footer-leading-blank`).
- Every new source file opens with `// SPDX-License-Identifier: Apache-2.0`. This plan adds no new file.
- `rs/Cargo.toml` sets `warnings = "deny"`. Dead code and unused bindings are compile errors.
- `repo:error-code-single-site` scans every `.rs` file under a `src/` tree. Do not write a quoted registry code (`"prn-mismatch"`, `"not-found"`, `"forbidden"`, `"invalid-pagination"`) in any `src/` file, comments included. Unit tests in `src/` compare `TenancyError` variants. Files under `tests/` may quote codes (they do today).
- No log line on a principal `prn-mismatch` (spec Q2).
- An unknown principal uuid answers `not-found` (spec Q4, B4, G3). A forged organization slot answers `prn-mismatch`, not `invalid-prn` (spec Q5, B2).
- Do not change `MembershipRepository::list_by_principal` or `list_by_node` behaviour, and do not remove or rename any port method (SMA-719 owns that).
- Do not change `RoleService::revoke` (spec §5). `RoleService::list` changes only in Task 6 (spec §4.8). Do not change `RoleGrantStore::list_by_principal` or `RoleGrantQuery::find` (AC19).
- No proto change, no CLAUDE.md change (spec §4.5).
- Postgres tests skip silently when Docker is absent. Run every Postgres test with `CI=1` so that a missing Docker daemon fails instead of skipping. Check `docker info` first.
- A mutation (M1–M7) must COMPILE. Use the `if false && …` / `if false { … }` / `.filter(|_| false)` shapes given in the tasks, run with `cargo nextest run --no-fail-fast`, and restore each mutation with the Edit tool (never `git checkout --`, which also reverts the uncommitted fix).

## Review Focus

These six inputs follow from the spec, but no spec test names them. Each one has a test in the task that owns the code.

1. **Both slots forged at once** (`prn:pgs:iam:eu-west-1:<org>:principal/<real-uuid>`): expect `prn-mismatch`, not a pass because one check covers only one slot. Tests: Task 1 (`list_refuses_a_forged_principal_prn`, shape "both slots"), Task 4 (`grant_refuses_a_forged_principal_prn`, shape "both slots").
2. **An upper-case uuid together with a forged region**: expect `prn-mismatch`. A fix that lower-cases before it compares must not also drop the region. Tests: Task 1 (shape "upper-case uuid and a region"), Task 4 (same shape).
3. **A forged PRN with an offset past the last row**: expect `prn-mismatch`, not an empty OK list. The guard must run before paging. Test: Task 1 (`list_refuses_a_forged_principal_prn_past_the_last_page`).
4. **A service-account principal with a forged PRN and `principal_kind = service_account`**: expect `PrnMismatch` from Postgres. The guard must not depend on the kind. Test: Task 2 (`list_of_kind_confirms_the_principal_prn`, service-account block).
5. **A known principal with zero memberships**: expect an empty OK list, not `not-found`. The guard must tell "no principal" from "no rows". Test: Task 1 (`list_returns_an_empty_list_for_a_known_principal_without_memberships`) and Task 2 (the same case in Postgres).
6. **The caller's own uuid with a forged region on `ListRoleGrants`** (added 2026-09-28): expect `forbidden` for an ungranted caller, not a self listing. The self check must compare canonical forms. Tests: Task 6 (`list_with_a_forged_own_principal_prn_is_not_self`), Task 7 (the "own uuid, forged region" rows).

---

## File map

| file | task | change |
|---|---|---|
| `rs/crates/libs/paigasus-iam-core/src/ports.rs` | 1 | `MembershipAxis::Principal(PrincipalId)`; `list_of_kind` takes `Option<PrincipalKind>`; three doc comments |
| `rs/crates/services/paigasus-iam/src/adapters/persistence/pg_memberships.rs` | 1 | `principal_list_uuid`; guarded principal arm; `Option` kind; docs |
| `rs/crates/services/paigasus-iam/src/application/fakes.rs` | 1, 4 | Task 1: `InMemoryMemberships::list_of_kind` guard + `Option` kind. Task 4: new `InMemoryTenancyPrincipals` |
| `rs/crates/services/paigasus-iam/src/application/memberships.rs` | 1 | `list` routes through `list_of_kind`; docs; T1 unit tests |
| `rs/crates/services/paigasus-iam/tests/tenancy_memberships.rs` | 1, 2 | Task 1: one existing test to the new types. Task 2: T2 tests |
| `rs/crates/services/paigasus-iam/tests/grpc_tenancy.rs` | 3 | T3 gRPC tests |
| `rs/crates/services/paigasus-iam/tests/http_memberships.rs` | 3 | T3 HTTP tests |
| `rs/crates/services/paigasus-iam/src/adapters/grpc/tenancy.rs` | 3 | module doc only |
| `rs/crates/services/paigasus-iam/src/adapters/http/memberships.rs` | 3 | module doc only |
| `rs/crates/services/paigasus-iam/src/application/roles.rs` | 4 | `principals` dependency, `resolve_principal`, call in `grant`, docs, harness, T6 unit tests |
| `rs/crates/services/paigasus-iam/src/adapters/http/mod.rs` | 4 | `RoleServiceDeps.principals` wiring |
| `rs/crates/services/paigasus-iam/tests/authz_bootstrap.rs` | 4 | `RoleServiceDeps.principals` wiring |
| `rs/crates/services/paigasus-iam/tests/grpc_authz.rs` | 5, 7 | T6 gRPC test; T7 gRPC test |
| `rs/crates/services/paigasus-iam/tests/http_authz.rs` | 5, 7 | T6 HTTP test; T7 HTTP test |
| `rs/crates/services/paigasus-iam/src/application/roles.rs` | 6 | also: `resolve_principal` call in `list`, docs, harness seeds principal 1, T7 unit tests |

Before Task 1, confirm the implementer counts. Both commands must print exactly the lines shown in the spec (§8, §4.7):

```bash
git grep -n "impl MembershipKindQuery for"   # 2 lines: pg_memberships.rs, fakes.rs
git grep -n "RoleServiceDeps {"              # 3 lines: http/mod.rs, roles.rs, tests/authz_bootstrap.rs
```

If a count differs, update every extra site in the task that changes that type, and record it in the task report.

---

### Task 1: One guarded listing port for ListMemberships

**Files:**
- Modify: `rs/crates/libs/paigasus-iam-core/src/ports.rs:220-243`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/persistence/pg_memberships.rs:1-8` (module doc), `:376-428`
- Modify: `rs/crates/services/paigasus-iam/src/application/fakes.rs:643-653`
- Modify: `rs/crates/services/paigasus-iam/src/application/memberships.rs:40-48` (doc), `:245-259`, test module (append tests)
- Modify: `rs/crates/services/paigasus-iam/tests/tenancy_memberships.rs:419-442`

**Interfaces:**
- Consumes: nothing from other tasks.
- Produces (Tasks 2 and 3 rely on these exact shapes):
  - `pub enum MembershipAxis { Principal(PrincipalId), Node(TenancyNodeRef) }`
  - `MembershipKindQuery::list_of_kind(&self, axis: &MembershipAxis, kind: Option<PrincipalKind>, limit: u64, offset: u64) -> Result<Vec<MembershipRecord>, RepositoryError>`
  - `PgMembershipRepository::principal_list_uuid(&self, principal: &PrincipalId) -> Result<Uuid, RepositoryError>` (private)
  - `MembershipService::list` answers `TenancyError::PrnMismatch` / `TenancyError::NotFound` for a principal filter, through `self.kinds.list_of_kind`.

- [ ] **Step 1: Write the failing unit tests (T1 and Review Focus 1, 2, 3, 5)**

Append these tests at the end of `mod tests` in `rs/crates/services/paigasus-iam/src/application/memberships.rs` (before the closing `}` of the module). They use only the service API, so they compile before the fix.

```rust
    /// SMA-649: a principal PRN with `region` and `org` in the two slots, and `principal`'s uuid.
    /// A stored principal PRN has both slots empty, so any non-empty slot is a forgery.
    fn forged_principal(principal: &PrincipalId, region: &str, org: &str) -> String {
        format!("prn:pgs:iam:{region}:{org}:principal/{}", principal.uuid())
    }

    /// Seeds one `User` principal with one org membership, and returns the service, the
    /// principal and the org uuid. `principal_kinds` is set, so the kind-set path
    /// (`Only(User)`) keeps the row: without it the control list of that path is empty and
    /// proves nothing (spec T1).
    async fn one_member(principal_n: u128, org_n: u128) -> (MembershipService<InMemoryMemberships, SeqIds, FixedClock>, PrincipalId, Uuid) {
        let store = TenancyStore::default();
        let now = Utc.timestamp_opt(0, 0).unwrap();
        let person = seed_principal(&store, principal_n);
        store.principal_kinds.lock().unwrap().insert(person.uuid(), PrincipalKind::User);
        let (org, _team) = seed_org_and_team(&store, org_n, org_n + 1, now);
        let svc = new_service(store);
        svc.attach(&person.canonical(), &OrganizationId::from_uuid(org).canonical(), &actor(999)).await.unwrap();
        (svc, person, org)
    }

    /// SMA-649 T1 (B1, B2, Review Focus 1 and 2): a forged region or organization slot on the
    /// principal filter answers `PrnMismatch`, with and without a kind. The control in the same
    /// test lists the seeded row, so the refusal cannot pass because `list` is broken for every
    /// input.
    #[tokio::test]
    async fn list_refuses_a_forged_principal_prn() {
        // 0xabc0: the uuid must contain letters, or the upper-case shape changes nothing.
        let (svc, person, org) = one_member(0xabc0, 500).await;
        let page = Page::new(None, None).unwrap();
        let random_org = Uuid::from_u128(0x0f49).to_string();
        let upper_uuid = person.uuid().to_string().to_uppercase();
        let shapes = [
            ("non-empty region", forged_principal(&person, "eu-west-1", "")),
            ("real org uuid in the org slot", forged_principal(&person, "", &org.to_string())),
            ("random org uuid in the org slot", forged_principal(&person, "", &random_org)),
            ("both slots", forged_principal(&person, "eu-west-1", &random_org)),
            ("upper-case uuid and a region", format!("prn:pgs:iam:eu-west-1::principal/{upper_uuid}")),
        ];
        for kind in [PrincipalKindFilter::Any, PrincipalKindFilter::Only(PrincipalKind::User)] {
            let control = svc.list(MembershipFilter::Principal(person.canonical()), kind, page).await.unwrap();
            assert_eq!(control.len(), 1, "{kind:?}: the canonical prn must list the seeded membership");
            for (shape, prn) in &shapes {
                let err = svc.list(MembershipFilter::Principal(prn.clone()), kind, page).await.unwrap_err();
                assert_eq!(err, TenancyError::PrnMismatch, "{kind:?} {shape}");
            }
        }
    }

    /// SMA-649 T1 (B4): a canonical principal PRN whose uuid no principal has answers
    /// `NotFound`, with and without a kind — not an empty OK list.
    #[tokio::test]
    async fn list_answers_not_found_for_an_unknown_principal() {
        let (svc, _person, _org) = one_member(51, 510).await;
        let page = Page::new(None, None).unwrap();
        let unknown = PrincipalId::from_prn(Prn::build("iam", "", None, "principal", Uuid::from_u128(0x0f4a)).unwrap()).canonical();
        for kind in [PrincipalKindFilter::Any, PrincipalKindFilter::Only(PrincipalKind::User)] {
            let err = svc.list(MembershipFilter::Principal(unknown.clone()), kind, page).await.unwrap_err();
            assert_eq!(err, TenancyError::NotFound, "{kind:?}");
        }
    }

    /// SMA-649 T1 (B3): the canonical PRN with an upper-case uuid is still correct. The guard
    /// compares canonical forms, never the raw request string.
    #[tokio::test]
    async fn list_accepts_an_upper_case_uuid_in_a_correct_principal_prn() {
        let (svc, person, _org) = one_member(0xab, 520).await;
        let page = Page::new(None, None).unwrap();
        let upper = format!("prn:pgs:iam:::principal/{}", person.uuid().to_string().to_uppercase());
        assert_ne!(upper, person.canonical(), "the test uuid must contain letters, or the case changes nothing");
        for kind in [PrincipalKindFilter::Any, PrincipalKindFilter::Only(PrincipalKind::User)] {
            assert_eq!(svc.list(MembershipFilter::Principal(upper.clone()), kind, page).await.unwrap().len(), 1, "{kind:?}");
        }
    }

    /// SMA-649 Review Focus 3: the guard runs before paging. A forged PRN with an offset past
    /// the last row answers `PrnMismatch`, not an empty OK list.
    #[tokio::test]
    async fn list_refuses_a_forged_principal_prn_past_the_last_page() {
        let (svc, person, _org) = one_member(53, 530).await;
        let past_the_end = Page::new(Some(10), Some(1_000)).unwrap();
        let forged = forged_principal(&person, "eu-west-1", "");
        for kind in [PrincipalKindFilter::Any, PrincipalKindFilter::Only(PrincipalKind::User)] {
            let err = svc.list(MembershipFilter::Principal(forged.clone()), kind, past_the_end).await.unwrap_err();
            assert_eq!(err, TenancyError::PrnMismatch, "{kind:?}");
        }
    }

    /// SMA-649 Review Focus 5: a known principal with no membership lists an empty OK list.
    /// The guard tells "no principal" (`NotFound`) from "no rows".
    #[tokio::test]
    async fn list_returns_an_empty_list_for_a_known_principal_without_memberships() {
        let store = TenancyStore::default();
        let lonely = seed_principal(&store, 54);
        store.principal_kinds.lock().unwrap().insert(lonely.uuid(), PrincipalKind::User);
        let svc = new_service(store);
        let page = Page::new(None, None).unwrap();
        for kind in [PrincipalKindFilter::Any, PrincipalKindFilter::Only(PrincipalKind::User)] {
            assert!(svc.list(MembershipFilter::Principal(lonely.canonical()), kind, page).await.unwrap().is_empty(), "{kind:?}");
        }
    }
```

`Page::new` takes `Option<i64>` for both arguments and refuses a `limit` above 200 (`application/pagination.rs`). An offset of 1 000 is valid.

- [ ] **Step 2: Run the new tests and confirm that they fail for the right reason**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-649-list-memberships-principal-prn/rs && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cargo nextest run -p paigasus-iam --lib --no-fail-fast -E 'test(/application::memberships::tests::list_/)'
```

Expected: `list_refuses_a_forged_principal_prn`, `list_answers_not_found_for_an_unknown_principal` and `list_refuses_a_forged_principal_prn_past_the_last_page` FAIL (today they get `Ok(rows)` or `Ok([])` where the test expects an error). `list_accepts_an_upper_case_uuid_in_a_correct_principal_prn`, `list_returns_an_empty_list_for_a_known_principal_without_memberships` and the two existing `list_` tests PASS. If a new test fails with a different message (a panic in setup, for example), fix the test first.

- [ ] **Step 3: Change the port (`ports.rs`)**

In `rs/crates/libs/paigasus-iam-core/src/ports.rs`, replace the `list_by_principal` line of `MembershipRepository` (line 222) with:

```rust
    /// Filters on a bare uuid and does NOT confirm a PRN. For callers that hold a
    /// server-resolved `PrincipalId` only (authn introspection,
    /// `principal_context::load_all_memberships`). A PRN from the wire goes through
    /// [`MembershipKindQuery::list_of_kind`], which confirms it against storage (SMA-649).
    async fn list_by_principal(&self, principal: Uuid, limit: u64, offset: u64) -> Result<Vec<MembershipRecord>, RepositoryError>;
```

Replace the `MembershipAxis` enum and the `MembershipKindQuery` trait (lines 227-243) with:

```rust
/// Which axis a membership listing reads: one principal's memberships, or one node's. The
/// principal arm carries the full `PrincipalId`, not a bare uuid, so the repository can
/// confirm the supplied PRN against the stored one (SMA-649).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MembershipAxis {
    Principal(PrincipalId),
    Node(TenancyNodeRef),
}

/// Read port: the wire membership listing (SMA-676 D8, SMA-649). A separate port, not a new
/// [`MembershipRepository`] method: that trait has six implementations, five of them test
/// fakes (the rule `authz::ports::SystemPolicyReconciler` records).
#[async_trait]
pub trait MembershipKindQuery: Send + Sync {
    /// The listing that both transports use for every `ListMemberships` request. Order
    /// `created_at, id`, with `limit`/`offset` paging. Both axes confirm the supplied PRN
    /// against storage before any row is read: an absent principal or node answers
    /// `NotFound`, and a stored PRN that differs from the canonical form of the supplied PRN
    /// answers `PrnMismatch`. `kind = None` keeps every kind; `Some(kind)` keeps only
    /// memberships whose principal is of that kind.
    async fn list_of_kind(&self, axis: &MembershipAxis, kind: Option<PrincipalKind>, limit: u64, offset: u64) -> Result<Vec<MembershipRecord>, RepositoryError>;
}
```

`PrincipalId` is already imported in `ports.rs` (`use crate::value::{PrincipalId, Stamp};`).

- [ ] **Step 4: Guard the Postgres adapter (`pg_memberships.rs`)**

In the second `impl PgMembershipRepository` block, directly after `node_list_sql` (ends near line 403), add:

```rust
    /// SMA-649: the principal twin of [`Self::node_list_sql`]. Loads the principal by uuid with
    /// no lock (a plain listing, not a guarded mutation), answers `NotFound` when it is absent
    /// and `PrnMismatch` when the stored `prn` differs from the canonical form of the supplied
    /// PRN (a forged region or organization slot). Returns the uuid to bind as `$1`. No log
    /// line, the same as the node guard (spec Q2).
    async fn principal_list_uuid(&self, principal: &PrincipalId) -> Result<Uuid, RepositoryError> {
        let stored = principal::Entity::find_by_id(principal.uuid()).one(&self.db).await.map_err(map_err)?.ok_or(RepositoryError::NotFound)?;
        if stored.prn != principal.canonical() {
            return Err(RepositoryError::PrnMismatch);
        }
        Ok(stored.id)
    }
```

Replace the whole `MembershipKindQuery` impl (the doc line `/// SMA-676 D8: the same four statements, with `$4` bound to the kind.` and the `impl` below it) with:

```rust
/// SMA-676 D8, SMA-649: the same four statements, with `$4` bound to the kind (NULL = any
/// kind). Both arms run their guard first: `principal_list_uuid` for a principal,
/// `node_list_sql` for a node.
#[async_trait]
impl MembershipKindQuery for PgMembershipRepository {
    async fn list_of_kind(&self, axis: &MembershipAxis, kind: Option<PrincipalKind>, limit: u64, offset: u64) -> Result<Vec<MembershipRecord>, RepositoryError> {
        match axis {
            MembershipAxis::Principal(principal) => {
                let principal_uuid = self.principal_list_uuid(principal).await?;
                self.list_rows(LIST_BY_PRINCIPAL_SQL, principal_uuid, kind, limit, offset).await
            }
            MembershipAxis::Node(node) => {
                let (sql, node_uuid) = self.node_list_sql(node).await?;
                self.list_rows(sql, node_uuid, kind, limit, offset).await
            }
        }
    }
}
```

At the end of the module doc (after line 8, `//! principal's team/project memberships in that org (rule 5), also in one transaction.`), add:

```rust
//!
//! **Listing guards (SMA-649).** `MembershipKindQuery::list_of_kind` is the wire listing. It
//! confirms the supplied PRN against the stored row on both axes before any row is read:
//! `node_list_sql` for a node, `principal_list_uuid` for a principal (`NotFound` if absent,
//! `PrnMismatch` on a difference). `MembershipRepository::list_by_principal` filters on a bare
//! uuid and is for server-resolved ids only (authn introspection).
```

The `principal` entity and `PrincipalId` are already imported in this file.

- [ ] **Step 5: Guard the in-memory fake (`fakes.rs`)**

In `rs/crates/services/paigasus-iam/src/application/fakes.rs`, replace the `impl MembershipKindQuery for InMemoryMemberships` block with:

```rust
/// SMA-649: faithful to the port doc. The principal arm runs the same guard `attach_in`
/// applies in this fake (`store.principals`: absent -> `NotFound`, stored PRN differs from the
/// supplied canonical PRN -> `PrnMismatch`); the node arm reuses `list_by_node`'s guard.
/// `kind = None` keeps every row, also for a principal that has no `principal_kinds` entry.
#[async_trait]
impl MembershipKindQuery for InMemoryMemberships {
    async fn list_of_kind(&self, axis: &MembershipAxis, kind: Option<PrincipalKind>, limit: u64, offset: u64) -> Result<Vec<MembershipRecord>, RepositoryError> {
        let all = match axis {
            MembershipAxis::Principal(principal) => {
                let stored = { self.0.principals.lock().unwrap().get(&principal.uuid()).cloned() }.ok_or(RepositoryError::NotFound)?;
                if stored != principal.canonical() {
                    return Err(RepositoryError::PrnMismatch);
                }
                self.list_by_principal(principal.uuid(), u64::MAX, 0).await?
            }
            MembershipAxis::Node(node) => self.list_by_node(node, u64::MAX, 0).await?,
        };
        let Some(kind) = kind else {
            return Ok(all.into_iter().skip(offset as usize).take(limit as usize).collect());
        };
        let kinds = self.0.principal_kinds.lock().unwrap().clone();
        let of_kind = |r: &MembershipRecord| Prn::parse(&r.principal_prn).ok().map(|p| PrincipalId::from_prn(p).uuid()).and_then(|u| kinds.get(&u).copied()) == Some(kind);
        Ok(all.into_iter().filter(of_kind).skip(offset as usize).take(limit as usize).collect())
    }
}
```

- [ ] **Step 6: Route the service through the guarded port (`memberships.rs`)**

Replace the `list` method (doc and body) with:

```rust
    /// Lists memberships by principal or node, `ORDER BY created_at, id` (design doc §5.1
    /// rule 9). SMA-676 D8: `kind` AND-s with the filter; an unknown kind is refused first
    /// (D7). SMA-649: both axes go through the one guarded port,
    /// `MembershipKindQuery::list_of_kind`, which confirms the supplied principal or node PRN
    /// against storage (`NotFound`, `PrnMismatch`) before it reads a row.
    pub async fn list(&self, filter: MembershipFilter, kind: PrincipalKindFilter, page: Page) -> Result<Vec<MembershipRecord>, TenancyError> {
        let kind = kind.resolve()?;
        let axis = match filter {
            MembershipFilter::Principal(raw) => MembershipAxis::Principal(parse_principal_prn(&raw)?),
            MembershipFilter::Node(raw) => MembershipAxis::Node(parse_node_prn(&raw)?),
        };
        Ok(self.kinds.list_of_kind(&axis, kind, page.limit, page.offset).await?)
    }
```

Replace the doc of `parse_principal_prn` (the three `///` lines above it) with:

```rust
/// Parses a raw principal PRN string: must be syntactically valid (else `InvalidPrn` with
/// the kernel's stable error-kind token), and must be service `"iam"`, resource type
/// `"principal"` (else `InvalidPrn` with the PRN's canonical form). It checks ONLY the syntax,
/// the service and the type. The region and the organization slot are confirmed against the
/// stored principal by the repository (`attach_in`, `MembershipKindQuery::list_of_kind`,
/// SMA-649).
```

`self.repo` stays in use (`attach`, `detach`, `get`). If rustc reports an unused import of `MembershipRepository` or anything else, remove only that import.

- [ ] **Step 7: Update the one Postgres test that uses the old types**

In `rs/crates/services/paigasus-iam/tests/tenancy_memberships.rs`, in `list_of_kind_keeps_only_members_of_that_kind_on_both_axes`:

- Replace its doc comment (two lines above `#[tokio::test]`) with:

```rust
/// SMA-676 D8 against Postgres: `list_of_kind` keeps only members of that kind, on the node
/// axis and on the principal axis. The guards of both axes are pinned by
/// `list_of_kind_confirms_the_node_prn` and `list_of_kind_confirms_the_principal_prn`.
```

- Change the three `list_of_kind` calls to pass `Some(PrincipalKind::User)` / `Some(PrincipalKind::ServiceAccount)`, and the principal axis to `MembershipAxis::Principal(bot.clone())`:

```rust
    let users = repo.list_of_kind(&at_org, Some(PrincipalKind::User), 200, 0).await.unwrap();
    assert_eq!(users.iter().map(|r| r.principal_prn.clone()).collect::<Vec<_>>(), vec![person.canonical()]);
    let bots = repo.list_of_kind(&at_org, Some(PrincipalKind::ServiceAccount), 200, 0).await.unwrap();
    assert_eq!(bots.iter().map(|r| r.principal_prn.clone()).collect::<Vec<_>>(), vec![bot.canonical()]);
    assert!(repo.list_of_kind(&MembershipAxis::Principal(bot.clone()), Some(PrincipalKind::User), 200, 0).await.unwrap().is_empty());
```

- [ ] **Step 8: Build and run the unit tests**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-649-list-memberships-principal-prn/rs && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cargo build --workspace --all-targets && cargo nextest run -p paigasus-iam --lib --no-fail-fast
```

Expected: the build succeeds with no warning; every `--lib` test passes, the five new ones included. `principal_context` tests pass unchanged (AC6).

- [ ] **Step 9: Run the changed Postgres test**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-649-list-memberships-principal-prn/rs && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && docker info >/dev/null && CI=1 cargo nextest run -p paigasus-iam --test tenancy_memberships --no-fail-fast
```

Expected: every test in `tenancy_memberships` passes.

- [ ] **Step 10: Mutation M2 (fake guard) — must red T1**

With the Edit tool, in `fakes.rs` `InMemoryMemberships::list_of_kind`, change `if stored != principal.canonical() {` to `if false && stored != principal.canonical() {`. Run:

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-649-list-memberships-principal-prn/rs && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cargo nextest run -p paigasus-iam --lib --no-fail-fast -E 'test(/application::memberships::tests::list_/)'
```

Expected: the tree compiles, and `list_refuses_a_forged_principal_prn` and `list_refuses_a_forged_principal_prn_past_the_last_page` FAIL. Restore the line with the Edit tool (remove `false && `).

- [ ] **Step 11: Mutation M3 (service routing) — must red T1**

With the Edit tool, in `MembershipService::list`, insert this line directly before `Ok(self.kinds.list_of_kind(…`:

```rust
        if let (MembershipAxis::Principal(p), None) = (&axis, kind) { return Ok(self.repo.list_by_principal(p.uuid(), page.limit, page.offset).await?); } // MUTATION M3
```

Run the command of Step 10. Expected: the tree compiles, and the kind-`Any` cases of `list_refuses_a_forged_principal_prn`, `list_answers_not_found_for_an_unknown_principal` and `list_refuses_a_forged_principal_prn_past_the_last_page` FAIL. Delete the marked line with the Edit tool.

- [ ] **Step 12: Format, lint, and commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-649-list-memberships-principal-prn/rs && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo nextest run -p paigasus-iam --lib && git diff --stat && git -C .. branch --show-current
```

Expected: clippy is clean, tests pass, `git diff --stat` shows only the five files of this task, and the branch is `feature/sma-649-list-memberships-principal-prn`. Then:

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-649-list-memberships-principal-prn && git add rs/crates/libs/paigasus-iam-core/src/ports.rs rs/crates/services/paigasus-iam/src/adapters/persistence/pg_memberships.rs rs/crates/services/paigasus-iam/src/application/fakes.rs rs/crates/services/paigasus-iam/src/application/memberships.rs rs/crates/services/paigasus-iam/tests/tenancy_memberships.rs && git commit -m "fix(rs): confirm the principal PRN of ListMemberships against storage (SMA-649)

Both membership filters now list through MembershipKindQuery::list_of_kind,
which confirms the supplied principal or node PRN against the stored row.
A forged region or organization slot answers prn-mismatch; an unknown
principal uuid answers not-found.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: Postgres tests for both listing guards (T2)

**Files:**
- Modify: `rs/crates/services/paigasus-iam/tests/tenancy_memberships.rs` (append two tests)

**Interfaces:**
- Consumes (Task 1): `MembershipAxis::Principal(PrincipalId)`, `list_of_kind(&axis, Option<PrincipalKind>, limit, offset)`, the existing helpers `seed_user`, `seed_chain`, `seed_service_account_principal`, `membership_at`, `stamp_of` in this file.
- Produces: `list_of_kind_confirms_the_principal_prn`, `list_of_kind_confirms_the_node_prn` (Task 8 re-runs them in the mutation battery).

These tests pass as soon as they compile, because Task 1 already added the guards. Their proof is the mutations in Steps 3 and 4, not a red run first.

- [ ] **Step 1: Write the two tests**

Append to `rs/crates/services/paigasus-iam/tests/tenancy_memberships.rs`:

```rust
/// A principal id with `region` and `org` in the two slots and `principal`'s uuid. A stored
/// principal PRN has both slots empty (`prn:pgs:iam:::principal/<uuid>`).
fn forged_principal(principal: &PrincipalId, region: &str, org: &str) -> PrincipalId {
    PrincipalId::from_prn(Prn::parse(&format!("prn:pgs:iam:{region}:{org}:principal/{}", principal.uuid())).unwrap())
}

/// SMA-649 T2: `list_of_kind` confirms a principal PRN against the stored `principal` row, for
/// `kind = None` and `kind = Some(_)`. The unit tests prove only the fake; this proves the SQL
/// helper `principal_list_uuid`. Also Review Focus 4 (a service account, filtered by its own
/// kind) and Review Focus 5 (a known principal with no membership lists an empty OK list).
#[tokio::test]
async fn list_of_kind_confirms_the_principal_prn() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let ids = KernelIdGenerator;
    let clock = SystemClock;
    let (org, _team, _project) = seed_chain(&db).await;
    let person = seed_user(&db, 81).await;
    let bot = seed_service_account_principal(&db, mint_uuid7(1_700_000_000_600, [82u8; 10])).await;
    let lonely = seed_user(&db, 83).await;
    let repo = PgMembershipRepository::new(db.clone());
    for p in [&person, &bot] {
        let m = membership_at(&ids, p, TenancyNodeRef::Organization(org.id.clone()), clock.now());
        repo.attach(&m, &stamp_of(&m)).await.unwrap();
    }
    let random_org = Uuid::from_u128(0x0f49).to_string();
    let real_org = org.id.uuid().to_string();

    for (who, principal, kind) in [("user, kind None", &person, None), ("user, kind User", &person, Some(PrincipalKind::User)), ("service account, kind ServiceAccount", &bot, Some(PrincipalKind::ServiceAccount))] {
        for (shape, forged) in [
            ("non-empty region", forged_principal(principal, "eu-west-1", "")),
            ("real org uuid in the org slot", forged_principal(principal, "", &real_org)),
            ("random org uuid in the org slot", forged_principal(principal, "", &random_org)),
        ] {
            let err = repo.list_of_kind(&MembershipAxis::Principal(forged), kind, 200, 0).await.unwrap_err();
            assert!(matches!(err, RepositoryError::PrnMismatch), "{who} {shape}: got {err:?}");
        }
        let rows = repo.list_of_kind(&MembershipAxis::Principal(principal.clone()), kind, 200, 0).await.unwrap();
        assert_eq!(rows.len(), 1, "{who}: the canonical prn must list the one seeded membership");
        assert_eq!(rows[0].node_prn, org.id.canonical(), "{who}");
    }

    let unknown = PrincipalId::from_prn(Prn::build("iam", "", None, "principal", Uuid::from_u128(0x0f4a)).unwrap());
    for kind in [None, Some(PrincipalKind::User)] {
        let err = repo.list_of_kind(&MembershipAxis::Principal(unknown.clone()), kind, 200, 0).await.unwrap_err();
        assert!(matches!(err, RepositoryError::NotFound), "unknown principal, {kind:?}: got {err:?}");
        let rows = repo.list_of_kind(&MembershipAxis::Principal(lonely.clone()), kind, 200, 0).await.unwrap();
        assert!(rows.is_empty(), "a known principal with no membership lists an empty OK list, {kind:?}");
    }
}

/// SMA-649 T2 / AC10: the node guard of `list_of_kind`. No test pinned it before, and since
/// SMA-649 it carries every node-filtered `ListMemberships` (kind set or not). Under
/// `enforce_tenancy = true` the handler authorizes against the REAL node, found by uuid, so
/// this compare is the only defense against a forged node org slot.
#[tokio::test]
async fn list_of_kind_confirms_the_node_prn() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let ids = KernelIdGenerator;
    let clock = SystemClock;
    let (org, team, _project) = seed_chain(&db).await;
    let person = seed_user(&db, 84).await;
    let repo = PgMembershipRepository::new(db.clone());
    for node in [TenancyNodeRef::Organization(org.id.clone()), TenancyNodeRef::Team(team.id.clone())] {
        let m = membership_at(&ids, &person, node, clock.now());
        repo.attach(&m, &stamp_of(&m)).await.unwrap();
    }
    let random_org = Uuid::from_u128(0x0f4b);
    let forged_team = TenancyNodeRef::Team(TeamId::from_parts(random_org, team.id.uuid()));
    let forged_org = TenancyNodeRef::Organization(
        paigasus_iam_core::OrganizationId::from_prn(Prn::parse(&format!("prn:pgs:iam:eu-west-1::organization/{}", org.id.uuid())).unwrap()).unwrap(),
    );
    let unknown_team = TenancyNodeRef::Team(TeamId::from_parts(org.id.uuid(), Uuid::from_u128(0x0f4c)));

    for kind in [None, Some(PrincipalKind::User)] {
        for (shape, node) in [("team with a forged org slot", &forged_team), ("organization with a forged region", &forged_org)] {
            let err = repo.list_of_kind(&MembershipAxis::Node(node.clone()), kind, 200, 0).await.unwrap_err();
            assert!(matches!(err, RepositoryError::PrnMismatch), "{shape}, {kind:?}: got {err:?}");
        }
        let err = repo.list_of_kind(&MembershipAxis::Node(unknown_team.clone()), kind, 200, 0).await.unwrap_err();
        assert!(matches!(err, RepositoryError::NotFound), "unknown team, {kind:?}: got {err:?}");
        for node in [TenancyNodeRef::Organization(org.id.clone()), TenancyNodeRef::Team(team.id.clone())] {
            let rows = repo.list_of_kind(&MembershipAxis::Node(node.clone()), kind, 200, 0).await.unwrap();
            assert_eq!(rows.len(), 1, "the canonical {node:?} must list the seeded membership, {kind:?}");
        }
    }
}
```

Notes for the implementer:
- `OrganizationId::from_prn` returns `Result`. It refuses an org SLOT on an organization PRN (`tenancy::check`) but does not look at the region, so the region forgery parses and reaches the compare (spec §2). If it does not parse, stop and report: the spec's premise is then wrong.
- `RepositoryError` does not derive `PartialEq`, so the tests use `matches!`. If it does derive it, `assert_eq!` is also fine.
- `seed_user` seeds are unique per test file run; the seeds 81–84 are unused elsewhere in this file (check with `grep -n "seed_user(&db, 8" tests/tenancy_memberships.rs`).

- [ ] **Step 2: Run the two tests**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-649-list-memberships-principal-prn/rs && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && docker info >/dev/null && CI=1 cargo nextest run -p paigasus-iam --test tenancy_memberships --no-fail-fast -E 'test(/list_of_kind_confirms/)'
```

Expected: both PASS.

- [ ] **Step 3: Mutation M1 (Postgres principal guard) — must red the principal test**

With the Edit tool, in `pg_memberships.rs` `principal_list_uuid`, change `if stored.prn != principal.canonical() {` to `if false && stored.prn != principal.canonical() {`. Run the command of Step 2. Expected: the tree compiles; `list_of_kind_confirms_the_principal_prn` FAILS (forged shapes list rows); `list_of_kind_confirms_the_node_prn` PASSES. Restore with the Edit tool.

- [ ] **Step 4: Mutation M4 (Postgres node guard) — must red the node test**

With the Edit tool, in `pg_memberships.rs` `node_list_sql`, change `if stored != node.canonical() {` to `if false && stored != node.canonical() {`. Run the command of Step 2. Expected: the tree compiles; `list_of_kind_confirms_the_node_prn` FAILS on the forged cases (the unknown-team case stays green, because `ok_or(NotFound)` is not mutated). Restore with the Edit tool.

- [ ] **Step 5: Format and commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-649-list-memberships-principal-prn/rs && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && git diff --stat && git -C .. branch --show-current
```

Expected: only `tests/tenancy_memberships.rs` changed (no mutation left in `pg_memberships.rs`). Then:

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-649-list-memberships-principal-prn && git add rs/crates/services/paigasus-iam/tests/tenancy_memberships.rs && git commit -m "test(rs): pin both list_of_kind PRN guards against Postgres (SMA-649)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: ListMemberships transport tests (T3) and the module docs

**Files:**
- Modify: `rs/crates/services/paigasus-iam/tests/grpc_tenancy.rs` (append two tests)
- Modify: `rs/crates/services/paigasus-iam/tests/http_memberships.rs` (append helpers and two tests)
- Modify: `rs/crates/services/paigasus-iam/src/adapters/grpc/tenancy.rs:30-39` (module doc)
- Modify: `rs/crates/services/paigasus-iam/src/adapters/http/memberships.rs:1-23` (module doc)

**Interfaces:**
- Consumes (Task 1): `ListMemberships` answers `prn-mismatch` (gRPC `InvalidArgument`, HTTP 400) and `not-found` (gRPC `NotFound`, HTTP 404) for a principal filter. Existing helpers in `grpc_tenancy.rs`: `two_states`, `spawn_tenancy_server`, `connect`, `authed`, `create_org`, `with_org`, `with_region`, `upper_uuid`, `reason`, `check`. In `http_memberships.rs`: `create_user`, `support::{app_with_state, app_with_config, test_config, provision, provision_platform_admin, send}`.
- Produces: `a_forged_principal_prn_never_lists_memberships` (gRPC), `an_ungranted_caller_cannot_list_memberships_by_any_principal_prn` (gRPC), `a_forged_principal_prn_never_lists_memberships_over_http`, `an_ungranted_caller_cannot_list_memberships_by_any_principal_prn_over_http`.

The fix is already in place (Task 1), so these tests pass on first run. Their proof is the mutation run in Step 5.

- [ ] **Step 1: Write the gRPC tests**

Append to `rs/crates/services/paigasus-iam/tests/grpc_tenancy.rs`. `AttachMembershipRequest`, `ListMembershipsRequest`, `list_memberships_request`, `ProtoPrincipalKind`, `NewUser` and `Uuid` are already imported by this file; add any import that rustc reports missing.

```rust
/// SMA-649 T3 (gRPC): `ListMemberships` with a PRINCIPAL filter confirms the PRN against the
/// stored principal, under both `enforce_tenancy` settings, with `principal_kind` unset and
/// `USER`. A forged region or organization slot answers `prn-mismatch`; an unknown uuid answers
/// `not-found`; an unknown uuid with an out-of-range `limit` still answers
/// `invalid-pagination` (B8, `to_page` runs before the service). Controls: the canonical PRN
/// and its upper-case-uuid form list the seeded membership (B3).
#[tokio::test]
async fn a_forged_principal_prn_never_lists_memberships() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let (enforced, unenforced) = two_states(&db, &idp).await;
    let token = idp.bearer("forged-lm", Some("forged-lm@example.com"), "paigasus", 3600);
    support::provision_platform_admin(&enforced, &token).await;
    let alice = enforced
        .users
        .execute(NewUser {
            email: "lm-alice@example.com".to_string(),
            display_name: "Alice".to_string(),
            locale: None,
            timezone: None,
        })
        .await
        .unwrap()
        .canonical();
    let (on_addr, on_server) = spawn_tenancy_server(enforced).await;
    let (off_addr, off_server) = spawn_tenancy_server(unenforced).await;
    let mut on = connect(on_addr).await;
    let mut off = connect(off_addr).await;

    let org = create_org(&mut on, &token, "lm-org", "List Memberships").await;
    on.attach_membership(authed(
        AttachMembershipRequest {
            principal_prn: alice.clone(),
            node_prn: org.prn.clone(),
        },
        &token,
    ))
    .await
    .expect("attach alice to the org");

    let real_org = org.prn.rsplit('/').next().expect("org prn ends in /<uuid>").to_string();
    let absent_org = Uuid::from_u128(0x0f49).as_hyphenated().to_string();
    let unknown = format!("prn:pgs:iam:::principal/{}", Uuid::from_u128(0x0f4a).as_hyphenated());
    let mut failures: Vec<String> = Vec::new();

    for (setting, client) in [("enforce=on", &mut on), ("enforce=off", &mut off)] {
        for (kind_label, kind) in [("kind unset", ProtoPrincipalKind::Unspecified as i32), ("kind USER", ProtoPrincipalKind::User as i32)] {
            let list = |prn: String, limit: u32| {
                authed(
                    ListMembershipsRequest {
                        filter: Some(list_memberships_request::Filter::PrincipalPrn(prn)),
                        principal_kind: kind,
                        limit,
                        offset: 0,
                    },
                    &token,
                )
            };
            for (shape, forged) in [
                ("non-empty region", with_region(&alice, "eu-west-1")),
                ("real org uuid in the org slot", with_org(&alice, &real_org)),
                ("absent org uuid in the org slot", with_org(&alice, &absent_org)),
            ] {
                let label = format!("{setting} {kind_label} {shape}");
                match client.list_memberships(list(forged, 0)).await {
                    Ok(resp) => failures.push(format!("{label}: listed {:?}", resp.into_inner().memberships)),
                    Err(err) => {
                        check(&mut failures, &label, err.code() == Code::InvalidArgument, format!("code was {:?}", err.code()));
                        check(&mut failures, &label, reason(&err) == "prn-mismatch", format!("reason was {}", reason(&err)));
                    }
                }
            }

            let label = format!("{setting} {kind_label} unknown principal");
            match client.list_memberships(list(unknown.clone(), 0)).await {
                Ok(resp) => failures.push(format!("{label}: listed {:?}", resp.into_inner().memberships)),
                Err(err) => {
                    check(&mut failures, &label, err.code() == Code::NotFound, format!("code was {:?}", err.code()));
                    check(&mut failures, &label, reason(&err) == "not-found", format!("reason was {}", reason(&err)));
                }
            }

            let label = format!("{setting} {kind_label} unknown principal + limit 500 (B8)");
            match client.list_memberships(list(unknown.clone(), 500)).await {
                Ok(_) => failures.push(format!("{label}: listed")),
                Err(err) => check(&mut failures, &label, reason(&err) == "invalid-pagination", format!("reason was {}", reason(&err))),
            }

            for (shape, prn) in [("canonical", alice.clone()), ("upper-case uuid", upper_uuid(&alice))] {
                let label = format!("{setting} {kind_label} control {shape}");
                match client.list_memberships(list(prn, 0)).await {
                    Ok(resp) => {
                        let rows = resp.into_inner().memberships;
                        check(&mut failures, &label, rows.iter().any(|m| m.node_prn == org.prn), format!("the seeded membership is missing: {rows:?}"));
                    }
                    Err(err) => failures.push(format!("{label}: {err:?}")),
                }
            }
        }
    }

    on_server.abort();
    off_server.abort();
    assert!(failures.is_empty(), "forged principal ListMemberships cases failed:\n{}", failures.join("\n"));
}

/// SMA-649 T3, B6 / AC5 (gRPC): with `enforce_tenancy` on, a caller without a root grant gets
/// `forbidden` for a forged, a canonical and an unknown principal PRN alike. The handler
/// authorizes at `root_prn()` before the service runs, so the new `prn-mismatch` and
/// `not-found` answers are not reachable by an unauthorized caller.
#[tokio::test]
async fn an_ungranted_caller_cannot_list_memberships_by_any_principal_prn() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let state = AppState::new(db.clone(), &support::test_config(&idp)).await.unwrap();
    let admin = idp.bearer("b6-admin", Some("b6-admin@example.com"), "paigasus", 3600);
    support::provision_platform_admin(&state, &admin).await;
    let stranger = idp.bearer("b6-stranger", Some("b6-stranger@example.com"), "paigasus", 3600);
    let stranger_prn = support::provision(&state, &stranger).await;
    let (addr, server) = spawn_tenancy_server(state).await;
    let mut client = connect(addr).await;

    let unknown = format!("prn:pgs:iam:::principal/{}", Uuid::from_u128(0x0f4a).as_hyphenated());
    let mut failures: Vec<String> = Vec::new();
    for (label, prn) in [
        ("canonical", stranger_prn.clone()),
        ("forged region", with_region(&stranger_prn, "eu-west-1")),
        ("forged org slot", with_org(&stranger_prn, &Uuid::from_u128(0x0f49).as_hyphenated().to_string())),
        ("unknown", unknown),
    ] {
        match client
            .list_memberships(authed(
                ListMembershipsRequest {
                    filter: Some(list_memberships_request::Filter::PrincipalPrn(prn)),
                    ..Default::default()
                },
                &stranger,
            ))
            .await
        {
            Ok(_) => failures.push(format!("{label}: listed")),
            Err(err) => {
                check(&mut failures, label, err.code() == Code::PermissionDenied, format!("code was {:?}", err.code()));
                check(&mut failures, label, reason(&err) == "forbidden", format!("reason was {}", reason(&err)));
            }
        }
    }

    server.abort();
    assert!(failures.is_empty(), "ungranted principal ListMemberships cases failed:\n{}", failures.join("\n"));
}
```

Note: the stranger lists its OWN principal PRN in the "canonical" row. The principal filter authorizes at `root_prn()` with no self exception, so that row must also be `forbidden`. If the run shows `Ok` for that row, stop and report it: the spec (§1.2, B6) assumes no self exception.

- [ ] **Step 2: Write the HTTP tests**

In `rs/crates/services/paigasus-iam/tests/http_memberships.rs`, change the `use support::{…}` line to `use support::{app_with_config, app_with_state, provision, provision_platform_admin, send, test_config};`, then append:

```rust
/// Percent-encodes the two PRN characters that are reserved in a URL. A colon is legal in a
/// query value, but the test must not depend on that (spec T3).
fn q(prn: &str) -> String {
    prn.replace(':', "%3A").replace('/', "%2F")
}

/// Replaces the region and the organization slot of a canonical principal PRN.
fn principal_with(prn: &str, region: &str, org: &str) -> String {
    let uuid = prn.rsplit('/').next().expect("a principal prn ends in /<uuid>");
    format!("prn:pgs:iam:{region}:{org}:principal/{uuid}")
}

/// SMA-649 T3 (HTTP): `GET /v1/memberships?principal=…` confirms the principal PRN against the
/// stored principal, under both `enforce_tenancy` settings, with `principal_kind` unset and
/// `user`. Forged region or org slot -> 400 `prn-mismatch`; unknown uuid -> 404 `not-found`;
/// unknown uuid + `limit=500` -> 400 `invalid-pagination` (B8). Controls: the canonical PRN
/// and its upper-case-uuid form list the seeded membership.
#[tokio::test]
async fn a_forged_principal_prn_never_lists_memberships_over_http() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (on_app, on_state, idp) = app_with_state(db.clone()).await;
    let mut cfg = test_config(&idp);
    cfg.authz.enforce_tenancy = false;
    let (off_app, _off_state) = app_with_config(db.clone(), &cfg).await;
    let token = idp.bearer("forged-lm-http", Some("forged-lm-http@example.com"), "paigasus", 3600);
    provision_platform_admin(&on_state, &token).await;

    let alice = create_user(&on_app, &token, "lm-http-alice@example.com").await;
    let (status, org_body) = send(&on_app, "POST", "/v1/organizations", Some(json!({"slug": "lm-http", "name": "LM HTTP"})), Some(token.as_str())).await;
    assert_eq!(status, StatusCode::CREATED, "{org_body}");
    let org_prn = org_body["organization"]["prn"].as_str().unwrap().to_string();
    let real_org = org_prn.rsplit('/').next().unwrap().to_string();
    let (status, body) = send(&on_app, "POST", "/v1/memberships", Some(json!({"principal_prn": alice, "node_prn": org_prn})), Some(token.as_str())).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    let absent_org = Uuid::from_u128(0x0f49).to_string();
    let unknown = format!("prn:pgs:iam:::principal/{}", Uuid::from_u128(0x0f4a));
    let upper = principal_with(&alice, "", "").replace(alice.rsplit('/').next().unwrap(), &alice.rsplit('/').next().unwrap().to_uppercase());
    let mut failures: Vec<String> = Vec::new();

    for (setting, app) in [("enforce=on", &on_app), ("enforce=off", &off_app)] {
        for kind in ["", "&principal_kind=user"] {
            let expect = |failures: &mut Vec<String>, label: String, got: (StatusCode, Value), status: StatusCode, code: &str| {
                if got.0 != status || got.1["error"]["code"] != code {
                    failures.push(format!("{label}: expected {status} {code}, got {} {}", got.0, got.1));
                }
            };
            for (shape, forged) in [
                ("non-empty region", principal_with(&alice, "eu-west-1", "")),
                ("real org uuid in the org slot", principal_with(&alice, "", &real_org)),
                ("absent org uuid in the org slot", principal_with(&alice, "", &absent_org)),
            ] {
                let got = send(app, "GET", &format!("/v1/memberships?principal={}{kind}", q(&forged)), None, Some(token.as_str())).await;
                expect(&mut failures, format!("{setting} [{kind}] {shape}"), got, StatusCode::BAD_REQUEST, "prn-mismatch");
            }
            let got = send(app, "GET", &format!("/v1/memberships?principal={}{kind}", q(&unknown)), None, Some(token.as_str())).await;
            expect(&mut failures, format!("{setting} [{kind}] unknown principal"), got, StatusCode::NOT_FOUND, "not-found");
            let got = send(app, "GET", &format!("/v1/memberships?principal={}{kind}&limit=500", q(&unknown)), None, Some(token.as_str())).await;
            expect(&mut failures, format!("{setting} [{kind}] unknown principal + limit 500 (B8)"), got, StatusCode::BAD_REQUEST, "invalid-pagination");

            for (shape, prn) in [("canonical", alice.clone()), ("upper-case uuid", upper.clone())] {
                let (status, body) = send(app, "GET", &format!("/v1/memberships?principal={}{kind}", q(&prn)), None, Some(token.as_str())).await;
                let listed = body.as_array().map(|rows| rows.iter().any(|r| r["node_prn"] == org_prn)).unwrap_or(false);
                if status != StatusCode::OK || !listed {
                    failures.push(format!("{setting} [{kind}] control {shape}: {status} {body}"));
                }
            }
        }
    }

    assert!(failures.is_empty(), "forged principal ListMemberships HTTP cases failed:\n{}", failures.join("\n"));
}

/// SMA-649 T3, B6 / AC5 (HTTP): with `enforce_tenancy` on, a caller without a root grant gets
/// 403 `forbidden` for a forged, a canonical and an unknown principal PRN alike. The handler
/// authorizes at `root_prn()` before `list` (`adapters/http/memberships.rs`); only this test
/// pins that order on HTTP.
#[tokio::test]
async fn an_ungranted_caller_cannot_list_memberships_by_any_principal_prn_over_http() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, state, idp) = app_with_state(db).await;
    let admin = idp.bearer("b6-http-admin", Some("b6-http-admin@example.com"), "paigasus", 3600);
    provision_platform_admin(&state, &admin).await;
    let alice = create_user(&app, &admin, "b6-http-alice@example.com").await;
    let stranger = idp.bearer("b6-http-stranger", Some("b6-http-stranger@example.com"), "paigasus", 3600);
    provision(&state, &stranger).await;

    let unknown = format!("prn:pgs:iam:::principal/{}", Uuid::from_u128(0x0f4a));
    for (label, prn) in [
        ("canonical", alice.clone()),
        ("forged region", principal_with(&alice, "eu-west-1", "")),
        ("forged org slot", principal_with(&alice, "", &Uuid::from_u128(0x0f49).to_string())),
        ("unknown", unknown),
    ] {
        let (status, body) = send(&app, "GET", &format!("/v1/memberships?principal={}", q(&prn)), None, Some(stranger.as_str())).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{label}: {body}");
        assert_eq!(body["error"]["code"], "forbidden", "{label}");
    }
}
```

If `test_config`, `app_with_config` or `provision` is not `pub` in `tests/support/mod.rs`, call them as `support::…` instead of importing them (they are `pub async fn` today, lines 453, 535, 635).

- [ ] **Step 3: Run the four tests**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-649-list-memberships-principal-prn/rs && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && docker info >/dev/null && CI=1 cargo nextest run -p paigasus-iam --test grpc_tenancy --test http_memberships --no-fail-fast -E 'test(/principal_prn/)'
```

Expected: all four PASS.

- [ ] **Step 4: Update the two module docs**

In `rs/crates/services/paigasus-iam/src/adapters/grpc/tenancy.rs`, replace the paragraph that starts `//! **The rule, and its one exception.**` and ends `//! accepted. That is SMA-649, not a property of this design.` (lines 30-39) with:

```rust
//! **The rule.** Every tenancy-NODE PRN this module accepts is confirmed against the stored node
//! before it is acted on: in the handler for the sixteen node RPCs — the thirteen that route
//! through `load_{org,team,project}_checked`, plus `GetOrganization`/`GetTeam`/`GetProject`,
//! which compare inline after their read — and in the REPOSITORY for the two membership RPCs
//! that take a node PRN (`pg_memberships`'s `attach_in` and `MembershipKindQuery::list_of_kind`
//! both compare the stored `prn` column and answer [`TenancyError::PrnMismatch`]).
//! `ListMemberships` with a PRINCIPAL filter is confirmed in the repository too:
//! `MembershipKindQuery::list_of_kind` loads the principal by uuid and answers
//! [`TenancyError::PrnMismatch`] on a difference (SMA-649).
```

Do not change the SMA-444 paragraph that follows (spec §4.5).

In `rs/crates/services/paigasus-iam/src/adapters/http/memberships.rs`, at the end of the last module-doc bullet (after `//!   a principal belongs to" — mirrofs `ListOrganizations`' platform-only posture, D4).`), add:

```rust
//!   A principal filter's PRN is then confirmed against the stored principal in the
//!   repository (`MembershipKindQuery::list_of_kind`, SMA-649): a forged region or organization
//!   slot answers [`TenancyError::PrnMismatch`], and an unknown principal answers
//!   [`TenancyError::NotFound`].
```

If `TenancyError` is not in scope for an intra-doc link in that file, write the full path `crate::application::error::TenancyError` inside the link brackets' target, for example ``[`TenancyError::NotFound`](crate::application::error::TenancyError::NotFound)``. Do not write any quoted error code.

- [ ] **Step 5: Mutations M1 and M3 on the transport tests**

M1: apply the M1 edit from Task 2 Step 3 (`if false && stored.prn != principal.canonical() {`). Run the command of Step 3. Expected: the tree compiles; `a_forged_principal_prn_never_lists_memberships` and `a_forged_principal_prn_never_lists_memberships_over_http` FAIL (forged shapes list rows); the two ungranted-caller tests PASS. Restore with the Edit tool.

M3: apply the M3 line from Task 1 Step 11. Run the command of Step 3. Expected: the tree compiles; the same two tests FAIL on their `kind unset` rows. Delete the marked line with the Edit tool.

- [ ] **Step 6: Build the docs, format, lint, commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-649-list-memberships-principal-prn/rs && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && RUSTDOCFLAGS="-D warnings" cargo doc -p paigasus-iam --no-deps && cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && git diff --stat && git -C .. branch --show-current
```

Expected: no rustdoc warning (a broken intra-doc link fails here); only the four files of this task changed. Then:

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-649-list-memberships-principal-prn && git add rs/crates/services/paigasus-iam/tests/grpc_tenancy.rs rs/crates/services/paigasus-iam/tests/http_memberships.rs rs/crates/services/paigasus-iam/src/adapters/grpc/tenancy.rs rs/crates/services/paigasus-iam/src/adapters/http/memberships.rs && git commit -m "test(rs): pin the principal PRN guard of ListMemberships on both transports (SMA-649)

The tenancy module doc now states the PRN-confirmation rule with no exception.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: `RoleService::grant` confirms the principal PRN (§4.7)

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/application/fakes.rs` (import list at lines 12-18; new fake after `InMemoryMemberships`' `MembershipKindQuery` impl)
- Modify: `rs/crates/services/paigasus-iam/src/application/roles.rs` (imports lines 44-51; `parse_principal_prn` doc lines 55-59; struct and deps lines 110-155; `new`; new `resolve_principal`; `grant` doc and body lines 206-240; test harness lines 395-470; new tests)
- Modify: `rs/crates/services/paigasus-iam/src/adapters/http/mod.rs:536-551`
- Modify: `rs/crates/services/paigasus-iam/tests/authz_bootstrap.rs:40-42, 207-220`

**Interfaces:**
- Consumes: `paigasus_iam_core::PrincipalRepository::find_principal(&self, id: &PrincipalId) -> Result<Option<Principal>, RepositoryError>`; `TenancyError: From<RepositoryError>` (exists, `application/error.rs`).
- Produces (Task 5 relies on the wire behaviour):
  - `RoleServiceDeps { …, principals: Arc<dyn PrincipalRepository>, … }` and the same private field on `RoleService`.
  - `async fn resolve_principal(&self, principal: &PrincipalId) -> Result<(), TenancyError>` (private).
  - `pub struct InMemoryTenancyPrincipals(pub TenancyStore)` in `application/fakes.rs`, `impl PrincipalRepository`.
  - `GrantRole`: forged principal region or org slot -> `TenancyError::PrnMismatch`; unknown principal uuid -> `TenancyError::NotFound`; both only after the authorization check and `resolve_scope`, and before `existing_grant`.

- [ ] **Step 1: Add the in-memory principal fake (`fakes.rs`)**

Add `PrincipalRepository` and `User` to the `use paigasus_iam_core::{…}` list at the top of `fakes.rs` (keep it sorted as `cargo fmt` wants). Then add, directly after the `impl MembershipKindQuery for InMemoryMemberships` block:

```rust
/// In-memory `PrincipalRepository` over the shared `TenancyStore` (SMA-649 §4.7), for
/// `RoleService::resolve_principal`. `find_principal` reads `store.principals` (uuid -> stored
/// canonical PRN) and builds the `Principal` FROM THE STORED PRN, exactly as
/// `PgPrincipalRepository::find_principal` does (`map_principal_row`). The kind comes from
/// `store.principal_kinds` (else `User`); the status is `Active`. `RoleService` calls no other
/// method, so the other three panic.
#[derive(Clone, Default)]
pub struct InMemoryTenancyPrincipals(pub TenancyStore);

#[async_trait]
impl PrincipalRepository for InMemoryTenancyPrincipals {
    async fn create_user(&self, _principal: &Principal, _user: &User) -> Result<(), RepositoryError> {
        unimplemented!("InMemoryTenancyPrincipals only exercises find_principal")
    }

    async fn create_user_in(&self, _tx: &dyn Transaction, _principal: &Principal, _user: &User) -> Result<(), RepositoryError> {
        unimplemented!("InMemoryTenancyPrincipals only exercises find_principal")
    }

    async fn find_user(&self, _id: &PrincipalId) -> Result<Option<(Principal, User)>, RepositoryError> {
        unimplemented!("InMemoryTenancyPrincipals only exercises find_principal")
    }

    async fn find_principal(&self, id: &PrincipalId) -> Result<Option<Principal>, RepositoryError> {
        let Some(stored) = self.0.principals.lock().unwrap().get(&id.uuid()).cloned() else {
            return Ok(None);
        };
        let prn = Prn::parse(&stored).map_err(|e| RepositoryError::Backend(Box::new(std::io::Error::other(e.to_string()))))?;
        let kind = self.0.principal_kinds.lock().unwrap().get(&id.uuid()).copied().unwrap_or(PrincipalKind::User);
        let epoch = DateTime::<Utc>::UNIX_EPOCH;
        Ok(Some(Principal::new(PrincipalId::from_prn(prn), kind, PrincipalStatus::Active, epoch, epoch)))
    }
}
```

`fakes.rs` is `#[cfg(test)]`-only, so a fake that is used only by tests does not trip `dead_code`.

- [ ] **Step 2: Add the dependency and the harness seeding (`roles.rs`), no guard yet**

1. Add `PrincipalRepository` to the `use paigasus_iam_core::{…}` list at the top of `roles.rs`.
2. Add `principals: Arc<dyn PrincipalRepository>,` to `struct RoleService` after `projects`, add `pub principals: Arc<dyn PrincipalRepository>,` to `struct RoleServiceDeps` after `projects`, and add `principals: deps.principals,` in `RoleService::new` after `projects: deps.projects,`.
3. In the struct doc of `RoleService`, after the sentence that ends `independent of `grants`.`, add: `` `principals` is [`RoleService::resolve_principal`]'s own lookup of the grant's target principal (SMA-649). ``
4. In `mod tests`, add `InMemoryTenancyPrincipals` to the `use crate::application::fakes::{…}` list, and add this helper above `fn new_service`:

```rust
    /// SMA-649: `grant` confirms its target principal against storage (`resolve_principal`).
    /// Every grant test here grants to `principal_prn(2)`, so every harness seeds it into
    /// `store.principals` with its canonical PRN. `principal_prn(3)` is never seeded: the
    /// unknown-principal tests use it. Tests that reach the principal check and rely on this
    /// seed: `grant_succeeds_for_an_authorized_actor`,
    /// `grant_emits_one_event_and_one_audit_entry_sharing_a_correlation_id_and_awaits_the_bump`,
    /// `a_store_error_mid_txn_rolls_back_and_never_emits_or_bumps_guard_d2`,
    /// `a_second_grant_returns_the_existing_grant_and_emits_nothing` and
    /// `a_lost_insert_race_returns_the_winner_s_grant_and_emits_nothing`.
    fn seed_grant_target(store: &TenancyStore) {
        seed_principal(store, 2);
    }

    /// Seeds `principal_prn(n)` into `store.principals` (uuid -> stored canonical PRN).
    fn seed_principal(store: &TenancyStore, n: u128) {
        let id = PrincipalId::from_prn(principal_prn(n));
        store.principals.lock().unwrap().insert(id.uuid(), id.canonical());
    }
```

5. In `new_service_with_fakes`, call `seed_grant_target(&store);` as its first line, and add `principals: Arc::new(InMemoryTenancyPrincipals(store.clone())),` to the `RoleServiceDeps { … }` literal before `projects: Arc::new(InMemoryProjects(store)),` (that line moves `store`, so the clone must come first).

6. Wire the production site in `rs/crates/services/paigasus-iam/src/adapters/http/mod.rs` (the `RoleService::new(RoleServiceDeps {` literal near line 536). Add after `projects: role_projects,`:

```rust
            // SMA-649: `RoleService::resolve_principal` confirms a grant's target principal PRN
            // against the stored row. A fresh handle over the same `db` (a cheap pool clone).
            principals: Arc::new(PgPrincipalRepository::new(db.clone())),
```

`PgPrincipalRepository` is already imported in `http/mod.rs` (line 67).

7. Wire `rs/crates/services/paigasus-iam/tests/authz_bootstrap.rs`: add `PgPrincipalRepository` to the `use paigasus_iam::adapters::persistence::{…}` list, and add `principals: Arc::new(PgPrincipalRepository::new(db.clone())),` after `projects: role_projects,` in its `RoleServiceDeps` literal. The member principal of that test is seeded by `seed_principal(&db, member_uuid)` with a canonical PRN, so its grant still succeeds.

Run:

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-649-list-memberships-principal-prn/rs && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cargo build --workspace --all-targets
```

Expected: FAILS with a `dead_code`-class error, because the new `principals` field is never read. This is the `warnings = "deny"` staging trap. Do not add `#[allow]`: continue with Step 3 and Step 4 before you build again.

- [ ] **Step 3: Write the failing unit tests (T6, Review Focus 1 and 2)**

Append to `mod tests` in `roles.rs`:

```rust
    /// SMA-649: a principal PRN string with `region` and `org` in the two slots and
    /// `principal_prn(n)`'s uuid. A stored principal PRN has both slots empty.
    fn forged_principal(n: u128, region: &str, org: &str) -> String {
        format!("prn:pgs:iam:{region}:{org}:principal/{}", Uuid::from_u128(n))
    }

    /// A harness whose authorizer allows `GrantRole` at Root, over a store with one real org
    /// (uuid 100), with direct handles on the grant map and the emission fakes.
    fn grant_harness() -> (ServiceWithFakes, InMemoryRoleGrants, TenancyStore) {
        let store = TenancyStore::default();
        let stamp = test_stamp(Utc.timestamp_opt(0, 0).unwrap(), 1);
        let real_org = Uuid::from_u128(100);
        store
            .orgs
            .lock()
            .unwrap()
            .insert(real_org, Organization::new(OrganizationId::from_uuid(real_org), Slug::parse("acme").unwrap(), "Acme", &stamp).unwrap());
        let fake = FakeAuthorizer::default();
        fake.allow(Action::GrantRole, &root_prn());
        let grants = InMemoryRoleGrants::default();
        let query = InMemoryRoleGrantQuery::over(&grants, &store);
        let harness = new_service_with_fakes(fake, Arc::new(grants.clone()), Arc::new(query), store.clone());
        (harness, grants, store)
    }

    /// Asserts that `grant` wrote nothing: no grant row, no event, no audit entry, no bump.
    fn assert_nothing_written(h: &ServiceWithFakes, grants: &InMemoryRoleGrants, label: &str) {
        assert!(grants.0.lock().unwrap().is_empty(), "{label}: a refused grant stores no row");
        assert!(h.outbox.0.lock().unwrap().is_empty(), "{label}: a refused grant enqueues no event");
        assert!(h.audit.0.lock().unwrap().is_empty(), "{label}: a refused grant records no audit entry");
        assert_eq!(h.bumper.calls(), 0, "{label}: a refused grant never bumps policy_gen");
    }

    /// SMA-649 G1, G2, AC11, Review Focus 1 and 2: a forged region or organization slot on the
    /// target principal answers `PrnMismatch` and writes nothing. Control: the canonical PRN
    /// grants, and the one event carries the STORED canonical PRN (AC13).
    #[tokio::test]
    async fn grant_refuses_a_forged_principal_prn() {
        let (h, grants, store) = grant_harness();
        let actor = principal_prn(1);
        // 0xab: a seeded principal whose uuid contains letters, for the upper-case shape.
        seed_principal(&store, 0xab);
        let upper_uuid = Uuid::from_u128(0xab).to_string().to_uppercase();
        for (shape, forged) in [
            ("non-empty region", forged_principal(2, "eu-west-1", "")),
            ("real org uuid in the org slot", forged_principal(2, "", &Uuid::from_u128(100).to_string())),
            ("random org uuid in the org slot", forged_principal(2, "", &Uuid::from_u128(0x0f49).to_string())),
            ("both slots", forged_principal(2, "eu-west-1", &Uuid::from_u128(0x0f49).to_string())),
            ("upper-case uuid and a region", format!("prn:pgs:iam:eu-west-1::principal/{upper_uuid}")),
        ] {
            let err = h.svc.grant(&actor, &forged, "platform_admin", &root_prn().canonical()).await.unwrap_err();
            assert_eq!(err, TenancyError::PrnMismatch, "{shape}");
            assert_nothing_written(&h, &grants, shape);
        }

        let grant = h.svc.grant(&actor, &principal_prn(2).canonical(), "platform_admin", &root_prn().canonical()).await.unwrap();
        assert_eq!(grant.principal.canonical(), principal_prn(2).canonical());
        let events = h.outbox.0.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].aggregate_prn, principal_prn(2).canonical());
    }

    /// SMA-649 G3, AC12: an unknown principal uuid answers `NotFound` (before SMA-649 the FK
    /// violation surfaced as `Internal`) and writes nothing.
    #[tokio::test]
    async fn grant_answers_not_found_for_an_unknown_principal() {
        let (h, grants, _store) = grant_harness();
        let err = h.svc.grant(&principal_prn(1), &principal_prn(3).canonical(), "platform_admin", &root_prn().canonical()).await.unwrap_err();
        assert_eq!(err, TenancyError::NotFound);
        assert_nothing_written(&h, &grants, "unknown principal");
        h.svc.grant(&principal_prn(1), &principal_prn(2).canonical(), "platform_admin", &root_prn().canonical()).await.expect("control: the seeded principal grants");
    }

    /// SMA-649 G4: the canonical PRN with an upper-case uuid grants, and the event carries the
    /// lower-case stored PRN. The uuid must contain letters, or the case changes nothing.
    #[tokio::test]
    async fn grant_accepts_an_upper_case_uuid() {
        let (h, _grants, store) = grant_harness();
        seed_principal(&store, 0xab);
        let upper = format!("prn:pgs:iam:::principal/{}", Uuid::from_u128(0xab).to_string().to_uppercase());
        assert_ne!(upper, principal_prn(0xab).canonical());
        let grant = h.svc.grant(&principal_prn(1), &upper, "platform_admin", &root_prn().canonical()).await.unwrap();
        assert_eq!(grant.principal.canonical(), principal_prn(0xab).canonical());
        assert_eq!(h.outbox.0.lock().unwrap()[0].aggregate_prn, principal_prn(0xab).canonical());
    }

    /// SMA-649 G6: a forged PRN for an EXISTING (principal, role, scope) grant answers
    /// `PrnMismatch`, not the existing grant. The principal check runs before `existing_grant`.
    #[tokio::test]
    async fn grant_refuses_a_forged_principal_prn_for_an_existing_grant() {
        let (h, _grants, _store) = grant_harness();
        let existing = h.svc.grant(&principal_prn(1), &principal_prn(2).canonical(), "platform_admin", &root_prn().canonical()).await.unwrap();
        let err = h.svc.grant(&principal_prn(1), &forged_principal(2, "eu-west-1", ""), "platform_admin", &root_prn().canonical()).await.unwrap_err();
        assert_eq!(err, TenancyError::PrnMismatch, "not Ok({existing:?})");
        assert_eq!(h.outbox.0.lock().unwrap().len(), 1, "only the first grant emitted");
    }

    /// SMA-649 G5, AC14: an actor without `GrantRole` at the scope gets `Forbidden` for a forged
    /// and for an unknown principal PRN alike. Authorization runs before the principal lookup.
    #[tokio::test]
    async fn grant_with_a_forged_principal_prn_is_denied_before_the_lookup() {
        let svc = new_service(FakeAuthorizer::default());
        for (label, prn) in [("forged region", forged_principal(2, "eu-west-1", "")), ("unknown principal", principal_prn(3).canonical())] {
            let err = svc.grant(&principal_prn(1), &prn, "platform_admin", &root_prn().canonical()).await.unwrap_err();
            assert_eq!(err, TenancyError::Forbidden, "{label}");
        }
    }
```

`InMemoryRoleGrants` must be `Clone` with a public `.0` map (existing tests use `grants.clone()` and `grants.0.lock()`). `FakePolicyGenBumper::calls()` exists (existing tests use it).

- [ ] **Step 4: Implement `resolve_principal` and call it in `grant`**

In `impl RoleService`, directly after `resolve_scope`, add:

```rust
    /// SMA-649 §4.7: the principal twin of [`RoleService::resolve_scope`]. Loads the grant's
    /// target principal by uuid and confirms that the caller-supplied PRN is the STORED one.
    /// `NotFound` if no principal has that uuid; `PrnMismatch` if the stored PRN differs from
    /// the canonical form of the supplied PRN (a forged region or organization slot). Without
    /// this, `grant` wrote the caller's forged PRN into the event's `aggregate_prn` and into the
    /// response (the SMA-606 D2 hazard). No log line, the same as `resolve_scope` (spec Q2).
    async fn resolve_principal(&self, principal: &PrincipalId) -> Result<(), TenancyError> {
        let stored = self.principals.find_principal(principal).await?.ok_or(TenancyError::NotFound)?;
        if stored.id.canonical() != principal.canonical() {
            return Err(TenancyError::PrnMismatch);
        }
        Ok(())
    }
```

In `grant`, directly after `self.resolve_scope(&scope).await?;`, add:

```rust
        self.resolve_principal(&principal).await?;
```

In the `grant` doc comment, replace `Only after all six succeed is the grant minted;` with:

```rust
    /// … (7) [`RoleService::resolve_principal`] confirms the target principal PRN against the
    /// stored principal (else `NotFound`/`PrnMismatch`, SMA-649), also only once `actor` is
    /// authorized, and BEFORE the idempotency pre-check below, so a forged PRN never reaches the
    /// existing-grant OK path. Only after all seven succeed is the grant minted;
```

(Keep the text around it; change "six" to "seven" and insert step 7 before "Only after".) Also change `SMA-676 D9: after step 6,` to `SMA-676 D9: after step 7,`.

Replace the doc of `roles.rs`'s `parse_principal_prn` with:

```rust
/// Parses a raw principal PRN string: must be syntactically valid (else `InvalidPrn` with the
/// kernel's stable error-kind token), and must be service `"iam"`, resource type
/// `"principal"` (else `InvalidPrn` with the PRN's canonical form). Like
/// `application::memberships::parse_principal_prn`, it checks ONLY the syntax, the service and
/// the type — duplicated rather than shared across modules (a five-line pure parse). For
/// `grant`, [`RoleService::resolve_principal`] confirms the region and the organization slot
/// against the stored principal (SMA-649). `RoleService::list` does NOT confirm them (out of
/// scope of SMA-649, spec §5).
```

- [ ] **Step 5: Build and run the unit tests**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-649-list-memberships-principal-prn/rs && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cargo build --workspace --all-targets && cargo nextest run -p paigasus-iam --lib --no-fail-fast
```

Expected: the build succeeds with no warning; every `--lib` test passes, the five new `grant_` tests and every existing `roles.rs` test included.

- [ ] **Step 6: Prove the new tests would red without the fix (M5, M6)**

M5: with the Edit tool, in `resolve_principal`, change `if stored.id.canonical() != principal.canonical() {` to `if false && stored.id.canonical() != principal.canonical() {`. Run:

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-649-list-memberships-principal-prn/rs && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cargo nextest run -p paigasus-iam --lib --no-fail-fast -E 'test(/application::roles::tests::grant_/)'
```

Expected: the tree compiles; `grant_refuses_a_forged_principal_prn` and `grant_refuses_a_forged_principal_prn_for_an_existing_grant` FAIL. Restore with the Edit tool.

M6: with the Edit tool, in `grant`, replace `self.resolve_principal(&principal).await?;` with `if false { self.resolve_principal(&principal).await?; } // MUTATION M6`. Run the same command. Expected: the tree compiles; `grant_refuses_a_forged_principal_prn`, `grant_answers_not_found_for_an_unknown_principal` and `grant_refuses_a_forged_principal_prn_for_an_existing_grant` FAIL. Restore the original line with the Edit tool.

- [ ] **Step 7: Run the Postgres suite that wires `RoleService` directly**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-649-list-memberships-principal-prn/rs && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && docker info >/dev/null && CI=1 cargo nextest run -p paigasus-iam --test authz_bootstrap --no-fail-fast
```

Expected: PASS.

- [ ] **Step 8: Format, lint, commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-649-list-memberships-principal-prn/rs && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && git diff --stat && git -C .. branch --show-current
```

Expected: only the four files of this task changed, and no `MUTATION` marker remains (`git grep -n "MUTATION M" -- rs/` prints nothing). Then:

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-649-list-memberships-principal-prn && git add rs/crates/services/paigasus-iam/src/application/fakes.rs rs/crates/services/paigasus-iam/src/application/roles.rs rs/crates/services/paigasus-iam/src/adapters/http/mod.rs rs/crates/services/paigasus-iam/tests/authz_bootstrap.rs && git commit -m "fix(rs): confirm the target principal PRN of GrantRole against storage (SMA-649)

RoleService::grant wrote a forged principal PRN into the outbox aggregate_prn
and into the response. resolve_principal now answers prn-mismatch for a
forged region or organization slot and not-found for an unknown uuid.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: GrantRole transport tests (T6 gRPC and HTTP)

**Files:**
- Modify: `rs/crates/services/paigasus-iam/tests/grpc_authz.rs` (imports, one helper, one test)
- Modify: `rs/crates/services/paigasus-iam/tests/http_authz.rs` (imports, one helper, one test)

**Interfaces:**
- Consumes (Task 4): `GrantRole` answers gRPC `InvalidArgument` / HTTP 400 `prn-mismatch` and gRPC `NotFound` / HTTP 404 `not-found`, and writes nothing. Existing helpers: `grpc_authz.rs` `spawn_server`, `channel`, `authed`, `reason_of`; `http_authz.rs` `self_principal_prn`; `support::{provision, seed_platform_admin, start_mock_idp, test_config}`; `AppState.role_grant_store: Arc<dyn RoleGrantStore>`.
- Produces: `grant_role_over_grpc_confirms_the_principal_prn_against_storage`, `grant_role_over_http_confirms_the_principal_prn_against_storage`.

- [ ] **Step 1: Write the gRPC test**

In `rs/crates/services/paigasus-iam/tests/grpc_authz.rs`, add these imports:

```rust
use paigasus_iam::adapters::persistence::entities::{audit_log, event_outbox};
use paigasus_iam_core::PrincipalId;
use paigasus_kernel::Prn;
use sea_orm::{ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter};
use uuid::Uuid;
```

Append:

```rust
/// Replaces the region and the organization slot of a canonical principal PRN.
fn principal_with(prn: &str, region: &str, org: &str) -> String {
    let uuid = prn.rsplit('/').next().expect("a principal prn ends in /<uuid>");
    format!("prn:pgs:iam:{region}:{org}:principal/{uuid}")
}

/// SMA-649 T6 (gRPC), AC11–AC13: a root-granted actor grants a role to a real principal. A
/// forged region or organization slot answers `InvalidArgument` / `prn-mismatch`, an unknown
/// uuid answers `NotFound` / `not-found`, and no refusal writes a `role_grant` row, an
/// `event_outbox` row or a `GrantRole` audit row. Control: the canonical PRN grants, and the
/// response carries the stored canonical PRN.
#[tokio::test]
async fn grant_role_over_grpc_confirms_the_principal_prn_against_storage() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let state = AppState::new(db.clone(), &support::test_config(&idp)).await.unwrap();
    let (addr, server) = spawn_server(state.clone()).await;
    let mut authz = AuthorizationServiceClient::new(channel(addr).await);

    let admin_token = idp.bearer("t6-grpc-admin", Some("t6-grpc-admin@example.com"), "paigasus", 3600);
    let admin_prn = support::provision(&state, &admin_token).await;
    support::seed_platform_admin(&state, &admin_prn).await;
    let member_token = idp.bearer("t6-grpc-member", Some("t6-grpc-member@example.com"), "paigasus", 3600);
    let member_prn = support::provision(&state, &member_token).await;
    let member = PrincipalId::from_prn(Prn::parse(&member_prn).unwrap());

    let outbox_before = event_outbox::Entity::find().count(&db).await.unwrap();
    let audit_before = audit_log::Entity::find().filter(audit_log::Column::Action.eq("GrantRole")).count(&db).await.unwrap();
    let absent_org = Uuid::from_u128(0x0f49).as_hyphenated().to_string();
    let unknown = format!("prn:pgs:iam:::principal/{}", Uuid::from_u128(0x0f4a).as_hyphenated());

    for (label, prn, code, reason) in [
        ("forged region", principal_with(&member_prn, "eu-west-1", ""), Code::InvalidArgument, "prn-mismatch"),
        ("forged org slot", principal_with(&member_prn, "", &absent_org), Code::InvalidArgument, "prn-mismatch"),
        ("unknown principal", unknown, Code::NotFound, "not-found"),
    ] {
        let err = authz
            .grant_role(authed(
                GrantRoleRequest {
                    principal_prn: prn,
                    role_key: "platform_admin".to_string(),
                    scope_prn: root_prn().canonical(),
                },
                &admin_token,
            ))
            .await
            .unwrap_err();
        assert_eq!(err.code(), code, "{label}: {err:?}");
        assert_eq!(reason_of(&err), reason, "{label}");
        assert!(state.role_grant_store.list_by_principal(&member).await.unwrap().is_empty(), "{label}: no grant row for the real principal");
    }
    assert_eq!(event_outbox::Entity::find().count(&db).await.unwrap(), outbox_before, "a refused grant enqueues no event");
    assert_eq!(
        audit_log::Entity::find().filter(audit_log::Column::Action.eq("GrantRole")).count(&db).await.unwrap(),
        audit_before,
        "a refused grant records no GrantRole audit row"
    );

    let granted = authz
        .grant_role(authed(
            GrantRoleRequest {
                principal_prn: member_prn.clone(),
                role_key: "platform_admin".to_string(),
                scope_prn: root_prn().canonical(),
            },
            &admin_token,
        ))
        .await
        .expect("control: the canonical prn grants")
        .into_inner()
        .grant
        .expect("grant");
    assert_eq!(granted.principal_prn, member_prn, "the response carries the stored canonical prn");

    server.abort();
}
```

- [ ] **Step 2: Write the HTTP test**

In `rs/crates/services/paigasus-iam/tests/http_authz.rs`, add these imports:

```rust
use paigasus_iam::adapters::persistence::entities::{audit_log, event_outbox};
use paigasus_iam_core::PrincipalId;
use paigasus_kernel::Prn;
use sea_orm::{ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter};
```

(`Uuid`, `json`, `StatusCode`, `root_prn`, `send`, `seed_platform_admin`, `app_with_state` are already imported.) Append:

```rust
/// Replaces the region and the organization slot of a canonical principal PRN.
fn principal_with(prn: &str, region: &str, org: &str) -> String {
    let uuid = prn.rsplit('/').next().expect("a principal prn ends in /<uuid>");
    format!("prn:pgs:iam:{region}:{org}:principal/{uuid}")
}

/// SMA-649 T6 (HTTP), AC11–AC13: `POST /v1/authz/role-grants` with a forged region or
/// organization slot answers 400 `prn-mismatch`, with an unknown uuid 404 `not-found`, and no
/// refusal writes a grant, an event or a `GrantRole` audit row. Control: the canonical PRN
/// grants, and the body carries the stored canonical PRN.
#[tokio::test]
async fn grant_role_over_http_confirms_the_principal_prn_against_storage() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, state, idp) = app_with_state(db.clone()).await;
    let admin_token = idp.bearer("t6-http-admin", Some("t6-http-admin@example.com"), "paigasus", 3600);
    let admin_prn = self_principal_prn(&app, &state, &admin_token).await;
    seed_platform_admin(&state, &admin_prn).await;
    let member_token = idp.bearer("t6-http-member", Some("t6-http-member@example.com"), "paigasus", 3600);
    let member_prn = self_principal_prn(&app, &state, &member_token).await;
    let member = PrincipalId::from_prn(Prn::parse(&member_prn).unwrap());

    let outbox_before = event_outbox::Entity::find().count(&db).await.unwrap();
    let audit_before = audit_log::Entity::find().filter(audit_log::Column::Action.eq("GrantRole")).count(&db).await.unwrap();
    let unknown = format!("prn:pgs:iam:::principal/{}", Uuid::from_u128(0x0f4a));

    for (label, prn, status, code) in [
        ("forged region", principal_with(&member_prn, "eu-west-1", ""), StatusCode::BAD_REQUEST, "prn-mismatch"),
        ("forged org slot", principal_with(&member_prn, "", &Uuid::from_u128(0x0f49).to_string()), StatusCode::BAD_REQUEST, "prn-mismatch"),
        ("unknown principal", unknown, StatusCode::NOT_FOUND, "not-found"),
    ] {
        let (got, body) = send(
            &app,
            "POST",
            "/v1/authz/role-grants",
            Some(json!({"principal_prn": prn, "role_key": "platform_admin", "scope_prn": root_prn().canonical()})),
            Some(admin_token.as_str()),
        )
        .await;
        assert_eq!(got, status, "{label}: {body}");
        assert_eq!(body["error"]["code"], code, "{label}");
        assert!(state.role_grant_store.list_by_principal(&member).await.unwrap().is_empty(), "{label}: no grant row for the real principal");
    }
    assert_eq!(event_outbox::Entity::find().count(&db).await.unwrap(), outbox_before, "a refused grant enqueues no event");
    assert_eq!(
        audit_log::Entity::find().filter(audit_log::Column::Action.eq("GrantRole")).count(&db).await.unwrap(),
        audit_before,
        "a refused grant records no GrantRole audit row"
    );

    let (got, granted) = send(
        &app,
        "POST",
        "/v1/authz/role-grants",
        Some(json!({"principal_prn": member_prn, "role_key": "platform_admin", "scope_prn": root_prn().canonical()})),
        Some(admin_token.as_str()),
    )
    .await;
    assert_eq!(got, StatusCode::CREATED, "{granted}");
    assert_eq!(granted["principal_prn"], member_prn);
}
```

If the audit `action` column value for a grant is not `"GrantRole"` (check `roles.rs`: `action: "GrantRole".into()`), use the value that `roles.rs` writes.

- [ ] **Step 3: Run the two tests**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-649-list-memberships-principal-prn/rs && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && docker info >/dev/null && CI=1 cargo nextest run -p paigasus-iam --test grpc_authz --test http_authz --no-fail-fast -E 'test(/confirms_the_principal_prn/)'
```

Expected: both PASS.

- [ ] **Step 4: Mutations M5 and M6 on the transport tests**

Apply M5 (Task 4 Step 6), run the command of Step 3. Expected: the tree compiles; both tests FAIL on the forged rows. Restore with the Edit tool.

Apply M6 (Task 4 Step 6), run the command of Step 3. Expected: the tree compiles; both tests FAIL (forged rows grant; the unknown row answers `internal` from the FK). Restore with the Edit tool.

- [ ] **Step 5: Format, lint, commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-649-list-memberships-principal-prn/rs && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && git diff --stat && git -C .. branch --show-current && git grep -n "MUTATION M" -- . ; true
```

Expected: only the two test files changed, and the `git grep` prints nothing. Then:

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-649-list-memberships-principal-prn && git add rs/crates/services/paigasus-iam/tests/grpc_authz.rs rs/crates/services/paigasus-iam/tests/http_authz.rs && git commit -m "test(rs): pin the principal PRN guard of GrantRole on both transports (SMA-649)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: `RoleService::list` confirms the principal PRN (§4.8)

Scope extended on 2026-09-28 (spec §0 Q6). This task depends on Task 4 (`resolve_principal`, the `principals` dependency, `seed_principal`, `InMemoryTenancyPrincipals`).

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/application/roles.rs` (`parse_principal_prn` doc; `resolve_principal` doc; `list` doc and body; `new_service_with_fakes`; new tests in `mod tests`)

**Interfaces:**
- Consumes (Task 4): `async fn resolve_principal(&self, principal: &PrincipalId) -> Result<(), TenancyError>`; the test helpers `seed_principal(store, n)` and `seed_grant_target(store)`; `new_service_with_fakes` builds `principals: Arc::new(InMemoryTenancyPrincipals(store.clone()))`. Existing list helpers: `ListHarness`, `list_harness`, `seed`, `by_principal`, `org_scope`.
- Produces (Task 7 relies on the wire behaviour): `RoleService::list` answers `TenancyError::PrnMismatch` for a forged principal filter and `TenancyError::NotFound` for an unknown principal uuid, on the principal-only path and on the query path, after the D4 authorization and before `Page::new`.

- [ ] **Step 1: Seed the actor principal in the unit-test harness**

`list` will look up every principal filter, the self path included. The existing `list` tests filter on `principal_prn(1)` (the actor) and `principal_prn(2)`. Task 4 seeds only `principal_prn(2)`. In `new_service_with_fakes`, directly after `seed_grant_target(&store);`, add:

```rust
        // SMA-649 §4.8: `list` confirms a principal filter against storage, also on the self
        // path, and the list tests filter on the actor `principal_prn(1)`. `principal_prn(3)`
        // stays unseeded for the unknown-principal tests.
        seed_principal(&store, 1);
```

Tests that rely on this seed: `list_allows_self_without_authorization_but_denies_listing_another_principal`, `list_self_needs_no_check_even_with_a_scope`, `list_principal_only_path_ignores_limit_and_offset`. `list_refuses_an_unknown_kind_even_for_self` fails at the kind step before the lookup. Tests that list `principal_prn(2)` rely on Task 4's seed.

- [ ] **Step 2: Write the failing unit tests (T7)**

Append to `mod tests` in `roles.rs`:

```rust
    /// SMA-649 §4.8: a list harness whose authorizer allows `ListRoleGrants` at Root. The target
    /// is `principal_prn(0xab)` (its uuid contains letters, for the upper-case shapes), seeded in
    /// `store.principals`, with one Root grant (returned) and one grant at org 100.
    fn list_guard_harness() -> (ListHarness, RoleGrant) {
        let fake = FakeAuthorizer::default();
        fake.allow(Action::ListRoleGrants, &root_prn());
        let store = TenancyStore::default();
        seed_principal(&store, 0xab);
        let grants = InMemoryRoleGrants::default();
        let query = InMemoryRoleGrantQuery::over(&grants, &store);
        let svc = new_service_with_fakes(fake, Arc::new(grants.clone()), Arc::new(query.clone()), store).svc;
        let h = ListHarness { svc, grants, query };
        let root_grant = seed(&h, 40, 0xab, "platform_admin", GrantScope::Root, PrincipalKind::User);
        seed(&h, 41, 0xab, "gateway_user", org_scope(100), PrincipalKind::User);
        (h, root_grant)
    }

    /// SMA-649 §4.8: the same principal filter on both read paths — principal-only (D6,
    /// `RoleGrantStore::list_by_principal`) and principal + the Root scope (`RoleGrantQuery::find`).
    fn on_both_paths(prn: &str) -> [(&'static str, ListRoleGrantsInput); 2] {
        [
            ("principal-only", by_principal(prn)),
            (
                "principal + Root scope",
                ListRoleGrantsInput {
                    scope_prn: Some(root_prn().canonical()),
                    ..by_principal(prn)
                },
            ),
        ]
    }

    /// SMA-649 L1–L3, AC15: a forged region or organization slot on the principal filter answers
    /// `PrnMismatch` on both read paths. Control: the canonical PRN lists the seeded Root grant on
    /// both paths, so the refusal cannot pass because `list` is broken for every input.
    #[tokio::test]
    async fn list_refuses_a_forged_principal_prn() {
        let (h, root_grant) = list_guard_harness();
        let actor = principal_prn(1);
        for (path, input) in on_both_paths(&principal_prn(0xab).canonical()) {
            assert!(h.svc.list(&actor, input).await.unwrap().contains(&root_grant), "control, {path}");
        }
        let random_org = Uuid::from_u128(0x0f49).to_string();
        let upper_uuid = Uuid::from_u128(0xab).to_string().to_uppercase();
        for (shape, forged) in [
            ("non-empty region", forged_principal(0xab, "eu-west-1", "")),
            ("org uuid 100 in the org slot", forged_principal(0xab, "", &Uuid::from_u128(100).to_string())),
            ("random org uuid in the org slot", forged_principal(0xab, "", &random_org)),
            ("both slots", forged_principal(0xab, "eu-west-1", &random_org)),
            ("upper-case uuid and a region", format!("prn:pgs:iam:eu-west-1::principal/{upper_uuid}")),
        ] {
            for (path, input) in on_both_paths(&forged) {
                assert_eq!(h.svc.list(&actor, input).await.unwrap_err(), TenancyError::PrnMismatch, "{shape}, {path}");
            }
        }
    }

    /// SMA-649 L4, AC16: an unknown principal uuid answers `NotFound` on both read paths, not an
    /// empty OK list.
    #[tokio::test]
    async fn list_answers_not_found_for_an_unknown_principal() {
        let (h, _root_grant) = list_guard_harness();
        for (path, input) in on_both_paths(&principal_prn(3).canonical()) {
            assert_eq!(h.svc.list(&principal_prn(1), input).await.unwrap_err(), TenancyError::NotFound, "{path}");
        }
    }

    /// SMA-649 L5, AC17: the canonical PRN with an upper-case uuid is still correct, on both
    /// read paths. The guard compares canonical forms, never the raw request string.
    #[tokio::test]
    async fn list_accepts_an_upper_case_uuid_in_a_correct_principal_prn() {
        let (h, root_grant) = list_guard_harness();
        let upper = format!("prn:pgs:iam:::principal/{}", Uuid::from_u128(0xab).to_string().to_uppercase());
        assert_ne!(upper, principal_prn(0xab).canonical(), "the uuid must contain letters, or the case changes nothing");
        for (path, input) in on_both_paths(&upper) {
            assert!(h.svc.list(&principal_prn(1), input).await.unwrap().contains(&root_grant), "{path}");
        }
    }

    /// SMA-649 L9: the guard runs before `Page::new`. On the query path with `limit` 201, a
    /// forged PRN answers `PrnMismatch` and an unknown uuid `NotFound`; the canonical PRN still
    /// answers `InvalidPagination`, so the page check itself still runs.
    #[tokio::test]
    async fn list_refuses_a_forged_principal_prn_before_the_page_check() {
        let (h, _root_grant) = list_guard_harness();
        let too_big = |prn: String| ListRoleGrantsInput {
            scope_prn: Some(root_prn().canonical()),
            limit: Some(201),
            ..by_principal(&prn)
        };
        let actor = principal_prn(1);
        assert_eq!(h.svc.list(&actor, too_big(forged_principal(0xab, "eu-west-1", ""))).await.unwrap_err(), TenancyError::PrnMismatch);
        assert_eq!(h.svc.list(&actor, too_big(principal_prn(3).canonical())).await.unwrap_err(), TenancyError::NotFound);
        assert_eq!(h.svc.list(&actor, too_big(principal_prn(0xab).canonical())).await.unwrap_err(), TenancyError::InvalidPagination);
    }

    /// SMA-649 L7, L8, AC18: the actor's own uuid with a forged region is NOT a self listing (the
    /// canonical forms differ), so an actor with no grant gets `Forbidden`, before the lookup. The
    /// actor's canonical PRN is a self listing and still lists (empty here).
    #[tokio::test]
    async fn list_with_a_forged_own_principal_prn_is_not_self() {
        let h = list_harness(FakeAuthorizer::default());
        let actor = principal_prn(1);
        let err = h.svc.list(&actor, by_principal(&forged_principal(1, "eu-west-1", ""))).await.unwrap_err();
        assert_eq!(err, TenancyError::Forbidden);
        assert!(h.svc.list(&actor, by_principal(&actor.canonical())).await.unwrap().is_empty());
    }
```

These tests call `forged_principal(n, region, org)` from Task 4 Step 3. Do not add a second copy of it.

`RoleGrant` derives `PartialEq` (existing tests compare `Vec<RoleGrant>`), so `contains` works. `TenancyError::InvalidPagination` is the variant that `Page::new` returns (existing test `list_scope_path_honours_page_bounds_and_order`).

- [ ] **Step 3: Run the new tests and confirm that they fail for the right reason**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-649-list-memberships-principal-prn/rs && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cargo nextest run -p paigasus-iam --lib --no-fail-fast -E 'test(/roles::tests::list_/)'
```

Expected: `list_refuses_a_forged_principal_prn`, `list_answers_not_found_for_an_unknown_principal` and `list_refuses_a_forged_principal_prn_before_the_page_check` FAIL (today they get `Ok(rows)`, `Ok([])` or `InvalidPagination`). `list_accepts_an_upper_case_uuid_in_a_correct_principal_prn`, `list_with_a_forged_own_principal_prn_is_not_self` and every existing `list_` test PASS. If a new test fails in setup (a panic, a missing import), fix the test first.

- [ ] **Step 4: Call `resolve_principal` in `list`**

In `RoleService::list`, directly after the `if !is_self { … }` authorization block and before `if let Some(principal) = filter.principal_only() {`, add:

```rust
        // SMA-649 §4.8: confirm the principal filter against the stored principal, on both read
        // paths and also on the self path, after D4 and before `Page::new` and any read.
        if let Some(principal) = filter.principal() {
            self.resolve_principal(principal).await?;
        }
```

In the `list` doc comment, replace `Then D6: the bare principal request` with:

```rust
    /// Then SMA-649 §4.8: a principal filter is confirmed against the stored principal
    /// ([`RoleService::resolve_principal`]: `NotFound` / `PrnMismatch`), on every read path and
    /// also for a self listing, before the page check. Then D6: the bare principal request
```

In the `resolve_principal` doc (Task 4), after its first sentence, add: `` `list` calls it too, for a principal filter (§4.8). ``

In the `parse_principal_prn` doc of `roles.rs` (Task 4 text), replace:

```rust
/// against the stored principal (SMA-649). `RoleService::list` does NOT confirm them (out of
/// scope of SMA-649, spec §5).
```

with:

```rust
/// against the stored principal (SMA-649); [`RoleService::list`] does the same for a principal
/// filter (SMA-649 §4.8).
```

If the Task 4 text in the file differs (for example after `cargo fmt`), keep its meaning and change only the `list` sentence.

- [ ] **Step 5: Build and run the unit tests**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-649-list-memberships-principal-prn/rs && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cargo build --workspace --all-targets && cargo nextest run -p paigasus-iam --lib --no-fail-fast
```

Expected: the build succeeds with no warning; every `--lib` test passes, the five new `list_` tests and every existing `roles.rs` test included.

- [ ] **Step 6: Mutations M7 and M5 on the unit tests**

M7: with the Edit tool, in `list`, change `if let Some(principal) = filter.principal() {` to `if let Some(principal) = filter.principal().filter(|_| false) {`. Run:

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-649-list-memberships-principal-prn/rs && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cargo nextest run -p paigasus-iam --lib --no-fail-fast -E 'test(/roles::tests::list_/)'
```

Expected: the tree compiles; `list_refuses_a_forged_principal_prn`, `list_answers_not_found_for_an_unknown_principal` and `list_refuses_a_forged_principal_prn_before_the_page_check` FAIL. Restore with the Edit tool (remove `.filter(|_| false)`).

M5 (Task 4 Step 6 edit in `resolve_principal`): apply it and run the same command. Expected: the tree compiles; `list_refuses_a_forged_principal_prn` and `list_refuses_a_forged_principal_prn_before_the_page_check` FAIL (`list_answers_not_found_for_an_unknown_principal` stays green, because `ok_or(NotFound)` is not mutated). Restore with the Edit tool.

After both, `git diff` must show only this task's edits (no `.filter(|_| false)`, no `if false &&`).

- [ ] **Step 7: Run the Postgres suites that list role grants**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-649-list-memberships-principal-prn/rs && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && docker info >/dev/null && CI=1 cargo nextest run -p paigasus-iam --no-fail-fast --test grpc_authz --test http_authz --test authz_people_model_access --test authz_forged_org_slot_escalation --test authz_bootstrap --test http_request_extractors
```

Expected: PASS. These suites list role grants of provisioned principals, the self path included (`authz_people_model_access.rs:140`). A failure here with `not-found` means that a test lists a principal that has no `principal` row: stop and report it, because spec §4.8 assumes that no such caller exists.

- [ ] **Step 8: Format, lint, commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-649-list-memberships-principal-prn/rs && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && git diff --stat && git -C .. branch --show-current && git grep -n "MUTATION M\|filter(|_| false)" -- . ; true
```

Expected: only `roles.rs` changed, the branch is `feature/sma-649-list-memberships-principal-prn`, and the `git grep` prints nothing. Then:

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-649-list-memberships-principal-prn && git add rs/crates/services/paigasus-iam/src/application/roles.rs && git commit -m "fix(rs): confirm the principal PRN of ListRoleGrants against storage (SMA-649)

RoleService::list filtered role grants on the bare principal uuid, so a
forged region or organization slot listed the real principal's grants.
It now calls resolve_principal for a principal filter: prn-mismatch for a
forged slot, not-found for an unknown uuid.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: ListRoleGrants transport tests (T7 gRPC and HTTP)

**Files:**
- Modify: `rs/crates/services/paigasus-iam/tests/grpc_authz.rs` (one test)
- Modify: `rs/crates/services/paigasus-iam/tests/http_authz.rs` (one helper, one test)

**Interfaces:**
- Consumes (Task 6): `ListRoleGrants` answers gRPC `InvalidArgument` / HTTP 400 `prn-mismatch` and gRPC `NotFound` / HTTP 404 `not-found` for a principal filter; gRPC `PermissionDenied` / HTTP 403 `forbidden` for a caller without the D4 grant. Consumes (Task 5): the `principal_with` helper and the imports that Task 5 adds to both files (`Uuid` in `grpc_authz.rs`). Existing helpers: `grpc_authz.rs` `spawn_server`, `channel`, `authed`, `reason_of`; `http_authz.rs` `self_principal_prn`, `app_with_state`, `send`, `seed_platform_admin`; `support::{provision, seed_platform_admin, start_mock_idp, test_config}`.
- Produces: `list_role_grants_over_grpc_confirms_the_principal_prn_against_storage`, `list_role_grants_over_http_confirms_the_principal_prn_against_storage` (Task 8 re-runs them; the name ends in `confirms_the_principal_prn_against_storage`, so the Task 8 filter `test(/confirms_the_principal_prn/)` selects them).

The fix is in place (Task 6), so these tests pass on the first run. Their proof is the mutation run in Step 4.

- [ ] **Step 1: Write the gRPC test**

Append to `rs/crates/services/paigasus-iam/tests/grpc_authz.rs`:

```rust
/// SMA-649 T7 (gRPC), AC15–AC18: `ListRoleGrants` with a principal filter confirms the PRN
/// against the stored principal, on the principal-only path and on the principal + Root-scope
/// path. A forged region or organization slot answers `InvalidArgument` / `prn-mismatch`, an
/// unknown uuid `NotFound` / `not-found`. Control: the canonical PRN lists the member's grant. An
/// ungranted caller gets `PermissionDenied` / `forbidden` for the member's PRN in every shape and
/// for its OWN uuid with a forged region (not a self listing), and its own canonical PRN lists.
#[tokio::test]
async fn list_role_grants_over_grpc_confirms_the_principal_prn_against_storage() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let state = AppState::new(db, &support::test_config(&idp)).await.unwrap();
    let (addr, server) = spawn_server(state.clone()).await;
    let mut authz = AuthorizationServiceClient::new(channel(addr).await);

    let admin_token = idp.bearer("t7-grpc-admin", Some("t7-grpc-admin@example.com"), "paigasus", 3600);
    let admin_prn = support::provision(&state, &admin_token).await;
    support::seed_platform_admin(&state, &admin_prn).await;
    let member_token = idp.bearer("t7-grpc-member", Some("t7-grpc-member@example.com"), "paigasus", 3600);
    let member_prn = support::provision(&state, &member_token).await;
    let stranger_token = idp.bearer("t7-grpc-stranger", Some("t7-grpc-stranger@example.com"), "paigasus", 3600);
    let stranger_prn = support::provision(&state, &stranger_token).await;

    authz
        .grant_role(authed(
            GrantRoleRequest {
                principal_prn: member_prn.clone(),
                role_key: "platform_admin".to_string(),
                scope_prn: root_prn().canonical(),
            },
            &admin_token,
        ))
        .await
        .expect("seed: the admin grants the member a Root role");

    let request = |prn: &str, scoped: bool| ListRoleGrantsRequest {
        principal_prn: prn.to_string(),
        scope_prn: if scoped { root_prn().canonical() } else { String::new() },
        ..Default::default()
    };
    let absent_org = Uuid::from_u128(0x0f49).as_hyphenated().to_string();
    let unknown = format!("prn:pgs:iam:::principal/{}", Uuid::from_u128(0x0f4a).as_hyphenated());
    let forged_region = principal_with(&member_prn, "eu-west-1", "");

    for (path, scoped) in [("principal-only", false), ("principal + Root scope", true)] {
        for (shape, prn, code, reason) in [
            ("forged region", forged_region.clone(), Code::InvalidArgument, "prn-mismatch"),
            ("forged org slot", principal_with(&member_prn, "", &absent_org), Code::InvalidArgument, "prn-mismatch"),
            ("unknown principal", unknown.clone(), Code::NotFound, "not-found"),
        ] {
            let err = authz.list_role_grants(authed(request(&prn, scoped), &admin_token)).await.unwrap_err();
            assert_eq!(err.code(), code, "{path}, {shape}: {err:?}");
            assert_eq!(reason_of(&err), reason, "{path}, {shape}");
        }
        let listed = authz.list_role_grants(authed(request(&member_prn, scoped), &admin_token)).await.expect("control: the canonical prn lists").into_inner().grants;
        assert!(listed.iter().any(|g| g.principal_prn == member_prn && g.role_key == "platform_admin"), "{path}: {listed:?}");
    }

    for (label, prn) in [
        ("member, canonical", member_prn.clone()),
        ("member, forged region", forged_region),
        ("unknown principal", unknown),
        ("own uuid, forged region", principal_with(&stranger_prn, "eu-west-1", "")),
    ] {
        let err = authz.list_role_grants(authed(request(&prn, false), &stranger_token)).await.unwrap_err();
        assert_eq!(err.code(), Code::PermissionDenied, "ungranted caller, {label}: {err:?}");
        assert_eq!(reason_of(&err), "forbidden", "ungranted caller, {label}");
    }
    let own = authz.list_role_grants(authed(request(&stranger_prn, false), &stranger_token)).await.expect("self listing").into_inner().grants;
    assert!(own.is_empty(), "the stranger holds no grant: {own:?}");

    server.abort();
}
```

If the ungranted "own uuid, forged region" row answers `Ok`, stop and report it: spec L7 assumes that the self check compares canonical forms.

- [ ] **Step 2: Write the HTTP test**

In `rs/crates/services/paigasus-iam/tests/http_authz.rs`, append:

```rust
/// Percent-encodes the two PRN characters that are reserved in a URL. A colon is legal in a
/// query value, but the test must not depend on that.
fn q(prn: &str) -> String {
    prn.replace(':', "%3A").replace('/', "%2F")
}

/// SMA-649 T7 (HTTP), AC15–AC18: `GET /v1/authz/role-grants` with a principal filter. A forged
/// region or organization slot answers 400 `prn-mismatch`, an unknown uuid 404 `not-found`, on
/// the principal-only path and with `scope_prn` = Root. Control: the canonical PRN lists the
/// member's grant. An ungranted caller gets 403 `forbidden` for the member's PRN in every shape
/// and for its OWN uuid with a forged region, and its own canonical PRN lists.
#[tokio::test]
async fn list_role_grants_over_http_confirms_the_principal_prn_against_storage() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, state, idp) = app_with_state(db).await;
    let admin_token = idp.bearer("t7-http-admin", Some("t7-http-admin@example.com"), "paigasus", 3600);
    let admin_prn = self_principal_prn(&app, &state, &admin_token).await;
    seed_platform_admin(&state, &admin_prn).await;
    let member_token = idp.bearer("t7-http-member", Some("t7-http-member@example.com"), "paigasus", 3600);
    let member_prn = self_principal_prn(&app, &state, &member_token).await;
    let stranger_token = idp.bearer("t7-http-stranger", Some("t7-http-stranger@example.com"), "paigasus", 3600);
    let stranger_prn = self_principal_prn(&app, &state, &stranger_token).await;

    let (status, body) = send(
        &app,
        "POST",
        "/v1/authz/role-grants",
        Some(json!({"principal_prn": member_prn, "role_key": "platform_admin", "scope_prn": root_prn().canonical()})),
        Some(admin_token.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "seed: {body}");

    let url = |prn: &str, scoped: bool| {
        if scoped {
            format!("/v1/authz/role-grants?principal_prn={}&scope_prn={}", q(prn), q(&root_prn().canonical()))
        } else {
            format!("/v1/authz/role-grants?principal_prn={}", q(prn))
        }
    };
    let unknown = format!("prn:pgs:iam:::principal/{}", Uuid::from_u128(0x0f4a));
    let forged_region = principal_with(&member_prn, "eu-west-1", "");

    for (path, scoped) in [("principal-only", false), ("principal + Root scope", true)] {
        for (shape, prn, want, code) in [
            ("forged region", forged_region.clone(), StatusCode::BAD_REQUEST, "prn-mismatch"),
            ("forged org slot", principal_with(&member_prn, "", &Uuid::from_u128(0x0f49).to_string()), StatusCode::BAD_REQUEST, "prn-mismatch"),
            ("unknown principal", unknown.clone(), StatusCode::NOT_FOUND, "not-found"),
        ] {
            let (got, body) = send(&app, "GET", &url(&prn, scoped), None, Some(admin_token.as_str())).await;
            assert_eq!(got, want, "{path}, {shape}: {body}");
            assert_eq!(body["error"]["code"], code, "{path}, {shape}");
        }
        let (got, listed) = send(&app, "GET", &url(&member_prn, scoped), None, Some(admin_token.as_str())).await;
        assert_eq!(got, StatusCode::OK, "{path}, control: {listed}");
        assert!(listed.as_array().unwrap().iter().any(|g| g["principal_prn"] == member_prn.as_str() && g["role_key"] == "platform_admin"), "{path}: {listed}");
    }

    for (label, prn) in [
        ("member, canonical", member_prn.clone()),
        ("member, forged region", forged_region),
        ("unknown principal", unknown),
        ("own uuid, forged region", principal_with(&stranger_prn, "eu-west-1", "")),
    ] {
        let (got, body) = send(&app, "GET", &url(&prn, false), None, Some(stranger_token.as_str())).await;
        assert_eq!(got, StatusCode::FORBIDDEN, "ungranted caller, {label}: {body}");
        assert_eq!(body["error"]["code"], "forbidden", "ungranted caller, {label}");
    }
    let (got, own) = send(&app, "GET", &url(&stranger_prn, false), None, Some(stranger_token.as_str())).await;
    assert_eq!(got, StatusCode::OK, "self listing: {own}");
    assert!(own.as_array().unwrap().is_empty(), "the stranger holds no grant: {own}");
}
```

If `self_principal_prn` for a third token needs a different setup than for the first two, follow the file's existing `grant_list_revoke` test. If the HTTP error body has a different shape than `body["error"]["code"]`, use the shape that the Task 5 HTTP test uses.

- [ ] **Step 3: Run the two tests**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-649-list-memberships-principal-prn/rs && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && docker info >/dev/null && CI=1 cargo nextest run -p paigasus-iam --test grpc_authz --test http_authz --no-fail-fast -E 'test(/list_role_grants_over_.*confirms_the_principal_prn/)'
```

Expected: both PASS.

- [ ] **Step 4: Mutations M7 and M5 on the transport tests**

Apply M7 (Task 6 Step 6), run the command of Step 3. Expected: the tree compiles; both tests FAIL (the forged rows list, the unknown row lists an empty OK list). Restore with the Edit tool.

Apply M5 (Task 4 Step 6), run the command of Step 3. Expected: the tree compiles; both tests FAIL on the forged rows. Restore with the Edit tool.

- [ ] **Step 5: Format, lint, commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-649-list-memberships-principal-prn/rs && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && git diff --stat && git -C .. branch --show-current && git grep -n "MUTATION M\|filter(|_| false)" -- . ; true
```

Expected: only the two test files changed, and the `git grep` prints nothing. Then:

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-649-list-memberships-principal-prn && git add rs/crates/services/paigasus-iam/tests/grpc_authz.rs rs/crates/services/paigasus-iam/tests/http_authz.rs && git commit -m "test(rs): pin the principal PRN guard of ListRoleGrants on both transports (SMA-649)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 8: Whole mutation battery and the full gate graph (T4, T5)

**Files:** none changed, unless a gate reds. A fix goes in its own commit.

**Interfaces:**
- Consumes: every test and guard of Tasks 1–7.
- Produces: the evidence for AC8 and AC9 in the task report.

- [ ] **Step 1: Re-run every mutation on the final tree**

A later fix can make an earlier mutation inert, so re-run all seven on the final tree (project memory "re-run a mutation battery whole"). For each row: apply the edit with the Edit tool, run the command, record the failing test names, restore with the Edit tool, and confirm `git diff --stat` is empty before the next row.

| id | edit (Edit tool) | command (from `rs/`, with the PATH prefix) | must FAIL |
|---|---|---|---|
| M1 | `pg_memberships.rs` `principal_list_uuid`: `if false && stored.prn != principal.canonical() {` | `CI=1 cargo nextest run -p paigasus-iam --no-fail-fast --test tenancy_memberships --test grpc_tenancy --test http_memberships -E 'test(/principal_prn/)'` | `list_of_kind_confirms_the_principal_prn`, `a_forged_principal_prn_never_lists_memberships`, `a_forged_principal_prn_never_lists_memberships_over_http` |
| M2 | `fakes.rs` `InMemoryMemberships::list_of_kind`: `if false && stored != principal.canonical() {` | `cargo nextest run -p paigasus-iam --lib --no-fail-fast -E 'test(/memberships::tests::list_/)'` | `list_refuses_a_forged_principal_prn`, `list_refuses_a_forged_principal_prn_past_the_last_page` |
| M3 | `memberships.rs` `list`: the M3 line of Task 1 Step 11 | `cargo nextest run -p paigasus-iam --lib --no-fail-fast -E 'test(/memberships::tests::list_/)'` then `CI=1 cargo nextest run -p paigasus-iam --no-fail-fast --test grpc_tenancy --test http_memberships -E 'test(/principal_prn/)'` | the T1 tests of the M2 row plus `list_answers_not_found_for_an_unknown_principal`; both forged-principal transport tests |
| M4 | `pg_memberships.rs` `node_list_sql`: `if false && stored != node.canonical() {` | `CI=1 cargo nextest run -p paigasus-iam --no-fail-fast --test tenancy_memberships -E 'test(/list_of_kind_confirms_the_node_prn/)'` | `list_of_kind_confirms_the_node_prn` |
| M5 | `roles.rs` `resolve_principal`: `if false && stored.id.canonical() != principal.canonical() {` | `cargo nextest run -p paigasus-iam --lib --no-fail-fast -E 'test(/roles::tests::grant_/) or test(/roles::tests::list_/)'` then `CI=1 cargo nextest run -p paigasus-iam --no-fail-fast --test grpc_authz --test http_authz -E 'test(/confirms_the_principal_prn/)'` | `grant_refuses_a_forged_principal_prn`, `grant_refuses_a_forged_principal_prn_for_an_existing_grant`, `roles::tests::list_refuses_a_forged_principal_prn`, `list_refuses_a_forged_principal_prn_before_the_page_check`, both T6 and both T7 transport tests |
| M6 | `roles.rs` `grant`: `if false { self.resolve_principal(&principal).await?; } // MUTATION M6` | the two M5 commands | the `grant_` unit tests of the M5 row plus `grant_answers_not_found_for_an_unknown_principal`; both T6 transport tests |
| M7 | `roles.rs` `list`: `if let Some(principal) = filter.principal().filter(\|_\| false) {` | the two M5 commands | `roles::tests::list_refuses_a_forged_principal_prn`, `roles::tests::list_answers_not_found_for_an_unknown_principal`, `list_refuses_a_forged_principal_prn_before_the_page_check`, both T7 transport tests (the `grant_` tests and both T6 tests stay green) |

In the M7 row, `\|` is only the Markdown table escape: the edit is `filter.principal().filter(|_| false)`.

Expected: every mutated tree compiles (a compile error proves nothing — memory "a mutation must compile"), and each row reds at least the tests named. If a row stays green, stop: the test does not pin the guard. Fix the test in the task that owns it, commit the fix, and re-run the WHOLE table.

- [ ] **Step 2: Confirm AC7 (no quoted code in an edited `src/` file, no exception left in the doc)**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-649-list-memberships-principal-prn && git diff origin/main --name-only -- 'rs/**/src/**' | xargs grep -nE '"(prn-mismatch|not-found|forbidden|invalid-pagination)"' ; grep -n "exception" rs/crates/services/paigasus-iam/src/adapters/grpc/tenancy.rs
```

Expected: the first `grep` prints nothing. The second prints no line from the PRN-confirmation paragraph (other paragraphs of the file may use the word for another subject; read each hit).

- [ ] **Step 3: Run the full gate graph**

Run the command between the `ci-targets` markers in the root `CLAUDE.md`, exactly as written there, from the worktree root:

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-649-list-memberships-principal-prn && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && git fetch origin main && moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site :http-extractor-envelope :input-liveness :promtool :observability-drift :nats-permissions :release-parity :release-parity-py :release-parity-ts :publish-metadata :version-lockstep :workflow-credentials :pyo3-stub-drift :ruff-ci :next-public-free :helm-render :test-e2e --base origin/main --include-relations
```

Expected: green. The gates most at risk are `repo:error-code-single-site`, `paigasus-iam-rs:fmt`, `paigasus-iam-rs:lint` and `paigasus-iam-rs:test`.

If a task fails, follow the root `CLAUDE.md` "Diagnosing an unattributed `moon ci` failure" procedure (Step 0 first: copy `.moon/cache/ciReport.json` and the task's `.moon/cache/states/<project>/<task>/` outside the repo before any re-run). Read a failure of `repo:affected-smoke`, `repo:ruff-ci`, `repo:next-public-free`, `repo:publish-metadata`, `repo:version-lockstep`, `repo:nats-permissions` or `repo:actionlint` against the "This development Mac only" bash rules before you treat it as a finding: re-run that gate directly with the bash it needs (`/bin/bash ci/<gate>/run.sh` or `/opt/homebrew/bin/bash ci/<gate>/run.sh`). This change touches no gate script, so a red in one of those gates that reproduces on `origin/main` is a host artifact: record it, do not fix it here. A red `paigasus-iam-rs:test` in a Docker-gated suite that this change does not touch (for example `authz_policy_store.rs`) can be a known flake: re-run that one target once and record both results.

<!-- moon-diagnosis:ok -->
<!-- This file names ciReport.json only to point at CLAUDE.md's procedure. It does not restate
     or supersede that procedure. Check 12 of `ci/actionlint/run.sh` requires this marker on any
     file that names ciReport.json. -->

- [ ] **Step 4: Report**

Record in the task report: the M1–M7 table with the failing test names seen for each row, the AC7 output, the `moon ci` verdict (and any gate re-run directly, with the bash used), and every deviation from this plan. No commit in this task unless a fix was needed.
