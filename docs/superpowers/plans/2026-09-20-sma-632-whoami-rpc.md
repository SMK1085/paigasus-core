# IAM `WhoAmI` RPC Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a bearer-enforced `AuthnService.WhoAmI` RPC that provisions the caller and returns its principal, so the consoles stop calling `ServiceInfoService.GetServiceInfo` purely for its provisioning side effect.

**Architecture:** The mechanism is *omission*. `AuthEnforce` (gRPC) and `require_bearer` (HTTP) already resolve the bearer with `Provisioning::Enabled` and seed the bootstrap `platform_admin` grant for every RPC absent from `is_exempt`. `WhoAmI` is simply not added to that list, so provisioning happens before its handler runs. The handler turns the `AuthContext` the middleware inserted into a `WhoAmIResponse`. The consoles then make one call where they made two plus a conditional retry.

**Tech Stack:** protobuf + buf (v2, `STANDARD` lint), Rust (edition 2024, rust-version 1.95, tonic + axum + SeaORM, `cargo nextest`), TypeScript (pnpm, vitest, Playwright, `@connectrpc` clients), Moon task orchestration.

**Spec:** `docs/superpowers/specs/2026-09-20-sma-632-whoami-rpc-design.md` (revision 2, approved)

## Global Constraints

- **Working tree:** `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-632-whoami`, branch `feature/sma-632-iam-whoami-rpc`. Do not `cd` to the main checkout.
- **PATH:** every shell step must start from a shell that has run `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`. Without it `moon`, `buf`, `uv` and `cargo-nextest` resolve to the wrong versions or not at all.
- **`PROTO_REPORTER=text`** must be exported in any script that captures `proto` or proto-shimmed output.
- **SPDX header:** every new source file opens with `// SPDX-License-Identifier: Apache-2.0` (`#` for Python).
- **Rust: `warnings = deny`.** An unused field, function or import is a **hard compile error**, not a warning. This is why Tasks 4 and 5 each land a definition together with its caller. Never split a definition from its only reader across two commits.
- **Rust edition 2024, rust-version 1.95.**
- **Commits:** conventional, with a workspace scope — `feat(rs):`, `feat(contracts):`, `refactor(ts):`. Every commit message ends with the line `Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>`, preceded by a blank line. Do **not** put a bare `#NNN` or a `token: value` line in a commit body — commitlint's `footer-leading-blank` rule rejects it.
- **Never use `--no-verify`.** The worktree's dependencies are installed; the `commit-msg` hook works.
- **Test runner (Rust):** `cargo nextest run`, never `cargo test`.
- **Docker-gated tests skip quietly.** A green run does not prove they executed. Set `PAIGASUS_REQUIRE_DOCKER=1` to turn a skip into a failure when you need proof.

---

## File Structure

**Created:**

| Path | Responsibility |
|---|---|
| `rs/crates/services/paigasus-iam/src/application/principal_context.rs` | One membership-paging helper, shared by three callers. Holds the page-size constant. |
| `rs/crates/services/paigasus-iam/tests/grpc_whoami.rs` | The RPC's integration tests, including the provisioning control. |

**Modified:**

| Path | Change |
|---|---|
| `contracts/proto/paigasus/iam/v1/iam.proto` | `WhoAmIRequest`, `WhoAmIResponse`, the `WhoAmI` RPC. |
| `rs/crates/services/paigasus-iam/src/application/mod.rs` | Declare the new module. |
| `rs/crates/services/paigasus-iam/src/application/authenticate_token.rs` | Delete the duplicate loop and constant; add `context_for`. |
| `rs/crates/services/paigasus-iam/src/application/authenticate_api_key.rs` | Delete the duplicate loop and constant. |
| `rs/crates/services/paigasus-iam/src/adapters/auth.rs` | `AuthContext` gains `kind` and `status`. |
| `rs/crates/services/paigasus-iam/src/adapters/grpc/authn.rs` | `actor_context`, the `who_am_i` handler, the `is_exempt` doc, the exemption test. |
| `rs/crates/services/paigasus-iam/src/adapters/grpc/convert.rs` | `to_who_am_i_response` and its tests. |
| `rs/crates/services/paigasus-iam/src/adapters/http/auth_middleware.rs` | Pass the two new `AuthContext` fields. |
| `rs/crates/services/paigasus-iam/src/adapters/http/authz_middleware.rs` | Test helper: the two new fields. |
| `rs/crates/services/paigasus-iam/src/adapters/http/dto.rs` | `WhoAmIResponseDto`. |
| `rs/crates/services/paigasus-iam/src/adapters/http/authn.rs` | `whoami_router`, the handler. |
| `rs/crates/services/paigasus-iam/src/adapters/http/mod.rs` | Mount the route inside `protected`; two route-conflict tests. |
| `ts/packages/paigasus-console-core/testing/fake-iam.ts` | The `whoAmI` answer, keyed on the context token. |
| `ts/packages/paigasus-console-core/testing/dev-world.ts` | An `authn.whoAmI` handler; the unscripted-method comment. |
| `ts/packages/paigasus-console-core/src/principal.ts` | `introspectWithProvisioning` becomes `whoAmI`. |
| `ts/packages/paigasus-console-core/src/principal-resolver.ts` | Two calls become one. |
| `ts/packages/paigasus-console-core/src/runtime.ts` | Drop `provisionFirst`. |
| `ts/packages/paigasus-console-core/src/index.ts` | The renamed export. |
| `ts/packages/paigasus-console-core/tests/**` | The ordering and degradation tests. |
| `ts/apps/{iam,gateway}-console/tests/**` | Ordering assertions, the call-count formula. |
| `ts/apps/{iam,gateway}-console/lib/auth.ts` | Comment correction. |
| `ts/packages/paigasus-sdk/src/iam.ts` | Comment correction. |

---

## Task 1: The proto contract and its generated bindings

**Files:**
- Modify: `contracts/proto/paigasus/iam/v1/iam.proto:289-292` (the `AuthnService` block) and the message area above it
- Regenerate: `rs/crates/libs/paigasus-proto/src/generated/**`, `py/**/paigasus_proto/generated/**`, `ts/packages/paigasus-proto/src/generated/**`

**Interfaces:**
- Consumes: nothing.
- Produces: Rust `paigasus_proto::paigasus::iam::v1::{WhoAmIRequest, WhoAmIResponse}` and the `AuthnService` trait method `who_am_i`. TypeScript `AuthnService.method.whoAmI`. Every later task depends on these existing.

- [ ] **Step 1: Add the two messages next to their `Introspect` siblings**

In `contracts/proto/paigasus/iam/v1/iam.proto`, directly after the existing `IntrospectResponse` message, add:

```proto
message WhoAmIRequest {}

// The caller's own principal, as resolved by bearer enforcement. Mirrors IntrospectResponse's
// fields, with two differences that the WhoAmI RPC's doc explains: `issuer`/`subject` are empty
// for an API-key bearer, and `expires_at` is absent when the credential has no expiry.
//
// A SEPARATE message from IntrospectResponse because buf lint STANDARD's
// RPC_REQUEST_RESPONSE_UNIQUE forbids one message serving two RPCs, and
// RPC_RESPONSE_STANDARD_NAME requires this exact name. Field numbering starts clean: this
// message has no history, so `role_grants` takes 7, where IntrospectResponse reserves 7 for the
// retired `role_group_prns` and puts `role_grants` at 8.
message WhoAmIResponse {
  string principal_prn = 1;
  string status = 2; // principal status
  string issuer = 3; // empty for an API-key bearer
  string subject = 4; // empty for an API-key bearer
  google.protobuf.Timestamp expires_at = 5; // absent when the credential has no expiry
  repeated Membership memberships = 6; // reuse tenancy message
  repeated RoleGrantRef role_grants = 7; // empty until SMA-633 populates it
}
```

- [ ] **Step 2: Add the RPC to `AuthnService`**

Replace the `service AuthnService { ... }` block (currently at `iam.proto:289-292`) with:

```proto
service AuthnService {
  rpc Introspect(IntrospectRequest) returns (IntrospectResponse);
  rpc IntrospectApiKey(IntrospectApiKeyRequest) returns (IntrospectApiKeyResponse);
  // The caller's own principal. BEARER-ENFORCED: this RPC is deliberately ABSENT from
  // `is_exempt` (rs/crates/services/paigasus-iam/src/adapters/grpc/authn.rs), so `AuthEnforce`
  // resolves the bearer with `Provisioning::Enabled` and seeds the bootstrap platform_admin
  // grant BEFORE the handler runs. That omission is the whole mechanism — the handler never
  // sees a token.
  //
  // Provisioning still obeys the issuer's JIT policy: an issuer whose jit flag is off yields
  // `identity-not-provisioned`, the same answer Introspect gives.
  //
  // Serves both credential kinds. For a Paigasus API-key bearer, `issuer` and `subject` are
  // empty: a service account has no (issuer, subject) pair, and this response carries no
  // `key_id`/`scope_prn` (those are IntrospectApiKeyResponse's).
  rpc WhoAmI(WhoAmIRequest) returns (WhoAmIResponse);
}
```

- [ ] **Step 3: Format, then lint — lint is the gate that killed the first design**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd contracts && buf format -w && buf lint
```

Expected: no output from either. If `buf lint` reports `RPC_RESPONSE_STANDARD_NAME` or
`RPC_REQUEST_RESPONSE_UNIQUE`, the RPC is still pointing at `IntrospectResponse` — re-read Step 2.

Skipping `buf format -w` makes `contracts:fmt` fail later **without printing a diff**, which is
hard to diagnose.

- [ ] **Step 4: Check the change is not breaking**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd contracts && buf breaking --against '.git#branch=origin/main,subdir=contracts'
```

Expected: no output. Two new messages and a new RPC are additive under the `FILE` category.

- [ ] **Step 5: Generate the bindings**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-632-whoami
moon run contracts:generate
```

Expected: success. `buf.gen.yaml` uses `clean: true`, so each output directory is rewritten.

If this fails with a BSR rate limit, `ts/packages/paigasus-proto/src/generated/.../error_details_pb.ts`
may be **deleted** rather than stale, which reds `ts:lint` across the whole workspace. Restore that
one path from git; do not debug the lint error.

- [ ] **Step 6: Confirm the generated symbols exist**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-632-whoami
grep -rn "WhoAmIResponse" rs/crates/libs/paigasus-proto/src/generated/ | head -5
grep -rn "whoAmI" ts/packages/paigasus-proto/src/generated/ | head -5
```

Expected: both print matches. The Rust output must contain `pub struct WhoAmIResponse` and an
`async fn who_am_i` in the `AuthnService` trait.

- [ ] **Step 7: Verify the workspace still builds**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo build --workspace
```

Expected: **this FAILS**, with `not all trait items implemented, missing: who_am_i` for
`AuthnGrpc` in `adapters/grpc/authn.rs`. That is correct and expected — adding an RPC to a tonic
service makes the trait impl incomplete until Task 4 writes the handler. Record the error text;
do not try to fix it here.

- [ ] **Step 8: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-632-whoami
git add contracts/ rs/crates/libs/paigasus-proto/src/generated/ py/ ts/packages/paigasus-proto/src/generated/
git commit -F - <<'EOF'
feat(contracts): add AuthnService.WhoAmI and its messages (SMA-632)

A bearer-enforced RPC that describes the caller. WhoAmIResponse is a
separate message from IntrospectResponse because buf lint STANDARD's
RPC_REQUEST_RESPONSE_UNIQUE forbids one message serving two RPCs.

The Rust build is intentionally red after this commit: the AuthnService
trait now has an unimplemented method, which the next commits fill in.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
```

---

## Task 2: One membership-paging helper

**Files:**
- Create: `rs/crates/services/paigasus-iam/src/application/principal_context.rs`
- Modify: `rs/crates/services/paigasus-iam/src/application/mod.rs` (add `pub mod principal_context;` in alphabetical order, between `pagination` and `policies`)
- Modify: `rs/crates/services/paigasus-iam/src/application/authenticate_token.rs:49` (delete the constant) and `:149-159` (delete the loop)
- Modify: `rs/crates/services/paigasus-iam/src/application/authenticate_api_key.rs:44-46` (delete the constant) and `:264-274` (delete the loop)
- Test: inline `#[cfg(test)] mod tests` in the new file

**Interfaces:**
- Consumes: `paigasus_iam_core::{AuthnError, MembershipRecord, MembershipRepository, PrincipalId, RepositoryError}`.
- Produces: `pub(crate) async fn load_all_memberships<M: MembershipRepository>(memberships: &M, principal: &PrincipalId) -> Result<Vec<MembershipRecord>, AuthnError>`. Tasks 3 and 4 call it.

- [ ] **Step 1: Write the failing tests**

Create `rs/crates/services/paigasus-iam/src/application/principal_context.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0

//! The membership half of a `PrincipalContext`, in ONE place.
//!
//! `AuthenticateToken::introspect` and `AuthenticateApiKey::introspect` used to run
//! byte-identical paging loops, each with its own `MEMBERSHIP_PAGE_SIZE` constant, and the
//! api-key copy's own comment admitted the duplication. SMA-632 needed the same loop a third
//! time (for `WhoAmI`), so the loop moved here instead.
//!
//! D13's rule is unchanged and now enforceable: this function is the only place that fetches a
//! principal's memberships for an authn context.

use paigasus_iam_core::{AuthnError, MembershipRecord, MembershipRepository, PrincipalId, RepositoryError};

/// `list_by_principal` page size for introspection's membership assembly (§6.1).
const MEMBERSHIP_PAGE_SIZE: u64 = 200;

/// Wraps any `RepositoryError` as `AuthnError::Backend` — the catch-all for repository
/// failures the authn use cases don't specifically interpret (§6.2 rule 4).
fn backend(err: RepositoryError) -> AuthnError {
    AuthnError::Backend(Box::new(err))
}

/// Every membership row for `principal`, paged internally.
///
/// Pages until a short page arrives. A page of exactly `MEMBERSHIP_PAGE_SIZE` rows is followed
/// by another request, so a principal with exactly 200 memberships costs two calls — that is the
/// cost of not being able to distinguish "full page, more to come" from "full page, that's all".
pub(crate) async fn load_all_memberships<M>(memberships: &M, principal: &PrincipalId) -> Result<Vec<MembershipRecord>, AuthnError>
where
    M: MembershipRepository,
{
    let mut all = Vec::new();
    let mut offset = 0u64;
    loop {
        let page = memberships.list_by_principal(principal.uuid(), MEMBERSHIP_PAGE_SIZE, offset).await.map_err(backend)?;
        let page_len = page.len() as u64;
        all.extend(page);
        if page_len < MEMBERSHIP_PAGE_SIZE {
            break;
        }
        offset += MEMBERSHIP_PAGE_SIZE;
    }
    Ok(all)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::fakes::InMemoryMembershipRepository;

    /// Builds a repository holding `count` memberships for one principal, and returns both.
    async fn with_memberships(count: usize) -> (InMemoryMembershipRepository, PrincipalId) {
        let repo = InMemoryMembershipRepository::default();
        let principal = PrincipalId::from_uuid(uuid::Uuid::now_v7());
        repo.seed_for(&principal, count);
        (repo, principal)
    }

    #[tokio::test]
    async fn returns_nothing_for_a_principal_with_no_memberships() {
        let (repo, principal) = with_memberships(0).await;
        assert_eq!(load_all_memberships(&repo, &principal).await.unwrap().len(), 0);
    }

    #[tokio::test]
    async fn returns_a_single_membership() {
        let (repo, principal) = with_memberships(1).await;
        assert_eq!(load_all_memberships(&repo, &principal).await.unwrap().len(), 1);
    }

    /// Exactly one full page. The loop cannot tell this from "more to come", so it asks again
    /// and gets an empty second page. Both facts are asserted: the count, and the two calls.
    #[tokio::test]
    async fn returns_exactly_one_full_page_and_asks_once_more() {
        let (repo, principal) = with_memberships(200).await;
        assert_eq!(load_all_memberships(&repo, &principal).await.unwrap().len(), 200);
        assert_eq!(repo.list_call_count(), 2);
    }

    /// The case a single un-paged call would get wrong: 201 rows behind a 200-row page size.
    /// With the loop deleted this returns 200 and the test fails.
    #[tokio::test]
    async fn pages_past_the_first_page() {
        let (repo, principal) = with_memberships(201).await;
        assert_eq!(load_all_memberships(&repo, &principal).await.unwrap().len(), 201);
        assert_eq!(repo.list_call_count(), 2);
    }
}
```

- [ ] **Step 2: Add the module declaration**

In `rs/crates/services/paigasus-iam/src/application/mod.rs`, insert between `pub mod pagination;`
and `pub mod policies;`:

```rust
pub mod principal_context;
```

- [ ] **Step 3: Give the fakes module what the tests need**

The tests use `InMemoryMembershipRepository` with `seed_for` and `list_call_count`. Open
`rs/crates/services/paigasus-iam/src/application/fakes.rs` and check what already exists.

If a membership fake is already there, adapt the test code above to its real API rather than
adding a second fake — match the existing name and constructor exactly.

If one is not, add it. It must: implement `MembershipRepository`; honour `limit` and `offset` in
`list_by_principal` so the paging tests are meaningful; count `list_by_principal` calls behind an
`AtomicUsize`; and panic with `unimplemented!()` in every other trait method, so a future caller
that leans on an unimplemented path fails loudly. Model it on the existing fakes in that file.

- [ ] **Step 4: Run the tests to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam principal_context
```

Expected: FAIL. The whole crate does not compile yet (Task 1 left `who_am_i` unimplemented), so
the failure is a compile error naming `who_am_i`, not an assertion failure. That is the expected
state; do not add a stub handler to make it compile. Read the error, confirm it names
`who_am_i`, and continue.

- [ ] **Step 5: Delete the duplicate in `authenticate_token.rs`**

Delete the constant at `:48-49`:

```rust
/// `list_by_principal` page size for introspection's membership assembly (§6.1).
const MEMBERSHIP_PAGE_SIZE: u64 = 200;
```

Then replace the loop inside `introspect` (currently `:149-159`) so the whole method reads:

```rust
    /// Full authorization context for a request (§6.1): `resolve(.., Disabled)` (D10, never
    /// provisions) plus every membership row, paged by `principal_context` (D13 — that helper
    /// is the only place an authn context's memberships are fetched).
    pub async fn introspect(&self, token: &str) -> Result<PrincipalContext, AuthnError> {
        let principal = self.resolve(token, Provisioning::Disabled).await?;
        let memberships = crate::application::principal_context::load_all_memberships(&self.memberships, &principal.principal_id).await?;
        Ok(PrincipalContext {
            principal,
            memberships,
            role_grants: Vec::new(),
        })
    }
```

- [ ] **Step 6: Delete the duplicate in `authenticate_api_key.rs`**

Delete the constant at `:44-46` (the one whose comment says "identical to"). Then replace the
loop inside `introspect` (currently `:264-274`) so the method reads:

```rust
    /// Full authorization context for an API-key-authenticated request: `resolve` plus every
    /// membership row, paged by `principal_context::load_all_memberships` — the same helper
    /// `AuthenticateToken::introspect` uses, since SMA-632. `role_grants` stays empty (no
    /// current caller populates it from the `RoleGrantStore` here either).
    pub async fn introspect(&self, token: &str) -> Result<PrincipalContext, AuthnError> {
        let principal = self.resolve(token).await?;
        let memberships = crate::application::principal_context::load_all_memberships(&self.memberships, &principal.principal_id).await?;
        Ok(PrincipalContext {
            principal,
            memberships,
            role_grants: Vec::new(),
        })
    }
```

- [ ] **Step 7: Remove now-unused imports**

Both files may now have an unused `MembershipRepository` import or an unused `backend` function.
Under `warnings = deny` an unused import is a compile error. Compile and let the compiler tell
you:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo check -p paigasus-iam 2>&1 | grep -E "unused|never used" || echo "no unused-item errors"
```

Remove only what it names. `backend` is still used elsewhere in both files for non-membership
errors — do **not** delete it without checking.

- [ ] **Step 8: Prove the tests fail for the right reason, then pass**

The crate still cannot compile until Task 4. Run the helper's tests in isolation by temporarily
confirming with `cargo check`, then defer the green run to Task 4 Step 12.

Record here that Steps 1-7 are complete and the tests are written but not yet runnable.

- [ ] **Step 9: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-632-whoami
git add rs/crates/services/paigasus-iam/src/application/
git commit -F - <<'EOF'
refactor(rs): one membership-paging helper for the authn contexts (SMA-632)

AuthenticateToken::introspect and AuthenticateApiKey::introspect ran
byte-identical paging loops, each with its own page-size constant. WhoAmI
needs the same loop, so it moves to application/principal_context.rs
rather than becoming a third copy.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
```

---

## Task 3: `AuthenticateToken::context_for`

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/application/authenticate_token.rs` (add the method, have `introspect` delegate to it)

**Interfaces:**
- Consumes: `principal_context::load_all_memberships` from Task 2.
- Produces: `pub async fn context_for(&self, principal: AuthnPrincipal) -> Result<PrincipalContext, AuthnError>` on `AuthenticateToken<A, E, P, M, I, C>`. Task 4's gRPC handler and Task 5's HTTP handler both call it as `state.authn.context_for(principal)`.

- [ ] **Step 1: Add the method**

In `authenticate_token.rs`, directly above the existing `introspect` method, add:

```rust
    /// The full context for an ALREADY-RESOLVED principal — the bearer-enforced path's peer of
    /// `introspect`, which resolves a token first.
    ///
    /// `WhoAmI` (SMA-632) uses this. `AuthEnforce`/`require_bearer` have already verified the
    /// bearer, provisioned the caller and seeded the bootstrap grant by the time a handler runs,
    /// so re-verifying the token here would repeat a JWKS-backed verification and could answer
    /// differently from the middleware that let the request through.
    ///
    /// `role_grants` stays empty, exactly as in `introspect` — SMA-633 owns populating it.
    pub async fn context_for(&self, principal: AuthnPrincipal) -> Result<PrincipalContext, AuthnError> {
        let memberships = crate::application::principal_context::load_all_memberships(&self.memberships, &principal.principal_id).await?;
        Ok(PrincipalContext {
            principal,
            memberships,
            role_grants: Vec::new(),
        })
    }
```

- [ ] **Step 2: Have `introspect` delegate to it**

Replace the `introspect` body written in Task 2 Step 5 with:

```rust
    /// Full authorization context for a request (§6.1): `resolve(.., Disabled)` (D10, never
    /// provisions) plus `context_for`'s memberships.
    pub async fn introspect(&self, token: &str) -> Result<PrincipalContext, AuthnError> {
        let principal = self.resolve(token, Provisioning::Disabled).await?;
        self.context_for(principal).await
    }
```

Both callers now walk the same code, so `introspect` cannot drift from `WhoAmI`.

- [ ] **Step 3: Write the failing test**

Add to `authenticate_token.rs`'s existing `#[cfg(test)] mod tests`:

```rust
    /// `context_for` must NOT verify the token again — it takes an already-resolved principal.
    /// The fake authenticator here would panic if called, so a future implementation that
    /// re-resolves fails this test loudly rather than silently costing a JWKS round trip.
    #[tokio::test]
    async fn context_for_does_not_re_authenticate() {
        let (use_case, principal) = context_for_fixture().await;
        let ctx = use_case.context_for(principal.clone()).await.unwrap();
        assert_eq!(ctx.principal.principal_id, principal.principal_id);
        assert!(ctx.role_grants.is_empty());
    }
```

Write `context_for_fixture` alongside it, following the fixtures the existing tests in this
module already use. It must build an `AuthenticateToken` whose authenticator panics on
`authenticate`, seed one membership for the principal, and return both the use case and a
resolved `AuthnPrincipal`. Read the neighbouring tests first and match their construction style
exactly rather than inventing a new one.

Add a second test asserting the membership is present:

```rust
    #[tokio::test]
    async fn context_for_returns_the_principals_memberships() {
        let (use_case, principal) = context_for_fixture().await;
        let ctx = use_case.context_for(principal).await.unwrap();
        assert_eq!(ctx.memberships.len(), 1);
    }
```

- [ ] **Step 4: Verify the crate still does not compile, for the same one reason**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo check -p paigasus-iam 2>&1 | tail -20
```

Expected: the only remaining error names `who_am_i` on `AuthnGrpc`. If any other error appears,
fix it before continuing — it belongs to this task.

- [ ] **Step 5: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-632-whoami
git add rs/crates/services/paigasus-iam/src/application/authenticate_token.rs
git commit -F - <<'EOF'
feat(rs): AuthenticateToken::context_for for an already-resolved principal (SMA-632)

The bearer-enforced path's peer of introspect. The middleware has already
verified and provisioned the caller, so the WhoAmI handler must not
re-verify its token. introspect now delegates to it, so the two cannot
drift.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
```

---

## Task 4: `AuthContext`, the mapper and the gRPC handler

These land together **on purpose**. `warnings = deny` makes an unread field and an uncalled
function hard compile errors, so `AuthContext.kind`, `to_who_am_i_response` and the handler that
reads both cannot be split across commits.

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/auth.rs:15-22`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/grpc/authn.rs` (imports, `actor_context`, the handler, the `is_exempt` doc, the tests)
- Modify: `rs/crates/services/paigasus-iam/src/adapters/grpc/convert.rs` (the mapper and its tests)
- Modify: `rs/crates/services/paigasus-iam/src/adapters/http/auth_middleware.rs:69-72`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/http/authz_middleware.rs:198-203`

**Interfaces:**
- Consumes: `AuthenticateToken::context_for` (Task 3); `WhoAmIRequest`/`WhoAmIResponse` (Task 1).
- Produces: `convert::to_who_am_i_response(ctx: &PrincipalContext) -> WhoAmIResponse`, used again by nothing (the HTTP side has its own DTO). `AuthContext` with four fields, read by Task 5.

- [ ] **Step 1: Extend `AuthContext`**

In `rs/crates/services/paigasus-iam/src/adapters/auth.rs`, replace the struct and correct the
doc comment above it (which currently says the field set is "deliberately fixed"):

```rust
/// The bearer-resolved caller, attached to a request's extensions by the gRPC `AuthEnforce`
/// layer and by the HTTP `require_bearer` middleware. Both attach the SAME shape, which is what
/// "fixed" means here — a field added for one transport must be filled by both.
///
/// `kind` and `status` were added by SMA-632: `WhoAmI` returns the caller's principal, and
/// `WhoAmIResponse.status` needs the status. Both middlewares already hold a full
/// `AuthnPrincipal` and used to discard these two fields, so carrying them costs no extra query.
pub struct AuthContext {
    pub principal_id: PrincipalId,
    /// Carried so a handler can rebuild a complete `AuthnPrincipal`. No mapper reads it
    /// directly — do not delete it as unused without checking `grpc::authn::who_am_i`.
    pub kind: PrincipalKind,
    pub status: PrincipalStatus,
    pub credential: Credential,
}
```

Add `PrincipalKind` and `PrincipalStatus` to this file's `paigasus_iam_core` import list.

- [ ] **Step 2: Fill the two new fields at all three construction sites**

`adapters/grpc/authn.rs`, currently `:191-194` — replace the `insert` with:

```rust
                    req.extensions_mut().insert(AuthContext {
                        principal_id: principal.principal_id,
                        kind: principal.kind,
                        status: principal.status,
                        credential: principal.credential,
                    });
```

`adapters/http/auth_middleware.rs`, currently `:69-72` — the identical change:

```rust
            request.extensions_mut().insert(AuthContext {
                principal_id: principal.principal_id,
                kind: principal.kind,
                status: principal.status,
                credential: principal.credential,
            });
```

`adapters/http/authz_middleware.rs`, the test helper at `:198-203` — add the two fields. Use
`PrincipalKind::User` and `PrincipalStatus::Active`; this helper builds a synthetic context for
authorization tests, and neither field affects what it tests.

- [ ] **Step 3: Write the failing mapper tests**

Add to `adapters/grpc/convert.rs`'s existing `#[cfg(test)] mod tests`:

```rust
    /// An API-key bearer is reachable here — AuthEnforce accepts one — so unlike
    /// `to_introspect_response`, this mapper has no `debug_assert!` on the ApiKey arm.
    #[test]
    fn to_who_am_i_response_leaves_issuer_and_subject_empty_for_an_api_key() {
        let expiry = Utc::now() + chrono::Duration::hours(1);
        let ctx = api_key_context(Some(expiry));
        let response = to_who_am_i_response(&ctx);
        assert_eq!(response.issuer, "");
        assert_eq!(response.subject, "");
        assert_eq!(response.expires_at, Some(ts(expiry)));
    }

    /// An API key may be minted with no expiry (`Credential::ApiKey.expires_at` is an Option).
    /// The old shared mapper answered `Utc::now()` here, which is a fabricated value.
    #[test]
    fn to_who_am_i_response_omits_an_absent_expiry() {
        let ctx = api_key_context(None);
        assert_eq!(to_who_am_i_response(&ctx).expires_at, None);
    }

    #[test]
    fn to_who_am_i_response_reports_an_oidc_caller_in_full() {
        let ctx = oidc_context();
        let response = to_who_am_i_response(&ctx);
        assert!(!response.issuer.is_empty());
        assert!(!response.subject.is_empty());
        assert!(response.expires_at.is_some());
    }
```

Write `api_key_context(expires_at: Option<DateTime<Utc>>) -> PrincipalContext` and
`oidc_context() -> PrincipalContext` next to them. Read the fixtures the existing
`to_introspect_response` tests in this module already use and reuse or extend those rather than
inventing new ones.

- [ ] **Step 4: Run them to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam to_who_am_i 2>&1 | tail -20
```

Expected: a compile error naming `to_who_am_i_response` as not found (plus the standing
`who_am_i` trait error). Not an assertion failure.

- [ ] **Step 5: Write the mapper**

In `adapters/grpc/convert.rs`, directly after the existing `to_introspect_response`, add:

```rust
/// `WhoAmIResponse` for an already-resolved caller.
///
/// Deliberately NOT `to_introspect_response`. That function asserts its `ApiKey` arm is
/// unreachable, which is true of `Introspect` (it resolves only through the OIDC authenticator)
/// and false of `WhoAmI` (`AuthEnforce` accepts an API-key bearer). Keeping two mappers lets
/// `Introspect` keep its guarantee instead of loosening it for a second caller's sake.
pub fn to_who_am_i_response(ctx: &PrincipalContext) -> WhoAmIResponse {
    let (issuer, subject) = match &ctx.principal.credential {
        Credential::Oidc { issuer, subject, .. } => (issuer.as_str().to_string(), subject.clone()),
        // A service account has no (issuer, subject) pair, and this response carries no
        // key_id/scope_prn — those belong to IntrospectApiKeyResponse.
        Credential::ApiKey { .. } => (String::new(), String::new()),
    };
    WhoAmIResponse {
        principal_prn: ctx.principal.principal_id.canonical(),
        status: ctx.principal.status.as_str().to_string(),
        issuer,
        subject,
        // Option, not a fabricated `Utc::now()`: an API key may have been minted without an
        // expiry. `AuthnPrincipal::expires_at()` already answers for both credential kinds.
        expires_at: ctx.principal.expires_at().map(ts),
        memberships: ctx.memberships.iter().map(to_proto_membership).collect(),
        role_grants: ctx.role_grants.iter().map(to_proto_role_grant_ref).collect(),
    }
}
```

Add `WhoAmIResponse` to this file's `paigasus_proto::paigasus::iam::v1::{...}` import list.

- [ ] **Step 6: Add `actor_context` to the gRPC authn module**

`grpc/authn.rs` has no such helper, because neither of its RPCs was bearer-enforced. Add it
directly below the `impl AuthnGrpc` block, matching the six existing copies verbatim in body:

```rust
/// Extracts the bearer-resolved [`AuthContext`] from a gRPC request's extensions — mirrors
/// the identical private helper in `grpc::tenancy`/`grpc::users`/`grpc::authz`/`grpc::audit`/
/// `grpc::service_accounts`/`grpc::dead_letters`. `WhoAmI` is the first bearer-enforced RPC on
/// this service, which is why this module did not need one before SMA-632.
fn actor_context<T>(request: &Request<T>) -> Result<AuthContext, Status> {
    request.extensions().get::<AuthContext>().cloned().ok_or_else(convert::missing_auth_context)
}
```

- [ ] **Step 7: Write the handler**

Add as the third method of `impl AuthnService for AuthnGrpc`, after `introspect_api_key`:

```rust
    /// `WhoAmI` (SMA-632): the caller's own principal.
    ///
    /// BEARER-ENFORCED BY OMISSION. This RPC is deliberately absent from `is_exempt` below, so
    /// `AuthEnforce` has already verified the bearer, JIT-provisioned an unknown identity (when
    /// the issuer's JIT flag allows it) and seeded the bootstrap platform_admin grant before
    /// this runs. That omission is the whole feature — `who_am_i_is_not_exempt` pins it.
    ///
    /// The handler never sees a token. It reads the `AuthContext` the middleware inserted and
    /// asks for the memberships, so it repeats no verification work.
    async fn who_am_i(&self, request: Request<WhoAmIRequest>) -> Result<Response<WhoAmIResponse>, Status> {
        let started = Instant::now();
        let result: Result<Response<WhoAmIResponse>, Status> = async {
            let actor = actor_context(&request)?;
            let principal = AuthnPrincipal {
                principal_id: actor.principal_id,
                kind: actor.kind,
                status: actor.status,
                credential: actor.credential,
            };
            let ctx = self.state.authn.context_for(principal).await.map_err(|e| convert::authn_status(&e))?;
            Ok(Response::new(convert::to_who_am_i_response(&ctx)))
        }
        .await;
        record_grpc("Authentication", "WhoAmI", started, &result);
        result
    }
```

Extend this file's imports: add `AuthnPrincipal` to the `paigasus_iam_core::{...}` list, and
`WhoAmIRequest, WhoAmIResponse` to the `paigasus_proto::paigasus::iam::v1::{...}` list.

- [ ] **Step 8: Update the `is_exempt` doc comment**

The body at `:139-141` does **not** change. The doc above it, currently `:120-138`, gets a new
paragraph — § 6.3 of the spec relies on this reasoning being written down:

```rust
//! `WhoAmI` is deliberately ABSENT from this list, and that absence is a feature, not an
//! oversight. It is the RPC the consoles call to get provisioned: being enforced is what makes
//! `AuthEnforce` resolve its bearer with `Provisioning::Enabled` and seed the bootstrap grant.
//! Adding it here would silently break console login. `who_am_i_is_not_exempt` fails if anyone
//! does, and `who_am_i_provisions_a_new_identity` (tests/grpc_whoami.rs) fails for a second,
//! independent reason.
```

- [ ] **Step 9: Write the exemption test**

Add to `grpc/authn.rs`'s existing `#[cfg(test)] mod tests`:

```rust
    /// SMA-632's mechanism, asserted directly. WhoAmI provisions its caller ONLY because
    /// enforcement covers it; adding it to `is_exempt` would make console login fail with
    /// `identity-not-provisioned` for every new user.
    #[test]
    fn who_am_i_is_not_exempt() {
        assert!(!is_exempt("/paigasus.iam.v1.AuthnService/WhoAmI"));
    }

    /// The two RPCs that ARE exempt stay exempt — this test fails if someone "fixes" the one
    /// above by widening the predicate rather than leaving WhoAmI out of it.
    #[test]
    fn the_two_introspection_rpcs_are_still_exempt() {
        assert!(is_exempt("/paigasus.iam.v1.AuthnService/Introspect"));
        assert!(is_exempt("/paigasus.iam.v1.AuthnService/IntrospectApiKey"));
    }
```

- [ ] **Step 10: Build**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo build --workspace
```

Expected: SUCCESS. This is the first green build since Task 1. If `AuthContext.kind` is reported
as never read, the handler in Step 7 is not reading it — re-check that it builds the
`AuthnPrincipal` from all four fields.

- [ ] **Step 11: Lint and format**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo clippy --workspace -- -D warnings && cargo fmt --check
```

Expected: both clean. If `cargo fmt --check` reports a diff in a file you only renamed an
identifier in, that is expected — a longer identifier reflows past `max_width`. Run `cargo fmt`
and re-check.

- [ ] **Step 12: Run every test written so far**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam
```

Expected: PASS, including Task 2's four `principal_context` tests, Task 3's two `context_for`
tests, and this task's three mapper tests and two exemption tests. Confirm all eleven appear in
the output by name — a test that silently did not run proves nothing.

- [ ] **Step 13: Prove the mapper tests are not vacuous**

Temporarily change `to_who_am_i_response`'s `expires_at` line to `expires_at: None,` and re-run:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam to_who_am_i
```

Expected: `to_who_am_i_response_leaves_issuer_and_subject_empty_for_an_api_key` and
`to_who_am_i_response_reports_an_oidc_caller_in_full` both FAIL. If they pass, they assert
nothing about the expiry and must be strengthened.

Restore the line by editing it back — **do not** use `git checkout --`, which would also discard
the rest of this task's uncommitted work.

- [ ] **Step 14: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-632-whoami
git add rs/crates/services/paigasus-iam/src/adapters/
git commit -F - <<'EOF'
feat(rs): the WhoAmI gRPC handler, its mapper and the AuthContext fields (SMA-632)

AuthContext carries kind and status, which both middlewares already held
and discarded. The handler rebuilds an AuthnPrincipal from it and asks
context_for for the memberships, so it re-verifies nothing.

to_who_am_i_response is a separate mapper rather than a shared one:
Introspect resolves only OIDC and keeps its debug_assert, while WhoAmI
must answer an API-key bearer honestly, including an absent expiry.

is_exempt is unchanged. Two tests pin that omission.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
```

---

## Task 5: The HTTP twin

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/http/dto.rs` (add `WhoAmIResponseDto`)
- Modify: `rs/crates/services/paigasus-iam/src/adapters/http/authn.rs` (add `whoami_router` and the handler)
- Modify: `rs/crates/services/paigasus-iam/src/adapters/http/mod.rs:912-958` (mount it) and the tests at the bottom
- Test: inline tests in `dto.rs` and `mod.rs`

**Interfaces:**
- Consumes: `AuthContext` (Task 4), `AuthenticateToken::context_for` (Task 3).
- Produces: `POST /v1/authn/whoami`. Task 6's parity test calls it.

- [ ] **Step 1: Write the failing DTO test**

Add to `adapters/http/dto.rs`'s `#[cfg(test)] mod tests`:

```rust
    /// An API key minted with no expiry must produce NO `expires_at` key — not `null`, and not
    /// a fabricated timestamp. `IntrospectResponseDto` cannot express this, which is why
    /// WhoAmI has its own DTO.
    #[test]
    fn who_am_i_response_dto_omits_an_absent_expiry() {
        let dto = WhoAmIResponseDto::from(api_key_context(None));
        let json = serde_json::to_value(&dto).unwrap();
        assert!(json.get("expires_at").is_none(), "expected no expires_at key, got {json}");
    }

    #[test]
    fn who_am_i_response_dto_reports_an_api_key_expiry_when_there_is_one() {
        let expiry = Utc::now() + chrono::Duration::hours(1);
        let dto = WhoAmIResponseDto::from(api_key_context(Some(expiry)));
        assert_eq!(dto.expires_at, Some(expiry));
        assert_eq!(dto.issuer, "");
        assert_eq!(dto.subject, "");
    }
```

Write `api_key_context` in this module following the fixtures its neighbouring
`IntrospectResponseDto` tests already use.

- [ ] **Step 2: Run to verify failure**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam who_am_i_response_dto 2>&1 | tail -15
```

Expected: a compile error naming `WhoAmIResponseDto` as not found.

- [ ] **Step 3: Write the DTO**

In `adapters/http/dto.rs`, directly after `IntrospectResponseDto` and its `From` impl, add:

```rust
/// `WhoAmIResponse`-shaped JSON: mirrors proto `paigasus.iam.v1.WhoAmIResponse` field-for-field.
///
/// Separate from [`IntrospectResponseDto`] for the same reason the proto messages are separate:
/// this one must express an API-key caller, whose `issuer`/`subject` are empty and whose expiry
/// may be absent. `IntrospectResponseDto` keeps its non-optional `expires_at`, so
/// `POST /v1/authn/introspect`'s response shape is unchanged.
#[derive(Debug, Clone, Serialize)]
pub struct WhoAmIResponseDto {
    pub principal_prn: String,
    pub status: String,
    pub issuer: String,
    pub subject: String,
    /// Absent, not null: a key minted without an expiry has none to report.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
    pub memberships: Vec<MembershipDto>,
    pub role_grants: Vec<RoleGrantRefDto>,
}

impl From<PrincipalContext> for WhoAmIResponseDto {
    fn from(ctx: PrincipalContext) -> Self {
        let expires_at = ctx.principal.expires_at();
        let principal = ctx.principal;
        let (issuer, subject) = match principal.credential {
            Credential::Oidc { issuer, subject, .. } => (issuer.as_str().to_string(), subject),
            // A service account has no (issuer, subject) pair. No debug_assert here: unlike
            // Introspect's, this arm IS reachable — AuthEnforce accepts an API-key bearer.
            Credential::ApiKey { .. } => (String::new(), String::new()),
        };
        WhoAmIResponseDto {
            principal_prn: principal.principal_id.canonical(),
            status: principal.status.as_str().to_string(),
            issuer,
            subject,
            expires_at,
            memberships: ctx.memberships.into_iter().map(MembershipDto::from).collect(),
            role_grants: ctx.role_grants.into_iter().map(RoleGrantRefDto::from).collect(),
        }
    }
}
```

- [ ] **Step 4: Run to verify the DTO tests pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam who_am_i_response_dto
```

Expected: both PASS.

- [ ] **Step 5: Add the router and handler**

In `adapters/http/authn.rs`, after the existing `router` function, add:

```rust
/// The `whoami` sub-router. Unlike [`router`] above, this one is merged INSIDE `app_routes`'s
/// `protected` group, so `require_bearer` covers it — which is the point: enforcement is what
/// provisions the caller.
///
/// POST, not GET (SMA-632 D7): the call creates a principal row and a user row, and for a
/// configured bootstrap identity it seeds a platform_admin grant. RFC 9110 makes GET a safe
/// method, and browsers, proxies and Next.js prefetch GETs freely.
pub fn whoami_router() -> Router<AppState> {
    Router::new().route("/v1/authn/whoami", post(whoami))
}

/// `POST /v1/authn/whoami`: the caller's own principal, the HTTP twin of the WhoAmI RPC.
///
/// `require_bearer` has already resolved, provisioned and seeded by the time this runs, and it
/// inserted the `AuthContext` this reads. The extension cannot be absent inside `protected`, but
/// the `Option` keeps the failure inside the `{"error":{code,message}}` envelope instead of
/// axum's plain-text extension rejection.
async fn whoami(State(state): State<AppState>, actor: Option<Extension<AuthContext>>) -> Result<Json<WhoAmIResponseDto>, AuthnApiError> {
    let Some(Extension(actor)) = actor else {
        return Err(AuthnApiError(AuthnError::InvalidToken(TokenDefect::Malformed)));
    };
    let principal = AuthnPrincipal {
        principal_id: actor.principal_id,
        kind: actor.kind,
        status: actor.status,
        credential: actor.credential,
    };
    let ctx = state.authn.context_for(principal).await?;
    Ok(Json(ctx.into()))
}
```

Extend this file's imports: `axum::Extension`, `paigasus_iam_core::{AuthnPrincipal, TokenDefect}`,
`crate::adapters::auth::AuthContext`, and `WhoAmIResponseDto` from `super::dto`.

- [ ] **Step 6: Mount it inside `protected`**

In `adapters/http/mod.rs`'s `app_routes` (currently `:914-923`), add one line to the initial
`protected` chain, after `service_info::router()`:

```rust
        // The descriptor itself is always mounted and always inside the bearer layer (SMA-505).
        .merge(service_info::router())
        // WhoAmI is always mounted too, and for the same reason it is bearer-enforced: being
        // inside this layer is what provisions the caller (SMA-632 D11 — no capability gate).
        .merge(authn::whoami_router());
```

Note the `;` moves from the `service_info` line to the new last line.

- [ ] **Step 7: Add the new route to the existing route-conflict test**

The test `protected_router_merge_has_no_path_conflicts_in_any_capability_combination`
(`adapters/http/mod.rs:1033`) reproduces `app_routes`'s `protected` chain for all eight
capability combinations. Add `.merge(authn::whoami_router())` to its reproduction, in the same
position it holds in `app_routes`. If the two chains diverge, the test stops reproducing what it
claims to reproduce.

- [ ] **Step 8: Write the missing outer-merge test**

The existing test covers only `protected`. It does not reproduce `app_routes`'s final
`Router::new().merge(protected).merge(authn_api).merge(api_key_introspect_api)` at `:947-950`,
where a `/v1/authn/*` route inside `protected` meets `authn::router`'s routes outside it. Add,
next to the existing test:

```rust
    /// SMA-632 put `/v1/authn/whoami` INSIDE `protected` while `/v1/authn/introspect` stays
    /// OUTSIDE it. axum panics at REGISTRATION time on a pattern conflict, so this reproduces
    /// `app_routes`'s outer merge — which the test above does not cover — and fails here rather
    /// than at the first request in production.
    #[test]
    fn outer_router_merge_has_no_path_conflicts() {
        let protected: Router<AppState> = Router::new().merge(service_info::router()).merge(authn::whoami_router());
        let authn_api: Router<AppState> = authn::router(4096);
        let api_key_introspect_api: Router<AppState> = api_keys::introspect_router(4096);
        // The merge itself is the assertion: a conflicting pair panics here.
        let _merged: Router<AppState> = Router::new().merge(protected).merge(authn_api).merge(api_key_introspect_api);
    }
```

- [ ] **Step 9: Run the router tests**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam path_conflicts
```

Expected: both tests PASS.

- [ ] **Step 10: Prove the outer-merge test is not vacuous**

Temporarily change `whoami_router`'s route to `"/v1/authn/introspect"` and re-run Step 9.

Expected: `outer_router_merge_has_no_path_conflicts` FAILS with an axum registration panic. If it
passes, the test is not exercising the merge and must be fixed.

Restore the path by editing it back. Do not use `git checkout --`.

- [ ] **Step 11: Full crate check**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo clippy --workspace -- -D warnings && cargo fmt --check && cargo nextest run -p paigasus-iam
```

Expected: all clean, all tests pass.

- [ ] **Step 12: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-632-whoami
git add rs/crates/services/paigasus-iam/src/adapters/http/
git commit -F - <<'EOF'
feat(rs): POST /v1/authn/whoami, the HTTP twin of the WhoAmI RPC (SMA-632)

Mounted inside the bearer layer, unlike POST /v1/authn/introspect, which
is merged outside it. POST rather than GET because the call provisions a
user and can seed a platform_admin grant, and RFC 9110 makes GET safe.

WhoAmIResponseDto is separate from IntrospectResponseDto so an absent
API-key expiry is an absent key. The introspect response shape does not
change.

A new test reproduces the outer router merge, which the existing
conflict test never covered.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
```

---

## Task 6: Integration tests

**Files:**
- Create: `rs/crates/services/paigasus-iam/tests/grpc_whoami.rs`

**Interfaces:**
- Consumes: everything from Tasks 1-5, plus `tests/support/`'s `start_migrated_postgres`, `start_mock_idp`, `test_config`, `grpc_bearer`, `send`.
- Produces: nothing.

- [ ] **Step 1: Read the template before writing anything**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-632-whoami
sed -n '1,140p' rs/crates/services/paigasus-iam/tests/grpc_service_info.rs
sed -n '1,80p' rs/crates/services/paigasus-iam/tests/authz_bootstrap_admin.rs
```

`grpc_service_info.rs` gives the `spawn_server` + `authed` + Docker-skip shape and the HTTP/gRPC
parity test. `authz_bootstrap_admin.rs` gives the bootstrap-admin configuration shape. Copy
their structure exactly; do not invent a new harness.

- [ ] **Step 2: Write the test file**

Create `rs/crates/services/paigasus-iam/tests/grpc_whoami.rs`. It opens with the SPDX header and
a module doc explaining what it pins, then defines `spawn_server` and `authed` exactly as
`grpc_service_info.rs` does, then these seven tests. Every one begins with the standard skip
guard `let Some((_node, db)) = support::start_migrated_postgres().await else { return; };`.

**Test 1 — enforcement, asserted by reason, not by code:**

```rust
/// The bearer test that MUST assert the reason. `missing-auth-context` (what the handler answers
/// when the extension is absent) and enforcement's own rejection are BOTH `Unauthenticated`, so
/// a code-only assertion would pass even with WhoAmI added to `is_exempt` — which is exactly the
/// regression this file exists to catch.
#[tokio::test]
async fn who_am_i_without_a_bearer_is_rejected_by_enforcement() {
    // ... harness setup ...
    let status = client.who_am_i(tonic::Request::new(WhoAmIRequest {})).await.unwrap_err();
    assert_eq!(status.code(), tonic::Code::Unauthenticated);
    assert_eq!(reason_of(&status), "invalid-token", "expected ENFORCEMENT's rejection, not the handler's missing-auth-context");
}
```

Write `reason_of(&Status) -> String`, which decodes the `google.rpc.ErrorInfo` from the
`grpc-status-details-bin` trailer. Check whether `tests/support/` already has such a helper and
reuse it; only write one if none exists.

**Test 2 — the issue's own claim, and the primary control:**

```rust
/// SMA-632's whole point. A token for an identity IAM has never seen:
///   1. Introspect refuses it — the exempt, non-provisioning path.
///   2. ONE WhoAmI call succeeds and names a principal.
///   3. Introspect now succeeds, because step 2 provisioned.
/// No GetServiceInfo call appears anywhere. If anyone adds WhoAmI to `is_exempt`, step 2 fails:
/// AuthEnforce forwards without an AuthContext and the handler answers missing-auth-context.
#[tokio::test]
async fn who_am_i_provisions_a_new_identity() { /* ... */ }
```

**Test 3 — the JIT precondition:**

```rust
/// Bearer enforcement is necessary but not sufficient. `resolve` still checks the issuer's JIT
/// flag under Provisioning::Enabled (authenticate_token.rs:107-109), so an issuer with JIT off
/// gets the same `identity-not-provisioned` Introspect gives. The console keeps a branch for it.
#[tokio::test]
async fn who_am_i_obeys_the_jit_policy() { /* ... */ }
```

Configure the mock IdP's issuer with `jit_provisioning: false` via `test_config`.

**Test 4 — memberships:**

```rust
/// The memberships a console renders from. Attach one, then read it back through WhoAmI.
#[tokio::test]
async fn who_am_i_returns_memberships() { /* ... */ }
```

**Test 5 — bootstrap seeding:**

```rust
/// One WhoAmI call is enough for a configured bootstrap identity to hold platform_admin — the
/// second side effect the consoles used to get from GetServiceInfo.
#[tokio::test]
async fn who_am_i_seeds_the_bootstrap_admin() { /* ... */ }
```

Assert through `state.role_grant_store.list_by_principal(..)`, following
`tests/authz_bootstrap_admin.rs`.

**Test 6 — the API-key bearer:**

```rust
/// D4: WhoAmI serves both credential kinds. A service account's key names its principal, and
/// issuer/subject are empty — it has no (issuer, subject) pair.
#[tokio::test]
async fn who_am_i_serves_an_api_key_bearer() { /* ... */ }
```

Follow `tests/api_keys_grpc.rs::grpc_issue_and_introspect_parity` for creating a service account
and issuing a key.

**Test 7 — transport parity:**

```rust
/// The two transports describe the same principal, per the service's parity convention.
/// POST, not GET (D7).
#[tokio::test]
async fn who_am_i_grpc_http_parity() { /* ... */ }
```

- [ ] **Step 3: Run them with Docker required, so a skip is a failure**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test grpc_whoami
```

Expected: 7 passed. Without `PAIGASUS_REQUIRE_DOCKER=1` these skip silently when Docker is
unreachable, and a green run would prove nothing.

If Docker is unreachable on this machine, stop and report that — do **not** record the tests as
passing.

- [ ] **Step 4: Prove the primary control actually controls**

Temporarily add `|| path == "/paigasus.iam.v1.AuthnService/WhoAmI"` to `is_exempt` in
`adapters/grpc/authn.rs` and re-run:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test grpc_whoami
cd rs && cargo nextest run -p paigasus-iam who_am_i_is_not_exempt
```

Expected:
- `who_am_i_is_not_exempt` FAILS.
- `who_am_i_provisions_a_new_identity` FAILS.
- `who_am_i_without_a_bearer_is_rejected_by_enforcement` FAILS (the reason becomes
  `missing-auth-context`).

Three independent failures. If fewer than three fail, the tests that passed are weaker than the
spec claims — strengthen them before continuing.

Restore `is_exempt` by editing the line back out. Do not use `git checkout --`.

- [ ] **Step 5: Re-run the whole battery after restoring**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam
```

Expected: everything passes. Re-running the whole suite after a mutation matters — a previous
issue in this repo found a mutation that went inert because a later edit changed its context.

- [ ] **Step 6: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-632-whoami
git add rs/crates/services/paigasus-iam/tests/grpc_whoami.rs
git commit -F - <<'EOF'
test(rs): integration tests for the WhoAmI RPC (SMA-632)

who_am_i_provisions_a_new_identity is the control the repo lacked: before
this, nothing failed when provisioning broke. Measured by adding WhoAmI to
is_exempt, which fails three tests independently.

The bearer test asserts the ErrorInfo reason, not the gRPC code. Both
enforcement's rejection and the handler's missing-auth-context are
Unauthenticated, so a code-only assertion would pass while exempt.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
```

---

## Task 7: The fake IAM server and the dev world

**Files:**
- Modify: `ts/packages/paigasus-console-core/testing/fake-iam.ts:147-148` (`UNENFORCED` comment), `:233-249` (add a `whoAmI` helper), `:270-282` (`dispatch`)
- Modify: `ts/packages/paigasus-console-core/testing/dev-world.ts:165-178`
- Modify: `ts/packages/paigasus-console-core/tests/unit/dev-world.test.ts:48,55`

**Interfaces:**
- Consumes: the generated `AuthnService.method.whoAmI` from Task 1.
- Produces: the fake answers `authn.whoAmI`. Tasks 8-10 rely on it.

- [ ] **Step 1: Confirm the method appears automatically**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-632-whoami
grep -rn "whoAmI" ts/packages/paigasus-proto/src/generated/ | head -3
```

Expected: matches. `FakeIamMethod` is derived from the service descriptors
(`fake-iam.ts:64-88`), so `authn.whoAmI` is already a valid key with no hand edit. Do not add it
to a list by hand.

- [ ] **Step 2: Add the `whoAmI` answer**

In `fake-iam.ts`, directly after the existing `introspect` helper (`:233-249`), add:

```ts
  /**
   * WhoAmI's answer. Two differences from `introspect` above, both load-bearing:
   *
   * 1. It keys on `context.token`, NOT `request.token`. `WhoAmIRequest` is EMPTY — the bearer
   *    header is the only input — so reading `request.token` yields `undefined` and every
   *    answer would name a principal for the token `undefined`.
   * 2. It never checks `provisioned`. `dispatch` adds the token to that set before it gets here,
   *    because WhoAmI is not in UNENFORCED. That IS the behaviour under test: a bearer-enforced
   *    call provisions its caller, so WhoAmI never answers identity-not-provisioned.
   */
  async function whoAmI(context: FakeIamContext): Promise<unknown> {
    const token = context.token ?? '';
    const handler = scripted('authn.whoAmI');
    const extra = handler === undefined ? {} : ((await handler({}, context)) as object);
    return {
      principalPrn: principalPrnFor(token),
      status: 'active',
      issuer: FAKE_IAM_ISSUER,
      subject: `subject-of-${principalPrnFor(token).slice(-12)}`,
      memberships: [],
      ...extra,
      roleGrants: [],
    };
  }
```

`principalPrnFor(token)` is the same memoized map `introspect` uses, so the two RPCs name the
same principal for one token. A test in Task 9 asserts that.

- [ ] **Step 3: Route it in `dispatch`**

In `dispatch` (`:278`), add a branch next to the existing `authn.introspect` one:

```ts
      if (method === 'authn.introspect') return await introspect(request as { token: string }, context);
      if (method === 'authn.whoAmI') return await whoAmI(context);
```

Leave `UNENFORCED` alone — `authn.whoAmI` must **not** be in it. Extend its comment:

```ts
/**
 * Calls IAM serves WITHOUT bearer enforcement (authn.rs:139-141).
 *
 * `authn.whoAmI` is deliberately NOT here: enforcement is what provisions its caller, which is
 * the whole of SMA-632. Adding it would make the fake diverge from IAM and hide the regression.
 */
const UNENFORCED: ReadonlySet<string> = new Set(['authn.introspect', 'authn.introspectApiKey']);
```

- [ ] **Step 4: Script `authn.whoAmI` in the dev world**

`dev-world.ts:166-175` scripts `authn.introspect` with the dev principal's PRN and its two
memberships. Without a matching `whoAmI` handler, every dev and e2e scenario falls back to the
fake's built-in answer — a random PRN with no memberships — and loses the dev principal's scopes.

Replace the `'authn.introspect'` entry with both, sharing one body so they cannot drift:

```ts
  const devPrincipal = () => ({
    principalPrn: PRINCIPAL_PRN,
    status: 'active',
    issuer: 'fake-idp',
    subject: 'dev-user',
    memberships: [
      { id: '0190a1d4-0000-7000-8000-00000000d101', principalPrn: PRINCIPAL_PRN, nodePrn: ORG_PRN },
      { id: '0190a1d4-0000-7000-8000-00000000d102', principalPrn: PRINCIPAL_PRN, nodePrn: TEAM_PRN },
    ],
  });

  return {
    // Both authn reads answer the SAME dev principal. The console calls whoAmI; introspect stays
    // scripted because IAM still serves it and a test may drive it directly.
    'authn.introspect': devPrincipal,
    'authn.whoAmI': devPrincipal,
    // ... the rest of the map unchanged ...
```

Place `devPrincipal` next to the other local helpers (`organizationAt`, `teamAt`, `projectAt`)
above the `return`.

- [ ] **Step 5: Update the unscripted-method assertion**

`tests/unit/dev-world.test.ts:48,55` asserts `Object.keys(handlers)` excludes
`serviceInfo.getServiceInfo`. That assertion stays correct and unchanged. Add a new one next to
it:

```ts
  it('scripts authn.whoAmI with the same dev principal as authn.introspect, so the console sees its scopes', () => {
    const handlers = devWorldHandlers();
    expect(Object.keys(handlers)).toContain('authn.whoAmI');
    expect(handlers['authn.whoAmI']?.({}, ctx)).toEqual(handlers['authn.introspect']?.({ token: 't' }, ctx));
  });
```

Match the existing test's way of obtaining `handlers` and a context value — read lines 40-60
first and follow it exactly.

- [ ] **Step 6: Run the fake's own tests**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-632-whoami
pnpm -C ts --filter @paigasus/console-core test
```

Expected: the `dev-world` unit tests pass, including the new one. Other suites in this package
still reference `introspectWithProvisioning` and will fail — that is Task 8's work. Confirm the
failures are only those, by name.

- [ ] **Step 7: Prove the new assertion is not vacuous**

Temporarily delete the `'authn.whoAmI': devPrincipal,` line from `dev-world.ts` and re-run
Step 6.

Expected: the new test FAILS on `toContain`. Restore the line by editing it back.

- [ ] **Step 8: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-632-whoami
git add ts/packages/paigasus-console-core/testing/ ts/packages/paigasus-console-core/tests/unit/dev-world.test.ts
git commit -F - <<'EOF'
test(ts): teach the fake IAM and the dev world to answer WhoAmI (SMA-632)

The answer keys on the context token, not the request token: WhoAmIRequest
is empty, so reading request.token would name a principal for undefined.
It never checks the provisioned set, because dispatch fills that set for
any enforced method — which is the behaviour under test.

dev-world scripts whoAmI with the same principal as introspect, or every
dev and e2e scenario loses the dev principal's memberships.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
```

---

## Task 8: `@paigasus/console-core` — one call instead of two

**Files:**
- Modify: `ts/packages/paigasus-console-core/src/principal.ts` (whole file)
- Modify: `ts/packages/paigasus-console-core/src/principal-resolver.ts:55-75`
- Modify: `ts/packages/paigasus-console-core/src/runtime.ts:99`
- Modify: `ts/packages/paigasus-console-core/src/index.ts:19`
- Modify: `ts/packages/paigasus-console-core/tests/integration/principal.test.ts` (whole file)

**Interfaces:**
- Consumes: the fake's `authn.whoAmI` (Task 7); the generated `clients.authn.whoAmI`.
- Produces: `export async function whoAmI(clients: Pick<IamClients, 'authn'>, opts?: { timeoutMs?: number }): Promise<IamResult<Principal>>`. `runtime.ts` and both apps' `lib/auth.ts` consume it.

- [ ] **Step 1: Rewrite the tests first**

In `tests/integration/principal.test.ts`, make these changes. The file header changes from the
two-call story to the one-call one:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// Provisioning (SMA-632), against the fake IAM over the real SDK transport:
//   - the login resolver makes ONE WhoAmI call, and never fails the login;
//   - whoAmI needs no retry, because a bearer-enforced call provisions its caller.
```

Replace the import of `introspectWithProvisioning` with `whoAmI`.

**The ordering tests become one-call tests.** At `:35-50`, the resolver test:

```ts
    it('makes one WhoAmI call and maps it, reporting grants as unknown', async () => {
      fake.setHandlers({ 'authn.whoAmI': (_req, ctx) => ({ memberships: [{ id: 'm-1', principalPrn: fake.principalPrnFor(ctx.token ?? ''), nodePrn: ORG }] }) });
      const { logger, events } = captureLogger();
      const resolver = createIntrospectPrincipalResolver({ clientsForToken: clientsFor, logger });
      const principal = await resolver.resolve({ accessToken: 'first-login', idTokenClaims: CLAIMS });
      expect(fake.calls.map((call) => call.method)).toEqual(['authn.whoAmI']);
      expect(principal).toEqual({
        principalPrn: fake.principalPrnFor('first-login'),
        issuer: FAKE_IAM_ISSUER,
        subject: expect.any(String) as string,
        memberships: [{ id: 'm-1', principalPrn: fake.principalPrnFor('first-login'), nodePrn: ORG }],
        roleGrants: [],
        grantsAvailable: false,
      });
      expect(events()).toEqual([]);
    });
```

**The degradation tests switch which method they sabotage.** At `:95` and `:106`, replace
`'serviceInfo.getServiceInfo'` with `'authn.whoAmI'` in both `setHandlers` calls. Their
assertions do not change — the resolver must still degrade rather than fail the login.

**The crashing-client stub at `:131` loses its `serviceInfo` member** and its `authn` member
gains `whoAmI` in place of `introspect`:

```ts
      const crashingClients: Pick<IamClients, 'authn'> = {
        authn: {
          whoAmI: () =>
            Promise.resolve({
              principalPrn: 'prn:pgs:iam:::principal/crash-test',
              issuer: FAKE_IAM_ISSUER,
              subject: 'subject-crash-test',
              get memberships(): never {
                throw new TypeError('memberships getter exploded');
              },
            }),
        } as unknown as IamClients['authn'],
      };
```

**The `introspectWithProvisioning` describe block becomes a `whoAmI` one.** Delete the retry test
at `:152-157` and the second-refusal test at `:197-207` — both describe a retry loop that no
longer exists. Replace the whole block with:

```ts
  describe('whoAmI', () => {
    // SMA-632's deliverable. This is the assertion the old code could not make: provisioning was
    // a side effect of a GetServiceInfo call, so a cold identity always cost two calls plus a
    // conditional third. It now costs one, for a cold identity and a warm one alike.
    it('makes exactly ONE call for an identity IAM has never seen', async () => {
      const result = await whoAmI(clientsFor('new-user'));
      expect(result).toEqual({ ok: true, value: { prn: fake.principalPrnFor('new-user'), memberships: [] } });
      expect(fake.calls.map((call) => call.method)).toEqual(['authn.whoAmI']);
    });

    it('makes exactly one call for an already-provisioned user too', async () => {
      fake.provisioned.add('known-user');
      const result = await whoAmI(clientsFor('known-user'));
      expect(result).toEqual({ ok: true, value: { prn: fake.principalPrnFor('known-user'), memberships: [] } });
      expect(fake.calls.map((call) => call.method)).toEqual(['authn.whoAmI']);
    });

    // WhoAmI and Introspect must name the SAME principal for one token, or a page that reads one
    // and a test that drives the other disagree about who is logged in.
    it('names the same principal Introspect names for the same token', async () => {
      const mine = await whoAmI(clientsFor('same-user'));
      const theirs = await clientsFor('same-user').authn.introspect({ token: 'same-user' });
      expect(mine.ok ? mine.value.prn : null).toBe(theirs.principalPrn);
    });

    // Bearer enforcement is necessary but not sufficient: IAM still checks the issuer's JIT flag
    // (authenticate_token.rs:107-109), so this error survives and the console keeps a branch.
    it('returns identity-not-provisioned as an error rather than retrying', async () => {
      fake.setHandlers({
        'authn.whoAmI': () => {
          throw denial(ErrorReason.IDENTITY_NOT_PROVISIONED);
        },
      });
      const result = await whoAmI(clientsFor('jit-off'));
      expect(result.ok ? null : result.error.reason).toBe(ErrorReason.IDENTITY_NOT_PROVISIONED);
      expect(fake.callsTo('authn.whoAmI')).toHaveLength(1);
    });
  });
```

Check `denial`'s real signature in `testing/fake-iam.ts` before using it with an argument; if it
takes none, throw an `iamError`-shaped `ConnectError` directly, matching how the other tests in
this file build a refusal.

**The two blank-PRN tests at `:172` and `:184`** keep their assertions; change their
`setHandlers` key from `'authn.introspect'` to `'authn.whoAmI'`, their handler signature to
`() => ({ principalPrn: '', ... })`, and their call from `introspectWithProvisioning(clients, token)`
to `whoAmI(clients)`. Their comments about defect 1 stay — the behaviour they guard is unchanged.

- [ ] **Step 2: Run them to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-632-whoami
pnpm -C ts --filter @paigasus/console-core test principal
```

Expected: FAIL — `whoAmI` is not exported from `../../src/principal`.

- [ ] **Step 3: Rewrite `principal.ts`**

```ts
// SPDX-License-Identifier: Apache-2.0
//
// Who the current user is, according to IAM, NOW. The pages use this, never the login snapshot
// in the session record: a degraded login must not stay degraded for the session, and a
// membership change must appear on the next render.
//
// ONE CALL (SMA-632). WhoAmI is bearer-enforced — it is deliberately absent from `is_exempt`
// (rs/crates/services/paigasus-iam/src/adapters/grpc/authn.rs:139-141) — so IAM resolves the
// bearer with Provisioning::Enabled and JIT-provisions the caller before the handler runs.
// Provisioning is no longer a side effect of GetServiceInfo, and there is nothing left to retry.
//
// `identity-not-provisioned` can still come back: enforcement is necessary but not sufficient,
// because `resolve` also checks the issuer's JIT flag (application/authenticate_token.rs:107-109).
// A retry would not help — the issuer's configuration is what refused — so it is reported.
import 'server-only';
import type { IamClients } from './iam-clients';
import { callIam, type IamResult } from './errors';
import { principalPrnOf } from './principal-prn';

/**
 * `prn` is `null` when IAM answered but named no principal (review, defect 1). That is NOT the
 * login-time degrade in principal-resolver.ts, which discards the whole answer: here the
 * memberships IAM did send stay usable, and only the two things that need a name change — mayI()
 * cannot ask IAM about an unnamed principal (authorize.ts) and myScopes() cannot list its role
 * grants (scopes.ts). Both say so in the log rather than passing `''` to IAM.
 */
export type Principal = { prn: string | null; memberships: readonly { nodePrn: string }[] };

export async function whoAmI(clients: Pick<IamClients, 'authn'>, opts: { timeoutMs?: number } = {}): Promise<IamResult<Principal>> {
  const callOptions = opts.timeoutMs === undefined ? {} : { timeoutMs: opts.timeoutMs };
  const answer = await callIam(() => clients.authn.whoAmI({}, callOptions));
  if (!answer.ok) return answer;
  return { ok: true, value: { prn: principalPrnOf(answer.value.principalPrn), memberships: answer.value.memberships.map((m) => ({ nodePrn: m.nodePrn })) } };
}
```

The `ErrorReason` import goes, because the retry branch that used it goes.

- [ ] **Step 4: Rewrite the resolver's call sequence**

In `principal-resolver.ts`, narrow both the `clientsForToken` type and the body. The parameter
type at `:31` becomes `Pick<IamClients, 'authn'>`. Replace the numbered steps 1-3 inside the
`try` with:

```ts
        const clients = await deps.clientsForToken(accessToken);
        // ONE bearer-enforced call (SMA-632). WhoAmI provisions the caller because enforcement
        // covers it, so the GetServiceInfo-then-Introspect sequence this used to run is gone.
        const answer = await callIam(() => clients.authn.whoAmI({}, { timeoutMs }));
        if (!answer.ok) return degraded(answer.error.presentation);
        const me = answer.value;
        // IAM reports no role grants here (authenticate_token.rs:161-165), and the port says to
        // treat that as UNKNOWN (ports/principal-resolver.ts:32-36). SMA-633 owns filling it.
        return {
          // The SAME reading as the live path (principal-prn.ts). The two used to disagree.
          principalPrn: principalPrnOf(me.principalPrn),
          issuer: me.issuer,
          subject: me.subject,
          memberships: me.memberships.map((m) => ({ id: m.id, principalPrn: m.principalPrn, nodePrn: m.nodePrn })),
          roleGrants: [],
          grantsAvailable: false,
        };
```

The file header's paragraph about `@paigasus/auth` not importing `@paigasus/sdk` stays. Update
its first paragraph to say one call, not two, and keep the `IT NEVER FAILS THE LOGIN` paragraph
exactly as it is.

- [ ] **Step 5: Update the two remaining source sites**

`src/runtime.ts:99`:

```ts
  const currentPrincipal: () => Promise<IamResult<Principal>> = cache(async () => whoAmI(await iamClients()));
```

and its import at `:29` becomes `import { whoAmI, type Principal } from './principal';`.

`src/index.ts:19`:

```ts
export { whoAmI, type Principal } from './principal';
```

- [ ] **Step 6: Run the tests**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-632-whoami
pnpm -C ts --filter @paigasus/console-core test
```

Expected: PASS. Every test in `principal.test.ts` runs; confirm the four new `whoAmI` tests
appear by name.

- [ ] **Step 7: Typecheck and lint the package**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-632-whoami
moon run console-core-ts:typecheck console-core-ts:lint
```

Expected: clean. If the Moon project id differs, find it with `moon query projects | grep console-core`.

- [ ] **Step 8: Prove the one-call test is not vacuous**

Temporarily add a second call to `whoAmI` in `principal.ts`:

```ts
  await callIam(() => clients.authn.whoAmI({}, callOptions));
```

Re-run Step 6. Expected: `makes exactly ONE call for an identity IAM has never seen` FAILS with
two entries in the array. Remove the added line by editing it out.

- [ ] **Step 9: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-632-whoami
git add ts/packages/paigasus-console-core/src/ ts/packages/paigasus-console-core/tests/integration/principal.test.ts
git commit -F - <<'EOF'
refactor(ts): one WhoAmI call replaces GetServiceInfo plus Introspect (SMA-632)

introspectWithProvisioning becomes whoAmI. Two calls and a conditional
retry become one, for a cold identity and a warm one alike, and the
provisionFirst option goes with the ordering it expressed.

identity-not-provisioned stays handled: enforcement provisions the caller,
but resolve still checks the issuer's JIT flag, so the error survives and
a retry would not help.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
```

---

## Task 9: The two console apps

**Files:**
- Modify: `ts/apps/gateway-console/tests/integration/provisioning.test.ts:41-55`
- Modify: `ts/apps/iam-console/tests/e2e/login.spec.ts:1-10,28-52`
- Modify: `ts/apps/gateway-console/tests/e2e/login.spec.ts:1-30`
- Modify: `ts/apps/gateway-console/tests/e2e/call-count.spec.ts:34`
- Modify: `ts/apps/iam-console/tests/integration/doubles/fake-iam.test.ts:101-110`
- Modify: `ts/apps/iam-console/lib/auth.ts:15`, `ts/apps/gateway-console/lib/auth.ts:15`
- Modify: `ts/packages/paigasus-sdk/src/iam.ts:8`, `ts/packages/paigasus-sdk/tests/iam-service-info.test.ts:4`

**Interfaces:**
- Consumes: `whoAmI` from Task 8, the fake from Task 7.
- Produces: nothing.

- [ ] **Step 1: Re-derive the site list by grep, not by reading this plan**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-632-whoami
grep -rn 'getServiceInfo\|introspectWithProvisioning\|authn\.introspect\|GetServiceInfo\|Introspect' ts/ \
  | grep -v node_modules | grep -v '/generated/' | grep -v paigasus-console-core
```

Work the list this produces. If it names a file this task does not list, handle it too and say so
in the commit — the plan's list was derived on 2026-09-20 and the tree may have moved.

- [ ] **Step 2: Fix the gateway-console provisioning test**

`ts/apps/gateway-console/tests/integration/provisioning.test.ts:41` currently reads
`it('calls GetServiceInfo BEFORE Introspect, both carrying the request correlation id', ...)`.
It asserts an ordering that no longer exists. Rewrite it as:

```ts
  it('makes one WhoAmI call, carrying the request correlation id', async () => {
    // ... existing setup unchanged ...
    const calls = callsSince(iam)('authn.whoAmI');
    expect(calls).toHaveLength(1);
    expect(calls[0]?.correlationId).toBe(/* the same expected id the old test asserted */);
  });
```

Keep the correlation-id assertion exactly as the old test made it — that property is unrelated to
this issue and must not weaken. Delete the `serviceInfoIndex` lookup at `:50` and the ordering
comparison that used it.

- [ ] **Step 3: Fix both e2e login specs**

`ts/apps/iam-console/tests/e2e/login.spec.ts` — the header at `:5` says "IAM provisions a
principal only inside a bearer-enforced call, so GetServiceInfo must come before Introspect."
Replace it with:

```ts
// IAM provisions a principal only inside a bearer-enforced call. WhoAmI IS that call (SMA-632),
// so the login makes one gRPC call where it used to make two.
```

At `:32`, `expect(mine[0]?.method).toBe('serviceInfo.getServiceInfo');` becomes
`expect(mine[0]?.method).toBe('authn.whoAmI');`. At `:49`, the `findIndex` for the first
provisioning call changes to `(call) => call.method === 'authn.whoAmI'`.

`ts/apps/gateway-console/tests/e2e/login.spec.ts` — the same three changes. Its `:21-28` comment
block explains that the HTTP discovery probe (`http.getServiceInfo`) runs later, during the
overview render. **That paragraph stays true and must not be deleted** — the HTTP probe is
unaffected by this issue. Only the gRPC assertion at `:28` changes.

- [ ] **Step 4: Fix the call-count formula**

`ts/apps/gateway-console/tests/e2e/call-count.spec.ts:34`:

```ts
    'authn.whoAmI': 1, // the session principal, memoized per request
```

The total does not change. The provisioning call was never in this formula, because a warm
session never made it — the formula measures one render, not a cold login.

- [ ] **Step 5: Fix the iam-console fake double test**

`ts/apps/iam-console/tests/integration/doubles/fake-iam.test.ts:101-110` proves
"identity-not-provisioned until the token makes a bearer-enforced call", using
`info.getServiceInfo({})` as that call. `GetServiceInfo` is still bearer-enforced, so the test is
not wrong — but it now documents the wrong RPC as the provisioning one. Re-point it:

```ts
    // WhoAmI is the bearer-enforced call the console actually makes (SMA-632). GetServiceInfo is
    // still enforced and would still provision; it is simply no longer what the console uses.
    const before = mapError({ kind: 'grpc', error: await rejection(authn.introspect({ token: 'token-b' })) });
    // ... existing assertion on `before` unchanged ...
    await authn.whoAmI({});
    const after = await authn.introspect({ token: 'token-b' });
    // ... existing assertion on `after` unchanged ...
```

Keep `:141` and `:145` (the capability and HTTP-probe assertions) unchanged.

- [ ] **Step 6: Correct the four comments that name the old sequence**

These are comments only; no behaviour changes.

`ts/apps/iam-console/lib/auth.ts:15` and `ts/apps/gateway-console/lib/auth.ts:15` both read
"(GetServiceInfo, Introspect) carry the same id as every later call of that request". Replace the
parenthetical with `(WhoAmI)`.

`ts/packages/paigasus-sdk/src/iam.ts:8` reads "the iam console calls GetServiceInfo as its
bearer-enforced provisioning call". Replace with: "the consoles call AuthnService.WhoAmI as their
bearer-enforced provisioning call (SMA-632); GetServiceInfo remains the capability descriptor."

`ts/packages/paigasus-sdk/tests/iam-service-info.test.ts:4` repeats the old claim in a comment.
Correct the comment. **The test body does not change** — it checks the generated
`ServiceInfoService` client, which is unaffected.

- [ ] **Step 7: Run the integration tiers**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-632-whoami
pnpm -C ts --filter @paigasus/iam-console --filter @paigasus/gateway-console --filter @paigasus/sdk test
```

Expected: PASS. If a filter name is wrong, list them with `pnpm -C ts ls --depth -1`.

- [ ] **Step 8: Run the e2e tier**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-632-whoami
moon run :test-e2e
```

Expected: PASS, including the two-zone tier.

Two known flakes in this repo, neither caused by this diff: a Keycloak tab can hang on
`page.close()`, and a React transition's `isPending` can lag. If a failure matches either
signature, re-run once before investigating. If it reproduces, investigate — do not assume.

- [ ] **Step 9: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-632-whoami
git add ts/apps/ ts/packages/paigasus-sdk/
git commit -F - <<'EOF'
test(ts): both console zones make one WhoAmI call at login (SMA-632)

Six test files asserted GetServiceInfo before Introspect by name. They now
assert one WhoAmI call. The gateway call-count formula names whoAmI; its
total is unchanged, because the provisioning call was never in it.

The HTTP discovery probe is untouched: it is a capability read, not a
provisioning call.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
```

---

## Task 10: Full-graph verification

**Files:** none modified unless a gate fails.

**Interfaces:**
- Consumes: everything.
- Produces: the evidence the PR body cites.

- [ ] **Step 1: Confirm the working tree is clean and rebased**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-632-whoami
git status --short
git fetch origin && git log --oneline origin/main -1
```

Expected: no output from `git status --short`. If `origin/main` has moved, rebase onto it before
running the gates — `main` requires strict up-to-date branches.

- [ ] **Step 2: Run the full gate graph**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
export PROTO_REPORTER=text
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-632-whoami
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site \
  :http-extractor-envelope :input-liveness :promtool :observability-drift \
  :nats-permissions :release-parity :release-parity-py :release-parity-ts \
  :publish-metadata :version-lockstep :workflow-credentials :pyo3-stub-drift :ruff-ci \
  :next-public-free :test-e2e \
  --base origin/main --include-relations
```

This must run under **system `/bin/bash` 3.2**, which `repo:affected-smoke` needs. The gates that
need bash 4+ are re-run in Step 4.

- [ ] **Step 3: If a task fails, diagnose it before re-running**

**Capture first.** A re-run overwrites the evidence, and a *passing* re-run is as destructive as
a failing one:

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-632-whoami
cp .moon/cache/ciReport.json /tmp/sma632-ciReport.json
```

Then find the task, its command and its real exit code:

```bash
jq '.actions[] | select(.status=="failed")
    | {label, error,
       exec: (.operations[] | select(.meta.type=="task-execution") | {command, exitCode})}' \
   /tmp/sma632-ciReport.json
```

There is no action-level `exitCode` key; the real one is in `operations[]`. Read the output from
`.moon/cache/states/<project>/<task>/stdout.log` and `stderr.log` — that is the only place it
exists. Before trusting those logs, compare the action's `finishedAt` against `lastRun.json`'s
`lastRunTime`; if they disagree, the logs are from a different run and pairing them yields a
confident wrong answer.

**Three failures that are NOT this diff:**
- A `cargo metadata` error naming a `.napi-stage-<random>` path under `rs/crates/bindings/` is a
  CI-only concurrency flake. Confirm by the exact error text and re-run.
- An empty `stdout.log` plus a one-line `declare: -A: invalid option` or `mapfile: command not
  found` on stderr means bash 3.2 ran a gate that needs 4+. See Step 4.
- `repo:actionlint` exiting rc 2 with a message about the pipe holding only 512 bytes is a host
  condition, not a gate failure. Read the gate's own preflight line before concluding anything.

- [ ] **Step 4: Re-run the bash-4+ gates under Homebrew bash**

No single local bash runs every gate. Four need bash 4 or 5:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
export PROTO_REPORTER=text
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-632-whoami
/opt/homebrew/bin/bash ci/ruff/run.sh
/opt/homebrew/bin/bash ci/next-public/run.sh
/opt/homebrew/bin/bash ci/publish-metadata/run.sh
/opt/homebrew/bin/bash ci/actionlint/run.sh
```

Read **these** results for those four gates, not `moon ci`'s verdict on them.

`ci/actionlint/run.sh` yields a local verdict only when its pipe preflight passes. If it prints
`pipe capacity 512 bytes` and exits rc 2, there is no local verdict for it — record that, and let
CI decide.

- [ ] **Step 5: Confirm the Docker-gated tests actually ran**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam
```

Expected: everything passes, and `grpc_whoami`'s seven tests appear by name. A `moon ci` green
alone does not prove this — these suites skip quietly without Docker.

If Docker is unreachable, record that plainly. The PR body must say so.

- [ ] **Step 6: Record the evidence for the PR body**

Write down, for the PR:
- the `moon ci` verdict, and any gate re-run under a different bash with its own result;
- whether Docker was reachable, and whether `grpc_whoami`'s seven tests ran;
- the three mutation results: adding `WhoAmI` to `is_exempt` fails three tests (Task 6 Step 4);
  a second `whoAmI` call fails the one-call test (Task 8 Step 8); a conflicting route path fails
  the outer-merge test (Task 5 Step 10).

- [ ] **Step 7: Commit anything the gates changed**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-632-whoami
git status --short
```

If a gate rewrote a generated file (`.github/CODEOWNERS` is Moon-generated and must not be
hand-edited), commit it:

```bash
git add -A && git commit -F - <<'EOF'
chore(repo): gate-generated updates (SMA-632)

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
```

If nothing changed, skip this step.

---

## Self-Review

**Spec coverage.** Every section of the spec maps to a task:

| Spec section | Task |
|---|---|
| § 4 contract, codegen, error registry, `error-code-single-site` | 1 |
| § 5.2 the paging helper and both duplicates | 2 |
| § 5.2 `context_for` and its wiring | 3 |
| § 5.1 `AuthContext`; § 5.3 the mapper; § 5.4 the handler, `actor_context`, the `is_exempt` doc | 4 |
| § 5.5 the HTTP twin, the mount, both route-conflict tests | 5 |
| § 6.1 mapper and DTO tests | 4, 5 |
| § 6.2 the seven integration tests; § 6.3 the controls | 6 |
| § 6.4 the fake, the dev world, `dev-world.test.ts` | 7 |
| § 6.4 `principal.test.ts`; § 7 the console-core sources | 8 |
| § 6.4 both apps' tiers; § 7 the comment corrections | 9 |
| § 10 verification | 10 |

**Two spec items deliberately not given their own task**, both recorded rather than dropped:

- § 6.1's `who_am_i_response_mirrors_introspect_response` drift guard. A Rust test cannot compare
  two prost structs' field names without reflection the generated code does not carry. Task 1
  Step 3's `buf lint` plus Task 1 Step 4's `buf breaking` catch a renamed or renumbered field,
  and the two messages' *shapes* are asserted by Task 4's and Task 5's mapper and DTO tests. **The
  implementer should raise this at review if they see a cheap way to assert it directly.**
- § 9.5's note that SMA-633 inherits two messages. That is a Linear comment, not code. It is
  handled in the PR's follow-up section, not by a task.

**Placeholder scan.** No `TBD`, no "similar to Task N", no "add error handling". Four steps say
"read the neighbouring code and match it" (Task 2 Step 3, Task 3 Step 3, Task 4 Step 3, Task 6
Step 2) — that is deliberate, because those fixtures must match conventions the plan cannot
reproduce faithfully without copying hundreds of lines, and each names the exact file to read.

**Type consistency.** `load_all_memberships(&M, &PrincipalId) -> Result<Vec<MembershipRecord>, AuthnError>`
is defined in Task 2 and called with that signature in Tasks 2 and 3. `context_for(AuthnPrincipal) -> Result<PrincipalContext, AuthnError>`
is defined in Task 3 and called in Tasks 4 and 5. `to_who_am_i_response(&PrincipalContext) -> WhoAmIResponse`
is defined and used in Task 4 only. `whoAmI(Pick<IamClients, 'authn'>, opts?) -> Promise<IamResult<Principal>>`
is defined in Task 8 and called in Tasks 8 and 9. `AuthContext` gains `kind` and `status` in
Task 4 and is read with all four fields in Tasks 4 and 5.
