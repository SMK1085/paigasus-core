// SPDX-License-Identifier: Apache-2.0

//! `NatsEventPublisher`: the production [`EventPublisher`] (SMA-471, ADR-0016) — the outbox
//! relay's real delivery sink, replacing `TracingEventPublisher` wherever a broker is configured.
//!
//! **Ack-waiting is mandatory, not an optimization.** `send_publish` returns a future; awaiting
//! *that* is what waits for JetStream to persist the message. The relay stamps `published_at`
//! and never revisits a row on `Ok(())`, so returning `Ok` before the ack would silently lose
//! events — strictly worse than the `tracing` publisher this replaces.
//!
//! **What `Nats-Msg-Id` does and does not cover.** Every publish carries the outbox row's id, so
//! JetStream drops a redelivery within the stream's `duplicate_window` and acks it as a
//! duplicate — which this adapter treats as success. That covers the common case: a lost ack
//! retried on a later tick. It does NOT cover a tick that published and then failed to commit
//! (the relay does the whole batch on one transaction), a crash-restart beyond the window, or an
//! operator dead-letter replay hours later. The contract is therefore **at-least-once with a
//! best-effort dedup window; consumers must be idempotent** — see the spec's D3 and ADR-0016.
//!
//! The window itself IS durable within its span: `file`-storage JetStream rebuilds its dedup map
//! from the stored messages' `Nats-Msg-Id` headers after a broker restart, which
//! `tests/nats_publisher.rs::dedup_survives_a_broker_restart` asserts against a real broker.
//!
//! **Both the credential and the CA bundle are re-read from disk on every connection attempt**
//! (SMA-493 D7/D8), not just the first — this is why `connect` installs an auth *callback*
//! (`creds::auth_from_credentials`) instead of `ConnectOptions::with_credentials_file`, which
//! reads once and caches. `add_root_certificates` stores only the path at options-build time;
//! the file itself is loaded fresh inside `try_connect_to_server`'s TLS setup on each attempt
//! (`tls.rs::config_tls`, called from `connector.rs:501`/`544`), same as the auth callback is
//! invoked fresh each attempt (`connector.rs:681`). A rotated `.creds` or CA bundle therefore
//! takes effect on the next reconnect, with no process restart. **`event_callback` is what makes
//! a NATS permissions violation visible at all**: a denied publish or subscribe comes back from
//! the broker as an asynchronous `-ERR 'Permissions Violation …'` on the connection, not as an
//! error on the request that triggered it — without a callback logging `Event::ServerError` /
//! `Event::ClientError`, a subject-permission misconfiguration (D9) is indistinguishable from a
//! broker that is merely slow to ack.

use std::future::Future;
use std::time::Duration;

use async_nats::jetstream::context::{CreateStreamError, PublishError as JetStreamPublishError};
use async_nats::jetstream::message::PublishMessage;
use async_nats::jetstream::{self, publish::PublishAck, stream::StorageType};
use async_trait::async_trait;
use metrics::{counter, gauge, histogram};
use paigasus_iam_core::{DomainEvent, EventPublisher, PublishError};
use paigasus_observability::names;
use tokio::task::JoinHandle;

use crate::adapters::events::cloud_event::{CloudEvent, render_id};
use crate::config::{PublisherConfig, RedactedUrl};

/// The stream's subject filter. Every `EventType` wire string is `iam.`-prefixed
/// (`domain_event.rs`), so one wildcard covers them all.
const SUBJECT_FILTER: &str = "iam.>";

/// Structured-mode CloudEvents content type (SMA-471 D6).
const CONTENT_TYPE: &str = "application/cloudevents+json; charset=utf-8";

/// Consecutive publish failures that open the breaker. Three rather than one, mirroring
/// `redis_conn.rs`'s `FAILURE_THRESHOLD`: a single blip during a reconnect must not disable the
/// sink for a whole window.
const FAILURE_THRESHOLD: u32 = 3;

/// How long an open breaker short-circuits before admitting one probe.
const OPEN_DURATION: Duration = Duration::from_secs(2);

/// A deliberately minimal consecutive-failure breaker (SMA-471 D11).
///
/// **Why this exists at all**: `OutboxRelay::tick` publishes the whole batch inside ONE
/// transaction holding `FOR UPDATE` locks. At `batch_size = 100` and a 2 s ack timeout, an
/// unbroken adapter against a blackholed broker holds 100 row locks for ~200 s, blocks
/// autovacuum, and makes SIGTERM take just as long — past a normal grace period, so the
/// orchestrator SIGKILLs mid-tick and the batch rolls back. With the breaker a bad tick costs
/// `FAILURE_THRESHOLD × publish_timeout_secs` instead.
///
/// Far simpler than `redis_conn.rs`'s `Breaker`: no half-open permit, no epoch, no metrics
/// role label. Those exist there because the Redis breaker guards eleven concurrent call sites
/// on the authz hot path; this one guards a single serial background loop, where a probe that
/// is admitted and then fails simply re-opens the window on the next `on_failure`.
#[derive(Debug)]
struct Breaker {
    open_duration: Duration,
    inner: std::sync::Mutex<BreakerInner>,
}

#[derive(Debug)]
struct BreakerInner {
    consecutive_failures: u32,
    opened_at: Option<std::time::Instant>,
}

impl Breaker {
    fn with_durations(open_duration: Duration) -> Breaker {
        Breaker {
            open_duration,
            inner: std::sync::Mutex::new(BreakerInner {
                consecutive_failures: 0,
                opened_at: None,
            }),
        }
    }

    /// `true` = go ahead and dial. An open breaker admits exactly one probe per window: the
    /// `opened_at` reset means the next caller short-circuits again until this probe reports.
    fn admit(&self) -> bool {
        let mut inner = self.inner.lock().expect("breaker mutex poisoned");
        match inner.opened_at {
            None => true,
            Some(at) if at.elapsed() >= self.open_duration => {
                inner.opened_at = Some(std::time::Instant::now());
                true
            }
            Some(_) => false,
        }
    }

    fn on_success(&self) {
        let mut inner = self.inner.lock().expect("breaker mutex poisoned");
        inner.consecutive_failures = 0;
        inner.opened_at = None;
    }

    fn on_failure(&self) {
        let mut inner = self.inner.lock().expect("breaker mutex poisoned");
        inner.consecutive_failures = inner.consecutive_failures.saturating_add(1);
        if inner.consecutive_failures >= FAILURE_THRESHOLD {
            inner.opened_at = Some(std::time::Instant::now());
        }
    }
}

/// Why a publish (or the connect that precedes it) failed.
///
/// Every fallible variant keeps its cause as a `source()` rather than folding it into the
/// message: `relay.rs::describe_error` walks that chain into `event_outbox.last_error`, and
/// that string is the whole of what an operator sees on a parked row.
///
/// **Reading that output:** every `async_nats` error is an `async_nats::error::Error<Kind>`,
/// which puts its cause in BOTH its own `Display` (`"{kind}: {source}"`) and its `source()`.
/// `describe_error` walks the chain, so when a NATS error carries an inner cause the innermost
/// text appears twice — `"backend error: nats connect failed: IO error: connection refused:
/// connection refused"`. That is upstream's rendering, not a bug here, and it is left alone:
/// dropping `#[source]` to de-duplicate it would truncate the chain instead, which costs an
/// operator far more than the repetition does.
#[derive(Debug, thiserror::Error)]
pub enum NatsPublisherError {
    /// The configured `credentials_file` could not be read. Split out from [`Self::Connect`] so
    /// a missing/unreadable file surfaces a typed boot error naming the path — a bare
    /// `io::Error` alone ("No such file or directory") is not an actionable boot error without
    /// it. A file that reads fine but is not a valid credential is [`Self::CredentialsParse`],
    /// not this variant.
    #[error("nats credentials file {path} could not be loaded")]
    Credentials {
        path: String,
        #[source]
        source: std::io::Error,
    },
    /// The `credentials_file` was read but is not a NATS credential. Split from
    /// [`Self::Credentials`] because "the file is missing" and "the file is not what you think it
    /// is" have different remediations, and an `io::Error` for a file that plainly exists reads
    /// as a filesystem problem.
    #[error("nats credentials file {path} could not be parsed")]
    CredentialsParse {
        path: String,
        #[source]
        source: crate::adapters::events::creds::CredsError,
    },
    #[error("nats connect failed")]
    Connect(#[source] async_nats::ConnectError),
    #[error("jetstream stream {stream} could not be ensured")]
    Ensure {
        stream: String,
        #[source]
        source: CreateStreamError,
    },
    #[error("stream {stream} has {field} = {got}, but this service requires {want}")]
    StreamConfigDrift { stream: String, field: &'static str, want: String, got: String },
    #[error("event payload could not be serialized")]
    Serialize(#[source] serde_json::Error),
    #[error("jetstream publish failed")]
    Publish(#[source] JetStreamPublishError),
    /// The connection-state short-circuit (SMA-471 Task 4 / D11): the client already knows its
    /// connection is down, or the [`Breaker`] is open, so publishing is refused up front instead
    /// of burning a full `publish_timeout_secs` per outbox row in the batch.
    #[error("nats connection is down")]
    Disconnected,
}

/// The production outbox sink. The underlying `Context` holds a `Client`, which multiplexes one
/// TCP connection and reconnects in the background, so no pooling is needed.
///
/// The client itself is deliberately NOT a separate field: [`jetstream::Context::client`] hands
/// back a (cheap) clone of it, which is where the connection-state short-circuit (Task 4) gets
/// its `Client::connection_state()` — see `publish_ack`.
///
/// `Debug` is derived rather than redacted: neither field carries the (possibly
/// credential-bearing) broker URL — `PublisherConfig` keeps that redacted at the config layer,
/// and `Context`'s own `Debug` renders connection handles, not the address they were dialled
/// from.
#[derive(Debug)]
pub struct NatsEventPublisher {
    jetstream: jetstream::Context,
    source: String,
    breaker: Breaker,
}

impl NatsEventPublisher {
    /// Connects, ensures the stream, and **verifies the live stream's config** (SMA-471 D7).
    ///
    /// `get_or_create_stream` creates or fetches; it does NOT reconcile an existing stream's
    /// config. That non-reconciliation is deliberate — this service must never silently reshape
    /// a stream external consumers depend on — but adoption is conditional: a stream whose
    /// `duplicate_window` is shorter than configured, whose `retention` is not `Limits`, whose
    /// storage is not `File`, or whose subjects do not cover `iam.>`, fails boot rather than
    /// being adopted.
    ///
    /// # Errors
    ///
    /// [`NatsPublisherError::Credentials`] when the credentials file cannot be read,
    /// [`NatsPublisherError::CredentialsParse`] when it can be read but is not a valid NATS
    /// credential, [`NatsPublisherError::Connect`] when the broker cannot be reached or
    /// authenticated against, [`NatsPublisherError::Ensure`] when the stream can neither be
    /// fetched nor created, and [`NatsPublisherError::StreamConfigDrift`] when an existing
    /// stream is weaker than this service requires.
    ///
    /// # Panics
    ///
    /// If `cfg.url` is `None`. `IamConfig::validate` rejects that combination at load time, so
    /// reaching this is a programming error, not an operator one.
    pub async fn connect(cfg: &PublisherConfig) -> Result<NatsEventPublisher, NatsPublisherError> {
        let url = cfg.url.as_ref().map(RedactedUrl::as_str).expect("validate() guarantees url is Some for the nats backend");

        // D8: read and parse the credential EAGERLY, before any connection machinery exists, so a
        // missing or malformed file is a typed boot error naming the path — then install the
        // callback that re-reads it on every subsequent attempt. `with_auth_callback` is a
        // CONSTRUCTOR (`options.rs:204`), so it has to start the chain rather than join it.
        let mut opts = match &cfg.credentials_file {
            Some(path) => {
                let raw = tokio::fs::read_to_string(path).await.map_err(|source| NatsPublisherError::Credentials { path: path.clone(), source })?;
                crate::adapters::events::creds::parse_credentials(&raw).map_err(|source| NatsPublisherError::CredentialsParse { path: path.clone(), source })?;

                let path = path.clone();
                async_nats::ConnectOptions::with_auth_callback(move |nonce| {
                    let path = path.clone();
                    // Nothing non-`Sync` is held across the await inside: the callback's future
                    // must be `Send + Sync + 'static` (`options.rs:207`).
                    async move { crate::adapters::events::creds::auth_from_credentials(&path, &nonce).await }
                })
            }
            None => async_nats::ConnectOptions::new(),
        };

        // D4: the client's inbox prefix must match the account's `subscribe` grant. A mismatch is
        // not an error anywhere — it presents as every publish timing out on an ack the broker
        // refuses to deliver — which is why the event callback below matters so much.
        if let Some(prefix) = &cfg.inbox_prefix {
            opts = opts.custom_inbox_prefix(prefix.clone());
        } else if cfg.credentials_file.is_some() {
            // Belt-and-braces (SMA-493 review): a credentialed deployment on a least-privilege
            // account almost always grants `subscribe` on a per-user inbox prefix, not the
            // async-nats default `_INBOX`. Not a `validate()` error — an account that grants
            // `sub _INBOX.>` is a legitimate (if wider) deployment shape — but the failure mode
            // when it's wrong is a silent hang (every publish times out waiting for an ack the
            // broker refuses to deliver), so warn instead of staying silent.
            tracing::warn!(
                "outbox.publisher.credentials_file is set but outbox.publisher.inbox_prefix is not — using the default `_INBOX` prefix, which will make every publish time out if the account grants only a per-user prefix (see ops/nats/permissions.md §7)"
            );
        }
        // D7: REPLACES the system trust store (see the field's doc). Re-read per attempt.
        if let Some(bundle) = &cfg.root_ca_bundle {
            opts = opts.add_root_certificates(std::path::PathBuf::from(bundle));
        }
        // D9: a denied publish is answered with an ASYNCHRONOUS `-ERR 'Permissions Violation …'`
        // and the request itself simply times out. Without this callback the single most likely
        // misconfiguration in a permissioned deployment is indistinguishable from a slow broker.
        opts = opts.event_callback(|event| async move {
            match event {
                async_nats::Event::ServerError(ref e) => tracing::error!(event = %event, "nats server error: {e}"),
                async_nats::Event::ClientError(ref e) => tracing::error!(event = %event, "nats client error: {e}"),
                async_nats::Event::Disconnected | async_nats::Event::LameDuckMode => tracing::warn!(event = %event, "nats connection event"),
                _ => tracing::info!(event = %event, "nats connection event"),
            }
        });

        let client = opts.connect(url).await.map_err(NatsPublisherError::Connect)?;

        let mut js = jetstream::new(client);
        // Covers the API request AND the ack wait — `send_publish` stamps this same value onto
        // the `PublishAckFuture` it returns. A `tokio::time::timeout` around only the ack await
        // would leave the request leg unbounded (SMA-471 D11).
        js.set_timeout(Duration::from_secs(cfg.publish_timeout_secs));

        let want_window = Duration::from_secs(cfg.duplicate_window_secs);
        let stream = js
            .get_or_create_stream(jetstream::stream::Config {
                name: cfg.stream.clone(),
                subjects: vec![SUBJECT_FILTER.to_string()],
                retention: jetstream::stream::RetentionPolicy::Limits,
                storage: StorageType::File,
                duplicate_window: want_window,
                max_age: Duration::from_secs(cfg.max_age_secs),
                num_replicas: 1,
                ..Default::default()
            })
            .await
            .map_err(|source| NatsPublisherError::Ensure { stream: cfg.stream.clone(), source })?;

        let info = stream.cached_info();
        verify_stream(&cfg.stream, &info.config, want_window)?;

        // Primed HERE, not in `describe_iam_metrics`: that runs only when `metrics.enabled`, and
        // a metrics-rs counter first appears at the value of its first increment — an unprimed
        // counter can never satisfy an `increase() > 0` alert on the FIRST duplicate. Same
        // constructor-priming pattern as `redis_conn::Breaker::with_durations`.
        counter!(names::IAM_NATS_PUBLISH_DUPLICATES_TOTAL).increment(0);

        // The `iam_nats_connected` gauge sampler is NOT started here (SMA-471 review fix — it
        // used to be a bare detached `tokio::spawn` at this exact point). Starting it inside
        // `connect` would mean every caller — including the several unit/integration tests that
        // call `connect` and never intend to run anything past their own scope — gets an orphan
        // background task with no shutdown future and no `JoinHandle`, which is exactly the bug
        // this fix removes. See [`Self::spawn_connection_gauge_sampler`]'s doc: the production
        // caller (`main.rs`) calls it exactly once, immediately after `connect` succeeds.
        if cfg.max_age_secs == 0 {
            tracing::warn!(stream = %cfg.stream, "outbox.publisher.max_age_secs = 0 — the JetStream stream has no age limit and will grow until the broker's disk fills");
        }
        tracing::info!(
            stream = %cfg.stream,
            subjects = ?info.config.subjects,
            duplicate_window_secs = info.config.duplicate_window.as_secs(),
            "jetstream stream ready"
        );

        Ok(NatsEventPublisher {
            jetstream: js,
            source: cfg.source.clone(),
            breaker: Breaker::with_durations(OPEN_DURATION),
        })
    }

    /// Spawns the `iam_nats_connected` gauge sampler on its own task and hands back its
    /// [`JoinHandle`] — the SMA-471 review carry-over fix.
    ///
    /// **Before this fix**, `connect` spawned this loop itself with a bare `tokio::spawn`: no
    /// shutdown future, no `JoinHandle`. That broke this service's universal background-task
    /// convention — `PolicySnapshot::spawn_reload`, the denial-audit drain, the outbox relay,
    /// and `PgPartitionMaintainer::run` all take a `shutdown: impl Future<Output = ()>`,
    /// `tokio::select!` against it, and hand a `JoinHandle` back to `main.rs` to fold into the
    /// `servers` `JoinSet` (mirrors [`crate::adapters::authz::PolicySnapshot::spawn_reload`]'s
    /// shape exactly). A detached sampler has two concrete failure modes: a panic inside it dies
    /// silently with nothing to `.await` and surface the panic, AND the gauge freezes at
    /// whatever value it last sampled — a second-order recurrence of exactly the outage
    /// [`names::IAM_NATS_CONNECTED`] exists to catch (its own doc: sampled by a background task
    /// rather than inside `publish`, specifically because during a total outage `publish` stops
    /// being called and a publish-driven gauge would freeze exactly when it matters).
    ///
    /// **Why a separate method rather than inline in `connect`**: several unit/integration
    /// tests call `connect` directly and never intend to run a background task past their own
    /// scope (`tests/nats_publisher.rs` alone calls it ~10 times); a caller-supplied `shutdown`
    /// future is the only sound way to bound this loop's lifetime, and `connect`'s own signature
    /// has no shutdown parameter to source one from. Splitting it out also means calling this
    /// method twice on the same publisher is the caller's mistake to avoid, not something
    /// `connect` could ever have double-spawn-guarded on its own — `main.rs` calls it exactly
    /// once, immediately after `connect` succeeds, which is also the only call site that exists.
    ///
    /// Samples immediately (not after the first `poll` interval) — unlike `spawn_reload`, whose
    /// `select!` races the first sleep against shutdown before ever polling — so the gauge
    /// reflects reality from the instant this task starts rather than reporting nothing (a
    /// scrape in the gap would simply see no series yet) for up to `poll`.
    pub fn spawn_connection_gauge_sampler<S>(&self, shutdown: S) -> JoinHandle<()>
    where
        S: Future<Output = ()> + Send + 'static,
    {
        // `self.jetstream.client()` hands back a cheap clone of the same multiplexed connection
        // handle `publish_ack` itself reads via `self.jetstream.client().connection_state()` —
        // there is no separate `client` field (see this struct's doc).
        let probe = self.jetstream.client();
        tokio::spawn(async move {
            tokio::pin!(shutdown);
            loop {
                let up = probe.connection_state() != async_nats::connection::State::Disconnected;
                gauge!(names::IAM_NATS_CONNECTED).set(if up { 1.0 } else { 0.0 });
                tokio::select! {
                    () = tokio::time::sleep(Duration::from_secs(5)) => {}
                    () = &mut shutdown => break,
                }
            }
        })
    }

    /// The real publish. [`EventPublisher::publish`] delegates here and discards the ack; tests
    /// use this to assert `duplicate == true`, which the port's `Result<(), _>` cannot express.
    ///
    /// **Gated before any I/O** (SMA-471 Task 4 / D11): an open breaker, or a client that
    /// already knows its connection is down, is refused up front instead of burning a full
    /// `publish_timeout_secs` per outbox row — see [`Breaker`]'s doc for why that bound matters
    /// to `OutboxRelay::tick`'s lock-holding transaction.
    ///
    /// # Errors
    ///
    /// [`NatsPublisherError::Disconnected`] if the breaker is open or the client already
    /// reports a down connection, [`NatsPublisherError::Serialize`] if the event will not
    /// render as JSON, and [`NatsPublisherError::Publish`] for anything the broker refuses or
    /// fails to ack in time — including "no stream found for given subject", which is what an
    /// operator sees if the stream is deleted out from under a running service.
    pub async fn publish_ack(&self, ev: &DomainEvent) -> Result<PublishAck, NatsPublisherError> {
        if !self.breaker.admit() {
            return Err(NatsPublisherError::Disconnected);
        }
        // `Pending` (a reconnect in flight) is allowed through: a reconnect typically completes
        // well inside the ack timeout, and short-circuiting it would turn every brief blip into
        // a breaker trip. Only `Disconnected` fails fast.
        if self.jetstream.client().connection_state() == async_nats::connection::State::Disconnected {
            self.breaker.on_failure();
            return Err(NatsPublisherError::Disconnected);
        }

        let started = std::time::Instant::now();
        let result = self.send_and_await_ack(ev).await;
        histogram!(names::IAM_NATS_PUBLISH_DURATION_SECONDS).record(started.elapsed().as_secs_f64());
        if let Ok(ack) = &result
            && ack.duplicate
        {
            counter!(names::IAM_NATS_PUBLISH_DUPLICATES_TOTAL).increment(1);
        }
        match &result {
            Ok(_) => self.breaker.on_success(),
            Err(_) => self.breaker.on_failure(),
        }
        result
    }

    /// Test-only override of the breaker's open window. Not `#[cfg(test)]`-gated: an
    /// integration test in `tests/nats_publisher.rs` (a separate compilation unit that does not
    /// see this crate's `cfg(test)`) needs to shrink `OPEN_DURATION` so its assertion bounds the
    /// breaker's own behaviour rather than an incidental interaction with a fixed iteration
    /// count. `pub` on an already-public type, so it costs nothing in a production build.
    pub fn with_breaker_durations_for_tests(mut self, open_duration: Duration) -> NatsEventPublisher {
        self.breaker = Breaker::with_durations(open_duration);
        self
    }

    /// Serialize, publish, and await the persistence ack — the body `publish_ack` gates with
    /// the breaker and connection-state checks above.
    async fn send_and_await_ack(&self, ev: &DomainEvent) -> Result<PublishAck, NatsPublisherError> {
        let body = serde_json::to_vec(&CloudEvent::from_domain_event(ev, &self.source)).map_err(NatsPublisherError::Serialize)?;

        let mut headers = async_nats::HeaderMap::new();
        headers.insert("Content-Type", CONTENT_TYPE);

        let publish = PublishMessage::build()
            .payload(body.into())
            .headers(headers)
            // Same string the CloudEvents `id` renders as — both go through `render_id`.
            .message_id(render_id(ev.id));

        let ack_future = self.jetstream.send_publish(ev.event_type.as_wire(), publish).await.map_err(NatsPublisherError::Publish)?;
        // The SECOND await. This is what makes `Ok(())` mean "persisted" — see the module doc.
        ack_future.await.map_err(NatsPublisherError::Publish)
    }
}

/// Fails when the live stream's config is weaker than what this service requires (D7).
///
/// Deliberately narrow: this checks `retention`, `duplicate_window`, `storage`, `subjects`, and
/// `max_age` — the properties D3's dedup story and D8's "survive a restart" claim actually rest
/// on. `max_msgs` / `max_bytes` / `discard` are the same class of drift (a pre-existing stream
/// could be adopted with a byte or message cap this service never asked for) but are out of
/// scope for SMA-471; a residual gap, not an oversight.
fn verify_stream(name: &str, live: &jetstream::stream::Config, want_window: Duration) -> Result<(), NatsPublisherError> {
    // Checked first: `retention` is the property every other check is pointless without. A
    // `WorkQueue` stream drops a message once ONE subscriber acks it; an `Interest` stream drops
    // it once "all known observables" have acked — vacuously true when nothing subscribes, which
    // is every deployment of this PR (no consumer side ships yet, spec §8). A stream adopted with
    // either policy passes `duplicate_window`/`storage`/`subjects`/`max_age` unmodified and looks
    // completely healthy while discarding every message on arrival.
    if live.retention != jetstream::stream::RetentionPolicy::Limits {
        return Err(NatsPublisherError::StreamConfigDrift {
            stream: name.to_string(),
            field: "retention",
            want: "limits".to_string(),
            got: format!("{:?}", live.retention).to_lowercase(),
        });
    }
    if live.duplicate_window < want_window {
        return Err(NatsPublisherError::StreamConfigDrift {
            stream: name.to_string(),
            field: "duplicate_window",
            want: format!("{}s", want_window.as_secs()),
            got: format!("{}s", live.duplicate_window.as_secs()),
        });
    }
    if live.storage != StorageType::File {
        return Err(NatsPublisherError::StreamConfigDrift {
            stream: name.to_string(),
            field: "storage",
            want: "file".to_string(),
            got: format!("{:?}", live.storage).to_lowercase(),
        });
    }
    if !live.subjects.iter().any(|s| s == SUBJECT_FILTER) {
        return Err(NatsPublisherError::StreamConfigDrift {
            stream: name.to_string(),
            field: "subjects",
            want: SUBJECT_FILTER.to_string(),
            got: live.subjects.join(","),
        });
    }
    // `max_age == 0` is JetStream's "unlimited" sentinel, so it can never be too short. Anything
    // else must outlive the dedup window: a message aged out of the stream is also gone from the
    // rebuilt dedup map, which would make the window shorter than it claims to be.
    let live_max_age = live.max_age.as_secs();
    if live_max_age != 0 && live_max_age <= want_window.as_secs() {
        return Err(NatsPublisherError::StreamConfigDrift {
            stream: name.to_string(),
            field: "max_age",
            want: format!("> {}s or 0", want_window.as_secs()),
            got: format!("{live_max_age}s"),
        });
    }
    Ok(())
}

#[async_trait]
impl EventPublisher for NatsEventPublisher {
    async fn publish(&self, ev: &DomainEvent) -> Result<(), PublishError> {
        self.publish_ack(ev)
            .await
            .map(|_ack| ())
            .map_err(|e| PublishError::from(Box::new(e) as Box<dyn std::error::Error + Send + Sync>))
    }
}

#[cfg(test)]
mod tests {
    //! `verify_stream` unit tests (SMA-471 D7). No broker needed — the function takes a live
    //! `stream::Config` by reference, so each case is a hand-built config differing from the
    //! wanted one in exactly the field it is about. The Docker-backed end of the same behaviour
    //! (that `connect` actually reaches this on an adopted stream) is
    //! `tests/nats_publisher.rs`.
    use super::*;

    const WANT_WINDOW: Duration = Duration::from_secs(3_600);

    /// A live config that matches everything `connect` asks for.
    fn matching() -> jetstream::stream::Config {
        jetstream::stream::Config {
            name: "IAM_EVENTS".to_string(),
            subjects: vec![SUBJECT_FILTER.to_string()],
            retention: jetstream::stream::RetentionPolicy::Limits,
            storage: StorageType::File,
            duplicate_window: WANT_WINDOW,
            max_age: Duration::from_secs(604_800),
            num_replicas: 1,
            ..Default::default()
        }
    }

    /// The drifted `field` of an expected drift error, or a panic naming what came back instead.
    fn drifted_field(result: Result<(), NatsPublisherError>) -> &'static str {
        match result {
            Err(NatsPublisherError::StreamConfigDrift { field, .. }) => field,
            Err(other) => panic!("expected a StreamConfigDrift, got: {other}"),
            Ok(()) => panic!("expected a StreamConfigDrift, got Ok"),
        }
    }

    #[test]
    fn a_matching_config_passes() {
        verify_stream("IAM_EVENTS", &matching(), WANT_WINDOW).expect("a matching config must be adopted");
    }

    /// `WorkQueue` removes a message once ONE subscriber acks it. This PR ships no consumer
    /// side, so on a `WorkQueue` stream nothing would ever ack and — worse — anything that did
    /// subscribe would race every other reader for the single delivery.
    #[test]
    fn work_queue_retention_is_drift() {
        let mut live = matching();
        live.retention = jetstream::stream::RetentionPolicy::WorkQueue;
        assert_eq!(drifted_field(verify_stream("IAM_EVENTS", &live, WANT_WINDOW)), "retention");
    }

    /// `Interest` removes a message once all known observables have acked it — vacuously true
    /// with zero observables, which is every deployment of this PR. The worst finding in the
    /// SMA-471 review: this is the one drift that discards every message on arrival while every
    /// other check (`duplicate_window`, `storage`, `subjects`, `max_age`) still passes.
    #[test]
    fn interest_retention_is_drift() {
        let mut live = matching();
        live.retention = jetstream::stream::RetentionPolicy::Interest;
        assert_eq!(drifted_field(verify_stream("IAM_EVENTS", &live, WANT_WINDOW)), "retention");
    }

    // No "non-drift direction" test analogous to `a_longer_duplicate_window_is_not_drift` or
    // `an_extra_subject_alongside_the_filter_is_not_drift`: unlike a window or a subject set,
    // retention has no "stronger than asked" value — `Limits` is the only policy that does not
    // drop messages based on subscriber acknowledgement, so `a_matching_config_passes` above
    // already covers the sole non-drift case.

    #[test]
    fn a_shorter_duplicate_window_is_drift() {
        let mut live = matching();
        live.duplicate_window = Duration::from_secs(5);
        assert_eq!(drifted_field(verify_stream("IAM_EVENTS", &live, WANT_WINDOW)), "duplicate_window");
    }

    /// A window LONGER than configured is safe (it dedups more, never less), so it is adopted.
    #[test]
    fn a_longer_duplicate_window_is_not_drift() {
        let mut live = matching();
        live.duplicate_window = WANT_WINDOW * 2;
        verify_stream("IAM_EVENTS", &live, WANT_WINDOW).expect("a wider window is not a weakening");
    }

    #[test]
    fn memory_storage_is_drift() {
        let mut live = matching();
        live.storage = StorageType::Memory;
        assert_eq!(drifted_field(verify_stream("IAM_EVENTS", &live, WANT_WINDOW)), "storage");
    }

    #[test]
    fn a_missing_subject_is_drift() {
        let mut live = matching();
        live.subjects = vec!["iam.principal.>".to_string()];
        assert_eq!(drifted_field(verify_stream("IAM_EVENTS", &live, WANT_WINDOW)), "subjects");
    }

    /// The filter only has to be PRESENT, not alone: a stream that also carries other subjects
    /// still covers every event this service publishes.
    #[test]
    fn an_extra_subject_alongside_the_filter_is_not_drift() {
        let mut live = matching();
        live.subjects = vec!["other.>".to_string(), SUBJECT_FILTER.to_string()];
        verify_stream("IAM_EVENTS", &live, WANT_WINDOW).expect("a superset of subjects still covers iam.>");
    }

    #[test]
    fn a_max_age_below_the_dedup_window_is_drift() {
        let mut live = matching();
        live.max_age = Duration::from_secs(60);
        assert_eq!(drifted_field(verify_stream("IAM_EVENTS", &live, WANT_WINDOW)), "max_age");
    }

    /// The boundary: `max_age == duplicate_window` is rejected too — a message aged out at
    /// exactly the window's edge is already gone from the rebuilt dedup map.
    #[test]
    fn a_max_age_equal_to_the_dedup_window_is_drift() {
        let mut live = matching();
        live.max_age = WANT_WINDOW;
        assert_eq!(drifted_field(verify_stream("IAM_EVENTS", &live, WANT_WINDOW)), "max_age");
    }

    /// `0` is JetStream's "unlimited" sentinel, not a zero-length retention.
    #[test]
    fn an_unlimited_max_age_is_not_drift() {
        let mut live = matching();
        live.max_age = Duration::ZERO;
        verify_stream("IAM_EVENTS", &live, WANT_WINDOW).expect("max_age = 0 means unlimited");
    }

    /// The rendered message must name the stream, the field, and both values — it is what boot
    /// logs and an operator's first look at a failed rollout.
    #[test]
    fn the_drift_message_names_the_stream_field_and_both_values() {
        let mut live = matching();
        live.duplicate_window = Duration::from_secs(5);
        let err = verify_stream("IAM_EVENTS", &live, WANT_WINDOW).unwrap_err();
        let rendered = format!("{err}");
        assert_eq!(rendered, "stream IAM_EVENTS has duplicate_window = 5s, but this service requires 3600s");
    }

    // `Breaker` unit tests (SMA-471 Task 4 / D11). No broker needed — `Breaker` is a plain
    // consecutive-failure counter with no NATS dependency at all.

    #[test]
    fn the_breaker_opens_after_three_consecutive_failures() {
        let b = Breaker::with_durations(Duration::from_secs(2));
        assert!(b.admit(), "starts closed");
        for _ in 0..3 {
            b.on_failure();
        }
        assert!(!b.admit(), "three consecutive failures must open it");
    }

    #[test]
    fn a_success_resets_the_failure_run() {
        let b = Breaker::with_durations(Duration::from_secs(2));
        b.on_failure();
        b.on_failure();
        b.on_success();
        b.on_failure();
        b.on_failure();
        assert!(b.admit(), "the run was broken, so two more failures must not open it");
    }

    #[test]
    fn an_open_breaker_admits_a_probe_once_the_window_elapses() {
        let b = Breaker::with_durations(Duration::from_millis(20));
        for _ in 0..3 {
            b.on_failure();
        }
        assert!(!b.admit());
        std::thread::sleep(Duration::from_millis(40));
        assert!(b.admit(), "one probe must be admitted after the open window");
    }

    /// The D8 pre-flight: a bad credential path fails boot with a typed error naming the path,
    /// rather than surfacing as an authentication failure on the first connection attempt.
    #[tokio::test]
    async fn connect_reports_a_missing_credentials_file_by_path() {
        let cfg = PublisherConfig {
            backend: crate::config::PublisherBackend::Nats,
            url: Some("nats://127.0.0.1:14222".into()),
            credentials_file: Some("/nonexistent/iam.creds".to_string()),
            ..PublisherConfig::default()
        };
        let err = NatsEventPublisher::connect(&cfg).await.expect_err("a missing creds file must fail boot");
        assert!(matches!(err, NatsPublisherError::Credentials { .. }), "got {err}");
        assert!(format!("{err}").contains("/nonexistent/iam.creds"), "{err}");
    }

    /// A file that reads but is not a credential gets its own variant: an operator seeing
    /// "No such file or directory" for a file that plainly exists learns nothing.
    #[tokio::test]
    async fn connect_reports_a_malformed_credentials_file_distinctly() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("iam.creds");
        std::fs::write(&path, "this is not a creds file").unwrap();

        let cfg = PublisherConfig {
            backend: crate::config::PublisherBackend::Nats,
            url: Some("nats://127.0.0.1:14222".into()),
            credentials_file: Some(path.to_string_lossy().to_string()),
            ..PublisherConfig::default()
        };
        let err = NatsEventPublisher::connect(&cfg).await.expect_err("a malformed creds file must fail boot");
        assert!(matches!(err, NatsPublisherError::CredentialsParse { .. }), "got {err}");
        assert!(format!("{err}").contains(&path.to_string_lossy().to_string()), "{err}");
    }
}
