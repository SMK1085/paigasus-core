// SPDX-License-Identifier: Apache-2.0
//! The session of one request and its store time budget (SMA-753, spec D4 and D10).

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use paigasus_kernel::tool_policy::{Findings, SessionInfo};
use uuid::Uuid;

/// The store time budget of one request (spec D10 "Deadline"). It reads no clock: callers pass
/// the time they spent.
#[derive(Debug)]
pub struct StoreBudget {
    limit: Duration,
    spent_nanos: AtomicU64,
    failed: AtomicBool,
}

impl StoreBudget {
    /// A budget of `limit`, with nothing spent.
    #[must_use]
    pub fn new(limit: Duration) -> Self {
        Self {
            limit,
            spent_nanos: AtomicU64::new(0),
            failed: AtomicBool::new(false),
        }
    }

    /// The time left, or `None` when the budget is spent or a store call failed.
    #[must_use]
    pub fn remaining(&self) -> Option<Duration> {
        if self.is_failed() {
            return None;
        }
        let spent = Duration::from_nanos(self.spent_nanos.load(Ordering::Acquire));
        self.limit.checked_sub(spent).filter(|d| !d.is_zero())
    }

    /// Add `spent` to the time used. The sum saturates.
    pub fn charge(&self, spent: Duration) {
        let n = u64::try_from(spent.as_nanos()).unwrap_or(u64::MAX);
        let _ = self.spent_nanos.try_update(Ordering::AcqRel, Ordering::Acquire, |v| Some(v.saturating_add(n)));
    }

    /// Record that a store call failed. No time is left after this.
    pub fn fail(&self) {
        self.failed.store(true, Ordering::Release);
    }

    /// Whether a store call failed.
    #[must_use]
    pub fn is_failed(&self) -> bool {
        self.failed.load(Ordering::Acquire)
    }
}

/// The session of one request (spec D4).
#[non_exhaustive]
#[derive(Debug)]
pub struct SessionContext {
    pub org: Uuid,
    pub session_id: Uuid,
    /// `SessionInfo::new(session_id.to_string(), turn)`.
    pub info: SessionInfo,
    /// Request-level findings: `mixed_sessions`, `forged_history`.
    pub findings: Findings,
    pub budget: StoreBudget,
}

impl SessionContext {
    #[must_use]
    pub fn new(org: Uuid, session_id: Uuid, turn: i64, findings: Findings, budget: StoreBudget) -> Self {
        Self {
            org,
            session_id,
            info: SessionInfo::new(session_id.to_string(), turn),
            findings,
            budget,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn the_budget_shrinks_with_each_charge_and_ends_at_zero() {
        let b = StoreBudget::new(Duration::from_millis(100));
        assert_eq!(b.remaining(), Some(Duration::from_millis(100)));
        b.charge(Duration::from_millis(30));
        assert_eq!(b.remaining(), Some(Duration::from_millis(70)));
        b.charge(Duration::from_millis(60));
        assert_eq!(b.remaining(), Some(Duration::from_millis(10)));
        b.charge(Duration::from_millis(10));
        assert_eq!(b.remaining(), None);
        b.charge(Duration::from_millis(500));
        assert_eq!(b.remaining(), None);
        assert!(!b.is_failed());
    }

    #[test]
    fn a_failed_budget_has_no_time_left() {
        let b = StoreBudget::new(Duration::from_secs(1));
        assert!(!b.is_failed());
        b.fail();
        assert!(b.is_failed());
        assert_eq!(b.remaining(), None);
    }
}
