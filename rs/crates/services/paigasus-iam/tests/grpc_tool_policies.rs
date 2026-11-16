// SPDX-License-Identifier: Apache-2.0

//! `ToolPolicyService` over gRPC (SMA-774 Task 13, spec sections 6.2, 7 and 9.1). Docker-gated
//! like the other gRPC suites. The compiler is a shell script in a temporary directory: its
//! answer to `compile` is the file `result.json`, which a test can change between calls.

mod support;

use std::net::SocketAddr;
use std::time::Duration;

use paigasus_iam::adapters::grpc;
use paigasus_iam::adapters::http::AppState;
use paigasus_iam::adapters::id::KernelIdGenerator;
use paigasus_iam_core::tool_policy::{STARTER_TOOL_POLICY_SOURCE, base_schema_json, tenant_schema_json};
use paigasus_iam_core::{IdGenerator, PrincipalId};
use paigasus_kernel::Prn;
use paigasus_proto::paigasus::common::v1::{Capability, ErrorReason};
use paigasus_proto::paigasus::iam::v1::tool_policy_service_client::ToolPolicyServiceClient;
use paigasus_proto::paigasus::iam::v1::{
    DeleteToolRequest, GetFloorPolicySetRequest, GetToolPolicySetRequest, ImportToolsRequest, ListToolsRequest, PutFloorPolicySetRequest, PutToolPolicySetRequest, PutToolRequest, ToolImportFormat,
    ToolKind, ToolPolicySetKind, ToolPolicyVersionOrigin, ToolPolicyVersionStatus, delete_tool_response, import_tools_response, put_floor_policy_set_response, put_tool_policy_set_response,
    put_tool_response,
};
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use serde_json::json;
use support::{MockIdp, accepted_result, fake_compiler_script, rejected_result, set_compiler_result};
use tempfile::TempDir;
use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::testcontainers::ContainerAsync;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tonic::transport::Channel;
use tonic::{Code, Request, Status};

const FLOOR_DENY: &str = "@id(\"floor-deny\")\nforbid(principal, action in Pgs::Gateway::Action::\"Unregistered\", resource);\n";

struct Env {
    _node: ContainerAsync<Postgres>,
    dir: TempDir,
    state: AppState,
    idp: MockIdp,
    ch: Channel,
    server: JoinHandle<()>,
    /// The token of a platform admin. It also owns the organization.
    admin: String,
    org_prn: String,
}

impl Env {
    fn client(&self) -> ToolPolicyServiceClient<Channel> {
        ToolPolicyServiceClient::new(self.ch.clone())
    }

    /// A principal with no grant but the given role at the organization, and its token.
    async fn principal_with(&self, sub: &str, role: &str) -> String {
        let token = self.idp.bearer(sub, Some(&format!("{sub}@example.com")), "paigasus", 3600);
        let prn = support::provision(&self.state, &token).await;
        support::seed_org_role(&self.state, &prn, &self.org_prn, role).await;
        token
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        self.server.abort();
    }
}

fn tenant_accepted() -> String {
    accepted_result(STARTER_TOOL_POLICY_SOURCE, &tenant_schema_json(&[]))
}

fn floor_accepted() -> String {
    accepted_result(FLOOR_DENY, &base_schema_json())
}

/// `enabled`: the config toggle. `compiler_present`: whether the script exists.
async fn setup(enabled: bool, compiler_present: bool, result: &str) -> Option<Env> {
    let (node, db) = support::start_migrated_postgres().await?;
    let dir = TempDir::new().expect("tempdir");
    let path = if compiler_present {
        fake_compiler_script(dir.path(), result)
    } else {
        dir.path().join("no-such-compiler")
    };
    let (state, idp) = support::state_with_tool_policies(db, enabled, &path).await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);
    let router = grpc::router(state.clone(), Duration::from_secs(10)).await;
    let server = tokio::spawn(async move {
        router.serve_with_incoming(incoming).await.unwrap();
    });
    let ch = tonic::transport::Endpoint::new(format!("http://{addr}")).unwrap().connect().await.unwrap();
    let admin = idp.bearer("tp-admin", Some("tp-admin@example.com"), "paigasus", 3600);
    let admin_prn = support::provision_platform_admin(&state, &admin).await;
    let out = state
        .orgs
        .create(&PrincipalId::from_prn(Prn::parse(&admin_prn).unwrap()), "acme", "Acme")
        .await
        .expect("create the organization");
    let org_prn = out.organization.id.canonical();
    Some(Env {
        _node: node,
        dir,
        state,
        idp,
        ch,
        server,
        admin,
        org_prn,
    })
}

fn authed<T>(msg: T, token: &str) -> Request<T> {
    let mut req = Request::new(msg);
    support::grpc_bearer(&mut req, token);
    req
}

fn put_tool(org: &str, name: &str, expected: u64) -> PutToolRequest {
    PutToolRequest {
        org_prn: org.to_string(),
        name: name.to_string(),
        kind: ToolKind::Function as i32,
        input_schema_json: String::new(),
        expected_catalog_revision: expected,
    }
}

fn put_set(org: &str, source: &str, expected: u64) -> PutToolPolicySetRequest {
    PutToolPolicySetRequest {
        org_prn: org.to_string(),
        source: source.to_string(),
        expected_version: expected,
    }
}

fn get_set(org: &str, version: Option<u64>) -> GetToolPolicySetRequest {
    GetToolPolicySetRequest { org_prn: org.to_string(), version }
}

fn assert_status(status: &Status, code: Code, reason: &str) {
    assert_eq!((status.code(), support::reason_of(status).as_str()), (code, reason), "{status:?}");
}

fn retryable_of(status: &Status) -> String {
    let details = tonic_types::StatusExt::get_error_details(status);
    details.error_info().expect("ErrorInfo").metadata.get("retryable").cloned().unwrap_or_default()
}

// ---- the capability -------------------------------------------------------------------------

/// All eight RPCs, each with a valid shape, called by a platform admin.
async fn call_all_eight(env: &Env) -> Vec<(&'static str, Status)> {
    let mut c = env.client();
    let t = &env.admin;
    let org = &env.org_prn;
    vec![
        ("PutTool", c.put_tool(authed(put_tool(org, "search", 0), t)).await.unwrap_err()),
        (
            "DeleteTool",
            c.delete_tool(authed(
                DeleteToolRequest {
                    org_prn: org.clone(),
                    name: "search".into(),
                    expected_catalog_revision: 0,
                },
                t,
            ))
            .await
            .unwrap_err(),
        ),
        (
            "ListTools",
            c.list_tools(authed(
                ListToolsRequest {
                    org_prn: org.clone(),
                    limit: 0,
                    offset: 0,
                },
                t,
            ))
            .await
            .unwrap_err(),
        ),
        (
            "ImportTools",
            c.import_tools(authed(
                ImportToolsRequest {
                    org_prn: org.clone(),
                    format: ToolImportFormat::OpenaiResponses as i32,
                    document_json: "[]".into(),
                    namespace: String::new(),
                    replace: false,
                    expected_catalog_revision: 0,
                },
                t,
            ))
            .await
            .unwrap_err(),
        ),
        ("PutToolPolicySet", c.put_tool_policy_set(authed(put_set(org, "", 0), t)).await.unwrap_err()),
        ("GetToolPolicySet", c.get_tool_policy_set(authed(get_set(org, None), t)).await.unwrap_err()),
        (
            "PutFloorPolicySet",
            c.put_floor_policy_set(authed(
                PutFloorPolicySetRequest {
                    source: String::new(),
                    expected_version: 0,
                },
                t,
            ))
            .await
            .unwrap_err(),
        ),
        ("GetFloorPolicySet", c.get_floor_policy_set(authed(GetFloorPolicySetRequest { version: None }, t)).await.unwrap_err()),
    ]
}

fn assert_all_capability_disabled(answers: &[(&'static str, Status)]) {
    assert_eq!(answers.len(), 8);
    for (rpc, status) in answers {
        assert_status(status, Code::Unimplemented, &support::wire(ErrorReason::CapabilityDisabled));
        let details = tonic_types::StatusExt::get_error_details(status);
        assert_eq!(details.error_info().unwrap().metadata.get("capability").map(String::as_str), Some("iam.tool.policies"), "{rpc}");
    }
}

#[tokio::test]
async fn the_eight_rpcs_answer_capability_disabled_when_the_toggle_is_off() {
    let Some(env) = setup(false, true, &tenant_accepted()).await else { return };
    assert!(!env.state.capabilities.tool_policies);
    assert!(!env.state.capabilities.enabled().contains(&Capability::IamToolPolicies));
    assert_all_capability_disabled(&call_all_eight(&env).await);
}

#[tokio::test]
async fn the_rpcs_answer_capability_disabled_when_the_binary_is_missing() {
    // The toggle is on, but the probe finds no binary: IAM boots, and the eight RPCs are off.
    let Some(env) = setup(true, false, "").await else { return };
    assert!(!env.state.capabilities.tool_policies);
    assert!(!env.state.capabilities.enabled().contains(&Capability::IamToolPolicies));
    assert_all_capability_disabled(&call_all_eight(&env).await);
}

#[tokio::test]
async fn a_working_compiler_and_the_toggle_turn_the_capability_on() {
    let Some(env) = setup(true, true, &tenant_accepted()).await else { return };
    assert!(env.state.capabilities.tool_policies);
    assert!(env.state.capabilities.enabled().contains(&Capability::IamToolPolicies));
    let listed = env
        .client()
        .list_tools(authed(
            ListToolsRequest {
                org_prn: env.org_prn.clone(),
                limit: 0,
                offset: 0,
            },
            &env.admin,
        ))
        .await
        .expect("the RPC is on");
    assert_eq!(listed.into_inner().catalog_revision, 0);
}

// ---- the errors -----------------------------------------------------------------------------

#[tokio::test]
async fn every_error_code_reaches_the_wire_with_its_grpc_code() {
    let Some(env) = setup(true, true, &tenant_accepted()).await else { return };
    let mut c = env.client();
    let org = env.org_prn.clone();

    // forbidden: a member holds no ManageToolCatalog.
    let member = env.principal_with("tp-member", "org_member").await;
    let status = c.put_tool(authed(put_tool(&org, "search", 0), &member)).await.unwrap_err();
    assert_status(&status, Code::PermissionDenied, "forbidden");

    // not-found: an organization that does not exist.
    let ghost = KernelIdGenerator.new_organization_id().canonical();
    let status = c.put_tool(authed(put_tool(&ghost, "search", 0), &env.admin)).await.unwrap_err();
    assert_status(&status, Code::NotFound, "not-found");

    // tool-catalog-conflict: the same match key with another display name.
    c.put_tool(authed(put_tool(&org, "Search", 0), &env.admin)).await.expect("the first tool");
    let status = c.put_tool(authed(put_tool(&org, "search", 1), &env.admin)).await.unwrap_err();
    assert_status(&status, Code::AlreadyExists, "tool-catalog-conflict");

    // tool-policy-version-conflict: the set has no version 5.
    let status = c.put_tool_policy_set(authed(put_set(&org, STARTER_TOOL_POLICY_SOURCE, 5), &env.admin)).await.unwrap_err();
    assert_status(&status, Code::Aborted, "tool-policy-version-conflict");

    // tool-policy-version-not-found.
    let status = c.get_tool_policy_set(authed(get_set(&org, Some(9)), &env.admin)).await.unwrap_err();
    assert_status(&status, Code::NotFound, "tool-policy-version-not-found");

    // tool-policy-compiler-unavailable: the compiler answers with a text that is not JSON.
    set_compiler_result(env.dir.path(), "this is not json");
    let status = c.put_tool_policy_set(authed(put_set(&org, STARTER_TOOL_POLICY_SOURCE, 0), &env.admin)).await.unwrap_err();
    assert_status(&status, Code::Unavailable, "tool-policy-compiler-unavailable");

    // tool-policy-compiler-busy: three saves at once on a slow compiler with two permits.
    set_compiler_result(env.dir.path(), &tenant_accepted());
    support::make_compiler_slow(env.dir.path());
    let (mut c1, mut c2, mut c3) = (env.client(), env.client(), env.client());
    let (a, b, d) = tokio::join!(
        c1.put_tool_policy_set(authed(put_set(&org, STARTER_TOOL_POLICY_SOURCE, 0), &env.admin)),
        c2.put_tool_policy_set(authed(put_set(&org, STARTER_TOOL_POLICY_SOURCE, 0), &env.admin)),
        c3.put_tool_policy_set(authed(put_set(&org, STARTER_TOOL_POLICY_SOURCE, 0), &env.admin)),
    );
    let busy: Vec<Status> = [a, b, d].into_iter().filter_map(Result::err).filter(|s| s.code() == Code::ResourceExhausted).collect();
    assert!(!busy.is_empty(), "one of three saves must meet a full compiler");
    assert_status(&busy[0], Code::ResourceExhausted, "tool-policy-compiler-busy");
    assert_eq!(retryable_of(&busy[0]), "true");
    std::fs::remove_file(env.dir.path().join("slow")).expect("remove the slow marker");

    // tool-policy-recompile-pending: the live set comes from an older compiler generation.
    env.state
        .db
        .execute_raw(Statement::from_string(DbBackend::Postgres, "UPDATE tool_policy_set_version SET compiler_generation = 0".to_string()))
        .await
        .expect("age the live version");
    let status = c.put_tool(authed(put_tool(&org, "other", 1), &env.admin)).await.unwrap_err();
    assert_status(&status, Code::FailedPrecondition, "tool-policy-recompile-pending");
}

#[tokio::test]
async fn every_rejection_reason_reaches_the_wire_in_the_rejected_oneof() {
    let six = [
        "tool-policy-invalid",
        "tool-policy-shape-unsupported",
        "tool-policy-lowering-failed",
        "tool-policy-floor-permit",
        "tool-policy-id-invalid",
        "tool-policy-window-too-long",
    ];
    let compiler_rejections: Vec<(&str, &str)> = six.iter().map(|reason| (*reason, "the message")).collect();
    let Some(env) = setup(true, true, &rejected_result(&compiler_rejections)).await else { return };
    let mut c = env.client();
    let org = env.org_prn.clone();

    // The six compiler reasons.
    let response = c.put_tool_policy_set(authed(put_set(&org, "x", 0), &env.admin)).await.expect("a rejection is a result").into_inner();
    let Some(put_tool_policy_set_response::Result::Rejected(rejected)) = response.result else {
        panic!("expected rejected, got {response:?}")
    };
    let got: Vec<&str> = rejected.rejections.iter().map(|r| r.reason.as_str()).collect();
    assert_eq!(got, six);
    let first = &rejected.rejections[0];
    assert_eq!((first.policy_id.as_str(), first.line, first.column, first.message.as_str()), ("p1", 2, 5, "the message"));

    // tool-catalog-invalid: a name outside the grammar.
    let response = c.put_tool(authed(put_tool(&org, "bad name", 0), &env.admin)).await.expect("a rejection is a result").into_inner();
    let Some(put_tool_response::Result::Rejected(rejected)) = response.result else {
        panic!("expected rejected, got {response:?}")
    };
    assert_eq!(rejected.rejections[0].reason, "tool-catalog-invalid");

    // tool-catalog-breaks-policy: a live set that no longer compiles against the new catalog.
    set_compiler_result(env.dir.path(), &tenant_accepted());
    let saved = c.put_tool_policy_set(authed(put_set(&org, STARTER_TOOL_POLICY_SOURCE, 0), &env.admin)).await.unwrap().into_inner();
    assert!(matches!(saved.result, Some(put_tool_policy_set_response::Result::Accepted(_))));
    set_compiler_result(env.dir.path(), &rejected_result(&[("tool-policy-invalid", "the policy names a tool that is gone")]));
    let response = c.put_tool(authed(put_tool(&org, "search", 0), &env.admin)).await.expect("a rejection is a result").into_inner();
    let Some(put_tool_response::Result::Rejected(rejected)) = response.result else {
        panic!("expected rejected, got {response:?}")
    };
    assert_eq!(rejected.rejections[0].reason, "tool-catalog-breaks-policy");
}

// ---- the sets -------------------------------------------------------------------------------

#[tokio::test]
async fn a_saved_set_round_trips_with_its_compiled_message() {
    let Some(env) = setup(true, true, &tenant_accepted()).await else { return };
    let mut c = env.client();
    let org = env.org_prn.clone();
    // The catalog holds two tools, so `catalog_names` has two entries, sorted by match key.
    c.put_tool(authed(put_tool(&org, "Zeta", 0), &env.admin)).await.unwrap();
    c.put_tool(authed(put_tool(&org, "alpha", 1), &env.admin)).await.unwrap();

    let saved = c.put_tool_policy_set(authed(put_set(&org, STARTER_TOOL_POLICY_SOURCE, 0), &env.admin)).await.unwrap().into_inner();
    let Some(put_tool_policy_set_response::Result::Accepted(saved)) = saved.result else {
        panic!("expected accepted")
    };
    assert_eq!(saved.version, 1);

    let got = c.get_tool_policy_set(authed(get_set(&org, None), &env.admin)).await.unwrap().into_inner().set.expect("a set");
    assert_eq!(got.version, 1);
    assert_eq!(got.source, STARTER_TOOL_POLICY_SOURCE);
    assert_eq!(got.status, ToolPolicyVersionStatus::Live as i32);
    assert_eq!(got.origin, ToolPolicyVersionOrigin::Save as i32);
    assert_eq!(got.kind, ToolPolicySetKind::Tenant as i32);
    assert_eq!(got.catalog_revision, 2);
    assert_eq!(got.org_id, support::principal_uuid(&org).to_string());
    assert!(got.created_by.starts_with("prn:"), "{}", got.created_by);
    let compiled = got.compiled.expect("the compiled message");
    assert_eq!(compiled.version, 1);
    assert_eq!(compiled.cedar_policies, STARTER_TOOL_POLICY_SOURCE);
    assert_eq!(compiled.catalog_names, vec!["alpha".to_string(), "zeta".to_string()]);
    assert_eq!(compiled.kind, ToolPolicySetKind::Tenant as i32);

    // The same version by number.
    let again = c.get_tool_policy_set(authed(get_set(&org, Some(1)), &env.admin)).await.unwrap().into_inner().set.unwrap();
    assert_eq!(again.version, 1);
}

#[tokio::test]
async fn get_tool_policy_set_with_no_set_returns_version_0() {
    let Some(env) = setup(true, true, &tenant_accepted()).await else { return };
    let got = env.client().get_tool_policy_set(authed(get_set(&env.org_prn, None), &env.admin)).await.unwrap().into_inner();
    let set = got.set.expect("an empty set is still a message");
    assert_eq!((set.version, set.source.as_str(), set.compiled), (0, "", None));
}

#[tokio::test]
async fn import_and_delete_report_the_stored_tools() {
    let Some(env) = setup(true, true, &tenant_accepted()).await else { return };
    let mut c = env.client();
    let org = env.org_prn.clone();
    let document = json!([
        {"type": "function", "name": "search", "parameters": {"type": "object", "properties": {"q": {"type": "string"}}}},
        {"type": "web_search"}
    ]);
    let response = c
        .import_tools(authed(
            ImportToolsRequest {
                org_prn: org.clone(),
                format: ToolImportFormat::OpenaiResponses as i32,
                document_json: document.to_string(),
                namespace: String::new(),
                replace: false,
                expected_catalog_revision: 0,
            },
            &env.admin,
        ))
        .await
        .unwrap()
        .into_inner();
    let Some(import_tools_response::Result::Accepted(accepted)) = response.result else {
        panic!("expected accepted, got {response:?}")
    };
    assert_eq!(accepted.catalog_revision, 1);
    assert_eq!(accepted.tools.iter().map(|t| t.name_key.as_str()).collect::<Vec<_>>(), vec!["search"]);
    assert_eq!((accepted.skipped[0].index, accepted.skipped[0].r#type.as_str()), (1, "web_search"));

    let listed = c
        .list_tools(authed(
            ListToolsRequest {
                org_prn: org.clone(),
                limit: 0,
                offset: 0,
            },
            &env.admin,
        ))
        .await
        .unwrap()
        .into_inner();
    assert_eq!((listed.tools.len(), listed.catalog_revision), (1, 1));

    let deleted = c
        .delete_tool(authed(
            DeleteToolRequest {
                org_prn: org,
                name: "SEARCH".into(),
                expected_catalog_revision: 1,
            },
            &env.admin,
        ))
        .await
        .unwrap()
        .into_inner();
    let Some(delete_tool_response::Result::Accepted(accepted)) = deleted.result else {
        panic!("expected accepted")
    };
    assert_eq!(accepted.catalog_revision, 2);
}

// ---- authorization --------------------------------------------------------------------------

#[tokio::test]
async fn org_member_can_list_tools_but_not_read_policies() {
    let Some(env) = setup(true, true, &tenant_accepted()).await else { return };
    let member = env.principal_with("tp-member", "org_member").await;
    let mut c = env.client();
    let listed = c
        .list_tools(authed(
            ListToolsRequest {
                org_prn: env.org_prn.clone(),
                limit: 0,
                offset: 0,
            },
            &member,
        ))
        .await
        .expect("a member reads the catalog");
    assert_eq!(listed.into_inner().catalog_revision, 0);
    let status = c.get_tool_policy_set(authed(get_set(&env.org_prn, None), &member)).await.unwrap_err();
    assert_status(&status, Code::PermissionDenied, "forbidden");
}

#[tokio::test]
async fn only_platform_admin_can_read_the_floor() {
    let Some(env) = setup(true, true, &floor_accepted()).await else { return };
    let mut c = env.client();
    let got = c
        .get_floor_policy_set(authed(GetFloorPolicySetRequest { version: None }, &env.admin))
        .await
        .expect("a platform admin reads the floor")
        .into_inner();
    assert_eq!(got.set.expect("an empty floor is a message").version, 0);
    for (sub, role) in [("tp-org-admin", "org_admin"), ("tp-member", "org_member")] {
        let token = env.principal_with(sub, role).await;
        let status = c.get_floor_policy_set(authed(GetFloorPolicySetRequest { version: None }, &token)).await.unwrap_err();
        assert_status(&status, Code::PermissionDenied, "forbidden");
    }
}

/// The floor RPCs carry no organization. The service authorizes them at Root, so an org admin,
/// who holds every tool policy action at the organization, is still denied (carried item 3).
#[tokio::test]
async fn a_floor_call_by_an_org_admin_is_denied() {
    let Some(env) = setup(true, true, &floor_accepted()).await else { return };
    let org_admin = env.principal_with("tp-org-admin", "org_admin").await;
    let mut c = env.client();
    // The same org admin can write the tenant set of its organization.
    set_compiler_result(env.dir.path(), &tenant_accepted());
    let tenant = c
        .put_tool_policy_set(authed(put_set(&env.org_prn, STARTER_TOOL_POLICY_SOURCE, 0), &org_admin))
        .await
        .expect("the org admin writes the tenant set");
    assert!(matches!(tenant.into_inner().result, Some(put_tool_policy_set_response::Result::Accepted(_))));

    set_compiler_result(env.dir.path(), &floor_accepted());
    let request = PutFloorPolicySetRequest {
        source: FLOOR_DENY.to_string(),
        expected_version: 0,
    };
    let status = c.put_floor_policy_set(authed(request.clone(), &org_admin)).await.unwrap_err();
    assert_status(&status, Code::PermissionDenied, "forbidden");

    // The platform admin writes it.
    let written = c.put_floor_policy_set(authed(request, &env.admin)).await.expect("a platform admin writes the floor").into_inner();
    let Some(put_floor_policy_set_response::Result::Accepted(version)) = written.result else {
        panic!("expected accepted, got {written:?}")
    };
    assert_eq!((version.version, version.kind), (1, ToolPolicySetKind::Floor as i32));
    assert_eq!(version.org_id, "");
}
