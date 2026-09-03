# SMA-584 `CreateUser` Authorization Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `POST /v1/users` and `UserService.CreateUser` both require `Action::CreateUser` at `Root`, so minting a user principal is a platform-admin operation on both transports instead of an unauthorized, bearer-only one.

**Architecture:** Add one `Action` variant to the Cedar catalog in `paigasus-iam-core`, then check it in each of the two adapters with the same `if enforce_tenancy { authorize.check(actor, Action::CreateUser, &root_prn()) }` guard every other enforced route already uses. The application use case `CreateUser::execute` is deliberately untouched. Tests are built around two properties: no single-transport change can pass CI (P1), and the binding to `CreateUser` *specifically* is falsifiable (P2).

**Tech Stack:** Rust (edition 2024, rust-version 1.95), axum, tonic, `cedar-policy`, SeaORM, `cargo nextest`, Moon 2.3.2, buf.

**Spec:** `docs/superpowers/specs/2026-08-23-sma-584-create-user-authorization-design.md`

## Global Constraints

- Every source file opens with `// SPDX-License-Identifier: Apache-2.0`.
- Rust crates are edition 2024 + rust-version 1.95.
- Conventional commits with a workspace scope: `feat(rs):`, `docs(rs):`, `fix(contracts):`.
- **Commit message trap:** never let a body line begin `word:` — commitlint reads it as a trailer and fails `footer-leading-blank`. Write "owner/repo PR NNN", not `#NNN`. Subject starts lowercase, ≤100 chars.
- **Bash PATH:** prefix every tooling command with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` so `moon`/`buf`/`cargo-nextest` resolve to the repo-pinned versions.
- Work in the worktree `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-584`, branch `feature/sma-584-iam-v1users-has-no-per-action-authorization-on-either`. Never `cd` to the main checkout.
- `cargo nextest` exits non-zero on a package with no tests — use `--no-tests=pass` where relevant.
- `paigasus-iam`'s integration suites need a reachable Docker daemon. A **filtered** run (`-E 'test(foo)'`, `--test http_users`) does not include the `docker_preflight` canary, so filtered runs must set `PAIGASUS_REQUIRE_DOCKER=1` or a Docker outage silently reports PASS having run nothing.
- **`repo:error-code-single-site` trap:** the gate scans `rs/crates/**/src/**/*.rs` and matches a quoted registry code **anywhere in the file, comments included**. In any doc comment, write `` `forbidden` `` with backticks — never `"forbidden"`.
- Do not run `git stash`; use a WIP commit if you must set work aside.

---

### Task 1: Cedar catalog — `Action::CreateUser`

Adds the action to the embedded schema and the Rust catalog, takes the starter-policy revision consequence, and pins D1/D4 (that only a Root-scoped grant can satisfy it) with executable table cases.

**Files:**
- Modify: `rs/crates/libs/paigasus-iam-core/src/authz/schema.rs`
- Modify: `rs/crates/libs/paigasus-iam-core/src/authz/action.rs`
- Modify: `rs/crates/libs/paigasus-iam-core/src/authz/roles.rs`
- Test: all three files' inline `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: nothing — this is the first task.
- Produces: `paigasus_iam_core::Action::CreateUser` (a unit variant of the existing `pub enum Action`), whose `as_wire()` is `"CreateUser"` and whose `is_write()` is `true`. Tasks 2, 3 and 4 import it as `use paigasus_iam_core::Action;` and use `Action::CreateUser`.

- [ ] **Step 1: Write the failing tests**

In `rs/crates/libs/paigasus-iam-core/src/authz/schema.rs`, add to `mod tests` (after `the_retire_action_validates_against_the_embedded_schema`):

```rust
    /// SMA-584: the twin of the `RetireSystemPolicy` test above. `SCHEMA_SRC`'s action list is
    /// hand-maintained, so a `CreateUser` present in `Action::ALL` but missing here makes the
    /// generated `forbid-archived-writes` source fail validation.
    #[test]
    fn the_create_user_action_validates_against_the_embedded_schema() {
        assert!(validate_policy(r#"permit(principal, action == Pgs::Iam::Action::"CreateUser", resource);"#).is_ok());
    }
```

In `rs/crates/libs/paigasus-iam-core/src/authz/roles.rs`, add to `mod tests` (after `the_retire_action_is_in_the_generated_forbid_source`):

```rust
    /// SMA-584: `CreateUser` is a non-restore write, so it must reach the generated forbid
    /// list — the reason `STARTER_POLICY_REVISION` has to move. A hand-updated content hash
    /// with the action missing from `Action::ALL` would otherwise look green.
    #[test]
    fn the_create_user_action_is_in_the_generated_forbid_source() {
        assert!(
            forbid_archived_writes_source().contains(r#"Pgs::Iam::Action::"CreateUser""#),
            "CreateUser is a write action, so it must appear in forbid-archived-writes"
        );
    }
```

In the same file, inside `fn starter_policy_table()`'s `let cases = vec![...]`, append these two cases just before the closing `];`. They make spec decisions D1 and D4 executable — without them, "only `platform_admin` can create users" is prose backed by nothing:

```rust
            Case {
                name: "platform_admin at Root allows CreateUser at Root itself",
                grants: vec![grant(90, &uni.principal, "platform_admin", GrantScope::Root)],
                action: Action::CreateUser,
                resource: root_prn(),
                expect: Effect::Allow,
            },
            Case {
                name: "org_admin denies CreateUser at Root (Root is the ancestor, never a descendant)",
                grants: vec![grant(91, &uni.principal, "org_admin", GrantScope::Node(TenancyNodeRef::Organization(uni.org_o.clone())))],
                action: Action::CreateUser,
                resource: root_prn(),
                expect: Effect::Deny,
            },
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam-core
```

Expected: **compile failure**, `error[E0599]: no variant or associated item named 'CreateUser' found for enum 'Action'`. In Rust a not-yet-existing variant fails at compile time — that is this task's red state. Do not proceed until you have seen it.

- [ ] **Step 3: Add the action to the embedded Cedar schema**

In `rs/crates/libs/paigasus-iam-core/src/authz/schema.rs`, in `SCHEMA_SRC`, change the tail of the `action` declaration from:

```
         ReplayOutboxDeadLetter, DiscardOutboxDeadLetter, RetireSystemPolicy, InvokeModel
```

to:

```
         ReplayOutboxDeadLetter, DiscardOutboxDeadLetter, RetireSystemPolicy, InvokeModel,
         CreateUser
```

- [ ] **Step 4: Add the variant to the Rust catalog**

In `rs/crates/libs/paigasus-iam-core/src/authz/action.rs`, four edits — all four are required, and `Action::ALL` must stay in schema-declaration order, so `CreateUser` goes **last** everywhere.

(a) The enum — after `InvokeModel,`:

```rust
    /// Mint a user principal (SMA-584). Authorized at `Root` only: a user principal has no
    /// owner tenancy node (users attach to orgs/teams later, via memberships), so there is no
    /// narrower resource to scope against. Checked in `adapters::http::users` and
    /// `adapters::grpc::users`.
    CreateUser,
```

(b) `ALL` — after `Action::InvokeModel,`:

```rust
        Action::CreateUser,
```

(c) `as_wire` — after the `Action::InvokeModel => "InvokeModel",` arm:

```rust
            Action::CreateUser => "CreateUser",
```

(d) `is_write` — in the `=> true` group, change `| Action::InvokeModel => true,` to:

```rust
            | Action::InvokeModel
            | Action::CreateUser => true,
```

- [ ] **Step 5: Update the exhaustive-match test and its count**

Still in `action.rs`, in `fn all_covers_every_variant`, add `CreateUser` to `assert_in_all`'s inner `match` (rustc will refuse to compile without it) — change `| Action::InvokeModel => {}` to:

```rust
                | Action::InvokeModel
                | Action::CreateUser => {}
```

Then update the length assertion. Both the number **and** its explanatory message must move, or the message becomes arithmetically wrong:

```rust
        assert_eq!(
            Action::ALL.len(),
            41,
            "27 pre-existing + 7 M4 + 1 audit + 1 invoke-model + 3 outbox dead-letter + 1 SMA-481 RetireSystemPolicy + 1 SMA-584 CreateUser"
        );
```

- [ ] **Step 6: Run the tests to see the content-hash failure**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam-core
```

Expected: it now compiles, the new schema/forbid/table tests **pass**, and exactly one test fails: `starter_policy_content_is_pinned_to_the_declared_revision`. That is correct and expected — `CreateUser` is a write, so it joined the derived `forbid-archived-writes` action list and changed the starter policy set's content. **Copy the 64-hex digest the failure message prints**; it is the value for the next step.

- [ ] **Step 7: Bump the revision and re-pin the hash**

In `rs/crates/libs/paigasus-iam-core/src/authz/roles.rs`:

Change the `STARTER_POLICY_REVISION` doc tail and value. Replace:

```rust
/// `2`: SMA-481 added the `RetireSystemPolicy` action, which — being a non-restore write —
/// joins `forbid-archived-writes`'s generated action list and so changes its `source`.
pub const STARTER_POLICY_REVISION: u32 = 2;
```

with:

```rust
/// `2`: SMA-481 added the `RetireSystemPolicy` action, which — being a non-restore write —
/// joins `forbid-archived-writes`'s generated action list and so changes its `source`.
///
/// `3`: SMA-584 added `CreateUser` for the same reason. The forbid can never actually bite on
/// it (`entity Root;` declares no attributes, so `resource has effective_status` is
/// unsatisfiable at `Root`), but the action list is *derived*, not hand-written, so the
/// content moves and every deployed database now holds an older set.
pub const STARTER_POLICY_REVISION: u32 = 3;
```

Then replace `EXPECTED_STARTER_CONTENT_HASH`'s value with the digest from Step 6:

```rust
const EXPECTED_STARTER_CONTENT_HASH: &str = "<paste the 64-hex digest printed by Step 6>";
```

- [ ] **Step 8: Run the full crate test suite**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam-core && cargo clippy -p paigasus-iam-core --all-targets -- -D warnings && cargo fmt --check
```

Expected: PASS, no warnings. `every_starter_policy_passes_schema_validation` passing is what proves Step 3 and Step 4 agree — an action in `ALL` but missing from `SCHEMA_SRC` fails there.

- [ ] **Step 9: Commit**

```bash
git add rs/crates/libs/paigasus-iam-core/src/authz/schema.rs \
        rs/crates/libs/paigasus-iam-core/src/authz/action.rs \
        rs/crates/libs/paigasus-iam-core/src/authz/roles.rs
git commit -m "feat(rs): add Action::CreateUser to the Cedar catalog (SMA-584)

Add the action to the embedded schema and the Rust catalog. Being a
non-restore write it joins the derived forbid-archived-writes action
list, so the starter policy content moves - hence the revision bump to
3 and the re-pinned content hash.

Two starter_policy_table cases pin the decision that matters: a
Root-scoped grant allows it, an Organization-scoped one does not.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01Cb6J6fQxMweNZ3Q3G3wuf2"
```

---

### Task 2: HTTP transport enforcement

Adds the guard to `POST /v1/users`, repairs the three `http_memberships.rs` tests it breaks, and adds the dedicated HTTP authorization test the issue's AC-2 asks for.

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/http/users.rs`
- Modify: `rs/crates/services/paigasus-iam/tests/http_memberships.rs:132,155,170`
- Create: `rs/crates/services/paigasus-iam/tests/http_users.rs`

**Interfaces:**
- Consumes: `paigasus_iam_core::Action::CreateUser` (Task 1).
- Produces: `POST /v1/users` returns `403` with body `error.code == "forbidden"` for a principal lacking `CreateUser`@`Root`, `201` otherwise. Task 4 adds a second test to `tests/http_users.rs` and relies on this file existing with its `mod support;` declaration.

- [ ] **Step 1: Write the failing test**

Create `rs/crates/services/paigasus-iam/tests/http_users.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0

//! End-to-end HTTP coverage for `POST /v1/users`' AUTHORIZATION (SMA-584). The sibling of
//! `tests/grpc_users.rs`'s `create_user_requires_platform_admin`: this file exists because
//! `http_memberships.rs` authenticates as `platform_admin` throughout, so it cannot express
//! the denied case and would stay green if authorization were added to gRPC only.
//!
//! Drives the real `router(AppState::new(db, &cfg))` via `tower::ServiceExt::oneshot` — no
//! listening socket — against an ephemeral Postgres (Docker; see `tests/support/mod.rs`).

mod support;

use axum::http::StatusCode;
use paigasus_iam::adapters::persistence::entities::principal;
use paigasus_kernel::Prn;
use sea_orm::{EntityTrait, PaginatorTrait};
use serde_json::json;
use support::{app_with_state, provision, provision_platform_admin, send};

/// The three-outcome pin for `POST /v1/users` (SMA-584 AC-1/AC-2). A mutation that removes the
/// `Action::CreateUser` guard from `adapters::http::users` fails the middle row; a mutation
/// that puts `/v1/users` on an unauthenticated router fails the first.
#[tokio::test]
async fn create_user_requires_platform_admin_over_http() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    // Cloned BEFORE `app_with_state` consumes `db`, so the row-count assertion below can query
    // the same database independently (mirrors `tests/grpc_users.rs`'s identical setup).
    let count_db = db.clone();
    let (app, state, idp) = app_with_state(db).await;

    // An ORDINARY principal: JIT-provisioned, no grant of any kind.
    let plain_token = idp.bearer("http-plain-tester", Some("http-plain@example.com"), "paigasus", 3600);
    provision(&state, &plain_token).await;

    // A platform_admin, seeded at Root.
    let admin_token = idp.bearer("http-admin-tester", Some("http-admin@example.com"), "paigasus", 3600);
    provision_platform_admin(&state, &admin_token).await;

    // 1. No bearer at all -> 401. `/v1/users` sits on the bearer-gated `protected` sub-router.
    let (status, body) = send(&app, "POST", "/v1/users", Some(json!({"email": "no-bearer@example.com", "display_name": "No Bearer"})), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");

    // Baseline taken AFTER both principals are provisioned, so the only thing that could move
    // it is the denied create below.
    let before = principal::Entity::find().count(&count_db).await.unwrap();

    // 2. An ordinary, non-admin principal -> 403 `forbidden`.
    let (status, body) = send(
        &app,
        "POST",
        "/v1/users",
        Some(json!({"email": "denied@example.com", "display_name": "Denied"})),
        Some(plain_token.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"]["code"], "forbidden", "{body}");

    // The check must run BEFORE the use case: a denied create must not mint a principal row.
    let after = principal::Entity::find().count(&count_db).await.unwrap();
    assert_eq!(after, before, "a denied create must not mint a principal row");

    // 3. platform_admin -> 201, and the returned PRN parses.
    let (status, body) = send(
        &app,
        "POST",
        "/v1/users",
        Some(json!({"email": "allowed@example.com", "display_name": "Allowed"})),
        Some(admin_token.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let prn = body["principal_prn"].as_str().expect("principal_prn");
    Prn::parse(prn).unwrap_or_else(|e| panic!("unexpected principal prn {prn}: {e}"));
}
```

- [ ] **Step 2: Run it to verify it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test http_users
```

Expected: FAIL at the 403 assertion — `assertion failed: left: 201, right: 403`. The endpoint is still unauthorized, so the ordinary principal's create succeeds. `PAIGASUS_REQUIRE_DOCKER=1` is mandatory here: this is a filtered run, so the `docker_preflight` canary is excluded and a missing daemon would otherwise report a silent PASS.

- [ ] **Step 3: Add the guard to the HTTP adapter**

In `rs/crates/services/paigasus-iam/src/adapters/http/users.rs`, replace the module doc's second paragraph. Delete:

```rust
//! **No authorization check beyond the bearer, deliberately (design D0).**
//! `CreateUser::execute` takes no `actor` parameter, this handler extracts no `AuthContext`,
//! and there is no `Action::CreateUser` in the Cedar action catalog — so any bearer-authenticated
//! caller may create a user principal. `grpc::users`'s `UserGrpc::create_user` is the gRPC
//! mirror of this exact posture (see its module doc for the three-part justification); tightening
//! authorization here without tightening it there (or vice versa) breaks the parity that is this
//! surface's whole acceptance criterion, so treat the two as one decision, not two.
```

and put in its place:

```rust
//! **Authorized (SMA-584):** the handler checks `Action::CreateUser` at `root_prn()`, gated by
//! `AppState.enforce_tenancy` — the same shape `organizations.rs`'s `create_org` uses for
//! `CreateOrganization`. `Root` is the top of the Cedar hierarchy and `resource in ?resource`
//! is descendant-or-self, so no `Organization`/`Team`/`Project`-scoped grant can satisfy it:
//! under the starter role set this is `platform_admin` only. (An operator-authored STATIC
//! policy via `PutPolicy` can still permit it narrowly — that is the intended escape hatch,
//! not a hole.)
//!
//! The check runs BEFORE `to_command`/`execute`, so a denied caller never reaches email
//! validation or the unit of work and cannot use the endpoint as an email-existence oracle.
//! `grpc::users`'s `UserGrpc::create_user` mirrors this exactly; the two transports are ONE
//! decision, not two, and `tests/http_users.rs` + `tests/grpc_users.rs` are written so that
//! changing either transport alone reds CI.
```

Then change the imports and the handler. Replace:

```rust
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};

use super::AppState;
use super::dto::{CreateUserBody, CreateUserResponse};
use super::error::ApiError;
use crate::application::create_user::NewUser;
```

with:

```rust
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Extension, Json, Router};
use paigasus_iam_core::Action;
use paigasus_iam_core::authz::model::root_prn;

use super::AppState;
use super::dto::{CreateUserBody, CreateUserResponse};
use super::error::ApiError;
use crate::adapters::auth::AuthContext;
use crate::application::create_user::NewUser;
```

Add the `actor_prn` helper immediately above `to_command` (verbatim from `organizations.rs`):

```rust
/// The acting principal's canonical `Prn`, from the bearer-resolved `AuthContext` — mirrors
/// `adapters::http::organizations::actor_prn`.
fn actor_prn(ctx: &AuthContext) -> paigasus_kernel::Prn {
    ctx.principal_id.prn().clone()
}
```

And replace the handler:

```rust
async fn create_user(
    State(s): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Json(b): Json<CreateUserBody>,
) -> Result<(StatusCode, Json<CreateUserResponse>), ApiError> {
    if s.enforce_tenancy {
        s.authorize.check(&actor_prn(&ctx), Action::CreateUser, &root_prn()).await?;
    }
    let cmd = to_command(b);
    let id = s.users.execute(cmd).await?;
    Ok((StatusCode::CREATED, Json(CreateUserResponse { principal_prn: id.canonical() })))
}
```

Note the argument order: axum requires `State` first and the `Json` body extractor **last**; `Extension` goes between them.

- [ ] **Step 4: Run the new test to verify it passes**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test http_users
```

Expected: PASS.

- [ ] **Step 5: Run `http_memberships` to see the breakage this task must repair**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test http_memberships
```

Expected: **three failures** — `list_memberships_requires_exactly_one_filter`, `create_user_rejects_duplicate_email`, `create_user_rejects_invalid_email`. All three build the app with `support::app(db)` and an ungranted JIT bearer, then call `POST /v1/users`; `enforce_tenancy` defaults to `true`, so they now get `403`. This is expected, and repairing it is part of this task — do not skip ahead.

- [ ] **Step 6: Repair the three `http_memberships.rs` tests**

In `rs/crates/services/paigasus-iam/tests/http_memberships.rs`, change the import line:

```rust
use support::{app, app_with_state, provision_platform_admin, send};
```

to:

```rust
use support::{app_with_state, provision_platform_admin, send};
```

Then in each of the three tests replace the two setup lines. In `list_memberships_requires_exactly_one_filter` and `create_user_rejects_duplicate_email` and `create_user_rejects_invalid_email`, change:

```rust
    let (app, idp) = app(db).await;
    let token = idp.bearer("sweep-user", Some("sweep@example.com"), "paigasus", 3600);
```

to:

```rust
    let (app, state, idp) = app_with_state(db).await;
    let token = idp.bearer("sweep-user", Some("sweep@example.com"), "paigasus", 3600);
    // SMA-584: `POST /v1/users` now requires `Action::CreateUser`@`Root`. These tests are about
    // the membership filter / duplicate-email / invalid-email behaviour, not authorization, so
    // they authenticate as a platform_admin to get past the gate — `tests/http_users.rs` owns
    // the authorization cases.
    provision_platform_admin(&state, &token).await;
```

- [ ] **Step 7: Run both suites to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test http_users --test http_memberships
```

Expected: PASS, all tests in both files.

- [ ] **Step 8: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/adapters/http/users.rs \
        rs/crates/services/paigasus-iam/tests/http_memberships.rs \
        rs/crates/services/paigasus-iam/tests/http_users.rs
git commit -m "feat(rs): authorize POST /v1/users with Action::CreateUser (SMA-584)

Check Action::CreateUser at root_prn() before the use case runs, gated
by enforce_tenancy - the same shape create_org uses. A denied caller
never reaches email validation, so the endpoint is not an email-
existence oracle.

Three http_memberships tests used an ungranted bearer to create users
and now authenticate as platform_admin; the new http_users suite owns
the authorization cases.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01Cb6J6fQxMweNZ3Q3G3wuf2"
```

---

### Task 3: gRPC transport enforcement

Mirrors Task 2 on `UserService.CreateUser`, rewrites the four stale doc sites that assert the RPC is unauthorized, and migrates `grpc_users.rs`.

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/grpc/users.rs`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/grpc/authn.rs:126`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/grpc/mod.rs:7-8,79,102`
- Modify: `rs/crates/services/paigasus-iam/tests/grpc_users.rs`

**Interfaces:**
- Consumes: `paigasus_iam_core::Action::CreateUser` (Task 1).
- Produces: `UserService.CreateUser` returns `Code::PermissionDenied` for a principal lacking `CreateUser`@`Root`. Task 4 adds a test to `tests/grpc_users.rs` and reuses its existing `spawn_server`, `channel`, `authed` and `create_user_request` helpers unchanged.

- [ ] **Step 1: Write the failing test**

In `rs/crates/services/paigasus-iam/tests/grpc_users.rs`, replace the whole of
`create_user_requires_a_bearer_but_no_authorization` (its doc comment and body) with:

```rust
/// **Design pin (SMA-584).** `UserService.CreateUser` is bearer-required AND authorized: it
/// checks `Action::CreateUser` at `root_prn()`, gated by `enforce_tenancy`, exactly as
/// `POST /v1/users` does (`adapters::grpc::users` module doc). This test pins all three halves
/// so a future maintainer who changes authorization on ONE transport is forced to consider the
/// other: unauthenticated is rejected (proving `UserService` carries no `is_exempt` allowlist
/// entry), an ordinary non-admin principal is DENIED, and a `platform_admin` succeeds.
/// `tests/http_users.rs` is the HTTP-side twin.
#[tokio::test]
async fn create_user_requires_platform_admin() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let state = AppState::new(db, &support::test_config(&idp)).await.unwrap();

    // An ORDINARY principal: JIT-provisioned, no grant of any kind.
    let plain_token = idp.bearer("grpc-plain-tester", Some("grpc-plain-tester@example.com"), "paigasus", 3600);
    support::provision(&state, &plain_token).await;

    // A platform_admin, seeded at Root.
    let admin_token = idp.bearer("grpc-admin-tester", Some("grpc-admin-tester@example.com"), "paigasus", 3600);
    support::provision_platform_admin(&state, &admin_token).await;

    let (addr, server) = spawn_server(state).await;
    let mut client = UserServiceClient::new(channel(addr).await);

    // No bearer at all -> Unauthenticated: `UserService` is not on `AuthLayer`'s `is_exempt`
    // allowlist (module doc), so this never even reaches the handler.
    let err = client.create_user(create_user_request("no-bearer@example.com")).await.unwrap_err();
    assert_eq!(err.code(), Code::Unauthenticated, "{err:?}");

    // An ordinary, non-admin principal -> PermissionDenied.
    let err = client.create_user(authed(create_user_request("denied@example.com"), &plain_token)).await.unwrap_err();
    assert_eq!(err.code(), Code::PermissionDenied, "{err:?}");

    // platform_admin -> Ok.
    let resp = client.create_user(authed(create_user_request("allowed@example.com"), &admin_token)).await.unwrap().into_inner();
    Prn::parse(&resp.principal_prn).unwrap_or_else(|e| panic!("unexpected principal prn {}: {e}", resp.principal_prn));

    server.abort();
}
```

In the same file, migrate the four pre-existing tests: in `create_user_over_grpc_mints_a_principal`, `a_duplicate_email_is_already_exists`, `a_malformed_email_is_invalid_argument` and `an_empty_locale_becomes_unset`, change each

```rust
    support::provision(&state, &token).await;
```

to

```rust
    // SMA-584: CreateUser now requires `Action::CreateUser`@`Root`. These tests cover minting,
    // conflicts and the D11 wire sentinel — not authorization — so they act as a platform_admin.
    support::provision_platform_admin(&state, &token).await;
```

In `a_malformed_email_is_invalid_argument`, leave the existing `let before = ...count()` line exactly where it is — it is already taken *after* provisioning, so the extra grant does not disturb the baseline.

Finally update the file's module doc: change the first paragraph's tail from

```rust
//! empty-locale-becomes-unset wire sentinel, and the D0 pin that this RPC is bearer-required
//! but performs NO further authorization check — mirroring `POST /v1/users` exactly (see
//! `adapters::grpc::users` module doc).
```

to

```rust
//! empty-locale-becomes-unset wire sentinel, and the SMA-584 pin that this RPC is
//! bearer-required AND authorized (`Action::CreateUser`@`Root`) — mirroring `POST /v1/users`
//! exactly (see `adapters::grpc::users` module doc; `tests/http_users.rs` is the HTTP twin).
```

- [ ] **Step 2: Run it to verify it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test grpc_users
```

Expected: `create_user_requires_platform_admin` FAILS — the ordinary principal's call returns `Ok`, not `PermissionDenied`. The other four should pass (the extra grant is harmless while the endpoint is still open).

- [ ] **Step 3: Add the guard to the gRPC adapter**

In `rs/crates/services/paigasus-iam/src/adapters/grpc/users.rs`, replace the module doc's second and third paragraphs. Delete:

```rust
//! **This RPC performs NO authorization check, deliberately, and that is why the service
//! exists.** `CreateUser::execute` takes no `actor` parameter, `http::users` extracts no
//! `AuthContext`, and there is no `Action::CreateUser` in the Cedar action catalog — so
//! `POST /v1/users` is bearer-gated and otherwise unauthorized. This adapter mirrors that
//! exactly, because parity with the HTTP surface is the acceptance criterion and tightening
//! authorization on an existing endpoint is a behavior change belonging to its own issue.
//!
//! It sits on `UserService` rather than `TenancyService` for exactly this reason: all 21
//! `TenancyService` RPCs authorize in the adapter (`if self.state.enforce_tenancy { … }`), so
//! parking the one unchecked RPC among them would camouflage the single property a reviewer
//! most needs to see. On its own service, the absence is legible in the contract.
```

and put in its place:

```rust
//! **This RPC IS authorized (SMA-584):** it checks `Action::CreateUser` at `root_prn()`, gated
//! by `enforce_tenancy`, exactly as `POST /v1/users` does. `Root` is the top of the Cedar
//! hierarchy and `resource in ?resource` is descendant-or-self, so no `Organization`/`Team`/
//! `Project`-scoped grant can satisfy it: under the starter role set this is `platform_admin`
//! only. The check runs BEFORE `CreateUser::execute`, so a denied caller never reaches email
//! validation or the unit of work.
//!
//! It sits on `UserService` rather than `TenancyService` because a **user is a principal, not
//! a tenancy node** — a different aggregate from `TenancyService`'s org/team/project/membership
//! surface, exactly as `ServiceAccountService` is. `UserService` is the intended home for
//! future user-principal operations (`GetUser`, `ListUsers`, `ArchiveUser`). (Until SMA-584 the
//! stated reason was that this was the one *unauthorized* RPC and parking it among 21
//! authorized ones would camouflage it; that reason is now obsolete.)
```

Then add the imports. Change:

```rust
use super::convert;
use crate::adapters::http::AppState;
use crate::application::create_user::NewUser;
use crate::application::error::TenancyError;
```

to:

```rust
use paigasus_iam_core::Action;
use paigasus_iam_core::authz::model::root_prn;

use super::convert;
use crate::adapters::auth::AuthContext;
use crate::adapters::http::AppState;
use crate::application::create_user::NewUser;
use crate::application::error::TenancyError;
```

Add the `actor_context` helper immediately above `pub(crate) fn opt_string` (verbatim from `grpc/tenancy.rs` — every gRPC adapter in this crate keeps its own private copy):

```rust
/// Extracts the bearer-resolved [`AuthContext`] from a gRPC request's extensions — mirrors
/// the identical private helper in `grpc::tenancy`/`grpc::authz`/`grpc::audit`/
/// `grpc::service_accounts`/`grpc::dead_letters`.
fn actor_context<T>(request: &Request<T>) -> Result<AuthContext, Status> {
    request.extensions().get::<AuthContext>().cloned().ok_or_else(convert::missing_auth_context)
}
```

Now the RPC itself. Change its doc comment from:

```rust
    /// `CreateUser`: bearer-required, otherwise UNAUTHORIZED BY DESIGN — see this module's doc.
```

to:

```rust
    /// `CreateUser`: bearer-required AND authorized (`Action::CreateUser`@`Root`) — see this
    /// module's doc.
```

and insert the guard as the first thing inside the async block, before `let req = request.into_inner();`. The `actor` must be read from `request` *before* `into_inner()` consumes it:

```rust
        let result: Result<Response<CreateUserResponse>, Status> = async {
            let actor = actor_context(&request)?.principal_id.prn().clone();
            if self.state.enforce_tenancy {
                self.state.authorize.check(&actor, Action::CreateUser, &root_prn()).await.map_err(convert::status_to_grpc)?;
            }
            let req = request.into_inner();
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test grpc_users
```

Expected: PASS, all five tests.

- [ ] **Step 5: Rewrite the three remaining stale doc sites**

These claim the RPC is unauthorized and are now false. None affects behaviour; all three are required so the codebase does not document a security posture it no longer has.

In `rs/crates/services/paigasus-iam/src/adapters/grpc/authn.rs`, around line 126, the `is_exempt` doc says `UserService.CreateUser` performs no authorization check of its own. Rewrite that sentence to state that `UserService` is not exempt from the bearer layer **and** authorizes `Action::CreateUser` in its handler (SMA-584), so the exemption list is unrelated to its authorization posture.

In `rs/crates/services/paigasus-iam/src/adapters/grpc/mod.rs`, three sites:
- the module-level doc at lines 7-8 ("`CreateUser` is deliberately unauthorized");
- line ~79 ("`UserService.CreateUser` is bearer-required but otherwise unauthorized BY DESIGN");
- line ~102's inline comment pointing at the `users` module doc "for why `CreateUser` performs no authorization check".

Rewrite each to the SMA-584 posture: `UserService` is always mounted and its one RPC authorizes `Action::CreateUser` at `Root`, mirroring `POST /v1/users`.

**Do not write the bare string `"forbidden"` in any of these comments** — `repo:error-code-single-site` matches a quoted registry code in comments too. Use backticks if you need to name the code.

- [ ] **Step 6: Verify the whole crate still builds clean**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo clippy -p paigasus-iam --all-targets -- -D warnings && cargo fmt --check
```

Expected: PASS, no warnings.

- [ ] **Step 7: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/adapters/grpc/users.rs \
        rs/crates/services/paigasus-iam/src/adapters/grpc/authn.rs \
        rs/crates/services/paigasus-iam/src/adapters/grpc/mod.rs \
        rs/crates/services/paigasus-iam/tests/grpc_users.rs
git commit -m "feat(rs): authorize UserService.CreateUser with Action::CreateUser (SMA-584)

Mirror the HTTP guard on the gRPC transport, reading the actor from the
request extensions before into_inner consumes it.

Rewrite the four doc sites that asserted this RPC was deliberately
unauthorized. UserService keeps its own service, but for the durable
reason - a user is a principal, not a tenancy node.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01Cb6J6fQxMweNZ3Q3G3wuf2"
```

---

### Task 4: Pin the action identity (P2)

Without this task every test in Tasks 2 and 3 passes with `Action::CreateOrganization` (or any other Root-only action) wired into the adapters, because `platform_admin`'s template carries no `action in [...]` clause and no other role has `CreateUser`. A copy-paste of the `create_org` guard that forgot to change the action would ship green and the new variant would be decorative. This task makes the binding falsifiable on each transport.

The mechanism is the §4.3 lever-1 shape from the spec — a narrow static Cedar policy permitting exactly `CreateUser` — so these tests double as executable documentation of the recommended operator remediation.

**Files:**
- Modify: `rs/crates/services/paigasus-iam/tests/http_users.rs`
- Modify: `rs/crates/services/paigasus-iam/tests/grpc_users.rs`

**Interfaces:**
- Consumes: `tests/http_users.rs` (Task 2) and `tests/grpc_users.rs` (Task 3) with their existing helpers.
- Produces: nothing consumed downstream.

- [ ] **Step 1: Write the failing HTTP test**

Append to `rs/crates/services/paigasus-iam/tests/http_users.rs`. Extend the `use` lines at the top to add `provision` is already there; add `app_with_config`, `test_config` and `root_prn`:

```rust
use paigasus_iam_core::authz::model::root_prn;
use support::{app_with_config, app_with_state, provision, provision_platform_admin, send, test_config};
```

(replace the existing `use support::{...};` line with the one above, and add the `root_prn` import next to the other `paigasus_*` imports).

Then append the test:

```rust
/// **The action-identity pin (SMA-584).** Every other test in this file and in
/// `tests/grpc_users.rs` passes identically if the adapter checks `CreateOrganization` — or any
/// other Root-only action — instead of `CreateUser`, because `platform_admin`'s template has no
/// `action in [...]` clause and no other system role carries `CreateUser`. So no ROLE grant can
/// tell the two apart.
///
/// A narrow STATIC policy can. This seeds one permitting exactly `CreateUser` (the operator
/// remediation the design doc recommends, §4.3 lever 1) and asserts the principal it covers can
/// create a user but NOT an organization. A mutation that wires any other action into
/// `adapters::http::users` fails here.
#[tokio::test]
async fn the_http_guard_is_bound_to_create_user_specifically() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let mut cfg = test_config(&idp);
    // Mirrors `tests/authz_acceptance.rs`'s AC3 setup: a short cache TTL so the decision made
    // immediately after PutPolicy reflects the new policy.
    cfg.authz.policy_cache_ttl_secs = 1;
    let (app, state) = app_with_config(db, &cfg).await;

    // The admin who authors the policy (`Action::PutPolicy` is Root-only).
    let admin_token = idp.bearer("http-bind-admin", Some("http-bind-admin@example.com"), "paigasus", 3600);
    provision_platform_admin(&state, &admin_token).await;

    // The subject: an ordinary principal with no role grant at all.
    let subject_token = idp.bearer("http-bind-subject", Some("http-bind-subject@example.com"), "paigasus", 3600);
    provision(&state, &subject_token).await;

    // Before the policy: the subject cannot create a user.
    let (status, body) = send(&app, "POST", "/v1/users", Some(json!({"email": "before@example.com", "display_name": "Before"})), Some(subject_token.as_str())).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

    // Seed a static policy permitting EXACTLY CreateUser, and nothing else.
    let policy_body = json!({
        "policy_id": "sma-584-create-user-only",
        "kind": "static",
        "source": r#"permit(principal, action == Pgs::Iam::Action::"CreateUser", resource);"#,
        "description": "SMA-584 action-identity pin: CreateUser only",
    });
    let (status, put) = send(&app, "POST", "/v1/authz/policies", Some(policy_body), Some(admin_token.as_str())).await;
    assert_eq!(status, StatusCode::OK, "{put}");

    // The subject can now create a user...
    let (status, body) = send(&app, "POST", "/v1/users", Some(json!({"email": "bound@example.com", "display_name": "Bound"})), Some(subject_token.as_str())).await;
    assert_eq!(status, StatusCode::CREATED, "the guard must check CreateUser, not some other Root action: {body}");

    // ...but CONTROL: it still cannot create an organization. This is what proves the seeded
    // policy is genuinely narrow and the subject is not a platform admin by another name.
    let (status, body) = send(
        &app,
        "POST",
        "/v1/organizations",
        Some(json!({"slug": "bound-org", "name": "Bound Org"})),
        Some(subject_token.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "the CreateUser-only policy must not permit CreateOrganization: {body}");

    // Sanity: `root_prn()` is the resource the guard authorizes against, and it is the Cedar
    // hierarchy root — asserted here so this test names the resource the design fixes.
    assert!(root_prn().canonical().contains("root/"), "root_prn is the Root sentinel");
}
```

- [ ] **Step 2: Run it to verify it fails against a wrong-action mutation**

First confirm it passes as written:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test http_users
```

Expected: PASS.

Now **prove the test bites.** Temporarily change `Action::CreateUser` to `Action::CreateOrganization` in `src/adapters/http/users.rs`'s guard, re-run, and confirm the test FAILS at the "must check CreateUser, not some other Root action" assertion. Then revert the mutation with `git checkout -- rs/crates/services/paigasus-iam/src/adapters/http/users.rs` and re-run to confirm PASS.

A test that cannot be made to fail is not a pin. Do not skip this step — and revert via `git checkout`, not by re-editing from a `.bak`, because a restored file with a rolled-back mtime makes cargo reuse the binary built from the mutation.

- [ ] **Step 3: Write the gRPC half**

Append to `rs/crates/services/paigasus-iam/tests/grpc_users.rs`. This one seeds the policy through the store-backed HTTP router is not available here, so use the same `AppState` and drive `PolicyService` directly through the state the test already holds:

```rust
/// **The action-identity pin, gRPC half (SMA-584).** The twin of
/// `tests/http_users.rs::the_http_guard_is_bound_to_create_user_specifically`; see that test's
/// doc for why a role grant cannot distinguish `CreateUser` from any other Root-only action.
/// A mutation that wires a different action into `adapters::grpc::users` fails here.
#[tokio::test]
async fn the_grpc_guard_is_bound_to_create_user_specifically() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let mut cfg = support::test_config(&idp);
    cfg.authz.policy_cache_ttl_secs = 1;
    let state = AppState::new(db, &cfg).await.unwrap();

    let admin_token = idp.bearer("grpc-bind-admin", Some("grpc-bind-admin@example.com"), "paigasus", 3600);
    let admin_prn = support::provision_platform_admin(&state, &admin_token).await;

    let subject_token = idp.bearer("grpc-bind-subject", Some("grpc-bind-subject@example.com"), "paigasus", 3600);
    support::provision(&state, &subject_token).await;

    let (addr, server) = spawn_server(state.clone()).await;
    let mut client = UserServiceClient::new(channel(addr).await);

    // Before the policy: denied.
    let err = client.create_user(authed(create_user_request("grpc-before@example.com"), &subject_token)).await.unwrap_err();
    assert_eq!(err.code(), Code::PermissionDenied, "{err:?}");

    // Seed a static policy permitting EXACTLY CreateUser, authored by the platform_admin.
    let doc = paigasus_iam_core::PolicyDocument {
        policy_id: "sma-584-create-user-only-grpc".to_string(),
        kind: paigasus_iam_core::authz::model::PolicyKind::Static,
        source: r#"permit(principal, action == Pgs::Iam::Action::"CreateUser", resource);"#.to_string(),
        description: "SMA-584 action-identity pin: CreateUser only".to_string(),
        system: false,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };
    let actor = paigasus_kernel::Prn::parse(&admin_prn).expect("valid principal prn");
    // `PolicyService::put(&self, actor: &Prn, doc: PolicyDocument)` — `doc` is taken BY VALUE.
    // It authorizes `Action::PutPolicy` at Root itself, which the seeded platform_admin holds.
    state.policies.put(&actor, doc).await.expect("platform_admin may PutPolicy at Root");

    // Now permitted...
    let resp = client.create_user(authed(create_user_request("grpc-bound@example.com"), &subject_token)).await.unwrap().into_inner();
    Prn::parse(&resp.principal_prn).unwrap_or_else(|e| panic!("unexpected principal prn {}: {e}", resp.principal_prn));

    server.abort();
}
```

**Verified interfaces** (`src/application/policies.rs:118`, `libs/paigasus-iam-core/src/authz/model.rs:171`): `PolicyService::put(&self, actor: &Prn, doc: PolicyDocument) -> Result<(), TenancyError>` takes the document **by value** and authorizes `Action::PutPolicy` at `root_prn()` internally, so the seeded `platform_admin` is exactly the authority it needs. `PolicyDocument`'s seven fields are `policy_id`, `kind`, `source`, `description`, `system`, `created_at`, `updated_at` — as written above.

If driving `PolicyService` directly proves awkward, an equally valid alternative is to mount the HTTP router over the same `AppState` (`paigasus_iam::adapters::http::router(state.clone())`) and seed via `POST /v1/authz/policies` exactly as the HTTP half does — the point of this test is the gRPC *read* path, not how the policy got written.

The gRPC control case (that the subject still cannot do some other Root action) is **not** repeated here — `AuthorizationService`'s admin RPCs are a different surface and the HTTP half already proves the seeded policy is narrow. What this half uniquely pins is that `grpc::users`'s guard names `CreateUser`.

- [ ] **Step 4: Run it, and prove it bites**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test grpc_users
```

Expected: PASS. Then mutate `Action::CreateUser` → `Action::CreateOrganization` in `src/adapters/grpc/users.rs`, confirm FAIL, and `git checkout --` the file to revert.

- [ ] **Step 5: Commit**

```bash
git add rs/crates/services/paigasus-iam/tests/http_users.rs \
        rs/crates/services/paigasus-iam/tests/grpc_users.rs
git commit -m "test(rs): pin the CreateUser action binding on both transports (SMA-584)

Role grants cannot distinguish CreateUser from any other Root-only
action, because platform_admin's template has no action clause and no
other role carries it - so every other test passes with the wrong
action wired. A narrow static policy permitting exactly CreateUser can
tell them apart, with CreateOrganization as the control.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01Cb6J6fQxMweNZ3Q3G3wuf2"
```

---

### Task 5: `enforce_tenancy` — coverage, visibility, documentation

The guard is gated by `enforce_tenancy`, so the `false` path needs a test. And since this change widens that flag's blast radius to include principal minting, the flag stops being invisible: it gets a boot warning and a config-example entry.

**Files:**
- Modify: `rs/crates/services/paigasus-iam/tests/authz_enforce_toggle.rs`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/http/mod.rs` (near line 672)
- Modify: `rs/crates/services/paigasus-iam/iam.toml.example` (the `[authz]` block, ~line 54)

**Interfaces:**
- Consumes: the HTTP guard from Task 2.
- Produces: nothing consumed downstream.

- [ ] **Step 1: Write the failing test**

Append to `rs/crates/services/paigasus-iam/tests/authz_enforce_toggle.rs`:

```rust
/// SMA-584: `POST /v1/users` joined the set of `enforce_tenancy`-gated routes, so the `false`
/// setting must short-circuit its `Action::CreateUser` check too. Without this, a guard that
/// ignored `enforce_tenancy` entirely would pass every other test in the suite.
#[tokio::test]
async fn enforce_tenancy_false_lets_an_otherwise_ungranted_principal_create_a_user() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let mut cfg = test_config(&idp);
    cfg.authz.enforce_tenancy = false;
    let (app, state) = app_with_config(db, &cfg).await;

    let token = idp.bearer("no-grants-user", Some("no-grants@example.com"), "paigasus", 3600);
    // JIT-provision the principal but grant it NOTHING — under the default
    // `enforce_tenancy = true` this exact call is a 403 (`tests/http_users.rs`).
    provision(&state, &token).await;

    let (status, body) = send(
        &app,
        "POST",
        "/v1/users",
        Some(json!({"email": "toggle-off@example.com", "display_name": "Toggle Off"})),
        Some(token.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "enforce_tenancy = false must bypass the CreateUser gate: {body}");
}
```

- [ ] **Step 2: Run it to verify it passes**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test authz_enforce_toggle
```

Expected: PASS. Unlike the other tasks this test is green from the start — Task 2 already wrote the `if s.enforce_tenancy` guard correctly. **Prove it bites** by temporarily deleting the `if s.enforce_tenancy {` wrapper in `src/adapters/http/users.rs` (leaving the bare `check` call), confirming this test FAILS with 403, then reverting with `git checkout --`.

- [ ] **Step 3: Make the flag visible at boot**

In `rs/crates/services/paigasus-iam/src/adapters/http/mod.rs`, find the `accept_invalid_tls` warning near line 672:

```rust
        if authn_cfg.accept_invalid_tls {
            tracing::warn!("accept_invalid_tls is enabled: TLS certificate verification for IdP discovery/JWKS fetches is DISABLED — test-only configuration, never use in production");
        }
```

Add a sibling warning for `enforce_tenancy`. Place it next to wherever `enforce_tenancy` is read into `AppState` (search for `enforce_tenancy` in this file) so it fires once per `AppState::new`:

```rust
        if !cfg.authz.enforce_tenancy {
            tracing::warn!(
                "enforce_tenancy is disabled: EVERY tenancy authorization check is bypassed, including Action::CreateUser on POST /v1/users and UserService.CreateUser — test-only configuration, never use in production"
            );
        }
```

Adjust `cfg.authz` to whatever the surrounding code binds the config to (it may already be destructured).

- [ ] **Step 4: Document the flag**

In `rs/crates/services/paigasus-iam/iam.toml.example`, in the `[authz]` block (starting `# [authz]` around line 54), add an `enforce_tenancy` entry above the existing `admin_enabled` one, matching the file's comment style:

```
# enforce_tenancy = true              # Enforce per-action Cedar authorization on the tenancy
#                                      # surface (default true). When false, EVERY
#                                      # `authorize.check(..)` call site is skipped — org/team/
#                                      # project/membership CRUD, service accounts, API keys,
#                                      # and `Action::CreateUser` on POST /v1/users and
#                                      # UserService.CreateUser (SMA-584). Authentication is
#                                      # unaffected: a bearer is still required. Test-only —
#                                      # never set this false in production.
```

- [ ] **Step 5: Verify**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo clippy -p paigasus-iam --all-targets -- -D warnings && cargo fmt --check
cd rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test authz_enforce_toggle
```

Expected: PASS, no warnings.

- [ ] **Step 6: Commit**

```bash
git add rs/crates/services/paigasus-iam/tests/authz_enforce_toggle.rs \
        rs/crates/services/paigasus-iam/src/adapters/http/mod.rs \
        rs/crates/services/paigasus-iam/iam.toml.example
git commit -m "feat(rs): warn and document enforce_tenancy, cover its CreateUser path (SMA-584)

POST /v1/users joined the enforce_tenancy-gated routes, so cover the
false setting. That flag now also bypasses principal minting, and it
was invisible - absent from the config example and silent at boot - so
give it a warning mirroring accept_invalid_tls and an example entry.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01Cb6J6fQxMweNZ3Q3G3wuf2"
```

---

### Task 6: Contract comment, suite counts, and the full gate run

Everything that documents the old posture outside the two adapters, plus the repo-level verification that per-project tasks do not perform.

**Files:**
- Modify: `contracts/proto/paigasus/iam/v1/iam.proto` (~line 508)
- Modify: `rs/crates/services/paigasus-iam/tests/docker_preflight.rs:5,44`
- Modify: `rs/crates/services/paigasus-iam/tests/support/docker.rs:255`
- Modify: `docs/dev-setup.md:67`
- Modify: `CLAUDE.md:140`

**Interfaces:**
- Consumes: all prior tasks.
- Produces: a branch ready for Stage 5.

- [ ] **Step 1: Rewrite the proto banner comment**

In `contracts/proto/paigasus/iam/v1/iam.proto`, replace the `Users (SMA-501)` banner:

```proto
// ─────────────────────────────────────────────────────────────────────────
// Users (SMA-501). CreateUser has NO per-action authorization — it requires
// a bearer and nothing more, mirroring `POST /v1/users` exactly. That is a
// deliberate parity decision, not an oversight: see the design doc's D0.
// It lives on its own service rather than on TenancyService precisely so
// that property is visible in the contract instead of hidden among 21
// authorized RPCs.
// ─────────────────────────────────────────────────────────────────────────
```

with:

```proto
// ─────────────────────────────────────────────────────────────────────────
// Users (SMA-501, authorization added in SMA-584). CreateUser requires a
// bearer AND `Action::CreateUser` at the Cedar hierarchy root, gated by the
// server's enforce_tenancy setting — mirroring `POST /v1/users` exactly.
// Root is the top of the hierarchy, so no org/team/project-scoped role
// grant can satisfy it: under the starter role set this is platform_admin
// only. UserService is separate from TenancyService because a user is a
// principal, not a tenancy node — a different aggregate, and the intended
// home for future user-principal operations.
// ─────────────────────────────────────────────────────────────────────────
```

This is comment-only: no field, message or service changes, so `repo:breaking` is unaffected.

- [ ] **Step 2: Format the proto and regenerate the bindings**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd contracts && buf format -w && buf generate
```

`buf format -w` is mandatory — an unformatted `.proto` reds `contracts:fmt` silently inside `moon ci`. Run `buf generate` **directly** rather than via the Moon task: `contracts:generate` declares no `outputs:` and can serve stale cached output. Even a comment-only change shifts the embedded `FILE_DESCRIPTOR_SET`, so the generated bindings genuinely do change.

- [ ] **Step 3: Update the integration-suite counts**

Task 2 added one Docker-backed test binary (`tests/http_users.rs`), so four hand-maintained counts move. **Count them; do not trust these numbers.** Determine the true totals with:

```bash
cd rs/crates/services/paigasus-iam && ls tests/*.rs | wc -l
```

Then update, keeping each site's existing sentence shape:
- `rs/crates/services/paigasus-iam/tests/docker_preflight.rs:5` — "61 of this crate's 65 integration binaries start a container"
- `rs/crates/services/paigasus-iam/tests/docker_preflight.rs:44` — the assertion message, "so 60 of this crate's 65 integration suites will report PASS"
- `rs/crates/services/paigasus-iam/tests/support/docker.rs:255`
- `docs/dev-setup.md:67` — "61 of its 65 integration"
- `CLAUDE.md:140` — "(61 of its 65 integration binaries)"

No CI gate enforces these, which is exactly why they must be done deliberately here.

- [ ] **Step 4: Run the full repo gate graph**

Per CLAUDE.md, per-project Moon tasks do **not** run the repo-level gates. Run what CI runs:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site \
  :input-liveness :promtool :observability-drift :nats-permissions :release-parity \
  :release-parity-py :release-parity-ts :publish-metadata --base origin/main --include-relations
```

Expected: PASS. Gates most likely to bite, and what to do:
- `:error-code-single-site` — if it reds, a doc comment somewhere spells `"forbidden"` with double quotes. Backtick it.
- `:iam-docker-policy-single-site` — if it reds, `tests/http_users.rs` hand-rolled a Docker skip instead of using `support::start_migrated_postgres`. It must use the shared helper.
- `:breaking` — should pass; the proto change is comment-only. If it reds, something structural changed in Step 1.
- `:fmt` — run `buf format -w` (Step 2) and `cargo fmt`.

Moon reports failures without attribution. Diagnose with:

```bash
jq '.actions[] | select(.status == "failed")' .moon/cache/ciReport.json
```

- [ ] **Step 5: Commit**

```bash
git add contracts/ rs/crates/services/paigasus-iam/tests/docker_preflight.rs \
        rs/crates/services/paigasus-iam/tests/support/docker.rs \
        docs/dev-setup.md CLAUDE.md
git commit -m "docs(contracts): state CreateUser's authorization in the proto (SMA-584)

Rewrite the Users banner comment, which said the RPC had no per-action
authorization. Comment-only, so the wire contract is unchanged, but the
embedded descriptor set shifts - hence the regenerated bindings.

Also move the four hand-maintained integration-suite counts, since the
new http_users suite is Docker-backed.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01Cb6J6fQxMweNZ3Q3G3wuf2"
```

---

## Self-review

**Spec coverage.** Every spec section maps to a task: D1/D5 §5.1 → Task 1; D2/§5.2 HTTP + §6.2 → Task 2; D3/D7/§5.2 gRPC + §6.1 → Task 3; §6.3 (P2) → Task 4; §6.4 + §5.5 → Task 5; §5.4 + §5.3 counts + §7 → Task 6. D4 is satisfied by *omission* (no `*_ACTIONS` edit) and pinned by Task 1 Step 1's second table case. D6 is satisfied by doing nothing plus the backtick constraint, which appears in Global Constraints and again in Tasks 3 and 6. D8 needs no code. §4 (rollout) is operator-facing and generates only the Task 5 config entry; the rest travels in the spec and belongs in the PR body. §6.5 (AC-3) is a *negative* requirement — those five suites must stay unmodified — and is verified by Task 6's full graph run, not by an edit.

**Deliberate non-coverage.** §4.5 (auditing already-squatted `user` rows) and §8's deferrals are out of scope by design and get no task.

**Placeholder scan.** Clean. Every interface the plan names was read out of the source: `PolicyService::put` (`application/policies.rs:118`), `PolicyDocument` (`authz/model.rs:171`), the `starter_policy_table` `Case`/`grant`/`universe` helpers (`authz/roles.rs:380-475`), and the `support` test helpers (`tests/support/mod.rs:493-656`). The one `<paste ...>` — Task 1 Step 7's content hash — is machine-generated by Step 6 and cannot exist before the code does; that is the correct shape for it, not a gap.

**Type consistency.** `Action::CreateUser` is defined once (Task 1) and referenced identically in Tasks 2, 3, 4. `root_prn()` is imported from `paigasus_iam_core::authz::model` in both adapters. `actor_prn(&ctx)` (HTTP) and `actor_context(&request)?.principal_id.prn().clone()` (gRPC) match the helpers each transport already uses. `provision_platform_admin` returns `String` (a PRN), which Task 4's gRPC half parses with `Prn::parse` — consistent with `seed_platform_admin`'s own signature.

<!-- moon-diagnosis:superseded -->
> **Superseded (SMA-597).** The `ciReport.json` diagnosis advice above does not work as written:
> there is no action-level `exitCode` key, and the file carries no stdout/stderr at all. The
> measured procedure is in CLAUDE.md between the `moon-diagnosis` markers. This document is left
> otherwise unedited as a record of what was believed when it was written.
