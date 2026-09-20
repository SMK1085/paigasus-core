# Introspect role grants — Implementation Plan (SMA-633)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the OIDC `Introspect` RPC report the principal's real role grants, and advertise an `iam.authn.grants` capability so a console can tell a populating build from an old one.

**Architecture:** `AuthenticateToken` gains an `Arc<dyn RoleGrantStore>` field and calls `list_by_principal` inside `introspect` (inside `context_for` after the SMA-632 rebase), maps each `RoleGrant` to the wire projection `RoleGrantRef`, and sorts. The two mappers already forward the field, so no adapter code changes. The API-key path stays empty on purpose, and no TypeScript behaviour changes.

**Tech Stack:** Rust (edition 2024, rust-version 1.95), tokio, async-trait, SeaORM, prost/tonic, buf, Moon.

**Spec:** `docs/superpowers/specs/2026-09-20-sma-633-introspect-role-grants-design.md`

## Global Constraints

- Every source file opens with `// SPDX-License-Identifier: Apache-2.0` (`#` for Python).
- Rust crates are edition 2024, rust-version 1.95. `cargo clippy --workspace -- -D warnings` must pass; warnings are hard errors, so no dead code may be staged "to wire later".
- Conventional commits with a workspace scope: `feat(rs):`, `fix(contracts):`. No `#NNN` line and no `token: value` line in a commit body — commitlint fails `footer-leading-blank`.
- Branch: `feature/sma-633-iam-populate-role_grants-in-introspect`. Worktree: `.claude/worktrees/sma-633-role-grants`.
- Prefix every shell command with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` so moon, cargo, buf and nextest resolve to the repo-pinned versions.
- The capability wire key is `iam.authn.grants`. The enum value is `CAPABILITY_IAM_AUTHN_GRANTS = 6`. The generated names are `Capability::IamAuthnGrants` (Rust) and `Capability.IAM_AUTHN_GRANTS` (TypeScript and Python).
- **This plan starts blocked.** Task 1 does not begin until SMA-632 is merged to `main`.

---

### Task 1: Rebase onto merged SMA-632 and re-derive the line numbers

SMA-632 rewrites `introspect` into a `context_for` helper and deletes `MEMBERSHIP_PAGE_SIZE` and both paging loops. Every line number in Tasks 2-4 is pre-rebase. This task makes them true again.

**Files:**
- Modify: none yet. This task only rebases and records findings.

**Interfaces:**
- Consumes: SMA-632's merged `AuthenticateToken::context_for(&self, principal: AuthnPrincipal) -> Result<PrincipalContext, AuthnError>`.
- Produces: a verified target method name and line for Task 2, written into this file.

- [ ] **Step 1: Confirm SMA-632 is merged**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
git fetch origin
git log --oneline origin/main | grep -i "SMA-632"
```

Expected: one or more commits naming SMA-632. If there are none, **stop** — this plan is still blocked, and no other task may start.

- [ ] **Step 2: Rebase this branch**

```bash
git rebase origin/main
```

Expected: a clean rebase. Only two documentation files are on this branch, so a conflict is unlikely. If one appears, keep both files as they are on this branch.

- [ ] **Step 3: Find the method that now builds `PrincipalContext`**

```bash
grep -n "PrincipalContext {" rs/crates/services/paigasus-iam/src/application/authenticate_token.rs
grep -n "fn context_for\|fn introspect" rs/crates/services/paigasus-iam/src/application/authenticate_token.rs
```

Expected: one construction site of `PrincipalContext`, inside either `introspect` or `context_for`. That site is Task 2's target. If the grep returns more than one construction site in this file, read both and pick the one `introspect` reaches.

- [ ] **Step 4: Re-derive the other line numbers**

```bash
grep -rn "role_grants\|roleGrants" rs/crates/services/paigasus-iam/src rs/crates/services/paigasus-iam/tests rs/crates/libs/paigasus-iam-core/src
```

Expected: the sites Tasks 2-4 name, at their post-rebase lines. Write the corrected numbers into this plan file before continuing. Do not trust the pre-rebase numbers below.

- [ ] **Step 5: Commit the corrected plan**

```bash
git add docs/superpowers/plans/2026-09-20-sma-633-introspect-role-grants.md
git commit -m "docs(repo): re-derive SMA-633 plan line numbers after the SMA-632 rebase"
```

---

### Task 2: Populate `role_grants` in the OIDC introspection path

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/application/authenticate_token.rs` (struct, `new`, the `PrincipalContext` construction site, and every test constructor call)
- Modify: `rs/crates/services/paigasus-iam/src/adapters/http/mod.rs:772-780` (pass the existing store)
- Test: `rs/crates/services/paigasus-iam/src/application/authenticate_token.rs` (its own `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `RoleGrantStore::list_by_principal(&PrincipalId) -> Result<Vec<RoleGrant>, AuthzError>`; `GrantScope::canonical_prn() -> String`; `InMemoryRoleGrants(pub Arc<Mutex<HashMap<Uuid, RoleGrant>>>)` from `crate::application::fakes`.
- Produces: `AuthenticateToken::new(authenticator, identities, principals, memberships, grants: Arc<dyn RoleGrantStore>, id_gen, clock, jit)` — the `grants` argument is **fifth**, directly after `memberships`. Every later task and every existing test calls this arity.

- [ ] **Step 1: Write the failing tests**

Add these four tests to the `mod tests` block in `authenticate_token.rs`. They follow the shape of `introspect_pages_through_memberships`, which already builds an `AuthnStore`, a principal and an external identity.

```rust
    /// Helper: a store seeded with one principal and its external identity, returning the
    /// pieces the four grant tests need. Mirrors `introspect_pages_through_memberships`'s
    /// own setup, which predates this helper.
    fn seeded_principal(store: &AuthnStore, issuer: &Issuer, subject: &str) -> PrincipalId {
        let pid = principal_id(1);
        let principal = Principal::new(pid.clone(), PrincipalKind::User, PrincipalStatus::Active, epoch(), epoch());
        let user = User::new(pid.clone(), Email::parse("grants@example.com").unwrap(), "Grants".into(), None, None, epoch(), epoch());
        store.principals.lock().unwrap().insert(pid.uuid(), (principal, user));
        store.identities.lock().unwrap().insert(
            (issuer.as_str().to_string(), subject.to_string()),
            ExternalIdentity {
                id: Uuid::from_u128(7),
                principal_id: pid.clone(),
                issuer: issuer.clone(),
                subject: subject.into(),
                created_at: epoch(),
                updated_at: epoch(),
            },
        );
        pid
    }

    /// A grant at Root and a grant at a node both reach the caller, each carrying the
    /// canonical PRN of its own scope (spec D4).
    #[tokio::test]
    async fn introspect_returns_the_principals_role_grants() {
        let store = AuthnStore::default();
        let issuer = Issuer::parse("https://idp.example.com").unwrap();
        let pid = seeded_principal(&store, &issuer, "sub-grants");

        let org = TenancyNodeRef::from_prn(&Prn::parse("prn:paigasus:iam:::organization/11111111-1111-1111-1111-111111111111").unwrap()).unwrap();
        let grants = InMemoryRoleGrants::default();
        grants
            .grant(&RoleGrant {
                id: Uuid::from_u128(1),
                principal: pid.clone(),
                role_key: "platform_admin".into(),
                scope: GrantScope::Root,
                linked_policy_id: "lp-1".into(),
                created_at: epoch(),
            })
            .await
            .unwrap();
        grants
            .grant(&RoleGrant {
                id: Uuid::from_u128(2),
                principal: pid.clone(),
                role_key: "org_admin".into(),
                scope: GrantScope::Node(org.clone()),
                linked_policy_id: "lp-2".into(),
                created_at: epoch(),
            })
            .await
            .unwrap();

        let uc = AuthenticateToken::new(
            FakeAuthenticator::ok(claims("https://idp.example.com", "sub-grants", Some("grants@example.com"), Some("Grants"))),
            InMemoryIdentities(store.clone()),
            InMemoryPrincipals(store.clone()),
            InMemoryMemberships::default(),
            Arc::new(grants),
            SeqIds::default(),
            FixedClock::default(),
            JitPolicy::from_issuers(&[(issuer.clone(), true)]),
        );

        let ctx = uc.introspect("token").await.unwrap();
        assert_eq!(ctx.role_grants.len(), 2);
        assert!(ctx.role_grants.iter().any(|g| g.role_key == "platform_admin" && g.scope_prn == GrantScope::Root.canonical_prn()));
        assert!(ctx.role_grants.iter().any(|g| g.role_key == "org_admin" && g.scope_prn == org.canonical()));
    }

    /// Spec D7: the order is `(scope_prn, role_key)`, not insertion order and not the
    /// fake's `HashMap` iteration order. The two grants share a scope and differ only in
    /// role key, which the database's `uq_role_grant_principal_role_scope` permits.
    #[tokio::test]
    async fn introspect_sorts_role_grants_deterministically() {
        let store = AuthnStore::default();
        let issuer = Issuer::parse("https://idp.example.com").unwrap();
        let pid = seeded_principal(&store, &issuer, "sub-sorted");

        let grants = InMemoryRoleGrants::default();
        for (n, role) in [(1u128, "zeta_role"), (2, "alpha_role")] {
            grants
                .grant(&RoleGrant {
                    id: Uuid::from_u128(n),
                    principal: pid.clone(),
                    role_key: role.into(),
                    scope: GrantScope::Root,
                    linked_policy_id: format!("lp-{n}"),
                    created_at: epoch(),
                })
                .await
                .unwrap();
        }

        let uc = AuthenticateToken::new(
            FakeAuthenticator::ok(claims("https://idp.example.com", "sub-sorted", Some("grants@example.com"), Some("Grants"))),
            InMemoryIdentities(store.clone()),
            InMemoryPrincipals(store.clone()),
            InMemoryMemberships::default(),
            Arc::new(grants),
            SeqIds::default(),
            FixedClock::default(),
            JitPolicy::from_issuers(&[(issuer.clone(), true)]),
        );

        let ctx = uc.introspect("token").await.unwrap();
        let keys: Vec<&str> = ctx.role_grants.iter().map(|g| g.role_key.as_str()).collect();
        assert_eq!(keys, vec!["alpha_role", "zeta_role"], "grants must be sorted, not insertion-ordered");
    }

    /// Spec D6: a grant-store failure fails the call. It must NOT degrade to an empty
    /// list, because a consumer reads an empty list as "this principal holds no grants".
    #[tokio::test]
    async fn introspect_fails_when_the_grant_store_fails() {
        let store = AuthnStore::default();
        let issuer = Issuer::parse("https://idp.example.com").unwrap();
        seeded_principal(&store, &issuer, "sub-broken");

        let uc = AuthenticateToken::new(
            FakeAuthenticator::ok(claims("https://idp.example.com", "sub-broken", Some("grants@example.com"), Some("Grants"))),
            InMemoryIdentities(store.clone()),
            InMemoryPrincipals(store.clone()),
            InMemoryMemberships::default(),
            Arc::new(FailingGrants),
            SeqIds::default(),
            FixedClock::default(),
            JitPolicy::from_issuers(&[(issuer.clone(), true)]),
        );

        let err = uc.introspect("token").await.unwrap_err();
        assert!(matches!(err, AuthnError::Backend(_)), "expected Backend, got {err:?}");
    }

    /// A principal with no grants gets an empty list and a SUCCESSFUL call — the
    /// discriminator against the failure case above.
    #[tokio::test]
    async fn introspect_returns_an_empty_list_for_a_principal_without_grants() {
        let store = AuthnStore::default();
        let issuer = Issuer::parse("https://idp.example.com").unwrap();
        seeded_principal(&store, &issuer, "sub-none");

        let uc = AuthenticateToken::new(
            FakeAuthenticator::ok(claims("https://idp.example.com", "sub-none", Some("grants@example.com"), Some("Grants"))),
            InMemoryIdentities(store.clone()),
            InMemoryPrincipals(store.clone()),
            InMemoryMemberships::default(),
            Arc::new(InMemoryRoleGrants::default()),
            SeqIds::default(),
            FixedClock::default(),
            JitPolicy::from_issuers(&[(issuer.clone(), true)]),
        );

        let ctx = uc.introspect("token").await.unwrap();
        assert!(ctx.role_grants.is_empty());
    }
```

Add the failing double the third test needs, next to the other test doubles in the same `mod tests`:

```rust
    /// Every method fails. Only `list_by_principal` is reached by `introspect`; the rest
    /// satisfy the trait. Mirrors `roles.rs`'s own `FailingGrantStore`.
    struct FailingGrants;

    #[async_trait::async_trait]
    impl RoleGrantStore for FailingGrants {
        async fn grant(&self, _g: &RoleGrant) -> Result<(), AuthzError> {
            Err(AuthzError::Backend("boom".into()))
        }
        async fn revoke(&self, _id: Uuid) -> Result<(), AuthzError> {
            Err(AuthzError::Backend("boom".into()))
        }
        async fn grant_in(&self, _tx: &dyn Transaction, _g: &RoleGrant) -> Result<(), AuthzError> {
            Err(AuthzError::Backend("boom".into()))
        }
        async fn revoke_in(&self, _tx: &dyn Transaction, _id: Uuid) -> Result<bool, AuthzError> {
            Err(AuthzError::Backend("boom".into()))
        }
        async fn list_all(&self) -> Result<Vec<RoleGrant>, AuthzError> {
            Err(AuthzError::Backend("boom".into()))
        }
        async fn list_by_principal(&self, _p: &PrincipalId) -> Result<Vec<RoleGrant>, AuthzError> {
            Err(AuthzError::Backend("boom".into()))
        }
        async fn find(&self, _id: Uuid) -> Result<Option<RoleGrant>, AuthzError> {
            Err(AuthzError::Backend("boom".into()))
        }
    }
```

Before writing it, run `grep -n "enum AuthzError" -A 30 rs/crates/libs/paigasus-iam-core/src/authz/model.rs` and use a variant that actually exists. `Backend` is the expected one; if the enum spells it differently, use that spelling and keep the test's meaning.

- [ ] **Step 2: Run the tests and watch them fail to COMPILE**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --lib authenticate_token 2>&1 | tail -30
```

Expected: a compile error, because `AuthenticateToken::new` takes seven arguments and the new tests pass eight. That is the correct first failure. A compile error is a weaker signal than a failing assertion, so Step 6 re-runs these tests against the real implementation and Step 8 mutates it.

- [ ] **Step 3: Add the port to the struct and the constructor**

In `authenticate_token.rs`, extend the import from `paigasus_iam_core` with `AuthzError, GrantScope, RoleGrant, RoleGrantRef, RoleGrantStore` (all are re-exported at the crate root — see `rs/crates/libs/paigasus-iam-core/src/lib.rs:25-26`), and add `use std::sync::Arc;`.

```rust
/// Wraps an `AuthzError` as `AuthnError::Backend`. Separate from `backend` above because
/// the grant store's error type is not `RepositoryError`. Spec D6: a grant-store failure
/// fails the call — it must never degrade to an empty grant list.
fn backend_authz(err: AuthzError) -> AuthnError {
    AuthnError::Backend(Box::new(err))
}
```

```rust
#[derive(Clone)]
pub struct AuthenticateToken<A, E, P, M, I, C> {
    authenticator: A,
    identities: E,
    principals: P,
    memberships: M,
    /// Spec D3: a trait object, not a generic parameter — the composition root clones the
    /// one `Arc` it already composes into `PolicySnapshot`, exactly as `RoleService` does
    /// (`application/roles.rs:91`).
    grants: Arc<dyn RoleGrantStore>,
    id_gen: I,
    clock: C,
    jit: JitPolicy,
}
```

```rust
    #[allow(clippy::too_many_arguments)]
    pub fn new(authenticator: A, identities: E, principals: P, memberships: M, grants: Arc<dyn RoleGrantStore>, id_gen: I, clock: C, jit: JitPolicy) -> Self {
        AuthenticateToken {
            authenticator,
            identities,
            principals,
            memberships,
            grants,
            id_gen,
            clock,
            jit,
        }
    }
```

- [ ] **Step 4: Populate and sort at the `PrincipalContext` construction site**

Replace `role_grants: Vec::new(),` at the site Task 1 Step 3 identified:

```rust
        // Spec D3/D4/D7: the principal's own grants, projected to the wire type and sorted.
        // `list_by_principal` is unbounded and unordered (`PgRoleGrantStore` issues no
        // `ORDER BY`), so the sort is what makes the response deterministic. The pair is a
        // total order in the database: `uq_role_grant_principal_role_scope`.
        let mut role_grants: Vec<RoleGrantRef> = self
            .grants
            .list_by_principal(&principal.principal_id)
            .await
            .map_err(backend_authz)?
            .into_iter()
            .map(|g| RoleGrantRef {
                scope_prn: g.scope.canonical_prn(),
                role_key: g.role_key,
            })
            .collect();
        role_grants.sort_by(|a, b| (&a.scope_prn, &a.role_key).cmp(&(&b.scope_prn, &b.role_key)));

        Ok(PrincipalContext {
            principal,
            memberships,
            role_grants,
        })
```

- [ ] **Step 5: Update every existing constructor call**

```bash
grep -n "AuthenticateToken::new(" rs/crates/services/paigasus-iam/src/application/authenticate_token.rs rs/crates/services/paigasus-iam/src/adapters/http/mod.rs
```

In each unit-test call, insert `Arc::new(InMemoryRoleGrants::default()),` directly after the memberships argument. For the call that uses `PanicIfCalled*` fakes (`invalid_token_short_circuits`), use `Arc::new(FailingGrants)` instead: that test must fail at token verification, so a grant store that panics-or-fails proves the call never got that far.

In `adapters/http/mod.rs`, the production call passes the store built at `:371`:

```rust
        let authn = AuthenticateToken::new(
            authenticator,
            PgExternalIdentityRepository::new(db.clone()),
            PgPrincipalRepository::new(db.clone()),
            PgMembershipRepository::new(db.clone()),
            role_grant_store.clone(),
            KernelIdGenerator,
            SystemClock,
            JitPolicy::from_issuers(&jit_flags),
        );
```

Do **not** change the `AuthnSvc` type alias at `:158`. The new field is not a generic parameter, so the alias is still correct.

- [ ] **Step 6: Run the tests to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --lib authenticate_token
```

Expected: PASS for the four new tests. `introspect_pages_through_memberships` still FAILS at its `assert!(ctx.role_grants.is_empty())` — Task 3 fixes it. If it passes, the population code is not running; stop and find out why.

- [ ] **Step 7: Fix the in-file assertion that the change disproves**

In `introspect_pages_through_memberships`, that principal has no grants, so replace the line with an explicit statement of why it is empty:

```rust
        // This principal holds no grants, so the list is empty for that reason — not
        // because the field is hardcoded. `introspect_returns_the_principals_role_grants`
        // is what proves the field is populated.
        assert!(ctx.role_grants.is_empty());
```

- [ ] **Step 8: Prove the assertions bite (mutation check)**

Delete the `.sort_by(...)` line, run `cargo nextest run -p paigasus-iam --lib authenticate_token`, and confirm `introspect_sorts_role_grants_deterministically` FAILS. If it passes, the fake returned the grants in sorted order by luck — change the role keys until the unsorted order differs, then restore the line.

Then replace `map_err(backend_authz)?` with `.unwrap_or_default()`, re-run, and confirm `introspect_fails_when_the_grant_store_fails` FAILS. Restore it.

Finally delete the whole `let mut role_grants = …` block and return `Vec::new()`, re-run, and confirm `introspect_returns_the_principals_role_grants` FAILS. Restore it.

Restore each mutation by deleting the edit you made, never with `git checkout --`: that would also revert the uncommitted implementation.

- [ ] **Step 9: Run the full crate test suite**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --lib && cargo clippy -p paigasus-iam --all-targets -- -D warnings && cargo fmt --check
```

Expected: PASS. Integration tests under `tests/` are Task 3's job and may still fail here; run only `--lib` at this step.

- [ ] **Step 10: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/application/authenticate_token.rs rs/crates/services/paigasus-iam/src/adapters/http/mod.rs
git commit -m "feat(rs): populate role_grants in the OIDC introspection path (SMA-633)"
```

---

### Task 3: Update the integration tests that pin the empty list

**Files:**
- Modify: `rs/crates/services/paigasus-iam/tests/http_authn.rs:63,71`
- Modify: `rs/crates/services/paigasus-iam/tests/grpc_authn.rs:100`
- Modify: `rs/crates/services/paigasus-iam/tests/http_service_accounts.rs:177`
- Modify: `rs/crates/services/paigasus-iam/tests/api_keys_grpc.rs:143`

**Interfaces:**
- Consumes: Task 2's populated `PrincipalContext.role_grants`; `support::seed_platform_admin(&state, &principal_prn)` and `support::provision_platform_admin(&state, &token)` from `tests/support/mod.rs:648-690`.
- Produces: nothing later tasks depend on.

- [ ] **Step 1: Find every remaining assertion**

```bash
grep -rn "role_grants\|roleGrants" rs/crates/services/paigasus-iam/tests
```

Expected: the four files above. If the sweep finds more, treat each the same way — decide whether that principal holds a grant, and assert the truth rather than adjusting the expectation to whatever the code returns.

- [ ] **Step 2: Flip the two OIDC assertions**

`tests/http_authn.rs` — the test calls `support::seed_platform_admin` first, so the grant is real. Replace the comment at `:63` and the assertion at `:71`:

```rust
    // SMA-633: `seed_platform_admin` above granted `platform_admin` at Root, so introspection
    // reports exactly that grant.
    let grants = body["role_grants"].as_array().expect("role_grants array");
    assert_eq!(grants.len(), 1, "expected exactly the seeded grant: {body}");
    assert_eq!(grants[0]["role_key"], "platform_admin");
    assert_eq!(grants[0]["scope_prn"], root_prn().canonical());
```

`tests/grpc_authn.rs:100` — same change over the gRPC context:

```rust
    // SMA-633: `provision_platform_admin` above granted `platform_admin` at Root.
    assert_eq!(ctx.role_grants.len(), 1, "expected exactly the seeded grant");
    assert_eq!(ctx.role_grants[0].role_key, "platform_admin");
    assert_eq!(ctx.role_grants[0].scope_prn, root_prn().canonical());
```

Import `root_prn` where it is not already in scope: `use paigasus_iam_core::authz::model::root_prn;`. Check the file's existing imports first — `grep -n "^use" rs/crates/services/paigasus-iam/tests/grpc_authn.rs` — and do not add a duplicate.

- [ ] **Step 3: Give the two API-key assertions their reason**

`tests/http_service_accounts.rs:177` and `tests/api_keys_grpc.rs:143` assert an empty list on an **API-key** introspection. Under spec D2 that is now deliberate. Keep each assertion and put the reason above it:

```rust
    // SMA-633 D2: the API-key path reports no grants by decision, not by omission. It runs on
    // the gateway's per-request path and nothing reads the field, so it does not pay for the
    // query. Do not "fix" this to match the OIDC path without reading D2 first.
```

- [ ] **Step 4: Run the integration tests**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --test http_authn --test grpc_authn --test http_service_accounts --test api_keys_grpc
```

Expected: PASS. Several of these suites are Docker-gated and skip silently without a running Docker daemon. Check the output for skips before reporting a pass; if they skipped, start Docker and re-run. A skipped suite is not a green one.

- [ ] **Step 5: Commit**

```bash
git add rs/crates/services/paigasus-iam/tests
git commit -m "test(rs): assert introspection reports the seeded role grant (SMA-633)"
```

---

### Task 4: Register the `iam.authn.grants` capability

**Files:**
- Modify: `contracts/proto/paigasus/common/v1/service_info.proto`
- Modify: `rs/crates/libs/paigasus-proto/src/capability.rs:69-75,86-95,158-162`
- Modify: `rs/crates/services/paigasus-iam/src/service_info.rs:52-71,103,110,118-120,134`
- Modify: `ts/packages/paigasus-discovery/src/types.ts:68`
- Modify: `py/packages/paigasus-proto/tests/test_service_info_smoke.py:19-24`
- Modify: `rs/crates/services/paigasus-iam/tests/http_service_info.rs:52`
- Modify: `rs/crates/services/paigasus-iam/tests/grpc_service_info.rs:110`

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: proto `CAPABILITY_IAM_AUTHN_GRANTS = 6`; Rust `Capability::IamAuthnGrants`; TypeScript `Capability.IAM_AUTHN_GRANTS` and the `CapabilityKey` member `'iam.authn.grants'`; Python `Capability.IAM_AUTHN_GRANTS`.

- [ ] **Step 1: Write the failing Rust tests**

In `rs/crates/libs/paigasus-proto/src/capability.rs`, grow `ALL` and pin the spelling:

```rust
    const ALL: [Capability; 6] = [
        Capability::IamAuthzCedar,
        Capability::IamApikeys,
        Capability::IamAudit,
        Capability::GatewayChatStream,
        Capability::IamDeadletters,
        Capability::IamAuthnGrants,
    ];
```

```rust
        // SMA-633. The round-trip test alone would also pass for "iam.authngrants"; this pins
        // the three-segment spelling.
        assert_eq!(Capability::IamAuthnGrants.as_wire_key().unwrap(), "iam.authn.grants");
```

```rust
    fn adding_a_capability_forces_updating_these_tests() {
        // ALL covers discriminants 1..=6. Registering a seventh value fails here,
        // which is the signal to extend ALL and the literals test above.
        assert!(Capability::try_from(7).is_err());
    }
```

In `rs/crates/services/paigasus-iam/src/service_info.rs`, add the key to every test expectation: `HashSet::from([…, Capability::IamAuthnGrants])` at `:103`, `:110` and all three lines of `:118-120`, and in the combination loop at `:134` assert it is present in every combination, the way `IamDeadletters` already is.

- [ ] **Step 2: Run them to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-proto capability 2>&1 | tail -20
```

Expected: a compile error — `Capability::IamAuthnGrants` does not exist yet. That is the correct first failure.

- [ ] **Step 3: Add the proto enum value**

In `contracts/proto/paigasus/common/v1/service_info.proto`, after `CAPABILITY_IAM_DEADLETTERS = 5;`:

```protobuf
  // "iam.authn.grants" — Introspect populates `role_grants`. IAM always reports it: the
  // population has no config switch. A client that does not see this key must treat an
  // empty `role_grants` as UNKNOWN, not as "this principal holds no grants" (SMA-633 D9).
  CAPABILITY_IAM_AUTHN_GRANTS = 6;
```

Also extend the mapping example in the file's header comment at `:48` if it lists the keys exhaustively — read it first and match whatever form it uses.

- [ ] **Step 4: Format the proto and regenerate**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
buf format -w contracts/proto
moon run contracts:generate
git status --short
```

Expected: regenerated files under `rs/crates/libs/paigasus-proto/src/generated/`, `py/**/paigasus_proto/generated/`, and `ts/packages/paigasus-proto/src/generated/`. If `git status` shows no generated change, the codegen cache served a stale result — re-run with `moon run contracts:generate --force` before continuing. Skipping `buf format -w` makes `contracts:fmt` red later with no useful message.

- [ ] **Step 5: Make the capability advertised**

In `rs/crates/services/paigasus-iam/src/service_info.rs`, push the key unconditionally, next to `IamDeadletters`:

```rust
        caps.push(Capability::IamDeadletters);
        // SMA-633 D9: unconditional, for the same reason as `iam.deadletters`. The population
        // has no config switch, and a `Capabilities` field that is always `true` would be a
        // false degree of freedom. The key means "this build populates `role_grants` in
        // `Introspect`".
        caps.push(Capability::IamAuthnGrants);
        caps
```

Update the module doc at `:11` and the `enabled()` doc at `:52`: there are now **two** unconditional keys, not one.

- [ ] **Step 6: Update the TypeScript and Python registries**

`ts/packages/paigasus-discovery/src/types.ts:68`:

```typescript
export type CapabilityKey = 'iam.authz.cedar' | 'iam.apikeys' | 'iam.audit' | 'gateway.chat.stream' | 'iam.deadletters' | 'iam.authn.grants';
```

`py/packages/paigasus-proto/tests/test_service_info_smoke.py`, in `test_capability_registry_keeps_the_proto_names`:

```python
    assert names[Capability.IAM_AUTHN_GRANTS.value] == "CAPABILITY_IAM_AUTHN_GRANTS"
```

- [ ] **Step 7: Update the two service-info integration tests**

`tests/http_service_info.rs:52` and `tests/grpc_service_info.rs:110` both build a `HashSet` of the full expected key set. Add `"iam.authn.grants".to_string()` to each. Then re-read the "siblings survive" assertions in the same files (`http_service_info.rs:80,124,161,189` and `grpc_service_info.rs:189`) and extend each one that enumerates the unconditional keys. `http_service_info.rs:189` asserts the exact JSON array `["iam.deadletters"]` when every flag is off — it becomes `["iam.deadletters", "iam.authn.grants"]`, in the order `enabled()` pushes them.

- [ ] **Step 8: Run everything the change touches**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-proto -p paigasus-iam
cd .. && uv run --project py pytest packages/paigasus-proto/tests/test_service_info_smoke.py
pnpm -C ts test --filter @paigasus/discovery
```

Expected: PASS everywhere.

- [ ] **Step 9: Prove the capability assertion bites**

Delete the `caps.push(Capability::IamAuthnGrants);` line, run `cargo nextest run -p paigasus-iam --lib service_info`, and confirm the tests FAIL. Restore the line by deleting the edit.

- [ ] **Step 10: Commit**

```bash
git add contracts rs ts py
git commit -m "feat(contracts): register the iam.authn.grants capability (SMA-633)"
```

---

### Task 5: Correct the doc comments the change falsifies

**Files:**
- Modify: `rs/crates/libs/paigasus-iam-core/src/authn.rs:133-134`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/http/dto.rs:242`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/grpc/convert.rs:398`
- Modify: `rs/crates/services/paigasus-iam/src/application/authenticate_api_key.rs:258-260`
- Modify: `ts/packages/paigasus-console-core/testing/fake-iam.ts:15`

**Interfaces:**
- Consumes: nothing. This task changes only comments.
- Produces: nothing.

- [ ] **Step 1: Find every stale claim**

```bash
grep -rn "until a later M3 task\|role_grants staying empty\|always returns an empty" rs ts --include=*.rs --include=*.ts
```

Expected: the five sites above. Read each before editing — the surrounding sentence differs at each one.

- [ ] **Step 2: Rewrite the four Rust comments**

`paigasus-iam-core/src/authn.rs:133-134` — this sits on `PrincipalContext`, the type every introspection path returns, so it is the most load-bearing of the five:

```rust
/// `role_grants` carries the principal's own role grants, sorted by `(scope_prn, role_key)`
/// (SMA-633). The OIDC `Introspect` path populates it; `IntrospectApiKey` leaves it empty by
/// decision (SMA-633 D2), so a reader must not treat an empty list from an API-key
/// introspection as "this service account holds no grants".
```

`adapters/http/dto.rs:242` and `adapters/grpc/convert.rs:398` — replace "empty until a later M3 task populates it" with a pointer to the same truth:

```rust
/// `role_grants` is whatever the application layer assembled: populated for the OIDC
/// `Introspect` path, empty for `IntrospectApiKey` (SMA-633 D2).
```

`application/authenticate_api_key.rs:258-260` — state the decision, not a missing implementation:

```rust
    /// `role_grants` stays EMPTY here by decision (SMA-633 D2), not by omission. This method
    /// runs on the gateway's per-request path (`paigasus-gateway`'s `require_iam_auth` and
    /// `require_authenticated` both call it), and nothing reads the field, so it does not pay
    /// for a `role_grant` read per request. Adding one also puts a `role_grant` outage on the
    /// gateway's 503 path — read SMA-633 D6 before changing this.
```

- [ ] **Step 3: Rewrite the TypeScript comment**

`ts/packages/paigasus-console-core/testing/fake-iam.ts:15` — the double's behaviour does **not** change (spec D1 defers the TypeScript work), only the reason:

```typescript
// This double returns an empty `role_grants`. Real IAM populates it for `Introspect` since
// SMA-633, but the resolver still discards it, so the double matches what the consoles
// actually consume. The follow-up that reads grants updates this double with it.
```

- [ ] **Step 4: Verify nothing broke**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo clippy -p paigasus-iam -p paigasus-iam-core --all-targets -- -D warnings && cargo fmt --check
cd .. && pnpm -C ts run fmt:check
```

Expected: PASS. `ts:fmt` is its own whole-tree gate, so run it after touching any TypeScript file, even a comment.

- [ ] **Step 5: Commit**

```bash
git add rs ts
git commit -m "docs(rs): correct the role_grants comments the population falsifies (SMA-633)"
```

---

### Task 6: Full gate run

**Files:**
- Modify: none, unless a gate fails.

**Interfaces:**
- Consumes: every earlier task.
- Produces: the evidence for the PR body.

- [ ] **Step 1: Run the affected graph the way CI does**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site \
  :http-extractor-envelope :input-liveness :promtool :observability-drift \
  :nats-permissions :release-parity :release-parity-py :release-parity-ts \
  :publish-metadata :version-lockstep :workflow-credentials :pyo3-stub-drift :ruff-ci \
  :next-public-free :test-e2e \
  --base origin/main \
  --include-relations
```

Expected: PASS. `:breaking` runs because the proto changed; a new enum value is additive, so it must not report a break. If it does, read its output before assuming the gate is wrong.

- [ ] **Step 2: Re-run the gates this machine's bash cannot serve**

No single local bash runs every gate on this Mac. After the run above, re-run these directly and read **their** verdict instead of the `moon ci` one:

```bash
/opt/homebrew/bin/bash ci/ruff/run.sh
/opt/homebrew/bin/bash ci/next-public/run.sh
/opt/homebrew/bin/bash ci/publish-metadata/run.sh
/bin/bash ci/affected-graph/run.sh
```

An empty stdout plus a one-line `declare: -A: invalid option` or `mapfile: command not found` on stderr means the wrong bash ran the gate. It is not a finding.

- [ ] **Step 3: Diagnose any failure before re-running**

If a task fails, **capture before you re-run** — a passing re-run overwrites the evidence:

```bash
cp .moon/cache/ciReport.json /tmp/sma-633-ciReport.json
jq '.actions[] | select(.status=="failed")
    | {label, error,
       exec: (.operations[] | select(.meta.type=="task-execution") | {command, exitCode})}' \
   /tmp/sma-633-ciReport.json
```

Then read `.moon/cache/states/<project>/<task>/stdout.log` and `stderr.log`. A `cargo metadata` error naming a `.napi-stage-<random>` path is a known CI-only concurrency flake, not a defect in this diff.

- [ ] **Step 4: Record the evidence**

Write the gate result into the PR body at Stage 6: which targets ran, which passed, and which were re-run under a different bash. Do not claim a gate passed that only skipped.

---

## Self-Review

**Spec coverage.** D1 (Rust only, no console change) — Tasks 2 and 5 Step 3 keep TypeScript behaviour fixed. D2 (API key stays empty) — Task 3 Step 3 and Task 5 Step 2. D3 (the port) — Task 2 Step 3. D4 (projection) — Task 2 Step 4. D5 (`resolve` untouched) — Task 2 changes only the `introspect`/`context_for` site. D6 (fail the call) — Task 2 Steps 1, 4 and 8. D7 (order) — Task 2 Steps 1, 4 and 8. D8 (no cap) — nothing to implement; the plan adds no limit. D9 (capability) — Task 4. D10 (SMA-632 first) — Task 1. § 4's doc comments — Task 5. § 5's test list — Tasks 2 and 3.

**Placeholders.** None. Every code step carries the code. Two steps deliberately instruct the implementer to read a file before editing (the `AuthzError` variant name in Task 2 Step 1, the proto header comment in Task 4 Step 3), because the exact text there was not measured; each says what to do with what it finds.

**Type consistency.** `AuthenticateToken::new` takes `grants` fifth in every call site across Tasks 2 and 4. `RoleGrantRef` is `{ scope_prn: String, role_key: String }` everywhere. `Capability::IamAuthnGrants` is spelled the same in Task 4 Steps 1, 5 and 9. `backend_authz` is defined in Task 2 Step 3 and used in Step 4.
