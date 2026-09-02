// SPDX-License-Identifier: Apache-2.0

//! `AuthenticateApiKey` use case: validates a presented API-key token (cache-first hot path,
//! spec §9) and resolves it to its service account's `AuthnPrincipal` (SMA-445, M4, Task 18).
//! Mirrors `AuthenticateToken`'s structure (`application/authenticate_token.rs`) — same
//! `AuthnError` vocabulary, same `introspect` paging shape — but for the API-key credential
//! path instead of OIDC.
//!
//! **Cache-first hot path (spec §9/D5):** `resolve` parses the token (`parse_token`, core), then
//! checks `ApiKeyValidationCache::get(key_id)` BEFORE ever touching Postgres. A HIT ALWAYS
//! re-verifies the presented secret against the cached peppered `key_hash`
//! (`hasher.verify(secret, cached.key_hash)`, constant-time) and re-checks `expires_at` + the
//! cached `sa_status` fresh (all computed at read time, never trusted stale) — the two things a
//! hit saves are the DB round-trips (`find_by_id` + `find_principal`), NOT the secret check. The
//! `key_id` a hit is keyed by is a NON-secret token segment (`pgs_sk_<keyid>_<secret>`), so
//! authenticating on a cache hit WITHOUT re-verifying the secret would be an auth-bypass: any
//! secret paired with a cached `key_id` would pass for up to the cache TTL. Caching the stored
//! hash (the SAME peppered HMAC in Postgres, safe to cache — a Redis leak can't validate keys
//! without the pepper) is what lets the hit path stay a real accelerator AND still prove secret
//! possession. `cache` is `Arc<dyn ApiKeyValidationCache>` (not generic-DI) so it is the exact
//! same shared instance `ServiceAccountService::archive`/`ApiKeyService::revoke` evict through —
//! a revoke/archive's eviction is what bounds the (revocation-vs-cache) staleness window to one
//! TTL (`service_accounts.rs`/`cache.rs` module docs).
//!
//! **Unconditional SA-status check on every miss (D16, carry-forward from Task 17's review):**
//! `resolve_uncached` ALWAYS reads the service account's live `Principal` via
//! `PrincipalRepository::find_principal` and asserts `status == PrincipalStatus::Active` before
//! ever caching a positive validation or returning a principal — this is the SOLE thing that
//! stops a key issued for (or belonging to) a Disabled SA from authenticating (mirrors
//! `AuthenticateToken::resolve`'s own principal-status guard, `authenticate_token.rs:127`).
//!
//! **`ApiKey.scope_actions`/`scope_roles` are NEVER read here (spec D4):** they are stored,
//! v1-UNENFORCED metadata (`ApiKeyService::issue`'s own doc, `api_keys.rs`) — authorization comes
//! ONLY from the SA's M3 role grants, later, at authz time. This use case ONLY authenticates.

use crate::adapters::api_keys::{ApiKeyValidationCache, CachedValidation};
use crate::config::ApiKeyConfig;
use paigasus_iam_core::{
    ApiKeyDefect, ApiKeyId, ApiKeyRepository, ApiKeyStatus, AuthnError, AuthnPrincipal, Clock, Credential, MembershipRepository, ParsedToken, PrincipalContext, PrincipalKind, PrincipalRepository,
    PrincipalStatus, RepositoryError, SecretHasher, TokenDefect, parse_token,
};
use std::sync::Arc;

/// `list_by_principal` page size for introspection's membership assembly — identical to
/// `AuthenticateToken`'s own constant (`authenticate_token.rs:49`), same rationale (§6.1).
const MEMBERSHIP_PAGE_SIZE: u64 = 200;

/// Wraps any `RepositoryError` as `AuthnError::Backend` — the catch-all for repository failures
/// this use case doesn't specifically interpret, mirroring `authenticate_token.rs::backend`.
fn backend(err: RepositoryError) -> AuthnError {
    AuthnError::Backend(Box::new(err))
}

/// Maps this file's own [`ApiKeyDefect`] — `parse_token`'s parse failures, plus the
/// revoked/expired/bad-secret outcomes this use case determines from a DB (or cache) read — onto
/// `AuthnError::InvalidToken`'s shared `TokenDefect` vocabulary. `TokenDefect` predates API keys
/// (OIDC-only, `authn.rs`) and stays that way: `AuthnError` is deliberately ONE shape across both
/// credential kinds rather than growing a second, API-key-specific `InvalidToken` variant.
/// `Display` never distinguishes ANY of these (both `ApiKeyDefect` and `AuthnError::InvalidToken`
/// scrub to a single generic message, `api_key.rs`/`authn.rs`) — this mapping only affects
/// `Debug`, i.e. tests/internal logs, so the specific `TokenDefect` chosen per variant is a
/// best-effort nearest-analog, not a load-bearing distinction:
/// - [`ApiKeyDefect::Malformed`] (parse failure, or an unrecognized `key_id`) -> `Malformed`.
/// - [`ApiKeyDefect::BadSecret`] (HMAC verification failed) -> `BadSignature`.
/// - [`ApiKeyDefect::Revoked`]/[`ApiKeyDefect::Expired`] -> `Expired` — neither has a dedicated
///   `TokenDefect` analog; `Expired` is the nearest existing "was valid once, isn't now" concept
///   (as opposed to `Malformed`/`BadSignature`'s "never structurally valid to begin with").
fn invalid_token(defect: ApiKeyDefect) -> AuthnError {
    let kind = match defect {
        ApiKeyDefect::Malformed => TokenDefect::Malformed,
        ApiKeyDefect::BadSecret => TokenDefect::BadSignature,
        ApiKeyDefect::Revoked | ApiKeyDefect::Expired => TokenDefect::Expired,
    };
    AuthnError::InvalidToken(kind)
}

/// Generic-by-value over most of the ports it depends on, mirroring `AuthenticateToken`
/// (M1/M2 use-case DI pattern) — `cache` is the one exception: `Arc<dyn ApiKeyValidationCache>`,
/// the SAME shared handle `ServiceAccountService`/`ApiKeyService` hold (module docs above), so a
/// revoke/archive's `cache.evict` is visible to this use case's `cache.get` without a second
/// cache instance to keep in sync.
#[derive(Clone)]
pub struct AuthenticateApiKey<K, P, H, C, M> {
    keys: K,
    principals: P,
    hasher: H,
    clock: C,
    cache: Arc<dyn ApiKeyValidationCache>,
    memberships: M,
    config: ApiKeyConfig,
}

impl<K, P, H, C, M> AuthenticateApiKey<K, P, H, C, M>
where
    K: ApiKeyRepository,
    P: PrincipalRepository,
    H: SecretHasher,
    C: Clock,
    M: MembershipRepository,
{
    #[allow(clippy::too_many_arguments)]
    pub fn new(keys: K, principals: P, hasher: H, clock: C, cache: Arc<dyn ApiKeyValidationCache>, memberships: M, config: ApiKeyConfig) -> Self {
        AuthenticateApiKey {
            keys,
            principals,
            hasher,
            clock,
            cache,
            memberships,
            config,
        }
    }

    /// Validates `token` and resolves it to the service account's `AuthnPrincipal` (spec §9).
    /// `parse_token` failures (wrong prefix, oversized, malformed keyid/secret encoding) map
    /// straight to `InvalidToken` before any port is touched. Otherwise cache-first: a hit
    /// short-circuits to [`Self::resolve_cached`]; a miss falls through to [`Self::resolve_uncached`],
    /// the full DB validation.
    pub async fn resolve(&self, token: &str) -> Result<AuthnPrincipal, AuthnError> {
        let parsed = parse_token(&self.config.key_prefix, token, self.config.max_token_bytes).map_err(invalid_token)?;

        match self.cache.get(parsed.key_id).await {
            Some(cached) => self.resolve_cached(parsed.key_id, &parsed.secret, cached).await,
            None => self.resolve_uncached(parsed).await,
        }
    }

    /// The cache-HIT path (module docs). The `key_id` a hit is keyed by is a NON-secret token
    /// segment (`pgs_sk_<keyid>_<secret>`), so a hit MUST still prove possession of the secret —
    /// it saves the two DB round-trips (`find_by_id` + `find_principal`), NOT the secret check:
    ///
    /// 1. `hasher.verify(secret, cached.key_hash)` (constant-time, against the SAME peppered hash
    ///    stored in Postgres and cached in [`CachedValidation`]) — a mismatch is an ATTACK (a
    ///    wrong secret against a live cached key), not a revocation: deny `InvalidToken(BadSecret)`
    ///    and do NOT evict (evicting on a wrong-secret guess would let an attacker knock a valid
    ///    key out of the cache, forcing a DB round-trip — a mild DoS lever; a genuine revocation
    ///    evicts via `RevokeApiKey`/`ArchiveServiceAccount`, never from here).
    /// 2. `expires_at` — past `clock.now()` -> evict + `InvalidToken` (a stale positive entry the
    ///    caller is entitled to have age out).
    /// 3. the cached `sa_status` -> `PrincipalInactive` if not `Active`.
    ///
    /// Secret FIRST: a wrong secret is denied without leaking whether the key is expired/disabled.
    async fn resolve_cached(&self, key_id: ApiKeyId, secret: &[u8], cached: CachedValidation) -> Result<AuthnPrincipal, AuthnError> {
        if !self.hasher.verify(secret, &cached.key_hash) {
            return Err(invalid_token(ApiKeyDefect::BadSecret));
        }
        if cached.expires_at.is_some_and(|expires_at| expires_at <= self.clock.now()) {
            self.cache.evict(key_id).await;
            return Err(invalid_token(ApiKeyDefect::Expired));
        }
        if cached.sa_status != PrincipalStatus::Active {
            return Err(AuthnError::PrincipalInactive);
        }

        Ok(AuthnPrincipal {
            principal_id: cached.principal_id,
            kind: PrincipalKind::ServiceAccount,
            status: PrincipalStatus::Active,
            credential: Credential::ApiKey {
                key_id,
                expires_at: cached.expires_at,
                // D11, the load-bearing line: the key's tenancy scope comes straight from the
                // cached validation, so a HIT surfaces `scope_prn` for introspection with NO DB read.
                scope_prn: cached.scope_prn.clone(),
            },
        })
    }

    /// The cache-MISS path: the full DB validation. Order of checks:
    ///
    /// 1. `keys.find_by_id(key_id)` — an unrecognized `key_id` -> `InvalidToken`.
    /// 2. `hasher.verify(secret, stored_hash)` (constant-time) — a mismatch -> `InvalidToken`.
    /// 3. The KEY's own `status == Active` and (`expires_at` is null OR in the future) — else
    ///    `InvalidToken`, best-effort `cache.evict` (belt-and-braces: this path is already a
    ///    cache miss, but a concurrent `put` could have just raced one in).
    /// 4. UNCONDITIONAL: the SA's live `Principal` (`principals.find_principal`) must exist, be
    ///    `PrincipalKind::ServiceAccount`, and be `PrincipalStatus::Active` — the module docs'
    ///    D16 guard. A missing principal or a kind mismatch is a backend data-integrity fault
    ///    (`AuthnError::Backend`, mirroring `authenticate_token.rs`'s own "principal missing"
    ///    guard); `Disabled` is the ordinary, expected outcome (`AuthnError::PrincipalInactive`).
    ///
    /// Only once every check passes does this `cache.put` the validation and best-effort
    /// `touch_last_used` (errors swallowed — never fails the request, module docs).
    async fn resolve_uncached(&self, parsed: ParsedToken) -> Result<AuthnPrincipal, AuthnError> {
        let key_id = parsed.key_id;

        let (api_key, stored_hash) = self.keys.find_by_id(key_id).await.map_err(backend)?.ok_or_else(|| invalid_token(ApiKeyDefect::Malformed))?;

        if !self.hasher.verify(&parsed.secret, &stored_hash) {
            return Err(invalid_token(ApiKeyDefect::BadSecret));
        }

        let now = self.clock.now();
        if api_key.status != ApiKeyStatus::Active {
            self.cache.evict(key_id).await;
            return Err(invalid_token(ApiKeyDefect::Revoked));
        }
        if api_key.expires_at.is_some_and(|expires_at| expires_at <= now) {
            self.cache.evict(key_id).await;
            return Err(invalid_token(ApiKeyDefect::Expired));
        }

        // D16, THE security-critical check (module docs): every miss-path resolve re-reads the
        // SA's live Principal status — a still-Active key belonging to a Disabled SA must never
        // authenticate.
        let sa_principal = self.principals.find_principal(&api_key.service_account_id).await.map_err(backend)?.ok_or_else(|| {
            AuthnError::Backend(Box::<dyn std::error::Error + Send + Sync>::from(
                "service account principal missing for an api key's service_account_id",
            ))
        })?;

        if sa_principal.kind != PrincipalKind::ServiceAccount {
            return Err(AuthnError::Backend(Box::<dyn std::error::Error + Send + Sync>::from(
                "api key's service_account_id does not reference a service-account principal",
            )));
        }
        if sa_principal.status != PrincipalStatus::Active {
            return Err(AuthnError::PrincipalInactive);
        }

        // The key's tenancy scope PRN (`TenancyNodeRef::canonical`, mirroring `ApiKeyDto`'s own
        // `scope_prn`) — computed once and threaded into BOTH the cached validation and the
        // returned credential, so a later cache HIT can return it without re-reading the DB (D11).
        let scope_prn = api_key.scope.canonical();

        // Cache the stored hash (and scope) alongside so the hit path can re-verify the secret and
        // surface the scope without a DB read (`stored_hash` is unused after this — move it in).
        self.cache
            .put(
                key_id,
                &CachedValidation {
                    principal_id: sa_principal.id.clone(),
                    sa_status: sa_principal.status,
                    expires_at: api_key.expires_at,
                    key_hash: stored_hash,
                    scope_prn: scope_prn.clone(),
                },
            )
            .await;

        // Best-effort: off the critical path, errors are swallowed (never fail the request on a
        // touch error).
        let _ = self.keys.touch_last_used(key_id, now, self.config.last_used_throttle_secs).await;

        Ok(AuthnPrincipal {
            principal_id: sa_principal.id,
            kind: PrincipalKind::ServiceAccount,
            status: PrincipalStatus::Active,
            credential: Credential::ApiKey {
                key_id,
                expires_at: api_key.expires_at,
                scope_prn,
            },
        })
    }

    /// Full authorization context for an API-key-authenticated request: `resolve` plus every
    /// membership row, paged internally — mirrors `AuthenticateToken::introspect`'s shape
    /// exactly (`authenticate_token.rs:144-166`), including `role_grants` staying empty (no
    /// current caller populates it from the `RoleGrantStore` here either).
    pub async fn introspect(&self, token: &str) -> Result<PrincipalContext, AuthnError> {
        let principal = self.resolve(token).await?;

        let mut memberships = Vec::new();
        let mut offset = 0u64;
        loop {
            let page = self.memberships.list_by_principal(principal.principal_id.uuid(), MEMBERSHIP_PAGE_SIZE, offset).await.map_err(backend)?;
            let page_len = page.len() as u64;
            memberships.extend(page);
            if page_len < MEMBERSHIP_PAGE_SIZE {
                break;
            }
            offset += MEMBERSHIP_PAGE_SIZE;
        }

        Ok(PrincipalContext {
            principal,
            memberships,
            role_grants: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::api_keys::MemoryApiKeyCache;
    use crate::application::fakes::{FakeSecretHasher, FixedClock, InMemoryApiKeys};
    use async_trait::async_trait;
    use chrono::{Duration, TimeZone, Utc};
    use paigasus_iam_core::{ApiKey, MembershipRecord, Principal, RepositoryError, TenancyNodeRef, User, display_prefix, format_token};
    use paigasus_iam_core::{OrganizationId, PrincipalId};
    use paigasus_kernel::Prn;
    use std::collections::HashMap;
    use std::sync::Mutex;
    use uuid::Uuid;

    fn epoch() -> chrono::DateTime<Utc> {
        Utc.timestamp_opt(0, 0).unwrap()
    }

    fn principal_id(n: u128) -> PrincipalId {
        PrincipalId::from_prn(Prn::build("iam", "", None, "principal", Uuid::from_u128(n)).unwrap())
    }

    fn key_id(n: u128) -> ApiKeyId {
        ApiKeyId::from_uuid(Uuid::from_u128(n))
    }

    fn scope() -> TenancyNodeRef {
        TenancyNodeRef::Organization(OrganizationId::from_uuid(Uuid::from_u128(500)))
    }

    /// A local `PrincipalRepository` fake — this file's own store, mirroring
    /// `authenticate_token.rs`'s private `InMemoryPrincipals` test fake (not shared via
    /// `application::fakes`, same "each file gets its own small store" convention as
    /// `api_keys.rs`'s duplicated `owner_resource_prn`).
    #[derive(Clone, Default)]
    struct InMemoryPrincipals(Arc<Mutex<HashMap<Uuid, Principal>>>);

    impl InMemoryPrincipals {
        fn seed(&self, principal: Principal) {
            self.0.lock().unwrap().insert(principal.id.uuid(), principal);
        }
    }

    #[async_trait]
    impl PrincipalRepository for InMemoryPrincipals {
        async fn create_user(&self, _principal: &Principal, _user: &User) -> Result<(), RepositoryError> {
            unimplemented!("AuthenticateApiKey never calls create_user")
        }
        async fn create_user_in(&self, _tx: &dyn paigasus_iam_core::Transaction, _principal: &Principal, _user: &User) -> Result<(), RepositoryError> {
            unimplemented!("AuthenticateApiKey never calls create_user_in")
        }
        async fn find_user(&self, _id: &PrincipalId) -> Result<Option<(Principal, User)>, RepositoryError> {
            unimplemented!("AuthenticateApiKey never calls find_user")
        }
        async fn find_principal(&self, id: &PrincipalId) -> Result<Option<Principal>, RepositoryError> {
            Ok(self.0.lock().unwrap().get(&id.uuid()).cloned())
        }
    }

    /// Panics on any call — proves a short-circuit test never reached the repository, mirroring
    /// `authenticate_token.rs`'s `PanicIfCalledIdentities`/`PanicIfCalledPrincipals`.
    #[derive(Clone, Default)]
    struct PanicIfCalledApiKeys;
    #[async_trait]
    impl ApiKeyRepository for PanicIfCalledApiKeys {
        async fn issue(&self, _key: &ApiKey, _key_hash: &[u8]) -> Result<(), RepositoryError> {
            panic!("ApiKeyRepository must not be called")
        }
        async fn issue_in(&self, _tx: &dyn paigasus_iam_core::Transaction, _key: &ApiKey, _key_hash: &[u8]) -> Result<(), RepositoryError> {
            panic!("ApiKeyRepository must not be called")
        }
        async fn find_by_id(&self, _id: ApiKeyId) -> Result<Option<(ApiKey, Vec<u8>)>, RepositoryError> {
            panic!("ApiKeyRepository::find_by_id must not be called on a cache hit")
        }
        async fn revoke(&self, _id: ApiKeyId, _now: chrono::DateTime<Utc>) -> Result<(), RepositoryError> {
            panic!("ApiKeyRepository must not be called")
        }
        async fn revoke_in(&self, _tx: &dyn paigasus_iam_core::Transaction, _id: ApiKeyId, _now: chrono::DateTime<Utc>) -> Result<bool, RepositoryError> {
            panic!("ApiKeyRepository must not be called")
        }
        async fn list_by_service_account(&self, _sa: &PrincipalId, _limit: u64, _offset: u64) -> Result<Vec<ApiKey>, RepositoryError> {
            panic!("ApiKeyRepository must not be called")
        }
        async fn list_ids_by_service_account(&self, _sa: &PrincipalId) -> Result<Vec<ApiKeyId>, RepositoryError> {
            panic!("ApiKeyRepository must not be called")
        }
        async fn touch_last_used(&self, _id: ApiKeyId, _now: chrono::DateTime<Utc>, _throttle_secs: u64) -> Result<(), RepositoryError> {
            panic!("ApiKeyRepository::touch_last_used must not be called on a cache hit")
        }
    }

    #[derive(Clone, Default)]
    struct PanicIfCalledPrincipals;
    #[async_trait]
    impl PrincipalRepository for PanicIfCalledPrincipals {
        async fn create_user(&self, _principal: &Principal, _user: &User) -> Result<(), RepositoryError> {
            panic!("PrincipalRepository must not be called")
        }
        async fn create_user_in(&self, _tx: &dyn paigasus_iam_core::Transaction, _principal: &Principal, _user: &User) -> Result<(), RepositoryError> {
            panic!("PrincipalRepository must not be called")
        }
        async fn find_user(&self, _id: &PrincipalId) -> Result<Option<(Principal, User)>, RepositoryError> {
            panic!("PrincipalRepository must not be called")
        }
        async fn find_principal(&self, _id: &PrincipalId) -> Result<Option<Principal>, RepositoryError> {
            panic!("PrincipalRepository::find_principal must not be called on a cache hit")
        }
    }

    /// `list_by_principal` fake: a plain, insertion-ordered map keyed by principal uuid —
    /// mirrors `authenticate_token.rs`'s own `InMemoryMemberships` test fake. The other
    /// `MembershipRepository` methods are unused by `AuthenticateApiKey` (only ever calls
    /// `list_by_principal`) and panic if invoked.
    #[derive(Default)]
    struct InMemoryMemberships {
        rows: Mutex<HashMap<Uuid, Vec<MembershipRecord>>>,
    }

    impl InMemoryMemberships {
        fn seed(&self, principal: Uuid, count: usize) {
            let now = epoch();
            let mut rows = self.rows.lock().unwrap();
            let entry = rows.entry(principal).or_default();
            for i in 0..count {
                entry.push(MembershipRecord {
                    id: Uuid::from_u128(i as u128 + 1),
                    principal_prn: format!("prn:pgs:iam:::principal/{principal}"),
                    node_prn: format!("prn:pgs:iam:::organization/{i}"),
                    created_at: now,
                    created_by: None,
                });
            }
        }
    }

    #[async_trait]
    impl MembershipRepository for InMemoryMemberships {
        async fn attach(&self, _membership: &paigasus_iam_core::Membership, _stamp: &paigasus_iam_core::Stamp) -> Result<MembershipRecord, RepositoryError> {
            unimplemented!("AuthenticateApiKey never calls attach")
        }
        async fn attach_in(
            &self,
            _tx: &dyn paigasus_iam_core::Transaction,
            _membership: &paigasus_iam_core::Membership,
            _stamp: &paigasus_iam_core::Stamp,
        ) -> Result<MembershipRecord, RepositoryError> {
            unimplemented!("AuthenticateApiKey never calls attach_in")
        }
        async fn find(&self, _id: Uuid) -> Result<Option<MembershipRecord>, RepositoryError> {
            unimplemented!("AuthenticateApiKey never calls find")
        }
        async fn detach(&self, _id: Uuid) -> Result<(), RepositoryError> {
            unimplemented!("AuthenticateApiKey never calls detach")
        }
        async fn detach_in(&self, _tx: &dyn paigasus_iam_core::Transaction, _id: Uuid) -> Result<Vec<MembershipRecord>, RepositoryError> {
            unimplemented!("AuthenticateApiKey never calls detach_in")
        }
        async fn list_by_principal(&self, principal: Uuid, limit: u64, offset: u64) -> Result<Vec<MembershipRecord>, RepositoryError> {
            let rows = self.rows.lock().unwrap();
            let items = rows.get(&principal).cloned().unwrap_or_default();
            Ok(items.into_iter().skip(offset as usize).take(limit as usize).collect())
        }
        async fn list_by_node(&self, _node: &TenancyNodeRef, _limit: u64, _offset: u64) -> Result<Vec<MembershipRecord>, RepositoryError> {
            unimplemented!("AuthenticateApiKey never calls list_by_node")
        }
    }

    /// Seeds a fully valid, Active key (secret `[7u8; 32]`, `FakeSecretHasher`'s identity hash)
    /// for an Active `ServiceAccount` principal, plus the plaintext token that resolves it.
    /// Returns `(plaintext, key_id, sa_principal_id)`.
    fn seed_active_key(keys: &InMemoryApiKeys, principals: &InMemoryPrincipals, n: u128, expires_at: Option<chrono::DateTime<Utc>>) -> (String, ApiKeyId, PrincipalId) {
        let sa_id = principal_id(n);
        principals.seed(Principal::new(sa_id.clone(), PrincipalKind::ServiceAccount, PrincipalStatus::Active, epoch(), epoch()));

        let id = key_id(n);
        let secret = [7u8; 32];
        let hash = FakeSecretHasher.hash(&secret);
        let key = ApiKey {
            id,
            service_account_id: sa_id.clone(),
            scope: scope(),
            prefix: display_prefix("pgs_sk_", id),
            status: ApiKeyStatus::Active,
            expires_at,
            last_used_at: None,
            created_at: epoch(),
            revoked_at: None,
            scope_actions: Vec::new(),
            scope_roles: Vec::new(),
        };
        keys.0.lock().unwrap().insert(id, (key, hash));
        let plaintext = format_token("pgs_sk_", id, &secret);
        (plaintext, id, sa_id)
    }

    #[allow(clippy::type_complexity)]
    fn new_service(
        keys: InMemoryApiKeys,
        principals: InMemoryPrincipals,
        cache: Arc<dyn ApiKeyValidationCache>,
        memberships: InMemoryMemberships,
    ) -> AuthenticateApiKey<InMemoryApiKeys, InMemoryPrincipals, FakeSecretHasher, FixedClock, InMemoryMemberships> {
        AuthenticateApiKey::new(keys, principals, FakeSecretHasher, FixedClock::default(), cache, memberships, ApiKeyConfig::default())
    }

    #[tokio::test]
    async fn valid_key_resolves_to_sa_principal() {
        let keys = InMemoryApiKeys::default();
        let principals = InMemoryPrincipals::default();
        let (plaintext, id, sa_id) = seed_active_key(&keys, &principals, 1, None);
        let svc = new_service(keys, principals, Arc::new(MemoryApiKeyCache::new(30)), InMemoryMemberships::default());

        let resolved = svc.resolve(&plaintext).await.unwrap();
        assert_eq!(resolved.principal_id, sa_id);
        assert_eq!(resolved.kind, PrincipalKind::ServiceAccount);
        assert_eq!(resolved.status, PrincipalStatus::Active);
        assert!(matches!(resolved.credential, Credential::ApiKey { key_id: k, expires_at: None, .. } if k == id));
    }

    #[tokio::test]
    async fn revoked_key_denied() {
        let keys = InMemoryApiKeys::default();
        let principals = InMemoryPrincipals::default();
        let (plaintext, id, _sa_id) = seed_active_key(&keys, &principals, 2, None);
        {
            let mut map = keys.0.lock().unwrap();
            let (key, _hash) = map.get_mut(&id).unwrap();
            key.status = ApiKeyStatus::Revoked;
            key.revoked_at = Some(epoch());
        }
        let svc = new_service(keys, principals, Arc::new(MemoryApiKeyCache::new(30)), InMemoryMemberships::default());

        let err = svc.resolve(&plaintext).await.unwrap_err();
        assert!(matches!(err, AuthnError::InvalidToken(_)));
    }

    #[tokio::test]
    async fn expired_key_denied() {
        let keys = InMemoryApiKeys::default();
        let principals = InMemoryPrincipals::default();
        let past = epoch() - Duration::seconds(60);
        let (plaintext, ..) = seed_active_key(&keys, &principals, 3, Some(past));
        let svc = new_service(keys, principals, Arc::new(MemoryApiKeyCache::new(30)), InMemoryMemberships::default());

        let err = svc.resolve(&plaintext).await.unwrap_err();
        assert!(matches!(err, AuthnError::InvalidToken(_)));
    }

    #[tokio::test]
    async fn wrong_secret_denied() {
        let keys = InMemoryApiKeys::default();
        let principals = InMemoryPrincipals::default();
        let (_plaintext, id, _sa_id) = seed_active_key(&keys, &principals, 4, None);
        let tampered = format_token("pgs_sk_", id, &[9u8; 32]);
        let svc = new_service(keys, principals, Arc::new(MemoryApiKeyCache::new(30)), InMemoryMemberships::default());

        let err = svc.resolve(&tampered).await.unwrap_err();
        assert!(matches!(err, AuthnError::InvalidToken(_)));
    }

    #[tokio::test]
    async fn unknown_key_id_denied() {
        let keys = InMemoryApiKeys::default();
        let principals = InMemoryPrincipals::default();
        let plaintext = format_token("pgs_sk_", key_id(999), &[1u8; 32]);
        let svc = new_service(keys, principals, Arc::new(MemoryApiKeyCache::new(30)), InMemoryMemberships::default());

        let err = svc.resolve(&plaintext).await.unwrap_err();
        assert!(matches!(err, AuthnError::InvalidToken(_)));
    }

    #[tokio::test]
    async fn malformed_token_short_circuits() {
        let svc = AuthenticateApiKey::new(
            PanicIfCalledApiKeys,
            PanicIfCalledPrincipals,
            FakeSecretHasher,
            FixedClock::default(),
            Arc::new(MemoryApiKeyCache::new(30)) as Arc<dyn ApiKeyValidationCache>,
            InMemoryMemberships::default(),
            ApiKeyConfig::default(),
        );

        let err = svc.resolve("not-even-shaped-like-a-key").await.unwrap_err();
        assert!(matches!(err, AuthnError::InvalidToken(TokenDefect::Malformed)));
    }

    /// THE carry-forward test (Task 17's review): a still-Active key whose SA principal is
    /// `Disabled` must be denied `PrincipalInactive` — proves the unconditional D16 check in
    /// `resolve_uncached` runs on every miss-path resolve, not just at issuance time.
    #[tokio::test]
    async fn disabled_sa_denies_live_key() {
        let keys = InMemoryApiKeys::default();
        let principals = InMemoryPrincipals::default();
        let (plaintext, ..) = seed_active_key(&keys, &principals, 5, None);
        let sa_id = principal_id(5);
        principals.seed(Principal::new(sa_id, PrincipalKind::ServiceAccount, PrincipalStatus::Disabled, epoch(), epoch()));
        let svc = new_service(keys, principals, Arc::new(MemoryApiKeyCache::new(30)), InMemoryMemberships::default());

        let err = svc.resolve(&plaintext).await.unwrap_err();
        assert!(matches!(err, AuthnError::PrincipalInactive));
    }

    /// A disabled SA must ALSO be denied when the key's OWN status is validly Active and the
    /// deny happens on the very FIRST resolve — no key was ever cached for a disabled SA
    /// (`resolve_uncached` returns before `cache.put`).
    #[tokio::test]
    async fn disabled_sa_is_never_cached() {
        let keys = InMemoryApiKeys::default();
        let principals = InMemoryPrincipals::default();
        let (plaintext, id, _sa_id) = seed_active_key(&keys, &principals, 6, None);
        let sa_id = principal_id(6);
        principals.seed(Principal::new(sa_id, PrincipalKind::ServiceAccount, PrincipalStatus::Disabled, epoch(), epoch()));
        let cache = Arc::new(MemoryApiKeyCache::new(30));
        let svc = new_service(keys, principals, cache.clone(), InMemoryMemberships::default());

        assert!(svc.resolve(&plaintext).await.is_err());
        assert!(cache.get(id).await.is_none(), "a denied (disabled-SA) resolve must never populate the cache");
    }

    /// The correct secret (`[7u8; 32]`, whose `FakeSecretHasher` identity hash equals itself) is
    /// seeded as the cached `key_hash`. Presenting the matching token resolves on a HIT — and the
    /// `PanicIfCalledApiKeys`/`PanicIfCalledPrincipals` repos prove the hit path never read the DB
    /// (`find_by_id`/`find_principal` panic if called): the cache genuinely saved the two DB
    /// round-trips, while STILL verifying the secret (that verification is the next test).
    #[tokio::test]
    async fn cache_hit_skips_db_but_still_verifies_secret() {
        let id = key_id(7);
        let sa_id = principal_id(7);
        let secret = [7u8; 32];
        let cache = Arc::new(MemoryApiKeyCache::new(30));
        cache
            .put(
                id,
                &CachedValidation {
                    principal_id: sa_id.clone(),
                    sa_status: PrincipalStatus::Active,
                    expires_at: None,
                    key_hash: FakeSecretHasher.hash(&secret),
                    scope_prn: scope().canonical(),
                },
            )
            .await;

        let svc = AuthenticateApiKey::new(
            PanicIfCalledApiKeys,
            PanicIfCalledPrincipals,
            FakeSecretHasher,
            FixedClock::default(),
            cache as Arc<dyn ApiKeyValidationCache>,
            InMemoryMemberships::default(),
            ApiKeyConfig::default(),
        );

        let token = format_token("pgs_sk_", id, &secret);
        let resolved = svc.resolve(&token).await.unwrap();
        assert_eq!(resolved.principal_id, sa_id);
        assert_eq!(resolved.kind, PrincipalKind::ServiceAccount);
    }

    /// THE D11 guard (SMA-446): a cache HIT must return the key's `scope_prn` straight from the
    /// cached validation, with NO DB read. Seeds a `CachedValidation` carrying a known scope,
    /// resolves with `PanicIfCalledApiKeys`/`PanicIfCalledPrincipals` (which panic on ANY DB
    /// call), and asserts the resolved `Credential::ApiKey.scope_prn` equals the cached value —
    /// proving the scope came from the cache, not a `find_by_id`/`find_principal` round-trip (so
    /// the gateway can authorize `InvokeModel` against it without a per-request DB hit).
    #[tokio::test]
    async fn cache_hit_returns_scope_prn_without_a_db_read() {
        let id = key_id(71);
        let sa_id = principal_id(71);
        let secret = [7u8; 32];
        let expected_scope = scope().canonical();
        let cache = Arc::new(MemoryApiKeyCache::new(30));
        cache
            .put(
                id,
                &CachedValidation {
                    principal_id: sa_id.clone(),
                    sa_status: PrincipalStatus::Active,
                    expires_at: None,
                    key_hash: FakeSecretHasher.hash(&secret),
                    scope_prn: expected_scope.clone(),
                },
            )
            .await;

        let svc = AuthenticateApiKey::new(
            PanicIfCalledApiKeys,
            PanicIfCalledPrincipals,
            FakeSecretHasher,
            FixedClock::default(),
            cache as Arc<dyn ApiKeyValidationCache>,
            InMemoryMemberships::default(),
            ApiKeyConfig::default(),
        );

        let token = format_token("pgs_sk_", id, &secret);
        let resolved = svc.resolve(&token).await.unwrap();
        assert_eq!(resolved.principal_id, sa_id);
        match resolved.credential {
            Credential::ApiKey { scope_prn, .. } => {
                assert_eq!(scope_prn, expected_scope, "a cache hit must return the cached scope_prn with no DB read (D11)");
            }
            other => panic!("expected an ApiKey credential, got {other:?}"),
        }
    }

    /// THE regression test for the auth-bypass this fix closes: a token with a VALID (cached)
    /// `key_id` but the WRONG secret must be denied `InvalidToken`, even on a cache hit — the
    /// secret is the credential, `key_id` alone is not. `PanicIfCalled*` proves the deny happens
    /// straight from the cached hash, without any DB read; the entry must NOT be evicted (a
    /// wrong-secret guess is an attack, not a revocation — evicting would be a DoS lever).
    #[tokio::test]
    async fn cache_hit_with_wrong_secret_is_denied() {
        let id = key_id(70);
        let sa_id = principal_id(70);
        let correct = [7u8; 32];
        let cache = Arc::new(MemoryApiKeyCache::new(30));
        cache
            .put(
                id,
                &CachedValidation {
                    principal_id: sa_id,
                    sa_status: PrincipalStatus::Active,
                    expires_at: None,
                    key_hash: FakeSecretHasher.hash(&correct),
                    scope_prn: scope().canonical(),
                },
            )
            .await;

        let svc = AuthenticateApiKey::new(
            PanicIfCalledApiKeys,
            PanicIfCalledPrincipals,
            FakeSecretHasher,
            FixedClock::default(),
            cache.clone() as Arc<dyn ApiKeyValidationCache>,
            InMemoryMemberships::default(),
            ApiKeyConfig::default(),
        );

        // SAME key_id, DIFFERENT secret.
        let forged = format_token("pgs_sk_", id, &[0xFFu8; 32]);
        let err = svc.resolve(&forged).await.unwrap_err();
        assert!(
            matches!(err, AuthnError::InvalidToken(TokenDefect::BadSignature)),
            "a wrong secret on a cache hit must be InvalidToken, not success: {err:?}"
        );
        assert!(cache.get(id).await.is_some(), "a wrong-secret guess must NOT evict the valid cached entry");
    }

    #[tokio::test]
    async fn cache_hit_with_expired_entry_evicts_and_denies() {
        let id = key_id(8);
        let sa_id = principal_id(8);
        let secret = [7u8; 32];
        let cache = Arc::new(MemoryApiKeyCache::new(30));
        cache
            .put(
                id,
                &CachedValidation {
                    principal_id: sa_id,
                    sa_status: PrincipalStatus::Active,
                    expires_at: Some(epoch() - Duration::seconds(1)),
                    key_hash: FakeSecretHasher.hash(&secret),
                    scope_prn: scope().canonical(),
                },
            )
            .await;

        let svc = AuthenticateApiKey::new(
            PanicIfCalledApiKeys,
            PanicIfCalledPrincipals,
            FakeSecretHasher,
            FixedClock::default(),
            cache.clone() as Arc<dyn ApiKeyValidationCache>,
            InMemoryMemberships::default(),
            ApiKeyConfig::default(),
        );

        // Correct secret, so verification passes and the expiry check is actually reached.
        let token = format_token("pgs_sk_", id, &secret);
        let err = svc.resolve(&token).await.unwrap_err();
        assert!(matches!(err, AuthnError::InvalidToken(_)));
        assert!(cache.get(id).await.is_none(), "an expired cache hit must evict itself");
    }

    #[tokio::test]
    async fn cache_hit_with_disabled_sa_status_denies() {
        let id = key_id(9);
        let sa_id = principal_id(9);
        let secret = [7u8; 32];
        let cache = Arc::new(MemoryApiKeyCache::new(30));
        cache
            .put(
                id,
                &CachedValidation {
                    principal_id: sa_id,
                    sa_status: PrincipalStatus::Disabled,
                    expires_at: None,
                    key_hash: FakeSecretHasher.hash(&secret),
                    scope_prn: scope().canonical(),
                },
            )
            .await;

        let svc = AuthenticateApiKey::new(
            PanicIfCalledApiKeys,
            PanicIfCalledPrincipals,
            FakeSecretHasher,
            FixedClock::default(),
            cache as Arc<dyn ApiKeyValidationCache>,
            InMemoryMemberships::default(),
            ApiKeyConfig::default(),
        );

        // Correct secret, so verification passes and the sa_status check is actually reached.
        let token = format_token("pgs_sk_", id, &secret);
        let err = svc.resolve(&token).await.unwrap_err();
        assert!(matches!(err, AuthnError::PrincipalInactive));
    }

    #[tokio::test]
    async fn introspect_pages_through_memberships() {
        let keys = InMemoryApiKeys::default();
        let principals = InMemoryPrincipals::default();
        let (plaintext, _id, sa_id) = seed_active_key(&keys, &principals, 10, None);
        let memberships = InMemoryMemberships::default();
        memberships.seed(sa_id.uuid(), 450);
        let svc = new_service(keys, principals, Arc::new(MemoryApiKeyCache::new(30)), memberships);

        let ctx = svc.introspect(&plaintext).await.unwrap();
        assert_eq!(ctx.memberships.len(), 450);
        assert!(ctx.role_grants.is_empty());
        assert_eq!(ctx.principal.principal_id, sa_id);
    }

    #[tokio::test]
    async fn introspect_denies_like_resolve() {
        let keys = InMemoryApiKeys::default();
        let principals = InMemoryPrincipals::default();
        let (plaintext, ..) = seed_active_key(&keys, &principals, 11, None);
        {
            let mut map = keys.0.lock().unwrap();
            for (key, _hash) in map.values_mut() {
                key.status = ApiKeyStatus::Revoked;
            }
        }
        let svc = new_service(keys, principals, Arc::new(MemoryApiKeyCache::new(30)), InMemoryMemberships::default());

        let err = svc.introspect(&plaintext).await.unwrap_err();
        assert!(matches!(err, AuthnError::InvalidToken(_)));
    }
}
