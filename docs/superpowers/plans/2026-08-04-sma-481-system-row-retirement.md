# SMA-481 — System Row Retirement Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give an operator a safe, audited, Root-only way to remove the `policy` and `role` rows of a system role that the code catalog no longer defines, so a retired role stops granting.

**Architecture:** One new HTTP endpoint, `POST /v1/authz/system-policies/{id}/retire`, over a new `SystemRetirementService` and a single new narrow port `SystemRowRetirer`. The public `PolicyStore::put_in`/`delete_in` `SystemImmutable` guards are untouched — retirement uses its own privileged adapter, exactly as SMA-477's `reconcile_system` does. Retirement refuses while any grant of the role survives, so it only ever deletes provably inert rows; retiring a *static* policy is a real decision change and requires explicit acknowledgement.

**Tech Stack:** Rust (edition 2024, rust-version 1.95), SeaORM over Postgres, axum, Cedar (`cedar-policy`), `metrics`, `tokio`/`async-trait`, `cargo nextest`, Moon.

**Spec:** `docs/superpowers/specs/2026-08-04-sma-481-system-row-retirement-design.md` — read it before Task 1. Decision references below (D1–D12) point into its §2.

## Global Constraints

- Every source file opens with `// SPDX-License-Identifier: Apache-2.0`.
- Rust crates are **edition 2024 + rust-version 1.95**.
- Conventional commits with a workspace scope: `feat(rs):`, `fix(rs):`, `docs(docs):`. Subject must **start lowercase** and be **≤100 chars**. Body lines ≤100 chars. Never write `#NNN` in a commit body (it breaks `footer-leading-blank`); write "owner/repo PR NNN".
- Never bypass the commit hook with `--no-verify`.
- `cargo nextest` needs `--no-tests=pass` on a crate with no tests.
- Prefix shell commands with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` so the repo-pinned toolchain resolves (shims FIRST).
- Run all commands from the worktree root: `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-481-iam-retired-system-role-retirement`.
- Doc comments carry the *reasoning*, not just the *what* — match the density of the surrounding modules (`pg_policies.rs`, `bootstrap.rs`, `dead_letters.rs` are the house style).
- Integration tests under `rs/crates/services/paigasus-iam/tests/` require Docker; unit tests must stay Docker-free.

## File Structure

**Create**
- `rs/crates/libs/paigasus-iam-core/src/authz/retirement.rs` — pure types + the `SystemRowRetirer` port (Task 4)
- `rs/crates/services/paigasus-iam/src/adapters/persistence/pg_system_row_retirer.rs` — the Postgres impl (Task 5)
- `rs/crates/services/paigasus-iam/src/application/system_retirement.rs` — the use case (Task 6)
- `rs/crates/services/paigasus-iam/src/adapters/http/system_retirement.rs` — the endpoint (Task 7)
- `rs/crates/services/paigasus-iam/tests/authz_system_retirement_pg.rs` — integration tests (Task 9)

**Modify**
- `rs/crates/libs/paigasus-iam-core/src/authz/action.rs` — new action (Task 1)
- `rs/crates/libs/paigasus-iam-core/src/authz/schema.rs` — schema action list (Task 1)
- `rs/crates/libs/paigasus-iam-core/src/authz/roles.rs` — revision + pinned hash (Task 1)
- `rs/crates/libs/paigasus-iam-core/src/authz/mod.rs` + `src/lib.rs` — re-exports (Task 4)
- `rs/crates/services/paigasus-iam/src/application/error.rs` — two variants + codes (Task 2)
- `rs/crates/services/paigasus-iam/src/adapters/persistence/pg_role_grants.rs` — FK remap (Task 3)
- `rs/crates/services/paigasus-iam/src/adapters/persistence/mod.rs` — export the adapter (Task 5)
- `rs/crates/services/paigasus-iam/src/application/mod.rs` — export the service (Task 6)
- `rs/crates/services/paigasus-iam/src/application/bootstrap.rs` — `pub(crate)` helper + WARN pointer (Tasks 6, 8)
- `rs/crates/services/paigasus-iam/src/adapters/http/mod.rs` — `AppState` field, router merge, merge test (Task 7)
- `rs/crates/services/paigasus-iam/src/main.rs` — one `describe_counter!` (Task 7)
- `rs/crates/libs/paigasus-observability/src/names.rs` — one metric name const (Task 7)
- `docs/ops/RUNBOOK-observability.md` — the retirement procedure (Task 8)

---

### Task 1: Action catalog, Cedar schema, and the starter-policy revision bump

Adding a write action changes the generated `forbid-archived-writes` source, which is pinned by a content hash. Doing this first means every later task compiles against the final catalog.

**Files:**
- Modify: `rs/crates/libs/paigasus-iam-core/src/authz/action.rs`
- Modify: `rs/crates/libs/paigasus-iam-core/src/authz/schema.rs:19-27`
- Modify: `rs/crates/libs/paigasus-iam-core/src/authz/roles.rs:54` and `:82`

**Interfaces:**
- Produces: `Action::RetireSystemPolicy` (wire string `"RetireSystemPolicy"`, `is_write() == true`, `is_restore() == false`); `STARTER_POLICY_REVISION == 2`.

- [ ] **Step 1: Write the failing tests**

Add to the `#[cfg(test)] mod tests` in `rs/crates/libs/paigasus-iam-core/src/authz/roles.rs`:

```rust
/// The new action must actually reach the generated forbid list — the whole reason
/// STARTER_POLICY_REVISION has to move. A hand-updated hash with the action missing from
/// `Action::ALL` would otherwise look green.
#[test]
fn the_retire_action_is_in_the_generated_forbid_source() {
    assert!(
        forbid_archived_writes_source().contains(r#"Pgs::Iam::Action::"RetireSystemPolicy""#),
        "RetireSystemPolicy is a write action, so it must appear in forbid-archived-writes"
    );
}
```

Add to the `#[cfg(test)] mod tests` in `rs/crates/libs/paigasus-iam-core/src/authz/action.rs`:

```rust
#[test]
fn retire_system_policy_is_a_non_restore_write() {
    assert_eq!(Action::RetireSystemPolicy.as_wire(), "RetireSystemPolicy");
    assert!(Action::RetireSystemPolicy.is_write(), "retirement deletes policy and role rows");
    assert!(!Action::RetireSystemPolicy.is_restore());
    assert!(Action::ALL.contains(&Action::RetireSystemPolicy), "must be in the catalog or the forbid list misses it");
}
```

Add to the `#[cfg(test)] mod tests` in `rs/crates/libs/paigasus-iam-core/src/authz/schema.rs`:

```rust
/// SCHEMA_SRC's action list is hand-maintained. If the new action is missing there,
/// `validate_policy` rejects the newly-generated forbid-archived-writes source and boot fails.
#[test]
fn the_retire_action_validates_against_the_embedded_schema() {
    assert!(validate_policy(r#"permit(principal, action == Pgs::Iam::Action::"RetireSystemPolicy", resource);"#).is_ok());
}
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cargo nextest run -p paigasus-iam-core -E 'test(retire_system_policy) or test(the_retire_action)' --no-tests=pass
```

Expected: FAIL — `no variant named RetireSystemPolicy found for enum Action` (a compile error at this stage is the expected failure).

- [ ] **Step 3: Add the action to the catalog**

In `action.rs`, add the variant after `DiscardOutboxDeadLetter` in the enum, in `Action::ALL`, and in `as_wire()`:

```rust
    DiscardOutboxDeadLetter,
    /// Retire an orphaned system-owned policy row (and its `role` row, if any) whose id the
    /// code catalog no longer defines — SMA-481. Root-only, enforced in
    /// `SystemRetirementService` rather than the Cedar schema, exactly like the three
    /// dead-letter actions above.
    RetireSystemPolicy,
```

```rust
        Action::DiscardOutboxDeadLetter,
        Action::RetireSystemPolicy,
```

```rust
            Action::DiscardOutboxDeadLetter => "DiscardOutboxDeadLetter",
            Action::RetireSystemPolicy => "RetireSystemPolicy",
```

Add it to the `is_write()` `=> true` arm, alongside `DiscardOutboxDeadLetter`:

```rust
            | Action::DiscardOutboxDeadLetter
            | Action::RetireSystemPolicy
            | Action::InvokeModel => true,
```

Find the `Action::ALL.len()` assertion (around `action.rs:290`) and update the count from `39` to `40`, extending its message to mention SMA-481's action.

- [ ] **Step 4: Add the action to the Cedar schema**

In `schema.rs`, extend `SCHEMA_SRC`'s action list — add `RetireSystemPolicy` after `DiscardOutboxDeadLetter`:

```
         IssueApiKey, RevokeApiKey, ListApiKeys, ListAuditLog, ListOutboxDeadLetters,
         ReplayOutboxDeadLetter, DiscardOutboxDeadLetter, RetireSystemPolicy, InvokeModel
```

- [ ] **Step 5: Bump the revision and re-pin the hash**

In `roles.rs`, set `pub const STARTER_POLICY_REVISION: u32 = 2;` and extend its doc comment with a line noting SMA-481 added `RetireSystemPolicy`.

Run the pin test to learn the new hash:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cargo nextest run -p paigasus-iam-core -E 'test(starter_policy_content_is_pinned)' --no-tests=pass 2>&1 | grep -A 4 'assertion'
```

Copy the **actual** hash from the failure output into `EXPECTED_STARTER_CONTENT_HASH`. Do not compute it by hand.

- [ ] **Step 6: Run the tests to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cargo nextest run -p paigasus-iam-core --no-tests=pass
```

Expected: PASS, all tests.

- [ ] **Step 7: Commit**

```bash
git add rs/crates/libs/paigasus-iam-core/src/authz/action.rs \
        rs/crates/libs/paigasus-iam-core/src/authz/schema.rs \
        rs/crates/libs/paigasus-iam-core/src/authz/roles.rs
git commit -m "feat(rs): add the RetireSystemPolicy action and bump the starter revision (SMA-481)"
```

---

### Task 2: Error variants for the two new refusals

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/application/error.rs`

**Interfaces:**
- Produces: `TenancyError::NotSystemOwned(String)` (code `"not-system-owned"`), `TenancyError::FleetNotConverged` (code `"fleet-not-converged"`), both `ErrorClass::Precondition`.

- [ ] **Step 1: Write the failing test**

Add to `error.rs`'s `#[cfg(test)] mod tests`:

```rust
/// Both retirement refusals are Preconditions, not Conflicts. They render 409 either way
/// today, but ErrorClass is what a future gRPC mirror translates — and SystemImmutable, the
/// third refusal this same endpoint can return, is already Precondition (see the assertion
/// above). Two sibling refusals on one endpoint must not diverge in class.
#[test]
fn the_retirement_refusals_share_system_immutable_s_class_and_have_stable_codes() {
    assert_eq!(TenancyError::NotSystemOwned("p1".to_string()).class(), ErrorClass::Precondition);
    assert_eq!(TenancyError::FleetNotConverged.class(), ErrorClass::Precondition);
    assert_eq!(TenancyError::SystemImmutable("p1".to_string()).class(), ErrorClass::Precondition);

    assert_eq!(TenancyError::NotSystemOwned("p1".to_string()).code(), "not-system-owned");
    assert_eq!(TenancyError::FleetNotConverged.code(), "fleet-not-converged");
}
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cargo nextest run -p paigasus-iam -E 'test(the_retirement_refusals_share)' --no-tests=pass
```

Expected: FAIL — `no variant named NotSystemOwned found for enum TenancyError`.

- [ ] **Step 3: Add the variants**

In `error.rs`, add next to `SystemImmutable`:

```rust
    /// The row at this id exists but is not system-owned, so `RetireSystemPolicy` refuses it
    /// (SMA-481 D7). Retirement must not become a second, differently-audited delete path for
    /// operator-authored policies — `DeletePolicy` already serves those and applies its own
    /// authorization. Raised for a non-system `policy` row AND for a non-system `role` row at
    /// the same id.
    #[error("not a system-owned row: {0}")]
    NotSystemOwned(String),
    /// At least one remaining system-owned row was last written by a binary older than this
    /// one, so the fleet has not converged past the release that dropped the retiring id
    /// (SMA-481 D11). Retiring now would be silently undone: `classify_starter_policy`
    /// classifies an absent row as `Absent` BEFORE the revision guard runs, so any replica
    /// whose catalog still defines the id re-seeds it unconditionally.
    #[error("the fleet has not converged past this binary's starter policy revision")]
    FleetNotConverged,
```

Add to `code()`:

```rust
            Self::NotSystemOwned(_) => "not-system-owned",
            Self::FleetNotConverged => "fleet-not-converged",
```

Add both to the `ErrorClass::Precondition` arm of `class()`, alongside `SystemImmutable(_)`.

- [ ] **Step 4: Run the test to verify it passes**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cargo nextest run -p paigasus-iam -E 'test(the_retirement_refusals_share) or test(error)' --no-tests=pass
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/application/error.rs
git commit -m "feat(rs): add the two retirement refusal errors (SMA-481)"
```

---

### Task 3: Map the lost-race FK violation to `UnknownRole`

D6's lock defers the FK conflict; it does not remove it. When retirement commits with the role row gone, a concurrent grant from an older replica resumes, fails `fk_role_grant_role`, and today becomes a `500`.

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/persistence/pg_role_grants.rs:74-76` and its `grant_in`

**Interfaces:**
- Produces: `PgRoleGrantStore::grant_in` returning `AuthzError::UnknownRole(role_key)` on an `fk_role_grant_role` violation.

- [ ] **Step 1: Write the failing test**

This needs Postgres. Add to `rs/crates/services/paigasus-iam/tests/authz_grants_pg.rs` if it exists; otherwise create `rs/crates/services/paigasus-iam/tests/authz_grant_fk_pg.rs` with the same harness header the other `*_pg.rs` tests use (copy the container/setup preamble from `tests/authz_bootstrap.rs` verbatim — do not invent a new harness):

```rust
// SPDX-License-Identifier: Apache-2.0

//! A grant against a role key with no `role` row must report the role as unknown, not as an
//! internal error (SMA-481 D6). This is the state a concurrent grant lands in after a
//! retirement commits: it blocked on the `FOR UPDATE` lock, then resumed to find no parent row.

#[tokio::test]
async fn granting_a_role_with_no_row_reports_unknown_role_not_internal() {
    let (_container, db) = setup().await;
    let store = PgRoleGrantStore::new(db.clone(), test_generations());
    let uow = SeaOrmUnitOfWork::new(db);

    let grant = RoleGrant {
        id: Uuid::now_v7(),
        principal_id: seeded_principal_id(),
        role_key: "a_role_with_no_row".to_string(),
        scope: GrantScope::Root,
        linked_policy_id: format!("grant:{}", Uuid::now_v7()),
        created_at: Utc::now(),
    };

    let tx = uow.begin().await.unwrap();
    let err = store.grant_in(&*tx, &grant).await.expect_err("no role row means no grant");
    assert!(
        matches!(err, AuthzError::UnknownRole(ref k) if k == "a_role_with_no_row"),
        "an FK violation on fk_role_grant_role means the role is gone, not that the backend broke; got {err:?}"
    );
}
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cargo nextest run -p paigasus-iam -E 'test(granting_a_role_with_no_row)' --no-tests=pass
```

Expected: FAIL — the error is `AuthzError::Backend(..)`, not `UnknownRole`.

- [ ] **Step 3: Add the FK branch**

In `pg_role_grants.rs`, change `grant_in` to attribute the violation. Leave the private `map_err` alone for every other call site:

```rust
    async fn grant_in(&self, tx: &dyn Transaction, g: &RoleGrant) -> Result<(), AuthzError> {
        let txn = recover_txn(tx).map_err(map_txn_err)?;
        grant_to_model(g).insert(txn).await.map_err(|e| map_grant_err(e, &g.role_key))?;
        Ok(())
    }
```

And add, next to the private `map_err`:

```rust
/// `grant_in`'s error mapping. The private [`map_err`] above collapses every `DbErr` into
/// `Backend`, which is right for the reads and deletes around it but wrong here: a violation of
/// `fk_role_grant_role` means the `role` row this grant names does not exist, which is exactly
/// what `RoleService::grant` reports as `UnknownRole` before it ever reaches the database.
///
/// This is not a theoretical branch. SMA-481 D6: a retirement holds the role row `FOR UPDATE`
/// while a concurrent grant from a replica on an OLDER binary — one whose code catalog still
/// defines the retired role — blocks behind it. When the retirement commits with the row
/// deleted, that grant resumes, re-runs its FK check and fails. Without this mapping the caller
/// gets a `500 internal error` for a condition the service understands perfectly well.
fn map_grant_err(e: DbErr, role_key: &str) -> AuthzError {
    match e.sql_err() {
        Some(SqlErr::ForeignKeyConstraintViolation(_)) => AuthzError::UnknownRole(role_key.to_string()),
        _ => map_err(e),
    }
}
```

Add `SqlErr` to the `sea_orm` import list at the top of the file.

- [ ] **Step 4: Run the test to verify it passes**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cargo nextest run -p paigasus-iam -E 'test(granting_a_role_with_no_row)' --no-tests=pass
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/adapters/persistence/pg_role_grants.rs \
        rs/crates/services/paigasus-iam/tests/
git commit -m "fix(rs): report a missing role row as UnknownRole, not an internal error (SMA-481)"
```

---

### Task 4: The `SystemRowRetirer` port and its pure types

One narrow port with one production impl and one fake — SMA-477 D5 applied properly. Adding these methods to `PolicyStore`/`RoleGrantStore` would force `unimplemented!()` stubs into fourteen impls, which is the exact cost D5 rejected.

**Files:**
- Create: `rs/crates/libs/paigasus-iam-core/src/authz/retirement.rs`
- Modify: `rs/crates/libs/paigasus-iam-core/src/authz/mod.rs`, `rs/crates/libs/paigasus-iam-core/src/lib.rs`

**Interfaces:**
- Produces: `SystemRowRetirer` trait; `StoredPolicy { policy_id, kind, source, description, system }`, `StoredRole { key, system }`, `GrantRef { id, principal_prn, scope_prn }`, `SurvivingGrants { grants, total }`, `RetireOutcome`.
- Consumes: `Action::RetireSystemPolicy` (Task 1) is not used here; `Transaction`, `AuthzError`, `PolicyKind` are existing core types.

- [ ] **Step 1: Write the failing test**

Create the file with only its tests plus the type declarations they need, so the failure is real:

```rust
// SPDX-License-Identifier: Apache-2.0

//! Retirement of orphaned system-owned rows (SMA-481). Pure types + the one narrow port the
//! `SystemRetirementService` drives.
//!
//! **Why its own port.** SMA-477 D5 kept boot's reconciliation off `PolicyStore` because that
//! trait "has seven implementations, six of which are test fakes on the request path that would
//! gain a method nothing calls". `RoleGrantStore` has seven too. Spreading retirement's seven
//! methods across those traits would force fourteen `unimplemented!()` stubs — the exact cost
//! D5 rejected. One purpose-built port has one production impl and one fake.
//!
//! **What this port must never become.** It bypasses `PolicyStore::delete_in`'s
//! `SystemImmutable` guard, which is precisely what must keep holding for the public
//! `DeletePolicy` API (D3). Nothing reachable from an ordinary API request may hold one.

#[cfg(test)]
mod tests {
    use super::*;

    /// `Retired` is the only success. The two refusals are `Ok` values rather than errors —
    /// they are the system working correctly and saying so — which makes discarding the value
    /// the one real hazard. `#[must_use]` plus this helper is the guard.
    #[test]
    fn only_the_retired_outcome_reports_success() {
        let retired = RetireOutcome::Retired {
            policy_id: "legacy_auditor".to_string(),
            kind: PolicyKind::Template,
            role_deleted: true,
        };
        assert!(retired.is_retired());

        let blocked = RetireOutcome::Blocked {
            role_key: "legacy_auditor".to_string(),
            grants: vec![],
            total: 3,
            truncated: false,
        };
        assert!(!blocked.is_retired(), "a blocked retirement wrote nothing and must never read as success");

        let unacked = RetireOutcome::NeedsAcknowledgement {
            policy_id: "legacy_forbid".to_string(),
            kind: PolicyKind::Static,
            source: "forbid(principal, action, resource);".to_string(),
            description: String::new(),
        };
        assert!(!unacked.is_retired());
    }

    /// The cap is what keeps an unbounded grant list off the wire and out of memory. The
    /// adapter selects `cap + 1` rows so the service can detect truncation without a second
    /// COUNT-shaped round trip being the only source of truth.
    #[test]
    fn truncation_is_derived_from_the_cap_not_guessed() {
        let under = SurvivingGrants { grants: vec![], total: 3 };
        assert!(!under.truncated(2), "3 total with 2 returned is truncated");
        // `truncated` compares what was RETURNED against the cap, so build the returned list.
        let returned: Vec<GrantRef> = (0..3).map(|i| GrantRef {
            id: format!("00000000-0000-0000-0000-00000000000{i}"),
            principal_prn: "prn:pgs:iam:::principal/p".to_string(),
            scope_prn: "prn:pgs:iam:::root/root".to_string(),
        }).collect();
        let exact = SurvivingGrants { grants: returned.clone(), total: 3 };
        assert!(!exact.truncated(3), "3 returned under a cap of 3 is complete");
        assert!(exact.truncated(2), "3 returned under a cap of 2 means more exist");
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cargo nextest run -p paigasus-iam-core -E 'test(only_the_retired_outcome) or test(truncation_is_derived)' --no-tests=pass
```

Expected: FAIL — `cannot find type RetireOutcome in this scope` (compile error).

- [ ] **Step 3: Write the types and the port**

Insert above the `#[cfg(test)]` block in the same file:

```rust
use super::model::{AuthzError, PolicyKind};
use crate::ports::Transaction;
use async_trait::async_trait;
use std::time::Duration;

/// A stored `policy` row, as the retirement path needs to see it. Deliberately not
/// `PolicyDocument`: retirement cares about `system` and the content it is about to destroy,
/// never about timestamps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredPolicy {
    pub policy_id: String,
    pub kind: PolicyKind,
    pub source: String,
    pub description: String,
    pub system: bool,
}

/// A stored `role` row. Only `system` is load-bearing — D7 refuses a non-system role row at a
/// system policy's id for the same reason it refuses a non-system policy row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredRole {
    pub key: String,
    pub system: bool,
}

/// One surviving grant, projected to what a refusal needs to name it. Stringly-typed on
/// purpose: this crosses straight into an HTTP body and never back into a decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrantRef {
    pub id: String,
    pub principal_prn: String,
    pub scope_prn: String,
}

/// Surviving grants of a retiring key: a capped page, plus the true total.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurvivingGrants {
    /// At most `cap` rows, ordered by id so a refusal lists them deterministically.
    pub grants: Vec<GrantRef>,
    /// Every surviving grant, not just the returned page.
    pub total: u64,
}

impl SurvivingGrants {
    /// Whether more grants exist than were returned under `cap`.
    #[must_use]
    pub fn truncated(&self, cap: u64) -> bool {
        self.total > cap
    }
}

/// What a retirement attempt did. Two of the three wrote NOTHING — they are the system working
/// correctly and saying so, which is why they are `Ok` values rather than `TenancyError`
/// variants (D5). `#[must_use]` guards the one real hazard that creates: a caller writing
/// `svc.retire(..).await?;` and discarding the value would otherwise treat a refusal as success.
#[must_use]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetireOutcome {
    /// The chain was removed. `role_deleted` is false for a retired static policy.
    Retired { policy_id: String, kind: PolicyKind, role_deleted: bool },
    /// Nothing was written: grants of this role survive and must be revoked first (D4).
    Blocked { role_key: String, grants: Vec<GrantRef>, total: u64, truncated: bool },
    /// Nothing was written: this is a STATIC policy, so removing it changes decisions
    /// fleet-wide, and the caller has not acknowledged that (D4). Carries the content that
    /// would be destroyed, so the refusal doubles as the operator's preview.
    NeedsAcknowledgement { policy_id: String, kind: PolicyKind, source: String, description: String },
}

impl RetireOutcome {
    /// Whether rows were actually removed.
    #[must_use]
    pub fn is_retired(&self) -> bool {
        matches!(self, RetireOutcome::Retired { .. })
    }
}

/// The privileged, operator-initiated removal path for orphaned system-owned rows.
///
/// Every method that reads a row LOCKS it. That is not incidental: `fk_role_template` and
/// `fk_role_grant_role` are both restrict, so an unlocked read lets a concurrent insert from an
/// older replica turn a delete into an unmapped foreign-key error between the check and the
/// write (D6).
#[async_trait]
pub trait SystemRowRetirer: Send + Sync {
    /// Opens the retirement transaction with `SET LOCAL lock_timeout` already applied. A
    /// dedicated constructor because [`Transaction`] exposes no way to set it after the fact,
    /// and this is an operator-triggered request: it must fail with a message rather than hang
    /// behind a concurrent writer's row lock.
    async fn begin_retirement(&self, lock_timeout: Duration) -> Result<Box<dyn Transaction>, AuthzError>;

    /// Reads the `policy` row `FOR UPDATE`. Locked first, because it is the FK *parent* of the
    /// role row: an older replica's `reconcile_role` INSERT takes `FOR KEY SHARE` on it, and
    /// nothing else would block that when no role row exists to lock.
    async fn lock_policy_in(&self, tx: &dyn Transaction, policy_id: &str) -> Result<Option<StoredPolicy>, AuthzError>;

    /// Reads `key`'s `role` row `FOR UPDATE`, blocking any concurrent `role_grant` insert
    /// against it for the transaction's duration (D6).
    async fn lock_role_in(&self, tx: &dyn Transaction, key: &str) -> Result<Option<StoredRole>, AuthzError>;

    /// Up to `cap` surviving grants of `role_key`, ordered by id, plus the true total.
    async fn surviving_grants_in(&self, tx: &dyn Transaction, role_key: &str, cap: u64) -> Result<SurvivingGrants, AuthzError>;

    /// The lowest `starter_revision` across all remaining system-owned `policy` rows, or `None`
    /// if any is NULL. D11's proof-of-convergence input: a value below this binary's
    /// `STARTER_POLICY_REVISION` means some replica older than this one wrote a row recently,
    /// so retiring now risks being silently undone. Read outside the transaction — it is
    /// advisory evidence, not an invariant.
    async fn min_starter_revision(&self) -> Result<Option<u32>, AuthzError>;

    /// Deletes the `role` row; returns whether one existed.
    async fn delete_role_in(&self, tx: &dyn Transaction, key: &str) -> Result<bool, AuthzError>;

    /// Deletes the `policy` row, bypassing `PolicyStore::delete_in`'s `SystemImmutable` guard.
    /// Callers must have established the row is orphaned and unreferenced (D3/D7).
    async fn delete_policy_in(&self, tx: &dyn Transaction, policy_id: &str) -> Result<bool, AuthzError>;
}
```

- [ ] **Step 4: Wire the module into the crate**

In `rs/crates/libs/paigasus-iam-core/src/authz/mod.rs`, add `pub mod retirement;` next to the other `pub mod` lines.

In `rs/crates/libs/paigasus-iam-core/src/lib.rs`, re-export alongside the other authz re-exports:

```rust
pub use authz::retirement::{GrantRef, RetireOutcome, StoredPolicy, StoredRole, SurvivingGrants, SystemRowRetirer};
```

- [ ] **Step 5: Run the tests to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cargo nextest run -p paigasus-iam-core --no-tests=pass && cargo clippy -p paigasus-iam-core -- -D warnings
```

Expected: PASS, no clippy warnings.

- [ ] **Step 6: Commit**

```bash
git add rs/crates/libs/paigasus-iam-core/src/authz/retirement.rs \
        rs/crates/libs/paigasus-iam-core/src/authz/mod.rs \
        rs/crates/libs/paigasus-iam-core/src/lib.rs
git commit -m "feat(rs): add the SystemRowRetirer port and its retirement types (SMA-481)"
```

---

### Task 5: `PgSystemRowRetirer` — the Postgres implementation

**Files:**
- Create: `rs/crates/services/paigasus-iam/src/adapters/persistence/pg_system_row_retirer.rs`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/persistence/mod.rs`
- Test: `rs/crates/services/paigasus-iam/tests/authz_system_retirement_pg.rs` (created here, extended in Task 9)

**Interfaces:**
- Consumes: `SystemRowRetirer`, `StoredPolicy`, `StoredRole`, `GrantRef`, `SurvivingGrants` (Task 4).
- Produces: `PgSystemRowRetirer::new(db: DatabaseConnection) -> Self`, exported from `adapters::persistence`.

- [ ] **Step 1: Write the failing test**

Create `rs/crates/services/paigasus-iam/tests/authz_system_retirement_pg.rs`. Copy the Docker/Postgres harness preamble verbatim from `tests/authz_bootstrap.rs` (container setup, `Migrator::up`, connection helper) — do not write a new one.

```rust
// SPDX-License-Identifier: Apache-2.0

//! Postgres-level behaviour of the SMA-481 retirement path: the FK ordering the schema forces,
//! the locks that make the checks trustworthy, and the deletes themselves.

/// Seeds a system-owned template + role at a NON-code-defined id. There is deliberately no
/// supported path that writes a `role` row for a key the code catalog does not define — that
/// absence IS the bug SMA-481 exists for — so the fixture inserts directly.
async fn seed_orphan_chain(db: &DatabaseConnection, id: &str) { /* direct SeaORM inserts */ }

#[tokio::test]
async fn the_fk_ordering_is_real_and_the_retirer_respects_it() {
    let (_c, db) = setup().await;
    seed_orphan_chain(&db, "legacy_auditor").await;
    let retirer = PgSystemRowRetirer::new(db.clone());

    // Deleting the policy while the role row still references it must fail: fk_role_template.
    let tx = retirer.begin_retirement(Duration::from_secs(5)).await.unwrap();
    retirer
        .delete_policy_in(&*tx, "legacy_auditor")
        .await
        .expect_err("fk_role_template must block a policy delete while its role row survives");
    drop(tx);

    // role first, then policy — the only order the schema permits.
    let tx = retirer.begin_retirement(Duration::from_secs(5)).await.unwrap();
    assert!(retirer.delete_role_in(&*tx, "legacy_auditor").await.unwrap());
    assert!(retirer.delete_policy_in(&*tx, "legacy_auditor").await.unwrap());
    tx.commit().await.unwrap();

    assert!(retirer.lock_policy_in(&*retirer.begin_retirement(Duration::from_secs(5)).await.unwrap(), "legacy_auditor").await.unwrap().is_none());
}

#[tokio::test]
async fn surviving_grants_are_capped_and_report_the_true_total() {
    let (_c, db) = setup().await;
    seed_orphan_chain(&db, "legacy_auditor").await;
    seed_grants(&db, "legacy_auditor", 5).await;
    let retirer = PgSystemRowRetirer::new(db.clone());

    let tx = retirer.begin_retirement(Duration::from_secs(5)).await.unwrap();
    let survivors = retirer.surviving_grants_in(&*tx, "legacy_auditor", 2).await.unwrap();
    assert_eq!(survivors.grants.len(), 2, "the page is capped");
    assert_eq!(survivors.total, 5, "the total is the truth, not the page size");
    assert!(survivors.truncated(2));

    let ids: Vec<&str> = survivors.grants.iter().map(|g| g.id.as_str()).collect();
    let mut sorted = ids.clone();
    sorted.sort_unstable();
    assert_eq!(ids, sorted, "ordered by id so a refusal lists them deterministically");
}

#[tokio::test]
async fn min_starter_revision_reports_null_as_none() {
    let (_c, db) = setup().await;
    // A pre-m0010 row: system-owned with a NULL starter_revision.
    seed_system_policy_with_revision(&db, "legacy_forbid", None).await;
    let retirer = PgSystemRowRetirer::new(db.clone());
    assert_eq!(retirer.min_starter_revision().await.unwrap(), None, "a NULL revision is unprovable, not zero");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cargo nextest run -p paigasus-iam -E 'test(the_fk_ordering_is_real) or test(surviving_grants_are_capped) or test(min_starter_revision)' --no-tests=pass
```

Expected: FAIL — `cannot find struct PgSystemRowRetirer`.

- [ ] **Step 3: Write the adapter**

Create `pg_system_row_retirer.rs`. Use `super::entities::{policy, role, role_grant}`, `super::uow::{SeaOrmTransaction, recover_txn}`, and `crate::adapters::persistence::pg_policies::map_db_err`. Key implementation points:

```rust
#[async_trait]
impl SystemRowRetirer for PgSystemRowRetirer {
    async fn begin_retirement(&self, lock_timeout: Duration) -> Result<Box<dyn Transaction>, AuthzError> {
        let txn = self.db.begin().await.map_err(map_db_err)?;
        // Mirrors `PgPolicyStore::reconcile_system`. Postgres takes an interval literal, so the
        // duration is rendered in milliseconds — an operator-triggered request must fail with a
        // message rather than hang behind a concurrent writer's row lock.
        txn.execute_unprepared(&format!("SET LOCAL lock_timeout = '{}ms';", lock_timeout.as_millis()))
            .await
            .map_err(map_db_err)?;
        Ok(Box::new(SeaOrmTransaction { txn }))
    }

    async fn lock_policy_in(&self, tx: &dyn Transaction, policy_id: &str) -> Result<Option<StoredPolicy>, AuthzError> {
        let txn = recover_txn(tx).map_err(map_txn_err)?;
        let found = policy::Entity::find_by_id(policy_id.to_string()).lock_exclusive().one(txn).await.map_err(map_db_err)?;
        found.map(to_stored_policy).transpose()
    }
    // lock_role_in: role::Entity::find_by_id(..).lock_exclusive().one(txn)

    async fn surviving_grants_in(&self, tx: &dyn Transaction, role_key: &str, cap: u64) -> Result<SurvivingGrants, AuthzError> {
        let txn = recover_txn(tx).map_err(map_txn_err)?;
        // The COUNT and the page run on the SAME transaction that already holds the role row's
        // FOR UPDATE lock, so no grant can appear between them (D6).
        let total = role_grant::Entity::find()
            .filter(role_grant::Column::RoleKey.eq(role_key))
            .count(txn)
            .await
            .map_err(map_db_err)?;
        let models = role_grant::Entity::find()
            .filter(role_grant::Column::RoleKey.eq(role_key))
            .order_by_asc(role_grant::Column::Id)
            .limit(cap)
            .all(txn)
            .await
            .map_err(map_db_err)?;
        Ok(SurvivingGrants { grants: models.into_iter().map(to_grant_ref).collect(), total })
    }

    async fn min_starter_revision(&self) -> Result<Option<u32>, AuthzError> {
        // Any NULL means "unprovable", never "zero" — a pre-m0010 row proves nothing about
        // which binary last wrote it, and reading it as 0 would be the safe-sounding direction
        // that silently permits the retirement D11 exists to defer.
        let revisions: Vec<Option<i32>> = policy::Entity::find()
            .select_only()
            .column(policy::Column::StarterRevision)
            .filter(policy::Column::System.eq(true))
            .into_tuple()
            .all(&self.db)
            .await
            .map_err(map_db_err)?;
        if revisions.iter().any(Option::is_none) {
            return Ok(None);
        }
        Ok(revisions.into_iter().flatten().map(|r| u32::try_from(r).unwrap_or(0)).min())
    }
    // delete_role_in / delete_policy_in: Entity::delete_by_id(..).exec(txn), rows_affected > 0
}
```

`to_stored_policy` maps the `kind` column via the same `{static, template}` parse `pg_policies.rs::kind_from_str` uses — a value outside that set is a data-integrity break and must surface as `Backend`, never a silent default. Add a `pub(crate)` re-export of `kind_from_str` from `pg_policies.rs` rather than duplicating the match.

`to_grant_ref` renders `scope_prn` from the stored `scope_node_prn` column and `principal_prn` from `principal_id` using the same `PrincipalId::from_uuid(..).prn().canonical()` path `pg_role_grants.rs::model_to_grant` uses.

Export from `adapters/persistence/mod.rs`:

```rust
mod pg_system_row_retirer;
pub use pg_system_row_retirer::PgSystemRowRetirer;
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cargo nextest run -p paigasus-iam -E 'test(the_fk_ordering_is_real) or test(surviving_grants_are_capped) or test(min_starter_revision)' --no-tests=pass
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/adapters/persistence/pg_system_row_retirer.rs \
        rs/crates/services/paigasus-iam/src/adapters/persistence/mod.rs \
        rs/crates/services/paigasus-iam/src/adapters/persistence/pg_policies.rs \
        rs/crates/services/paigasus-iam/tests/authz_system_retirement_pg.rs
git commit -m "feat(rs): add the Postgres system-row retirer adapter (SMA-481)"
```

---

### Task 6: `SystemRetirementService` — the use case

The heart of the change. Every guard, in order, with fake-based tests that pin each one.

**Files:**
- Create: `rs/crates/services/paigasus-iam/src/application/system_retirement.rs`
- Modify: `rs/crates/services/paigasus-iam/src/application/mod.rs`, `rs/crates/services/paigasus-iam/src/application/bootstrap.rs`

**Interfaces:**
- Consumes: `SystemRowRetirer` + types (Task 4); `TenancyError::NotSystemOwned`/`FleetNotConverged` (Task 2); `Action::RetireSystemPolicy` + `STARTER_POLICY_REVISION` (Task 1).
- Produces: `SystemRetirementService::new(SystemRetirementDeps) -> Self`, `SystemRetirementService::retire(&self, actor: &Prn, id: &str, ack: bool) -> Result<RetireOutcome, TenancyError>`; consts `GRANT_LIST_CAP: u64 = 100`, `LOCK_TIMEOUT: Duration = Duration::from_secs(5)`.

- [ ] **Step 1: Make `truncate_audited_text` reusable**

In `application/bootstrap.rs`, change `fn truncate_audited_text` to `pub(crate) fn truncate_audited_text` and extend its doc with one line: retirement destroys the same attacker-influenced content and reuses this cap rather than re-deriving it (SMA-481 D9).

- [ ] **Step 2: Write the failing tests**

Create `system_retirement.rs` with a `ScriptedRetirer` fake (mirroring `bootstrap.rs`'s `ScriptedPolicies`: `Mutex`-held scripted returns plus recorded calls) and these tests. Reuse `crate::application::fakes::{FakeAuditLog, FakeOutbox, FakeBumper}` where they exist; add fakes locally only for what is missing.

```rust
    /// Root-only, and the check comes first: an unauthorized caller must not learn whether the
    /// id exists, and must not take a row lock.
    #[tokio::test]
    async fn an_unauthorized_actor_is_forbidden_and_touches_no_retirer_port() {
        let retirer = ScriptedRetirer::default();
        let svc = svc_denying_authz(retirer.clone());
        assert_eq!(svc.retire(&actor(), "legacy_auditor", false).await.unwrap_err(), TenancyError::Forbidden);
        assert_eq!(retirer.calls(), Vec::<String>::new(), "not even begin_retirement may run");
    }

    /// D7's load-bearing guard. Retiring a LIVE starter policy would be re-seeded next boot,
    /// but in the window between, that policy stops governing: forbid-archived-writes gone
    /// means archived resources become writable. Asserted for a role key AND the static id.
    #[tokio::test]
    async fn a_still_code_defined_id_is_refused_before_any_read() {
        for id in ["platform_admin", "forbid-archived-writes"] {
            let retirer = ScriptedRetirer::default();
            let svc = svc(retirer.clone());
            assert_eq!(svc.retire(&actor(), id, true).await.unwrap_err(), TenancyError::SystemImmutable(id.to_string()));
            assert!(retirer.calls().is_empty(), "{id} must be refused before a transaction opens");
        }
    }

    /// D11. Both a low revision and a NULL must refuse — a NULL proves nothing about which
    /// binary last wrote the row, and reading it as 0 would permit exactly the retirement this
    /// guard exists to defer.
    #[tokio::test]
    async fn an_unconverged_fleet_is_refused() {
        for min in [Some(STARTER_POLICY_REVISION - 1), None] {
            let retirer = ScriptedRetirer { min_revision: min, ..Default::default() }.shared();
            let svc = svc(retirer.clone());
            assert_eq!(svc.retire(&actor(), "legacy_auditor", true).await.unwrap_err(), TenancyError::FleetNotConverged);
            assert!(!retirer.calls().contains(&"begin_retirement".to_string()));
        }
    }

    #[tokio::test]
    async fn an_absent_row_is_not_found_and_a_non_system_row_is_refused() {
        let svc = svc(ScriptedRetirer { policy: None, ..converged() }.shared());
        assert_eq!(svc.retire(&actor(), "gone", true).await.unwrap_err(), TenancyError::NotFound);

        // A non-system POLICY row: DeletePolicy already serves those.
        let svc = svc(ScriptedRetirer { policy: Some(stored_policy(false, PolicyKind::Template)), ..converged() }.shared());
        assert_eq!(svc.retire(&actor(), "op_policy", true).await.unwrap_err(), TenancyError::NotSystemOwned("op_policy".to_string()));

        // A non-system ROLE row at a system policy's id — the half the first draft omitted.
        let svc = svc(ScriptedRetirer {
            policy: Some(stored_policy(true, PolicyKind::Template)),
            role: Some(StoredRole { key: "legacy_auditor".to_string(), system: false }),
            ..converged()
        }.shared());
        assert_eq!(svc.retire(&actor(), "legacy_auditor", true).await.unwrap_err(), TenancyError::NotSystemOwned("legacy_auditor".to_string()));
    }

    /// D4/D5: survivors block, and blocking writes NOTHING. The `total` is reported from the
    /// store, not from the returned page's length.
    #[tokio::test]
    async fn surviving_grants_block_the_retirement_and_write_nothing() {
        let retirer = ScriptedRetirer {
            survivors: SurvivingGrants { grants: vec![grant_ref("a"), grant_ref("b")], total: 7 },
            ..converged_with_role()
        }.shared();
        let (svc, outbox, audit, bumper) = svc_with_sinks(retirer.clone());

        let outcome = svc.retire(&actor(), "legacy_auditor", true).await.unwrap();
        match outcome {
            RetireOutcome::Blocked { role_key, grants, total, truncated } => {
                assert_eq!(role_key, "legacy_auditor");
                assert_eq!(grants.len(), 2);
                assert_eq!(total, 7, "the true total, not the page length");
                assert!(truncated || total <= GRANT_LIST_CAP);
            }
            other => panic!("expected Blocked, got {other:?}"),
        }
        assert!(!retirer.calls().iter().any(|c| c.starts_with("delete_")), "a blocked retirement deletes nothing");
        assert!(outbox.events().is_empty(), "and enqueues nothing");
        assert!(audit.entries().is_empty(), "and audits nothing");
        assert_eq!(bumper.count(), 0, "and bumps nothing");
    }

    /// D4's static half — the finding that invalidated the first draft's central claim.
    /// A static policy compiles unconditionally, so removing it changes decisions fleet-wide.
    #[tokio::test]
    async fn a_static_policy_needs_acknowledgement_and_the_refusal_previews_what_would_be_lost() {
        let retirer = ScriptedRetirer {
            policy: Some(StoredPolicy {
                policy_id: "legacy_forbid".to_string(),
                kind: PolicyKind::Static,
                source: "forbid(principal, action, resource);".to_string(),
                description: "a retired guard".to_string(),
                system: true,
            }),
            role: None,
            ..converged()
        }.shared();
        let (svc, outbox, audit, bumper) = svc_with_sinks(retirer.clone());

        match svc.retire(&actor(), "legacy_forbid", false).await.unwrap() {
            RetireOutcome::NeedsAcknowledgement { source, description, kind, .. } => {
                assert_eq!(kind, PolicyKind::Static);
                assert_eq!(source, "forbid(principal, action, resource);", "the refusal IS the preview");
                assert_eq!(description, "a retired guard");
            }
            other => panic!("expected NeedsAcknowledgement, got {other:?}"),
        }
        assert!(!retirer.calls().iter().any(|c| c.starts_with("delete_")));
        assert!(outbox.events().is_empty() && audit.entries().is_empty() && bumper.count() == 0);

        // With the flag it proceeds, and reports role_deleted: false (no role row exists).
        let outcome = svc.retire(&actor(), "legacy_forbid", true).await.unwrap();
        assert_eq!(outcome, RetireOutcome::Retired { policy_id: "legacy_forbid".to_string(), kind: PolicyKind::Static, role_deleted: false });
    }

    /// The flag is a no-op on a template: an operator must not have to know which kind they
    /// are dealing with before calling.
    #[tokio::test]
    async fn the_acknowledgement_flag_is_ignored_for_a_template() {
        let svc = svc(converged_with_role_and_no_grants().shared());
        assert!(svc.retire(&actor(), "legacy_auditor", false).await.unwrap().is_retired());
    }

    /// The happy path's full contract: role BEFORE policy, one event carrying the
    /// discriminator, one audit entry, ONE shared correlation_id, one awaited bump.
    #[tokio::test]
    async fn the_template_happy_path_deletes_role_then_policy_and_emits_one_of_each() {
        let retirer = converged_with_role_and_no_grants().shared();
        let (svc, outbox, audit, bumper) = svc_with_sinks(retirer.clone());

        let outcome = svc.retire(&actor(), "legacy_auditor", false).await.unwrap();
        assert_eq!(outcome, RetireOutcome::Retired { policy_id: "legacy_auditor".to_string(), kind: PolicyKind::Template, role_deleted: true });

        let calls = retirer.calls();
        let role_at = calls.iter().position(|c| c == "delete_role_in").expect("role must be deleted");
        let policy_at = calls.iter().position(|c| c == "delete_policy_in").expect("policy must be deleted");
        assert!(role_at < policy_at, "fk_role_template forces role before policy");

        let events = outbox.events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, EventType::PolicyDeleted);
        assert_eq!(events[0].payload["reason"], serde_json::json!("system_retirement"));
        assert_eq!(events[0].payload["role_deleted"], serde_json::json!(true));

        let entries = audit.entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].action, Action::RetireSystemPolicy.as_wire());
        assert_eq!(entries[0].resource_prn.as_deref(), Some(root_prn().canonical().as_str()));
        assert_eq!(entries[0].correlation_id, events[0].correlation_id, "one act, one correlation id");

        assert_eq!(bumper.count(), 1, "awaited exactly once, post-commit");
    }

    /// D9: retirement destroys the evidence, so the audit row carries it — capped by the same
    /// helper boot's convergence audit uses.
    #[tokio::test]
    async fn the_audit_entry_records_the_destroyed_content_and_caps_an_oversized_source() {
        let huge = "x".repeat(MAX_AUDITED_SOURCE_BYTES + 500);
        let retirer = ScriptedRetirer { policy: Some(stored_policy_with_source(&huge)), ..converged_with_role_and_no_grants() }.shared();
        let (svc, _outbox, audit, _bumper) = svc_with_sinks(retirer);
        svc.retire(&actor(), "legacy_auditor", true).await.unwrap();

        let entries = audit.entries();
        let destroyed = &entries[0].detail["destroyed_content"];
        assert_eq!(destroyed["kind"], serde_json::json!("template"));
        assert_eq!(destroyed["source"].as_str().unwrap().len(), MAX_AUDITED_SOURCE_BYTES);
        assert_eq!(destroyed["truncated"], serde_json::json!(true));
    }

    /// A failure between the deletes and the commit must leave nothing behind.
    #[tokio::test]
    async fn a_failure_before_commit_emits_nothing() {
        let retirer = ScriptedRetirer { fail_delete_policy: true, ..converged_with_role_and_no_grants() }.shared();
        let (svc, outbox, audit, bumper) = svc_with_sinks(retirer);
        svc.retire(&actor(), "legacy_auditor", true).await.expect_err("a store failure must propagate");
        assert!(outbox.events().is_empty() && audit.entries().is_empty() && bumper.count() == 0);
    }

    /// The rows were observed present under a held lock, so a `false` here is a data-integrity
    /// break — never a silent `role_deleted: false`, which would misreport what happened.
    #[tokio::test]
    async fn a_delete_that_affected_no_rows_under_a_held_lock_is_an_error() {
        let retirer = ScriptedRetirer { role_delete_returns_false: true, ..converged_with_role_and_no_grants() }.shared();
        let svc = svc(retirer);
        assert_eq!(svc.retire(&actor(), "legacy_auditor", true).await.unwrap_err(), TenancyError::Internal);
    }
```

- [ ] **Step 3: Run the tests to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cargo nextest run -p paigasus-iam -E 'binary(paigasus-iam) and test(system_retirement)' --no-tests=pass
```

Expected: FAIL — `cannot find struct SystemRetirementService`.

- [ ] **Step 4: Write the service**

Write `system_retirement.rs` following `application/dead_letters.rs`'s shape exactly: a `SystemRetirementDeps` bag, `Arc`-held ports, `#[derive(Clone)]` service, an `audit_entry` helper. The module doc must carry D3/D4/D6/D11/D12's reasoning inline.

```rust
/// At most this many surviving grants are listed in a refusal. Unbounded would load every row
/// into a Vec inside a transaction and serialise it whole — a denial of service against the
/// operator's own tooling. The true total is reported separately, so nothing is hidden.
pub const GRANT_LIST_CAP: u64 = 100;

/// Bounds the wait for a contended row. Mirrors `reconcile_system`'s own 5s: this is an
/// operator-triggered request and must fail with a message rather than hang.
pub const LOCK_TIMEOUT: Duration = Duration::from_secs(5);

pub async fn retire(&self, actor: &Prn, id: &str, ack: bool) -> Result<RetireOutcome, TenancyError> {
    self.authorize.check(actor, Action::RetireSystemPolicy, &root_prn()).await?;

    if authz_roles::is_starter_policy_id(id) {
        return Err(TenancyError::SystemImmutable(id.to_string()));
    }

    if self.retirer.min_starter_revision().await?.is_none_or(|r| r < STARTER_POLICY_REVISION) {
        counter!(names::IAM_SYSTEM_ROWS_RETIRED_TOTAL, "outcome" => "refused").increment(1);
        return Err(TenancyError::FleetNotConverged);
    }

    let tx = self.retirer.begin_retirement(LOCK_TIMEOUT).await?;

    let Some(policy) = self.retirer.lock_policy_in(&*tx, id).await? else {
        return Err(TenancyError::NotFound);
    };
    if !policy.system {
        return Err(TenancyError::NotSystemOwned(id.to_string()));
    }

    let role = self.retirer.lock_role_in(&*tx, id).await?;
    if role.as_ref().is_some_and(|r| !r.system) {
        return Err(TenancyError::NotSystemOwned(id.to_string()));
    }

    if role.is_some() {
        let survivors = self.retirer.surviving_grants_in(&*tx, id, GRANT_LIST_CAP).await?;
        if survivors.total > 0 {
            counter!(names::IAM_SYSTEM_ROWS_RETIRED_TOTAL, "outcome" => "blocked").increment(1);
            return Ok(RetireOutcome::Blocked {
                role_key: id.to_string(),
                truncated: survivors.truncated(GRANT_LIST_CAP),
                grants: survivors.grants,
                total: survivors.total,
            });
        }
    }

    if policy.kind == PolicyKind::Static && !ack {
        counter!(names::IAM_SYSTEM_ROWS_RETIRED_TOTAL, "outcome" => "refused").increment(1);
        return Ok(RetireOutcome::NeedsAcknowledgement {
            policy_id: id.to_string(),
            kind: policy.kind,
            source: policy.source,
            description: policy.description,
        });
    }

    let role_deleted = match &role {
        Some(_) => {
            require_deleted(self.retirer.delete_role_in(&*tx, id).await?, "role", id)?;
            true
        }
        None => false,
    };
    require_deleted(self.retirer.delete_policy_in(&*tx, id).await?, "policy", id)?;

    let corr = self.ids.new_correlation_id();
    let event = /* DomainEvent { event_type: EventType::PolicyDeleted, aggregate_prn: format!("policy/{id}"),
                   payload: json!({"policy_id": id, "reason": "system_retirement", "role_deleted": role_deleted}),
                   correlation_id: Some(corr), .. } */;
    let entry = self.audit_entry(actor, corr, id, &policy, role_deleted);

    self.outbox.enqueue(&*tx, &event).await?;
    self.audit.record(&*tx, &entry).await?;
    tx.commit().await?;

    self.gen_bumper.bump().await;
    counter!(names::IAM_SYSTEM_ROWS_RETIRED_TOTAL, "outcome" => "retired").increment(1);
    Ok(RetireOutcome::Retired { policy_id: id.to_string(), kind: policy.kind, role_deleted })
}
```

Add the invariant helper:

```rust
/// The row was observed present under a `FOR UPDATE` lock held by this very transaction, so a
/// delete affecting no rows is impossible without a data-integrity break. Surfaced as an error
/// rather than degraded to `role_deleted: false`, which would report a retirement that did not
/// happen — mirrors `pg_policies.rs`'s "unique-constraint violation but no row on re-read".
fn require_deleted(deleted: bool, what: &str, id: &str) -> Result<(), TenancyError> {
    if deleted {
        return Ok(());
    }
    tracing::error!(kind = what, policy_id = %id, "a locked {what} row vanished mid-retirement");
    Err(TenancyError::Internal)
}
```

The audit detail is `{"policy_id", "role_deleted", "source": "system_retirement", "destroyed_content": {"kind", "source", "description", "truncated", "description_truncated"}}`, capped via `crate::application::bootstrap::truncate_audited_text`.

Export from `application/mod.rs`: `pub mod system_retirement;`.

- [ ] **Step 5: Run the tests to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cargo nextest run -p paigasus-iam -E 'binary(paigasus-iam) and test(system_retirement)' --no-tests=pass
cargo clippy -p paigasus-iam -- -D warnings
```

Expected: PASS, no clippy warnings. (`IAM_SYSTEM_ROWS_RETIRED_TOTAL` is added in Task 7; add the const there first if the compile blocks here — move that one-line addition forward rather than stubbing it.)

- [ ] **Step 6: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/application/system_retirement.rs \
        rs/crates/services/paigasus-iam/src/application/mod.rs \
        rs/crates/services/paigasus-iam/src/application/bootstrap.rs
git commit -m "feat(rs): add the system row retirement use case (SMA-481)"
```

---

### Task 7: The HTTP endpoint, wiring, and the metric

**Files:**
- Create: `rs/crates/services/paigasus-iam/src/adapters/http/system_retirement.rs`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/http/mod.rs`, `rs/crates/services/paigasus-iam/src/main.rs`, `rs/crates/libs/paigasus-observability/src/names.rs`

**Interfaces:**
- Consumes: `SystemRetirementService::retire` (Task 6), `RetireOutcome` (Task 4).
- Produces: `POST /v1/authz/system-policies/{id}/retire`; `AppState.retirement: SystemRetirementSvc`; `names::IAM_SYSTEM_ROWS_RETIRED_TOTAL`.

- [ ] **Step 1: Add the metric name**

In `rs/crates/libs/paigasus-observability/src/names.rs`, next to the outbox dead-letter families:

```rust
/// Retirements of orphaned system-owned rows (SMA-481); label
/// `outcome=retired|blocked|refused`. The audit row is the durable record, but nothing alerts
/// on `audit_log` — this counter is the remediation path, exactly as SMA-477's own reconcile
/// labels are.
pub const IAM_SYSTEM_ROWS_RETIRED_TOTAL: &str = "iam_system_rows_retired_total";
```

In `main.rs`'s `describe_iam_metrics`, add:

```rust
    describe_counter!(
        names::IAM_SYSTEM_ROWS_RETIRED_TOTAL,
        "Retirements of orphaned system-owned policy/role rows, by outcome (retired/blocked/refused)."
    );
```

and update that function's doc comment's family count (23 → 24).

- [ ] **Step 2: Write the failing test**

Add to `rs/crates/services/paigasus-iam/src/adapters/http/mod.rs`'s test module — extend the existing merge guard:

```rust
    fn protected_router_merge_has_no_path_conflicts() {
        let _: Router<AppState> = Router::new()
            .merge(organizations::router())
            // … existing merges …
            .merge(dead_letters::router())
            .merge(system_retirement::router());
    }
```

- [ ] **Step 3: Run it to verify it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cargo nextest run -p paigasus-iam -E 'test(protected_router_merge)' --no-tests=pass
```

Expected: FAIL — `cannot find module system_retirement`.

- [ ] **Step 4: Write the handler**

Create `adapters/http/system_retirement.rs`, mirroring `http/dead_letters.rs`:

```rust
pub fn router() -> Router<AppState> {
    Router::new().route("/v1/authz/system-policies/{id}/retire", post(retire))
}

/// An absent or empty body means "not acknowledged" — the flag must be typed deliberately, so
/// the safe reading is the default (D4).
#[derive(serde::Deserialize, Default)]
#[serde(default)]
struct RetireBody {
    acknowledge_decision_change: bool,
}

/// `POST /v1/authz/system-policies/{id}/retire`: Root-only (enforced inside
/// `SystemRetirementService::retire`). Returns 200 with what was destroyed, never 204 — a body
/// is the operator's only immediate record of an irreversible act, and the two refusals below
/// carry the information needed to act on them.
async fn retire(
    State(s): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<String>,
    body: Option<Json<RetireBody>>,
) -> Result<Response, ApiError> {
    let ack = body.map(|Json(b)| b.acknowledge_decision_change).unwrap_or(false);
    match s.retirement.retire(&actor_prn(&ctx), &id, ack).await? {
        RetireOutcome::Retired { policy_id, kind, role_deleted } => Ok((
            StatusCode::OK,
            Json(json!({ "policy_id": policy_id, "kind": policy_kind_str(kind), "role_deleted": role_deleted })),
        ).into_response()),
        RetireOutcome::Blocked { role_key, grants, total, truncated } => Ok(conflict(
            "grants-survive",
            &format!(
                "{total} grant(s) of '{role_key}' must be revoked before it can be retired. \
                 If a revoke returns 403 because its scope node is archived, restore the node, \
                 revoke, then re-archive it."
            ),
            json!({ "grants": grants_json(&grants), "total_surviving": total, "truncated": truncated }),
        )),
        RetireOutcome::NeedsAcknowledgement { policy_id, kind, source, description } => Ok(conflict(
            "decision-change-unacknowledged",
            &format!(
                "'{policy_id}' is a static policy: it is evaluated on every request, so retiring \
                 it changes decisions fleet-wide. Re-send with acknowledge_decision_change=true."
            ),
            json!({ "kind": policy_kind_str(kind), "source": source, "description": description }),
        )),
    }
}

/// Builds a 409 that keeps the stable `{"error": {"code", "message"}}` envelope every other
/// error in this service uses, and hangs the retirement-specific data off it as sibling fields.
/// The handler builds these itself rather than going through `ApiError` because a refusal is an
/// `Ok` outcome carrying information, not a `TenancyError` (D5).
fn conflict(code: &str, message: &str, mut extra: serde_json::Value) -> Response {
    let obj = extra.as_object_mut().expect("extra is always a json object");
    obj.insert("error".to_string(), json!({ "code": code, "message": message }));
    (StatusCode::CONFLICT, Json(extra)).into_response()
}
```

Add `mod system_retirement;` and the `.merge(system_retirement::router())` call to the **real** protected router in `mod.rs` (not only the test), and add the `AppState` field:

```rust
    /// Retirement of orphaned system-owned rows (SMA-481) — the `/v1/authz/system-policies/
    /// {id}/retire` route calls through this. Deliberately its own service rather than a
    /// `PolicySvc` method: it drives the privileged `SystemRowRetirer` port, which bypasses
    /// `PolicyStore::delete_in`'s `SystemImmutable` guard and must stay unreachable from
    /// ordinary policy CRUD.
    pub retirement: SystemRetirementSvc,
```

Build it in `AppState::new` from `PgSystemRowRetirer::new(db.clone())` plus the same `Arc`-shared `uow`/`audit`/`outbox`/`gen_bumper`/`authorize` the other services take.

- [ ] **Step 5: Run the tests to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cargo nextest run -p paigasus-iam --no-tests=pass && cargo clippy --workspace -- -D warnings && cargo fmt --check
```

Expected: PASS, clean clippy, clean fmt.

- [ ] **Step 6: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/adapters/http/ \
        rs/crates/services/paigasus-iam/src/main.rs \
        rs/crates/libs/paigasus-observability/src/names.rs
git commit -m "feat(rs): expose the system-policy retirement endpoint (SMA-481)"
```

---

### Task 8: Point the orphan WARN at the remedy, and write the runbook

A log line that reports a problem should state its remedy. The runbook must lead with D11's precondition and D12's archived-scope escape, because those are the two ways an operator gets stuck.

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/application/bootstrap.rs:220-226` and `:347-349`
- Modify: `docs/ops/RUNBOOK-observability.md`

**Interfaces:**
- Consumes: the endpoint path from Task 7.

- [ ] **Step 1: Update the two WARN messages**

Policy half:

```rust
        tracing::warn!(
            policy_id = %orphan,
            "a system-owned policy row is no longer code-defined; it still compiles and still links grants, and DeletePolicy refuses to remove it. Retire it with POST /v1/authz/system-policies/{policy_id}/retire once every replica is on a binary that no longer defines it (see RUNBOOK-observability)"
        );
```

Role half:

```rust
        tracing::warn!(
            role_key = %orphan,
            "a system role row is no longer code-defined; existing grants of it still resolve. Revoke those grants, then retire it with POST /v1/authz/system-policies/{role_key}/retire (see RUNBOOK-observability)"
        );
```

- [ ] **Step 2: Add the runbook section**

Append a `### Retiring an orphaned system-owned row (SMA-481)` section to `docs/ops/RUNBOOK-observability.md` with, in this order:

1. **Precondition, first.** Every replica must be on a binary that no longer defines the id. `classify_starter_policy` classifies an absent row as `Absent` *before* the revision guard, so a replica whose catalog still defines the id re-seeds it. Retiring mid-rollout is silently undone.
2. Read the orphan `WARN`; call `POST /v1/authz/system-policies/{id}/retire` as a `platform_admin`.
3. On `409 grants-survive`: revoke each listed grant via `DELETE /v1/authz/role-grants/{id}`. **If a revoke returns `403` because its scope node is archived, restore the node → revoke → re-archive** — `RevokeRole` is a write action and `forbid-archived-writes` blocks it even for `platform_admin`.
4. On `409 decision-change-unacknowledged`: the id is a *static* policy, evaluated on every request. Read the returned `source`, decide, then re-send with `{"acknowledge_decision_change": true}`.
5. On `409 fleet-not-converged`: a system-owned row was last written by an older binary. Wait for the rollout to finish and retry.
6. Confirm the `WARN` is gone on the next boot. **If it returns, the fleet had not converged — repeat.** Nothing is corrupted; the rows were re-seeded.
7. Watch `iam_system_rows_retired_total{outcome="retired"}`.
8. **The one case this does not cover:** a hand-inserted `role` row whose `template_id ≠ key`. The endpoint keys off a single id, so such a row is unreachable through it and needs direct database work.

- [ ] **Step 3: Verify the crate still builds and the docs gate passes**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cargo nextest run -p paigasus-iam --no-tests=pass
```

Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/application/bootstrap.rs docs/ops/RUNBOOK-observability.md
git commit -m "docs(docs): document the system row retirement procedure (SMA-481)"
```

---

### Task 9: Integration tests — the decision change, the locks, and the fleet-skew failure mode

The unit tests prove the guards. These prove the thing the issue is actually about: a retired role stops granting.

**Files:**
- Modify: `rs/crates/services/paigasus-iam/tests/authz_system_retirement_pg.rs`

**Interfaces:**
- Consumes: everything from Tasks 1–7.

- [ ] **Step 1: Write the failing tests**

```rust
/// THE test. Everything else checks rows; this checks decisions. A grant of a retired role must
/// stop conferring permission — which is the entire point of SMA-481.
#[tokio::test]
async fn a_retired_role_s_grant_stops_conferring_permission() {
    let (_c, db) = setup().await;
    seed_orphan_chain(&db, "legacy_auditor").await;
    let grant = seed_grant(&db, "legacy_auditor", GrantScope::Root).await;

    let before = PolicyEngine::compile(&load_all_policies(&db).await, &[grant.clone()]).unwrap();
    assert!(decide(&before, &grant, Action::ListAuditLog).is_allow(), "the fixture must actually grant, or the after-assertion proves nothing");

    retire(&db, "legacy_auditor", true).await.expect("no grants survive after the revoke below");

    let after = PolicyEngine::compile(&load_all_policies(&db).await, &[grant.clone()]).unwrap();
    assert!(!decide(&after, &grant, Action::ListAuditLog).is_allow(), "the template is gone, so the grant links nothing");
}

/// D6, both halves. The lock blocks the concurrent insert AND the caller that loses the race
/// gets a mapped error, not a 500. Asserting only the blocking — as an earlier draft did —
/// would go green against the unmapped bug.
#[tokio::test]
async fn a_concurrent_grant_blocks_then_reports_unknown_role() { /* two connections */ }

/// The policy row is the FK parent of the role row, so it is locked first — otherwise an older
/// replica's reconcile_role INSERT slips in when no role row exists to lock, and the policy
/// delete fails on fk_role_template with an unmapped error.
#[tokio::test]
async fn locking_the_policy_row_blocks_a_concurrent_role_insert() { /* two connections */ }

/// lock_timeout bounds the wait rather than hanging: this runs on an operator's request.
#[tokio::test]
async fn a_contended_row_times_out_rather_than_hanging() { /* hold a lock, assert error within ~5s */ }

/// D11's known failure mode, pinned deliberately. A replica whose catalog still defines the id
/// re-seeds the deleted rows — the retirement is undone and the orphan WARN returns. Pinning
/// this is what stops it being discovered in production.
#[tokio::test]
async fn a_binary_that_still_defines_the_id_re_seeds_it_after_retirement() {
    let (_c, db) = setup().await;
    seed_orphan_chain(&db, "legacy_auditor").await;
    retire(&db, "legacy_auditor", true).await.unwrap();
    assert!(policy_row(&db, "legacy_auditor").await.is_none());

    // Simulate the older replica: reconcile the id as though the catalog still defined it.
    let reconciler = PgPolicyStore::new(db.clone(), test_generations());
    reconciler.reconcile_system(&orphan_doc("legacy_auditor"), STARTER_POLICY_REVISION).await.unwrap();

    assert!(
        policy_row(&db, "legacy_auditor").await.is_some(),
        "Absent is classified BEFORE the revision guard, so an older binary re-seeds unconditionally — the documented D11 failure mode"
    );
}

/// Retirement is not an idempotent DELETE: a second call means the operator's model of the
/// system is wrong and they should be told, not silently congratulated.
#[tokio::test]
async fn a_repeated_retirement_is_not_found_and_writes_nothing() { /* … */ }

/// End to end, the way the runbook reads.
#[tokio::test]
async fn grant_then_retire_then_revoke_then_retire() { /* 409 → revoke → 200 → rows gone */ }
```

- [ ] **Step 2: Run them to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cargo nextest run -p paigasus-iam -E 'binary(authz_system_retirement_pg)' --no-tests=pass
```

Expected: FAIL — missing helpers.

- [ ] **Step 3: Implement the helpers and make them pass**

Write `seed_grant`, `load_all_policies`, `decide`, `retire`, `policy_row`, `orphan_doc`. `decide` builds a `Request` + `EntitySlice` the way `tests/authz_*.rs` already do — copy that construction rather than inventing one.

- [ ] **Step 4: Run them to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cargo nextest run -p paigasus-iam -E 'binary(authz_system_retirement_pg)' --no-tests=pass
```

Expected: PASS.

- [ ] **Step 5: Run the full repo gate exactly as CI does**

Per CLAUDE.md, per-project tasks do not run the repo-level gates.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon ci :build :test :lint :fmt :deny :machete :typecheck :breaking :affected-smoke \
  :parity-corpus-drift :wasm-getrandom-free :promtool :observability-drift \
  :release-parity :release-parity-py :release-parity-ts --base origin/main --include-relations
```

Expected: all green. If `:observability-drift` fails, the new metric name needs its `ops/observability/` counterpart — add it. If Moon reports an unattributed failure, diagnose via `jq '.actions[]|select(.status=="failed")' .moon/cache/ciReport.json`.

- [ ] **Step 6: Commit**

```bash
git add rs/crates/services/paigasus-iam/tests/authz_system_retirement_pg.rs
git commit -m "test(rs): prove a retired role stops granting and pin the fleet-skew case (SMA-481)"
```

---

## Self-Review

**Spec coverage.** D1→Task 7 (runtime endpoint). D2→Task 7 (one endpoint, one id). D3→Tasks 4–5 (own port, guards untouched; Task 9 step 5 re-runs the existing `SystemImmutable` tests). D4→Task 6 (blocked + acknowledgement). D5→Tasks 4, 6, 7 (`#[must_use]` outcome, cap, registered codes). D6→Tasks 3, 5, 6, 9 (lock + FK remap + both tests). D7→Tasks 2, 6 (both refusals, both rows, Precondition class). D8→Tasks 1, 6 (action, Root-only). D9→Task 6 (audit + event + shared correlation id). D10→Tasks 6, 7 (bump + metric). D11→Tasks 2, 5, 6, 8, 9 (refusal, `min_starter_revision`, runbook precondition, pinned failure mode). D12→Tasks 7, 8 (message + runbook). §3.1→Task 1. §3.2→Task 4. §3.3→Task 6. §3.4→Task 7. §3.5→Tasks 2, 3. §3.6→Task 7. §3.7→Task 8. §4.1→Task 6. §4.2→Task 1. §4.3→Tasks 5, 9. §4.4→Task 9 step 5. §5→Task 8. All ACs 1–13 are covered; AC8 is Task 9 step 1's first test and AC11 its fifth.

**Placeholder scan.** The `/* … */` markers in Tasks 5, 6 and 9 mark bodies whose exact shape must be copied from a named existing file (`authz_bootstrap.rs`'s harness, `dead_letters.rs`'s deps bag, `pg_role_grants.rs::model_to_grant`) rather than invented — each says which file. Every signature, const, error variant, JSON key and test assertion is written out.

**Type consistency.** `SurvivingGrants { grants, total }` with `truncated(cap)` as a method is used identically in Tasks 4, 5 and 6. `RetireOutcome::Blocked` carries `total`, not `total_surviving`; only the HTTP body (Task 7) renames it to `total_surviving`, which is deliberate and stated. `retire(&self, actor, id, ack)` matches between Tasks 6 and 7. `GRANT_LIST_CAP: u64` matches `surviving_grants_in`'s `cap: u64`. `StoredPolicy.description` is `String` (not `Option<String>`), so the adapter maps the nullable column with `unwrap_or_default`, as `model_to_doc` already does.
