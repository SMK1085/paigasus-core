// SPDX-License-Identifier: Apache-2.0

//! `MembershipService`: attach/detach/list principal-to-tenancy-node memberships
//! (SMA-442, ADR-0014).
//!
//! **SMA-606 D1/D2/D6 — the UoW reference pattern, applied to memberships:** same shape as
//! `TeamService` (see its module docs for the full rationale), minus even the `entity_gen`
//! bump — `MembershipRepository` has no `gens` field and has never bumped one, because
//! `pg_entity_slice.rs` never reads memberships, so a membership change invalidates nothing
//! (D7). `attach` calls `attach_in` first and builds its event/entry from the **returned**
//! `MembershipRecord`, never from its own locally-parsed PRN or the caller's raw input string:
//! `attach_in` byte-matches the supplied node PRN against the stored one and answers
//! `PrnMismatch` on a mismatch, and echoing the caller's input would route a forged org slot
//! straight past that defense into the event stream (the security corollary
//! `tests/authz_forged_org_slot_escalation.rs` covers end-to-end). `detach` fans out: it calls
//! `detach_in`, which returns every row its cascade deleted (D6), and emits ONE event and ONE
//! audit entry PER DELETED ROW, all sharing one correlation id — the directly requested row's
//! detail carries no extra key, while each cascaded row's detail carries `"cascade_of"` naming
//! the requested membership id, since authorization ran once, at the requested node, and an
//! unmarked entry would misstate what was authorized.

use crate::application::error::TenancyError;
use crate::application::pagination::Page;
use paigasus_iam_core::{
    Action, AuditEntry, AuditLog, AuditOutcome, Clock, DomainEvent, EventType, IdGenerator, Membership, MembershipRecord, MembershipRepository, Outbox, PrincipalId, Stamp, TenancyNodeRef, UnitOfWork,
};
use paigasus_kernel::Prn;
use std::sync::Arc;
use uuid::Uuid;

/// Which axis to filter a membership listing by — raw PRN strings from the wire, parsed
/// the same way `attach`'s arguments are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MembershipFilter {
    Principal(String),
    Node(String),
}

/// Parses a raw principal PRN string: must be syntactically valid (else `InvalidPrn` with
/// the kernel's stable error-kind token), and must be service `"iam"`, resource type
/// `"principal"` (else `InvalidPrn` with the PRN's canonical form).
fn parse_principal_prn(raw: &str) -> Result<PrincipalId, TenancyError> {
    let prn = Prn::parse(raw).map_err(|e| TenancyError::InvalidPrn(e.kind().to_owned()))?;
    if prn.service() != "iam" || prn.resource_type() != "principal" {
        return Err(TenancyError::InvalidPrn(prn.canonical()));
    }
    Ok(PrincipalId::from_prn(prn))
}

/// Parses a raw tenancy-node PRN string into a typed node ref (organization/team/project).
/// A `DomainError` from `TenancyNodeRef::from_prn` (wrong resource type, malformed org slot)
/// auto-converts into `TenancyError::InvalidPrn`.
fn parse_node_prn(raw: &str) -> Result<TenancyNodeRef, TenancyError> {
    let prn = Prn::parse(raw).map_err(|e| TenancyError::InvalidPrn(e.kind().to_owned()))?;
    Ok(TenancyNodeRef::from_prn(prn)?)
}

/// Named-field constructor params for [`MembershipService::new`] (SMA-606 D1), mirroring
/// `TeamServiceDeps` — one field per dependency, built with struct syntax at the call site so
/// each argument is self-labeling. There is deliberately no `gen_bumper` field at all (not
/// even an `EntityGenBumper`, unlike `TeamServiceDeps`): memberships never feed the entity
/// slice, so a membership change invalidates nothing (D7).
pub struct MembershipServiceDeps<M, I, C> {
    pub repo: M,
    pub uow: Arc<dyn UnitOfWork>,
    pub outbox: Arc<dyn Outbox>,
    pub audit: Arc<dyn AuditLog>,
    pub ids: I,
    pub clock: C,
}

/// Membership lifecycle use cases: attach a principal to a tenancy node, detach, list.
/// Generic-DI-by-value (`M`embership repository, `I`d generator, `C`lock) — no `Arc<dyn>`,
/// mirroring `OrganizationService`/`TeamService`/`ProjectService` (design doc §5);
/// `uow`/`outbox`/`audit` are the shared `Arc<dyn ...>` port handles (SMA-606 D1).
#[derive(Clone)]
pub struct MembershipService<M, I, C> {
    repo: M,
    uow: Arc<dyn UnitOfWork>,
    outbox: Arc<dyn Outbox>,
    audit: Arc<dyn AuditLog>,
    ids: I,
    clock: C,
}

impl<M, I, C> MembershipService<M, I, C>
where
    M: MembershipRepository,
    I: IdGenerator,
    C: Clock,
{
    pub fn new(deps: MembershipServiceDeps<M, I, C>) -> Self {
        Self {
            repo: deps.repo,
            uow: deps.uow,
            outbox: deps.outbox,
            audit: deps.audit,
            ids: deps.ids,
            clock: deps.clock,
        }
    }

    /// Attaches `principal_prn` to `node_prn`, recording `actor` as the creator. Parses/
    /// validates the wire PRNs and mints the membership id/timestamp, then calls `attach_in` —
    /// every existence/guard check (principal exists, node exists, prn byte-match,
    /// effectively-active, org-membership invariant, duplicate) happens in-txn there (D8, port
    /// doc contract).
    ///
    /// SMA-606 D2 security corollary: the event/entry's `node_prn` (and `principal_prn`) are
    /// read from the **returned** `MembershipRecord`, never from the locally-parsed `node`/
    /// `principal` or the caller's raw `node_prn`/`principal_prn` strings. `attach_in` already
    /// byte-matched the caller's input against the stored PRN and would have answered
    /// `PrnMismatch` on any divergence — but echoing the caller's own input here, rather than
    /// the value the repository actually resolved and stored, would make that defense
    /// irrelevant to the event stream: a future repository bug (or a change that weakens the
    /// byte-match) would then leak a forged PRN to every subscriber even though the write
    /// itself stayed correct.
    pub async fn attach(&self, principal_prn: &str, node_prn: &str, actor: &PrincipalId) -> Result<MembershipRecord, TenancyError> {
        let principal = parse_principal_prn(principal_prn)?;
        let node = parse_node_prn(node_prn)?;
        let stamp = Stamp::new(self.clock.now(), actor.clone());
        let membership = Membership::new(self.ids.new_membership_id(), principal, node, &stamp);
        let corr = self.ids.new_correlation_id();

        let tx = self.uow.begin().await?;
        let record = self.repo.attach_in(&*tx, &membership, &stamp).await?;

        // SMA-606 D5: the audit detail carries the same payload shape as the event, not an
        // empty object.
        let detail = serde_json::json!({
            "membership_id": record.id,
            "principal_prn": record.principal_prn,
            "node_prn": record.node_prn,
        });
        let ev = DomainEvent {
            id: self.ids.new_event_id(),
            event_type: EventType::MembershipAttached,
            schema_version: 1,
            aggregate_prn: record.node_prn.clone(),
            actor_prn: Some(actor.canonical()),
            occurred_at: stamp.at,
            payload: detail.clone(),
            correlation_id: Some(corr),
        };
        let entry = AuditEntry {
            id: self.ids.new_audit_id(),
            occurred_at: stamp.at,
            actor_prn: Some(actor.canonical()),
            action: Action::AttachMembership.as_wire().to_string(),
            resource_prn: Some(record.node_prn.clone()),
            outcome: AuditOutcome::Committed,
            determining_policies: vec![],
            detail,
            correlation_id: Some(corr),
        };
        self.outbox.enqueue(&*tx, &ev).await?;
        self.audit.record(&*tx, &entry).await?;
        tx.commit().await?;

        Ok(record)
    }

    /// Fetches a membership record by id. `NotFound` if absent. SMA-444 Task 20: the
    /// `DetachMembership` HTTP/gRPC handlers call this FIRST to learn the membership's
    /// `node_prn` (the resource `Action::DetachMembership` authorizes against) before
    /// authorizing — mirrors `OrganizationService::get`/`TeamService::get`/
    /// `ProjectService::get`'s existence-check posture.
    pub async fn get(&self, id: Uuid) -> Result<MembershipRecord, TenancyError> {
        self.repo.find(id).await?.ok_or(TenancyError::NotFound)
    }

    /// Detaches a membership by id. `NotFound` if missing. Detaching an org membership
    /// cascades: `detach_in` also detaches the principal's team/project memberships scoped to
    /// that org, in one transaction (rule 5), and returns every row it deleted.
    ///
    /// SMA-606 D6: one event and one audit entry PER DELETED ROW, all sharing ONE correlation
    /// id — one operation, even though it may touch several rows. This is what makes "when did
    /// this principal lose access to project X" answerable by filtering on that project's PRN;
    /// a single org-level event would hide exactly the fact the trail exists to expose. The
    /// directly requested row's detail carries no `cascade_of` key; each cascaded row's detail
    /// carries `"cascade_of": id` — authorization ran once, against the requested node, and an
    /// unmarked cascaded entry would read as a separately authorized `DetachMembership` (D5).
    /// No generation bump: `MembershipRepository` has no bumper at all (see module docs).
    pub async fn detach(&self, id: Uuid, actor: &PrincipalId) -> Result<(), TenancyError> {
        let now = self.clock.now();
        let corr = self.ids.new_correlation_id();

        let tx = self.uow.begin().await?;
        let deleted = self.repo.detach_in(&*tx, id).await?;
        for record in &deleted {
            // SMA-606 fix wave (D9/D5): `cascade_of` is provenance, not payload — D9's table
            // pins the detached payload at exactly `{membership_id, principal_prn, node_prn}`,
            // so it must never appear on the wire event. It belongs on the audit entry's
            // `detail` only. A consumer that wants the grouping already has the shared
            // `correlation_id`.
            let payload = serde_json::json!({
                "membership_id": record.id,
                "principal_prn": record.principal_prn,
                "node_prn": record.node_prn,
            });
            let detail = if record.id == id {
                payload.clone()
            } else {
                serde_json::json!({
                    "membership_id": record.id,
                    "principal_prn": record.principal_prn,
                    "node_prn": record.node_prn,
                    "cascade_of": id,
                })
            };
            let ev = DomainEvent {
                id: self.ids.new_event_id(),
                event_type: EventType::MembershipDetached,
                schema_version: 1,
                aggregate_prn: record.node_prn.clone(),
                actor_prn: Some(actor.canonical()),
                occurred_at: now,
                payload,
                correlation_id: Some(corr),
            };
            let entry = AuditEntry {
                id: self.ids.new_audit_id(),
                occurred_at: now,
                actor_prn: Some(actor.canonical()),
                action: Action::DetachMembership.as_wire().to_string(),
                resource_prn: Some(record.node_prn.clone()),
                outcome: AuditOutcome::Committed,
                determining_policies: vec![],
                detail,
                correlation_id: Some(corr),
            };
            self.outbox.enqueue(&*tx, &ev).await?;
            self.audit.record(&*tx, &entry).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Lists memberships by principal or node, `ORDER BY created_at, id` (design doc §5.1
    /// rule 9).
    pub async fn list(&self, filter: MembershipFilter, page: Page) -> Result<Vec<MembershipRecord>, TenancyError> {
        match filter {
            MembershipFilter::Principal(raw) => {
                let principal_id = parse_principal_prn(&raw)?;
                Ok(self.repo.list_by_principal(principal_id.uuid(), page.limit, page.offset).await?)
            }
            MembershipFilter::Node(raw) => {
                let node = parse_node_prn(&raw)?;
                Ok(self.repo.list_by_node(&node, page.limit, page.offset).await?)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::fakes::{FakeAuditLog, FakeOutbox, FakeUnitOfWork, FixedClock, InMemoryMemberships, SeqIds, TenancyStore, test_stamp};
    use chrono::{DateTime, TimeZone, Utc};
    use paigasus_iam_core::{NodeStatus, Organization, OrganizationId, Project, ProjectId, RepositoryError, Slug, Team, TeamId, Transaction};

    /// Builds a `MembershipService` over `store` with fresh, unobserved fakes for every
    /// dependency this SMA-606 conversion added — the tests that only care about lifecycle
    /// behaviour (not what got emitted) use this so they don't have to thread outbox/audit/uow
    /// handles through.
    fn new_service(store: TenancyStore) -> MembershipService<InMemoryMemberships, SeqIds, FixedClock> {
        MembershipService::new(MembershipServiceDeps {
            repo: InMemoryMemberships(store),
            uow: Arc::new(FakeUnitOfWork::default()),
            outbox: Arc::new(FakeOutbox::default()),
            audit: Arc::new(FakeAuditLog::default()),
            ids: SeqIds::default(),
            clock: FixedClock::default(),
        })
    }

    /// Bundles a `MembershipService` together with every fake it was built over (SMA-606,
    /// mirrors `organizations.rs`/`teams.rs`'s `service_with_fakes`). The `FakeUnitOfWork`
    /// handle is returned (not just consumed) so a test can assert `commits()` directly —
    /// `fakes.rs:1082-1088` documents that every other fake mutates its own state regardless of
    /// whether `commit` is ever called, so a deleted `tx.commit().await?` would otherwise pass
    /// every test unnoticed.
    fn service_with_fakes(store: TenancyStore) -> (MembershipService<InMemoryMemberships, SeqIds, FixedClock>, FakeOutbox, FakeAuditLog, FakeUnitOfWork) {
        let outbox = FakeOutbox::default();
        let audit = FakeAuditLog::default();
        let uow = FakeUnitOfWork::default();
        let svc = MembershipService::new(MembershipServiceDeps {
            repo: InMemoryMemberships(store),
            uow: Arc::new(uow.clone()),
            outbox: Arc::new(outbox.clone()),
            audit: Arc::new(audit.clone()),
            ids: SeqIds::default(),
            clock: FixedClock::default(),
        });
        (svc, outbox, audit, uow)
    }

    /// A deterministic `PrincipalId` for `attach`'s `actor` argument — these tests exercise
    /// `attach`'s own guards, not authorization, so any well-formed actor does.
    fn actor(n: u128) -> PrincipalId {
        PrincipalId::from_prn(Prn::build("iam", "", None, "principal", Uuid::from_u128(n)).unwrap())
    }

    /// A `MembershipRepository` whose `attach_in` returns a record carrying a `node_prn`
    /// deliberately DIFFERENT from the membership it was handed (SMA-606 D2 security
    /// corollary). A real repository's byte-match guard makes the two identical on every
    /// successful call, so nothing built over `InMemoryMemberships`/`PgMembershipRepository`
    /// can pull them apart to prove `MembershipService::attach` forwards the REPOSITORY's
    /// returned prn into the event — never its own locally-parsed `node` or the caller's raw
    /// input string. This fake exists solely to make that assertion possible at the unit level.
    #[derive(Clone, Default)]
    struct StoredPrnDiffersRepo;

    #[async_trait::async_trait]
    impl MembershipRepository for StoredPrnDiffersRepo {
        async fn attach(&self, membership: &Membership, stamp: &Stamp) -> Result<MembershipRecord, RepositoryError> {
            let tx: Box<dyn Transaction> = Box::new(crate::application::fakes::CountingTransaction::detached());
            self.attach_in(&*tx, membership, stamp).await
        }

        async fn attach_in(&self, _tx: &dyn Transaction, membership: &Membership, _stamp: &Stamp) -> Result<MembershipRecord, RepositoryError> {
            Ok(MembershipRecord {
                id: membership.id,
                principal_prn: membership.principal_id.canonical(),
                // Deliberately NOT `membership.node.canonical()` — stands in for whatever a
                // real repository resolved and actually stored, which the service must use
                // instead of its own locally-parsed value.
                node_prn: "prn:pgs:iam::00000000-0000-0000-0000-0000000000ab:organization/00000000-0000-0000-0000-0000000000cd".to_string(),
                created_at: membership.created_at,
                created_by: membership.created_by.clone(),
            })
        }

        async fn find(&self, _id: Uuid) -> Result<Option<MembershipRecord>, RepositoryError> {
            unimplemented!("this test double only exercises attach")
        }

        async fn detach(&self, _id: Uuid) -> Result<(), RepositoryError> {
            unimplemented!("this test double only exercises attach")
        }

        async fn detach_in(&self, _tx: &dyn Transaction, _id: Uuid) -> Result<Vec<MembershipRecord>, RepositoryError> {
            unimplemented!("this test double only exercises attach")
        }

        async fn list_by_principal(&self, _principal: Uuid, _limit: u64, _offset: u64) -> Result<Vec<MembershipRecord>, RepositoryError> {
            unimplemented!("this test double only exercises attach")
        }

        async fn list_by_node(&self, _node: &TenancyNodeRef, _limit: u64, _offset: u64) -> Result<Vec<MembershipRecord>, RepositoryError> {
            unimplemented!("this test double only exercises attach")
        }
    }

    /// Seeds a principal directly into the shared store's `principals` map (the
    /// canonical-prn record `InMemoryMemberships` checks caller prns against).
    fn seed_principal(store: &TenancyStore, uuid: u128) -> PrincipalId {
        let id = Uuid::from_u128(uuid);
        let principal_id = PrincipalId::from_prn(Prn::build("iam", "", None, "principal", id).unwrap());
        store.principals.lock().unwrap().insert(id, principal_id.canonical());
        principal_id
    }

    /// Seeds an org + a team under it directly into the shared store.
    fn seed_org_and_team(store: &TenancyStore, org_n: u128, team_n: u128, now: DateTime<Utc>) -> (Uuid, Uuid) {
        let stamp = test_stamp(now, 1);
        let org_id = Uuid::from_u128(org_n);
        let org = Organization::new(OrganizationId::from_uuid(org_id), Slug::parse("acme").unwrap(), "Acme", &stamp).unwrap();
        store.orgs.lock().unwrap().insert(org_id, org);

        let team_id = Uuid::from_u128(team_n);
        let team = Team::new(TeamId::from_parts(org_id, team_id), Slug::parse("eng").unwrap(), "Engineering", &stamp).unwrap();
        store.teams.lock().unwrap().insert(team_id, team);

        (org_id, team_id)
    }

    #[tokio::test]
    async fn attach_happy_paths_and_invariant() {
        let store = TenancyStore::default();
        let now = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        let principal = seed_principal(&store, 1);
        let (org, team) = seed_org_and_team(&store, 100, 101, now);
        let svc = new_service(store.clone());

        let org_prn = OrganizationId::from_uuid(org).canonical();
        let team_prn = TeamId::from_parts(org, team).canonical();

        // Without an org membership, attaching to a team fails the invariant.
        assert_eq!(svc.attach(&principal.canonical(), &team_prn, &actor(999)).await.unwrap_err(), TenancyError::MissingOrgMembership);

        // Attaching to the org itself succeeds and returns the org's canonical prn.
        let org_membership = svc.attach(&principal.canonical(), &org_prn, &actor(999)).await.unwrap();
        assert_eq!(org_membership.node_prn, org_prn);
        assert_eq!(org_membership.principal_prn, principal.canonical());

        // Now that the org membership exists, the team attach succeeds.
        svc.attach(&principal.canonical(), &team_prn, &actor(999)).await.unwrap();

        // A duplicate org attach is a conflict.
        assert_eq!(svc.attach(&principal.canonical(), &org_prn, &actor(999)).await.unwrap_err(), TenancyError::DuplicateMembership);
    }

    #[tokio::test]
    async fn attach_rejects_bad_prns() {
        let store = TenancyStore::default();
        let now = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        let principal = seed_principal(&store, 10);
        let (org, team) = seed_org_and_team(&store, 200, 201, now);
        let svc = new_service(store.clone());
        let team_prn = TeamId::from_parts(org, team).canonical();

        // Not a PRN at all.
        assert!(matches!(svc.attach("not-a-prn", &team_prn, &actor(999)).await.unwrap_err(), TenancyError::InvalidPrn(_)));

        // Well-formed PRN, but the wrong resource type for a principal.
        let user_prn = Prn::build("iam", "", None, "user", Uuid::from_u128(999)).unwrap().canonical();
        assert!(matches!(svc.attach(&user_prn, &team_prn, &actor(999)).await.unwrap_err(), TenancyError::InvalidPrn(_)));

        // Forged node prn: the correct team uuid, but a different org uuid in the org slot.
        let wrong_org = Uuid::from_u128(9_999);
        let forged_team_prn = format!("prn:pgs:iam::{wrong_org}:team/{team}");
        assert_eq!(svc.attach(&principal.canonical(), &forged_team_prn, &actor(999)).await.unwrap_err(), TenancyError::PrnMismatch);

        // Unknown principal (well-formed, but never seeded into the store).
        let unknown_principal = Prn::build("iam", "", None, "principal", Uuid::from_u128(12_345)).unwrap().canonical();
        assert_eq!(svc.attach(&unknown_principal, &team_prn, &actor(999)).await.unwrap_err(), TenancyError::NotFound);

        // Archived team: satisfy the org-membership invariant first, then archive the team
        // directly in the store and confirm the effective-status guard still fires.
        svc.attach(&principal.canonical(), &OrganizationId::from_uuid(org).canonical(), &actor(999)).await.unwrap();
        store.teams.lock().unwrap().get_mut(&team).unwrap().status = NodeStatus::Archived;
        assert_eq!(svc.attach(&principal.canonical(), &team_prn, &actor(999)).await.unwrap_err(), TenancyError::NodeArchived);
    }

    #[tokio::test]
    async fn detach_cascades_org_memberships() {
        let store = TenancyStore::default();
        let now = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        let principal = seed_principal(&store, 20);
        let (org, team) = seed_org_and_team(&store, 300, 301, now);

        let project_id = Uuid::from_u128(302);
        let project = Project::new(
            ProjectId::from_parts(org, project_id),
            TeamId::from_parts(org, team),
            Slug::parse("web").unwrap(),
            "Web",
            &test_stamp(now, 1),
        )
        .unwrap();
        store.projects.lock().unwrap().insert(project_id, project);

        let svc = new_service(store.clone());
        let org_prn = OrganizationId::from_uuid(org).canonical();
        let team_prn = TeamId::from_parts(org, team).canonical();
        let project_prn = ProjectId::from_parts(org, project_id).canonical();
        let page = Page::new(None, None).unwrap();

        let org_membership = svc.attach(&principal.canonical(), &org_prn, &actor(999)).await.unwrap();
        svc.attach(&principal.canonical(), &team_prn, &actor(999)).await.unwrap();
        svc.attach(&principal.canonical(), &project_prn, &actor(999)).await.unwrap();
        assert_eq!(svc.list(MembershipFilter::Principal(principal.canonical()), page).await.unwrap().len(), 3);

        // Detaching the org membership cascades: the team and project memberships for the
        // same principal in that org go with it.
        svc.detach(org_membership.id, &actor(999)).await.unwrap();
        assert!(svc.list(MembershipFilter::Principal(principal.canonical()), page).await.unwrap().is_empty());

        // Detaching an already-detached membership is `NotFound`.
        assert_eq!(svc.detach(org_membership.id, &actor(999)).await.unwrap_err(), TenancyError::NotFound);

        // A team-only detach removes only itself, leaving the org membership intact.
        let org_membership2 = svc.attach(&principal.canonical(), &org_prn, &actor(999)).await.unwrap();
        let team_membership2 = svc.attach(&principal.canonical(), &team_prn, &actor(999)).await.unwrap();
        svc.detach(team_membership2.id, &actor(999)).await.unwrap();
        let remaining = svc.list(MembershipFilter::Principal(principal.canonical()), page).await.unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, org_membership2.id);
    }

    #[tokio::test]
    async fn get_returns_the_record_or_not_found() {
        let store = TenancyStore::default();
        let now = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        let principal = seed_principal(&store, 30);
        let (org, _team) = seed_org_and_team(&store, 400, 401, now);
        let svc = new_service(store.clone());
        let org_prn = OrganizationId::from_uuid(org).canonical();

        let membership = svc.attach(&principal.canonical(), &org_prn, &actor(999)).await.unwrap();
        let fetched = svc.get(membership.id).await.unwrap();
        assert_eq!(fetched.node_prn, org_prn);
        assert_eq!(fetched.principal_prn, principal.canonical());

        assert_eq!(svc.get(Uuid::from_u128(999_999)).await.unwrap_err(), TenancyError::NotFound);
    }

    /// SMA-606 D1/D2: `attach` emits one event and one entry, sharing one correlation id, with
    /// the action taken from `Action::as_wire()` rather than a hand-typed literal, and the
    /// detail carrying the event's payload shape (not an empty object — Task 8 fix-round
    /// correction). Also asserts `commits()` directly: every other fake mutates its own state
    /// regardless of whether `tx.commit().await?` is ever called, so without this a deleted
    /// commit call would pass unnoticed (`fakes.rs:1082-1088`).
    #[tokio::test]
    async fn attach_emits_one_event_and_entry_and_commits() {
        let store = TenancyStore::default();
        let now = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        let principal = seed_principal(&store, 500);
        let (org, _team) = seed_org_and_team(&store, 501, 502, now);
        let (svc, outbox, audit, uow) = service_with_fakes(store.clone());
        let actor = actor(1);
        let org_prn = OrganizationId::from_uuid(org).canonical();

        let record = svc.attach(&principal.canonical(), &org_prn, &actor).await.unwrap();
        assert_eq!(uow.commits(), 1, "attach must commit its one transaction");

        let events = outbox.0.lock().unwrap().clone();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, EventType::MembershipAttached);
        assert_eq!(events[0].payload["node_prn"], record.node_prn);
        assert_eq!(events[0].payload["principal_prn"], record.principal_prn);

        let entries = audit.0.lock().unwrap().clone();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].action, Action::AttachMembership.as_wire());
        // SMA-606 D5 / Task 8 fix-round correction: the detail must carry the event's payload
        // shape, not an empty object.
        assert_eq!(entries[0].detail["node_prn"], record.node_prn);
        assert_eq!(entries[0].detail["principal_prn"], record.principal_prn);
        assert_eq!(entries[0].correlation_id, events[0].correlation_id);
    }

    /// SMA-606 D2 security corollary: the event carries the STORED node PRN from the returned
    /// record, never the caller's input (or the service's own locally-parsed value).
    /// `attach_in` byte-matches the supplied PRN against the stored one and answers
    /// `PrnMismatch` on a mismatch; echoing the input on success would make that defense
    /// irrelevant to the event stream should the returned record ever diverge — this test
    /// forces that divergence with [`StoredPrnDiffersRepo`] to prove the wiring is right
    /// regardless.
    #[tokio::test]
    async fn attach_emits_the_stored_node_prn_not_the_callers_input() {
        let outbox = FakeOutbox::default();
        let audit = FakeAuditLog::default();
        let uow = FakeUnitOfWork::default();
        let svc = MembershipService::new(MembershipServiceDeps {
            repo: StoredPrnDiffersRepo,
            uow: Arc::new(uow.clone()),
            outbox: Arc::new(outbox.clone()),
            audit: Arc::new(audit.clone()),
            ids: SeqIds::default(),
            clock: FixedClock::default(),
        });
        let actor = actor(1);
        let caller_input_prn = OrganizationId::from_uuid(Uuid::from_u128(1)).canonical();
        let caller_principal_prn = PrincipalId::from_prn(Prn::build("iam", "", None, "principal", Uuid::from_u128(2)).unwrap()).canonical();

        let record = svc.attach(&caller_principal_prn, &caller_input_prn, &actor).await.unwrap();
        assert_ne!(
            record.node_prn, caller_input_prn,
            "sanity: the repo's returned record must differ from the caller's input, or this test proves nothing"
        );
        assert_eq!(uow.commits(), 1);

        let events = outbox.0.lock().unwrap().clone();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, EventType::MembershipAttached);
        assert_eq!(events[0].payload["node_prn"], record.node_prn, "the event's node_prn is the record's, which the repository resolved");
        assert_ne!(events[0].payload["node_prn"], caller_input_prn, "must never equal the caller's raw, differing input");
        // Fix round 1 (Important): `aggregate_prn` is what a subscriber actually routes/filters
        // on — a `payload["node_prn"]`-only assertion misses an `aggregate_prn: node.canonical()`
        // echo bug entirely, since `node` (the service's own locally-parsed value) is exactly
        // what `StoredPrnDiffersRepo` exists to diverge from `record.node_prn`.
        assert_eq!(events[0].aggregate_prn, record.node_prn, "aggregate_prn must be the repository-resolved prn too");
        assert_ne!(events[0].aggregate_prn, caller_input_prn, "aggregate_prn must never equal the caller's raw, differing input");

        let entries = audit.0.lock().unwrap().clone();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].detail["node_prn"], record.node_prn);
        assert_eq!(entries[0].resource_prn.as_deref(), Some(record.node_prn.as_str()));
    }

    /// SMA-606 D6/D5: a cascading org detach emits one event and one entry PER DELETED ROW,
    /// all on one correlation id, so "when did this principal lose access to project X" is
    /// answerable by filtering on that project's PRN. Each cascaded entry is marked
    /// `cascade_of`; the directly authorized row carries no such key — authorization ran once,
    /// at the org node, and an unmarked cascaded entry would read as a separately authorized
    /// `DetachMembership`. Also asserts `commits()`: detach fans out to three rows but must
    /// still commit exactly ONE transaction.
    #[tokio::test]
    async fn a_cascading_detach_emits_one_event_per_deleted_row() {
        let store = TenancyStore::default();
        let now = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        let principal = seed_principal(&store, 510);
        let (org, team) = seed_org_and_team(&store, 511, 512, now);
        let project_id = Uuid::from_u128(513);
        let project = Project::new(
            ProjectId::from_parts(org, project_id),
            TeamId::from_parts(org, team),
            Slug::parse("web").unwrap(),
            "Web",
            &test_stamp(now, 1),
        )
        .unwrap();
        store.projects.lock().unwrap().insert(project_id, project);

        let (svc, outbox, audit, uow) = service_with_fakes(store.clone());
        let actor = actor(1);
        let org_prn = OrganizationId::from_uuid(org).canonical();
        let team_prn = TeamId::from_parts(org, team).canonical();
        let project_prn = ProjectId::from_parts(org, project_id).canonical();

        let org_membership = svc.attach(&principal.canonical(), &org_prn, &actor).await.unwrap();
        svc.attach(&principal.canonical(), &team_prn, &actor).await.unwrap();
        svc.attach(&principal.canonical(), &project_prn, &actor).await.unwrap();
        assert_eq!(uow.commits(), 3, "the three attaches each commit their own transaction");
        outbox.0.lock().unwrap().clear();
        audit.0.lock().unwrap().clear();

        svc.detach(org_membership.id, &actor).await.unwrap();
        assert_eq!(uow.commits(), 4, "detach commits exactly one transaction, even though it fans out to three rows");

        let events = outbox.0.lock().unwrap().clone();
        assert_eq!(events.len(), 3, "the org row plus the two rows its cascade removed");
        assert!(events.iter().all(|e| e.event_type == EventType::MembershipDetached));
        let corr = events[0].correlation_id.unwrap();
        assert!(events.iter().all(|e| e.correlation_id == Some(corr)), "one operation, one correlation id");
        // Fix round 1 (Important): a completeness assertion on `aggregate_prn`, not a
        // `.any(...)` on `payload["node_prn"]` — the latter is satisfied even if all three
        // events wrongly named the same node (e.g. the org), which is exactly the per-row
        // routing failure this fan-out exists to prevent. Collecting the set proves each row
        // got ITS OWN aggregate_prn, not merely that one of the three happened to be right.
        let aggregate_prns: std::collections::HashSet<String> = events.iter().map(|e| e.aggregate_prn.clone()).collect();
        assert_eq!(
            aggregate_prns,
            std::collections::HashSet::from([org_prn.clone(), team_prn.clone(), project_prn.clone()]),
            "each event's aggregate_prn must name its OWN node — one org row event, one team row event, one project row event"
        );

        let entries = audit.0.lock().unwrap().clone();
        assert_eq!(entries.len(), 3);
        assert!(entries.iter().all(|e| e.action == Action::DetachMembership.as_wire()));
        assert!(entries.iter().all(|e| e.correlation_id == Some(corr)));
        assert_eq!(
            entries.iter().filter(|e| e.detail.get("cascade_of").is_some()).count(),
            2,
            "the two cascaded rows are marked; the directly authorized one is not"
        );
        let direct = entries
            .iter()
            .find(|e| e.detail["membership_id"] == org_membership.id.to_string())
            .expect("the directly requested org row has its own entry");
        assert!(direct.detail.get("cascade_of").is_none(), "the directly requested row must carry no cascade_of key");
    }
}
