// SPDX-License-Identifier: Apache-2.0

//! End-to-end gRPC coverage for `AuthorizationService.RetireSystemPolicy` (SMA-481/SMA-501):
//! all three outcomes of the response `oneof`
//! (`paigasus_proto::paigasus::iam::v1::retire_system_policy_response::Outcome`) return gRPC
//! `OK` (design D3) — the two refusals (`Blocked`, `NeedsAcknowledgement`) are NOT error
//! statuses, so every refusal test here asserts the response is `OK` carrying the right
//! variant, never merely "not `Retired`" or an error code.
//!
//! This suite is separate from `tests/grpc_authz.rs` for one reason, not an authorization
//! difference: `SystemRetirementService::retire` refuses on a `min_starter_revision`
//! fleet-convergence guard (D11) until the starter policy set has converged. Every scenario
//! here drives that convergence through the REAL boot path — `AppState::new` itself runs
//! `bootstrap::reconcile_starter` before returning (see its own module doc), stamping every
//! `STARTER_POLICY_IDS` row at this binary's `STARTER_POLICY_REVISION` — exactly what
//! `tests/authz_system_retirement_pg.rs::converge_starter_set` wires by hand for its own,
//! `AppState`-free harness. Using `AppState::new(db, &cfg)` (as every gRPC suite already does)
//! gets that convergence for free; no separate seeding step is needed.
//!
//! Mirrors `tests/authz_system_retirement_pg.rs`'s seeders (`seed_orphan_chain`, `seed_grants`,
//! `seed_system_policy_with_revision` — re-declared here, private to this file, per this
//! crate's established posture of duplicating a private seeder across `tests/*.rs` binaries)
//! and `tests/grpc_dead_letters.rs`'s harness: the real `grpc::router(AppState::new(db, &cfg),
//! ..)` over an ephemeral `TcpListener`, against an ephemeral Postgres (Docker) + the HTTPS
//! mock IdP.

mod support;

use std::net::SocketAddr;
use std::time::Duration;

use chrono::Utc;
use paigasus_iam::adapters::grpc;
use paigasus_iam::adapters::http::AppState;
use paigasus_iam::adapters::persistence::entities::{policy, principal, role, role_grant};
use paigasus_iam_core::GrantScope;
use paigasus_iam_core::authz::roles::STARTER_POLICY_REVISION;
use paigasus_kernel::Prn;
use paigasus_proto::paigasus::iam::v1::RetireSystemPolicyRequest;
use paigasus_proto::paigasus::iam::v1::authorization_service_client::AuthorizationServiceClient;
use paigasus_proto::paigasus::iam::v1::retire_system_policy_response::Outcome;
use sea_orm::{ActiveModelTrait, ActiveValue::NotSet, DatabaseConnection, EntityTrait, Set};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tonic::Code;
use tonic::transport::Channel;
use uuid::Uuid;

/// The revision a REAL orphan carries: strictly below this binary's own — see
/// `tests/authz_system_retirement_pg.rs`'s identical constant for the full rationale. Its
/// numeric value doesn't gate anything here (D11 measures only the CODE-DEFINED starter set,
/// never the orphan's own row), but stamping it keeps the fixture representative of what an
/// orphan can actually hold.
const ORPHAN_REVISION: u32 = STARTER_POLICY_REVISION.saturating_sub(1);

/// Seeds a system-owned template + role at a NON-code-defined id: the `policy` row first, then
/// the `role` row that references it — `fk_role_template` requires that order. Copied from
/// `tests/authz_system_retirement_pg.rs::seed_orphan_chain` (private to that file).
async fn seed_orphan_chain(db: &DatabaseConnection, id: &str) {
    let now = Utc::now();
    policy::ActiveModel {
        policy_id: Set(id.to_string()),
        kind: Set("template".to_string()),
        source: Set("permit(principal == ?principal, action, resource in ?resource);".to_string()),
        description: Set(None),
        system: Set(true),
        created_at: Set(now),
        updated_at: Set(now),
        content_fingerprint: NotSet,
        starter_revision: Set(Some(i32::try_from(ORPHAN_REVISION).expect("test revision fits i32"))),
    }
    .insert(db)
    .await
    .unwrap();
    role::ActiveModel {
        key: Set(id.to_string()),
        template_id: Set(id.to_string()),
        scope_kinds: Set(r#"["organization"]"#.to_string()),
        description: Set(None),
        system: Set(true),
        created_at: Set(now),
    }
    .insert(db)
    .await
    .unwrap();
}

/// Seeds one grant of `role_key` at the synthetic Root scope per entry in `ids`, inserted in
/// the GIVEN order — the caller controls insertion order deliberately, so an "ordered by id"
/// assertion can't pass by coincidence of insertion order. Copied from
/// `tests/authz_system_retirement_pg.rs::seed_grants` (private to that file).
async fn seed_grants(db: &DatabaseConnection, role_key: &str, ids: &[Uuid]) {
    let now = Utc::now();
    for &id in ids {
        principal::ActiveModel {
            id: Set(id),
            prn: Set(format!("prn:pgs:iam:::principal/{id}")),
            kind: Set("user".to_string()),
            status: Set("active".to_string()),
            created_at: Set(now),
            updated_at: Set(now),
        }
        .insert(db)
        .await
        .unwrap();

        role_grant::ActiveModel {
            id: Set(id),
            principal_id: Set(id),
            role_key: Set(role_key.to_string()),
            scope_kind: Set("root".to_string()),
            scope_node_prn: Set(GrantScope::Root.canonical_prn()),
            scope_org_id: Set(None),
            scope_team_id: Set(None),
            scope_project_id: Set(None),
            linked_policy_id: Set(format!("grant:{id}")),
            created_at: Set(now),
        }
        .insert(db)
        .await
        .unwrap();
    }
}

/// A STATIC forbid-shaped policy scoped to an action this suite's actor never calls
/// (`ListOrganizations`), deliberately NOT
/// `tests/authz_system_retirement_pg.rs::seed_system_policy_with_revision`'s unconditional
/// `forbid(principal, action, resource);`. That file drives `SystemRetirementService` through
/// an `AllowAllAuthorizer` fake, so an unconditional forbid never reaches a live decision; this
/// suite drives the REAL Cedar-backed router (`AppState::new`), whose background policy
/// refresh (`refresh_interval_secs: 1`, `AuthzConfig::default()`) compiles ANY inserted
/// `system = true` static row into the live `PolicySet` within about a second — an
/// unconditional forbid then denies this suite's own subsequent `RetireSystemPolicy` call (and
/// every other decision), which is exactly what a first draft of this suite hit. Scoping the
/// forbid to an action this actor never invokes keeps the row a faithful "static policy that
/// would change decisions if left in place" fixture without self-sabotaging the harness that
/// drives it.
const STATIC_ORPHAN_SOURCE: &str = r#"forbid(principal, action == Pgs::Iam::Action::"ListOrganizations", resource);"#;

/// Seeds a bare system-owned `policy` row at `id` with `starter_revision` forced to `revision`
/// — deliberately without the role-row half of `seed_orphan_chain`. Mirrors
/// `tests/authz_system_retirement_pg.rs::seed_system_policy_with_revision` (private to that
/// file) except for `source`; see [`STATIC_ORPHAN_SOURCE`]'s doc for why.
async fn seed_system_policy_with_revision(db: &DatabaseConnection, id: &str, revision: Option<u32>) {
    let now = Utc::now();
    policy::ActiveModel {
        policy_id: Set(id.to_string()),
        kind: Set("static".to_string()),
        source: Set(STATIC_ORPHAN_SOURCE.to_string()),
        description: Set(None),
        system: Set(true),
        created_at: Set(now),
        updated_at: Set(now),
        content_fingerprint: NotSet,
        starter_revision: Set(revision.map(|r| i32::try_from(r).expect("test revision fits i32"))),
    }
    .insert(db)
    .await
    .unwrap();
}

/// The stored `policy` row at `id`, if any.
async fn policy_row(db: &DatabaseConnection, id: &str) -> Option<policy::Model> {
    policy::Entity::find_by_id(id.to_string()).one(db).await.unwrap()
}

/// The stored `role` row at `key`, if any.
async fn role_row(db: &DatabaseConnection, key: &str) -> Option<role::Model> {
    role::Entity::find_by_id(key.to_string()).one(db).await.unwrap()
}

/// The canonical principal PRN `to_grant_ref` (the production adapter) rebuilds for a grant
/// seeded by [`seed_grants`] — matches
/// `pg_system_row_retirer.rs::to_grant_ref`'s own `Prn::build` call, rather than re-deriving
/// the `prn:pgs:iam:::principal/<id>` string by hand.
fn grant_principal_prn(id: Uuid) -> String {
    Prn::build("iam", "", None, "principal", id).unwrap().canonical()
}

/// Spawns the full `grpc::router` (health, tenancy, authn, authz, service-account,
/// service-info, users, outbox, authorization — all wrapped by the bearer layer) on an
/// ephemeral port; `abort()` the returned handle when the test finishes.
async fn spawn_server(state: AppState) -> (SocketAddr, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);
    let router = grpc::router(state, Duration::from_secs(5)).await;
    let server = tokio::spawn(async move {
        router.serve_with_incoming(incoming).await.unwrap();
    });
    (addr, server)
}

async fn channel(addr: SocketAddr) -> Channel {
    tonic::transport::Endpoint::new(format!("http://{addr}")).unwrap().connect().await.unwrap()
}

/// Wraps a request message in a `tonic::Request` carrying an `authorization: Bearer <token>`
/// metadata entry.
fn authed<T>(msg: T, token: &str) -> tonic::Request<T> {
    let mut req = tonic::Request::new(msg);
    support::grpc_bearer(&mut req, token);
    req
}

fn retire_req(policy_id: &str, ack: bool) -> RetireSystemPolicyRequest {
    RetireSystemPolicyRequest {
        policy_id: policy_id.to_string(),
        acknowledge_decision_change: ack,
    }
}

/// 1. A template orphan with no surviving grants retires outright: `Retired`, its role row
/// gone, and the policy row gone too.
#[tokio::test]
async fn retiring_an_orphan_template_returns_the_retired_variant() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let seed_db = db.clone();
    let state = AppState::new(db, &support::test_config(&idp)).await.unwrap();
    let token = idp.bearer("grpc-retire-template-admin", Some("grpc-retire-template-admin@example.com"), "paigasus", 3600);
    support::provision_platform_admin(&state, &token).await;
    seed_orphan_chain(&seed_db, "legacy_auditor").await;
    let (addr, server) = spawn_server(state).await;
    let mut client = AuthorizationServiceClient::new(channel(addr).await);

    let resp = client.retire_system_policy(authed(retire_req("legacy_auditor", false), &token)).await.unwrap().into_inner();
    match resp.outcome.expect("outcome must be set") {
        Outcome::Retired(r) => {
            assert_eq!(r.policy_id, "legacy_auditor");
            assert_eq!(r.kind, "template");
            assert!(r.role_deleted, "an orphan template always carries a role row");
        }
        other => panic!("expected Retired, got {other:?}"),
    }
    assert!(policy_row(&seed_db, "legacy_auditor").await.is_none(), "the policy row must be gone");
    assert!(role_row(&seed_db, "legacy_auditor").await.is_none(), "the role row must be gone");

    server.abort();
}

/// 2. Surviving grants block the retirement outright: `Blocked`, carrying the grant list and
/// the TRUE `total_surviving`/`truncated` — and nothing was written (the policy row survives).
#[tokio::test]
async fn surviving_grants_return_the_blocked_variant_with_the_true_total() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let seed_db = db.clone();
    let state = AppState::new(db, &support::test_config(&idp)).await.unwrap();
    let token = idp.bearer("grpc-retire-blocked-admin", Some("grpc-retire-blocked-admin@example.com"), "paigasus", 3600);
    support::provision_platform_admin(&state, &token).await;
    seed_orphan_chain(&seed_db, "legacy_auditor").await;
    // Shuffled insertion order, and well clear of `support::next_grant_id()`'s own
    // process-global counter (shared across every concurrently-running test in this binary,
    // starting at 1) — `provision_platform_admin` above already consumed one of ITS low
    // values for the platform_admin grant it seeds, so reusing small integers here risks a
    // `role_grant_pkey` collision in this same per-test database. An "ordered by id ascending"
    // assertion below can't pass by coincidence of insertion order (mirrors the pg-level
    // suite's own caution).
    let ids = [Uuid::from_u128(1_000_003), Uuid::from_u128(1_000_001), Uuid::from_u128(1_000_002)];
    seed_grants(&seed_db, "legacy_auditor", &ids).await;
    let (addr, server) = spawn_server(state).await;
    let mut client = AuthorizationServiceClient::new(channel(addr).await);

    let resp = client.retire_system_policy(authed(retire_req("legacy_auditor", false), &token)).await.unwrap().into_inner();
    match resp.outcome.expect("outcome must be set") {
        Outcome::Blocked(b) => {
            assert_eq!(b.role_key, "legacy_auditor");
            assert_eq!(b.total_surviving, 3, "the true total, not a page size");
            assert!(!b.truncated, "3 survivors under the 100-row cap must not report truncated");
            let got: Vec<(String, String, String)> = b.grants.into_iter().map(|g| (g.id, g.principal_prn, g.scope_prn)).collect();
            let want: Vec<(String, String, String)> = [Uuid::from_u128(1_000_001), Uuid::from_u128(1_000_002), Uuid::from_u128(1_000_003)]
                .into_iter()
                .map(|id| (id.to_string(), grant_principal_prn(id), GrantScope::Root.canonical_prn()))
                .collect();
            assert_eq!(got, want, "ordered by id ascending, with the true grant/principal/scope fields");
        }
        other => panic!("expected Blocked while grants survive, got {other:?}"),
    }
    assert!(policy_row(&seed_db, "legacy_auditor").await.is_some(), "a blocked retirement must write nothing");
    assert!(role_row(&seed_db, "legacy_auditor").await.is_some(), "a blocked retirement must write nothing");

    server.abort();
}

/// 3. A static orphan without acknowledgement refuses with `NeedsAcknowledgement`, previewing
/// exactly what would be destroyed — and writes nothing.
#[tokio::test]
async fn a_static_policy_without_acknowledgement_returns_the_needs_acknowledgement_variant() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let seed_db = db.clone();
    let state = AppState::new(db, &support::test_config(&idp)).await.unwrap();
    let token = idp.bearer("grpc-retire-needsack-admin", Some("grpc-retire-needsack-admin@example.com"), "paigasus", 3600);
    support::provision_platform_admin(&state, &token).await;
    seed_system_policy_with_revision(&seed_db, "legacy_forbid", Some(ORPHAN_REVISION)).await;
    // A NON-EMPTY description, set deliberately (mirrors the pg-level suite's own fixture):
    // the shared seeder stores NULL, and asserting an empty-string preview would pass just as
    // well if the field were dropped entirely.
    let mut described: policy::ActiveModel = policy_row(&seed_db, "legacy_forbid").await.unwrap().into();
    described.description = Set(Some("retired guard, kept for the audit trail".to_string()));
    described.update(&seed_db).await.unwrap();
    let (addr, server) = spawn_server(state).await;
    let mut client = AuthorizationServiceClient::new(channel(addr).await);

    let resp = client.retire_system_policy(authed(retire_req("legacy_forbid", false), &token)).await.unwrap().into_inner();
    match resp.outcome.expect("outcome must be set") {
        Outcome::NeedsAcknowledgement(n) => {
            assert_eq!(n.policy_id, "legacy_forbid");
            assert_eq!(n.kind, "static");
            assert_eq!(n.source, STATIC_ORPHAN_SOURCE, "the refusal must preview what would be lost");
            assert_eq!(n.description, "retired guard, kept for the audit trail");
        }
        other => panic!("expected NeedsAcknowledgement, got {other:?}"),
    }
    assert!(policy_row(&seed_db, "legacy_forbid").await.is_some(), "an unacknowledged refusal must delete nothing");

    server.abort();
}

/// 4. The same request with `acknowledge_decision_change: true` retires the static policy.
#[tokio::test]
async fn an_acknowledged_static_policy_retires() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let seed_db = db.clone();
    let state = AppState::new(db, &support::test_config(&idp)).await.unwrap();
    let token = idp.bearer("grpc-retire-ack-admin", Some("grpc-retire-ack-admin@example.com"), "paigasus", 3600);
    support::provision_platform_admin(&state, &token).await;
    seed_system_policy_with_revision(&seed_db, "legacy_forbid", Some(ORPHAN_REVISION)).await;
    let (addr, server) = spawn_server(state).await;
    let mut client = AuthorizationServiceClient::new(channel(addr).await);

    let resp = client.retire_system_policy(authed(retire_req("legacy_forbid", true), &token)).await.unwrap().into_inner();
    match resp.outcome.expect("outcome must be set") {
        Outcome::Retired(r) => {
            assert_eq!(r.policy_id, "legacy_forbid");
            assert_eq!(r.kind, "static");
            assert!(!r.role_deleted, "a static orphan has no role row");
        }
        other => panic!("expected Retired once acknowledged, got {other:?}"),
    }
    assert!(policy_row(&seed_db, "legacy_forbid").await.is_none());
    assert!(role_row(&seed_db, "legacy_forbid").await.is_none(), "no role row should ever have existed");

    server.abort();
}

/// 5. `SystemRetirementService::retire` is Root-only, enforced INSIDE the service — a caller
/// with no grant at Root is denied before any handler-specific logic runs.
#[tokio::test]
async fn a_non_root_caller_is_permission_denied() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let seed_db = db.clone();
    let state = AppState::new(db, &support::test_config(&idp)).await.unwrap();
    let token = idp.bearer("grpc-retire-nonadmin", Some("grpc-retire-nonadmin@example.com"), "paigasus", 3600);
    // An ORDINARY principal: JIT-provisioned, deliberately NOT `platform_admin`.
    support::provision(&state, &token).await;
    seed_orphan_chain(&seed_db, "legacy_auditor").await;
    let (addr, server) = spawn_server(state).await;
    let mut client = AuthorizationServiceClient::new(channel(addr).await);

    let err = client.retire_system_policy(authed(retire_req("legacy_auditor", false), &token)).await.unwrap_err();
    assert_eq!(err.code(), Code::PermissionDenied, "{err:?}");
    assert!(policy_row(&seed_db, "legacy_auditor").await.is_some(), "a denied caller must write nothing");
    assert!(role_row(&seed_db, "legacy_auditor").await.is_some(), "a denied caller must write nothing");

    server.abort();
}

/// 6. `AuthorizationService` carries no `is_exempt` allowlist entry, so an unauthenticated
/// caller gets `Unauthenticated` before ever reaching the handler.
#[tokio::test]
async fn retire_requires_a_bearer() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let state = AppState::new(db, &support::test_config(&idp)).await.unwrap();
    let (addr, server) = spawn_server(state).await;
    let mut client = AuthorizationServiceClient::new(channel(addr).await);

    let err = client.retire_system_policy(retire_req("legacy_auditor", false)).await.unwrap_err();
    assert_eq!(err.code(), Code::Unauthenticated, "{err:?}");

    server.abort();
}

/// 7. `iam.authz.cedar` off: `RetireSystemPolicy` is `Unimplemented`, matching HTTP's
/// 404-by-non-registration (`system_retirement::router()` is merged only under
/// `caps.authz_admin`) — asserted against the EXACT status `convert::capability_disabled`
/// produces (`Code::Unimplemented`, reason `"capability-disabled"`, `capability` metadata
/// `"iam.authz.cedar"`), not merely the code, so an accidental switch to a different
/// `Unimplemented` reason would still fail here.
#[tokio::test]
async fn retire_is_unimplemented_when_authz_admin_is_disabled() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let seed_db = db.clone();
    let mut cfg = support::test_config(&idp);
    cfg.authz.admin_enabled = false;
    let state = AppState::new(db, &cfg).await.unwrap();
    let token = idp.bearer("grpc-retire-authzoff-admin", Some("grpc-retire-authzoff-admin@example.com"), "paigasus", 3600);
    // Root-authorized (would pass the in-service Root check if the RPC were reached at all) —
    // isolating that the refusal is the capability gate, not a permission denial.
    support::provision_platform_admin(&state, &token).await;
    seed_orphan_chain(&seed_db, "legacy_auditor").await;
    let (addr, server) = spawn_server(state).await;
    let mut client = AuthorizationServiceClient::new(channel(addr).await);

    let err = client.retire_system_policy(authed(retire_req("legacy_auditor", false), &token)).await.unwrap_err();
    assert_eq!(err.code(), Code::Unimplemented, "{err:?}");
    let details = tonic_types::StatusExt::get_error_details(&err);
    let info = details.error_info().expect("every IAM status carries ErrorInfo");
    assert_eq!(info.reason, "capability-disabled", "unexpected reason: {info:?}");
    assert_eq!(info.metadata.get("capability").map(String::as_str), Some("iam.authz.cedar"), "unexpected metadata: {info:?}");
    assert!(policy_row(&seed_db, "legacy_auditor").await.is_some(), "a capability-disabled call must never reach the handler");

    server.abort();
}
