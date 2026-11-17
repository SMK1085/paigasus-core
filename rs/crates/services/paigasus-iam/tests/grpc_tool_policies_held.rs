// SPDX-License-Identifier: Apache-2.0

//! The held tool policy version RPCs and the re-compile run over gRPC (SMA-781 Task 8). Docker
//! gated like `grpc_tool_policies.rs`. The compiler is the fake script of
//! `support::fake_compiler_script`, which answers `revision` with generation 1. A set becomes
//! stale when a test changes its `compiler_generation` to 0 with raw SQL. No API path makes a
//! live or a held row stale, so the tests do it in the database.

mod support;

use std::net::SocketAddr;
use std::time::Duration;

use paigasus_iam::adapters::grpc;
use paigasus_iam::adapters::http::AppState;
use paigasus_iam::adapters::persistence::entities::{audit_log, event_outbox};
use paigasus_iam_core::PrincipalId;
use paigasus_iam_core::tool_policy::{STARTER_TOOL_POLICY_SOURCE, base_schema_json, tenant_schema_json};
use paigasus_kernel::Prn;
use paigasus_proto::paigasus::common::v1::ErrorReason;
use paigasus_proto::paigasus::iam::v1::tool_policy_service_client::ToolPolicyServiceClient;
use paigasus_proto::paigasus::iam::v1::{
    AcceptHeldToolPolicyVersionRequest, DiscardHeldToolPolicyVersionRequest, GetCompiledToolPoliciesRequest, GetToolPolicySetRequest, HeldToolPolicyVersion, ListHeldToolPolicyVersionsRequest,
    PutFloorPolicySetRequest, PutToolPolicySettingsRequest, PutToolRequest, RecompileOutcome, RecompileRunStatus, RunToolPolicyRecompileRequest, RunToolPolicyRecompileResponse, ToolKind,
    ToolPolicyFailMode, ToolPolicyMode, ToolPolicySetKind, ToolPolicyVersionOrigin, ToolPolicyVersionStatus,
};
use sea_orm::{ColumnTrait, ConnectionTrait, DbBackend, EntityTrait, PaginatorTrait, QueryFilter, Statement};
use support::{MockIdp, accepted_result, fake_compiler_script, rejected_result, set_compiler_result};
use tempfile::TempDir;
use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::testcontainers::ContainerAsync;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tonic::transport::Channel;
use tonic::{Code, Request, Status};

const FLOOR_SOURCE: &str = "@id(\"floor-deny\")\nforbid(principal, action in Pgs::Gateway::Action::\"Unregistered\", resource);\n";
const OTHER_SOURCE: &str = "@id(\"other\")\nforbid(principal, action, resource);\n";

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

    async fn principal_with(&self, sub: &str, role: &str) -> String {
        let token = self.idp.bearer(sub, Some(&format!("{sub}@example.com")), "paigasus", 3600);
        let prn = support::provision(&self.state, &token).await;
        support::seed_org_role(&self.state, &prn, &self.org_prn, role).await;
        token
    }

    async fn sql(&self, sql: String) {
        self.state.db.execute_raw(Statement::from_string(DbBackend::Postgres, sql)).await.expect("raw sql");
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

fn tenant_other() -> String {
    accepted_result(OTHER_SOURCE, &tenant_schema_json(&[]))
}

async fn setup(enabled: bool) -> Option<Env> {
    let (node, db) = support::start_migrated_postgres().await?;
    let dir = TempDir::new().expect("tempdir");
    let path = fake_compiler_script(dir.path(), &tenant_accepted());
    let (state, idp) = support::state_with_tool_policies(db, enabled, &path).await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);
    let router = grpc::router(state.clone(), Duration::from_secs(30)).await;
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

fn assert_status(status: &Status, code: Code, reason: &str) {
    assert_eq!((status.code(), support::reason_of(status).as_str()), (code, reason), "{status:?}");
}

fn list_req(org: &str) -> ListHeldToolPolicyVersionsRequest {
    ListHeldToolPolicyVersionsRequest {
        org_prn: org.to_string(),
        limit: 0,
        offset: 0,
    }
}

fn accept_req(kind: ToolPolicySetKind, org: &str, version: u64) -> AcceptHeldToolPolicyVersionRequest {
    AcceptHeldToolPolicyVersionRequest {
        kind: kind as i32,
        org_prn: org.to_string(),
        held_version: version,
    }
}

fn discard_req(kind: ToolPolicySetKind, org: &str, version: u64) -> DiscardHeldToolPolicyVersionRequest {
    DiscardHeldToolPolicyVersionRequest {
        kind: kind as i32,
        org_prn: org.to_string(),
        held_version: version,
    }
}

async fn run(env: &Env) -> RunToolPolicyRecompileResponse {
    env.client()
        .run_tool_policy_recompile(authed(RunToolPolicyRecompileRequest {}, &env.admin))
        .await
        .expect("run")
        .into_inner()
}

/// The first enable of `org` to `log_only`: a live version 1 at generation 1.
async fn enable_log_only(env: &Env, org: &str) {
    set_compiler_result(env.dir.path(), &tenant_accepted());
    env.client()
        .put_tool_policy_settings(authed(
            PutToolPolicySettingsRequest {
                org_prn: org.to_string(),
                mode: ToolPolicyMode::LogOnly as i32,
                fail_mode: ToolPolicyFailMode::Closed as i32,
            },
            &env.admin,
        ))
        .await
        .expect("the first enable");
}

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

/// Makes the live rows that match `filter` stale (`kind = 'floor'` or a tenant filter).
async fn age_live(env: &Env, filter: &str) {
    env.sql(format!("UPDATE tool_policy_set_version SET compiler_generation = 0 WHERE status = 'live' AND {filter}")).await;
}

fn tenant_filter(org_prn: &str) -> String {
    format!("kind = 'tenant' AND org_id = '{}'", support::principal_uuid(org_prn))
}

/// A live tenant version 1 at generation 0 for `org`, then a run that holds the other output
/// as version 2. Returns the held version.
async fn make_held_tenant(env: &Env, org: &str) -> u64 {
    enable_log_only(env, org).await;
    age_live(env, &tenant_filter(org)).await;
    set_compiler_result(env.dir.path(), &tenant_other());
    let org_uuid = support::principal_uuid(org).to_string();
    let response = run(env).await;
    assert_eq!(response.status, RecompileRunStatus::Ran as i32);
    let result = response.results.iter().find(|r| r.org_id == org_uuid).expect("a result for the org");
    assert_eq!(result.outcome, RecompileOutcome::HeldDifferent as i32, "{result:?}");
    assert_eq!((result.base_version, result.version), (1, 2));
    result.version
}

async fn list(env: &Env, token: &str, org: &str) -> Vec<HeldToolPolicyVersion> {
    env.client().list_held_tool_policy_versions(authed(list_req(org), token)).await.expect("list").into_inner().versions
}

async fn live_version(env: &Env, org: &str) -> u64 {
    env.client()
        .get_compiled_tool_policies(authed(
            GetCompiledToolPoliciesRequest {
                org_prn: org.to_string(),
                version: None,
            },
            &env.admin,
        ))
        .await
        .expect("fetch")
        .into_inner()
        .set
        .expect("a set")
        .version
}

async fn audits(env: &Env, action: &str) -> u64 {
    audit_log::Entity::find()
        .filter(audit_log::Column::Action.eq(action))
        .count(&env.state.db)
        .await
        .expect("count audit_log")
}

async fn published_events(env: &Env) -> u64 {
    event_outbox::Entity::find()
        .filter(event_outbox::Column::EventType.eq("iam.policy.tool_set.published"))
        .count(&env.state.db)
        .await
        .expect("count event_outbox")
}

#[tokio::test]
async fn the_four_rpcs_answer_capability_disabled_when_the_toggle_is_off() {
    let Some(env) = setup(false).await else { return };
    let mut c = env.client();
    let t = &env.admin;
    let org = &env.org_prn;
    let answers = vec![
        c.list_held_tool_policy_versions(authed(list_req(org), t)).await.unwrap_err(),
        c.accept_held_tool_policy_version(authed(accept_req(ToolPolicySetKind::Tenant, org, 2), t)).await.unwrap_err(),
        c.discard_held_tool_policy_version(authed(discard_req(ToolPolicySetKind::Tenant, org, 2), t)).await.unwrap_err(),
        c.run_tool_policy_recompile(authed(RunToolPolicyRecompileRequest {}, t)).await.unwrap_err(),
    ];
    for status in &answers {
        assert_status(status, Code::Unimplemented, &support::wire(ErrorReason::CapabilityDisabled));
        let details = tonic_types::StatusExt::get_error_details(status);
        assert_eq!(details.error_info().unwrap().metadata.get("capability").map(String::as_str), Some("iam.tool.policies"));
    }
}

#[tokio::test]
async fn a_platform_admin_run_holds_a_different_result_and_lists_it() {
    let Some(env) = setup(true).await else { return };
    let org = env.org_prn.clone();
    let version = make_held_tenant(&env, &org).await;
    assert_eq!(version, 2);

    for filter in ["", org.as_str()] {
        let held = list(&env, &env.admin, filter).await;
        assert_eq!(held.len(), 1, "{filter}");
        let row = &held[0];
        assert_eq!((row.kind, row.version, row.base_version), (ToolPolicySetKind::Tenant as i32, 2, 1));
        assert_eq!(row.status, ToolPolicyVersionStatus::Held as i32);
        assert_eq!(row.org_id, support::principal_uuid(&org).to_string());
        let policies = row.report.as_ref().and_then(|r| r.policies.as_ref()).expect("a policy diff");
        assert!(!policies.added.is_empty() || !policies.changed.is_empty(), "{policies:?}");
        assert!(row.compiled.as_ref().expect("compiled").cedar_policies.contains("other"));
    }
    // The live set is unchanged, and a second run proposes nothing new.
    assert_eq!(live_version(&env, &org).await, 1);
    let again = run(&env).await;
    assert_eq!(again.results.len(), 1);
    assert_eq!(again.results[0].outcome, RecompileOutcome::AlreadyProposed as i32);
}

#[tokio::test]
async fn an_identical_result_is_applied_and_the_fetch_sees_origin_recompile() {
    let Some(env) = setup(true).await else { return };
    let org = env.org_prn.clone();
    enable_log_only(&env, &org).await;
    age_live(&env, &tenant_filter(&org)).await;
    let response = run(&env).await;
    assert_eq!(response.results.len(), 1);
    assert_eq!((response.results[0].outcome, response.results[0].version), (RecompileOutcome::Identical as i32, 2));
    let got = env
        .client()
        .get_tool_policy_set(authed(GetToolPolicySetRequest { org_prn: org.clone(), version: None }, &env.admin))
        .await
        .unwrap()
        .into_inner()
        .set
        .expect("a set");
    assert_eq!((got.version, got.origin, got.compiler_generation), (2, ToolPolicyVersionOrigin::Recompile as i32, 1));
    assert_eq!(live_version(&env, &org).await, 2);
    assert!(list(&env, &env.admin, "").await.is_empty());
}

#[tokio::test]
async fn an_org_admin_cannot_run_the_job_or_list_every_set() {
    let Some(env) = setup(true).await else { return };
    let org = env.org_prn.clone();
    make_held_tenant(&env, &org).await;
    let token = env.principal_with("tp-org-admin", "org_admin").await;
    let mut c = env.client();
    let status = c.run_tool_policy_recompile(authed(RunToolPolicyRecompileRequest {}, &token)).await.unwrap_err();
    assert_status(&status, Code::PermissionDenied, "forbidden");
    let status = c.list_held_tool_policy_versions(authed(list_req(""), &token)).await.unwrap_err();
    assert_status(&status, Code::PermissionDenied, "forbidden");
    // The org admin lists the held rows of its own organization.
    assert_eq!(list(&env, &token, &org).await.len(), 1);
}

#[tokio::test]
async fn the_list_limit_defaults_to_50_and_is_at_most_200() {
    let Some(env) = setup(true).await else { return };
    let org = env.org_prn.clone();
    make_held_tenant(&env, &org).await;
    let mut c = env.client();
    for limit in [0, 1, 200] {
        let req = ListHeldToolPolicyVersionsRequest {
            org_prn: org.clone(),
            limit,
            offset: 0,
        };
        assert_eq!(c.list_held_tool_policy_versions(authed(req, &env.admin)).await.unwrap().into_inner().versions.len(), 1, "limit {limit}");
    }
    let req = ListHeldToolPolicyVersionsRequest {
        org_prn: org.clone(),
        limit: 201,
        offset: 0,
    };
    let status = c.list_held_tool_policy_versions(authed(req, &env.admin)).await.unwrap_err();
    assert_status(&status, Code::InvalidArgument, "invalid-pagination");
    let req = ListHeldToolPolicyVersionsRequest {
        org_prn: org.clone(),
        limit: 10,
        offset: 1,
    };
    assert!(c.list_held_tool_policy_versions(authed(req, &env.admin)).await.unwrap().into_inner().versions.is_empty());
}

#[tokio::test]
async fn an_org_admin_accepts_its_held_row_and_the_fetch_returns_it() {
    let Some(env) = setup(true).await else { return };
    let org = env.org_prn.clone();
    let held = make_held_tenant(&env, &org).await;
    let token = env.principal_with("tp-org-admin", "org_admin").await;
    let accepted = env
        .client()
        .accept_held_tool_policy_version(authed(accept_req(ToolPolicySetKind::Tenant, &org, held), &token))
        .await
        .expect("accept")
        .into_inner()
        .accepted
        .expect("the new live version");
    assert_eq!(
        (accepted.version, accepted.origin, accepted.status),
        (3, ToolPolicyVersionOrigin::Accept as i32, ToolPolicyVersionStatus::Live as i32)
    );
    let compiled = accepted.compiled.expect("compiled");
    assert_eq!(compiled.version, 3);
    assert!(compiled.cedar_policies.contains("other"));
    let fetched = env
        .client()
        .get_compiled_tool_policies(authed(
            GetCompiledToolPoliciesRequest {
                org_prn: org.clone(),
                version: Some(3),
            },
            &env.admin,
        ))
        .await
        .expect("fetch version 3")
        .into_inner()
        .set
        .expect("a set");
    assert_eq!(fetched.version, 3);
    assert_eq!(live_version(&env, &org).await, 3);
    // The accepted row is no longer held, and the set is current: a run finds nothing to do.
    assert!(list(&env, &env.admin, "").await.is_empty());
    assert!(run(&env).await.results.is_empty());
}

#[tokio::test]
async fn a_stale_accept_is_aborted_with_version_conflict() {
    let Some(env) = setup(true).await else { return };
    let org = env.org_prn.clone();
    let held = make_held_tenant(&env, &org).await;
    // No API path makes a held row stale (F21): the row now names an older live version.
    env.sql(format!("UPDATE tool_policy_set_version SET base_version = 0 WHERE status = 'held' AND {}", tenant_filter(&org)))
        .await;
    let status = env
        .client()
        .accept_held_tool_policy_version(authed(accept_req(ToolPolicySetKind::Tenant, &org, held), &env.admin))
        .await
        .unwrap_err();
    assert_status(&status, Code::Aborted, "tool-policy-version-conflict");
    assert_eq!(live_version(&env, &org).await, 1);
}

#[tokio::test]
async fn a_failed_held_accept_is_failed_precondition_held_not_acceptable() {
    let Some(env) = setup(true).await else { return };
    let org = env.org_prn.clone();
    enable_log_only(&env, &org).await;
    age_live(&env, &tenant_filter(&org)).await;
    set_compiler_result(env.dir.path(), &rejected_result(&[("tool-policy-invalid", "no good")]));
    let response = run(&env).await;
    assert_eq!((response.results[0].outcome, response.results[0].version), (RecompileOutcome::HeldFailed as i32, 2));
    let held = list(&env, &env.admin, &org).await;
    assert!(held[0].compiled.is_none(), "a failed compile has no compiled message");
    let rejections = &held[0].report.as_ref().expect("a report").rejections.as_ref().expect("rejections").rejections;
    assert_eq!(rejections.len(), 1);
    let status = env
        .client()
        .accept_held_tool_policy_version(authed(accept_req(ToolPolicySetKind::Tenant, &org, 2), &env.admin))
        .await
        .unwrap_err();
    assert_status(&status, Code::FailedPrecondition, "tool-policy-held-not-acceptable");
}

#[tokio::test]
async fn discard_then_catalog_write_is_recompile_pending() {
    let Some(env) = setup(true).await else { return };
    let org = env.org_prn.clone();
    let held = make_held_tenant(&env, &org).await;
    let mut c = env.client();
    let discarded = c
        .discard_held_tool_policy_version(authed(discard_req(ToolPolicySetKind::Tenant, &org, held), &env.admin))
        .await
        .expect("discard")
        .into_inner()
        .discarded
        .expect("the row");
    assert_eq!((discarded.version, discarded.status), (2, ToolPolicyVersionStatus::Discarded as i32));
    assert!(list(&env, &env.admin, &org).await.is_empty());
    // The live set is still stale, so the catalog is closed until a run or an accept.
    let put = PutToolRequest {
        org_prn: org.clone(),
        name: "search".into(),
        kind: ToolKind::Function as i32,
        input_schema_json: String::new(),
        expected_catalog_revision: 0,
    };
    let status = c.put_tool(authed(put, &env.admin)).await.unwrap_err();
    assert_status(&status, Code::FailedPrecondition, "tool-policy-recompile-pending");
    // The discarded row stays on record: the next run does not propose it again.
    assert_eq!(run(&env).await.results[0].outcome, RecompileOutcome::AlreadyProposed as i32);
    // A second discard of the same row finds no held row.
    let status = c
        .discard_held_tool_policy_version(authed(discard_req(ToolPolicySetKind::Tenant, &org, held), &env.admin))
        .await
        .unwrap_err();
    assert_status(&status, Code::NotFound, "tool-policy-version-not-found");
}

#[tokio::test]
async fn a_floor_held_row_needs_the_floor_action() {
    let Some(env) = setup(true).await else { return };
    let mut c = env.client();
    set_compiler_result(env.dir.path(), &accepted_result(FLOOR_SOURCE, &base_schema_json()));
    c.put_floor_policy_set(authed(
        PutFloorPolicySetRequest {
            source: FLOOR_SOURCE.into(),
            expected_version: 0,
        },
        &env.admin,
    ))
    .await
    .expect("save the floor");
    age_live(&env, "kind = 'floor'").await;
    set_compiler_result(env.dir.path(), &accepted_result(OTHER_SOURCE, &base_schema_json()));
    let response = run(&env).await;
    assert_eq!(response.results.len(), 1);
    let result = &response.results[0];
    assert_eq!(
        (result.kind, result.outcome, result.version),
        (ToolPolicySetKind::Floor as i32, RecompileOutcome::HeldDifferent as i32, 2)
    );

    let held = list(&env, &env.admin, "").await;
    assert_eq!((held.len(), held[0].kind, held[0].org_id.as_str()), (1, ToolPolicySetKind::Floor as i32, ""));

    let token = env.principal_with("tp-org-admin", "org_admin").await;
    let status = c.accept_held_tool_policy_version(authed(accept_req(ToolPolicySetKind::Floor, "", 2), &token)).await.unwrap_err();
    assert_status(&status, Code::PermissionDenied, "forbidden");
    let status = c.discard_held_tool_policy_version(authed(discard_req(ToolPolicySetKind::Floor, "", 2), &token)).await.unwrap_err();
    assert_status(&status, Code::PermissionDenied, "forbidden");
    // A floor row has no organization (F10).
    let status = c
        .accept_held_tool_policy_version(authed(accept_req(ToolPolicySetKind::Floor, &env.org_prn, 2), &env.admin))
        .await
        .unwrap_err();
    assert_status(&status, Code::InvalidArgument, "mutually-exclusive-fields");
    let accepted = c
        .accept_held_tool_policy_version(authed(accept_req(ToolPolicySetKind::Floor, "", 2), &env.admin))
        .await
        .expect("a platform admin accepts the floor row")
        .into_inner()
        .accepted
        .expect("the new live version");
    assert_eq!(
        (accepted.kind, accepted.version, accepted.origin),
        (ToolPolicySetKind::Floor as i32, 3, ToolPolicyVersionOrigin::Accept as i32)
    );
}

#[tokio::test]
async fn an_unspecified_kind_is_missing_required_field() {
    let Some(env) = setup(true).await else { return };
    let mut c = env.client();
    let missing = support::wire(ErrorReason::MissingRequiredField);
    let status = c
        .accept_held_tool_policy_version(authed(accept_req(ToolPolicySetKind::Unspecified, &env.org_prn, 2), &env.admin))
        .await
        .unwrap_err();
    assert_status(&status, Code::InvalidArgument, &missing);
    let mut bad = discard_req(ToolPolicySetKind::Tenant, &env.org_prn, 2);
    bad.kind = 99;
    let status = c.discard_held_tool_policy_version(authed(bad, &env.admin)).await.unwrap_err();
    assert_status(&status, Code::InvalidArgument, &missing);
    // A held version of 0 and a tenant with no organization are missing fields too.
    let status = c
        .accept_held_tool_policy_version(authed(accept_req(ToolPolicySetKind::Tenant, &env.org_prn, 0), &env.admin))
        .await
        .unwrap_err();
    assert_status(&status, Code::InvalidArgument, &missing);
    let status = c.discard_held_tool_policy_version(authed(discard_req(ToolPolicySetKind::Tenant, "", 2), &env.admin)).await.unwrap_err();
    assert_status(&status, Code::InvalidArgument, &missing);
}

#[tokio::test]
async fn each_mutation_writes_its_audit_row() {
    let Some(env) = setup(true).await else { return };
    let org_a = env.org_prn.clone();
    let held_a = make_held_tenant(&env, &org_a).await;
    assert_eq!(audits(&env, "RunToolPolicyRecompile").await, 1);
    let org_b = second_org(&env, "beta").await;
    let held_b = make_held_tenant(&env, &org_b).await;
    assert_eq!(audits(&env, "RunToolPolicyRecompile").await, 2);

    let mut c = env.client();
    c.accept_held_tool_policy_version(authed(accept_req(ToolPolicySetKind::Tenant, &org_a, held_a), &env.admin))
        .await
        .expect("accept");
    assert_eq!((audits(&env, "AcceptHeldToolPolicyVersion").await, audits(&env, "DiscardHeldToolPolicyVersion").await), (1, 0));
    c.discard_held_tool_policy_version(authed(discard_req(ToolPolicySetKind::Tenant, &org_b, held_b), &env.admin))
        .await
        .expect("discard");
    assert_eq!((audits(&env, "AcceptHeldToolPolicyVersion").await, audits(&env, "DiscardHeldToolPolicyVersion").await), (1, 1));
}

#[tokio::test]
async fn accept_commits_one_outbox_row_and_discard_none() {
    let Some(env) = setup(true).await else { return };
    let org_a = env.org_prn.clone();
    let held_a = make_held_tenant(&env, &org_a).await;
    let org_b = second_org(&env, "beta").await;
    let held_b = make_held_tenant(&env, &org_b).await;
    let before = published_events(&env).await;
    let mut c = env.client();
    c.discard_held_tool_policy_version(authed(discard_req(ToolPolicySetKind::Tenant, &org_b, held_b), &env.admin))
        .await
        .expect("discard");
    assert_eq!(published_events(&env).await, before, "a discard writes no event");
    c.accept_held_tool_policy_version(authed(accept_req(ToolPolicySetKind::Tenant, &org_a, held_a), &env.admin))
        .await
        .expect("accept");
    assert_eq!(published_events(&env).await, before + 1, "an accept writes one event");
}

/// Two calls on one held row at the same time: exactly one succeeds, the other finds no held row
/// or a changed set, and a discarded row never goes live (spec section 8, H3).
#[tokio::test]
async fn a_concurrent_accept_and_discard_has_one_winner() {
    let Some(env) = setup(true).await else { return };
    let org = env.org_prn.clone();
    let held = make_held_tenant(&env, &org).await;
    let (mut c1, mut c2) = (env.client(), env.client());
    let (accept, discard) = tokio::join!(
        c1.accept_held_tool_policy_version(authed(accept_req(ToolPolicySetKind::Tenant, &org, held), &env.admin)),
        c2.discard_held_tool_policy_version(authed(discard_req(ToolPolicySetKind::Tenant, &org, held), &env.admin)),
    );
    let live = live_version(&env, &org).await;
    match (accept, discard) {
        (Ok(_), Err(loser)) => {
            assert!(matches!(loser.code(), Code::NotFound | Code::Aborted), "{loser:?}");
            assert_eq!(live, 3, "the accepted row is live");
        }
        (Err(loser), Ok(_)) => {
            assert!(matches!(loser.code(), Code::NotFound | Code::Aborted), "{loser:?}");
            assert_eq!(live, 1, "a discarded row never goes live");
        }
        (a, d) => panic!("exactly one call must win: accept {:?}, discard {:?}", a.map(|_| ()), d.map(|_| ())),
    }
}

/// A tenant list returns only the rows of that tenant, and an admin of A cannot list B.
#[tokio::test]
async fn a_tenant_list_holds_only_its_own_rows_and_another_tenant_is_forbidden() {
    let Some(env) = setup(true).await else { return };
    let org_a = env.org_prn.clone();
    make_held_tenant(&env, &org_a).await;
    let org_b = second_org(&env, "beta").await;
    make_held_tenant(&env, &org_b).await;
    let (uuid_a, uuid_b) = (support::principal_uuid(&org_a).to_string(), support::principal_uuid(&org_b).to_string());

    let all = list(&env, &env.admin, "").await;
    assert_eq!(all.len(), 2, "the all-sets list holds both rows");
    let only_a = list(&env, &env.admin, &org_a).await;
    assert_eq!(only_a.iter().map(|h| h.org_id.as_str()).collect::<Vec<_>>(), vec![uuid_a.as_str()]);
    let only_b = list(&env, &env.admin, &org_b).await;
    assert_eq!(only_b.iter().map(|h| h.org_id.as_str()).collect::<Vec<_>>(), vec![uuid_b.as_str()]);

    let token_a = env.principal_with("tp-org-admin", "org_admin").await;
    let status = env.client().list_held_tool_policy_versions(authed(list_req(&org_b), &token_a)).await.unwrap_err();
    assert_status(&status, Code::PermissionDenied, "forbidden");
}

#[tokio::test]
async fn two_concurrent_accepts_make_one_live_version() {
    let Some(env) = setup(true).await else { return };
    let org = env.org_prn.clone();
    let held = make_held_tenant(&env, &org).await;
    let (mut c1, mut c2) = (env.client(), env.client());
    let (a, b) = tokio::join!(
        c1.accept_held_tool_policy_version(authed(accept_req(ToolPolicySetKind::Tenant, &org, held), &env.admin)),
        c2.accept_held_tool_policy_version(authed(accept_req(ToolPolicySetKind::Tenant, &org, held), &env.admin)),
    );
    let (ok, err): (Vec<_>, Vec<_>) = [a, b].into_iter().partition(Result::is_ok);
    assert_eq!((ok.len(), err.len()), (1, 1));
    let loser = err.into_iter().next().unwrap().unwrap_err();
    assert!(matches!(loser.code(), Code::NotFound | Code::Aborted), "{loser:?}");
    assert_eq!(live_version(&env, &org).await, 3);
}
