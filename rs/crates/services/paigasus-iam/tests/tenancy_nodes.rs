// SPDX-License-Identifier: Apache-2.0

//! `PgTeamRepository` + `PgProjectRepository` — end-to-end coverage against real Postgres:
//! in-txn parent guards (D8: `NotFound`/`Precondition(ParentArchived)`), the effective-status
//! matrix (spec §7's SQL-vs-core parity test), per-parent slug scoping (D7), and D10's
//! "`set_status` always permitted, own flag survives ancestor restore" rule.
//!
//! Runs against an ephemeral Postgres in Docker. In CI (`CI` env set) a missing Docker
//! daemon is a HARD FAILURE; on a Docker-less laptop the test skips (returns) with a note.

mod support;

use paigasus_iam::adapters::authz::Generations;
use paigasus_iam::adapters::clock::SystemClock;
use paigasus_iam::adapters::id::KernelIdGenerator;
use paigasus_iam::adapters::persistence::{PgOrganizationRepository, PgProjectRepository, PgTeamRepository};
use paigasus_iam_core::{
    Clock, ConflictKind, IdGenerator, NodeStatus, Organization, OrganizationRepository, PreconditionKind, PrincipalId, Project, ProjectRepository, RepositoryError, Slug, Stamp, Team, TeamRepository,
};
use sea_orm::DatabaseConnection;
use uuid::Uuid;

/// Builds an `Organization` + its auto-provisioned `"default"` `Team`, mirroring
/// `tenancy_orgs.rs`'s helper of the same shape.
fn new_org_and_default_team(ids: &KernelIdGenerator, clock: &SystemClock, actor: &PrincipalId, slug: &str, name: &str) -> (Organization, Team) {
    let stamp = Stamp::new(clock.now(), actor.clone());
    let org_id = ids.new_organization_id();
    let org = Organization::new(org_id, Slug::parse(slug).unwrap(), name, &stamp).unwrap();
    let team_id = ids.new_team_id(org.id.uuid());
    let default_team = Team::new(team_id, Slug::parse("default").unwrap(), "Default", &stamp).unwrap();
    (org, default_team)
}

/// Seeds a full org -> team -> project chain: an org (via `PgOrganizationRepository::create`,
/// which also provisions the org's own auto-provisioned default team), a separate team under
/// that org, and a project under that (non-default) team. Returns the three domain values a
/// test drives `set_status`/`find` against.
async fn seed_chain(db: &DatabaseConnection) -> (Organization, Team, Project) {
    let ids = KernelIdGenerator;
    let clock = SystemClock;
    let org_repo = PgOrganizationRepository::new(db.clone(), Generations::memory());
    let team_repo = PgTeamRepository::new(db.clone(), Generations::memory());
    let project_repo = PgProjectRepository::new(db.clone(), Generations::memory());

    let owner = ids.new_principal_id();
    let (org, default_team) = new_org_and_default_team(&ids, &clock, &owner, "acme", "Acme Corp.");
    let grant = support::pg_owner_grant(db, &owner, ids.new_membership_id(), &org.id).await;
    let create_stamp = Stamp::new(org.created_at, owner.clone());
    org_repo.create(&org, &default_team, &grant, &create_stamp).await.unwrap();

    let team_id = ids.new_team_id(org.id.uuid());
    let team_stamp = Stamp::new(clock.now(), owner.clone());
    let team = Team::new(team_id, Slug::parse("eng").unwrap(), "Engineering", &team_stamp).unwrap();
    team_repo.create(&team, &team_stamp).await.unwrap();

    let project_id = ids.new_project_id(org.id.uuid());
    let project_stamp = Stamp::new(clock.now(), owner.clone());
    let project = Project::new(project_id, team.id.clone(), Slug::parse("web").unwrap(), "Web", &project_stamp).unwrap();
    project_repo.create(&project, &project_stamp).await.unwrap();

    (org, team, project)
}

#[tokio::test]
async fn create_guards_against_missing_and_archived_parents() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let ids = KernelIdGenerator;
    let clock = SystemClock;
    let org_repo = PgOrganizationRepository::new(db.clone(), Generations::memory());
    let team_repo = PgTeamRepository::new(db.clone(), Generations::memory());
    let project_repo = PgProjectRepository::new(db.clone(), Generations::memory());
    let owner = ids.new_principal_id();

    // Team under a random/missing org -> NotFound.
    let missing_org = Uuid::from_u128(999);
    let orphan_stamp = Stamp::new(clock.now(), owner.clone());
    let orphan_team = Team::new(ids.new_team_id(missing_org), Slug::parse("eng").unwrap(), "Engineering", &orphan_stamp).unwrap();
    let result = team_repo.create(&orphan_team, &orphan_stamp).await;
    assert!(matches!(result, Err(RepositoryError::NotFound)), "expected NotFound, got {result:?}");

    // Seed a real org, then archive it.
    let (org, default_team) = new_org_and_default_team(&ids, &clock, &owner, "acme", "Acme Corp.");
    let grant = support::pg_owner_grant(&db, &owner, ids.new_membership_id(), &org.id).await;
    let create_stamp = Stamp::new(org.created_at, owner.clone());
    org_repo.create(&org, &default_team, &grant, &create_stamp).await.unwrap();
    org_repo.set_status(org.id.uuid(), NodeStatus::Archived, &Stamp::new(clock.now(), owner.clone())).await.unwrap();

    // Team create under an (effectively) archived org -> Precondition(ParentArchived).
    let team_stamp = Stamp::new(clock.now(), owner.clone());
    let team = Team::new(ids.new_team_id(org.id.uuid()), Slug::parse("eng").unwrap(), "Engineering", &team_stamp).unwrap();
    let result = team_repo.create(&team, &team_stamp).await;
    assert!(
        matches!(result, Err(RepositoryError::Precondition(PreconditionKind::ParentArchived))),
        "expected Precondition(ParentArchived), got {result:?}"
    );

    // Restore the org, create the team for real, then archive the team itself.
    org_repo.set_status(org.id.uuid(), NodeStatus::Active, &Stamp::new(clock.now(), owner.clone())).await.unwrap();
    team_repo.create(&team, &team_stamp).await.unwrap();
    team_repo.set_status(team.id.uuid(), NodeStatus::Archived, &Stamp::new(clock.now(), owner.clone())).await.unwrap();

    // Project create under an (effectively) archived team -> Precondition(ParentArchived).
    let project_stamp = Stamp::new(clock.now(), owner.clone());
    let project = Project::new(ids.new_project_id(org.id.uuid()), team.id.clone(), Slug::parse("web").unwrap(), "Web", &project_stamp).unwrap();
    let result = project_repo.create(&project, &project_stamp).await;
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
    let ids = KernelIdGenerator;
    let owner = ids.new_principal_id();
    let org_repo = PgOrganizationRepository::new(db.clone(), Generations::memory());
    let team_repo = PgTeamRepository::new(db.clone(), Generations::memory());
    let project_repo = PgProjectRepository::new(db.clone(), Generations::memory());

    let (org, team, project) = seed_chain(&db).await;

    // Spec §7's SQL-vs-core parity test: for every (org, team, project) status combination,
    // drive `set_status` on all three, read the project back, and assert the repo's
    // SQL-computed `effective_status` matches `NodeStatus::effective` called directly — the
    // combination rule must have exactly one source of truth (D1/D10).
    for &org_status in &[NodeStatus::Active, NodeStatus::Archived] {
        for &team_status in &[NodeStatus::Active, NodeStatus::Archived] {
            for &project_status in &[NodeStatus::Active, NodeStatus::Archived] {
                org_repo.set_status(org.id.uuid(), org_status, &Stamp::new(clock.now(), owner.clone())).await.unwrap();
                team_repo.set_status(team.id.uuid(), team_status, &Stamp::new(clock.now(), owner.clone())).await.unwrap();
                project_repo.set_status(project.id.uuid(), project_status, &Stamp::new(clock.now(), owner.clone())).await.unwrap();

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
    let org_repo = PgOrganizationRepository::new(db.clone(), Generations::memory());
    let team_repo = PgTeamRepository::new(db.clone(), Generations::memory());
    let project_repo = PgProjectRepository::new(db.clone(), Generations::memory());

    let owner = ids.new_principal_id();
    let (org1, default1) = new_org_and_default_team(&ids, &clock, &owner, "acme", "Acme Corp.");
    let grant1 = support::pg_owner_grant(&db, &owner, ids.new_membership_id(), &org1.id).await;
    org_repo.create(&org1, &default1, &grant1, &Stamp::new(org1.created_at, owner.clone())).await.unwrap();
    let (org2, default2) = new_org_and_default_team(&ids, &clock, &owner, "beta", "Beta Corp.");
    let grant2 = support::pg_owner_grant(&db, &owner, ids.new_membership_id(), &org2.id).await;
    org_repo.create(&org2, &default2, &grant2, &Stamp::new(org2.created_at, owner.clone())).await.unwrap();

    // Same team slug in two different orgs -> both ok.
    let team1_stamp = Stamp::new(clock.now(), owner.clone());
    let team1 = Team::new(ids.new_team_id(org1.id.uuid()), Slug::parse("eng").unwrap(), "Engineering", &team1_stamp).unwrap();
    team_repo.create(&team1, &team1_stamp).await.unwrap();
    let team2_stamp = Stamp::new(clock.now(), owner.clone());
    let team2 = Team::new(ids.new_team_id(org2.id.uuid()), Slug::parse("eng").unwrap(), "Engineering", &team2_stamp).unwrap();
    team_repo.create(&team2, &team2_stamp).await.unwrap();

    // Same team slug, same org -> Conflict(SlugTaken).
    let team1_dup_stamp = Stamp::new(clock.now(), owner.clone());
    let team1_dup = Team::new(ids.new_team_id(org1.id.uuid()), Slug::parse("eng").unwrap(), "Engineering Dup", &team1_dup_stamp).unwrap();
    let result = team_repo.create(&team1_dup, &team1_dup_stamp).await;
    assert!(
        matches!(result, Err(RepositoryError::Conflict(ConflictKind::SlugTaken))),
        "expected Conflict(SlugTaken), got {result:?}"
    );

    // Same project slug under two different teams -> both ok.
    let project1_stamp = Stamp::new(clock.now(), owner.clone());
    let project1 = Project::new(ids.new_project_id(org1.id.uuid()), team1.id.clone(), Slug::parse("web").unwrap(), "Web", &project1_stamp).unwrap();
    project_repo.create(&project1, &project1_stamp).await.unwrap();
    let project2_stamp = Stamp::new(clock.now(), owner.clone());
    let project2 = Project::new(ids.new_project_id(org2.id.uuid()), team2.id.clone(), Slug::parse("web").unwrap(), "Web", &project2_stamp).unwrap();
    project_repo.create(&project2, &project2_stamp).await.unwrap();

    // Same project slug, same team -> Conflict(SlugTaken).
    let project1_dup_stamp = Stamp::new(clock.now(), owner.clone());
    let project1_dup = Project::new(ids.new_project_id(org1.id.uuid()), team1.id.clone(), Slug::parse("web").unwrap(), "Web Dup", &project1_dup_stamp).unwrap();
    let result = project_repo.create(&project1_dup, &project1_dup_stamp).await;
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
    let ids = KernelIdGenerator;
    let owner = ids.new_principal_id();
    let org_repo = PgOrganizationRepository::new(db.clone(), Generations::memory());
    let team_repo = PgTeamRepository::new(db.clone(), Generations::memory());

    let (org, team, _project) = seed_chain(&db).await;

    // Archive the org.
    org_repo.set_status(org.id.uuid(), NodeStatus::Archived, &Stamp::new(clock.now(), owner.clone())).await.unwrap();

    // Archiving the team's own status still succeeds even while the org is already
    // effectively archived (D10 truth table row 3: `set_status` is always permitted).
    let archived_team = team_repo.set_status(team.id.uuid(), NodeStatus::Archived, &Stamp::new(clock.now(), owner.clone())).await.unwrap();
    assert_eq!(archived_team.node.status, NodeStatus::Archived);
    assert_eq!(archived_team.effective_status, NodeStatus::Archived);

    // Restoring the org does not clear the team's own archived flag: it stays effectively
    // archived via its own status.
    org_repo.set_status(org.id.uuid(), NodeStatus::Active, &Stamp::new(clock.now(), owner.clone())).await.unwrap();
    let view = team_repo.find(team.id.uuid()).await.unwrap().expect("team row present");
    assert_eq!(view.node.status, NodeStatus::Archived);
    assert_eq!(view.effective_status, NodeStatus::Archived);
}

#[tokio::test]
async fn rename_guards_and_lists_round_trip() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let clock = SystemClock;
    let ids = KernelIdGenerator;
    let owner = ids.new_principal_id();
    let org_repo = PgOrganizationRepository::new(db.clone(), Generations::memory());
    let team_repo = PgTeamRepository::new(db.clone(), Generations::memory());
    let project_repo = PgProjectRepository::new(db.clone(), Generations::memory());

    let (org, team, project) = seed_chain(&db).await;
    let team_uuid = team.id.uuid();
    let project_uuid = project.id.uuid();

    // Archive the org.
    org_repo.set_status(org.id.uuid(), NodeStatus::Archived, &Stamp::new(clock.now(), owner.clone())).await.unwrap();

    // Attempt to rename team when org is archived (effective guard, even though team's own
    // status is Active) -> NodeArchived.
    let stamp = Stamp::new(clock.now(), owner.clone());
    let result = team_repo.rename(team_uuid, Some(&Slug::parse("team-renamed").unwrap()), None, &stamp).await;
    assert!(
        matches!(result, Err(RepositoryError::Precondition(PreconditionKind::NodeArchived))),
        "expected Precondition(NodeArchived) for team rename with archived org, got {result:?}"
    );

    // Attempt to rename project when org is archived -> NodeArchived.
    let result = project_repo.rename(project_uuid, None, Some("Renamed"), &stamp).await;
    assert!(
        matches!(result, Err(RepositoryError::Precondition(PreconditionKind::NodeArchived))),
        "expected Precondition(NodeArchived) for project rename with archived org, got {result:?}"
    );

    // Restore the org.
    org_repo.set_status(org.id.uuid(), NodeStatus::Active, &Stamp::new(clock.now(), owner.clone())).await.unwrap();

    // Rename the team: slug only (name stays unchanged).
    let stamp2 = Stamp::new(clock.now(), owner.clone());
    let renamed_team = team_repo
        .rename(team_uuid, Some(&Slug::parse("team-renamed").unwrap()), None, &stamp2)
        .await
        .expect("team rename should succeed");
    assert_eq!(renamed_team.node.slug.as_str(), "team-renamed", "team slug should be updated");
    assert_eq!(renamed_team.node.name, team.name, "team name should remain unchanged");
    assert!(renamed_team.node.updated_at > team.created_at, "updated_at should have advanced");

    // Rename the project: name only (slug stays unchanged).
    let stamp3 = Stamp::new(clock.now(), owner.clone());
    let renamed_project = project_repo.rename(project_uuid, None, Some("Project Renamed"), &stamp3).await.expect("project rename should succeed");
    assert_eq!(renamed_project.node.name, "Project Renamed", "project name should be updated");
    assert_eq!(renamed_project.node.slug.as_str(), project.slug.as_str(), "project slug should remain unchanged");
    assert_eq!(renamed_project.effective_status, NodeStatus::Active, "project should remain effectively Active");

    // List teams by org: should include both the default team (auto-provisioned) and the renamed team.
    let teams = team_repo.list_by_org(org.id.uuid(), 10, 0).await.expect("list_by_org should succeed");
    assert_eq!(teams.len(), 2, "org should have 2 teams (default + eng)");
    // Verify ordering: created_at then id.
    assert!(teams[0].node.created_at <= teams[1].node.created_at, "teams should be ordered by created_at");

    // List projects by team: should include the renamed project.
    let projects = project_repo.list_by_team(team_uuid, 10, 0).await.expect("list_by_team should succeed");
    assert!(projects.iter().any(|p| p.node.id.uuid() == project_uuid), "project should be in list_by_team result");
    let found = projects.iter().find(|p| p.node.id.uuid() == project_uuid).unwrap();
    assert_eq!(found.node.name, "Project Renamed", "renamed project should have updated name");
    assert_eq!(found.effective_status, NodeStatus::Active, "project should remain effectively Active");
}

/// SMA-440 FINDING 1 (final review): the spec's Risks section named "the rename-as-A-then-as-B
/// assertion per aggregate" as a control it never got. Before this test, deleting
/// `active.modified_by = Set(Some(stamp.by.canonical()));` from `pg_organizations.rs`'s
/// rename/set_status sites still left the whole suite green — a rename by one actor could leave
/// a stale `modified_by` naming whoever created the row, forever, with nothing to catch it.
///
/// Uses three DISTINCT actors (A creates, B renames, C archives): reusing one actor would make
/// the assertion unable to tell a stale modifier from a fresh one.
#[tokio::test]
async fn organization_rename_and_archive_restamp_distinct_actors() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let ids = KernelIdGenerator;
    let clock = SystemClock;
    let org_repo = PgOrganizationRepository::new(db.clone(), Generations::memory());

    let actor_a = ids.new_principal_id();
    let actor_b = ids.new_principal_id();
    let actor_c = ids.new_principal_id();

    let (org, default_team) = new_org_and_default_team(&ids, &clock, &actor_a, "acme", "Acme Corp.");
    let grant = support::pg_owner_grant(&db, &actor_a, ids.new_membership_id(), &org.id).await;
    org_repo.create(&org, &default_team, &grant, &Stamp::new(org.created_at, actor_a.clone())).await.unwrap();

    // Rename stamped by actor B: created_by must stay A, modified_by must move to B.
    let renamed = org_repo
        .rename(org.id.uuid(), Some(&Slug::parse("acme-renamed").unwrap()), None, &Stamp::new(clock.now(), actor_b.clone()))
        .await
        .expect("rename should succeed");
    assert_eq!(renamed.node.created_by.as_ref(), Some(&actor_a), "rename must not touch created_by");
    assert_eq!(renamed.node.modified_by.as_ref(), Some(&actor_b), "rename must restamp modified_by to the renaming actor");

    // set_status twin: archive stamped by actor C. created_by must still be A.
    let archived = org_repo
        .set_status(org.id.uuid(), NodeStatus::Archived, &Stamp::new(clock.now(), actor_c.clone()))
        .await
        .expect("archive should succeed");
    assert_eq!(archived.node.created_by.as_ref(), Some(&actor_a), "set_status must not touch created_by");
    assert_eq!(archived.node.modified_by.as_ref(), Some(&actor_c), "set_status must restamp modified_by to the archiving actor");
}

/// SMA-440 FINDING 1 + FINDING 2 (final review): the team twin of
/// `organization_rename_and_archive_restamp_distinct_actors`. Also closes FINDING 2: before this
/// test, `pg_teams.rs`'s `created_by`/`modified_by` columns never round-tripped through Postgres
/// at all — swapping the two field reads in `model_to_team` compiled and passed the whole suite,
/// transposing a team's creator and modifier on every read. Checking BOTH columns here catches
/// that transposition, not only a stale modifier.
#[tokio::test]
async fn team_rename_and_archive_restamp_distinct_actors() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let ids = KernelIdGenerator;
    let clock = SystemClock;
    let org_repo = PgOrganizationRepository::new(db.clone(), Generations::memory());
    let team_repo = PgTeamRepository::new(db.clone(), Generations::memory());

    // The org itself is created by an unrelated owner: this test is about the TEAM's own
    // actor columns, not the org's.
    let owner = ids.new_principal_id();
    let (org, default_team) = new_org_and_default_team(&ids, &clock, &owner, "acme", "Acme Corp.");
    let grant = support::pg_owner_grant(&db, &owner, ids.new_membership_id(), &org.id).await;
    org_repo.create(&org, &default_team, &grant, &Stamp::new(org.created_at, owner.clone())).await.unwrap();

    let actor_a = ids.new_principal_id();
    let actor_b = ids.new_principal_id();
    let actor_c = ids.new_principal_id();

    let team_id = ids.new_team_id(org.id.uuid());
    let create_stamp = Stamp::new(clock.now(), actor_a.clone());
    let team = Team::new(team_id, Slug::parse("eng").unwrap(), "Engineering", &create_stamp).unwrap();
    team_repo.create(&team, &create_stamp).await.unwrap();

    // Rename stamped by actor B: created_by must stay A, modified_by must move to B.
    let renamed = team_repo
        .rename(team.id.uuid(), Some(&Slug::parse("eng-renamed").unwrap()), None, &Stamp::new(clock.now(), actor_b.clone()))
        .await
        .expect("rename should succeed");
    assert_eq!(renamed.node.created_by.as_ref(), Some(&actor_a), "rename must not touch created_by");
    assert_eq!(renamed.node.modified_by.as_ref(), Some(&actor_b), "rename must restamp modified_by to the renaming actor");

    // set_status twin: archive stamped by actor C. created_by must still be A.
    let archived = team_repo
        .set_status(team.id.uuid(), NodeStatus::Archived, &Stamp::new(clock.now(), actor_c.clone()))
        .await
        .expect("archive should succeed");
    assert_eq!(archived.node.created_by.as_ref(), Some(&actor_a), "set_status must not touch created_by");
    assert_eq!(archived.node.modified_by.as_ref(), Some(&actor_c), "set_status must restamp modified_by to the archiving actor");
}

/// SMA-440 FINDING 1 + FINDING 2 (final review): the project twin of
/// `organization_rename_and_archive_restamp_distinct_actors`, and — like the team test above —
/// also proves `pg_projects.rs`'s `created_by`/`modified_by` columns round-trip correctly and
/// are not transposed on read-back.
#[tokio::test]
async fn project_rename_and_archive_restamp_distinct_actors() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let ids = KernelIdGenerator;
    let clock = SystemClock;
    let org_repo = PgOrganizationRepository::new(db.clone(), Generations::memory());
    let team_repo = PgTeamRepository::new(db.clone(), Generations::memory());
    let project_repo = PgProjectRepository::new(db.clone(), Generations::memory());

    // The org and its team are created by an unrelated owner: this test is about the
    // PROJECT's own actor columns, not the org's or team's.
    let owner = ids.new_principal_id();
    let (org, default_team) = new_org_and_default_team(&ids, &clock, &owner, "acme", "Acme Corp.");
    let grant = support::pg_owner_grant(&db, &owner, ids.new_membership_id(), &org.id).await;
    org_repo.create(&org, &default_team, &grant, &Stamp::new(org.created_at, owner.clone())).await.unwrap();

    let team_id = ids.new_team_id(org.id.uuid());
    let team_stamp = Stamp::new(clock.now(), owner.clone());
    let team = Team::new(team_id, Slug::parse("eng").unwrap(), "Engineering", &team_stamp).unwrap();
    team_repo.create(&team, &team_stamp).await.unwrap();

    let actor_a = ids.new_principal_id();
    let actor_b = ids.new_principal_id();
    let actor_c = ids.new_principal_id();

    let project_id = ids.new_project_id(org.id.uuid());
    let create_stamp = Stamp::new(clock.now(), actor_a.clone());
    let project = Project::new(project_id, team.id.clone(), Slug::parse("web").unwrap(), "Web", &create_stamp).unwrap();
    project_repo.create(&project, &create_stamp).await.unwrap();

    // Rename stamped by actor B: created_by must stay A, modified_by must move to B.
    let renamed = project_repo
        .rename(project.id.uuid(), None, Some("Web Renamed"), &Stamp::new(clock.now(), actor_b.clone()))
        .await
        .expect("rename should succeed");
    assert_eq!(renamed.node.created_by.as_ref(), Some(&actor_a), "rename must not touch created_by");
    assert_eq!(renamed.node.modified_by.as_ref(), Some(&actor_b), "rename must restamp modified_by to the renaming actor");

    // set_status twin: archive stamped by actor C. created_by must still be A.
    let archived = project_repo
        .set_status(project.id.uuid(), NodeStatus::Archived, &Stamp::new(clock.now(), actor_c.clone()))
        .await
        .expect("archive should succeed");
    assert_eq!(archived.node.created_by.as_ref(), Some(&actor_a), "set_status must not touch created_by");
    assert_eq!(archived.node.modified_by.as_ref(), Some(&actor_c), "set_status must restamp modified_by to the archiving actor");
}
