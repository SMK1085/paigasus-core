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
use paigasus_iam::adapters::persistence::PgRoleGrantStore;
use paigasus_iam::adapters::persistence::entities::{policy, role, role_grant};
use paigasus_iam_core::authz::model::root_prn;
use paigasus_iam_core::{AuthzError, GrantScope, OrganizationId, PrincipalId, ProjectId, RoleGrant, RoleGrantStore, TeamId, TenancyNodeRef};
use paigasus_kernel::{Prn, mint_uuid7};
use sea_orm::{ActiveModelTrait, ConnectionTrait, DatabaseConnection, DbBackend, EntityTrait, Set, Statement};
use uuid::Uuid;

/// Seeds a `principal` + `organization` row via raw SQL — this test only needs valid FK
/// targets, not the domain layer — mirroring `authz_schema.rs`'s `seed_principal_and_org`.
/// UUIDs are inlined literals (not bind params): a bound `text` parameter against a `uuid`
/// column needs an explicit cast, whereas an inline literal is coerced from Postgres's
/// "unknown"-typed constant (same reasoning as `authz_schema.rs`).
async fn seed_principal_and_org(db: &DatabaseConnection, principal_id: Uuid, org_id: Uuid) {
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!(
            r#"INSERT INTO "principal" (id, prn, kind, status, created_at, updated_at)
               VALUES ('{principal_id}', 'prn:pgs:iam:::principal/{principal_id}', 'user', 'active', now(), now())"#
        ),
        [],
    ))
    .await
    .unwrap();

    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!(
            r#"INSERT INTO "organization" (id, prn, slug, name, status, created_at, updated_at)
               VALUES ('{org_id}', 'prn:pgs:iam:::organization/{org_id}', 'acme', 'Acme', 'active', now(), now())"#
        ),
        [],
    ))
    .await
    .unwrap();
}

/// Seeds a `team` row under an already-seeded organization — the FK target
/// `fk_team_org`/`fk_role_grant_team` needs. Mirrors `seed_principal_and_org`'s
/// inline-literal convention (see its doc comment for why literals, not bind params).
async fn seed_team(db: &DatabaseConnection, org_id: Uuid, team_id: Uuid) {
    db.execute(Statement::from_sql_and_values(
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
    db.execute(Statement::from_sql_and_values(
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
    assert!(matches!(err, AuthzError::Backend(_)), "expected AuthzError::Backend for a unique-constraint violation, got {err:?}");

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
