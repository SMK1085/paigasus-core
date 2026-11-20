// SPDX-License-Identifier: Apache-2.0

//! The generation record of the decision log (SMA-821 § 5.2, § 5.3). This module holds the pure part: the
//! stored record, the views of Redis and of the KV entry, and `decide`, which says when a Redis reset or a
//! loss inside one generation is counted. It does no I/O.

use std::time::{SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use metrics::{counter, gauge};
use paigasus_observability::names;
use serde::{Deserialize, Serialize};

/// The KV key of the entry for a Redis stream key: the lower-case hex of its UTF-8 bytes. It is deterministic,
/// collision-free and a valid NATS KV key.
#[must_use]
pub fn entry_key(stream_key: &str) -> String {
    use std::fmt::Write;
    stream_key.bytes().fold(String::with_capacity(stream_key.len() * 2), |mut out, b| {
        let _ = write!(out, "{b:02x}");
        out
    })
}

/// A fresh random id (a UUID v4 string).
#[must_use]
pub fn new_id() -> String {
    uuid::Builder::from_random_bytes(rand::random()).into_uuid().to_string()
}

/// A claim: a loss that one relay will count, tagged with a nonce so that it counts once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Claim {
    pub nonce: String,
    pub lost: Option<u64>,
}

/// The value of the KV entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenRecord {
    pub generation: String,
    pub epoch: u64,
    pub sample: Option<u64>,
    pub sampled_at: u64,
    pub claim: Option<Claim>,
}

impl GenRecord {
    /// Parse a KV value. A value that does not parse gives `None` (the same as a missing entry).
    #[must_use]
    pub fn parse(bytes: &[u8]) -> Option<GenRecord> {
        serde_json::from_slice(bytes).ok()
    }

    #[must_use]
    pub fn to_bytes(&self) -> Bytes {
        // A record of strings and integers always serializes.
        Bytes::from(serde_json::to_vec(self).unwrap_or_default())
    }
}

/// What the read-only gauge step saw for one generation. `epoch` is `None` before the first KV step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalTuple {
    pub generation: String,
    pub epoch: Option<u64>,
    pub sample: u64,
    pub sampled_at: u64,
}

/// The tuples of the current and the previous generation (D13).
#[derive(Debug, Clone, Default)]
pub struct LocalRecord {
    current: Option<LocalTuple>,
    previous: Option<LocalTuple>,
}

impl LocalRecord {
    /// Store `t`. The same generation as `current` replaces it. Another generation moves `current` to `previous`.
    pub fn observe(&mut self, t: LocalTuple) {
        match &self.current {
            Some(cur) if cur.generation == t.generation => self.current = Some(t),
            _ => {
                self.previous = self.current.take();
                self.current = Some(t);
            }
        }
    }

    #[must_use]
    pub fn of(&self, generation: &str) -> Option<&LocalTuple> {
        [self.current.as_ref(), self.previous.as_ref()].into_iter().flatten().find(|t| t.generation == generation)
    }
}

/// What the Redis detection script (step 2) returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RedisView {
    NoGroup,
    Live { generation: String, epoch: u64, xlen: u64 },
}

/// What the KV get (step 1) returned. A missing entry can still have a revision (a delete or purge marker).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KvView {
    Missing { revision: Option<u64> },
    Record { record: GenRecord, revision: u64 },
}

/// The kind of a counted loss.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LossKind {
    /// Another generation: Redis lost the stream.
    Reset,
    /// The same generation with an epoch that did not grow: Redis went back to an older state.
    Rewind,
}

/// What a KV step does next. `revision: None` means "create"; `Some(r)` means "update with revision r".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Nothing,
    Foreign,
    Write {
        record: GenRecord,
        revision: Option<u64>,
        count: Option<(LossKind, Claim)>,
    },
}

/// The rules of spec § 5.3. A write stores the values of Redis as they are. Only the claim of an `E > e` write
/// comes from the old record.
#[must_use]
pub fn decide(redis: &RedisView, kv: &KvView, last_own: Option<&str>, local: &LocalRecord, now_ms: u64, nonce: &str) -> Action {
    let RedisView::Live { generation, epoch, xlen } = redis else {
        return Action::Nothing;
    };
    let record_with = |claim: Option<Claim>| GenRecord {
        generation: generation.clone(),
        epoch: *epoch,
        sample: Some(*xlen),
        sampled_at: now_ms,
        claim,
    };
    match kv {
        KvView::Missing { revision } => Action::Write {
            record: record_with(None),
            revision: *revision,
            count: None,
        },
        KvView::Record { record: old, revision } if old.generation == *generation => {
            if *epoch > old.epoch {
                Action::Write {
                    record: record_with(old.claim.clone()),
                    revision: Some(*revision),
                    count: None,
                }
            } else {
                let claim = Claim {
                    nonce: nonce.to_owned(),
                    lost: old.sample,
                };
                Action::Write {
                    record: record_with(Some(claim.clone())),
                    revision: Some(*revision),
                    count: Some((LossKind::Rewind, claim)),
                }
            }
        }
        KvView::Record { record: old, revision } => {
            if last_own == Some(generation.as_str()) {
                return Action::Foreign;
            }
            let lost = match local.of(&old.generation) {
                Some(t) if t.sampled_at > old.sampled_at => Some(t.sample),
                _ => old.sample,
            };
            let claim = Claim { nonce: nonce.to_owned(), lost };
            Action::Write {
                record: record_with(Some(claim.clone())),
                revision: Some(*revision),
                count: Some((LossKind::Reset, claim)),
            }
        }
    }
}

/// The pending claim of an earlier, unconfirmed write: `Some(claim)` only when the entry holds its nonce.
#[must_use]
pub fn resolve_pending(pending: Option<Claim>, kv: &KvView) -> Option<Claim> {
    let pending = pending?;
    match kv {
        KvView::Record { record, .. } if record.claim.as_ref().is_some_and(|c| c.nonce == pending.nonce) => Some(pending),
        _ => None,
    }
}

/// Count one loss: the reset counter, the lost estimate (the sample is not a bound), and the last-reset gauge.
pub fn count_loss(kind: LossKind, lost: Option<u64>, now: SystemTime) {
    counter!(names::GATEWAY_DECISION_LOG_REDIS_RESETS_TOTAL).increment(1);
    let secs = now.duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
    gauge!(names::GATEWAY_DECISION_LOG_LAST_RESET_TIMESTAMP_SECONDS).set(secs.floor());
    match (kind, lost) {
        (LossKind::Reset, Some(n)) => {
            counter!(names::GATEWAY_DECISION_LOG_LOST_ESTIMATE_TOTAL).increment(n);
            tracing::error!(lost_estimate = n, "Redis lost the decision stream; the lost events are an estimate (it can undercount or overcount)");
        }
        (LossKind::Reset, None) => tracing::error!("Redis lost the decision stream; the number of lost events is unknown (no XLEN sample)"),
        (LossKind::Rewind, Some(n)) => {
            counter!(names::GATEWAY_DECISION_LOG_LOST_ESTIMATE_TOTAL).increment(n);
            tracing::error!(
                lost_estimate = n,
                "the Redis decision stream went back to an older state; the lost events are an estimate (it can undercount or overcount)"
            );
        }
        (LossKind::Rewind, None) => tracing::error!("the Redis decision stream went back to an older state; the number of lost events is unknown (no XLEN sample)"),
    }
}

/// The kind of a failed KV step. The labels are `CHECK_ERROR_KINDS`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckErrorKind {
    Timeout,
    Permission,
    NoBucket,
    Redis,
    ForeignWriter,
    Other,
}

impl CheckErrorKind {
    pub const ALL: [CheckErrorKind; 6] = [
        CheckErrorKind::Timeout,
        CheckErrorKind::Permission,
        CheckErrorKind::NoBucket,
        CheckErrorKind::Redis,
        CheckErrorKind::ForeignWriter,
        CheckErrorKind::Other,
    ];

    #[must_use]
    pub fn as_label(self) -> &'static str {
        match self {
            CheckErrorKind::Timeout => "timeout",
            CheckErrorKind::Permission => "permission",
            CheckErrorKind::NoBucket => "no_bucket",
            CheckErrorKind::Redis => "redis",
            CheckErrorKind::ForeignWriter => "foreign_writer",
            CheckErrorKind::Other => "other",
        }
    }
}

/// Count one failed KV step of `kind`.
pub fn count_check_error(kind: CheckErrorKind) {
    counter!(names::GATEWAY_DECISION_LOG_GENERATION_CHECK_ERRORS_TOTAL, "kind" => kind.as_label()).increment(1);
}

#[cfg(test)]
mod tests {
    use super::*;
    use metrics_util::debugging::{DebugValue, DebuggingRecorder};
    use std::time::Duration;

    fn live(g: &str, e: u64, x: u64) -> RedisView {
        RedisView::Live {
            generation: g.to_owned(),
            epoch: e,
            xlen: x,
        }
    }

    fn rec(g: &str, e: u64, s: Option<u64>, t: u64, claim: Option<Claim>) -> GenRecord {
        GenRecord {
            generation: g.to_owned(),
            epoch: e,
            sample: s,
            sampled_at: t,
            claim,
        }
    }

    fn kv(r: GenRecord, revision: u64) -> KvView {
        KvView::Record { record: r, revision }
    }

    fn claim(nonce: &str, lost: Option<u64>) -> Claim {
        Claim { nonce: nonce.to_owned(), lost }
    }

    fn tuple(g: &str, sample: u64, at: u64) -> LocalTuple {
        LocalTuple {
            generation: g.to_owned(),
            epoch: Some(1),
            sample,
            sampled_at: at,
        }
    }

    fn run(redis: &RedisView, kv: &KvView, last_own: Option<&str>, local: &LocalRecord) -> Action {
        decide(redis, kv, last_own, local, 9_000, "n1")
    }

    #[test]
    fn no_group_writes_nothing() {
        let l = LocalRecord::default();
        assert_eq!(run(&RedisView::NoGroup, &KvView::Missing { revision: None }, None, &l), Action::Nothing);
        assert_eq!(run(&RedisView::NoGroup, &kv(rec("K", 3, Some(1), 5, None), 7), Some("K"), &l), Action::Nothing);
    }

    #[test]
    fn a_missing_entry_creates_the_record_and_counts_nothing() {
        let l = LocalRecord::default();
        let want = rec("R", 1, Some(4), 9_000, None);
        assert_eq!(
            run(&live("R", 1, 4), &KvView::Missing { revision: None }, None, &l),
            Action::Write {
                record: want.clone(),
                revision: None,
                count: None
            }
        );
        // A delete marker has a revision: the write updates with it and still counts nothing.
        assert_eq!(
            run(&live("R", 1, 4), &KvView::Missing { revision: Some(12) }, None, &l),
            Action::Write {
                record: want,
                revision: Some(12),
                count: None
            }
        );
    }

    #[test]
    fn an_epoch_above_the_stored_one_writes_and_counts_nothing() {
        let l = LocalRecord::default();
        let old = rec("R", 4, Some(2), 100, Some(claim("old", Some(9))));
        let got = run(&live("R", 5, 6), &kv(old, 21), None, &l);
        assert_eq!(
            got,
            Action::Write {
                record: rec("R", 5, Some(6), 9_000, Some(claim("old", Some(9)))),
                revision: Some(21),
                count: None
            }
        );
    }

    #[test]
    fn an_epoch_that_went_back_counts_a_loss_with_the_stored_sample() {
        // The local tuple of R holds sample 0 (the XLEN after the loss). The count must use the stored 7.
        let mut l = LocalRecord::default();
        l.observe(tuple("R", 0, 8_000));
        let got = run(&live("R", 3, 0), &kv(rec("R", 10, Some(7), 100, None), 5), None, &l);
        let c = claim("n1", Some(7));
        assert_eq!(
            got,
            Action::Write {
                record: rec("R", 3, Some(0), 9_000, Some(c.clone())),
                revision: Some(5),
                count: Some((LossKind::Rewind, c))
            }
        );
    }

    #[test]
    fn an_equal_epoch_counts_a_loss() {
        let l = LocalRecord::default();
        let got = run(&live("R", 10, 1), &kv(rec("R", 10, None, 100, None), 6), None, &l);
        let c = claim("n1", None);
        assert_eq!(
            got,
            Action::Write {
                record: rec("R", 10, Some(1), 9_000, Some(c.clone())),
                revision: Some(6),
                count: Some((LossKind::Rewind, c))
            }
        );
    }

    #[test]
    fn a_new_generation_stores_the_epoch_of_redis_as_it_is() {
        let l = LocalRecord::default();
        let got = run(&live("R", 1, 2), &kv(rec("K", 50, Some(3), 100, None), 8), None, &l);
        let Action::Write { record, revision, count } = got else {
            panic!("expected a write, got {got:?}")
        };
        assert_eq!(record.epoch, 1);
        assert_eq!(record.generation, "R");
        assert_eq!(revision, Some(8));
        assert_eq!(count.map(|(k, c)| (k, c.lost)), Some((LossKind::Reset, Some(3))));
        // The next tick: Redis {R, E=2} against the stored record {R, 1}. Nothing is counted.
        let next = run(&live("R", 2, 2), &kv(record, 9), Some("R"), &l);
        let Action::Write { count, .. } = next else { panic!("expected a write, got {next:?}") };
        assert_eq!(count, None);
    }

    #[test]
    fn a_new_generation_counts_the_newer_of_the_kv_and_the_local_sample() {
        let lost = |stored: GenRecord, local: Option<LocalTuple>| {
            let mut l = LocalRecord::default();
            if let Some(t) = local {
                l.observe(t);
            }
            match run(&live("R", 1, 0), &kv(stored, 1), None, &l) {
                Action::Write {
                    count: Some((LossKind::Reset, c)), ..
                } => c.lost,
                other => panic!("expected a counted reset, got {other:?}"),
            }
        };
        // The local tuple is newer: 3.
        assert_eq!(lost(rec("K", 1, Some(0), 1_000, None), Some(tuple("K", 3, 2_000))), Some(3));
        // The KV sample is newer: 5.
        assert_eq!(lost(rec("K", 1, Some(5), 3_000, None), Some(tuple("K", 3, 2_000))), Some(5));
        // A local tuple of another generation is ignored.
        assert_eq!(lost(rec("K", 1, Some(5), 3_000, None), Some(tuple("X", 9, 9_999))), Some(5));
        // No sample anywhere: unknown.
        assert_eq!(lost(rec("K", 1, None, 3_000, None), None), None);
        // The KV sample is newer and null: it stands, so the loss is unknown.
        assert_eq!(lost(rec("K", 1, None, 3_000, None), Some(tuple("K", 3, 2_000))), None);
    }

    #[test]
    fn an_unchanged_own_generation_with_a_foreign_entry_is_a_foreign_writer() {
        let l = LocalRecord::default();
        assert_eq!(run(&live("R", 4, 1), &kv(rec("K", 9, Some(1), 100, None), 3), Some("R"), &l), Action::Foreign);
    }

    #[test]
    fn a_changed_own_generation_with_another_entry_is_a_reset() {
        let l = LocalRecord::default();
        let got = run(&live("R", 1, 1), &kv(rec("OLD", 9, Some(6), 100, None), 3), Some("OLD"), &l);
        let c = claim("n1", Some(6));
        assert_eq!(
            got,
            Action::Write {
                record: rec("R", 1, Some(1), 9_000, Some(c.clone())),
                revision: Some(3),
                count: Some((LossKind::Reset, c))
            }
        );
    }

    #[test]
    fn a_pending_claim_counts_only_when_the_entry_holds_its_nonce() {
        let pending = claim("mine", Some(4));
        let held = kv(rec("R", 2, Some(0), 1, Some(claim("mine", Some(4)))), 3);
        let other = kv(rec("R", 2, Some(0), 1, Some(claim("theirs", Some(4)))), 3);
        let none = kv(rec("R", 2, Some(0), 1, None), 3);
        assert_eq!(resolve_pending(Some(pending.clone()), &held), Some(pending.clone()));
        assert_eq!(resolve_pending(Some(pending.clone()), &other), None);
        assert_eq!(resolve_pending(Some(pending.clone()), &none), None);
        assert_eq!(resolve_pending(Some(pending), &KvView::Missing { revision: None }), None);
        assert_eq!(resolve_pending(None, &held), None);
    }

    #[test]
    fn the_local_record_keeps_the_current_and_the_previous_generation() {
        let mut l = LocalRecord::default();
        l.observe(tuple("A", 1, 10));
        l.observe(tuple("A", 2, 20));
        assert_eq!(l.of("A").map(|t| t.sample), Some(2));
        l.observe(tuple("B", 5, 30));
        assert_eq!(l.of("A").map(|t| t.sample), Some(2));
        assert_eq!(l.of("B").map(|t| t.sample), Some(5));
        l.observe(tuple("C", 7, 40));
        assert_eq!(l.of("A"), None);
        assert_eq!(l.of("B").map(|t| t.sample), Some(5));
        assert_eq!(l.of("C").map(|t| t.sample), Some(7));
    }

    fn hex_of(s: &str) -> String {
        s.bytes().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn the_entry_key_is_hex_and_collision_free() {
        assert_eq!(entry_key("paigasus:gateway:decisions"), hex_of("paigasus:gateway:decisions"));
        assert_ne!(entry_key("a:b"), entry_key("a.b"));
        assert!(entry_key("x:y").chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        assert_eq!(entry_key("x:y"), "783a79");
        assert_ne!(new_id(), new_id());
        assert_eq!(new_id().len(), 36);
    }

    #[test]
    fn the_record_round_trips_and_garbage_does_not_parse() {
        let r = rec("R", 3, Some(4), 5, Some(claim("n", None)));
        assert_eq!(GenRecord::parse(&r.to_bytes()), Some(r.clone()));
        let v: serde_json::Value = serde_json::from_slice(&r.to_bytes()).expect("json");
        let mut top: Vec<_> = v.as_object().expect("object").keys().cloned().collect();
        top.sort();
        assert_eq!(top, ["claim", "epoch", "generation", "sample", "sampled_at"]);
        let mut inner: Vec<_> = v["claim"].as_object().expect("object").keys().cloned().collect();
        inner.sort();
        assert_eq!(inner, ["lost", "nonce"]);
        assert_eq!(v["claim"]["lost"], serde_json::Value::Null);
        assert_eq!(GenRecord::parse(b"not json"), None);
        assert_eq!(GenRecord::parse(b"{}"), None);
        assert_eq!(GenRecord::parse(b""), None);
    }

    #[test]
    fn count_loss_sets_the_counters_and_the_gauge() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let now = UNIX_EPOCH + Duration::from_millis(1_700_000_000_900);
        metrics::with_local_recorder(&recorder, || count_loss(LossKind::Reset, Some(3), now));
        let snap = snapshotter.snapshot().into_vec();
        let counter_of = |name: &str| {
            snap.iter().find_map(|(k, _, _, v)| match v {
                DebugValue::Counter(n) if k.key().name() == name => Some(*n),
                _ => None,
            })
        };
        let gauge_of = |name: &str| {
            snap.iter().find_map(|(k, _, _, v)| match v {
                DebugValue::Gauge(g) if k.key().name() == name => Some(g.into_inner()),
                _ => None,
            })
        };
        assert_eq!(counter_of(names::GATEWAY_DECISION_LOG_REDIS_RESETS_TOTAL), Some(1));
        assert_eq!(counter_of(names::GATEWAY_DECISION_LOG_LOST_ESTIMATE_TOTAL), Some(3));
        assert_eq!(gauge_of(names::GATEWAY_DECISION_LOG_LAST_RESET_TIMESTAMP_SECONDS), Some(1_700_000_000.0));
    }

    #[test]
    fn an_unknown_loss_counts_the_reset_and_adds_no_estimate() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        metrics::with_local_recorder(&recorder, || count_loss(LossKind::Rewind, None, UNIX_EPOCH + Duration::from_secs(10)));
        let snap = snapshotter.snapshot().into_vec();
        let has = |name: &str| snap.iter().any(|(k, _, _, v)| k.key().name() == name && matches!(v, DebugValue::Counter(n) if *n > 0));
        assert!(has(names::GATEWAY_DECISION_LOG_REDIS_RESETS_TOTAL));
        assert!(!has(names::GATEWAY_DECISION_LOG_LOST_ESTIMATE_TOTAL));
    }

    #[test]
    fn the_check_error_labels_match_the_primed_kinds() {
        assert_eq!(CheckErrorKind::ALL.map(CheckErrorKind::as_label), super::super::CHECK_ERROR_KINDS);
    }

    #[test]
    fn count_check_error_uses_the_kind_label() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        metrics::with_local_recorder(&recorder, || count_check_error(CheckErrorKind::NoBucket));
        let snap = snapshotter.snapshot().into_vec();
        let n = snap.iter().find_map(|(k, _, _, v)| match v {
            DebugValue::Counter(n) if k.key().name() == names::GATEWAY_DECISION_LOG_GENERATION_CHECK_ERRORS_TOTAL && k.key().labels().any(|l| l.key() == "kind" && l.value() == "no_bucket") => {
                Some(*n)
            }
            _ => None,
        });
        assert_eq!(n, Some(1));
    }
}
