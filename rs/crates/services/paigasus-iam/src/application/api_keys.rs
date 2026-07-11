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

use crate::adapters::api_keys::ApiKeyValidationCache;
use crate::application::authorize::Authorize;
use crate::application::error::TenancyError;
use crate::application::pagination::Page;
use crate::config::ApiKeyConfig;
use chrono::{DateTime, Duration, Utc};
use paigasus_iam_core::authz::model::root_prn;
use paigasus_iam_core::{
    Action, ApiKey, ApiKeyId, ApiKeyRepository, ApiKeyStatus, Clock, GrantScope, IdGenerator, KeyEntropy, NewApiKey, PrincipalId, RoleGrantStore, SecretHasher, ServiceAccountRepository,
    TenancyNodeRef, display_prefix, format_token,
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
    ids: I,
    clock: C,
    config: ApiKeyConfig,
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
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        keys: K,
        service_accounts: S,
        grants: Arc<dyn RoleGrantStore>,
        authorize: Authorize,
        hasher: H,
        entropy: E,
        cache: Arc<dyn ApiKeyValidationCache>,
        ids: I,
        clock: C,
        config: ApiKeyConfig,
    ) -> Self {
        Self {
            keys,
            service_accounts,
            grants,
            authorize,
            hasher,
            entropy,
            cache,
            ids,
            clock,
            config,
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

        self.keys.issue(&key, &hash).await?;
        Ok(NewApiKey { key, plaintext })
    }

    /// `expires_at.or_else`'s fallback: `now + config.default_expiry_days` if configured, else
    /// `None` (non-expiring until revoked — spec §11).
    fn default_expires_at(&self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        self.config.default_expiry_days.map(|days| now + Duration::days(i64::from(days)))
    }

    /// Revokes `key_id`: `NotFound` if it doesn't exist; finds its service account (`NotFound`
    /// if that's somehow gone too — belt-and-braces, the FK guarantees it exists in practice);
    /// authorizes `Action::RevokeApiKey` AT the SA's owner node; persists the revocation; then
    /// evicts the key's cached validation (mirrors `ServiceAccountService::archive`'s own
    /// cache-evict step, spec §9/D5 "revocation-vs-cache honesty" — without this, a just-revoked
    /// key that was already cached as valid would keep authenticating until its cache entry
    /// expires on its own).
    pub async fn revoke(&self, actor: &Prn, key_id: ApiKeyId) -> Result<(), TenancyError> {
        let (key, _hash) = self.keys.find_by_id(key_id).await?.ok_or(TenancyError::NotFound)?;
        let sa = self.service_accounts.find(&key.service_account_id).await?.ok_or(TenancyError::NotFound)?;
        self.authorize.check(actor, Action::RevokeApiKey, &owner_resource_prn(&sa.account.owner)).await?;

        let now = self.clock.now();
        self.keys.revoke(key_id, now).await?;
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
    use crate::application::fakes::{FakeAuthorizer, FakeSecretHasher, FixedClock, InMemoryApiKeys, InMemoryRoleGrants, InMemoryServiceAccounts, SeqIds, SeqKeyEntropy};
    use paigasus_iam_core::{OrganizationId, PrincipalStatus, RoleGrant, ServiceAccount, parse_token};
    use uuid::Uuid;

    fn actor_prn(n: u128) -> Prn {
        Prn::build("iam", "", None, "principal", Uuid::from_u128(n)).unwrap()
    }

    fn owner_org(n: u128) -> TenancyNodeRef {
        TenancyNodeRef::Organization(OrganizationId::from_uuid(Uuid::from_u128(n)))
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
        let keys = InMemoryApiKeys::default();
        let service_accounts = InMemoryServiceAccounts::default();
        let grants = Arc::new(InMemoryRoleGrants::default());
        let cache = Arc::new(MemoryApiKeyCache::new(30));
        let svc = ApiKeyService::new(
            keys.clone(),
            service_accounts.clone(),
            grants.clone(),
            Authorize::new(Arc::new(fake)),
            FakeSecretHasher,
            SeqKeyEntropy::default(),
            cache.clone(),
            SeqIds::default(),
            FixedClock::default(),
            ApiKeyConfig::default(),
        );
        (svc, keys, service_accounts, grants, cache)
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

    /// D15, THE key test: the SA holds an `org_admin` grant at `org_x`. `actor` is authorized
    /// for `IssueApiKey` at the SA's owner but was NEVER granted `GrantRole` at `org_x` — so
    /// `issue` must deny, `Forbidden`, and must not persist a key (an actor who could not grant
    /// `org_x`'s role to a third party must not be able to mint a key that wields it).
    #[tokio::test]
    async fn issue_denied_when_actor_cannot_grant_all_sa_roles() {
        let owner = owner_org(1);
        let org_x = owner_org(2);
        let fake = FakeAuthorizer::default();
        fake.allow(Action::IssueApiKey, &owner_resource_prn(&owner));
        // Deliberately NOT allowed: `GrantRole` @ org_x.

        let (svc, keys, service_accounts, grants, _cache) = new_service(fake);
        let sa_id = seed_service_account(&service_accounts, owner, 200);
        seed_role_grant(&grants, 900, &sa_id, "org_admin", GrantScope::Node(org_x));
        let actor = actor_prn(1);

        let err = svc.issue(&actor, &sa_id, owner_org(1), None, Vec::new(), Vec::new()).await.unwrap_err();
        assert_eq!(err, TenancyError::Forbidden);
        assert!(keys.0.lock().unwrap().is_empty(), "a D15-denied issue must not persist any key");
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
