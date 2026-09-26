// SPDX-License-Identifier: Apache-2.0

//! `PgRoleGrantStore` integration test (SMA-444 Task 11): `grant` inserts a `role_grant` row
//! for a seeded principal at each of the four `ck_role_grant_scope` kinds — the synthetic
//! Root scope, and organization/team/project tenancy nodes — in each case with the correct
//! `scope_kind`/`scope_*_id` columns, and bumps `policy_gen`; `list_by_principal`/`list_all`
//! reconstruct the domain `RoleGrant` (in particular its `GrantScope`) byte-for-byte; a
//! duplicate `(principal, role, scope)` grant is rejected, not silently swallowed; `revoke`
//! deletes the row and bumps the generation (idempotent — a second revoke of the same id, or
//! of an id that never existed, is a no-op that does not bump again).
//!
//! Runs against an ephemeral Postgres in Docker. In CI (`CI` env set) a missing Docker
//! daemon is a HARD FAILURE; on a Docker-less laptop the test skips (returns) with a note —
//! same gating pattern as `tests/roundtrip.rs`/`tests/authz_policy_store.rs`.

mod support;

use chrono::{DateTime, SubsecRound, Utc};
use paigasus_iam::adapters::authz::Generations;
use paigasus_iam::adapters::id::KernelIdGenerator;
use paigasus_iam::adapters::persistence::entities::{audit_log, event_outbox, policy, role, role_grant};
use paigasus_iam::adapters::persistence::{PgAuditLog, PgOutbox, PgRoleGrantStore, SeaOrmUnitOfWork};
use paigasus_iam_core::authz::model::root_prn;
use paigasus_iam_core::{
    AuditEntry, AuditLog, AuditOutcome, AuthzError, DomainEvent, EventType, GrantScope, IdGenerator, OrganizationId, Outbox, PrincipalId, PrincipalKind, ProjectId, RoleGrant, RoleGrantFilter,
    RoleGrantQuery, RoleGrantStore, TeamId, TenancyNodeRef, UnitOfWork,
};
use paigasus_kernel::{Prn, mint_uuid7};
use sea_orm::{ActiveModelTrait, ActiveValue::NotSet, ConnectionTrait, DatabaseConnection, DbBackend, EntityTrait, Set, Statement};
use uuid::Uuid;

/// Seeds a `principal` + `organization` row — this test only needs valid FK targets, not the
/// domain layer — mirroring `authz_schema.rs`'s `seed_principal_and_org`. SMA-676 R4: this is
/// a thin wrapper over `seed_principal_of_kind`/`seed_org` below, not a second copy of their
/// INSERTs; the principal is always `user`-kind and the org keeps its pre-SMA-676 `acme` slug
/// (nothing in this file reads it — every seeded org has its own unique id per test — but it
/// stays fixed rather than becoming an unlabelled magic literal at this call site).
async fn seed_principal_and_org(db: &DatabaseConnection, principal_id: Uuid, org_id: Uuid) {
    seed_principal_of_kind(db, principal_id, "user").await;
    seed_org(db, org_id, "acme").await;
}

/// Seeds a `team` row under an already-seeded organization — the FK target
/// `fk_team_org`/`fk_role_grant_team` needs. Mirrors `seed_principal_and_org`'s
/// inline-literal convention (see its doc comment for why literals, not bind params).
async fn seed_team(db: &DatabaseConnection, org_id: Uuid, team_id: Uuid) {
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!(
            r#"INSERT INTO "team" (id, org_id, prn, slug, name, status, created_at, updated_at)
               VALUES ('{team_id}', '{org_id}', 'prn:pgs:iam::{org_id}:team/{team_id}', 'core', 'Core', 'active', now(), now())"#
        ),
        [],
    ))
    .await
    .unwrap();
}

/// Seeds a `project` row under an already-seeded team (which must itself already be under
/// `org_id`) — the FK target `fk_project_team`/`fk_role_grant_project` needs.
async fn seed_project(db: &DatabaseConnection, org_id: Uuid, team_id: Uuid, project_id: Uuid) {
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!(
            r#"INSERT INTO "project" (id, team_id, org_id, prn, slug, name, status, created_at, updated_at)
               VALUES ('{project_id}', '{team_id}', '{org_id}', 'prn:pgs:iam::{org_id}:project/{project_id}', 'svc', 'Svc', 'active', now(), now())"#
        ),
        [],
    ))
    .await
    .unwrap();
}

/// Seeds a `policy` (template) + a `role` row referencing it — the `fk_role_grant_role`
/// target every grant below needs — mirroring `authz_schema.rs`'s round-trip fixture. `key`
/// must be unique per call within a test (each test gets its own ephemeral container, so no
/// cross-test collisions).
async fn seed_role(db: &DatabaseConnection, role_key: &str, now: DateTime<Utc>) {
    let template_id = format!("{role_key}_template");
    policy::ActiveModel {
        policy_id: Set(template_id.clone()),
        kind: Set("template".to_string()),
        source: Set("permit(principal == ?principal, action, resource in ?resource);".to_string()),
        description: Set(None),
        system: Set(false),
        created_at: Set(now),
        updated_at: Set(now),
        content_fingerprint: NotSet,
        starter_revision: NotSet,
    }
    .insert(db)
    .await
    .unwrap();

    role::ActiveModel {
        key: Set(role_key.to_string()),
        template_id: Set(template_id),
        scope_kinds: Set(r#"["organization","root"]"#.to_string()),
        description: Set(None),
        system: Set(false),
        created_at: Set(now),
    }
    .insert(db)
    .await
    .unwrap();
}

/// A `RoleGrant` domain value with a deterministic `linked_policy_id` (`grant:<id>`),
/// mirroring `authz::roles`'s test helper and the `link_grant` convention.
fn make_grant(id: Uuid, principal: &PrincipalId, role_key: &str, scope: GrantScope, created_at: DateTime<Utc>) -> RoleGrant {
    RoleGrant {
        id,
        principal: principal.clone(),
        role_key: role_key.to_string(),
        scope,
        linked_policy_id: format!("grant:{id}"),
        created_at,
    }
}

#[tokio::test]
async fn authz_role_grant_org_scoped_insert_bumps_gen_and_list_by_principal_reconstructs_it() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let now = Utc::now().trunc_subsecs(6);

    let principal_uuid = mint_uuid7(1_700_000_000_000, [1u8; 10]);
    let org_uuid = Uuid::from_u128(1);
    seed_principal_and_org(&db, principal_uuid, org_uuid).await;
    seed_role(&db, "org_admin", now).await;

    let principal = PrincipalId::from_prn(Prn::build("iam", "", None, "principal", principal_uuid).unwrap());
    let org = OrganizationId::from_uuid(org_uuid);
    let grant_id = Uuid::from_u128(100);
    let grant = make_grant(grant_id, &principal, "org_admin", GrantScope::Node(TenancyNodeRef::Organization(org.clone())), now);

    let gens = Generations::memory();
    let store = PgRoleGrantStore::new(db.clone(), gens.clone());
    let before = gens.policy_gen().await.unwrap();
    store.grant(&grant).await.unwrap();
    let after = gens.policy_gen().await.unwrap();
    assert_eq!(after, before + 1, "a successful grant must bump policy_gen exactly once");

    // The row's scope columns are exactly what `ck_role_grant_scope`'s `organization` arm
    // requires: `scope_org_id` set, `scope_team_id`/`scope_project_id` NULL.
    let row = role_grant::Entity::find_by_id(grant_id).one(&db).await.unwrap().expect("row present after grant");
    assert_eq!(row.scope_kind, "organization");
    assert_eq!(row.scope_node_prn, org.canonical());
    assert_eq!(row.scope_org_id, Some(org_uuid));
    assert_eq!(row.scope_team_id, None);
    assert_eq!(row.scope_project_id, None);
    assert_eq!(row.linked_policy_id, format!("grant:{grant_id}"));

    let listed = store.list_by_principal(&principal).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0], grant, "list_by_principal must reconstruct the exact RoleGrant, including its GrantScope");
}

#[tokio::test]
async fn authz_role_grant_root_scoped_stores_all_scope_ids_as_null() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let now = Utc::now().trunc_subsecs(6);

    let principal_uuid = mint_uuid7(1_700_000_000_001, [2u8; 10]);
    let org_uuid = Uuid::from_u128(2);
    seed_principal_and_org(&db, principal_uuid, org_uuid).await;
    seed_role(&db, "platform_admin", now).await;

    let principal = PrincipalId::from_prn(Prn::build("iam", "", None, "principal", principal_uuid).unwrap());
    let grant_id = Uuid::from_u128(101);
    let grant = make_grant(grant_id, &principal, "platform_admin", GrantScope::Root, now);

    let store = PgRoleGrantStore::new(db.clone(), Generations::memory());
    store.grant(&grant).await.unwrap();

    let row = role_grant::Entity::find_by_id(grant_id).one(&db).await.unwrap().expect("row present after grant");
    assert_eq!(row.scope_kind, "root");
    assert_eq!(row.scope_node_prn, root_prn().canonical());
    assert_eq!(row.scope_org_id, None);
    assert_eq!(row.scope_team_id, None);
    assert_eq!(row.scope_project_id, None);

    let listed = store.list_by_principal(&principal).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].scope, GrantScope::Root);
}

/// AC — a duplicate `(principal, role, scope)` grant hits `uq_role_grant_principal_role_scope`
/// and must surface as an `AuthzError`, never be silently swallowed; the generation must not
/// bump a second time since nothing new was written.
#[tokio::test]
async fn authz_role_grant_duplicate_principal_role_scope_is_rejected_not_silently_swallowed() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let now = Utc::now().trunc_subsecs(6);

    let principal_uuid = mint_uuid7(1_700_000_000_002, [3u8; 10]);
    let org_uuid = Uuid::from_u128(3);
    seed_principal_and_org(&db, principal_uuid, org_uuid).await;
    seed_role(&db, "org_admin", now).await;

    let principal = PrincipalId::from_prn(Prn::build("iam", "", None, "principal", principal_uuid).unwrap());
    let org = OrganizationId::from_uuid(org_uuid);
    let gens = Generations::memory();
    let store = PgRoleGrantStore::new(db.clone(), gens.clone());

    let first = make_grant(Uuid::from_u128(102), &principal, "org_admin", GrantScope::Node(TenancyNodeRef::Organization(org.clone())), now);
    store.grant(&first).await.unwrap();
    let before = gens.policy_gen().await.unwrap();

    // Same principal + role + scope, but a distinct grant id/linked_policy_id — the
    // `uq_role_grant_principal_role_scope` constraint (not `uq_role_grant_linked_policy`) is
    // what must reject this.
    let dup = make_grant(Uuid::from_u128(103), &principal, "org_admin", GrantScope::Node(TenancyNodeRef::Organization(org.clone())), now);
    let err = store.grant(&dup).await.unwrap_err();
    assert!(
        matches!(err, AuthzError::DuplicateGrant),
        "SMA-676 D9: expected AuthzError::DuplicateGrant for uq_role_grant_principal_role_scope, got {err:?}"
    );

    assert_eq!(gens.policy_gen().await.unwrap(), before, "a rejected grant must not bump policy_gen");
    let listed = store.list_by_principal(&principal).await.unwrap();
    assert_eq!(listed.len(), 1, "the duplicate must not have been written");
}

#[tokio::test]
async fn authz_role_grant_revoke_deletes_row_and_bumps_gen_idempotent_on_second_revoke() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let now = Utc::now().trunc_subsecs(6);

    let principal_uuid = mint_uuid7(1_700_000_000_003, [4u8; 10]);
    let org_uuid = Uuid::from_u128(4);
    seed_principal_and_org(&db, principal_uuid, org_uuid).await;
    seed_role(&db, "org_admin", now).await;

    let principal = PrincipalId::from_prn(Prn::build("iam", "", None, "principal", principal_uuid).unwrap());
    let org = OrganizationId::from_uuid(org_uuid);
    let grant_id = Uuid::from_u128(104);
    let grant = make_grant(grant_id, &principal, "org_admin", GrantScope::Node(TenancyNodeRef::Organization(org)), now);

    let gens = Generations::memory();
    let store = PgRoleGrantStore::new(db.clone(), gens.clone());
    store.grant(&grant).await.unwrap();
    let before = gens.policy_gen().await.unwrap();

    store.revoke(grant_id).await.unwrap();
    assert_eq!(gens.policy_gen().await.unwrap(), before + 1, "a successful revoke must bump policy_gen exactly once");
    assert!(store.list_by_principal(&principal).await.unwrap().is_empty(), "row must be gone after revoke");

    // Idempotent: revoking the same (now-nonexistent) id again is a no-op success that does
    // NOT bump the generation again — mirrors `PgPolicyStore::delete`'s posture.
    store.revoke(grant_id).await.unwrap();
    assert_eq!(gens.policy_gen().await.unwrap(), before + 1, "revoking an already-revoked id must not bump policy_gen again");

    // Revoking an id that was never granted is likewise a no-op success.
    store.revoke(Uuid::from_u128(999_999)).await.unwrap();
    assert_eq!(gens.policy_gen().await.unwrap(), before + 1);
}

#[tokio::test]
async fn authz_role_grant_list_all_returns_every_grant_with_scope_reconstructed() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let now = Utc::now().trunc_subsecs(6);

    let principal_uuid = mint_uuid7(1_700_000_000_004, [5u8; 10]);
    let org_uuid = Uuid::from_u128(5);
    seed_principal_and_org(&db, principal_uuid, org_uuid).await;
    seed_role(&db, "org_admin", now).await;
    seed_role(&db, "platform_admin", now).await;

    let principal = PrincipalId::from_prn(Prn::build("iam", "", None, "principal", principal_uuid).unwrap());
    let org = OrganizationId::from_uuid(org_uuid);
    let org_grant = make_grant(Uuid::from_u128(105), &principal, "org_admin", GrantScope::Node(TenancyNodeRef::Organization(org)), now);
    let root_grant = make_grant(Uuid::from_u128(106), &principal, "platform_admin", GrantScope::Root, now);

    let store = PgRoleGrantStore::new(db.clone(), Generations::memory());
    store.grant(&org_grant).await.unwrap();
    store.grant(&root_grant).await.unwrap();

    let mut all = store.list_all().await.unwrap();
    all.sort_by_key(|g| g.id);
    assert_eq!(all, vec![org_grant, root_grant]);
}

/// Closes the coverage gap on the other two `ck_role_grant_scope` arms (review finding):
/// `organization`/`root` are round-tripped above, but `team`/`project` never were, even
/// though this store feeds the policy-snapshot compile for every scope kind alike. A
/// team-scoped grant must land with exactly `scope_team_id` set — `scope_org_id`/
/// `scope_project_id` NULL (the `team` arm of `ck_role_grant_scope`) — and
/// `list_by_principal` must reconstruct the exact `GrantScope::Node(TenancyNodeRef::Team)`.
#[tokio::test]
async fn authz_role_grant_team_scoped_round_trips() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let now = Utc::now().trunc_subsecs(6);

    let principal_uuid = mint_uuid7(1_700_000_000_005, [6u8; 10]);
    let org_uuid = Uuid::from_u128(6);
    let team_uuid = Uuid::from_u128(60);
    seed_principal_and_org(&db, principal_uuid, org_uuid).await;
    seed_team(&db, org_uuid, team_uuid).await;
    seed_role(&db, "team_admin", now).await;

    let principal = PrincipalId::from_prn(Prn::build("iam", "", None, "principal", principal_uuid).unwrap());
    let team = TeamId::from_parts(org_uuid, team_uuid);
    let grant_id = Uuid::from_u128(107);
    let grant = make_grant(grant_id, &principal, "team_admin", GrantScope::Node(TenancyNodeRef::Team(team.clone())), now);

    let store = PgRoleGrantStore::new(db.clone(), Generations::memory());
    store.grant(&grant).await.unwrap();

    // The row's scope columns are exactly what `ck_role_grant_scope`'s `team` arm requires:
    // `scope_team_id` set, `scope_org_id`/`scope_project_id` NULL.
    let row = role_grant::Entity::find_by_id(grant_id).one(&db).await.unwrap().expect("row present after grant");
    assert_eq!(row.scope_kind, "team");
    assert_eq!(row.scope_node_prn, team.canonical());
    assert_eq!(row.scope_org_id, None);
    assert_eq!(row.scope_team_id, Some(team_uuid));
    assert_eq!(row.scope_project_id, None);

    let listed = store.list_by_principal(&principal).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0], grant, "list_by_principal must reconstruct the exact RoleGrant, including its team-scoped GrantScope");
}

/// Same gap-closing round trip as the team-scoped test above, for `ck_role_grant_scope`'s
/// `project` arm: a project-scoped grant must land with exactly `scope_project_id` set —
/// `scope_org_id`/`scope_team_id` NULL — and reconstruct to
/// `GrantScope::Node(TenancyNodeRef::Project)`.
#[tokio::test]
async fn authz_role_grant_project_scoped_round_trips() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let now = Utc::now().trunc_subsecs(6);

    let principal_uuid = mint_uuid7(1_700_000_000_006, [7u8; 10]);
    let org_uuid = Uuid::from_u128(7);
    let team_uuid = Uuid::from_u128(70);
    let project_uuid = Uuid::from_u128(700);
    seed_principal_and_org(&db, principal_uuid, org_uuid).await;
    seed_team(&db, org_uuid, team_uuid).await;
    seed_project(&db, org_uuid, team_uuid, project_uuid).await;
    seed_role(&db, "project_admin", now).await;

    let principal = PrincipalId::from_prn(Prn::build("iam", "", None, "principal", principal_uuid).unwrap());
    let project = ProjectId::from_parts(org_uuid, project_uuid);
    let grant_id = Uuid::from_u128(108);
    let grant = make_grant(grant_id, &principal, "project_admin", GrantScope::Node(TenancyNodeRef::Project(project.clone())), now);

    let store = PgRoleGrantStore::new(db.clone(), Generations::memory());
    store.grant(&grant).await.unwrap();

    // The row's scope columns are exactly what `ck_role_grant_scope`'s `project` arm requires:
    // `scope_project_id` set, `scope_org_id`/`scope_team_id` NULL.
    let row = role_grant::Entity::find_by_id(grant_id).one(&db).await.unwrap().expect("row present after grant");
    assert_eq!(row.scope_kind, "project");
    assert_eq!(row.scope_node_prn, project.canonical());
    assert_eq!(row.scope_org_id, None);
    assert_eq!(row.scope_team_id, None);
    assert_eq!(row.scope_project_id, Some(project_uuid));

    let listed = store.list_by_principal(&principal).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0], grant, "list_by_principal must reconstruct the exact RoleGrant, including its project-scoped GrantScope");
}

/// SMA-446 Task B4 — the UoW reference pattern's atomicity proof at the store level:
/// `PgRoleGrantStore::grant_in` + `PgOutbox::enqueue` + `PgAuditLog::record`, driven through
/// the SAME `SeaOrmUnitOfWork` transaction and committed together, land as three durable
/// rows — `role_grant`/`event_outbox`/`audit_log` — sharing the ONE correlation id
/// `RoleService` mints per mutation (mirrors `tests/outbox_uow_pg.rs`'s own commit-atomicity
/// scenario, but through the real role-grant store instead of a raw SeaORM insert).
/// `grant_in` itself must NOT bump `policy_gen` — that is the caller's (`RoleService`'s) own
/// awaited, post-commit responsibility (`pg_role_grants.rs` module docs), never the store's.
#[tokio::test]
async fn grant_in_enqueue_and_record_commit_atomically_sharing_correlation_id() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let now = Utc::now().trunc_subsecs(6);

    let principal_uuid = mint_uuid7(1_700_000_000_007, [8u8; 10]);
    let org_uuid = Uuid::from_u128(8);
    seed_principal_and_org(&db, principal_uuid, org_uuid).await;
    seed_role(&db, "org_admin", now).await;

    let principal = PrincipalId::from_prn(Prn::build("iam", "", None, "principal", principal_uuid).unwrap());
    let org = OrganizationId::from_uuid(org_uuid);
    let grant_id = Uuid::from_u128(200);
    let grant = make_grant(grant_id, &principal, "org_admin", GrantScope::Node(TenancyNodeRef::Organization(org.clone())), now);

    let gens = Generations::memory();
    let store = PgRoleGrantStore::new(db.clone(), gens.clone());
    let uow = SeaOrmUnitOfWork::new(db.clone());
    let outbox = PgOutbox::new(true);
    let audit = PgAuditLog::new(db.clone());

    let corr = KernelIdGenerator.new_correlation_id();
    let event = DomainEvent {
        id: KernelIdGenerator.new_event_id(),
        event_type: EventType::RoleGranted,
        schema_version: 1,
        aggregate_prn: principal.canonical(),
        actor_prn: None,
        occurred_at: now,
        payload: serde_json::json!({"grant_id": grant_id, "role_key": "org_admin"}),
        correlation_id: Some(corr),
    };
    let entry = AuditEntry {
        id: KernelIdGenerator.new_audit_id(),
        occurred_at: now,
        actor_prn: None,
        action: "GrantRole".to_string(),
        resource_prn: Some(org.canonical()),
        outcome: AuditOutcome::Committed,
        determining_policies: Vec::new(),
        detail: serde_json::json!({}),
        correlation_id: Some(corr),
    };

    let before = gens.policy_gen().await.unwrap();
    let tx = uow.begin().await.unwrap();
    store.grant_in(&*tx, &grant).await.unwrap();
    outbox.enqueue(&*tx, &event).await.unwrap();
    audit.record(&*tx, &entry).await.unwrap();
    tx.commit().await.unwrap();

    assert!(
        role_grant::Entity::find_by_id(grant_id).one(&db).await.unwrap().is_some(),
        "the committed role_grant row must be visible"
    );
    let outbox_row = event_outbox::Entity::find_by_id(event.id).one(&db).await.unwrap().expect("outbox row present");
    let audit_row = audit_log::Entity::find_by_id(entry.id).one(&db).await.unwrap().expect("audit row present");
    assert_eq!(outbox_row.correlation_id, Some(corr));
    assert_eq!(audit_row.correlation_id, Some(corr));
    assert_eq!(
        outbox_row.correlation_id, audit_row.correlation_id,
        "the outbox event and the audit entry must share one correlation id"
    );

    assert_eq!(
        gens.policy_gen().await.unwrap(),
        before,
        "grant_in must never bump policy_gen itself — that is the caller's own post-commit responsibility"
    );
}

/// Guard D2 (SMA-446 Task B4): a store error mid-txn — here, `grant_in` hitting
/// `uq_role_grant_principal_role_scope` on a duplicate grant — rolls the WHOLE unit of work
/// back: an outbox event and an audit entry enqueued/recorded earlier on the SAME
/// transaction must never become visible either, and `policy_gen` must not move (nothing in
/// this transaction ever committed).
#[tokio::test]
async fn a_store_error_mid_txn_leaves_no_outbox_or_audit_rows_and_no_gen_bump() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let now = Utc::now().trunc_subsecs(6);

    let principal_uuid = mint_uuid7(1_700_000_000_008, [9u8; 10]);
    let org_uuid = Uuid::from_u128(9);
    seed_principal_and_org(&db, principal_uuid, org_uuid).await;
    seed_role(&db, "org_admin", now).await;

    let principal = PrincipalId::from_prn(Prn::build("iam", "", None, "principal", principal_uuid).unwrap());
    let org = OrganizationId::from_uuid(org_uuid);
    let gens = Generations::memory();
    let store = PgRoleGrantStore::new(db.clone(), gens.clone());

    // Seed a first, successfully committed grant out of band — its (principal, role, scope)
    // is what the in-txn attempt below will collide with.
    let first = make_grant(Uuid::from_u128(201), &principal, "org_admin", GrantScope::Node(TenancyNodeRef::Organization(org.clone())), now);
    store.grant(&first).await.unwrap();
    let before = gens.policy_gen().await.unwrap();

    let uow = SeaOrmUnitOfWork::new(db.clone());
    let outbox = PgOutbox::new(true);
    let audit = PgAuditLog::new(db.clone());
    let corr = KernelIdGenerator.new_correlation_id();
    let event = DomainEvent {
        id: KernelIdGenerator.new_event_id(),
        event_type: EventType::RoleGranted,
        schema_version: 1,
        aggregate_prn: principal.canonical(),
        actor_prn: None,
        occurred_at: now,
        payload: serde_json::json!({}),
        correlation_id: Some(corr),
    };
    let entry = AuditEntry {
        id: KernelIdGenerator.new_audit_id(),
        occurred_at: now,
        actor_prn: None,
        action: "GrantRole".to_string(),
        resource_prn: Some(org.canonical()),
        outcome: AuditOutcome::Committed,
        determining_policies: Vec::new(),
        detail: serde_json::json!({}),
        correlation_id: Some(corr),
    };

    let tx = uow.begin().await.unwrap();
    outbox.enqueue(&*tx, &event).await.unwrap();
    audit.record(&*tx, &entry).await.unwrap();

    // Same (principal, role, scope) as `first` — `uq_role_grant_principal_role_scope` rejects
    // it; the txn is now aborted at the DB level and must be dropped, never committed.
    let dup = make_grant(Uuid::from_u128(202), &principal, "org_admin", GrantScope::Node(TenancyNodeRef::Organization(org)), now);
    let err = store.grant_in(&*tx, &dup).await.unwrap_err();
    assert!(
        matches!(err, AuthzError::DuplicateGrant),
        "SMA-676 D9: expected AuthzError::DuplicateGrant for uq_role_grant_principal_role_scope, got {err:?}"
    );
    drop(tx); // no commit -> rollback

    assert!(
        event_outbox::Entity::find_by_id(event.id).one(&db).await.unwrap().is_none(),
        "the rolled-back outbox row must never become visible"
    );
    assert!(
        audit_log::Entity::find_by_id(entry.id).one(&db).await.unwrap().is_none(),
        "the rolled-back audit row must never become visible"
    );
    assert_eq!(gens.policy_gen().await.unwrap(), before, "a rolled-back mid-txn failure must not bump policy_gen");
}

/// SMA-481 D6 — a grant against a role key with no `role` row must report the role as
/// unknown, not as an internal error. This is the state a concurrent grant lands in after a
/// retirement commits: it blocked on the `role` row's `FOR UPDATE` lock, then resumed to find
/// no parent row, failing `fk_role_grant_role`. The principal/org are seeded (a real FK target
/// `role_grant` also needs), but deliberately no `seed_role` call — the missing `role` row is
/// exactly what this test is proving is handled.
#[tokio::test]
async fn granting_a_role_with_no_row_reports_unknown_role_not_internal() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let now = Utc::now().trunc_subsecs(6);

    let principal_uuid = mint_uuid7(1_700_000_000_009, [10u8; 10]);
    let org_uuid = Uuid::from_u128(10);
    seed_principal_and_org(&db, principal_uuid, org_uuid).await;

    let principal = PrincipalId::from_prn(Prn::build("iam", "", None, "principal", principal_uuid).unwrap());
    let grant = make_grant(Uuid::from_u128(300), &principal, "a_role_with_no_row", GrantScope::Root, now);

    let store = PgRoleGrantStore::new(db.clone(), Generations::memory());
    let uow = SeaOrmUnitOfWork::new(db);

    let tx = uow.begin().await.unwrap();
    let err = store.grant_in(&*tx, &grant).await.expect_err("no role row means no grant");
    assert!(
        matches!(err, AuthzError::UnknownRole(ref k) if k == "a_role_with_no_row"),
        "an FK violation on fk_role_grant_role means the role is gone, not that the backend broke; got {err:?}"
    );
}

/// SMA-481 review round 1 — `map_grant_err` must attribute the FK violation by constraint
/// NAME, not just by "some foreign key fired": `role_grant` has five FKs
/// (`fk_role_grant_principal`/`_role`/`_org`/`_team`/`_project`), and mapping all of them to
/// `UnknownRole` would tell the caller the role is missing even when the role is fine and
/// something else — here, the principal — is what's gone. A real `role` row is seeded below;
/// the grant's `principal_id` names no row, so `fk_role_grant_principal` is what must fail,
/// and the caller must be told the backend broke, not that a role it never asked about doesn't
/// exist.
#[tokio::test]
async fn a_missing_principal_is_not_reported_as_an_unknown_role() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let now = Utc::now().trunc_subsecs(6);

    seed_role(&db, "org_admin", now).await;

    let principal_uuid = mint_uuid7(1_700_000_000_010, [11u8; 10]);
    let principal = PrincipalId::from_prn(Prn::build("iam", "", None, "principal", principal_uuid).unwrap());
    let grant = make_grant(Uuid::from_u128(301), &principal, "org_admin", GrantScope::Root, now);

    let store = PgRoleGrantStore::new(db.clone(), Generations::memory());
    let uow = SeaOrmUnitOfWork::new(db);

    let tx = uow.begin().await.unwrap();
    let err = store.grant_in(&*tx, &grant).await.expect_err("no principal row means no grant");
    assert!(
        matches!(err, AuthzError::Backend(_)),
        "a missing principal must not be reported as an unknown role — the role is fine, the principal is what's gone; got {err:?}"
    );
}

/// Seeds a bare `principal` row of `kind` (`user` or `service_account`) — the query's kind
/// join reads this column. Inline literals, for the reason `seed_principal_and_org` states.
async fn seed_principal_of_kind(db: &DatabaseConnection, principal_id: Uuid, kind: &str) {
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!(
            r#"INSERT INTO "principal" (id, prn, kind, status, created_at, updated_at)
               VALUES ('{principal_id}', 'prn:pgs:iam:::principal/{principal_id}', '{kind}', 'active', now(), now())"#
        ),
        [],
    ))
    .await
    .unwrap();
}

/// Seeds a bare `organization` row with its own slug (the slug is unique).
async fn seed_org(db: &DatabaseConnection, org_id: Uuid, slug: &str) {
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!(
            r#"INSERT INTO "organization" (id, prn, slug, name, status, created_at, updated_at)
               VALUES ('{org_id}', 'prn:pgs:iam:::organization/{org_id}', '{slug}', 'Org', 'active', now(), now())"#
        ),
        [],
    ))
    .await
    .unwrap();
}

fn pid(uuid: Uuid) -> PrincipalId {
    PrincipalId::from_prn(Prn::build("iam", "", None, "principal", uuid).unwrap())
}

fn ids(grants: &[RoleGrant]) -> Vec<u128> {
    grants.iter().map(|g| g.id.as_u128()).collect()
}

/// SMA-676 D2, D5, D6, D7 against Postgres: the kind join, the EXACT scope match (a team grant
/// is not an org grant), the Root case, and `ORDER BY principal_id, id` with paging.
#[tokio::test]
async fn role_grant_query_filters_by_exact_scope_role_and_kind_in_principal_then_id_order() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let now = Utc::now().trunc_subsecs(6);

    let (user_a, user_b, bot) = (Uuid::from_u128(0x1), Uuid::from_u128(0x2), Uuid::from_u128(0x3));
    seed_principal_of_kind(&db, user_a, "user").await;
    seed_principal_of_kind(&db, user_b, "user").await;
    seed_principal_of_kind(&db, bot, "service_account").await;
    let (org_o, org_p, team_t) = (Uuid::from_u128(0x100), Uuid::from_u128(0x200), Uuid::from_u128(0x110));
    seed_org(&db, org_o, "org-o").await;
    seed_org(&db, org_p, "org-p").await;
    seed_team(&db, org_o, team_t).await;
    for role in ["gateway_user", "org_admin", "platform_admin"] {
        seed_role(&db, role, now).await;
    }

    let o = GrantScope::Node(TenancyNodeRef::Organization(OrganizationId::from_uuid(org_o)));
    let p = GrantScope::Node(TenancyNodeRef::Organization(OrganizationId::from_uuid(org_p)));
    let t = GrantScope::Node(TenancyNodeRef::Team(TeamId::from_parts(org_o, team_t)));
    let store = PgRoleGrantStore::new(db.clone(), Generations::memory());
    for g in [
        make_grant(Uuid::from_u128(0x1001), &pid(user_b), "gateway_user", o.clone(), now),
        make_grant(Uuid::from_u128(0x1002), &pid(user_a), "gateway_user", o.clone(), now),
        make_grant(Uuid::from_u128(0x1003), &pid(bot), "gateway_user", o.clone(), now),
        make_grant(Uuid::from_u128(0x1004), &pid(user_a), "org_admin", o.clone(), now),
        make_grant(Uuid::from_u128(0x1005), &pid(user_a), "gateway_user", t.clone(), now),
        make_grant(Uuid::from_u128(0x1006), &pid(user_b), "gateway_user", p.clone(), now),
        make_grant(Uuid::from_u128(0x1007), &pid(user_a), "platform_admin", GrantScope::Root, now),
    ] {
        store.grant(&g).await.unwrap();
    }

    let people = RoleGrantFilter::new(None, Some(o.clone()), Some("gateway_user".to_string()), Some(PrincipalKind::User)).unwrap();
    assert_eq!(ids(&RoleGrantQuery::find(&store, &people, 200, 0).await.unwrap()), vec![0x1002, 0x1001], "users only, principal order");

    let all_at_o = RoleGrantFilter::new(None, Some(o.clone()), None, None).unwrap();
    assert_eq!(
        ids(&RoleGrantQuery::find(&store, &all_at_o, 200, 0).await.unwrap()),
        vec![0x1002, 0x1004, 0x1001, 0x1003],
        "principal_id, then id; no team grant"
    );
    assert_eq!(
        ids(&RoleGrantQuery::find(&store, &all_at_o, 2, 1).await.unwrap()),
        vec![0x1004, 0x1001],
        "limit and offset apply after the order"
    );

    let bots = RoleGrantFilter::new(None, Some(o.clone()), None, Some(PrincipalKind::ServiceAccount)).unwrap();
    assert_eq!(ids(&RoleGrantQuery::find(&store, &bots, 200, 0).await.unwrap()), vec![0x1003]);

    let at_team = RoleGrantFilter::new(None, Some(t), None, None).unwrap();
    assert_eq!(ids(&RoleGrantQuery::find(&store, &at_team, 200, 0).await.unwrap()), vec![0x1005], "D5: exact match, the team only");

    let at_root = RoleGrantFilter::new(None, Some(GrantScope::Root), None, None).unwrap();
    assert_eq!(ids(&RoleGrantQuery::find(&store, &at_root, 200, 0).await.unwrap()), vec![0x1007]);

    let a_as_gateway_user = RoleGrantFilter::new(Some(pid(user_a)), None, Some("gateway_user".to_string()), None).unwrap();
    assert_eq!(ids(&RoleGrantQuery::find(&store, &a_as_gateway_user, 200, 0).await.unwrap()), vec![0x1002, 0x1005]);
}

/// SMA-676 Review Focus 4. Only `uq_role_grant_principal_role_scope` means "this grant already
/// exists". A `uq_role_grant_linked_policy` collision is a different grant with a clashing
/// policy id — a defect, and it must stay `Backend`, or `RoleService::grant` would answer it
/// with an unrelated "existing" grant.
#[tokio::test]
async fn a_linked_policy_collision_stays_a_backend_error_not_a_duplicate_grant() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let now = Utc::now().trunc_subsecs(6);
    let principal_uuid = mint_uuid7(1_700_000_000_020, [20u8; 10]);
    let org_uuid = Uuid::from_u128(20);
    seed_principal_and_org(&db, principal_uuid, org_uuid).await;
    seed_role(&db, "org_admin", now).await;
    seed_role(&db, "gateway_user", now).await;

    let principal = pid(principal_uuid);
    let org = GrantScope::Node(TenancyNodeRef::Organization(OrganizationId::from_uuid(org_uuid)));
    let store = PgRoleGrantStore::new(db.clone(), Generations::memory());
    let first = make_grant(Uuid::from_u128(400), &principal, "org_admin", org.clone(), now);
    store.grant(&first).await.unwrap();

    let mut second = make_grant(Uuid::from_u128(401), &principal, "gateway_user", org, now);
    second.linked_policy_id = first.linked_policy_id.clone();
    let err = store.grant(&second).await.unwrap_err();
    assert!(matches!(err, AuthzError::Backend(_)), "a linked-policy collision is not a duplicate grant; got {err:?}");
}
