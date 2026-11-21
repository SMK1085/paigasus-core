// SPDX-License-Identifier: Apache-2.0

//! The generation record of the decision log (SMA-821 § 5.2, § 5.3). This module holds the pure part: the
//! stored record, the views of Redis and of the KV entry, and `decide`, which says when a Redis reset or a
//! loss inside one generation is counted. It does no I/O.

use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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
    /// Parse a KV value. A value that does not parse gives `None` (the same as a missing entry). Only a JSON
    /// object is a record, and its claim is an object or null: serde also reads a struct from a positional array,
    /// and the relay never writes one.
    #[must_use]
    pub fn parse(bytes: &[u8]) -> Option<GenRecord> {
        let value = serde_json::from_slice::<serde_json::Value>(bytes).ok()?;
        let claim_ok = value.get("claim").is_none_or(|c| c.is_object() || c.is_null());
        if !value.is_object() || !claim_ok {
            return None;
        }
        serde_json::from_value(value).ok()
    }

    #[must_use]
    pub fn to_bytes(&self) -> Bytes {
        Bytes::from(serde_json::to_vec(self).expect("a record of strings and integers always serializes"))
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

/// A KV entry as the port returns it. `live` is false for a delete or purge marker.
#[derive(Debug, Clone)]
pub struct KvEntry {
    pub value: Bytes,
    pub revision: u64,
    pub live: bool,
}

/// A failed KV call. `WrongRevision` also covers a create that finds the key (`AlreadyExists`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KvError {
    WrongRevision,
    Timeout,
    NoBucket,
    Other,
}

/// The KV calls of one generation entry. The real adapter wraps the NATS bucket; tests use a fake.
pub trait GenKv: Send + Sync {
    fn get(&self) -> impl Future<Output = Result<Option<KvEntry>, KvError>> + Send;
    fn create(&self, value: Bytes) -> impl Future<Output = Result<u64, KvError>> + Send;
    fn update(&self, value: Bytes, revision: u64) -> impl Future<Output = Result<u64, KvError>> + Send;
}

impl KvView {
    /// `None` gives `Missing { None }`. A marker, or a value that does not parse, gives `Missing { Some(revision) }`.
    #[must_use]
    pub fn from_entry(e: Option<KvEntry>) -> KvView {
        match e {
            None => KvView::Missing { revision: None },
            Some(e) if !e.live => KvView::Missing { revision: Some(e.revision) },
            Some(e) => match GenRecord::parse(&e.value) {
                Some(record) => KvView::Record { record, revision: e.revision },
                None => KvView::Missing { revision: Some(e.revision) },
            },
        }
    }
}

/// How one KV step ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepOutcome {
    NoGroup,
    Wrote,
    Counted(LossKind),
    CasLost,
    Foreign,
    Failed(CheckErrorKind),
}

/// The minimum time between two warnings of one error kind.
const WARN_EVERY: Duration = Duration::from_secs(60);

#[derive(Debug, Default)]
struct GenState {
    /// A counting write that is not confirmed yet. It is stored BEFORE the write await.
    pending: Option<(LossKind, Claim)>,
    /// The generation of this relay's last successful KV write.
    last_own: Option<String>,
    local: LocalRecord,
    last_warn: [Option<tokio::time::Instant>; CheckErrorKind::ALL.len()],
}

/// The state of one relay's generation check.
#[derive(Debug, Default)]
pub struct Generation {
    state: Mutex<GenState>,
}

enum StepErr {
    Kv(KvError),
    Redis,
}

fn unix_ms(now: SystemTime) -> u64 {
    now.duration_since(UNIX_EPOCH).map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

impl Generation {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, GenState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Store what the read-only gauge step saw.
    pub fn observe(&self, t: LocalTuple) {
        self.lock().local.observe(t);
    }

    /// One KV step (spec § 5.3), inside one timeout. A failure forgets `last_own`.
    pub async fn step<K, F, Fut>(&self, kv: &K, detect: F, kv_violations: &AtomicU64, timeout: Duration) -> StepOutcome
    where
        K: GenKv,
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = Result<RedisView, redis::RedisError>> + Send,
    {
        let before = kv_violations.load(Ordering::Relaxed);
        // True while the Redis detection runs. The outer timeout reads it to know which call it cut (M4).
        let detecting = AtomicBool::new(false);
        let err = match tokio::time::timeout(timeout, self.run(kv, detect, &detecting)).await {
            Ok(Ok(outcome)) => return outcome,
            Ok(Err(e)) => e,
            Err(_elapsed) if detecting.load(Ordering::Relaxed) => StepErr::Redis,
            Err(_elapsed) => StepErr::Kv(KvError::Timeout),
        };
        let kind = match err {
            StepErr::Redis => CheckErrorKind::Redis,
            StepErr::Kv(KvError::Timeout) if kv_violations.load(Ordering::Relaxed) > before => CheckErrorKind::Permission,
            StepErr::Kv(KvError::Timeout) => CheckErrorKind::Timeout,
            StepErr::Kv(KvError::NoBucket) => CheckErrorKind::NoBucket,
            // `run` turns a `WrongRevision` of a write into `CasLost`, and the adapter's get never returns it. The
            // arm stays because the get has the type `KvError`, and the match must cover every variant.
            StepErr::Kv(KvError::Other | KvError::WrongRevision) => CheckErrorKind::Other,
        };
        count_check_error(kind);
        self.lock().last_own = None;
        if self.warn_due(kind) {
            tracing::warn!(kind = kind.as_label(), "the decision log generation check failed; the next tick tries again");
        }
        StepOutcome::Failed(kind)
    }

    /// True when no log of `kind` came in the last [`WARN_EVERY`]. It then stores the time of this one.
    fn warn_due(&self, kind: CheckErrorKind) -> bool {
        let mut st = self.lock();
        let i = CheckErrorKind::ALL.iter().position(|k| *k == kind).unwrap_or(0);
        let now = tokio::time::Instant::now();
        let due = st.last_warn[i].is_none_or(|t| now.duration_since(t) >= WARN_EVERY);
        if due {
            st.last_warn[i] = Some(now);
        }
        due
    }

    async fn run<K, F, Fut>(&self, kv: &K, detect: F, detecting: &AtomicBool) -> Result<StepOutcome, StepErr>
    where
        K: GenKv,
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = Result<RedisView, redis::RedisError>> + Send,
    {
        let entry = kv.get().await.map_err(StepErr::Kv)?;
        let view = KvView::from_entry(entry);
        let pending = self.lock().pending.take();
        if let Some((kind, c)) = pending
            && let Some(c) = resolve_pending(Some(c), &view)
        {
            count_loss(kind, c.lost, SystemTime::now());
        }
        detecting.store(true, Ordering::Relaxed);
        let redis_view = detect().await.map_err(|_| StepErr::Redis);
        detecting.store(false, Ordering::Relaxed);
        let redis_view = redis_view?;
        let now = SystemTime::now();
        let action = {
            let st = self.lock();
            decide(&redis_view, &view, st.last_own.as_deref(), &st.local, unix_ms(now), &new_id())
        };
        let (record, revision, count) = match action {
            Action::Nothing => return Ok(StepOutcome::NoGroup),
            Action::Foreign => {
                // The metric counts every tick. The log is rate-limited like the warnings (spec § 5.3).
                count_check_error(CheckErrorKind::ForeignWriter);
                if self.warn_due(CheckErrorKind::ForeignWriter) {
                    tracing::error!("the generation entry holds a generation that another writer set; two deployments may share one entry");
                }
                return Ok(StepOutcome::Foreign);
            }
            Action::Write { record, revision, count } => (record, revision, count),
        };
        if let Some((kind, claim)) = &count {
            self.lock().pending = Some((*kind, claim.clone()));
        }
        let generation = record.generation.clone();
        let bytes = record.to_bytes();
        let written = match revision {
            None => kv.create(bytes).await,
            Some(r) => kv.update(bytes, r).await,
        };
        match written {
            Ok(_) => {
                let mut st = self.lock();
                st.last_own = Some(generation);
                if let Some((kind, claim)) = count {
                    st.pending = None;
                    drop(st);
                    count_loss(kind, claim.lost, SystemTime::now());
                    Ok(StepOutcome::Counted(kind))
                } else {
                    Ok(StepOutcome::Wrote)
                }
            }
            Err(KvError::WrongRevision) => Ok(StepOutcome::CasLost),
            Err(e) => Err(StepErr::Kv(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use metrics_util::debugging::{DebugValue, DebuggingRecorder};
    use std::sync::Arc;

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
        // A tie: the KV sample stands (the local one must be strictly newer).
        assert_eq!(lost(rec("K", 1, Some(5), 2_000, None), Some(tuple("K", 3, 2_000))), Some(5));
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
        assert_eq!(GenRecord::parse(br#"["other",1,5,0,null]"#), None, "a positional array is not a record");
        let array_claim = br#"{"generation":"R","epoch":3,"sample":4,"sampled_at":5,"claim":["n",null]}"#;
        assert_eq!(GenRecord::parse(array_claim), None, "a positional claim is not a claim");
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

    fn snap_counter(snap: &[(metrics_util::CompositeKey, Option<metrics::Unit>, Option<metrics::SharedString>, DebugValue)], name: &str, label: Option<(&str, &str)>) -> u64 {
        snap.iter()
            .filter_map(|(k, _, _, v)| match v {
                DebugValue::Counter(n) if k.key().name() == name && label.is_none_or(|(lk, lv)| k.key().labels().any(|l| l.key() == lk && l.value() == lv)) => Some(*n),
                _ => None,
            })
            .sum()
    }

    #[test]
    fn count_loss_adds_the_estimate_only_when_it_is_known() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        metrics::with_local_recorder(&recorder, || {
            count_loss(LossKind::Rewind, Some(2), UNIX_EPOCH + Duration::from_secs(10));
        });
        let snap = snapshotter.snapshot().into_vec();
        assert_eq!(snap_counter(&snap, names::GATEWAY_DECISION_LOG_REDIS_RESETS_TOTAL, None), 1);
        assert_eq!(snap_counter(&snap, names::GATEWAY_DECISION_LOG_LOST_ESTIMATE_TOTAL, None), 2);
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        metrics::with_local_recorder(&recorder, || {
            count_loss(LossKind::Reset, None, UNIX_EPOCH + Duration::from_secs(10));
        });
        let snap = snapshotter.snapshot().into_vec();
        assert_eq!(snap_counter(&snap, names::GATEWAY_DECISION_LOG_REDIS_RESETS_TOTAL, None), 1);
        assert_eq!(snap_counter(&snap, names::GATEWAY_DECISION_LOG_LOST_ESTIMATE_TOTAL, None), 0);
    }

    // ---- the KV step ----

    #[derive(Default)]
    struct FakeInner {
        entry: Option<(Bytes, u64, bool)>,
        rev: u64,
    }

    #[derive(Default)]
    struct FakeKv {
        inner: Mutex<FakeInner>,
        update_then_fail: Option<KvError>,
        update_fail_without_apply: Option<KvError>,
        update_then_hang: bool,
        get_error: Option<KvError>,
        get_hang: bool,
        race: Mutex<Option<Bytes>>,
        bump_on_get: Option<Arc<AtomicU64>>,
    }

    impl FakeKv {
        fn set(&self, r: &GenRecord) {
            let mut i = self.inner.lock().expect("lock");
            i.rev += 1;
            let rev = i.rev;
            i.entry = Some((r.to_bytes(), rev, true));
        }
        fn set_raw(&self, b: &'static [u8]) {
            let mut i = self.inner.lock().expect("lock");
            i.rev += 1;
            let rev = i.rev;
            i.entry = Some((Bytes::from_static(b), rev, true));
        }
        fn record(&self) -> Option<GenRecord> {
            self.inner.lock().expect("lock").entry.as_ref().and_then(|(b, _, _)| GenRecord::parse(b))
        }
        fn revision(&self) -> u64 {
            self.inner.lock().expect("lock").rev
        }
        fn apply(&self, v: Bytes) -> u64 {
            let mut i = self.inner.lock().expect("lock");
            i.rev += 1;
            let rev = i.rev;
            i.entry = Some((v, rev, true));
            rev
        }
    }

    impl GenKv for FakeKv {
        async fn get(&self) -> Result<Option<KvEntry>, KvError> {
            if let Some(v) = &self.bump_on_get {
                v.fetch_add(1, Ordering::Relaxed);
            }
            if let Some(e) = self.get_error {
                return Err(e);
            }
            if self.get_hang {
                std::future::pending::<()>().await;
            }
            Ok(self.inner.lock().expect("lock").entry.clone().map(|(value, revision, live)| KvEntry { value, revision, live }))
        }
        async fn create(&self, value: Bytes) -> Result<u64, KvError> {
            if self.inner.lock().expect("lock").entry.as_ref().is_some_and(|e| e.2) {
                return Err(KvError::WrongRevision);
            }
            Ok(self.apply(value))
        }
        async fn update(&self, value: Bytes, revision: u64) -> Result<u64, KvError> {
            let race = self.race.lock().expect("lock").take();
            if let Some(b) = race {
                self.apply(b);
            }
            if let Some(e) = self.update_fail_without_apply {
                return Err(e);
            }
            if self.inner.lock().expect("lock").rev != revision {
                return Err(KvError::WrongRevision);
            }
            let rev = self.apply(value);
            if let Some(e) = self.update_then_fail {
                return Err(e);
            }
            if self.update_then_hang {
                std::future::pending::<()>().await;
            }
            Ok(rev)
        }
    }

    const T: Duration = Duration::from_secs(5);

    async fn step_with(g: &Generation, kv: &FakeKv, r: RedisView) -> StepOutcome {
        g.step(kv, || async move { Ok(r) }, &AtomicU64::new(0), T).await
    }

    /// The debugging recorder drains a counter when it reads it, so this keeps the running totals.
    struct Tally {
        snapshotter: metrics_util::debugging::Snapshotter,
        totals: Mutex<std::collections::HashMap<String, u64>>,
    }

    impl Tally {
        fn new(snapshotter: metrics_util::debugging::Snapshotter) -> Self {
            Self {
                snapshotter,
                totals: Mutex::default(),
            }
        }
        fn get(&self, name: &str, label: Option<(&str, &str)>) -> u64 {
            let mut totals = self.totals.lock().expect("lock");
            for (k, _, _, v) in self.snapshotter.snapshot().into_vec() {
                if let DebugValue::Counter(n) = v {
                    let labels: String = k.key().labels().map(|l| format!("{}={};", l.key(), l.value())).collect();
                    *totals.entry(format!("{}|{labels}", k.key().name())).or_default() += n;
                }
            }
            let want = label.map(|(lk, lv)| format!("{lk}={lv};"));
            totals
                .iter()
                .filter(|(key, _)| {
                    let (n, l) = key.split_once('|').expect("key");
                    n == name && want.as_ref().is_none_or(|w| l.contains(w.as_str()))
                })
                .map(|(_, v)| *v)
                .sum()
        }
    }

    fn resets(s: &Tally) -> u64 {
        s.get(names::GATEWAY_DECISION_LOG_REDIS_RESETS_TOTAL, None)
    }
    fn lost(s: &Tally) -> u64 {
        s.get(names::GATEWAY_DECISION_LOG_LOST_ESTIMATE_TOTAL, None)
    }
    fn errors(s: &Tally, kind: &str) -> u64 {
        s.get(names::GATEWAY_DECISION_LOG_GENERATION_CHECK_ERRORS_TOTAL, Some(("kind", kind)))
    }

    #[tokio::test(start_paused = true)]
    async fn an_ambiguous_counting_write_counts_once_on_the_next_get() {
        let rec_ = DebuggingRecorder::new();
        let s = Tally::new(rec_.snapshotter());
        let _guard = metrics::set_default_local_recorder(&rec_);
        let g = Generation::new();
        let mut kv = FakeKv::default();
        kv.set(&rec("K", 1, Some(7), 100, None));
        kv.update_then_fail = Some(KvError::Timeout);
        assert_eq!(step_with(&g, &kv, live("R", 1, 0)).await, StepOutcome::Failed(CheckErrorKind::Timeout));
        assert_eq!(resets(&s), 0);
        kv.update_then_fail = None;
        assert_eq!(step_with(&g, &kv, live("R", 2, 1)).await, StepOutcome::Wrote);
        assert_eq!(resets(&s), 1);
        assert_eq!(lost(&s), 7);
        for e in 3..6 {
            assert_eq!(step_with(&g, &kv, live("R", e, 1)).await, StepOutcome::Wrote);
        }
        assert_eq!(resets(&s), 1);
        assert_eq!(lost(&s), 7);
    }

    #[tokio::test(start_paused = true)]
    async fn a_cancelled_counting_write_counts_once_on_the_next_get() {
        let rec_ = DebuggingRecorder::new();
        let s = Tally::new(rec_.snapshotter());
        let _guard = metrics::set_default_local_recorder(&rec_);
        let g = Generation::new();
        let mut kv = FakeKv::default();
        kv.set(&rec("K", 1, Some(7), 100, None));
        kv.update_then_hang = true;
        assert_eq!(step_with(&g, &kv, live("R", 1, 0)).await, StepOutcome::Failed(CheckErrorKind::Timeout));
        assert_eq!(resets(&s), 0);
        kv.update_then_hang = false;
        for e in 2..5 {
            assert_eq!(step_with(&g, &kv, live("R", e, 1)).await, StepOutcome::Wrote);
        }
        assert_eq!(resets(&s), 1);
        assert_eq!(lost(&s), 7);
    }

    #[tokio::test(start_paused = true)]
    async fn a_pending_nonce_that_the_entry_does_not_hold_counts_nothing() {
        let rec_ = DebuggingRecorder::new();
        let s = Tally::new(rec_.snapshotter());
        let _guard = metrics::set_default_local_recorder(&rec_);
        let g = Generation::new();
        let mut kv = FakeKv::default();
        kv.set(&rec("K", 1, Some(7), 100, None));
        kv.update_fail_without_apply = Some(KvError::Timeout);
        assert_eq!(step_with(&g, &kv, live("R", 1, 0)).await, StepOutcome::Failed(CheckErrorKind::Timeout));
        kv.update_fail_without_apply = None;
        kv.set(&rec("R", 1, Some(0), 200, None));
        assert_eq!(step_with(&g, &kv, live("R", 2, 1)).await, StepOutcome::Wrote);
        assert_eq!(resets(&s), 0);
        assert_eq!(lost(&s), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn a_concurrent_write_loses_the_cas_and_counts_nothing() {
        let rec_ = DebuggingRecorder::new();
        let s = Tally::new(rec_.snapshotter());
        let _guard = metrics::set_default_local_recorder(&rec_);
        let g = Generation::new();
        let kv = FakeKv::default();
        kv.set(&rec("K", 1, Some(7), 100, None));
        *kv.race.lock().expect("lock") = Some(rec("R", 1, Some(0), 150, None).to_bytes());
        assert_eq!(step_with(&g, &kv, live("R", 1, 0)).await, StepOutcome::CasLost);
        assert_eq!(resets(&s), 0);
        for k in CheckErrorKind::ALL {
            assert_eq!(errors(&s, k.as_label()), 0);
        }
        assert_eq!(step_with(&g, &kv, live("R", 2, 1)).await, StepOutcome::Wrote);
        assert_eq!(resets(&s), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn an_unparsable_value_is_replaced_with_its_revision() {
        let rec_ = DebuggingRecorder::new();
        let s = Tally::new(rec_.snapshotter());
        let _guard = metrics::set_default_local_recorder(&rec_);
        let g = Generation::new();
        let kv = FakeKv::default();
        kv.set_raw(b"not json");
        kv.set_raw(b"not json");
        kv.set_raw(b"not json");
        assert_eq!(kv.revision(), 3);
        assert_eq!(step_with(&g, &kv, live("R", 1, 4)).await, StepOutcome::Wrote);
        assert_eq!(kv.revision(), 4);
        assert_eq!(kv.record().map(|r| r.generation), Some("R".to_owned()));
        assert_eq!(resets(&s), 0);
        for k in CheckErrorKind::ALL {
            assert_eq!(errors(&s, k.as_label()), 0);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_foreign_entry_counts_a_foreign_writer_error_and_writes_nothing() {
        let rec_ = DebuggingRecorder::new();
        let s = Tally::new(rec_.snapshotter());
        let _guard = metrics::set_default_local_recorder(&rec_);
        let g = Generation::new();
        let kv = FakeKv::default();
        assert_eq!(step_with(&g, &kv, live("R", 1, 1)).await, StepOutcome::Wrote);
        kv.set(&rec("K", 9, Some(1), 100, None));
        let rev = kv.revision();
        assert_eq!(step_with(&g, &kv, live("R", 2, 1)).await, StepOutcome::Foreign);
        assert_eq!(kv.revision(), rev);
        assert_eq!(errors(&s, "foreign_writer"), 1);
        assert_eq!(resets(&s), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn a_timeout_with_a_new_kv_violation_is_a_permission_error() {
        let rec_ = DebuggingRecorder::new();
        let s = Tally::new(rec_.snapshotter());
        let _guard = metrics::set_default_local_recorder(&rec_);
        let g = Generation::new();
        let violations = Arc::new(AtomicU64::new(0));
        let mut kv = FakeKv {
            get_error: Some(KvError::Timeout),
            bump_on_get: Some(Arc::clone(&violations)),
            ..FakeKv::default()
        };
        let out = g.step(&kv, || async { Ok(RedisView::NoGroup) }, &violations, T).await;
        assert_eq!(out, StepOutcome::Failed(CheckErrorKind::Permission));
        assert_eq!(errors(&s, "permission"), 1);
        assert_eq!(errors(&s, "timeout"), 0);
        kv.bump_on_get = None;
        let out = g.step(&kv, || async { Ok(RedisView::NoGroup) }, &violations, T).await;
        assert_eq!(out, StepOutcome::Failed(CheckErrorKind::Timeout));
        assert_eq!(errors(&s, "timeout"), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn a_failed_step_forgets_last_own_so_a_restore_still_counts() {
        let rec_ = DebuggingRecorder::new();
        let s = Tally::new(rec_.snapshotter());
        let _guard = metrics::set_default_local_recorder(&rec_);
        let g = Generation::new();
        let mut kv = FakeKv::default();
        assert_eq!(step_with(&g, &kv, live("G0", 1, 3)).await, StepOutcome::Wrote);
        kv.set(&rec("G1", 1, Some(2), 100, None));
        kv.get_error = Some(KvError::Timeout);
        assert_eq!(step_with(&g, &kv, live("G1", 1, 2)).await, StepOutcome::Failed(CheckErrorKind::Timeout));
        kv.get_error = None;
        assert_eq!(step_with(&g, &kv, live("G0", 2, 3)).await, StepOutcome::Counted(LossKind::Reset));
        assert_eq!(resets(&s), 1);
        assert_eq!(errors(&s, "foreign_writer"), 0);
        // Control: with no failed step in between, the same restore is a foreign writer.
        let g2 = Generation::new();
        let kv2 = FakeKv::default();
        assert_eq!(step_with(&g2, &kv2, live("G0", 1, 3)).await, StepOutcome::Wrote);
        kv2.set(&rec("G1", 1, Some(2), 100, None));
        assert_eq!(step_with(&g2, &kv2, live("G0", 2, 3)).await, StepOutcome::Foreign);
        assert_eq!(resets(&s), 1);
        assert_eq!(errors(&s, "foreign_writer"), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn a_redis_error_in_the_detection_is_a_redis_error() {
        let rec_ = DebuggingRecorder::new();
        let s = Tally::new(rec_.snapshotter());
        let _guard = metrics::set_default_local_recorder(&rec_);
        let g = Generation::new();
        let kv = FakeKv::default();
        let out = g.step(&kv, || async { Err(redis::RedisError::from(std::io::Error::other("down"))) }, &AtomicU64::new(0), T).await;
        assert_eq!(out, StepOutcome::Failed(CheckErrorKind::Redis));
        assert_eq!(errors(&s, "redis"), 1);
        assert_eq!(kv.revision(), 0);
    }

    /// T5(1): the claim of an ambiguous counting write survives a step whose KV get fails. The next working step
    /// counts it once.
    #[tokio::test(start_paused = true)]
    async fn a_pending_claim_survives_a_failed_get() {
        let rec_ = DebuggingRecorder::new();
        let s = Tally::new(rec_.snapshotter());
        let _guard = metrics::set_default_local_recorder(&rec_);
        let g = Generation::new();
        let mut kv = FakeKv::default();
        kv.set(&rec("K", 1, Some(7), 100, None));
        // The counting write applies, then returns a timeout: the claim is pending.
        kv.update_then_fail = Some(KvError::Timeout);
        assert_eq!(step_with(&g, &kv, live("R", 1, 0)).await, StepOutcome::Failed(CheckErrorKind::Timeout));
        kv.update_then_fail = None;
        // The next get fails. The claim must stay pending.
        kv.get_error = Some(KvError::Timeout);
        assert_eq!(step_with(&g, &kv, live("R", 2, 1)).await, StepOutcome::Failed(CheckErrorKind::Timeout));
        assert_eq!(resets(&s), 0);
        kv.get_error = None;
        // The recovery step reads the entry with the nonce and counts the loss once.
        assert_eq!(step_with(&g, &kv, live("R", 3, 1)).await, StepOutcome::Wrote);
        assert_eq!(resets(&s), 1);
        assert_eq!(lost(&s), 7);
    }

    /// M4: the outer timeout fires while the Redis detection runs. That is a Redis error, not a KV timeout.
    #[tokio::test(start_paused = true)]
    async fn a_timeout_during_the_detection_is_a_redis_error() {
        let rec_ = DebuggingRecorder::new();
        let s = Tally::new(rec_.snapshotter());
        let _guard = metrics::set_default_local_recorder(&rec_);
        let g = Generation::new();
        let kv = FakeKv::default();
        let out = g.step(&kv, std::future::pending::<Result<RedisView, redis::RedisError>>, &AtomicU64::new(0), T).await;
        assert_eq!(out, StepOutcome::Failed(CheckErrorKind::Redis));
        assert_eq!(errors(&s, "redis"), 1);
        assert_eq!(errors(&s, "timeout"), 0);
        assert_eq!(kv.revision(), 0);
        // Control: a KV get that hangs is still a timeout.
        let hung = FakeKv { get_hang: true, ..FakeKv::default() };
        let out = g.step(&hung, || async { Ok(RedisView::NoGroup) }, &AtomicU64::new(0), T).await;
        assert_eq!(out, StepOutcome::Failed(CheckErrorKind::Timeout));
        assert_eq!(errors(&s, "timeout"), 1);
        assert_eq!(errors(&s, "redis"), 1);
    }

    /// M1: the foreign-writer error log is rate-limited to one per 60 s. The metric counts every tick.
    #[tokio::test(start_paused = true)]
    async fn the_foreign_writer_log_is_rate_limited_and_the_metric_is_not() {
        let rec_ = DebuggingRecorder::new();
        let s = Tally::new(rec_.snapshotter());
        let _guard = metrics::set_default_local_recorder(&rec_);
        let (logs, _log_guard) = paigasus_logging::test_support::capture_logs_at(tracing::Level::ERROR);
        let g = Generation::new();
        let kv = FakeKv::default();
        assert_eq!(step_with(&g, &kv, live("R", 1, 1)).await, StepOutcome::Wrote);
        kv.set(&rec("K", 9, Some(1), 100, None));
        let line = "another writer set";
        for e in 2..5 {
            assert_eq!(step_with(&g, &kv, live("R", e, 1)).await, StepOutcome::Foreign);
        }
        assert_eq!(errors(&s, "foreign_writer"), 3);
        assert_eq!(logs.text().matches(line).count(), 1, "three foreign ticks inside 60 s log once");
        tokio::time::advance(Duration::from_secs(60)).await;
        assert_eq!(step_with(&g, &kv, live("R", 5, 1)).await, StepOutcome::Foreign);
        assert_eq!(errors(&s, "foreign_writer"), 4);
        assert_eq!(logs.text().matches(line).count(), 2, "a foreign tick after 60 s logs again");
    }

    #[tokio::test(start_paused = true)]
    async fn a_success_counts_at_once_and_forgets_the_nonce() {
        let rec_ = DebuggingRecorder::new();
        let s = Tally::new(rec_.snapshotter());
        let _guard = metrics::set_default_local_recorder(&rec_);
        let g = Generation::new();
        let kv = FakeKv::default();
        kv.set(&rec("K", 1, Some(7), 100, None));
        assert_eq!(step_with(&g, &kv, live("R", 1, 0)).await, StepOutcome::Counted(LossKind::Reset));
        assert_eq!(resets(&s), 1);
        assert_eq!(lost(&s), 7);
        assert!(g.lock().pending.is_none());
        // The entry still holds the claim; the next get must not count it again.
        assert_eq!(step_with(&g, &kv, live("R", 2, 0)).await, StepOutcome::Wrote);
        assert_eq!(resets(&s), 1);
        assert_eq!(lost(&s), 7);
    }
}
