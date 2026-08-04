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
    NeedsAcknowledgement {
        policy_id: String,
        kind: PolicyKind,
        source: String,
        description: String,
    },
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
        assert!(under.truncated(2), "3 total under a cap of 2 means more exist");
        // `truncated` compares the true TOTAL against the cap, so build the returned list too —
        // it must not matter that this one is populated when the empty one above already is.
        let returned: Vec<GrantRef> = (0..3)
            .map(|i| GrantRef {
                id: format!("00000000-0000-0000-0000-00000000000{i}"),
                principal_prn: "prn:pgs:iam:::principal/p".to_string(),
                scope_prn: "prn:pgs:iam:::root/root".to_string(),
            })
            .collect();
        let exact = SurvivingGrants { grants: returned.clone(), total: 3 };
        assert!(!exact.truncated(3), "3 returned under a cap of 3 is complete");
        assert!(exact.truncated(2), "3 returned under a cap of 2 means more exist");
    }
}
