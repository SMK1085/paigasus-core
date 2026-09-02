// SPDX-License-Identifier: Apache-2.0

//! Postgres-tier coverage for the SMA-606 tenancy audit trail (Task 10): the two invariants
//! the in-memory fakes structurally cannot prove.
//!
//! 1. **Atomicity**
//!    (`org_create_commits_organization_outbox_and_audit_atomically_and_rolls_back_together`):
//!    a real `PgOrganizationRepository::create_in` write, plus its outbox/audit rows, on one
//!    `SeaOrmUnitOfWork`-scoped transaction — committed, all three are visible and share one
//!    `correlation_id`; dropped without commit, none of the three is. Modeled directly on
//!    `tests/outbox_uow_pg.rs`'s commit/rollback pair.
//!
//! 2. **The cascade row-count control**
//!    (`a_cascading_detach_writes_exactly_one_audit_row_per_deleted_membership`): `detach_in`
//!    (`adapters/persistence/pg_memberships.rs`) runs a `FOR UPDATE` lock, a PRN-joining
//!    projection, and the cascade delete as three SEPARATE statements that must describe the
//!    SAME row set — only Postgres can prove they agree, because the in-memory fake cascades on
//!    a different key entirely (the caller's PRN-embedded org slot, not the stored `org_id`
//!    `detach_in` resolves by subquery), so it can prove fan-out but never agreement.
//!
//! 3. **The concurrency control**
//!    (`a_concurrent_detach_of_a_cascade_row_does_not_make_this_call_over_report`): without
//!    `DETACH_LOCK_SQL`'s `FOR UPDATE` step, a concurrent transaction detaching one of the
//!    cascade rows is not blocked by the pre-existing `lock_exclusive()` on the target row
//!    alone, so under READ COMMITTED the projection can see a row a peer's commit then removes
//!    out from under the later delete — this call would then report a detach it never
//!    performed. Proven by racing a peer transaction against one cascade row and asserting this
//!    call's own report stays correct regardless.
//!
//! 4. **The ancestor-lock control on `set_status_in`**
//!    (`a_concurrent_org_archive_is_reflected_in_a_racing_team_set_status_event`): PR 203
//!    review finding 1. `pg_teams.rs`'s `set_status_in` fetches the org ancestor purely to
//!    compute the returned `effective_status` — and since SMA-606 that value ships on the
//!    outbox event and the audit entry, an unlocked read can ship a value the org's own
//!    concurrent archive/restore has already invalidated by commit time. A peer archives the
//!    org and holds it uncommitted while a racing `TeamService::restore` call (whose own row
//!    change carries no "archived" information at all — it flips the team from `Archived` back
//!    to `Active`) blocks on the org row until the peer commits, then must emit
//!    `effective_status: "archived"`, not the stale `"active"`.
//!
//! Runs against an ephemeral Postgres in Docker. In CI (`CI` env set) a missing Docker daemon is
//! a HARD FAILURE; on a Docker-less laptop each test skips (returns) with a note — same gating
//! pattern as every other `tests/*_pg.rs` file (`tests/support/mod.rs`, SMA-538).

mod support;

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use paigasus_iam::adapters::authz::{Generations, GenerationsEntityGenBumper};
use paigasus_iam::adapters::clock::SystemClock;
use paigasus_iam::adapters::id::KernelIdGenerator;
use paigasus_iam::adapters::persistence::entities::{audit_log, event_outbox, membership, organization};
use paigasus_iam::adapters::persistence::{PgAuditLog, PgMembershipRepository, PgOrganizationRepository, PgOutbox, PgPrincipalRepository, PgProjectRepository, PgTeamRepository, SeaOrmUnitOfWork};
use paigasus_iam::application::memberships::{MembershipService, MembershipServiceDeps};
use paigasus_iam::application::teams::{TeamService, TeamServiceDeps};
use paigasus_iam_core::{
    Action, AuditEntry, AuditLog, AuditOutcome, Clock, DomainEvent, Email, EventType, IdGenerator, Membership, MembershipRepository, NodeStatus, Organization, OrganizationRepository, Outbox,
    Principal, PrincipalId, PrincipalKind, PrincipalRepository, PrincipalStatus, Project, ProjectRepository, RepositoryError, Slug, Stamp, Team, TeamRepository, TenancyNodeRef, UnitOfWork, User,
};
use paigasus_kernel::{Prn, mint_uuid7};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter};

/// Seeds a single user-principal (mirrors `tests/tenancy_memberships.rs::seed_user`, duplicated
/// here per this crate's established one-copy-per-test-binary convention — see
/// `tests/support/mod.rs`'s module docs).
async fn seed_user(db: &DatabaseConnection, seed: u8) -> PrincipalId {
    let uuid = mint_uuid7(1_700_000_000_000 + u64::from(seed), [seed; 10]);
    let id = PrincipalId::from_prn(Prn::build("iam", "", None, "principal", uuid).unwrap());
    let now = SystemClock.now();
    let principal = Principal::new(id.clone(), PrincipalKind::User, PrincipalStatus::Active, now, now);
    let user = User::new(id.clone(), Email::parse(&format!("user{seed}@example.com")).unwrap(), format!("User {seed}"), None, None, now, now);
    PgPrincipalRepository::new(db.clone()).create_user(&principal, &user).await.unwrap();
    id
}

/// Builds a `Membership` domain value stamped by a freshly minted, unseeded actor (mirrors
/// `tests/tenancy_memberships.rs::membership_at`) — none of these tests assert on `created_by`.
fn membership_at(ids: &KernelIdGenerator, principal_id: &PrincipalId, node: TenancyNodeRef, when: DateTime<Utc>) -> Membership {
    let stamp = Stamp::new(when, ids.new_principal_id());
    Membership::new(ids.new_membership_id(), principal_id.clone(), node, &stamp)
}

/// Rebuilds the `Stamp` a `membership_at`-built `Membership` already carries, for `repo.attach`'s
/// own `stamp` argument (mirrors `tests/tenancy_memberships.rs::stamp_of`).
fn stamp_of(m: &Membership) -> Stamp {
    Stamp::new(m.created_at, m.created_by.clone().expect("membership_at always stamps a creator"))
}

/// Seeds an org (with its auto-provisioned default team and an `org_admin` owner grant, ADR-0014)
/// plus one additional, non-default team under it.
async fn seed_org_and_team(db: &DatabaseConnection, slug: &str) -> (Organization, Team) {
    let ids = KernelIdGenerator;
    let clock = SystemClock;
    let org_repo = PgOrganizationRepository::new(db.clone(), Generations::memory());
    let team_repo = PgTeamRepository::new(db.clone(), Generations::memory());

    let owner = ids.new_principal_id();
    let stamp = Stamp::new(clock.now(), owner.clone());
    let org_id = ids.new_organization_id();
    let org = Organization::new(org_id, Slug::parse(slug).unwrap(), slug, &stamp).unwrap();
    let default_team_id = ids.new_team_id(org.id.uuid());
    let default_team = Team::new(default_team_id, Slug::parse("default").unwrap(), "Default", &stamp).unwrap();
    let grant = support::pg_owner_grant(db, &owner, ids.new_membership_id(), &org.id).await;
    org_repo.create(&org, &default_team, &grant, &stamp).await.unwrap();

    let team_id = ids.new_team_id(org.id.uuid());
    let team_stamp = Stamp::new(clock.now(), owner.clone());
    let team = Team::new(team_id, Slug::parse("eng").unwrap(), "Engineering", &team_stamp).unwrap();
    team_repo.create(&team, &team_stamp).await.unwrap();

    (org, team)
}

/// Seeds an org (default team + owner grant) plus TWO additional teams and one project under
/// each — the fixture the cascade row-count test needs: a principal attached to the org, both
/// teams and both projects must lose exactly those five memberships on an org-level detach.
async fn seed_cascade_chain(db: &DatabaseConnection) -> (Organization, Team, Team, Project, Project) {
    let ids = KernelIdGenerator;
    let clock = SystemClock;
    let (org, team1) = seed_org_and_team(db, "cascade-co").await;
    let team_repo = PgTeamRepository::new(db.clone(), Generations::memory());
    let project_repo = PgProjectRepository::new(db.clone(), Generations::memory());

    let owner = ids.new_principal_id();
    let team2_id = ids.new_team_id(org.id.uuid());
    let team2_stamp = Stamp::new(clock.now(), owner.clone());
    let team2 = Team::new(team2_id, Slug::parse("sales").unwrap(), "Sales", &team2_stamp).unwrap();
    team_repo.create(&team2, &team2_stamp).await.unwrap();

    let project1_id = ids.new_project_id(org.id.uuid());
    let project1_stamp = Stamp::new(clock.now(), owner.clone());
    let project1 = Project::new(project1_id, team1.id.clone(), Slug::parse("web").unwrap(), "Web", &project1_stamp).unwrap();
    project_repo.create(&project1, &project1_stamp).await.unwrap();

    let project2_id = ids.new_project_id(org.id.uuid());
    let project2_stamp = Stamp::new(clock.now(), owner.clone());
    let project2 = Project::new(project2_id, team2.id.clone(), Slug::parse("mobile").unwrap(), "Mobile", &project2_stamp).unwrap();
    project_repo.create(&project2, &project2_stamp).await.unwrap();

    (org, team1, team2, project1, project2)
}

/// Scenario 1 — atomicity: a committed org create makes the `organization` row, an
/// `event_outbox` row and an `audit_log` row all visible, sharing ONE correlation id; a
/// transaction dropped without commit leaves none of the three. Mirrors `tests/outbox_uow_pg.rs`
/// scenarios 1/2, but through `PgOrganizationRepository::create_in` (SMA-606 D2) rather than a
/// raw `enqueue`/`record`.
#[tokio::test]
async fn org_create_commits_organization_outbox_and_audit_atomically_and_rolls_back_together() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let uow = SeaOrmUnitOfWork::new(db.clone());
    let repo = PgOrganizationRepository::new(db.clone(), Generations::memory());
    let outbox = PgOutbox::new(true);
    let audit = PgAuditLog::new(db.clone());
    let ids = KernelIdGenerator;
    let clock = SystemClock;

    // --- commit path ---
    let owner = ids.new_principal_id();
    let stamp = Stamp::new(clock.now(), owner.clone());
    let org_id = ids.new_organization_id();
    let org = Organization::new(org_id, Slug::parse("atomic-co").unwrap(), "Atomic Co.", &stamp).unwrap();
    let team_id = ids.new_team_id(org.id.uuid());
    let default_team = Team::new(team_id, Slug::parse("default").unwrap(), "Default", &stamp).unwrap();
    let grant = support::pg_owner_grant(&db, &owner, ids.new_membership_id(), &org.id).await;

    let corr = ids.new_correlation_id();
    let ev = DomainEvent {
        id: ids.new_event_id(),
        event_type: EventType::OrganizationCreated,
        schema_version: 1,
        aggregate_prn: org.id.canonical(),
        actor_prn: Some(owner.canonical()),
        occurred_at: stamp.at,
        payload: serde_json::json!({"slug": "atomic-co"}),
        correlation_id: Some(corr),
    };
    let entry = AuditEntry {
        id: ids.new_audit_id(),
        occurred_at: stamp.at,
        actor_prn: Some(owner.canonical()),
        action: Action::CreateOrganization.as_wire().to_string(),
        resource_prn: Some(org.id.canonical()),
        outcome: AuditOutcome::Committed,
        determining_policies: Vec::new(),
        detail: serde_json::json!({}),
        correlation_id: Some(corr),
    };

    let tx = uow.begin().await.expect("begin");
    repo.create_in(&*tx, &org, &default_team, &grant, &stamp).await.expect("create_in");
    outbox.enqueue(&*tx, &ev).await.expect("enqueue outbox row");
    audit.record(&*tx, &entry).await.expect("record audit row");
    tx.commit().await.expect("commit");

    let org_row = organization::Entity::find_by_id(org.id.uuid()).one(&db).await.unwrap().expect("committed org row must be visible");
    assert_eq!(org_row.prn, org.id.canonical());

    let outbox_row = event_outbox::Entity::find()
        .filter(event_outbox::Column::CorrelationId.eq(corr))
        .one(&db)
        .await
        .unwrap()
        .expect("committed outbox row must be visible");
    let audit_row = audit_log::Entity::find()
        .filter(audit_log::Column::CorrelationId.eq(corr))
        .one(&db)
        .await
        .unwrap()
        .expect("committed audit row must be visible");
    assert_eq!(outbox_row.correlation_id, Some(corr));
    assert_eq!(audit_row.correlation_id, Some(corr), "the outbox and audit rows share the correlation id of the committed org create");

    // --- rollback path: a second, distinct org create, txn dropped without commit ---
    let owner2 = ids.new_principal_id();
    let stamp2 = Stamp::new(clock.now(), owner2.clone());
    let org2_id = ids.new_organization_id();
    let org2 = Organization::new(org2_id, Slug::parse("rollback-co").unwrap(), "Rollback Co.", &stamp2).unwrap();
    let team2_id = ids.new_team_id(org2.id.uuid());
    let default_team2 = Team::new(team2_id, Slug::parse("default").unwrap(), "Default", &stamp2).unwrap();
    let grant2 = support::pg_owner_grant(&db, &owner2, ids.new_membership_id(), &org2.id).await;

    let corr2 = ids.new_correlation_id();
    let ev2 = DomainEvent {
        id: ids.new_event_id(),
        event_type: EventType::OrganizationCreated,
        schema_version: 1,
        aggregate_prn: org2.id.canonical(),
        actor_prn: Some(owner2.canonical()),
        occurred_at: stamp2.at,
        payload: serde_json::json!({"slug": "rollback-co"}),
        correlation_id: Some(corr2),
    };
    let entry2 = AuditEntry {
        id: ids.new_audit_id(),
        occurred_at: stamp2.at,
        actor_prn: Some(owner2.canonical()),
        action: Action::CreateOrganization.as_wire().to_string(),
        resource_prn: Some(org2.id.canonical()),
        outcome: AuditOutcome::Committed,
        determining_policies: Vec::new(),
        detail: serde_json::json!({}),
        correlation_id: Some(corr2),
    };

    let tx2 = uow.begin().await.expect("begin");
    repo.create_in(&*tx2, &org2, &default_team2, &grant2, &stamp2).await.expect("create_in");
    outbox.enqueue(&*tx2, &ev2).await.expect("enqueue outbox row");
    audit.record(&*tx2, &entry2).await.expect("record audit row");
    drop(tx2); // no commit -> rollback

    assert!(
        organization::Entity::find_by_id(org2.id.uuid()).one(&db).await.unwrap().is_none(),
        "an uncommitted org create must never be visible"
    );
    assert!(
        event_outbox::Entity::find().filter(event_outbox::Column::CorrelationId.eq(corr2)).one(&db).await.unwrap().is_none(),
        "an uncommitted txn's outbox write must never be visible"
    );
    assert!(
        audit_log::Entity::find().filter(audit_log::Column::CorrelationId.eq(corr2)).one(&db).await.unwrap().is_none(),
        "an uncommitted txn's audit write must never be visible"
    );
}

/// Scenario 2 — SMA-606 D6, Testing case 11. THE control for Risk 3: the projection SELECT and
/// the cascade DELETE are two statements that must describe the same row set, and only Postgres
/// can prove they do — the fake implements the cascade on a different key entirely (the
/// caller's PRN, not the stored org_id), so it can prove fan-out but never agreement.
#[tokio::test]
async fn a_cascading_detach_writes_exactly_one_audit_row_per_deleted_membership() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let ids = KernelIdGenerator;
    let clock = SystemClock;
    let (org, team1, team2, project1, project2) = seed_cascade_chain(&db).await;
    let other_org = support::seed_org_ref(&db).await;
    let principal = seed_user(&db, 60).await;
    let membership_repo = PgMembershipRepository::new(db.clone());

    let org_membership = {
        let m = membership_at(&ids, &principal, TenancyNodeRef::Organization(org.id.clone()), clock.now());
        membership_repo.attach(&m, &stamp_of(&m)).await.unwrap()
    };
    for team in [&team1, &team2] {
        let m = membership_at(&ids, &principal, TenancyNodeRef::Team(team.id.clone()), clock.now());
        membership_repo.attach(&m, &stamp_of(&m)).await.unwrap();
    }
    for project in [&project1, &project2] {
        let m = membership_at(&ids, &principal, TenancyNodeRef::Project(project.id.clone()), clock.now());
        membership_repo.attach(&m, &stamp_of(&m)).await.unwrap();
    }
    // A membership in a DIFFERENT org, which must survive the org detach untouched.
    let other_membership = {
        let m = membership_at(&ids, &principal, other_org.clone(), clock.now());
        membership_repo.attach(&m, &stamp_of(&m)).await.unwrap()
    };

    let before = membership::Entity::find().filter(membership::Column::PrincipalId.eq(principal.uuid())).count(&db).await.unwrap();
    assert_eq!(before, 6, "sanity: org + 2 teams + 2 projects + the other-org membership");

    let svc = MembershipService::new(MembershipServiceDeps {
        repo: membership_repo,
        uow: Arc::new(SeaOrmUnitOfWork::new(db.clone())),
        outbox: Arc::new(PgOutbox::new(true)),
        audit: Arc::new(PgAuditLog::new(db.clone())),
        ids: KernelIdGenerator,
        clock: SystemClock,
    });
    let actor = ids.new_principal_id();

    svc.detach(org_membership.id, &actor).await.expect("cascading detach should succeed");

    let remaining = membership::Entity::find().filter(membership::Column::PrincipalId.eq(principal.uuid())).all(&db).await.unwrap();
    assert_eq!(remaining.len(), 1, "only the other-org membership should survive the cascade");
    assert_eq!(remaining[0].id, other_membership.id, "the surviving row must be the untouched other-org membership, byte for byte");

    let after = remaining.len() as u64;
    let deleted_count = before - after;
    assert_eq!(deleted_count, 5, "the org row plus its four cascaded team/project rows");

    let audit_rows = audit_log::Entity::find()
        .filter(audit_log::Column::Action.eq(Action::DetachMembership.as_wire()))
        .all(&db)
        .await
        .unwrap();
    let audit_row_count = audit_rows.len() as u64;
    let event_row_count = event_outbox::Entity::find()
        .filter(event_outbox::Column::EventType.eq(EventType::MembershipDetached.as_wire()))
        .count(&db)
        .await
        .unwrap();

    assert_eq!(audit_row_count, deleted_count, "the audit row count must equal the deleted row count, not merely be nonzero");
    assert_eq!(event_row_count, deleted_count, "the outbox row count must equal the deleted row count too");

    // Cardinality agreement alone would still pass a projection that reported one node twice
    // and omitted another — the audit rows must name exactly the five deleted nodes, not merely
    // five of them. `AuditEntry.resource_prn` is `record.node_prn` (`application/
    // memberships.rs`'s detach loop), so this is an identity check on the same field the row
    // count above only counted.
    let audit_resource_prns: std::collections::HashSet<String> = audit_rows.into_iter().filter_map(|r| r.resource_prn).collect();
    let expected_prns: std::collections::HashSet<String> = [org.id.canonical(), team1.id.canonical(), team2.id.canonical(), project1.id.canonical(), project2.id.canonical()]
        .into_iter()
        .collect();
    assert_eq!(audit_resource_prns, expected_prns, "the audit rows must name exactly the five deleted nodes, not merely five of them");
}

/// Scenario 3 — SMA-606 D6 step 1, Testing case 12. Without the `FOR UPDATE` lock the
/// projection can see a cascade row that a concurrent detach then removes first, and this call
/// reports a detach it never performed. The peer detaches a cascade row and holds it
/// uncommitted; this call blocks at its `FOR UPDATE` step until the peer commits, then projects
/// and deletes post-commit truth.
///
/// **This test was verified to fail with `FOR UPDATE` removed from `DETACH_LOCK_SQL`** — see
/// the Task 10 report for the exact output; the removal was reverted before this commit.
#[tokio::test]
async fn a_concurrent_detach_of_a_cascade_row_does_not_make_this_call_over_report() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let ids = KernelIdGenerator;
    let clock = SystemClock;
    let (org, team) = seed_org_and_team(&db, "race-co").await;
    let principal = seed_user(&db, 70).await;
    let repo = PgMembershipRepository::new(db.clone());

    let org_membership = {
        let m = membership_at(&ids, &principal, TenancyNodeRef::Organization(org.id.clone()), clock.now());
        repo.attach(&m, &stamp_of(&m)).await.unwrap()
    };
    let team_membership = {
        let m = membership_at(&ids, &principal, TenancyNodeRef::Team(team.id.clone()), clock.now());
        repo.attach(&m, &stamp_of(&m)).await.unwrap()
    };

    // The peer: begins, detaches the team (cascade) membership directly, but does NOT commit
    // yet — its uncommitted delete holds the row lock this call's own cascade must respect.
    let peer_uow = SeaOrmUnitOfWork::new(db.clone());
    let peer_tx = peer_uow.begin().await.unwrap();
    let peer_deleted = repo.detach_in(&*peer_tx, team_membership.id).await.unwrap();
    assert_eq!(peer_deleted.len(), 1, "sanity: the peer's own single-row detach should touch only itself");

    // This call: spawned, since — with the lock fix in place — it must BLOCK on the peer's
    // held row lock until the peer commits; awaiting it inline here would deadlock the test.
    let repo1 = repo.clone();
    let db1 = db.clone();
    let target_id = org_membership.id;
    let handle = tokio::spawn(async move {
        let uow1 = SeaOrmUnitOfWork::new(db1);
        let tx1 = uow1.begin().await?;
        let deleted = repo1.detach_in(&*tx1, target_id).await?;
        tx1.commit().await?;
        Ok::<_, RepositoryError>(deleted)
    });

    // Give the spawned call real wall-clock time to reach (and block on) its own conflicting
    // statement against the peer's uncommitted row — it cannot finish before the peer releases
    // that lock, so this is a generous budget, not a fragile one: on a warm connection the block
    // happens in well under this window.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!handle.is_finished(), "sanity: this call must still be blocked on the peer's uncommitted delete");

    peer_tx.commit().await.expect("peer commit");

    let deleted = handle
        .await
        .expect("this call's task must not panic")
        .expect("detach_in must still succeed once the peer releases its lock");
    assert_eq!(
        deleted.len(),
        1,
        "must report only the org row it actually deleted, not the cascade row the peer already removed: {deleted:?}"
    );
    assert_eq!(deleted[0].id, org_membership.id);
    assert!(
        !deleted.iter().any(|r| r.id == team_membership.id),
        "the peer's already-committed detach of the cascade row must not be double-reported here"
    );

    let remaining = membership::Entity::find().filter(membership::Column::PrincipalId.eq(principal.uuid())).count(&db).await.unwrap();
    assert_eq!(remaining, 0, "both rows must be gone in the end, whichever call actually deleted the cascade one");
}

/// Scenario 4 — PR 203 review finding 1: `pg_teams.rs::set_status_in` must lock its org
/// ancestor read, not merely fetch it, because since SMA-606 the derived `effective_status`
/// it computes ships on the outbox event and the audit entry — a value knowingly stale under
/// READ COMMITTED would then be the audit trail's own recorded truth, not merely a stale
/// return value.
///
/// The peer archives the ORG and holds it uncommitted. The racing call is `TeamService::restore`
/// on a team that was PRE-archived (uncontested, before the peer starts): flipping the team's
/// own status `Archived` -> `Active` carries no "archived" information of its own, so any
/// "archived" surfacing in the racing call's emitted payload can only have come from the org
/// ancestor. With `.lock_shared()` in place, the racing call blocks on the org row until the
/// peer commits, then reads the post-commit `Archived` org and must emit
/// `effective_status: "archived"` — not the stale `"active"` an unlocked read could see while
/// racing ahead of the peer's commit.
///
/// **This test was verified to FAIL with `.lock_shared()` removed from
/// `pg_teams.rs::set_status_in`'s ancestor read** — see the PR 203 round-1 report for the exact
/// output; the removal was reverted before this commit.
#[tokio::test]
async fn a_concurrent_org_archive_is_reflected_in_a_racing_team_set_status_event() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let ids = KernelIdGenerator;
    let clock = SystemClock;
    let (org, team) = seed_org_and_team(&db, "race-status-co").await;
    let actor = ids.new_principal_id();

    let gens = Generations::memory();
    let svc = TeamService::new(TeamServiceDeps {
        repo: PgTeamRepository::new(db.clone(), gens.clone()),
        uow: Arc::new(SeaOrmUnitOfWork::new(db.clone())),
        outbox: Arc::new(PgOutbox::new(true)),
        audit: Arc::new(PgAuditLog::new(db.clone())),
        gen_bumper: Arc::new(GenerationsEntityGenBumper::new(gens)),
        ids,
        clock,
    });

    // Put the team into Archived first, uncontested — so the racing call's own `restore` below
    // is a real status CHANGE (Archived -> Active), isolating the org ancestor as the ONLY
    // possible source of "archived" in the emitted payload.
    svc.archive(team.id.uuid(), &actor).await.expect("uncontested pre-archive");

    // The peer: begins, archives the ORG directly, but does NOT commit yet — its uncommitted
    // update holds the org row's exclusive lock this call's own ancestor read must respect.
    let org_repo = PgOrganizationRepository::new(db.clone(), Generations::memory());
    let peer_uow = SeaOrmUnitOfWork::new(db.clone());
    let peer_tx = peer_uow.begin().await.unwrap();
    let org_stamp = Stamp::new(clock.now(), actor.clone());
    let peer_out = org_repo.set_status_in(&*peer_tx, org.id.uuid(), NodeStatus::Archived, &org_stamp).await.unwrap();
    assert!(peer_out.changed, "sanity: the peer's own org archive must be a real change");

    // This call: spawned, since — with the lock fix in place — it must BLOCK on the peer's
    // held org-row lock until the peer commits; awaiting it inline here would deadlock the test.
    let team_id = team.id.uuid();
    let actor2 = actor.clone();
    let handle = tokio::spawn(async move { svc.restore(team_id, &actor2).await });

    // Give the spawned call real wall-clock time to reach (and block on) its own conflicting
    // ancestor read against the peer's uncommitted org update — a generous budget, not a
    // fragile one: on a warm connection the block happens in well under this window.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!handle.is_finished(), "sanity: this call must still be blocked on the peer's uncommitted org archive");

    peer_tx.commit().await.expect("peer commit");

    let restored = handle
        .await
        .expect("this call's task must not panic")
        .expect("restore must still succeed once the peer releases its lock");
    assert_eq!(restored.node.status, NodeStatus::Active, "the team's own status is the only thing this call actually changed");
    assert_eq!(
        restored.effective_status,
        NodeStatus::Archived,
        "the effective status must reflect the org's post-commit archive, not a stale pre-commit read"
    );

    let event_row = event_outbox::Entity::find()
        .filter(event_outbox::Column::AggregatePrn.eq(team.id.canonical()))
        .filter(event_outbox::Column::EventType.eq(EventType::TeamRestored.as_wire()))
        .one(&db)
        .await
        .unwrap()
        .expect("the restore must have enqueued exactly one outbox event");
    let event_payload: serde_json::Value = serde_json::from_str(&event_row.payload).unwrap();
    assert_eq!(
        event_payload["effective_status"], "archived",
        "the emitted event's payload must carry the post-commit truth, not the stale 'active': {event_payload}"
    );

    let audit_row = audit_log::Entity::find()
        .filter(audit_log::Column::ResourcePrn.eq(team.id.canonical()))
        .filter(audit_log::Column::Action.eq(Action::RestoreTeam.as_wire()))
        .one(&db)
        .await
        .unwrap()
        .expect("the restore must have recorded exactly one audit entry");
    let audit_detail: serde_json::Value = serde_json::from_str(&audit_row.detail).unwrap();
    assert_eq!(
        audit_detail["effective_status"], "archived",
        "the audit entry's detail must carry the post-commit truth too: {audit_detail}"
    );
}
