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
use paigasus_proto::paigasus::iam::v1::tool_policy_service_client::ToolPolicyServiceClient;
use paigasus_proto::paigasus::iam::v1::{GetCompiledToolPoliciesRequest, GetToolPolicySetRequest};
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use serde_json::{Value, json};
use support::{MockIdp, accepted_result, fake_compiler_script, rejected_result, send, send_bytes, set_compiler_result};
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
    let (app, state, idp) = if enabled {
        support::app_with_tool_policies(db, &path).await
    } else {
        let (state, idp) = support::state_with_tool_policies(db, false, &path).await;
        (router(state.clone()), state, idp)
    };
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

    // tool-policy-compiler-busy (429): two held compiles take both permits, then a third save comes.
    set_compiler_result(env.dir.path(), &tenant_accepted());
    support::hold_compiler(env.dir.path());
    let set_uri = env.set();
    let put = || send(&env.app, "PUT", &set_uri, Some(json!({"source": STARTER_TOOL_POLICY_SOURCE, "expected_version": 0})), Some(&env.admin));
    let (a, b, c) = tokio::join!(put(), put(), async {
        support::wait_for_held_compiles(env.dir.path(), 2).await;
        let third = put().await;
        support::release_compiler(env.dir.path());
        third
    });
    let busy: Vec<_> = [a, b, c].into_iter().filter(|(status, _)| *status == StatusCode::TOO_MANY_REQUESTS).collect();
    assert!(!busy.is_empty(), "one of three saves must meet a full compiler");
    assert_eq!(code(&busy[0].1), "tool-policy-compiler-busy");

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

/// The compile result fixture that holds two history conditions.
fn fixture_result() -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../tools/paigasus-policy-compiler/protocol/fixtures/result_tenant_accepted.json");
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

// SMA-753: IAM refuses a set with a history condition until the gateway evaluates one. When
// SMA-753 lifts that, restore the save-and-read test that renders `compiled.conditions` here.
// A unit test in `src/adapters/http/tool_policies.rs` covers the rendering until then.
#[tokio::test]
async fn a_history_rule_is_200_with_result_rejected_and_its_code() {
    let Some(env) = setup(true, &fixture_result()).await else { return };
    let (status, put) = send(&env.app, "PUT", &env.set(), Some(json!({"source": "x", "expected_version": 0})), Some(&env.admin)).await;
    assert_eq!((status, put["result"].as_str()), (StatusCode::OK, Some("rejected")), "{put}");
    let rejections = put["rejections"].as_array().expect("rejections");
    let reasons: Vec<&str> = rejections.iter().map(|r| r["reason"].as_str().unwrap_or_default()).collect();
    let ids: Vec<&str> = rejections.iter().map(|r| r["policy_id"].as_str().unwrap_or_default()).collect();
    assert_eq!(reasons, vec!["tool-policy-history-unsupported"; 2]);
    assert_eq!(ids, vec!["no-send-after-search", "search-rate"]);
}

#[tokio::test]
async fn malformed_input_answers_in_the_error_envelope() {
    let Some(env) = setup(true, &tenant_accepted()).await else { return };
    let code = |body: &Value| body["error"]["code"].as_str().unwrap_or_default().to_string();

    // A malformed organization uuid, on a one-segment route and on the two-segment route.
    let (status, body) = send(&env.app, "GET", "/v1/organizations/not-a-uuid/tools", None, Some(&env.admin)).await;
    assert_eq!((status, code(&body).as_str()), (StatusCode::BAD_REQUEST, "invalid-uuid"));
    let (status, body) = send(&env.app, "DELETE", "/v1/organizations/not-a-uuid/tools/x?expected_catalog_revision=0", None, Some(&env.admin)).await;
    assert_eq!((status, code(&body).as_str()), (StatusCode::BAD_REQUEST, "invalid-uuid"));

    // A tool segment that is not UTF-8 after percent-decoding names the tool segment.
    let uri = format!("{}/%FF?expected_catalog_revision=0", env.tools());
    let (status, body) = send(&env.app, "DELETE", &uri, None, Some(&env.admin)).await;
    assert_eq!((status, code(&body).as_str()), (StatusCode::BAD_REQUEST, "invalid-path-segment"));
    assert!(body["error"]["message"].as_str().unwrap_or_default().contains("tool_name"), "{body}");

    // An organization segment that is not UTF-8 names the organization segment, not the tool.
    let (status, body) = send(&env.app, "DELETE", "/v1/organizations/%FF/tools/x?expected_catalog_revision=0", None, Some(&env.admin)).await;
    assert_eq!((status, code(&body).as_str()), (StatusCode::BAD_REQUEST, "invalid-path-segment"));
    assert_eq!(body["error"]["message"].as_str().unwrap_or_default(), "organization_id is not a valid path segment", "{body}");

    // A body that is not JSON.
    let (status, body) = send_bytes(&env.app, "POST", &env.tools(), Some("application/json"), b"{not json", Some(&env.admin)).await;
    assert_eq!((status, code(&body).as_str()), (StatusCode::BAD_REQUEST, "invalid-request-body"));
    let (status, body) = send_bytes(&env.app, "PUT", &env.set(), Some("application/json"), b"{not json", Some(&env.admin)).await;
    assert_eq!((status, code(&body).as_str()), (StatusCode::BAD_REQUEST, "invalid-request-body"));
}

// ---- the settings and the compiled-policy fetch (SMA-775 Task 9) -----------------------------

impl Env {
    fn settings(&self) -> String {
        format!("/v1/organizations/{}/tool-policy-settings", self.org)
    }

    fn compiled(&self) -> String {
        format!("/v1/organizations/{}/compiled-tool-policies", self.org)
    }
}

#[tokio::test]
async fn the_settings_and_fetch_routes_are_404_when_the_capability_is_off() {
    for (enabled, present) in [(false, true), (true, false)] {
        let Some(env) = setup_with(enabled, present, &tenant_accepted()).await else { return };
        assert!(!env.state.capabilities.tool_policies);
        let calls: Vec<(&str, String, Option<Value>)> = vec![
            ("GET", env.settings(), None),
            ("PUT", env.settings(), Some(json!({"mode": "log_only", "fail_mode": "closed"}))),
            ("GET", env.compiled(), None),
            ("GET", "/v1/compiled-tool-policy-floor".to_string(), None),
        ];
        for (method, uri, body) in calls {
            let (status, _) = send(&env.app, method, &uri, body, Some(&env.admin)).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{method} {uri} (enabled={enabled})");
        }
    }
}

#[tokio::test]
async fn http_and_grpc_return_the_same_compiled_set() {
    let Some(env) = setup(true, &tenant_accepted()).await else { return };
    let (status, none) = send(&env.app, "GET", &env.settings(), None, Some(&env.admin)).await;
    assert_eq!(
        (status, none["mode"].as_str(), none["fail_mode"].as_str(), none["updated_at"].is_null()),
        (StatusCode::OK, Some("off"), Some("closed"), true)
    );
    let (status, put) = send(&env.app, "PUT", &env.settings(), Some(json!({"mode": "log_only", "fail_mode": "open"})), Some(&env.admin)).await;
    assert_eq!(status, StatusCode::OK, "{put}");
    assert_eq!(
        (put["starter_version"].as_u64(), put["settings"]["mode"].as_str(), put["settings"]["fail_mode"].as_str()),
        (Some(1), Some("log_only"), Some("open"))
    );

    let (status, http) = send(&env.app, "GET", &env.compiled(), None, Some(&env.admin)).await;
    assert_eq!(status, StatusCode::OK, "{http}");
    assert_eq!(
        (http["set"]["version"].as_u64(), http["floor_version"].as_u64(), http["settings"]["mode"].as_str()),
        (Some(1), Some(0), Some("log_only"))
    );
    let (status, floor) = send(&env.app, "GET", "/v1/compiled-tool-policy-floor", None, Some(&env.admin)).await;
    assert_eq!(
        (status, floor["set"]["version"].as_u64(), floor["set"]["kind"].as_str()),
        (StatusCode::OK, Some(0), Some("TOOL_POLICY_SET_KIND_FLOOR"))
    );
    assert!(floor["set"]["cedar_schema"].is_null(), "the empty floor has a null schema: {floor}");
    assert!(!http["set"]["cedar_schema"].is_null(), "a stored version keeps its schema");

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);
    let grpc_router = grpc::router(env.state.clone(), Duration::from_secs(10)).await;
    let server = tokio::spawn(async move { grpc_router.serve_with_incoming(incoming).await.unwrap() });
    let ch = tonic::transport::Endpoint::new(format!("http://{addr}")).unwrap().connect().await.unwrap();
    let mut request = tonic::Request::new(GetCompiledToolPoliciesRequest {
        org_prn: env.org_prn.clone(),
        version: None,
    });
    support::grpc_bearer(&mut request, &env.admin);
    let over_grpc = ToolPolicyServiceClient::new(ch).get_compiled_tool_policies(request).await.unwrap().into_inner();
    server.abort();
    let set = over_grpc.set.expect("a set");
    assert_eq!(set.version, http["set"]["version"].as_u64().unwrap());
    assert_eq!(set.cedar_policies, http["set"]["cedar_policies"].as_str().unwrap());
    assert_eq!(set.catalog_revision, http["set"]["catalog_revision"].as_u64().unwrap());
    assert_eq!(over_grpc.floor_version, http["floor_version"].as_u64().unwrap());
}

#[tokio::test]
async fn a_mode_transition_error_is_409_with_its_code() {
    let Some(env) = setup(true, &tenant_accepted()).await else { return };
    let (status, body) = send(&env.app, "PUT", &env.settings(), Some(json!({"mode": "enforce", "fail_mode": "closed"})), Some(&env.admin)).await;
    assert_eq!((status, body["error"]["code"].as_str()), (StatusCode::CONFLICT, Some("tool-policy-mode-transition-invalid")), "{body}");
}

// ---- the held versions and the re-compile run (SMA-781 Task 8) ---------------------------------

const OTHER_SOURCE: &str = "@id(\"other\")\nforbid(principal, action, resource);\n";

impl Env {
    /// Enables the org, then makes its live version stale (raw SQL, no API path does it).
    async fn make_stale(&self) {
        set_compiler_result(self.dir.path(), &tenant_accepted());
        let (status, put) = send(&self.app, "PUT", &self.settings(), Some(json!({"mode": "log_only", "fail_mode": "closed"})), Some(&self.admin)).await;
        assert_eq!(status, StatusCode::OK, "{put}");
        self.state
            .db
            .execute_raw(Statement::from_string(DbBackend::Postgres, "UPDATE tool_policy_set_version SET compiler_generation = 0".to_string()))
            .await
            .expect("age the live version");
    }

    /// A live tenant version 1 at generation 0, then a run that holds another output as version 2.
    async fn make_held(&self) {
        self.make_stale().await;
        set_compiler_result(self.dir.path(), &accepted_result(OTHER_SOURCE, &tenant_schema_json(&[])));
        let (status, run) = send(&self.app, "POST", "/v1/tool-policy-recompile", None, Some(&self.admin)).await;
        assert_eq!(status, StatusCode::OK, "{run}");
        assert_eq!(run["status"], "ran");
        assert_eq!(
            (run["results"][0]["outcome"].as_str(), run["results"][0]["version"].as_u64()),
            (Some("RECOMPILE_OUTCOME_HELD_DIFFERENT"), Some(2)),
            "{run}"
        );
    }

    fn held(&self) -> String {
        format!("/v1/organizations/{}/held-tool-policy-versions", self.org)
    }
}

#[tokio::test]
async fn the_held_and_recompile_routes_are_404_when_the_capability_is_off() {
    for (enabled, present) in [(false, true), (true, false)] {
        let Some(env) = setup_with(enabled, present, &tenant_accepted()).await else { return };
        assert!(!env.state.capabilities.tool_policies);
        let held = env.held();
        let calls: Vec<(&str, String)> = vec![
            ("GET", held.clone()),
            ("GET", "/v1/held-tool-policy-versions".to_string()),
            ("POST", format!("{held}/2/accept")),
            ("POST", format!("{held}/2/discard")),
            ("POST", "/v1/tool-policy-floor/held-versions/2/accept".to_string()),
            ("POST", "/v1/tool-policy-floor/held-versions/2/discard".to_string()),
            ("POST", "/v1/tool-policy-recompile".to_string()),
        ];
        for (method, uri) in calls {
            let (status, _) = send(&env.app, method, &uri, None, Some(&env.admin)).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{method} {uri} (enabled={enabled})");
        }
    }
}

#[tokio::test]
async fn http_and_grpc_return_the_same_held_row() {
    use paigasus_proto::paigasus::iam::v1::ListHeldToolPolicyVersionsRequest;
    let Some(env) = setup(true, &tenant_accepted()).await else { return };
    env.make_held().await;
    let (status, http) = send(&env.app, "GET", &env.held(), None, Some(&env.admin)).await;
    assert_eq!(status, StatusCode::OK, "{http}");
    let (status, all) = send(&env.app, "GET", "/v1/held-tool-policy-versions?limit=10", None, Some(&env.admin)).await;
    assert_eq!((status, all["versions"].as_array().map(Vec::len)), (StatusCode::OK, Some(1)), "{all}");
    let row = &http["versions"][0];
    assert_eq!(
        (row["kind"].as_str(), row["status"].as_str(), row["version"].as_u64(), row["base_version"].as_u64()),
        (Some("tenant"), Some("held"), Some(2), Some(1))
    );
    assert_eq!(row["report"]["result"], "HELD_TOOL_POLICY_RESULT_DIFFERENT");

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);
    let grpc_router = grpc::router(env.state.clone(), Duration::from_secs(10)).await;
    let server = tokio::spawn(async move { grpc_router.serve_with_incoming(incoming).await.unwrap() });
    let ch = tonic::transport::Endpoint::new(format!("http://{addr}")).unwrap().connect().await.unwrap();
    let mut request = tonic::Request::new(ListHeldToolPolicyVersionsRequest {
        org_prn: env.org_prn.clone(),
        limit: 0,
        offset: 0,
    });
    support::grpc_bearer(&mut request, &env.admin);
    let over_grpc = ToolPolicyServiceClient::new(ch).list_held_tool_policy_versions(request).await.unwrap().into_inner();
    server.abort();
    let g = &over_grpc.versions[0];
    assert_eq!(
        (g.version, g.base_version, g.org_id.as_str()),
        (row["version"].as_u64().unwrap(), row["base_version"].as_u64().unwrap(), row["org_id"].as_str().unwrap())
    );
    assert_eq!(g.compiled.as_ref().unwrap().cedar_policies, row["compiled"]["cedar_policies"].as_str().unwrap());
    assert_eq!(g.created_by, row["created_by"].as_str().unwrap());

    // A bad segment is a 400. Accept works. A second discard of the same number is a 404.
    let (status, bad) = send(&env.app, "POST", &format!("{}/abc/accept", env.held()), None, Some(&env.admin)).await;
    assert_eq!((status, bad["error"]["code"].as_str()), (StatusCode::BAD_REQUEST, Some("invalid-path-segment")), "{bad}");
    let (status, accepted) = send(&env.app, "POST", &format!("{}/2/accept", env.held()), None, Some(&env.admin)).await;
    assert_eq!(status, StatusCode::OK, "{accepted}");
    assert_eq!((accepted["accepted"]["version"].as_u64(), accepted["accepted"]["origin"].as_str()), (Some(3), Some("accept")));
    let (status, gone) = send(&env.app, "POST", &format!("{}/2/discard", env.held()), None, Some(&env.admin)).await;
    assert_eq!((status, gone["error"]["code"].as_str()), (StatusCode::NOT_FOUND, Some("tool-policy-version-not-found")), "{gone}");
}

#[tokio::test]
async fn a_held_discard_over_http_returns_the_discarded_row() {
    let Some(env) = setup(true, &tenant_accepted()).await else { return };
    env.make_held().await;
    let member = env.token_with("tp-member", "org_member").await;
    let (status, denied) = send(&env.app, "GET", "/v1/held-tool-policy-versions", None, Some(&member)).await;
    assert_eq!((status, denied["error"]["code"].as_str()), (StatusCode::FORBIDDEN, Some("forbidden")));
    let (status, out) = send(&env.app, "POST", &format!("{}/2/discard", env.held()), None, Some(&env.admin)).await;
    assert_eq!(
        (status, out["discarded"]["status"].as_str(), out["discarded"]["version"].as_u64()),
        (StatusCode::OK, Some("discarded"), Some(2)),
        "{out}"
    );
}

#[tokio::test]
async fn a_held_not_acceptable_accept_is_409_with_its_code() {
    let Some(env) = setup(true, &tenant_accepted()).await else { return };
    env.make_stale().await;
    set_compiler_result(env.dir.path(), &rejected_result(&[("tool-policy-invalid", "no good")]));
    let (status, run) = send(&env.app, "POST", "/v1/tool-policy-recompile", None, Some(&env.admin)).await;
    assert_eq!(status, StatusCode::OK, "{run}");
    let (status, body) = send(&env.app, "POST", &format!("{}/2/accept", env.held()), None, Some(&env.admin)).await;
    assert_eq!((status, body["error"]["code"].as_str()), (StatusCode::CONFLICT, Some("tool-policy-held-not-acceptable")), "{body}");
}
