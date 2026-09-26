# SMA-676 gateway_user grant to a person — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** An `org_admin` grants and revokes `gateway_user` for a person at org scope in the gateway console, and sees which people hold it. IAM gets the query, the authorization rule and the idempotent grant that the console needs.

**Architecture:** `ListRoleGrants` gets optional AND-ed filters (`scope_prn`, `role_key`, `principal_kind`) behind a new read port `RoleGrantQuery`. `ListMemberships` gets a `principal_kind` filter behind a new read port `MembershipKindQuery`. `RoleService::list` owns D3, D4, D6 and D7, so gRPC and HTTP behave the same. `RoleService::grant` becomes idempotent (D9). The gateway console adds a `people-model-access/` section on `/gateway/orgs/[org]`, the same shape as `service-accounts/`.

**Tech Stack:** protobuf + buf (contracts), Rust edition 2024 / SeaORM 2 / tonic / axum / Cedar (`paigasus-iam-core`, `paigasus-iam`), TypeScript / Next.js 16 / React 19 / zod 4 / vitest / Playwright (`gateway-console`, `@paigasus/console-core`, `@paigasus/sdk`).

**Spec:** `docs/superpowers/specs/2026-09-26-sma-676-gateway-user-grant-design.md` (revision 2).

## Spec deviations found while planning

Each item below is a fact in the code that the spec does not state, or states differently. The plan uses the smallest correct handling. No spec decision changes.

1. **D7 names no error reason.** D7 says an unknown kind is refused with `InvalidArgument`. No existing `TenancyError` variant fits: `InvalidAuditOutcome` names another field, and `InvalidQueryParameter` is HTTP-only. The SMA-586 precedent (`invalid-audit-outcome`, `error.proto:153-156`) is one reason per filter enum. **Handling:** add `ERROR_REASON_INVALID_PRINCIPAL_KIND = 40` (`invalid-principal-kind`) and `TenancyError::InvalidPrincipalKind(&'static str)`. This touches the registry mirror (`rs/crates/libs/paigasus-proto/src/error.rs:154-232`, count 59 → 60) and the TS presentation table (`ts/packages/paigasus-sdk/src/errors/presentation.ts`, test count 59 → 60). Task 1.
2. **§7.4 "authz_role_grants.rs:232,517 → returns the existing grant" is imprecise.** Those two asserts call the STORE (`PgRoleGrantStore::grant` and `grant_in`). The store cannot return a grant; it raises `AuthzError::DuplicateGrant` (spec §4.2). **Handling:** the two asserts change from `Backend` to `DuplicateGrant` (the second one is at `:519`). "Returns the existing grant" is proved at the service level (Task 5) and end to end over HTTP (Task 7).
3. **§4.4 and §5.3 disagree on a revoke whose id is absent.** §4.4: "else it returns `invalid-input`". §5.3 step 4: "the check in step 2 found no grant … shows no error". **Handling:** the command filters the answer to `gateway_user` grants of that principal at that org. If that set is EMPTY, the grant is gone: success, no `RevokeRole` call (§5.3). If the set is NOT empty and does not hold the id, the form was crafted: `invalid-input`, no `RevokeRole` call (§4.4). No revoke happens in either case, so §6 holds. Task 10.
4. **§10's claim "the id check stays correct" against an old IAM is incomplete.** An old IAM ignores `scope_prn` and `role_key` and returns every grant of the principal. An id-only check then accepts the id of that principal's OTHER grant (for example `org_admin`). **Handling:** the command also checks `roleKey === GATEWAY_ROLE`, `scopePrn === orgPrn` and `principalPrn === input.principalPrn` on the matching row. Task 10, Review Focus item 1.
5. **D6 does not say which path a principal-only request WITH `role_key` or `principal_kind` takes.** **Handling:** the "principal-only path" is exactly the request shape every pre-SMA-676 caller sends: `principal_prn` and nothing else. It still calls `list_by_principal` and ignores `limit`/`offset`. Any request that sets `scope_prn`, `role_key` or `principal_kind` uses `RoleGrantQuery::find` with `Page::new` (1..=200). No old caller changes. Task 4.
6. **§4.3's `ListRoleGrantsInput { …, page }`.** A `Page` built by the adapter would refuse `limit = 500` on the principal-only path, which D6 forbids. **Handling:** the input carries raw `limit: Option<i64>, offset: Option<i64>`. `RoleService::list` calls `Page::new` on the query path only. Task 4.
7. **The new core port `RoleGrantQuery` has the same short name as the existing HTTP DTO** `adapters::http::dto::RoleGrantQuery` (`dto.rs:426`). No module imports both, so nothing clashes at compile time. **Handling:** keep both names (the spec names the port). The DTO gains the new fields. Task 4.
8. **`peopleModelAccessBlock({ view, path })`.** The block needs no `path`: the actions revalidate `SETTINGS_PATH` on the server, as the service-account actions do. An unused parameter fails `ts:lint`. **Handling:** the block takes `{ view }` only. Task 11.
9. **The block needs a client component.** `block.tsx` is a server function (like `serviceAccountsBlock`); the combobox and the two forms need client state. **Handling:** `block.tsx` renders the states and passes the actions to a new client component `app/_components/people-model-access-section.tsx`, in the pattern of `service-account-section.tsx`. A plain `view.ts` holds the view model and the copy, in the pattern of `service-accounts/view.ts`. Task 11.
10. **The e2e row must be a `playground-*.spec.ts` file.** `playwright.config.ts:72-74` sends only `playground*.spec.ts` files to the project that runs the real gateway. **Handling:** the row lives in `tests/e2e/playground-people-model-access.spec.ts`. Task 13.
11. **The e2e world's self branch cannot read grants for every row.** R24 and R25 (`playground-authz.spec.ts`) need a default world in which the user's `InvokeModel` is allowed with no grant. **Handling:** a new world option `orgCreator` turns on the §7.3 behaviour (org_admin grant, no membership, `InvokeModel` from recorded grants). The default world does not change for other rows. Task 13.
12. **Two more stale docs than §11 lists.** `ts/apps/gateway-console/README.md:81` (the row list, R30 added) and `:89` ("only a platform admin can list another principal's grants" is no longer true after D4). The spec's `README.md:86` reference is the SMA-635 bullet at line 81-82 of the "Known limits" list in this tree. **Handling:** update all three in Task 12 and Task 13.
13. **The bootstrap seeder has no observable "silent success" today.** **Handling:** the test installs a thread-local `metrics_util::debugging::DebuggingRecorder` and asserts that `iam_bootstrap_admin_seed_failures_total` is NOT emitted for a `DuplicateGrant`. `#[tokio::test]` is current-thread, so `metrics::set_default_local_recorder` sees the counter. Task 5.

## Global Constraints

- Every new source file opens with `// SPDX-License-Identifier: Apache-2.0` (`#` for Python).
- Rust crates: edition 2024, rust-version 1.95. `warnings = "deny"`: an unused private item is a compile error. Each task must compile on its own.
- `cargo fmt --check` and `cargo clippy --workspace --all-targets -- -D warnings` pass after every Rust task. `rustfmt` `max_width = 200`.
- Page bounds: `Page::new` accepts `limit` 1..=200 (default 50) and `offset` >= 0 (`application/pagination.rs:11,28-29`).
- Console list read: pages of 200, at most 5 pages, one N+1 probe `limit: 1, offset: 1000`. Cap = 1000 rows per list.
- `GATEWAY_ROLE = 'gateway_user'` is defined once, in `ts/apps/gateway-console/app/(console)/gateway-role.ts`.
- HTTP kind strings: `user`, `service_account` (`PrincipalKind::as_str`). gRPC: `PRINCIPAL_KIND_UNSPECIFIED = 0` (any), `USER = 1`, `SERVICE_ACCOUNT = 2`, any other value is refused.
- A `src/**/*.rs` file must never spell a registry code literal such as `"invalid-principal-kind"` outside `application/error.rs` (`ci/error-registry/check.py` MANIFEST). Unit tests compare `TenancyError` values or `ErrorReason` enum values. Files under `tests/` may spell literals.
- Conventional commits with a workspace scope: `feat(contracts)`, `feat(rs)`, `feat(ts)`, `test(rs)`, `test(ts)`, `docs(ts)`.
- A commit body must not contain a line that starts with `#NNN` or a `token: value` line (commitlint `footer-leading-blank`). Write "SMA-676" in prose, never `Refs: SMA-676`.
- Every commit ends with the line `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Never `git commit --amend`, never `git reset`, never `--no-verify`. One new commit per task.
- Every shell starts with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.
- After a `.proto` edit: `buf format -w` in `contracts/`, then `moon run contracts:generate --force`, then commit the generated Rust, Python and TS bindings.
- `ts:fmt` (Prettier, whole tree) and `<project>:typecheck` are separate Moon targets from `:test` and `ts:lint`. Run all four after a TS task.
- Docker-gated Rust suites: run a filtered suite with `PAIGASUS_REQUIRE_DOCKER=1`, else a missing daemon makes the suite skip and pass.
- Do not install host software. Do not start background jobs. Work only in this worktree on branch `feature/sma-676-gateway-user-grant`.

## Review Focus

These input classes and failure modes are the most likely to bite. Each has a named test in the owning task.

1. **A crafted or stale revoke form against an IAM that ignores the new filters.** The command must not revoke the principal's `org_admin` grant when the answer is unfiltered. Test: `revokeModelAccess refuses a grant id of another role even when IAM ignores the filters (spec § 10)` — Task 10.
2. **The principal-only path must not start to page or to refuse a large limit.** `scopes.ts` sends no limit over HTTP, and gRPC callers may send any value. Tests: `list_principal_only_path_ignores_limit_and_offset` (Task 4) and `the_http_list_refuses_no_filter_an_unknown_kind_and_a_large_scope_page_but_not_a_large_principal_page` (Task 7).
3. **An unknown kind must never widen to "any".** Proto enums are open; `""`, `"users"`, `"USER"` and wire value `7` or `-1` must all refuse. Tests: `principal_kind_filter_refuses_every_unknown_value` (Task 4, application), `principal_kind_filter_maps_the_wire_and_refuses_unknown_values` (Task 4, gRPC), `list_refuses_an_unknown_kind_even_for_self` (Task 4), `list_refuses_an_unknown_kind_before_the_repository` (Task 6).
4. **The duplicate race and the other unique constraint.** `DuplicateGrant` must come only from `uq_role_grant_principal_role_scope`. A `uq_role_grant_linked_policy` collision stays `Backend`. The race path must roll back and read the winner. Tests: `a_linked_policy_collision_stays_a_backend_error_not_a_duplicate_grant` (Task 3), `a_lost_insert_race_returns_the_winner_s_grant_and_emits_nothing` (Task 5).
5. **Case and whitespace in PRNs and filters.** A scope PRN with an upper-case UUID must match the stored lower-case canonical PRN; a whitespace-only `principal_prn` is "absent", not a parse error. Tests: `list_treats_blank_strings_as_absent` (Task 4), `an_org_admin_may_list_at_its_own_org_with_an_upper_case_uuid_and_not_at_another_org` (Task 7).

Also watched (lower risk): the N+1 probe boundary at exactly 1000 rows (`reads five full pages and probes once; exactly 1000 rows is not truncated`, Task 9), and the "not a member" mark when the member list is truncated (`hides the not-a-member mark when the member list is truncated`, Task 9).

## File Structure

### contracts/
| File | Change | Responsibility |
|---|---|---|
| `contracts/proto/paigasus/iam/v1/iam.proto` | modify | `enum PrincipalKind`; new fields on `ListRoleGrantsRequest` (4-6) and `ListMembershipsRequest` (5) |
| `contracts/proto/paigasus/common/v1/error.proto` | modify | `ERROR_REASON_INVALID_PRINCIPAL_KIND = 40` |
| generated Rust / Python / TS bindings | regenerate | `moon run contracts:generate --force` |

### rs/
| File | Change | Responsibility |
|---|---|---|
| `rs/crates/libs/paigasus-proto/src/error.rs` | modify | registry mirror: 60 reasons, `invalid-principal-kind` |
| `rs/crates/libs/paigasus-iam-core/src/authz/model.rs` | modify | `RoleGrantFilter`; `AuthzError::DuplicateGrant` |
| `rs/crates/libs/paigasus-iam-core/src/authz/ports.rs` | modify | `RoleGrantQuery` read port |
| `rs/crates/libs/paigasus-iam-core/src/ports.rs` | modify | `MembershipAxis`, `MembershipKindQuery` read port |
| `rs/crates/libs/paigasus-iam-core/src/authz/mod.rs`, `src/lib.rs` | modify | re-exports |
| `rs/crates/libs/paigasus-iam-core/src/authz/roles.rs` | modify | four `ListRoleGrants` rows in `starter_policy_table` |
| `rs/crates/services/paigasus-iam/src/application/error.rs` | modify | `InvalidPrincipalKind`; `DuplicateGrant → Internal` |
| `rs/crates/services/paigasus-iam/src/application/principal_kind.rs` | create | `PrincipalKindFilter` (D7 in one place) |
| `rs/crates/services/paigasus-iam/src/application/mod.rs` | modify | `pub mod principal_kind;` |
| `rs/crates/services/paigasus-iam/src/application/roles.rs` | modify | `ListRoleGrantsInput`, `RoleService::list` (D3/D4/D6/D7), idempotent `grant` (D9), `RoleServiceDeps.query` |
| `rs/crates/services/paigasus-iam/src/application/memberships.rs` | modify | `MembershipServiceDeps.kinds`, `list(filter, kind, page)` |
| `rs/crates/services/paigasus-iam/src/application/bootstrap_admin.rs` | modify | `DuplicateGrant` from a concurrent seed is a success |
| `rs/crates/services/paigasus-iam/src/application/fakes.rs` | modify | `InMemoryRoleGrantQuery`; `TenancyStore.principal_kinds`; `MembershipKindQuery for InMemoryMemberships` |
| `rs/crates/services/paigasus-iam/src/adapters/persistence/pg_role_grants.rs` | modify | `map_grant_err` → `DuplicateGrant`; `RoleGrantQuery for PgRoleGrantStore` |
| `rs/crates/services/paigasus-iam/src/adapters/persistence/pg_memberships.rs` | modify | kind predicate in the four list SQLs; `MembershipKindQuery for PgMembershipRepository` |
| `rs/crates/services/paigasus-iam/src/adapters/grpc/convert.rs` | modify | `principal_kind_filter(i32)` |
| `rs/crates/services/paigasus-iam/src/adapters/grpc/authz.rs` | modify | `ListRoleGrants` moves fields into `ListRoleGrantsInput` |
| `rs/crates/services/paigasus-iam/src/adapters/grpc/tenancy.rs` | modify | `ListMemberships` passes the kind |
| `rs/crates/services/paigasus-iam/src/adapters/http/authz.rs`, `http/memberships.rs`, `http/dto.rs` | modify | HTTP parity (D10) |
| `rs/crates/services/paigasus-iam/src/adapters/http/mod.rs` | modify | wire `query` and `kinds` |
| `rs/crates/services/paigasus-iam/tests/authz_role_grants.rs` | modify | query SQL, duplicate asserts, linked-policy collision |
| `rs/crates/services/paigasus-iam/tests/authz_people_model_access.rs` | create | HTTP end to end (D3, D4, D7, D9, grant → InvokeModel → revoke) |
| `rs/crates/services/paigasus-iam/tests/authz_forged_org_slot_escalation.rs` | modify | forged team `scope_prn` list is denied |
| `rs/crates/services/paigasus-iam/tests/grpc_authz.rs` | modify | struct literals; D3/D7 over gRPC |
| `rs/crates/services/paigasus-iam/tests/tenancy_memberships.rs` | modify | `list_of_kind` against Postgres |
| `rs/crates/services/paigasus-iam/tests/authz_bootstrap.rs`, `tests/tenancy_events_pg.rs` | modify | new deps fields |

### ts/
| File | Change | Responsibility |
|---|---|---|
| `ts/packages/paigasus-sdk/src/errors/presentation.ts`, `tests/presentation.test.ts` | modify | new reason row; count 60 |
| `ts/packages/paigasus-sdk/src/iam/types.ts`, `src/index.ts`, `tests/iam-types.test.ts` | modify | re-export `PrincipalKind` |
| `ts/packages/paigasus-console-core/src/authorize.ts`, `tests/unit/action-names.test.ts` | modify | `RevokeRole` in `IAM_ACTIONS` |
| `ts/apps/iam-console/tests/e2e/support/world.ts` | modify | `RevokeRole` in `ALL_ACTIONS` |
| `ts/packages/paigasus-console-core/testing/fake-iam.ts` | modify | refuse `limit > 200` as IAM does |
| `ts/packages/paigasus-console-core/tests/integration/fake-iam-page-limit.test.ts` | create | the refusal |
| `ts/packages/paigasus-console-core/testing/dev-world.ts`, `tests/unit/dev-world.test.ts` | modify | stateful grants, kind filter, grant/revoke |
| `ts/apps/gateway-console/app/(console)/gateway-role.ts` | create | `GATEWAY_ROLE` |
| `ts/apps/gateway-console/app/(console)/people-model-access/view.ts` | create | view model and copy |
| `ts/apps/gateway-console/app/(console)/people-model-access/load.ts` | create | loader, join, bounded paging |
| `ts/apps/gateway-console/app/(console)/people-model-access/commands.ts` | create | grant and bounded revoke |
| `ts/apps/gateway-console/app/(console)/people-model-access/actions.ts` | create | two Server Actions |
| `ts/apps/gateway-console/app/(console)/people-model-access/block.tsx` | create | server block |
| `ts/apps/gateway-console/app/_components/people-model-access-section.tsx` | create | client table, combobox, forms |
| `ts/apps/gateway-console/app/(console)/orgs/[org]/load.ts`, `page.tsx` | modify | call the loader, render the block |
| `ts/apps/gateway-console/app/(console)/service-accounts/commands.ts`, `revalidation.ts` | modify | import `GATEWAY_ROLE`; comment |
| `ts/apps/gateway-console/app/_components/service-account-frame.tsx`, `playground.tsx` | modify | comments |
| `ts/apps/gateway-console/tests/integration/people-model-access-load.test.ts` | create | loader tests |
| `ts/apps/gateway-console/tests/integration/people-model-access-commands.test.ts` | create | command tests |
| `ts/apps/gateway-console/tests/unit/people-model-access-block.test.tsx` | create | block states |
| `ts/apps/gateway-console/tests/integration/org-settings-load.test.ts` | modify | people section in the org loader |
| `ts/apps/gateway-console/tests/unit/actions-structure.test.ts`, `actions.test.ts` | modify | new actions file; label |
| `ts/apps/gateway-console/tests/integration/service-account-commands.test.ts` | modify | D9 duplicate is OK |
| `ts/apps/gateway-console/tests/e2e/call-count.spec.ts` | modify | R19/R20 formula |
| `ts/apps/gateway-console/tests/e2e/support/world.ts`, `support/playground-harness.ts` | modify | `orgCreator`, scope path, revoke, idempotent grant |
| `ts/apps/gateway-console/tests/e2e/playground-people-model-access.spec.ts` | create | R30 |
| `ts/apps/gateway-console/tests/unit/e2e-rows.test.ts`, `README.md` | modify | R30; docs |

---

## Task 1: Contract, error reason and the registry mirrors

**Files:**
- Modify: `contracts/proto/paigasus/iam/v1/iam.proto:35-39` (after `enum NodeStatus`), `:219-226` (`ListMembershipsRequest`), `:382-386` (`ListRoleGrantsRequest`)
- Modify: `contracts/proto/paigasus/common/v1/error.proto:164-171` (request-validation group)
- Regenerate: `rs/crates/libs/paigasus-proto/src/generated/**`, `py/packages/paigasus-proto/src/paigasus_proto/generated/**`, `ts/packages/paigasus-proto/src/generated/**`
- Modify: `rs/crates/libs/paigasus-proto/src/error.rs:154-221` (`EXPECTED_REASONS`), `:232` (count), `:349-362` (request-validation test)
- Modify: `rs/crates/services/paigasus-iam/src/application/error.rs:58-70`, `:171-207` (`code`), `:211-236` (`class`), `:258-280` (`field`), `:484-512` (tests)
- Modify: `rs/crates/services/paigasus-iam/tests/grpc_authz.rs:223-227`, `:242-246`
- Modify: `ts/packages/paigasus-sdk/src/errors/presentation.ts:12`, `:58` (after `INVALID_AUDIT_OUTCOME`)
- Modify: `ts/packages/paigasus-sdk/tests/presentation.test.ts:13`
- Modify: `ts/packages/paigasus-sdk/src/iam/types.ts:12`, `src/index.ts:29`, `tests/iam-types.test.ts`

**Interfaces:**
- Consumes: nothing.
- Produces:
  - Rust `paigasus_proto::paigasus::iam::v1::PrincipalKind { Unspecified = 0, User = 1, ServiceAccount = 2 }`
  - `ListRoleGrantsRequest { principal_prn: String, limit: u32, offset: u64, scope_prn: String, role_key: String, principal_kind: i32 }`
  - `ListMembershipsRequest { filter: Option<list_memberships_request::Filter>, limit: u32, offset: u64, principal_kind: i32 }`
  - `ErrorReason::InvalidPrincipalKind` (wire `invalid-principal-kind`, number 40)
  - `TenancyError::InvalidPrincipalKind(&'static str)`: code `invalid-principal-kind`, class `Validation`, `field()` = the payload
  - TS: `PrincipalKind` from `@paigasus/sdk/iam/types` (`UNSPECIFIED = 0`, `USER = 1`, `SERVICE_ACCOUNT = 2`)

- [ ] **Step 1: Write the failing Rust tests (registry mirror and TenancyError)**

In `rs/crates/libs/paigasus-proto/src/error.rs`, add `"invalid-principal-kind",` after `"mutually-exclusive-fields",` in `EXPECTED_REASONS` (line ~198), change `assert_eq!(actual.len(), 59, "the registry should hold 59 reasons");` to:

```rust
        assert_eq!(actual.len(), 60, "the registry should hold 60 reasons");
```

and add this row to the `for (variant, wire) in [...]` list of `the_request_validation_reasons_resolve_both_ways`:

```rust
            (ErrorReason::InvalidPrincipalKind, "invalid-principal-kind"),
```

In `rs/crates/services/paigasus-iam/src/application/error.rs`, add this row to `the_request_validation_codes_are_stable_and_all_validation`:

```rust
            (TenancyError::InvalidPrincipalKind("principal_kind"), "invalid-principal-kind"),
```

and append this test to the `tests` module:

```rust
    /// SMA-676 D7. An unknown principal kind is a 400 that names its field, so a client can see
    /// which filter it got wrong. It never widens the filter to "any kind".
    #[test]
    fn an_unknown_principal_kind_is_a_named_validation_error() {
        let err = TenancyError::InvalidPrincipalKind("principal_kind");
        assert_eq!(err.class(), ErrorClass::Validation);
        assert_eq!(err.field(), Some("principal_kind"));
        assert_eq!(err.to_string(), "principal_kind is not a known principal kind");
    }
```

- [ ] **Step 2: Run the tests and see them fail**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; cd rs && cargo nextest run -p paigasus-proto -p paigasus-iam --lib error`
Expected: FAIL — compile errors `no variant named InvalidPrincipalKind` on `ErrorReason` and on `TenancyError`.

- [ ] **Step 3: Edit the contracts**

In `contracts/proto/paigasus/iam/v1/iam.proto`, after `enum NodeStatus { … }` (line 39) add:

```proto
// SMA-676 D7. The kind of a principal, as IAM stores it. UNSPECIFIED on a
// filter means "any kind". IAM refuses any other number with
// ERROR_REASON_INVALID_PRINCIPAL_KIND: an unknown value never widens a filter.
enum PrincipalKind {
  PRINCIPAL_KIND_UNSPECIFIED = 0;
  PRINCIPAL_KIND_USER = 1;
  PRINCIPAL_KIND_SERVICE_ACCOUNT = 2;
}
```

Replace `ListMembershipsRequest` (lines 219-226) with:

```proto
message ListMembershipsRequest {
  oneof filter {
    string principal_prn = 1;
    string node_prn = 2;
  }
  uint32 limit = 3;
  uint64 offset = 4;
  // SMA-676 D8. AND-ed with the filter above. UNSPECIFIED = any kind.
  PrincipalKind principal_kind = 5;
}
```

Replace `ListRoleGrantsRequest` (lines 382-386) with:

```proto
message ListRoleGrantsRequest {
  // Optional when scope_prn is set. A request must set principal_prn or
  // scope_prn, or both (SMA-676 D3).
  string principal_prn = 1;
  // Honoured when scope_prn, role_key or principal_kind is set. A request
  // with only principal_prn returns every grant and ignores both (D6).
  uint32 limit = 2;
  uint64 offset = 3;
  // An exact match on the grant's scope node (D5). No descendant grants.
  string scope_prn = 4;
  string role_key = 5;
  // UNSPECIFIED = any kind (D7).
  PrincipalKind principal_kind = 6;
}
```

In `contracts/proto/paigasus/common/v1/error.proto`, after `ERROR_REASON_MUTUALLY_EXCLUSIVE_FIELDS = 38;` (line 171) add:

```proto
  // "invalid-principal-kind" — a principal_kind filter did not name a known
  // kind (SMA-676 D7). Refused, never widened to "any kind": a proto3 enum is
  // open, so an unknown number reaches the server. Both transports carry it.
  ERROR_REASON_INVALID_PRINCIPAL_KIND = 40;
```

- [ ] **Step 4: Format, lint and regenerate**

Run:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd contracts && buf format -w && cd ..
moon run contracts:fmt contracts:lint contracts:breaking
moon run contracts:generate --force
git status --short
```

Expected: `contracts:breaking` PASS (additive only). `git status` shows changes under the three `generated` trees, and `ts/packages/paigasus-proto/src/generated/google/rpc/error_details_pb.ts` is NOT deleted. If it is deleted, run `moon run contracts:generate --force` again (the BSR rate-limit trap).

- [ ] **Step 5: Add the TenancyError variant**

In `rs/crates/services/paigasus-iam/src/application/error.rs`, after `InvalidAuditOutcome(&'static str),` (line 68) add:

```rust
    /// SMA-676 D7: a `principal_kind` filter named no known kind. Refused, never read as "any".
    #[error("{0} is not a known principal kind")]
    InvalidPrincipalKind(&'static str),
```

In `code()`, after the `InvalidAuditOutcome` arm add:

```rust
            Self::InvalidPrincipalKind(_) => "invalid-principal-kind",
```

In `class()`, add `| Self::InvalidPrincipalKind(_)` after `| Self::InvalidAuditOutcome(_)`.
In `field()`, add `| Self::InvalidPrincipalKind(f)` after `| Self::InvalidAuditOutcome(f)`.

- [ ] **Step 6: Fix the two struct literals the new fields break**

In `rs/crates/services/paigasus-iam/tests/grpc_authz.rs`, change both `ListRoleGrantsRequest { principal_prn: …, limit: 0, offset: 0, }` literals (lines 223-227 and 242-246) to:

```rust
            ListRoleGrantsRequest {
                principal_prn: member_prn.clone(),
                ..Default::default()
            },
```

(the second one uses `principal_prn: member_prn,`).

- [ ] **Step 7: Update the TS registry table and the SDK types entry**

In `ts/packages/paigasus-sdk/src/errors/presentation.ts`, change the comment on line 12 to `// demanding an entry for it would make the table 61 keys rather than 60.` and add after `[ErrorReason.INVALID_AUDIT_OUTCOME]: 'from-transport',`:

```ts
  [ErrorReason.INVALID_PRINCIPAL_KIND]: 'from-transport',
```

In `ts/packages/paigasus-sdk/tests/presentation.test.ts:13`, change `toHaveLength(59)` to `toHaveLength(60)`.

Replace `ts/packages/paigasus-sdk/src/iam/types.ts:12` with:

```ts
// PrincipalKind serves the gateway zone's "Model access for people" filter and its e2e world (SMA-676).
export { ApiKeyStatus, NodeStatus, PrincipalKind } from '@paigasus/proto/iam';
```

Replace `ts/packages/paigasus-sdk/src/index.ts:29` with:

```ts
export { ApiKeyStatus, NodeStatus, PrincipalKind } from './iam/types';
```

In `ts/packages/paigasus-sdk/tests/iam-types.test.ts`, change the imports to include `PrincipalKind as ProtoPrincipalKind` and `PrincipalKind`, and append inside the `describe`:

```ts
  it('re-exports PrincipalKind as the registry enum object (SMA-676)', () => {
    expect(PrincipalKind.UNSPECIFIED).toBe(0);
    expect(PrincipalKind.USER).toBe(1);
    expect(PrincipalKind.SERVICE_ACCOUNT).toBe(2);
    expect(PrincipalKind).toBe(ProtoPrincipalKind);
  });
```

- [ ] **Step 8: Run everything this task touches and see it pass**

Run:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo build --workspace --all-targets && cargo nextest run -p paigasus-proto -p paigasus-iam --lib && cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cd ..
python3 ci/error-registry/check.py --self-test && python3 ci/error-registry/check.py --single-site
moon run paigasus-sdk-ts:test paigasus-sdk-ts:typecheck paigasus-proto-rs:test ts:fmt ts:lint
```

Expected: PASS. `check.py --single-site` prints no unlisted file.

- [ ] **Step 9: Commit**

```bash
git add contracts rs/crates/libs/paigasus-proto rs/crates/services/paigasus-iam/src/application/error.rs rs/crates/services/paigasus-iam/tests/grpc_authz.rs py/packages/paigasus-proto ts/packages/paigasus-proto ts/packages/paigasus-sdk
git commit -F - <<'EOF'
feat(contracts): add principal-kind filters and the invalid-principal-kind reason

SMA-676 D2, D7, D8. ListRoleGrantsRequest gains scope_prn, role_key and
principal_kind; ListMembershipsRequest gains principal_kind. All changes
add fields or a new enum. The new reason invalid-principal-kind refuses
an unknown kind; the registry mirror, the SDK presentation table and the
TenancyError variant land with it.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
```

---

## Task 2: IAM core — `RoleGrantFilter`, two read ports, `DuplicateGrant`

**Files:**
- Modify: `rs/crates/libs/paigasus-iam-core/src/authz/model.rs:7-14` (imports), `:143-150` (after `RoleGrant`), `:230-263` (`AuthzError`), `:266+` (tests)
- Modify: `rs/crates/libs/paigasus-iam-core/src/authz/ports.rs:7` (imports), after `:80` (`RoleGrantStore`), tests at the end
- Modify: `rs/crates/libs/paigasus-iam-core/src/ports.rs:11` (imports), after `:225` (`MembershipRepository`)
- Modify: `rs/crates/libs/paigasus-iam-core/src/authz/mod.rs:12-13`, `src/lib.rs:24-34`
- Modify: `rs/crates/services/paigasus-iam/src/application/error.rs:352-364` (`From<AuthzError>`) and tests

**Interfaces:**
- Consumes: `PrincipalId`, `GrantScope`, `RoleGrant`, `PrincipalKind`, `MembershipRecord`, `RepositoryError`, `TenancyNodeRef`.
- Produces:

```rust
pub struct RoleGrantFilter { /* private */ }
impl RoleGrantFilter {
    pub fn new(principal: Option<PrincipalId>, scope: Option<GrantScope>, role_key: Option<String>, principal_kind: Option<PrincipalKind>) -> Option<Self>;
    pub fn principal(&self) -> Option<&PrincipalId>;
    pub fn scope(&self) -> Option<&GrantScope>;
    pub fn role_key(&self) -> Option<&str>;
    pub fn principal_kind(&self) -> Option<PrincipalKind>;
    pub fn principal_only(&self) -> Option<&PrincipalId>;
    pub fn matches(&self, grant: &RoleGrant, grantee_kind: Option<PrincipalKind>) -> bool;
}
#[async_trait] pub trait RoleGrantQuery: Send + Sync {
    async fn find(&self, f: &RoleGrantFilter, limit: u64, offset: u64) -> Result<Vec<RoleGrant>, AuthzError>;
}
pub enum MembershipAxis { Principal(Uuid), Node(TenancyNodeRef) }
#[async_trait] pub trait MembershipKindQuery: Send + Sync {
    async fn list_of_kind(&self, axis: &MembershipAxis, kind: PrincipalKind, limit: u64, offset: u64) -> Result<Vec<MembershipRecord>, RepositoryError>;
}
AuthzError::DuplicateGrant   // unit variant
```

- [ ] **Step 1: Write the failing tests**

Append to the `tests` module of `rs/crates/libs/paigasus-iam-core/src/authz/model.rs`:

```rust
    fn pid(n: u128) -> PrincipalId {
        PrincipalId::from_prn(Prn::build("iam", "", None, "principal", u(n)).unwrap())
    }

    fn org_scope(n: u128) -> GrantScope {
        GrantScope::Node(TenancyNodeRef::Organization(OrganizationId::from_uuid(u(n))))
    }

    fn a_grant(principal: u128, role: &str, scope: GrantScope) -> RoleGrant {
        RoleGrant {
            id: u(9),
            principal: pid(principal),
            role_key: role.to_string(),
            scope,
            linked_policy_id: "grant:9".to_string(),
            created_at: Utc::now(),
        }
    }

    /// SMA-676 D3: a filter needs a principal or a scope. A role key alone is not enough, so
    /// no caller can list every grant of one role across all tenants.
    #[test]
    fn role_grant_filter_needs_a_principal_or_a_scope() {
        assert_eq!(RoleGrantFilter::new(None, None, None, None), None);
        assert_eq!(RoleGrantFilter::new(None, None, Some("gateway_user".to_string()), Some(PrincipalKind::User)), None);
        assert!(RoleGrantFilter::new(Some(pid(1)), None, None, None).is_some());
        assert!(RoleGrantFilter::new(None, Some(GrantScope::Root), None, None).is_some());
    }

    /// SMA-676 D6: only the request shape of the pre-SMA-676 callers is "principal-only".
    #[test]
    fn principal_only_is_the_bare_principal_filter() {
        let bare = RoleGrantFilter::new(Some(pid(1)), None, None, None).unwrap();
        assert_eq!(bare.principal_only(), Some(&pid(1)));
        let with_role = RoleGrantFilter::new(Some(pid(1)), None, Some("gateway_user".to_string()), None).unwrap();
        assert_eq!(with_role.principal_only(), None);
        let with_kind = RoleGrantFilter::new(Some(pid(1)), None, None, Some(PrincipalKind::User)).unwrap();
        assert_eq!(with_kind.principal_only(), None);
        let with_scope = RoleGrantFilter::new(Some(pid(1)), Some(org_scope(5)), None, None).unwrap();
        assert_eq!(with_scope.principal_only(), None);
    }

    /// SMA-676 D2, D5: every set filter is AND-ed; the scope is an EXACT match on the canonical
    /// scope PRN (a team grant does not match its org); a kind filter needs a known kind.
    #[test]
    fn matches_ands_every_set_filter_with_an_exact_scope() {
        let g = a_grant(1, "gateway_user", org_scope(5));
        let f = RoleGrantFilter::new(None, Some(org_scope(5)), Some("gateway_user".to_string()), Some(PrincipalKind::User)).unwrap();
        assert!(f.matches(&g, Some(PrincipalKind::User)));
        assert!(!f.matches(&g, Some(PrincipalKind::ServiceAccount)));
        assert!(!f.matches(&g, None), "a grantee with no known kind never matches a kind filter");
        assert!(!f.matches(&a_grant(1, "org_admin", org_scope(5)), Some(PrincipalKind::User)));
        assert!(!f.matches(&a_grant(1, "gateway_user", org_scope(6)), Some(PrincipalKind::User)));
        let team = GrantScope::Node(TenancyNodeRef::Team(crate::tenancy::TeamId::from_parts(u(5), u(50))));
        assert!(!f.matches(&a_grant(1, "gateway_user", team), Some(PrincipalKind::User)), "D5: no descendant match");
        let by_principal = RoleGrantFilter::new(Some(pid(1)), None, None, None).unwrap();
        assert!(by_principal.matches(&g, None));
        assert!(!by_principal.matches(&a_grant(2, "gateway_user", org_scope(5)), None));
    }

    #[test]
    fn duplicate_grant_has_a_static_message() {
        assert_eq!(AuthzError::DuplicateGrant.to_string(), "a grant for this principal, role and scope already exists");
    }
```

Append to the `tests` module of `rs/crates/libs/paigasus-iam-core/src/authz/ports.rs`:

```rust
    #[allow(dead_code)]
    fn assert_query_object_safe(_: &dyn RoleGrantQuery, _: &dyn crate::ports::MembershipKindQuery) {}
```

In `rs/crates/services/paigasus-iam/src/application/error.rs` tests append:

```rust
    /// SMA-676 D9: only `RoleService::grant` handles `DuplicateGrant`. Any other path that lets
    /// it reach this funnel is a defect, so it is an `Internal`, never a 409.
    #[test]
    fn a_duplicate_grant_that_escapes_the_grant_path_is_internal() {
        assert_eq!(TenancyError::from(AuthzError::DuplicateGrant), TenancyError::Internal);
    }
```

- [ ] **Step 2: Run the tests and see them fail**

Run: `cd rs && cargo nextest run -p paigasus-iam-core --lib authz && cargo nextest run -p paigasus-iam --lib application::error`
Expected: FAIL — `cannot find type RoleGrantFilter`, `no variant DuplicateGrant`, `cannot find trait RoleGrantQuery`.

- [ ] **Step 3: Implement the core types**

In `rs/crates/libs/paigasus-iam-core/src/authz/model.rs`, add `use crate::principal::PrincipalKind;` to the imports. After `pub struct RoleGrant { … }` (line 150) add:

```rust
/// The AND-ed filter of a role-grant listing (SMA-676 D2). A filter always names a principal or
/// a scope, or both: [`RoleGrantFilter::new`] refuses a filter with neither (D3), so no caller
/// can list the grants of every tenant by leaving both out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoleGrantFilter {
    principal: Option<PrincipalId>,
    scope: Option<GrantScope>,
    role_key: Option<String>,
    principal_kind: Option<PrincipalKind>,
}

impl RoleGrantFilter {
    /// `None` when neither `principal` nor `scope` is set (D3).
    #[must_use]
    pub fn new(principal: Option<PrincipalId>, scope: Option<GrantScope>, role_key: Option<String>, principal_kind: Option<PrincipalKind>) -> Option<Self> {
        if principal.is_none() && scope.is_none() {
            return None;
        }
        Some(Self { principal, scope, role_key, principal_kind })
    }

    #[must_use]
    pub fn principal(&self) -> Option<&PrincipalId> {
        self.principal.as_ref()
    }

    #[must_use]
    pub fn scope(&self) -> Option<&GrantScope> {
        self.scope.as_ref()
    }

    #[must_use]
    pub fn role_key(&self) -> Option<&str> {
        self.role_key.as_deref()
    }

    #[must_use]
    pub fn principal_kind(&self) -> Option<PrincipalKind> {
        self.principal_kind
    }

    /// The principal, when it is the ONLY filter: the request shape every caller sent before
    /// SMA-676. That path keeps its old behaviour — every row, no paging (D6).
    #[must_use]
    pub fn principal_only(&self) -> Option<&PrincipalId> {
        if self.scope.is_none() && self.role_key.is_none() && self.principal_kind.is_none() {
            self.principal.as_ref()
        } else {
            None
        }
    }

    /// Whether `grant` satisfies every set filter. The scope compares canonical PRNs, which is
    /// the stored `scope_node_prn` column's exact match (D5). `grantee_kind` is the grantee's
    /// stored kind; `None` (no principal row) never matches a kind filter, as an inner join
    /// drops such a row.
    #[must_use]
    pub fn matches(&self, grant: &RoleGrant, grantee_kind: Option<PrincipalKind>) -> bool {
        self.principal.as_ref().is_none_or(|p| *p == grant.principal)
            && self.scope.as_ref().is_none_or(|s| s.canonical_prn() == grant.scope.canonical_prn())
            && self.role_key.as_deref().is_none_or(|r| r == grant.role_key)
            && self.principal_kind.is_none_or(|k| grantee_kind == Some(k))
    }
}
```

In `AuthzError`, after the `Conflict(String)` variant add:

```rust
    /// A grant insert hit `uq_role_grant_principal_role_scope`: a grant for the same
    /// (principal, role, scope) already exists (SMA-676 D9). Raised ONLY by the grant insert.
    /// `RoleService::grant` answers it with the existing grant; nothing else may see it.
    #[error("a grant for this principal, role and scope already exists")]
    DuplicateGrant,
```

In `rs/crates/libs/paigasus-iam-core/src/authz/ports.rs`, change line 7 to import `RoleGrantFilter` too:

```rust
use super::model::{AccessRequest, AuthzDecisionEvent, AuthzError, Decision, EntitySlice, PolicyDocument, PutOutcome, Role, RoleGrant, RoleGrantFilter};
```

and after the `RoleGrantStore` trait (line 80) add:

```rust
/// Read port: role grants that match a [`RoleGrantFilter`] (SMA-676 D2), ordered by
/// `principal_id`, then `id`, with `limit`/`offset` applied after the order (D6). A separate
/// port, not a new [`RoleGrantStore`] method: that trait has nine implementations, seven of
/// them test fakes that would gain a method nothing calls — the rule
/// [`SystemPolicyReconciler`] records.
#[async_trait]
pub trait RoleGrantQuery: Send + Sync {
    async fn find(&self, f: &RoleGrantFilter, limit: u64, offset: u64) -> Result<Vec<RoleGrant>, AuthzError>;
}
```

In `rs/crates/libs/paigasus-iam-core/src/ports.rs`, change line 11 to `use crate::principal::{Principal, PrincipalKind, PrincipalStatus};` and after the `MembershipRepository` trait (line 225) add:

```rust
/// Which axis a membership listing reads: one principal's memberships, or one node's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MembershipAxis {
    Principal(Uuid),
    Node(TenancyNodeRef),
}

/// Read port: a membership listing narrowed to one principal kind (SMA-676 D8). A separate
/// port, not a new [`MembershipRepository`] method: that trait has six implementations, four
/// of them test fakes (the rule `authz::ports::SystemPolicyReconciler` records).
#[async_trait]
pub trait MembershipKindQuery: Send + Sync {
    /// The same order (`created_at, id`), paging and node guards (`NotFound`, `PrnMismatch`)
    /// as [`MembershipRepository::list_by_principal`] and [`MembershipRepository::list_by_node`],
    /// but only memberships whose principal is of `kind`.
    async fn list_of_kind(&self, axis: &MembershipAxis, kind: PrincipalKind, limit: u64, offset: u64) -> Result<Vec<MembershipRecord>, RepositoryError>;
}
```

In `rs/crates/libs/paigasus-iam-core/src/authz/mod.rs`, add `RoleGrantFilter` to the `model` re-export and `RoleGrantQuery` to the `ports` re-export. In `src/lib.rs`, add `RoleGrantFilter, RoleGrantQuery` to the `pub use authz::{…}` list and `MembershipAxis, MembershipKindQuery` to the `pub use ports::{…}` list.

In `rs/crates/services/paigasus-iam/src/application/error.rs`, in `From<AuthzError> for TenancyError`, change the last arm to:

```rust
            // SMA-676 D9: `RoleService::grant` consumes `DuplicateGrant`. Reaching this funnel
            // means another path let it through — a defect, so `Internal`, not a 409.
            AuthzError::Evaluation(_) | AuthzError::Backend(_) | AuthzError::ResourceNotFound(_) | AuthzError::DuplicateGrant => Self::Internal,
```

- [ ] **Step 4: Run the tests and see them pass**

Run: `cd rs && cargo nextest run -p paigasus-iam-core --lib && cargo nextest run -p paigasus-iam --lib application::error && cargo build --workspace --all-targets && cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add rs/crates/libs/paigasus-iam-core rs/crates/services/paigasus-iam/src/application/error.rs
git commit -F - <<'EOF'
feat(rs): add RoleGrantFilter, the RoleGrantQuery and MembershipKindQuery ports, and DuplicateGrant

SMA-676 D2, D3, D8, D9. The filter refuses a request with neither a
principal nor a scope. The two read ports keep RoleGrantStore and
MembershipRepository unchanged. DuplicateGrant maps to Internal outside
the grant path.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
```

---

## Task 3: Postgres and in-memory `RoleGrantQuery`; `DuplicateGrant` from the insert

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/persistence/pg_role_grants.rs:33-38` (imports), `:91-96` (`map_grant_err`), `:178-252` (impl), `:186-190` (doc comment), new `RoleGrantQuery` impl after `:252`
- Modify: `rs/crates/services/paigasus-iam/src/application/fakes.rs:10-22` (imports), after `:852` (`InMemoryRoleGrants`)
- Modify: `rs/crates/services/paigasus-iam/tests/authz_role_grants.rs:19-28` (imports), `:216-236`, `:480-523`, new tests at the end

**Interfaces:**
- Consumes: `RoleGrantFilter`, `RoleGrantQuery`, `AuthzError::DuplicateGrant` (Task 2).
- Produces: `impl RoleGrantQuery for PgRoleGrantStore`; `pub struct InMemoryRoleGrantQuery` with `pub fn over(store: &InMemoryRoleGrants) -> Self` and `pub fn set_kind(&self, principal: &PrincipalId, kind: PrincipalKind)`; `map_grant_err` returns `DuplicateGrant` for `uq_role_grant_principal_role_scope`.

- [ ] **Step 1: Write the failing tests**

In `rs/crates/services/paigasus-iam/tests/authz_role_grants.rs`:
- add `PrincipalKind, RoleGrantFilter, RoleGrantQuery` to the `paigasus_iam_core::{…}` import;
- in `authz_role_grant_duplicate_principal_role_scope_is_rejected_not_silently_swallowed` (line 232) replace the assert with:

```rust
    assert!(matches!(err, AuthzError::DuplicateGrant), "SMA-676 D9: expected AuthzError::DuplicateGrant for uq_role_grant_principal_role_scope, got {err:?}");
```

- in the mid-transaction test (line 519) replace the assert with:

```rust
    assert!(matches!(err, AuthzError::DuplicateGrant), "SMA-676 D9: expected AuthzError::DuplicateGrant for uq_role_grant_principal_role_scope, got {err:?}");
```

- append:

```rust
/// Seeds a bare `principal` row of `kind` (`user` or `service_account`) — the query's kind
/// join reads this column. Inline literals, for the reason `seed_principal_and_org` states.
async fn seed_principal_of_kind(db: &DatabaseConnection, principal_id: Uuid, kind: &str) {
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!(
            r#"INSERT INTO "principal" (id, prn, kind, status, created_at, updated_at)
               VALUES ('{principal_id}', 'prn:pgs:iam:::principal/{principal_id}', '{kind}', 'active', now(), now())"#
        ),
        [],
    ))
    .await
    .unwrap();
}

/// Seeds a bare `organization` row with its own slug (the slug is unique).
async fn seed_org(db: &DatabaseConnection, org_id: Uuid, slug: &str) {
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!(
            r#"INSERT INTO "organization" (id, prn, slug, name, status, created_at, updated_at)
               VALUES ('{org_id}', 'prn:pgs:iam:::organization/{org_id}', '{slug}', 'Org', 'active', now(), now())"#
        ),
        [],
    ))
    .await
    .unwrap();
}

fn pid(uuid: Uuid) -> PrincipalId {
    PrincipalId::from_prn(Prn::build("iam", "", None, "principal", uuid).unwrap())
}

fn ids(grants: &[RoleGrant]) -> Vec<u128> {
    grants.iter().map(|g| g.id.as_u128()).collect()
}

/// SMA-676 D2, D5, D6, D7 against Postgres: the kind join, the EXACT scope match (a team grant
/// is not an org grant), the Root case, and `ORDER BY principal_id, id` with paging.
#[tokio::test]
async fn role_grant_query_filters_by_exact_scope_role_and_kind_in_principal_then_id_order() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let now = Utc::now().trunc_subsecs(6);

    let (user_a, user_b, bot) = (Uuid::from_u128(0x1), Uuid::from_u128(0x2), Uuid::from_u128(0x3));
    seed_principal_of_kind(&db, user_a, "user").await;
    seed_principal_of_kind(&db, user_b, "user").await;
    seed_principal_of_kind(&db, bot, "service_account").await;
    let (org_o, org_p, team_t) = (Uuid::from_u128(0x100), Uuid::from_u128(0x200), Uuid::from_u128(0x110));
    seed_org(&db, org_o, "org-o").await;
    seed_org(&db, org_p, "org-p").await;
    seed_team(&db, org_o, team_t).await;
    for role in ["gateway_user", "org_admin", "platform_admin"] {
        seed_role(&db, role, now).await;
    }

    let o = GrantScope::Node(TenancyNodeRef::Organization(OrganizationId::from_uuid(org_o)));
    let p = GrantScope::Node(TenancyNodeRef::Organization(OrganizationId::from_uuid(org_p)));
    let t = GrantScope::Node(TenancyNodeRef::Team(TeamId::from_parts(org_o, team_t)));
    let store = PgRoleGrantStore::new(db.clone(), Generations::memory());
    for g in [
        make_grant(Uuid::from_u128(0x1001), &pid(user_b), "gateway_user", o.clone(), now),
        make_grant(Uuid::from_u128(0x1002), &pid(user_a), "gateway_user", o.clone(), now),
        make_grant(Uuid::from_u128(0x1003), &pid(bot), "gateway_user", o.clone(), now),
        make_grant(Uuid::from_u128(0x1004), &pid(user_a), "org_admin", o.clone(), now),
        make_grant(Uuid::from_u128(0x1005), &pid(user_a), "gateway_user", t.clone(), now),
        make_grant(Uuid::from_u128(0x1006), &pid(user_b), "gateway_user", p.clone(), now),
        make_grant(Uuid::from_u128(0x1007), &pid(user_a), "platform_admin", GrantScope::Root, now),
    ] {
        store.grant(&g).await.unwrap();
    }

    let people = RoleGrantFilter::new(None, Some(o.clone()), Some("gateway_user".to_string()), Some(PrincipalKind::User)).unwrap();
    assert_eq!(ids(&store.find(&people, 200, 0).await.unwrap()), vec![0x1002, 0x1001], "users only, principal order");

    let all_at_o = RoleGrantFilter::new(None, Some(o.clone()), None, None).unwrap();
    assert_eq!(ids(&store.find(&all_at_o, 200, 0).await.unwrap()), vec![0x1002, 0x1004, 0x1001, 0x1003], "principal_id, then id; no team grant");
    assert_eq!(ids(&store.find(&all_at_o, 2, 1).await.unwrap()), vec![0x1004, 0x1001], "limit and offset apply after the order");

    let bots = RoleGrantFilter::new(None, Some(o.clone()), None, Some(PrincipalKind::ServiceAccount)).unwrap();
    assert_eq!(ids(&store.find(&bots, 200, 0).await.unwrap()), vec![0x1003]);

    let at_team = RoleGrantFilter::new(None, Some(t), None, None).unwrap();
    assert_eq!(ids(&store.find(&at_team, 200, 0).await.unwrap()), vec![0x1005], "D5: exact match, the team only");

    let at_root = RoleGrantFilter::new(None, Some(GrantScope::Root), None, None).unwrap();
    assert_eq!(ids(&store.find(&at_root, 200, 0).await.unwrap()), vec![0x1007]);

    let a_as_gateway_user = RoleGrantFilter::new(Some(pid(user_a)), None, Some("gateway_user".to_string()), None).unwrap();
    assert_eq!(ids(&store.find(&a_as_gateway_user, 200, 0).await.unwrap()), vec![0x1002, 0x1005]);
}

/// SMA-676 Review Focus 4. Only `uq_role_grant_principal_role_scope` means "this grant already
/// exists". A `uq_role_grant_linked_policy` collision is a different grant with a clashing
/// policy id — a defect, and it must stay `Backend`, or `RoleService::grant` would answer it
/// with an unrelated "existing" grant.
#[tokio::test]
async fn a_linked_policy_collision_stays_a_backend_error_not_a_duplicate_grant() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let now = Utc::now().trunc_subsecs(6);
    let principal_uuid = mint_uuid7(1_700_000_000_020, [20u8; 10]);
    let org_uuid = Uuid::from_u128(20);
    seed_principal_and_org(&db, principal_uuid, org_uuid).await;
    seed_role(&db, "org_admin", now).await;
    seed_role(&db, "gateway_user", now).await;

    let principal = pid(principal_uuid);
    let org = GrantScope::Node(TenancyNodeRef::Organization(OrganizationId::from_uuid(org_uuid)));
    let store = PgRoleGrantStore::new(db.clone(), Generations::memory());
    let first = make_grant(Uuid::from_u128(400), &principal, "org_admin", org.clone(), now);
    store.grant(&first).await.unwrap();

    let mut second = make_grant(Uuid::from_u128(401), &principal, "gateway_user", org, now);
    second.linked_policy_id = first.linked_policy_id.clone();
    let err = store.grant(&second).await.unwrap_err();
    assert!(matches!(err, AuthzError::Backend(_)), "a linked-policy collision is not a duplicate grant; got {err:?}");
}
```

In `rs/crates/services/paigasus-iam/src/application/fakes.rs`, append a test module at the end of the file (the file is `#[cfg(test)]` only, so a plain `mod` is enough):

```rust
#[cfg(test)]
mod role_grant_query_fake_tests {
    use super::*;
    // `fakes.rs` does not import `GrantScope` at the top.
    use paigasus_iam_core::GrantScope;

    fn pid(n: u128) -> PrincipalId {
        PrincipalId::from_prn(Prn::build("iam", "", None, "principal", Uuid::from_u128(n)).unwrap())
    }

    fn grant(id: u128, principal: u128, role: &str, scope: GrantScope) -> RoleGrant {
        RoleGrant {
            id: Uuid::from_u128(id),
            principal: pid(principal),
            role_key: role.to_string(),
            scope,
            linked_policy_id: format!("grant:{id}"),
            created_at: DateTime::<Utc>::from_timestamp(0, 0).unwrap(),
        }
    }

    /// The fake agrees with `PgRoleGrantStore::find`: the same filter semantics, the same
    /// `principal_id, id` order, and a grantee with no known kind drops out of a kind filter.
    #[tokio::test]
    async fn the_in_memory_query_filters_orders_and_pages_like_postgres() {
        let store = InMemoryRoleGrants::default();
        let query = InMemoryRoleGrantQuery::over(&store);
        let org = GrantScope::Node(TenancyNodeRef::Organization(OrganizationId::from_uuid(Uuid::from_u128(100))));
        for g in [grant(12, 2, "gateway_user", org.clone()), grant(11, 1, "gateway_user", org.clone()), grant(13, 3, "gateway_user", org.clone()), grant(14, 1, "org_admin", org.clone())] {
            store.0.lock().unwrap().insert(g.id, g);
        }
        query.set_kind(&pid(1), PrincipalKind::User);
        query.set_kind(&pid(2), PrincipalKind::User);
        // Principal 3 has no kind entry: no principal row.

        let users = RoleGrantFilter::new(None, Some(org.clone()), Some("gateway_user".to_string()), Some(PrincipalKind::User)).unwrap();
        let found: Vec<u128> = query.find(&users, 200, 0).await.unwrap().iter().map(|g| g.id.as_u128()).collect();
        assert_eq!(found, vec![11, 12]);

        let all = RoleGrantFilter::new(None, Some(org), None, None).unwrap();
        let page: Vec<u128> = query.find(&all, 2, 1).await.unwrap().iter().map(|g| g.id.as_u128()).collect();
        assert_eq!(page, vec![14, 12], "order (1,11) (1,14) (2,12) (3,13); offset 1, limit 2");
    }
}
```

- [ ] **Step 2: Run the tests and see them fail**

Run: `cd rs && cargo nextest run -p paigasus-iam --lib role_grant_query_fake_tests`
Expected: FAIL — `cannot find type InMemoryRoleGrantQuery`.
Run: `cd rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test authz_role_grants`
Expected: FAIL — compile error `no method named find` on `PgRoleGrantStore` (the trait is not implemented).

- [ ] **Step 3: Implement `map_grant_err` and the Postgres query**

In `pg_role_grants.rs`, change the imports:

```rust
use super::entities::role_grant;
use super::uow::{SeaOrmTransaction, recover_txn};
use crate::adapters::authz::Generations;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use paigasus_iam_core::{AuthzError, GrantScope, PrincipalId, RepositoryError, RoleGrant, RoleGrantFilter, RoleGrantQuery, RoleGrantStore, TenancyNodeRef, Transaction};
use paigasus_kernel::Prn;
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, DbBackend, DbErr, EntityTrait, FromQueryResult, QueryFilter, Set, SqlErr, Statement, TransactionTrait};
use uuid::Uuid;
```

Replace `map_grant_err` (lines 91-96) with:

```rust
fn map_grant_err(e: DbErr, role_key: &str) -> AuthzError {
    match e.sql_err() {
        Some(SqlErr::ForeignKeyConstraintViolation(ref msg)) if msg.contains("fk_role_grant_role") => AuthzError::UnknownRole(role_key.to_string()),
        // SMA-676 D9. By constraint NAME, like the FK arm above: `uq_role_grant_linked_policy`
        // is a different defect and must stay `Backend`. The caller's transaction is now
        // aborted at the database; `RoleService::grant` drops it and reads the winner.
        Some(SqlErr::UniqueConstraintViolation(ref msg)) if msg.contains("uq_role_grant_principal_role_scope") => AuthzError::DuplicateGrant,
        _ => map_err(e),
    }
}
```

Extend the `map_grant_err` doc comment with one sentence: `SMA-676 adds the unique-violation arm: a duplicate (principal, role, scope) is AuthzError::DuplicateGrant.`

Replace the comment at lines 186-190 inside `grant` with:

```rust
        // A `uq_role_grant_principal_role_scope` violation surfaces here as
        // `AuthzError::DuplicateGrant` (SMA-676 D9; `map_grant_err`). A
        // `uq_role_grant_linked_policy` violation stays `AuthzError::Backend`. Either way
        // dropping `tx` without committing rolls the failed insert back.
```

After the `impl RoleGrantStore for PgRoleGrantStore` block add:

```rust
/// `RoleGrantQuery::find` (SMA-676). One statement: the inner join on `principal` supplies the
/// kind (every grant has a principal row, `fk_role_grant_principal`, so the join drops nothing
/// unless a kind filter asks it to). A NULL parameter means "no filter on this column".
const FIND_SQL: &str = r#"
SELECT g.id, g.principal_id, g.role_key, g.scope_kind, g.scope_node_prn, g.scope_org_id,
       g.scope_team_id, g.scope_project_id, g.linked_policy_id, g.created_at
  FROM "role_grant" g JOIN "principal" pr ON pr.id = g.principal_id
 WHERE ($1::uuid IS NULL OR g.principal_id = $1)
   AND ($2::text IS NULL OR g.scope_node_prn = $2)
   AND ($3::boolean IS FALSE OR g.scope_kind = 'root')
   AND ($4::text IS NULL OR g.role_key = $4)
   AND ($5::text IS NULL OR pr.kind = $5)
 ORDER BY g.principal_id, g.id
 LIMIT $6 OFFSET $7"#;

/// `FIND_SQL`'s row. Mirrors `role_grant::Model` field for field, so `model_to_grant` stays the
/// one reconstruction path.
#[derive(Debug, FromQueryResult)]
struct GrantRow {
    id: Uuid,
    principal_id: Uuid,
    role_key: String,
    scope_kind: String,
    scope_node_prn: String,
    scope_org_id: Option<Uuid>,
    scope_team_id: Option<Uuid>,
    scope_project_id: Option<Uuid>,
    linked_policy_id: String,
    created_at: DateTime<Utc>,
}

impl From<GrantRow> for role_grant::Model {
    fn from(r: GrantRow) -> Self {
        role_grant::Model {
            id: r.id,
            principal_id: r.principal_id,
            role_key: r.role_key,
            scope_kind: r.scope_kind,
            scope_node_prn: r.scope_node_prn,
            scope_org_id: r.scope_org_id,
            scope_team_id: r.scope_team_id,
            scope_project_id: r.scope_project_id,
            linked_policy_id: r.linked_policy_id,
            created_at: r.created_at,
        }
    }
}

#[async_trait]
impl RoleGrantQuery for PgRoleGrantStore {
    async fn find(&self, f: &RoleGrantFilter, limit: u64, offset: u64) -> Result<Vec<RoleGrant>, AuthzError> {
        let principal: Option<Uuid> = f.principal().map(PrincipalId::uuid);
        // D5: the Root sentinel matches `scope_kind = 'root'`; a node matches its stored
        // canonical PRN exactly (the column the duplicate key already uses).
        let (scope_prn, root): (Option<String>, bool) = match f.scope() {
            None => (None, false),
            Some(GrantScope::Root) => (None, true),
            Some(scope @ GrantScope::Node(_)) => (Some(scope.canonical_prn()), false),
        };
        let role_key: Option<String> = f.role_key().map(str::to_owned);
        let kind: Option<String> = f.principal_kind().map(|k| k.as_str().to_owned());
        let stmt = Statement::from_sql_and_values(
            DbBackend::Postgres,
            FIND_SQL,
            [principal.into(), scope_prn.into(), root.into(), role_key.into(), kind.into(), limit.into(), offset.into()],
        );
        let rows = GrantRow::find_by_statement(stmt).all(&self.db).await.map_err(map_err)?;
        rows.into_iter().map(|r| model_to_grant(r.into())).collect()
    }
}
```

- [ ] **Step 4: Implement the in-memory fake**

In `fakes.rs`, add `PrincipalKind, RoleGrantFilter, RoleGrantQuery` to the `paigasus_iam_core::{…}` import. After `impl RoleGrantStore for InMemoryRoleGrants { … }` add:

```rust
/// In-memory `RoleGrantQuery` fake (SMA-676). It reads the SAME map as the
/// `InMemoryRoleGrants` it was built over, so a grant written through the store is visible to
/// the query. `kinds` stands in for the `principal` table's `kind` column: a principal with no
/// entry has no row, and `PgRoleGrantStore::find`'s inner join drops its grants from a
/// kind-filtered answer, so this fake drops them too.
#[derive(Clone, Default)]
pub struct InMemoryRoleGrantQuery {
    grants: Arc<Mutex<HashMap<Uuid, RoleGrant>>>,
    kinds: Arc<Mutex<HashMap<Uuid, PrincipalKind>>>,
}

impl InMemoryRoleGrantQuery {
    pub fn over(store: &InMemoryRoleGrants) -> Self {
        Self {
            grants: store.0.clone(),
            kinds: Arc::default(),
        }
    }

    pub fn set_kind(&self, principal: &PrincipalId, kind: PrincipalKind) {
        self.kinds.lock().unwrap().insert(principal.uuid(), kind);
    }
}

#[async_trait]
impl RoleGrantQuery for InMemoryRoleGrantQuery {
    async fn find(&self, f: &RoleGrantFilter, limit: u64, offset: u64) -> Result<Vec<RoleGrant>, AuthzError> {
        let kinds = self.kinds.lock().unwrap().clone();
        let mut hits: Vec<RoleGrant> = self.grants.lock().unwrap().values().filter(|g| f.matches(g, kinds.get(&g.principal.uuid()).copied())).cloned().collect();
        hits.sort_by_key(|g| (g.principal.uuid(), g.id));
        Ok(hits.into_iter().skip(offset as usize).take(limit as usize).collect())
    }
}
```

- [ ] **Step 5: Run the tests and see them pass**

Run:

```bash
cd rs
cargo nextest run -p paigasus-iam --lib role_grant_query_fake_tests
PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test authz_role_grants
cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings
```

Expected: PASS. The Docker suite needs a running Docker daemon; with `PAIGASUS_REQUIRE_DOCKER=1` a missing daemon fails loudly instead of skipping.

- [ ] **Step 6: Commit**

```bash
git add rs/crates/services/paigasus-iam
git commit -F - <<'EOF'
feat(rs): implement RoleGrantQuery for Postgres and the fakes, and map the duplicate grant

SMA-676 D5, D6, D9. One statement with an inner join on principal for
the kind, an exact scope_node_prn match, and principal_id, id order.
map_grant_err raises DuplicateGrant for uq_role_grant_principal_role_scope
only; a linked-policy collision stays Backend.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
```

---

## Task 4: `RoleService::list` (D3, D4, D6, D7) and the gRPC/HTTP wiring

**Files:**
- Create: `rs/crates/services/paigasus-iam/src/application/principal_kind.rs`
- Modify: `rs/crates/services/paigasus-iam/src/application/mod.rs:21` (add the module)
- Modify: `rs/crates/services/paigasus-iam/src/application/roles.rs:1-8` (module doc), `:41-51` (imports), `:91-153` (struct, deps, `new`), `:307-319` (`list`), tests `:322-692`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/grpc/convert.rs` after `:193` (`to_page`), tests module
- Modify: `rs/crates/services/paigasus-iam/src/adapters/grpc/authz.rs:41` (drop `require_present` import), `:226-245`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/http/authz.rs:145-150`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/http/dto.rs:418-429`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/http/mod.rs:535-547`
- Modify: `rs/crates/services/paigasus-iam/tests/authz_bootstrap.rs:207-219`
- Modify: `rs/crates/services/paigasus-iam/tests/grpc_authz.rs` (new test + `reason_of`)

**Interfaces:**
- Consumes: `RoleGrantFilter`, `RoleGrantQuery`, `InMemoryRoleGrantQuery`, `TenancyError::InvalidPrincipalKind`, proto `PrincipalKind`.
- Produces:

```rust
// application/principal_kind.rs
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PrincipalKindFilter { #[default] Any, Only(PrincipalKind), Unknown }
impl PrincipalKindFilter {
    pub fn from_query(raw: Option<&str>) -> Self;
    pub fn resolve(self) -> Result<Option<PrincipalKind>, TenancyError>;
}
// application/roles.rs
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ListRoleGrantsInput { pub principal_prn: Option<String>, pub scope_prn: Option<String>, pub role_key: Option<String>, pub principal_kind: PrincipalKindFilter, pub limit: Option<i64>, pub offset: Option<i64> }
pub struct RoleServiceDeps<I, C> { /* existing fields */ pub query: Arc<dyn RoleGrantQuery>, }
impl RoleService { pub async fn list(&self, actor: &Prn, input: ListRoleGrantsInput) -> Result<Vec<RoleGrant>, TenancyError>; }
// adapters/grpc/convert.rs
pub fn principal_kind_filter(raw: i32) -> PrincipalKindFilter;
// adapters/http/dto.rs
pub struct RoleGrantQuery { pub principal_prn: Option<String>, pub scope_prn: Option<String>, pub role_key: Option<String>, pub principal_kind: Option<String>, pub limit: Option<i64>, pub offset: Option<i64> }
```

- [ ] **Step 1: Write the failing tests**

Create `rs/crates/services/paigasus-iam/src/application/principal_kind.rs` with ONLY the test module first (the type comes in Step 3):

```rust
// SPDX-License-Identifier: Apache-2.0

#[cfg(test)]
mod tests {
    use super::*;

    /// SMA-676 D7 and Review Focus 3: every string that is not exactly `user` or
    /// `service_account` refuses. An empty value and a different case refuse too.
    #[test]
    fn principal_kind_filter_refuses_every_unknown_value() {
        assert_eq!(PrincipalKindFilter::from_query(None), PrincipalKindFilter::Any);
        assert_eq!(PrincipalKindFilter::from_query(Some("user")), PrincipalKindFilter::Only(PrincipalKind::User));
        assert_eq!(PrincipalKindFilter::from_query(Some("service_account")), PrincipalKindFilter::Only(PrincipalKind::ServiceAccount));
        for raw in ["", "users", "USER", " user", "any"] {
            assert_eq!(PrincipalKindFilter::from_query(Some(raw)), PrincipalKindFilter::Unknown, "{raw:?}");
        }
        assert_eq!(PrincipalKindFilter::Unknown.resolve(), Err(TenancyError::InvalidPrincipalKind("principal_kind")));
        assert_eq!(PrincipalKindFilter::Any.resolve(), Ok(None));
        assert_eq!(PrincipalKindFilter::Only(PrincipalKind::User).resolve(), Ok(Some(PrincipalKind::User)));
    }
}
```

Add `pub mod principal_kind;` to `application/mod.rs` after `pub mod principal_context;`.

In `rs/crates/services/paigasus-iam/src/adapters/grpc/convert.rs` tests module, append:

```rust
    /// SMA-676 D7 and Review Focus 3: a proto3 enum is open, so an unknown number reaches the
    /// server. It maps to `Unknown` (refused by the service), never to `Any`.
    #[test]
    fn principal_kind_filter_maps_the_wire_and_refuses_unknown_values() {
        use crate::application::principal_kind::PrincipalKindFilter;
        assert_eq!(principal_kind_filter(0), PrincipalKindFilter::Any);
        assert_eq!(principal_kind_filter(1), PrincipalKindFilter::Only(paigasus_iam_core::PrincipalKind::User));
        assert_eq!(principal_kind_filter(2), PrincipalKindFilter::Only(paigasus_iam_core::PrincipalKind::ServiceAccount));
        assert_eq!(principal_kind_filter(3), PrincipalKindFilter::Unknown);
        assert_eq!(principal_kind_filter(-1), PrincipalKindFilter::Unknown);
    }
```

In `rs/crates/services/paigasus-iam/src/application/roles.rs` tests:
- change the import line to add `InMemoryRoleGrantQuery`:

```rust
    use crate::application::fakes::{
        FakeAuditLog, FakeAuthorizer, FakeOutbox, FakePolicyGenBumper, FakeUnitOfWork, FixedClock, InMemoryOrgs, InMemoryProjects, InMemoryRoleGrantQuery, InMemoryRoleGrants, InMemoryTeams, SeqIds,
        TenancyStore, test_stamp,
    };
    use crate::application::principal_kind::PrincipalKindFilter;
```

- add `PrincipalKind` to the `paigasus_iam_core::{…}` test import;
- replace `new_service_with_store` and `new_service_with_fakes` with:

```rust
    fn new_service_with_store(fake: FakeAuthorizer, store: TenancyStore) -> RoleService<SeqIds, FixedClock> {
        let grants = InMemoryRoleGrants::default();
        let query = InMemoryRoleGrantQuery::over(&grants);
        new_service_with_fakes(fake, Arc::new(grants), Arc::new(query), store).svc
    }

    fn new_service_with_fakes(fake: FakeAuthorizer, grants: Arc<dyn RoleGrantStore>, query: Arc<dyn RoleGrantQuery>, store: TenancyStore) -> ServiceWithFakes {
        let outbox = FakeOutbox::default();
        let audit = FakeAuditLog::default();
        let bumper = FakePolicyGenBumper::default();
        let svc = RoleService::new(RoleServiceDeps {
            grants,
            query,
            orgs: Arc::new(InMemoryOrgs(store.clone())),
            teams: Arc::new(InMemoryTeams(store.clone())),
            projects: Arc::new(InMemoryProjects(store)),
            authorize: Authorize::new(Arc::new(fake)),
            uow: Arc::new(FakeUnitOfWork::default()),
            outbox: Arc::new(outbox.clone()),
            audit: Arc::new(audit.clone()),
            gen_bumper: Arc::new(bumper.clone()),
            ids: SeqIds::default(),
            clock: FixedClock::default(),
        });
        ServiceWithFakes { svc, outbox, audit, bumper }
    }
```

- update the three other callers of `new_service_with_fakes` to pass a query: `grant_emits_one_event_…` passes `Arc::new(InMemoryRoleGrants::default())` and `Arc::new(InMemoryRoleGrantQuery::default())`; `a_store_error_mid_txn_…` passes `Arc::new(FailingGrantStore)` and `Arc::new(InMemoryRoleGrantQuery::default())`; `revoke_of_a_grant_that_vanished_…` passes `Arc::new(VanishesBeforeRevoke { grant: grant.clone() })` and `Arc::new(InMemoryRoleGrantQuery::default())`.
- add these helpers and replace every `svc.list(&x, &y.canonical())` call (lines 510, 612, 651, 686, 690) with `svc.list(&x, by_principal(&y.canonical()))`:

```rust
    fn by_principal(prn: &str) -> ListRoleGrantsInput {
        ListRoleGrantsInput {
            principal_prn: Some(prn.to_string()),
            ..ListRoleGrantsInput::default()
        }
    }

    fn at_scope(scope_prn: &str) -> ListRoleGrantsInput {
        ListRoleGrantsInput {
            scope_prn: Some(scope_prn.to_string()),
            ..ListRoleGrantsInput::default()
        }
    }

    fn org_scope(n: u128) -> GrantScope {
        GrantScope::Node(TenancyNodeRef::Organization(OrganizationId::from_uuid(Uuid::from_u128(n))))
    }

    /// A service plus direct handles on its grant store and query, for the `list` tests.
    struct ListHarness {
        svc: RoleService<SeqIds, FixedClock>,
        grants: InMemoryRoleGrants,
        query: InMemoryRoleGrantQuery,
    }

    fn list_harness(fake: FakeAuthorizer) -> ListHarness {
        let grants = InMemoryRoleGrants::default();
        let query = InMemoryRoleGrantQuery::over(&grants);
        let svc = new_service_with_fakes(fake, Arc::new(grants.clone()), Arc::new(query.clone()), TenancyStore::default()).svc;
        ListHarness { svc, grants, query }
    }

    fn seed(h: &ListHarness, id: u128, principal: u128, role: &str, scope: GrantScope, kind: PrincipalKind) -> RoleGrant {
        let g = RoleGrant {
            id: Uuid::from_u128(id),
            principal: PrincipalId::from_prn(principal_prn(principal)),
            role_key: role.to_string(),
            scope,
            linked_policy_id: format!("grant:{}", Uuid::from_u128(id)),
            created_at: Utc.timestamp_opt(0, 0).unwrap(),
        };
        h.grants.0.lock().unwrap().insert(g.id, g.clone());
        h.query.set_kind(&g.principal, kind);
        g
    }
```

- append the `list` tests:

```rust
    /// D4 (a): an actor may list their OWN grants with no check, with or without a scope.
    #[tokio::test]
    async fn list_self_needs_no_check_even_with_a_scope() {
        let h = list_harness(FakeAuthorizer::default());
        let mine = seed(&h, 10, 1, "gateway_user", org_scope(100), PrincipalKind::User);
        let input = ListRoleGrantsInput {
            scope_prn: Some(org_prn(100).canonical()),
            ..by_principal(&principal_prn(1).canonical())
        };
        assert_eq!(h.svc.list(&principal_prn(1), input).await.unwrap(), vec![mine]);
    }

    /// D4 (b): the scope path checks `ListRoleGrants` at the scope node, and the filters AND.
    #[tokio::test]
    async fn list_scope_path_checks_at_the_scope_node_and_filters() {
        let fake = FakeAuthorizer::default();
        fake.allow(Action::ListRoleGrants, &org_prn(100));
        let h = list_harness(fake);
        let person = seed(&h, 11, 2, "gateway_user", org_scope(100), PrincipalKind::User);
        seed(&h, 12, 3, "gateway_user", org_scope(100), PrincipalKind::ServiceAccount);
        seed(&h, 13, 2, "org_admin", org_scope(100), PrincipalKind::User);
        seed(&h, 14, 4, "gateway_user", org_scope(101), PrincipalKind::User);
        let input = ListRoleGrantsInput {
            role_key: Some("gateway_user".to_string()),
            principal_kind: PrincipalKindFilter::Only(PrincipalKind::User),
            ..at_scope(&org_prn(100).canonical())
        };
        assert_eq!(h.svc.list(&principal_prn(1), input).await.unwrap(), vec![person]);
    }

    #[tokio::test]
    async fn list_scope_path_denies_without_the_grant_at_that_scope() {
        let fake = FakeAuthorizer::default();
        fake.allow(Action::ListRoleGrants, &org_prn(101));
        let h = list_harness(fake);
        assert_eq!(h.svc.list(&principal_prn(1), at_scope(&org_prn(100).canonical())).await.unwrap_err(), TenancyError::Forbidden);
    }

    /// D4 (c): another principal with no scope still needs `ListRoleGrants` at Root.
    #[tokio::test]
    async fn list_other_principal_without_scope_checks_at_root() {
        let fake = FakeAuthorizer::default();
        fake.allow(Action::ListRoleGrants, &org_prn(100));
        let h = list_harness(fake.clone());
        assert_eq!(h.svc.list(&principal_prn(1), by_principal(&principal_prn(2).canonical())).await.unwrap_err(), TenancyError::Forbidden);
        fake.allow(Action::ListRoleGrants, &root_prn());
        assert!(h.svc.list(&principal_prn(1), by_principal(&principal_prn(2).canonical())).await.is_ok());
    }

    /// D5: the Root sentinel as a scope matches Root grants, and D4 (b) checks at Root.
    #[tokio::test]
    async fn list_root_sentinel_scope_checks_at_root() {
        let fake = FakeAuthorizer::default();
        fake.allow(Action::ListRoleGrants, &org_prn(100));
        let h = list_harness(fake.clone());
        let root_grant = seed(&h, 15, 2, "platform_admin", GrantScope::Root, PrincipalKind::User);
        seed(&h, 16, 2, "org_admin", org_scope(100), PrincipalKind::User);
        assert_eq!(h.svc.list(&principal_prn(1), at_scope(&root_prn().canonical())).await.unwrap_err(), TenancyError::Forbidden);
        fake.allow(Action::ListRoleGrants, &root_prn());
        assert_eq!(h.svc.list(&principal_prn(1), at_scope(&root_prn().canonical())).await.unwrap(), vec![root_grant]);
    }

    /// D3: neither a principal nor a scope is `MissingRequiredField("principal_prn|scope_prn")`.
    #[tokio::test]
    async fn list_with_neither_principal_nor_scope_is_missing_required_field() {
        let h = list_harness(FakeAuthorizer::default());
        let input = ListRoleGrantsInput {
            role_key: Some("gateway_user".to_string()),
            ..ListRoleGrantsInput::default()
        };
        assert_eq!(h.svc.list(&principal_prn(1), input).await.unwrap_err(), TenancyError::MissingRequiredField("principal_prn|scope_prn"));
    }

    /// Review Focus 5: a whitespace-only field is "absent", not a PRN to parse.
    #[tokio::test]
    async fn list_treats_blank_strings_as_absent() {
        let h = list_harness(FakeAuthorizer::default());
        let input = ListRoleGrantsInput {
            principal_prn: Some("   ".to_string()),
            scope_prn: Some(String::new()),
            role_key: Some(" ".to_string()),
            ..ListRoleGrantsInput::default()
        };
        assert_eq!(h.svc.list(&principal_prn(1), input).await.unwrap_err(), TenancyError::MissingRequiredField("principal_prn|scope_prn"));
    }

    /// D7: an unknown kind is refused, even on the self path, and never read as "any".
    #[tokio::test]
    async fn list_refuses_an_unknown_kind_even_for_self() {
        let h = list_harness(FakeAuthorizer::default());
        let input = ListRoleGrantsInput {
            principal_kind: PrincipalKindFilter::Unknown,
            ..by_principal(&principal_prn(1).canonical())
        };
        assert_eq!(h.svc.list(&principal_prn(1), input).await.unwrap_err(), TenancyError::InvalidPrincipalKind("principal_kind"));
    }

    /// D6: the scope path honours `Page::new` (1..=200) and orders by principal, then id.
    #[tokio::test]
    async fn list_scope_path_honours_page_bounds_and_order() {
        let fake = FakeAuthorizer::default();
        fake.allow(Action::ListRoleGrants, &org_prn(100));
        let h = list_harness(fake);
        let b = seed(&h, 21, 3, "gateway_user", org_scope(100), PrincipalKind::User);
        let a2 = seed(&h, 22, 2, "org_admin", org_scope(100), PrincipalKind::User);
        seed(&h, 20, 2, "gateway_user", org_scope(100), PrincipalKind::User);
        let too_big = ListRoleGrantsInput {
            limit: Some(201),
            ..at_scope(&org_prn(100).canonical())
        };
        assert_eq!(h.svc.list(&principal_prn(1), too_big).await.unwrap_err(), TenancyError::InvalidPagination);
        let second_page = ListRoleGrantsInput {
            limit: Some(2),
            offset: Some(1),
            ..at_scope(&org_prn(100).canonical())
        };
        assert_eq!(h.svc.list(&principal_prn(1), second_page).await.unwrap(), vec![a2, b]);
    }

    /// D6 and Review Focus 2: the principal-only path is unchanged — every row, and `limit`/
    /// `offset` are ignored, even out of `Page`'s bounds.
    #[tokio::test]
    async fn list_principal_only_path_ignores_limit_and_offset() {
        let h = list_harness(FakeAuthorizer::default());
        for id in 30..33 {
            seed(&h, id, 1, "gateway_user", org_scope(100 + id), PrincipalKind::User);
        }
        let input = ListRoleGrantsInput {
            limit: Some(500),
            offset: Some(-1),
            ..by_principal(&principal_prn(1).canonical())
        };
        assert_eq!(h.svc.list(&principal_prn(1), input).await.unwrap().len(), 3);
    }
```

In `rs/crates/services/paigasus-iam/tests/grpc_authz.rs`, change the proto import to include `PrincipalKind as ProtoPrincipalKind`, and add:

```rust
/// Reads `ErrorInfo.reason` off a `tonic::Status` (mirrors `tests/grpc_whoami.rs::reason_of`).
fn reason_of(err: &tonic::Status) -> String {
    let details = tonic_types::StatusExt::get_error_details(err);
    details.error_info().expect("every IAM status carries ErrorInfo").reason.clone()
}

/// SMA-676 D3, D6, D7 over gRPC: no principal and no scope is refused; an unknown kind is
/// refused; a scope page over 200 is refused; a scope request with a kind and a page works.
#[tokio::test]
async fn list_role_grants_over_grpc_applies_the_scope_path_rules() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let state = AppState::new(db, &support::test_config(&idp)).await.unwrap();
    let (addr, server) = spawn_server(state.clone()).await;
    let mut authz = AuthorizationServiceClient::new(channel(addr).await);

    let admin_token = idp.bearer("grpc-scope-admin", Some("grpc-scope-admin@example.com"), "paigasus", 3600);
    let admin_prn = support::provision(&state, &admin_token).await;
    support::seed_platform_admin(&state, &admin_prn).await;

    let err = authz.list_role_grants(authed(ListRoleGrantsRequest::default(), &admin_token)).await.unwrap_err();
    assert_eq!(err.code(), Code::InvalidArgument);
    assert_eq!(reason_of(&err), "missing-required-field");

    let unknown_kind = ListRoleGrantsRequest {
        scope_prn: root_prn().canonical(),
        principal_kind: 7,
        ..Default::default()
    };
    let err = authz.list_role_grants(authed(unknown_kind, &admin_token)).await.unwrap_err();
    assert_eq!(err.code(), Code::InvalidArgument);
    assert_eq!(reason_of(&err), "invalid-principal-kind");

    let too_big = ListRoleGrantsRequest {
        scope_prn: root_prn().canonical(),
        limit: 201,
        ..Default::default()
    };
    let err = authz.list_role_grants(authed(too_big, &admin_token)).await.unwrap_err();
    assert_eq!(reason_of(&err), "invalid-pagination");

    let users_at_root = ListRoleGrantsRequest {
        scope_prn: root_prn().canonical(),
        principal_kind: ProtoPrincipalKind::User as i32,
        limit: 200,
        ..Default::default()
    };
    let listed = authz.list_role_grants(authed(users_at_root, &admin_token)).await.unwrap().into_inner().grants;
    assert!(listed.iter().any(|g| g.principal_prn == admin_prn && g.role_key == "platform_admin"), "{listed:?}");

    server.abort();
}
```

- [ ] **Step 2: Run the tests and see them fail**

Run: `cd rs && cargo nextest run -p paigasus-iam --lib principal_kind roles::tests convert::tests`
Expected: FAIL — compile errors: `cannot find type PrincipalKindFilter`, `cannot find struct ListRoleGrantsInput`, `struct RoleServiceDeps has no field named query`, `cannot find function principal_kind_filter`.

- [ ] **Step 3: Implement `PrincipalKindFilter`**

Put this ABOVE the test module in `application/principal_kind.rs`:

```rust
//! The principal-kind filter of `ListRoleGrants` and `ListMemberships` (SMA-676 D7, D8).
//!
//! Each transport maps its raw value into [`PrincipalKindFilter`]; the service calls
//! [`PrincipalKindFilter::resolve`]. An unknown value is its own variant, so no adapter can
//! turn it into "any kind": the refusal lives in one place for both transports (D10).

use crate::application::error::TenancyError;
use paigasus_iam_core::PrincipalKind;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PrincipalKindFilter {
    /// No filter (gRPC `PRINCIPAL_KIND_UNSPECIFIED`, or no HTTP parameter).
    #[default]
    Any,
    Only(PrincipalKind),
    /// A value that names no kind. [`PrincipalKindFilter::resolve`] refuses it.
    Unknown,
}

impl PrincipalKindFilter {
    /// The HTTP query value: absent is `Any`; `user` and `service_account` (the strings of
    /// `PrincipalKind::as_str`) name a kind; anything else, the empty string included, is
    /// `Unknown`.
    #[must_use]
    pub fn from_query(raw: Option<&str>) -> Self {
        match raw {
            None => Self::Any,
            Some(s) => PrincipalKind::parse(s).map_or(Self::Unknown, Self::Only),
        }
    }

    /// `None` = any kind. `Unknown` is refused (D7).
    pub fn resolve(self) -> Result<Option<PrincipalKind>, TenancyError> {
        match self {
            Self::Any => Ok(None),
            Self::Only(kind) => Ok(Some(kind)),
            Self::Unknown => Err(TenancyError::InvalidPrincipalKind("principal_kind")),
        }
    }
}
```

- [ ] **Step 4: Implement `RoleService::list` and the new dep**

In `roles.rs`, replace the module doc lines 3-8 with:

```rust
//! `RoleService`: grant/revoke/list role-grant management use cases (SMA-444 Task 17,
//! ADR-0013). `grant` enforces the anti-escalation invariant — only an actor who may already
//! `GrantRole` AT the target scope itself (Root, or a tenancy node) may grant a role there,
//! so a principal can never bootstrap authority it doesn't already hold. `list`'s exposure
//! rule (SMA-676 D4, widening SMA-444's "M3" rule): self is always visible; a request with a
//! scope needs `ListRoleGrants` AT that scope node; anyone else's grants with no scope need
//! `ListRoleGrants` at Root — see [`RoleService::list`]'s doc.
```

Change the imports (lines 41-51) to:

```rust
use crate::application::authorize::Authorize;
use crate::application::error::TenancyError;
use crate::application::pagination::Page;
use crate::application::principal_kind::PrincipalKindFilter;
use paigasus_iam_core::authz::model::root_prn;
use paigasus_iam_core::authz::roles as authz_roles;
use paigasus_iam_core::{
    Action, AuditEntry, AuditLog, AuditOutcome, Clock, DomainEvent, EventType, GrantScope, IdGenerator, OrganizationRepository, Outbox, PolicyGenBumper, PrincipalId, ProjectRepository, RoleGrant,
    RoleGrantFilter, RoleGrantQuery, RoleGrantStore, TeamRepository, TenancyNodeRef, UnitOfWork,
};
use paigasus_kernel::Prn;
use std::sync::Arc;
use uuid::Uuid;
```

After `scope_resource_prn` add:

```rust
/// A raw wire string, or `None` when the caller left it out or sent only whitespace. A proto3
/// string field is `""` when absent, so blank and absent are one case on both transports.
fn non_blank(raw: Option<String>) -> Option<String> {
    raw.filter(|s| !s.trim().is_empty())
}

/// The raw input of [`RoleService::list`]. Both adapters only move fields in (D10): the
/// service owns D3, D4, D6 and D7. `limit`/`offset` stay raw because only the query path
/// validates them (D6) — a `Page` built by the adapter would refuse `limit = 500` on the
/// principal-only path, which must keep ignoring it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ListRoleGrantsInput {
    pub principal_prn: Option<String>,
    pub scope_prn: Option<String>,
    pub role_key: Option<String>,
    pub principal_kind: PrincipalKindFilter,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}
```

Add `query: Arc<dyn RoleGrantQuery>,` after `grants` in `RoleService`, add `pub query: Arc<dyn RoleGrantQuery>,` after `pub grants` in `RoleServiceDeps`, and add `query: deps.query,` after `grants: deps.grants,` in `new`. Extend the `RoleService` doc with one sentence: `query is the SMA-676 read port (RoleGrantQuery): the list query path and grant's idempotency pre-check read through it.`

Replace `list` (lines 307-319) with:

```rust
    /// Lists role grants (SMA-676). Order of checks: parse the principal and the scope PRN
    /// (`InvalidPrn`); resolve the kind (D7, `InvalidPrincipalKind`); build the filter (D3,
    /// `MissingRequiredField("principal_prn|scope_prn")`); then authorize, BEFORE any read (D4):
    /// (a) the principal is the actor → no check; (b) else a scope is set → `ListRoleGrants` at
    /// the scope node (`scope_resource_prn`; under Cedar's `resource in ?resource`, an
    /// `org_admin` passes at its own org only, and a forged team PRN is decided against the
    /// team's STORED ancestry); (c) else → `ListRoleGrants` at Root (only `platform_admin`).
    /// Then D6: the bare principal request — the only shape callers sent before SMA-676 —
    /// returns every row through `list_by_principal` and ignores `limit`/`offset`; every other
    /// request reads `RoleGrantQuery::find` with `Page::new` (1..=200, default 50), ordered by
    /// `principal_id`, then `id`.
    pub async fn list(&self, actor: &Prn, input: ListRoleGrantsInput) -> Result<Vec<RoleGrant>, TenancyError> {
        let principal = non_blank(input.principal_prn).map(|raw| parse_principal_prn(&raw)).transpose()?;
        let scope = non_blank(input.scope_prn).map(|raw| parse_grant_scope(&raw)).transpose()?;
        let role_key = non_blank(input.role_key);
        let principal_kind = input.principal_kind.resolve()?;
        let filter = RoleGrantFilter::new(principal, scope, role_key, principal_kind).ok_or(TenancyError::MissingRequiredField("principal_prn|scope_prn"))?;

        let is_self = filter.principal().is_some_and(|p| p.canonical() == actor.canonical());
        if !is_self {
            let resource = filter.scope().map_or_else(root_prn, scope_resource_prn);
            self.authorize.check(actor, Action::ListRoleGrants, &resource).await?;
        }

        if let Some(principal) = filter.principal_only() {
            return Ok(self.grants.list_by_principal(principal).await?);
        }
        let page = Page::new(input.limit, input.offset)?;
        Ok(self.query.find(&filter, page.limit, page.offset).await?)
    }
```

- [ ] **Step 5: Wire the adapters**

`rs/crates/services/paigasus-iam/src/adapters/grpc/convert.rs`, after `to_page`:

```rust
/// SMA-676 D7: the wire `PrincipalKind` into the service's filter. A proto3 enum is open, so
/// an unknown number (for example a newer client's kind) is `Unknown`, which the service
/// refuses — never `Any`, which would widen the listing.
pub fn principal_kind_filter(raw: i32) -> crate::application::principal_kind::PrincipalKindFilter {
    use crate::application::principal_kind::PrincipalKindFilter;
    use paigasus_proto::paigasus::iam::v1::PrincipalKind as ProtoPrincipalKind;
    match ProtoPrincipalKind::try_from(raw) {
        Ok(ProtoPrincipalKind::Unspecified) => PrincipalKindFilter::Any,
        Ok(ProtoPrincipalKind::User) => PrincipalKindFilter::Only(paigasus_iam_core::PrincipalKind::User),
        Ok(ProtoPrincipalKind::ServiceAccount) => PrincipalKindFilter::Only(paigasus_iam_core::PrincipalKind::ServiceAccount),
        Err(_) => PrincipalKindFilter::Unknown,
    }
}
```

`rs/crates/services/paigasus-iam/src/adapters/grpc/authz.rs`: delete line 41 (`use super::convert::require_present;`), add `use crate::application::roles::ListRoleGrantsInput;`, and replace the body of `list_role_grants` between `let req = request.into_inner();` and `Ok(Response::new(` with:

```rust
            // SMA-676 D10: move fields only. `RoleService::list` owns D3, D4, D6 and D7. A
            // `limit` of 0 is "unset" on the wire (`to_page`'s rule).
            let input = ListRoleGrantsInput {
                principal_prn: Some(req.principal_prn),
                scope_prn: Some(req.scope_prn),
                role_key: Some(req.role_key),
                principal_kind: convert::principal_kind_filter(req.principal_kind),
                limit: (req.limit != 0).then(|| i64::from(req.limit)),
                offset: Some(req.offset as i64),
            };
            let grants = self.state.roles.list(&actor, input).await.map_err(convert::status_to_grpc)?;
```

`rs/crates/services/paigasus-iam/src/adapters/http/dto.rs`: replace `RoleGrantQuery` and its doc (lines 418-429) with:

```rust
/// Query params for `GET /v1/authz/role-grants` (SMA-676 D10). Every field is optional here:
/// `RoleService::list` refuses a request with neither `principal_prn` nor `scope_prn`
/// (`missing-required-field`, D3), refuses an unknown `principal_kind` (D7), and applies
/// `limit`/`offset` only when a filter beyond the bare principal is set (D6). `Option`, not a
/// bare `String`, so a missing parameter reaches that SPECIFIC reason rather than
/// `EnvelopeQuery`'s general `invalid-query-parameter` (SMA-588).
#[derive(Debug, Clone, Deserialize)]
pub struct RoleGrantQuery {
    pub principal_prn: Option<String>,
    pub scope_prn: Option<String>,
    pub role_key: Option<String>,
    pub principal_kind: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}
```

`rs/crates/services/paigasus-iam/src/adapters/http/authz.rs`: add `use crate::application::principal_kind::PrincipalKindFilter;` and `use crate::application::roles::ListRoleGrantsInput;`, and replace `list_role_grants` (lines 145-150) with:

```rust
async fn list_role_grants(State(s): State<AppState>, Extension(ctx): Extension<AuthContext>, EnvelopeQuery(q): EnvelopeQuery<RoleGrantQuery>) -> Result<Json<Vec<RoleGrantDto>>, ApiError> {
    let actor = actor_prn(&ctx);
    // SMA-676 D10: the same input the gRPC handler builds. The service owns every rule.
    let input = ListRoleGrantsInput {
        principal_prn: q.principal_prn,
        scope_prn: q.scope_prn,
        role_key: q.role_key,
        principal_kind: PrincipalKindFilter::from_query(q.principal_kind.as_deref()),
        limit: q.limit,
        offset: q.offset,
    };
    let grants = s.roles.list(&actor, input).await?;
    Ok(Json(grants.into_iter().map(RoleGrantDto::from).collect()))
}
```

`rs/crates/services/paigasus-iam/src/adapters/http/mod.rs`: in the `RoleServiceDeps { … }` literal (line ~536), after `grants: role_grant_store.clone(),` add:

```rust
            // SMA-676: the read port for `list`'s query path and `grant`'s idempotency
            // pre-check. A second `PgRoleGrantStore` value over the same `db` and `gens`
            // handles — the struct is not what must be shared (the SMA-477 policy-store note).
            query: Arc::new(PgRoleGrantStore::new(db.clone(), gens.clone())),
```

`rs/crates/services/paigasus-iam/tests/authz_bootstrap.rs`: in the `RoleServiceDeps { … }` literal (line 207), after `grants: role_grant_store,` add `query: Arc::new(PgRoleGrantStore::new(db.clone(), gens.clone())),`. If `PgRoleGrantStore` is not yet in that file's `paigasus_iam::adapters::persistence::{…}` import, add it there.

- [ ] **Step 6: Run the tests and see them pass**

Run:

```bash
cd rs
cargo nextest run -p paigasus-iam --lib
PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test grpc_authz --test http_authz --test authz_bootstrap --test authz_acceptance
cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings
cd .. && python3 ci/error-registry/check.py --single-site
```

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add rs/crates/services/paigasus-iam
git commit -F - <<'EOF'
feat(rs): list role grants at a scope with kind and role filters

SMA-676 D3, D4, D6, D7, D10. RoleService::list takes a raw input from
both transports and owns every rule: a principal or a scope is required,
a scope request authorizes ListRoleGrants at the scope node before any
read, the bare principal request keeps its old unpaged behaviour, and an
unknown kind is refused.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
```

---

## Task 5: Idempotent `GrantRole` (D9) and the bootstrap seeder

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/application/roles.rs:181-254` (`grant` and its doc), tests
- Modify: `rs/crates/services/paigasus-iam/src/application/bootstrap_admin.rs:94-101` (after `SeedError`), `:238-246`, tests

**Interfaces:**
- Consumes: `RoleGrantQuery` (dep `query`), `AuthzError::DuplicateGrant`.
- Produces: `RoleService::grant` returns the existing grant for a duplicate (no event, no audit, no bump); private `fn classify_seed(result: Result<(), SeedError>) -> SeedOutcome` in the seeder.

- [ ] **Step 1: Write the failing tests**

In `roles.rs` tests, add:

```rust
    /// A `RoleGrantStore` whose `grant_in` loses a concurrent-insert race: the winner's row
    /// appears in the shared map, then the insert fails with `DuplicateGrant` (D9).
    struct RaceLostGrantStore {
        map: Arc<std::sync::Mutex<std::collections::HashMap<Uuid, RoleGrant>>>,
        winner: RoleGrant,
    }

    #[async_trait]
    impl RoleGrantStore for RaceLostGrantStore {
        async fn grant(&self, _g: &RoleGrant) -> Result<(), AuthzError> {
            unimplemented!("this fake only exercises grant_in")
        }
        async fn revoke(&self, _id: Uuid) -> Result<(), AuthzError> {
            unimplemented!("this fake only exercises grant_in")
        }
        async fn grant_in(&self, _tx: &dyn Transaction, _g: &RoleGrant) -> Result<(), AuthzError> {
            self.map.lock().unwrap().insert(self.winner.id, self.winner.clone());
            Err(AuthzError::DuplicateGrant)
        }
        async fn revoke_in(&self, _tx: &dyn Transaction, _id: Uuid) -> Result<bool, AuthzError> {
            unimplemented!("this fake only exercises grant_in")
        }
        async fn list_all(&self) -> Result<Vec<RoleGrant>, AuthzError> {
            Ok(Vec::new())
        }
        async fn list_by_principal(&self, _p: &PrincipalId) -> Result<Vec<RoleGrant>, AuthzError> {
            Ok(Vec::new())
        }
        async fn find(&self, _id: Uuid) -> Result<Option<RoleGrant>, AuthzError> {
            Ok(None)
        }
    }

    /// D9: a second grant of the same role at the same scope returns the FIRST grant with OK,
    /// and writes no event, no audit row and no policy-generation bump.
    #[tokio::test]
    async fn a_second_grant_returns_the_existing_grant_and_emits_nothing() {
        let fake = FakeAuthorizer::default();
        fake.allow(Action::GrantRole, &root_prn());
        let grants = InMemoryRoleGrants::default();
        let query = InMemoryRoleGrantQuery::over(&grants);
        let ServiceWithFakes { svc, outbox, audit, bumper } = new_service_with_fakes(fake, Arc::new(grants.clone()), Arc::new(query), TenancyStore::default());
        let (actor, target) = (principal_prn(1), principal_prn(2));

        let first = svc.grant(&actor, &target.canonical(), "platform_admin", &root_prn().canonical()).await.unwrap();
        let second = svc.grant(&actor, &target.canonical(), "platform_admin", &root_prn().canonical()).await.unwrap();

        assert_eq!(second, first, "D9: the existing grant, not a new one");
        assert_eq!(grants.0.lock().unwrap().len(), 1);
        assert_eq!(outbox.0.lock().unwrap().len(), 1, "only the first grant enqueues an event");
        assert_eq!(audit.0.lock().unwrap().len(), 1, "only the first grant records an audit row");
        assert_eq!(bumper.calls(), 1, "only the first grant bumps policy_gen");
    }

    /// D9, §6: the pre-check runs AFTER the authorization check, so an actor without
    /// `GrantRole` at the scope learns nothing about an existing grant.
    #[tokio::test]
    async fn the_existing_grant_is_not_revealed_to_an_actor_without_grant_role() {
        let grants = InMemoryRoleGrants::default();
        let existing = RoleGrant {
            id: Uuid::from_u128(55),
            principal: PrincipalId::from_prn(principal_prn(2)),
            role_key: "platform_admin".to_string(),
            scope: GrantScope::Root,
            linked_policy_id: "grant:55".to_string(),
            created_at: Utc.timestamp_opt(0, 0).unwrap(),
        };
        grants.0.lock().unwrap().insert(existing.id, existing);
        let query = InMemoryRoleGrantQuery::over(&grants);
        let svc = new_service_with_fakes(FakeAuthorizer::default(), Arc::new(grants), Arc::new(query), TenancyStore::default()).svc;
        let err = svc.grant(&principal_prn(1), &principal_prn(2).canonical(), "platform_admin", &root_prn().canonical()).await.unwrap_err();
        assert_eq!(err, TenancyError::Forbidden);
    }

    /// D9 and Review Focus 4: a concurrent insert won. The transaction rolled back; `grant`
    /// reads the winner's grant and returns it, and emits nothing of its own.
    #[tokio::test]
    async fn a_lost_insert_race_returns_the_winner_s_grant_and_emits_nothing() {
        let fake = FakeAuthorizer::default();
        fake.allow(Action::GrantRole, &root_prn());
        let shared = InMemoryRoleGrants::default();
        let winner = RoleGrant {
            id: Uuid::from_u128(777),
            principal: PrincipalId::from_prn(principal_prn(2)),
            role_key: "platform_admin".to_string(),
            scope: GrantScope::Root,
            linked_policy_id: "grant:777".to_string(),
            created_at: Utc.timestamp_opt(0, 0).unwrap(),
        };
        let store = RaceLostGrantStore { map: shared.0.clone(), winner: winner.clone() };
        let query = InMemoryRoleGrantQuery::over(&shared);
        let ServiceWithFakes { svc, outbox, audit, bumper } = new_service_with_fakes(fake, Arc::new(store), Arc::new(query), TenancyStore::default());

        let got = svc.grant(&principal_prn(1), &principal_prn(2).canonical(), "platform_admin", &root_prn().canonical()).await.unwrap();

        assert_eq!(got, winner);
        assert!(outbox.0.lock().unwrap().is_empty(), "the loser enqueues no event");
        assert!(audit.0.lock().unwrap().is_empty(), "the loser records no audit row");
        assert_eq!(bumper.calls(), 0, "the loser does not bump; the winner already did");
    }
```

In `bootstrap_admin.rs` tests, add:

```rust
    /// A store whose insert always loses the race to a concurrent seed (SMA-676 D9).
    #[derive(Default)]
    struct DuplicateGrants;

    #[async_trait::async_trait]
    impl RoleGrantStore for DuplicateGrants {
        async fn list_by_principal(&self, _p: &PrincipalId) -> Result<Vec<RoleGrant>, AuthzError> {
            Ok(Vec::new())
        }
        async fn grant_in(&self, _tx: &dyn paigasus_iam_core::Transaction, _g: &RoleGrant) -> Result<(), AuthzError> {
            Err(AuthzError::DuplicateGrant)
        }
        async fn grant(&self, _g: &RoleGrant) -> Result<(), AuthzError> {
            unimplemented!("the seeder only uses grant_in")
        }
        async fn revoke(&self, _id: Uuid) -> Result<(), AuthzError> {
            unimplemented!("the seeder never revokes")
        }
        async fn revoke_in(&self, _tx: &dyn paigasus_iam_core::Transaction, _id: Uuid) -> Result<bool, AuthzError> {
            unimplemented!("the seeder never revokes")
        }
        async fn list_all(&self) -> Result<Vec<RoleGrant>, AuthzError> {
            Ok(Vec::new())
        }
        async fn find(&self, _id: Uuid) -> Result<Option<RoleGrant>, AuthzError> {
            unimplemented!("the seeder never looks up by id")
        }
    }

    /// SMA-676 D9: a concurrent seed that already won is a SUCCESS, not a "lockout" warning.
    /// A thread-local recorder proves the failure counter is not emitted; `#[tokio::test]` is
    /// current-thread, so the recorder sees every increment of this future.
    #[tokio::test]
    async fn a_concurrent_seed_that_already_won_is_not_a_seed_failure() {
        let recorder = metrics_util::debugging::DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _guard = metrics::set_default_local_recorder(&recorder);
        let seeder = BootstrapAdminSeeder::new(BootstrapAdminSeederDeps {
            admins_config: admin_cfg(),
            grants: Arc::new(DuplicateGrants),
            uow: Arc::new(FakeUnitOfWork::default()),
            outbox: Arc::new(FakeOutbox::default()),
            audit: Arc::new(FakeAuditLog::default()),
            gen_bumper: Arc::new(FakePolicyGenBumper::default()),
            ids: SeqIds::default(),
            clock: FixedClock::default(),
        });

        seeder.ensure_platform_admin(&principal(1), &issuer("https://idp.example.com"), "sub-admin").await;

        let failures = snapshotter.snapshot().into_vec().into_iter().filter(|(key, ..)| key.key().name() == names::IAM_BOOTSTRAP_ADMIN_SEED_FAILURES_TOTAL).count();
        assert_eq!(failures, 0, "a lost seed race must not count as a seed failure");
    }

    /// Control for the test above: a real write failure still counts.
    #[tokio::test]
    async fn a_failed_seed_write_still_counts_as_a_seed_failure() {
        let recorder = metrics_util::debugging::DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _guard = metrics::set_default_local_recorder(&recorder);
        let seeder = BootstrapAdminSeeder::new(BootstrapAdminSeederDeps {
            admins_config: admin_cfg(),
            grants: Arc::new(FailingGrants),
            uow: Arc::new(FakeUnitOfWork::default()),
            outbox: Arc::new(FakeOutbox::default()),
            audit: Arc::new(FakeAuditLog::default()),
            gen_bumper: Arc::new(FakePolicyGenBumper::default()),
            ids: SeqIds::default(),
            clock: FixedClock::default(),
        });

        seeder.ensure_platform_admin(&principal(1), &issuer("https://idp.example.com"), "sub-admin").await;

        let failures = snapshotter.snapshot().into_vec().into_iter().filter(|(key, ..)| key.key().name() == names::IAM_BOOTSTRAP_ADMIN_SEED_FAILURES_TOTAL).count();
        assert_eq!(failures, 1);
    }
```

- [ ] **Step 2: Run the tests and see them fail**

Run: `cd rs && cargo nextest run -p paigasus-iam --lib roles::tests bootstrap_admin::tests`
Expected: FAIL — `a_second_grant_returns_the_existing_grant_and_emits_nothing` fails with `TenancyError::Internal` (the in-memory store has no uniqueness, so it inserts a second row: `grants.len() == 2` → assertion `second == first` fails); `a_lost_insert_race_…` fails with `Err(Internal)`; `a_concurrent_seed_that_already_won_…` fails with `failures == 1`.

- [ ] **Step 3: Implement D9 in `grant`**

In `roles.rs`, add this private helper inside `impl<I, C> RoleService<I, C>` before `grant`:

```rust
    /// The grant of `role_key` to `principal` at `scope`, if one exists (SMA-676 D9). An exact
    /// (principal, role, scope) filter: the duplicate key `uq_role_grant_principal_role_scope`.
    async fn existing_grant(&self, principal: &PrincipalId, role_key: &str, scope: &GrantScope) -> Result<Option<RoleGrant>, TenancyError> {
        let filter = RoleGrantFilter::new(Some(principal.clone()), Some(scope.clone()), Some(role_key.to_string()), None).ok_or(TenancyError::Internal)?;
        Ok(self.query.find(&filter, 1, 0).await?.into_iter().next())
    }
```

In `grant`, after `self.resolve_scope(&scope).await?;` add:

```rust
        // SMA-676 D9: idempotent. AFTER the authorization check, so a caller without
        // `GrantRole` at the scope learns nothing about existing grants (§6).
        if let Some(existing) = self.existing_grant(&principal, role.key.as_str(), &scope).await? {
            return Ok(existing);
        }
```

Replace `self.grants.grant_in(&*tx, &grant).await?;` with:

```rust
        match self.grants.grant_in(&*tx, &grant).await {
            Ok(()) => {}
            // D9: a concurrent insert of the same (principal, role, scope) won. The database
            // aborted this transaction; drop it (a rollback) and return the winner's grant.
            // The winner already wrote its event and audit row and bumped policy_gen.
            Err(paigasus_iam_core::AuthzError::DuplicateGrant) => {
                drop(tx);
                return self.existing_grant(&grant.principal, &grant.role_key, &grant.scope).await?.ok_or(TenancyError::Internal);
            }
            Err(e) => return Err(e.into()),
        }
```

Add to the end of `grant`'s doc comment: `SMA-676 D9: after step 6, an existing grant for the same (principal, role, scope) is returned with OK — no event, no audit row, no bump. If the insert loses a race (AuthzError::DuplicateGrant), the transaction is dropped and the winner's grant is returned.`

If `role.key` is a `String`, `role.key.as_str()` is correct; the grant later moves `role.key`, so the pre-check must borrow before the `RoleGrant { role_key: role.key, … }` literal (it does, by position).

- [ ] **Step 4: Implement the seeder rule**

In `bootstrap_admin.rs`, after `enum SeedError { … }` add:

```rust
/// What one seed attempt means for the caller (SMA-676 D9).
#[derive(Debug)]
enum SeedOutcome {
    Seeded,
    /// A concurrent seed of the same grant won the unique key: the admin IS seeded.
    AlreadySeeded,
    Failed(SeedError),
}

fn classify_seed(result: Result<(), SeedError>) -> SeedOutcome {
    match result {
        Ok(()) => SeedOutcome::Seeded,
        Err(SeedError::Authz(paigasus_iam_core::AuthzError::DuplicateGrant)) => SeedOutcome::AlreadySeeded,
        Err(e) => SeedOutcome::Failed(e),
    }
}
```

Replace the `if let Err(e) = self.seed_grant(&grant, issuer).await { … }` block (lines 239-245) with:

```rust
        match classify_seed(self.seed_grant(&grant, issuer).await) {
            SeedOutcome::Seeded | SeedOutcome::AlreadySeeded => {}
            SeedOutcome::Failed(e) => {
                counter!(names::IAM_BOOTSTRAP_ADMIN_SEED_FAILURES_TOTAL, "stage" => "txn").increment(1);
                tracing::warn!(
                    principal = %principal.canonical(),
                    error = ?e,
                    "bootstrap-admin seeding: failed to persist the platform_admin grant with its audit row; will retry on the next authentication. If this persists the bootstrap admin is NEVER seeded (lockout) — seed it manually and record the matching audit row"
                );
            }
        }
```

- [ ] **Step 5: Run the tests and see them pass**

Run: `cd rs && cargo nextest run -p paigasus-iam --lib && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test authz_bootstrap_admin --test authz_bootstrap && cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/application
git commit -F - <<'EOF'
feat(rs): make GrantRole idempotent

SMA-676 D9. After the authorization check, an existing grant for the
same principal, role and scope is returned with OK and writes nothing.
A lost insert race drops the transaction and returns the winner's
grant. A concurrent bootstrap seed that lost the same race is a success.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
```

---

## Task 6: `ListMemberships` kind filter (D8)

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/persistence/pg_memberships.rs:10-18`, `:94-138` (four list SQLs), `:367-409`, new impl
- Modify: `rs/crates/services/paigasus-iam/src/application/fakes.rs:38-48` (`TenancyStore`), after `:610` (`InMemoryMemberships`)
- Modify: `rs/crates/services/paigasus-iam/src/application/memberships.rs:23-29`, `:57-100`, `:239-253`, tests `:267-296`, `:459-480`, `:546-553`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/grpc/tenancy.rs:680`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/http/memberships.rs:120-132`, `http/dto.rs:192-201`, `http/mod.rs:437-444`
- Modify: `rs/crates/services/paigasus-iam/tests/tenancy_events_pg.rs:306-313`, `tests/tenancy_memberships.rs`

**Interfaces:**
- Consumes: `MembershipAxis`, `MembershipKindQuery`, `PrincipalKindFilter`, `convert::principal_kind_filter`.
- Produces: `impl MembershipKindQuery for PgMembershipRepository`, `impl MembershipKindQuery for InMemoryMemberships`, `TenancyStore.principal_kinds: Arc<Mutex<HashMap<Uuid, PrincipalKind>>>`, `MembershipServiceDeps.kinds: Arc<dyn MembershipKindQuery>`, `MembershipService::list(&self, filter: MembershipFilter, kind: PrincipalKindFilter, page: Page)`, `MembershipQuery.principal_kind: Option<String>`.

- [ ] **Step 1: Write the failing tests**

In `memberships.rs` tests:
- add `use crate::application::principal_kind::PrincipalKindFilter;` and `PrincipalKind` to the core test import;
- in `new_service` and `service_with_fakes`, add `kinds: Arc::new(InMemoryMemberships(store.clone())),` after `repo:` (change `repo: InMemoryMemberships(store)` to `repo: InMemoryMemberships(store.clone())`); in the `StoredPrnDiffersRepo` test (line 546) add `kinds: Arc::new(InMemoryMemberships::default()),`;
- change the three `svc.list(MembershipFilter::Principal(principal.canonical()), page)` calls to `svc.list(MembershipFilter::Principal(principal.canonical()), PrincipalKindFilter::Any, page)`;
- append:

```rust
    /// SMA-676 D8: the kind filter AND-s with the node filter, and with the principal filter.
    #[tokio::test]
    async fn list_filters_by_principal_kind_on_both_axes() {
        let store = TenancyStore::default();
        let now = Utc.timestamp_opt(0, 0).unwrap();
        let person = seed_principal(&store, 40);
        let bot = seed_principal(&store, 41);
        store.principal_kinds.lock().unwrap().insert(person.uuid(), PrincipalKind::User);
        store.principal_kinds.lock().unwrap().insert(bot.uuid(), PrincipalKind::ServiceAccount);
        let (org, _team) = seed_org_and_team(&store, 400, 401, now);
        let org_prn = OrganizationId::from_uuid(org).canonical();
        let svc = new_service(store);
        svc.attach(&person.canonical(), &org_prn, &actor(999)).await.unwrap();
        svc.attach(&bot.canonical(), &org_prn, &actor(999)).await.unwrap();
        let page = Page::new(None, None).unwrap();

        let users = svc.list(MembershipFilter::Node(org_prn.clone()), PrincipalKindFilter::Only(PrincipalKind::User), page).await.unwrap();
        assert_eq!(users.iter().map(|r| r.principal_prn.clone()).collect::<Vec<_>>(), vec![person.canonical()]);
        let bots = svc.list(MembershipFilter::Node(org_prn.clone()), PrincipalKindFilter::Only(PrincipalKind::ServiceAccount), page).await.unwrap();
        assert_eq!(bots.iter().map(|r| r.principal_prn.clone()).collect::<Vec<_>>(), vec![bot.canonical()]);
        assert_eq!(svc.list(MembershipFilter::Node(org_prn), PrincipalKindFilter::Any, page).await.unwrap().len(), 2);
        assert!(svc.list(MembershipFilter::Principal(bot.canonical()), PrincipalKindFilter::Only(PrincipalKind::User), page).await.unwrap().is_empty());
    }

    /// D7 for memberships: an unknown kind is refused before the repository is read.
    #[tokio::test]
    async fn list_refuses_an_unknown_kind_before_the_repository() {
        let svc = new_service(TenancyStore::default());
        let page = Page::new(None, None).unwrap();
        let err = svc.list(MembershipFilter::Node(OrganizationId::from_uuid(Uuid::from_u128(1)).canonical()), PrincipalKindFilter::Unknown, page).await.unwrap_err();
        assert_eq!(err, TenancyError::InvalidPrincipalKind("principal_kind"), "not NotFound: the node does not exist, and the kind is checked first");
    }
```

In `rs/crates/services/paigasus-iam/tests/tenancy_memberships.rs`, add `MembershipAxis, MembershipKindQuery` to the core import and `ConnectionTrait, DbBackend, Statement` to the `sea_orm` import, then append:

```rust
/// Seeds a bare service-account `principal` row (raw SQL: this test needs only the kind).
async fn seed_service_account_principal(db: &DatabaseConnection, uuid: Uuid) -> PrincipalId {
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!(
            r#"INSERT INTO "principal" (id, prn, kind, status, created_at, updated_at)
               VALUES ('{uuid}', 'prn:pgs:iam:::principal/{uuid}', 'service_account', 'active', now(), now())"#
        ),
        [],
    ))
    .await
    .unwrap();
    PrincipalId::from_prn(Prn::build("iam", "", None, "principal", uuid).unwrap())
}

/// SMA-676 D8 against Postgres: `list_of_kind` keeps only members of that kind, on the node
/// axis and on the principal axis, and still applies the node guard.
#[tokio::test]
async fn list_of_kind_keeps_only_members_of_that_kind_on_both_axes() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let ids = KernelIdGenerator;
    let clock = SystemClock;
    let (org, _team, _project) = seed_chain(&db).await;
    let person = seed_user(&db, 71).await;
    let bot = seed_service_account_principal(&db, mint_uuid7(1_700_000_000_500, [72u8; 10])).await;
    let repo = PgMembershipRepository::new(db.clone());
    for p in [&person, &bot] {
        let m = membership_at(&ids, p, TenancyNodeRef::Organization(org.id.clone()), clock.now());
        repo.attach(&m, &stamp_of(&m)).await.unwrap();
    }
    let at_org = MembershipAxis::Node(TenancyNodeRef::Organization(org.id.clone()));

    let users = repo.list_of_kind(&at_org, PrincipalKind::User, 200, 0).await.unwrap();
    assert_eq!(users.iter().map(|r| r.principal_prn.clone()).collect::<Vec<_>>(), vec![person.canonical()]);
    let bots = repo.list_of_kind(&at_org, PrincipalKind::ServiceAccount, 200, 0).await.unwrap();
    assert_eq!(bots.iter().map(|r| r.principal_prn.clone()).collect::<Vec<_>>(), vec![bot.canonical()]);
    assert!(repo.list_of_kind(&MembershipAxis::Principal(bot.uuid()), PrincipalKind::User, 200, 0).await.unwrap().is_empty());
    assert_eq!(repo.list_by_node(&TenancyNodeRef::Organization(org.id.clone()), 200, 0).await.unwrap().len(), 2, "the unfiltered path is unchanged");
}
```

- [ ] **Step 2: Run the tests and see them fail**

Run: `cd rs && cargo nextest run -p paigasus-iam --lib memberships::tests`
Expected: FAIL — `no field principal_kinds on TenancyStore`, `struct MembershipServiceDeps has no field named kinds`, `this method takes 2 arguments but 3 arguments were supplied`.

- [ ] **Step 3: Implement the Postgres side**

In `pg_memberships.rs`, change the core import to add `MembershipAxis, MembershipKindQuery, PrincipalKind`. In each of the four list SQL constants, add the kind predicate to EVERY `WHERE` clause. `LIST_BY_PRINCIPAL_SQL` becomes:

```rust
const LIST_BY_PRINCIPAL_SQL: &str = r#"
SELECT m.id, pr.prn AS principal_prn, o.prn AS node_prn, m.created_at, m.created_by
  FROM "membership" m JOIN "principal" pr ON pr.id = m.principal_id
  JOIN "organization" o ON o.id = m.org_id
 WHERE m.principal_id = $1 AND ($4::text IS NULL OR pr.kind = $4)
UNION ALL
SELECT m.id, pr.prn, t.prn, m.created_at, m.created_by FROM "membership" m
  JOIN "principal" pr ON pr.id = m.principal_id JOIN "team" t ON t.id = m.team_id
 WHERE m.principal_id = $1 AND ($4::text IS NULL OR pr.kind = $4)
UNION ALL
SELECT m.id, pr.prn, pj.prn, m.created_at, m.created_by FROM "membership" m
  JOIN "principal" pr ON pr.id = m.principal_id JOIN "project" pj ON pj.id = m.project_id
 WHERE m.principal_id = $1 AND ($4::text IS NULL OR pr.kind = $4)
ORDER BY created_at, id LIMIT $2 OFFSET $3"#;
```

and in `LIST_BY_ORG_SQL`, `LIST_BY_TEAM_SQL`, `LIST_BY_PROJECT_SQL` change ` WHERE m.org_id = $1` (and the team/project twins) to ` WHERE m.org_id = $1 AND ($4::text IS NULL OR pr.kind = $4)`. Add to the doc of `LIST_BY_PRINCIPAL_SQL`: `$4 is the SMA-676 kind filter; NULL = any kind.`

Replace `list_by_principal` and `list_by_node` in the `MembershipRepository` impl with:

```rust
    async fn list_by_principal(&self, principal: Uuid, limit: u64, offset: u64) -> Result<Vec<MembershipRecord>, RepositoryError> {
        self.list_rows(LIST_BY_PRINCIPAL_SQL, principal, None, limit, offset).await
    }

    async fn list_by_node(&self, node: &TenancyNodeRef, limit: u64, offset: u64) -> Result<Vec<MembershipRecord>, RepositoryError> {
        let (sql, node_uuid) = self.node_list_sql(node).await?;
        self.list_rows(sql, node_uuid, None, limit, offset).await
    }
```

and add to `impl PgMembershipRepository`:

```rust
    /// Resolves `node` by uuid with no lock (a plain listing, not a guarded mutation) and
    /// applies the NotFound/PrnMismatch guard; returns the list SQL for that node's kind.
    async fn node_list_sql(&self, node: &TenancyNodeRef) -> Result<(&'static str, Uuid), RepositoryError> {
        let (stored, sql, uuid) = match node {
            TenancyNodeRef::Organization(id) => (organization::Entity::find_by_id(id.uuid()).one(&self.db).await.map_err(map_err)?.map(|m| m.prn), LIST_BY_ORG_SQL, id.uuid()),
            TenancyNodeRef::Team(id) => (team::Entity::find_by_id(id.uuid()).one(&self.db).await.map_err(map_err)?.map(|m| m.prn), LIST_BY_TEAM_SQL, id.uuid()),
            TenancyNodeRef::Project(id) => (project::Entity::find_by_id(id.uuid()).one(&self.db).await.map_err(map_err)?.map(|m| m.prn), LIST_BY_PROJECT_SQL, id.uuid()),
        };
        let stored = stored.ok_or(RepositoryError::NotFound)?;
        if stored != node.canonical() {
            return Err(RepositoryError::PrnMismatch);
        }
        Ok((sql, uuid))
    }

    /// Runs one of the four list SQLs. `kind` binds `$4`; `None` = any kind.
    async fn list_rows(&self, sql: &str, id: Uuid, kind: Option<PrincipalKind>, limit: u64, offset: u64) -> Result<Vec<MembershipRecord>, RepositoryError> {
        let kind: Option<String> = kind.map(|k| k.as_str().to_owned());
        let stmt = Statement::from_sql_and_values(DbBackend::Postgres, sql, [id.into(), limit.into(), offset.into(), kind.into()]);
        let rows = MembershipRow::find_by_statement(stmt).all(&self.db).await.map_err(map_err)?;
        Ok(rows.into_iter().map(MembershipRecord::from).collect())
    }
```

After the `MembershipRepository` impl add:

```rust
/// SMA-676 D8: the same four statements, with `$4` bound to the kind.
#[async_trait]
impl MembershipKindQuery for PgMembershipRepository {
    async fn list_of_kind(&self, axis: &MembershipAxis, kind: PrincipalKind, limit: u64, offset: u64) -> Result<Vec<MembershipRecord>, RepositoryError> {
        match axis {
            MembershipAxis::Principal(principal) => self.list_rows(LIST_BY_PRINCIPAL_SQL, *principal, Some(kind), limit, offset).await,
            MembershipAxis::Node(node) => {
                let (sql, node_uuid) = self.node_list_sql(node).await?;
                self.list_rows(sql, node_uuid, Some(kind), limit, offset).await
            }
        }
    }
}
```

- [ ] **Step 4: Implement the fake, the service and the adapters**

`fakes.rs`: add `MembershipAxis, MembershipKindQuery` to the core import. In `TenancyStore`, after `principals` add:

```rust
    /// SMA-676: the `principal.kind` column for `MembershipKindQuery`. A principal with no
    /// entry has no known kind and drops out of every kind-filtered listing.
    pub principal_kinds: Arc<Mutex<HashMap<Uuid, PrincipalKind>>>,
```

After `impl MembershipRepository for InMemoryMemberships { … }` add:

```rust
#[async_trait]
impl MembershipKindQuery for InMemoryMemberships {
    async fn list_of_kind(&self, axis: &MembershipAxis, kind: PrincipalKind, limit: u64, offset: u64) -> Result<Vec<MembershipRecord>, RepositoryError> {
        let all = match axis {
            MembershipAxis::Principal(principal) => self.list_by_principal(*principal, u64::MAX, 0).await?,
            MembershipAxis::Node(node) => self.list_by_node(node, u64::MAX, 0).await?,
        };
        let kinds = self.0.principal_kinds.lock().unwrap().clone();
        let of_kind = |r: &MembershipRecord| Prn::parse(&r.principal_prn).ok().map(|p| PrincipalId::from_prn(p).uuid()).and_then(|u| kinds.get(&u).copied()) == Some(kind);
        Ok(all.into_iter().filter(of_kind).skip(offset as usize).take(limit as usize).collect())
    }
}
```

`memberships.rs`: change the core import to add `MembershipAxis, MembershipKindQuery`, add `use crate::application::principal_kind::PrincipalKindFilter;`. Add `pub kinds: Arc<dyn MembershipKindQuery>,` after `pub repo: M,` in `MembershipServiceDeps` (doc: `SMA-676 D8: the read port for a kind-filtered listing.`), `kinds: Arc<dyn MembershipKindQuery>,` after `repo: M,` in `MembershipService`, `kinds: deps.kinds,` in `new`. Replace `list` with:

```rust
    /// Lists memberships by principal or node, `ORDER BY created_at, id` (design doc §5.1
    /// rule 9). SMA-676 D8: `kind` AND-s with the filter; an unknown kind is refused first
    /// (D7). `Any` keeps the pre-SMA-676 repository path.
    pub async fn list(&self, filter: MembershipFilter, kind: PrincipalKindFilter, page: Page) -> Result<Vec<MembershipRecord>, TenancyError> {
        let kind = kind.resolve()?;
        let axis = match filter {
            MembershipFilter::Principal(raw) => MembershipAxis::Principal(parse_principal_prn(&raw)?.uuid()),
            MembershipFilter::Node(raw) => MembershipAxis::Node(parse_node_prn(&raw)?),
        };
        Ok(match (axis, kind) {
            (MembershipAxis::Principal(principal), None) => self.repo.list_by_principal(principal, page.limit, page.offset).await?,
            (MembershipAxis::Node(node), None) => self.repo.list_by_node(&node, page.limit, page.offset).await?,
            (axis, Some(kind)) => self.kinds.list_of_kind(&axis, kind, page.limit, page.offset).await?,
        })
    }
```

`grpc/tenancy.rs:680`: change to

```rust
            let records = self.state.memberships.list(filter, convert::principal_kind_filter(req.principal_kind), page).await.map_err(convert::status_to_grpc)?;
```

`http/dto.rs` `MembershipQuery`: add `pub principal_kind: Option<String>,` after `node`, and extend its doc: `principal_kind (SMA-676 D8) is user or service_account; any other value is refused.`

`http/memberships.rs`: add `use crate::application::principal_kind::PrincipalKindFilter;` and change line 130 to:

```rust
    let records = s.memberships.list(filter, PrincipalKindFilter::from_query(q.principal_kind.as_deref()), page).await?;
```

`http/mod.rs` (line 437): add `kinds: Arc::new(PgMembershipRepository::new(db.clone())),` after `repo: PgMembershipRepository::new(db.clone()),`.
`tests/tenancy_events_pg.rs` (line 306): add `kinds: Arc::new(PgMembershipRepository::new(db.clone())),` after `repo: membership_repo,`.

- [ ] **Step 5: Run the tests and see them pass**

Run:

```bash
cd rs
cargo nextest run -p paigasus-iam --lib
PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test tenancy_memberships --test http_memberships --test grpc_tenancy --test tenancy_events_pg
cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add rs/crates/services/paigasus-iam
git commit -F - <<'EOF'
feat(rs): filter ListMemberships by principal kind

SMA-676 D8. The four membership list statements take an optional kind
predicate on the principal join. MembershipKindQuery serves a filtered
listing; the unfiltered path is unchanged. An unknown kind is refused
before any read, on both transports.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
```

---

## Task 7: Real Cedar rows and the Docker end-to-end tests

**Files:**
- Modify: `rs/crates/libs/paigasus-iam-core/src/authz/roles.rs:692` (append cases before `];`)
- Create: `rs/crates/services/paigasus-iam/tests/authz_people_model_access.rs`
- Modify: `rs/crates/services/paigasus-iam/tests/authz_forged_org_slot_escalation.rs` (append a test)

**Interfaces:**
- Consumes: Tasks 1-6 over HTTP (`GET/POST /v1/authz/role-grants`, `DELETE /v1/authz/role-grants/{id}`, `POST /v1/authz/is-authorized`).
- Produces: tests only.

- [ ] **Step 1: Write the real-Cedar rows**

In `starter_policy_table` (`rs/crates/libs/paigasus-iam-core/src/authz/roles.rs`), before the closing `];` of `cases` (line 693) add:

```rust
            // -- SMA-676 D4 (b): ListRoleGrants at a SCOPE node, decided by the real starter
            // policy set. An org_admin passes at its own org only; a team_admin does not pass
            // at its parent org; an org_member holds no ListRoleGrants.
            Case {
                name: "org_admin allows ListRoleGrants at its own org (SMA-676 D4)",
                grants: vec![grant(94, &uni.principal, "org_admin", GrantScope::Node(TenancyNodeRef::Organization(uni.org_o.clone())))],
                action: Action::ListRoleGrants,
                resource: uni.org_o.prn().clone(),
                expect: Effect::Allow,
            },
            Case {
                name: "org_admin denies ListRoleGrants at another org (SMA-676 D4)",
                grants: vec![grant(95, &uni.principal, "org_admin", GrantScope::Node(TenancyNodeRef::Organization(uni.org_o.clone())))],
                action: Action::ListRoleGrants,
                resource: OrganizationId::from_uuid(uni.team_other.org_uuid()).prn().clone(),
                expect: Effect::Deny,
            },
            Case {
                name: "team_admin denies ListRoleGrants at its parent org (SMA-676 D4)",
                grants: vec![grant(96, &uni.principal, "team_admin", GrantScope::Node(TenancyNodeRef::Team(uni.team_o.clone())))],
                action: Action::ListRoleGrants,
                resource: uni.org_o.prn().clone(),
                expect: Effect::Deny,
            },
            Case {
                name: "org_member denies ListRoleGrants at its own org (SMA-676 D4)",
                grants: vec![grant(97, &uni.principal, "org_member", GrantScope::Node(TenancyNodeRef::Organization(uni.org_o.clone())))],
                action: Action::ListRoleGrants,
                resource: uni.org_o.prn().clone(),
                expect: Effect::Deny,
            },
```

If `OrganizationId` is not yet imported in that test module, add it to the module's `use` list.

- [ ] **Step 2: Write the HTTP end-to-end file**

Create `rs/crates/services/paigasus-iam/tests/authz_people_model_access.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0

//! SMA-676 end to end over HTTP, against the real Cedar policy set and Postgres (Docker):
//! an `org_admin` lists the grants at its own org (D4 b) and at no other org; grants
//! `gateway_user` to a person, twice, and gets one grant (D9); the person may then
//! `InvokeModel` on the org; the admin revokes, and the person may not. Plus the D3, D6 and D7
//! refusals. The fake IAM of the console is not Cedar, so this file is where the decision is
//! proved; the console e2e row R30 proves only the wiring.
//!
//! Runs against an ephemeral Postgres in Docker. In CI a missing daemon is a hard failure;
//! run a filtered local invocation with `PAIGASUS_REQUIRE_DOCKER=1`.

mod support;

use axum::http::StatusCode;
use paigasus_iam::adapters::authz::Generations;
use paigasus_iam::adapters::clock::SystemClock;
use paigasus_iam::adapters::id::KernelIdGenerator;
use paigasus_iam::adapters::persistence::PgOrganizationRepository;
use paigasus_iam_core::{Clock, IdGenerator, Organization, OrganizationRepository, Slug, Stamp, Team};
use sea_orm::DatabaseConnection;
use serde_json::json;
use support::{app_with_state, provision, seed_org_admin, send};

/// A fresh org with its default team and an owner `org_admin` grant, through the real repo.
async fn seed_org(db: &DatabaseConnection, slug: &str) -> Organization {
    let ids = KernelIdGenerator;
    let clock = SystemClock;
    let repo = PgOrganizationRepository::new(db.clone(), Generations::memory());
    let owner = ids.new_principal_id();
    let stamp = Stamp::new(clock.now(), owner.clone());
    let org = Organization::new(ids.new_organization_id(), Slug::parse(slug).unwrap(), "Org", &stamp).unwrap();
    let default_team = Team::new(ids.new_team_id(org.id.uuid()), Slug::parse("default").unwrap(), "Default", &stamp).unwrap();
    let owner_grant = support::pg_owner_grant(db, &owner, ids.new_membership_id(), &org.id).await;
    repo.create(&org, &default_team, &owner_grant, &stamp).await.unwrap();
    org
}

#[tokio::test]
async fn an_org_admin_grants_and_revokes_gateway_user_and_invoke_model_follows() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, state, idp) = app_with_state(db.clone()).await;
    let org = seed_org(&db, "pma-acme").await;
    let org_prn = org.id.canonical();

    let admin_token = idp.bearer("pma-admin", Some("pma-admin@example.com"), "paigasus", 3600);
    let admin_prn = provision(&state, &admin_token).await;
    seed_org_admin(&state, &admin_prn, &org_prn).await;
    let person_token = idp.bearer("pma-person", Some("pma-person@example.com"), "paigasus", 3600);
    let person_prn = provision(&state, &person_token).await;

    let holders = format!("/v1/authz/role-grants?scope_prn={org_prn}&role_key=gateway_user&principal_kind=user&limit=200");
    let (status, listed) = send(&app, "GET", &holders, None, Some(admin_token.as_str())).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert_eq!(listed, json!([]));

    let body = json!({ "principal_prn": person_prn, "role_key": "gateway_user", "scope_prn": org_prn });
    let (status, first) = send(&app, "POST", "/v1/authz/role-grants", Some(body.clone()), Some(admin_token.as_str())).await;
    assert_eq!(status, StatusCode::CREATED, "{first}");
    let (status, second) = send(&app, "POST", "/v1/authz/role-grants", Some(body), Some(admin_token.as_str())).await;
    assert_eq!(status, StatusCode::CREATED, "D9: a second grant is a success: {second}");
    assert_eq!(second["id"], first["id"], "D9: the second grant returns the existing grant");

    let (status, listed) = send(&app, "GET", &holders, None, Some(admin_token.as_str())).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert_eq!(listed.as_array().map(Vec::len), Some(1), "{listed}");
    assert_eq!(listed[0]["principal_prn"], json!(person_prn));

    let invoke = json!({ "principal_prn": person_prn, "action": "InvokeModel", "resource_prn": org_prn });
    let (status, decision) = send(&app, "POST", "/v1/authz/is-authorized", Some(invoke.clone()), Some(person_token.as_str())).await;
    assert_eq!(status, StatusCode::OK, "{decision}");
    assert_eq!(decision["allowed"], true, "gateway_user at the org allows InvokeModel on the org: {decision}");

    let grant_id = first["id"].as_str().expect("a grant id");
    let (status, body) = send(&app, "DELETE", &format!("/v1/authz/role-grants/{grant_id}"), None, Some(admin_token.as_str())).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    let (status, decision) = send(&app, "POST", "/v1/authz/is-authorized", Some(invoke), Some(person_token.as_str())).await;
    assert_eq!(status, StatusCode::OK, "{decision}");
    assert_eq!(decision["allowed"], false, "after the revoke the person may not InvokeModel: {decision}");
}

/// D4 (b) and Review Focus 5: the same `org_admin` lists at its own org — also when the PRN
/// carries an UPPER-case uuid, which must match the stored lower-case canonical PRN — and is
/// refused at another org.
#[tokio::test]
async fn an_org_admin_may_list_at_its_own_org_with_an_upper_case_uuid_and_not_at_another_org() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, state, idp) = app_with_state(db.clone()).await;
    let own = seed_org(&db, "pma-own").await;
    let other = seed_org(&db, "pma-other").await;
    let admin_token = idp.bearer("pma-lister", Some("pma-lister@example.com"), "paigasus", 3600);
    let admin_prn = provision(&state, &admin_token).await;
    seed_org_admin(&state, &admin_prn, &own.id.canonical()).await;

    let upper = format!("prn:pgs:iam:::organization/{}", own.id.uuid().to_string().to_uppercase());
    let (status, listed) = send(&app, "GET", &format!("/v1/authz/role-grants?scope_prn={upper}&role_key=org_admin"), None, Some(admin_token.as_str())).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert!(listed.as_array().unwrap().iter().any(|g| g["principal_prn"] == json!(admin_prn)), "the admin's own org_admin grant is at this org: {listed}");

    let (status, body) = send(&app, "GET", &format!("/v1/authz/role-grants?scope_prn={}", other.id.canonical()), None, Some(admin_token.as_str())).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"]["code"], "forbidden");
}

/// D3, D6, D7 and Review Focus 2 over HTTP.
#[tokio::test]
async fn the_http_list_refuses_no_filter_an_unknown_kind_and_a_large_scope_page_but_not_a_large_principal_page() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, state, idp) = app_with_state(db.clone()).await;
    let org = seed_org(&db, "pma-rules").await;
    let token = idp.bearer("pma-rules", Some("pma-rules@example.com"), "paigasus", 3600);
    let me = provision(&state, &token).await;
    seed_org_admin(&state, &me, &org.id.canonical()).await;
    let scope = org.id.canonical();

    let (status, body) = send(&app, "GET", "/v1/authz/role-grants", None, Some(token.as_str())).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["code"], "missing-required-field");

    for kind in ["robot", "", "USER"] {
        let (status, body) = send(&app, "GET", &format!("/v1/authz/role-grants?scope_prn={scope}&principal_kind={kind}"), None, Some(token.as_str())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{kind:?}: {body}");
        assert_eq!(body["error"]["code"], "invalid-principal-kind", "{kind:?}");
    }

    let (status, body) = send(&app, "GET", &format!("/v1/authz/role-grants?scope_prn={scope}&limit=201"), None, Some(token.as_str())).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["code"], "invalid-pagination");

    let (status, body) = send(&app, "GET", &format!("/v1/authz/role-grants?principal_prn={me}&limit=500"), None, Some(token.as_str())).await;
    assert_eq!(status, StatusCode::OK, "D6: the bare principal request ignores limit: {body}");
    assert!(!body.as_array().unwrap().is_empty());
}
```

Append to `tests/authz_forged_org_slot_escalation.rs`:

```rust
/// SMA-676 D5 and §6: a team `scope_prn` with a forged org slot matches no stored row, and
/// Cedar decides the `ListRoleGrants` check against the team's STORED ancestry. An org_admin
/// of ORG_A is refused for team_b (really under ORG_B), and is allowed at ORG_A itself — so
/// the 403 is the forged slot, not a missing grant.
#[tokio::test]
async fn forged_org_slot_in_a_team_scope_list_is_denied() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, state, idp) = app_with_state(db.clone()).await;
    let (org_a, _team_a) = seed_org_with_team(&db, "list-org-a", "list-team-a").await;
    let (_org_b, team_b) = seed_org_with_team(&db, "list-org-b", "list-team-b").await;
    let actor_token = idp.bearer("list-mallory", Some("list-mallory@example.com"), "paigasus", 3600);
    let actor_prn = support::provision(&state, &actor_token).await;
    seed_org_admin(&state, &actor_prn, &org_a.id.canonical()).await;

    let forged_team_prn = TeamId::from_parts(org_a.id.uuid(), team_b.id.uuid()).canonical();
    let (status, body) = send(&app, "GET", &format!("/v1/authz/role-grants?scope_prn={forged_team_prn}"), None, Some(actor_token.as_str())).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "a forged org slot must not list team_b's grants: {body}");
    assert_eq!(body["error"]["code"], "forbidden");

    let (status, body) = send(&app, "GET", &format!("/v1/authz/role-grants?scope_prn={}", org_a.id.canonical()), None, Some(actor_token.as_str())).await;
    assert_eq!(status, StatusCode::OK, "control: the same actor lists at its own org: {body}");
}
```

- [ ] **Step 3: Run the tests**

Run:

```bash
cd rs
cargo nextest run -p paigasus-iam-core --lib starter_policy_table
PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test authz_people_model_access --test authz_forged_org_slot_escalation
cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings
```

Expected: PASS. These tests pin behaviour that Tasks 2-6 built. To prove they bite, run this mutation once and restore it: in `roles.rs` `list`, change `filter.scope().map_or_else(root_prn, scope_resource_prn)` to `root_prn()`; run `PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test authz_people_model_access --no-fail-fast`; expected FAIL in `an_org_admin_grants_and_revokes…` and `…upper_case_uuid…` with status 403. Restore the line with the Edit tool (not `git checkout`), then re-run and see PASS.

- [ ] **Step 4: Commit**

```bash
git add rs/crates/libs/paigasus-iam-core/src/authz/roles.rs rs/crates/services/paigasus-iam/tests
git commit -F - <<'EOF'
test(rs): prove the scope list, the idempotent grant and InvokeModel end to end

SMA-676 section 7.1. Real Cedar rows for ListRoleGrants at a scope, a
Docker HTTP test from grant to InvokeModel to revoke, the D3, D6 and D7
refusals, and a forged team scope that is denied.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
```

---

## Task 8: console-core — `RevokeRole`, the fake IAM page limit, the dev world

**Files:**
- Modify: `ts/packages/paigasus-console-core/src/authorize.ts:25-72`
- Modify: `ts/packages/paigasus-console-core/tests/unit/action-names.test.ts:27-29`, `:53-57`
- Modify: `ts/apps/iam-console/tests/e2e/support/world.ts:76-81`
- Modify: `ts/packages/paigasus-console-core/testing/fake-iam.ts:7-49` (header), `:67` area (constants), `:280-294` (`dispatch`)
- Create: `ts/packages/paigasus-console-core/tests/integration/fake-iam-page-limit.test.ts`
- Modify: `ts/packages/paigasus-console-core/testing/dev-world.ts:19-22`, `:175-205`
- Modify: `ts/packages/paigasus-console-core/tests/unit/dev-world.test.ts:11-39`, new cases

**Interfaces:**
- Consumes: `PrincipalKind` from `@paigasus/sdk/iam/types` (Task 1).
- Produces: `IamAction` includes `'RevokeRole'`; the fake refuses `limit > 200` on `tenancy.listMemberships` and on a `authz.listRoleGrants` request that sets `scopePrn`, `roleKey` or `principalKind`; `devWorld()` scripts `authz.grantRole` and `authz.revokeRole`.

- [ ] **Step 1: Write the failing tests**

In `action-names.test.ts`, replace the `SERVICE_ACCOUNT` comment and constant (lines 27-29) with:

```ts
// SMA-636 spec § 4.5, SMA-676. mayI() asks about the CURRENT user only. InvokeModel is asked about a
// service account through modelCallState, so it stays out. ListRoleGrants stays out: no page shows
// a control for it. RevokeRole joins for the "Model access for people" section (SMA-676 D14).
const SERVICE_ACCOUNT = ['CreateServiceAccount', 'ArchiveServiceAccount', 'IssueApiKey', 'RevokeApiKey', 'GrantRole', 'RevokeRole'];
```

and change the test title on line 53 to `'holds the gateway-settings names of SMA-636 and SMA-676, and neither InvokeModel nor ListRoleGrants'`.

Create `ts/packages/paigasus-console-core/tests/integration/fake-iam-page-limit.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// SMA-676 spec § 7.2. IAM's Page::new refuses a limit over 200 (application/pagination.rs:11) with
// InvalidArgument + invalid-pagination. ListMemberships always pages; ListRoleGrants pages only when
// a filter beyond the bare principal is set (D6). The fake must refuse the same requests, or a
// console test that asks for 500 rows passes against the fake and fails against IAM.
import { afterAll, afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { disposeTransports } from '@paigasus/sdk/iam';
import { PrincipalKind } from '@paigasus/sdk/iam/types';
import { callIam } from '../../src/errors';
import { createIamClients } from '../../src/iam-clients';
import { logger } from '../../src/logger';
import { startFakeIam, type FakeIam } from '@paigasus/console-core/testing';

const ORG = 'prn:pgs:iam:::organization/0190a100-0000-7000-8000-00000000000a';
const ME = 'prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-0000000000aa';

describe('the fake IAM page limit', () => {
  let fake: FakeIam;

  beforeEach(async () => {
    vi.spyOn(logger, 'appEvent').mockImplementation(() => undefined);
    fake = await startFakeIam({
      handlers: { 'tenancy.listMemberships': () => ({ memberships: [] }), 'authz.listRoleGrants': () => ({ grants: [] }) },
    });
  });
  afterEach(async () => {
    vi.restoreAllMocks();
    await fake.close();
  });
  afterAll(() => disposeTransports());

  const clients = () => createIamClients({ baseUrl: fake.grpcUrl, token: 'tok-page-limit' });

  it('refuses ListMemberships with limit 201 as invalid-pagination, and accepts 200', async () => {
    const refused = await callIam(() => clients().tenancy.listMemberships({ filter: { case: 'nodePrn', value: ORG }, limit: 201, offset: 0n }));
    expect(refused.ok).toBe(false);
    if (refused.ok) return;
    expect(refused.error.rawReason).toBe('invalid-pagination');
    expect(refused.error.transport).toMatchObject({ kind: 'grpc', code: 3 });
    expect((await callIam(() => clients().tenancy.listMemberships({ filter: { case: 'nodePrn', value: ORG }, limit: 200, offset: 0n }))).ok).toBe(true);
  });

  it.each([
    ['a scope', { scopePrn: ORG }],
    ['a role key', { principalPrn: ME, roleKey: 'gateway_user' }],
    ['a kind', { principalPrn: ME, principalKind: PrincipalKind.USER }],
  ] as const)('refuses ListRoleGrants with limit 201 when the request has %s', async (_label, filter) => {
    const refused = await callIam(() => clients().authz.listRoleGrants({ ...filter, limit: 201, offset: 0n }));
    expect(refused.ok).toBe(false);
    if (refused.ok) return;
    expect(refused.error.rawReason).toBe('invalid-pagination');
  });

  it('ignores the limit on the bare principal request, as IAM does (D6)', async () => {
    expect((await callIam(() => clients().authz.listRoleGrants({ principalPrn: ME, limit: 500, offset: 0n }))).ok).toBe(true);
  });
});
```

In `dev-world.test.ts`, add `'authz.grantRole', 'authz.revokeRole',` to `REQUIRED` after `'authz.listRoleGrants',` and append inside the `describe`:

```ts
  type Grant = { id: string; principalPrn: string; roleKey: string; scopePrn: string };
  const ctx = { token: 'dev', correlationId: 'corr-dev' };

  it('grants idempotently, lists at a scope, and revokes (SMA-676 D9)', async () => {
    const handlers = devWorld();
    const grant = handlers['authz.grantRole'] as (req: Omit<Grant, 'id'>, c: typeof ctx) => { grant: Grant };
    const list = handlers['authz.listRoleGrants'] as (req: { principalPrn: string; scopePrn: string; roleKey: string; principalKind: number }, c: typeof ctx) => { grants: Grant[] };
    const revoke = handlers['authz.revokeRole'] as (req: { id: string }, c: typeof ctx) => object;
    const org = 'prn:pgs:iam:::organization/0190a100-0000-7000-8000-00000000d002';
    const person = 'prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-00000000d0aa';

    const first = grant({ principalPrn: person, roleKey: 'gateway_user', scopePrn: org }, ctx).grant;
    const second = grant({ principalPrn: person, roleKey: 'gateway_user', scopePrn: org }, ctx).grant;
    expect(second.id).toBe(first.id);
    expect(list({ principalPrn: '', scopePrn: org, roleKey: 'gateway_user', principalKind: 1 }, ctx).grants).toEqual([first]);
    expect(list({ principalPrn: '', scopePrn: org, roleKey: 'gateway_user', principalKind: 2 }, ctx).grants).toEqual([]);

    revoke({ id: first.id }, ctx);
    expect(list({ principalPrn: '', scopePrn: org, roleKey: 'gateway_user', principalKind: 0 }, ctx).grants).toEqual([]);
    expect(() => revoke({ id: first.id }, ctx)).toThrow(ConnectError);
  });

  it('answers no member for a service-account kind filter (SMA-676 D8)', async () => {
    const handlers = devWorld();
    const members = handlers['tenancy.listMemberships'] as (req: { filter: { case: 'nodePrn'; value: string }; principalKind: number }, c: typeof ctx) => { memberships: unknown[] };
    const org = 'prn:pgs:iam:::organization/0190a100-0000-7000-8000-00000000d002';
    expect(members({ filter: { case: 'nodePrn', value: org }, principalKind: 1 }, ctx).memberships).toHaveLength(1);
    expect(members({ filter: { case: 'nodePrn', value: org }, principalKind: 2 }, ctx).memberships).toHaveLength(0);
  });
```

- [ ] **Step 2: Run the tests and see them fail**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; pnpm -C ts/packages/paigasus-console-core exec vitest run tests/unit/action-names.test.ts tests/unit/dev-world.test.ts tests/integration/fake-iam-page-limit.test.ts`
Expected: FAIL — `RevokeRole` not in `IAM_ACTIONS`; `authz.grantRole` missing from `devWorld()`; the page-limit cases answer OK.

- [ ] **Step 3: Implement**

`authorize.ts`: replace the comment paragraph at lines 31-35 with:

```ts
 * SMA-636 added the five names of the gateway settings. SMA-676 added RevokeRole: the gateway zone's
 * "Model access for people" section asks GrantRole and RevokeRole at the org. InvokeModel and
 * ListRoleGrants are deliberately ABSENT: mayI() asks about the current user. The gateway zone asks
 * InvokeModel about a service account through its own fail-closed modelCallState. ListRoleGrants is
 * allowed for self and, since SMA-676 D4, at a scope node where the actor administers roles; no page
 * shows a control that depends on it, so no question is asked.
```

and add `'RevokeRole',` after `'GrantRole',` in `IAM_ACTIONS`.

`ts/apps/iam-console/tests/e2e/support/world.ts`: add after `'GrantRole',` (line 81):

```ts
  // SMA-676: the gateway zone's "Model access for people". This zone asks none, but the SET must equal IAM_ACTIONS.
  'RevokeRole',
```

`fake-iam.ts`: in the header list of copied behaviours (after the "A denial is …" bullet, line 23) add:

```ts
//   - A list page over 200 rows is refused with `InvalidArgument` + reason `invalid-pagination`
//     (application/pagination.rs:11, 28-29): on `tenancy.listMemberships` always, and on
//     `authz.listRoleGrants` when the request sets `scopePrn`, `roleKey` or `principalKind` (SMA-676
//     D6). The bare principal request ignores its limit, as IAM does.
```

After `const UUID_RE = …` add:

```ts
/** IAM's Page maximum (application/pagination.rs:11). */
const MAX_PAGE_LIMIT = 200;

/** The refusal IAM's Page::new gives a list call, or null when the call pages within bounds (SMA-676). */
function pageRefusal(method: string, request: unknown): ConnectError | null {
  const req = request as { limit?: number; scopePrn?: string; roleKey?: string; principalKind?: number };
  const pages = method === 'tenancy.listMemberships' || (method === 'authz.listRoleGrants' && ((req.scopePrn ?? '') !== '' || (req.roleKey ?? '') !== '' || (req.principalKind ?? 0) !== 0));
  if (!pages || (req.limit ?? 0) <= MAX_PAGE_LIMIT) return null;
  return iamError(Code.InvalidArgument, 'invalid-pagination', 'invalid pagination parameters');
}
```

In `dispatch`, after `provisioned.add(token);`'s enclosing `if` block and before `if (method === 'authn.introspect')`, add:

```ts
      const refusal = pageRefusal(method, request);
      if (refusal !== null) throw refusal;
```

`dev-world.ts`: add `PrincipalKind` to the `@paigasus/sdk/iam/types` import. Inside `devWorld()`, after `const deadLetters = seededDeadLetters();` add:

```ts
  // SMA-676: role grants live in per-devWorld() state, so the "Model access for people" section can
  // grant and revoke. Every dev principal is a USER: the dev world makes no service account.
  type DevGrant = { id: string; principalPrn: string; roleKey: string; scopePrn: string };
  const grants: DevGrant[] = [{ id: '0190a1d4-0000-7000-8000-00000000d103', principalPrn: PRINCIPAL_PRN, roleKey: 'project_viewer', scopePrn: PROJECT_PRN }];
```

Replace the `'authz.listRoleGrants'` entry with:

```ts
    'authz.listRoleGrants': (req) => ({
      grants: grants.filter(
        (grant) =>
          (req.principalPrn === '' || grant.principalPrn === req.principalPrn) &&
          (req.scopePrn === '' || grant.scopePrn === req.scopePrn) &&
          (req.roleKey === '' || grant.roleKey === req.roleKey) &&
          req.principalKind !== PrincipalKind.SERVICE_ACCOUNT,
      ),
    }),
    // SMA-676 D9: a second grant of the same role at the same scope returns the existing grant.
    'authz.grantRole': (req) => {
      const existing = grants.find((grant) => grant.principalPrn === req.principalPrn && grant.roleKey === req.roleKey && grant.scopePrn === req.scopePrn);
      if (existing !== undefined) return { grant: existing };
      const grant: DevGrant = { id: randomUUID(), principalPrn: req.principalPrn, roleKey: req.roleKey, scopePrn: req.scopePrn };
      grants.push(grant);
      return { grant };
    },
    'authz.revokeRole': (req) => {
      const index = grants.findIndex((grant) => grant.id === req.id);
      if (index === -1) throw notFound();
      grants.splice(index, 1);
      return {};
    },
```

Replace the `'tenancy.listMemberships'` entry with:

```ts
    'tenancy.listMemberships': (req) => ({
      memberships:
        req.principalKind === PrincipalKind.SERVICE_ACCOUNT
          ? []
          : [{ id: '0190a1d4-0000-7000-8000-00000000d104', principalPrn: PRINCIPAL_PRN, nodePrn: req.filter.case === 'nodePrn' ? req.filter.value : '' }],
    }),
```

- [ ] **Step 4: Run the tests and see them pass**

Run: `moon run paigasus-console-core-ts:test paigasus-console-core-ts:typecheck iam-console-ts:typecheck ts:fmt ts:lint && pnpm -C ts/apps/iam-console exec vitest run tests/unit/world-actions.test.ts`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add ts/packages/paigasus-console-core ts/apps/iam-console/tests/e2e/support/world.ts
git commit -F - <<'EOF'
feat(ts): add RevokeRole to mayI, refuse large pages in the fake IAM, script grants in the dev world

SMA-676 D14 and section 7.2. The fake IAM refuses a limit over 200 where
IAM's Page::new does. The dev world keeps role grants, grants
idempotently, revokes, and filters members by kind.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
```

---

## Task 9: gateway-console loader — join and bounded paging

**Files:**
- Create: `ts/apps/gateway-console/app/(console)/gateway-role.ts`
- Modify: `ts/apps/gateway-console/app/(console)/service-accounts/commands.ts:16-17` (import instead of define)
- Modify: `ts/apps/gateway-console/tests/integration/service-account-commands.test.ts:12` (import path)
- Create: `ts/apps/gateway-console/app/(console)/people-model-access/view.ts`
- Create: `ts/apps/gateway-console/app/(console)/people-model-access/load.ts`
- Create: `ts/apps/gateway-console/tests/integration/people-model-access-load.test.ts`

**Interfaces:**
- Consumes: `listRoleGrants`, `listMemberships`, `MayI` with `'RevokeRole'` (Task 8), `PrincipalKind` (Task 1), `cedarCapabilityOf`, `callIam`.
- Produces:

```ts
export const GATEWAY_ROLE = 'gateway_user';
export type HolderRowView = { readonly grantId: string; readonly principalPrn: string; readonly member: boolean | null };
export type CandidateView = { readonly principalPrn: string };
export type PeopleFlags = { readonly canGrant: boolean; readonly canRevoke: boolean; readonly grantsTruncated: boolean; readonly membersTruncated: boolean };
export type PeopleModelAccessOk = { readonly kind: 'ok'; readonly orgPrn: string; readonly readOnly: boolean; readonly holders: readonly HolderRowView[]; readonly candidates: readonly CandidateView[]; readonly flags: PeopleFlags };
export type PeopleModelAccessView = { readonly kind: 'disabled' } | { readonly kind: 'denied' } | { readonly kind: 'error'; readonly error: PaigasusError } | PeopleModelAccessOk;
export type PeopleModelAccessActions = { readonly grant: FormAction; readonly revoke: FormAction };
export const PEOPLE_COPY: { … };
export const LIST_PAGE_SIZE = 200; export const LIST_MAX_PAGES = 5; export const LIST_ROW_CAP = 1000;
export type PeopleModelAccessDeps = { authz: Pick<IamClients['authz'], 'listRoleGrants'>; tenancy: Pick<IamClients['tenancy'], 'listMemberships'>; mayI: MayI; iam: ServiceState };
export function readBounded<T>(read: (limit: number, offset: bigint) => Promise<IamResult<readonly T[]>>): Promise<IamResult<{ rows: readonly T[]; truncated: boolean }>>;
export function joinPeople(grants, members, membersTruncated): { holders: HolderRowView[]; candidates: CandidateView[] };
export function loadPeopleModelAccess(deps: PeopleModelAccessDeps, params: { orgPrn: string; orgActive: boolean }): Promise<PeopleModelAccessView>;
```

- [ ] **Step 1: Write the failing test**

Create `ts/apps/gateway-console/tests/integration/people-model-access-load.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The "Model access for people" loader (SMA-676 spec § 4.4, § 5.1, § 7.2) against the fake IAM:
// the disabled state makes no call (D15), the join offers the org creator as a candidate (D12,
// § 1.1 fact 6), a holder who is not a member shows the mark (D13), and each list reads pages of
// 200 with at most five pages and one N+1 probe.
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { Code } from '@connectrpc/connect';
import type { ServiceState } from '@paigasus/discovery/types';
import { disposeTransports } from '@paigasus/sdk/iam';
import { PrincipalKind } from '@paigasus/sdk/iam/types';
import { organizationPrn } from '@paigasus/console-core';
import { denial, startFakeIam, type FakeIam, type FakeIamHandlers } from '@paigasus/console-core/testing';
import { LIST_ROW_CAP, loadPeopleModelAccess, type PeopleModelAccessDeps } from '../../app/(console)/people-model-access/load';
import { IDS, callsSince, clientsFor, scriptedMayI } from './support';

const ORG = organizationPrn(IDS.orgA);
const CEDAR: ServiceState = { state: 'available', service: 'iam', descriptor: { service: 'iam', version: '1.0.0', capabilities: ['iam.authz.cedar'] }, capabilities: ['iam.authz.cedar'] };
const NO_CEDAR: ServiceState = { state: 'available', service: 'iam', descriptor: { service: 'iam', version: '1.0.0', capabilities: [] }, capabilities: [] };
const person = (n: number): string => `prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-${String(n).padStart(12, '0')}`;
const grant = (n: number, principalPrn: string, roleKey: string) => ({ id: `0190a1d4-0000-7000-8000-${String(n).padStart(12, '0')}`, principalPrn, roleKey, scopePrn: ORG });
const member = (principalPrn: string) => ({ id: `m-${principalPrn.slice(-4)}`, principalPrn, nodePrn: ORG });

let iam: FakeIam;
beforeAll(async () => {
  iam = await startFakeIam();
});
afterAll(async () => {
  disposeTransports();
  await iam.close();
});

function deps(allowed: { GrantRole?: boolean; RevokeRole?: boolean } = { GrantRole: true, RevokeRole: true }, state: ServiceState = CEDAR): PeopleModelAccessDeps {
  const clients = clientsFor(iam);
  return { authz: clients.authz, tenancy: clients.tenancy, mayI: scriptedMayI(allowed), iam: state };
}

/** A world of `grantRows` grants and `memberRows` members, both served with IAM's offset paging. */
function world(grantRows: readonly ReturnType<typeof grant>[], memberRows: readonly ReturnType<typeof member>[]): FakeIamHandlers {
  const slice = <T>(rows: readonly T[], req: { limit: number; offset: bigint }): T[] => rows.slice(Number(req.offset), Number(req.offset) + req.limit);
  return {
    'authz.listRoleGrants': (req) => ({ grants: slice(grantRows, req) }),
    'tenancy.listMemberships': (req) => ({ memberships: slice(memberRows, req) }),
  };
}

describe('loadPeopleModelAccess', () => {
  it('makes no IAM call when iam.authz.cedar is off (D15)', async () => {
    iam.setHandlers({});
    const calls = callsSince(iam);
    const may = scriptedMayI({ GrantRole: true, RevokeRole: true });
    const view = await loadPeopleModelAccess({ ...deps(), mayI: may, iam: NO_CEDAR }, { orgPrn: ORG, orgActive: true });
    expect(view).toEqual({ kind: 'disabled' });
    expect(calls('authz.listRoleGrants')).toHaveLength(0);
    expect(calls('tenancy.listMemberships')).toHaveLength(0);
    expect(may.asked).toEqual([]);
  });

  it('asks for user grants at the org and user members of the org, in pages of 200', async () => {
    iam.setHandlers(world([], []));
    const calls = callsSince(iam);
    await loadPeopleModelAccess(deps(), { orgPrn: ORG, orgActive: true });
    expect(calls('authz.listRoleGrants')[0]?.request).toMatchObject({ principalPrn: '', scopePrn: ORG, principalKind: PrincipalKind.USER, limit: 200, offset: 0n });
    expect(calls('tenancy.listMemberships')[0]?.request).toMatchObject({ filter: { case: 'nodePrn', value: ORG }, principalKind: PrincipalKind.USER, limit: 200, offset: 0n });
  });

  it('joins holders and candidates, offering the org creator who is not a member (D12, D13)', async () => {
    const creator = person(1);
    const memberHolder = person(2);
    const plainMember = person(3);
    iam.setHandlers(world([grant(1, creator, 'org_admin'), grant(2, memberHolder, 'gateway_user'), grant(3, person(4), 'gateway_user')], [member(memberHolder), member(plainMember)]));
    const view = await loadPeopleModelAccess(deps(), { orgPrn: ORG, orgActive: true });
    expect(view).toEqual({
      kind: 'ok',
      orgPrn: ORG,
      readOnly: false,
      holders: [
        { grantId: grant(2, memberHolder, 'gateway_user').id, principalPrn: memberHolder, member: true },
        { grantId: grant(3, person(4), 'gateway_user').id, principalPrn: person(4), member: false },
      ],
      candidates: [{ principalPrn: creator }, { principalPrn: plainMember }],
      flags: { canGrant: true, canRevoke: true, grantsTruncated: false, membersTruncated: false },
    });
  });

  it('shows controls only when mayI allows them and the org is active (D14)', async () => {
    iam.setHandlers(world([], []));
    const inactive = await loadPeopleModelAccess(deps(), { orgPrn: ORG, orgActive: false });
    expect(inactive).toMatchObject({ kind: 'ok', readOnly: true, flags: { canGrant: false, canRevoke: false } });
    const noRevoke = await loadPeopleModelAccess(deps({ GrantRole: true, RevokeRole: false }), { orgPrn: ORG, orgActive: true });
    expect(noRevoke).toMatchObject({ kind: 'ok', flags: { canGrant: true, canRevoke: false } });
  });

  it('reads five full pages and probes once; exactly 1000 rows is not truncated', async () => {
    const rows = Array.from({ length: LIST_ROW_CAP }, (_, i) => grant(10_000 + i, person(10_000 + i), 'org_admin'));
    iam.setHandlers(world(rows, []));
    const calls = callsSince(iam);
    const view = await loadPeopleModelAccess(deps(), { orgPrn: ORG, orgActive: true });
    const requests = calls('authz.listRoleGrants').map((call) => call.request as { limit: number; offset: bigint });
    expect(requests.map((r) => [r.limit, r.offset])).toEqual([
      [200, 0n],
      [200, 200n],
      [200, 400n],
      [200, 600n],
      [200, 800n],
      [1, 1000n],
    ]);
    expect(view).toMatchObject({ kind: 'ok', flags: { grantsTruncated: false } });
    if (view.kind === 'ok') expect(view.candidates).toHaveLength(LIST_ROW_CAP);
  });

  it('marks a list truncated when the probe finds row 1001, and hides the not-a-member mark when the member list is truncated', async () => {
    const holder = person(1);
    const members = Array.from({ length: LIST_ROW_CAP + 1 }, (_, i) => member(person(20_000 + i)));
    iam.setHandlers(world([grant(1, holder, 'gateway_user')], members));
    const view = await loadPeopleModelAccess(deps(), { orgPrn: ORG, orgActive: true });
    expect(view).toMatchObject({ kind: 'ok', flags: { grantsTruncated: false, membersTruncated: true } });
    if (view.kind === 'ok') expect(view.holders).toEqual([{ grantId: grant(1, holder, 'gateway_user').id, principalPrn: holder, member: null }]);
  });

  it('stops after a short page: one call for a list of 199', async () => {
    iam.setHandlers(world(Array.from({ length: 199 }, (_, i) => grant(30_000 + i, person(30_000 + i), 'org_admin')), []));
    const calls = callsSince(iam);
    await loadPeopleModelAccess(deps(), { orgPrn: ORG, orgActive: true });
    expect(calls('authz.listRoleGrants')).toHaveLength(1);
  });

  it('is denied when either list is forbidden, and an error for another failure; the page stays up', async () => {
    iam.setHandlers({
      'authz.listRoleGrants': () => {
        throw denial();
      },
      'tenancy.listMemberships': () => ({ memberships: [] }),
    });
    expect(await loadPeopleModelAccess(deps(), { orgPrn: ORG, orgActive: true })).toEqual({ kind: 'denied' });
    iam.setHandlers({
      'authz.listRoleGrants': () => ({ grants: [] }),
      'tenancy.listMemberships': () => {
        throw denial({ code: Code.Unavailable, reason: 'internal' });
      },
    });
    expect(await loadPeopleModelAccess(deps(), { orgPrn: ORG, orgActive: true })).toMatchObject({ kind: 'error' });
  });
});
```

- [ ] **Step 2: Run the test and see it fail**

Run: `pnpm -C ts/apps/gateway-console exec vitest run tests/integration/people-model-access-load.test.ts`
Expected: FAIL — `Cannot find module '../../app/(console)/people-model-access/load'`.

- [ ] **Step 3: Implement**

Create `ts/apps/gateway-console/app/(console)/gateway-role.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The role that holds only InvokeModel (SMA-636 spec § 1.2, SMA-676). ONE definition: the
// service-account commands and the "Model access for people" commands both grant it. A plain
// module with no runtime import, so a client component and a test may import it.
export const GATEWAY_ROLE = 'gateway_user';
```

In `service-accounts/commands.ts`, replace lines 16-17 (`/** The role … */ export const GATEWAY_ROLE = 'gateway_user';`) with `import { GATEWAY_ROLE } from '../gateway-role';` placed with the other imports. In `tests/integration/service-account-commands.test.ts:12`, remove `GATEWAY_ROLE,` from the `commands` import and add `import { GATEWAY_ROLE } from '../../app/(console)/gateway-role';`.

Create `ts/apps/gateway-console/app/(console)/people-model-access/view.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The view model and the copy of the "Model access for people" section (SMA-676 spec § 4.4, § 5).
// PLAIN DATA ONLY: a server loader builds it and a client component receives it. No `server-only`
// import and no runtime import except the copy below; the type imports are erased.
import type { FormAction } from '@paigasus/console-core';
import type { PaigasusError } from '@paigasus/sdk/errors/types';

export type HolderRowView = {
  readonly grantId: string;
  readonly principalPrn: string;
  /** D13: false shows "Not a member"; null hides the mark because the member list is truncated. */
  readonly member: boolean | null;
};

export type CandidateView = { readonly principalPrn: string };

export type PeopleFlags = { readonly canGrant: boolean; readonly canRevoke: boolean; readonly grantsTruncated: boolean; readonly membersTruncated: boolean };

export type PeopleModelAccessOk = {
  readonly kind: 'ok';
  readonly orgPrn: string;
  /** § 5.1 step 4: the org or an ancestor is not active, so IAM refuses changes. */
  readonly readOnly: boolean;
  readonly holders: readonly HolderRowView[];
  readonly candidates: readonly CandidateView[];
  readonly flags: PeopleFlags;
};

export type PeopleModelAccessView = { readonly kind: 'disabled' } | { readonly kind: 'denied' } | { readonly kind: 'error'; readonly error: PaigasusError } | PeopleModelAccessOk;

export type PeopleModelAccessActions = { readonly grant: FormAction; readonly revoke: FormAction };

export const PEOPLE_COPY = {
  title: 'Model access for people',
  disabled: 'Role administration is not enabled on this IAM.',
  denied: 'You cannot see model access for this organization.',
  readOnly: 'This organization is not active. IAM refuses changes to its role grants.',
  noHolders: 'No person holds model access.',
  noCandidates: 'No other person to grant model access to.',
  notMember: 'Not a member',
  grantsTruncated: 'Showing the first 1000 role grants at this organization.',
  membersTruncated: 'Showing the first 1000 members of this organization.',
  choose: 'Choose a person',
  grant: 'Grant model access',
  revoke: 'Revoke',
} as const;
```

Create `ts/apps/gateway-console/app/(console)/people-model-access/load.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The loader of the "Model access for people" section (SMA-676 spec § 4.4, § 5.1). It returns a
// VIEW MODEL (./view). All IAM calls go through callIam.
//
// D15: with iam.authz.cedar off it makes NO call — ListRoleGrants itself is behind that capability,
// so a call could only produce the error state. Else it reads, in parallel, the USER grants at the
// org and the USER members of the org, each in pages of 200 (IAM's Page maximum; ListMemberships
// refuses more), at most five pages, then one N+1 probe (lib/paging.ts:3-6's rule on a bounded
// read), and asks mayI for GrantRole and RevokeRole. mayI is cosmetic: IAM is the gate (D14).
import 'server-only';
import type { ServiceState } from '@paigasus/discovery/types';
import { PrincipalKind } from '@paigasus/sdk/iam/types';
import { callIam, cedarCapabilityOf, type IamClients, type IamResult, type MayI } from '@paigasus/console-core';
import { GATEWAY_ROLE } from '../gateway-role';
import type { CandidateView, HolderRowView, PeopleModelAccessView } from './view';

/** IAM's Page maximum (application/pagination.rs:11). */
export const LIST_PAGE_SIZE = 200;
/** § 4.4: at most five pages per list. */
export const LIST_MAX_PAGES = 5;
/** § 9: each list stops at 1000 rows. */
export const LIST_ROW_CAP = LIST_PAGE_SIZE * LIST_MAX_PAGES;

export type PeopleModelAccessDeps = {
  readonly authz: Pick<IamClients['authz'], 'listRoleGrants'>;
  readonly tenancy: Pick<IamClients['tenancy'], 'listMemberships'>;
  readonly mayI: MayI;
  /** This request's IAM discovery state (memoized per request by discovery()). */
  readonly iam: ServiceState;
};

export type PeopleModelAccessParams = { readonly orgPrn: string; readonly orgActive: boolean };

type Bounded<T> = { readonly rows: readonly T[]; readonly truncated: boolean };

/** Pages of LIST_PAGE_SIZE until a short page, at most LIST_MAX_PAGES; after five full pages, one probe for row 1001. */
export async function readBounded<T>(read: (limit: number, offset: bigint) => Promise<IamResult<readonly T[]>>): Promise<IamResult<Bounded<T>>> {
  const rows: T[] = [];
  for (let page = 0; page < LIST_MAX_PAGES; page += 1) {
    const got = await read(LIST_PAGE_SIZE, BigInt(page * LIST_PAGE_SIZE));
    if (!got.ok) return got;
    rows.push(...got.value);
    if (got.value.length < LIST_PAGE_SIZE) return { ok: true, value: { rows, truncated: false } };
  }
  const probe = await read(1, BigInt(LIST_ROW_CAP));
  if (!probe.ok) return probe;
  return { ok: true, value: { rows, truncated: probe.value.length > 0 } };
}

type GrantRow = { readonly id: string; readonly principalPrn: string; readonly roleKey: string };
type MemberRow = { readonly principalPrn: string };

/**
 * § 4.4 "Join". Holders = the gateway_user grants. Candidates = (member PRNs ∪ grantee PRNs) − holder
 * PRNs, sorted: a user with ANY role at the org is offered, so the org creator appears (D12). A
 * holder's `member` is null when the member list is truncated (D13).
 */
export function joinPeople(grants: readonly GrantRow[], members: readonly MemberRow[], membersTruncated: boolean): { holders: HolderRowView[]; candidates: CandidateView[] } {
  const memberPrns = new Set(members.map((row) => row.principalPrn));
  const holders = grants
    .filter((row) => row.roleKey === GATEWAY_ROLE)
    .map((row): HolderRowView => ({ grantId: row.id, principalPrn: row.principalPrn, member: membersTruncated ? null : memberPrns.has(row.principalPrn) }))
    .sort((a, b) => (a.principalPrn < b.principalPrn ? -1 : a.principalPrn > b.principalPrn ? 1 : 0));
  const holderPrns = new Set(holders.map((row) => row.principalPrn));
  const pool = new Set([...memberPrns, ...grants.map((row) => row.principalPrn)]);
  const candidates = [...pool]
    .filter((prn) => !holderPrns.has(prn))
    .sort()
    .map((principalPrn) => ({ principalPrn }));
  return { holders, candidates };
}

export async function loadPeopleModelAccess(deps: PeopleModelAccessDeps, params: PeopleModelAccessParams): Promise<PeopleModelAccessView> {
  if (!cedarCapabilityOf(deps.iam)) return { kind: 'disabled' };
  const { orgPrn } = params;
  const [grants, members, mayGrant, mayRevoke] = await Promise.all([
    readBounded((limit, offset) => callIam(async () => (await deps.authz.listRoleGrants({ scopePrn: orgPrn, principalKind: PrincipalKind.USER, limit, offset })).grants)),
    readBounded((limit, offset) => callIam(async () => (await deps.tenancy.listMemberships({ filter: { case: 'nodePrn', value: orgPrn }, principalKind: PrincipalKind.USER, limit, offset })).memberships)),
    deps.mayI('GrantRole', orgPrn),
    deps.mayI('RevokeRole', orgPrn),
  ]);
  if ((!grants.ok && grants.error.presentation === 'forbidden') || (!members.ok && members.error.presentation === 'forbidden')) return { kind: 'denied' };
  if (!grants.ok) return { kind: 'error', error: grants.error };
  if (!members.ok) return { kind: 'error', error: members.error };

  const { holders, candidates } = joinPeople(grants.value.rows, members.value.rows, members.value.truncated);
  return {
    kind: 'ok',
    orgPrn,
    readOnly: !params.orgActive,
    holders,
    candidates,
    flags: { canGrant: params.orgActive && mayGrant, canRevoke: params.orgActive && mayRevoke, grantsTruncated: grants.value.truncated, membersTruncated: members.value.truncated },
  };
}
```

- [ ] **Step 4: Run the tests and see them pass**

Run: `pnpm -C ts/apps/gateway-console exec vitest run tests/integration/people-model-access-load.test.ts tests/integration/service-account-commands.test.ts && moon run gateway-console-ts:typecheck ts:fmt ts:lint`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add ts/apps/gateway-console
git commit -F - <<'EOF'
feat(ts): load model access for people on the organization page

SMA-676 section 4.4 and 5.1. The loader reads user grants at the org and
user members of the org in bounded pages of 200 with an N+1 probe, joins
holders and candidates, and makes no call when the cedar capability is
off. GATEWAY_ROLE moves to one shared module.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
```

---

## Task 10: Commands and Server Actions (grant, bounded revoke)

**Files:**
- Create: `ts/apps/gateway-console/app/(console)/people-model-access/commands.ts`
- Create: `ts/apps/gateway-console/app/(console)/people-model-access/actions.ts`
- Create: `ts/apps/gateway-console/tests/integration/people-model-access-commands.test.ts`
- Modify: `ts/apps/gateway-console/tests/unit/actions-structure.test.ts:26-28` (`EXPECTED`)

**Interfaces:**
- Consumes: `GATEWAY_ROLE`, `callIam`, `toActionResult`, `prnField`, `formFields`, `invalidFormInput`, `neverReachedIam`, `iamClientsForAction`, `refreshesAfterMutation`, `SETTINGS_PATH`.
- Produces:

```ts
export const grantModelAccessForm: z.ZodObject<{ principalPrn; orgPrn }>;
export const revokeModelAccessForm: z.ZodObject<{ principalPrn; orgPrn; grantId }>;
export function grantModelAccess(deps: { authz: Pick<Authz, 'grantRole'> }, input: GrantModelAccessInput): Promise<ActionResult>;
export function revokeModelAccess(deps: { authz: Pick<Authz, 'listRoleGrants' | 'revokeRole'> }, input: RevokeModelAccessInput): Promise<ActionResult>;
export async function grantModelAccessAction(_previous: ActionState, form: FormData): Promise<ActionState>;
export async function revokeModelAccessAction(_previous: ActionState, form: FormData): Promise<ActionState>;
```

- [ ] **Step 1: Write the failing tests**

Create `ts/apps/gateway-console/tests/integration/people-model-access-commands.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The two commands of "Model access for people" (SMA-676 spec § 4.4, § 5.2, § 5.3, § 6) against the
// fake IAM. The role key is a server constant. The revoke is bounded to gateway_user of THAT
// principal at THIS org: a crafted form cannot revoke another role (§ 6), also against an old IAM
// that ignores the new filters (§ 10).
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { Code } from '@connectrpc/connect';
import { disposeTransports } from '@paigasus/sdk/iam';
import { organizationPrn } from '@paigasus/console-core';
import { denial, startFakeIam, type FakeIam } from '@paigasus/console-core/testing';
import { grantModelAccess, revokeModelAccess } from '../../app/(console)/people-model-access/commands';
import { IDS, callsSince, clientsFor } from './support';

const ORG = organizationPrn(IDS.orgA);
const PERSON = 'prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-0000000000c1';
const GRANT_ID = '0190a1d4-0000-7000-8000-0000000000c2';
const ADMIN_GRANT_ID = '0190a1d4-0000-7000-8000-0000000000c3';

let iam: FakeIam;
beforeAll(async () => {
  iam = await startFakeIam();
});
afterAll(async () => {
  disposeTransports();
  await iam.close();
});

describe('grantModelAccess (§ 5.2)', () => {
  it('grants gateway_user to the person at the org', async () => {
    iam.setHandlers({ 'authz.grantRole': (req) => ({ grant: { id: GRANT_ID, principalPrn: req.principalPrn, roleKey: req.roleKey, scopePrn: req.scopePrn } }) });
    const calls = callsSince(iam);
    expect(await grantModelAccess({ authz: clientsFor(iam).authz }, { principalPrn: PERSON, orgPrn: ORG })).toEqual({ ok: true });
    expect(calls('authz.grantRole').map((call) => call.request)).toEqual([expect.objectContaining({ principalPrn: PERSON, roleKey: 'gateway_user', scopePrn: ORG })]);
  });

  it('returns a denial as IAM answers it', async () => {
    iam.setHandlers({
      'authz.grantRole': () => {
        throw denial();
      },
    });
    const result = await grantModelAccess({ authz: clientsFor(iam).authz }, { principalPrn: PERSON, orgPrn: ORG });
    expect(result).toMatchObject({ ok: false, error: { presentation: 'forbidden' } });
  });
});

describe('revokeModelAccess (§ 5.3)', () => {
  it('checks the grant first, then revokes it', async () => {
    iam.setHandlers({
      'authz.listRoleGrants': () => ({ grants: [{ id: GRANT_ID, principalPrn: PERSON, roleKey: 'gateway_user', scopePrn: ORG }] }),
      'authz.revokeRole': () => ({}),
    });
    const calls = callsSince(iam);
    expect(await revokeModelAccess({ authz: clientsFor(iam).authz }, { principalPrn: PERSON, orgPrn: ORG, grantId: GRANT_ID })).toEqual({ ok: true });
    expect(calls('authz.listRoleGrants')[0]?.request).toMatchObject({ principalPrn: PERSON, scopePrn: ORG, roleKey: 'gateway_user' });
    expect(calls('authz.revokeRole').map((call) => call.request)).toEqual([expect.objectContaining({ id: GRANT_ID })]);
  });

  it('refuses a grant id that is not this person’s gateway_user grant at this org, and revokes nothing (§ 4.4, § 6)', async () => {
    iam.setHandlers({ 'authz.listRoleGrants': () => ({ grants: [{ id: GRANT_ID, principalPrn: PERSON, roleKey: 'gateway_user', scopePrn: ORG }] }) });
    const calls = callsSince(iam);
    const result = await revokeModelAccess({ authz: clientsFor(iam).authz }, { principalPrn: PERSON, orgPrn: ORG, grantId: ADMIN_GRANT_ID });
    expect(result).toMatchObject({ ok: false, error: { presentation: 'invalid-input' } });
    expect(calls('authz.revokeRole')).toHaveLength(0);
  });

  it('refuses a grant id of another role even when IAM ignores the filters (spec § 10)', async () => {
    // An old IAM answers every grant of the principal: the org_admin grant is in the answer too.
    iam.setHandlers({
      'authz.listRoleGrants': () => ({
        grants: [
          { id: GRANT_ID, principalPrn: PERSON, roleKey: 'gateway_user', scopePrn: ORG },
          { id: ADMIN_GRANT_ID, principalPrn: PERSON, roleKey: 'org_admin', scopePrn: ORG },
        ],
      }),
    });
    const calls = callsSince(iam);
    const result = await revokeModelAccess({ authz: clientsFor(iam).authz }, { principalPrn: PERSON, orgPrn: ORG, grantId: ADMIN_GRANT_ID });
    expect(result).toMatchObject({ ok: false, error: { presentation: 'invalid-input' } });
    expect(calls('authz.revokeRole')).toHaveLength(0);
  });

  it('is a success with no revoke when the grant is already gone (§ 5.3 step 4)', async () => {
    iam.setHandlers({ 'authz.listRoleGrants': () => ({ grants: [] }) });
    const calls = callsSince(iam);
    expect(await revokeModelAccess({ authz: clientsFor(iam).authz }, { principalPrn: PERSON, orgPrn: ORG, grantId: GRANT_ID })).toEqual({ ok: true });
    expect(calls('authz.revokeRole')).toHaveLength(0);
  });

  it('is a success when another admin revoked between the check and the revoke (not-found)', async () => {
    iam.setHandlers({
      'authz.listRoleGrants': () => ({ grants: [{ id: GRANT_ID, principalPrn: PERSON, roleKey: 'gateway_user', scopePrn: ORG }] }),
      'authz.revokeRole': () => {
        throw denial({ code: Code.NotFound, reason: 'not-found' });
      },
    });
    expect(await revokeModelAccess({ authz: clientsFor(iam).authz }, { principalPrn: PERSON, orgPrn: ORG, grantId: GRANT_ID })).toEqual({ ok: true });
  });

  it('returns a denial of the check or of the revoke', async () => {
    iam.setHandlers({
      'authz.listRoleGrants': () => {
        throw denial();
      },
    });
    expect(await revokeModelAccess({ authz: clientsFor(iam).authz }, { principalPrn: PERSON, orgPrn: ORG, grantId: GRANT_ID })).toMatchObject({ ok: false, error: { presentation: 'forbidden' } });
  });
});
```

In `tests/unit/actions-structure.test.ts`, change `EXPECTED` to:

```ts
const EXPECTED: Readonly<Record<string, readonly string[]>> = {
  '(console)/people-model-access/actions.ts': ['grantModelAccessAction', 'revokeModelAccessAction'],
  '(console)/service-accounts/actions.ts': ['allowModelCallsAction', 'archiveServiceAccountAction', 'createServiceAccountAction', 'issueApiKeyAction', 'revokeApiKeyAction'],
};
```

- [ ] **Step 2: Run the tests and see them fail**

Run: `pnpm -C ts/apps/gateway-console exec vitest run tests/integration/people-model-access-commands.test.ts tests/unit/actions-structure.test.ts`
Expected: FAIL — module not found; `actions-structure` reports a missing file.

- [ ] **Step 3: Implement**

Create `ts/apps/gateway-console/app/(console)/people-model-access/commands.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The two commands of "Model access for people" (SMA-676 spec § 4.4, § 5.2, § 5.3). Pure and
// dependency-injected; they take NO mayI: IAM decides (§ 6).
//
// The role key is the server constant GATEWAY_ROLE, so a form cannot grant another role. The grant
// does NOT check that the principal is a candidate: D12 is a UI choice, and IAM already lets an
// org_admin grant to any principal with a direct call (§ 6).
//
// The revoke is BOUNDED (§ 5.3 step 2): ListRoleGrants(principal, org, gateway_user) first, and only
// a grant of that set is revoked. The rows are checked for role, scope and principal too, because an
// OLD IAM ignores the filters and answers every grant of the principal (§ 10).
import 'server-only';
import { z } from 'zod';
import type { PaigasusError } from '@paigasus/sdk/errors/types';
import { callIam, neverReachedIam, prnField, toActionResult, type ActionResult, type IamClients } from '@paigasus/console-core';
import { GATEWAY_ROLE } from '../gateway-role';

type Authz = IamClients['authz'];

export const grantModelAccessForm = z.object({ principalPrn: prnField, orgPrn: prnField });
export type GrantModelAccessInput = z.infer<typeof grantModelAccessForm>;

/** zod 4's z.uuid() is RFC-strict; IAM mints UUIDv7 grant ids, which are RFC 4122 ids. */
export const revokeModelAccessForm = z.object({ principalPrn: prnField, orgPrn: prnField, grantId: z.string().trim().pipe(z.uuid()) });
export type RevokeModelAccessInput = z.infer<typeof revokeModelAccessForm>;

/** The form named a grant that is not this person's model access at this org. */
function notThisGrant(): PaigasusError {
  return neverReachedIam({ presentation: 'invalid-input', message: 'The grant is not model access of this person at this organization.', transport: { kind: 'http', status: 400 } });
}

/** § 5.2. The idempotent case (D9) is a success too: IAM returns the existing grant. */
export async function grantModelAccess(deps: { readonly authz: Pick<Authz, 'grantRole'> }, input: GrantModelAccessInput): Promise<ActionResult> {
  return toActionResult(await callIam(() => deps.authz.grantRole({ principalPrn: input.principalPrn, roleKey: GATEWAY_ROLE, scopePrn: input.orgPrn })));
}

/**
 * § 5.3. An empty model-access set means the grant is gone (another admin revoked first): success,
 * no call. A set that does not hold the id means a crafted form: invalid-input, no call. A not-found
 * from RevokeRole is a race with another admin: success.
 */
export async function revokeModelAccess(deps: { readonly authz: Pick<Authz, 'listRoleGrants' | 'revokeRole'> }, input: RevokeModelAccessInput): Promise<ActionResult> {
  const listed = await callIam(() => deps.authz.listRoleGrants({ principalPrn: input.principalPrn, scopePrn: input.orgPrn, roleKey: GATEWAY_ROLE }));
  if (!listed.ok) return { ok: false, error: listed.error };
  const access = listed.value.grants.filter((grant) => grant.roleKey === GATEWAY_ROLE && grant.scopePrn === input.orgPrn && grant.principalPrn === input.principalPrn);
  if (access.length === 0) return { ok: true };
  if (!access.some((grant) => grant.id === input.grantId)) return { ok: false, error: notThisGrant() };
  const revoked = await callIam(() => deps.authz.revokeRole({ id: input.grantId }));
  if (!revoked.ok && revoked.error.presentation === 'not-found') return { ok: true };
  return toActionResult(revoked);
}
```

Create `ts/apps/gateway-console/app/(console)/people-model-access/actions.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
'use server';

// The two Server Actions of "Model access for people" (SMA-676 spec § 4.4). Each follows the rule
// of ../service-accounts/actions.ts, in this order: iamClientsForAction() first; then a zod check of
// ONLY the fields it accepts (formFields names them); then a pure command. No action consults mayI():
// IAM decides. tests/unit/actions-structure.test.ts holds these rules.
//
// Revalidation follows ../service-accounts/revalidation.ts: after every result except `relogin`.
import { revalidatePath } from 'next/cache';
import { formFields, invalidFormInput, type ActionState } from '@paigasus/console-core';
import { iamClientsForAction } from '../../../lib/console';
import { SETTINGS_PATH } from '../../../lib/settings-path';
import { refreshesAfterMutation } from '../service-accounts/revalidation';
import { grantModelAccess, grantModelAccessForm, revokeModelAccess, revokeModelAccessForm } from './commands';

export async function grantModelAccessAction(_previous: ActionState, form: FormData): Promise<ActionState> {
  const clients = await iamClientsForAction();
  if (!clients.ok) return clients;
  const parsed = grantModelAccessForm.safeParse(formFields(form, ['principalPrn', 'orgPrn']));
  if (!parsed.success) return { ok: false, error: invalidFormInput() };
  const result = await grantModelAccess({ authz: clients.value.authz }, parsed.data);
  if (refreshesAfterMutation(result)) revalidatePath(SETTINGS_PATH, 'layout');
  return result;
}

export async function revokeModelAccessAction(_previous: ActionState, form: FormData): Promise<ActionState> {
  const clients = await iamClientsForAction();
  if (!clients.ok) return clients;
  const parsed = revokeModelAccessForm.safeParse(formFields(form, ['principalPrn', 'orgPrn', 'grantId']));
  if (!parsed.success) return { ok: false, error: invalidFormInput() };
  const result = await revokeModelAccess({ authz: clients.value.authz }, parsed.data);
  if (refreshesAfterMutation(result)) revalidatePath(SETTINGS_PATH, 'layout');
  return result;
}
```

- [ ] **Step 4: Run the tests and see them pass**

Run: `pnpm -C ts/apps/gateway-console exec vitest run tests/integration/people-model-access-commands.test.ts tests/unit/actions-structure.test.ts && moon run gateway-console-ts:typecheck ts:fmt ts:lint`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add ts/apps/gateway-console
git commit -F - <<'EOF'
feat(ts): add the grant and bounded revoke of model access for people

SMA-676 section 4.4, 5.2, 5.3 and 6. The grant uses the server constant
role. The revoke first lists the person's gateway_user grants at the org
and revokes only an id in that set, checked by role, scope and principal
so an IAM that ignores the filters cannot widen it. A grant that is gone
is a success.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
```

---

## Task 11: The block, the client section and the org page

**Files:**
- Create: `ts/apps/gateway-console/app/(console)/people-model-access/block.tsx`
- Create: `ts/apps/gateway-console/app/_components/people-model-access-section.tsx`
- Create: `ts/apps/gateway-console/tests/unit/people-model-access-block.test.tsx`
- Modify: `ts/apps/gateway-console/app/(console)/orgs/[org]/load.ts:17-19` (imports), `:34-43` (type), `:50` (deps), `:101-109` (loader)
- Modify: `ts/apps/gateway-console/app/(console)/orgs/[org]/page.tsx:22-28` (imports), `:61-62` (render)
- Modify: `ts/apps/gateway-console/tests/integration/org-settings-load.test.ts:44-51` (world), new case

**Interfaces:**
- Consumes: `PeopleModelAccessView`, `PEOPLE_COPY`, the two actions, `loadPeopleModelAccess`, `lifecycleView`.
- Produces: `peopleModelAccessBlock({ view }): Promise<ReactElement>`; `PeopleModelAccessSection({ view, actions })`; `OrganizationSettings` ok gains `people: PeopleModelAccessView`; `OrganizationSettingsDeps = SectionDeps & PeopleModelAccessDeps & { tenancy: Pick<Tenancy, 'getOrganization' | 'listTeams' | 'listProjects'> }`.

- [ ] **Step 1: Write the failing tests**

Create `ts/apps/gateway-console/tests/unit/people-model-access-block.test.tsx`:

```tsx
// SPDX-License-Identifier: Apache-2.0
// @vitest-environment jsdom
//
// The "Model access for people" block in each state (SMA-676 spec § 5.1, § 7.2): disabled, denied,
// error, holders and candidates, empty, read-only, each truncation line, the not-a-member mark and its
// hiding, and the two forms carrying the right fields to their actions.
import { cleanup, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactElement } from 'react';
import { afterEach, beforeAll, describe, expect, it, vi } from 'vitest';
import { ZoneProvider } from '@paigasus/app-shell';
import type { PaigasusError } from '@paigasus/sdk/errors/types';
import { peopleModelAccessBlock } from '../../app/(console)/people-model-access/block';
import { PEOPLE_COPY, type PeopleModelAccessOk, type PeopleModelAccessView } from '../../app/(console)/people-model-access/view';

const actions = vi.hoisted(() => ({
  grant: vi.fn((_previous: unknown, _form: FormData) => Promise.resolve({ ok: true as const })),
  revoke: vi.fn((_previous: unknown, _form: FormData) => Promise.resolve({ ok: true as const })),
}));
vi.mock('../../app/(console)/people-model-access/actions', () => ({ grantModelAccessAction: actions.grant, revokeModelAccessAction: actions.revoke }));
vi.mock('next/navigation', async (importOriginal) => ({ ...(await importOriginal<typeof import('next/navigation')>()), usePathname: () => '/orgs/0190a100-0000-7000-8000-00000000000a' }));

// cmdk's CommandList measures itself with ResizeObserver, which jsdom lacks (the same stub
// ts/packages/paigasus-ui/tests/setup.ts installs for its combobox tests).
beforeAll(() => {
  globalThis.ResizeObserver ??= class {
    observe(): void {}
    unobserve(): void {}
    disconnect(): void {}
  } as unknown as typeof ResizeObserver;
});
afterEach(() => {
  cleanup();
  actions.grant.mockClear();
  actions.revoke.mockClear();
});

const ORG = 'prn:pgs:iam:::organization/0190a100-0000-7000-8000-00000000000a';
const HOLDER = 'prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-0000000000d1';
const OUTSIDER = 'prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-0000000000d2';
const CANDIDATE = 'prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-0000000000d3';
const GRANT_ID = '0190a1d4-0000-7000-8000-0000000000d4';
const ERROR: PaigasusError = {
  presentation: 'degraded',
  domain: null,
  reason: null,
  rawReason: null,
  rawDomain: null,
  message: 'IAM text that must never show',
  correlationId: 'corr-unit-people',
  requestId: null,
  retryable: true,
  metadata: {},
  transport: { kind: 'grpc', code: 14, codeName: 'Unavailable' },
};

function ok(patch: Partial<PeopleModelAccessOk> = {}): PeopleModelAccessOk {
  return {
    kind: 'ok',
    orgPrn: ORG,
    readOnly: false,
    holders: [
      { grantId: GRANT_ID, principalPrn: HOLDER, member: true },
      { grantId: '0190a1d4-0000-7000-8000-0000000000d5', principalPrn: OUTSIDER, member: false },
    ],
    candidates: [{ principalPrn: CANDIDATE }],
    flags: { canGrant: true, canRevoke: true, grantsTruncated: false, membersTruncated: false },
    ...patch,
  };
}

async function block(view: PeopleModelAccessView): Promise<ReactElement> {
  return <ZoneProvider zone="gateway" zones={{ gateway: '/gateway' }}>{await peopleModelAccessBlock({ view })}</ZoneProvider>;
}

describe('peopleModelAccessBlock', () => {
  it('shows the disabled line (D15)', async () => {
    render(await block({ kind: 'disabled' }));
    expect(screen.getByTestId('people-model-access-disabled').textContent).toBe(PEOPLE_COPY.disabled);
    expect(screen.getByRole('heading', { name: PEOPLE_COPY.title })).toBeDefined();
  });

  it('shows the denied line', async () => {
    render(await block({ kind: 'denied' }));
    expect(screen.getByTestId('people-model-access-denied').textContent).toBe(PEOPLE_COPY.denied);
  });

  it('shows the section error, never IAM’s message', async () => {
    render(await block({ kind: 'error', error: ERROR }));
    expect(screen.getByTestId('section-error')).toBeDefined();
    expect(screen.queryByText('IAM text that must never show')).toBeNull();
  });

  it('lists holders with the not-a-member mark, and a Revoke per row', async () => {
    render(await block(ok()));
    const rows = screen.getAllByTestId('people-holder-row');
    expect(rows.map((row) => row.getAttribute('data-principal'))).toEqual([HOLDER, OUTSIDER]);
    expect(within(rows[0] as HTMLElement).queryByTestId('people-not-member')).toBeNull();
    expect(within(rows[1] as HTMLElement).getByTestId('people-not-member').textContent).toBe(PEOPLE_COPY.notMember);
    expect(within(rows[0] as HTMLElement).getByRole('button', { name: PEOPLE_COPY.revoke })).toBeDefined();
  });

  it('hides the mark when the member list is truncated (member: null), and shows both truncation lines', async () => {
    render(await block(ok({ holders: [{ grantId: GRANT_ID, principalPrn: HOLDER, member: null }], flags: { canGrant: true, canRevoke: true, grantsTruncated: true, membersTruncated: true } })));
    expect(screen.queryByTestId('people-not-member')).toBeNull();
    expect(screen.getByTestId('people-grants-truncated').textContent).toBe(PEOPLE_COPY.grantsTruncated);
    expect(screen.getByTestId('people-members-truncated').textContent).toBe(PEOPLE_COPY.membersTruncated);
  });

  it('shows the empty states', async () => {
    render(await block(ok({ holders: [], candidates: [] })));
    expect(screen.getByText(PEOPLE_COPY.noHolders)).toBeDefined();
    expect(screen.getByText(PEOPLE_COPY.noCandidates)).toBeDefined();
    expect(screen.queryByRole('combobox')).toBeNull();
  });

  it('is read-only: a note, and no control', async () => {
    render(await block(ok({ readOnly: true, flags: { canGrant: false, canRevoke: false, grantsTruncated: false, membersTruncated: false } })));
    expect(screen.getByTestId('people-read-only').textContent).toBe(PEOPLE_COPY.readOnly);
    expect(screen.queryByRole('button', { name: PEOPLE_COPY.revoke })).toBeNull();
    expect(screen.queryByRole('button', { name: PEOPLE_COPY.grant })).toBeNull();
  });

  it('grants the chosen candidate at the org', async () => {
    const user = userEvent.setup();
    render(await block(ok()));
    await user.click(screen.getByRole('combobox'));
    await user.click(await screen.findByRole('option', { name: CANDIDATE }));
    await user.click(screen.getByRole('button', { name: PEOPLE_COPY.grant }));
    expect(actions.grant).toHaveBeenCalledTimes(1);
    const form = actions.grant.mock.calls[0]?.[1];
    expect(form?.get('principalPrn')).toBe(CANDIDATE);
    expect(form?.get('orgPrn')).toBe(ORG);
  });

  it('revokes with the principal, the org and the grant id of the row', async () => {
    const user = userEvent.setup();
    render(await block(ok()));
    await user.click(within(screen.getAllByTestId('people-holder-row')[0] as HTMLElement).getByRole('button', { name: PEOPLE_COPY.revoke }));
    const form = actions.revoke.mock.calls[0]?.[1];
    expect([form?.get('principalPrn'), form?.get('orgPrn'), form?.get('grantId')]).toEqual([HOLDER, ORG, GRANT_ID]);
  });
});
```

In `tests/integration/org-settings-load.test.ts`, extend `world()` with:

```ts
    'authz.listRoleGrants': () => ({ grants: [] }),
    'tenancy.listMemberships': () => ({ memberships: [] }),
```

and append inside `describe('loadOrganizationSettings', …)`:

```ts
  it('loads the people section with the org PRN, and makes no people call before GetOrganization succeeds (SMA-676)', async () => {
    iam.setHandlers(world());
    const calls = callsSince(iam);
    const data = await loadOrganizationSettings(deps(), params());
    if (data.kind !== 'ok') throw new Error(`expected ok, got ${data.kind}`);
    expect(data.people).toMatchObject({ kind: 'ok', orgPrn: ORG, holders: [], candidates: [] });
    expect(calls('authz.listRoleGrants')[0]?.request).toMatchObject({ scopePrn: ORG });
    expect(calls('tenancy.listMemberships')[0]?.request).toMatchObject({ filter: { case: 'nodePrn', value: ORG } });
  });
```

- [ ] **Step 2: Run the tests and see them fail**

Run: `pnpm -C ts/apps/gateway-console exec vitest run tests/unit/people-model-access-block.test.tsx tests/integration/org-settings-load.test.ts`
Expected: FAIL — `block` module not found; `data.people` is `undefined`.

- [ ] **Step 3: Implement the client section**

Create `ts/apps/gateway-console/app/_components/people-model-access-section.tsx`:

```tsx
// SPDX-License-Identifier: Apache-2.0
//
// The body of "Model access for people" (SMA-676 spec § 4.4, § 5): the holders table with a Revoke
// per row, the candidate combobox with a Grant button, the empty states, the read-only note and the
// truncation lines. CLIENT component. Every submit control renders only after hydration (the rule of
// service-account-section.tsx). Each form shows its own error; a success revalidates the page, and
// the revalidated render moves the person between the two lists.
'use client';

import { useActionState, useState, type ReactElement } from 'react';
import { Combobox, EmptyState, PRIMARY_BUTTON_CLASS, SECONDARY_BUTTON_CLASS, Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@paigasus/ui';
import { PEOPLE_COPY, type CandidateView, type HolderRowView, type PeopleModelAccessActions, type PeopleModelAccessOk } from '../(console)/people-model-access/view';
import { FormError } from './form-error';
import { useHydrated } from './use-hydrated';

function HolderRow({ row, orgPrn, canRevoke, revoke }: { readonly row: HolderRowView; readonly orgPrn: string; readonly canRevoke: boolean; readonly revoke: PeopleModelAccessActions['revoke'] }): ReactElement {
  const [state, formAction, pending] = useActionState(revoke, null);
  const hydrated = useHydrated();
  return (
    <TableRow data-testid="people-holder-row" data-principal={row.principalPrn}>
      <TableCell className="font-mono text-xs">{row.principalPrn}</TableCell>
      <TableCell>
        {row.member === false ? (
          <span data-testid="people-not-member" className="border-input text-muted-foreground rounded-pgs border px-2 py-0.5 text-xs font-medium">
            {PEOPLE_COPY.notMember}
          </span>
        ) : null}
      </TableCell>
      <TableCell>
        {canRevoke && hydrated ? (
          <form action={formAction} aria-label={`Revoke model access of ${row.principalPrn}`}>
            <input type="hidden" name="principalPrn" value={row.principalPrn} />
            <input type="hidden" name="orgPrn" value={orgPrn} />
            <input type="hidden" name="grantId" value={row.grantId} />
            <button type="submit" disabled={pending} className={SECONDARY_BUTTON_CLASS}>
              {PEOPLE_COPY.revoke}
            </button>
            <FormError error={state?.ok === false ? state.error : null} />
          </form>
        ) : null}
      </TableCell>
    </TableRow>
  );
}

function GrantForm({ orgPrn, candidates, grant }: { readonly orgPrn: string; readonly candidates: readonly CandidateView[]; readonly grant: PeopleModelAccessActions['grant'] }): ReactElement {
  const [principalPrn, setPrincipalPrn] = useState('');
  const [state, formAction, pending] = useActionState(grant, null);
  return (
    <form action={formAction} aria-label={PEOPLE_COPY.grant} data-testid="people-grant-form" className="flex max-w-xl flex-col gap-3">
      <input type="hidden" name="orgPrn" value={orgPrn} />
      <input type="hidden" name="principalPrn" value={principalPrn} />
      <Combobox items={candidates.map((candidate) => ({ value: candidate.principalPrn, label: candidate.principalPrn }))} value={principalPrn} onValueChange={setPrincipalPrn} placeholder={PEOPLE_COPY.choose} />
      <button type="submit" disabled={pending || principalPrn === ''} className={PRIMARY_BUTTON_CLASS}>
        {PEOPLE_COPY.grant}
      </button>
      <FormError error={state?.ok === false ? state.error : null} />
    </form>
  );
}

export function PeopleModelAccessSection({ view, actions }: { readonly view: PeopleModelAccessOk; readonly actions: PeopleModelAccessActions }): ReactElement {
  const hydrated = useHydrated();
  return (
    <div className="flex flex-col gap-3">
      {view.readOnly ? (
        <p data-testid="people-read-only" className="text-muted-foreground text-sm">
          {PEOPLE_COPY.readOnly}
        </p>
      ) : null}
      {view.holders.length === 0 ? (
        <EmptyState title={PEOPLE_COPY.noHolders} />
      ) : (
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>Person</TableHead>
              <TableHead>Membership</TableHead>
              <TableHead>
                <span className="sr-only">{PEOPLE_COPY.revoke}</span>
              </TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {view.holders.map((row) => (
              <HolderRow key={row.grantId} row={row} orgPrn={view.orgPrn} canRevoke={view.flags.canRevoke} revoke={actions.revoke} />
            ))}
          </TableBody>
        </Table>
      )}
      {view.flags.grantsTruncated ? (
        <p data-testid="people-grants-truncated" className="text-muted-foreground text-sm">
          {PEOPLE_COPY.grantsTruncated}
        </p>
      ) : null}
      {view.flags.membersTruncated ? (
        <p data-testid="people-members-truncated" className="text-muted-foreground text-sm">
          {PEOPLE_COPY.membersTruncated}
        </p>
      ) : null}
      {view.candidates.length === 0 ? (
        <p className="text-muted-foreground text-sm">{PEOPLE_COPY.noCandidates}</p>
      ) : view.flags.canGrant && hydrated ? (
        // Keyed by the candidate set: after a grant the page revalidates, the chosen person leaves the
        // set, and a fresh form starts with no choice.
        <GrantForm key={view.candidates.map((candidate) => candidate.principalPrn).join(' ')} orgPrn={view.orgPrn} candidates={view.candidates} grant={actions.grant} />
      ) : null}
    </div>
  );
}
```

Create `ts/apps/gateway-console/app/(console)/people-model-access/block.tsx`:

```tsx
// SPDX-License-Identifier: Apache-2.0
//
// The "Model access for people" section of the organization page (SMA-676 spec § 4.4, D11). A plain
// async FUNCTION awaited by the page, like serviceAccountsBlock: its error branch awaits
// SectionError, and a page must return a fully resolved tree. A denial or an error shows only in this
// section; the page stays 200 (§ 5.1 step 2).
import type { ReactElement } from 'react';
import { PeopleModelAccessSection } from '../../_components/people-model-access-section';
import { SectionError } from '../../_components/section-error';
import { grantModelAccessAction, revokeModelAccessAction } from './actions';
import { PEOPLE_COPY, type PeopleModelAccessView } from './view';

export async function peopleModelAccessBlock({ view }: { readonly view: PeopleModelAccessView }): Promise<ReactElement> {
  let body: ReactElement;
  if (view.kind === 'disabled') {
    body = (
      <p data-testid="people-model-access-disabled" className="text-muted-foreground text-sm">
        {PEOPLE_COPY.disabled}
      </p>
    );
  } else if (view.kind === 'denied') {
    body = (
      <p data-testid="people-model-access-denied" className="text-muted-foreground text-sm">
        {PEOPLE_COPY.denied}
      </p>
    );
  } else if (view.kind === 'error') {
    body = await SectionError({ error: view.error });
  } else {
    body = <PeopleModelAccessSection view={view} actions={{ grant: grantModelAccessAction, revoke: revokeModelAccessAction }} />;
  }
  return (
    <section aria-labelledby="people-model-access-heading" data-testid="people-model-access" className="flex flex-col gap-3">
      <h2 id="people-model-access-heading" className="text-lg font-semibold">
        {PEOPLE_COPY.title}
      </h2>
      {body}
    </section>
  );
}
```

- [ ] **Step 4: Wire the org loader and page**

In `orgs/[org]/load.ts`:
- add imports:

```ts
import { lifecycleOf, lifecycleView, type NodeLifecycle } from '../../node-status';
import { loadPeopleModelAccess, type PeopleModelAccessDeps } from '../../people-model-access/load';
import type { PeopleModelAccessView } from '../../people-model-access/view';
```

  (replacing the existing `lifecycleOf` import line);
- in the `OrganizationSettings` ok arm, after `readonly projects: ProjectsView;` add `readonly people: PeopleModelAccessView;`;
- replace line 50 with:

```ts
export type OrganizationSettingsDeps = SectionDeps & PeopleModelAccessDeps & { readonly tenancy: Pick<Tenancy, 'getOrganization' | 'listTeams' | 'listProjects'> };
```

- replace `loadOrganizationSettings`'s body after `if (head.kind !== 'ok') return head;` with:

```ts
  // SMA-676: the people section runs in parallel with the other two. D14: controls need the org and
  // its ancestors active, which is the lifecycle view's 'active'.
  const [section, projects, people] = await Promise.all([
    loadServiceAccountSection(deps, { ownerPrn: head.orgPrn, lifecycle: head.organization.lifecycle, saOffset: params.saOffset, keyOffset: params.keyOffset, sa: params.sa }),
    loadProjects(deps.tenancy, head.orgPrn),
    loadPeopleModelAccess(deps, { orgPrn: head.orgPrn, orgActive: lifecycleView(head.organization.lifecycle) === 'active' }),
  ]);
  return { kind: 'ok', orgId: head.orgId, orgPrn: head.orgPrn, organization: head.organization, section, projects, people };
```

- extend the file header's second paragraph: `SMA-676 adds a third parallel read: the "Model access for people" section (people-model-access/load.ts).`

In `orgs/[org]/page.tsx`, add `import { peopleModelAccessBlock } from '../../people-model-access/block';` and render it between the two existing blocks:

```tsx
      {await serviceAccountsBlock({ view: data.section, ownerKind: 'organization', ownerPrn: data.orgPrn, path })}
      {await peopleModelAccessBlock({ view: data.people })}
      {await projectsBlock({ orgId: data.orgId, projects: data.projects })}
```

- [ ] **Step 5: Run the tests and see them pass**

Run: `pnpm -C ts/apps/gateway-console exec vitest run tests/unit/people-model-access-block.test.tsx tests/integration/org-settings-load.test.ts && moon run gateway-console-ts:test gateway-console-ts:typecheck ts:fmt ts:lint`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add ts/apps/gateway-console
git commit -F - <<'EOF'
feat(ts): show model access for people on the organization page

SMA-676 D11 to D16. The section lists the gateway_user holders with a
Revoke per row and a not-a-member mark, offers the candidates in a
combobox, and shows the disabled, denied, error, read-only and
truncation states. It renders below the service accounts.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
```

---

## Task 12: Tests and documents that change on purpose (§7.4, §11)

**Files:**
- Modify: `ts/apps/gateway-console/tests/e2e/call-count.spec.ts:24-45`
- Modify: `ts/apps/gateway-console/tests/integration/service-account-commands.test.ts:143-156`
- Modify: `ts/apps/gateway-console/tests/unit/actions.test.ts:217`
- Modify: `ts/apps/gateway-console/app/(console)/service-accounts/revalidation.ts:24`
- Modify: `ts/apps/gateway-console/app/_components/service-account-frame.tsx:51`
- Modify: `ts/apps/gateway-console/app/_components/playground.tsx:18`
- Modify: `ts/apps/gateway-console/README.md:81-82` (the SMA-635 "out of band" bullet), `:89` ("Stop model calls" reason)

**Interfaces:**
- Consumes: the page of Task 11.
- Produces: tests and comments only.

- [ ] **Step 1: Derive the new R19/R20 formula BEFORE any measurement (SMA-636 §7.3)**

One default-world render of `/gateway/orgs/<org>` gains, from `loadPeopleModelAccess` (Task 9), with `iam.authz.cedar` present in `DEFAULT_IAM_DESCRIPTOR`:
- `authz.listRoleGrants` +1: one page of user grants at the org. The default world answers fewer than 200 rows, so the loop stops after page 1 and makes no probe.
- `tenancy.listMemberships` +1: one page of user members, same reason.
- `authz.isAuthorized` +1: `mayI('RevokeRole', orgPrn)`. `mayI('GrantRole', orgPrn)` adds NOTHING: the service-accounts section asks the same (action, resource) pair (`service-accounts/load.ts:128`), and `createMayI` memoizes per request on `action\0resource` (`authorize.ts`).

R20 derives from `formula()`, so it needs no separate edit.

- [ ] **Step 2: Apply the formula**

In `call-count.spec.ts`, replace `formula()` with:

```ts
/** § 7.3's table for S scopes and T teams, with no `sa` parameter. SMA-676 adds the people section. */
function formula(): Record<string, number> {
  return {
    'authn.whoAmI': 1, // the session principal, memoized per request
    'authz.listRoleGrants': 2, // myScopes(), with iam.authz.cedar present; and the people section's user grants at the org (SMA-676): one short page, no probe
    'tenancy.getOrganization': 1 + SCOPES.organization, // the page, plus myScopes()'s label of the org scope
    'tenancy.getTeam': SCOPES.team, // myScopes() labels
    'tenancy.getProject': SCOPES.project, // myScopes() labels
    'authz.isAuthorized': 6, // mayI(), six distinct questions: the five of SMA-636, and RevokeRole at the org (SMA-676); GrantRole at the org is memoized
    'serviceAccounts.listServiceAccounts': 1, // the section
    'tenancy.listTeams': 1, // the Projects list
    'tenancy.listProjects': TEAMS, // the Projects list, one per shown team
    'tenancy.listMemberships': 1, // the people section's user members of the org (SMA-676): one short page, no probe
  };
}
```

- [ ] **Step 3: Update the duplicate-grant tests (D9)**

In `service-account-commands.test.ts`, replace the test at lines 143-156 with:

```ts
  it('treats a second grant as a success: IAM returns the existing grant (SMA-676 D9)', async () => {
    iam.setHandlers({
      'serviceAccounts.getServiceAccount': () => storedAccount(OWNER),
      'authz.grantRole': (req) => ({ grant: { id: 'g-existing', principalPrn: req.principalPrn, roleKey: req.roleKey, scopePrn: req.scopePrn } }),
    });
    const clients = clientsFor(iam);

    expect(await allowModelCalls({ serviceAccounts: clients.serviceAccounts, authz: clients.authz }, { saPrn: SA })).toEqual({ ok: true });
  });

  it('returns an internal IAM failure as generic', async () => {
    iam.setHandlers({
      'serviceAccounts.getServiceAccount': () => storedAccount(OWNER),
      'authz.grantRole': () => {
        throw new ConnectError('internal failure', Code.Internal);
      },
    });
    const clients = clientsFor(iam);

    const result = await allowModelCalls({ serviceAccounts: clients.serviceAccounts, authz: clients.authz }, { saPrn: SA });

    expect(result.ok).toBe(false);
    if (result.ok) throw new Error('expected a refusal');
    expect(result.error.presentation).toBe('generic');
  });
```

In `tests/unit/actions.test.ts:217`, change the row to:

```ts
    ['a generic error', new ConnectError('internal failure', Code.Internal), true],
```

`tests/unit/service-account-section.test.tsx:159` needs NO change: it tests the "may already be allowed" copy with a `generic` error and names no duplicate. The copy stays for real `generic` errors (§7.4).

- [ ] **Step 4: Update the comments and the README**

- `service-accounts/revalidation.ts:24` → `/** §§ 5.3, 5.5, 5.6: after every result. A \`generic\` allow can have committed before the answer was lost. */`
- `service-account-frame.tsx:51` → `/** § 5.3: a generic answer to "Allow model calls" can come after IAM committed the grant. Since SMA-676 D9 a second grant is OK, not an error. */`
- `playground.tsx:18` → `/** D10: a person needs gateway_user at the org. An org admin grants it on the organization page (SMA-676). */`
- `README.md`, the SMA-635 "Acceptance criterion 3" bullet: replace the sentences from "The console has no control that grants it to a person" to "…(SMA-635 D10)." with: `An organization admin grants and revokes it for a person in the "Model access for people" section of the organization page (SMA-676).`
- `README.md`, the "Stop model calls" bullet: replace "`RevokeRole` needs a grant id, and only a platform admin can list another principal's grants." with "`RevokeRole` needs a grant id, and the service-account panel does not read the account's grants."

- [ ] **Step 5: Run the affected tests**

Run: `pnpm -C ts/apps/gateway-console exec vitest run tests/integration/service-account-commands.test.ts tests/unit/actions.test.ts tests/unit/service-account-section.test.tsx && moon run gateway-console-ts:typecheck ts:fmt ts:lint`
Expected: PASS. `call-count.spec.ts` is an e2e row; it runs in Task 13's e2e step.

- [ ] **Step 6: Commit**

```bash
git add ts/apps/gateway-console
git commit -F - <<'EOF'
test(ts): update the call count, the duplicate-grant tests and the docs for SMA-676

Section 7.4 and 11. The organization page adds one ListRoleGrants, one
ListMemberships and one IsAuthorized per render, derived before the
measurement. A second grant is now OK. Comments that called a generic
allow a duplicate grant, and the out-of-band grant docs, are updated.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
```

---

## Task 13: The one-identity e2e row (R30) and the e2e world

**Files:**
- Modify: `ts/apps/gateway-console/tests/e2e/support/world.ts:17-22` (imports), `:43-60` (`WorldOptions`), `:95-100` (`userGrants`), `:102-236` (`worldHandlers`)
- Modify: `ts/apps/gateway-console/tests/e2e/support/playground-harness.ts:34`
- Create: `ts/apps/gateway-console/tests/e2e/playground-people-model-access.spec.ts`
- Modify: `ts/apps/gateway-console/tests/unit/e2e-rows.test.ts:1-12` (header), `ROWS`
- Modify: `ts/apps/gateway-console/README.md:81` (row list)

**Interfaces:**
- Consumes: the page (Task 11), the fake IAM refusal (Task 8), `PrincipalKind` (Task 1).
- Produces: `WorldOptions.orgCreator?: boolean`; world handlers `authz.revokeRole`, scope-path `authz.listRoleGrants`, kind-aware `tenancy.listMemberships`, idempotent `authz.grantRole`; row R30.

- [ ] **Step 1: Write the failing row registration and the row**

In `tests/unit/e2e-rows.test.ts`, add `'R30',` after `'R29',` and append to the header comment: `// SMA-676 § 7.3 adds R30 (one identity grants model access to themself, in the playground project).`

Create `ts/apps/gateway-console/tests/e2e/playground-people-model-access.spec.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// SMA-676 spec § 7.3: ONE identity. In the `orgCreator` world the signed-in admin created the
// organization: it holds org_admin there and is not a member, so the section offers it as a
// candidate (D12). The admin grants model access to themself, the playground answers, the admin
// revokes, and the playground refuses. The file name starts with `playground` so the playground
// project (the real gateway binary) runs it. The fake IAM is not Cedar (playground-authz.spec.ts:3):
// this row proves the wiring; rs/crates/services/paigasus-iam/tests/authz_people_model_access.rs
// proves the decision.
import type { Page } from '@playwright/test';
import { signIn, waitForHydration } from './support/login';
import { expect, test } from './support/playground-harness';
import { ORG_ID, PRINCIPAL_PRN } from './support/world';

const ORG_PATH = `/gateway/orgs/${ORG_ID}`;
const PLAYGROUND = `${ORG_PATH}/playground`;
// app/_components/playground.tsx MISSING_ROLE_TEXT. Copied, not imported: that module is a React
// client component, and this file runs under plain Playwright.
const MISSING_ROLE_TEXT = 'You need the gateway_user role on this organization. Ask an organization admin to grant it.';

async function send(page: Page): Promise<void> {
  await page.getByLabel('Model').fill('gpt-e2e');
  await page.getByLabel('Message').fill('hello');
  await page.getByRole('button', { name: 'Send' }).click();
}

test('R30: the org creator grants model access to themself, the playground answers, and after a revoke it refuses (SMA-676 § 7.3)', async ({ page, harness }) => {
  harness.useWorld({ orgCreator: true });
  await signIn(page, harness, ORG_PATH);
  const section = page.getByTestId('people-model-access');
  await expect(section.getByTestId('people-holder-row')).toHaveCount(0);

  await section.getByRole('combobox').click();
  await page.getByRole('option', { name: PRINCIPAL_PRN }).click();
  await section.getByRole('button', { name: 'Grant model access' }).click();
  await expect(section.getByTestId('people-holder-row')).toHaveCount(1);
  await expect(section.getByTestId('people-holder-row')).toHaveAttribute('data-principal', PRINCIPAL_PRN);
  await expect(section.getByTestId('people-not-member')).toHaveCount(1);

  await page.goto(harness.url(PLAYGROUND));
  await waitForHydration(page);
  await send(page);
  await expect(page.getByTestId('assistant-turn').last()).toHaveAttribute('data-status', 'done');

  await page.goto(harness.url(ORG_PATH));
  await waitForHydration(page);
  await section.getByRole('button', { name: 'Revoke' }).click();
  await expect(section.getByTestId('people-holder-row')).toHaveCount(0);

  await page.goto(harness.url(PLAYGROUND));
  await waitForHydration(page);
  await send(page);
  await expect(page.getByTestId('playground-error')).toContainText(MISSING_ROLE_TEXT);
});
```

- [ ] **Step 2: Run the registration test and see it fail**

Run: `pnpm -C ts/apps/gateway-console exec vitest run tests/unit/e2e-rows.test.ts && moon run gateway-console-ts:typecheck`
Expected: `e2e-rows.test.ts` PASS (R30 has exactly one test now). `typecheck` FAIL: `Object literal may only specify known properties, and 'orgCreator' does not exist`.

- [ ] **Step 3: Implement the world**

In `support/world.ts`, change the SDK import to `import { ApiKeyStatus, NodeStatus, PrincipalKind } from '@paigasus/sdk/iam/types';`. In `WorldOptions` add:

```ts
  /**
   * SMA-676 § 7.3: the user CREATED the organization. It holds org_admin at ORG_PRN and is not an
   * org member (§ 1.1 fact 6), and IsAuthorized(InvokeModel) about the user answers from the
   * recorded gateway_user grants, as the service-account branch does. Off by default, so R24 and
   * R25 keep a user whose InvokeModel is allowed with no grant.
   */
  readonly orgCreator?: boolean;
```

Replace `userGrants` with:

```ts
/** The signed-in user's OWN role grants (myScopes() lists them). */
function userGrants(projectAdmin: boolean, withScopes: boolean, orgCreator: boolean): Grant[] {
  if (projectAdmin) return [{ id: '0190a1d4-0000-7000-8000-0000000000f6', principalPrn: PRINCIPAL_PRN, roleKey: 'project_admin', scopePrn: PROJECT_PRN }];
  const own: Grant[] = withScopes ? [{ id: '0190a1d4-0000-7000-8000-0000000000f3', principalPrn: PRINCIPAL_PRN, roleKey: 'project_viewer', scopePrn: PROJECT_PRN }] : [];
  if (orgCreator) own.push({ id: '0190a1d4-0000-7000-8000-0000000000f7', principalPrn: PRINCIPAL_PRN, roleKey: 'org_admin', scopePrn: ORG_PRN });
  return own;
}
```

In `worldHandlers`, after `const projectAdmin = …` add `const orgCreator = options.orgCreator === true;`, and add these helpers after `organizationOnly`:

```ts
  /** A principal is a service account when the world made it one; every other principal is a user. */
  const isServiceAccount = (prn: string): boolean => accounts.some((account) => account.prn === prn);
  const ofKind = (prn: string, kind: PrincipalKind): boolean => kind === PrincipalKind.UNSPECIFIED || (kind === PrincipalKind.SERVICE_ACCOUNT) === isServiceAccount(prn);
  const grantsAt = (): Grant[] => [...userGrants(projectAdmin, withScopes, orgCreator), ...grants];
```

Replace the four handlers with:

```ts
    // The bare principal request (myScopes) keeps its old answer. A request with a scope, a role or a
    // kind is IAM's query path (SMA-676 D2, D6): exact scope match, principal order, IAM's paging.
    'authz.listRoleGrants': (req) => {
      const query = req.scopePrn !== '' || req.roleKey !== '' || req.principalKind !== PrincipalKind.UNSPECIFIED;
      if (!query) return { grants: userGrants(projectAdmin, withScopes, orgCreator) };
      const hits = grantsAt()
        .filter((grant) => (req.principalPrn === '' || grant.principalPrn === req.principalPrn) && (req.scopePrn === '' || grant.scopePrn === req.scopePrn) && (req.roleKey === '' || grant.roleKey === req.roleKey) && ofKind(grant.principalPrn, req.principalKind))
        .sort((a, b) => (a.principalPrn < b.principalPrn ? -1 : a.principalPrn > b.principalPrn ? 1 : a.id < b.id ? -1 : 1));
      return { grants: page(hits, req) };
    },
    'authz.isAuthorized': (req) => {
      if (req.principalPrn === PRINCIPAL_PRN) {
        // SMA-676 § 7.3: the org creator's InvokeModel answers from the recorded grants.
        if (orgCreator && req.action === 'InvokeModel') {
          const allowed = grants.some((grant) => grant.principalPrn === PRINCIPAL_PRN && grant.roleKey === 'gateway_user' && covers(grant.scopePrn, req.resourcePrn));
          return { allowed, determiningPolicies: [], reason: '' };
        }
        return { allowed: allow === null || allow.has(req.action), determiningPolicies: [], reason: '' };
      }
      // About a service account: only InvokeModel, and only from a recorded gateway_user grant.
      const allowed = req.action === 'InvokeModel' && grants.some((grant) => grant.principalPrn === req.principalPrn && grant.roleKey === 'gateway_user' && covers(grant.scopePrn, req.resourcePrn));
      return { allowed, determiningPolicies: [], reason: '' };
    },
    'authz.grantRole': (req) => {
      if (grantFailuresLeft > 0) {
        grantFailuresLeft -= 1;
        throw new ConnectError('the e2e world fails this grant', Code.Internal);
      }
      // SMA-676 D9: a second grant of the same role at the same scope returns the existing grant.
      const existing = grants.find((grant) => grant.principalPrn === req.principalPrn && grant.roleKey === req.roleKey && grant.scopePrn === req.scopePrn);
      if (existing !== undefined) return { grant: existing };
      const grant: Grant = { id: randomUUID(), principalPrn: req.principalPrn, roleKey: req.roleKey, scopePrn: req.scopePrn };
      grants.push(grant);
      return { grant };
    },
    'authz.revokeRole': (req) => {
      const index = grants.findIndex((grant) => grant.id === req.id);
      if (index === -1) throw notFound();
      grants.splice(index, 1);
      return {};
    },
```

Replace `'tenancy.listMemberships'` with:

```ts
    // `filter` is a oneof: its type includes `{ case: undefined }`, so narrow instead of annotating.
    // SMA-676: the org creator is no member of the org; the kind filter keeps only users.
    'tenancy.listMemberships': (req) => {
      const nodePrn = req.filter.case === 'nodePrn' ? req.filter.value : '';
      if (orgCreator && nodePrn === ORG_PRN) return { memberships: [] };
      if (!ofKind(PRINCIPAL_PRN, req.principalKind)) return { memberships: [] };
      return { memberships: [{ id: '0190a1d4-0000-7000-8000-0000000000f4', principalPrn: PRINCIPAL_PRN, nodePrn }] };
    },
```

Update the file header's second paragraph: after "…the main row cannot show 'Can call models: Yes' without a grant." add `A second grant of the same role at the same scope returns the existing grant (SMA-676 D9). With orgCreator, IsAuthorized(InvokeModel) about the user reads the grants too (SMA-676 § 7.3).`

In `support/playground-harness.ts:34`, change to `useWorld(options?: Pick<WorldOptions, 'overrides' | 'allow' | 'orgCreator'>): void;`.

In `README.md:81`, change "then R22–R29 for the playground of SMA-635 (R29, the streaming-off composer, is in the single-zone project))" to "then R22–R29 for the playground of SMA-635 (R29, the streaming-off composer, is in the single-zone project), then R30 for the model-access grant of SMA-676, in the playground project)".

- [ ] **Step 4: Run the unit gates and the e2e tier**

Run:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run gateway-console-ts:test gateway-console-ts:typecheck ts:fmt ts:lint
moon run gateway-console-ts:test-e2e
```

Expected: PASS, including R19, R20 (Task 12's formula), R24, R25, R30. `gateway-console-ts:test-e2e` needs Docker and builds the gateway binary; if Docker is not reachable it fails loudly, and the implementer reports that and does not claim the e2e result.

- [ ] **Step 5: Run the full gate graph once**

Run the command between the `ci-targets` markers in the root `CLAUDE.md` (`moon ci :build :test … --base origin/main --include-relations`). On this development Mac, read the "This development Mac only" section of the root `CLAUDE.md` first: pick one bash for the run, then re-run the gates that need the other bash directly. Report each gate's verdict; do not re-run a failing gate away (Step 0 of the diagnosis procedure).

- [ ] **Step 6: Commit**

```bash
git add ts/apps/gateway-console
git commit -F - <<'EOF'
test(ts): add the one-identity model-access e2e row R30

SMA-676 section 7.3. In the orgCreator world the admin grants model
access to themself, the playground answers, the admin revokes, and the
playground refuses. The e2e world lists grants at a scope, revokes, grants
idempotently, and filters members by kind.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
```

---

## Self-review

### Spec coverage

| Spec section / decision | Task(s) |
|---|---|
| §1.1 facts 1-3 (no scope query, org_admin refused, revoke needs an id) | 2, 3, 4, 10 |
| §1.1 facts 4-5 (kind not in a PRN; SA members and SA grants at the org) | 1, 3, 6, 9 |
| §1.1 fact 6 (creator is no member) | 9 (join test), 13 (`orgCreator`) |
| D1 (one PR, IAM + console) | all |
| D2 (`scope_prn`, `role_key`, `principal_kind` on `ListRoleGrants`) | 1, 2, 3, 4 |
| D3 (principal or scope required, field name) | 2, 4, 7 |
| D4 (a/b/c authorization, before any read) | 4, 7 (real Cedar rows + Docker) |
| D5 (exact match, Root, forged slot) | 2, 3, 7 |
| D6 (paging on the query path only, order) | 3, 4, 7, 8 (fake) — see deviation 5 |
| D7 (enum, refuse unknown, HTTP strings) | 1, 4, 6, 7 — see deviation 1 |
| D8 (`ListMemberships` kind) | 1, 2, 6, 8 |
| D9 (idempotent grant, race, seeder) | 2, 3, 5, 7, 12, 13 |
| D10 (HTTP parity) | 4, 6, 7 |
| D11 (section on the org page, below SAs) | 11 |
| D12 (candidates) | 9, 11, 13 |
| D13 (holders, not-a-member mark, hidden when truncated) | 9, 11 |
| D14 (`mayI` + active org) | 8, 9, 11 |
| D15 (no call when cedar is off) | 9, 11 |
| D16 (PRN, no names) | 11 |
| §3 facts (org_admin actions, fail-closed missing node) | 7 |
| §4.1 contract | 1 |
| §4.2 core (`RoleGrantFilter`, `RoleGrantQuery`, `DuplicateGrant`) | 2 |
| §4.3 service, pg, fakes, bootstrap, adapters | 3, 4, 5, 6 |
| §4.4 console files, join, paging, shared `GATEWAY_ROLE`, `RevokeRole` | 8, 9, 10, 11 |
| §5.1-§5.3 flows | 9, 10, 11 |
| §6 security (gate order, bounded revoke, server-constant role) | 4, 5, 7, 10 |
| §7.1 Rust tests | 2, 3, 4, 5, 6, 7 |
| §7.2 TS unit/integration, fake limits, dev world | 8, 9, 10, 11 |
| §7.3 e2e one identity | 13 |
| §7.4 tests that change on purpose | 3 (Rust asserts), 8 (action names), 12, 13 (world) — see deviation 2 |
| §8 follow-ups | not in scope |
| §9 limits (1000 rows, team grants, platform_admin, ~31 s) | 9 (cap); no code for the others |
| §10 rollout / version skew | 10 (filter-ignoring IAM test) — see deviation 4 |
| §11 docs, no metric, no audit | 4 (roles.rs docs), 3 (pg doc), 12 (README, playground) |

### Type consistency check

- `RoleGrantFilter::new(Option<PrincipalId>, Option<GrantScope>, Option<String>, Option<PrincipalKind>) -> Option<Self>` — used identically in Task 2 tests, Task 3 (Postgres test, fake test), Task 4 (`list`), Task 5 (`existing_grant`).
- `RoleGrantQuery::find(&self, &RoleGrantFilter, u64, u64) -> Result<Vec<RoleGrant>, AuthzError>` — implemented in Task 3 (Pg, fake), consumed in Tasks 4 and 5 through `RoleServiceDeps.query: Arc<dyn RoleGrantQuery>`.
- `MembershipKindQuery::list_of_kind(&self, &MembershipAxis, PrincipalKind, u64, u64) -> Result<Vec<MembershipRecord>, RepositoryError>` — declared in Task 2, implemented and consumed in Task 6 through `MembershipServiceDeps.kinds`.
- `PrincipalKindFilter { Any, Only(PrincipalKind), Unknown }` with `from_query(Option<&str>)` and `resolve(self) -> Result<Option<PrincipalKind>, TenancyError>` — Task 4; consumed by `convert::principal_kind_filter(i32)` (Task 4), `RoleService::list` (Task 4), `MembershipService::list` (Task 6), both HTTP handlers.
- `ListRoleGrantsInput { principal_prn, scope_prn, role_key: Option<String>, principal_kind: PrincipalKindFilter, limit, offset: Option<i64> }` — built by gRPC and HTTP (Task 4) and by the Task 4 test helpers.
- `TenancyError::InvalidPrincipalKind(&'static str)` — Task 1; the payload is always `"principal_kind"`.
- TS `PeopleModelAccessView` / `PeopleModelAccessOk` / `HolderRowView.member: boolean | null` — defined in Task 9 `view.ts`, built by Task 9 `load.ts`, rendered by Task 11, asserted in Tasks 9 and 11.
- TS `PeopleModelAccessDeps` — Task 9; `OrganizationSettingsDeps` intersects it in Task 11, and `org-settings-load.test.ts`'s `deps()` already passes full `clients.authz` and `clients.tenancy`.
- TS actions `grantModelAccessAction` / `revokeModelAccessAction` — Task 10; named in `actions-structure.test.ts` `EXPECTED` (Task 10), passed by `block.tsx` (Task 11), mocked in the Task 11 unit test.
- `GATEWAY_ROLE` — one definition in `gateway-role.ts` (Task 9); imported by both command modules and by `load.ts`.
