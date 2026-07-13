// SPDX-License-Identifier: Apache-2.0

//! `ApiKeyService`: API-key lifecycle use cases — issue/revoke/list (SMA-445 Task 17). Mirrors
//! `RoleService`/`ServiceAccountService`'s DI + authorize pattern (`application/roles.rs:78-204`,
//! `application/service_accounts.rs:63-143`): every method authorizes BEFORE mutating/reading.
//!
//! **D15 anti-escalation (THE security-critical invariant of this file, spec D15):** an API key
//! is a bearer credential for its service account's ENTIRE current grant set — whoever holds the
//! plaintext token wields every `RoleGrant` the SA has at the moment of use (`AuthenticateApiKey`,
//! Task 18, resolves the token straight to the SA's principal; Cedar then evaluates against
//! whatever grants that principal holds). So minting a key must never let an actor obtain
//! authority THEY could not already grant directly. [`ApiKeyService::issue`] enforces this by
//! mirroring [`crate::application::roles::RoleService::grant`]'s own anti-escalation check
//! ("you can only grant what you already hold" — `roles.rs:140-155`) once PER GRANT the target
//! SA currently holds: `actor` must be authorized for `Action::IssueApiKey` AT the SA's owner
//! node (the ordinary "may this actor manage this SA's keys at all" check, mirroring
//! `ServiceAccountService`'s `owner_resource_prn` posture) **AND**, for every one of the SA's
//! `RoleGrant`s (fetched via `grants: Arc<dyn RoleGrantStore>::list_by_principal`, exactly how
//! `RoleService` shares the same store — module docs there), `Action::GrantRole` AT that grant's
//! own scope. A single denied grant anywhere in the SA's grant set fails the WHOLE issuance,
//! `Forbidden`, before any id is minted or secret generated — an actor who could not grant even
//! ONE of the SA's roles to a third party must not be able to mint a key that wields all of them.
//!
//! **Deliberately not gated on status (mirrors Task 16's own `entity_gen` deferral):**
//! `issue`/`revoke`/`list` all look up the service account via `ServiceAccountRepository::find`,
//! which — since the SMA-445 PR's CodeRabbit fix surfacing SA status to callers — now returns a
//! `ServiceAccountRecord` carrying the underlying `Principal`'s lifecycle status alongside the
//! account (D16: status still lives on `Principal`, never on `ServiceAccount`, so `find`'s
//! signature is a join-read, not a stored field). This service still never READS that `status`
//! field, on purpose: no current Cedar policy reads `principal.status`, and a disabled SA is
//! already blocked twice over without a check here — `ServiceAccountService::archive`'s own
//! cache-evict step (any key already cached as valid stops authenticating immediately), plus
//! Task 18's `AuthenticateApiKey`, which checks `PrincipalStatus` at authentication time on every
//! request regardless of what `issue` did or didn't check at mint time. Issuing a key to an
//! already-disabled SA is therefore harmless (the key is simply unusable until/unless the SA is
//! ever re-enabled) rather than a security hole — gating `issue` on it here would be a redundant
//! belt-and-braces check outside this task's declared file list (`src/application/api_keys.rs` +
//! `application/mod.rs`).
//!
//! **SMA-446 Slice B Task B6 - the Unit-of-Work reference pattern, applied to issue/revoke
//! (copied from `RoleService::grant`/`revoke`, Task B4 - see `application::roles`'s module
//! docs for the pattern itself):** once `issue`'s D15 checks (or `revoke`'s authorize check)
//! pass, the mutation, its `DomainEvent`, and its `AuditEntry` all share ONE freshly-minted
//! `correlation_id` and commit together on ONE `UnitOfWork`-scoped transaction
//! (`keys.issue_in`/`revoke_in`, `outbox.enqueue`, `audit.record`, then `tx.commit()`). Unlike
//! `RoleService`/`PolicyService`, there is NO `gen_bumper` here at all - API-key issue/revoke
//! never bump `policy_gen`/`entity_gen` (they are bearer-credential lifecycle events, not
//! authz-policy changes; the D15 checks above are what keep a minted key's authority bounded).
//! The outbox payload/audit detail carry only `key_id`/`prefix`/`scope`/`status`/`expires_at` -
//! NEVER the plaintext token or its hash (SECRET SAFETY, module docs above on `issue`'s own
//! "returned exactly ONCE" contract).
//!
//! **`revoke`'s post-commit cache-evict is the one deviation from the B4/B5 shape
//! (SECURITY-CRITICAL, spec section 9/D5):** the pre-Slice-B code evicted the key's cached
//! validation INLINE, right after the (then-unwrapped) repository call. That evict now moves to
//! AFTER `tx.commit()` - still AWAITED, so it's guaranteed to have happened by the time `revoke`
//! returns - for the same reason `RoleService`/`PolicyService`'s bump moved post-commit: an
//! evict for a mutation that never actually committed (rolled back mid-txn) must never fire, or
//! a legitimate key could be evicted from cache over a revoke that didn't happen, forcing an
//! extra DB round-trip on its next use but nothing worse. The reverse direction is the
//! genuinely dangerous one this whole task exists to preserve: a revoke that DID commit must
//! ALWAYS evict - run UNCONDITIONALLY after a successful commit (not gated on `revoke_in`'s own
//! bool, unlike the outbox/audit emission), because a cached-valid key that keeps authenticating
//! past its owner's revoke is a live authorization bypass, worse than a redundant no-op evict.
//! `keys.revoke_in`'s own txn-scoped implementation never touches the cache itself (port docs)
//! - that stays this service's own post-commit responsibility, exactly as before Slice B.

use crate::adapters::api_keys::ApiKeyValidationCache;
use crate::application::authorize::Authorize;
use crate::application::error::TenancyError;
use crate::application::pagination::Page;
use crate::config::ApiKeyConfig;
use chrono::{DateTime, Duration, Utc};
use paigasus_iam_core::authz::model::root_prn;
use paigasus_iam_core::{
    Action, ApiKey, ApiKeyId, ApiKeyRepository, ApiKeyStatus, AuditEntry, AuditLog, AuditOutcome, Clock, DomainEvent, EventType, GrantScope, IdGenerator, KeyEntropy, NewApiKey, Outbox, PrincipalId,
    RoleGrantStore, SecretHasher, ServiceAccountRepository, TenancyNodeRef, UnitOfWork, display_prefix, format_token,
};
use paigasus_kernel::Prn;
use std::sync::Arc;

/// The `Prn` a tenancy node represents as an authorization *resource* — mirrors
/// `roles.rs::scope_resource_prn`'s / `service_accounts.rs::owner_resource_prn`'s node arms
/// exactly. Shared by [`owner_resource_prn`] (a service account's owner is always a concrete
/// node, `ck_service_account_owner`, no `Root` arm) and [`grant_scope_resource_prn`] (a
/// `RoleGrant`'s scope CAN be `Root`, handled separately there).
fn node_resource_prn(node: &TenancyNodeRef) -> Prn {
    match node {
        TenancyNodeRef::Organization(id) => id.prn().clone(),
        TenancyNodeRef::Team(id) => id.prn().clone(),
        TenancyNodeRef::Project(id) => id.prn().clone(),
    }
}

/// The `Prn` a service account's owner node represents as an authorization *resource* —
/// identical in shape to `service_accounts.rs::owner_resource_prn` (duplicated rather than
/// made `pub(crate)` across an unrelated module, mirroring `roles.rs::parse_principal_prn`'s
/// own "not worth a visibility change" doc rationale for a five-line pure helper).
fn owner_resource_prn(owner: &TenancyNodeRef) -> Prn {
    node_resource_prn(owner)
}

/// The `Prn` a `RoleGrant`'s scope represents as an authorization *resource* — mirrors
/// `roles.rs::scope_resource_prn` exactly (duplicated for the same reason as
/// [`owner_resource_prn`] above): `root_prn()` for `GrantScope::Root`, else the tenancy node's
/// own PRN. This is what [`ApiKeyService::issue`]'s D15 check authorizes `actor` against for
/// EACH of the target SA's grants.
fn grant_scope_resource_prn(scope: &GrantScope) -> Prn {
    match scope {
        GrantScope::Root => root_prn(),
        GrantScope::Node(node) => node_resource_prn(node),
    }
}

/// API-key lifecycle use cases. `grants`/`cache` are shared `Arc<dyn ...>` handles — `grants`
/// is the same store handle `RoleService` holds (module docs there: a later task's `AppState`
/// wiring clones one `Arc` rather than standing up a second store instance), `cache` is the
/// same `ApiKeyValidationCache` handle `ServiceAccountService::archive` evicts through.
/// `keys`/`service_accounts`/`ids`/`clock` stay generic-DI, mirroring `ServiceAccountService`'s
/// `repo`. `hasher`/`entropy` are the SMA-445 Task 5 ports (`SecretHasher`/`KeyEntropy`),
/// generic-DI so unit tests can inject deterministic fakes instead of the real
/// `HmacSecretHasher`/`OsRngKeyEntropy` adapters (which need a real `Pepper`/OS CSPRNG). `config`
/// carries the SMA-445 Task 15 `[api_keys]` block wholesale (`key_prefix`, `default_expiry_days`
/// are the only fields this service reads) rather than a bespoke subset struct, so a future
/// `AppState` wiring can pass `iam_config.api_keys.clone()` directly.
#[derive(Clone)]
pub struct ApiKeyService<K, S, I, C, H, E> {
    keys: K,
    service_accounts: S,
    grants: Arc<dyn RoleGrantStore>,
    authorize: Authorize,
    hasher: H,
    entropy: E,
    cache: Arc<dyn ApiKeyValidationCache>,
    uow: Arc<dyn UnitOfWork>,
    outbox: Arc<dyn Outbox>,
    audit: Arc<dyn AuditLog>,
    ids: I,
    clock: C,
    config: ApiKeyConfig,
}

/// Named-field constructor params for [`ApiKeyService::new`] (SMA-446 Slice B Task B6) —
/// copies `application::roles::RoleServiceDeps`/`application::policies::PolicyServiceDeps`'s
/// DI-params idiom verbatim: one field per dependency, built with struct syntax at the call
/// site so each argument is self-labeling. Deliberately has NO `gen_bumper` field — module
/// docs: API-key issue/revoke never bump `policy_gen`/`entity_gen`.
pub struct ApiKeyServiceDeps<K, S, I, C, H, E> {
    pub keys: K,
    pub service_accounts: S,
    pub grants: Arc<dyn RoleGrantStore>,
    pub authorize: Authorize,
    pub hasher: H,
    pub entropy: E,
    pub cache: Arc<dyn ApiKeyValidationCache>,
    pub uow: Arc<dyn UnitOfWork>,
    pub outbox: Arc<dyn Outbox>,
    pub audit: Arc<dyn AuditLog>,
    pub ids: I,
    pub clock: C,
    pub config: ApiKeyConfig,
}

impl<K, S, I, C, H, E> ApiKeyService<K, S, I, C, H, E>
where
    K: ApiKeyRepository,
    S: ServiceAccountRepository,
    I: IdGenerator,
    C: Clock,
    H: SecretHasher,
    E: KeyEntropy,
{
    pub fn new(deps: ApiKeyServiceDeps<K, S, I, C, H, E>) -> Self {
        Self {
            keys: deps.keys,
            service_accounts: deps.service_accounts,
            grants: deps.grants,
            authorize: deps.authorize,
            hasher: deps.hasher,
            entropy: deps.entropy,
            cache: deps.cache,
            uow: deps.uow,
            outbox: deps.outbox,
            audit: deps.audit,
            ids: deps.ids,
            clock: deps.clock,
            config: deps.config,
        }
    }

    /// Issues a new API key for `sa_id`. Order of checks (D15, module docs — the
    /// SECURITY-CRITICAL part of this file):
    ///
    /// 1. The service account must exist (`NotFound` if absent).
    /// 2. `actor` must be authorized for `Action::IssueApiKey` AT the SA's owner node.
    /// 3. For EVERY `RoleGrant` the SA currently holds (`grants.list_by_principal`), `actor`
    ///    must ALSO be authorized for `Action::GrantRole` AT that grant's own scope — any single
    ///    denial fails the whole call, `Forbidden`, before anything is minted.
    ///
    /// Only once both authorization checks fully pass does this mint an id, generate a secret
    /// (`entropy.new_secret()`), hash it (`hasher.hash`), and persist the `ApiKey` row together
    /// with the hash — never the plaintext, which is returned to the caller exactly ONCE inside
    /// the result's `plaintext` field and never re-derivable afterward.
    pub async fn issue(
        &self,
        actor: &Prn,
        sa_id: &PrincipalId,
        scope: TenancyNodeRef,
        expires_at: Option<DateTime<Utc>>,
        scope_actions: Vec<Action>,
        scope_roles: Vec<String>,
    ) -> Result<NewApiKey, TenancyError> {
        let sa = self.service_accounts.find(sa_id).await?.ok_or(TenancyError::NotFound)?;

        self.authorize.check(actor, Action::IssueApiKey, &owner_resource_prn(&sa.account.owner)).await?;

        let sa_grants = self.grants.list_by_principal(sa_id).await?;
        for grant in &sa_grants {
            self.authorize.check(actor, Action::GrantRole, &grant_scope_resource_prn(&grant.scope)).await?;
        }

        let id = self.ids.new_api_key_id();
        let secret = self.entropy.new_secret();
        let hash = self.hasher.hash(&secret);
        let prefix = display_prefix(&self.config.key_prefix, id);
        let now = self.clock.now();
        let key = ApiKey {
            id,
            service_account_id: sa_id.clone(),
            scope,
            prefix,
            status: ApiKeyStatus::Active,
            expires_at: expires_at.or_else(|| self.default_expires_at(now)),
            last_used_at: None,
            created_at: now,
            revoked_at: None,
            scope_actions,
            scope_roles,
        };
        let plaintext = format_token(&self.config.key_prefix, id, &secret);

        // SMA-446 Slice B Task B6 (module docs): the key row, its `DomainEvent`, and its
        // `AuditEntry` share ONE correlation id and commit together on ONE UoW transaction.
        // The payload/detail carry ONLY key_id/prefix/scope/status/expires_at — NEVER `hash`
        // or `secret`/`plaintext` (SECRET SAFETY: those two local bindings are never even
        // referenced below this point except to build `plaintext`'s one-time return value).
        let corr = self.ids.new_correlation_id();
        let event = DomainEvent {
            id: self.ids.new_event_id(),
            event_type: EventType::ApiKeyIssued,
            schema_version: 1,
            aggregate_prn: sa_id.canonical(),
            actor_prn: Some(actor.canonical()),
            occurred_at: now,
            payload: serde_json::json!({
                "key_id": key.id.uuid(),
                "prefix": key.prefix,
                "scope": key.scope.canonical(),
                "status": key.status.as_str(),
                "expires_at": key.expires_at,
            }),
            correlation_id: Some(corr),
        };
        let entry = AuditEntry {
            id: self.ids.new_audit_id(),
            occurred_at: now,
            actor_prn: Some(actor.canonical()),
            action: "IssueApiKey".to_string(),
            resource_prn: Some(owner_resource_prn(&sa.account.owner).canonical()),
            outcome: AuditOutcome::Committed,
            determining_policies: vec![],
            detail: serde_json::json!({"key_id": key.id.uuid(), "prefix": key.prefix}),
            correlation_id: Some(corr),
        };

        let tx = self.uow.begin().await?;
        self.keys.issue_in(&*tx, &key, &hash).await?;
        self.outbox.enqueue(&*tx, &event).await?;
        self.audit.record(&*tx, &entry).await?;
        tx.commit().await?;

        Ok(NewApiKey { key, plaintext })
    }

    /// `expires_at.or_else`'s fallback: `now + config.default_expiry_days` if configured, else
    /// `None` (non-expiring until revoked — spec §11).
    fn default_expires_at(&self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        self.config.default_expiry_days.map(|days| now + Duration::days(i64::from(days)))
    }

    /// Revokes `key_id`: `NotFound` if it doesn't exist; finds its service account (`NotFound`
    /// if that's somehow gone too — belt-and-braces, the FK guarantees it exists in practice);
    /// authorizes `Action::RevokeApiKey` AT the SA's owner node; persists the revocation, its
    /// `DomainEvent`, and its `AuditEntry` atomically (SMA-446 Slice B Task B6, module docs —
    /// `keys.revoke_in`, only enqueuing/recording when it reports a genuine Active -> Revoked
    /// transition); then — POST-COMMIT, AWAITED, UNCONDITIONALLY (module docs, SECURITY-
    /// CRITICAL) — evicts the key's cached validation (mirrors `ServiceAccountService::
    /// archive`'s own cache-evict step, spec §9/D5 "revocation-vs-cache honesty" — without
    /// this, a just-revoked key that was already cached as valid would keep authenticating
    /// until its cache entry expires on its own). The evict runs whenever this method reaches
    /// its final `Ok(())` — including a `revoke_in == false` idempotent no-op, so a repeat
    /// revoke still clears any cache entry a previous call's evict might have missed — and
    /// NEVER runs if an earlier `?` (authorize, `revoke_in`, `tx.commit()`) short-circuits the
    /// function first: a rolled-back mutation must never evict a cache entry for a revoke that
    /// didn't actually happen.
    pub async fn revoke(&self, actor: &Prn, key_id: ApiKeyId) -> Result<(), TenancyError> {
        let (key, _hash) = self.keys.find_by_id(key_id).await?.ok_or(TenancyError::NotFound)?;
        let sa = self.service_accounts.find(&key.service_account_id).await?.ok_or(TenancyError::NotFound)?;
        self.authorize.check(actor, Action::RevokeApiKey, &owner_resource_prn(&sa.account.owner)).await?;

        let now = self.clock.now();
        let corr = self.ids.new_correlation_id();
        let event = DomainEvent {
            id: self.ids.new_event_id(),
            event_type: EventType::ApiKeyRevoked,
            schema_version: 1,
            aggregate_prn: key.service_account_id.canonical(),
            actor_prn: Some(actor.canonical()),
            occurred_at: now,
            payload: serde_json::json!({
                "key_id": key.id.uuid(),
                "prefix": key.prefix,
                "scope": key.scope.canonical(),
                "status": ApiKeyStatus::Revoked.as_str(),
                "expires_at": key.expires_at,
            }),
            correlation_id: Some(corr),
        };
        let entry = AuditEntry {
            id: self.ids.new_audit_id(),
            occurred_at: now,
            actor_prn: Some(actor.canonical()),
            action: "RevokeApiKey".to_string(),
            resource_prn: Some(owner_resource_prn(&sa.account.owner).canonical()),
            outcome: AuditOutcome::Committed,
            determining_policies: vec![],
            detail: serde_json::json!({"key_id": key.id.uuid(), "prefix": key.prefix}),
            correlation_id: Some(corr),
        };

        let tx = self.uow.begin().await?;
        let did_revoke = self.keys.revoke_in(&*tx, key_id, now).await?;
        if did_revoke {
            self.outbox.enqueue(&*tx, &event).await?;
            self.audit.record(&*tx, &entry).await?;
        }
        tx.commit().await?;

        // POST-COMMIT, AWAITED, UNCONDITIONAL (module docs — SECURITY-CRITICAL, spec §9/D5):
        // only reachable once `tx.commit()` above has actually succeeded, and always run from
        // here regardless of `did_revoke` — see this method's own doc for why both halves of
        // that contract matter.
        self.cache.evict(key_id).await;
        Ok(())
    }

    /// Lists the API keys belonging to `sa_id`: `NotFound` if the SA doesn't exist; authorizes
    /// `Action::ListApiKeys` AT its owner node; delegates to `repo.list_by_service_account`.
    /// Never exposes a secret or hash — `ApiKey` carries neither field at all (structural: the
    /// hash lives only in `ApiKeyRepository::issue`/`find_by_id`'s separate `Vec<u8>`, and the
    /// plaintext is never persisted anywhere, module docs).
    pub async fn list(&self, actor: &Prn, sa_id: &PrincipalId, page: Page) -> Result<Vec<ApiKey>, TenancyError> {
        let sa = self.service_accounts.find(sa_id).await?.ok_or(TenancyError::NotFound)?;
        self.authorize.check(actor, Action::ListApiKeys, &owner_resource_prn(&sa.account.owner)).await?;
        Ok(self.keys.list_by_service_account(sa_id, page.limit, page.offset).await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::api_keys::{CachedValidation, MemoryApiKeyCache};
    use crate::application::fakes::{
        FakeAuditLog, FakeAuthorizer, FakeOutbox, FakeSecretHasher, FakeUnitOfWork, FixedClock, InMemoryApiKeys, InMemoryRoleGrants, InMemoryServiceAccounts, SeqIds, SeqKeyEntropy,
    };
    use async_trait::async_trait;
    use paigasus_iam_core::{OrganizationId, PrincipalStatus, RepositoryError, RoleGrant, ServiceAccount, Transaction, parse_token};
    use uuid::Uuid;

    fn actor_prn(n: u128) -> Prn {
        Prn::build("iam", "", None, "principal", Uuid::from_u128(n)).unwrap()
    }

    fn owner_org(n: u128) -> TenancyNodeRef {
        TenancyNodeRef::Organization(OrganizationId::from_uuid(Uuid::from_u128(n)))
    }

    /// Bundles an `ApiKeyService` together with every fake it was built over (SMA-446 Slice B
    /// Task B6 — mirrors `application::roles::tests::ServiceWithFakes`/`application::
    /// policies::tests::ServiceWithFakes`), so a test can assert on exactly what `issue`/
    /// `revoke` persisted AND exactly what they emitted through the UoW reference pattern's
    /// outbox/audit ports.
    struct ServiceWithFakes {
        svc: ApiKeyService<InMemoryApiKeys, InMemoryServiceAccounts, SeqIds, FixedClock, FakeSecretHasher, SeqKeyEntropy>,
        keys: InMemoryApiKeys,
        service_accounts: InMemoryServiceAccounts,
        grants: Arc<InMemoryRoleGrants>,
        cache: Arc<MemoryApiKeyCache>,
        outbox: FakeOutbox,
        audit: FakeAuditLog,
    }

    fn new_service_with_fakes(fake: FakeAuthorizer) -> ServiceWithFakes {
        let keys = InMemoryApiKeys::default();
        let service_accounts = InMemoryServiceAccounts::default();
        let grants = Arc::new(InMemoryRoleGrants::default());
        let cache = Arc::new(MemoryApiKeyCache::new(30));
        let outbox = FakeOutbox::default();
        let audit = FakeAuditLog::default();
        let svc = ApiKeyService::new(ApiKeyServiceDeps {
            keys: keys.clone(),
            service_accounts: service_accounts.clone(),
            grants: grants.clone(),
            authorize: Authorize::new(Arc::new(fake)),
            hasher: FakeSecretHasher,
            entropy: SeqKeyEntropy::default(),
            cache: cache.clone(),
            uow: Arc::new(FakeUnitOfWork),
            outbox: Arc::new(outbox.clone()),
            audit: Arc::new(audit.clone()),
            ids: SeqIds::default(),
            clock: FixedClock::default(),
            config: ApiKeyConfig::default(),
        });
        ServiceWithFakes {
            svc,
            keys,
            service_accounts,
            grants,
            cache,
            outbox,
            audit,
        }
    }

    #[allow(clippy::type_complexity)]
    fn new_service(
        fake: FakeAuthorizer,
    ) -> (
        ApiKeyService<InMemoryApiKeys, InMemoryServiceAccounts, SeqIds, FixedClock, FakeSecretHasher, SeqKeyEntropy>,
        InMemoryApiKeys,
        InMemoryServiceAccounts,
        Arc<InMemoryRoleGrants>,
        Arc<MemoryApiKeyCache>,
    ) {
        let ServiceWithFakes {
            svc,
            keys,
            service_accounts,
            grants,
            cache,
            ..
        } = new_service_with_fakes(fake);
        (svc, keys, service_accounts, grants, cache)
    }

    /// An `ApiKeyRepository` whose `revoke_in` always fails — simulates a store error mid-txn
    /// (mirrors `roles.rs::FailingGrantStore`/`policies.rs::FailingPutStore`): `ApiKeyService::
    /// revoke` must roll back before ever touching the outbox/audit log, and — the
    /// SECURITY-CRITICAL part this task adds — must NEVER evict the key's cached validation for
    /// a revoke that never actually committed. `issue`/`issue_in`/`find_by_id`/etc. all
    /// delegate to a real backing `InMemoryApiKeys` so a test can seed/read normally; only
    /// `revoke_in` is overridden.
    #[derive(Clone, Default)]
    struct FailingRevokeApiKeys(InMemoryApiKeys);

    #[async_trait]
    impl ApiKeyRepository for FailingRevokeApiKeys {
        async fn issue(&self, key: &ApiKey, key_hash: &[u8]) -> Result<(), RepositoryError> {
            self.0.issue(key, key_hash).await
        }
        async fn issue_in(&self, tx: &dyn Transaction, key: &ApiKey, key_hash: &[u8]) -> Result<(), RepositoryError> {
            self.0.issue_in(tx, key, key_hash).await
        }
        async fn find_by_id(&self, id: ApiKeyId) -> Result<Option<(ApiKey, Vec<u8>)>, RepositoryError> {
            self.0.find_by_id(id).await
        }
        async fn revoke(&self, id: ApiKeyId, now: DateTime<Utc>) -> Result<(), RepositoryError> {
            self.0.revoke(id, now).await
        }
        async fn revoke_in(&self, _tx: &dyn Transaction, _id: ApiKeyId, _now: DateTime<Utc>) -> Result<bool, RepositoryError> {
            Err(RepositoryError::Backend(Box::new(std::io::Error::other("simulated mid-txn store failure"))))
        }
        async fn list_by_service_account(&self, sa: &PrincipalId, limit: u64, offset: u64) -> Result<Vec<ApiKey>, RepositoryError> {
            self.0.list_by_service_account(sa, limit, offset).await
        }
        async fn list_ids_by_service_account(&self, sa: &PrincipalId) -> Result<Vec<ApiKeyId>, RepositoryError> {
            self.0.list_ids_by_service_account(sa).await
        }
        async fn touch_last_used(&self, id: ApiKeyId, now: DateTime<Utc>, throttle_secs: u64) -> Result<(), RepositoryError> {
            self.0.touch_last_used(id, now, throttle_secs).await
        }
    }

    /// Seeds a service account row directly into the fake repo (bypassing
    /// `ServiceAccountService`, which lives in a sibling module and isn't this service's
    /// concern) and returns its principal id.
    fn seed_service_account(repo: &InMemoryServiceAccounts, owner: TenancyNodeRef, n: u128) -> PrincipalId {
        let id = PrincipalId::from_prn(Prn::build("iam", "", None, "principal", Uuid::from_u128(n)).unwrap());
        let now = Utc::now();
        let sa = ServiceAccount::new(id.clone(), owner, "ci-bot", now).unwrap();
        repo.accounts.lock().unwrap().insert(id.uuid(), sa);
        repo.statuses.lock().unwrap().insert(id.uuid(), PrincipalStatus::Active);
        id
    }

    /// Seeds a `RoleGrant` directly into the fake store, mirroring `fakes.rs::owner_grant`'s
    /// own construction style.
    fn seed_role_grant(store: &InMemoryRoleGrants, n: u128, principal: &PrincipalId, role_key: &str, scope: GrantScope) {
        let id = Uuid::from_u128(n);
        let grant = RoleGrant {
            id,
            principal: principal.clone(),
            role_key: role_key.to_string(),
            scope,
            linked_policy_id: format!("grant:{id}"),
            created_at: Utc::now(),
        };
        store.0.lock().unwrap().insert(id, grant);
    }

    #[tokio::test]
    async fn issue_returns_plaintext_once_and_persists_only_hash() {
        let owner = owner_org(1);
        let fake = FakeAuthorizer::default();
        fake.allow(Action::IssueApiKey, &owner_resource_prn(&owner));
        let (svc, keys, service_accounts, ..) = new_service(fake);
        let sa_id = seed_service_account(&service_accounts, owner, 100);
        let actor = actor_prn(1);

        let new_key = svc.issue(&actor, &sa_id, owner_org(1), None, Vec::new(), Vec::new()).await.unwrap();

        // The plaintext parses back to the persisted key's id.
        let parsed = parse_token(&ApiKeyConfig::default().key_prefix, &new_key.plaintext, 512).unwrap();
        assert_eq!(parsed.key_id, new_key.key.id);

        // The persisted row is byte-identical to the one returned, and (structurally — `ApiKey`
        // carries no plaintext/secret field at all) can never leak the secret.
        let (stored_key, stored_hash) = keys.find_by_id(new_key.key.id).await.unwrap().unwrap();
        assert_eq!(stored_key, new_key.key);

        // The stored hash is exactly `hasher.hash(secret)` for the secret embedded in the
        // returned plaintext.
        assert_eq!(stored_hash, FakeSecretHasher.hash(&parsed.secret));
    }

    /// SMA-446 Slice B Task B6 — the UoW reference pattern's core contract for `issue`:
    /// enqueues exactly one `DomainEvent` and records exactly one `AuditEntry`, the two
    /// sharing ONE correlation id. SECRET SAFETY (this task's other headline requirement):
    /// neither the outbox payload nor the audit detail contains the plaintext token or the raw
    /// secret bytes — `FakeSecretHasher` is an identity transform (`hash(secret) == secret`),
    /// so the stored hash IS the raw secret, and the plaintext token textually embeds a
    /// base64url encoding of it; a substring search for the plaintext therefore also proves the
    /// hash itself never leaks. The payload/detail key sets are also asserted exactly, so a
    /// future field addition that smuggled the secret in under an unexpected key name would
    /// fail this test too.
    #[tokio::test]
    async fn issue_emits_one_event_and_one_audit_entry_sharing_a_correlation_id_and_never_leaks_the_secret() {
        let owner = owner_org(1);
        let fake = FakeAuthorizer::default();
        fake.allow(Action::IssueApiKey, &owner_resource_prn(&owner));
        let ServiceWithFakes {
            svc, service_accounts, outbox, audit, ..
        } = new_service_with_fakes(fake);
        let sa_id = seed_service_account(&service_accounts, owner, 600);
        let actor = actor_prn(1);

        let new_key = svc.issue(&actor, &sa_id, owner_org(1), None, Vec::new(), Vec::new()).await.unwrap();

        let events = outbox.0.lock().unwrap();
        assert_eq!(events.len(), 1, "issue must enqueue exactly one domain event");
        assert_eq!(events[0].event_type, EventType::ApiKeyIssued);
        assert_eq!(events[0].aggregate_prn, sa_id.canonical());
        assert_eq!(events[0].actor_prn, Some(actor.canonical()));
        assert_eq!(events[0].payload["key_id"], serde_json::json!(new_key.key.id.uuid()));
        assert_eq!(events[0].payload["prefix"], serde_json::json!(new_key.key.prefix));
        assert_eq!(events[0].payload["status"], serde_json::json!("active"));
        let payload_obj = events[0].payload.as_object().unwrap();
        let expected_keys: std::collections::BTreeSet<&str> = ["key_id", "prefix", "scope", "status", "expires_at"].into_iter().collect();
        assert_eq!(
            payload_obj.keys().map(String::as_str).collect::<std::collections::BTreeSet<_>>(),
            expected_keys,
            "the payload must carry ONLY these fields — no hash, no secret"
        );

        let entries = audit.0.lock().unwrap();
        assert_eq!(entries.len(), 1, "issue must record exactly one audit entry");
        assert_eq!(entries[0].action, "IssueApiKey");
        assert_eq!(entries[0].outcome, AuditOutcome::Committed);
        assert_eq!(entries[0].actor_prn, Some(actor.canonical()));
        let detail_obj = entries[0].detail.as_object().unwrap();
        let expected_detail_keys: std::collections::BTreeSet<&str> = ["key_id", "prefix"].into_iter().collect();
        assert_eq!(
            detail_obj.keys().map(String::as_str).collect::<std::collections::BTreeSet<_>>(),
            expected_detail_keys,
            "the audit detail must carry ONLY these fields — no hash, no secret"
        );

        assert!(events[0].correlation_id.is_some());
        assert_eq!(events[0].correlation_id, entries[0].correlation_id, "the event and the audit entry must share one correlation id");

        // SECRET SAFETY: the plaintext (and, transitively, the raw secret it encodes) never
        // appears anywhere in the JSON emitted through either port.
        let payload_str = events[0].payload.to_string();
        let detail_str = entries[0].detail.to_string();
        assert!(!payload_str.contains(&new_key.plaintext), "outbox payload must never contain the plaintext token");
        assert!(!detail_str.contains(&new_key.plaintext), "audit detail must never contain the plaintext token");
    }

    /// D15, THE key test: the SA holds an `org_admin` grant at `org_x`. `actor` is authorized
    /// for `IssueApiKey` at the SA's owner but was NEVER granted `GrantRole` at `org_x` — so
    /// `issue` must deny, `Forbidden`, and must not persist a key (an actor who could not grant
    /// `org_x`'s role to a third party must not be able to mint a key that wields it).
    ///
    /// SMA-446 Slice B Task B6 additionally proves the UoW reference pattern's own contract on
    /// this SAME denial: a D15-denied `issue` never even reaches `uow.begin()` (the D15 checks
    /// run before any mutation is built), so it must persist nothing AND emit nothing — no
    /// outbox event, no audit entry.
    #[tokio::test]
    async fn issue_denied_when_actor_cannot_grant_all_sa_roles() {
        let owner = owner_org(1);
        let org_x = owner_org(2);
        let fake = FakeAuthorizer::default();
        fake.allow(Action::IssueApiKey, &owner_resource_prn(&owner));
        // Deliberately NOT allowed: `GrantRole` @ org_x.

        let ServiceWithFakes {
            svc,
            keys,
            service_accounts,
            grants,
            outbox,
            audit,
            ..
        } = new_service_with_fakes(fake);
        let sa_id = seed_service_account(&service_accounts, owner, 200);
        seed_role_grant(&grants, 900, &sa_id, "org_admin", GrantScope::Node(org_x));
        let actor = actor_prn(1);

        let err = svc.issue(&actor, &sa_id, owner_org(1), None, Vec::new(), Vec::new()).await.unwrap_err();
        assert_eq!(err, TenancyError::Forbidden);
        assert!(keys.0.lock().unwrap().is_empty(), "a D15-denied issue must not persist any key");
        assert!(outbox.0.lock().unwrap().is_empty(), "a D15-denied issue must not enqueue an event");
        assert!(audit.0.lock().unwrap().is_empty(), "a D15-denied issue must not record an audit entry");
    }

    /// The positive mirror of the D15 test: `actor` dominates EVERY grant the SA holds (as well
    /// as `IssueApiKey` at the owner) — issuance must succeed.
    #[tokio::test]
    async fn issue_allowed_when_actor_dominates_all_sa_grants() {
        let owner = owner_org(1);
        let org_x = owner_org(2);
        let fake = FakeAuthorizer::default();
        fake.allow(Action::IssueApiKey, &owner_resource_prn(&owner));
        fake.allow(Action::GrantRole, &owner_resource_prn(&org_x));

        let (svc, keys, service_accounts, grants, _cache) = new_service(fake);
        let sa_id = seed_service_account(&service_accounts, owner, 300);
        seed_role_grant(&grants, 901, &sa_id, "org_admin", GrantScope::Node(org_x));
        let actor = actor_prn(1);

        let new_key = svc.issue(&actor, &sa_id, owner_org(1), None, Vec::new(), Vec::new()).await.unwrap();
        assert_eq!(new_key.key.service_account_id, sa_id);
        assert!(keys.0.lock().unwrap().contains_key(&new_key.key.id));
    }

    #[tokio::test]
    async fn issue_missing_service_account_is_not_found() {
        let (svc, ..) = new_service(FakeAuthorizer::default());
        let missing = PrincipalId::from_prn(Prn::build("iam", "", None, "principal", Uuid::from_u128(999)).unwrap());
        let actor = actor_prn(1);

        let err = svc.issue(&actor, &missing, owner_org(1), None, Vec::new(), Vec::new()).await.unwrap_err();
        assert_eq!(err, TenancyError::NotFound);
    }

    #[tokio::test]
    async fn revoke_authorizes_and_evicts_cache() {
        let owner = owner_org(1);
        let fake = FakeAuthorizer::default();
        fake.allow(Action::IssueApiKey, &owner_resource_prn(&owner));
        fake.allow(Action::RevokeApiKey, &owner_resource_prn(&owner));
        let (svc, keys, service_accounts, _grants, cache) = new_service(fake);
        let sa_id = seed_service_account(&service_accounts, owner, 400);
        let actor = actor_prn(1);

        let new_key = svc.issue(&actor, &sa_id, owner_org(1), None, Vec::new(), Vec::new()).await.unwrap();
        cache
            .put(
                new_key.key.id,
                &CachedValidation {
                    principal_id: sa_id.clone(),
                    sa_status: PrincipalStatus::Active,
                    expires_at: None,
                    key_hash: b"hash".to_vec(),
                    scope_prn: owner_org(1).canonical(),
                },
            )
            .await;
        assert!(cache.get(new_key.key.id).await.is_some(), "sanity: the cache entry exists before revoke");

        svc.revoke(&actor, new_key.key.id).await.unwrap();

        // The security-critical part: the cached validation is gone.
        assert!(cache.get(new_key.key.id).await.is_none(), "revoke must evict the key's cached validation");
        // And the repo call actually happened: the stored row flips to Revoked.
        let (stored_key, _) = keys.find_by_id(new_key.key.id).await.unwrap().unwrap();
        assert_eq!(stored_key.status, ApiKeyStatus::Revoked);
    }

    /// SMA-446 Slice B Task B6 — the UoW reference pattern's core contract for `revoke`:
    /// mirrors `issue`'s own event/audit/correlation proof above.
    #[tokio::test]
    async fn revoke_emits_one_event_and_one_audit_entry_sharing_a_correlation_id() {
        let owner = owner_org(1);
        let fake = FakeAuthorizer::default();
        fake.allow(Action::IssueApiKey, &owner_resource_prn(&owner));
        fake.allow(Action::RevokeApiKey, &owner_resource_prn(&owner));
        let ServiceWithFakes {
            svc, service_accounts, outbox, audit, ..
        } = new_service_with_fakes(fake);
        let sa_id = seed_service_account(&service_accounts, owner, 700);
        let actor = actor_prn(1);
        let new_key = svc.issue(&actor, &sa_id, owner_org(1), None, Vec::new(), Vec::new()).await.unwrap();
        // `issue` above already enqueued/recorded once — clear those so this test only sees
        // `revoke`'s own emissions.
        outbox.0.lock().unwrap().clear();
        audit.0.lock().unwrap().clear();

        svc.revoke(&actor, new_key.key.id).await.unwrap();

        let events = outbox.0.lock().unwrap();
        assert_eq!(events.len(), 1, "revoke must enqueue exactly one domain event");
        assert_eq!(events[0].event_type, EventType::ApiKeyRevoked);
        assert_eq!(events[0].aggregate_prn, sa_id.canonical());
        assert_eq!(events[0].payload["status"], serde_json::json!("revoked"));

        let entries = audit.0.lock().unwrap();
        assert_eq!(entries.len(), 1, "revoke must record exactly one audit entry");
        assert_eq!(entries[0].action, "RevokeApiKey");
        assert_eq!(entries[0].outcome, AuditOutcome::Committed);

        assert_eq!(events[0].correlation_id, entries[0].correlation_id, "the event and the audit entry must share one correlation id");
    }

    /// SMA-446 Slice B Task B6, SECURITY-CRITICAL: a revoke whose mutation rolls back mid-txn
    /// (here, `revoke_in` failing on a store error, guard D2's analogue) must NEVER evict the
    /// key's cached validation — an eviction for a revoke that never actually committed would
    /// be harmless in this direction (just an extra DB round-trip on the key's next use), but
    /// proving it does NOT happen is what pins down that the evict really did move to
    /// POST-commit rather than staying inline before `tx.commit()`.
    #[tokio::test]
    async fn revoke_never_evicts_cache_when_the_mutation_rolls_back() {
        let owner = owner_org(1);
        let fake = FakeAuthorizer::default();
        fake.allow(Action::RevokeApiKey, &owner_resource_prn(&owner));

        let service_accounts = InMemoryServiceAccounts::default();
        let sa_id = seed_service_account(&service_accounts, owner.clone(), 800);

        let now = Utc::now();
        let key = ApiKey {
            id: ApiKeyId::from_uuid(Uuid::from_u128(8000)),
            service_account_id: sa_id.clone(),
            scope: owner.clone(),
            prefix: "pgs_sk_test".to_string(),
            status: ApiKeyStatus::Active,
            expires_at: None,
            last_used_at: None,
            created_at: now,
            revoked_at: None,
            scope_actions: Vec::new(),
            scope_roles: Vec::new(),
        };
        let failing_keys = FailingRevokeApiKeys::default();
        failing_keys.0.issue(&key, b"hash").await.unwrap();

        let cache = Arc::new(MemoryApiKeyCache::new(30));
        cache
            .put(
                key.id,
                &CachedValidation {
                    principal_id: sa_id.clone(),
                    sa_status: PrincipalStatus::Active,
                    expires_at: None,
                    key_hash: b"hash".to_vec(),
                    scope_prn: owner_org(1).canonical(),
                },
            )
            .await;
        assert!(cache.get(key.id).await.is_some(), "sanity: the cache entry exists before the failed revoke");

        let svc = ApiKeyService::new(ApiKeyServiceDeps {
            keys: failing_keys,
            service_accounts,
            grants: Arc::new(InMemoryRoleGrants::default()),
            authorize: Authorize::new(Arc::new(fake)),
            hasher: FakeSecretHasher,
            entropy: SeqKeyEntropy::default(),
            cache: cache.clone(),
            uow: Arc::new(FakeUnitOfWork),
            outbox: Arc::new(FakeOutbox::default()),
            audit: Arc::new(FakeAuditLog::default()),
            ids: SeqIds::default(),
            clock: FixedClock::default(),
            config: ApiKeyConfig::default(),
        });

        let actor = actor_prn(1);
        let err = svc.revoke(&actor, key.id).await.unwrap_err();
        assert_eq!(err, TenancyError::Internal, "a Backend error from a mid-txn store failure maps to Internal");

        assert!(cache.get(key.id).await.is_some(), "a rolled-back revoke must NOT evict the cache — SECURITY-CRITICAL");
    }

    /// SMA-446 Slice B Task B6: `revoke_in` returning `false` for an already-revoked key (an
    /// idempotent no-op — module docs) must still run the post-commit cache-evict
    /// UNCONDITIONALLY (in case an earlier revoke's own evict attempt failed and left a stale
    /// entry behind), even though it emits NEITHER a new outbox event NOR a new audit entry.
    #[tokio::test]
    async fn revoke_of_an_already_revoked_key_still_evicts_the_cache_but_emits_nothing_new() {
        let owner = owner_org(1);
        let fake = FakeAuthorizer::default();
        fake.allow(Action::IssueApiKey, &owner_resource_prn(&owner));
        fake.allow(Action::RevokeApiKey, &owner_resource_prn(&owner));
        let ServiceWithFakes {
            svc,
            service_accounts,
            cache,
            outbox,
            audit,
            ..
        } = new_service_with_fakes(fake);
        let sa_id = seed_service_account(&service_accounts, owner, 900);
        let actor = actor_prn(1);
        let new_key = svc.issue(&actor, &sa_id, owner_org(1), None, Vec::new(), Vec::new()).await.unwrap();

        svc.revoke(&actor, new_key.key.id).await.unwrap();
        outbox.0.lock().unwrap().clear();
        audit.0.lock().unwrap().clear();

        // Re-populate the cache as if a previous evict attempt had failed, then revoke again.
        cache
            .put(
                new_key.key.id,
                &CachedValidation {
                    principal_id: sa_id.clone(),
                    sa_status: PrincipalStatus::Active,
                    expires_at: None,
                    key_hash: b"hash".to_vec(),
                    scope_prn: owner_org(1).canonical(),
                },
            )
            .await;
        assert!(cache.get(new_key.key.id).await.is_some(), "sanity: the cache entry exists before the second revoke");

        svc.revoke(&actor, new_key.key.id).await.unwrap();

        assert!(cache.get(new_key.key.id).await.is_none(), "a repeat revoke must still evict the cache unconditionally");
        assert!(outbox.0.lock().unwrap().is_empty(), "an idempotent no-op revoke must not enqueue a new event");
        assert!(audit.0.lock().unwrap().is_empty(), "an idempotent no-op revoke must not record a new audit entry");
    }

    #[tokio::test]
    async fn revoke_denied_without_authz_then_succeeds_once_authorized() {
        let owner = owner_org(1);
        let fake = FakeAuthorizer::default();
        fake.allow(Action::IssueApiKey, &owner_resource_prn(&owner));
        // `RevokeApiKey` never allowed yet.
        let (svc, keys, service_accounts, ..) = new_service(fake.clone());
        let sa_id = seed_service_account(&service_accounts, owner.clone(), 401);
        let actor = actor_prn(1);
        let new_key = svc.issue(&actor, &sa_id, owner.clone(), None, Vec::new(), Vec::new()).await.unwrap();

        assert_eq!(svc.revoke(&actor, new_key.key.id).await.unwrap_err(), TenancyError::Forbidden);
        let (still_active, _) = keys.find_by_id(new_key.key.id).await.unwrap().unwrap();
        assert_eq!(still_active.status, ApiKeyStatus::Active);

        fake.allow(Action::RevokeApiKey, &owner_resource_prn(&owner));
        svc.revoke(&actor, new_key.key.id).await.unwrap();
        let (revoked, _) = keys.find_by_id(new_key.key.id).await.unwrap().unwrap();
        assert_eq!(revoked.status, ApiKeyStatus::Revoked);
    }

    #[tokio::test]
    async fn revoke_missing_key_is_not_found() {
        let (svc, ..) = new_service(FakeAuthorizer::default());
        let actor = actor_prn(1);
        let missing = ApiKeyId::from_uuid(Uuid::from_u128(12345));
        assert_eq!(svc.revoke(&actor, missing).await.unwrap_err(), TenancyError::NotFound);
    }

    /// Structural: `list`'s return type is `Vec<ApiKey>`, and `ApiKey` has no plaintext/secret
    /// field at all (module docs) — the compiler enforces this, so there is nothing a listing
    /// could ever leak beyond confirming the listed row matches the issued key's own metadata.
    #[tokio::test]
    async fn list_omits_secrets() {
        let owner = owner_org(1);
        let fake = FakeAuthorizer::default();
        fake.allow(Action::IssueApiKey, &owner_resource_prn(&owner));
        fake.allow(Action::ListApiKeys, &owner_resource_prn(&owner));
        let (svc, _keys, service_accounts, ..) = new_service(fake);
        let sa_id = seed_service_account(&service_accounts, owner.clone(), 500);
        let actor = actor_prn(1);
        let new_key = svc.issue(&actor, &sa_id, owner, None, Vec::new(), Vec::new()).await.unwrap();

        let listed = svc.list(&actor, &sa_id, Page::new(None, None).unwrap()).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, new_key.key.id);
    }

    #[tokio::test]
    async fn list_denied_without_authz() {
        let owner = owner_org(1);
        let fake = FakeAuthorizer::default();
        fake.allow(Action::IssueApiKey, &owner_resource_prn(&owner));
        // `ListApiKeys` never allowed.
        let (svc, _keys, service_accounts, ..) = new_service(fake);
        let sa_id = seed_service_account(&service_accounts, owner, 501);
        let actor = actor_prn(1);

        let err = svc.list(&actor, &sa_id, Page::new(None, None).unwrap()).await.unwrap_err();
        assert_eq!(err, TenancyError::Forbidden);
    }

    #[tokio::test]
    async fn list_missing_service_account_is_not_found() {
        let (svc, ..) = new_service(FakeAuthorizer::default());
        let missing = PrincipalId::from_prn(Prn::build("iam", "", None, "principal", Uuid::from_u128(999)).unwrap());
        let actor = actor_prn(1);
        let err = svc.list(&actor, &missing, Page::new(None, None).unwrap()).await.unwrap_err();
        assert_eq!(err, TenancyError::NotFound);
    }
}
