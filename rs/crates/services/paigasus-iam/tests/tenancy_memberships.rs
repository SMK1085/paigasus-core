// SPDX-License-Identifier: Apache-2.0

//! `PgMembershipRepository` — end-to-end coverage against real Postgres of the most
//! safety-critical persistence adapter in the tenancy slice: `attach`'s in-txn guard chain
//! (principal exists + prn match, node exists + prn match, effective status, org-membership
//! invariant, duplicate), `detach`'s cascade delete, and the UNION-ALL list queries.
//!
//! Runs against an ephemeral Postgres in Docker. In CI (`CI` env set) a missing Docker
//! daemon is a HARD FAILURE; on a Docker-less laptop the test skips (returns) with a note.

mod support;

use chrono::{DateTime, Duration, SubsecRound, Utc};
use paigasus_iam::adapters::authz::Generations;
use paigasus_iam::adapters::clock::SystemClock;
use paigasus_iam::adapters::id::KernelIdGenerator;
use paigasus_iam::adapters::persistence::{PgMembershipRepository, PgOrganizationRepository, PgPrincipalRepository, PgProjectRepository, PgTeamRepository};
use paigasus_iam_core::{
    Clock, ConflictKind, Email, IdGenerator, Membership, MembershipRepository, NodeStatus, Organization, OrganizationRepository, PreconditionKind, Principal, PrincipalId, PrincipalKind,
    PrincipalRepository, PrincipalStatus, Project, ProjectRepository, RepositoryError, Slug, Team, TeamId, TeamRepository, TenancyNodeRef, User,
};
use paigasus_kernel::{Prn, mint_uuid7};
use sea_orm::DatabaseConnection;
use uuid::Uuid;

/// Seeds a single user-principal via `PgPrincipalRepository::create_user` (M0 pattern from
/// `roundtrip.rs`), keyed by `seed` so distinct calls mint distinct uuids/emails.
async fn seed_user(db: &DatabaseConnection, seed: u8) -> PrincipalId {
    let uuid = mint_uuid7(1_700_000_000_000 + u64::from(seed), [seed; 10]);
    let id = PrincipalId::from_prn(Prn::build("iam", "", None, "principal", uuid).unwrap());
    let now = Utc::now().trunc_subsecs(6);
    let principal = Principal::new(id.clone(), PrincipalKind::User, PrincipalStatus::Active, now, now);
    let user = User::new(id.clone(), Email::parse(&format!("user{seed}@example.com")).unwrap(), format!("User {seed}"), None, None, now, now);
    PgPrincipalRepository::new(db.clone()).create_user(&principal, &user).await.unwrap();
    id
}

/// Builds an `Organization` + its auto-provisioned `"default"` `Team`, mirroring
/// `tenancy_nodes.rs`'s helper of the same shape.
fn new_org_and_default_team(ids: &KernelIdGenerator, clock: &SystemClock, slug: &str, name: &str) -> (Organization, Team) {
    let now = clock.now();
    let org_id = ids.new_organization_id();
    let org = Organization::new(org_id, Slug::parse(slug).unwrap(), name, now).unwrap();
    let team_id = ids.new_team_id(org.id.uuid());
    let default_team = Team::new(team_id, Slug::parse("default").unwrap(), "Default", now).unwrap();
    (org, default_team)
}

/// Seeds a full org -> team -> project chain (mirrors `tenancy_nodes.rs::seed_chain`): an
/// org (via `PgOrganizationRepository::create`, which also provisions the org's own
/// auto-provisioned default team), a separate team under that org, and a project under that
/// (non-default) team.
async fn seed_chain(db: &DatabaseConnection) -> (Organization, Team, Project) {
    let ids = KernelIdGenerator;
    let clock = SystemClock;
    let org_repo = PgOrganizationRepository::new(db.clone(), Generations::memory());
    let team_repo = PgTeamRepository::new(db.clone(), Generations::memory());
    let project_repo = PgProjectRepository::new(db.clone(), Generations::memory());

    let (org, default_team) = new_org_and_default_team(&ids, &clock, "acme", "Acme Corp.");
    org_repo.create(&org, &default_team).await.unwrap();

    let team_id = ids.new_team_id(org.id.uuid());
    let team = Team::new(team_id, Slug::parse("eng").unwrap(), "Engineering", clock.now()).unwrap();
    team_repo.create(&team).await.unwrap();

    let project_id = ids.new_project_id(org.id.uuid());
    let project = Project::new(project_id, team.id.clone(), Slug::parse("web").unwrap(), "Web", clock.now()).unwrap();
    project_repo.create(&project).await.unwrap();

    (org, team, project)
}

/// Builds a `Membership` domain value with an explicit `created_at` (bypassing the real
/// clock) so ordering-sensitive tests get deterministic, strictly increasing timestamps.
fn membership_at(ids: &KernelIdGenerator, principal_id: &PrincipalId, node: TenancyNodeRef, when: DateTime<Utc>) -> Membership {
    Membership::new(ids.new_membership_id(), principal_id.clone(), node, when)
}

#[tokio::test]
async fn attach_happy_path_org_then_team_then_project() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let ids = KernelIdGenerator;
    let clock = SystemClock;
    let (org, team, project) = seed_chain(&db).await;
    let principal = seed_user(&db, 1).await;
    let repo = PgMembershipRepository::new(db.clone());

    let org_membership = membership_at(&ids, &principal, TenancyNodeRef::Organization(org.id.clone()), clock.now());
    let org_record = repo.attach(&org_membership).await.expect("org attach should succeed");
    assert_eq!(org_record.principal_prn, principal.canonical());
    assert_eq!(org_record.node_prn, org.id.canonical());

    let team_membership = membership_at(&ids, &principal, TenancyNodeRef::Team(team.id.clone()), clock.now());
    let team_record = repo.attach(&team_membership).await.expect("team attach should succeed once org membership exists");
    assert_eq!(team_record.node_prn, team.id.canonical());

    let project_membership = membership_at(&ids, &principal, TenancyNodeRef::Project(project.id.clone()), clock.now());
    let project_record = repo.attach(&project_membership).await.expect("project attach should succeed (shares the org membership)");
    assert_eq!(project_record.node_prn, project.id.canonical());

    // `find` round-trips each attached membership.
    let found = repo.find(org_record.id).await.unwrap().expect("org membership row present");
    assert_eq!(found, org_record);
}

#[tokio::test]
async fn attach_team_without_org_membership_is_missing_org_membership() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let ids = KernelIdGenerator;
    let clock = SystemClock;
    let (_org, team, _project) = seed_chain(&db).await;
    let principal = seed_user(&db, 2).await;
    let repo = PgMembershipRepository::new(db.clone());

    let team_membership = membership_at(&ids, &principal, TenancyNodeRef::Team(team.id.clone()), clock.now());
    let result = repo.attach(&team_membership).await;
    assert!(
        matches!(result, Err(RepositoryError::Precondition(PreconditionKind::MissingOrgMembership))),
        "expected Precondition(MissingOrgMembership), got {result:?}"
    );
}

#[tokio::test]
async fn attach_duplicate_org_membership_is_conflict() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let ids = KernelIdGenerator;
    let clock = SystemClock;
    let (org, _team, _project) = seed_chain(&db).await;
    let principal = seed_user(&db, 3).await;
    let repo = PgMembershipRepository::new(db.clone());

    let first = membership_at(&ids, &principal, TenancyNodeRef::Organization(org.id.clone()), clock.now());
    repo.attach(&first).await.expect("first org attach should succeed");

    let second = membership_at(&ids, &principal, TenancyNodeRef::Organization(org.id.clone()), clock.now());
    let result = repo.attach(&second).await;
    assert!(
        matches!(result, Err(RepositoryError::Conflict(ConflictKind::DuplicateMembership))),
        "expected Conflict(DuplicateMembership) from uq_membership_principal_org, got {result:?}"
    );
}

#[tokio::test]
async fn attach_forged_team_prn_is_prn_mismatch() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let ids = KernelIdGenerator;
    let clock = SystemClock;
    let (org, team, _project) = seed_chain(&db).await;
    let principal = seed_user(&db, 4).await;
    let repo = PgMembershipRepository::new(db.clone());

    // Correct team uuid, but a different org uuid forged into the org slot (the
    // forged-org-slot defense: org must come from the persisted row, never the caller's prn).
    let wrong_org = Uuid::from_u128(999_999);
    assert_ne!(wrong_org, org.id.uuid());
    let forged_team_id = TeamId::from_parts(wrong_org, team.id.uuid());
    let forged_membership = membership_at(&ids, &principal, TenancyNodeRef::Team(forged_team_id), clock.now());

    let result = repo.attach(&forged_membership).await;
    assert!(matches!(result, Err(RepositoryError::PrnMismatch)), "expected PrnMismatch, got {result:?}");
}

#[tokio::test]
async fn attach_to_archived_team_is_node_archived() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let ids = KernelIdGenerator;
    let clock = SystemClock;
    let (_org, team, _project) = seed_chain(&db).await;
    let principal = seed_user(&db, 5).await;
    let team_repo = PgTeamRepository::new(db.clone(), Generations::memory());
    let repo = PgMembershipRepository::new(db.clone());

    team_repo.set_status(team.id.uuid(), NodeStatus::Archived, clock.now()).await.unwrap();

    let team_membership = membership_at(&ids, &principal, TenancyNodeRef::Team(team.id.clone()), clock.now());
    let result = repo.attach(&team_membership).await;
    assert!(
        matches!(result, Err(RepositoryError::Precondition(PreconditionKind::NodeArchived))),
        "expected Precondition(NodeArchived) (should fire before the org-membership guard), got {result:?}"
    );
}

#[tokio::test]
async fn attach_unknown_principal_is_not_found() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let ids = KernelIdGenerator;
    let clock = SystemClock;
    let (org, _team, _project) = seed_chain(&db).await;
    let repo = PgMembershipRepository::new(db.clone());

    let unknown_principal = PrincipalId::from_prn(Prn::build("iam", "", None, "principal", Uuid::from_u128(123_456)).unwrap());
    let membership = membership_at(&ids, &unknown_principal, TenancyNodeRef::Organization(org.id.clone()), clock.now());
    let result = repo.attach(&membership).await;
    assert!(matches!(result, Err(RepositoryError::NotFound)), "expected NotFound, got {result:?}");
}

#[tokio::test]
async fn detach_org_cascades_but_leaves_other_principals_untouched() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let ids = KernelIdGenerator;
    let clock = SystemClock;
    let (org, team, project) = seed_chain(&db).await;
    let principal1 = seed_user(&db, 6).await;
    let principal2 = seed_user(&db, 7).await;
    let repo = PgMembershipRepository::new(db.clone());

    // Principal 1: org + team + project memberships.
    let p1_org = membership_at(&ids, &principal1, TenancyNodeRef::Organization(org.id.clone()), clock.now());
    let p1_org_record = repo.attach(&p1_org).await.unwrap();
    let p1_team = membership_at(&ids, &principal1, TenancyNodeRef::Team(team.id.clone()), clock.now());
    repo.attach(&p1_team).await.unwrap();
    let p1_project = membership_at(&ids, &principal1, TenancyNodeRef::Project(project.id.clone()), clock.now());
    repo.attach(&p1_project).await.unwrap();

    // Principal 2: org membership only, in the SAME org.
    let p2_org = membership_at(&ids, &principal2, TenancyNodeRef::Organization(org.id.clone()), clock.now());
    let p2_org_record = repo.attach(&p2_org).await.unwrap();

    assert_eq!(repo.list_by_principal(principal1.uuid(), 10, 0).await.unwrap().len(), 3);
    assert_eq!(repo.list_by_principal(principal2.uuid(), 10, 0).await.unwrap().len(), 1);

    // Detaching principal 1's org membership cascades: its team + project memberships in
    // that org go with it, but principal 2's org membership is untouched.
    repo.detach(p1_org_record.id).await.expect("detach should succeed");
    assert!(
        repo.list_by_principal(principal1.uuid(), 10, 0).await.unwrap().is_empty(),
        "principal 1's memberships should all be gone"
    );
    let p2_remaining = repo.list_by_principal(principal2.uuid(), 10, 0).await.unwrap();
    assert_eq!(p2_remaining.len(), 1, "principal 2's org membership must survive principal 1's cascade");
    assert_eq!(p2_remaining[0].id, p2_org_record.id);
}

#[tokio::test]
async fn detach_unknown_id_is_not_found() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let repo = PgMembershipRepository::new(db.clone());
    let result = repo.detach(Uuid::from_u128(999_999_999)).await;
    assert!(matches!(result, Err(RepositoryError::NotFound)), "expected NotFound, got {result:?}");
}

#[tokio::test]
async fn list_by_principal_orders_by_created_at_and_paginates() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let ids = KernelIdGenerator;
    let (org, team, project) = seed_chain(&db).await;
    let principal = seed_user(&db, 8).await;
    let repo = PgMembershipRepository::new(db.clone());

    let base = Utc::now().trunc_subsecs(6);
    let org_membership = membership_at(&ids, &principal, TenancyNodeRef::Organization(org.id.clone()), base);
    let org_record = repo.attach(&org_membership).await.unwrap();
    let team_membership = membership_at(&ids, &principal, TenancyNodeRef::Team(team.id.clone()), base + Duration::seconds(1));
    let team_record = repo.attach(&team_membership).await.unwrap();
    let project_membership = membership_at(&ids, &principal, TenancyNodeRef::Project(project.id.clone()), base + Duration::seconds(2));
    let project_record = repo.attach(&project_membership).await.unwrap();

    let page1 = repo.list_by_principal(principal.uuid(), 2, 0).await.unwrap();
    assert_eq!(page1.len(), 2);
    assert_eq!(page1[0].id, org_record.id, "page 1 item 0 should be the earliest (org)");
    assert_eq!(page1[1].id, team_record.id, "page 1 item 1 should be the team membership");

    let page2 = repo.list_by_principal(principal.uuid(), 2, 2).await.unwrap();
    assert_eq!(page2.len(), 1);
    assert_eq!(page2[0].id, project_record.id, "page 2 should hold the remaining (project) item");
}

#[tokio::test]
async fn list_by_node_on_org_returns_its_members() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let ids = KernelIdGenerator;
    let clock = SystemClock;
    let (org, _team, _project) = seed_chain(&db).await;
    let principal1 = seed_user(&db, 9).await;
    let principal2 = seed_user(&db, 10).await;
    let repo = PgMembershipRepository::new(db.clone());

    let m1 = membership_at(&ids, &principal1, TenancyNodeRef::Organization(org.id.clone()), clock.now());
    repo.attach(&m1).await.unwrap();
    let m2 = membership_at(&ids, &principal2, TenancyNodeRef::Organization(org.id.clone()), clock.now());
    repo.attach(&m2).await.unwrap();

    let members = repo.list_by_node(&TenancyNodeRef::Organization(org.id.clone()), 10, 0).await.unwrap();
    assert_eq!(members.len(), 2);
    let prns: Vec<&str> = members.iter().map(|m| m.principal_prn.as_str()).collect();
    assert!(prns.contains(&principal1.canonical().as_str()));
    assert!(prns.contains(&principal2.canonical().as_str()));
    assert!(members.iter().all(|m| m.node_prn == org.id.canonical()));
}
