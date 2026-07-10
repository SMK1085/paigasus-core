// SPDX-License-Identifier: Apache-2.0

//! `PgOrganizationRepository` — end-to-end coverage against real Postgres: the
//! create-is-transactional invariant (org + auto-provisioned default team, ADR-0014), the
//! rename/lifecycle guard contracts (D8/D10), and `list`'s `ORDER BY created_at, id`.
//!
//! Runs against an ephemeral Postgres in Docker. In CI (`CI` env set) a missing Docker
//! daemon is a HARD FAILURE; on a Docker-less laptop the test skips (returns) with a note.

mod support;

use paigasus_iam::adapters::authz::Generations;
use paigasus_iam::adapters::clock::SystemClock;
use paigasus_iam::adapters::id::KernelIdGenerator;
use paigasus_iam::adapters::persistence::PgOrganizationRepository;
use paigasus_iam::adapters::persistence::entities::team;
use paigasus_iam_core::{Clock, ConflictKind, IdGenerator, NodeStatus, Organization, OrganizationRepository, PreconditionKind, RepositoryError, Slug, Team};
use sea_orm::EntityTrait;
use uuid::Uuid;

/// Builds an `Organization` + its auto-provisioned `"default"` `Team`, minted via the same
/// `KernelIdGenerator`/`SystemClock` adapters the composition root will eventually wire in
/// (M0's `roundtrip.rs` precedent for building real, non-fake domain values).
fn new_org_and_default_team(ids: &KernelIdGenerator, clock: &SystemClock, slug: &str, name: &str) -> (Organization, Team) {
    let now = clock.now();
    let org_id = ids.new_organization_id();
    let org = Organization::new(org_id, Slug::parse(slug).unwrap(), name, now).unwrap();
    let team_id = ids.new_team_id(org.id.uuid());
    let default_team = Team::new(team_id, Slug::parse("default").unwrap(), "Default", now).unwrap();
    (org, default_team)
}

#[tokio::test]
async fn create_is_transactional_and_provisions_default_team() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let ids = KernelIdGenerator;
    let clock = SystemClock;
    let repo = PgOrganizationRepository::new(db.clone(), Generations::memory());

    let (org, default_team) = new_org_and_default_team(&ids, &clock, "acme", "Acme Corp.");
    repo.create(&org, &default_team).await.unwrap();

    // Both rows exist: the org row via the repo's own `find`, the team row via a direct
    // entity query (the repo has no team-lookup method yet — that's Task 11).
    let found = repo.find(org.id.uuid()).await.unwrap().expect("org row present");
    assert_eq!(found.node, org);
    assert_eq!(found.effective_status, NodeStatus::Active);

    let team_row = team::Entity::find_by_id(default_team.id.uuid()).one(&db).await.unwrap();
    assert!(team_row.is_some(), "default team row missing");

    // A second org with the same slug must fail atomically: Conflict(SlugTaken), AND no
    // orphan team row is left behind for the failed org's auto-provisioned default team.
    let (org2, default_team2) = new_org_and_default_team(&ids, &clock, "acme", "Acme Duplicate");
    let result = repo.create(&org2, &default_team2).await;
    assert!(
        matches!(result, Err(RepositoryError::Conflict(ConflictKind::SlugTaken))),
        "expected Conflict(SlugTaken), got {result:?}"
    );

    let orphan_count = team::Entity::find().all(&db).await.unwrap().into_iter().filter(|t| t.org_id == org2.id.uuid()).count();
    assert_eq!(orphan_count, 0, "no orphan team row for the failed org id");
}

#[tokio::test]
async fn rename_and_lifecycle_contracts() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let ids = KernelIdGenerator;
    let clock = SystemClock;
    let repo = PgOrganizationRepository::new(db.clone(), Generations::memory());

    // rename missing id -> NotFound.
    let missing = repo.rename(Uuid::from_u128(999), None, Some("x"), clock.now()).await;
    assert!(matches!(missing, Err(RepositoryError::NotFound)), "expected NotFound, got {missing:?}");

    let (org, default_team) = new_org_and_default_team(&ids, &clock, "acme", "Acme Corp.");
    repo.create(&org, &default_team).await.unwrap();
    let id = org.id.uuid();
    let created_at = org.updated_at;

    // archive -> Archived + updated_at advanced.
    let archived = repo.set_status(id, NodeStatus::Archived, clock.now()).await.unwrap();
    assert_eq!(archived.node.status, NodeStatus::Archived);
    assert_eq!(archived.effective_status, NodeStatus::Archived);
    assert!(archived.node.updated_at > created_at, "updated_at must advance on archive");
    let archived_at = archived.node.updated_at;

    // archive again -> no-op (updated_at unchanged).
    let archived_again = repo.set_status(id, NodeStatus::Archived, clock.now()).await.unwrap();
    assert_eq!(archived_again.node.status, NodeStatus::Archived);
    assert_eq!(archived_again.node.updated_at, archived_at, "re-archiving must be a no-op");

    // rename archived -> Precondition(NodeArchived).
    let rename_archived = repo.rename(id, Some(&Slug::parse("acme2").unwrap()), None, clock.now()).await;
    assert!(
        matches!(rename_archived, Err(RepositoryError::Precondition(PreconditionKind::NodeArchived))),
        "expected Precondition(NodeArchived), got {rename_archived:?}"
    );

    // restore -> Active.
    let restored = repo.set_status(id, NodeStatus::Active, clock.now()).await.unwrap();
    assert_eq!(restored.node.status, NodeStatus::Active);
    assert_eq!(restored.effective_status, NodeStatus::Active);
    assert!(restored.node.updated_at > archived_at, "updated_at must advance on restore");

    // rename slug to another org's slug -> Conflict(SlugTaken).
    let (other_org, other_default_team) = new_org_and_default_team(&ids, &clock, "other", "Other Corp.");
    repo.create(&other_org, &other_default_team).await.unwrap();

    let slug_conflict = repo.rename(id, Some(&Slug::parse("other").unwrap()), None, clock.now()).await;
    assert!(
        matches!(slug_conflict, Err(RepositoryError::Conflict(ConflictKind::SlugTaken))),
        "expected Conflict(SlugTaken), got {slug_conflict:?}"
    );

    // A legitimate rename succeeds and updates both slug and name.
    let renamed = repo.rename(id, Some(&Slug::parse("acme-renamed").unwrap()), Some("Acme Renamed"), clock.now()).await.unwrap();
    assert_eq!(renamed.node.slug.as_str(), "acme-renamed");
    assert_eq!(renamed.node.name, "Acme Renamed");
}

#[tokio::test]
async fn list_orders_by_created_at_then_id() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let ids = KernelIdGenerator;
    let clock = SystemClock;
    let repo = PgOrganizationRepository::new(db.clone(), Generations::memory());

    // Three orgs, minted (and inserted) one at a time so `SystemClock`'s µs-truncated
    // `now()` calls land on distinct instants (real round-trips to Postgres between
    // inserts make a tie astronomically unlikely — no artificial sleep needed).
    let mut created = Vec::new();
    for (slug, name) in [("alpha", "Alpha"), ("beta", "Beta"), ("gamma", "Gamma")] {
        let (org, default_team) = new_org_and_default_team(&ids, &clock, slug, name);
        repo.create(&org, &default_team).await.unwrap();
        created.push(org);
    }

    let mut expected = created.clone();
    expected.sort_by_key(|o| (o.created_at, o.id.uuid()));
    // Sanity: the three creation instants really are distinct, else this test would pass
    // vacuously regardless of whether `list` sorts correctly.
    assert_eq!(
        expected.iter().map(|o| o.created_at).collect::<std::collections::BTreeSet<_>>().len(),
        3,
        "created_at values were not distinct"
    );

    let page1 = repo.list(2, 0).await.unwrap();
    let page2 = repo.list(2, 2).await.unwrap();
    assert_eq!(page1.len(), 2);
    assert_eq!(page2.len(), 1);

    let mut got: Vec<Organization> = page1.into_iter().map(|v| v.node).collect();
    got.extend(page2.into_iter().map(|v| v.node));
    assert_eq!(got, expected);
}
