// SPDX-License-Identifier: Apache-2.0

//! `ServiceAccountService`: service-account lifecycle use cases — create/get/list/archive
//! (SMA-445 Task 16). Every method authorizes BEFORE mutating/reading. The Cedar resource is always the
//! service account's OWNER tenancy node, never the service account's own principal PRN — the
//! embedded schema only lets `CreateServiceAccount`/`GetServiceAccount`/`ListServiceAccounts`/
//! `ArchiveServiceAccount` apply to `[Root, Organization, Team, Project]` (`authz/schema.rs`),
//! and `TenancyNodeRef` itself has no `Root` arm (a service account's owner is always a
//! concrete node, `ck_service_account_owner`), so [`owner_resource_prn`] only ever needs the
//! three node arms.
//!
//! **SMA-646: `create` and `list` confirm the owner PRN against storage.** The caller's
//! `owner` comes from a parsed wire PRN, and `TenancyNodeRef::from_prn` does not check the
//! org-slot value or the region. Both methods call
//! [`TenancyNodes::resolve_and_authorize`](crate::application::tenancy_nodes::TenancyNodes):
//! load the owner node (unknown -> `NotFound`), authorize against its STORED PRN, refuse a
//! differing caller PRN with `PrnMismatch` (one warning line), then use the STORED node for the
//! write, the list filter and the answer. The F1 invariant (a node's `prn` column never changes)
//! makes the compare sound outside the transaction. `get`/`archive` take the SA's own principal
//! PRN and authorize against the stored owner of the SA row, so they need no such check.
//!
//! `archive`'s cache-eviction step (spec §9/D5, "revocation-vs-cache honesty") is the
//! security-critical half of this file: once `set_principal_status` disables the SA, every one
//! of its cached positive API-key validations must stop authenticating immediately —
//! `AuthenticateApiKey` (Task 18) checks `ApiKeyValidationCache::get` before ever touching
//! Postgres, so a stale entry would let a disabled SA's key keep authenticating for up to the
//! cache's TTL. `keys`/`cache` are `Arc<dyn ...>` trait objects (not generic-DI) — the same
//! shared handles a later task's `AppState`/`ApiKeyService`/`AuthenticateApiKey` wiring will
//! reuse, mirroring `RoleService::grants`'s own `Arc<dyn RoleGrantStore>` rationale (module
//! docs there). `repo`/`ids`/`clock` stay generic-DI, mirroring `ProjectService`/`TeamService`.
//!
//! **`entity_gen` bump deferred (Task 16 brief, spec §6/D16).** Every OTHER write that bumps
//! `entity_gen` (org/team/project `set_status`, role grant/revoke, policy CRUD) does so INSIDE
//! its `Pg*` persistence adapter via a `gens: Generations` field threaded through the
//! adapter's own constructor (`PgOrganizationRepository::new(db, gens)`, etc.) — no
//! application service ever imports `crate::adapters::authz::Generations` directly (grepped:
//! zero such imports anywhere under `application/`). `PgServiceAccountRepository` (Task 9,
//! already implemented and covered by `tests/service_accounts.rs`) was built WITHOUT a `gens`
//! field, so bumping `entity_gen` from here would mean either (a) reaching into
//! `crate::adapters::authz::Generations` straight from the application layer — breaking the
//! hexagonal boundary every sibling service respects — or (b) reopening Task 9's already-
//! committed `PgServiceAccountRepository` constructor and its passing persistence-test suite —
//! both outside this task's declared file list (`src/application/service_accounts.rs` +
//! `application/mod.rs`). Deferred to whichever future task wires `ServiceAccountService` into
//! `AppState`, which can thread `gens` into `PgServiceAccountRepository::set_principal_status`
//! exactly like every sibling `Pg*Repository` does. Harmless today: (1) no current Cedar policy
//! reads `principal.status` (brief), and (2) a disabled SA is already blocked twice over
//! without it — this archive's own cache-evict step below, plus Task 18's authentication-time
//! `PrincipalStatus` check.
//!
//! **SMA-446 Slice B Task B7 — the Unit-of-Work reference pattern, OUTBOX-ONLY (copied from
//! `RoleService::grant`/`revoke`, `application::roles`'s module docs, minus the audit/gen-bump
//! halves — principal creation/archive are NOT in the AC audit set, mirrors `application::
//! create_user`'s own B7 posture):** `create` drives the principal+SA insert and its
//! `iam.principal.created` `DomainEvent` through ONE `UnitOfWork`-scoped transaction
//! (`repo.create_in`, `outbox.enqueue`, `tx.commit()`); `archive` drives `set_principal_status_in`
//! plus its `iam.principal.archived` event through another. Neither writes an `AuditEntry` or
//! bumps a generation counter (`ServiceAccountServiceDeps` has no `audit`/`gen_bumper` field). A
//! duplicate-name-per-owner unique-violation inside `create_in` (or any other mid-txn failure)
//! rolls the whole unit of work back before `Outbox::enqueue` is ever reached, so a rejected
//! create emits nothing — the existing `Conflict(ServiceAccountNameTaken)` ->
//! `ServiceAccountNameConflict` mapping is unchanged.
//!
//! **`archive`'s cache-evict moves POST-COMMIT (SECURITY-CRITICAL, unchanged intent, module
//! docs above):** the evict loop (`keys.list_ids_by_service_account` -> `cache.evict` per id)
//! now runs AFTER `tx.commit()` — still AWAITED, so it's guaranteed to have happened by the time
//! `archive` returns — rather than immediately after the (previously untransacted)
//! `set_principal_status` call. This mirrors `ApiKeyService::revoke`'s own B6 move: an evict for
//! a disable that never actually committed (rolled back mid-txn) must never fire, and a disable
//! that DID commit must always evict, unconditionally, once it has.

use crate::adapters::api_keys::ApiKeyValidationCache;
use crate::application::authorize::Authorize;
use crate::application::error::TenancyError;
use crate::application::pagination::Page;
use crate::application::tenancy_nodes::TenancyNodes;
use paigasus_iam_core::{
    Action, ApiKeyRepository, Clock, DomainEvent, EventType, IdGenerator, Outbox, Principal, PrincipalId, PrincipalKind, PrincipalStatus, ServiceAccount, ServiceAccountRecord,
    ServiceAccountRepository, TenancyNodeRef, UnitOfWork,
};
use paigasus_kernel::Prn;
use std::sync::Arc;

/// The `Prn` a service account's owner node represents as an authorization *resource* —
/// mirrors `roles.rs::scope_resource_prn`'s node arms exactly (no `Root` arm: a service
/// account's owner is always a concrete tenancy node, `ck_service_account_owner`).
fn owner_resource_prn(owner: &TenancyNodeRef) -> Prn {
    match owner {
        TenancyNodeRef::Organization(id) => id.prn().clone(),
        TenancyNodeRef::Team(id) => id.prn().clone(),
        TenancyNodeRef::Project(id) => id.prn().clone(),
    }
}

/// Service-account lifecycle use cases. `keys`/`cache` are shared `Arc<dyn ...>` handles
/// (module docs); `uow`/`outbox` are SMA-446 Slice B Task B7's Unit-of-Work reference pattern
/// (module docs): `create`/`archive` drive their mutation + outbox event through `uow`
/// atomically. `repo`/`ids`/`clock` stay generic-DI.
#[derive(Clone)]
pub struct ServiceAccountService<R, I, C> {
    repo: R,
    keys: Arc<dyn ApiKeyRepository>,
    cache: Arc<dyn ApiKeyValidationCache>,
    authorize: Authorize,
    nodes: TenancyNodes,
    uow: Arc<dyn UnitOfWork>,
    outbox: Arc<dyn Outbox>,
    ids: I,
    clock: C,
}

/// Named-field constructor params for [`ServiceAccountService::new`] (SMA-446 Slice B Task
/// B7) — copies `application::roles::RoleServiceDeps`'s DI-params idiom (module docs there):
/// one field per dependency, built with struct syntax at the call site so each argument is
/// self-labeling. Deliberately has NO `audit`/`gen_bumper` field — module docs: principal
/// creation/archive are OUTBOX-ONLY, not in the AC audit set.
pub struct ServiceAccountServiceDeps<R, I, C> {
    pub repo: R,
    pub keys: Arc<dyn ApiKeyRepository>,
    pub cache: Arc<dyn ApiKeyValidationCache>,
    pub authorize: Authorize,
    /// SMA-646: loads the owner node for the stored-PRN check of `create`/`list`.
    pub nodes: TenancyNodes,
    pub uow: Arc<dyn UnitOfWork>,
    pub outbox: Arc<dyn Outbox>,
    pub ids: I,
    pub clock: C,
}

impl<R, I, C> ServiceAccountService<R, I, C>
where
    R: ServiceAccountRepository,
    I: IdGenerator,
    C: Clock,
{
    pub fn new(deps: ServiceAccountServiceDeps<R, I, C>) -> Self {
        Self {
            repo: deps.repo,
            keys: deps.keys,
            cache: deps.cache,
            authorize: deps.authorize,
            nodes: deps.nodes,
            uow: deps.uow,
            outbox: deps.outbox,
            ids: deps.ids,
            clock: deps.clock,
        }
    }

    /// Creates a service account owned by `owner`. SMA-646 order: load `owner`'s node
    /// (`NotFound` if absent), authorize `Action::CreateServiceAccount` against its STORED PRN
    /// BEFORE minting anything, then refuse a caller PRN that differs from the stored one
    /// (`PrnMismatch`, module docs). Authorize before compare keeps a mismatch from telling an
    /// ungranted caller which organization owns the node. `name` validation
    /// (`ServiceAccount::new`) runs after that, so a forged owner outranks an invalid name
    /// (SMA-645 B2). The record carries the STORED owner. The returned `status` is `Active`
    /// WITHOUT a re-query — a freshly created SA's principal is minted `Active` right above.
    /// SMA-446 Slice B Task B7 (module docs — OUTBOX-ONLY): the principal+SA insert and its
    /// `iam.principal.created` event share ONE UoW transaction; a duplicate-name-per-owner
    /// unique-violation inside `create_in` rolls the whole unit of work back before the event is
    /// ever enqueued.
    pub async fn create(&self, actor: &Prn, owner: TenancyNodeRef, name: &str) -> Result<ServiceAccountRecord, TenancyError> {
        let owner = self
            .nodes
            .resolve_and_authorize(&self.authorize, actor, Action::CreateServiceAccount, &owner, "CreateServiceAccount")
            .await?;

        let id = self.ids.new_service_account_id();
        let now = self.clock.now();
        let principal = Principal::new(id.clone(), PrincipalKind::ServiceAccount, PrincipalStatus::Active, now, now);
        let sa = ServiceAccount::new(id, owner, name, now)?;

        let event = DomainEvent {
            id: self.ids.new_event_id(),
            event_type: EventType::PrincipalCreated,
            schema_version: 1,
            aggregate_prn: sa.principal_id.canonical(),
            actor_prn: Some(actor.canonical()),
            occurred_at: now,
            payload: serde_json::json!({"principal_id": sa.principal_id.uuid(), "kind": "service_account", "name": sa.name}),
            correlation_id: Some(self.ids.new_correlation_id()),
        };

        let tx = self.uow.begin().await?;
        self.repo.create_in(&*tx, &principal, &sa).await?;
        self.outbox.enqueue(&*tx, &event).await?;
        tx.commit().await?;

        Ok(ServiceAccountRecord {
            account: sa,
            status: PrincipalStatus::Active,
        })
    }

    /// Fetches a service account by principal id. `NotFound` if absent — checked BEFORE
    /// authorization since the resource to authorize against (the SA's owner node) can only be
    /// learned by reading the row first (mirrors `archive`'s own order, brief).
    pub async fn get(&self, actor: &Prn, id: &PrincipalId) -> Result<ServiceAccountRecord, TenancyError> {
        let record = self.repo.find(id).await?.ok_or(TenancyError::NotFound)?;
        self.authorize.check(actor, Action::GetServiceAccount, &owner_resource_prn(&record.account.owner)).await?;
        Ok(record)
    }

    /// Lists service accounts owned by `owner`, `ORDER BY created_at, id` (rule 9, delegated to
    /// the repo). SMA-646: loads `owner`'s node (`NotFound` if absent), authorizes
    /// `Action::ListServiceAccounts` against its STORED PRN, refuses a differing caller PRN
    /// (`PrnMismatch`), and filters on the STORED node. Before SMA-646 a forged owner PRN
    /// listed the real owner's accounts, each labelled with the forged PRN. The `Page` is built
    /// in the transport, so `InvalidPagination` still outranks a forged owner (spec B7).
    pub async fn list(&self, actor: &Prn, owner: &TenancyNodeRef, page: Page) -> Result<Vec<ServiceAccountRecord>, TenancyError> {
        let owner = self
            .nodes
            .resolve_and_authorize(&self.authorize, actor, Action::ListServiceAccounts, owner, "ListServiceAccounts")
            .await?;
        Ok(self.repo.list_by_owner(&owner, page.limit, page.offset).await?)
    }

    /// Archives (disables) a service account: `NotFound` if it doesn't exist; authorizes
    /// `Action::ArchiveServiceAccount` AT its OWNER node (found via the lookup above, since the
    /// caller only supplies the SA's own id); disables the underlying `Principal` (D16: status
    /// lives there, not on `ServiceAccount`) and its `iam.principal.archived` event atomically
    /// (SMA-446 Slice B Task B7, module docs — OUTBOX-ONLY); then, POST-COMMIT and AWAITED,
    /// evicts every one of the SA's cached API-key validations (`keys.list_ids_by_service_account`
    /// -> `cache.evict` per id) — the SECURITY-CRITICAL step (module docs): without it, a
    /// disabled SA's already-cached keys would keep authenticating until their cache entries
    /// expire on their own. The evict is only ever reached once `tx.commit()` above has actually
    /// succeeded — a rolled-back mid-txn failure must never evict a cache entry for a disable
    /// that never actually happened.
    pub async fn archive(&self, actor: &Prn, id: &PrincipalId) -> Result<(), TenancyError> {
        let record = self.repo.find(id).await?.ok_or(TenancyError::NotFound)?;
        self.authorize.check(actor, Action::ArchiveServiceAccount, &owner_resource_prn(&record.account.owner)).await?;

        let now = self.clock.now();
        let event = DomainEvent {
            id: self.ids.new_event_id(),
            event_type: EventType::PrincipalArchived,
            schema_version: 1,
            aggregate_prn: id.canonical(),
            actor_prn: Some(actor.canonical()),
            occurred_at: now,
            payload: serde_json::json!({"principal_id": id.uuid(), "kind": "service_account"}),
            correlation_id: Some(self.ids.new_correlation_id()),
        };

        let tx = self.uow.begin().await?;
        self.repo.set_principal_status_in(&*tx, id, PrincipalStatus::Disabled).await?;
        self.outbox.enqueue(&*tx, &event).await?;
        tx.commit().await?;

        // POST-COMMIT, AWAITED (module docs, SECURITY-CRITICAL): only reachable once the
        // transaction above has actually committed — never for a rolled-back archive.
        let key_ids = self.keys.list_ids_by_service_account(id).await?;
        for key_id in key_ids {
            self.cache.evict(key_id).await;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::api_keys::{CachedValidation, MemoryApiKeyCache};
    use crate::application::fakes::{
        FakeAuthorizer, FakeOutbox, FakeUnitOfWork, FixedClock, InMemoryApiKeys, InMemoryServiceAccounts, SeqIds, TenancyStore, forged_variants, seed_org, seed_project, seed_team, store_with_orgs,
        tenancy_nodes,
    };
    use crate::log_capture::capture_logs;
    use async_trait::async_trait;
    use chrono::{TimeZone, Utc};
    use paigasus_iam_core::{ApiKey, ApiKeyId, ApiKeyStatus, OrganizationId, ProjectId, RepositoryError, TeamId, Transaction};
    use uuid::Uuid;

    fn actor_prn(n: u128) -> Prn {
        Prn::build("iam", "", None, "principal", Uuid::from_u128(n)).unwrap()
    }

    fn owner_org(n: u128) -> TenancyNodeRef {
        TenancyNodeRef::Organization(OrganizationId::from_uuid(Uuid::from_u128(n)))
    }

    fn missing_id(n: u128) -> PrincipalId {
        PrincipalId::from_prn(Prn::build("iam", "", None, "principal", Uuid::from_u128(n)).unwrap())
    }

    /// SMA-646: seeds org 100 -> team 101 -> project 102 into `store`.
    fn seed_chain(store: &TenancyStore) -> (OrganizationId, TeamId, ProjectId) {
        let org = seed_org(store, 100);
        let team = seed_team(store, &org, 101);
        let project = seed_project(store, &team, 102);
        (org, team, project)
    }

    /// SMA-646: the three STORED nodes of `seed_chain` as owners.
    fn stored_owners(org: &OrganizationId, team: &TeamId, project: &ProjectId) -> [TenancyNodeRef; 3] {
        [TenancyNodeRef::Organization(org.clone()), TenancyNodeRef::Team(team.clone()), TenancyNodeRef::Project(project.clone())]
    }

    /// Bundles a `ServiceAccountService` together with the SMA-446 Slice B Task B7 fakes it was
    /// built over, so a test can assert on exactly what — and how many — events `create`/
    /// `archive` emitted (mirrors `application::roles::tests::ServiceWithFakes`).
    struct ServiceWithFakes {
        svc: ServiceAccountService<InMemoryServiceAccounts, SeqIds, FixedClock>,
        repo: InMemoryServiceAccounts,
        keys: Arc<InMemoryApiKeys>,
        cache: Arc<MemoryApiKeyCache>,
        outbox: FakeOutbox,
        store: TenancyStore,
    }

    /// Builds a service over fresh, empty backing stores (including a fresh, unshared
    /// `FakeUnitOfWork`/`FakeOutbox`, SMA-446 Slice B Task B7).
    fn new_service_with_fakes(fake: FakeAuthorizer) -> ServiceWithFakes {
        let repo = InMemoryServiceAccounts::default();
        let keys = Arc::new(InMemoryApiKeys::default());
        let cache = Arc::new(MemoryApiKeyCache::new(30));
        let outbox = FakeOutbox::default();
        // SMA-646: `create`/`list` now load the owner node. Every `owner_org(n)` with n in
        // 1..=32 names a STORED organization, so the pre-SMA-646 tests keep their meaning.
        // `owner_org(404)` stays unseeded for the not-found test.
        let store = store_with_orgs(1..=32);
        let svc = ServiceAccountService::new(ServiceAccountServiceDeps {
            repo: repo.clone(),
            keys: keys.clone(),
            cache: cache.clone(),
            authorize: Authorize::new(Arc::new(fake)),
            nodes: tenancy_nodes(&store),
            uow: Arc::new(FakeUnitOfWork::default()),
            outbox: Arc::new(outbox.clone()),
            ids: SeqIds::default(),
            clock: FixedClock::default(),
        });
        ServiceWithFakes {
            svc,
            repo,
            keys,
            cache,
            outbox,
            store,
        }
    }

    /// Builds a service over fresh, empty backing stores. Returns the `InMemoryServiceAccounts`
    /// repo and the `keys`/`cache` handles alongside the service itself so tests can inspect
    /// state the service's own read methods don't ALWAYS need a second call for (e.g. re-`find`ing
    /// the repo fake directly instead of a second, differently-authorized service instance) —
    /// `get`/`list`/`create` do now surface the principal's lifecycle status via
    /// `ServiceAccountRecord` (D16: still read from `Principal`, never stored on
    /// `ServiceAccount` itself).
    #[allow(clippy::type_complexity)]
    fn new_service(
        fake: FakeAuthorizer,
    ) -> (
        ServiceAccountService<InMemoryServiceAccounts, SeqIds, FixedClock>,
        InMemoryServiceAccounts,
        Arc<InMemoryApiKeys>,
        Arc<MemoryApiKeyCache>,
    ) {
        let ServiceWithFakes { svc, repo, keys, cache, .. } = new_service_with_fakes(fake);
        (svc, repo, keys, cache)
    }

    /// A `ServiceAccountRepository` whose `set_principal_status_in` always fails — simulates a
    /// store error mid-txn (mirrors `application::api_keys::tests::FailingRevokeApiKeys`):
    /// `ServiceAccountService::archive` must roll back before ever touching the outbox, and —
    /// the SECURITY-CRITICAL part this task adds — must NEVER evict the SA's cached key
    /// validations for an archive that never actually committed. `create`/`find`/etc. all
    /// delegate to a real backing `InMemoryServiceAccounts` so a test can seed/read normally;
    /// only `set_principal_status_in` is overridden.
    #[derive(Clone, Default)]
    struct FailingArchiveServiceAccounts(InMemoryServiceAccounts);

    #[async_trait]
    impl ServiceAccountRepository for FailingArchiveServiceAccounts {
        async fn create(&self, principal: &Principal, sa: &ServiceAccount) -> Result<(), RepositoryError> {
            self.0.create(principal, sa).await
        }
        async fn create_in(&self, tx: &dyn Transaction, principal: &Principal, sa: &ServiceAccount) -> Result<(), RepositoryError> {
            self.0.create_in(tx, principal, sa).await
        }
        async fn find(&self, id: &PrincipalId) -> Result<Option<ServiceAccountRecord>, RepositoryError> {
            self.0.find(id).await
        }
        async fn list_by_owner(&self, owner: &TenancyNodeRef, limit: u64, offset: u64) -> Result<Vec<ServiceAccountRecord>, RepositoryError> {
            self.0.list_by_owner(owner, limit, offset).await
        }
        async fn set_principal_status(&self, id: &PrincipalId, status: PrincipalStatus) -> Result<(), RepositoryError> {
            self.0.set_principal_status(id, status).await
        }
        async fn set_principal_status_in(&self, _tx: &dyn Transaction, _id: &PrincipalId, _status: PrincipalStatus) -> Result<(), RepositoryError> {
            Err(RepositoryError::Backend(Box::new(std::io::Error::other("simulated mid-txn store failure"))))
        }
    }

    #[tokio::test]
    async fn create_authorizes_and_persists() {
        let owner = owner_org(1);
        let fake = FakeAuthorizer::default();
        fake.allow(Action::CreateServiceAccount, &owner_resource_prn(&owner));
        fake.allow(Action::GetServiceAccount, &owner_resource_prn(&owner));
        let (svc, ..) = new_service(fake);
        let actor = actor_prn(1);

        let sa = svc.create(&actor, owner.clone(), "ci-bot").await.unwrap();
        assert_eq!(sa.account.name, "ci-bot");
        assert_eq!(sa.account.owner, owner);
        assert_eq!(sa.status, PrincipalStatus::Active, "a freshly created SA's principal is Active (D16)");

        let got = svc.get(&actor, &sa.account.principal_id).await.unwrap();
        assert_eq!(got, sa);
    }

    #[tokio::test]
    async fn create_denied_without_authz() {
        // The always-deny-by-default fake never allows `CreateServiceAccount` — the create
        // must be rejected `Forbidden`, and nothing may land in the repo.
        let (svc, repo, ..) = new_service(FakeAuthorizer::default());
        let actor = actor_prn(1);
        let owner = owner_org(2);

        let err = svc.create(&actor, owner, "ci-bot").await.unwrap_err();
        assert_eq!(err, TenancyError::Forbidden);
        assert!(repo.accounts.lock().unwrap().is_empty(), "a denied create must not persist anything");
    }

    #[tokio::test]
    async fn create_duplicate_name_is_conflict() {
        let owner = owner_org(3);
        let fake = FakeAuthorizer::default();
        fake.allow(Action::CreateServiceAccount, &owner_resource_prn(&owner));
        let ServiceWithFakes { svc, outbox, .. } = new_service_with_fakes(fake);
        let actor = actor_prn(1);

        svc.create(&actor, owner.clone(), "dup").await.unwrap();
        assert_eq!(outbox.0.lock().unwrap().len(), 1, "sanity: the first create enqueued its own event");

        let err = svc.create(&actor, owner, "dup").await.unwrap_err();
        assert_eq!(err, TenancyError::ServiceAccountNameConflict, "must surface as a 409 Conflict, not Internal");

        // SMA-446 Slice B Task B7: the rolled-back second create must not enqueue a second
        // event — the unique-violation inside `create_in` rolls the whole unit of work back
        // before `Outbox::enqueue` is ever reached.
        assert_eq!(outbox.0.lock().unwrap().len(), 1, "a rejected duplicate-name create must not enqueue an event");
    }

    /// SMA-446 Slice B Task B7 — the UoW reference pattern's core contract for `create`:
    /// enqueues exactly one `iam.principal.created` `DomainEvent`, with a payload carrying
    /// `principal_id`/`kind`/`name`.
    #[tokio::test]
    async fn create_emits_one_principal_created_event() {
        let owner = owner_org(20);
        let fake = FakeAuthorizer::default();
        fake.allow(Action::CreateServiceAccount, &owner_resource_prn(&owner));
        let ServiceWithFakes { svc, outbox, .. } = new_service_with_fakes(fake);
        let actor = actor_prn(1);

        let sa = svc.create(&actor, owner, "ci-bot").await.unwrap();

        let events = outbox.0.lock().unwrap();
        assert_eq!(events.len(), 1, "create must enqueue exactly one domain event");
        assert_eq!(events[0].event_type, EventType::PrincipalCreated);
        assert_eq!(events[0].aggregate_prn, sa.account.principal_id.canonical());
        assert_eq!(events[0].actor_prn, Some(actor.canonical()));
        assert_eq!(events[0].payload["principal_id"], serde_json::json!(sa.account.principal_id.uuid()));
        assert_eq!(events[0].payload["kind"], serde_json::json!("service_account"));
        assert_eq!(events[0].payload["name"], serde_json::json!("ci-bot"));
    }

    #[tokio::test]
    async fn archive_disables_and_evicts_keys() {
        let owner = owner_org(4);
        let fake = FakeAuthorizer::default();
        fake.allow(Action::CreateServiceAccount, &owner_resource_prn(&owner));
        fake.allow(Action::ArchiveServiceAccount, &owner_resource_prn(&owner));
        let ServiceWithFakes { svc, repo, keys, cache, outbox, .. } = new_service_with_fakes(fake);
        let actor = actor_prn(1);

        let sa = svc.create(&actor, owner.clone(), "ci-bot").await.unwrap();

        // Seed a key issued to this SA directly into the fake repo + a cached validation for
        // it, so `archive` has something real to enumerate and evict.
        let key_id = ApiKeyId::from_uuid(Uuid::from_u128(500));
        let key = ApiKey {
            id: key_id,
            service_account_id: sa.account.principal_id.clone(),
            scope: owner.clone(),
            prefix: "pgs_sk_test".to_string(),
            status: ApiKeyStatus::Active,
            expires_at: None,
            last_used_at: None,
            created_at: Utc.timestamp_opt(0, 0).unwrap(),
            revoked_at: None,
            scope_actions: Vec::new(),
            scope_roles: Vec::new(),
        };
        keys.issue(&key, b"hash").await.unwrap();
        cache
            .put(
                key_id,
                &CachedValidation {
                    principal_id: sa.account.principal_id.clone(),
                    sa_status: PrincipalStatus::Active,
                    expires_at: None,
                    key_hash: b"hash".to_vec(),
                    scope_prn: key.scope.canonical(),
                },
            )
            .await;
        assert!(cache.get(key_id).await.is_some(), "sanity: the cache entry exists before archive");

        svc.archive(&actor, &sa.account.principal_id).await.unwrap();

        // The cached validation for the SA's key must be gone (the security-critical part).
        assert!(cache.get(key_id).await.is_none(), "archive must evict every cached key of the archived SA");

        // The underlying principal status is disabled — D16: status lives on `Principal`, not
        // `ServiceAccount`, so this is only observable via the repo fake's own `statuses` map.
        assert_eq!(repo.statuses.lock().unwrap().get(&sa.account.principal_id.uuid()), Some(&PrincipalStatus::Disabled));

        // The read path itself agrees: `find`'s returned `status` reflects the archive too.
        let after = repo.find(&sa.account.principal_id).await.unwrap().expect("row present");
        assert_eq!(after.status, PrincipalStatus::Disabled);

        // SMA-446 Slice B Task B7: `archive` enqueued its own `iam.principal.archived` event
        // (index 1 — index 0 is `create`'s own `iam.principal.created`).
        let events = outbox.0.lock().unwrap();
        assert_eq!(events.len(), 2, "create + archive must each enqueue exactly one event");
        assert_eq!(events[1].event_type, EventType::PrincipalArchived);
        assert_eq!(events[1].aggregate_prn, sa.account.principal_id.canonical());
        assert_eq!(events[1].actor_prn, Some(actor.canonical()));
        assert_eq!(events[1].payload["principal_id"], serde_json::json!(sa.account.principal_id.uuid()));
    }

    /// SMA-446 Slice B Task B7, SECURITY-CRITICAL: an archive whose mutation rolls back
    /// mid-txn (here, `set_principal_status_in` failing, guard D2's analogue) must NEVER evict
    /// the SA's cached key validations — mirrors `application::api_keys::tests::
    /// revoke_never_evicts_cache_when_the_mutation_rolls_back`.
    #[tokio::test]
    async fn archive_never_evicts_the_cache_when_the_mutation_rolls_back() {
        let owner = owner_org(7);
        let fake = FakeAuthorizer::default();
        fake.allow(Action::CreateServiceAccount, &owner_resource_prn(&owner));
        fake.allow(Action::ArchiveServiceAccount, &owner_resource_prn(&owner));

        let failing_repo = FailingArchiveServiceAccounts::default();
        let keys = Arc::new(InMemoryApiKeys::default());
        let cache = Arc::new(MemoryApiKeyCache::new(30));
        let svc = ServiceAccountService::new(ServiceAccountServiceDeps {
            repo: failing_repo,
            keys: keys.clone(),
            cache: cache.clone(),
            authorize: Authorize::new(Arc::new(fake)),
            nodes: tenancy_nodes(&store_with_orgs([7])),
            uow: Arc::new(FakeUnitOfWork::default()),
            outbox: Arc::new(FakeOutbox::default()),
            ids: SeqIds::default(),
            clock: FixedClock::default(),
        });
        let actor = actor_prn(1);

        let sa = svc.create(&actor, owner.clone(), "ci-bot").await.unwrap();

        let key_id = ApiKeyId::from_uuid(Uuid::from_u128(700));
        let key = ApiKey {
            id: key_id,
            service_account_id: sa.account.principal_id.clone(),
            scope: owner,
            prefix: "pgs_sk_test".to_string(),
            status: ApiKeyStatus::Active,
            expires_at: None,
            last_used_at: None,
            created_at: Utc.timestamp_opt(0, 0).unwrap(),
            revoked_at: None,
            scope_actions: Vec::new(),
            scope_roles: Vec::new(),
        };
        keys.issue(&key, b"hash").await.unwrap();
        cache
            .put(
                key_id,
                &CachedValidation {
                    principal_id: sa.account.principal_id.clone(),
                    sa_status: PrincipalStatus::Active,
                    expires_at: None,
                    key_hash: b"hash".to_vec(),
                    scope_prn: key.scope.canonical(),
                },
            )
            .await;
        assert!(cache.get(key_id).await.is_some(), "sanity: the cache entry exists before the failed archive");

        let err = svc.archive(&actor, &sa.account.principal_id).await.unwrap_err();
        assert_eq!(err, TenancyError::Internal, "a Backend error from a mid-txn store failure maps to Internal");

        assert!(cache.get(key_id).await.is_some(), "a rolled-back archive must NOT evict the cache — SECURITY-CRITICAL");
    }

    #[tokio::test]
    async fn archive_denied_without_authz_then_succeeds_once_authorized() {
        let owner = owner_org(5);
        let fake = FakeAuthorizer::default();
        fake.allow(Action::CreateServiceAccount, &owner_resource_prn(&owner));
        let (svc, repo, ..) = new_service(fake.clone());
        let actor = actor_prn(1);
        let sa = svc.create(&actor, owner.clone(), "ci-bot").await.unwrap();

        // `ArchiveServiceAccount` was never allowed — must deny, and leave the principal Active.
        assert_eq!(svc.archive(&actor, &sa.account.principal_id).await.unwrap_err(), TenancyError::Forbidden);
        assert_eq!(repo.statuses.lock().unwrap().get(&sa.account.principal_id.uuid()), Some(&PrincipalStatus::Active));

        fake.allow(Action::ArchiveServiceAccount, &owner_resource_prn(&owner));
        svc.archive(&actor, &sa.account.principal_id).await.unwrap();
        assert_eq!(repo.statuses.lock().unwrap().get(&sa.account.principal_id.uuid()), Some(&PrincipalStatus::Disabled));
    }

    #[tokio::test]
    async fn archive_missing_service_account_is_not_found() {
        let (svc, ..) = new_service(FakeAuthorizer::default());
        assert_eq!(svc.archive(&actor_prn(1), &missing_id(999)).await.unwrap_err(), TenancyError::NotFound);
    }

    #[tokio::test]
    async fn list_authorizes_at_owner_and_scopes_results() {
        let owner_a = owner_org(10);
        let owner_b = owner_org(11);
        let fake = FakeAuthorizer::default();
        fake.allow(Action::CreateServiceAccount, &owner_resource_prn(&owner_a));
        fake.allow(Action::CreateServiceAccount, &owner_resource_prn(&owner_b));
        fake.allow(Action::ListServiceAccounts, &owner_resource_prn(&owner_a));
        let (svc, ..) = new_service(fake);
        let actor = actor_prn(1);

        svc.create(&actor, owner_a.clone(), "a-one").await.unwrap();
        svc.create(&actor, owner_b.clone(), "b-one").await.unwrap();

        let listed = svc.list(&actor, &owner_a, Page::new(None, None).unwrap()).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].account.name, "a-one");
        assert_eq!(listed[0].status, PrincipalStatus::Active);

        // Listing owner_b was never authorized -> Forbidden.
        assert_eq!(svc.list(&actor, &owner_b, Page::new(None, None).unwrap()).await.unwrap_err(), TenancyError::Forbidden);
    }

    #[tokio::test]
    async fn get_denied_without_authz() {
        let owner = owner_org(6);
        let fake = FakeAuthorizer::default();
        fake.allow(Action::CreateServiceAccount, &owner_resource_prn(&owner));
        // `GetServiceAccount` never allowed.
        let (svc, ..) = new_service(fake);
        let actor = actor_prn(1);
        let sa = svc.create(&actor, owner, "ci-bot").await.unwrap();

        assert_eq!(svc.get(&actor, &sa.account.principal_id).await.unwrap_err(), TenancyError::Forbidden);
    }

    #[tokio::test]
    async fn get_missing_is_not_found() {
        let (svc, ..) = new_service(FakeAuthorizer::default());
        assert_eq!(svc.get(&actor_prn(1), &missing_id(404)).await.unwrap_err(), TenancyError::NotFound);
    }

    /// SMA-646 U1 (B1, AC 1): a forged org slot or region on an EXISTING owner answers
    /// `PrnMismatch`. Nothing is written and no event is enqueued.
    #[tokio::test]
    async fn create_refuses_a_forged_owner() {
        let fake = FakeAuthorizer::default();
        let ServiceWithFakes { svc, repo, outbox, store, .. } = new_service_with_fakes(fake.clone());
        let (org, team, project) = seed_chain(&store);
        for node in stored_owners(&org, &team, &project) {
            fake.allow(Action::CreateServiceAccount, &owner_resource_prn(&node));
        }
        let actor = actor_prn(1);

        for (label, forged) in forged_variants(&org, &team, &project) {
            let err = svc.create(&actor, forged, "ci-bot").await.unwrap_err();
            assert_eq!(err, TenancyError::PrnMismatch, "{label}");
        }
        assert!(repo.accounts.lock().unwrap().is_empty(), "a refused create must not persist anything");
        assert!(outbox.0.lock().unwrap().is_empty(), "a refused create must not enqueue an event");
    }

    /// SMA-646 U2 (AC 3): the correct PRN succeeds and the record carries the STORED owner.
    #[tokio::test]
    async fn create_returns_the_stored_owner() {
        let fake = FakeAuthorizer::default();
        let ServiceWithFakes { svc, store, .. } = new_service_with_fakes(fake.clone());
        let (org, team, project) = seed_chain(&store);
        let actor = actor_prn(1);
        for (i, node) in stored_owners(&org, &team, &project).into_iter().enumerate() {
            fake.allow(Action::CreateServiceAccount, &owner_resource_prn(&node));
            let record = svc.create(&actor, node.clone(), &format!("bot-{i}")).await.unwrap();
            assert_eq!(record.account.owner.canonical(), node.canonical());
        }
    }

    /// SMA-646 U3 (B2, AC 2): a forged owner on List answers `PrnMismatch`.
    #[tokio::test]
    async fn list_refuses_a_forged_owner() {
        let fake = FakeAuthorizer::default();
        let ServiceWithFakes { svc, store, .. } = new_service_with_fakes(fake.clone());
        let (org, team, project) = seed_chain(&store);
        for node in stored_owners(&org, &team, &project) {
            fake.allow(Action::ListServiceAccounts, &owner_resource_prn(&node));
        }
        let actor = actor_prn(1);

        for (label, forged) in forged_variants(&org, &team, &project) {
            let err = svc.list(&actor, &forged, Page::new(None, None).unwrap()).await.unwrap_err();
            assert_eq!(err, TenancyError::PrnMismatch, "{label}");
        }
    }

    /// SMA-646 U4: the correct PRN lists the owner's accounts, each with the STORED owner.
    #[tokio::test]
    async fn list_returns_accounts_for_the_correct_owner() {
        let fake = FakeAuthorizer::default();
        let ServiceWithFakes { svc, store, .. } = new_service_with_fakes(fake.clone());
        let (_org, team, _project) = seed_chain(&store);
        let owner = TenancyNodeRef::Team(team.clone());
        fake.allow(Action::CreateServiceAccount, &owner_resource_prn(&owner));
        fake.allow(Action::ListServiceAccounts, &owner_resource_prn(&owner));
        let actor = actor_prn(1);
        svc.create(&actor, owner.clone(), "one").await.unwrap();
        svc.create(&actor, owner.clone(), "two").await.unwrap();

        let listed = svc.list(&actor, &owner, Page::new(None, None).unwrap()).await.unwrap();
        assert_eq!(listed.len(), 2);
        for record in &listed {
            assert_eq!(record.account.owner.canonical(), team.canonical());
        }
    }

    /// SMA-646 U5 (B6): with a default-deny authorizer, Create AND List answer `Forbidden` for a
    /// forged and a correct PRN alike. This proves the ORDER (authorize before compare) only;
    /// the "alike" property for real decisions rests on Cedar (I1).
    #[tokio::test]
    async fn an_ungranted_caller_cannot_tell_a_forged_owner_from_a_correct_one() {
        let ServiceWithFakes { svc, store, .. } = new_service_with_fakes(FakeAuthorizer::default());
        let (_org, team, _project) = seed_chain(&store);
        let correct = TenancyNodeRef::Team(team.clone());
        let forged = TenancyNodeRef::Team(TeamId::from_parts(Uuid::from_u128(0xF0F0), team.uuid()));
        let actor = actor_prn(1);

        for (label, node) in [("correct", correct), ("forged", forged)] {
            assert_eq!(svc.create(&actor, node.clone(), "ci-bot").await.unwrap_err(), TenancyError::Forbidden, "create {label}");
            assert_eq!(svc.list(&actor, &node, Page::new(None, None).unwrap()).await.unwrap_err(), TenancyError::Forbidden, "list {label}");
        }
    }

    /// SMA-646 U6 (B3, AC 5): a forged owner outranks an invalid name. The control (correct PRN,
    /// same name) answers the name error, so this test cannot pass vacuously.
    #[tokio::test]
    async fn a_forged_owner_outranks_an_invalid_name() {
        let fake = FakeAuthorizer::default();
        let ServiceWithFakes { svc, store, .. } = new_service_with_fakes(fake.clone());
        let (_org, team, _project) = seed_chain(&store);
        let correct = TenancyNodeRef::Team(team.clone());
        fake.allow(Action::CreateServiceAccount, &owner_resource_prn(&correct));
        let forged = TenancyNodeRef::Team(TeamId::from_parts(Uuid::from_u128(0xF0F0), team.uuid()));
        let actor = actor_prn(1);

        assert_eq!(svc.create(&actor, forged, "   ").await.unwrap_err(), TenancyError::PrnMismatch);
        assert!(matches!(svc.create(&actor, correct, "   ").await.unwrap_err(), TenancyError::InvalidName(_)));
    }

    /// SMA-646 U7 (B5, AC 7): an unknown owner uuid answers `NotFound` on Create and List, also
    /// for a default-deny authorizer (the load runs first).
    #[tokio::test]
    async fn an_unknown_owner_is_not_found() {
        let (svc, ..) = new_service(FakeAuthorizer::default());
        let actor = actor_prn(1);
        let unknown_team = TenancyNodeRef::Team(TeamId::from_parts(Uuid::from_u128(1), Uuid::from_u128(4040)));

        for node in [owner_org(404), unknown_team] {
            assert_eq!(svc.create(&actor, node.clone(), "ci-bot").await.unwrap_err(), TenancyError::NotFound);
            assert_eq!(svc.list(&actor, &node, Page::new(None, None).unwrap()).await.unwrap_err(), TenancyError::NotFound);
        }
    }

    /// SMA-646 U8 (AC 8): each refused mismatch writes exactly one warning line, with the RPC,
    /// the requested PRN and the stored PRN.
    #[tokio::test]
    async fn a_forged_owner_logs_one_warning() {
        let (logs, _guard) = capture_logs();
        let fake = FakeAuthorizer::default();
        let ServiceWithFakes { svc, store, .. } = new_service_with_fakes(fake.clone());
        let (_org, team, _project) = seed_chain(&store);
        let correct = TenancyNodeRef::Team(team.clone());
        fake.allow(Action::CreateServiceAccount, &owner_resource_prn(&correct));
        fake.allow(Action::ListServiceAccounts, &owner_resource_prn(&correct));
        let forged = TenancyNodeRef::Team(TeamId::from_parts(Uuid::from_u128(0xF0F0), team.uuid()));
        let actor = actor_prn(1);

        let _ = svc.create(&actor, forged.clone(), "ci-bot").await.unwrap_err();
        let _ = svc.list(&actor, &forged, Page::new(None, None).unwrap()).await.unwrap_err();

        let text = logs.text();
        assert_eq!(text.matches("requested_prn=").count(), 2, "one warning per refused call: {text}");
        assert_eq!(text.matches("rpc=CreateServiceAccount").count(), 1, "{text}");
        assert_eq!(text.matches("rpc=ListServiceAccounts").count(), 1, "{text}");
        assert!(text.contains(&format!("requested_prn={}", forged.canonical())), "{text}");
        assert!(text.contains(&format!("stored_prn={}", correct.canonical())), "{text}");
        assert!(text.contains("WARN"), "{text}");
    }
}
