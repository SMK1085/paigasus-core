// SPDX-License-Identifier: Apache-2.0

//! The relay task (SMA-755 § 5.7, § 5.8). It reads the Redis stream with a consumer group and publishes the
//! ORIGINAL bytes of each entry to JetStream. It decodes an entry only to read `org_id` and `decision_id`.
//! After the PubAcks it runs `XACK` and `XDEL` in one `MULTI`. It claims the entries of a dead instance with
//! `XAUTOCLAIM`, and prunes old consumers. It does not count on `NOGROUP`: it makes the group again, and a new
//! generation with it. A sampler reads `XLEN` every 15 s, sets the backlog gauge, and runs the generation check
//! (SMA-821): one single-flight KV step with a timeout, which never blocks the relay or the sampler.
//!
//! Logs name error kinds and classes only. They never hold an event, a payload or the URL.

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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

use super::gen_kv::NatsGenKv;
use super::gen_redis;
use super::generation::{Generation, LocalTuple, entry_key, new_id};
use super::jetstream::{MAX_BYTES, PublishClass, STREAM, Violations, api_error_of, classify, max_bytes_for_account, stream_config, subject_for, violation_since};
use super::redis_conn::RedisSlot;
use super::supervised::run_supervised;
use super::{DropReason, count_dropped};

/// Entries in one read, and in one claim page.
const COUNT: usize = 256;
/// Publishes in flight at a time.
const IN_FLIGHT: usize = 32;
/// The most pages of one claim run.
const CLAIM_PAGES: usize = 16;
/// The server error code of a publish that `Nats-Expected-Stream` refused: another stream holds the subject.
const STREAM_NOT_MATCH: u64 = 10060;
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
    /// The time limit of one KV step of the generation check (5 s).
    pub kv_timeout: Duration,
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
            kv_timeout: Duration::from_secs(5),
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
    /// The last publish was AMBIGUOUS (a timeout while a permission violation came and other publishes were in
    /// flight). The entry stays pending and the relay publishes it again with nothing else in flight.
    solo: bool,
}

enum Outcome {
    Published,
    Rejected,
    Transient,
    /// A transient error that says the stream is gone, or JetStream is not ready (R9). The relay keeps the entry
    /// and makes the stream again before the next publish.
    StreamGone,
    /// A `TimedOut` that coincided with a violation while other publishes ran. The entry is kept for a solo publish.
    Ambiguous,
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
    violations: Violations,
    /// The generation check of this relay (SMA-821).
    generation: Generation,
    /// The KV entry of the generation record.
    kv: NatsGenKv,
    /// A KV step runs. The sampler starts a new one only when this is false (single flight).
    kv_in_flight: AtomicBool,
    inner: Mutex<Inner>,
}

/// Delete the consumer only if it holds no pending entry at the moment of the delete. `XINFO CONSUMERS` and
/// `XGROUP DELCONSUMER` are two commands: another relay can claim an entry into the consumer between them, and
/// Redis drops the pending entries of a deleted consumer. One Lua script checks `XPENDING` and deletes in one
/// atomic step. `Ok(true)` when it deleted the consumer.
///
/// # Errors
///
/// The Redis error of the script call.
pub async fn delete_consumer_if_empty(conn: &mut paigasus_redis::RedisHandle, key: &str, group: &str, consumer: &str) -> Result<bool, redis::RedisError> {
    let script = redis::Script::new(
        "local p = redis.call('XPENDING', KEYS[1], ARGV[1], '-', '+', 1, ARGV[2]) \
         if #p > 0 then return 0 end \
         redis.call('XGROUP', 'DELCONSUMER', KEYS[1], ARGV[1], ARGV[2]) \
         return 1",
    );
    let deleted: i64 = script.key(key).arg(group).arg(consumer).invoke_async(conn).await?;
    Ok(deleted == 1)
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

/// Page through `XAUTOCLAIM`. `page` runs one call with the cursor. The ids that Redis reports as deleted count as
/// `redis_deleted`. A failed page does not discard the pages before it: Redis already moved those entries to this
/// consumer, and `XREADGROUP >` never returns them again. So the result holds the claimed entries AND the error.
async fn claim_pages<F, Fut>(mut page: F) -> (Vec<Entry>, Option<redis::RedisError>)
where
    F: FnMut(String) -> Fut,
    Fut: Future<Output = Result<StreamAutoClaimReply, redis::RedisError>>,
{
    let mut out = Vec::new();
    let mut cursor = "0-0".to_owned();
    for _ in 0..CLAIM_PAGES {
        let reply = match page(cursor).await {
            Ok(reply) => reply,
            Err(err) => return (out, Some(err)),
        };
        count_dropped(DropReason::RedisDeleted, reply.deleted_ids.len() as u64);
        for id in reply.claimed {
            let bytes = match id.map.get("e") {
                Some(Value::BulkString(b)) => Some(Bytes::from(b.clone())),
                _ => None,
            };
            out.push(Entry {
                id: id.id,
                bytes,
                done: false,
                solo: false,
            });
        }
        if reply.next_stream_id == "0-0" {
            break;
        }
        cursor = reply.next_stream_id;
    }
    (out, None)
}

fn entries_of(reply: Option<StreamReadReply>) -> Vec<Entry> {
    let mut out = Vec::new();
    for key in reply.into_iter().flat_map(|r| r.keys) {
        for id in key.ids {
            let bytes = match id.map.get("e") {
                Some(Value::BulkString(b)) => Some(Bytes::from(b.clone())),
                _ => None,
            };
            out.push(Entry {
                id: id.id,
                bytes,
                done: false,
                solo: false,
            });
        }
    }
    out
}

impl State {
    async fn backoff(&self, inner: &mut Inner, shutdown: &mut watch::Receiver<()>) {
        let wait = inner.backoff;
        inner.backoff = (inner.backoff * 2).min(self.s.backoff_max);
        nap(wait, shutdown).await;
    }

    /// `NOGROUP`: Redis lost the stream and the group (§ 5.8). The relay makes the group again, and a new
    /// generation with it. It counts nothing: the sampler's generation check counts the loss (SMA-821).
    async fn handle_nogroup(&self, conn: &mut paigasus_redis::RedisHandle, inner: &mut Inner) {
        match gen_redis::ensure_group(conn, &self.s.key, self.s.group, &new_id()).await {
            Ok(_) => inner.group_ready = true,
            Err(err) => {
                inner.group_ready = false;
                tracing::warn!(kind = ?err.kind(), "the relay cannot make the consumer group again");
            }
        }
    }

    /// The `max_bytes` for the stream. The relay reads the account limits with `$JS.API.INFO` and takes 90 % of the
    /// storage limit when that is below [`MAX_BYTES`], and never more than the per-stream limit of the account
    /// ([`max_bytes_for_account`]). A broker refuses a stream above either limit, so a fixed value would stall the
    /// relay on a smaller account. If the query fails, the relay uses `MAX_BYTES` and goes on.
    async fn stream_max_bytes(&self) -> i64 {
        match self.js.query_account().await {
            Ok(account) => max_bytes_for_account(&account),
            Err(err) => {
                tracing::warn!(kind = ?err.kind(), "the relay cannot read the account limits; it uses the default max_bytes");
                MAX_BYTES
            }
        }
    }

    /// `get_or_create_stream` with `stream_config(max_bytes)`. A live stream that differs gets a warning and no
    /// change. The drift check uses the same `max_bytes`, so a stream that `provision-decisions.sh` made for a
    /// smaller account is not a drift.
    async fn ensure_stream(&self) -> bool {
        let want = stream_config(self.stream_max_bytes().await);
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

    /// `XAUTOCLAIM` of the entries that another consumer holds for too long ([`claim_pages`]).
    async fn claim(&self, conn: &paigasus_redis::RedisHandle) -> (Vec<Entry>, Option<redis::RedisError>) {
        let idle = u64::try_from(self.s.claim_idle.as_millis()).unwrap_or(u64::MAX);
        claim_pages(|cursor| {
            let mut conn = conn.clone();
            let cmd = redis::cmd("XAUTOCLAIM")
                .arg(&self.s.key)
                .arg(self.s.group)
                .arg(&self.s.consumer)
                .arg(idle)
                .arg(cursor)
                .arg("COUNT")
                .arg(COUNT)
                .clone();
            async move { cmd.query_async::<StreamAutoClaimReply>(&mut conn).await }
        })
        .await
    }

    /// `XGROUP DELCONSUMER` for each other consumer with no pending entry and an idle time above the limit.
    async fn prune(&self, conn: &mut paigasus_redis::RedisHandle) -> Result<(), redis::RedisError> {
        let reply: StreamInfoConsumersReply = redis::cmd("XINFO").arg("CONSUMERS").arg(&self.s.key).arg(self.s.group).query_async(conn).await?;
        let idle_ms = usize::try_from(self.s.prune_idle.as_millis()).unwrap_or(usize::MAX);
        for c in reply.consumers {
            if c.name != self.s.consumer && c.pending == 0 && c.idle > idle_ms && delete_consumer_if_empty(conn, &self.s.key, self.s.group, &c.name).await? {
                tracing::info!("the relay removed an idle consumer without pending entries");
            }
        }
        Ok(())
    }

    /// `concurrent` is true when other publishes run at the same time. Then a `TimedOut` that coincides with a
    /// violation is AMBIGUOUS: the violation can belong to another publish.
    async fn publish_one(&self, e: &Entry, concurrent: bool) -> Outcome {
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
        // `Nats-Expected-Stream`: if another stream holds the subject (a drifted `GATEWAY_DECISIONS`), the broker
        // refuses the message with 10060 and stores nothing. 10060 is transient, so the event stays in Redis.
        let mut msg = PublishMessage::build().payload(bytes).expected_stream(STREAM);
        if !event.decision_id.is_empty() {
            msg = msg.message_id(&event.decision_id);
        }
        let generation = self.violations.publish.load(Ordering::SeqCst);
        tracing::debug!(concurrent, "the relay publishes an entry");
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
            Err(err) => match classify(err.kind(), api_error_of(&err), violation_since(&self.violations.publish, generation), concurrent) {
                PublishClass::Ambiguous => Outcome::Ambiguous,
                PublishClass::Permanent(class) => {
                    let code = api_error_of(&err).map(|a| a.code);
                    tracing::warn!(class, code, "the relay rejects an event that JetStream cannot take");
                    Outcome::Rejected
                }
                PublishClass::Transient if err.kind() == jetstream::context::PublishErrorKind::StreamNotFound => Outcome::StreamGone,
                PublishClass::Transient if api_error_of(&err).is_some_and(|a| a.code == STREAM_NOT_MATCH) => {
                    tracing::error!(stream = STREAM, "another JetStream stream holds the decision subject; the relay keeps the event in Redis");
                    Outcome::Transient
                }
                PublishClass::Transient => Outcome::Transient,
            },
        }
    }

    /// `XACK` and `XDEL` of the done entries in one `MULTI`. The result is the sum of the `XACK` replies: the
    /// number of entries that were pending in the group.
    async fn ack(&self, conn: &mut paigasus_redis::RedisHandle, ids: &[&str]) -> Result<u64, redis::RedisError> {
        let mut pipe = redis::pipe();
        pipe.atomic();
        for id in ids {
            pipe.cmd("XACK").arg(&self.s.key).arg(self.s.group).arg(*id);
            pipe.cmd("XDEL").arg(&self.s.key).arg(*id).ignore();
        }
        let acks: Vec<u64> = pipe.query_async(conn).await?;
        Ok(acks.iter().sum())
    }

    /// `XACK` and `XDEL` of the done entries of the carry, and drop them from the carry when Redis took the
    /// command. Entries that are not done stay.
    async fn settle(&self, conn: &mut paigasus_redis::RedisHandle, inner: &mut Inner) {
        let ids: Vec<&str> = inner.carry.iter().filter(|e| e.done).map(|e| e.id.as_str()).collect();
        if ids.is_empty() {
            return;
        }
        let acked = match self.ack(conn, &ids).await {
            Ok(n) => {
                // `XACK` returns 0, not `NOGROUP`, for a missing stream or group. Another relay that settled a
                // claimed entry first also gives a lower count. So this is a warning, not a reset.
                if n < ids.len() as u64 {
                    tracing::warn!(
                        acked = n,
                        settled = ids.len(),
                        "the relay acked fewer entries than it settled; another relay settled them, or Redis lost the group"
                    );
                }
                true
            }
            Err(err) if is_code(&err, "NOGROUP") => {
                self.handle_nogroup(conn, inner).await;
                true
            }
            Err(err) => {
                tracing::warn!(kind = ?err.kind(), "the relay cannot ack and delete published entries; it keeps them");
                false
            }
        };
        if acked {
            inner.carry.retain(|e| !e.done);
        }
    }

    async fn iteration(&self, mut shutdown: watch::Receiver<()>) {
        let mut inner = self.inner.lock().await;
        let Some(handle) = self.slot.get() else {
            nap(self.s.idle_sleep, &mut shutdown).await;
            return;
        };
        let mut conn = handle.clone();
        if !inner.group_ready {
            match gen_redis::ensure_group(&mut conn, &self.s.key, self.s.group, &new_id()).await {
                Ok(_) => inner.group_ready = true,
                Err(err) => {
                    tracing::warn!(kind = ?err.kind(), "the relay cannot make the consumer group");
                    self.backoff(&mut inner, &mut shutdown).await;
                    return;
                }
            }
        }
        // While a carried batch waits for NATS, this block does not run: no `XAUTOCLAIM`, no `XREADGROUP`. So the
        // loss of the Redis stream (`NOGROUP`) is found only when the carry is empty. During a NATS outage the
        // carry stays full, so the relay makes the group again only when it can publish again. The generation check
        // of the sampler counts the loss, and it does not wait for the relay.
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
                // Keep the entries of the pages that Redis gave before an error.
                let (entries, failed) = self.claim(&conn).await;
                inner.carry.extend(entries);
                match failed {
                    None => {}
                    Some(err) if is_code(&err, "NOGROUP") => self.handle_nogroup(&mut conn, &mut inner).await,
                    Some(err) => tracing::warn!(kind = ?err.kind(), "the relay cannot claim entries"),
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
        // Entries that a PubAck confirmed (or that the relay rejected) need no stream. Settle them first, so that
        // a JetStream that is down does not keep them in Redis.
        if inner.carry.iter().any(|e| e.done) {
            self.settle(&mut conn, &mut inner).await;
            if inner.carry.is_empty() {
                return;
            }
        }
        if !inner.stream_ready {
            if self.ensure_stream().await {
                inner.stream_ready = true;
            } else {
                self.backoff(&mut inner, &mut shutdown).await;
                return;
            }
        }

        // Publish the entries that are not done. Entries that were AMBIGUOUS go one at a time after the others, with
        // nothing else in flight. The others run up to 32 at a time.
        let (solo, normal): (Vec<Entry>, Vec<Entry>) = std::mem::take(&mut inner.carry).into_iter().partition(|e| e.solo && !e.done);
        let concurrent = normal.iter().filter(|e| !e.done).count() > 1;
        let this = self;
        let mut results: Vec<(Entry, Option<Outcome>)> = futures::stream::iter(normal)
            .map(|e| async move {
                if e.done {
                    return (e, None);
                }
                let outcome = this.publish_one(&e, concurrent).await;
                (e, Some(outcome))
            })
            .buffer_unordered(IN_FLIGHT)
            .collect()
            .await;
        for e in solo {
            let outcome = self.publish_one(&e, false).await;
            results.push((e, Some(outcome)));
        }
        let mut transient = false;
        let mut kept = Vec::new();
        for (mut e, outcome) in results {
            match outcome {
                None => {}
                Some(Outcome::Published) => {
                    counter!(names::GATEWAY_DECISION_LOG_PUBLISHED_TOTAL).increment(1);
                    e.done = true;
                    e.solo = false;
                }
                Some(Outcome::Rejected) => {
                    count_dropped(DropReason::PublishRejected, 1);
                    e.done = true;
                    e.solo = false;
                }
                Some(Outcome::Transient) => {
                    transient = true;
                    e.solo = false;
                }
                Some(Outcome::StreamGone) => {
                    transient = true;
                    e.solo = false;
                    inner.stream_ready = false;
                }
                Some(Outcome::Ambiguous) => e.solo = true,
            }
            kept.push(e);
        }
        inner.carry = kept;
        self.settle(&mut conn, &mut inner).await;
        if transient || !inner.carry.is_empty() {
            self.backoff(&mut inner, &mut shutdown).await;
        } else {
            inner.backoff = self.s.backoff_start;
        }
    }

    /// The gauge, the local record and the single-flight KV step, each tick. The gauge is set first and from a
    /// read-only script, so a KV outage or a full Redis never stops it. The KV step runs in its own task.
    async fn sampler(self: Arc<Self>, mut shutdown: watch::Receiver<()>) {
        let mut step: Option<tokio::task::JoinHandle<()>> = None;
        loop {
            if let Some(handle) = self.slot.get() {
                let mut conn = handle.clone();
                if let Ok(sample) = gen_redis::sample(&mut conn, &self.s.key, self.s.group).await {
                    #[allow(clippy::cast_precision_loss)]
                    gauge!(names::GATEWAY_DECISION_LOG_BACKLOG).set(sample.xlen as f64);
                    if sample.has_group
                        && let (Some(generation), Some(epoch)) = (sample.generation, sample.epoch)
                    {
                        let sampled_at = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX));
                        self.generation.observe(LocalTuple {
                            generation,
                            epoch: Some(epoch),
                            sample: sample.xlen,
                            sampled_at,
                        });
                    }
                }
                if self.kv_in_flight.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
                    let this = Arc::clone(&self);
                    step = Some(tokio::spawn(async move {
                        let _guard = InFlight(Arc::clone(&this));
                        let (key, group) = (this.s.key.clone(), this.s.group);
                        this.generation
                            .step(
                                &this.kv,
                                move || async move { gen_redis::detect(&mut conn, &key, group, &new_id()).await },
                                &this.violations.kv,
                                this.s.kv_timeout,
                            )
                            .await;
                    }));
                }
            }
            tokio::select! {
                () = tokio::time::sleep(self.s.sample_every) => {}
                _ = shutdown.changed() => {
                    if let Some(task) = step {
                        task.abort();
                    }
                    return;
                }
            }
        }
    }
}

/// Clears `kv_in_flight` when the KV step task ends, also when it is aborted.
struct InFlight(Arc<State>);

impl Drop for InFlight {
    fn drop(&mut self) {
        self.0.kv_in_flight.store(false, Ordering::SeqCst);
    }
}

/// Run the relay until the shutdown signal. The batch in flight gets `budget` after the signal. The entries
/// that are not acked stay in Redis for the next instance.
pub async fn run_relay(slot: RedisSlot, js: jetstream::Context, s: RelaySettings, violations: Violations, shutdown: watch::Receiver<()>, budget: Duration) {
    let now = Instant::now();
    let state = Arc::new(State {
        slot,
        inner: Mutex::new(Inner {
            carry: Vec::new(),
            group_ready: false,
            stream_ready: false,
            backoff: s.backoff_start,
            next_claim: now + s.claim_every,
            next_prune: now + s.prune_every,
        }),
        generation: Generation::new(),
        kv: NatsGenKv::new(js.clone(), entry_key(&s.key)),
        kv_in_flight: AtomicBool::new(false),
        js,
        s,
        violations,
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
    let sampler = Arc::clone(&state).sampler(shutdown);
    tokio::join!(relay, sampler);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn page(next: &str, ids: &[&str]) -> StreamAutoClaimReply {
        StreamAutoClaimReply {
            next_stream_id: next.to_owned(),
            claimed: ids
                .iter()
                .map(|id| redis::streams::StreamId {
                    id: (*id).to_owned(),
                    map: HashMap::from([("e".to_owned(), Value::BulkString(b"x".to_vec()))]),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn a_failed_claim_page_keeps_the_entries_of_the_pages_before_it() {
        let mut cursors = Vec::new();
        let (entries, err) = claim_pages(|cursor| {
            cursors.push(cursor.clone());
            async move {
                match cursor.as_str() {
                    "0-0" => Ok(page("5-0", &["1-0", "2-0"])),
                    "5-0" => Ok(page("9-0", &["5-0"])),
                    _ => Err(redis::RedisError::from(std::io::Error::new(std::io::ErrorKind::TimedOut, "the third page timed out"))),
                }
            }
        })
        .await;
        assert_eq!(cursors, ["0-0", "5-0", "9-0"], "the claim follows the cursor");
        let ids: Vec<&str> = entries.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, ["1-0", "2-0", "5-0"], "the entries of the first two pages stay");
        assert!(entries.iter().all(|e| e.bytes.as_deref() == Some(b"x".as_slice()) && !e.done && !e.solo));
        assert!(err.is_some_and(|e| e.is_timeout() || e.kind() == redis::ErrorKind::Io), "the error of the third page is returned");
    }

    #[tokio::test]
    async fn a_claim_stops_at_the_end_cursor_with_no_error() {
        let (entries, err) = claim_pages(|cursor| async move {
            assert_eq!(cursor, "0-0", "one page only");
            Ok(page("0-0", &["1-0"]))
        })
        .await;
        assert_eq!(entries.len(), 1);
        assert!(err.is_none());
    }
}
