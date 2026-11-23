// SPDX-License-Identifier: Apache-2.0

//! The lease handle and the errors of the lease and claim ports.

use crate::ids::{InboxId, RunId};
use crate::run::LeaseToken;

/// The heartbeat of a lease failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HeartbeatError {
    /// The lease is lost. Another holder owns it, or the handle is poisoned.
    #[error("lease lost")]
    LeaseLost,
    /// The store failed. The text is the cause.
    #[error("store error: {0}")]
    Store(String),
}

/// A claim failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ClaimError {
    /// The lease is lost.
    #[error("lease lost")]
    LeaseLost,
    /// The handle has an open claim. Commit it before the next claim.
    #[error("a claim is open")]
    ClaimOpen,
    /// The handle is poisoned. A failed claim or commit left its state unknown.
    #[error("the lease handle is poisoned")]
    Poisoned,
    /// The store failed. The text is the cause.
    #[error("store error: {0}")]
    Store(String),
}

/// A held lease on one inbox. It is not `Clone`: one holder owns one handle.
///
/// The handle carries the current fencing token. A claim rotates the token, and the
/// adapter records the new token with `record_claim`.
#[derive(Debug)]
pub struct LeaseHandle {
    inbox: InboxId,
    token: LeaseToken,
    holder: String,
    open_claim: Option<RunId>,
    poisoned: bool,
    claimed_before: bool,
}

impl LeaseHandle {
    /// Make a handle for a lease that the adapter just acquired.
    #[must_use]
    pub fn new(inbox: InboxId, token: LeaseToken, holder: String) -> Self {
        Self {
            inbox,
            token,
            holder,
            open_claim: None,
            poisoned: false,
            claimed_before: false,
        }
    }

    /// The inbox that the lease covers.
    #[must_use]
    pub fn inbox(&self) -> &InboxId {
        &self.inbox
    }

    /// The current fencing token.
    #[must_use]
    pub fn token(&self) -> LeaseToken {
        self.token
    }

    /// The holder name.
    #[must_use]
    pub fn holder(&self) -> &str {
        &self.holder
    }

    /// The run of the open claim, if one is open.
    #[must_use]
    pub fn open_claim(&self) -> Option<RunId> {
        self.open_claim.clone()
    }

    /// True when the handle is poisoned.
    #[must_use]
    pub fn is_poisoned(&self) -> bool {
        self.poisoned
    }

    /// True after the first claim. A later claim must rotate the token.
    #[must_use]
    pub fn needs_rotation(&self) -> bool {
        self.claimed_before
    }

    /// Check that the handle can start a claim.
    pub fn check_claim(&self) -> Result<(), ClaimError> {
        if self.poisoned {
            return Err(ClaimError::Poisoned);
        }
        if self.open_claim.is_some() {
            return Err(ClaimError::ClaimOpen);
        }
        Ok(())
    }

    /// Record a claim. It sets the new token and the open claim (`None` for an empty claim).
    pub fn record_claim(&mut self, token: LeaseToken, run: Option<RunId>) {
        self.token = token;
        self.claimed_before = true;
        self.open_claim = run;
    }

    /// Record that a commit finished the open claim.
    pub fn record_complete(&mut self) {
        self.open_claim = None;
    }

    /// Poison the handle. Every later claim, heartbeat and commit fails.
    pub fn poison(&mut self) {
        self.poisoned = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn u(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    fn h() -> LeaseHandle {
        LeaseHandle::new(InboxId::new(u(1), u(2)), LeaseToken(1), "w".into())
    }

    #[test]
    fn a_new_handle_allows_a_claim_and_needs_no_rotation() {
        let h = h();
        assert_eq!(h.check_claim(), Ok(()));
        assert!(!h.needs_rotation());
    }

    #[test]
    fn after_a_claim_the_handle_needs_rotation() {
        let mut h = h();
        h.record_claim(LeaseToken(1), None);
        assert!(h.needs_rotation());
    }

    #[test]
    fn an_open_claim_refuses_a_second_claim() {
        let mut h = h();
        h.record_claim(LeaseToken(2), Some(RunId::new(u(1), u(3))));
        assert_eq!(h.check_claim(), Err(ClaimError::ClaimOpen));
    }

    #[test]
    fn record_complete_clears_the_open_claim() {
        let mut h = h();
        h.record_claim(LeaseToken(2), Some(RunId::new(u(1), u(3))));
        h.record_complete();
        assert_eq!(h.check_claim(), Ok(()));
        assert_eq!(h.token(), LeaseToken(2));
    }

    #[test]
    fn a_poisoned_handle_refuses_every_claim() {
        let mut h = h();
        h.poison();
        assert!(h.is_poisoned());
        assert_eq!(h.check_claim(), Err(ClaimError::Poisoned));
    }

    #[test]
    fn lease_handle_is_not_clone() {
        trait AmbiguousIfClone<A> {
            fn some() {}
        }
        impl<T: ?Sized> AmbiguousIfClone<()> for T {}
        impl<T: Clone> AmbiguousIfClone<u8> for T {}
        // Compiles only while `LeaseHandle` is not `Clone`.
        let _ = <LeaseHandle as AmbiguousIfClone<_>>::some;
    }
}
