// SPDX-License-Identifier: Apache-2.0

//! Tenancy `entity_gen` invalidation (SMA-444 Task 15b, spec §7/D11): a successful
//! `create`/`rename`/`set_status` on `PgOrganizationRepository`/`PgTeamRepository`/
//! `PgProjectRepository` bumps the shared `Generations`' `entity_gen` counter, so the
//! decision/entity-slice caches (keyed off it) see the change without any SCAN/DEL.
//! `tests/authz_entity_slice.rs` already proves the READ side (an archived ancestor's
//! `effective_status` flips on the very next `load`); this file proves the counter itself
//! actually moves. It also proves `policy_gen`'s independence — except for `create`'s own
//! `org_admin` owner grant (SMA-444 Task 20b, spec D8), itself a policy change, which bumps
//! `policy_gen` exactly once alongside the usual `entity_gen` bump.
//!
//! Runs against an ephemeral Postgres in Docker. In CI (`CI` env set) a missing Docker
//! daemon is a HARD FAILURE; on a Docker-less laptop the test skips (returns) with a note —
//! same gating pattern as `tests/roundtrip.rs`/`tests/authz_entity_slice.rs`.

mod support;

use paigasus_iam::adapters::authz::Generations;
use paigasus_iam::adapters::clock::SystemClock;
use paigasus_iam::adapters::id::KernelIdGenerator;
use paigasus_iam::adapters::persistence::{PgOrganizationRepository, PgProjectRepository, PgTeamRepository};
use paigasus_iam_core::{Clock, IdGenerator, NodeStatus, Organization, OrganizationRepository, PrincipalId, Project, ProjectRepository, Slug, Stamp, Team, TeamRepository};

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

/// Every successful org/team/project `create`/`rename`/`set_status` bumps the SAME shared
/// `entity_gen` counter — proven end to end by driving all three repos, sharing one
/// `Generations` handle, and asserting the counter strictly increases after each call.
#[tokio::test]
async fn tenancy_writes_bump_the_shared_entity_gen_across_org_team_project() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let gens = Generations::memory();
    let ids = KernelIdGenerator;
    let clock = SystemClock;
    let org_repo = PgOrganizationRepository::new(db.clone(), gens.clone());
    let team_repo = PgTeamRepository::new(db.clone(), gens.clone());
    let project_repo = PgProjectRepository::new(db.clone(), gens.clone());

    assert_eq!(gens.entity_gen().await.unwrap(), 0, "starts at zero");

    let owner = ids.new_principal_id();
    let (org, default_team) = new_org_and_default_team(&ids, &clock, &owner, "acme-gen", "Acme Gen");
    let grant = support::pg_owner_grant(&db, &owner, ids.new_membership_id(), &org.id).await;
    let create_stamp = Stamp::new(org.created_at, owner.clone());
    org_repo.create(&org, &default_team, &grant, &create_stamp).await.unwrap();
    assert_eq!(gens.entity_gen().await.unwrap(), 1, "org create must bump entity_gen");

    let team_id = ids.new_team_id(org.id.uuid());
    let team_stamp = Stamp::new(clock.now(), owner.clone());
    let team = Team::new(team_id, Slug::parse("eng-gen").unwrap(), "Engineering", &team_stamp).unwrap();
    team_repo.create(&team, &team_stamp).await.unwrap();
    assert_eq!(gens.entity_gen().await.unwrap(), 2, "team create must bump entity_gen");

    let project_id = ids.new_project_id(org.id.uuid());
    let project_stamp = Stamp::new(clock.now(), owner.clone());
    let project = Project::new(project_id, team.id.clone(), Slug::parse("web-gen").unwrap(), "Web", &project_stamp).unwrap();
    project_repo.create(&project, &project_stamp).await.unwrap();
    assert_eq!(gens.entity_gen().await.unwrap(), 3, "project create must bump entity_gen");

    org_repo.rename(org.id.uuid(), None, Some("Acme Gen Renamed"), &Stamp::new(clock.now(), owner.clone())).await.unwrap();
    assert_eq!(gens.entity_gen().await.unwrap(), 4, "org rename must bump entity_gen");

    team_repo
        .rename(team.id.uuid(), None, Some("Engineering Renamed"), &Stamp::new(clock.now(), owner.clone()))
        .await
        .unwrap();
    assert_eq!(gens.entity_gen().await.unwrap(), 5, "team rename must bump entity_gen");

    project_repo
        .rename(project.id.uuid(), None, Some("Web Renamed"), &Stamp::new(clock.now(), owner.clone()))
        .await
        .unwrap();
    assert_eq!(gens.entity_gen().await.unwrap(), 6, "project rename must bump entity_gen");

    // Project/team set_status first (D10: always permitted, no ancestor guard), org last —
    // avoids any ordering dependency on ancestor-archived preconditions above.
    project_repo.set_status(project.id.uuid(), NodeStatus::Archived, &Stamp::new(clock.now(), owner.clone())).await.unwrap();
    assert_eq!(gens.entity_gen().await.unwrap(), 7, "project set_status must bump entity_gen");

    team_repo.set_status(team.id.uuid(), NodeStatus::Archived, &Stamp::new(clock.now(), owner.clone())).await.unwrap();
    assert_eq!(gens.entity_gen().await.unwrap(), 8, "team set_status must bump entity_gen");

    org_repo.set_status(org.id.uuid(), NodeStatus::Archived, &Stamp::new(clock.now(), owner.clone())).await.unwrap();
    assert_eq!(gens.entity_gen().await.unwrap(), 9, "org set_status must bump entity_gen");
}

/// `entity_gen` is independent of `policy_gen` (a completely separate counter under the
/// shared `Generations` handle, spec §7/D11): a pure tenancy write (no grant involved, e.g.
/// `set_status`) must never move `policy_gen`. `create` is the one exception — its
/// owner-grant side effect (SMA-444 Task 20b, spec D8) is itself a policy change, so it
/// bumps `policy_gen` exactly once, same as `PgRoleGrantStore::grant` would.
#[tokio::test]
async fn only_creates_owner_grant_bumps_policy_gen_not_plain_tenancy_writes() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let gens = Generations::memory();
    let ids = KernelIdGenerator;
    let clock = SystemClock;
    let org_repo = PgOrganizationRepository::new(db.clone(), gens.clone());
    let owner = ids.new_principal_id();

    assert_eq!(gens.policy_gen().await.unwrap(), 0);

    let (org, default_team) = new_org_and_default_team(&ids, &clock, &owner, "acme-policy-gen", "Acme Policy Gen");
    let grant = support::pg_owner_grant(&db, &owner, ids.new_membership_id(), &org.id).await;
    let create_stamp = Stamp::new(org.created_at, owner.clone());
    org_repo.create(&org, &default_team, &grant, &create_stamp).await.unwrap();
    assert_eq!(gens.policy_gen().await.unwrap(), 1, "create's owner grant must bump policy_gen exactly once");

    org_repo.set_status(org.id.uuid(), NodeStatus::Archived, &Stamp::new(clock.now(), owner.clone())).await.unwrap();

    assert_eq!(gens.policy_gen().await.unwrap(), 1, "a plain tenancy write (set_status) must never bump policy_gen further");
    assert_eq!(gens.entity_gen().await.unwrap(), 2, "sanity: entity_gen did move for the same two writes");
}
