// SPDX-License-Identifier: Apache-2.0

//! `AuthenticateToken` use case: verifies a bearer token, resolves (and optionally
//! just-in-time provisions) the local principal, and assembles the full introspection
//! context. Spec §6.1/§6.2 (D5 per-issuer JIT flag, D9 one-transaction provisioning, D10
//! introspect never provisions, D13 hot path resolves only — no membership fetch).

use paigasus_iam_core::{
    Authenticator, AuthnError, AuthnPrincipal, AuthzError, Clock, ConflictKind, Credential, Email, ExternalIdentity, ExternalIdentityRepository, IdGenerator, Issuer, MembershipRepository, Principal,
    PrincipalContext, PrincipalId, PrincipalKind, PrincipalRepository, PrincipalStatus, ProvisioningDefect, RepositoryError, RoleGrantRef, RoleGrantStore, User, ValidatedClaims,
};
use std::collections::HashMap;
use std::sync::Arc;

/// Whether `resolve` may just-in-time provision an unknown `(issuer, subject)` identity.
/// The middleware calls `resolve(.., Enabled)`; `Introspect` always calls `resolve(..,
/// Disabled)` (D10) — an unauthenticated, middleware-exempt endpoint must not have a
/// user-creation side effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provisioning {
    Enabled,
    Disabled,
}

/// Per-issuer JIT-provisioning flags (D5). Config (Task 13) supplies the default
/// (`true`) per issuer; this policy stores exactly what it is built from — it has no
/// opinion of its own about issuers absent from the list (`allows` reports `false`).
#[derive(Clone)]
pub struct JitPolicy {
    allowed: HashMap<Issuer, bool>,
}

impl JitPolicy {
    #[must_use]
    pub fn from_issuers(issuers: &[(Issuer, bool)]) -> Self {
        JitPolicy {
            allowed: issuers.iter().cloned().collect(),
        }
    }

    /// `false` for an issuer absent from the configured set — mirrors the token-validation
    /// rule that an unconfigured issuer is never trusted, so it can never JIT either.
    #[must_use]
    pub fn allows(&self, issuer: &Issuer) -> bool {
        self.allowed.get(issuer).copied().unwrap_or(false)
    }
}

/// Wraps any `RepositoryError` as `AuthnError::Backend` — the catch-all for repository
/// failures this use case doesn't specifically interpret (§6.2 rule 4: "other repo errors
/// -> Backend").
fn backend(err: RepositoryError) -> AuthnError {
    AuthnError::Backend(Box::new(err))
}

/// Wraps an `AuthzError` as `AuthnError::Backend`. Separate from `backend` above because
/// the grant store's error type is not `RepositoryError`. Spec D6: a grant-store failure
/// fails the call — it must never degrade to an empty grant list.
fn backend_authz(err: AuthzError) -> AuthnError {
    AuthnError::Backend(Box::new(err))
}

/// Generic-by-value over the ports it depends on, mirroring the M1 use cases
/// (`CreateUser` et al.): the composition root instantiates this once per concrete adapter
/// set (Task 14).
#[derive(Clone)]
pub struct AuthenticateToken<A, E, P, M, I, C> {
    authenticator: A,
    identities: E,
    principals: P,
    memberships: M,
    /// Spec D3: a trait object, not a generic parameter — the composition root clones the
    /// one `Arc` it already composes into `PolicySnapshot`, exactly as `RoleService` does
    /// (`application/roles.rs:91`).
    grants: Arc<dyn RoleGrantStore>,
    id_gen: I,
    clock: C,
    jit: JitPolicy,
}

impl<A, E, P, M, I, C> AuthenticateToken<A, E, P, M, I, C>
where
    A: Authenticator,
    E: ExternalIdentityRepository,
    P: PrincipalRepository,
    M: MembershipRepository,
    I: IdGenerator,
    C: Clock,
{
    #[allow(clippy::too_many_arguments)]
    pub fn new(authenticator: A, identities: E, principals: P, memberships: M, grants: Arc<dyn RoleGrantStore>, id_gen: I, clock: C, jit: JitPolicy) -> Self {
        AuthenticateToken {
            authenticator,
            identities,
            principals,
            memberships,
            grants,
            id_gen,
            clock,
            jit,
        }
    }

    /// Verifies `token` and resolves it to a local principal (§6.1). `provisioning`
    /// controls whether an unknown `(issuer, subject)` gets just-in-time provisioned
    /// (`Enabled`, the authenticated-request path) or rejected (`Disabled`, `Introspect`'s
    /// D10 read-only guarantee). JIT additionally requires the issuer's `JitPolicy` flag —
    /// an issuer with JIT disabled never provisions even under `Enabled`.
    pub async fn resolve(&self, token: &str, provisioning: Provisioning) -> Result<AuthnPrincipal, AuthnError> {
        let claims = self.authenticator.authenticate(token).await?;

        let principal_id = match self.identities.find_by_issuer_subject(&claims.issuer, &claims.subject).await.map_err(backend)? {
            Some(identity) => identity.principal_id,
            None => match provisioning {
                Provisioning::Disabled => return Err(AuthnError::IdentityNotProvisioned),
                Provisioning::Enabled => {
                    if !self.jit.allows(&claims.issuer) {
                        return Err(AuthnError::IdentityNotProvisioned);
                    }
                    self.jit_provision(&claims).await?
                }
            },
        };

        // The principal must exist — the external_identity/principal FK guarantees it in
        // production; a `None` here is a backend inconsistency, not a business outcome.
        let principal = self
            .principals
            .find_principal(&principal_id)
            .await
            .map_err(backend)?
            .ok_or_else(|| AuthnError::Backend(Box::<dyn std::error::Error + Send + Sync>::from("principal missing for a resolved external identity")))?;

        // Forward-looking guard (§3.3): `PrincipalStatus` has only `Active` in M2, so this
        // branch is unreachable today — specified now so a later suspend/disable milestone
        // needs no authn change.
        if principal.status != PrincipalStatus::Active {
            return Err(AuthnError::PrincipalInactive);
        }

        Ok(AuthnPrincipal {
            principal_id,
            kind: principal.kind,
            status: principal.status,
            credential: Credential::Oidc {
                issuer: claims.issuer,
                subject: claims.subject,
                expires_at: claims.expires_at,
            },
        })
    }

    /// The full context for an ALREADY-RESOLVED principal — the bearer-enforced path's peer of
    /// `introspect`, which resolves a token first.
    ///
    /// `WhoAmI` (SMA-632) uses this. `AuthEnforce`/`require_bearer` have already verified the
    /// bearer, provisioned the caller and seeded the bootstrap grant by the time a handler runs,
    /// so re-verifying the token here would repeat a JWKS-backed verification and could answer
    /// differently from the middleware that let the request through.
    ///
    /// `role_grants` carries the principal's own grants, sorted by `(scope_prn, role_key)`
    /// (SMA-633). `Introspect` and `WhoAmI` both read this one struct, so both report them.
    pub async fn context_for(&self, principal: AuthnPrincipal) -> Result<PrincipalContext, AuthnError> {
        let memberships = crate::application::principal_context::load_all_memberships(&self.memberships, &principal.principal_id).await?;

        // Spec D3/D4/D7: the principal's own grants, projected to the wire type and sorted.
        // `list_by_principal` is unbounded and unordered (`PgRoleGrantStore` issues no
        // `ORDER BY`), so the sort is what makes the response deterministic. The pair is a
        // total order in the database: `uq_role_grant_principal_role_scope`.
        let mut role_grants: Vec<RoleGrantRef> = self
            .grants
            .list_by_principal(&principal.principal_id)
            .await
            .map_err(backend_authz)?
            .into_iter()
            .map(|g| RoleGrantRef {
                scope_prn: g.scope.canonical_prn(),
                role_key: g.role_key,
            })
            .collect();
        role_grants.sort_by(|a, b| (&a.scope_prn, &a.role_key).cmp(&(&b.scope_prn, &b.role_key)));

        Ok(PrincipalContext { principal, memberships, role_grants })
    }

    /// Full authorization context for a request (§6.1): `resolve(.., Disabled)` (D10, never
    /// provisions) plus `context_for`'s memberships.
    pub async fn introspect(&self, token: &str) -> Result<PrincipalContext, AuthnError> {
        let principal = self.resolve(token, Provisioning::Disabled).await?;
        self.context_for(principal).await
    }

    /// Just-in-time provisioning (§6.2). `email` is required (absent, or unparseable ->
    /// `ProvisioningFailed(MissingEmail)`); `display_name` is the `name` claim, falling back
    /// to the email's local part; `locale`/`zoneinfo` pass through untouched. One call to
    /// `ExternalIdentityRepository::provision` spans principal + user + external_identity in
    /// a single transaction (D9). A lost race (`Conflict(ExternalIdentityExists)`) re-reads
    /// the winner's row and proceeds with it — no orphan principal/user, no auto-linking by
    /// email (D5): an email conflict fails provisioning instead.
    async fn jit_provision(&self, claims: &ValidatedClaims) -> Result<PrincipalId, AuthnError> {
        let email = match claims.email.as_deref().map(Email::parse) {
            Some(Ok(email)) => email,
            _ => return Err(AuthnError::ProvisioningFailed(ProvisioningDefect::MissingEmail)),
        };
        let local_part = email.as_str().split('@').next().unwrap_or_default().to_string();
        let display_name = claims.name.clone().unwrap_or(local_part);

        let now = self.clock.now();
        let principal_id = self.id_gen.new_principal_id();
        let principal = Principal::new(principal_id.clone(), PrincipalKind::User, PrincipalStatus::Active, now, now);
        let user = User::new(principal_id.clone(), email, display_name, claims.locale.clone(), claims.zoneinfo.clone(), now, now);
        let identity = ExternalIdentity {
            id: self.id_gen.new_external_identity_id(),
            principal_id: principal_id.clone(),
            issuer: claims.issuer.clone(),
            subject: claims.subject.clone(),
            created_at: now,
            updated_at: now,
        };

        match self.identities.provision(&principal, &user, &identity).await {
            Ok(()) => Ok(principal_id),
            Err(RepositoryError::Conflict(ConflictKind::ExternalIdentityExists)) => self
                .identities
                .find_by_issuer_subject(&claims.issuer, &claims.subject)
                .await
                .map_err(backend)?
                .map(|winner| winner.principal_id)
                .ok_or_else(|| AuthnError::Backend(Box::<dyn std::error::Error + Send + Sync>::from("external identity vanished after a provisioning conflict"))),
            Err(RepositoryError::Conflict(ConflictKind::EmailTaken)) => Err(AuthnError::ProvisioningFailed(ProvisioningDefect::EmailConflict)),
            Err(other) => Err(backend(other)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::fakes::{FixedClock, InMemoryMembershipRepository, InMemoryRoleGrants, SeqIds};
    use async_trait::async_trait;
    use chrono::{TimeZone, Utc};
    use paigasus_iam_core::{ApiKeyId, GrantScope, Membership, MembershipRecord, RoleGrant, Stamp, TenancyNodeRef, TokenDefect, Transaction};
    use paigasus_kernel::Prn;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use uuid::Uuid;

    fn principal_id(n: u128) -> PrincipalId {
        PrincipalId::from_prn(Prn::build("iam", "", None, "principal", Uuid::from_u128(n)).unwrap())
    }

    fn epoch() -> chrono::DateTime<Utc> {
        Utc.timestamp_opt(0, 0).unwrap()
    }

    fn claims_with_profile(issuer: &str, subject: &str, email: Option<&str>, name: Option<&str>, locale: Option<&str>, zoneinfo: Option<&str>) -> ValidatedClaims {
        ValidatedClaims {
            issuer: Issuer::parse(issuer).unwrap(),
            subject: subject.to_string(),
            audiences: vec!["aud".to_string()],
            expires_at: Utc.timestamp_opt(2_000_000_000, 0).unwrap(),
            email: email.map(str::to_string),
            name: name.map(str::to_string),
            locale: locale.map(str::to_string),
            zoneinfo: zoneinfo.map(str::to_string),
        }
    }

    fn claims(issuer: &str, subject: &str, email: Option<&str>, name: Option<&str>) -> ValidatedClaims {
        claims_with_profile(issuer, subject, email, name, None, None)
    }

    /// Shared backing store for the authn in-memory fakes — mirrors `application::fakes`'
    /// `TenancyStore`: `InMemoryIdentities` and `InMemoryPrincipals` each clone a handle onto
    /// the *same* data, because `ExternalIdentityRepository::provision` (D9) writes both a
    /// principal+user and an external_identity in what is, in production, one transaction.
    #[derive(Clone, Default)]
    struct AuthnStore {
        principals: Arc<Mutex<HashMap<Uuid, (Principal, User)>>>,
        identities: Arc<Mutex<HashMap<(String, String), ExternalIdentity>>>,
    }

    #[derive(Clone, Default)]
    struct InMemoryPrincipals(AuthnStore);

    #[async_trait]
    impl PrincipalRepository for InMemoryPrincipals {
        async fn create_user(&self, principal: &Principal, user: &User) -> Result<(), RepositoryError> {
            self.0.principals.lock().unwrap().insert(principal.id.uuid(), (principal.clone(), user.clone()));
            Ok(())
        }
        // Txn-scoped twin (SMA-446, Slice B Task B7 — the `CreateUser::execute` reference
        // pattern): `AuthenticateToken` never calls `create_user`/`create_user_in` itself (it
        // provisions via `ExternalIdentityRepository::provision`, D9), but the fake must still
        // implement the full port surface — `tx` is ignored, mirroring `create_user` above.
        async fn create_user_in(&self, _tx: &dyn Transaction, principal: &Principal, user: &User) -> Result<(), RepositoryError> {
            self.create_user(principal, user).await
        }
        async fn find_user(&self, id: &PrincipalId) -> Result<Option<(Principal, User)>, RepositoryError> {
            Ok(self.0.principals.lock().unwrap().get(&id.uuid()).cloned())
        }
        async fn find_principal(&self, id: &PrincipalId) -> Result<Option<Principal>, RepositoryError> {
            Ok(self.0.principals.lock().unwrap().get(&id.uuid()).map(|(p, _)| p.clone()))
        }
    }

    /// Faithful to `ExternalIdentityRepository::provision`'s doc contract: duplicate
    /// `(issuer, subject)` -> `Conflict(ExternalIdentityExists)`; duplicate email across
    /// principals -> `Conflict(EmailTaken)` (identity conflict checked first, matching D9's
    /// "no auto-link" priority — a race on the identity key is resolved before an email
    /// coincidence is even considered).
    #[derive(Clone, Default)]
    struct InMemoryIdentities(AuthnStore);

    #[async_trait]
    impl ExternalIdentityRepository for InMemoryIdentities {
        async fn find_by_issuer_subject(&self, issuer: &Issuer, subject: &str) -> Result<Option<ExternalIdentity>, RepositoryError> {
            let key = (issuer.as_str().to_string(), subject.to_string());
            Ok(self.0.identities.lock().unwrap().get(&key).cloned())
        }

        async fn provision(&self, principal: &Principal, user: &User, identity: &ExternalIdentity) -> Result<(), RepositoryError> {
            let key = (identity.issuer.as_str().to_string(), identity.subject.clone());
            let mut identities = self.0.identities.lock().unwrap();
            if identities.contains_key(&key) {
                return Err(RepositoryError::Conflict(ConflictKind::ExternalIdentityExists));
            }
            let mut principals = self.0.principals.lock().unwrap();
            if principals.values().any(|(_, u)| u.email == user.email) {
                return Err(RepositoryError::Conflict(ConflictKind::EmailTaken));
            }
            principals.insert(principal.id.uuid(), (principal.clone(), user.clone()));
            identities.insert(key, identity.clone());
            Ok(())
        }
    }

    /// Wraps `InMemoryIdentities` to reproduce a lost provisioning race: the first
    /// `find_by_issuer_subject` call (the use case's initial "is this known?" check) reports
    /// a miss regardless of the store, simulating a lookup that ran *before* a concurrent
    /// request's row landed; every subsequent call answers honestly from the shared store
    /// (which the test pre-seeds with the "winner" row).
    struct RaceOnceIdentities {
        inner: InMemoryIdentities,
        missed_once: AtomicBool,
    }

    impl RaceOnceIdentities {
        fn new(inner: InMemoryIdentities) -> Self {
            RaceOnceIdentities {
                inner,
                missed_once: AtomicBool::new(false),
            }
        }
    }

    #[async_trait]
    impl ExternalIdentityRepository for RaceOnceIdentities {
        async fn find_by_issuer_subject(&self, issuer: &Issuer, subject: &str) -> Result<Option<ExternalIdentity>, RepositoryError> {
            if !self.missed_once.swap(true, Ordering::SeqCst) {
                return Ok(None);
            }
            self.inner.find_by_issuer_subject(issuer, subject).await
        }

        async fn provision(&self, principal: &Principal, user: &User, identity: &ExternalIdentity) -> Result<(), RepositoryError> {
            self.inner.provision(principal, user, identity).await
        }
    }

    /// `list_by_principal` fake: a plain, insertion-ordered map keyed by principal uuid.
    /// The other `MembershipRepository` methods are unused by `AuthenticateToken` (it only
    /// ever calls `list_by_principal`, D13) and panic if invoked so a wiring mistake fails
    /// loudly instead of silently returning nonsense.
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
        async fn attach(&self, _membership: &Membership, _stamp: &Stamp) -> Result<MembershipRecord, RepositoryError> {
            unimplemented!("AuthenticateToken never calls attach")
        }
        async fn attach_in(&self, _tx: &dyn Transaction, _membership: &Membership, _stamp: &Stamp) -> Result<MembershipRecord, RepositoryError> {
            unimplemented!("AuthenticateToken never calls attach_in")
        }
        async fn find(&self, _id: Uuid) -> Result<Option<MembershipRecord>, RepositoryError> {
            unimplemented!("AuthenticateToken never calls find")
        }
        async fn detach(&self, _id: Uuid) -> Result<(), RepositoryError> {
            unimplemented!("AuthenticateToken never calls detach")
        }
        async fn detach_in(&self, _tx: &dyn Transaction, _id: Uuid) -> Result<Vec<MembershipRecord>, RepositoryError> {
            unimplemented!("AuthenticateToken never calls detach_in")
        }
        async fn list_by_principal(&self, principal: Uuid, limit: u64, offset: u64) -> Result<Vec<MembershipRecord>, RepositoryError> {
            let rows = self.rows.lock().unwrap();
            let items = rows.get(&principal).cloned().unwrap_or_default();
            Ok(items.into_iter().skip(offset as usize).take(limit as usize).collect())
        }
        async fn list_by_node(&self, _node: &TenancyNodeRef, _limit: u64, _offset: u64) -> Result<Vec<MembershipRecord>, RepositoryError> {
            unimplemented!("AuthenticateToken never calls list_by_node")
        }
    }

    /// One-shot scripted `Authenticator`: yields its stored result exactly once, so tests
    /// assert `authenticate` was called at most once by construction rather than by counting.
    struct FakeAuthenticator {
        result: Mutex<Option<Result<ValidatedClaims, AuthnError>>>,
    }

    impl FakeAuthenticator {
        fn ok(claims: ValidatedClaims) -> Self {
            FakeAuthenticator { result: Mutex::new(Some(Ok(claims))) }
        }
        fn err(err: AuthnError) -> Self {
            FakeAuthenticator { result: Mutex::new(Some(Err(err))) }
        }
    }

    #[async_trait]
    impl Authenticator for FakeAuthenticator {
        async fn authenticate(&self, _token: &str) -> Result<ValidatedClaims, AuthnError> {
            self.result.lock().unwrap().take().expect("FakeAuthenticator.authenticate called more than once")
        }
    }

    /// Proves `context_for_does_not_re_authenticate`: any call panics the test immediately, so
    /// a passing test is definitive evidence `context_for` never called `authenticate` — it
    /// takes an already-resolved principal and must not verify a token at all.
    struct PanicIfCalledAuthenticator;

    #[async_trait]
    impl Authenticator for PanicIfCalledAuthenticator {
        async fn authenticate(&self, _token: &str) -> Result<ValidatedClaims, AuthnError> {
            panic!("Authenticator must not be called by context_for — it takes an already-resolved principal")
        }
    }

    /// Proves `invalid_token_short_circuits`: any call panics the test immediately, so a
    /// passing test is definitive evidence `resolve` never reached the repositories.
    struct PanicIfCalledIdentities;
    #[async_trait]
    impl ExternalIdentityRepository for PanicIfCalledIdentities {
        async fn find_by_issuer_subject(&self, _issuer: &Issuer, _subject: &str) -> Result<Option<ExternalIdentity>, RepositoryError> {
            panic!("ExternalIdentityRepository must not be called once authenticate() has failed")
        }
        async fn provision(&self, _principal: &Principal, _user: &User, _identity: &ExternalIdentity) -> Result<(), RepositoryError> {
            panic!("ExternalIdentityRepository must not be called once authenticate() has failed")
        }
    }

    struct PanicIfCalledPrincipals;
    #[async_trait]
    impl PrincipalRepository for PanicIfCalledPrincipals {
        async fn create_user(&self, _principal: &Principal, _user: &User) -> Result<(), RepositoryError> {
            panic!("PrincipalRepository must not be called once authenticate() has failed")
        }
        async fn create_user_in(&self, _tx: &dyn Transaction, _principal: &Principal, _user: &User) -> Result<(), RepositoryError> {
            panic!("PrincipalRepository must not be called once authenticate() has failed")
        }
        async fn find_user(&self, _id: &PrincipalId) -> Result<Option<(Principal, User)>, RepositoryError> {
            panic!("PrincipalRepository must not be called once authenticate() has failed")
        }
        async fn find_principal(&self, _id: &PrincipalId) -> Result<Option<Principal>, RepositoryError> {
            panic!("PrincipalRepository must not be called once authenticate() has failed")
        }
    }

    /// Every method fails. Only `list_by_principal` is reached by `introspect`; the rest
    /// satisfy the trait. Mirrors `roles.rs`'s own `FailingGrantStore`.
    struct FailingGrants;

    #[async_trait]
    impl RoleGrantStore for FailingGrants {
        async fn grant(&self, _g: &RoleGrant) -> Result<(), AuthzError> {
            Err(AuthzError::Backend("boom".into()))
        }
        async fn revoke(&self, _id: Uuid) -> Result<(), AuthzError> {
            Err(AuthzError::Backend("boom".into()))
        }
        async fn grant_in(&self, _tx: &dyn Transaction, _g: &RoleGrant) -> Result<(), AuthzError> {
            Err(AuthzError::Backend("boom".into()))
        }
        async fn revoke_in(&self, _tx: &dyn Transaction, _id: Uuid) -> Result<bool, AuthzError> {
            Err(AuthzError::Backend("boom".into()))
        }
        async fn list_all(&self) -> Result<Vec<RoleGrant>, AuthzError> {
            Err(AuthzError::Backend("boom".into()))
        }
        async fn list_by_principal(&self, _p: &PrincipalId) -> Result<Vec<RoleGrant>, AuthzError> {
            Err(AuthzError::Backend("boom".into()))
        }
        async fn find(&self, _id: Uuid) -> Result<Option<RoleGrant>, AuthzError> {
            Err(AuthzError::Backend("boom".into()))
        }
    }

    /// Returns `list_by_principal` in a FIXED, code-chosen order — never `InMemoryRoleGrants`'s
    /// `HashMap` iteration order, which reseeds its `RandomState` every process and made an
    /// earlier version of the sort mutation check a coin flip (SMA-633 review finding 1: the
    /// first `.sort_by` deletion run passed, and only 3 of 5 re-runs failed). Only
    /// `list_by_principal` is reached by `introspect`; the rest satisfy the trait.
    struct FixedOrderGrants(Vec<RoleGrant>);

    #[async_trait]
    impl RoleGrantStore for FixedOrderGrants {
        async fn grant(&self, _g: &RoleGrant) -> Result<(), AuthzError> {
            unimplemented!("this fake only exercises list_by_principal")
        }
        async fn revoke(&self, _id: Uuid) -> Result<(), AuthzError> {
            unimplemented!("this fake only exercises list_by_principal")
        }
        async fn grant_in(&self, _tx: &dyn Transaction, _g: &RoleGrant) -> Result<(), AuthzError> {
            unimplemented!("this fake only exercises list_by_principal")
        }
        async fn revoke_in(&self, _tx: &dyn Transaction, _id: Uuid) -> Result<bool, AuthzError> {
            unimplemented!("this fake only exercises list_by_principal")
        }
        async fn list_all(&self) -> Result<Vec<RoleGrant>, AuthzError> {
            unimplemented!("this fake only exercises list_by_principal")
        }
        async fn list_by_principal(&self, _p: &PrincipalId) -> Result<Vec<RoleGrant>, AuthzError> {
            Ok(self.0.clone())
        }
        async fn find(&self, _id: Uuid) -> Result<Option<RoleGrant>, AuthzError> {
            unimplemented!("this fake only exercises list_by_principal")
        }
    }

    #[tokio::test]
    async fn known_identity_resolves_without_provisioning() {
        let store = AuthnStore::default();
        let issuer = Issuer::parse("https://idp.example.com").unwrap();
        let pid = principal_id(1);
        let principal = Principal::new(pid.clone(), PrincipalKind::User, PrincipalStatus::Active, epoch(), epoch());
        let user = User::new(pid.clone(), Email::parse("alice@example.com").unwrap(), "Alice".into(), None, None, epoch(), epoch());
        store.principals.lock().unwrap().insert(pid.uuid(), (principal, user));
        store.identities.lock().unwrap().insert(
            (issuer.as_str().to_string(), "sub-1".to_string()),
            ExternalIdentity {
                id: Uuid::from_u128(99),
                principal_id: pid.clone(),
                issuer: issuer.clone(),
                subject: "sub-1".into(),
                created_at: epoch(),
                updated_at: epoch(),
            },
        );

        let uc = AuthenticateToken::new(
            FakeAuthenticator::ok(claims("https://idp.example.com", "sub-1", Some("alice@example.com"), Some("Alice"))),
            InMemoryIdentities(store.clone()),
            InMemoryPrincipals(store.clone()),
            InMemoryMemberships::default(),
            Arc::new(InMemoryRoleGrants::default()),
            SeqIds::default(),
            FixedClock::default(),
            JitPolicy::from_issuers(&[(issuer.clone(), true)]),
        );

        let resolved = uc.resolve("token", Provisioning::Disabled).await.unwrap();
        assert_eq!(resolved.principal_id, pid);
        assert_eq!(resolved.status, PrincipalStatus::Active);
        assert_eq!(resolved.issuer(), Some(&issuer));
        assert_eq!(resolved.subject(), Some("sub-1"));
    }

    #[tokio::test]
    async fn unknown_identity_jit_provisions_user() {
        let store = AuthnStore::default();
        let issuer = Issuer::parse("https://idp.example.com").unwrap();
        let uc = AuthenticateToken::new(
            FakeAuthenticator::ok(claims("https://idp.example.com", "sub-2", Some("bob@example.com"), Some("Bob"))),
            InMemoryIdentities(store.clone()),
            InMemoryPrincipals(store.clone()),
            InMemoryMemberships::default(),
            Arc::new(InMemoryRoleGrants::default()),
            SeqIds::default(),
            FixedClock::default(),
            JitPolicy::from_issuers(&[(issuer.clone(), true)]),
        );

        let resolved = uc.resolve("token", Provisioning::Enabled).await.unwrap();

        let (_, user) = store.principals.lock().unwrap().get(&resolved.principal_id.uuid()).cloned().unwrap();
        assert_eq!(user.email.as_str(), "bob@example.com");
        assert_eq!(user.display_name, "Bob");
        assert!(store.identities.lock().unwrap().contains_key(&(issuer.as_str().to_string(), "sub-2".to_string())));
    }

    #[tokio::test]
    async fn jit_disabled_issuer_returns_identity_not_provisioned() {
        let store = AuthnStore::default();
        let issuer = Issuer::parse("https://idp.example.com").unwrap();
        let uc = AuthenticateToken::new(
            FakeAuthenticator::ok(claims("https://idp.example.com", "sub-3", Some("x@example.com"), None)),
            InMemoryIdentities(store.clone()),
            InMemoryPrincipals(store.clone()),
            InMemoryMemberships::default(),
            Arc::new(InMemoryRoleGrants::default()),
            SeqIds::default(),
            FixedClock::default(),
            JitPolicy::from_issuers(&[(issuer.clone(), false)]),
        );

        let err = uc.resolve("token", Provisioning::Enabled).await.unwrap_err();
        assert!(matches!(err, AuthnError::IdentityNotProvisioned));
        assert!(store.identities.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn introspect_never_provisions() {
        let store = AuthnStore::default();
        let issuer = Issuer::parse("https://idp.example.com").unwrap();
        // JIT is enabled for the issuer — but Introspect must still refuse (D10).
        let uc = AuthenticateToken::new(
            FakeAuthenticator::ok(claims("https://idp.example.com", "sub-4", Some("y@example.com"), None)),
            InMemoryIdentities(store.clone()),
            InMemoryPrincipals(store.clone()),
            InMemoryMemberships::default(),
            Arc::new(InMemoryRoleGrants::default()),
            SeqIds::default(),
            FixedClock::default(),
            JitPolicy::from_issuers(&[(issuer.clone(), true)]),
        );

        let err = uc.introspect("token").await.unwrap_err();
        assert!(matches!(err, AuthnError::IdentityNotProvisioned));
        assert!(store.identities.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn missing_email_fails_provisioning() {
        let store = AuthnStore::default();
        let issuer = Issuer::parse("https://idp.example.com").unwrap();
        let uc = AuthenticateToken::new(
            FakeAuthenticator::ok(claims("https://idp.example.com", "sub-5", None, Some("No Email"))),
            InMemoryIdentities(store.clone()),
            InMemoryPrincipals(store.clone()),
            InMemoryMemberships::default(),
            Arc::new(InMemoryRoleGrants::default()),
            SeqIds::default(),
            FixedClock::default(),
            JitPolicy::from_issuers(&[(issuer.clone(), true)]),
        );

        let err = uc.resolve("token", Provisioning::Enabled).await.unwrap_err();
        assert!(matches!(err, AuthnError::ProvisioningFailed(ProvisioningDefect::MissingEmail)));
        assert!(store.principals.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn missing_name_falls_back_to_email_local_part() {
        let store = AuthnStore::default();
        let issuer = Issuer::parse("https://idp.example.com").unwrap();
        let uc = AuthenticateToken::new(
            FakeAuthenticator::ok(claims("https://idp.example.com", "sub-lp", Some("carol.smith@example.com"), None)),
            InMemoryIdentities(store.clone()),
            InMemoryPrincipals(store.clone()),
            InMemoryMemberships::default(),
            Arc::new(InMemoryRoleGrants::default()),
            SeqIds::default(),
            FixedClock::default(),
            JitPolicy::from_issuers(&[(issuer, true)]),
        );

        let resolved = uc.resolve("token", Provisioning::Enabled).await.unwrap();

        let (_, user) = store.principals.lock().unwrap().get(&resolved.principal_id.uuid()).cloned().unwrap();
        assert_eq!(user.display_name, "carol.smith", "display_name must fall back to the email local part when the name claim is absent");
    }

    #[tokio::test]
    async fn unparseable_email_fails_provisioning_as_missing_email() {
        // An email claim that is PRESENT but unparseable (no '@') is the same defect as an
        // absent one: MissingEmail, and nothing is provisioned.
        let store = AuthnStore::default();
        let issuer = Issuer::parse("https://idp.example.com").unwrap();
        let uc = AuthenticateToken::new(
            FakeAuthenticator::ok(claims("https://idp.example.com", "sub-bad-email", Some("not-an-email"), Some("Broken"))),
            InMemoryIdentities(store.clone()),
            InMemoryPrincipals(store.clone()),
            InMemoryMemberships::default(),
            Arc::new(InMemoryRoleGrants::default()),
            SeqIds::default(),
            FixedClock::default(),
            JitPolicy::from_issuers(&[(issuer, true)]),
        );

        let err = uc.resolve("token", Provisioning::Enabled).await.unwrap_err();
        assert!(matches!(err, AuthnError::ProvisioningFailed(ProvisioningDefect::MissingEmail)));
        assert!(store.principals.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn locale_and_zoneinfo_pass_through_to_the_provisioned_user() {
        let store = AuthnStore::default();
        let issuer = Issuer::parse("https://idp.example.com").unwrap();
        let uc = AuthenticateToken::new(
            FakeAuthenticator::ok(claims_with_profile(
                "https://idp.example.com",
                "sub-loc",
                Some("dora@example.com"),
                Some("Dora"),
                Some("de-DE"),
                Some("Europe/Berlin"),
            )),
            InMemoryIdentities(store.clone()),
            InMemoryPrincipals(store.clone()),
            InMemoryMemberships::default(),
            Arc::new(InMemoryRoleGrants::default()),
            SeqIds::default(),
            FixedClock::default(),
            JitPolicy::from_issuers(&[(issuer, true)]),
        );

        let resolved = uc.resolve("token", Provisioning::Enabled).await.unwrap();

        let (_, user) = store.principals.lock().unwrap().get(&resolved.principal_id.uuid()).cloned().unwrap();
        assert_eq!(user.locale.as_deref(), Some("de-DE"));
        assert_eq!(user.timezone.as_deref(), Some("Europe/Berlin"), "the zoneinfo claim lands on User.timezone untouched");
    }

    #[tokio::test]
    async fn email_conflict_maps_to_provisioning_failed() {
        let store = AuthnStore::default();
        let issuer = Issuer::parse("https://idp.example.com").unwrap();
        let existing_id = principal_id(7);
        let existing_principal = Principal::new(existing_id.clone(), PrincipalKind::User, PrincipalStatus::Active, epoch(), epoch());
        let existing_user = User::new(existing_id.clone(), Email::parse("taken@example.com").unwrap(), "Existing".into(), None, None, epoch(), epoch());
        store.principals.lock().unwrap().insert(existing_id.uuid(), (existing_principal, existing_user));

        let uc = AuthenticateToken::new(
            FakeAuthenticator::ok(claims("https://idp.example.com", "sub-6", Some("taken@example.com"), Some("New Guy"))),
            InMemoryIdentities(store.clone()),
            InMemoryPrincipals(store.clone()),
            InMemoryMemberships::default(),
            Arc::new(InMemoryRoleGrants::default()),
            SeqIds::default(),
            FixedClock::default(),
            JitPolicy::from_issuers(&[(issuer.clone(), true)]),
        );

        let err = uc.resolve("token", Provisioning::Enabled).await.unwrap_err();
        assert!(matches!(err, AuthnError::ProvisioningFailed(ProvisioningDefect::EmailConflict)));
        assert!(store.identities.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn provision_race_loser_reuses_winner_row() {
        let store = AuthnStore::default();
        let issuer = Issuer::parse("https://idp.example.com").unwrap();
        let winner_id = principal_id(42);
        let winner_principal = Principal::new(winner_id.clone(), PrincipalKind::User, PrincipalStatus::Active, epoch(), epoch());
        let winner_user = User::new(winner_id.clone(), Email::parse("racer@example.com").unwrap(), "Racer".into(), None, None, epoch(), epoch());
        store.principals.lock().unwrap().insert(winner_id.uuid(), (winner_principal, winner_user));
        store.identities.lock().unwrap().insert(
            (issuer.as_str().to_string(), "sub-race".to_string()),
            ExternalIdentity {
                id: Uuid::from_u128(4242),
                principal_id: winner_id.clone(),
                issuer: issuer.clone(),
                subject: "sub-race".into(),
                created_at: epoch(),
                updated_at: epoch(),
            },
        );

        let uc = AuthenticateToken::new(
            FakeAuthenticator::ok(claims("https://idp.example.com", "sub-race", Some("racer2@example.com"), Some("Racer Two"))),
            RaceOnceIdentities::new(InMemoryIdentities(store.clone())),
            InMemoryPrincipals(store.clone()),
            InMemoryMemberships::default(),
            Arc::new(InMemoryRoleGrants::default()),
            SeqIds::default(),
            FixedClock::default(),
            JitPolicy::from_issuers(&[(issuer.clone(), true)]),
        );

        let resolved = uc.resolve("token", Provisioning::Enabled).await.unwrap();
        assert_eq!(resolved.principal_id, winner_id);
        // The loser's own (different-email) user must never have been persisted.
        assert!(!store.principals.lock().unwrap().values().any(|(_, u)| u.email.as_str() == "racer2@example.com"));
    }

    #[tokio::test]
    async fn introspect_pages_through_memberships() {
        let store = AuthnStore::default();
        let issuer = Issuer::parse("https://idp.example.com").unwrap();
        let pid = principal_id(1);
        let principal = Principal::new(pid.clone(), PrincipalKind::User, PrincipalStatus::Active, epoch(), epoch());
        let user = User::new(pid.clone(), Email::parse("paged@example.com").unwrap(), "Paged".into(), None, None, epoch(), epoch());
        store.principals.lock().unwrap().insert(pid.uuid(), (principal, user));
        store.identities.lock().unwrap().insert(
            (issuer.as_str().to_string(), "sub-page".to_string()),
            ExternalIdentity {
                id: Uuid::from_u128(2),
                principal_id: pid.clone(),
                issuer: issuer.clone(),
                subject: "sub-page".into(),
                created_at: epoch(),
                updated_at: epoch(),
            },
        );

        let memberships = InMemoryMemberships::default();
        memberships.seed(pid.uuid(), 450);

        let uc = AuthenticateToken::new(
            FakeAuthenticator::ok(claims("https://idp.example.com", "sub-page", Some("paged@example.com"), Some("Paged"))),
            InMemoryIdentities(store.clone()),
            InMemoryPrincipals(store.clone()),
            memberships,
            Arc::new(InMemoryRoleGrants::default()),
            SeqIds::default(),
            FixedClock::default(),
            JitPolicy::from_issuers(&[(issuer.clone(), true)]),
        );

        let ctx = uc.introspect("token").await.unwrap();
        assert_eq!(ctx.memberships.len(), 450);
        // This principal holds no grants, so the list is empty for that reason — not
        // because the field is hardcoded. `introspect_returns_the_principals_role_grants`
        // is what proves the field is populated.
        assert!(ctx.role_grants.is_empty());
    }

    #[tokio::test]
    async fn invalid_token_short_circuits() {
        let uc = AuthenticateToken::new(
            FakeAuthenticator::err(AuthnError::InvalidToken(TokenDefect::BadSignature)),
            PanicIfCalledIdentities,
            PanicIfCalledPrincipals,
            InMemoryMemberships::default(),
            Arc::new(FailingGrants),
            SeqIds::default(),
            FixedClock::default(),
            JitPolicy::from_issuers(&[]),
        );

        let err = uc.resolve("token", Provisioning::Enabled).await.unwrap_err();
        assert!(matches!(err, AuthnError::InvalidToken(TokenDefect::BadSignature)));
    }

    /// Builds an `AuthenticateToken` whose authenticator panics if `authenticate` is ever
    /// called (proving `context_for` never re-verifies a token), with one membership seeded
    /// for the returned, already-resolved `AuthnPrincipal`.
    async fn context_for_fixture() -> (
        AuthenticateToken<PanicIfCalledAuthenticator, InMemoryIdentities, InMemoryPrincipals, InMemoryMembershipRepository, SeqIds, FixedClock>,
        AuthnPrincipal,
    ) {
        let store = AuthnStore::default();
        let issuer = Issuer::parse("https://idp.example.com").unwrap();
        let pid = principal_id(1);

        let memberships = InMemoryMembershipRepository::default();
        memberships.seed_for(&pid, 1);

        let uc = AuthenticateToken::new(
            PanicIfCalledAuthenticator,
            InMemoryIdentities(store.clone()),
            InMemoryPrincipals(store.clone()),
            memberships,
            Arc::new(InMemoryRoleGrants::default()),
            SeqIds::default(),
            FixedClock::default(),
            JitPolicy::from_issuers(&[(issuer.clone(), true)]),
        );

        let principal = AuthnPrincipal {
            principal_id: pid,
            kind: PrincipalKind::User,
            status: PrincipalStatus::Active,
            credential: Credential::Oidc {
                issuer,
                subject: "sub-1".into(),
                expires_at: epoch(),
            },
        };

        (uc, principal)
    }

    /// `context_for` must NOT verify the token again — it takes an already-resolved principal.
    /// The fake authenticator here would panic if called, so a future implementation that
    /// re-resolves fails this test loudly rather than silently costing a JWKS round trip.
    #[tokio::test]
    async fn context_for_does_not_re_authenticate() {
        let (use_case, principal) = context_for_fixture().await;
        let ctx = use_case.context_for(principal.clone()).await.unwrap();
        assert_eq!(ctx.principal.principal_id, principal.principal_id);
        // This principal holds no grants, so the list is empty for that reason — not
        // because the field is hardcoded. `introspect_returns_the_principals_role_grants`
        // is what proves the field is populated.
        assert!(ctx.role_grants.is_empty());
    }

    #[tokio::test]
    async fn context_for_returns_the_principals_memberships() {
        let (use_case, principal) = context_for_fixture().await;
        let ctx = use_case.context_for(principal).await.unwrap();
        assert_eq!(ctx.memberships.len(), 1);
    }

    /// Helper: a store seeded with one principal and its external identity, returning the
    /// pieces the four grant tests need. Mirrors `introspect_pages_through_memberships`'s
    /// own setup, which predates this helper.
    fn seeded_principal(store: &AuthnStore, issuer: &Issuer, subject: &str) -> PrincipalId {
        let pid = principal_id(1);
        let principal = Principal::new(pid.clone(), PrincipalKind::User, PrincipalStatus::Active, epoch(), epoch());
        let user = User::new(pid.clone(), Email::parse("grants@example.com").unwrap(), "Grants".into(), None, None, epoch(), epoch());
        store.principals.lock().unwrap().insert(pid.uuid(), (principal, user));
        store.identities.lock().unwrap().insert(
            (issuer.as_str().to_string(), subject.to_string()),
            ExternalIdentity {
                id: Uuid::from_u128(7),
                principal_id: pid.clone(),
                issuer: issuer.clone(),
                subject: subject.into(),
                created_at: epoch(),
                updated_at: epoch(),
            },
        );
        pid
    }

    /// A grant at Root and a grant at a node both reach the caller, each carrying the
    /// canonical PRN of its own scope (spec D4).
    #[tokio::test]
    async fn introspect_returns_the_principals_role_grants() {
        let store = AuthnStore::default();
        let issuer = Issuer::parse("https://idp.example.com").unwrap();
        let pid = seeded_principal(&store, &issuer, "sub-grants");

        let org = TenancyNodeRef::from_prn(Prn::parse("prn:pgs:iam:::organization/11111111-1111-1111-1111-111111111111").unwrap()).unwrap();
        let grants = InMemoryRoleGrants::default();
        grants
            .grant(&RoleGrant {
                id: Uuid::from_u128(1),
                principal: pid.clone(),
                role_key: "platform_admin".into(),
                scope: GrantScope::Root,
                linked_policy_id: "lp-1".into(),
                created_at: epoch(),
            })
            .await
            .unwrap();
        grants
            .grant(&RoleGrant {
                id: Uuid::from_u128(2),
                principal: pid.clone(),
                role_key: "org_admin".into(),
                scope: GrantScope::Node(org.clone()),
                linked_policy_id: "lp-2".into(),
                created_at: epoch(),
            })
            .await
            .unwrap();

        let uc = AuthenticateToken::new(
            FakeAuthenticator::ok(claims("https://idp.example.com", "sub-grants", Some("grants@example.com"), Some("Grants"))),
            InMemoryIdentities(store.clone()),
            InMemoryPrincipals(store.clone()),
            InMemoryMemberships::default(),
            Arc::new(grants),
            SeqIds::default(),
            FixedClock::default(),
            JitPolicy::from_issuers(&[(issuer.clone(), true)]),
        );

        let ctx = uc.introspect("token").await.unwrap();
        assert_eq!(ctx.role_grants.len(), 2);
        assert!(ctx.role_grants.iter().any(|g| g.role_key == "platform_admin" && g.scope_prn == GrantScope::Root.canonical_prn()));
        assert!(ctx.role_grants.iter().any(|g| g.role_key == "org_admin" && g.scope_prn == org.canonical()));
    }

    /// Spec D7: the order is `(scope_prn, role_key)` — `scope_prn` PRIMARY, `role_key`
    /// secondary. `grant_org` and `grant_root` deliberately disagree on the two components:
    /// by `scope_prn` alone, `"organization/…"` sorts before `"root/…"` (so `grant_org`
    /// comes first); by `role_key` alone, `"aaa_role"` sorts before `"zzz_role"` (so
    /// `grant_root` would come first). A `sort_by` narrowed to `role_key` only — or deleted
    /// entirely — therefore produces the WRONG order here, unlike the two-scope test above
    /// where `.any()` can't see order at all (SMA-633 review finding 2). The fixture is
    /// returned via `FixedOrderGrants` in a fixed insertion order that also disagrees with
    /// the correct order, so this is not a coin flip on `InMemoryRoleGrants`'s `HashMap`
    /// iteration order either (SMA-633 review finding 1).
    #[tokio::test]
    async fn introspect_sorts_role_grants_deterministically() {
        let store = AuthnStore::default();
        let issuer = Issuer::parse("https://idp.example.com").unwrap();
        let pid = seeded_principal(&store, &issuer, "sub-sorted");

        let org = TenancyNodeRef::from_prn(Prn::parse("prn:pgs:iam:::organization/11111111-1111-1111-1111-111111111111").unwrap()).unwrap();
        let grant_org = RoleGrant {
            id: Uuid::from_u128(1),
            principal: pid.clone(),
            role_key: "zzz_role".into(),
            scope: GrantScope::Node(org),
            linked_policy_id: "lp-1".into(),
            created_at: epoch(),
        };
        let grant_root = RoleGrant {
            id: Uuid::from_u128(2),
            principal: pid.clone(),
            role_key: "aaa_role".into(),
            scope: GrantScope::Root,
            linked_policy_id: "lp-2".into(),
            created_at: epoch(),
        };
        // Fixed insertion order (root, then org) matches neither the correct
        // `(scope_prn, role_key)` order below nor a `role_key`-only order — both wrong
        // orderings are distinguishable failures, not a lucky pass.
        let grants = FixedOrderGrants(vec![grant_root, grant_org]);

        let uc = AuthenticateToken::new(
            FakeAuthenticator::ok(claims("https://idp.example.com", "sub-sorted", Some("grants@example.com"), Some("Grants"))),
            InMemoryIdentities(store.clone()),
            InMemoryPrincipals(store.clone()),
            InMemoryMemberships::default(),
            Arc::new(grants),
            SeqIds::default(),
            FixedClock::default(),
            JitPolicy::from_issuers(&[(issuer.clone(), true)]),
        );

        let ctx = uc.introspect("token").await.unwrap();
        let keys: Vec<&str> = ctx.role_grants.iter().map(|g| g.role_key.as_str()).collect();
        assert_eq!(
            keys,
            vec!["zzz_role", "aaa_role"],
            "scope_prn is the PRIMARY key: organization/… sorts before root/…, so grant_org (zzz_role) must come first even though \
             role_key alone would put grant_root (aaa_role) first"
        );
    }

    /// Spec D6: a grant-store failure fails the call. It must NOT degrade to an empty
    /// list, because a consumer reads an empty list as "this principal holds no grants".
    #[tokio::test]
    async fn introspect_fails_when_the_grant_store_fails() {
        let store = AuthnStore::default();
        let issuer = Issuer::parse("https://idp.example.com").unwrap();
        seeded_principal(&store, &issuer, "sub-broken");

        let uc = AuthenticateToken::new(
            FakeAuthenticator::ok(claims("https://idp.example.com", "sub-broken", Some("grants@example.com"), Some("Grants"))),
            InMemoryIdentities(store.clone()),
            InMemoryPrincipals(store.clone()),
            InMemoryMemberships::default(),
            Arc::new(FailingGrants),
            SeqIds::default(),
            FixedClock::default(),
            JitPolicy::from_issuers(&[(issuer.clone(), true)]),
        );

        let err = uc.introspect("token").await.unwrap_err();
        assert!(matches!(err, AuthnError::Backend(_)), "expected Backend, got {err:?}");
    }

    /// A principal with no grants gets an empty list and a SUCCESSFUL call — the
    /// discriminator against the failure case above.
    #[tokio::test]
    async fn introspect_returns_an_empty_list_for_a_principal_without_grants() {
        let store = AuthnStore::default();
        let issuer = Issuer::parse("https://idp.example.com").unwrap();
        seeded_principal(&store, &issuer, "sub-none");

        let uc = AuthenticateToken::new(
            FakeAuthenticator::ok(claims("https://idp.example.com", "sub-none", Some("grants@example.com"), Some("Grants"))),
            InMemoryIdentities(store.clone()),
            InMemoryPrincipals(store.clone()),
            InMemoryMemberships::default(),
            Arc::new(InMemoryRoleGrants::default()),
            SeqIds::default(),
            FixedClock::default(),
            JitPolicy::from_issuers(&[(issuer.clone(), true)]),
        );

        let ctx = uc.introspect("token").await.unwrap();
        assert!(ctx.role_grants.is_empty());
    }

    /// SMA-666. `context_for` reads the grants of an API-key principal too, not only of an OIDC
    /// principal. `WhoAmI` calls `context_for` for both credential kinds
    /// (`adapters/grpc/authn.rs:105`, `adapters/http/authn.rs:119`), so a service account that
    /// calls `WhoAmI` with an API key gets its grants. Only `IntrospectApiKey` returns an empty
    /// list (SMA-633 D2), and it does not call this function. Before this test, no test read the
    /// grants of an API-key principal, so a change that skipped the grant read for an API key
    /// failed no test.
    #[tokio::test]
    async fn context_for_returns_the_grants_of_an_api_key_principal() {
        let org_prn = "prn:pgs:iam:::organization/22222222-2222-2222-2222-222222222222";
        let pid = principal_id(2);
        let org = TenancyNodeRef::from_prn(Prn::parse(org_prn).unwrap()).unwrap();
        let grants = InMemoryRoleGrants::default();
        grants
            .grant(&RoleGrant {
                id: Uuid::from_u128(3),
                principal: pid.clone(),
                role_key: "org_viewer".into(),
                scope: GrantScope::Node(org),
                linked_policy_id: "lp-3".into(),
                created_at: epoch(),
            })
            .await
            .unwrap();

        let store = AuthnStore::default();
        let uc = AuthenticateToken::new(
            PanicIfCalledAuthenticator,
            InMemoryIdentities(store.clone()),
            InMemoryPrincipals(store.clone()),
            InMemoryMemberships::default(),
            Arc::new(grants),
            SeqIds::default(),
            FixedClock::default(),
            JitPolicy::from_issuers(&[]),
        );

        let principal = AuthnPrincipal {
            principal_id: pid,
            kind: PrincipalKind::ServiceAccount,
            status: PrincipalStatus::Active,
            credential: Credential::ApiKey {
                key_id: ApiKeyId::from_uuid(Uuid::from_u128(2)),
                expires_at: None,
                scope_prn: org_prn.to_string(),
            },
        };

        let ctx = uc.context_for(principal).await.unwrap();
        assert_eq!(
            ctx.role_grants,
            vec![RoleGrantRef {
                scope_prn: org_prn.to_string(),
                role_key: "org_viewer".to_string(),
            }],
            "context_for must read the grants of an API-key principal: WhoAmI reports them for both credential kinds"
        );
    }
}
