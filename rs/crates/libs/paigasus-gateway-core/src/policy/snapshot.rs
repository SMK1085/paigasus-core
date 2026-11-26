// SPDX-License-Identifier: Apache-2.0
//! The policy snapshot of one request (SMA-752 D30): the compiled policies, the tenant mode and
//! the effective `on_policy_unavailable`. The gateway builds it once per request; the decider
//! reads it from `DecisionContext` and never loads policies itself.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use paigasus_kernel::detect::HostSet;
use paigasus_kernel::tool_policy::{Facts, ToolPolicies};

use super::history::HistoryPolicy;

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
    /// floor alone, `ToolPolicies::compile(floor, "", &[])` (P-4). `floor_only` is always that
    /// floor-only set (the `Arc` of the cache's `FloorEntry::floor_only`, SMA-753 spec P6), so a
    /// hold whose tenant part is unavailable is decided without a new compile.
    Available {
        policies: Arc<ToolPolicies>,
        floor_only: Arc<ToolPolicies>,
        tenant: TenantPart,
        facts: Facts,
    },
    /// The floor is unavailable, or a PRN of the request does not parse (P-6).
    Unavailable,
}

/// The hosts that the request side saw (§ 6.8.1, D14). The response side reads them for
/// `unseen_host`. `Debug` prints the count only (§ 6.2.4).
#[derive(Clone)]
pub struct SeenHosts(pub Arc<HostSet>);

impl fmt::Debug for SeenHosts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SeenHosts {{ len: {} }}", self.0.len())
    }
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
    /// The seen hosts of the request screen. `None` when no screen ran (§ 6.8.0), so
    /// `unseen_host` does not run.
    pub seen_hosts: Option<SeenHosts>,
    /// The history conditions of the org, bound and planned at load (SMA-753 spec P6). `None` when
    /// no history applies to the request.
    pub history: Option<Arc<HistoryPolicy>>,
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
            seen_hosts: None,
            history: None,
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

    /// Set the seen hosts of the request screen (§ 6.8.1).
    #[must_use]
    pub fn with_seen_hosts(mut self, seen: SeenHosts) -> Self {
        self.seen_hosts = Some(seen);
        self
    }

    /// Set the history policy of the org (SMA-753 spec P6).
    #[must_use]
    pub fn with_history(mut self, history: Arc<HistoryPolicy>) -> Self {
        self.history = Some(history);
        self
    }

    /// A copy of the snapshot whose tenant part is unavailable (SMA-753 spec P7): an available set
    /// gets `policies = floor_only` and `TenantPart::Unavailable`; an unavailable set stays.
    ///
    /// The copy holds a copy of the time sums. Add decide time to the original snapshot, not to
    /// the copy, or the overhead sample of the request loses it.
    #[must_use]
    pub fn with_tenant_unavailable(&self) -> PolicySnapshot {
        let mut copy = self.clone();
        if let PolicySet::Available { policies, floor_only, tenant, .. } = &mut copy.set {
            *policies = Arc::clone(floor_only);
            *tenant = TenantPart::Unavailable;
        }
        copy
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
    use paigasus_kernel::tool_policy::history::{Catalog, Digest, FieldPath, KeyId, Scalar};

    use super::*;

    struct NoKey;

    impl paigasus_kernel::tool_policy::history::HistoryDigester for NoKey {
        fn current(&self, _: uuid::Uuid, _: &FieldPath, _: &Scalar) -> Option<(KeyId, Digest)> {
            None
        }
        fn all(&self, _: uuid::Uuid, _: &FieldPath, _: &Scalar) -> Vec<(KeyId, Digest)> {
            vec![]
        }
    }

    #[test]
    fn with_history_sets_the_snapshot_history_and_keeps_the_rest() {
        let s = PolicySnapshot::new(PolicySet::Unavailable, TenantMode::LogOnly, OnUnavailable::Closed).with_request_elapsed(Duration::from_millis(3));
        assert!(s.history.is_none(), "a new snapshot has no history");
        let history = Arc::new(HistoryPolicy::build(&[], &[], uuid::Uuid::nil(), Catalog::unknown(), None, &NoKey));
        let s = s.with_history(Arc::clone(&history));
        assert!(Arc::ptr_eq(s.history.as_ref().expect("history"), &history));
        assert_eq!((s.mode, s.on_unavailable, s.request_elapsed), (TenantMode::LogOnly, OnUnavailable::Closed, Duration::from_millis(3)));
        assert!(matches!(s.set, PolicySet::Unavailable));
    }

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
    fn a_new_snapshot_has_no_seen_hosts_and_the_builder_sets_them() {
        let s = PolicySnapshot::new(PolicySet::Unavailable, TenantMode::Off, OnUnavailable::Open);
        assert!(s.seen_hosts.is_none());
        let mut set = HostSet::new();
        set.insert("canary-host.example");
        let s = s.with_seen_hosts(SeenHosts(Arc::new(set)));
        let seen = s.seen_hosts.as_ref().expect("seen hosts");
        assert!(seen.0.contains("canary-host.example"));
        let debug = format!("{s:?}");
        assert!(debug.contains("SeenHosts { len: 1 }"), "{debug}");
        assert!(!debug.contains("canary-host"), "{debug}");
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
