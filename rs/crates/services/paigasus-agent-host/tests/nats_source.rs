// SPDX-License-Identifier: Apache-2.0

//! The NATS source against a real JetStream and a real Postgres (spec §9.2, §9.3). The test makes
//! `IAM_EVENTS` and the durable itself, with the flags of `ops/nats/provision.sh`: the host never
//! makes them. The bodies are IAM's own encoding (the Task 1 fixtures).

mod support;

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_nats::jetstream::{self, consumer};
use paigasus_agent_host::adapters::nats::consumer::{BoundChannel, NatsBootError, bind_channels, connect_and_check};
use paigasus_agent_host::adapters::nats::{DURABLE_NAME, STREAM_NAME};
use paigasus_agent_host::adapters::persistence::inbox_repository::PgInboxRepository;
use paigasus_agent_host::config::NatsConfig;
use paigasus_agent_host_core::ports::InboxRepository;
use sea_orm::{ConnectionTrait, DatabaseConnection};
use support::inbox::{insert_inbox, migrated_postgres};
use testcontainers::ContainerAsync;
use testcontainers::ImageExt;
use testcontainers_modules::nats::{Nats, NatsServerCmd};
use testcontainers_modules::postgres::Postgres;
use tokio::sync::watch;
use uuid::Uuid;

/// IAM's encoding of `iam.organization.created` for the org `u(0x500)`.
const FIX_ORG: &[u8] = include_bytes!("../../../libs/paigasus-cloudevents/tests/fixtures/iam/organization_created_trailing_zeros.json");
const SUBJECT: &str = "iam.organization.created";
const FILTER: &str = "iam.organization.>";
const SOURCE: &str = "urn:paigasus:agents:nats:iam-org";
const INBOX: u128 = 0x10;

fn u(n: u128) -> Uuid {
    Uuid::from_u128(n)
}

/// FIX_ORG with an `actorprn` and a `threadid` extension added (R7). The host drops both.
fn fix_org_with_actor_and_thread() -> Vec<u8> {
    let mut v: serde_json::Value = serde_json::from_slice(FIX_ORG).expect("fixture json");
    let o = v.as_object_mut().expect("object");
    o.insert("actorprn".into(), serde_json::json!(format!("prn:pgs:iam:::principal/{}", u(0x77))));
    o.insert("threadid".into(), serde_json::json!(format!("prn:pgs:agents::{}:thread/{}", u(0x500), u(0x78))));
    serde_json::to_vec(&v).expect("bytes")
}

fn nats_config(url: &str, inbox: Uuid) -> NatsConfig {
    serde_json::from_value(serde_json::json!({
        "url": url,
        "stream": STREAM_NAME,
        "durable": DURABLE_NAME,
        "channels": [{ "subject": FILTER, "inbox": inbox, "source": SOURCE }],
    }))
    .expect("nats config")
}

struct Stack {
    _nats: ContainerAsync<Nats>,
    _pg: ContainerAsync<Postgres>,
    url: String,
    js: jetstream::Context,
    db: DatabaseConnection,
    repo: Arc<dyn InboxRepository>,
}

/// NATS with JetStream, the stream and the durable with `filters`, and a migrated Postgres with
/// the inbox `INBOX` of the org `org`.
async fn stack(what: &str, org: u128, filters: &[&str]) -> Option<Stack> {
    let cmd = NatsServerCmd::default().with_jetstream();
    let nats = paigasus_test_docker::start_or_skip(Nats::default().with_cmd(&cmd), what).await?;
    let port = paigasus_test_docker::mapped_port(&nats, 4222, what).await;
    let url = format!("nats://127.0.0.1:{port}");
    let js = jetstream::new(async_nats::connect(&url).await.expect("the test client connects"));
    js.create_stream(jetstream::stream::Config {
        name: STREAM_NAME.to_string(),
        subjects: vec!["iam.>".to_string()],
        ..Default::default()
    })
    .await
    .expect("the stream is made");
    js.create_consumer_on_stream(
        consumer::pull::Config {
            durable_name: Some(DURABLE_NAME.to_string()),
            ack_policy: consumer::AckPolicy::Explicit,
            deliver_policy: consumer::DeliverPolicy::New,
            max_deliver: -1,
            ack_wait: Duration::from_secs(30),
            filter_subjects: filters.iter().map(|s| (*s).to_string()).collect(),
            ..Default::default()
        },
        STREAM_NAME,
    )
    .await
    .expect("the durable is made");
    let (pg, _pg_url, db) = migrated_postgres().await?;
    insert_inbox(&db, u(org), u(INBOX), u(0x11), u(0x12)).await;
    let repo: Arc<dyn InboxRepository> = Arc::new(PgInboxRepository::new(db.clone()));
    Some(Stack {
        _nats: nats,
        _pg: pg,
        url,
        js,
        db,
        repo,
    })
}

/// Bind, check and start the source. The returned sender stops it.
async fn start_source(s: &Stack) -> (watch::Sender<()>, tokio::task::JoinHandle<anyhow::Result<()>>) {
    let cfg = nats_config(&s.url, u(INBOX));
    let channels: Vec<BoundChannel> = bind_channels(&cfg, &*s.repo).await.expect("bind");
    let source = connect_and_check(&cfg, &channels).await.expect("the boot check passes");
    let (tx, rx) = watch::channel(());
    let repo = Arc::clone(&s.repo);
    let task = tokio::spawn(async move { source.run(repo, rx).await });
    (tx, task)
}

async fn publish(js: &jetstream::Context, payload: Vec<u8>) {
    js.publish(SUBJECT, payload.into()).await.expect("publish").await.expect("the PubAck");
}

async fn rows(db: &DatabaseConnection) -> i64 {
    support::scalar_i64(db, "SELECT count(*) AS n FROM inbox_event").await
}

async fn consumer_info(js: &jetstream::Context) -> consumer::Info {
    let mut c: consumer::PullConsumer = js.get_consumer_from_stream(DURABLE_NAME, STREAM_NAME).await.expect("consumer");
    c.info().await.expect("info").clone()
}

/// Poll until `done` holds, at most `budget`.
async fn wait_until<F, Fut>(budget: Duration, what: &str, mut done: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = Instant::now() + budget;
    while !done().await {
        assert!(Instant::now() < deadline, "{what} did not happen within {budget:?}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// All delivered messages are acked: none pending, none waiting for an ack.
async fn all_acked(js: &jetstream::Context, budget: Duration) {
    wait_until(budget, "all messages acked", || async {
        let info = consumer_info(js).await;
        info.num_ack_pending == 0 && info.num_pending == 0 && info.delivered.stream_sequence > 0
    })
    .await;
}

#[tokio::test]
async fn ack_on_insert() {
    let Some(s) = stack("ack_on_insert", 0x500, &[FILTER]).await else { return };
    let (stop, task) = start_source(&s).await;
    publish(&s.js, fix_org_with_actor_and_thread()).await;
    let db = s.db.clone();
    wait_until(Duration::from_secs(10), "one row", || {
        let db = db.clone();
        async move { rows(&db).await == 1 }
    })
    .await;
    all_acked(&s.js, Duration::from_secs(10)).await;
    let row =
        s.db.query_one_raw(sea_orm::Statement::from_string(
            sea_orm::DatabaseBackend::Postgres,
            "SELECT source, ce_id, envelope, thread_id IS NULL AS no_thread FROM inbox_event".to_string(),
        ))
        .await
        .expect("query")
        .expect("row");
    assert_eq!(row.try_get::<String>("", "source").unwrap(), SOURCE);
    assert_eq!(row.try_get::<String>("", "ce_id").unwrap(), "00000000-0000-0000-0000-000000000002");
    assert!(row.try_get::<bool>("", "no_thread").unwrap());
    let envelope: serde_json::Value = row.try_get("", "envelope").unwrap();
    let env = envelope.as_object().expect("object");
    assert_eq!(env["source"], SOURCE);
    assert!(!env.contains_key("actorprn"), "actorprn is dropped: {env:?}");
    assert!(!env.contains_key("threadid"), "threadid is dropped: {env:?}");
    assert_eq!(env["data"], serde_json::json!({"slug": "acme"}));
    let _ = stop.send(());
    task.await.expect("join").expect("run");
}

#[tokio::test]
async fn ack_on_duplicate() {
    let Some(s) = stack("ack_on_duplicate", 0x500, &[FILTER]).await else { return };
    let (stop, task) = start_source(&s).await;
    // No `Nats-Msg-Id`, so JetStream stores both. The dedup of the inbox keeps one row.
    publish(&s.js, FIX_ORG.to_vec()).await;
    publish(&s.js, FIX_ORG.to_vec()).await;
    wait_until(Duration::from_secs(10), "both messages delivered and acked", || async {
        let info = consumer_info(&s.js).await;
        info.delivered.stream_sequence == 2 && info.num_ack_pending == 0 && info.num_pending == 0
    })
    .await;
    assert_eq!(rows(&s.db).await, 1);
    let _ = stop.send(());
    task.await.expect("join").expect("run");
}

#[tokio::test]
async fn ack_and_skip_on_a_tenancy_refusal() {
    // The inbox belongs to another org than the event.
    let Some(s) = stack("ack_and_skip_on_a_tenancy_refusal", 0x600, &[FILTER]).await else { return };
    let (stop, task) = start_source(&s).await;
    publish(&s.js, FIX_ORG.to_vec()).await;
    all_acked(&s.js, Duration::from_secs(10)).await;
    assert_eq!(consumer_info(&s.js).await.num_redelivered, 0);
    assert_eq!(rows(&s.db).await, 0);
    let _ = stop.send(());
    task.await.expect("join").expect("run");
}

#[tokio::test]
async fn term_on_a_bad_body() {
    let Some(s) = stack("term_on_a_bad_body", 0x500, &[FILTER]).await else { return };
    let (stop, task) = start_source(&s).await;
    publish(&s.js, b"{".to_vec()).await;
    all_acked(&s.js, Duration::from_secs(10)).await;
    // A nak would redeliver within the 3 s; a term never does.
    tokio::time::sleep(Duration::from_secs(3)).await;
    let info = consumer_info(&s.js).await;
    assert_eq!(info.num_ack_pending, 0);
    assert_eq!(info.num_redelivered, 0);
    assert_eq!(rows(&s.db).await, 0);
    let _ = stop.send(());
    task.await.expect("join").expect("run");
}

#[tokio::test]
async fn redelivery_after_a_database_error() {
    let Some(s) = stack("redelivery_after_a_database_error", 0x500, &[FILTER]).await else { return };
    let (stop, task) = start_source(&s).await;
    s.db.execute_unprepared("ALTER TABLE inbox_event RENAME TO inbox_event_off").await.expect("rename");
    publish(&s.js, FIX_ORG.to_vec()).await;
    wait_until(Duration::from_secs(15), "a redelivery", || async { consumer_info(&s.js).await.num_redelivered >= 1 }).await;
    s.db.execute_unprepared("ALTER TABLE inbox_event_off RENAME TO inbox_event").await.expect("rename back");
    let db = s.db.clone();
    wait_until(Duration::from_secs(10), "one row after the database is back", || {
        let db = db.clone();
        async move { rows(&db).await == 1 }
    })
    .await;
    all_acked(&s.js, Duration::from_secs(10)).await;
    let _ = stop.send(());
    task.await.expect("join").expect("run");
}

#[tokio::test]
async fn boot_fails_on_a_filter_mismatch() {
    let Some(s) = stack("boot_fails_on_a_filter_mismatch", 0x500, &["iam.role.>"]).await else {
        return;
    };
    let cfg = nats_config(&s.url, u(INBOX));
    let channels = bind_channels(&cfg, &*s.repo).await.expect("bind");
    let err = connect_and_check(&cfg, &channels).await.err().expect("the boot check fails");
    match err {
        NatsBootError::FilterMismatch { configured, server } => {
            assert_eq!(configured, vec![FILTER.to_string()]);
            assert_eq!(server, vec!["iam.role.>".to_string()]);
        }
        other => panic!("want FilterMismatch, got {other:?}"),
    }
}

#[tokio::test]
async fn boot_fails_on_an_unknown_inbox() {
    let Some((_pg, _url, db)) = migrated_postgres().await else { return };
    let repo = PgInboxRepository::new(db);
    let cfg = nats_config("nats://127.0.0.1:1", u(0x999));
    let err = bind_channels(&cfg, &repo).await.expect_err("the bind fails");
    assert!(matches!(err, NatsBootError::UnknownInbox(id) if id == u(0x999)), "got {err:?}");
}
