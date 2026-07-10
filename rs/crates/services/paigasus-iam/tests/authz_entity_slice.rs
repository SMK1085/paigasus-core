// SPDX-License-Identifier: Apache-2.0

//! `PgEntitySliceLoader` integration test (SMA-444 Task 12): `load` against a real
//! org -> team -> project chain returns a slice with the synthetic `Root`, the resource's
//! full ancestor chain (each node's own `effective_status`), and the principal; archiving
//! an ancestor is visible on the whole subtree on the very next `load` (no separate
//! propagation step — every node's `effective_status` is re-read fresh, and M1's read
//! adapters already fold ancestor status into it); `load(root_prn(), principal)` returns
//! just `[Root, principal]`; every slice contains exactly one `Root` entity.
//!
//! Runs against an ephemeral Postgres in Docker. In CI (`CI` env set) a missing Docker
//! daemon is a HARD FAILURE; on a Docker-less laptop the test skips (returns) with a note —
//! same gating pattern as `tests/tenancy_nodes.rs`/`tests/authz_role_grants.rs`.

mod support;

use paigasus_iam::adapters::authz::Generations;
use paigasus_iam::adapters::clock::SystemClock;
use paigasus_iam::adapters::id::KernelIdGenerator;
use paigasus_iam::adapters::persistence::{PgEntitySliceLoader, PgOrganizationRepository, PgProjectRepository, PgTeamRepository};
use paigasus_iam_core::authz::model::{ContextValue, ROOT_ENTITY, root_prn};
use paigasus_iam_core::{Clock, EntitySliceLoader, IdGenerator, NodeStatus, Organization, OrganizationRepository, Project, ProjectRepository, Slug, Team, TeamId, TeamRepository};
use paigasus_kernel::{Prn, mint_uuid7, to_cedar_uid};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};
use std::collections::BTreeMap;
use uuid::Uuid;

/// Builds an `Organization` + its auto-provisioned `"default"` `Team`, mirroring
/// `tenancy_orgs.rs`'s helper of the same shape.
fn new_org_and_default_team(ids: &KernelIdGenerator, clock: &SystemClock, slug: &str, name: &str) -> (Organization, Team) {
    let now = clock.now();
    let org_id = ids.new_organization_id();
    let org = Organization::new(org_id, Slug::parse(slug).unwrap(), name, now).unwrap();
    let team_id = ids.new_team_id(org.id.uuid());
    let default_team = Team::new(team_id, Slug::parse("default").unwrap(), "Default", now).unwrap();
    (org, default_team)
}

/// Seeds a full org -> team -> project chain via the real M1 repos (`tenancy_nodes.rs`'s
/// `seed_chain` precedent): an org (with its auto-provisioned default team), a separate
/// team under that org, and a project under that (non-default) team.
async fn seed_chain(db: &DatabaseConnection) -> (Organization, Team, Project) {
    let ids = KernelIdGenerator;
    let clock = SystemClock;
    let org_repo = PgOrganizationRepository::new(db.clone(), Generations::memory());
    let team_repo = PgTeamRepository::new(db.clone(), Generations::memory());
    let project_repo = PgProjectRepository::new(db.clone(), Generations::memory());

    let (org, default_team) = new_org_and_default_team(&ids, &clock, "acme", "Acme Corp.");
    let owner = ids.new_principal_id();
    let grant = support::pg_owner_grant(db, &owner, ids.new_membership_id(), &org.id).await;
    org_repo.create(&org, &default_team, &grant).await.unwrap();

    let team_id = ids.new_team_id(org.id.uuid());
    let team = Team::new(team_id, Slug::parse("eng").unwrap(), "Engineering", clock.now()).unwrap();
    team_repo.create(&team).await.unwrap();

    let project_id = ids.new_project_id(org.id.uuid());
    let project = Project::new(project_id, team.id.clone(), Slug::parse("web").unwrap(), "Web", clock.now()).unwrap();
    project_repo.create(&project).await.unwrap();

    (org, team, project)
}

/// Seeds a `principal` row via raw SQL — this test only needs a valid identity for the
/// loader's principal-status lookup, not the full `PrincipalRepository::create_user` flow
/// (email/user row) — mirroring `authz_role_grants.rs`'s `seed_principal_and_org` (see its
/// doc comment for why an inline UUID literal, not a bind param).
async fn seed_principal(db: &DatabaseConnection, principal_id: Uuid) {
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
}

fn principal_prn(uuid: Uuid) -> Prn {
    Prn::build("iam", "", None, "principal", uuid).unwrap()
}

fn root_uid_pair() -> (String, String) {
    (ROOT_ENTITY.0.to_string(), ROOT_ENTITY.1.to_string())
}

fn effective_status_attrs(status: &str) -> BTreeMap<String, ContextValue> {
    BTreeMap::from([("effective_status".to_string(), ContextValue::Str(status.to_string()))])
}

/// Counts entities in the slice whose uid is the synthetic `Root` uid — every slice must
/// have exactly one, never zero (Root is always injected) and never more than one (the
/// project chain must not re-inject it as a plain ancestor).
fn root_count(slice: &paigasus_iam_core::authz::model::EntitySlice) -> usize {
    slice.entities.iter().filter(|e| e.uid == root_uid_pair()).count()
}

fn find_entity<'a>(slice: &'a paigasus_iam_core::authz::model::EntitySlice, uid: &(String, String)) -> &'a paigasus_iam_core::authz::model::SliceEntity {
    slice
        .entities
        .iter()
        .find(|e| &e.uid == uid)
        .unwrap_or_else(|| panic!("entity {uid:?} not found in slice; got {:?}", slice.entities.iter().map(|e| &e.uid).collect::<Vec<_>>()))
}

#[tokio::test]
async fn authz_slice_full_chain_has_root_org_team_project_and_principal_each_active() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (org, team, project) = seed_chain(&db).await;

    let principal_uuid = mint_uuid7(1_700_000_000_100, [10u8; 10]);
    seed_principal(&db, principal_uuid).await;
    let principal = principal_prn(principal_uuid);

    let loader = PgEntitySliceLoader::new(db.clone(), Generations::memory());
    let slice = loader.load(project.id.prn(), &principal).await.unwrap();

    assert_eq!(slice.entities.len(), 5, "Root + org + team + project + principal, got {:?}", slice.entities);
    assert_eq!(root_count(&slice), 1, "exactly one Root entity");

    let org_uid = {
        let u = to_cedar_uid(org.id.prn());
        (u.entity_type, u.entity_id)
    };
    let team_uid = {
        let u = to_cedar_uid(team.id.prn());
        (u.entity_type, u.entity_id)
    };
    let project_uid = {
        let u = to_cedar_uid(project.id.prn());
        (u.entity_type, u.entity_id)
    };
    let principal_uid = {
        let u = to_cedar_uid(&principal);
        (u.entity_type, u.entity_id)
    };

    let root_entity = find_entity(&slice, &root_uid_pair());
    assert!(root_entity.parents.is_empty(), "Root has no parents");
    assert!(root_entity.attrs.is_empty(), "Root carries no attrs");

    let org_entity = find_entity(&slice, &org_uid);
    assert_eq!(org_entity.parents, vec![root_uid_pair()]);
    assert_eq!(org_entity.attrs, effective_status_attrs("active"));

    let team_entity = find_entity(&slice, &team_uid);
    assert_eq!(team_entity.parents, vec![org_uid.clone()]);
    assert_eq!(team_entity.attrs, effective_status_attrs("active"));

    let project_entity = find_entity(&slice, &project_uid);
    assert_eq!(project_entity.parents, vec![team_uid.clone()]);
    assert_eq!(project_entity.attrs, effective_status_attrs("active"));

    let principal_entity = find_entity(&slice, &principal_uid);
    assert!(principal_entity.parents.is_empty());
    assert_eq!(
        principal_entity.attrs,
        BTreeMap::from([
            ("kind".to_string(), ContextValue::Str("user".to_string())),
            ("status".to_string(), ContextValue::Str("active".to_string()))
        ])
    );
}

#[tokio::test]
async fn authz_slice_archiving_the_org_flips_effective_status_on_the_whole_subtree() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (org, team, project) = seed_chain(&db).await;

    let principal_uuid = mint_uuid7(1_700_000_000_101, [11u8; 10]);
    seed_principal(&db, principal_uuid).await;
    let principal = principal_prn(principal_uuid);

    let loader = PgEntitySliceLoader::new(db.clone(), Generations::memory());

    // Sanity: active before archiving.
    let before = loader.load(project.id.prn(), &principal).await.unwrap();
    let org_uid = {
        let u = to_cedar_uid(org.id.prn());
        (u.entity_type, u.entity_id)
    };
    let team_uid = {
        let u = to_cedar_uid(team.id.prn());
        (u.entity_type, u.entity_id)
    };
    let project_uid = {
        let u = to_cedar_uid(project.id.prn());
        (u.entity_type, u.entity_id)
    };
    assert_eq!(find_entity(&before, &org_uid).attrs, effective_status_attrs("active"));

    // Archive the org via the real repo (D1/D10's own invalidation path — Task 15's
    // `entity_gen` bump is covered separately by `authz_entity_gen_bumps.rs`; this test only
    // asserts the read side folds the new status).
    let org_repo = PgOrganizationRepository::new(db.clone(), Generations::memory());
    let clock = SystemClock;
    org_repo.set_status(org.id.uuid(), NodeStatus::Archived, clock.now()).await.unwrap();

    let after = loader.load(project.id.prn(), &principal).await.unwrap();
    assert_eq!(root_count(&after), 1, "exactly one Root entity even after archiving");
    assert_eq!(find_entity(&after, &org_uid).attrs, effective_status_attrs("archived"), "org's own effective_status must flip");
    assert_eq!(
        find_entity(&after, &team_uid).attrs,
        effective_status_attrs("archived"),
        "team's effective_status folds the archived org"
    );
    assert_eq!(
        find_entity(&after, &project_uid).attrs,
        effective_status_attrs("archived"),
        "project's effective_status folds the archived org"
    );
}

#[tokio::test]
async fn authz_slice_root_resource_returns_only_root_and_principal() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };

    let principal_uuid = mint_uuid7(1_700_000_000_102, [12u8; 10]);
    seed_principal(&db, principal_uuid).await;
    let principal = principal_prn(principal_uuid);

    let loader = PgEntitySliceLoader::new(db.clone(), Generations::memory());
    let slice = loader.load(&root_prn(), &principal).await.unwrap();

    assert_eq!(slice.entities.len(), 2, "just Root + principal, got {:?}", slice.entities);
    assert_eq!(root_count(&slice), 1);
    let principal_uid = {
        let u = to_cedar_uid(&principal);
        (u.entity_type, u.entity_id)
    };
    assert!(slice.entities.iter().any(|e| e.uid == principal_uid));
}

/// A principal with no `principal` row (never provisioned, or a stale PRN) must not fail
/// the whole slice — the loader falls back to `"active"` per its doc comment.
#[tokio::test]
async fn authz_slice_principal_without_a_row_falls_back_to_active_status() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };

    let principal_uuid = mint_uuid7(1_700_000_000_103, [13u8; 10]);
    let principal = principal_prn(principal_uuid); // deliberately not seeded

    let loader = PgEntitySliceLoader::new(db.clone(), Generations::memory());
    let slice = loader.load(&root_prn(), &principal).await.unwrap();

    let principal_uid = {
        let u = to_cedar_uid(&principal);
        (u.entity_type, u.entity_id)
    };
    let entity = find_entity(&slice, &principal_uid);
    assert_eq!(entity.attrs.get("status"), Some(&ContextValue::Str("active".to_string())));
}

/// A resource PRN naming a tenancy node that doesn't exist can't be sliced — `load` must
/// surface an error, never silently return a partial/empty chain. That error is specifically
/// `AuthzError::ResourceNotFound` (SMA-444 review fix), distinct from a genuine
/// `AuthzError::Backend` failure, so `CedarAuthorizer` can fail CLOSED as a `Deny` for a
/// missing resource instead of a 500.
#[tokio::test]
async fn authz_slice_nonexistent_resource_node_is_an_error() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };

    let principal_uuid = mint_uuid7(1_700_000_000_104, [14u8; 10]);
    seed_principal(&db, principal_uuid).await;
    let principal = principal_prn(principal_uuid);

    let loader = PgEntitySliceLoader::new(db.clone(), Generations::memory());
    let missing_org = paigasus_iam_core::OrganizationId::from_uuid(Uuid::from_u128(999_999));
    let err = loader.load(missing_org.prn(), &principal).await.unwrap_err();
    assert!(matches!(err, paigasus_iam_core::AuthzError::ResourceNotFound(_)), "expected AuthzError::ResourceNotFound, got {err:?}");
}

/// SMA-444 cross-tenant-escalation regression (FIX 1, the root-cause fix): loading a slice for
/// a team PRN whose org slot is WRONG must parent the team on its REAL stored org, never the
/// PRN's (forgeable) org slot. `TenancyNodeRef::from_prn`/`TeamId::from_prn` only check that a
/// team PRN's org slot is PRESENT, never that it's CORRECT (`tenancy::check`), so a caller can
/// pair a team's real uuid with an arbitrary org uuid — before this fix, `PgEntitySliceLoader`'s
/// `Team` branch parented the team on exactly that caller-controlled slot, so e.g. an
/// `org_admin` of the WRONG org could pass a Cedar `resource in ?resource` check for a team it
/// (and its org) has no real authority over (see `RoleService::grant`'s anti-escalation check,
/// which authorizes against exactly this slice). This test FAILS pre-fix: it would assert the
/// team is parented on `real_org_uid` but the pre-fix loader parents it on `wrong_org_uid`
/// instead.
#[tokio::test]
async fn authz_slice_team_with_a_forged_org_slot_parents_on_its_real_org_not_the_forged_one() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };

    let ids = KernelIdGenerator;
    let clock = SystemClock;
    let org_repo = PgOrganizationRepository::new(db.clone(), Generations::memory());
    let team_repo = PgTeamRepository::new(db.clone(), Generations::memory());

    // Two independent orgs. `real_team` genuinely lives under `real_org`, NEVER under
    // `wrong_org` — `wrong_org` only ever appears in the forged resource PRN below.
    let (wrong_org, wrong_default_team) = new_org_and_default_team(&ids, &clock, "wrong-org", "Wrong Org");
    let wrong_owner = ids.new_principal_id();
    let wrong_grant = support::pg_owner_grant(&db, &wrong_owner, ids.new_membership_id(), &wrong_org.id).await;
    org_repo.create(&wrong_org, &wrong_default_team, &wrong_grant).await.unwrap();

    let (real_org, real_default_team) = new_org_and_default_team(&ids, &clock, "real-org", "Real Org");
    let real_owner = ids.new_principal_id();
    let real_grant = support::pg_owner_grant(&db, &real_owner, ids.new_membership_id(), &real_org.id).await;
    org_repo.create(&real_org, &real_default_team, &real_grant).await.unwrap();

    let real_team = Team::new(ids.new_team_id(real_org.id.uuid()), Slug::parse("eng").unwrap(), "Engineering", clock.now()).unwrap();
    team_repo.create(&real_team).await.unwrap();

    let principal_uuid = mint_uuid7(1_700_000_000_105, [15u8; 10]);
    seed_principal(&db, principal_uuid).await;
    let principal = principal_prn(principal_uuid);

    // The forged resource PRN: `real_team`'s REAL uuid, paired with `wrong_org`'s uuid in the
    // PRN's org slot — exactly the shape `RoleService::grant`'s `scope_resource_prn` would hand
    // to `Authorize::check` for a forged `scope_prn`.
    let forged_team_prn = TeamId::from_parts(wrong_org.id.uuid(), real_team.id.uuid()).prn().clone();
    assert_ne!(
        forged_team_prn.canonical(),
        real_team.id.canonical(),
        "sanity: the forged prn must differ from the team's real canonical prn"
    );

    let loader = PgEntitySliceLoader::new(db.clone(), Generations::memory());
    let slice = loader.load(&forged_team_prn, &principal).await.unwrap();

    let team_uid = {
        let u = to_cedar_uid(real_team.id.prn());
        (u.entity_type, u.entity_id)
    };
    let real_org_uid = {
        let u = to_cedar_uid(real_org.id.prn());
        (u.entity_type, u.entity_id)
    };
    let wrong_org_uid = {
        let u = to_cedar_uid(wrong_org.id.prn());
        (u.entity_type, u.entity_id)
    };

    let team_entity = find_entity(&slice, &team_uid);
    assert_eq!(
        team_entity.parents,
        vec![real_org_uid.clone()],
        "the team must be parented on its REAL stored org, never the forged PRN's org slot"
    );
    assert_ne!(team_entity.parents, vec![wrong_org_uid.clone()], "sanity: must NOT be parented on the forged org");

    // The slice's own org entity is `real_org` — the loader never even fetches `wrong_org` for
    // this resource.
    assert!(slice.entities.iter().any(|e| e.uid == real_org_uid));
    assert!(!slice.entities.iter().any(|e| e.uid == wrong_org_uid), "the forged org must never appear in a team's own slice");
}

#[tokio::test]
async fn authz_slice_entity_gen_delegates_to_shared_generations() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };

    let gens = Generations::memory();
    let loader = PgEntitySliceLoader::new(db.clone(), gens.clone());

    assert_eq!(loader.entity_gen().await.unwrap(), 0);
    gens.bump_entity_gen().await.unwrap();
    assert_eq!(loader.entity_gen().await.unwrap(), 1, "loader must observe bumps made through the shared Generations handle");
}
