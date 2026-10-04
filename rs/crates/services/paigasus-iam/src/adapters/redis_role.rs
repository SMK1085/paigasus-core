// SPDX-License-Identifier: Apache-2.0

//! Which Redis connection a circuit breaker guards, and the metric names IAM gives it.
//!
//! The connection code and the breaker live in the `paigasus-redis` lib crate (SMA-726 D15). The
//! lib cannot name IAM's metrics, so IAM supplies them here. Every call site passes a
//! [`RedisRole`] to `paigasus_redis::connect`, and [`From<RedisRole>`] turns it into the
//! [`BreakerMetrics`] that the breaker emits under. The series stay byte-identical to the
//! pre-SMA-726 ones: the IAM dashboard and three IAM alert rules read them (A13).

use paigasus_observability::names;
use paigasus_redis::BreakerMetrics;

/// Which connection a breaker guards. A CLOSED set, so the `role` metric label is bounded by the
/// type system and cannot mint cardinality (SMA-476 D10).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RedisRole {
    Authz,
    ApiKeys,
    Jwks,
}

impl RedisRole {
    fn as_label(self) -> &'static str {
        match self {
            RedisRole::Authz => "authz",
            RedisRole::ApiKeys => "api_keys",
            RedisRole::Jwks => "jwks",
        }
    }
}

impl From<RedisRole> for BreakerMetrics {
    fn from(role: RedisRole) -> Self {
        BreakerMetrics {
            state: names::IAM_REDIS_BREAKER_STATE,
            transitions: names::IAM_REDIS_BREAKER_TRANSITIONS_TOTAL,
            role: role.as_label(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::RedisRole;
    use metrics_util::debugging::{DebugValue, DebuggingRecorder};
    use paigasus_observability::names;

    /// SMA-726 A13: after the move to `paigasus-redis`, every role still emits IAM's two breaker
    /// series under the same names, with exactly the same label keys and values. The IAM dashboard
    /// (`ops/observability/grafana/dashboards/iam.json`) and three alert rules
    /// (`ops/observability/prometheus/rules/iam.rules.yml`) read them.
    ///
    /// Forcing a breaker open is a transition, so one `with_open_breaker_for_tests` call emits the
    /// gauge (2) and one counter increment (`to="open"`). `#[tokio::test]` is required: the lazy
    /// `ConnectionManager` spawns on the current runtime. It never dials `127.0.0.1:1`.
    #[tokio::test]
    async fn every_role_emits_the_iam_breaker_series_under_the_pinned_names() {
        assert_eq!(names::IAM_REDIS_BREAKER_STATE, "iam_redis_breaker_state");
        assert_eq!(names::IAM_REDIS_BREAKER_TRANSITIONS_TOTAL, "iam_redis_breaker_transitions_total");

        for (role, label) in [(RedisRole::Authz, "authz"), (RedisRole::ApiKeys, "api_keys"), (RedisRole::Jwks, "jwks")] {
            let recorder = DebuggingRecorder::new();
            let snapshotter = recorder.snapshotter();
            let _handle = metrics::with_local_recorder(&recorder, || {
                paigasus_redis::with_open_breaker_for_tests("redis://127.0.0.1:1", role).expect("a well-formed redis URL; the lazy handle never dials")
            });
            let snapshot = snapshotter.snapshot().into_vec();

            let gauge = snapshot
                .iter()
                .find(|(key, _, _, _)| key.key().name() == "iam_redis_breaker_state")
                .unwrap_or_else(|| panic!("{label}: iam_redis_breaker_state was never emitted"));
            let mut gauge_labels: Vec<(&str, &str)> = gauge.0.key().labels().map(|l| (l.key(), l.value())).collect();
            gauge_labels.sort_unstable();
            assert_eq!(gauge_labels, vec![("role", label)], "{label}: the gauge must carry exactly the role label");
            assert!(
                matches!(gauge.3, DebugValue::Gauge(v) if (v.into_inner() - 2.0).abs() < f64::EPSILON),
                "{label}: an open breaker must report 2, got {:?}",
                gauge.3
            );

            let transitions = snapshot
                .iter()
                .find(|(key, _, _, _)| key.key().name() == "iam_redis_breaker_transitions_total")
                .unwrap_or_else(|| panic!("{label}: iam_redis_breaker_transitions_total was never emitted"));
            let mut transition_labels: Vec<(&str, &str)> = transitions.0.key().labels().map(|l| (l.key(), l.value())).collect();
            transition_labels.sort_unstable();
            assert_eq!(
                transition_labels,
                vec![("role", label), ("to", "open")],
                "{label}: the counter must carry exactly the role and to labels"
            );
            assert!(matches!(transitions.3, DebugValue::Counter(1)), "{label}: expected one transition, got {:?}", transitions.3);
        }
    }
}
