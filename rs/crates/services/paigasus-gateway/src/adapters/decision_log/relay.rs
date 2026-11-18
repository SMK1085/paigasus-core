// SPDX-License-Identifier: Apache-2.0

//! The relay task (SMA-755 § 5.7, § 5.8). It reads the Redis stream with a consumer group and publishes the
//! ORIGINAL bytes of each entry to JetStream. It decodes an entry only to read `org_id` and `decision_id`.
//! After the PubAcks it runs `XACK` and `XDEL` in one `MULTI`. It claims the entries of a dead instance with
//! `XAUTOCLAIM`, prunes old consumers, and counts a Redis loss (`NOGROUP`). A sampler reads `XLEN` every 15 s.
//!
//! Logs name error kinds and classes only. They never hold an event, a payload or the URL.

use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use async_nats::jetstream::{self, message::PublishMessage};
use bytes::Bytes;
use futures::StreamExt;
use metrics::{counter, gauge, histogram};
use paigasus_observability::names;
use paigasus_proto::paigasus::gateway::v1 as pb;
use prost::Message;
use redis::Value;
use redis::streams::{StreamAutoClaimReply, StreamInfoConsumersReply, StreamReadReply};
use tokio::sync::{Mutex, watch};
use tokio::time::Instant;

use super::jetstream::{PublishClass, classify, stream_config, subject_for, violation_since};
use super::redis_conn::RedisSlot;
use super::supervised::run_supervised;
use super::{DropReason, count_dropped};

/// Entries in one read, and in one claim page.
const COUNT: usize = 256;
/// Publishes in flight at a time.
const IN_FLIGHT: usize = 32;
/// The most pages of one claim run.
const CLAIM_PAGES: usize = 16;

/// What the relay needs to know about the stream and about its own timing. `new` gives the production values.
#[derive(Debug, Clone)]
pub struct RelaySettings {
    /// `<prefix>:decisions`.
    pub key: String,
    /// The consumer group, `relay`.
    pub group: &'static str,
    /// The consumer name: the instance id.
    pub consumer: String,
    /// An entry that a consumer holds for this long without an ack can be claimed (60 s).
    pub claim_idle: Duration,
    /// How often the relay claims (30 s).
    pub claim_every: Duration,
    /// How often the relay prunes consumers (10 min).
    pub prune_every: Duration,
    /// A consumer with no pending entry and this idle time is deleted (1 h).
    pub prune_idle: Duration,
    /// How often the sampler reads `XLEN` (15 s, P-19).
    pub sample_every: Duration,
    /// The pause when a read returns no entry (200 ms).
    pub idle_sleep: Duration,
    /// The first backoff after an error (1 s).
    pub backoff_start: Duration,
    /// The largest backoff (30 s).
    pub backoff_max: Duration,
}

impl RelaySettings {
    #[must_use]
    pub fn new(key: String, consumer: String) -> Self {
        RelaySettings {
            key,
            group: "relay",
            consumer,
            claim_idle: Duration::from_secs(60),
            claim_every: Duration::from_secs(30),
            prune_every: Duration::from_secs(600),
            prune_idle: Duration::from_secs(3600),
            sample_every: Duration::from_secs(15),
            idle_sleep: Duration::from_millis(200),
            backoff_start: Duration::from_secs(1),
            backoff_max: Duration::from_secs(30),
        }
    }
}

/// A stream entry that the relay holds.
#[derive(Debug)]
struct Entry {
    id: String,
    /// The bytes of field `e`. `None` when the entry has no such field.
    bytes: Option<Bytes>,
    /// The entry is published or rejected, and counted. It waits only for its `XACK` and `XDEL`.
    done: bool,
}

enum Outcome {
    Published,
    Rejected,
    Transient,
    /// A transient error that says the stream is gone, or JetStream is not ready (R9). The relay keeps the entry
    /// and makes the stream again before the next publish.
    StreamGone,
}

struct Inner {
    carry: Vec<Entry>,
    group_ready: bool,
    stream_ready: bool,
    backoff: Duration,
    next_claim: Instant,
    next_prune: Instant,
}

struct State {
    slot: RedisSlot,
    js: jetstream::Context,
    s: RelaySettings,
    violations: Arc<AtomicU64>,
    /// The last `XLEN` sample. `None` until the first one.
    sample: StdMutex<Option<u64>>,
    inner: Mutex<Inner>,
}

fn is_code(err: &redis::RedisError, code: &str) -> bool {
    err.code() == Some(code)
}

async fn nap(d: Duration, shutdown: &mut watch::Receiver<()>) {
    tokio::select! {
        () = tokio::time::sleep(d) => {}
        _ = shutdown.changed() => {}
    }
}

fn entries_of(reply: Option<StreamReadReply>) -> Vec<Entry> {
    let mut out = Vec::new();
    for key in reply.into_iter().flat_map(|r| r.keys) {
        for id in key.ids {
            let bytes = match id.map.get("e") {
                Some(Value::BulkString(b)) => Some(Bytes::from(b.clone())),
                _ => None,
            };
            out.push(Entry { id: id.id, bytes, done: false });
        }
    }
    out
}

impl State {
    fn last_sample(&self) -> Option<u64> {
        *self.sample.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn set_sample(&self, v: Option<u64>) {
        *self.sample.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = v;
    }

    async fn backoff(&self, inner: &mut Inner, shutdown: &mut watch::Receiver<()>) {
        let wait = inner.backoff;
        inner.backoff = (inner.backoff * 2).min(self.s.backoff_max);
        nap(wait, shutdown).await;
    }

    /// `XGROUP CREATE <key> <group> 0 MKSTREAM`. `Ok(true)` when this call made the group, `Ok(false)` on `BUSYGROUP`.
    async fn create_group(&self, conn: &mut paigasus_redis::RedisHandle) -> Result<bool, redis::RedisError> {
        match redis::cmd("XGROUP")
            .arg("CREATE")
            .arg(&self.s.key)
            .arg(self.s.group)
            .arg("0")
            .arg("MKSTREAM")
            .query_async::<()>(conn)
            .await
        {
            Ok(()) => Ok(true),
            Err(err) if is_code(&err, "BUSYGROUP") => Ok(false),
            Err(err) => Err(err),
        }
    }

    /// `NOGROUP`: Redis lost the stream and the group (§ 5.8). Only the relay whose `XGROUP CREATE` answers `OK`
    /// counts the incident.
    async fn handle_nogroup(&self, conn: &mut paigasus_redis::RedisHandle, inner: &mut Inner) {
        match self.create_group(conn).await {
            Ok(true) => {
                inner.group_ready = true;
                counter!(names::GATEWAY_DECISION_LOG_REDIS_RESETS_TOTAL).increment(1);
                match self.last_sample() {
                    Some(n) => {
                        counter!(names::GATEWAY_DECISION_LOG_LOST_ESTIMATE_TOTAL).increment(n);
                        tracing::error!(lost_estimate_upper_bound = n, "Redis lost the decision stream; the lost events are an estimate (an upper bound)");
                    }
                    None => tracing::error!("Redis lost the decision stream; the number of lost events is unknown (no XLEN sample)"),
                }
                // The new stream is empty.
                self.set_sample(Some(0));
            }
            Ok(false) => inner.group_ready = true,
            Err(err) => {
                inner.group_ready = false;
                tracing::warn!(kind = ?err.kind(), "the relay cannot make the consumer group again");
            }
        }
    }

    /// `get_or_create_stream` with `stream_config()`. A live stream that differs gets a warning and no change.
    async fn ensure_stream(&self) -> bool {
        let want = stream_config();
        match self.js.get_or_create_stream(want.clone()).await {
            Ok(stream) => {
                let have = &stream.cached_info().config;
                let mut drift = Vec::new();
                if have.subjects != want.subjects {
                    drift.push("subjects");
                }
                if have.retention != want.retention {
                    drift.push("retention");
                }
                if have.storage != want.storage {
                    drift.push("storage");
                }
                if have.max_age != want.max_age {
                    drift.push("max_age");
                }
                if have.max_bytes != want.max_bytes {
                    drift.push("max_bytes");
                }
                if have.discard != want.discard {
                    drift.push("discard");
                }
                if have.duplicate_window != want.duplicate_window {
                    drift.push("duplicate_window");
                }
                if have.max_message_size != want.max_message_size {
                    drift.push("max_message_size");
                }
                if have.num_replicas != want.num_replicas {
                    drift.push("num_replicas");
                }
                if !drift.is_empty() {
                    tracing::warn!(stream = super::jetstream::STREAM, drift = ?drift, "the decision stream differs from the expected config; the relay does not change it");
                }
                true
            }
            Err(err) => {
                tracing::warn!(kind = ?err.kind(), "the relay cannot get or create the decision stream");
                false
            }
        }
    }

    /// `XAUTOCLAIM` of the entries that another consumer holds for too long. The ids that Redis reports as
    /// deleted count as `redis_deleted`.
    async fn claim(&self, conn: &mut paigasus_redis::RedisHandle) -> Result<Vec<Entry>, redis::RedisError> {
        let mut out = Vec::new();
        let mut cursor = "0-0".to_owned();
        for _ in 0..CLAIM_PAGES {
            let reply: StreamAutoClaimReply = redis::cmd("XAUTOCLAIM")
                .arg(&self.s.key)
                .arg(self.s.group)
                .arg(&self.s.consumer)
                .arg(u64::try_from(self.s.claim_idle.as_millis()).unwrap_or(u64::MAX))
                .arg(&cursor)
                .arg("COUNT")
                .arg(COUNT)
                .query_async(conn)
                .await?;
            count_dropped(DropReason::RedisDeleted, reply.deleted_ids.len() as u64);
            for id in reply.claimed {
                let bytes = match id.map.get("e") {
                    Some(Value::BulkString(b)) => Some(Bytes::from(b.clone())),
                    _ => None,
                };
                out.push(Entry { id: id.id, bytes, done: false });
            }
            if reply.next_stream_id == "0-0" {
                break;
            }
            cursor = reply.next_stream_id;
        }
        Ok(out)
    }

    /// `XGROUP DELCONSUMER` for each other consumer with no pending entry and an idle time above the limit.
    async fn prune(&self, conn: &mut paigasus_redis::RedisHandle) -> Result<(), redis::RedisError> {
        let reply: StreamInfoConsumersReply = redis::cmd("XINFO").arg("CONSUMERS").arg(&self.s.key).arg(self.s.group).query_async(conn).await?;
        let idle_ms = usize::try_from(self.s.prune_idle.as_millis()).unwrap_or(usize::MAX);
        for c in reply.consumers {
            if c.name != self.s.consumer && c.pending == 0 && c.idle > idle_ms {
                redis::cmd("XGROUP").arg("DELCONSUMER").arg(&self.s.key).arg(self.s.group).arg(&c.name).query_async::<i64>(conn).await?;
                tracing::info!("the relay removed an idle consumer without pending entries");
            }
        }
        Ok(())
    }

    async fn publish_one(&self, e: &Entry) -> Outcome {
        let Some(bytes) = e.bytes.clone() else {
            return Outcome::Rejected;
        };
        // The event is decoded only to read the two fields. The ORIGINAL bytes go out.
        let Ok(event) = pb::ToolDecisionEvent::decode(bytes.clone()) else {
            return Outcome::Rejected;
        };
        let Some(subject) = subject_for(&event.org_id) else {
            return Outcome::Rejected;
        };
        let mut msg = PublishMessage::build().payload(bytes);
        if !event.decision_id.is_empty() {
            msg = msg.message_id(&event.decision_id);
        }
        let generation = self.violations.load(Ordering::SeqCst);
        let started = Instant::now();
        let result = match self.js.send_publish(subject, msg).await {
            Ok(ack) => ack.await.map(|_| ()),
            Err(err) => Err(err),
        };
        match result {
            Ok(()) => {
                histogram!(names::GATEWAY_DECISION_LOG_PUBLISH_SECONDS).record(started.elapsed().as_secs_f64());
                Outcome::Published
            }
            Err(err) => match classify(err.kind(), violation_since(&self.violations, generation)) {
                PublishClass::Permanent(class) => {
                    tracing::warn!(class, "the relay rejects an event that JetStream cannot take");
                    Outcome::Rejected
                }
                PublishClass::Transient if err.kind() == jetstream::context::PublishErrorKind::StreamNotFound => Outcome::StreamGone,
                PublishClass::Transient => Outcome::Transient,
            },
        }
    }

    /// `XACK` and `XDEL` of the done entries in one `MULTI`.
    async fn ack(&self, conn: &mut paigasus_redis::RedisHandle, ids: &[&str]) -> Result<(), redis::RedisError> {
        let mut pipe = redis::pipe();
        pipe.atomic();
        for id in ids {
            pipe.cmd("XACK").arg(&self.s.key).arg(self.s.group).arg(*id).ignore();
            pipe.cmd("XDEL").arg(&self.s.key).arg(*id).ignore();
        }
        pipe.query_async::<()>(conn).await
    }

    async fn iteration(&self, mut shutdown: watch::Receiver<()>) {
        let mut inner = self.inner.lock().await;
        let Some(handle) = self.slot.get() else {
            nap(self.s.idle_sleep, &mut shutdown).await;
            return;
        };
        let mut conn = handle.clone();
        if !inner.group_ready {
            match self.create_group(&mut conn).await {
                Ok(_) => inner.group_ready = true,
                Err(err) => {
                    tracing::warn!(kind = ?err.kind(), "the relay cannot make the consumer group");
                    self.backoff(&mut inner, &mut shutdown).await;
                    return;
                }
            }
        }
        if inner.carry.is_empty() {
            let now = Instant::now();
            if now >= inner.next_prune {
                inner.next_prune = now + self.s.prune_every;
                if let Err(err) = self.prune(&mut conn).await {
                    tracing::warn!(kind = ?err.kind(), "the relay cannot prune consumers");
                }
            }
            if now >= inner.next_claim {
                inner.next_claim = now + self.s.claim_every;
                match self.claim(&mut conn).await {
                    Ok(entries) => inner.carry.extend(entries),
                    Err(err) if is_code(&err, "NOGROUP") => self.handle_nogroup(&mut conn, &mut inner).await,
                    Err(err) => tracing::warn!(kind = ?err.kind(), "the relay cannot claim entries"),
                }
            }
        }
        if inner.carry.is_empty() {
            let read = redis::cmd("XREADGROUP")
                .arg("GROUP")
                .arg(self.s.group)
                .arg(&self.s.consumer)
                .arg("COUNT")
                .arg(COUNT)
                .arg("STREAMS")
                .arg(&self.s.key)
                .arg(">")
                .query_async::<Option<StreamReadReply>>(&mut conn)
                .await;
            match read {
                Ok(reply) => inner.carry = entries_of(reply),
                Err(err) if is_code(&err, "NOGROUP") => {
                    self.handle_nogroup(&mut conn, &mut inner).await;
                    return;
                }
                Err(err) => {
                    tracing::warn!(kind = ?err.kind(), "the relay cannot read the decision stream");
                    self.backoff(&mut inner, &mut shutdown).await;
                    return;
                }
            }
        }
        if inner.carry.is_empty() {
            nap(self.s.idle_sleep, &mut shutdown).await;
            return;
        }
        if !inner.stream_ready {
            if self.ensure_stream().await {
                inner.stream_ready = true;
            } else {
                self.backoff(&mut inner, &mut shutdown).await;
                return;
            }
        }

        // Publish the entries that are not done, up to 32 at a time.
        let batch = std::mem::take(&mut inner.carry);
        let this = self;
        let results: Vec<(Entry, Option<Outcome>)> = futures::stream::iter(batch)
            .map(|e| async move {
                if e.done {
                    return (e, None);
                }
                let outcome = this.publish_one(&e).await;
                (e, Some(outcome))
            })
            .buffer_unordered(IN_FLIGHT)
            .collect()
            .await;
        let mut transient = false;
        let mut kept = Vec::new();
        for (mut e, outcome) in results {
            match outcome {
                None => {}
                Some(Outcome::Published) => {
                    counter!(names::GATEWAY_DECISION_LOG_PUBLISHED_TOTAL).increment(1);
                    e.done = true;
                }
                Some(Outcome::Rejected) => {
                    count_dropped(DropReason::PublishRejected, 1);
                    e.done = true;
                }
                Some(Outcome::Transient) => transient = true,
                Some(Outcome::StreamGone) => {
                    transient = true;
                    inner.stream_ready = false;
                }
            }
            kept.push(e);
        }
        let ids: Vec<&str> = kept.iter().filter(|e| e.done).map(|e| e.id.as_str()).collect();
        let mut acked = false;
        if !ids.is_empty() {
            match self.ack(&mut conn, &ids).await {
                Ok(()) => acked = true,
                Err(err) if is_code(&err, "NOGROUP") => {
                    acked = true;
                    self.handle_nogroup(&mut conn, &mut inner).await;
                }
                Err(err) => tracing::warn!(kind = ?err.kind(), "the relay cannot ack and delete published entries; it keeps them"),
            }
        }
        inner.carry = if acked { kept.into_iter().filter(|e| !e.done).collect() } else { kept };
        if transient || !inner.carry.is_empty() {
            self.backoff(&mut inner, &mut shutdown).await;
        } else {
            inner.backoff = self.s.backoff_start;
        }
    }

    async fn sampler(&self, mut shutdown: watch::Receiver<()>) {
        loop {
            if let Some(handle) = self.slot.get() {
                let mut conn = handle.clone();
                if let Ok(n) = redis::cmd("XLEN").arg(&self.s.key).query_async::<u64>(&mut conn).await {
                    self.set_sample(Some(n));
                    #[allow(clippy::cast_precision_loss)]
                    gauge!(names::GATEWAY_DECISION_LOG_BACKLOG).set(n as f64);
                }
            }
            tokio::select! {
                () = tokio::time::sleep(self.s.sample_every) => {}
                _ = shutdown.changed() => return,
            }
        }
    }
}

/// Run the relay until the shutdown signal. The batch in flight gets `budget` after the signal. The entries
/// that are not acked stay in Redis for the next instance.
pub async fn run_relay(slot: RedisSlot, js: jetstream::Context, s: RelaySettings, violations: Arc<AtomicU64>, shutdown: watch::Receiver<()>, budget: Duration) {
    let now = Instant::now();
    let state = Arc::new(State {
        slot,
        js,
        inner: Mutex::new(Inner {
            carry: Vec::new(),
            group_ready: false,
            stream_ready: false,
            backoff: s.backoff_start,
            next_claim: now + s.claim_every,
            next_prune: now + s.prune_every,
        }),
        s,
        violations,
        sample: StdMutex::new(None),
    });
    let relay_state = Arc::clone(&state);
    let relay_shutdown = shutdown.clone();
    let relay = run_supervised("relay", shutdown.clone(), move || {
        let state = Arc::clone(&relay_state);
        let sd = relay_shutdown.clone();
        async move {
            let work = state.iteration(sd.clone());
            tokio::pin!(work);
            let mut stop = sd;
            tokio::select! {
                () = &mut work => {}
                () = async {
                    let _ = stop.changed().await;
                    tokio::time::sleep(budget).await;
                } => {}
            }
        }
    });
    let sampler = state.sampler(shutdown);
    tokio::join!(relay, sampler);
}
