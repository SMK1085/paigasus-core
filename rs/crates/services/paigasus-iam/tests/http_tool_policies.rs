// SPDX-License-Identifier: Apache-2.0

//! The tool catalog and tool policy HTTP routes (SMA-774 Task 13, spec sections 6.2, 7 and 9.1).
//! Docker-gated. The compiler is the fake script of `support::fake_compiler_script`.

mod support;

use std::time::Duration;

use axum::Router;
use axum::http::StatusCode;
use paigasus_iam::adapters::grpc;
use paigasus_iam::adapters::http::{AppState, router};
use paigasus_iam::adapters::id::KernelIdGenerator;
use paigasus_iam_core::tool_policy::{STARTER_TOOL_POLICY_SOURCE, tenant_schema_json};
use paigasus_iam_core::{IdGenerator, PrincipalId};
use paigasus_kernel::Prn;
use paigasus_proto::paigasus::iam::v1::GetToolPolicySetRequest;
use paigasus_proto::paigasus::iam::v1::tool_policy_service_client::ToolPolicyServiceClient;
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use serde_json::{Value, json};
use support::{MockIdp, accepted_result, fake_compiler_script, rejected_result, send, set_compiler_result};
use tempfile::TempDir;
use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::testcontainers::ContainerAsync;

struct Env {
    _node: ContainerAsync<Postgres>,
    dir: TempDir,
    app: Router,
    state: AppState,
    idp: MockIdp,
    admin: String,
    org: String,
    org_prn: String,
}

impl Env {
    async fn token_with(&self, sub: &str, role: &str) -> String {
        let token = self.idp.bearer(sub, Some(&format!("{sub}@example.com")), "paigasus", 3600);
        let prn = support::provision(&self.state, &token).await;
        support::seed_org_role(&self.state, &prn, &self.org_prn, role).await;
        token
    }

    fn tools(&self) -> String {
        format!("/v1/organizations/{}/tools", self.org)
    }

    fn set(&self) -> String {
        format!("/v1/organizations/{}/tool-policy-set", self.org)
    }
}

fn tenant_accepted() -> String {
    accepted_result(STARTER_TOOL_POLICY_SOURCE, &tenant_schema_json(&[]))
}

async fn setup(enabled: bool, result: &str) -> Option<Env> {
    setup_with(enabled, true, result).await
}

async fn setup_with(enabled: bool, compiler_present: bool, result: &str) -> Option<Env> {
    let (node, db) = support::start_migrated_postgres().await?;
    let dir = TempDir::new().expect("tempdir");
    let path = if compiler_present {
        fake_compiler_script(dir.path(), result)
    } else {
        dir.path().join("no-such-compiler")
    };
    let (state, idp) = support::state_with_tool_policies(db, enabled, &path).await;
    let app = router(state.clone());
    let admin = idp.bearer("tp-admin", Some("tp-admin@example.com"), "paigasus", 3600);
    let admin_prn = support::provision_platform_admin(&state, &admin).await;
    let out = state
        .orgs
        .create(&PrincipalId::from_prn(Prn::parse(&admin_prn).unwrap()), "acme", "Acme")
        .await
        .expect("create the organization");
    let org_prn = out.organization.id.canonical();
    let org = out.organization.id.uuid().to_string();
    Some(Env {
        _node: node,
        dir,
        app,
        state,
        idp,
        admin,
        org,
        org_prn,
    })
}

#[tokio::test]
async fn the_routes_are_404_when_the_capability_is_off() {
    // The toggle off, then the toggle on with a binary that fails its boot probe.
    for (enabled, present) in [(false, true), (true, false)] {
        let Some(env) = setup_with(enabled, present, &tenant_accepted()).await else { return };
        assert!(!env.state.capabilities.tool_policies);
        let tools = env.tools();
        let set = env.set();
        let body = json!({"name": "search", "kind": "function", "expected_catalog_revision": 0});
        let calls: Vec<(&str, String, Option<Value>)> = vec![
            ("GET", tools.clone(), None),
            ("POST", tools.clone(), Some(body)),
            ("DELETE", format!("{tools}/search?expected_catalog_revision=0"), None),
            (
                "POST",
                format!("/v1/organizations/{}/tool-imports", env.org),
                Some(json!({"format": "openai_responses", "document": [], "expected_catalog_revision": 0})),
            ),
            ("PUT", set.clone(), Some(json!({"source": "", "expected_version": 0}))),
            ("GET", set, None),
            ("PUT", "/v1/tool-policy-floor".to_string(), Some(json!({"source": "", "expected_version": 0}))),
            ("GET", "/v1/tool-policy-floor".to_string(), None),
        ];
        for (method, uri, body) in calls {
            let (status, _) = send(&env.app, method, &uri, body, Some(&env.admin)).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{method} {uri} (enabled={enabled})");
        }
    }
}

#[tokio::test]
async fn every_error_code_reaches_the_http_envelope_with_its_status() {
    let Some(env) = setup(true, &tenant_accepted()).await else { return };
    let tools = env.tools();
    let post_tool = |name: &str, rev: u64| json!({"name": name, "kind": "function", "expected_catalog_revision": rev});
    let code = |body: &Value| body["error"]["code"].as_str().unwrap_or_default().to_string();

    // forbidden (403).
    let member = env.token_with("tp-member", "org_member").await;
    let (status, body) = send(&env.app, "POST", &tools, Some(post_tool("search", 0)), Some(&member)).await;
    assert_eq!((status, code(&body).as_str()), (StatusCode::FORBIDDEN, "forbidden"));

    // not-found (404).
    let ghost = KernelIdGenerator.new_organization_id().uuid();
    let (status, body) = send(&env.app, "POST", &format!("/v1/organizations/{ghost}/tools"), Some(post_tool("search", 0)), Some(&env.admin)).await;
    assert_eq!((status, code(&body).as_str()), (StatusCode::NOT_FOUND, "not-found"));

    // tool-catalog-conflict (409).
    let (status, _) = send(&env.app, "POST", &tools, Some(post_tool("Search", 0)), Some(&env.admin)).await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = send(&env.app, "POST", &tools, Some(post_tool("search", 1)), Some(&env.admin)).await;
    assert_eq!((status, code(&body).as_str()), (StatusCode::CONFLICT, "tool-catalog-conflict"));

    // tool-policy-version-conflict (409).
    let (status, body) = send(
        &env.app,
        "PUT",
        &env.set(),
        Some(json!({"source": STARTER_TOOL_POLICY_SOURCE, "expected_version": 5})),
        Some(&env.admin),
    )
    .await;
    assert_eq!((status, code(&body).as_str()), (StatusCode::CONFLICT, "tool-policy-version-conflict"));

    // tool-policy-version-not-found (404).
    let (status, body) = send(&env.app, "GET", &format!("{}?version=9", env.set()), None, Some(&env.admin)).await;
    assert_eq!((status, code(&body).as_str()), (StatusCode::NOT_FOUND, "tool-policy-version-not-found"));

    // tool-policy-compiler-unavailable (503).
    set_compiler_result(env.dir.path(), "this is not json");
    let (status, body) = send(
        &env.app,
        "PUT",
        &env.set(),
        Some(json!({"source": STARTER_TOOL_POLICY_SOURCE, "expected_version": 0})),
        Some(&env.admin),
    )
    .await;
    assert_eq!((status, code(&body).as_str()), (StatusCode::SERVICE_UNAVAILABLE, "tool-policy-compiler-unavailable"));

    // tool-policy-compiler-busy (429): three saves at once on a slow compiler with two permits.
    set_compiler_result(env.dir.path(), &tenant_accepted());
    support::make_compiler_slow(env.dir.path());
    let set_uri = env.set();
    let put = || send(&env.app, "PUT", &set_uri, Some(json!({"source": STARTER_TOOL_POLICY_SOURCE, "expected_version": 0})), Some(&env.admin));
    let (a, b, c) = tokio::join!(put(), put(), put());
    let busy: Vec<_> = [a, b, c].into_iter().filter(|(status, _)| *status == StatusCode::TOO_MANY_REQUESTS).collect();
    assert!(!busy.is_empty(), "one of three saves must meet a full compiler");
    assert_eq!(code(&busy[0].1), "tool-policy-compiler-busy");
    std::fs::remove_file(env.dir.path().join("slow")).expect("remove the slow marker");

    // tool-policy-recompile-pending (409).
    env.state
        .db
        .execute_raw(Statement::from_string(DbBackend::Postgres, "UPDATE tool_policy_set_version SET compiler_generation = 0".to_string()))
        .await
        .expect("age the live version");
    let (status, body) = send(&env.app, "POST", &tools, Some(post_tool("other", 1)), Some(&env.admin)).await;
    assert_eq!((status, code(&body).as_str()), (StatusCode::CONFLICT, "tool-policy-recompile-pending"));
}

#[tokio::test]
async fn a_rejection_is_200_with_result_rejected() {
    let Some(env) = setup(true, &rejected_result(&[("tool-policy-invalid", "the policy is wrong"), ("tool-policy-id-invalid", "the id is wrong")])).await else {
        return;
    };
    let (status, body) = send(&env.app, "PUT", &env.set(), Some(json!({"source": "x", "expected_version": 0})), Some(&env.admin)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["result"], "rejected");
    let rejections = body["rejections"].as_array().expect("rejections");
    assert_eq!(rejections.len(), 2);
    assert_eq!(
        rejections[0],
        json!({"policy_id": "p1", "line": 2, "column": 5, "reason": "tool-policy-invalid", "message": "the policy is wrong"})
    );

    // A catalog rejection has the same shape; absent location fields are null.
    let (status, body) = send(
        &env.app,
        "POST",
        &env.tools(),
        Some(json!({"name": "bad name", "kind": "function", "expected_catalog_revision": 0})),
        Some(&env.admin),
    )
    .await;
    assert_eq!((status, body["result"].as_str()), (StatusCode::OK, Some("rejected")));
    assert_eq!(body["rejections"][0]["reason"], "tool-catalog-invalid");
    assert_eq!(body["rejections"][0]["line"], Value::Null);
}

#[tokio::test]
async fn a_tool_named_import_can_be_deleted() {
    let Some(env) = setup(true, &tenant_accepted()).await else { return };
    let (status, body) = send(
        &env.app,
        "POST",
        &env.tools(),
        Some(json!({"name": "import", "kind": "custom", "expected_catalog_revision": 0})),
        Some(&env.admin),
    )
    .await;
    assert_eq!((status, body["result"].as_str(), body["catalog_revision"].as_u64()), (StatusCode::OK, Some("accepted"), Some(1)));
    assert_eq!(body["tools"][0]["name_key"], "import");
    let (status, body) = send(&env.app, "DELETE", &format!("{}/import?expected_catalog_revision=1", env.tools()), None, Some(&env.admin)).await;
    assert_eq!((status, body["result"].as_str(), body["catalog_revision"].as_u64()), (StatusCode::OK, Some("accepted"), Some(2)));
    let (status, body) = send(&env.app, "GET", &env.tools(), None, Some(&env.admin)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!((body["tools"].as_array().unwrap().len(), body["catalog_revision"].as_u64()), (0, Some(2)));
}

#[tokio::test]
async fn import_reports_skipped_entries() {
    let Some(env) = setup(true, &tenant_accepted()).await else { return };
    let document = json!([{"type": "function", "name": "search"}, {"type": "web_search"}]);
    let body = json!({"format": "openai_responses", "document": document, "expected_catalog_revision": 0});
    let (status, body) = send(&env.app, "POST", &format!("/v1/organizations/{}/tool-imports", env.org), Some(body), Some(&env.admin)).await;
    assert_eq!((status, body["result"].as_str()), (StatusCode::OK, Some("accepted")));
    assert_eq!(body["skipped"], json!([{"index": 1, "type": "web_search"}]));
    assert_eq!(body["tools"][0]["source"], "openai");
}

#[tokio::test]
async fn http_and_grpc_return_the_same_accepted_version() {
    let Some(env) = setup(true, &tenant_accepted()).await else { return };
    let (status, put) = send(
        &env.app,
        "PUT",
        &env.set(),
        Some(json!({"source": STARTER_TOOL_POLICY_SOURCE, "expected_version": 0})),
        Some(&env.admin),
    )
    .await;
    assert_eq!((status, put["result"].as_str()), (StatusCode::OK, Some("accepted")));
    assert_eq!(put["set"]["version"], 1);
    let (status, got) = send(&env.app, "GET", &env.set(), None, Some(&env.admin)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        (got["version"].as_u64(), got["status"].as_str(), got["origin"].as_str(), got["kind"].as_str()),
        (Some(1), Some("live"), Some("save"), Some("tenant"))
    );
    assert_eq!(got["compiled"]["cedar_policies"], STARTER_TOOL_POLICY_SOURCE);

    // The same state over gRPC.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);
    let grpc_router = grpc::router(env.state.clone(), Duration::from_secs(10)).await;
    let server = tokio::spawn(async move { grpc_router.serve_with_incoming(incoming).await.unwrap() });
    let ch = tonic::transport::Endpoint::new(format!("http://{addr}")).unwrap().connect().await.unwrap();
    let mut request = tonic::Request::new(GetToolPolicySetRequest {
        org_prn: env.org_prn.clone(),
        version: None,
    });
    support::grpc_bearer(&mut request, &env.admin);
    let over_grpc = ToolPolicyServiceClient::new(ch).get_tool_policy_set(request).await.unwrap().into_inner().set.unwrap();
    server.abort();
    assert_eq!(over_grpc.version, 1);
    assert_eq!(over_grpc.version, put["set"]["version"].as_u64().unwrap());
}

#[tokio::test]
async fn the_floor_routes_authorize_at_root_only() {
    let Some(env) = setup(true, &tenant_accepted()).await else { return };
    let org_admin = env.token_with("tp-org-admin", "org_admin").await;
    let (status, body) = send(&env.app, "GET", "/v1/tool-policy-floor", None, Some(&org_admin)).await;
    assert_eq!((status, body["error"]["code"].as_str()), (StatusCode::FORBIDDEN, Some("forbidden")));
    let (status, _) = send(&env.app, "PUT", "/v1/tool-policy-floor", Some(json!({"source": "", "expected_version": 0})), Some(&org_admin)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, body) = send(&env.app, "GET", "/v1/tool-policy-floor", None, Some(&env.admin)).await;
    assert_eq!((status, body["version"].as_u64()), (StatusCode::OK, Some(0)));
}

#[tokio::test]
async fn a_bad_kind_or_format_is_a_validation_error() {
    let Some(env) = setup(true, &tenant_accepted()).await else { return };
    let (status, body) = send(
        &env.app,
        "POST",
        &env.tools(),
        Some(json!({"name": "x", "kind": "bogus", "expected_catalog_revision": 0})),
        Some(&env.admin),
    )
    .await;
    assert_eq!((status, body["error"]["code"].as_str()), (StatusCode::UNPROCESSABLE_ENTITY, Some("invalid-request-schema")));
    let (status, _) = send(
        &env.app,
        "POST",
        &format!("/v1/organizations/{}/tool-imports", env.org),
        Some(json!({"format": "bogus", "document": [], "expected_catalog_revision": 0})),
        Some(&env.admin),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}
