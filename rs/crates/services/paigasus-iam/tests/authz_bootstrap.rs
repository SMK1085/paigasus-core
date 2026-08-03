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
//! SMA-477 adds the `SystemPolicyReconciler` cases: `reconcile_system` seeds an absent row
//! (stamping `content_fingerprint`/`starter_revision`), is immediately idempotent, converges a
//! code change silently, converges AND reports an out-of-band edit (handing the destroyed
//! content back for the audit row), restores a cleared `system` flag, defers to a row written
//! by a newer release, adopts a pre-m0010 (NULL-fingerprint) row, bumps `policy_gen` only when
//! policy CONTENT changed, and survives two replicas racing the same absent row. Plus
//! `orphaned_system_policy_ids`, which reports retired system rows and never an operator's own.
//!
//! Runs against an ephemeral Postgres in Docker. In CI (`CI` env set) a missing Docker
//! daemon is a HARD FAILURE; on a Docker-less laptop the test skips (returns) with a note —
//! same gating pattern as `tests/roundtrip.rs`/`tests/authz_role_grants.rs`.

mod support;

use chrono::{DateTime, SubsecRound, Utc};
use paigasus_iam::adapters::authz::{CedarAuthorizer, Generations, GenerationsPolicyGenBumper, GenerationsReader, MemoryDecisionCache, PolicySnapshot, TracingAuditSink};
use paigasus_iam::adapters::clock::SystemClock;
use paigasus_iam::adapters::id::KernelIdGenerator;
use paigasus_iam::adapters::persistence::entities::role;
use paigasus_iam::adapters::persistence::{
    PgAuditLog, PgEntitySliceLoader, PgOrganizationRepository, PgOutbox, PgPolicyStore, PgProjectRepository, PgRoleGrantStore, PgTeamRepository, SeaOrmUnitOfWork,
};
use paigasus_iam::application::authorize::Authorize;
use paigasus_iam::application::bootstrap::reconcile_starter;
use paigasus_iam::application::roles::{RoleService, RoleServiceDeps};
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
    let role_service = RoleService::new(RoleServiceDeps {
        grants: role_grant_store,
        orgs: role_orgs,
        teams: role_teams,
        projects: role_projects,
        authorize: Authorize::new(authz.clone()),
        uow: role_uow,
        outbox: role_outbox,
        audit: role_audit,
        gen_bumper: role_gen_bumper,
        ids: KernelIdGenerator,
        clock: SystemClock,
    });
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

/// Rewrites a stored policy row's content via raw SQL, optionally leaving the fingerprint
/// stale — the difference between "a release changed the code" and "somebody edited the row".
async fn tamper_policy(db: &DatabaseConnection, policy_id: &str, source: &str, fingerprint: Option<&str>) {
    let fp = fingerprint.map_or("NULL".to_string(), |f| format!("'{f}'"));
    db.execute(Statement::from_string(
        DbBackend::Postgres,
        format!(r#"UPDATE "policy" SET source = '{source}', content_fingerprint = {fp} WHERE policy_id = '{policy_id}'"#),
    ))
    .await
    .unwrap();
}

async fn stored_source(db: &DatabaseConnection, policy_id: &str) -> String {
    use paigasus_iam::adapters::persistence::entities::policy;
    policy::Entity::find_by_id(policy_id.to_string()).one(db).await.unwrap().unwrap().source
}

#[tokio::test]
async fn reconcile_system_seeds_stamping_the_fingerprint_and_revision() {
    use paigasus_iam::adapters::persistence::entities::policy;
    use paigasus_iam_core::SystemPolicyReconciler;
    use paigasus_iam_core::authz::reconcile::{StarterPolicyOutcome, content_fingerprint};
    use paigasus_iam_core::authz::roles::STARTER_POLICY_REVISION;

    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let store = PgPolicyStore::new(db.clone(), Generations::memory());
    let doc = starter_policies().into_iter().next().unwrap();

    assert_eq!(store.reconcile_system(&doc, STARTER_POLICY_REVISION).await.unwrap(), StarterPolicyOutcome::Absent);

    let row = policy::Entity::find_by_id(doc.policy_id.clone()).one(&db).await.unwrap().unwrap();
    assert_eq!(row.content_fingerprint.as_deref(), Some(content_fingerprint(doc.kind, &doc.source, &doc.description).as_str()));
    assert_eq!(row.starter_revision, Some(i32::try_from(STARTER_POLICY_REVISION).unwrap()));
    assert!(row.system);

    // Immediately idempotent.
    assert_eq!(store.reconcile_system(&doc, STARTER_POLICY_REVISION).await.unwrap(), StarterPolicyOutcome::Unchanged);
}

#[tokio::test]
async fn reconcile_system_converges_a_code_change_without_reporting_an_edit() {
    use paigasus_iam::adapters::persistence::entities::policy;
    use paigasus_iam_core::SystemPolicyReconciler;
    use paigasus_iam_core::authz::reconcile::{StarterPolicyOutcome, content_fingerprint};
    use paigasus_iam_core::authz::roles::STARTER_POLICY_REVISION;

    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let store = PgPolicyStore::new(db.clone(), Generations::memory());
    let doc = starter_policies().into_iter().next().unwrap();
    store.reconcile_system(&doc, STARTER_POLICY_REVISION).await.unwrap();

    // Simulate "the previous release wrote this": different source, CORRECTLY fingerprinted,
    // stamped with a STRICTLY LOWER revision than this binary carries, and created LONG AGO —
    // the ordinary "an older release seeded this row, we are the upgrade" shape.
    //
    // The backdated `created_at` is what gives the assertion below any teeth. `doc.created_at`
    // is `starter_policies()`'s own `Utc::now()`, and the seeding INSERT wrote exactly that, so
    // reading the seeded value back and re-asserting it would compare `doc.created_at` against
    // itself — bit-identical under BOTH `converged_model(doc, row.created_at, …)` and the
    // `converged_model(doc, doc.created_at, …)` mistake it exists to catch. Forcing the stored
    // value to something `doc` cannot possibly carry is what separates the two.
    let backdated: DateTime<Utc> = "2020-01-01T00:00:00Z".parse().expect("static timestamp literal is valid RFC 3339");
    let old = "forbid(principal, action, resource) when { resource has effective_status };";
    let old_fp = content_fingerprint(doc.kind, old, &doc.description);
    tamper_policy(&db, &doc.policy_id, old, Some(&old_fp)).await;
    db.execute(Statement::from_string(
        DbBackend::Postgres,
        format!(
            r#"UPDATE "policy" SET starter_revision = {}, created_at = TIMESTAMPTZ '2020-01-01 00:00:00+00' WHERE policy_id = '{}'"#,
            STARTER_POLICY_REVISION - 1,
            doc.policy_id
        ),
    ))
    .await
    .unwrap();

    assert_eq!(store.reconcile_system(&doc, STARTER_POLICY_REVISION).await.unwrap(), StarterPolicyOutcome::Reconciled);
    let row = policy::Entity::find_by_id(doc.policy_id.clone()).one(&db).await.unwrap().unwrap();
    assert_eq!(row.source, doc.source);
    // A converge must preserve the STORED row's `created_at` — `reconcile_system`'s UPDATE
    // branch threads `row.created_at` through, deliberately NOT the incoming `doc.created_at`.
    // "Unifying" that with the INSERT branch three lines below it would silently reset every
    // starter policy's creation date on every converging boot, and nothing else in this suite
    // would notice. Mirrors the identical `put_in` invariant pinned in
    // `tests/authz_policy_store.rs`, which forges a `created_at` for the same reason.
    assert_eq!(row.created_at, backdated, "a converge must preserve the stored created_at, not write doc.created_at");
    assert_eq!(row.starter_revision, Some(i32::try_from(STARTER_POLICY_REVISION).unwrap()), "the converge must restamp the revision");
}

#[tokio::test]
async fn reconcile_system_reports_and_reverts_an_out_of_band_edit() {
    use paigasus_iam_core::SystemPolicyReconciler;
    use paigasus_iam_core::authz::reconcile::StarterPolicyOutcome;
    use paigasus_iam_core::authz::roles::STARTER_POLICY_REVISION;

    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let store = PgPolicyStore::new(db.clone(), Generations::memory());
    let doc = starter_policies().into_iter().next().unwrap();
    store.reconcile_system(&doc, STARTER_POLICY_REVISION).await.unwrap();

    // Content rewritten, fingerprint left stale — nobody but us writes that column.
    let edited = "forbid(principal, action, resource) when { resource has effective_status };";
    tamper_policy(&db, &doc.policy_id, edited, Some(&"0".repeat(64))).await;

    let out = store.reconcile_system(&doc, STARTER_POLICY_REVISION).await.unwrap();
    let StarterPolicyOutcome::ExternallyModified {
        content_changed: true,
        previous_content,
    } = out
    else {
        panic!("expected ExternallyModified, got {out:?}")
    };
    assert_eq!(previous_content.source, edited, "the overwritten source must be handed back for the audit row");
    assert_eq!(stored_source(&db, &doc.policy_id).await, doc.source);
}

#[tokio::test]
async fn reconcile_system_restores_a_cleared_system_flag() {
    use paigasus_iam::adapters::persistence::entities::policy;
    use paigasus_iam_core::SystemPolicyReconciler;
    use paigasus_iam_core::authz::reconcile::StarterPolicyOutcome;
    use paigasus_iam_core::authz::roles::STARTER_POLICY_REVISION;

    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let store = PgPolicyStore::new(db.clone(), Generations::memory());
    let doc = starter_policies().into_iter().next().unwrap();
    store.reconcile_system(&doc, STARTER_POLICY_REVISION).await.unwrap();

    // The bypass this guards: clearing `system` must not buy an exemption from convergence.
    // `system = false` is the ONLY thing that changes here — content and fingerprint are left
    // exactly as the seed wrote them. That isolation is the whole point: if the tamper also
    // rewrote `source`, the row would classify `ExternallyModified` on the fingerprint
    // mismatch alone and this test would stay green with `reconcile.rs`'s `!stored.system ||`
    // guard deleted, which is precisely the bypass an adversarial spec review called the
    // cheapest way to exempt a starter policy from convergence forever.
    db.execute(Statement::from_string(
        DbBackend::Postgres,
        format!(r#"UPDATE "policy" SET system = false WHERE policy_id = '{}'"#, doc.policy_id),
    ))
    .await
    .unwrap();

    assert!(matches!(
        store.reconcile_system(&doc, STARTER_POLICY_REVISION).await.unwrap(),
        StarterPolicyOutcome::ExternallyModified { .. }
    ));
    let row = policy::Entity::find_by_id(doc.policy_id.clone()).one(&db).await.unwrap().unwrap();
    assert!(row.system, "system must be restored, not left cleared");
    assert_eq!(row.source, doc.source);
}

#[tokio::test]
async fn reconcile_system_defers_to_a_newer_revision() {
    use paigasus_iam_core::SystemPolicyReconciler;
    use paigasus_iam_core::authz::reconcile::StarterPolicyOutcome;
    use paigasus_iam_core::authz::roles::STARTER_POLICY_REVISION;

    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let store = PgPolicyStore::new(db.clone(), Generations::memory());
    let doc = starter_policies().into_iter().next().unwrap();
    store.reconcile_system(&doc, STARTER_POLICY_REVISION).await.unwrap();

    let newer = "permit(principal, action, resource);";
    db.execute(Statement::from_string(
        DbBackend::Postgres,
        format!(
            r#"UPDATE "policy" SET source = '{newer}', starter_revision = {} WHERE policy_id = '{}'"#,
            STARTER_POLICY_REVISION + 5,
            doc.policy_id
        ),
    ))
    .await
    .unwrap();

    assert_eq!(store.reconcile_system(&doc, STARTER_POLICY_REVISION).await.unwrap(), StarterPolicyOutcome::StaleBinary);
    assert_eq!(stored_source(&db, &doc.policy_id).await, newer, "an older binary must not rewrite a newer release's row");
}

#[tokio::test]
async fn reconcile_system_adopts_a_pre_m0010_row() {
    use paigasus_iam_core::SystemPolicyReconciler;
    use paigasus_iam_core::authz::reconcile::StarterPolicyOutcome;
    use paigasus_iam_core::authz::roles::STARTER_POLICY_REVISION;

    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let store = PgPolicyStore::new(db.clone(), Generations::memory());
    let doc = starter_policies().into_iter().next().unwrap();
    store.reconcile_system(&doc, STARTER_POLICY_REVISION).await.unwrap();

    let old = "forbid(principal, action, resource) when { resource has effective_status };";
    tamper_policy(&db, &doc.policy_id, old, None).await;
    db.execute(Statement::from_string(
        DbBackend::Postgres,
        format!(r#"UPDATE "policy" SET starter_revision = NULL WHERE policy_id = '{}'"#, doc.policy_id),
    ))
    .await
    .unwrap();

    assert!(matches!(
        store.reconcile_system(&doc, STARTER_POLICY_REVISION).await.unwrap(),
        StarterPolicyOutcome::Adopted {
            content_changed: true,
            previous_content: Some(_)
        }
    ));
    assert_eq!(stored_source(&db, &doc.policy_id).await, doc.source);
}

#[tokio::test]
async fn a_fingerprint_only_stamp_does_not_bump_policy_gen_but_a_content_change_does() {
    use paigasus_iam_core::authz::roles::STARTER_POLICY_REVISION;
    use paigasus_iam_core::{PolicyStore, SystemPolicyReconciler};

    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let store = PgPolicyStore::new(db.clone(), Generations::memory());
    let doc = starter_policies().into_iter().next().unwrap();
    store.reconcile_system(&doc, STARTER_POLICY_REVISION).await.unwrap();

    // Clear the fingerprint but leave content correct: a pure stamp, invisible to any decision.
    tamper_policy(&db, &doc.policy_id, &doc.source, None).await;
    let before = store.policy_gen().await.unwrap();
    store.reconcile_system(&doc, STARTER_POLICY_REVISION).await.unwrap();
    assert_eq!(store.policy_gen().await.unwrap(), before, "a stamp changes nothing a decision can observe");

    // Now a real content change.
    tamper_policy(&db, &doc.policy_id, "permit(principal, action, resource);", None).await;
    store.reconcile_system(&doc, STARTER_POLICY_REVISION).await.unwrap();
    assert!(store.policy_gen().await.unwrap() > before, "a content change must invalidate");
}

#[tokio::test]
async fn concurrent_reconcile_of_the_same_absent_policy_yields_exactly_one_row() {
    use paigasus_iam_core::authz::roles::STARTER_POLICY_REVISION;
    use paigasus_iam_core::{PolicyStore, SystemPolicyReconciler};

    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let a = Arc::new(PgPolicyStore::new(db.clone(), Generations::memory()));
    let b = a.clone();
    let doc = Arc::new(starter_policies().into_iter().next().unwrap());
    let (d1, d2) = (doc.clone(), doc.clone());

    let (r1, r2) = tokio::join!(
        tokio::spawn(async move { a.reconcile_system(&d1, STARTER_POLICY_REVISION).await }),
        tokio::spawn(async move { b.reconcile_system(&d2, STARTER_POLICY_REVISION).await }),
    );
    r1.unwrap().expect("racer 1 must not error");
    r2.unwrap().expect("racer 2 must not error");

    let store = PgPolicyStore::new(db.clone(), Generations::memory());
    let rows = store.list_all().await.unwrap();
    assert_eq!(rows.iter().filter(|d| d.policy_id == doc.policy_id).count(), 1);
    assert_eq!(rows.iter().find(|d| d.policy_id == doc.policy_id).unwrap().source, doc.source);
}

#[tokio::test]
async fn orphaned_system_policy_ids_reports_retired_starter_policies_only() {
    use paigasus_iam_core::SystemPolicyReconciler;
    use paigasus_iam_core::authz::roles::{STARTER_POLICY_IDS, STARTER_POLICY_REVISION};

    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let store = PgPolicyStore::new(db.clone(), Generations::memory());
    for doc in starter_policies() {
        store.reconcile_system(&doc, STARTER_POLICY_REVISION).await.unwrap();
    }
    // A system row for a role this build no longer defines.
    db.execute(Statement::from_string(
        DbBackend::Postgres,
        r#"INSERT INTO "policy" (policy_id, kind, source, description, system, created_at, updated_at)
           VALUES ('retired_role', 'template', 'permit(principal == ?principal, action, resource in ?resource);', NULL, true, now(), now())"#
            .to_string(),
    ))
    .await
    .unwrap();
    // An operator's own (non-system) policy must NOT be reported.
    db.execute(Statement::from_string(
        DbBackend::Postgres,
        r#"INSERT INTO "policy" (policy_id, kind, source, description, system, created_at, updated_at)
           VALUES ('operator-policy', 'static', 'permit(principal, action, resource);', NULL, false, now(), now())"#
            .to_string(),
    ))
    .await
    .unwrap();

    let orphans = store.orphaned_system_policy_ids(STARTER_POLICY_IDS).await.unwrap();
    assert_eq!(orphans, vec!["retired_role".to_string()]);

    // `existing_policy_ids` feeds boot's fatal-vs-survivable decision (D12): a row that EXISTS
    // still governs, so a convergence failure over it is survivable, whereas a missing row
    // means the compiled snapshot would be incomplete. It must therefore report EVERY row —
    // including the non-system `operator-policy` that `orphaned_system_policy_ids` filters
    // out. Narrowing it to `system = true` would make an operator policy look absent.
    let mut all_ids = store.existing_policy_ids().await.unwrap();
    all_ids.sort();
    let mut want: Vec<String> = STARTER_POLICY_IDS.iter().map(|id| (*id).to_string()).collect();
    want.push("retired_role".to_string());
    want.push("operator-policy".to_string());
    want.sort();
    assert_eq!(all_ids, want, "existing_policy_ids must report every persisted row, system or not");
}
