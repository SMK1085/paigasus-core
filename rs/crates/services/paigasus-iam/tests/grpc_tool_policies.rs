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
    DeleteToolRequest, GetCompiledFloorPoliciesRequest, GetCompiledToolPoliciesRequest, GetFloorPolicySetRequest, GetToolPolicySetRequest, GetToolPolicySettingsRequest, ImportToolsRequest,
    ListToolsRequest, PutFloorPolicySetRequest, PutToolPolicySetRequest, PutToolPolicySettingsRequest, PutToolRequest, ToolImportFormat, ToolKind, ToolPolicyFailMode, ToolPolicyMode,
    ToolPolicySetKind, ToolPolicyVersionOrigin, ToolPolicyVersionStatus, delete_tool_response, import_tools_response, put_floor_policy_set_response, put_tool_policy_set_response, put_tool_response,
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

    // tool-policy-compiler-busy: two held compiles take both permits, then a third save comes.
    set_compiler_result(env.dir.path(), &tenant_accepted());
    support::hold_compiler(env.dir.path());
    let (mut c1, mut c2, mut c3) = (env.client(), env.client(), env.client());
    let (a, b, d) = tokio::join!(
        c1.put_tool_policy_set(authed(put_set(&org, STARTER_TOOL_POLICY_SOURCE, 0), &env.admin)),
        c2.put_tool_policy_set(authed(put_set(&org, STARTER_TOOL_POLICY_SOURCE, 0), &env.admin)),
        async {
            support::wait_for_held_compiles(env.dir.path(), 2).await;
            let third = c3.put_tool_policy_set(authed(put_set(&org, STARTER_TOOL_POLICY_SOURCE, 0), &env.admin)).await;
            support::release_compiler(env.dir.path());
            third
        },
    );
    let busy: Vec<Status> = [a, b, d].into_iter().filter_map(Result::err).filter(|s| s.code() == Code::ResourceExhausted).collect();
    assert!(!busy.is_empty(), "one of three saves must meet a full compiler");
    assert_status(&busy[0], Code::ResourceExhausted, "tool-policy-compiler-busy");
    assert_eq!(retryable_of(&busy[0]), "true");

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

    // tool-policy-history-unsupported: IAM's own reason, for a compiled set with a history
    // condition (SMA-753 removes it).
    set_compiler_result(env.dir.path(), &history_accepted());
    let response = c.put_tool_policy_set(authed(put_set(&org, "x", 1), &env.admin)).await.expect("a rejection is a result").into_inner();
    let Some(put_tool_policy_set_response::Result::Rejected(rejected)) = response.result else {
        panic!("expected rejected, got {response:?}")
    };
    let got: Vec<(&str, &str)> = rejected.rejections.iter().map(|r| (r.reason.as_str(), r.policy_id.as_str())).collect();
    assert_eq!(
        got,
        vec![("tool-policy-history-unsupported", "no-send-after-search"), ("tool-policy-history-unsupported", "search-rate")]
    );
}

/// The compile result fixture that holds two history conditions.
fn history_accepted() -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../tools/paigasus-policy-compiler/protocol/fixtures/result_tenant_accepted.json");
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
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

/// I1 (SMA-774 final review): an org admin of organization A holds every tool policy action at A,
/// and none at B. Each of the six tenant RPCs on B answers `forbidden`, and B's catalog and set do
/// not change. The authorizer is the real Cedar one, so this pins that every handler authorizes
/// at the stored PRN of the organization in the request.
#[tokio::test]
async fn an_org_admin_of_one_org_is_denied_every_tenant_rpc_on_another_org() {
    let Some(env) = setup(true, true, &tenant_accepted()).await else { return };
    let mut c = env.client();
    let org_a = env.org_prn.clone();
    // The admin is already provisioned, so this only returns its PRN.
    let admin_prn = support::provision(&env.state, &env.admin).await;
    let org_b = env
        .state
        .orgs
        .create(&PrincipalId::from_prn(Prn::parse(&admin_prn).unwrap()), "globex", "Globex")
        .await
        .expect("create the second organization")
        .organization
        .id
        .canonical();

    // B holds one tool (catalog revision 1) and a live set at version 1.
    c.put_tool(authed(put_tool(&org_b, "search", 0), &env.admin)).await.expect("seed B's tool");
    let saved = c
        .put_tool_policy_set(authed(put_set(&org_b, STARTER_TOOL_POLICY_SOURCE, 0), &env.admin))
        .await
        .expect("seed B's set")
        .into_inner();
    assert!(matches!(saved.result, Some(put_tool_policy_set_response::Result::Accepted(_))));

    let org_admin = env.principal_with("tp-org-admin-a", "org_admin").await;
    // The control: the same token works on A.
    let own = c
        .put_tool(authed(put_tool(&org_a, "search", 0), &org_admin))
        .await
        .expect("the org admin writes the catalog of its own organization")
        .into_inner();
    assert!(matches!(own.result, Some(put_tool_response::Result::Accepted(_))), "{own:?}");

    let t = &org_admin;
    let answers: Vec<(&str, Status)> = vec![
        ("PutTool", c.put_tool(authed(put_tool(&org_b, "other", 1), t)).await.unwrap_err()),
        (
            "DeleteTool",
            c.delete_tool(authed(
                DeleteToolRequest {
                    org_prn: org_b.clone(),
                    name: "search".into(),
                    expected_catalog_revision: 1,
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
                    org_prn: org_b.clone(),
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
                    org_prn: org_b.clone(),
                    format: ToolImportFormat::OpenaiResponses as i32,
                    document_json: "[]".into(),
                    namespace: String::new(),
                    replace: true,
                    expected_catalog_revision: 1,
                },
                t,
            ))
            .await
            .unwrap_err(),
        ),
        ("PutToolPolicySet", c.put_tool_policy_set(authed(put_set(&org_b, STARTER_TOOL_POLICY_SOURCE, 1), t)).await.unwrap_err()),
        ("GetToolPolicySet", c.get_tool_policy_set(authed(get_set(&org_b, None), t)).await.unwrap_err()),
    ];
    assert_eq!(answers.len(), 6);
    for (rpc, status) in &answers {
        assert_eq!((status.code(), support::reason_of(status).as_str()), (Code::PermissionDenied, "forbidden"), "{rpc}: {status:?}");
    }

    // B is unchanged: one tool at revision 1, and the live set at version 1.
    let listed = c
        .list_tools(authed(
            ListToolsRequest {
                org_prn: org_b.clone(),
                limit: 0,
                offset: 0,
            },
            &env.admin,
        ))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(listed.catalog_revision, 1);
    assert_eq!(listed.tools.iter().map(|t| t.name_key.as_str()).collect::<Vec<_>>(), vec!["search"]);
    let set = c.get_tool_policy_set(authed(get_set(&org_b, None), &env.admin)).await.unwrap().into_inner().set.unwrap();
    assert_eq!(set.version, 1);
    assert_eq!(set.org_id, support::principal_uuid(&org_b).to_string());
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

// ---- the settings and the compiled-policy fetch (SMA-775 Task 9) -----------------------------

fn put_settings(org: &str, mode: ToolPolicyMode, fail_mode: ToolPolicyFailMode) -> PutToolPolicySettingsRequest {
    PutToolPolicySettingsRequest {
        org_prn: org.to_string(),
        mode: mode as i32,
        fail_mode: fail_mode as i32,
    }
}

fn get_compiled(org: &str, version: Option<u64>) -> GetCompiledToolPoliciesRequest {
    GetCompiledToolPoliciesRequest { org_prn: org.to_string(), version }
}

fn get_compiled_floor() -> GetCompiledFloorPoliciesRequest {
    GetCompiledFloorPoliciesRequest { version: None }
}

/// The four new RPCs, each with a valid shape, called by a platform admin.
async fn call_all_four(env: &Env) -> Vec<(&'static str, Status)> {
    let mut c = env.client();
    let t = &env.admin;
    let org = &env.org_prn;
    vec![
        (
            "GetToolPolicySettings",
            c.get_tool_policy_settings(authed(GetToolPolicySettingsRequest { org_prn: org.clone() }, t)).await.unwrap_err(),
        ),
        (
            "PutToolPolicySettings",
            c.put_tool_policy_settings(authed(put_settings(org, ToolPolicyMode::LogOnly, ToolPolicyFailMode::Closed), t))
                .await
                .unwrap_err(),
        ),
        ("GetCompiledToolPolicies", c.get_compiled_tool_policies(authed(get_compiled(org, None), t)).await.unwrap_err()),
        ("GetCompiledFloorPolicies", c.get_compiled_floor_policies(authed(get_compiled_floor(), t)).await.unwrap_err()),
    ]
}

/// A second organization, created by the platform admin. Returns its canonical PRN.
async fn second_org(env: &Env, slug: &str) -> String {
    let admin_prn = support::provision(&env.state, &env.admin).await;
    let out = env
        .state
        .orgs
        .create(&PrincipalId::from_prn(Prn::parse(&admin_prn).unwrap()), slug, slug)
        .await
        .expect("create the second organization");
    out.organization.id.canonical()
}

/// The first enable of `org` to `log_only`. Returns the starter version.
async fn enable_log_only(env: &Env, org: &str) -> u64 {
    set_compiler_result(env.dir.path(), &tenant_accepted());
    env.client()
        .put_tool_policy_settings(authed(put_settings(org, ToolPolicyMode::LogOnly, ToolPolicyFailMode::Closed), &env.admin))
        .await
        .expect("the first enable")
        .into_inner()
        .starter_version
}

#[tokio::test]
async fn the_four_rpcs_answer_capability_disabled_when_the_toggle_is_off() {
    let Some(env) = setup(false, true, &tenant_accepted()).await else { return };
    let answers = call_all_four(&env).await;
    assert_eq!(answers.len(), 4);
    for (rpc, status) in &answers {
        assert_status(status, Code::Unimplemented, &support::wire(ErrorReason::CapabilityDisabled));
        let details = tonic_types::StatusExt::get_error_details(status);
        assert_eq!(details.error_info().unwrap().metadata.get("capability").map(String::as_str), Some("iam.tool.policies"), "{rpc}");
    }
}

/// The runbook identity end to end (operator runbook steps 1-4, then the fetch). The key scope is
/// the platform organization, so a read of other organizations also proves that the key scope does
/// not limit the fetch.
#[tokio::test]
async fn the_gateway_key_reads_the_compiled_set_of_any_org_and_the_floor() {
    let Some(env) = setup(true, true, &tenant_accepted()).await else { return };
    let admin_prn = Prn::parse(&support::provision(&env.state, &env.admin).await).unwrap();
    let beta = second_org(&env, "beta").await;
    assert_eq!(enable_log_only(&env, &env.org_prn).await, 1);
    set_compiler_result(env.dir.path(), &floor_accepted());
    let floor_put = PutFloorPolicySetRequest {
        source: FLOOR_DENY.into(),
        expected_version: 0,
    };
    let saved = env.client().put_floor_policy_set(authed(floor_put, &env.admin)).await.unwrap().into_inner();
    assert!(matches!(saved.result, Some(put_floor_policy_set_response::Result::Accepted(_))));

    let (platform_org, key) = support::gateway_reader_key(&env.state, &admin_prn).await;
    assert!(
        !env.org_prn.ends_with(&platform_org.to_string()) && !beta.ends_with(&platform_org.to_string()),
        "the key scope is a third organization"
    );
    let mut c = env.client();

    let acme = c
        .get_compiled_tool_policies(authed(get_compiled(&env.org_prn, None), &key))
        .await
        .expect("the key reads acme")
        .into_inner();
    let set = acme.set.expect("a set");
    assert_eq!((set.version, set.kind), (1, ToolPolicySetKind::Tenant as i32));
    assert_eq!(set.cedar_policies, STARTER_TOOL_POLICY_SOURCE);
    assert_eq!(acme.settings.expect("settings").mode, ToolPolicyMode::LogOnly as i32);
    assert_eq!(acme.floor_version, 1);

    let none = c.get_compiled_tool_policies(authed(get_compiled(&beta, None), &key)).await.expect("the key reads beta").into_inner();
    assert_eq!(none.set.expect("a set").version, 0);
    assert_eq!((none.settings.expect("settings").mode, none.floor_version), (ToolPolicyMode::Off as i32, 1));

    let floor = c.get_compiled_floor_policies(authed(get_compiled_floor(), &key)).await.expect("the key reads the floor");
    let floor = floor.into_inner().set.expect("a set");
    assert_eq!((floor.version, floor.kind, floor.cedar_policies.as_str()), (1, ToolPolicySetKind::Floor as i32, FLOOR_DENY));

    // The same key cannot call the tenant write RPCs or read the source set of another org.
    let denied = [
        c.get_tool_policy_set(authed(get_set(&env.org_prn, None), &key)).await.unwrap_err(),
        c.put_tool_policy_settings(authed(put_settings(&beta, ToolPolicyMode::LogOnly, ToolPolicyFailMode::Closed), &key))
            .await
            .unwrap_err(),
        c.put_tool_policy_set(authed(put_set(&beta, STARTER_TOOL_POLICY_SOURCE, 0), &key)).await.unwrap_err(),
        c.put_tool(authed(put_tool(&beta, "search", 0), &key)).await.unwrap_err(),
        c.list_tools(authed(
            ListToolsRequest {
                org_prn: env.org_prn.clone(),
                limit: 0,
                offset: 0,
            },
            &key,
        ))
        .await
        .unwrap_err(),
        c.get_tool_policy_settings(authed(GetToolPolicySettingsRequest { org_prn: env.org_prn.clone() }, &key))
            .await
            .unwrap_err(),
        c.get_floor_policy_set(authed(GetFloorPolicySetRequest { version: None }, &key)).await.unwrap_err(),
    ];
    for status in &denied {
        assert_status(status, Code::PermissionDenied, "forbidden");
    }
}

#[tokio::test]
async fn the_gateway_key_without_the_grant_reads_nothing() {
    let Some(env) = setup(true, true, &tenant_accepted()).await else { return };
    let admin_prn = Prn::parse(&support::provision(&env.state, &env.admin).await).unwrap();
    let (_, key, _) = support::gateway_key_without_grant(&env.state, &admin_prn).await;
    let mut c = env.client();
    let status = c.get_compiled_tool_policies(authed(get_compiled(&env.org_prn, None), &key)).await.unwrap_err();
    assert_status(&status, Code::PermissionDenied, "forbidden");
    let status = c.get_compiled_floor_policies(authed(get_compiled_floor(), &key)).await.unwrap_err();
    assert_status(&status, Code::PermissionDenied, "forbidden");
}

#[tokio::test]
async fn a_gateway_user_key_cannot_read_compiled_policies() {
    let Some(env) = setup(true, true, &tenant_accepted()).await else { return };
    let user = env.principal_with("tp-gateway-user", "gateway_user").await;
    let status = env.client().get_compiled_tool_policies(authed(get_compiled(&env.org_prn, None), &user)).await.unwrap_err();
    assert_status(&status, Code::PermissionDenied, "forbidden");
}

#[tokio::test]
async fn an_org_admin_cannot_read_compiled_policies() {
    let Some(env) = setup(true, true, &tenant_accepted()).await else { return };
    let admin = env.principal_with("tp-org-admin", "org_admin").await;
    let mut c = env.client();
    let status = c.get_compiled_tool_policies(authed(get_compiled(&env.org_prn, None), &admin)).await.unwrap_err();
    assert_status(&status, Code::PermissionDenied, "forbidden");
    let status = c.get_compiled_floor_policies(authed(get_compiled_floor(), &admin)).await.unwrap_err();
    assert_status(&status, Code::PermissionDenied, "forbidden");
}

#[tokio::test]
async fn an_org_with_no_set_gets_version_0_and_off_closed() {
    let Some(env) = setup(true, true, &tenant_accepted()).await else { return };
    let got = env
        .client()
        .get_compiled_tool_policies(authed(get_compiled(&env.org_prn, None), &env.admin))
        .await
        .unwrap()
        .into_inner();
    let set = got.set.expect("a set");
    assert_eq!((set.version, set.kind, set.org_id.as_str(), set.cedar_policies.as_str()), (0, ToolPolicySetKind::Tenant as i32, "", ""));
    assert!(set.conditions.is_empty() && set.catalog_names.is_empty());
    let settings = got.settings.expect("settings");
    assert_eq!(
        (settings.mode, settings.fail_mode, settings.updated_at, got.floor_version),
        (ToolPolicyMode::Off as i32, ToolPolicyFailMode::Closed as i32, None, 0)
    );
    let floor = env.client().get_compiled_floor_policies(authed(get_compiled_floor(), &env.admin)).await;
    let floor = floor.expect("a platform admin reads the compiled floor").into_inner().set.expect("a set");
    assert_eq!((floor.version, floor.kind), (0, ToolPolicySetKind::Floor as i32));
}

#[tokio::test]
async fn the_first_enable_over_grpc_returns_starter_version_1_and_the_fetch_sees_it() {
    let Some(env) = setup(true, true, &tenant_accepted()).await else { return };
    let mut c = env.client();
    let org = env.org_prn.clone();
    let put = c
        .put_tool_policy_settings(authed(put_settings(&org, ToolPolicyMode::LogOnly, ToolPolicyFailMode::Open), &env.admin))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(put.starter_version, 1);
    let settings = put.settings.expect("settings");
    assert_eq!((settings.mode, settings.fail_mode), (ToolPolicyMode::LogOnly as i32, ToolPolicyFailMode::Open as i32));
    assert!(settings.updated_at.is_some() && settings.updated_by.starts_with("prn:"), "{settings:?}");

    let got = c.get_compiled_tool_policies(authed(get_compiled(&org, None), &env.admin)).await.unwrap().into_inner();
    let set = got.set.expect("a set");
    assert_eq!(set.version, 1);
    assert!(!set.cedar_policies.is_empty());
    assert!(set.catalog_names.is_empty());
    assert_eq!(got.settings.expect("settings").mode, ToolPolicyMode::LogOnly as i32);
    let read = c
        .get_tool_policy_settings(authed(GetToolPolicySettingsRequest { org_prn: org.clone() }, &env.admin))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(read.settings.expect("settings").mode, ToolPolicyMode::LogOnly as i32);

    // A later write saves no second starter.
    let again = c
        .put_tool_policy_settings(authed(put_settings(&org, ToolPolicyMode::Enforce, ToolPolicyFailMode::Open), &env.admin))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(again.starter_version, 0);
}

#[tokio::test]
async fn off_to_enforce_with_no_set_is_failed_precondition_with_its_reason() {
    let Some(env) = setup(true, true, &tenant_accepted()).await else { return };
    let status = env
        .client()
        .put_tool_policy_settings(authed(put_settings(&env.org_prn, ToolPolicyMode::Enforce, ToolPolicyFailMode::Closed), &env.admin))
        .await
        .unwrap_err();
    assert_status(&status, Code::FailedPrecondition, "tool-policy-mode-transition-invalid");
}

/// F9: the fetch never returns a held or a discarded version. The adapters are the real ones.
#[tokio::test]
async fn a_held_version_is_not_found_over_grpc() {
    let Some(env) = setup(true, true, &tenant_accepted()).await else { return };
    let org = env.org_prn.clone();
    assert_eq!(enable_log_only(&env, &org).await, 1);
    let org_uuid = support::principal_uuid(&org);
    for (version, status) in [(2, "held"), (3, "discarded")] {
        let sql = format!(
            "INSERT INTO tool_policy_set_version (kind, org_id, version, source, compiled, compiler_generation, schema_revision, catalog_revision, status, origin, created_by, created_at) \
             VALUES ('tenant', '{org_uuid}', {version}, 's', '\\x00', 1, 1, 0, '{status}', 'save', 't', now())"
        );
        env.state.db.execute_raw(Statement::from_string(DbBackend::Postgres, sql)).await.expect("seed the row");
    }
    let mut c = env.client();
    let live = c.get_compiled_tool_policies(authed(get_compiled(&org, None), &env.admin)).await.unwrap().into_inner();
    assert_eq!(live.set.expect("a set").version, 1, "the live fetch skips the held row");
    for version in [2, 3] {
        let status = c.get_compiled_tool_policies(authed(get_compiled(&org, Some(version)), &env.admin)).await.unwrap_err();
        assert_status(&status, Code::NotFound, "tool-policy-version-not-found");
    }
    let by_number = c.get_compiled_tool_policies(authed(get_compiled(&org, Some(1)), &env.admin)).await.unwrap().into_inner();
    assert_eq!(by_number.set.expect("a set").version, 1);
}

#[tokio::test]
async fn an_unspecified_mode_is_missing_required_field() {
    let Some(env) = setup(true, true, &tenant_accepted()).await else { return };
    let mut c = env.client();
    let mut bad = put_settings(&env.org_prn, ToolPolicyMode::LogOnly, ToolPolicyFailMode::Closed);
    bad.mode = ToolPolicyMode::Unspecified as i32;
    let status = c.put_tool_policy_settings(authed(bad, &env.admin)).await.unwrap_err();
    assert_eq!(status.code(), Code::InvalidArgument, "{status:?}");
    assert_eq!(support::reason_of(&status), support::wire(ErrorReason::MissingRequiredField));
    let mut bad = put_settings(&env.org_prn, ToolPolicyMode::LogOnly, ToolPolicyFailMode::Closed);
    bad.fail_mode = 99;
    let status = c.put_tool_policy_settings(authed(bad, &env.admin)).await.unwrap_err();
    assert_eq!(support::reason_of(&status), support::wire(ErrorReason::MissingRequiredField));
}

#[tokio::test]
async fn the_first_enable_commits_two_outbox_rows() {
    use paigasus_iam::adapters::persistence::entities::event_outbox;
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};

    let Some(env) = setup(true, true, &tenant_accepted()).await else { return };
    let org = env.org_prn.clone();
    enable_log_only(&env, &org).await;
    let org_uuid = support::principal_uuid(&org).to_string();
    for event_type in ["iam.policy.tool_set.published", "iam.policy.tool_settings.changed"] {
        let rows = event_outbox::Entity::find().filter(event_outbox::Column::EventType.eq(event_type)).all(&env.state.db).await.unwrap();
        assert_eq!(rows.len(), 1, "{event_type}");
        let payload: serde_json::Value = serde_json::from_str(&rows[0].payload).expect("payload is json");
        assert_eq!(payload["org_id"], org_uuid, "{event_type}");
    }
}
