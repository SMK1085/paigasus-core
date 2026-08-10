// SPDX-License-Identifier: Apache-2.0

//! NATS permission-set integration tests (SMA-493 §4.3).
//!
//! Boots a broker with the **committed** `ops/nats/test/` configuration and asserts the
//! `iam-publisher` and `gateway-consumer` permission sets are exactly sufficient and no broader.
//! The publisher side runs through [`NatsEventPublisher`] itself — not a hand-rolled client —
//! which is why the fixture's users are static **nkeys**: `auth_from_credentials` presents a bare
//! seed file as nkey auth (D2), so `credentials_file` stays in the loop while no key material is
//! committed.
//!
//! **Denials need the event callback.** A denied `subscribe` returns `Ok(Subscriber)` and a denied
//! `$JS.API` request simply never gets a reply, so "assert it was denied" is otherwise
//! indistinguishable from a broken fixture — and a test that merely waited out a timeout would
//! pass exactly as happily against a wide-open broker, which would make it worthless. Every
//! negative case here asserts on the server's asynchronous `Permissions Violation` text naming the
//! exact subject (D9), captured through the connection's `event_callback`.
//!
//! Docker gating matches the rest of the suite: a missing daemon is a HARD FAILURE in CI (`CI`
//! set) and a skip on a Docker-less laptop.

use std::path::{Path, PathBuf};
use std::time::Duration;

use async_nats::jetstream;
use chrono::Utc;
use paigasus_iam::adapters::events::NatsEventPublisher;
use paigasus_iam::config::{PublisherBackend, PublisherConfig};
use paigasus_iam_core::{DomainEvent, EventPublisher, EventType};
use testcontainers::core::WaitFor;
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};
use tokio::sync::mpsc;
use tokio_stream::StreamExt;
use uuid::Uuid;

/// See `tests/nats_publisher.rs` — same load budget, same reasoning: a ceiling on Docker being
/// slow under contention, not an expectation of how long anything takes.
const CONTAINER_READY_BUDGET: Duration = Duration::from_secs(90);

/// How long a denial assertion waits for the server's asynchronous refusal. Generous, because the
/// cost of it being too short is a flake and the cost of it being too long is a slow failure —
/// the happy path returns as soon as the violation lands.
const VIOLATION_BUDGET: Duration = Duration::from_secs(10);

/// The stream and durable the committed fixture grants are written against.
const STREAM: &str = "IAM_EVENTS";
const DURABLE: &str = "gateway-cache-invalidator";

/// The inbox prefixes `ops/nats/subjects.env` declares, one per identity (D4). A client whose
/// prefix does not match its own `subscribe` grant does not error — every request simply times out
/// on a reply it is not allowed to receive — so these are load-bearing constants, not labels.
const PUBLISHER_INBOX: &str = "_INBOX_IAM_PUB";
const CONSUMER_INBOX: &str = "_INBOX_GW";
const PROVISIONER_INBOX: &str = "_INBOX_PROV";

/// Repo root, from this crate's manifest dir: `rs/crates/services/paigasus-iam` → four levels up.
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../..").canonicalize().expect("repo root resolves")
}

/// One rendered fixture identity: the seed file the adapter authenticates with, and the public
/// key the broker config declares.
struct Identity {
    seed_path: PathBuf,
    public_key: String,
}

/// Mints a user nkey and writes its seed as a bare seed file — the `.nk` shape `parse_credentials`
/// maps to nkey auth. Freshly minted per run rather than committed, because committing a fixed
/// identity means committing its seed, which is a private key.
fn mint(dir: &Path, name: &str) -> Identity {
    let kp = nkeys::KeyPair::new_user();
    let seed = kp.seed().expect("a fresh keypair exposes its seed");
    let seed_path = dir.join(format!("{name}.nk"));
    std::fs::write(&seed_path, format!("-----BEGIN USER NKEY SEED-----\n{seed}\n------END USER NKEY SEED------\n")).expect("write seed");
    Identity {
        seed_path,
        public_key: kp.public_key(),
    }
}

/// A running fixture broker plus the identities its account declares.
///
/// `url` is the plaintext client URL. A TLS fixture (Task 7) boots the same way with
/// `nats-server-tls.conf` plus its certificate in `extra_files`, and dials
/// `url.replace("nats://", "tls://")` — the port is identical, so nothing here needs reshaping.
struct Fixture {
    _node: ContainerAsync<GenericImage>,
    _dir: tempfile::TempDir,
    url: String,
    publisher: Identity,
    consumer: Identity,
    provisioner: Identity,
}

/// Renders `accounts.conf.tmpl` with freshly minted identities and boots the broker with the
/// committed server config. `None` when Docker is unavailable outside CI.
///
/// `extra_files` is copied in alongside the two configs, keyed by absolute container path — the
/// hook a TLS fixture uses for its per-run certificate and key.
async fn start_fixture(server_conf: &str, extra_files: Vec<(String, Vec<u8>)>) -> Option<Fixture> {
    let dir = tempfile::tempdir().expect("tempdir");
    let ops = repo_root().join("ops/nats/test");

    let publisher = mint(dir.path(), "iam-publisher");
    let consumer = mint(dir.path(), "gateway-consumer");
    let provisioner = mint(dir.path(), "iam-provisioner");
    let sys = mint(dir.path(), "sys");

    // The permission lists are read from the COMMITTED template verbatim; only the identities are
    // substituted. That is what makes this a test of the artifact that ships rather than of a
    // convenient copy of it.
    let tmpl_path = ops.join("accounts.conf.tmpl");
    let rendered = std::fs::read_to_string(&tmpl_path)
        .unwrap_or_else(|e| panic!("the committed accounts template {} must be readable: {e}", tmpl_path.display()))
        .replace("{{SYS_NKEY}}", &sys.public_key)
        .replace("{{PUBLISHER_NKEY}}", &publisher.public_key)
        .replace("{{CONSUMER_NKEY}}", &consumer.public_key)
        .replace("{{PROVISIONER_NKEY}}", &provisioner.public_key);

    // Named, not inlined: `server_conf` varies per caller (the TLS fixture picks a different file),
    // so a typo must panic with the path it looked for rather than a bare "os error 2".
    let server_conf_path = ops.join(server_conf);
    let server_conf_bytes = std::fs::read(&server_conf_path).unwrap_or_else(|e| panic!("the fixture server config {} must be readable: {e}", server_conf_path.display()));

    let mut image = GenericImage::new("nats", "2.10.14")
        .with_wait_for(WaitFor::message_on_stderr("Server is ready"))
        .with_copy_to("/etc/nats/accounts.conf", rendered.into_bytes())
        .with_copy_to("/etc/nats/nats-server.conf", server_conf_bytes)
        .with_cmd(["-c", "/etc/nats/nats-server.conf"]);
    for (target, bytes) in extra_files {
        image = image.with_copy_to(target, bytes);
    }

    let node = match image.start().await {
        Ok(n) => n,
        Err(e) => {
            if std::env::var_os("CI").is_some() {
                panic!("Docker is required for the nats permission tests in CI: {e}");
            }
            eprintln!("skipping nats_permissions: Docker unavailable ({e})");
            return None;
        }
    };

    let deadline = std::time::Instant::now() + CONTAINER_READY_BUDGET;
    let port = loop {
        match node.get_host_port_ipv4(4222).await {
            Ok(p) => break p,
            Err(e) if std::time::Instant::now() >= deadline => panic!("nats port was never published within {CONTAINER_READY_BUDGET:?}: {e}"),
            Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    };

    Some(Fixture {
        _node: node,
        _dir: dir,
        url: format!("nats://127.0.0.1:{port}"),
        publisher,
        consumer,
        provisioner,
    })
}

/// A publisher config pointed at the fixture, authenticating as `identity`.
fn cfg_for(fixture: &Fixture, identity: &Identity, inbox_prefix: &str) -> PublisherConfig {
    PublisherConfig {
        backend: PublisherBackend::Nats,
        url: Some(fixture.url.clone()),
        credentials_file: Some(identity.seed_path.to_string_lossy().to_string()),
        inbox_prefix: Some(inbox_prefix.to_string()),
        allow_insecure_broker: true,
        ..PublisherConfig::default()
    }
}

/// A raw client for `identity`, with its [`async_nats::Event`]s piped into the returned channel so
/// denials can be asserted on rather than inferred from a timeout.
///
/// Authenticates through the adapter's own `auth_from_credentials` — the same callback body
/// `NatsEventPublisher::connect` installs — so a fixture client and the production client cannot
/// diverge in how they present a credential.
async fn client_for(fixture: &Fixture, identity: &Identity, inbox_prefix: &str) -> (async_nats::Client, mpsc::UnboundedReceiver<String>) {
    let (tx, rx) = mpsc::unbounded_channel();
    let path = identity.seed_path.to_string_lossy().to_string();
    let client = async_nats::ConnectOptions::with_auth_callback(move |nonce| {
        let path = path.clone();
        async move { paigasus_iam::adapters::events::auth_from_credentials(&path, &nonce).await }
    })
    .custom_inbox_prefix(inbox_prefix.to_string())
    .event_callback(move |event| {
        let tx = tx.clone();
        async move {
            // A closed receiver just means the test finished; nothing here should panic on it.
            let _ = tx.send(event.to_string());
        }
    })
    .connect(&fixture.url)
    .await
    .expect("fixture client connects");
    (client, rx)
}

/// Publishes to `subject` and asserts the server refuses it — the positive form of "this was
/// denied". `flush` is what forces the PUB out of the client's write buffer, so the refusal is
/// bounded by the server's round trip rather than by the client's flush cadence.
async fn expect_publish_denied(client: &async_nats::Client, rx: &mut mpsc::UnboundedReceiver<String>, subject: &'static str) {
    client.publish(subject, "{}".into()).await.expect("a denied publish is still accepted locally");
    client.flush().await.expect("flush");
    expect_permissions_violation(rx, subject).await;
}

/// Waits for a server error naming `subject`, or panics with everything it did see.
///
/// Matches the QUOTED subject (`Permissions Violation for Publication to "…"`) rather than a bare
/// substring: `$JS.API.CONSUMER.CREATE.IAM_EVENTS` is a prefix of
/// `$JS.API.CONSUMER.CREATE.IAM_EVENTS.wide-open`, so an unquoted `contains` would let one
/// subject's refusal satisfy an assertion about a different one.
async fn expect_permissions_violation(rx: &mut mpsc::UnboundedReceiver<String>, subject: &str) {
    let needle = format!("\"{subject}\"");
    let deadline = tokio::time::Instant::now() + VIOLATION_BUDGET;
    let mut seen: Vec<String> = Vec::new();
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Some(event)) if event.to_lowercase().contains("permissions violation") && event.contains(&needle) => return,
            Ok(Some(event)) => seen.push(event),
            Ok(None) => panic!("event stream closed before a permissions violation for {subject}; saw {seen:?}"),
            Err(_) => panic!("no permissions violation for {subject} within {VIOLATION_BUDGET:?} — the grant is WIDER than intended; saw {seen:?}"),
        }
    }
}

/// `id` is caller-supplied and must be distinct per publish: it becomes `Nats-Msg-Id`, and
/// JetStream would otherwise collapse two events into one dedup hit. `Uuid::from_u128` rather than
/// a v4/v7 constructor because the workspace pins `uuid` with **no features** (a v7 rng pulls
/// `getrandom`, which the wasm binding's `repo:wasm-getrandom-free` gate forbids).
fn event(id: u128, et: EventType) -> DomainEvent {
    DomainEvent {
        id: Uuid::from_u128(id),
        event_type: et,
        schema_version: 1,
        aggregate_prn: "prn:pgs:iam:::principal/00000000-0000-0000-0000-0000000000aa".to_string(),
        actor_prn: None,
        occurred_at: Utc::now(),
        payload: serde_json::json!({"kind": "user"}),
        correlation_id: None,
    }
}

/// Provisions the stream and the filtered durable exactly as `ops/nats/provision.sh` does, using
/// the provisioner identity — because neither service identity can, which is the point of D5.
async fn provision(fixture: &Fixture) {
    let (client, _events) = client_for(fixture, &fixture.provisioner, PROVISIONER_INBOX).await;
    let js = jetstream::new(client);
    let stream = js
        .get_or_create_stream(jetstream::stream::Config {
            name: STREAM.to_string(),
            subjects: vec!["iam.>".to_string()],
            retention: jetstream::stream::RetentionPolicy::Limits,
            storage: jetstream::stream::StorageType::File,
            duplicate_window: Duration::from_secs(3_600),
            max_age: Duration::from_secs(604_800),
            num_replicas: 1,
            ..Default::default()
        })
        .await
        .expect("the provisioner grant must cover stream creation");

    stream
        .get_or_create_consumer(
            DURABLE,
            jetstream::consumer::pull::Config {
                durable_name: Some(DURABLE.to_string()),
                filter_subjects: vec![
                    "iam.role.granted".to_string(),
                    "iam.role.revoked".to_string(),
                    "iam.api_key.revoked".to_string(),
                    "iam.principal.archived".to_string(),
                    "iam.policy.put".to_string(),
                    "iam.policy.deleted".to_string(),
                ],
                ..Default::default()
            },
        )
        .await
        .expect("the provisioner grant must cover consumer creation");
}

/// Sufficiency: the committed publisher grant must cover stream ensure plus a publish on every
/// subject this service can emit.
///
/// **What the `EventType::ALL` loop does and does not buy.** It does NOT add per-subject permission
/// coverage: the publisher's grant is the `iam.>` wildcard, so any variant whose wire string stays
/// under `iam.` is permitted by construction and a single publish would prove the same thing. What
/// the loop actually earns is (a) the ack path and the stream's `subjects` filter exercised once per
/// variant, and (b) a variant that ever renders OUTSIDE the `iam.` prefix — the one case the
/// wildcard does not absorb, and the one that would otherwise ship silently unpublishable. Read it
/// as cheap breadth, not as the guarantee that a ninth event type is permitted.
///
/// The ack path is the substantive assertion here, and it is why a "write-only" publisher still
/// needs a `subscribe` grant at all: `publish` returns `Ok` only after JetStream's ack lands on the
/// client's inbox, so an inbox prefix that did not match the grant would time out every iteration.
#[tokio::test]
async fn the_publisher_grant_covers_ensure_and_every_event_subject() {
    let Some(fixture) = start_fixture("nats-server.conf", Vec::new()).await else { return };
    let publisher = NatsEventPublisher::connect(&cfg_for(&fixture, &fixture.publisher, PUBLISHER_INBOX))
        .await
        .expect("the committed publisher grant must cover get_or_create_stream and its config verification");

    for (i, et) in EventType::ALL.into_iter().enumerate() {
        publisher
            .publish(&event(i as u128 + 1, et))
            .await
            .unwrap_or_else(|e| panic!("publishing {} must be permitted: {e}", et.as_wire()));
    }
}

/// The publisher must not be able to READ the graph it writes — the whole point of SMA-493 §1.1.
#[tokio::test]
async fn the_publisher_cannot_subscribe_to_the_event_stream() {
    let Some(fixture) = start_fixture("nats-server.conf", Vec::new()).await else { return };
    let (client, mut events) = client_for(&fixture, &fixture.publisher, PUBLISHER_INBOX).await;

    // A denied subscribe still returns Ok: the refusal arrives asynchronously.
    let _sub = client.subscribe("iam.>").await.expect("subscribe is accepted locally, then refused by the server");
    client.flush().await.expect("flush");
    expect_permissions_violation(&mut events, "iam.>").await;
}

/// SMA-471 D7 made non-reconciliation deliberate; these two grants are that decision enforced at
/// the broker rather than merely intended in the code. The stream is ensured first so this is
/// "the stream exists and still cannot be destroyed", not "there was nothing to destroy".
#[tokio::test]
async fn the_publisher_cannot_delete_or_purge_the_stream() {
    let Some(fixture) = start_fixture("nats-server.conf", Vec::new()).await else { return };
    NatsEventPublisher::connect(&cfg_for(&fixture, &fixture.publisher, PUBLISHER_INBOX))
        .await
        .expect("ensure the stream first");
    let (client, mut events) = client_for(&fixture, &fixture.publisher, PUBLISHER_INBOX).await;

    expect_publish_denied(&client, &mut events, "$JS.API.STREAM.DELETE.IAM_EVENTS").await;
    expect_publish_denied(&client, &mut events, "$JS.API.STREAM.PURGE.IAM_EVENTS").await;
}

/// The third firehose route: a direct message get reads any message in the stream regardless of
/// any consumer filter, so it must be denied to both service identities.
#[tokio::test]
async fn neither_service_identity_can_direct_get_stream_messages() {
    let Some(fixture) = start_fixture("nats-server.conf", Vec::new()).await else { return };
    NatsEventPublisher::connect(&cfg_for(&fixture, &fixture.publisher, PUBLISHER_INBOX))
        .await
        .expect("ensure the stream first");

    for (identity, prefix) in [(&fixture.publisher, PUBLISHER_INBOX), (&fixture.consumer, CONSUMER_INBOX)] {
        let (client, mut events) = client_for(&fixture, identity, prefix).await;
        expect_publish_denied(&client, &mut events, "$JS.API.STREAM.MSG.GET.IAM_EVENTS").await;
        expect_publish_denied(&client, &mut events, "$JS.API.DIRECT.GET.IAM_EVENTS").await;
    }
}

/// Sufficiency for SMA-492: pull from the provisioned durable and ack.
///
/// **`double_ack`, not `ack`** — and that is a correctness requirement, not a preference.
/// `Message::ack` is fire-and-forget (`message.rs:285`): it publishes to the reply subject and
/// returns `Ok` without waiting for anything, so a `$JS.ACK…` grant that had been REMOVED would
/// produce exactly the same `Ok(())` and this assertion would prove nothing. `double_ack` publishes
/// the same ack with a reply inbox and waits for the server's confirmation, so a denial surfaces as
/// a timeout error instead of passing silently. Same subject, same permission — only the proof
/// differs.
#[tokio::test]
async fn the_consumer_grant_covers_pulling_and_acking_from_the_provisioned_durable() {
    let Some(fixture) = start_fixture("nats-server.conf", Vec::new()).await else { return };
    provision(&fixture).await;

    let publisher = NatsEventPublisher::connect(&cfg_for(&fixture, &fixture.publisher, PUBLISHER_INBOX)).await.expect("connect publisher");
    publisher.publish(&event(1, EventType::RoleRevoked)).await.expect("publish a filtered event");

    let (client, _events) = client_for(&fixture, &fixture.consumer, CONSUMER_INBOX).await;
    let js = jetstream::new(client);
    let consumer: jetstream::consumer::PullConsumer = js
        .get_consumer_from_stream(DURABLE, STREAM)
        .await
        .expect("the consumer grant must cover CONSUMER.INFO on its own durable");

    let mut batch = consumer.fetch().max_messages(1).messages().await.expect("the consumer grant must cover MSG.NEXT");
    let msg = tokio::time::timeout(Duration::from_secs(10), batch.next())
        .await
        .expect("a filtered event must be delivered")
        .expect("stream yields a message")
        .expect("message is Ok");
    assert_eq!(msg.subject.as_str(), "iam.role.revoked");
    msg.double_ack().await.expect("the consumer grant must cover $JS.ACK on its own durable");
}

/// The control that makes D5's filter binding. The consumer is narrowed by the PRE-PROVISIONED
/// durable's `filter_subjects`, not by any subject permission — pull deliveries arrive on its inbox,
/// never on `iam.*` — so that narrowing is only binding while the consumer cannot run a CREATE verb
/// against the stream. nats-server 2.10 lets CREATE **update** an existing consumer's
/// `filter_subjects`, so a consumer that can CREATE can rewrite its own filter to `iam.>` and read
/// the entire authorization change graph using nothing but the `MSG.NEXT` grant it already holds.
///
/// Four subjects, in two pairs, because they fail differently:
///
/// - The `wide-open` / bare forms catch a grant that named the verb too loosely. async-nats builds
///   `CONSUMER.CREATE.{stream}.{name}` (`context.rs:1512`), so a grant written as
///   `CONSUMER.CREATE.*` would match neither that nor the legacy `CONSUMER.DURABLE.CREATE` form —
///   the NAMED form has to be tested, not just the bare one.
/// - **The own-durable forms are the ones a convenience wildcard would open**, and they are the
///   reason this test is not already covered by the three above. The consumer's allow-list is
///   currently fully enumerated (`CONSUMER.MSG.NEXT.…gateway-cache-invalidator`,
///   `CONSUMER.INFO.…gateway-cache-invalidator`), so the property holds today. The natural future
///   edit — collapsing those two into `$JS.API.CONSUMER.*.IAM_EVENTS.gateway-cache-invalidator` —
///   leaves all three `wide-open`/bare subjects denied and would keep this test green while handing
///   the consumer CREATE on its own durable, which is the whole escalation. Asserting the
///   own-durable name is what makes that edit fail here instead of in production.
#[tokio::test]
async fn the_consumer_cannot_create_a_wider_consumer_in_any_form() {
    let Some(fixture) = start_fixture("nats-server.conf", Vec::new()).await else { return };
    provision(&fixture).await;
    let (client, mut events) = client_for(&fixture, &fixture.consumer, CONSUMER_INBOX).await;

    for subject in [
        "$JS.API.CONSUMER.CREATE.IAM_EVENTS.wide-open",
        "$JS.API.CONSUMER.CREATE.IAM_EVENTS",
        "$JS.API.CONSUMER.DURABLE.CREATE.IAM_EVENTS.wide-open",
        // Its OWN durable — the form a `CONSUMER.*.IAM_EVENTS.gateway-cache-invalidator` wildcard
        // would grant while every subject above stayed denied.
        "$JS.API.CONSUMER.CREATE.IAM_EVENTS.gateway-cache-invalidator",
        "$JS.API.CONSUMER.DURABLE.CREATE.IAM_EVENTS.gateway-cache-invalidator",
    ] {
        expect_publish_denied(&client, &mut events, subject).await;
    }
}

/// The consumer is a reader of ONE filtered durable, not of the stream and not of the account: it
/// can neither subscribe to the event subjects directly nor forge an event onto them.
#[tokio::test]
async fn the_consumer_cannot_subscribe_to_or_forge_events() {
    let Some(fixture) = start_fixture("nats-server.conf", Vec::new()).await else { return };
    let (client, mut events) = client_for(&fixture, &fixture.consumer, CONSUMER_INBOX).await;

    let _sub = client.subscribe("iam.>").await.expect("accepted locally, refused by the server");
    client.flush().await.expect("flush");
    expect_permissions_violation(&mut events, "iam.>").await;

    expect_publish_denied(&client, &mut events, "iam.role.granted").await;
}

/// **The claim D4 actually rests on**, and the only case here that is about the inbox space rather
/// than about a subject a service was never meant to touch.
///
/// This is NOT redundant with the other subscribe denials. `iam.>` and `$JS.API…` are subjects
/// neither identity has any business reading, so denying them is uncontroversial. An inbox is the
/// opposite: it is the one subject space every client MUST be able to subscribe to, because
/// JetStream acks and pull-consumer deliveries both land there and no request completes without
/// them. That is exactly why a shared `_INBOX.>` grant is dangerous — inside a single account it
/// would let `gateway-consumer` subscribe to the publisher's ack inbox, and `iam-publisher` to the
/// consumer's message deliveries, reading in full the event stream neither is allowed to read
/// directly. Per-user prefixes (`subjects.env`'s `*_INBOX_PREFIX`) are the only way to close that,
/// and until this test existed nothing proved the prefixes were ENFORCED rather than merely
/// distinct.
///
/// Asserted in both directions, so widening either identity's `subscribe` grant to a bare
/// `_INBOX.>` — the natural "just make it work" fix when an ack times out — fails here.
#[tokio::test]
async fn neither_service_identity_can_subscribe_to_the_others_inbox() {
    let Some(fixture) = start_fixture("nats-server.conf", Vec::new()).await else { return };

    let (consumer_client, mut consumer_events) = client_for(&fixture, &fixture.consumer, CONSUMER_INBOX).await;
    let _consumer_sub = consumer_client.subscribe("_INBOX_IAM_PUB.>").await.expect("accepted locally, refused by the server");
    consumer_client.flush().await.expect("flush");
    expect_permissions_violation(&mut consumer_events, "_INBOX_IAM_PUB.>").await;

    let (publisher_client, mut publisher_events) = client_for(&fixture, &fixture.publisher, PUBLISHER_INBOX).await;
    let _publisher_sub = publisher_client.subscribe("_INBOX_GW.>").await.expect("accepted locally, refused by the server");
    publisher_client.flush().await.expect("flush");
    expect_permissions_violation(&mut publisher_events, "_INBOX_GW.>").await;
}
