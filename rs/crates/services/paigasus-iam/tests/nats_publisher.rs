// SPDX-License-Identifier: Apache-2.0

//! `NatsEventPublisher` integration tests (SMA-471). Runs against an ephemeral JetStream-enabled
//! NATS in Docker; the skip-versus-panic decision lives once, in `tests/support/docker.rs`'s
//! `start_or_skip` (SMA-538), rather than being restated here.

mod support;

use std::time::Duration;

use async_nats::jetstream;
use chrono::Utc;
use paigasus_iam::adapters::events::NatsEventPublisher;
use paigasus_iam::config::{PublisherBackend, PublisherConfig};
use paigasus_iam_core::{DomainEvent, EventPublisher, EventType};
use testcontainers_modules::nats::{Nats, NatsServerCmd};
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt};
use uuid::Uuid;

/// Starts NATS **with JetStream enabled**. The stock `nats` image runs WITHOUT it, so the flag
/// is explicit; without it every test here fails at `get_or_create_stream`. The flag comes from
/// the module's own `NatsServerCmd::with_jetstream()` (which renders `--jetstream`) rather than
/// a hand-rolled `with_cmd(["-js"])`, so a future rename in the image's CLI is the module's
/// problem, not ours.
async fn start_nats() -> Option<(ContainerAsync<Nats>, String)> {
    let cmd = NatsServerCmd::default().with_jetstream();
    let node = support::docker::start_or_skip(Nats::default().with_cmd(&cmd), "nats_publisher").await?;
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
/// How long the container-readiness helpers below will wait. This is a LOAD BUDGET, not an
/// expectation: on an idle machine both return in well under a second, and they return as soon as
/// the condition holds, so a generous ceiling costs nothing in the happy path. It is deliberately
/// large because the failure it guards is Docker being slow under contention — this suite runs 12
/// tests in parallel, each with its own container, and the restart test stops and starts one while
/// the other eleven are still churning. A 30s ceiling was observed to expire under exactly that
/// load (a full-suite run that normally takes 3.7s took 33s), which is a CI flake, not a defect in
/// the code under test.
const CONTAINER_READY_BUDGET: Duration = Duration::from_secs(90);

async fn url_of(node: &ContainerAsync<Nats>) -> String {
    let deadline = std::time::Instant::now() + CONTAINER_READY_BUDGET;
    loop {
        match node.get_host_port_ipv4(4222).await {
            Ok(port) => return format!("nats://127.0.0.1:{port}"),
            Err(e) if std::time::Instant::now() >= deadline => {
                panic!("nats port was never published within {CONTAINER_READY_BUDGET:?}: {e}")
            }
            Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    }
}

/// Blocks until the broker is serving **JetStream**, or panics after [`CONTAINER_READY_BUDGET`].
/// `ContainerAsync::start` does NOT re-apply the image's `ready_conditions` — those are only
/// awaited by the initial `AsyncRunner::start` — so a restarted container needs its own wait.
///
/// Probes JetStream rather than merely opening a connection: `nats-server` accepts TCP before it
/// has finished recovering JetStream state from disk, so a bare `connect` can succeed while a
/// subsequent stream lookup still fails. Callers here restart a broker and then immediately assert
/// on recovered dedup state, so "accepts connections" is the wrong readiness signal.
async fn await_ready(url: &str) {
    let deadline = std::time::Instant::now() + CONTAINER_READY_BUDGET;
    loop {
        // A full JetStream round-trip: connect, then look up the stream the caller is about to
        // assert on. Retries on ANY error, including a not-yet-recovered stream — which is the
        // point. The sole caller restarts a broker whose `IAM_EVENTS` stream already exists and is
        // file-backed, so "the stream is queryable again" is precisely the condition it needs.
        let ready = match async_nats::connect(url).await {
            Ok(client) => async_nats::jetstream::new(client).get_stream("IAM_EVENTS").await.is_ok(),
            Err(_) => false,
        };
        if ready {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "nats jetstream did not recover the IAM_EVENTS stream within {CONTAINER_READY_BUDGET:?}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn cfg(url: &str) -> PublisherConfig {
    PublisherConfig {
        backend: PublisherBackend::Nats,
        url: Some(url.into()),
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

/// SMA-471 D11: a stopped broker must fail via the connection-state gate, an order of magnitude
/// faster than `publish_timeout_secs` — so this test FAILS if the gate is deleted and the ack
/// timeout provides the bound instead.
#[tokio::test]
async fn a_stopped_broker_fails_fast_not_on_the_ack_timeout() {
    let Some((node, url)) = start_nats().await else { return };
    let publisher = NatsEventPublisher::connect(&cfg(&url)).await.unwrap();
    node.stop().await.unwrap();

    // Let the client observe the drop before timing the gate.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let started = std::time::Instant::now();
    let err = publisher.publish(&event(Uuid::from_u128(5), EventType::PolicyDeleted)).await.expect_err("must fail");
    let elapsed = started.elapsed();

    assert!(elapsed < std::time::Duration::from_millis(200), "expected the connection-state gate, took {elapsed:?}");
    assert!(describe_chain(&err).contains("connection is down"), "{}", describe_chain(&err));
}

/// SMA-471 D11: the relay publishes serially inside ONE lock-holding transaction, so a
/// blackholed broker must not cost `batch_size × publish_timeout_secs`. This is the test that
/// distinguishes the breaker from its absence — the stopped-broker case above cannot, because
/// there `connection_state()` flips to `Disconnected` and the (cheaper) connection-state gate
/// alone bounds it.
///
/// **Deviates from a bare accept-and-never-answer TCP listener** (what an earlier draft of this
/// test used). Verified against the vendored async-nats 0.50.0 source
/// (`connector.rs::try_connect_to_server`, `options.rs::connection_timeout` default 5s): the
/// very first thing a client does after the TCP handshake is wait for the server's `INFO` line,
/// bounded by `connection_timeout`. A listener that never writes anything therefore fails
/// `NatsEventPublisher::connect` itself (both the initial handshake AND, even if that were
/// stubbed out, `get_or_create_stream`'s own request/response) — it can never reach the point of
/// calling `publish`, so it cannot prove anything about the breaker.
///
/// Instead: connect for real against a live, unpaused broker (so the handshake and stream-ensure
/// both succeed), then **pause the container**. Docker pause freezes the server's process via
/// the cgroup freezer without closing the TCP connection — the kernel keeps ACKing bytes into the
/// socket's receive buffer even though no userspace process on the other end will ever read or
/// respond to them. That is exactly the scenario the breaker exists for: the client's
/// `connection_state()` stays `Connected` (the default `ping_interval` is 60s, far longer than
/// this test runs, so the client's own heartbeat never notices), so the connection-state gate
/// does NOT catch it — only repeated ack-timeout failures accumulating in the breaker do.
#[tokio::test]
async fn a_blackholed_broker_does_not_hold_a_batch_open() {
    let Some((node, url)) = start_nats().await else { return };
    let mut c = cfg(&url);
    c.publish_timeout_secs = 1;
    let publisher = NatsEventPublisher::connect(&c)
        .await
        .expect("connect against a live, unpaused broker must succeed")
        // Shrinks the breaker's open window so this test's bound reflects the breaker's own
        // cost formula (FAILURE_THRESHOLD × publish_timeout_secs) rather than happening to pass
        // because a fixed 100-iteration loop can't outrun the production 2s window either way.
        .with_breaker_durations_for_tests(std::time::Duration::from_millis(200));

    node.pause().await.expect("pause the broker container to blackhole it");

    let started = std::time::Instant::now();
    for i in 0..100u128 {
        let _ = publisher.publish(&event(Uuid::from_u128(1000 + i), EventType::RoleGranted)).await;
    }
    let elapsed = started.elapsed();

    node.unpause().await.expect("unpause before the container is torn down");

    assert!(
        elapsed < std::time::Duration::from_secs(20),
        "100 publishes against a paused (blackholed) broker took {elapsed:?}; without the breaker this is ~100s"
    );
}

/// End-to-end: real Postgres + real NATS through the real relay. Proves the adapter satisfies the
/// contract `OutboxRelay` actually depends on, not just the one its own unit tests assert.
#[tokio::test]
async fn the_relay_drains_rows_into_jetstream() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let Some((node, url)) = start_nats().await else { return };
    let publisher = NatsEventPublisher::connect(&cfg(&url)).await.unwrap();

    // Insert two unpublished outbox rows directly, the way `relay_pg.rs` does.
    let ids = [Uuid::from_u128(0xA1), Uuid::from_u128(0xA2)];
    for id in ids {
        support::insert_outbox_row(&db, id).await;
    }

    let relay = paigasus_iam::adapters::events::OutboxRelay::new(db.clone(), Duration::from_secs(5), 100, 60);
    let report = relay.tick(&publisher).await.unwrap();
    assert_eq!(report.drained, 2);
    assert_eq!(report.failures, 0);

    let js = jetstream::new(async_nats::connect(&url).await.unwrap());
    let info = js.get_stream("IAM_EVENTS").await.unwrap().info().await.unwrap().clone();
    assert_eq!(info.state.messages, 2);
    assert_eq!(support::unpublished_count(&db).await, 0, "published_at must be stamped");

    // A stopped broker leaves rows unpublished, with attempts and last_error recorded.
    node.stop().await.unwrap();
    for id in [Uuid::from_u128(0xB1)] {
        support::insert_outbox_row(&db, id).await;
    }
    let report = relay.tick(&publisher).await.unwrap();
    assert_eq!(report.drained, 1);
    assert_eq!(report.failures, 1);
    assert_eq!(support::unpublished_count(&db).await, 1);
    assert!(support::last_error(&db, Uuid::from_u128(0xB1)).await.is_some(), "last_error must be recorded");
}

/// AC8: the three NATS metric families must be "registered, described, and **emitted**".
/// `describe_iam_metrics` (main.rs) and the `names::ALL` drift test already cover registration
/// and description; nothing previously proved emission, and the riskiest part — that
/// `spawn_connection_gauge_sampler` is ever actually called (`main.rs:98`) — was asserted by
/// nothing at all.
///
/// **This genuinely cannot be done Docker-free**, unlike `redis_conn.rs`'s equivalent
/// (`breaker_transitions_emit_the_gauge_and_the_counter`, which needs no Redis at all because
/// `ConnectionManager::new_lazy_with_config` never dials). `async-nats` has no lazy/mock client
/// constructor: `NatsEventPublisher::connect` cannot return `Ok` — and so a `NatsEventPublisher`
/// cannot exist at all, let alone reach the priming line or have a sampler spawned against it —
/// until a real TCP handshake AND a real JetStream `STREAM.INFO`/`STREAM.CREATE` round trip have
/// both already succeeded against a real server. A hand-rolled stub server would need to speak
/// enough of the JetStream request/reply protocol to satisfy `get_or_create_stream`, which is
/// reimplementing the thing under test, not a reasonable test shortcut.
///
/// Uses `metrics::set_default_local_recorder` (a guard), not `with_local_recorder` (which only
/// accepts a synchronous closure and so cannot straddle `.await` points). Holding the guard is
/// sound specifically because `#[tokio::test]` defaults to the CURRENT-THREAD runtime flavor:
/// this test body and the `tokio::spawn`ed sampler task below are polled on the very same OS
/// thread, so the thread-local recorder installed here is visible inside the sampler's loop too
/// — that would NOT hold under `#[tokio::test(flavor = "multi_thread")]`.
#[tokio::test]
async fn nats_metrics_are_actually_emitted() {
    use metrics_util::debugging::{DebugValue, DebuggingRecorder};
    use paigasus_observability::names;

    let Some((_node, url)) = start_nats().await else { return };

    let recorder = DebuggingRecorder::new();
    let snapshotter = recorder.snapshotter();
    let _guard = metrics::set_default_local_recorder(&recorder);

    let publisher = NatsEventPublisher::connect(&cfg(&url)).await.expect("connect");

    // `iam_nats_publish_duplicates_total` is primed at ZERO inside `connect` itself (SMA-471),
    // specifically so the FIRST duplicate can still satisfy `increase() > 0` — a metrics-rs
    // counter otherwise only appears in the registry at the value of its first increment.
    let snapshot = snapshotter.snapshot().into_vec();
    let primed = snapshot
        .iter()
        .find(|(key, _, _, _)| key.key().name() == names::IAM_NATS_PUBLISH_DUPLICATES_TOTAL)
        .expect("SMA-471 AC8: iam_nats_publish_duplicates_total was never emitted by connect()");
    assert!(matches!(primed.3, DebugValue::Counter(0)), "expected connect() to prime the counter at 0, got {:?}", primed.3);

    // `iam_nats_publish_duration_seconds` is recorded around every `publish_ack` call, and
    // `iam_nats_publish_duplicates_total` increments past its primed zero on a duplicate ack —
    // one fresh publish plus one deliberate redelivery of the same event id proves both.
    let id = Uuid::from_u128(0xC1);
    publisher.publish(&event(id, EventType::RoleGranted)).await.expect("first publish");
    publisher.publish(&event(id, EventType::RoleGranted)).await.expect("redelivery must dedup, not error");

    let snapshot = snapshotter.snapshot().into_vec();
    let duration = snapshot
        .iter()
        .find(|(key, _, _, _)| key.key().name() == names::IAM_NATS_PUBLISH_DURATION_SECONDS)
        .expect("SMA-471 AC8: iam_nats_publish_duration_seconds was never emitted by publish_ack()");
    assert!(
        matches!(&duration.3, DebugValue::Histogram(samples) if samples.len() >= 2),
        "expected 2 recorded durations, got {:?}",
        duration.3
    );
    let duplicates = snapshot
        .iter()
        .find(|(key, _, _, _)| key.key().name() == names::IAM_NATS_PUBLISH_DUPLICATES_TOTAL)
        .expect("iam_nats_publish_duplicates_total disappeared after being primed");
    assert!(
        matches!(duplicates.3, DebugValue::Counter(1)),
        "expected the redelivery to bump the counter to 1, got {:?}",
        duplicates.3
    );

    // `iam_nats_connected` is set by the BACKGROUND sampler, not by `publish` — this is the
    // review's specific concern: `main.rs:98` is the only production call site for
    // `spawn_connection_gauge_sampler`, and nothing previously proved it is ever reached.
    let (tx, mut rx) = tokio::sync::watch::channel(());
    let handle = publisher.spawn_connection_gauge_sampler(async move {
        let _ = rx.changed().await;
    });
    // The sampler's own doc promises it samples immediately on start (not after its first
    // sleep) — this still needs at least one scheduler tick to actually run on this
    // current-thread runtime, which the sleep below yields for.
    tokio::time::sleep(Duration::from_millis(50)).await;

    let snapshot = snapshotter.snapshot().into_vec();
    let gauge = snapshot
        .iter()
        .find(|(key, _, _, _)| key.key().name() == names::IAM_NATS_CONNECTED)
        .expect("SMA-471 AC8: iam_nats_connected was never emitted by spawn_connection_gauge_sampler()");
    assert!(
        matches!(gauge.3, DebugValue::Gauge(v) if (v.into_inner() - 1.0).abs() < f64::EPSILON),
        "expected a live connection to read 1, got {:?}",
        gauge.3
    );

    drop(tx);
    handle.await.expect("sampler task panicked");
}
