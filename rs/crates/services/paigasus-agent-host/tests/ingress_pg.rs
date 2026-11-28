// SPDX-License-Identifier: Apache-2.0

//! The ingress route against a real Postgres and the real `PgInboxRepository` (spec §5.2 steps 7
//! and 8, §7.2). IAM is a stub.

mod support;

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use paigasus_agent_host::adapters::http::{AppState, Ingress, app_router};
use paigasus_agent_host::adapters::memory_commit::MemoryCommit;
use paigasus_agent_host::adapters::persistence::inbox_repository::PgInboxRepository;
use paigasus_agent_host::adapters::persistence::memory::PgMemoryStore;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use serde_json::{Value, json};
use support::inbox::{insert_inbox, migrated_postgres};
use support::stub_iam::{StubIam, TOKEN, principal_prn};
use tower::ServiceExt;
use uuid::Uuid;

fn u(n: u128) -> Uuid {
    Uuid::from_u128(n)
}

const ORG: u128 = 1;
const INBOX: u128 = 2;
const AGENT: u128 = 3;
const OWNER: u128 = 4;

fn app(db: &DatabaseConnection) -> Router {
    app_with(db, StubIam::api_key(u(OWNER)))
}

fn app_with(db: &DatabaseConnection, iam: StubIam) -> Router {
    app_router(
        AppState {
            db: db.clone(),
            inboxes: Arc::new(PgInboxRepository::new(db.clone())),
            ingress: Some(Arc::new(Ingress {
                iam: Arc::new(iam),
                max_body_bytes: 262_144,
            })),
            agents: None,
            memory: MemoryCommit::InTransaction(PgMemoryStore::new(DatabaseConnection::default())),
        },
        None,
    )
}

async fn post(app: &Router, event: &Value) -> (StatusCode, Value) {
    let req = Request::post(format!("/v1/inboxes/{}/events", u(INBOX)))
        .header("content-type", "application/cloudevents+json")
        .header("authorization", format!("Bearer {TOKEN}"))
        .body(Body::from(event.to_string()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    (status, serde_json::from_slice(&bytes).expect("a JSON body"))
}

fn event() -> Value {
    json!({"specversion": "1.0", "id": "evt-1", "source": "caller", "type": "user.message", "data": {"text": "hi"}})
}

async fn one_row(db: &DatabaseConnection, sql: &str) -> sea_orm::QueryResult {
    db.query_one_raw(Statement::from_string(DatabaseBackend::Postgres, sql.to_string()))
        .await
        .expect("query")
        .expect("a row")
}

#[tokio::test]
async fn a_posted_event_is_stored_with_the_host_fields() {
    let Some((_node, _url, db)) = migrated_postgres().await else { return };
    insert_inbox(&db, u(ORG), u(INBOX), u(AGENT), u(OWNER)).await;
    let (status, body) = post(&app(&db), &event()).await;
    assert_eq!(status, StatusCode::ACCEPTED);

    let row = one_row(&db, "SELECT id::text AS id, source, ce_id, ce_type, status, envelope::text AS envelope FROM inbox_event").await;
    let prn = principal_prn(u(OWNER));
    let source = format!("urn:paigasus:agents:http:{prn}");
    assert_eq!(row.try_get::<String>("", "id").unwrap(), body["id"].as_str().unwrap());
    assert_eq!(row.try_get::<String>("", "source").unwrap(), source);
    assert_eq!(row.try_get::<String>("", "ce_id").unwrap(), "evt-1");
    assert_eq!(row.try_get::<String>("", "ce_type").unwrap(), "user.message");
    assert_eq!(row.try_get::<String>("", "status").unwrap(), "pending");
    let envelope: Value = serde_json::from_str(&row.try_get::<String>("", "envelope").unwrap()).unwrap();
    assert_eq!(envelope["source"], source);
    assert_eq!(envelope["actorprn"], prn);
    assert_eq!(envelope["data"], json!({"text": "hi"}));
    assert_eq!(envelope["id"], "evt-1");
}

#[tokio::test]
async fn a_client_retry_gets_the_same_body() {
    let Some((_node, _url, db)) = migrated_postgres().await else { return };
    insert_inbox(&db, u(ORG), u(INBOX), u(AGENT), u(OWNER)).await;
    let app = app(&db);
    let (s1, b1) = post(&app, &event()).await;
    let (s2, b2) = post(&app, &event()).await;
    assert_eq!((s1, s2), (StatusCode::ACCEPTED, StatusCode::ACCEPTED));
    assert_eq!(b1, b2);
    let row = one_row(&db, "SELECT count(*) AS n FROM inbox_event").await;
    assert_eq!(row.try_get::<i64>("", "n").unwrap(), 1);
}

#[tokio::test]
async fn a_foreign_threadid_is_stored_as_null() {
    let Some((_node, _url, db)) = migrated_postgres().await else { return };
    insert_inbox(&db, u(ORG), u(INBOX), u(AGENT), u(OWNER)).await;
    insert_inbox(&db, u(ORG), u(20), u(30), u(OWNER)).await;
    let foreign = u(900);
    db.execute_unprepared(&format!("INSERT INTO thread (id, inbox_id) VALUES ('{foreign}', '{}')", u(20)))
        .await
        .expect("insert thread");
    let mut e = event();
    e["threadid"] = json!(foreign.to_string());
    let (status, _) = post(&app(&db), &e).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let row = one_row(&db, "SELECT (thread_id IS NULL) AS v, envelope::text AS envelope FROM inbox_event").await;
    assert!(row.try_get::<bool>("", "v").unwrap());
    let envelope: Value = serde_json::from_str(&row.try_get::<String>("", "envelope").unwrap()).unwrap();
    assert_eq!(envelope["threadid"], foreign.to_string());
}

async fn archive_agent(db: &DatabaseConnection) {
    db.execute_unprepared(&format!("UPDATE agent SET archived_at = now() WHERE id = '{}'", u(AGENT)))
        .await
        .expect("archive");
}

#[tokio::test]
async fn an_authorized_post_to_an_archived_agents_inbox_is_409_agent_archived_and_writes_no_row() {
    let Some((_node, _url, db)) = migrated_postgres().await else { return };
    insert_inbox(&db, u(ORG), u(INBOX), u(AGENT), u(OWNER)).await;
    archive_agent(&db).await;
    let (status, body) = post(&app(&db), &event()).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"]["code"], "agent-archived");
    assert_eq!(body["error"]["message"], "the agent of this inbox is archived");
    let row = one_row(&db, "SELECT count(*) AS n FROM inbox_event").await;
    assert_eq!(row.try_get::<i64>("", "n").unwrap(), 0);
}

#[tokio::test]
async fn an_unauthorized_post_to_an_archived_agents_inbox_is_403() {
    let Some((_node, _url, db)) = migrated_postgres().await else { return };
    insert_inbox(&db, u(ORG), u(INBOX), u(AGENT), u(OWNER)).await;
    archive_agent(&db).await;
    // The caller is not the owner and IAM refuses: the answer must not tell that the agent is archived.
    let (status, body) = post(&app_with(&db, StubIam::api_key(u(0x11)).allowing(false)), &event()).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"]["code"], "forbidden");
    let row = one_row(&db, "SELECT count(*) AS n FROM inbox_event").await;
    assert_eq!(row.try_get::<i64>("", "n").unwrap(), 0);
}
