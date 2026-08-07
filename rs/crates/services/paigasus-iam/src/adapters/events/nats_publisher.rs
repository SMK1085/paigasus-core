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

use std::time::Duration;

use async_nats::jetstream::context::{CreateStreamError, PublishError as JetStreamPublishError};
use async_nats::jetstream::message::PublishMessage;
use async_nats::jetstream::{self, publish::PublishAck, stream::StorageType};
use async_trait::async_trait;
use paigasus_iam_core::{DomainEvent, EventPublisher, PublishError};

use crate::adapters::events::cloud_event::{CloudEvent, render_id};
use crate::config::PublisherConfig;

/// The stream's subject filter. Every `EventType` wire string is `iam.`-prefixed
/// (`domain_event.rs`), so one wildcard covers them all.
const SUBJECT_FILTER: &str = "iam.>";

/// Structured-mode CloudEvents content type (SMA-471 D6).
const CONTENT_TYPE: &str = "application/cloudevents+json; charset=utf-8";

/// Why a publish (or the connect that precedes it) failed.
///
/// Every fallible variant keeps its cause as a `source()` rather than folding it into the
/// message: `relay.rs::describe_error` walks that chain into `event_outbox.last_error`, and
/// that string is the whole of what an operator sees on a parked row.
#[derive(Debug, thiserror::Error)]
pub enum NatsPublisherError {
    /// The configured `credentials_file` could not be read or parsed. Split out from
    /// [`Self::Connect`] because `ConnectOptions::with_credentials_file` fails with a bare
    /// `io::Error` that names neither NATS nor the path — "No such file or directory" alone is
    /// not an actionable boot error.
    #[error("nats credentials file {path} could not be loaded")]
    Credentials {
        path: String,
        #[source]
        source: std::io::Error,
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
    /// Reserved for the connection-state short-circuit (SMA-471 Task 4): once the client
    /// reports a down connection, publishing is refused up front instead of burning a full
    /// `publish_timeout_secs` per outbox row in the batch.
    #[error("nats connection is down")]
    Disconnected,
}

/// The production outbox sink. The underlying `Context` holds a `Client`, which multiplexes one
/// TCP connection and reconnects in the background, so no pooling is needed.
///
/// The client itself is deliberately NOT a separate field: [`jetstream::Context::client`] hands
/// back a (cheap) clone of it, which is where the connection-state short-circuit of Task 4 gets
/// its `Client::connection_state()`.
///
/// `Debug` is derived rather than redacted: neither field carries the (possibly
/// credential-bearing) broker URL — `PublisherConfig` keeps that redacted at the config layer,
/// and `Context`'s own `Debug` renders connection handles, not the address they were dialled
/// from.
#[derive(Debug)]
pub struct NatsEventPublisher {
    jetstream: jetstream::Context,
    source: String,
}

impl NatsEventPublisher {
    /// Connects, ensures the stream, and **verifies the live stream's config** (SMA-471 D7).
    ///
    /// `get_or_create_stream` creates or fetches; it does NOT reconcile an existing stream's
    /// config. That non-reconciliation is deliberate — this service must never silently reshape
    /// a stream external consumers depend on — but adoption is conditional: a stream whose
    /// `duplicate_window` is shorter than configured, or whose storage is not `File`, or whose
    /// subjects do not cover `iam.>`, fails boot rather than being adopted.
    ///
    /// # Errors
    ///
    /// [`NatsPublisherError::Credentials`] / [`NatsPublisherError::Connect`] when the broker
    /// cannot be reached or authenticated against, [`NatsPublisherError::Ensure`] when the
    /// stream can neither be fetched nor created, and [`NatsPublisherError::StreamConfigDrift`]
    /// when an existing stream is weaker than this service requires.
    ///
    /// # Panics
    ///
    /// If `cfg.url` is `None`. `IamConfig::validate` rejects that combination at load time, so
    /// reaching this is a programming error, not an operator one.
    pub async fn connect(cfg: &PublisherConfig) -> Result<NatsEventPublisher, NatsPublisherError> {
        let url = cfg.url.as_deref().expect("validate() guarantees url is Some for the nats backend");

        let opts = match &cfg.credentials_file {
            Some(path) => async_nats::ConnectOptions::with_credentials_file(path)
                .await
                .map_err(|source| NatsPublisherError::Credentials { path: path.clone(), source })?,
            None => async_nats::ConnectOptions::new(),
        };
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
        })
    }

    /// The real publish. [`EventPublisher::publish`] delegates here and discards the ack; tests
    /// use this to assert `duplicate == true`, which the port's `Result<(), _>` cannot express.
    ///
    /// # Errors
    ///
    /// [`NatsPublisherError::Serialize`] if the event will not render as JSON, and
    /// [`NatsPublisherError::Publish`] for anything the broker refuses or fails to ack in time —
    /// including "no stream found for given subject", which is what an operator sees if the
    /// stream is deleted out from under a running service.
    pub async fn publish_ack(&self, ev: &DomainEvent) -> Result<PublishAck, NatsPublisherError> {
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
fn verify_stream(name: &str, live: &jetstream::stream::Config, want_window: Duration) -> Result<(), NatsPublisherError> {
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
}
