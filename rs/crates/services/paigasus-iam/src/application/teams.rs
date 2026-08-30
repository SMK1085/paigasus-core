// SPDX-License-Identifier: Apache-2.0

//! `TeamService`: team lifecycle (create, get, list, rename, archive, restore) scoped to
//! an organization (SMA-442, ADR-0014).

use crate::application::error::TenancyError;
use crate::application::pagination::Page;
use paigasus_iam_core::{Clock, IdGenerator, NodeStatus, NodeView, PrincipalId, Slug, Stamp, Team, TeamRepository};
use uuid::Uuid;

/// Team lifecycle use cases, scoped to an organization. Generic-DI-by-value
/// (`R`epository, `I`d generator, `C`lock) — no `Arc<dyn>`, mirroring `OrganizationService`
/// (design doc §5).
#[derive(Clone)]
pub struct TeamService<R, I, C> {
    repo: R,
    ids: I,
    clock: C,
}

impl<R, I, C> TeamService<R, I, C>
where
    R: TeamRepository,
    I: IdGenerator,
    C: Clock,
{
    pub fn new(repo: R, ids: I, clock: C) -> Self {
        Self { repo, ids, clock }
    }

    /// Creates a team under `org`. `NotFound` if the org is missing; `ParentArchived` if
    /// the org is effectively archived (repo-enforced in-txn guard, D8). Returns the
    /// repo-computed view (a team created under an active org is `Active`).
    pub async fn create(&self, org: Uuid, slug: &str, name: &str, actor: &PrincipalId) -> Result<NodeView<Team>, TenancyError> {
        let slug = Slug::parse(slug)?;
        let stamp = Stamp::new(self.clock.now(), actor.clone());
        let id = self.ids.new_team_id(org);
        let team = Team::new(id, slug, name, &stamp)?;

        self.repo.create(&team, &stamp).await?;
        self.repo.find(team.id.uuid()).await?.ok_or(TenancyError::Internal)
    }

    /// Fetches a team by id. `NotFound` if absent.
    pub async fn get(&self, id: Uuid) -> Result<NodeView<Team>, TenancyError> {
        self.repo.find(id).await?.ok_or(TenancyError::NotFound)
    }

    /// Lists teams under `org`, `ORDER BY created_at, id` (design doc §5.1 rule 9).
    pub async fn list_by_org(&self, org: Uuid, page: Page) -> Result<Vec<NodeView<Team>>, TenancyError> {
        Ok(self.repo.list_by_org(org, page.limit, page.offset).await?)
    }

    /// Renames the slug and/or display name. Requires at least one field
    /// (`NothingToRename` otherwise); rejected on an EFFECTIVELY archived team — own status
    /// or ancestor org (`NodeArchived`).
    pub async fn rename(&self, id: Uuid, new_slug: Option<&str>, new_name: Option<&str>, actor: &PrincipalId) -> Result<NodeView<Team>, TenancyError> {
        if new_slug.is_none() && new_name.is_none() {
            return Err(TenancyError::NothingToRename);
        }
        let slug = new_slug.map(Slug::parse).transpose()?;
        let stamp = Stamp::new(self.clock.now(), actor.clone());
        Ok(self.repo.rename(id, slug.as_ref(), new_name, &stamp).await?)
    }

    /// Sets the team's own status to `Archived`. Always permitted (D10) — a team may be
    /// archived directly even while its org is active, or while already effectively
    /// archived via the org. Idempotent: a no-op leaves `updated_at` untouched.
    pub async fn archive(&self, id: Uuid, actor: &PrincipalId) -> Result<NodeView<Team>, TenancyError> {
        let stamp = Stamp::new(self.clock.now(), actor.clone());
        Ok(self.repo.set_status(id, NodeStatus::Archived, &stamp).await?)
    }

    /// Sets the team's own status to `Active`. Idempotent, mirroring `archive`. Note the
    /// team may still be *effectively* archived afterward if its org remains archived.
    pub async fn restore(&self, id: Uuid, actor: &PrincipalId) -> Result<NodeView<Team>, TenancyError> {
        let stamp = Stamp::new(self.clock.now(), actor.clone());
        Ok(self.repo.set_status(id, NodeStatus::Active, &stamp).await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::fakes::{FixedClock, InMemoryOrgs, InMemoryTeams, SeqIds, TenancyStore, test_stamp};
    use chrono::{Duration, TimeZone, Utc};
    use paigasus_iam_core::{Organization, OrganizationId, OrganizationRepository};

    fn new_service(store: TenancyStore) -> TeamService<InMemoryTeams, SeqIds, FixedClock> {
        TeamService::new(InMemoryTeams(store), SeqIds::default(), FixedClock::default())
    }

    /// A deterministic `PrincipalId` for service-call `actor` arguments — mirrors
    /// `organizations.rs`'s own test helper of the same name.
    fn actor(n: u128) -> PrincipalId {
        PrincipalId::from_prn(paigasus_kernel::Prn::build("iam", "", None, "principal", Uuid::from_u128(n)).unwrap())
    }

    /// Seeds an org directly into the shared store (bypassing `InMemoryOrgs::create`, which
    /// would also provision an unrelated "default" team into the shared team map).
    fn seed_org(store: &TenancyStore, uuid: u128, slug: &str, stamp: &Stamp) -> Uuid {
        let id = Uuid::from_u128(uuid);
        let org = Organization::new(OrganizationId::from_uuid(id), Slug::parse(slug).unwrap(), "Org", stamp).unwrap();
        store.orgs.lock().unwrap().insert(id, org);
        id
    }

    #[tokio::test]
    async fn create_team_under_missing_or_archived_org_fails() {
        let store = TenancyStore::default();
        let svc = new_service(store.clone());

        // Missing org -> NotFound.
        assert_eq!(svc.create(Uuid::from_u128(1), "eng", "Engineering", &actor(1)).await.unwrap_err(), TenancyError::NotFound);

        // Effectively-archived org -> ParentArchived (fake honors the port's in-txn guard).
        let org = seed_org(&store, 9000, "acme", &test_stamp(Utc::now(), 1));
        InMemoryOrgs(store.clone()).set_status(org, NodeStatus::Archived, &test_stamp(Utc::now(), 1)).await.unwrap();
        assert_eq!(svc.create(org, "eng", "Engineering", &actor(1)).await.unwrap_err(), TenancyError::ParentArchived);
    }

    #[tokio::test]
    async fn team_effective_status_follows_org() {
        let store = TenancyStore::default();
        let org = seed_org(&store, 9001, "acme", &test_stamp(Utc::now(), 1));
        let svc = new_service(store.clone());

        let created = svc.create(org, "eng", "Engineering", &actor(1)).await.unwrap();
        let team_id = created.node.id.uuid();
        assert_eq!(created.node.status, NodeStatus::Active);
        assert_eq!(created.effective_status, NodeStatus::Active);

        // Archiving the org (via the shared store, same as an `InMemoryOrgs` handle would)
        // folds into the team's effective status without touching the team's own flag.
        InMemoryOrgs(store.clone()).set_status(org, NodeStatus::Archived, &test_stamp(Utc::now(), 1)).await.unwrap();
        let view = svc.get(team_id).await.unwrap();
        assert_eq!(view.node.status, NodeStatus::Active);
        assert_eq!(view.effective_status, NodeStatus::Archived);

        // D10: archiving the team directly is still permitted even while it is already
        // effectively archived via the org.
        let archived = svc.archive(team_id, &actor(1)).await.unwrap();
        assert_eq!(archived.node.status, NodeStatus::Archived);
        assert_eq!(archived.effective_status, NodeStatus::Archived);

        // Restoring the org does not clear the team's own archived flag — it stays
        // effectively archived.
        InMemoryOrgs(store.clone()).set_status(org, NodeStatus::Active, &test_stamp(Utc::now(), 1)).await.unwrap();
        let still_archived = svc.get(team_id).await.unwrap();
        assert_eq!(still_archived.node.status, NodeStatus::Archived);
        assert_eq!(still_archived.effective_status, NodeStatus::Archived);
    }

    #[tokio::test]
    async fn duplicate_slug_is_conflict_scoped_to_org() {
        let store = TenancyStore::default();
        let org1 = seed_org(&store, 9002, "acme", &test_stamp(Utc::now(), 1));
        let org2 = seed_org(&store, 9003, "beta", &test_stamp(Utc::now(), 1));
        let svc = new_service(store.clone());

        svc.create(org1, "eng", "Engineering", &actor(1)).await.unwrap();
        assert_eq!(svc.create(org1, "eng", "Eng 2", &actor(1)).await.unwrap_err(), TenancyError::SlugConflict);
        // The same slug under a different org is fine — uniqueness is scoped per org.
        svc.create(org2, "eng", "Engineering", &actor(1)).await.unwrap();
    }

    #[tokio::test]
    async fn archive_is_idempotent_and_restore_reverses() {
        let store = TenancyStore::default();
        let org = seed_org(&store, 9004, "acme", &test_stamp(Utc::now(), 1));
        let clock = FixedClock::default();
        let t0 = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        clock.set(t0);
        let svc = TeamService::new(InMemoryTeams(store.clone()), SeqIds::default(), clock.clone());

        let created = svc.create(org, "eng", "Engineering", &actor(1)).await.unwrap();
        let id = created.node.id.uuid();
        assert_eq!(created.node.updated_at, t0);

        let t1 = t0 + Duration::seconds(10);
        clock.set(t1);
        let archived = svc.archive(id, &actor(1)).await.unwrap();
        assert_eq!(archived.node.status, NodeStatus::Archived);
        assert_eq!(archived.node.updated_at, t1);

        // Archiving an already-archived team is a no-op: updated_at does not advance.
        let t2 = t1 + Duration::seconds(10);
        clock.set(t2);
        let archived_again = svc.archive(id, &actor(1)).await.unwrap();
        assert_eq!(archived_again.node.updated_at, t1);

        let t3 = t2 + Duration::seconds(10);
        clock.set(t3);
        let restored = svc.restore(id, &actor(1)).await.unwrap();
        assert_eq!(restored.node.status, NodeStatus::Active);
        assert_eq!(restored.node.updated_at, t3);

        // Restoring an already-active team is a no-op: updated_at does not advance.
        let t4 = t3 + Duration::seconds(10);
        clock.set(t4);
        let restored_again = svc.restore(id, &actor(1)).await.unwrap();
        assert_eq!(restored_again.node.updated_at, t3);
    }

    #[tokio::test]
    async fn rename_rejects_empty_change_and_effectively_archived_team() {
        let store = TenancyStore::default();
        let org = seed_org(&store, 9005, "acme", &test_stamp(Utc::now(), 1));
        let svc = new_service(store.clone());

        let created = svc.create(org, "eng", "Engineering", &actor(1)).await.unwrap();
        let id = created.node.id.uuid();

        assert_eq!(svc.rename(id, None, None, &actor(1)).await.unwrap_err(), TenancyError::NothingToRename);

        svc.archive(id, &actor(1)).await.unwrap();
        assert_eq!(svc.rename(id, Some("x"), None, &actor(1)).await.unwrap_err(), TenancyError::NodeArchived);
        svc.restore(id, &actor(1)).await.unwrap();

        // Effectively archived via the org (own status untouched) also blocks rename.
        InMemoryOrgs(store.clone()).set_status(org, NodeStatus::Archived, &test_stamp(Utc::now(), 1)).await.unwrap();
        assert_eq!(svc.rename(id, Some("x"), None, &actor(1)).await.unwrap_err(), TenancyError::NodeArchived);
    }

    #[tokio::test]
    async fn get_missing_is_not_found() {
        let store = TenancyStore::default();
        let svc = new_service(store);
        assert_eq!(svc.get(Uuid::from_u128(999)).await.unwrap_err(), TenancyError::NotFound);
    }

    #[tokio::test]
    async fn lists_are_ordered_and_paginated() {
        let store = TenancyStore::default();
        let org = seed_org(&store, 9006, "acme", &test_stamp(Utc::now(), 1));
        let clock = FixedClock::default();
        let t0 = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        clock.set(t0);
        let svc = TeamService::new(InMemoryTeams(store.clone()), SeqIds::default(), clock.clone());

        let a = svc.create(org, "alpha", "Alpha", &actor(1)).await.unwrap();
        clock.set(t0 + Duration::seconds(1));
        let b = svc.create(org, "bravo", "Bravo", &actor(1)).await.unwrap();
        clock.set(t0 + Duration::seconds(2));
        let c = svc.create(org, "charlie", "Charlie", &actor(1)).await.unwrap();

        let page = svc.list_by_org(org, Page::new(Some(2), Some(0)).unwrap()).await.unwrap();
        assert_eq!(page.len(), 2);
        assert_eq!(page[0].node.id.uuid(), a.node.id.uuid());
        assert_eq!(page[1].node.id.uuid(), b.node.id.uuid());

        let page2 = svc.list_by_org(org, Page::new(Some(2), Some(2)).unwrap()).await.unwrap();
        assert_eq!(page2.len(), 1);
        assert_eq!(page2[0].node.id.uuid(), c.node.id.uuid());
    }

    /// SMA-440 D5: a rename supplying the values the row already holds changes nothing, so it
    /// must advance neither `updated_at` nor `modified_by`.
    #[tokio::test]
    async fn rename_to_identical_values_is_a_no_op() {
        let store = TenancyStore::default();
        let org = seed_org(&store, 9007, "acme", &test_stamp(Utc::now(), 1));
        let clock = FixedClock::default();
        let t0 = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        clock.set(t0);
        let svc = TeamService::new(InMemoryTeams(store.clone()), SeqIds::default(), clock.clone());

        let created = svc.create(org, "eng", "Engineering", &actor(1)).await.unwrap();
        let id = created.node.id.uuid();

        clock.set(t0 + Duration::seconds(10));
        let same = svc.rename(id, Some("eng"), Some("Engineering"), &actor(2)).await.unwrap();
        assert_eq!(same.node.updated_at, t0, "a no-op rename must not advance updated_at");
        assert_eq!(same.node.modified_by.as_ref(), Some(&actor(1)), "a no-op rename must not restamp the modifier");
    }

    /// The negative half, and the one that catches an over-broad no-op: a matching slug with a
    /// DIFFERENT name is a real change and must restamp. Without this, a rename that compares
    /// only the slug would pass the test above while silently dropping every rename.
    #[tokio::test]
    async fn rename_with_a_matching_slug_but_a_new_name_still_changes() {
        let store = TenancyStore::default();
        let org = seed_org(&store, 9008, "acme", &test_stamp(Utc::now(), 1));
        let clock = FixedClock::default();
        let t0 = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        clock.set(t0);
        let svc = TeamService::new(InMemoryTeams(store.clone()), SeqIds::default(), clock.clone());

        let created = svc.create(org, "eng", "Engineering", &actor(1)).await.unwrap();
        let id = created.node.id.uuid();

        let t1 = t0 + Duration::seconds(10);
        clock.set(t1);
        let renamed = svc.rename(id, Some("eng"), Some("Engineering Team"), &actor(2)).await.unwrap();
        assert_eq!(renamed.node.name, "Engineering Team");
        assert_eq!(renamed.node.updated_at, t1);
        assert_eq!(renamed.node.modified_by.as_ref(), Some(&actor(2)));
        // Spec Testing case 2: an update moves the MODIFIER and leaves the CREATOR alone. An
        // implementation that stamps both on every write passes every other assertion here.
        assert_eq!(renamed.node.created_by.as_ref(), Some(&actor(1)), "an update must not rewrite created_by");
        assert_eq!(renamed.node.created_at, t0, "an update must not rewrite created_at");
    }

    /// SMA-440 D5, single-field no-op: supplying ONLY a matching slug (name omitted) must
    /// still be treated as a no-op. `new_slug.is_some_and(...)` instead of `is_none_or(...)`
    /// would treat the omitted `new_name` as "differs" and wrongly restamp here.
    #[tokio::test]
    async fn rename_to_identical_slug_only_is_a_no_op() {
        let store = TenancyStore::default();
        let org = seed_org(&store, 9011, "acme", &test_stamp(Utc::now(), 1));
        let clock = FixedClock::default();
        let t0 = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        clock.set(t0);
        let svc = TeamService::new(InMemoryTeams(store.clone()), SeqIds::default(), clock.clone());

        let created = svc.create(org, "eng", "Engineering", &actor(1)).await.unwrap();
        let id = created.node.id.uuid();

        clock.set(t0 + Duration::seconds(10));
        let same = svc.rename(id, Some("eng"), None, &actor(2)).await.unwrap();
        assert_eq!(same.node.updated_at, t0, "a slug-only no-op rename must not advance updated_at");
        assert_eq!(same.node.modified_by.as_ref(), Some(&actor(1)), "a slug-only no-op rename must not restamp the modifier");
    }

    /// The mirror of the above: supplying ONLY a matching name (slug omitted) must also be a
    /// no-op.
    #[tokio::test]
    async fn rename_to_identical_name_only_is_a_no_op() {
        let store = TenancyStore::default();
        let org = seed_org(&store, 9012, "acme", &test_stamp(Utc::now(), 1));
        let clock = FixedClock::default();
        let t0 = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        clock.set(t0);
        let svc = TeamService::new(InMemoryTeams(store.clone()), SeqIds::default(), clock.clone());

        let created = svc.create(org, "eng", "Engineering", &actor(1)).await.unwrap();
        let id = created.node.id.uuid();

        clock.set(t0 + Duration::seconds(10));
        let same = svc.rename(id, None, Some("Engineering"), &actor(2)).await.unwrap();
        assert_eq!(same.node.updated_at, t0, "a name-only no-op rename must not advance updated_at");
        assert_eq!(same.node.modified_by.as_ref(), Some(&actor(1)), "a name-only no-op rename must not restamp the modifier");
    }

    /// Spec case 4: a DIFFERENT slug paired with the SAME name is still a real change and
    /// must restamp both fields. Complements
    /// `rename_with_a_matching_slug_but_a_new_name_still_changes`, which covers the mirror
    /// case (same slug, different name).
    #[tokio::test]
    async fn rename_with_a_new_slug_but_matching_name_still_changes() {
        let store = TenancyStore::default();
        let org = seed_org(&store, 9013, "acme", &test_stamp(Utc::now(), 1));
        let clock = FixedClock::default();
        let t0 = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        clock.set(t0);
        let svc = TeamService::new(InMemoryTeams(store.clone()), SeqIds::default(), clock.clone());

        let created = svc.create(org, "eng", "Engineering", &actor(1)).await.unwrap();
        let id = created.node.id.uuid();

        let t1 = t0 + Duration::seconds(10);
        clock.set(t1);
        let renamed = svc.rename(id, Some("eng-2"), Some("Engineering"), &actor(2)).await.unwrap();
        assert_eq!(renamed.node.slug.as_str(), "eng-2");
        assert_eq!(renamed.node.updated_at, t1, "a new-slug rename must advance updated_at even with a matching name");
        assert_eq!(
            renamed.node.modified_by.as_ref(),
            Some(&actor(2)),
            "a new-slug rename must restamp the modifier even with a matching name"
        );
    }

    /// Guard order: the archived precondition runs BEFORE the no-op test, so renaming an
    /// archived node to its own slug is still an error and not a silent Ok.
    #[tokio::test]
    async fn a_no_op_rename_on_an_archived_node_is_still_rejected() {
        let store = TenancyStore::default();
        let org = seed_org(&store, 9009, "acme", &test_stamp(Utc::now(), 1));
        let svc = new_service(store.clone());
        let created = svc.create(org, "eng", "Engineering", &actor(1)).await.unwrap();
        let id = created.node.id.uuid();
        svc.archive(id, &actor(1)).await.unwrap();
        assert_eq!(svc.rename(id, Some("eng"), None, &actor(2)).await.unwrap_err(), TenancyError::NodeArchived);
    }

    /// The `set_status` half of D5: an idempotent archive advances neither field.
    #[tokio::test]
    async fn an_idempotent_archive_does_not_restamp_the_modifier() {
        let store = TenancyStore::default();
        let org = seed_org(&store, 9010, "acme", &test_stamp(Utc::now(), 1));
        let clock = FixedClock::default();
        let t0 = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        clock.set(t0);
        let svc = TeamService::new(InMemoryTeams(store.clone()), SeqIds::default(), clock.clone());

        let created = svc.create(org, "eng", "Engineering", &actor(1)).await.unwrap();
        let id = created.node.id.uuid();

        clock.set(t0 + Duration::seconds(10));
        svc.archive(id, &actor(2)).await.unwrap();

        clock.set(t0 + Duration::seconds(20));
        let again = svc.archive(id, &actor(3)).await.unwrap();
        assert_eq!(again.node.updated_at, t0 + Duration::seconds(10));
        assert_eq!(again.node.modified_by.as_ref(), Some(&actor(2)), "a no-op archive must not restamp");
    }
}
