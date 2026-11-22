// SPDX-License-Identifier: Apache-2.0
//! The policy snapshot of one request (SMA-752 D30): the compiled policies, the tenant mode and
//! the effective `on_policy_unavailable`. The gateway builds it once per request; the decider
//! reads it from `DecisionContext` and never loads policies itself.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use paigasus_kernel::tool_policy::{Facts, ToolPolicies};

/// The tenant mode (§ 6.4). It applies only to a deny that tenant policies decided: a floor deny
/// is always enforced (D33).
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TenantMode {
    /// The tenant set is not evaluated. Only the floor applies.
    Off,
    /// A tenant deny is reported as `Observe`, and the call goes through.
    LogOnly,
    /// A tenant deny blocks the call.
    Enforce,
}

impl TenantMode {
    /// The config and metric label spelling.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            TenantMode::Off => "off",
            TenantMode::LogOnly => "log_only",
            TenantMode::Enforce => "enforce",
        }
    }
}

/// The effective `on_policy_unavailable` of the tenant (D34: the per-org override, else the
/// global value).
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnUnavailable {
    /// Allow as `Observe { would: PolicyUnavailable }` (P-5).
    Open,
    /// Block with `PolicyUnavailable`.
    Closed,
}

/// What the tenant part of an available set is.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TenantPart {
    /// The policies hold the floor and the tenant set.
    Evaluated,
    /// The tenant mode is `off`: the policies are the floor alone (P-4).
    Off,
    /// The tenant set is unavailable: the policies are the floor alone (P-4), and the tenant part
    /// follows `on_policy_unavailable` (D34).
    Unavailable,
}

/// The policies of a request.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub enum PolicySet {
    /// The floor is available. With a `tenant` other than `Evaluated`, `policies` must be the
    /// floor alone, `ToolPolicies::compile(floor, "", &[])` (P-4).
    Available { policies: Arc<ToolPolicies>, tenant: TenantPart, facts: Facts },
    /// The floor is unavailable, or a PRN of the request does not parse (P-6).
    Unavailable,
}

/// One request's policy snapshot (D30).
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct PolicySnapshot {
    pub set: PolicySet,
    pub mode: TenantMode,
    pub on_unavailable: OnUnavailable,
    /// The time the request side took to build this snapshot and apply the plan (preflight F14).
    pub request_elapsed: Duration,
    /// The decider time of this request, summed over every decide call. The gateway records one
    /// overhead sample per request from [`PolicySnapshot::overhead`] (preflight F14).
    decide_elapsed: Nanos,
    /// Request-side time that comes after the snapshot is built, for example the history write
    /// after the limits admission (SMA-753). It is not in `request_elapsed`, so nothing counts twice.
    later_request_elapsed: Nanos,
}

/// A sum of time in nanoseconds. A clone copies the current value.
#[derive(Debug, Default)]
struct Nanos(AtomicU64);

impl Nanos {
    fn add(&self, elapsed: Duration) {
        let nanos = u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX);
        self.0.fetch_add(nanos, Ordering::Relaxed);
    }

    fn get(&self) -> Duration {
        Duration::from_nanos(self.0.load(Ordering::Relaxed))
    }
}

impl Clone for Nanos {
    fn clone(&self) -> Self {
        Nanos(AtomicU64::new(self.0.load(Ordering::Relaxed)))
    }
}

impl PolicySnapshot {
    #[must_use]
    pub fn new(set: PolicySet, mode: TenantMode, on_unavailable: OnUnavailable) -> Self {
        PolicySnapshot {
            set,
            mode,
            on_unavailable,
            request_elapsed: Duration::ZERO,
            decide_elapsed: Nanos::default(),
            later_request_elapsed: Nanos::default(),
        }
    }

    /// Add the time of one decide call (preflight F14). The decider calls it once per hold.
    pub fn add_decide_elapsed(&self, elapsed: Duration) {
        self.decide_elapsed.add(elapsed);
    }

    /// The decider time summed so far.
    #[must_use]
    pub fn decide_elapsed(&self) -> Duration {
        self.decide_elapsed.get()
    }

    /// Add request-side time that comes after the snapshot is built (SMA-753: the history write
    /// after the limits admission). Do not add time that `request_elapsed` already holds.
    pub fn add_request_elapsed(&self, elapsed: Duration) {
        self.later_request_elapsed.add(elapsed);
    }

    /// The request-side time added by [`PolicySnapshot::add_request_elapsed`] so far.
    #[must_use]
    pub fn later_request_elapsed(&self) -> Duration {
        self.later_request_elapsed.get()
    }

    /// The overhead of the request so far: the request side (`request_elapsed` plus the later
    /// request-side time) plus every decide call (preflight F14).
    #[must_use]
    pub fn overhead(&self) -> Duration {
        self.request_elapsed + self.later_request_elapsed() + self.decide_elapsed()
    }

    /// Set the request-side elapsed time (preflight F14).
    #[must_use]
    pub fn with_request_elapsed(mut self, elapsed: Duration) -> Self {
        self.request_elapsed = elapsed;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tenant_mode_spellings() {
        assert_eq!(TenantMode::Off.as_str(), "off");
        assert_eq!(TenantMode::LogOnly.as_str(), "log_only");
        assert_eq!(TenantMode::Enforce.as_str(), "enforce");
    }

    #[test]
    fn a_new_snapshot_has_no_request_time_and_the_builder_sets_it() {
        let s = PolicySnapshot::new(PolicySet::Unavailable, TenantMode::Off, OnUnavailable::Open);
        assert_eq!(s.request_elapsed, Duration::ZERO);
        let s = s.with_request_elapsed(Duration::from_millis(7));
        assert_eq!(s.request_elapsed, Duration::from_millis(7));
    }

    #[test]
    fn the_overhead_sums_the_request_side_and_every_decide_call() {
        let s = PolicySnapshot::new(PolicySet::Unavailable, TenantMode::Off, OnUnavailable::Open).with_request_elapsed(Duration::from_millis(7));
        s.add_decide_elapsed(Duration::from_millis(2));
        s.add_decide_elapsed(Duration::from_millis(3));
        assert_eq!(s.decide_elapsed(), Duration::from_millis(5));
        assert_eq!(s.overhead(), Duration::from_millis(12));
        assert_eq!(s.clone().overhead(), Duration::from_millis(12), "a clone keeps the sum");
    }

    #[test]
    fn later_request_side_time_adds_to_the_overhead_once() {
        let s = PolicySnapshot::new(PolicySet::Unavailable, TenantMode::Off, OnUnavailable::Open).with_request_elapsed(Duration::from_millis(7));
        s.add_decide_elapsed(Duration::from_millis(2));
        s.add_request_elapsed(Duration::from_millis(4));
        assert_eq!(
            (s.request_elapsed, s.later_request_elapsed(), s.decide_elapsed()),
            (Duration::from_millis(7), Duration::from_millis(4), Duration::from_millis(2))
        );
        assert_eq!(s.overhead(), Duration::from_millis(13));
        assert_eq!(s.clone().overhead(), Duration::from_millis(13), "a clone keeps the sum");
    }
}
