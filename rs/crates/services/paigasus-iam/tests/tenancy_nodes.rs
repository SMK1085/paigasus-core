// SPDX-License-Identifier: Apache-2.0

//! `PgTeamRepository` + `PgProjectRepository` — end-to-end coverage against real Postgres:
//! in-txn parent guards (D8: `NotFound`/`Precondition(ParentArchived)`), the effective-status
//! matrix (spec §7's SQL-vs-core parity test), per-parent slug scoping (D7), and D10's
//! "`set_status` always permitted, own flag survives ancestor restore" rule.
//!
//! Runs against an ephemeral Postgres in Docker. In CI (`CI` env set) a missing Docker
//! daemon is a HARD FAILURE; on a Docker-less laptop the test skips (returns) with a note.

mod support;

use paigasus_iam::adapters::clock::SystemClock;
use paigasus_iam::adapters::id::KernelIdGenerator;
use paigasus_iam::adapters::persistence::{PgOrganizationRepository, PgProjectRepository, PgTeamRepository};
use paigasus_iam_core::{
    Clock, ConflictKind, IdGenerator, NodeStatus, Organization, OrganizationRepository, PreconditionKind, Project, ProjectRepository, RepositoryError, Slug, Team, TeamRepository,
};
use sea_orm::DatabaseConnection;
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

/// Seeds a full org -> team -> project chain: an org (via `PgOrganizationRepository::create`,
/// which also provisions the org's own auto-provisioned default team), a separate team under
/// that org, and a project under that (non-default) team. Returns the three domain values a
/// test drives `set_status`/`find` against.
async fn seed_chain(db: &DatabaseConnection) -> (Organization, Team, Project) {
    let ids = KernelIdGenerator;
    let clock = SystemClock;
    let org_repo = PgOrganizationRepository::new(db.clone());
    let team_repo = PgTeamRepository::new(db.clone());
    let project_repo = PgProjectRepository::new(db.clone());

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

#[tokio::test]
async fn create_guards_against_missing_and_archived_parents() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let ids = KernelIdGenerator;
    let clock = SystemClock;
    let org_repo = PgOrganizationRepository::new(db.clone());
    let team_repo = PgTeamRepository::new(db.clone());
    let project_repo = PgProjectRepository::new(db.clone());

    // Team under a random/missing org -> NotFound.
    let missing_org = Uuid::from_u128(999);
    let orphan_team = Team::new(ids.new_team_id(missing_org), Slug::parse("eng").unwrap(), "Engineering", clock.now()).unwrap();
    let result = team_repo.create(&orphan_team).await;
    assert!(matches!(result, Err(RepositoryError::NotFound)), "expected NotFound, got {result:?}");

    // Seed a real org, then archive it.
    let (org, default_team) = new_org_and_default_team(&ids, &clock, "acme", "Acme Corp.");
    org_repo.create(&org, &default_team).await.unwrap();
    org_repo.set_status(org.id.uuid(), NodeStatus::Archived, clock.now()).await.unwrap();

    // Team create under an (effectively) archived org -> Precondition(ParentArchived).
    let team = Team::new(ids.new_team_id(org.id.uuid()), Slug::parse("eng").unwrap(), "Engineering", clock.now()).unwrap();
    let result = team_repo.create(&team).await;
    assert!(
        matches!(result, Err(RepositoryError::Precondition(PreconditionKind::ParentArchived))),
        "expected Precondition(ParentArchived), got {result:?}"
    );

    // Restore the org, create the team for real, then archive the team itself.
    org_repo.set_status(org.id.uuid(), NodeStatus::Active, clock.now()).await.unwrap();
    team_repo.create(&team).await.unwrap();
    team_repo.set_status(team.id.uuid(), NodeStatus::Archived, clock.now()).await.unwrap();

    // Project create under an (effectively) archived team -> Precondition(ParentArchived).
    let project = Project::new(ids.new_project_id(org.id.uuid()), team.id.clone(), Slug::parse("web").unwrap(), "Web", clock.now()).unwrap();
    let result = project_repo.create(&project).await;
    assert!(
        matches!(result, Err(RepositoryError::Precondition(PreconditionKind::ParentArchived))),
        "expected Precondition(ParentArchived), got {result:?}"
    );
}

#[tokio::test]
async fn effective_status_matrix_matches_core() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let clock = SystemClock;
    let org_repo = PgOrganizationRepository::new(db.clone());
    let team_repo = PgTeamRepository::new(db.clone());
    let project_repo = PgProjectRepository::new(db.clone());

    let (org, team, project) = seed_chain(&db).await;

    // Spec §7's SQL-vs-core parity test: for every (org, team, project) status combination,
    // drive `set_status` on all three, read the project back, and assert the repo's
    // SQL-computed `effective_status` matches `NodeStatus::effective` called directly — the
    // combination rule must have exactly one source of truth (D1/D10).
    for &org_status in &[NodeStatus::Active, NodeStatus::Archived] {
        for &team_status in &[NodeStatus::Active, NodeStatus::Archived] {
            for &project_status in &[NodeStatus::Active, NodeStatus::Archived] {
                org_repo.set_status(org.id.uuid(), org_status, clock.now()).await.unwrap();
                team_repo.set_status(team.id.uuid(), team_status, clock.now()).await.unwrap();
                project_repo.set_status(project.id.uuid(), project_status, clock.now()).await.unwrap();

                let view = project_repo.find(project.id.uuid()).await.unwrap().expect("project row present");
                let expected = NodeStatus::effective(project_status, &[team_status, org_status]);
                assert_eq!(view.effective_status, expected, "org={org_status:?} team={team_status:?} project={project_status:?}");
            }
        }
    }
}

#[tokio::test]
async fn slug_scopes_are_per_parent() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let ids = KernelIdGenerator;
    let clock = SystemClock;
    let org_repo = PgOrganizationRepository::new(db.clone());
    let team_repo = PgTeamRepository::new(db.clone());
    let project_repo = PgProjectRepository::new(db.clone());

    let (org1, default1) = new_org_and_default_team(&ids, &clock, "acme", "Acme Corp.");
    org_repo.create(&org1, &default1).await.unwrap();
    let (org2, default2) = new_org_and_default_team(&ids, &clock, "beta", "Beta Corp.");
    org_repo.create(&org2, &default2).await.unwrap();

    // Same team slug in two different orgs -> both ok.
    let team1 = Team::new(ids.new_team_id(org1.id.uuid()), Slug::parse("eng").unwrap(), "Engineering", clock.now()).unwrap();
    team_repo.create(&team1).await.unwrap();
    let team2 = Team::new(ids.new_team_id(org2.id.uuid()), Slug::parse("eng").unwrap(), "Engineering", clock.now()).unwrap();
    team_repo.create(&team2).await.unwrap();

    // Same team slug, same org -> Conflict(SlugTaken).
    let team1_dup = Team::new(ids.new_team_id(org1.id.uuid()), Slug::parse("eng").unwrap(), "Engineering Dup", clock.now()).unwrap();
    let result = team_repo.create(&team1_dup).await;
    assert!(
        matches!(result, Err(RepositoryError::Conflict(ConflictKind::SlugTaken))),
        "expected Conflict(SlugTaken), got {result:?}"
    );

    // Same project slug under two different teams -> both ok.
    let project1 = Project::new(ids.new_project_id(org1.id.uuid()), team1.id.clone(), Slug::parse("web").unwrap(), "Web", clock.now()).unwrap();
    project_repo.create(&project1).await.unwrap();
    let project2 = Project::new(ids.new_project_id(org2.id.uuid()), team2.id.clone(), Slug::parse("web").unwrap(), "Web", clock.now()).unwrap();
    project_repo.create(&project2).await.unwrap();

    // Same project slug, same team -> Conflict(SlugTaken).
    let project1_dup = Project::new(ids.new_project_id(org1.id.uuid()), team1.id.clone(), Slug::parse("web").unwrap(), "Web Dup", clock.now()).unwrap();
    let result = project_repo.create(&project1_dup).await;
    assert!(
        matches!(result, Err(RepositoryError::Conflict(ConflictKind::SlugTaken))),
        "expected Conflict(SlugTaken), got {result:?}"
    );
}

#[tokio::test]
async fn set_status_is_always_permitted_and_restore_preserves_own_flags() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let clock = SystemClock;
    let org_repo = PgOrganizationRepository::new(db.clone());
    let team_repo = PgTeamRepository::new(db.clone());

    let (org, team, _project) = seed_chain(&db).await;

    // Archive the org.
    org_repo.set_status(org.id.uuid(), NodeStatus::Archived, clock.now()).await.unwrap();

    // Archiving the team's own status still succeeds even while the org is already
    // effectively archived (D10 truth table row 3: `set_status` is always permitted).
    let archived_team = team_repo.set_status(team.id.uuid(), NodeStatus::Archived, clock.now()).await.unwrap();
    assert_eq!(archived_team.node.status, NodeStatus::Archived);
    assert_eq!(archived_team.effective_status, NodeStatus::Archived);

    // Restoring the org does not clear the team's own archived flag: it stays effectively
    // archived via its own status.
    org_repo.set_status(org.id.uuid(), NodeStatus::Active, clock.now()).await.unwrap();
    let view = team_repo.find(team.id.uuid()).await.unwrap().expect("team row present");
    assert_eq!(view.node.status, NodeStatus::Archived);
    assert_eq!(view.effective_status, NodeStatus::Archived);
}
