// SPDX-License-Identifier: Apache-2.0

//! The HTTP ingress routes (spec §5): one test for each step of §5.2, and the service-info route.
//! The router runs in process with a fake inbox store and a stub IAM.

mod support;

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use paigasus_agent_host::adapters::http::{AppState, Ingress, app_router};
use paigasus_agent_host_core::ids::{AgentId, EventId, InboxId, PrincipalId, ThreadId};
use paigasus_agent_host_core::inbox::Inbox;
use sea_orm::DatabaseConnection;
use serde_json::{Value, json};
use support::fake_inbox::{FakeInbox, InsertAnswer};
use support::stub_iam::{StubIam, TOKEN, principal_prn};
use tower::ServiceExt;
use uuid::Uuid;

const CE: &str = "application/cloudevents+json";
const MAX: usize = 262_144;

fn u(n: u128) -> Uuid {
    Uuid::from_u128(n)
}

fn owner() -> Uuid {
    u(0x10)
}

fn inbox_uuid() -> Uuid {
    u(0x1)
}

fn inbox() -> Inbox {
    Inbox::new(
        InboxId::new(u(0x500), inbox_uuid()),
        AgentId::new(u(0x500), u(2)),
        PrincipalId::from_uuid(owner()),
        jiff::Timestamp::UNIX_EPOCH,
    )
    .expect("inbox")
}

fn router(repo: &Arc<FakeInbox>, iam: &Arc<StubIam>) -> Router {
    app_router(
        AppState {
            db: DatabaseConnection::default(),
            inboxes: repo.clone(),
            ingress: Some(Arc::new(Ingress {
                iam: iam.clone(),
                max_body_bytes: MAX,
            })),
        },
        None,
    )
}

fn event() -> Value {
    json!({"specversion": "1.0", "id": "evt-1", "source": "x", "type": "user.message", "data": {"text": "hi"}})
}

struct Post {
    path: String,
    content_type: Option<String>,
    token: Option<&'static str>,
    content_length: Option<usize>,
    body: Body,
}

fn post() -> Post {
    Post {
        path: format!("/v1/inboxes/{}/events", inbox_uuid()),
        content_type: Some(CE.to_string()),
        token: Some(TOKEN),
        content_length: None,
        body: Body::from(event().to_string()),
    }
}

impl Post {
    fn json(mut self, v: &Value) -> Self {
        self.body = Body::from(v.to_string());
        self
    }
    fn raw(mut self, bytes: &str) -> Self {
        self.body = Body::from(bytes.to_string());
        self
    }
    fn content_type(mut self, v: Option<&str>) -> Self {
        self.content_type = v.map(str::to_string);
        self
    }
    fn token(mut self, v: Option<&'static str>) -> Self {
        self.token = v;
        self
    }
    fn path(mut self, p: String) -> Self {
        self.path = p;
        self
    }
    fn content_length(mut self, n: usize) -> Self {
        self.content_length = Some(n);
        self
    }
    fn body(mut self, b: Body) -> Self {
        self.body = b;
        self
    }
    async fn send(self, app: &Router) -> (StatusCode, Vec<u8>) {
        let mut b = Request::post(self.path);
        if let Some(ct) = self.content_type {
            b = b.header("content-type", ct);
        }
        if let Some(t) = self.token {
            b = b.header("authorization", format!("Bearer {t}"));
        }
        if let Some(n) = self.content_length {
            b = b.header("content-length", n.to_string());
        }
        let resp = app.clone().oneshot(b.body(self.body).unwrap()).await.unwrap();
        let status = resp.status();
        (status, axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap().to_vec())
    }
}

fn as_json(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).expect("a JSON body")
}

fn code(bytes: &[u8]) -> String {
    as_json(bytes)["error"]["code"].as_str().expect("an error code").to_string()
}

fn owner_setup() -> (Arc<FakeInbox>, Arc<StubIam>) {
    (Arc::new(FakeInbox::with(inbox())), Arc::new(StubIam::api_key(owner())))
}

#[tokio::test]
async fn a_wrong_content_type_is_415_before_auth() {
    let (repo, iam) = owner_setup();
    let (status, body) = post().content_type(Some("application/json")).token(None).send(&router(&repo, &iam)).await;
    assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert_eq!(code(&body), "unsupported-content-type");
    assert!(iam.calls().is_empty());
}

#[tokio::test]
async fn the_content_type_ignores_case_and_parameters() {
    let (repo, iam) = owner_setup();
    let app = router(&repo, &iam);
    let (status, _) = post().content_type(Some("Application/CloudEvents+JSON; charset=UTF-8")).send(&app).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let (status, body) = post().content_type(Some("application/cloudevents+jsonx")).send(&app).await;
    assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert_eq!(code(&body), "unsupported-content-type");
    let (status, _) = post().content_type(None).send(&app).await;
    assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
}

#[tokio::test]
async fn a_large_content_length_is_413_before_auth() {
    let (repo, iam) = owner_setup();
    let (status, body) = post().token(None).content_length(MAX + 1).send(&router(&repo, &iam)).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(code(&body), "request-too-large");
    assert!(iam.calls().is_empty());
}

#[tokio::test]
async fn a_chunked_body_over_the_limit_is_413_after_auth() {
    let (repo, iam) = owner_setup();
    let chunks: Vec<Result<Vec<u8>, std::io::Error>> = (0..300).map(|_| Ok(vec![b'x'; 1024])).collect();
    let stream = futures::stream::iter(chunks);
    let (status, body) = post().body(Body::from_stream(stream)).send(&router(&repo, &iam)).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(code(&body), "request-too-large");
    assert!(!iam.calls().is_empty());
}

#[tokio::test]
async fn no_credential_is_401_invalid_token() {
    let (repo, iam) = owner_setup();
    let (status, body) = post().token(None).send(&router(&repo, &iam)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(code(&body), "invalid-token");
}

#[tokio::test]
async fn an_iam_outage_is_503_iam_unavailable() {
    let repo = Arc::new(FakeInbox::with(inbox()));
    let iam = Arc::new(StubIam::outage());
    let (status, body) = post().send(&router(&repo, &iam)).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(code(&body), "iam-unavailable");
}

#[tokio::test]
async fn an_unknown_inbox_and_a_refused_caller_get_the_same_403() {
    // Unknown inbox: the store has none. The caller would be allowed.
    let unknown_repo = Arc::new(FakeInbox::empty());
    let unknown_iam = Arc::new(StubIam::api_key(owner()));
    let (s1, b1) = post().send(&router(&unknown_repo, &unknown_iam)).await;
    assert_eq!(unknown_iam.self_queries(), 0, "an unknown inbox makes no self-query");

    // Refused caller: a known inbox, a caller that is not the owner, and IAM says no.
    let repo = Arc::new(FakeInbox::with(inbox()));
    let iam = Arc::new(StubIam::api_key(u(0x11)).allowing(false));
    let (s2, b2) = post().send(&router(&repo, &iam)).await;
    assert_eq!(iam.self_queries(), 1);

    assert_eq!(s1, StatusCode::FORBIDDEN);
    assert_eq!(s2, StatusCode::FORBIDDEN);
    assert_eq!(b1, b2, "the two bodies are byte-equal");
    assert_eq!(code(&b1), "forbidden");
}

#[tokio::test]
async fn a_malformed_inbox_segment_is_400_invalid_uuid() {
    let (repo, iam) = owner_setup();
    let (status, body) = post().path("/v1/inboxes/not-a-uuid/events".to_string()).send(&router(&repo, &iam)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(code(&body), "invalid-uuid");
}

#[tokio::test]
async fn invalid_json_is_400_invalid_request_body() {
    let (repo, iam) = owner_setup();
    let (status, body) = post().raw("{not json").send(&router(&repo, &iam)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(code(&body), "invalid-request-body");
}

#[tokio::test]
async fn an_invalid_cloudevent_is_400_invalid_request_schema() {
    let (repo, iam) = owner_setup();
    let mut e = event();
    e["specversion"] = json!("0.3");
    let (status, body) = post().json(&e).send(&router(&repo, &iam)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(code(&body), "invalid-request-schema");
    assert!(repo.inserted().is_empty());
}

/// A NUL in `data` cannot be stored in `jsonb`. It is a malformed event (400), not a store error
/// (503), so a client does not retry it.
#[tokio::test]
async fn a_nul_in_data_is_400_invalid_request_schema() {
    let (repo, iam) = owner_setup();
    let mut e = event();
    e["data"] = json!({"outer": {"note": "a\u{0}b"}});
    let (status, body) = post().json(&e).send(&router(&repo, &iam)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(code(&body), "invalid-request-schema");
    assert!(repo.inserted().is_empty());
}

#[tokio::test]
async fn an_agents_type_is_400_invalid_request_schema() {
    let (repo, iam) = owner_setup();
    let mut e = event();
    e["type"] = json!("agents.outbox.message");
    let (status, body) = post().json(&e).send(&router(&repo, &iam)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(code(&body), "invalid-request-schema");
    assert!(repo.inserted().is_empty());
}

#[tokio::test]
async fn the_host_sets_source_and_actorprn() {
    let (repo, iam) = owner_setup();
    let mut e = event();
    e["source"] = json!("x");
    e["actorprn"] = json!("y");
    let (status, body) = post().json(&e).send(&router(&repo, &iam)).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let prn = principal_prn(owner());
    let inserted = repo.inserted();
    assert_eq!(inserted.len(), 1);
    let stored = &inserted[0];
    let want = format!("urn:paigasus:agents:http:{prn}");
    assert_eq!(stored.source, want);
    assert_eq!(stored.envelope["source"], want);
    assert_eq!(stored.envelope["actorprn"], prn);
    assert_eq!(stored.ce_id, "evt-1");
    assert_eq!(stored.ce_type, "user.message");
    assert_eq!(stored.inbox.uuid(), inbox_uuid());
    assert_eq!(as_json(&body)["id"], stored.id.uuid().to_string());
}

#[tokio::test]
async fn a_well_formed_threadid_is_passed_to_the_repository() {
    // The route cannot know the thread's inbox. The repository drops a foreign thread (Task 4,
    // and `ingress_pg`). The route keeps the raw `threadid` in the envelope.
    let (repo, iam) = owner_setup();
    let thread = u(0x77);
    let mut e = event();
    e["threadid"] = json!(thread.to_string());
    let (status, _) = post().json(&e).send(&router(&repo, &iam)).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let inserted = repo.inserted();
    assert_eq!(inserted[0].thread, Some(ThreadId::from_uuid(thread)));
    assert_eq!(inserted[0].envelope["threadid"], thread.to_string());
}

#[tokio::test]
async fn a_non_uuid_or_non_string_threadid_is_dropped() {
    let (repo, iam) = owner_setup();
    let app = router(&repo, &iam);
    for bad in [json!("x"), json!(7)] {
        let mut e = event();
        e["threadid"] = bad;
        let (status, _) = post().json(&e).send(&app).await;
        assert_eq!(status, StatusCode::ACCEPTED);
    }
    let inserted = repo.inserted();
    assert_eq!(inserted.len(), 2);
    assert!(inserted.iter().all(|i| i.thread.is_none()));
}

#[tokio::test]
async fn a_duplicate_is_202_with_the_stored_id() {
    let stored = EventId::from_uuid(u(0xE1));
    let repo = Arc::new(FakeInbox::with(inbox()).answering(InsertAnswer::Duplicate(stored)));
    let iam = Arc::new(StubIam::api_key(owner()));
    let (status, body) = post().send(&router(&repo, &iam)).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(as_json(&body), json!({"id": stored.uuid().to_string()}));
}

#[tokio::test]
async fn a_store_error_is_503_store_unavailable() {
    let iam = Arc::new(StubIam::api_key(owner()));
    let lookup = Arc::new(FakeInbox::with(inbox()).failing_lookup());
    let (status, body) = post().send(&router(&lookup, &iam)).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(code(&body), "store-unavailable");

    let insert = Arc::new(FakeInbox::with(inbox()).answering(InsertAnswer::Fail));
    let (status, body) = post().send(&router(&insert, &iam)).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(code(&body), "store-unavailable");
}

async fn get_info(app: &Router, token: Option<&str>) -> (StatusCode, Vec<u8>) {
    let mut b = Request::get("/v1/service-info");
    if let Some(t) = token {
        b = b.header("authorization", format!("Bearer {t}"));
    }
    let resp = app.clone().oneshot(b.body(Body::empty()).unwrap()).await.unwrap();
    let status = resp.status();
    (status, axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap().to_vec())
}

#[tokio::test]
async fn service_info_needs_a_credential() {
    let (repo, iam) = owner_setup();
    let (status, body) = get_info(&router(&repo, &iam), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(code(&body), "invalid-token");
}

#[tokio::test]
async fn service_info_accepts_an_unprovisioned_identity() {
    let repo = Arc::new(FakeInbox::with(inbox()));
    let iam = Arc::new(StubIam::unprovisioned());
    let (status, body) = get_info(&router(&repo, &iam), Some(TOKEN)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(as_json(&body), json!({"service": "agents", "version": env!("CARGO_PKG_VERSION"), "capabilities": []}));
}

#[tokio::test]
async fn without_iam_the_ingress_routes_are_404() {
    let repo = Arc::new(FakeInbox::with(inbox()));
    let app = app_router(
        AppState {
            db: DatabaseConnection::default(),
            inboxes: repo,
            ingress: None,
        },
        None,
    );
    let (status, body) = post().send(&app).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(code(&body), "not-found");
    let (status, _) = get_info(&app, Some(TOKEN)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
