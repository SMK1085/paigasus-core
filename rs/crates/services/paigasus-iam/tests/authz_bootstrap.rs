// SPDX-License-Identifier: Apache-2.0

//! `bootstrap::reconcile_starter` integration test (SMA-444 Task 17): against a freshly
//! migrated, unseeded Postgres, `reconcile_starter` seeds every `authz::roles::
//! starter_policies()` document (all `system = true`) and every `authz::roles::
//! system_roles()` row into the `role` table; a second reconcile is idempotent (no
//! duplicate rows, no error, no drift warning since nothing changed). A further end-to-end
//! case proves the seeded set actually enforces: after a bootstrap `platform_admin` grant
//! (seeded directly — the actual bootstrap-admin seeding path is a later task, SMA-444 Task
//! 21), a REAL `RoleService::grant` of `org_admin` (authorized by that bootstrap grant, thus
//! exercising the anti-escalation `GrantRole` check against a real `CedarAuthorizer`) makes a
//! subsequent `is_authorized` call for an org-scoped action ALLOW — seed -> grant ->
//! snapshot-reload -> enforce, real Cedar evaluation throughout, not fakes.
//!
//! Runs against an ephemeral Postgres in Docker. In CI (`CI` env set) a missing Docker
//! daemon is a HARD FAILURE; on a Docker-less laptop the test skips (returns) with a note —
//! same gating pattern as `tests/roundtrip.rs`/`tests/authz_role_grants.rs`.

mod support;

use chrono::{SubsecRound, Utc};
use paigasus_iam::adapters::authz::{CedarAuthorizer, Generations, GenerationsPolicyGenBumper, GenerationsReader, MemoryDecisionCache, PolicySnapshot, TracingAuditSink};
use paigasus_iam::adapters::clock::SystemClock;
use paigasus_iam::adapters::id::KernelIdGenerator;
use paigasus_iam::adapters::persistence::entities::role;
use paigasus_iam::adapters::persistence::{
    PgAuditLog, PgEntitySliceLoader, PgOrganizationRepository, PgOutbox, PgPolicyStore, PgProjectRepository, PgRoleGrantStore, PgTeamRepository, SeaOrmUnitOfWork,
};
use paigasus_iam::application::authorize::Authorize;
use paigasus_iam::application::bootstrap::reconcile_starter;
use paigasus_iam::application::roles::RoleService;
use paigasus_iam_core::authz::roles::{starter_policies, system_roles};
use paigasus_iam_core::{
    AccessRequest, Action, AuditLog, AuditSink, Authorizer, DecisionCache, Effect, EntitySliceLoader, GrantScope, OrganizationId, OrganizationRepository, Outbox, PolicyGenBumper, PolicyStore,
    PrincipalId, ProjectRepository, RequestContext, RoleGrant, RoleGrantStore, TeamRepository, UnitOfWork,
};
use paigasus_kernel::{Prn, mint_uuid7};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, EntityTrait, Statement};
use std::sync::Arc;
use uuid::Uuid;

/// Seeds a `principal` row via raw SQL — mirrors `authz_role_grants.rs`'s
/// `seed_principal_and_org` (inline UUID literals, not bind params — an inline literal is
/// coerced from Postgres's "unknown"-typed constant, whereas a bound `text` parameter
/// against a `uuid` column needs an explicit cast).
async fn seed_principal(db: &DatabaseConnection, id: Uuid) {
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!(r#"INSERT INTO "principal" (id, prn, kind, status, created_at, updated_at) VALUES ('{id}', 'prn:pgs:iam:::principal/{id}', 'user', 'active', now(), now())"#),
        [],
    ))
    .await
    .unwrap();
}

/// Seeds an `organization` row via raw SQL.
async fn seed_org(db: &DatabaseConnection, id: Uuid) {
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!(r#"INSERT INTO "organization" (id, prn, slug, name, status, created_at, updated_at) VALUES ('{id}', 'prn:pgs:iam:::organization/{id}', 'acme', 'Acme', 'active', now(), now())"#),
        [],
    ))
    .await
    .unwrap();
}

#[tokio::test]
async fn reconcile_seeds_every_starter_policy_and_the_seven_system_roles() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };

    let policy_store = PgPolicyStore::new(db.clone(), Generations::memory());
    reconcile_starter(&policy_store, &db).await.unwrap();

    let docs = policy_store.list_all().await.unwrap();
    let expected = starter_policies();
    assert_eq!(docs.len(), expected.len());
    assert!(docs.iter().all(|d| d.system), "every seeded starter policy must be system = true");
    for want in &expected {
        assert!(
            docs.iter().any(|d| d.policy_id == want.policy_id && d.source == want.source && d.kind == want.kind),
            "missing or mismatched seeded policy {}",
            want.policy_id
        );
    }

    let rows = role::Entity::find().all(&db).await.unwrap();
    assert_eq!(rows.len(), system_roles().len());
    for want in system_roles() {
        let row = rows.iter().find(|r| r.key == want.key).unwrap_or_else(|| panic!("role {} not seeded", want.key));
        assert_eq!(row.template_id, want.template_id);
        assert!(row.system);
    }
}

#[tokio::test]
async fn reconcile_is_idempotent_on_a_second_run() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };

    let policy_store = PgPolicyStore::new(db.clone(), Generations::memory());
    reconcile_starter(&policy_store, &db).await.unwrap();
    // A second run must succeed without error and without inserting duplicates or drifting.
    reconcile_starter(&policy_store, &db).await.unwrap();

    let docs = policy_store.list_all().await.unwrap();
    assert_eq!(docs.len(), starter_policies().len(), "a second reconcile must not duplicate policy rows");

    let rows = role::Entity::find().all(&db).await.unwrap();
    assert_eq!(rows.len(), system_roles().len(), "a second reconcile must not duplicate role rows");
}

/// End-to-end: seed -> a real `RoleService::grant` (through a real `CedarAuthorizer`,
/// authorized by a bootstrap `platform_admin` grant) -> the resulting `org_admin` grant
/// makes a real `is_authorized` decision for an org-scoped action ALLOW.
#[tokio::test]
async fn seeded_starter_set_plus_a_real_grant_enforces_end_to_end() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let now = Utc::now().trunc_subsecs(6);

    let bootstrap_admin_uuid = mint_uuid7(1_700_000_000_100, [10u8; 10]);
    let member_uuid = mint_uuid7(1_700_000_000_101, [11u8; 10]);
    let org_uuid = Uuid::from_u128(1);
    seed_principal(&db, bootstrap_admin_uuid).await;
    seed_principal(&db, member_uuid).await;
    seed_org(&db, org_uuid).await;

    // One shared `Generations` handle across every store AND the authorizer's own reader —
    // this is what lets a later `RoleService::grant` (through `role_grant_store`) become
    // visible to `is_authorized` (through `CedarAuthorizer`) without a manual reload:
    // `grant` bumps the SAME counter `CedarAuthorizer` checks before every decision.
    let gens = Generations::memory();
    let policy_store: Arc<dyn PolicyStore> = Arc::new(PgPolicyStore::new(db.clone(), gens.clone()));
    reconcile_starter(policy_store.as_ref(), &db).await.unwrap();

    let role_grant_store: Arc<dyn RoleGrantStore> = Arc::new(PgRoleGrantStore::new(db.clone(), gens.clone()));
    let snapshot = Arc::new(PolicySnapshot::new(policy_store.clone(), role_grant_store.clone()).await.unwrap());
    let slices: Arc<dyn EntitySliceLoader> = Arc::new(PgEntitySliceLoader::new(db.clone(), gens.clone()));
    let decisions: Arc<dyn DecisionCache> = Arc::new(MemoryDecisionCache::new());
    let authz: Arc<dyn Authorizer> = Arc::new(CedarAuthorizer::new(
        snapshot,
        slices,
        decisions,
        Arc::new(gens.clone()) as Arc<dyn GenerationsReader>,
        Arc::new(TracingAuditSink) as Arc<dyn AuditSink>,
    ));

    // Bootstrap: seed ONE `platform_admin` grant directly through the store, bypassing
    // `RoleService::grant`'s own anti-escalation check — there is necessarily no prior
    // authority to authorize the very first grant against (the actual bootstrap-admin
    // seeding path is SMA-444 Task 21; here we only need one authority to exist so the
    // NEXT grant below can go through the real `RoleService`/`Authorize` code path).
    let bootstrap_principal = PrincipalId::from_prn(Prn::build("iam", "", None, "principal", bootstrap_admin_uuid).unwrap());
    let bootstrap_grant_id = Uuid::from_u128(9_000);
    role_grant_store
        .grant(&RoleGrant {
            id: bootstrap_grant_id,
            principal: bootstrap_principal.clone(),
            role_key: "platform_admin".to_string(),
            scope: GrantScope::Root,
            linked_policy_id: format!("grant:{bootstrap_grant_id}"),
            created_at: now,
        })
        .await
        .unwrap();

    // The real use case under test: a `platform_admin`-scoped actor grants `org_admin` on a
    // real organization to another principal, through `RoleService::grant`'s full
    // parse/validate/anti-escalation-authorize/persist pipeline against a real Cedar engine.
    let role_orgs: Arc<dyn OrganizationRepository> = Arc::new(PgOrganizationRepository::new(db.clone(), gens.clone()));
    let role_teams: Arc<dyn TeamRepository> = Arc::new(PgTeamRepository::new(db.clone(), gens.clone()));
    let role_projects: Arc<dyn ProjectRepository> = Arc::new(PgProjectRepository::new(db.clone(), gens.clone()));
    // SMA-446 Task B4: the same UoW reference-pattern wiring `AppState::new` uses — a real
    // `SeaOrmUnitOfWork`/`PgOutbox`/`PgAuditLog` over `db`, and a `GenerationsPolicyGenBumper`
    // over the SAME `gens` handle this test's `CedarAuthorizer` reads through (so the grant
    // below's post-commit bump is the one `is_authorized`'s pre-decision reload observes).
    let role_uow: Arc<dyn UnitOfWork> = Arc::new(SeaOrmUnitOfWork::new(db.clone()));
    let role_outbox: Arc<dyn Outbox> = Arc::new(PgOutbox::new());
    let role_audit: Arc<dyn AuditLog> = Arc::new(PgAuditLog::new(db.clone()));
    let role_gen_bumper: Arc<dyn PolicyGenBumper> = Arc::new(GenerationsPolicyGenBumper::new(gens.clone()));
    let role_service = RoleService::new(
        role_grant_store,
        role_orgs,
        role_teams,
        role_projects,
        Authorize::new(authz.clone()),
        role_uow,
        role_outbox,
        role_audit,
        role_gen_bumper,
        KernelIdGenerator,
        SystemClock,
    );
    let bootstrap_actor = bootstrap_principal.prn().clone();
    let member_principal = PrincipalId::from_prn(Prn::build("iam", "", None, "principal", member_uuid).unwrap());
    let org = OrganizationId::from_uuid(org_uuid);

    role_service
        .grant(&bootstrap_actor, &member_principal.canonical(), "org_admin", &org.canonical())
        .await
        .expect("a platform_admin-scoped actor must be authorized to GrantRole anywhere, including a fresh org");

    // Proof: the member, who held no authority a moment ago, can now GetOrganization on
    // their own org — the boot-seeded `org_admin` template, linked by the grant just made,
    // was picked up by `CedarAuthorizer`'s pre-decision reload (same `gens` counter) without
    // any manual snapshot manipulation from this test.
    let req = AccessRequest {
        principal: member_principal.prn().clone(),
        action: Action::GetOrganization,
        resource: org.prn().clone(),
        context: RequestContext::empty(),
    };
    let decision = authz.is_authorized(&req).await.unwrap();
    assert_eq!(decision.effect, Effect::Allow, "org_admin on its own org must allow GetOrganization: {decision:?}");
}
