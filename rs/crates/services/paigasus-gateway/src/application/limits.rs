// SPDX-License-Identifier: Apache-2.0

//! The limits service (SMA-677 § 4.3a). The chat handler calls `Limits::admit` once, just before
//! egress (D9). It resolves the org (D2), builds the policy, skips the store for an empty policy
//! (D11), calls the store, applies the D8 precedence, counts the refusal, and returns a
//! `ChargeGuard`. A store failure admits the request with no ticket (fail-open, D10).

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex, PoisonError};
use std::time::{Duration, Instant};

use metrics::{counter, gauge};
use paigasus_observability::names;

use crate::application::charge_guard::{ChargeGuard, ChargeSource, NoTicket};
use crate::domain::limits::{ChargeDropReason, Clock, LimitDecision, LimitRefusal, LimitRules, LimitStore, LimitStoreError, RefusalReason, UnavailableKind, choose_refusal};
use crate::domain::{CallerContext, org_of};

/// D10: at most one store-failure log line per (operation, kind) in this interval — the interval
/// of IAM's `LOG_RATE_LIMIT_INTERVAL` (`paigasus-iam/src/application/log_rate_limit.rs:11`).
pub const STORE_LOG_INTERVAL: Duration = Duration::from_secs(10);

/// The `role` label of the gateway's Redis breaker series (spec § 4.8).
pub const LIMITS_BREAKER_ROLE: &str = "limits";

/// The `op` label of `gateway_limit_store_unavailable_total`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StoreOp {
    Check,
    Charge,
}

impl StoreOp {
    pub fn as_label(self) -> &'static str {
        match self {
            StoreOp::Check => "check",
            StoreOp::Charge => "charge",
        }
    }
}

/// Which store backs the limits; decides whether the breaker series are primed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreBackend {
    Memory,
    Redis,
}

/// The limits service. `Limits` is shared as `Arc<Limits>`: every `ChargeGuard` holds one.
pub struct Limits {
    rules: LimitRules,
    store: Arc<dyn LimitStore>,
    clock: Arc<dyn Clock>,
}

impl Limits {
    pub fn new(rules: LimitRules, store: Arc<dyn LimitStore>, clock: Arc<dyn Clock>) -> Self {
        Limits { rules, store, clock }
    }

    pub(crate) fn store(&self) -> &dyn LimitStore {
        self.store.as_ref()
    }

    pub(crate) fn clock(&self) -> &dyn Clock {
        self.clock.as_ref()
    }

    /// Admit or refuse one chat request (spec § 4.3a). `Err` is a refusal the handler maps to a
    /// `GatewayError`; `Ok` carries the guard that charges the request when it drops.
    pub async fn admit(self: &Arc<Self>, caller: &CallerContext) -> Result<ChargeGuard, LimitRefusal> {
        let org = org_of(&caller.scope_prn);
        let policy = self.rules.policy_for(org);
        if org.is_none() {
            // D2: a valid request always has an org, so this is a parse mismatch or an IAM defect.
            counter!(names::GATEWAY_LIMIT_UNSCOPED_REQUESTS_TOTAL).increment(1);
            tracing::warn!(principal = %caller.principal_prn, scope = %caller.scope_prn, "chat request scope names no organization");
            if policy.has_org_dimension() {
                return Err(LimitRefusal::Unscoped);
            }
        }
        if policy.is_empty() {
            return Ok(ChargeGuard::new(Arc::clone(self), None, org, NoTicket::NoBudget));
        }
        match self.store.check_and_admit(&caller.principal_prn, org, &policy, self.clock.now()).await {
            Ok(LimitDecision::Admit(ticket)) => Ok(ChargeGuard::new(Arc::clone(self), ticket, org, NoTicket::NoBudget)),
            Ok(LimitDecision::Refused(failed)) => match choose_refusal(&failed) {
                Some(refusal) => {
                    if let Some(reason) = refusal.reason() {
                        counter!(names::GATEWAY_LIMIT_REFUSALS_TOTAL, "reason" => reason.as_label()).increment(1);
                    }
                    Err(refusal)
                }
                // A refusal with no failed check is an adapter defect; treat it as a store fault.
                None => {
                    report_store_unavailable(StoreOp::Check, UnavailableKind::Decode, "the store refused a request with no failed check");
                    Ok(ChargeGuard::new(Arc::clone(self), None, org, NoTicket::StoreUnavailable))
                }
            },
            Err(LimitStoreError::Unavailable { kind, detail }) => {
                report_store_unavailable(StoreOp::Check, kind, &detail);
                Ok(ChargeGuard::new(Arc::clone(self), None, org, NoTicket::StoreUnavailable))
            }
        }
    }
}

/// D10: one store-failure log line per (operation, kind) per interval. IAM's `LogRateLimiter` is
/// `pub(crate)` there, so the gateway holds this small copy with the same policy: a suppressed
/// event is still counted by the metric, and the next line reports how many were suppressed.
pub struct StoreLogLimiter {
    interval: Duration,
    last: Mutex<HashMap<(StoreOp, UnavailableKind), (Instant, u64)>>,
}

impl StoreLogLimiter {
    pub fn new(interval: Duration) -> Self {
        StoreLogLimiter {
            interval,
            last: Mutex::new(HashMap::new()),
        }
    }

    /// `Some(suppressed)` when a line may be written at `now`; `None` when it is suppressed.
    pub fn admit_at(&self, op: StoreOp, kind: UnavailableKind, now: Instant) -> Option<u64> {
        let mut last = self.last.lock().unwrap_or_else(PoisonError::into_inner);
        match last.get_mut(&(op, kind)) {
            Some((at, suppressed)) if now.saturating_duration_since(*at) < self.interval => {
                *suppressed = suppressed.saturating_add(1);
                None
            }
            Some((at, suppressed)) => {
                let count = *suppressed;
                *at = now;
                *suppressed = 0;
                Some(count)
            }
            None => {
                last.insert((op, kind), (now, 0));
                Some(0)
            }
        }
    }
}

static STORE_LOG: LazyLock<StoreLogLimiter> = LazyLock::new(|| StoreLogLimiter::new(STORE_LOG_INTERVAL));

/// D10: count a failed store call and log it (rate-limited). `Io` logs at `warn`: Redis is down,
/// and fail-open is the designed answer. `Server` and `Decode` log at `error`: Redis answered, so
/// the fault is a misconfiguration or a defect that fail-open would otherwise hide.
pub fn report_store_unavailable(op: StoreOp, kind: UnavailableKind, detail: &str) {
    report_store_unavailable_with(&STORE_LOG, Instant::now(), op, kind, detail);
}

fn report_store_unavailable_with(limiter: &StoreLogLimiter, now: Instant, op: StoreOp, kind: UnavailableKind, detail: &str) {
    counter!(names::GATEWAY_LIMIT_STORE_UNAVAILABLE_TOTAL, "op" => op.as_label(), "kind" => kind.as_label()).increment(1);
    let Some(suppressed) = limiter.admit_at(op, kind, now) else {
        return;
    };
    match kind {
        UnavailableKind::Io => tracing::warn!(op = op.as_label(), kind = kind.as_label(), suppressed, error = %detail, "limit store unavailable; limits not applied (fail-open)"),
        UnavailableKind::Server | UnavailableKind::Decode => {
            tracing::error!(op = op.as_label(), kind = kind.as_label(), suppressed, error = %detail, "limit store unavailable; limits not applied (fail-open)")
        }
    }
}

/// A7: register every limit series at zero, so `increase()` sees the first event (memory
/// `prometheus-new-counter-first-sample-trap`). The two breaker series only for Redis; the
/// breaker's constructor also sets its gauge. `build_limits` (Task 13) and the metric tests call
/// this one function.
pub fn prime_metrics(backend: StoreBackend) {
    for reason in RefusalReason::ALL {
        counter!(names::GATEWAY_LIMIT_REFUSALS_TOTAL, "reason" => reason.as_label()).increment(0);
    }
    for source in ChargeSource::ALL {
        counter!(names::GATEWAY_TOKENS_CHARGED_TOTAL, "source" => source.as_label()).increment(0);
    }
    counter!(names::GATEWAY_LIMIT_UNSCOPED_REQUESTS_TOTAL).increment(0);
    for op in [StoreOp::Check, StoreOp::Charge] {
        for kind in UnavailableKind::ALL {
            counter!(names::GATEWAY_LIMIT_STORE_UNAVAILABLE_TOTAL, "op" => op.as_label(), "kind" => kind.as_label()).increment(0);
        }
    }
    for reason in ChargeDropReason::ALL {
        counter!(names::GATEWAY_LIMIT_CHARGES_DROPPED_TOTAL, "reason" => reason.as_label()).increment(0);
    }
    if backend == StoreBackend::Redis {
        gauge!(names::GATEWAY_REDIS_BREAKER_STATE, "role" => LIMITS_BREAKER_ROLE).set(0.0);
        for to in ["closed", "half_open", "open"] {
            counter!(names::GATEWAY_REDIS_BREAKER_TRANSITIONS_TOTAL, "role" => LIMITS_BREAKER_ROLE, "to" => to).increment(0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::limits::{BudgetPeriod, FailedCheck, OrgOverride, RateCounts, RefusalReason};
    use crate::test_support::{FixedClock, ORG_A, ORG_A_PRN, ScriptedStore, TEAM_IN_A, at, caller, counter, gauge};
    use metrics_util::debugging::DebuggingRecorder;
    use std::collections::HashMap;
    use std::num::NonZeroU64;
    use std::sync::atomic::Ordering;

    fn nz(n: u64) -> NonZeroU64 {
        NonZeroU64::new(n).expect("non-zero")
    }

    fn limits(rules: LimitRules, store: Arc<ScriptedStore>) -> Arc<Limits> {
        Arc::new(Limits::new(rules, store, Arc::new(FixedClock(at("2026-10-02T12:00:00Z")))))
    }

    fn budget_rules() -> LimitRules {
        LimitRules {
            tokens_per_period: Some(nz(100)),
            ..LimitRules::default()
        }
    }

    fn admit_without_ticket() -> Result<LimitDecision, LimitStoreError> {
        Ok(LimitDecision::Admit(None))
    }

    #[tokio::test]
    async fn an_empty_policy_never_calls_the_store() {
        let store = ScriptedStore::new(admit_without_ticket);
        let rules = LimitRules {
            overrides: HashMap::from([(
                ORG_A,
                OrgOverride {
                    exempt: true,
                    ..OrgOverride::default()
                },
            )]),
            tokens_per_period: Some(nz(5)),
            ..LimitRules::default()
        };
        let guard = limits(rules, store.clone()).admit(&caller(ORG_A_PRN)).await.expect("admitted");
        assert!(!guard.has_ticket());
        assert_eq!(store.checks.load(Ordering::SeqCst), 0, "D11: an exempt org with no principal rate makes no store call");
    }

    #[tokio::test]
    async fn the_store_sees_the_org_of_a_team_scope() {
        let store = ScriptedStore::new(admit_without_ticket);
        limits(budget_rules(), store.clone()).admit(&caller(TEAM_IN_A)).await.expect("admitted");
        assert_eq!(*store.orgs.lock().expect("not poisoned"), vec![Some(ORG_A)], "D2: a team scope counts against its org");
    }

    #[tokio::test]
    async fn a_refusal_counts_one_reason_by_the_d8_precedence() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _local = metrics::set_default_local_recorder(&recorder);
        let store = ScriptedStore::new(|| {
            let counts = RateCounts {
                previous: 0,
                current: 2,
                elapsed_ms: 0,
                limit: 2,
            };
            Ok(LimitDecision::Refused(vec![FailedCheck::OrgRate(counts), FailedCheck::PrincipalRate(counts)]))
        });
        let refusal = limits(
            LimitRules {
                principal_requests_per_minute: Some(nz(2)),
                org_requests_per_minute: Some(nz(2)),
                ..LimitRules::default()
            },
            store,
        )
        .admit(&caller(ORG_A_PRN))
        .await
        .expect_err("refused");
        assert!(
            matches!(
                refusal,
                LimitRefusal::RateLimited {
                    reason: RefusalReason::PrincipalRate,
                    ..
                }
            ),
            "{refusal:?}"
        );
        assert_eq!(counter(&snapshotter, names::GATEWAY_LIMIT_REFUSALS_TOTAL, &[("reason", "principal_rate")]), Some(1));
        assert_eq!(
            counter(&snapshotter, names::GATEWAY_LIMIT_REFUSALS_TOTAL, &[("reason", "org_rate")]),
            None,
            "only one refusal is counted"
        );
    }

    #[tokio::test]
    async fn a_budget_refusal_wins_and_is_counted_as_org_budget() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _local = metrics::set_default_local_recorder(&recorder);
        let store = ScriptedStore::new(|| {
            Ok(LimitDecision::Refused(vec![FailedCheck::Budget {
                period: BudgetPeriod::Monthly,
                resets_at_unix: 1_793_491_200,
            }]))
        });
        let refusal = limits(budget_rules(), store).admit(&caller(ORG_A_PRN)).await.expect_err("refused");
        assert_eq!(
            refusal,
            LimitRefusal::BudgetExhausted {
                period: BudgetPeriod::Monthly,
                resets_at_unix: 1_793_491_200
            }
        );
        assert_eq!(counter(&snapshotter, names::GATEWAY_LIMIT_REFUSALS_TOTAL, &[("reason", "org_budget")]), Some(1));
    }

    /// A12/D10: a failed check admits with no ticket and counts `op="check"` with its kind.
    #[tokio::test]
    async fn a_store_failure_admits_without_a_ticket_and_counts_the_kind() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _local = metrics::set_default_local_recorder(&recorder);
        let store = ScriptedStore::new(|| {
            Err(LimitStoreError::Unavailable {
                kind: UnavailableKind::Server,
                detail: "OOM command not allowed".to_owned(),
            })
        });
        let guard = limits(budget_rules(), store).admit(&caller(ORG_A_PRN)).await.expect("fail-open admits");
        assert!(!guard.has_ticket(), "no ticket, so no charge");
        assert_eq!(counter(&snapshotter, names::GATEWAY_LIMIT_STORE_UNAVAILABLE_TOTAL, &[("op", "check"), ("kind", "server")]), Some(1));
    }

    /// D2/Q11: a scope with no org is refused when an org dimension applies …
    #[tokio::test]
    async fn an_unscoped_request_is_refused_when_a_budget_applies() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _local = metrics::set_default_local_recorder(&recorder);
        let store = ScriptedStore::new(admit_without_ticket);
        let refusal = limits(budget_rules(), store.clone())
            .admit(&caller("prn:paigasus:iam:default:scope/team-a"))
            .await
            .expect_err("fail-closed");
        assert_eq!(refusal, LimitRefusal::Unscoped);
        assert_eq!(store.checks.load(Ordering::SeqCst), 0);
        assert_eq!(counter(&snapshotter, names::GATEWAY_LIMIT_UNSCOPED_REQUESTS_TOTAL, &[]), Some(1));
    }

    /// … and admitted under the principal rate alone when no org dimension applies.
    #[tokio::test]
    async fn an_unscoped_request_keeps_the_principal_rate() {
        let store = ScriptedStore::new(admit_without_ticket);
        let rules = LimitRules {
            principal_requests_per_minute: Some(nz(5)),
            ..LimitRules::default()
        };
        limits(rules, store.clone()).admit(&caller("not-a-prn")).await.expect("admitted");
        assert_eq!(*store.orgs.lock().expect("not poisoned"), vec![None]);
    }

    #[test]
    fn the_store_log_limiter_admits_one_line_per_interval_and_counts_the_rest() {
        let limiter = StoreLogLimiter::new(Duration::from_secs(10));
        let t0 = Instant::now();
        assert_eq!(limiter.admit_at(StoreOp::Check, UnavailableKind::Io, t0), Some(0));
        assert_eq!(limiter.admit_at(StoreOp::Check, UnavailableKind::Io, t0 + Duration::from_secs(1)), None);
        assert_eq!(limiter.admit_at(StoreOp::Check, UnavailableKind::Io, t0 + Duration::from_secs(2)), None);
        // Another (operation, kind) is independent.
        assert_eq!(limiter.admit_at(StoreOp::Charge, UnavailableKind::Io, t0 + Duration::from_secs(2)), Some(0));
        assert_eq!(limiter.admit_at(StoreOp::Check, UnavailableKind::Server, t0 + Duration::from_secs(2)), Some(0));
        assert_eq!(limiter.admit_at(StoreOp::Check, UnavailableKind::Io, t0 + Duration::from_secs(11)), Some(2));
    }

    #[test]
    fn io_logs_at_warn_and_server_logs_at_error() {
        let (logs, _guard) = paigasus_logging::test_support::capture_logs_at(tracing::Level::WARN);
        let limiter = StoreLogLimiter::new(STORE_LOG_INTERVAL);
        let now = Instant::now();
        report_store_unavailable_with(&limiter, now, StoreOp::Check, UnavailableKind::Io, "connection refused");
        report_store_unavailable_with(&limiter, now, StoreOp::Charge, UnavailableKind::Server, "OOM command not allowed");
        report_store_unavailable_with(&limiter, now, StoreOp::Charge, UnavailableKind::Server, "second-detail");
        let text = logs.text();
        let io = text.lines().find(|l| l.contains("connection refused")).expect("the io line");
        assert!(io.contains("WARN"), "{io}");
        let server = text.lines().find(|l| l.contains("OOM command not allowed")).expect("the server line");
        assert!(server.contains("ERROR"), "{server}");
        assert!(!text.contains("second-detail"), "a second line inside 10 s is suppressed");
    }

    /// A7: every series of § 4.8 at zero; the breaker series only for Redis.
    #[test]
    fn prime_metrics_primes_every_series_at_zero() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        metrics::with_local_recorder(&recorder, || prime_metrics(StoreBackend::Redis));
        for reason in ["principal_rate", "org_rate", "org_budget"] {
            assert_eq!(counter(&snapshotter, names::GATEWAY_LIMIT_REFUSALS_TOTAL, &[("reason", reason)]), Some(0), "{reason}");
        }
        for source in ["reported", "estimated"] {
            assert_eq!(counter(&snapshotter, names::GATEWAY_TOKENS_CHARGED_TOTAL, &[("source", source)]), Some(0), "{source}");
        }
        assert_eq!(counter(&snapshotter, names::GATEWAY_LIMIT_UNSCOPED_REQUESTS_TOTAL, &[]), Some(0));
        for op in ["check", "charge"] {
            for kind in ["io", "server", "decode"] {
                assert_eq!(
                    counter(&snapshotter, names::GATEWAY_LIMIT_STORE_UNAVAILABLE_TOTAL, &[("op", op), ("kind", kind)]),
                    Some(0),
                    "{op}/{kind}"
                );
            }
        }
        for reason in ["no_runtime", "shutdown", "period_expired"] {
            assert_eq!(counter(&snapshotter, names::GATEWAY_LIMIT_CHARGES_DROPPED_TOTAL, &[("reason", reason)]), Some(0), "{reason}");
        }
        assert_eq!(gauge(&snapshotter, names::GATEWAY_REDIS_BREAKER_STATE, &[("role", "limits")]), Some(0.0));
        for to in ["closed", "half_open", "open"] {
            assert_eq!(
                counter(&snapshotter, names::GATEWAY_REDIS_BREAKER_TRANSITIONS_TOTAL, &[("role", "limits"), ("to", to)]),
                Some(0),
                "{to}"
            );
        }
    }

    #[test]
    fn prime_metrics_for_memory_has_no_breaker_series() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        metrics::with_local_recorder(&recorder, || prime_metrics(StoreBackend::Memory));
        assert_eq!(counter(&snapshotter, names::GATEWAY_LIMIT_UNSCOPED_REQUESTS_TOTAL, &[]), Some(0));
        assert_eq!(gauge(&snapshotter, names::GATEWAY_REDIS_BREAKER_STATE, &[("role", "limits")]), None);
        assert_eq!(counter(&snapshotter, names::GATEWAY_REDIS_BREAKER_TRANSITIONS_TOTAL, &[("role", "limits"), ("to", "open")]), None);
    }
}
