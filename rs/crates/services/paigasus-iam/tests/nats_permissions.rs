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
//! Docker gating is the single policy owned by `tests/support/docker.rs`'s `start_or_skip`
//! (SMA-538), not restated here.

use std::path::{Path, PathBuf};
use std::time::Duration;

use async_nats::jetstream;
use chrono::Utc;
use paigasus_iam::adapters::events::{NatsEventPublisher, NatsPublisherError};
use paigasus_iam::config::{PublisherBackend, PublisherConfig};
use paigasus_iam_core::{DomainEvent, EventPublisher, EventType};
use testcontainers::core::WaitFor;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};
use tokio::sync::mpsc;
use tokio_stream::StreamExt;
use uuid::Uuid;

// This file has no `mod support;` — including `support/docker.rs` directly keeps it that way,
// pulling in one small standalone file rather than the whole support surface (SMA-538).
#[path = "support/docker.rs"]
mod docker;

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
    gw_publisher: Identity,
    gw_reader: Identity,
    gw_provisioner: Identity,
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
    let gw_publisher = mint(dir.path(), "gateway-decision-publisher");
    let gw_reader = mint(dir.path(), "gateway-decision-reader");
    let gw_provisioner = mint(dir.path(), "gateway-decision-provisioner");
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
        .replace("{{PROVISIONER_NKEY}}", &provisioner.public_key)
        .replace("{{GW_DEC_PUBLISHER_NKEY}}", &gw_publisher.public_key)
        .replace("{{GW_DEC_READER_NKEY}}", &gw_reader.public_key)
        .replace("{{GW_DEC_PROVISIONER_NKEY}}", &gw_provisioner.public_key);

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

    let node = docker::start_or_skip(image, "nats_permissions").await?;
    let port = docker::mapped_port(&node, 4222, "nats_permissions").await;

    Some(Fixture {
        _node: node,
        _dir: dir,
        url: format!("nats://127.0.0.1:{port}"),
        publisher,
        consumer,
        provisioner,
        gw_publisher,
        gw_reader,
        gw_provisioner,
    })
}

/// A publisher config pointed at the fixture, authenticating as `identity`.
fn cfg_for(fixture: &Fixture, identity: &Identity, inbox_prefix: &str) -> PublisherConfig {
    PublisherConfig {
        backend: PublisherBackend::Nats,
        url: Some(fixture.url.clone().into()),
        credentials_file: Some(identity.seed_path.to_string_lossy().to_string()),
        inbox_prefix: Some(inbox_prefix.to_string()),
        allow_insecure_broker: true,
        ..PublisherConfig::default()
    }
}

/// A raw client for `identity`, with its [`async_nats::Event`]s piped into the returned channel so
/// denials can be asserted on rather than inferred from a timeout.
///
/// Authenticates through `paigasus_nats_creds::auth_from_credentials` — the shared crate whose
/// callback body `NatsEventPublisher::connect` (and the gateway's tool-policy subscription) also
/// installs — so a fixture client and the production client cannot diverge in how they present a
/// credential.
async fn client_for(fixture: &Fixture, identity: &Identity, inbox_prefix: &str) -> (async_nats::Client, mpsc::UnboundedReceiver<String>) {
    let (tx, rx) = mpsc::unbounded_channel();
    let path = identity.seed_path.to_string_lossy().to_string();
    let client = async_nats::ConnectOptions::with_auth_callback(move |nonce| {
        let path = path.clone();
        async move { paigasus_nats_creds::auth_from_credentials(&path, &nonce).await }
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

/// Forces `client` to reconnect and blocks until the disconnect-then-reconnect cycle has actually
/// completed, observed as a `"disconnected"` [`async_nats::Event`] followed by a `"connected"`
/// one on `rx`.
///
/// **Why this is not just `client.force_reconnect().await`.** That call only confirms the
/// reconnect command was accepted into an internal channel — its own doc: "does not wait for
/// connection to be re-established". A `.publish()` issued right after it is a genuine race: it
/// can be queued and sent over the OLD, still-open connection before the connector has even begun
/// tearing it down, silently testing the ORIGINAL identity instead of the rotated one. This was
/// not a hypothetical — it is exactly what happened on the first run of the two rotation tests
/// below (see the report): both passed the reconnect but the publish that followed landed before
/// the new authentication took effect, so no violation (or, in the happy-path test, no delivery)
/// was ever observed within the budget.
async fn force_reconnect_and_await(client: &async_nats::Client, rx: &mut mpsc::UnboundedReceiver<String>) {
    // Drain any already-queued events first: a genuine blip between `client_for` and this call
    // would otherwise leave a stale `disconnected`/`connected` pair sitting in the channel, and
    // the loop below could match against THAT pair and return before the forced cycle below has
    // actually happened.
    while rx.try_recv().is_ok() {}
    client.force_reconnect().await.expect("force_reconnect is accepted locally");
    let deadline = tokio::time::Instant::now() + VIOLATION_BUDGET;
    let mut disconnected = false;
    let mut seen: Vec<String> = Vec::new();
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Some(event)) if !disconnected && event == "disconnected" => disconnected = true,
            Ok(Some(event)) if disconnected && event == "connected" => return,
            Ok(Some(event)) => seen.push(event),
            Ok(None) => panic!("event stream closed before the reconnect completed; saw {seen:?}"),
            Err(_) => panic!("the client never completed a disconnect-then-reconnect cycle within {VIOLATION_BUDGET:?}; saw {seen:?}"),
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
/// as cheap breadth, not as the guarantee that a specific event type is permitted — `EventType::ALL`
/// is 25 variants now (SMA-606, SMA-775), not the eight it was when this note was first written.
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

/// The control that makes D5's filter binding. The consumer's PULL path is narrowed by the
/// PRE-PROVISIONED durable's `filter_subjects`, not by any subject permission — pull deliveries
/// arrive on its inbox, never on `iam.*`. (Its one subject-level subscribe grant, the SMA-752 core
/// subscription on `iam.policy.>`, is a separate path and does not narrow this one; it is the only
/// subscribe grant outside the inbox.) So that narrowing is only binding while the consumer cannot run a CREATE verb
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
/// can neither subscribe to the event subjects as a whole nor forge an event onto them. Its only
/// direct subscription is `iam.policy.>` (SMA-752); see the three policy tests below.
#[tokio::test]
async fn the_consumer_cannot_subscribe_to_or_forge_events() {
    let Some(fixture) = start_fixture("nats-server.conf", Vec::new()).await else { return };
    let (client, mut events) = client_for(&fixture, &fixture.consumer, CONSUMER_INBOX).await;

    let _sub = client.subscribe("iam.>").await.expect("accepted locally, refused by the server");
    client.flush().await.expect("flush");
    expect_permissions_violation(&mut events, "iam.>").await;

    expect_publish_denied(&client, &mut events, "iam.role.granted").await;
}

/// SMA-752 D24 (Q1-A): the measurement the gateway's policy-cache invalidation rests on. The
/// consumer's ONE subject-level subscribe grant, `iam.policy.>`, must be enough for a core
/// subscriber to receive a JetStream publish on `iam.policy.put`. A JetStream publish is an
/// ordinary core message that the stream also captures, so every core subscriber of the subject
/// gets it. The publish goes through [`NatsEventPublisher`], the production path, and its `Ok` means
/// that the stream acked it.
#[tokio::test]
async fn the_consumer_core_subscription_receives_a_jetstream_policy_publish() {
    let Some(fixture) = start_fixture("nats-server.conf", Vec::new()).await else { return };
    provision(&fixture).await;

    let (client, mut events) = client_for(&fixture, &fixture.consumer, CONSUMER_INBOX).await;
    let mut sub = client.subscribe("iam.policy.>").await.expect("subscribe is accepted locally");
    // The flush is a PING/PONG round trip, so the server has the SUB before the publish below.
    client.flush().await.expect("flush");

    let publisher = NatsEventPublisher::connect(&cfg_for(&fixture, &fixture.publisher, PUBLISHER_INBOX)).await.expect("connect publisher");
    publisher.publish(&event(1, EventType::PolicyPut)).await.expect("a JetStream publish on iam.policy.put is acked");

    match tokio::time::timeout(VIOLATION_BUDGET, sub.next()).await {
        Ok(Some(msg)) => assert_eq!(msg.subject.as_str(), "iam.policy.put"),
        Ok(None) => panic!("the subscription closed; events: {:?}", drain(&mut events)),
        Err(_) => panic!("no iam.policy.put delivery within {VIOLATION_BUDGET:?}; events: {:?}", drain(&mut events)),
    }
}

/// SMA-775: the three tool policy events sit under `iam.policy.>`, so the consumer's one core
/// subscription must receive all of them from the production publisher.
#[tokio::test]
async fn the_consumer_core_subscription_receives_each_tool_policy_event() {
    let Some(fixture) = start_fixture("nats-server.conf", Vec::new()).await else { return };
    provision(&fixture).await;

    let (client, mut events) = client_for(&fixture, &fixture.consumer, CONSUMER_INBOX).await;
    let mut sub = client.subscribe("iam.policy.>").await.expect("subscribe is accepted locally");
    client.flush().await.expect("flush");

    let publisher = NatsEventPublisher::connect(&cfg_for(&fixture, &fixture.publisher, PUBLISHER_INBOX)).await.expect("connect publisher");
    let types = [EventType::ToolPolicySetPublished, EventType::ToolFloorPublished, EventType::ToolPolicySettingsChanged];
    for (i, et) in types.into_iter().enumerate() {
        publisher
            .publish(&event(i as u128 + 1, et))
            .await
            .unwrap_or_else(|e| panic!("publishing {} must be acked: {e}", et.as_wire()));
    }

    let mut got = Vec::new();
    for _ in 0..types.len() {
        match tokio::time::timeout(VIOLATION_BUDGET, sub.next()).await {
            Ok(Some(msg)) => got.push(msg.subject.to_string()),
            Ok(None) => panic!("the subscription closed; events: {:?}", drain(&mut events)),
            Err(_) => panic!("only {got:?} arrived within {VIOLATION_BUDGET:?}; events: {:?}", drain(&mut events)),
        }
    }
    for et in types {
        assert!(got.iter().any(|s| s == et.as_wire()), "{} was not delivered; got {got:?}", et.as_wire());
    }
}

/// Every event the client has seen so far, for a panic message.
fn drain(rx: &mut mpsc::UnboundedReceiver<String>) -> Vec<String> {
    let mut seen = Vec::new();
    while let Ok(event) = rx.try_recv() {
        seen.push(event);
    }
    seen
}

/// SMA-752 § 6.3.5: the policy grant is `iam.policy.>` and nothing wider. The other IAM subjects
/// stay closed to a core subscription: a key revocation and a role grant are not the gateway's
/// to read directly. `iam.>` itself is asserted in `the_consumer_cannot_subscribe_to_or_forge_events`.
#[tokio::test]
async fn the_consumer_cannot_subscribe_to_other_iam_subjects() {
    let Some(fixture) = start_fixture("nats-server.conf", Vec::new()).await else { return };
    let (client, mut events) = client_for(&fixture, &fixture.consumer, CONSUMER_INBOX).await;

    for subject in ["iam.api_key.>", "iam.role.granted"] {
        let _sub = client.subscribe(subject).await.expect("accepted locally, refused by the server");
        client.flush().await.expect("flush");
        expect_permissions_violation(&mut events, subject).await;
    }
}

/// SMA-752 § 6.3.5 (D4, Q1-A, Sven): the new grant is a core subscribe only. It adds no publish
/// right (so no forged policy event), no CREATE or UPDATE form on the durable, and no subscribe
/// to the JetStream API space. The durable's filter therefore stays binding.
#[tokio::test]
async fn the_policy_grant_adds_no_jetstream_right() {
    let Some(fixture) = start_fixture("nats-server.conf", Vec::new()).await else { return };
    provision(&fixture).await;
    let (client, mut events) = client_for(&fixture, &fixture.consumer, CONSUMER_INBOX).await;

    for subject in [
        "iam.policy.put",
        "$JS.API.CONSUMER.CREATE.IAM_EVENTS.gateway-cache-invalidator",
        "$JS.API.CONSUMER.UPDATE.IAM_EVENTS.gateway-cache-invalidator",
        // The filtered-create form (`CREATE.<stream>.<consumer>.<filter>`), with a policy filter.
        "$JS.API.CONSUMER.CREATE.IAM_EVENTS.gateway-cache-invalidator.iam.policy.put",
    ] {
        expect_publish_denied(&client, &mut events, subject).await;
    }

    let _sub = client.subscribe("$JS.>").await.expect("accepted locally, refused by the server");
    client.flush().await.expect("flush");
    expect_permissions_violation(&mut events, "$JS.>").await;
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

/// Mints a CA and a server certificate signed by it, with an IP SAN for 127.0.0.1 (the tests dial
/// a mapped host port). Nothing is committed: `rcgen` is already a dev-dependency here for the
/// mock IdP, and a per-run key pair keeps certificate material out of git entirely.
fn mint_tls(dir: &std::path::Path) -> (Vec<u8>, Vec<u8>, PathBuf) {
    use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, Issuer, KeyPair, SanType};
    use std::net::{IpAddr, Ipv4Addr};

    let mut ca_params = CertificateParams::new(Vec::new()).expect("ca params");
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params.distinguished_name.push(DnType::CommonName, "paigasus-nats-test-ca");
    let ca_key = KeyPair::generate().expect("ca key");
    let ca_cert = ca_params.self_signed(&ca_key).expect("self-signed ca");
    // rcgen 0.14 takes an `Issuer` instead of a `(&Certificate, &KeyPair)` pair. `Issuer::new`
    // consumes both the params and the key, so the CA must be self-signed first.
    let ca_issuer = Issuer::new(ca_params, ca_key);

    let mut srv_params = CertificateParams::new(vec!["localhost".to_string()]).expect("server params");
    srv_params.subject_alt_names.push(SanType::IpAddress(IpAddr::V4(Ipv4Addr::LOCALHOST)));
    let srv_key = KeyPair::generate().expect("server key");
    let srv_cert = srv_params.signed_by(&srv_key, &ca_issuer).expect("server cert signed by the ca");

    let ca_path = dir.join("ca.pem");
    std::fs::write(&ca_path, ca_cert.pem()).expect("write ca pem");
    (srv_cert.pem().into_bytes(), srv_key.serialize_pem().into_bytes(), ca_path)
}

/// D7's field is what makes a private-CA broker dialable at all — without it async-nats falls back
/// to the system trust store (`tls.rs:61`), which will never contain a per-run CA.
#[tokio::test]
async fn the_publisher_connects_over_tls_with_a_named_ca_bundle() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (cert_pem, key_pem, ca_path) = mint_tls(dir.path());
    let extra = vec![("/etc/nats/server-cert.pem".to_string(), cert_pem), ("/etc/nats/server-key.pem".to_string(), key_pem)];

    let Some(fixture) = start_fixture("nats-server-tls.conf", extra).await else { return };

    let mut cfg = cfg_for(&fixture, &fixture.publisher, "_INBOX_IAM_PUB");
    cfg.url = Some(fixture.url.replace("nats://", "tls://").into());
    cfg.root_ca_bundle = Some(ca_path.to_string_lossy().to_string());

    let publisher = NatsEventPublisher::connect(&cfg).await.expect("a tls:// connection with a named CA must succeed");
    publisher.publish(&event(1, EventType::RoleGranted)).await.expect("publish over TLS");
}

/// The negative control. Without it the test above would pass even if `root_ca_bundle` were
/// ignored entirely and verification silently disabled.
#[tokio::test]
async fn a_tls_connection_without_the_ca_bundle_fails_verification() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (cert_pem, key_pem, _ca_path) = mint_tls(dir.path());
    let extra = vec![("/etc/nats/server-cert.pem".to_string(), cert_pem), ("/etc/nats/server-key.pem".to_string(), key_pem)];

    let Some(fixture) = start_fixture("nats-server-tls.conf", extra).await else { return };

    let mut cfg = cfg_for(&fixture, &fixture.publisher, "_INBOX_IAM_PUB");
    cfg.url = Some(fixture.url.replace("nats://", "tls://").into());
    // root_ca_bundle deliberately unset: the per-run CA is in no system trust store.
    let err = NatsEventPublisher::connect(&cfg).await.expect_err("a private CA must not verify against the system trust store");
    // `expect_err` alone would also pass for an unrelated connect failure (a dead container, a
    // CI-contention timeout) — asserting the *cause* is what makes this a proof about
    // certificate verification specifically. `NatsPublisherError::Connect`'s `Display` is the
    // fixed literal `"nats connect failed"` (see the enum's doc comment); the rustls cause only
    // shows up in `Debug`, which renders `InvalidCertificate(UnknownIssuer)` here.
    assert!(matches!(err, NatsPublisherError::Connect(_)), "expected a connect failure, got {err:?}");
    assert!(format!("{err:?}").contains("InvalidCertificate"), "expected a certificate-verification failure, got {err:?}");
}

/// And a bundle that is well-formed but wrong must also fail — proving the bundle is actually
/// consulted rather than merely present.
#[tokio::test]
async fn a_tls_connection_with_an_unrelated_ca_bundle_fails_verification() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (cert_pem, key_pem, _ca_path) = mint_tls(dir.path());
    let extra = vec![("/etc/nats/server-cert.pem".to_string(), cert_pem), ("/etc/nats/server-key.pem".to_string(), key_pem)];

    let Some(fixture) = start_fixture("nats-server-tls.conf", extra).await else { return };

    let other = tempfile::tempdir().expect("tempdir");
    let (_c, _k, unrelated_ca) = mint_tls(other.path());

    let mut cfg = cfg_for(&fixture, &fixture.publisher, "_INBOX_IAM_PUB");
    cfg.url = Some(fixture.url.replace("nats://", "tls://").into());
    cfg.root_ca_bundle = Some(unrelated_ca.to_string_lossy().to_string());
    let err = NatsEventPublisher::connect(&cfg).await.expect_err("an unrelated CA must not verify the broker's certificate");
    // Same reasoning as the no-bundle control above. Empirically this renders
    // `InvalidCertificate(BadSignature)` rather than `UnknownIssuer` — both `mint_tls` CAs share a
    // CommonName, so the issuer name matches but the signature does not — but the shared
    // `InvalidCertificate` marker is what both controls are asserting on.
    assert!(matches!(err, NatsPublisherError::Connect(_)), "expected a connect failure, got {err:?}");
    assert!(format!("{err:?}").contains("InvalidCertificate"), "expected a certificate-verification failure, got {err:?}");
}

/// SMA-493 D8's regression net, and it must be able to detect the regression: rotate a LIVE,
/// already-authenticated client's credential file to a DIFFERENT declared identity, force that
/// SAME client to reconnect, and require the reconnect to adopt the new identity's grants.
///
/// **Why a live-client reconnect, not a second `connect()` call — a design mistake made and
/// caught in review.** An earlier version of this test called `NatsEventPublisher::connect`
/// twice. That does not discriminate: `with_credentials_file` (`options.rs:429-431` →
/// `credentials()` at `:520-528`) reads and parses the file inside `ConnectOptions`'s own
/// CONSTRUCTOR, freezing an `Arc<KeyPair>` into the signing closure it returns — but that cache
/// belongs to ONE `ConnectOptions` build. A brand-new `connect()` call constructs a brand-new
/// `ConnectOptions` and therefore reads the CURRENT file regardless of which implementation is in
/// force, so calling `connect()` twice cannot tell "cached across this client's own reconnects"
/// apart from "read fresh every attempt". The property D8 actually fixes only shows up on a
/// genuine reconnect of an ALREADY-connected client: `Client::force_reconnect`
/// (`client.rs:957`) re-triggers the same auth process its own doc names as the tool "to
/// re-trigger the auth-callback" — exactly what happens when the underlying TCP connection drops
/// and async-nats reconnects on its own.
///
/// `client_for` is used here rather than `NatsEventPublisher::connect` for two reasons: it hands
/// back the raw [`async_nats::Client`] `force_reconnect` needs (which `NatsEventPublisher` does
/// not expose), and its callback body is `auth_from_credentials` — the exact function
/// `NatsEventPublisher::connect` installs — so this test still exercises production code, not a
/// diverged test double.
///
/// The publisher's `iam.>` grant is already proven sufficient elsewhere in this file
/// (`the_publisher_grant_covers_ensure_and_every_event_subject`), so no separate "before"
/// assertion is needed here — the interesting behaviour is entirely in what happens AFTER the
/// rotation.
///
/// Under a per-CLIENT cached credential — the real pre-SMA-493 shape, and the shape any future
/// regression would take (hoisting the read/parse out of the callback into `ConnectOptions`
/// construction while keeping the bare-nkey parser) — this assertion is FALSE: the reconnect
/// re-authenticates as the ORIGINAL, still-cached publisher identity, the publish is permitted,
/// no violation ever arrives, and the test times out instead of catching anything. Verified via
/// an isolated per-client-cache mutation — see the report for the failing output.
#[tokio::test]
async fn a_rotated_credential_is_honoured_on_the_clients_own_reconnect() {
    let Some(fixture) = start_fixture("nats-server.conf", Vec::new()).await else { return };
    let (client, mut events) = client_for(&fixture, &fixture.publisher, PUBLISHER_INBOX).await;

    // Same path, different (narrower) identity: the consumer's seed, which cannot publish iam.*
    // at all.
    let consumer_seed = std::fs::read_to_string(&fixture.consumer.seed_path).expect("consumer seed");
    std::fs::write(&fixture.publisher.seed_path, &consumer_seed).expect("rotate the credential in place");

    // Force the SAME already-authenticated client to reconnect — the real code path a dropped
    // TCP connection's automatic reconnect takes, per `Client::force_reconnect`'s own doc — and
    // wait for that cycle to actually finish before publishing (see `force_reconnect_and_await`'s
    // doc for why publishing right after `force_reconnect()` alone is a race).
    force_reconnect_and_await(&client, &mut events).await;

    client.publish("iam.role.granted", "{}".into()).await.expect("a denied publish is still accepted locally");
    client.flush().await.expect("flush");
    expect_permissions_violation(&mut events, "iam.role.granted").await;
}

/// The happy half, and the actual operational claim SMA-493 D8 exists for: a credential rotated
/// to a NEW, VALID identity is picked up by the running process without a restart — the outage
/// this issue addresses is a service that could recover from a rotation but doesn't, not one that
/// merely notices a bad one.
///
/// Starts as the CONSUMER, whose grant excludes `iam.*` entirely, and confirms that denial as the
/// baseline — so a later success can only be explained by the rotated (publisher) identity
/// actually being in force, not by some other misconfiguration. Rotates the consumer's seed
/// FILE, in place, to the publisher's seed, force-reconnects the SAME client (see the sibling
/// test's doc for why a live reconnect and not a second `connect()` call), then publishes again.
///
/// **A CORE publish, not a JetStream request/reply, and verified through a SEPARATE client.**
/// `client_for`'s `custom_inbox_prefix` is fixed at construction (`CONSUMER_INBOX` here) and does
/// not change across a reconnect; the rotated (publisher) identity's own subscribe grant is
/// `_INBOX_IAM_PUB.>`, not `_INBOX_GW.>`, so a `$JS.API` round trip on THIS client would itself be
/// refused by the inbox mismatch (D4) — a false negative unrelated to the property under test.
/// A plain `client.publish` sidesteps that: JetStream's stream engine captures any message
/// published on a subject its `subjects` filter covers, ack or no ack, as long as the PUBLISH
/// itself is permitted. A second, independent, un-rotated provisioner connection then confirms
/// delivery by polling the stream's own message count — a positive signal that fails in the safe
/// direction: if the rotation were never adopted, the count never moves and the bounded poll times
/// out and panics, it does not silently pass.
///
/// Under a per-CLIENT cached credential this assertion is FALSE: the reconnect keeps
/// authenticating as the original (cached) consumer identity, the publish stays denied, the
/// stream's message count never moves, and the poll below times out — verified via the same
/// isolated per-client-cache mutation as the sibling test, see the report.
#[tokio::test]
async fn a_rotated_credential_restores_a_denied_publish_without_a_restart() {
    let Some(fixture) = start_fixture("nats-server.conf", Vec::new()).await else { return };
    // Ensure the stream exists first (via the real production path), so there is somewhere for a
    // later permitted publish to land and be independently verified.
    NatsEventPublisher::connect(&cfg_for(&fixture, &fixture.publisher, PUBLISHER_INBOX))
        .await
        .expect("ensure the stream exists first");

    let (client, mut events) = client_for(&fixture, &fixture.consumer, CONSUMER_INBOX).await;
    expect_publish_denied(&client, &mut events, "iam.role.granted").await;

    // Rotate the FILE in place to the publisher's seed, then force this SAME client to
    // reconnect and wait for that cycle to actually finish (see `force_reconnect_and_await`'s
    // doc for why publishing right after `force_reconnect()` alone is a race).
    let publisher_seed = std::fs::read_to_string(&fixture.publisher.seed_path).expect("publisher seed");
    std::fs::write(&fixture.consumer.seed_path, &publisher_seed).expect("rotate to the publisher identity");
    force_reconnect_and_await(&client, &mut events).await;

    client
        .publish("iam.role.granted", "{}".into())
        .await
        .expect("the rotated (publisher) identity's publish must be accepted locally");
    client.flush().await.expect("flush");

    let (verify_client, _verify_events) = client_for(&fixture, &fixture.provisioner, PROVISIONER_INBOX).await;
    let js = jetstream::new(verify_client);
    let deadline = tokio::time::Instant::now() + VIOLATION_BUDGET;
    loop {
        let mut stream = js.get_stream(STREAM).await.expect("the provisioner grant covers STREAM.INFO");
        let info = stream.info().await.expect("the provisioner grant covers a fresh STREAM.INFO round trip");
        if info.state.messages >= 1 {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the rotated (publisher) identity's publish never reached the stream within {VIOLATION_BUDGET:?} — \
             the reconnect must not have adopted the rotated credential"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

// ---------------------------------------------------------------------------------------------
// The gateway tool decision log account (SMA-755 § 5.10, P-13, P-15, P-16). The calls below are the
// exact calls of the relay (`get_or_create_stream`, `send_publish` with a message id) and of the
// export CLI (`get_stream_no_info`, `create_consumer(OrderedConfig)`).
// ---------------------------------------------------------------------------------------------

const GW_STREAM: &str = "GATEWAY_DECISIONS";
const GW_PUBLISHER_INBOX: &str = "_INBOX_GW_DEC";
const GW_READER_INBOX: &str = "_INBOX_GW_RD";
const GW_PROVISIONER_INBOX: &str = "_INBOX_GW_PROV";

/// The stream config of `ops/nats/subjects.env`, except `max_bytes`: the fixture account holds
/// 256 MiB, and a limit above the account limit is refused by the broker.
fn gw_stream_config() -> jetstream::stream::Config {
    jetstream::stream::Config {
        name: GW_STREAM.to_string(),
        subjects: vec!["gateway.decision.>".to_string()],
        retention: jetstream::stream::RetentionPolicy::Limits,
        storage: jetstream::stream::StorageType::File,
        max_age: Duration::from_secs(2_592_000),
        max_bytes: 134_217_728,
        discard: jetstream::stream::DiscardPolicy::Old,
        duplicate_window: Duration::from_secs(120),
        max_message_size: 65_536,
        num_replicas: 1,
        ..Default::default()
    }
}

/// Publishes one event with a message id, as the relay does, and waits for the PubAck.
async fn gw_publish(js: &jetstream::Context, subject: &str, id: &str) -> jetstream::publish::PublishAck {
    let publish = jetstream::message::PublishMessage::build().message_id(id).payload("{}".into());
    js.send_publish(subject.to_string(), publish)
        .await
        .expect("the publish is accepted locally")
        .await
        .expect("the publisher grant must deliver a PubAck")
}

#[tokio::test]
async fn the_decision_publisher_ensures_the_stream_and_publishes_with_an_ack() {
    let Some(fixture) = start_fixture("nats-server.conf", Vec::new()).await else { return };
    let (client, _events) = client_for(&fixture, &fixture.gw_publisher, GW_PUBLISHER_INBOX).await;
    let js = jetstream::new(client);
    js.get_or_create_stream(gw_stream_config())
        .await
        .expect("the publisher grant must cover STREAM.INFO and STREAM.CREATE under the account limits");
    let subject = format!("gateway.decision.{}", Uuid::from_u128(7));
    let ack = gw_publish(&js, &subject, "decision-1").await;
    assert_eq!(ack.stream, GW_STREAM);
    assert_eq!(ack.sequence, 1);
}

#[tokio::test]
async fn the_decision_publisher_cannot_read_delete_or_touch_iam() {
    let Some(fixture) = start_fixture("nats-server.conf", Vec::new()).await else { return };
    let (client, mut events) = client_for(&fixture, &fixture.gw_publisher, GW_PUBLISHER_INBOX).await;
    let _sub = client.subscribe("gateway.decision.>").await.expect("subscribe is accepted locally, then refused");
    client.flush().await.expect("flush");
    expect_permissions_violation(&mut events, "gateway.decision.>").await;
    expect_publish_denied(&client, &mut events, "$JS.API.STREAM.DELETE.GATEWAY_DECISIONS").await;
    expect_publish_denied(&client, &mut events, "$JS.API.STREAM.PURGE.GATEWAY_DECISIONS").await;
    expect_publish_denied(&client, &mut events, "iam.role.granted").await;
}

#[tokio::test]
async fn the_decision_reader_reads_with_an_ordered_consumer() {
    let Some(fixture) = start_fixture("nats-server.conf", Vec::new()).await else { return };
    let (pclient, _pevents) = client_for(&fixture, &fixture.gw_provisioner, GW_PROVISIONER_INBOX).await;
    jetstream::new(pclient)
        .get_or_create_stream(gw_stream_config())
        .await
        .expect("the provisioner grant must cover stream creation");
    let (wclient, _wevents) = client_for(&fixture, &fixture.gw_publisher, GW_PUBLISHER_INBOX).await;
    let wjs = jetstream::new(wclient);
    let subject = format!("gateway.decision.{}", Uuid::from_u128(9));
    gw_publish(&wjs, &subject, "decision-1").await;
    gw_publish(&wjs, &subject, "decision-2").await;

    let (client, _events) = client_for(&fixture, &fixture.gw_reader, GW_READER_INBOX).await;
    let js = jetstream::new(client);
    let stream = js.get_stream_no_info(GW_STREAM).await.expect("get_stream_no_info makes no network call");
    // The account has `max_mem: 0`: this create fails if an ordered consumer asks for memory storage.
    // `DeliverPolicy::All` here, not `ByStartTime`: the latter needs the `time` crate, and the
    // permission subjects are the same for both.
    let consumer = stream
        .create_consumer(jetstream::consumer::pull::OrderedConfig {
            filter_subject: subject.clone(),
            deliver_policy: jetstream::consumer::DeliverPolicy::All,
            ..Default::default()
        })
        .await
        .expect("an ordered consumer must work under the account limits (max_mem 0) with the reader grants");
    assert_eq!(consumer.cached_info().num_pending, 2);
    let mut messages = consumer.clone().messages().await.expect("start the read");
    for _ in 0..2 {
        let msg = tokio::time::timeout(Duration::from_secs(10), messages.next())
            .await
            .expect("a published event must be delivered")
            .expect("stream yields a message")
            .expect("message is Ok");
        assert_eq!(msg.subject.as_str(), subject);
    }

    // Delete the consumer server side with the reader's own DELETE grant. The ordered consumer must
    // recreate itself (a named CREATE with a filter suffix) and read on.
    let name = consumer.cached_info().name.clone();
    stream.delete_consumer(&name).await.expect("the reader grant must cover CONSUMER.DELETE on its own consumer");
    gw_publish(&wjs, &subject, "decision-3").await;
    let msg = tokio::time::timeout(Duration::from_secs(40), messages.next())
        .await
        .expect("the ordered consumer must recreate itself and deliver the third event")
        .expect("stream yields a message")
        .expect("message is Ok");
    assert_eq!(msg.subject.as_str(), subject);
}

#[tokio::test]
async fn the_decision_reader_cannot_publish_or_purge() {
    let Some(fixture) = start_fixture("nats-server.conf", Vec::new()).await else { return };
    let (client, mut events) = client_for(&fixture, &fixture.gw_reader, GW_READER_INBOX).await;
    expect_publish_denied(&client, &mut events, "gateway.decision.x").await;
    expect_publish_denied(&client, &mut events, "$JS.API.STREAM.PURGE.GATEWAY_DECISIONS").await;
    expect_publish_denied(&client, &mut events, "$JS.API.STREAM.CREATE.X").await;
}

#[tokio::test]
async fn a_gateway_account_user_cannot_see_iam_events() {
    let Some(fixture) = start_fixture("nats-server.conf", Vec::new()).await else { return };
    // The IAM stream exists, in the other account.
    provision(&fixture).await;
    let (client, mut events) = client_for(&fixture, &fixture.gw_reader, GW_READER_INBOX).await;
    let js = jetstream::new(client);
    let stream = js.get_stream_no_info(STREAM).await.expect("get_stream_no_info makes no network call");
    let created = stream
        .create_consumer(jetstream::consumer::pull::OrderedConfig {
            filter_subject: "iam.>".to_string(),
            ..Default::default()
        })
        .await;
    assert!(created.is_err(), "the reader must not create a consumer on IAM_EVENTS");
    expect_permissions_violation(&mut events, "$JS.API.CONSUMER.CREATE.IAM_EVENTS").await;

    // A user with the stream API right in the gateway account still does not find IAM_EVENTS.
    let (pclient, _pevents) = client_for(&fixture, &fixture.gw_provisioner, GW_PROVISIONER_INBOX).await;
    let found = jetstream::new(pclient).get_stream(STREAM).await;
    assert!(found.is_err(), "IAM_EVENTS lives in another account and must not be visible");
}
