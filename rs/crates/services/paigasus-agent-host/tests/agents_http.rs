// SPDX-License-Identifier: Apache-2.0

//! The REST routes of the agent API (SMA-852 spec §6.5, §6.6). The router runs in process with an
//! in-memory registry and a stub IAM, so no test needs Docker.

mod support;

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::header::{AUTHORIZATION, CONTENT_TYPE};
use axum::http::{Method, Request, StatusCode};
use paigasus_agent_host::adapters::http::{AppState, app_router};
use paigasus_agent_host::adapters::memory_commit::MemoryCommit;
use paigasus_agent_host::adapters::persistence::memory::PgMemoryStore;
use paigasus_agent_host::application::agents::AgentAdmin;
use paigasus_agent_host_core::ids::{AgentId, InboxId};
use paigasus_agent_host_core::ports::AgentRegistry;
use sea_orm::DatabaseConnection;
use serde_json::{Value, json};
use support::memory_registry::{CountingIds, FixedCapabilities, MemoryRegistry, UnreachableRegistry};
use support::stub_iam::{StubIam, TOKEN, org_prn, principal_prn};
use tower::ServiceExt;
use uuid::Uuid;

use paigasus_agent_host::adapters::iam::CallerCredential;

const BOTH: &[&str] = &["ManageAgents", "ReadAgents"];
const READ_ONLY: &[&str] = &["ReadAgents"];

fn u(n: u128) -> Uuid {
    Uuid::from_u128(n)
}

fn org() -> Uuid {
    u(0x500)
}

fn other_org() -> Uuid {
    u(0x600)
}

fn caller() -> Uuid {
    u(0x10)
}

fn agent_uuid() -> Uuid {
    u(0x700)
}

fn inbox_uuid() -> Uuid {
    u(0x800)
}

fn router(registry: Arc<dyn AgentRegistry>, iam: &Arc<StubIam>, mounted: bool) -> Router {
    let admin = Arc::new(AgentAdmin::new(registry, Arc::new(FixedCapabilities), iam.clone(), Arc::new(CountingIds::default())));
    app_router(
        AppState {
            db: DatabaseConnection::default(),
            inboxes: Arc::new(support::fake_inbox::FakeInbox::empty()),
            ingress: None,
            agents: mounted.then_some(admin),
            memory: MemoryCommit::InTransaction(PgMemoryStore::new(DatabaseConnection::default())),
        },
        None,
    )
}

async fn send(app: &Router, method: Method, path: &str, body: Option<&Value>, token: Option<&str>) -> (StatusCode, Value) {
    let mut req = Request::builder().method(method).uri(path);
    if let Some(token) = token {
        req = req.header(AUTHORIZATION, format!("Bearer {token}"));
    }
    let body = match body {
        Some(v) => {
            req = req.header(CONTENT_TYPE, "application/json");
            Body::from(v.to_string())
        }
        None => Body::empty(),
    };
    let resp = app.clone().oneshot(req.body(body).unwrap()).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

fn code(body: &Value) -> &str {
    body["error"]["code"].as_str().unwrap_or_default()
}

/// One route, with the IAM action and the success status.
struct Route {
    name: &'static str,
    method: Method,
    path: String,
    body: Option<Value>,
    action: &'static str,
    ok: StatusCode,
    /// `true` when the route names a row by id. Create and list name the org.
    by_id: bool,
}

fn routes() -> Vec<Route> {
    let a = agent_uuid();
    let fields = |version: bool| {
        let mut v = json!({ "name": "renamed", "model": "m1", "instructions": "Be brief.", "tools": ["memory"] });
        if version {
            v["version"] = json!(1);
        }
        v
    };
    let mut create = fields(false);
    create["org"] = json!(org_prn(org()));
    vec![
        Route {
            name: "create",
            method: Method::POST,
            path: "/v1/agents".into(),
            body: Some(create),
            action: "ManageAgents",
            ok: StatusCode::CREATED,
            by_id: false,
        },
        Route {
            name: "list",
            method: Method::GET,
            path: format!("/v1/agents?org={}", org_prn(org())),
            body: None,
            action: "ReadAgents",
            ok: StatusCode::OK,
            by_id: false,
        },
        Route {
            name: "get",
            method: Method::GET,
            path: format!("/v1/agents/{a}"),
            body: None,
            action: "ReadAgents",
            ok: StatusCode::OK,
            by_id: true,
        },
        Route {
            name: "update",
            method: Method::PATCH,
            path: format!("/v1/agents/{a}"),
            body: Some(fields(true)),
            action: "ManageAgents",
            ok: StatusCode::OK,
            by_id: true,
        },
        Route {
            name: "archive",
            method: Method::DELETE,
            path: format!("/v1/agents/{a}"),
            body: None,
            action: "ManageAgents",
            ok: StatusCode::OK,
            by_id: true,
        },
        Route {
            name: "bind",
            method: Method::POST,
            path: format!("/v1/agents/{a}/inboxes"),
            body: Some(json!({ "principal": principal_prn(u(0x20)) })),
            action: "ManageAgents",
            ok: StatusCode::CREATED,
            by_id: true,
        },
        Route {
            name: "list_inboxes",
            method: Method::GET,
            path: format!("/v1/agents/{a}/inboxes"),
            body: None,
            action: "ReadAgents",
            ok: StatusCode::OK,
            by_id: true,
        },
        Route {
            name: "get_inbox",
            method: Method::GET,
            path: format!("/v1/inboxes/{}", inbox_uuid()),
            body: None,
            action: "ReadAgents",
            ok: StatusCode::OK,
            by_id: true,
        },
    ]
}

/// A registry with the agent and its inbox in `org`.
fn seeded(org: Uuid) -> Arc<MemoryRegistry> {
    let registry = Arc::new(MemoryRegistry::default());
    let agent = registry.seed_agent(org, agent_uuid(), "seeded");
    registry.seed_inbox(&agent, inbox_uuid(), u(0x30));
    registry
}

fn key_iam() -> StubIam {
    StubIam::api_key(caller())
}

fn query(action: &str, org: Uuid) -> (CallerCredential, String, String, String) {
    (CallerCredential::ApiKey(TOKEN.to_owned()), principal_prn(caller()), action.to_owned(), org_prn(org))
}

async fn run(route: &Route, registry: Arc<dyn AgentRegistry>, iam: StubIam) -> (StatusCode, Value, Arc<StubIam>) {
    let iam = Arc::new(iam);
    let app = router(registry, &iam, true);
    let (status, body) = send(&app, route.method.clone(), &route.path, route.body.as_ref(), Some(TOKEN)).await;
    (status, body, iam)
}

#[tokio::test]
async fn the_self_query_is_credential_principal_action_org_and_both_verdicts_hold() {
    for route in routes() {
        let want = vec![query(route.action, org())];

        let (status, _, iam) = run(&route, seeded(org()), key_iam().granting(org(), BOTH)).await;
        assert_eq!(status, route.ok, "{}: allowed", route.name);
        assert_eq!(iam.self_query_log(), want, "{}: allowed", route.name);

        let (status, body, iam) = run(&route, seeded(org()), key_iam().allowing(false)).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{}: denied", route.name);
        assert_eq!(code(&body), "forbidden", "{}", route.name);
        assert_eq!(body["error"].as_object().unwrap().len(), 2, "{}: no metadata key", route.name);
        assert_eq!(iam.self_query_log(), want, "{}: denied", route.name);

        if route.action == "ManageAgents" {
            let (status, _, iam) = run(&route, seeded(org()), key_iam().granting(org(), READ_ONLY)).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{}: read-only grant", route.name);
            assert_eq!(iam.self_query_log(), want, "{}: read-only grant", route.name);
        }
    }
}

#[tokio::test]
async fn an_api_key_of_another_org_is_403_and_503_during_an_outage() {
    for route in routes() {
        let scope = org_prn(other_org());
        let want = vec![query(route.action, org())];

        // The probe says yes, and the answer is still 403.
        let (status, body, iam) = run(&route, seeded(org()), key_iam().with_key_scope(&scope)).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{}", route.name);
        assert_eq!(code(&body), "forbidden", "{}", route.name);
        assert_eq!(iam.self_query_log(), want, "{}", route.name);

        let (status, body, iam) = run(&route, seeded(org()), key_iam().with_key_scope(&scope).self_query_outage()).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{}", route.name);
        assert_eq!(code(&body), "iam-unavailable", "{}", route.name);
        assert_eq!(iam.self_query_log(), want, "{}", route.name);
    }
}

#[tokio::test]
async fn an_unknown_id_is_403_after_a_probe_against_the_nil_org_and_503_during_an_outage() {
    for route in routes().into_iter().filter(|r| r.by_id) {
        let want = vec![query(route.action, Uuid::nil())];

        // The probe says yes, and the answer is still 403.
        let (status, body, iam) = run(&route, Arc::new(MemoryRegistry::default()), key_iam()).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{}", route.name);
        assert_eq!(code(&body), "forbidden", "{}", route.name);
        assert_eq!(iam.self_query_log(), want, "{}", route.name);

        let (status, body, iam) = run(&route, Arc::new(MemoryRegistry::default()), key_iam().self_query_outage()).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{}", route.name);
        assert_eq!(code(&body), "iam-unavailable", "{}", route.name);
        assert_eq!(iam.self_query_log(), want, "{}", route.name);
    }
}

#[tokio::test]
async fn an_id_of_another_org_is_403_and_503_during_an_outage() {
    for route in routes().into_iter().filter(|r| r.by_id) {
        let want = vec![query(route.action, other_org())];

        let (status, body, iam) = run(&route, seeded(other_org()), key_iam().granting(org(), BOTH)).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{}", route.name);
        assert_eq!(code(&body), "forbidden", "{}", route.name);
        assert_eq!(iam.self_query_log(), want, "{}", route.name);

        let (status, body, iam) = run(&route, seeded(other_org()), key_iam().granting(org(), BOTH).self_query_outage()).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{}", route.name);
        assert_eq!(code(&body), "iam-unavailable", "{}", route.name);
        assert_eq!(iam.self_query_log(), want, "{}", route.name);
    }
}

#[tokio::test]
async fn an_unauthenticated_call_reads_nothing() {
    for route in routes() {
        // No header at all.
        let iam = Arc::new(key_iam());
        let app = router(Arc::new(UnreachableRegistry), &iam, true);
        let (status, body) = send(&app, route.method.clone(), &route.path, route.body.as_ref(), None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{}", route.name);
        assert_eq!(code(&body), "invalid-token", "{}", route.name);
        assert!(iam.calls().is_empty(), "{}: IAM was called", route.name);

        // A token that IAM refuses.
        let iam = Arc::new(StubIam::unprovisioned());
        let app = router(Arc::new(UnreachableRegistry), &iam, true);
        let (status, body) = send(&app, route.method.clone(), &route.path, route.body.as_ref(), Some(TOKEN)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{}", route.name);
        assert_eq!(code(&body), "invalid-token", "{}", route.name);
        assert_eq!(iam.self_queries(), 0, "{}", route.name);
    }
}

#[tokio::test]
async fn create_is_201_with_the_record_and_bind_is_201_with_the_inbox() {
    let registry = Arc::new(MemoryRegistry::default());
    let iam = Arc::new(key_iam().granting(org(), BOTH));
    let app = router(registry.clone(), &iam, true);

    let body = json!({ "org": org_prn(org()), "name": "helper", "model": "m1", "instructions": "Be brief.", "tools": ["memory"] });
    let (status, answer) = send(&app, Method::POST, "/v1/agents", Some(&body), Some(TOKEN)).await;
    assert_eq!(status, StatusCode::CREATED);
    let agent = AgentId::new(org(), u(0x1000));
    assert_eq!(
        answer,
        json!({
            "agent": agent.as_prn().canonical(),
            "org": org_prn(org()),
            "name": "helper",
            "model": "m1",
            "instructions": "Be brief.",
            "tools": ["memory"],
            "version": 1,
            "created_at": "1970-01-01T00:00:00Z",
            "updated_at": "1970-01-01T00:00:00Z",
            "archived_at": null,
        })
    );

    let bind = json!({ "principal": principal_prn(u(0x20)) });
    let (status, answer) = send(&app, Method::POST, &format!("/v1/agents/{}/inboxes", u(0x1000)), Some(&bind), Some(TOKEN)).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(
        answer,
        json!({
            "inbox": InboxId::new(org(), u(0x1001)).as_prn().canonical(),
            "agent": agent.as_prn().canonical(),
            "principal": principal_prn(u(0x20)),
            "created_at": "1970-01-01T00:00:00Z",
            "agent_archived_at": null,
        })
    );
}

#[tokio::test]
async fn a_list_answers_a_page_and_the_token_continues_it() {
    let registry = Arc::new(MemoryRegistry::default());
    for n in 1..=3 {
        registry.seed_agent(org(), u(0x700 + n), &format!("a{n}"));
    }
    let iam = Arc::new(key_iam().granting(org(), BOTH));
    let app = router(registry, &iam, true);
    let base = format!("/v1/agents?org={}&page_size=2", org_prn(org()));
    let (status, first) = send(&app, Method::GET, &base, None, Some(TOKEN)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(first["items"].as_array().unwrap().len(), 2);
    let token = first["next_page_token"].as_str().expect("a next token").to_owned();
    let (status, second) = send(&app, Method::GET, &format!("{base}&page_token={token}"), None, Some(TOKEN)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(second["items"].as_array().unwrap().len(), 1);
    assert_eq!(second["next_page_token"], Value::Null);

    let (status, body) = send(&app, Method::GET, &format!("{base}&page_token=junk"), None, Some(TOKEN)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(code(&body), "invalid-cursor");
    let (status, body) = send(&app, Method::GET, &format!("/v1/agents?org={}&page_size=1&page_size=2", org_prn(org())), None, Some(TOKEN)).await;
    assert_eq!((status, code(&body)), (StatusCode::BAD_REQUEST, "invalid-query-parameter"));
    let (status, body) = send(&app, Method::GET, &format!("/v1/agents/{}/inboxes?page_size=1&page_size=2", agent_uuid()), None, Some(TOKEN)).await;
    assert_eq!((status, code(&body)), (StatusCode::BAD_REQUEST, "invalid-query-parameter"));
    let (status, body) = send(&app, Method::GET, &format!("/v1/agents?org={}&include_archived=yes", org_prn(org())), None, Some(TOKEN)).await;
    assert_eq!((status, code(&body)), (StatusCode::BAD_REQUEST, "invalid-query-parameter"));
    let (status, body) = send(&app, Method::GET, &format!("/v1/agents?org={}&page_size=101", org_prn(org())), None, Some(TOKEN)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(code(&body), "invalid-pagination");
}

#[tokio::test]
async fn patch_with_a_stale_version_is_409_with_the_current_version_in_metadata() {
    let iam = Arc::new(key_iam().granting(org(), BOTH));
    let app = router(seeded(org()), &iam, true);
    let body = json!({ "version": 5, "name": "n", "model": "m1", "instructions": "i", "tools": [] });
    let (status, answer) = send(&app, Method::PATCH, &format!("/v1/agents/{}", agent_uuid()), Some(&body), Some(TOKEN)).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(code(&answer), "agent-version-mismatch");
    assert_eq!(answer["error"]["metadata"], json!({ "version": "1" }));
}

#[tokio::test]
async fn a_second_bind_is_409_inbox_exists_with_the_inbox_prn_in_metadata() {
    let iam = Arc::new(key_iam().granting(org(), BOTH));
    let app = router(seeded(org()), &iam, true);
    let body = json!({ "principal": principal_prn(u(0x30)) });
    let (status, answer) = send(&app, Method::POST, &format!("/v1/agents/{}/inboxes", agent_uuid()), Some(&body), Some(TOKEN)).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(code(&answer), "inbox-exists");
    assert_eq!(answer["error"]["metadata"], json!({ "inbox": InboxId::new(org(), inbox_uuid()).as_prn().canonical() }));
}

#[tokio::test]
async fn a_name_taken_and_an_unknown_model_hold_no_caller_input_in_the_message() {
    let registry = seeded(org());
    let iam = Arc::new(key_iam().granting(org(), BOTH));
    let app = router(registry, &iam, true);
    let taken = json!({ "org": org_prn(org()), "name": "seeded", "model": "m1", "instructions": "i", "tools": [] });
    let (status, answer) = send(&app, Method::POST, "/v1/agents", Some(&taken), Some(TOKEN)).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(code(&answer), "agent-name-taken");

    let model = json!({ "org": org_prn(org()), "name": "n2", "model": "secret-model", "instructions": "i", "tools": [] });
    let (status, answer) = send(&app, Method::POST, "/v1/agents", Some(&model), Some(TOKEN)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(code(&answer), "unknown-model");
    assert_eq!(answer["error"]["metadata"], json!({ "model": "secret-model" }));
    assert!(!answer["error"]["message"].as_str().unwrap().contains("secret"));

    let tool = json!({ "org": org_prn(org()), "name": "n3", "model": "m1", "instructions": "i", "tools": ["secret-tool"] });
    let (_, answer) = send(&app, Method::POST, "/v1/agents", Some(&tool), Some(TOKEN)).await;
    assert_eq!(code(&answer), "unknown-tool");
    assert_eq!(answer["error"]["metadata"], json!({ "tool": "secret-tool" }));

    let field = json!({ "org": org_prn(org()), "name": "", "model": "m1", "instructions": "i", "tools": [] });
    let (status, answer) = send(&app, Method::POST, "/v1/agents", Some(&field), Some(TOKEN)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(code(&answer), "invalid-agent-field");
    assert_eq!(answer["error"]["metadata"], json!({ "field": "name" }));
}

#[tokio::test]
async fn an_archived_agent_refuses_an_update_with_409() {
    let iam = Arc::new(key_iam().granting(org(), BOTH));
    let app = router(seeded(org()), &iam, true);
    let (status, answer) = send(&app, Method::DELETE, &format!("/v1/agents/{}", agent_uuid()), None, Some(TOKEN)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(answer["archived_at"], "1970-01-01T00:00:00Z");
    let body = json!({ "version": 1, "name": "n", "model": "m1", "instructions": "i", "tools": [] });
    let (status, answer) = send(&app, Method::PATCH, &format!("/v1/agents/{}", agent_uuid()), Some(&body), Some(TOKEN)).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(code(&answer), "agent-archived");
}

#[tokio::test]
async fn a_malformed_uuid_segment_is_400_invalid_uuid() {
    for route in routes().into_iter().filter(|r| r.by_id) {
        let iam = Arc::new(key_iam().granting(org(), BOTH));
        let app = router(seeded(org()), &iam, true);
        let path = route.path.replace(&agent_uuid().to_string(), "nope").replace(&inbox_uuid().to_string(), "nope");
        let (status, body) = send(&app, route.method.clone(), &path, route.body.as_ref(), Some(TOKEN)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{}", route.name);
        assert_eq!(code(&body), "invalid-uuid", "{}", route.name);
    }
}

#[tokio::test]
async fn a_bad_body_is_400_in_the_envelope() {
    let iam = Arc::new(key_iam().granting(org(), BOTH));
    let app = router(seeded(org()), &iam, true);
    let send_raw = |raw: &'static str| {
        let app = app.clone();
        async move {
            let req = Request::builder()
                .method(Method::POST)
                .uri("/v1/agents")
                .header(AUTHORIZATION, format!("Bearer {TOKEN}"))
                .body(Body::from(raw))
                .unwrap();
            let resp = app.oneshot(req).await.unwrap();
            let status = resp.status();
            let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
            (status, serde_json::from_slice::<Value>(&bytes).unwrap())
        }
    };
    let (status, body) = send_raw("{not json").await;
    assert_eq!((status, code(&body)), (StatusCode::BAD_REQUEST, "invalid-request-body"));
    let (status, body) = send_raw(r#"{"org":5,"name":"n","model":"m1","instructions":"i","tools":[]}"#).await;
    assert_eq!((status, code(&body)), (StatusCode::BAD_REQUEST, "invalid-request-schema"));
    let (status, body) = send_raw(r#"{"org":"nope","name":"n","model":"m1","instructions":"i","tools":[]}"#).await;
    assert_eq!((status, code(&body)), (StatusCode::BAD_REQUEST, "invalid-agent-field"));
    assert_eq!(body["error"]["metadata"], json!({ "field": "org" }));

    let bind = json!({ "principal": "nope" });
    let (status, body) = send(&app, Method::POST, &format!("/v1/agents/{}/inboxes", agent_uuid()), Some(&bind), Some(TOKEN)).await;
    assert_eq!((status, code(&body)), (StatusCode::BAD_REQUEST, "invalid-agent-field"));
    assert_eq!(body["error"]["metadata"], json!({ "field": "principal" }));
}

#[tokio::test]
async fn the_routes_are_not_mounted_without_iam() {
    let iam = Arc::new(key_iam());
    let app = router(Arc::new(UnreachableRegistry), &iam, false);
    for route in routes() {
        let (status, body) = send(&app, route.method.clone(), &route.path, route.body.as_ref(), Some(TOKEN)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{}", route.name);
        assert_eq!(code(&body), "not-found", "{}", route.name);
    }
}
