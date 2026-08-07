// SPDX-License-Identifier: Apache-2.0

//! `NatsEventPublisher` integration tests (SMA-471). Runs against an ephemeral JetStream-enabled
//! NATS in Docker, with the house gating: a missing Docker daemon is a HARD FAILURE in CI and a
//! skip on a Docker-less laptop (mirrors `tests/redis_jwks_cache.rs`).

use std::time::Duration;

use async_nats::jetstream;
use chrono::Utc;
use paigasus_iam::adapters::events::NatsEventPublisher;
use paigasus_iam::config::{PublisherBackend, PublisherConfig};
use paigasus_iam_core::{DomainEvent, EventPublisher, EventType};
use testcontainers_modules::nats::{Nats, NatsServerCmd};
use testcontainers_modules::testcontainers::runners::AsyncRunner;
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt};
use uuid::Uuid;

/// Starts NATS **with JetStream enabled**. The stock `nats` image runs WITHOUT it, so the flag
/// is explicit; without it every test here fails at `get_or_create_stream`. The flag comes from
/// the module's own `NatsServerCmd::with_jetstream()` (which renders `--jetstream`) rather than
/// a hand-rolled `with_cmd(["-js"])`, so a future rename in the image's CLI is the module's
/// problem, not ours.
async fn start_nats() -> Option<(ContainerAsync<Nats>, String)> {
    let cmd = NatsServerCmd::default().with_jetstream();
    let node = match Nats::default().with_cmd(&cmd).start().await {
        Ok(n) => n,
        Err(e) => {
            if std::env::var_os("CI").is_some() {
                panic!("Docker is required for the nats publisher tests in CI: {e}");
            }
            eprintln!("skipping nats_publisher: Docker unavailable ({e})");
            return None;
        }
    };
    let url = url_of(&node).await;
    Some((node, url))
}

/// The client URL for a running container. Re-read (not cached) after a restart: Docker
/// re-allocates the ephemeral host port when a container with a dynamic publish is started
/// again, so the pre-stop URL is not reusable.
///
/// Retries rather than unwrapping. `AsyncRunner::start` returns once the server has logged that
/// it is listening, but the runtime publishes the host-side port mapping independently — an
/// inspect issued in that gap comes back `PortNotExposed`. It is rare for one container and
/// reproducible when this suite races eight of them (nextest runs each `#[tokio::test]` in its
/// own process, in parallel).
async fn url_of(node: &ContainerAsync<Nats>) -> String {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        match node.get_host_port_ipv4(4222).await {
            Ok(port) => return format!("nats://127.0.0.1:{port}"),
            Err(e) if std::time::Instant::now() >= deadline => panic!("nats port was never published within 30s: {e}"),
            Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    }
}

/// Blocks until the broker accepts a connection, or panics after 30s. `ContainerAsync::start`
/// does NOT re-apply the image's `ready_conditions` — those are only awaited by the initial
/// `AsyncRunner::start` — so a restarted container needs its own readiness wait.
async fn await_ready(url: &str) {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        match async_nats::connect(url).await {
            Ok(client) => {
                drop(client);
                return;
            }
            Err(e) if std::time::Instant::now() >= deadline => {
                panic!("nats did not come back up within 30s: {e}");
            }
            Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    }
}

fn cfg(url: &str) -> PublisherConfig {
    PublisherConfig {
        backend: PublisherBackend::Nats,
        url: Some(url.to_string()),
        ..PublisherConfig::default()
    }
}

fn event(id: Uuid, et: EventType) -> DomainEvent {
    DomainEvent {
        id,
        event_type: et,
        schema_version: 1,
        aggregate_prn: "prn:pgs:iam:::principal/00000000-0000-0000-0000-0000000000aa".to_string(),
        actor_prn: None,
        occurred_at: Utc::now(),
        payload: serde_json::json!({"kind": "user"}),
        correlation_id: None,
    }
}

/// The live stream state, read fresh from the broker (not from any cached `Info`).
async fn stream_info(url: &str) -> jetstream::stream::Info {
    let js = jetstream::new(async_nats::connect(url).await.unwrap());
    js.get_stream("IAM_EVENTS").await.unwrap().info().await.unwrap().clone()
}

#[tokio::test]
async fn ensure_is_idempotent() {
    let Some((_node, url)) = start_nats().await else { return };
    let first = NatsEventPublisher::connect(&cfg(&url)).await.expect("first connect");
    drop(first);
    NatsEventPublisher::connect(&cfg(&url)).await.expect("second connect must adopt, not fail");

    let info = stream_info(&url).await;
    assert_eq!(info.config.subjects, vec!["iam.>".to_string()]);
}

/// Asserts what actually lands on the wire: the subject, both headers, and the whole
/// CloudEvents body.
///
/// **Read back from the STREAM, not from a core subscription** — and that is a correctness
/// requirement, not a style choice. A core subscriber would have to be registered on the server
/// before the publish lands, and nothing in the client API makes that orderable across two
/// independent connections: `Client::flush` resolves on a bare `AsyncWrite::poll_flush` of its
/// own socket (`async-nats-0.50.0/src/connection.rs:753`, signalled at `lib.rs:658`), so it
/// proves the SUB bytes left this process and nothing about whether the server has parsed them.
/// Meanwhile the publisher's ack-waited publish is a full round-trip on a *different*
/// connection, and can complete first. That is a genuine race, and it fails often on a loaded
/// machine.
///
/// Reading the stream has no ordering requirement at all: `publish` returns `Ok` only after
/// JetStream has acked persistence, so the message is durable state by the time this queries
/// for it. It is also the more faithful assertion — real consumers read the stream.
///
/// The read is keyed on **sequence**, not subject, so `msg.subject` is a real assertion rather
/// than an echo of the query. Sequence 1 is unambiguous: `connect` creates the stream fresh in
/// this test's own container, and this is the only publish against it.
#[tokio::test]
async fn publishes_a_cloud_event_on_the_wire_subject() {
    let Some((_node, url)) = start_nats().await else { return };
    let publisher = NatsEventPublisher::connect(&cfg(&url)).await.unwrap();

    // Through the PORT method — `OutboxRelay::tick` calls exactly this.
    let ev = event(Uuid::from_u128(1), EventType::PrincipalCreated);
    publisher.publish(&ev).await.expect("publish");

    let js = jetstream::new(async_nats::connect(&url).await.unwrap());
    let stream = js.get_stream("IAM_EVENTS").await.unwrap();
    let msg = stream.get_raw_message(1).await.expect("the published message must be readable at sequence 1");

    assert_eq!(msg.subject.as_str(), "iam.principal.created");
    assert_eq!(
        msg.headers.get("Content-Type").map(async_nats::HeaderValue::as_str),
        Some("application/cloudevents+json; charset=utf-8"),
        "structured-mode CloudEvents content type must be on the wire (D6)"
    );
    assert_eq!(
        msg.headers.get("Nats-Msg-Id").map(async_nats::HeaderValue::as_str),
        Some(ev.id.hyphenated().to_string().as_str()),
        "Nats-Msg-Id must be the event id, rendered exactly as the CloudEvents `id` (D3)"
    );

    let body: serde_json::Value = serde_json::from_slice(&msg.payload).unwrap();
    assert_eq!(body["specversion"], "1.0");
    assert_eq!(body["id"], ev.id.to_string());
    assert_eq!(body["type"], "iam.principal.created");
    assert_eq!(body["data"], serde_json::json!({"kind": "user"}));
}

/// SMA-471 D3 — the guarantee the whole design rests on. Asserted through `publish_ack` and NOT
/// through the stream count alone: an implementation that simply swallowed the second publish
/// would also leave one message in the stream.
#[tokio::test]
async fn a_duplicate_publish_is_deduped_and_still_succeeds() {
    let Some((_node, url)) = start_nats().await else { return };
    let publisher = NatsEventPublisher::connect(&cfg(&url)).await.unwrap();
    let ev = event(Uuid::from_u128(42), EventType::RoleRevoked);

    let first = publisher.publish_ack(&ev).await.unwrap();
    assert!(!first.duplicate, "first publish must not be a duplicate");
    let second = publisher.publish_ack(&ev).await.unwrap();
    assert!(second.duplicate, "second publish of the same id must be acked as a duplicate");

    assert!(publisher.publish(&ev).await.is_ok(), "a deduped publish is SUCCESS, not an error");

    assert_eq!(stream_info(&url).await.state.messages, 1, "dedup must leave exactly one message");
}

/// Guards that `message_id` is per-event rather than a constant or omitted.
#[tokio::test]
async fn distinct_ids_are_not_deduped() {
    let Some((_node, url)) = start_nats().await else { return };
    let publisher = NatsEventPublisher::connect(&cfg(&url)).await.unwrap();
    publisher.publish(&event(Uuid::from_u128(1), EventType::RoleGranted)).await.unwrap();
    publisher.publish(&event(Uuid::from_u128(2), EventType::RoleGranted)).await.unwrap();

    assert_eq!(stream_info(&url).await.state.messages, 2);
}

/// SMA-471 D7: an existing stream whose `duplicate_window` is smaller than configured must be
/// REJECTED, not silently adopted — it is the field every safety claim depends on.
#[tokio::test]
async fn a_drifted_duplicate_window_is_rejected_at_connect() {
    let Some((_node, url)) = start_nats().await else { return };
    let js = jetstream::new(async_nats::connect(&url).await.unwrap());
    js.create_stream(jetstream::stream::Config {
        name: "IAM_EVENTS".to_string(),
        subjects: vec!["iam.>".to_string()],
        storage: jetstream::stream::StorageType::File,
        duplicate_window: Duration::from_secs(5),
        ..Default::default()
    })
    .await
    .unwrap();

    let err = NatsEventPublisher::connect(&cfg(&url)).await.expect_err("drifted stream must be rejected");
    let rendered = format!("{err}");
    assert!(rendered.contains("duplicate_window"), "error must name the drifted field: {rendered}");
}

/// SMA-471 D7: memory storage loses every event on a broker restart.
#[tokio::test]
async fn a_memory_storage_stream_is_rejected_at_connect() {
    let Some((_node, url)) = start_nats().await else { return };
    let js = jetstream::new(async_nats::connect(&url).await.unwrap());
    js.create_stream(jetstream::stream::Config {
        name: "IAM_EVENTS".to_string(),
        subjects: vec!["iam.>".to_string()],
        storage: jetstream::stream::StorageType::Memory,
        duplicate_window: Duration::from_secs(3_600),
        ..Default::default()
    })
    .await
    .unwrap();

    let err = NatsEventPublisher::connect(&cfg(&url)).await.expect_err("memory storage must be rejected");
    assert!(format!("{err}").contains("storage"), "{err}");
}

/// SMA-471 D2's negative case. Without this, a fire-and-forget implementation (one that drops
/// the ack future instead of awaiting it) passes the entire rest of this suite.
#[tokio::test]
async fn publishing_with_no_stream_is_an_error_not_a_silent_success() {
    let Some((_node, url)) = start_nats().await else { return };
    let publisher = NatsEventPublisher::connect(&cfg(&url)).await.unwrap();

    let js = jetstream::new(async_nats::connect(&url).await.unwrap());
    js.delete_stream("IAM_EVENTS").await.unwrap();

    let err = publisher
        .publish(&event(Uuid::from_u128(7), EventType::PolicyPut))
        .await
        .expect_err("no stream covers the subject — publish must fail");
    let rendered = describe_chain(&err);
    assert!(rendered.len() > "backend error".len(), "error chain must be informative: {rendered}");
}

/// Mirrors `relay.rs::describe_error` so the test asserts what an operator actually sees in
/// `event_outbox.last_error`.
fn describe_chain(err: &(dyn std::error::Error + 'static)) -> String {
    let mut parts = vec![err.to_string()];
    let mut source = err.source();
    while let Some(e) = source {
        parts.push(e.to_string());
        source = e.source();
    }
    parts.join(": ")
}

/// SMA-471 D3/§4.3.5: JetStream's dedup state must survive a broker restart, or D3's coverage
/// claim is narrower than the docs say. If this fails, do NOT weaken the test — narrow the claim
/// in the spec, the rustdoc and ADR-0016.
#[tokio::test]
async fn dedup_survives_a_broker_restart() {
    let Some((node, url)) = start_nats().await else { return };
    let publisher = NatsEventPublisher::connect(&cfg(&url)).await.unwrap();
    let ev = event(Uuid::from_u128(99), EventType::ApiKeyRevoked);
    assert!(!publisher.publish_ack(&ev).await.unwrap().duplicate);

    node.stop().await.unwrap();
    node.start().await.unwrap();

    let url = url_of(&node).await;
    await_ready(&url).await;

    let publisher = NatsEventPublisher::connect(&cfg(&url)).await.unwrap();
    // The DISCRIMINATING assertion: a broker that lost its dedup state (or its whole stream)
    // would ack this as a fresh message and still leave `messages == 1`.
    assert!(
        publisher.publish_ack(&ev).await.unwrap().duplicate,
        "the re-publish must be acked as a duplicate — dedup state must survive a restart"
    );

    assert_eq!(stream_info(&url).await.state.messages, 1, "dedup state must survive a restart");
}
