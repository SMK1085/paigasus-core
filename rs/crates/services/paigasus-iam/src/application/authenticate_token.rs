// SPDX-License-Identifier: Apache-2.0

//! `AuthenticateToken` use case: verifies a bearer token, resolves (and optionally
//! just-in-time provisions) the local principal, and assembles the full introspection
//! context. Spec §6.1/§6.2 (D5 per-issuer JIT flag, D9 one-transaction provisioning, D10
//! introspect never provisions, D13 hot path resolves only — no membership fetch).

use paigasus_iam_core::{
    Authenticator, AuthnError, AuthnPrincipal, AuthzError, Clock, ConflictKind, Credential, Email, ExternalIdentity, ExternalIdentityRepository, IdGenerator, Issuer, MembershipRepository, Principal,
    PrincipalContext, PrincipalId, PrincipalKind, PrincipalRepository, PrincipalStatus, ProvisioningDefect, RepositoryError, RoleGrantRef, RoleGrantStore, TokenDefect, TokenScheme, User,
    ValidatedClaims,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use metrics::counter;
use paigasus_observability::names;

use crate::application::dpop::{DpopProofVerifier, DpopRequest};
use crate::application::log_rate_limit::{LOG_RATE_LIMIT_INTERVAL, LogRateLimiter};

/// Whether `resolve` may just-in-time provision an unknown `(issuer, subject)` identity.
/// The middleware calls `resolve(.., Enabled)`; `Introspect` always calls `resolve(..,
/// Disabled)` (D10) — an unauthenticated, middleware-exempt endpoint must not have a
/// user-creation side effect. Only `Enabled` writes the SMA-707 `info` line for an unknown
/// identity of an issuer with JIT disabled.
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

/// Every `ProvisioningDefect` value. [`prime_jit_provisioning_failures`] registers one series for
/// each entry. `the_defect_array_lists_every_defect_once` fails to compile when a variant is added
/// and not listed.
const PROVISIONING_DEFECTS: [ProvisioningDefect; 2] = [ProvisioningDefect::MissingEmail, ProvisioningDefect::EmailConflict];

/// The `defect` label of `iam_jit_provisioning_failures_total` and the `defect` field of the JIT
/// failure log line (SMA-698 spec 4.3). One exhaustive `match`, no wildcard: the log field, the
/// counter label and the prime use this one function, so they cannot differ, and a new variant
/// does not compile until it has a label.
fn provisioning_defect_label(defect: ProvisioningDefect) -> &'static str {
    match defect {
        ProvisioningDefect::MissingEmail => "missing_email",
        ProvisioningDefect::EmailConflict => "email_conflict",
    }
}

/// Registers every `defect` series of `iam_jit_provisioning_failures_total` at zero (SMA-698
/// spec 4.4). A metrics-rs series first appears at its first increment's value, and `increase()`
/// takes the first sample as its baseline, so without this the first failure is invisible to an
/// `increase() > 0` query. `main` calls this when metrics are on, even if no issuer has JIT on.
pub fn prime_jit_provisioning_failures() {
    for defect in PROVISIONING_DEFECTS {
        counter!(names::IAM_JIT_PROVISIONING_FAILURES_TOTAL, "defect" => provisioning_defect_label(defect)).increment(0);
    }
}

/// The state of the `email` claim when JIT provisioning failed. It never holds the claim value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EmailClaim {
    /// The token has no `email` claim.
    Absent,
    /// The token has an `email` claim, and `Email::parse` refused it.
    Invalid,
}

impl EmailClaim {
    fn label(self) -> &'static str {
        match self {
            EmailClaim::Absent => "absent",
            EmailClaim::Invalid => "invalid",
        }
    }
}

/// A JIT provisioning failure with its detail (SMA-698 spec 4.2). An invalid combination, such as
/// an `EmailConflict` with an `EmailClaim`, cannot be built.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JitFailure {
    MissingEmail(EmailClaim),
    EmailConflict,
}

impl JitFailure {
    fn defect(self) -> ProvisioningDefect {
        match self {
            JitFailure::MissingEmail(_) => ProvisioningDefect::MissingEmail,
            JitFailure::EmailConflict => ProvisioningDefect::EmailConflict,
        }
    }
}

/// What the DPoP scheme brings besides the token (SMA-700 § 4.7). No `Debug`: both arms hold a proof.
pub enum DpopInput {
    /// `Introspect` with a `dpop` context from the gateway.
    Introspect(DpopRequest),
    /// The `IsAuthorized` follow-up: the `dpop` metadata value, as received.
    FollowUp(String),
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
    /// SMA-698 D2: at most one JIT failure line per (issuer, defect) in 10 s. An `Arc`, because
    /// `AppState` clones this use case for each request, and a plain field would reset on each
    /// clone. Keyed by the `defect` label, because `ProvisioningDefect` does not implement `Hash`.
    provisioning_log: Arc<LogRateLimiter<&'static str>>,
    /// SMA-707: at most one JIT-disabled refusal line per issuer in 10 s. An `Arc`, for the same
    /// reason as `provisioning_log`. A separate instance keyed by `()`, because `provisioning_log`
    /// is keyed by the SMA-698 `defect` label, and `jit_disabled` is not a defect (SMA-707 D4).
    not_provisioned_log: Arc<LogRateLimiter<()>>,
    /// SMA-700: the DPoP proof verifier, or `None` when `authn.dpop.enabled` is false. An `Arc`,
    /// so every `AppState` clone shares the one replay store behind it (D3).
    dpop: Option<Arc<DpopProofVerifier>>,
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
            provisioning_log: Arc::new(LogRateLimiter::new(LOG_RATE_LIMIT_INTERVAL)),
            not_provisioned_log: Arc::new(LogRateLimiter::new(LOG_RATE_LIMIT_INTERVAL)),
            dpop: None,
        }
    }

    /// Verifies `token` and resolves it to a local principal (§6.1). `provisioning`
    /// controls whether an unknown `(issuer, subject)` gets just-in-time provisioned
    /// (`Enabled`, the authenticated-request path) or rejected (`Disabled`, `Introspect`'s
    /// D10 read-only guarantee). JIT additionally requires the issuer's `JitPolicy` flag —
    /// an issuer with JIT disabled never provisions even under `Enabled`. That refusal writes one
    /// rate-limited `info` line through `jit_disabled` (SMA-707); the `Disabled` refusal writes none.
    pub async fn resolve(&self, token: &str, provisioning: Provisioning) -> Result<AuthnPrincipal, AuthnError> {
        let claims = self.authenticator.authenticate(token, TokenScheme::Bearer).await?;
        self.resolve_claims(claims, provisioning).await
    }

    /// Everything `resolve` does after the token is verified: the identity lookup, JIT
    /// provisioning, the principal read and its status. Shared by `resolve` and `resolve_dpop`.
    async fn resolve_claims(&self, claims: ValidatedClaims, provisioning: Provisioning) -> Result<AuthnPrincipal, AuthnError> {
        let principal_id = match self.identities.find_by_issuer_subject(&claims.issuer, &claims.subject).await.map_err(backend)? {
            Some(identity) => identity.principal_id,
            None => match provisioning {
                Provisioning::Disabled => return Err(AuthnError::IdentityNotProvisioned),
                Provisioning::Enabled => {
                    if !self.jit.allows(&claims.issuer) {
                        return Err(self.jit_disabled(&claims.issuer));
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

    /// SMA-700: attaches the DPoP proof verifier. `AppState::new` calls this when
    /// `authn.dpop.enabled` is true.
    #[must_use]
    pub fn with_dpop(mut self, verifier: Arc<DpopProofVerifier>) -> Self {
        self.dpop = Some(verifier);
        self
    }

    /// Whether DPoP is on: `AuthEnforce` accepts the follow-up scheme only then.
    #[must_use]
    pub fn dpop_enabled(&self) -> bool {
        self.dpop.is_some()
    }

    /// The DPoP scheme (SMA-700 § 4.7). The order is fixed (D18): the request checks of § 4.8,
    /// `authenticate(.., Dpop)`, the proof check (`Introspect`) or the ticket redeem (the
    /// follow-up), and only then the identity lookup and anything after it. No identity lookup,
    /// provisioning, seeding or API-key branch runs before the proof check. With DPoP off, the
    /// answer is the one a malformed token gets (D11).
    pub async fn resolve_dpop(&self, token: &str, input: DpopInput, provisioning: Provisioning) -> Result<AuthnPrincipal, AuthnError> {
        let Some(verifier) = &self.dpop else {
            return Err(AuthnError::InvalidToken(TokenDefect::Malformed));
        };
        if let DpopInput::Introspect(request) = &input {
            verifier.check_request(request)?;
        }
        let claims = self.authenticator.authenticate(token, TokenScheme::Dpop).await?;
        match &input {
            DpopInput::Introspect(request) => verifier.verify(&claims, token, request)?,
            DpopInput::FollowUp(proof) => verifier.redeem(&claims, token, proof)?,
        }
        self.resolve_claims(claims, provisioning).await
    }

    /// `Introspect` with a `dpop` context (SMA-700 § 4.8): `resolve_dpop` with
    /// `Provisioning::Disabled` (D10 holds), plus `context_for`.
    pub async fn introspect_dpop(&self, token: &str, request: DpopRequest) -> Result<PrincipalContext, AuthnError> {
        let principal = self.resolve_dpop(token, DpopInput::Introspect(request), Provisioning::Disabled).await?;
        self.context_for(principal).await
    }

    /// Just-in-time provisioning (§6.2). `email` is required (absent, or unparseable ->
    /// `ProvisioningFailed(MissingEmail)`); `display_name` is the `name` claim, falling back
    /// to the email's local part; `locale`/`zoneinfo` pass through untouched. One call to
    /// `ExternalIdentityRepository::provision` spans principal + user + external_identity in
    /// a single transaction (D9). A lost race re-reads the winner's row and proceeds with it — no
    /// orphan principal/user. The race has two forms: `Conflict(ExternalIdentityExists)`, and
    /// `Conflict(EmailTaken)` with the identity present at the re-read (SMA-698 D1). No
    /// auto-linking by email (D5): an email conflict with the identity absent fails provisioning.
    async fn jit_provision(&self, claims: &ValidatedClaims) -> Result<PrincipalId, AuthnError> {
        // The `Email::parse` error is dropped here on purpose: `DomainError::InvalidEmail` holds
        // the raw claim, and the helper must never see it (SMA-698 spec 4.3).
        let email = match claims.email.as_deref().map(Email::parse) {
            Some(Ok(email)) => email,
            Some(Err(_)) => return Err(self.provisioning_failed(&claims.issuer, JitFailure::MissingEmail(EmailClaim::Invalid))),
            None => return Err(self.provisioning_failed(&claims.issuer, JitFailure::MissingEmail(EmailClaim::Absent))),
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
            // SMA-698 D1 (spec 4.6). In Postgres, `provision` inserts the `user` row before the
            // `external_identity` row, and `user.email` is unique. The loser of a race between two
            // first logins of ONE identity therefore fails on the email first. If the winner has
            // not committed yet, the loser's insert waits on the unique index until it does. If
            // the winner has already committed, the loser's insert fails at once. The re-read is
            // correct in both cases: it always finds the winner's row.
            Err(RepositoryError::Conflict(ConflictKind::EmailTaken)) => match self.identities.find_by_issuer_subject(&claims.issuer, &claims.subject).await.map_err(backend)? {
                Some(winner) => Ok(winner.principal_id),
                None => Err(self.provisioning_failed(&claims.issuer, JitFailure::EmailConflict)),
            },
            Err(other) => Err(backend(other)),
        }
    }

    /// The only way a JIT failure arm builds its error (SMA-698 spec 4.2). It counts the failure,
    /// asks the rate limiter, writes one `warn` line when admitted, and returns
    /// `ProvisioningFailed`. It never receives the email, the subject, the name or the token, so
    /// the line cannot carry them (spec S2).
    fn provisioning_failed(&self, issuer: &Issuer, failure: JitFailure) -> AuthnError {
        let defect = failure.defect();
        let label = provisioning_defect_label(defect);
        counter!(names::IAM_JIT_PROVISIONING_FAILURES_TOTAL, "defect" => label).increment(1);
        if let Some(suppressed) = self.provisioning_log.admit_at(issuer.as_str(), label, Instant::now()) {
            match failure {
                JitFailure::MissingEmail(claim) => tracing::warn!(
                    defect = label,
                    issuer = issuer.as_str(),
                    email_claim = claim.label(),
                    suppressed,
                    "just-in-time provisioning failed: the access token has no valid email claim; configure the issuer to put a valid email claim into the access token"
                ),
                JitFailure::EmailConflict => tracing::warn!(
                    defect = label,
                    issuer = issuer.as_str(),
                    suppressed,
                    "just-in-time provisioning failed: another user already has this email address, and IAM does not link identities by email"
                ),
            }
        }
        AuthnError::ProvisioningFailed(defect)
    }

    /// The only way the JIT-disabled branch of `resolve` builds its error (SMA-707 spec 4.2). It
    /// asks the rate limiter, writes one `info` line when admitted, and returns
    /// `IdentityNotProvisioned`. It receives only the issuer, so the line cannot carry the
    /// subject, a claim or the token. `info`, not `warn`: JIT off closes the user set of the
    /// issuer, so this refusal is the intended result of the operator's setting (SMA-707 D2).
    fn jit_disabled(&self, issuer: &Issuer) -> AuthnError {
        if let Some(suppressed) = self.not_provisioned_log.admit_at(issuer.as_str(), (), Instant::now()) {
            tracing::info!(
                issuer = issuer.as_str(),
                reason = "jit_disabled",
                suppressed,
                "request refused: the identity is not provisioned, and just-in-time provisioning is disabled for this issuer; IAM creates an identity only by just-in-time provisioning, so set jit_provisioning = true for the issuer to let new users of this issuer sign in"
            );
        }
        AuthnError::IdentityNotProvisioned
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::dpop_replay::InMemoryReplayStore;
    use crate::adapters::id::KernelIdGenerator;
    use crate::application::dpop::{DpopProofVerifier, DpopRequest};
    use crate::application::fakes::{FixedClock, InMemoryMembershipRepository, InMemoryRoleGrants, SeqIds};
    use async_trait::async_trait;
    use chrono::{TimeZone, Utc};
    use metrics_util::debugging::{DebugValue, DebuggingRecorder};
    use paigasus_iam_core::{ApiKeyId, GrantScope, Membership, MembershipRecord, RoleGrant, Stamp, TenancyNodeRef, TokenDefect, Transaction};
    use paigasus_iam_core::{DpopProofChecker, FollowUpClaims, Jkt, ProofClaims, ProofDefect};
    use paigasus_kernel::Prn;
    use paigasus_logging::test_support::capture_logs;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
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
            key_binding: None,
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

    /// The Postgres form of a lost race (SMA-698 spec 4.6). The winner commits all three rows;
    /// the loser's `user` insert fails on the email first, so its `provision` returns
    /// `Conflict(EmailTaken)`. `provision` inserts the winner's identity into the shared store
    /// and then returns that error, so the identity is present at the re-read.
    struct EmailTakenRaceIdentities {
        inner: InMemoryIdentities,
        winner: ExternalIdentity,
    }

    #[async_trait]
    impl ExternalIdentityRepository for EmailTakenRaceIdentities {
        async fn find_by_issuer_subject(&self, issuer: &Issuer, subject: &str) -> Result<Option<ExternalIdentity>, RepositoryError> {
            self.inner.find_by_issuer_subject(issuer, subject).await
        }

        async fn provision(&self, _principal: &Principal, _user: &User, _identity: &ExternalIdentity) -> Result<(), RepositoryError> {
            let key = (self.winner.issuer.as_str().to_string(), self.winner.subject.clone());
            self.inner.0.identities.lock().unwrap().insert(key, self.winner.clone());
            Err(RepositoryError::Conflict(ConflictKind::EmailTaken))
        }
    }

    /// `list_by_principal` fake: a plain, insertion-ordered map keyed by principal uuid.
    /// The other `MembershipRepository` methods are unused by `AuthenticateToken` (it only
    /// ever calls `list_by_principal`, D13) and panic if invoked so a wiring mistake fails
    /// loudly instead of silently returning nonsense.
    #[derive(Clone, Default)]
    struct InMemoryMemberships {
        rows: Arc<Mutex<HashMap<Uuid, Vec<MembershipRecord>>>>,
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
        async fn authenticate(&self, _token: &str, _scheme: TokenScheme) -> Result<ValidatedClaims, AuthnError> {
            self.result.lock().unwrap().take().expect("FakeAuthenticator.authenticate called more than once")
        }
    }

    /// Yields its claims in order, one per call, for tests that resolve more than once. `Clone`
    /// shares the queue, so a cloned use case reads from the same queue (U13).
    #[derive(Clone)]
    struct QueueAuthenticator(Arc<Mutex<VecDeque<ValidatedClaims>>>);

    impl QueueAuthenticator {
        fn new(claims: Vec<ValidatedClaims>) -> Self {
            QueueAuthenticator(Arc::new(Mutex::new(claims.into())))
        }
    }

    #[async_trait]
    impl Authenticator for QueueAuthenticator {
        async fn authenticate(&self, _token: &str, _scheme: TokenScheme) -> Result<ValidatedClaims, AuthnError> {
            Ok(self.0.lock().unwrap().pop_front().expect("QueueAuthenticator ran out of claims"))
        }
    }

    /// The initial lookup misses, and `provision` fails with a repository error that is not a
    /// conflict (U7). `RepositoryError::Backend` wraps a boxed error.
    struct FailingProvisionIdentities;

    #[async_trait]
    impl ExternalIdentityRepository for FailingProvisionIdentities {
        async fn find_by_issuer_subject(&self, _issuer: &Issuer, _subject: &str) -> Result<Option<ExternalIdentity>, RepositoryError> {
            Ok(None)
        }
        async fn provision(&self, _principal: &Principal, _user: &User, _identity: &ExternalIdentity) -> Result<(), RepositoryError> {
            Err(RepositoryError::Backend("database connection lost".into()))
        }
    }

    /// Proves `context_for_does_not_re_authenticate`: any call panics the test immediately, so
    /// a passing test is definitive evidence `context_for` never called `authenticate` — it
    /// takes an already-resolved principal and must not verify a token at all.
    struct PanicIfCalledAuthenticator;

    #[async_trait]
    impl Authenticator for PanicIfCalledAuthenticator {
        async fn authenticate(&self, _token: &str, _scheme: TokenScheme) -> Result<ValidatedClaims, AuthnError> {
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

    /// U5: the lost race in its `ExternalIdentityExists` form resolves to the winner, and it
    /// writes no helper line and makes no series (spec 4.7).
    #[tokio::test]
    async fn provision_race_loser_reuses_winner_row() {
        let (logs, _logs_guard) = capture_logs();
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _metrics_guard = metrics::set_default_local_recorder(&recorder);
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
        let text = logs.text();
        assert!(jit_lines(&text).is_empty(), "a lost race writes no helper line:\n{text}");
        assert_eq!(jit_series(&snapshotter.snapshot().into_vec()), 0, "a lost race is not counted");
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

    type MetricsSnapshot = Vec<(metrics_util::CompositeKey, Option<metrics::Unit>, Option<metrics::SharedString>, DebugValue)>;

    /// The value of `iam_jit_provisioning_failures_total{defect}` in `snapshot`, or `None` when that
    /// series does not exist. It reads the VALUE: a series primed with `increment(0)` also has a
    /// key. `Snapshotter::snapshot` resets the counters it reads, so take ONE snapshot per test.
    fn jit_failures(snapshot: &MetricsSnapshot, defect: &str) -> Option<u64> {
        snapshot.iter().find_map(|(key, _, _, value)| {
            let key = key.key();
            let matches = key.name() == names::IAM_JIT_PROVISIONING_FAILURES_TOTAL && key.labels().any(|label| label.key() == "defect" && label.value() == defect);
            match (matches, value) {
                (false, _) => None,
                (true, DebugValue::Counter(n)) => Some(*n),
                (true, other) => panic!("expected a counter, got {other:?}"),
            }
        })
    }

    /// How many `iam_jit_provisioning_failures_total` series exist in `snapshot`, at any value.
    fn jit_series(snapshot: &MetricsSnapshot) -> usize {
        snapshot.iter().filter(|(key, ..)| key.key().name() == names::IAM_JIT_PROVISIONING_FAILURES_TOTAL).count()
    }

    /// U10 (SMA-698 spec 4.4): the prime registers both `defect` series at zero, so `increase()`
    /// sees the first failure.
    #[test]
    fn prime_registers_both_defect_series_at_zero() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        metrics::with_local_recorder(&recorder, prime_jit_provisioning_failures);
        let snapshot = snapshotter.snapshot().into_vec();
        assert_eq!(jit_failures(&snapshot, "missing_email"), Some(0));
        assert_eq!(jit_failures(&snapshot, "email_conflict"), Some(0));
        assert_eq!(jit_series(&snapshot), 2, "exactly the two defect series");
    }

    /// `PROVISIONING_DEFECTS` names every `ProvisioningDefect` once, with its own label. The
    /// `match` has no wildcard: a new variant stops this test from compiling until someone adds
    /// it here AND to `PROVISIONING_DEFECTS`.
    #[test]
    fn the_defect_array_lists_every_defect_once() {
        fn listed(defect: ProvisioningDefect) -> usize {
            match defect {
                ProvisioningDefect::MissingEmail | ProvisioningDefect::EmailConflict => PROVISIONING_DEFECTS.iter().filter(|d| **d == defect).count(),
            }
        }
        assert_eq!(listed(ProvisioningDefect::MissingEmail), 1);
        assert_eq!(listed(ProvisioningDefect::EmailConflict), 1);
        let labels: std::collections::HashSet<&str> = PROVISIONING_DEFECTS.iter().map(|d| provisioning_defect_label(*d)).collect();
        assert_eq!(labels.len(), PROVISIONING_DEFECTS.len(), "each defect needs its own label");
    }

    const ISSUER: &str = "https://idp.example.com";
    const OTHER_ISSUER: &str = "https://other-idp.example.com";
    /// The fixed prefix of both helper messages. Tests select the helper's lines with it.
    const JIT_LINE: &str = "just-in-time provisioning failed";
    const MISSING_EMAIL_TEXT: &str = "just-in-time provisioning failed: the access token has no valid email claim; configure the issuer to put a valid email claim into the access token";
    const EMAIL_CONFLICT_TEXT: &str = "just-in-time provisioning failed: another user already has this email address, and IAM does not link identities by email";

    /// The helper's lines only. `AppState::new` and other code can write other `warn` lines.
    fn jit_lines(text: &str) -> Vec<&str> {
        text.lines().filter(|line| line.contains(JIT_LINE)).collect()
    }

    /// `true` when `line` carries `key` with `value`. `tracing-subscriber` writes a `&str` field
    /// as `key="value"` and a number as `key=value`; both forms are accepted. The match is on
    /// the whole token: whitespace or the line start must come before it, and whitespace or the
    /// line end must come after it. So `suppressed=1` does not match a line that has
    /// `suppressed=10`.
    fn has_field(line: &str, key: &str, value: &str) -> bool {
        is_whole_token(line, &format!("{key}=\"{value}\"")) || is_whole_token(line, &format!("{key}={value}"))
    }

    /// `true` when `token` occurs in `text` with whitespace, or the text start or end, on both
    /// sides.
    fn is_whole_token(text: &str, token: &str) -> bool {
        let mut search_from = 0;
        while let Some(found_at) = text[search_from..].find(token) {
            let start = search_from + found_at;
            let end = start + token.len();
            let before_ok = start == 0 || text.as_bytes()[start - 1].is_ascii_whitespace();
            let after_ok = end == text.len() || text.as_bytes()[end].is_ascii_whitespace();
            if before_ok && after_ok {
                return true;
            }
            search_from = start + 1;
        }
        false
    }

    /// Proves the whole-token fix (a prior substring match let `suppressed=1` match a line
    /// holding `suppressed=10`, and this test was red against that old body).
    #[test]
    fn has_field_does_not_match_a_longer_numeric_value() {
        let line = "defect=missing_email suppressed=10 issuer=\"https://idp.example.com\"";
        assert!(!has_field(line, "suppressed", "1"), "suppressed=10 must not match a search for suppressed=1: {line}");
        assert!(has_field(line, "suppressed", "10"), "suppressed=10 must match a search for suppressed=10: {line}");
    }

    /// Asserts that the WHOLE capture holds none of `secrets` (spec S2, 6.1).
    fn assert_no_secrets(text: &str, secrets: &[&str]) {
        for secret in secrets {
            assert!(!text.contains(secret), "the log must not contain {secret:?}:\n{text}");
        }
    }

    /// A user that already owns `email`, with no external identity: an email conflict for anyone else.
    fn seed_user(store: &AuthnStore, n: u128, email: &str) {
        let id = principal_id(n);
        let principal = Principal::new(id.clone(), PrincipalKind::User, PrincipalStatus::Active, epoch(), epoch());
        let user = User::new(id.clone(), Email::parse(email).unwrap(), "Existing".into(), None, None, epoch(), epoch());
        store.principals.lock().unwrap().insert(id.uuid(), (principal, user));
    }

    /// A use case over `store`, with JIT on for `ISSUER`.
    fn jit_use_case<A: Authenticator>(authenticator: A, store: &AuthnStore) -> AuthenticateToken<A, InMemoryIdentities, InMemoryPrincipals, InMemoryMemberships, SeqIds, FixedClock> {
        AuthenticateToken::new(
            authenticator,
            InMemoryIdentities(store.clone()),
            InMemoryPrincipals(store.clone()),
            InMemoryMemberships::default(),
            Arc::new(InMemoryRoleGrants::default()),
            SeqIds::default(),
            FixedClock::default(),
            JitPolicy::from_issuers(&[(Issuer::parse(ISSUER).unwrap(), true)]),
        )
    }

    /// U1: no `email` claim. One `warn` line with the defect, `absent` and the issuer; one count.
    #[tokio::test]
    async fn jit_missing_email_writes_one_warn_line_and_counts_once() {
        let (logs, _logs_guard) = capture_logs();
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _metrics_guard = metrics::set_default_local_recorder(&recorder);
        let store = AuthnStore::default();
        let uc = jit_use_case(FakeAuthenticator::ok(claims(ISSUER, "sub-u1-distinct", None, Some("Nora Distinctname"))), &store);

        let err = uc.resolve("bearer-u1-secret", Provisioning::Enabled).await.unwrap_err();

        assert!(matches!(err, AuthnError::ProvisioningFailed(ProvisioningDefect::MissingEmail)), "got {err:?}");
        let text = logs.text();
        let lines = jit_lines(&text);
        assert_eq!(lines.len(), 1, "exactly one helper line expected:\n{text}");
        let line = lines[0];
        assert!(line.contains("WARN"), "the helper logs at warn: {line}");
        assert!(line.contains(MISSING_EMAIL_TEXT), "the fixed missing_email message: {line}");
        assert!(has_field(line, "defect", "missing_email"), "names the defect: {line}");
        assert!(has_field(line, "email_claim", "absent"), "names the claim state: {line}");
        assert!(has_field(line, "issuer", ISSUER), "names the issuer: {line}");
        assert!(has_field(line, "suppressed", "0"), "the first line suppressed nothing: {line}");
        assert_no_secrets(&text, &["sub-u1-distinct", "Nora Distinctname", "bearer-u1-secret"]);
        let snapshot = snapshotter.snapshot().into_vec();
        assert_eq!(jit_failures(&snapshot, "missing_email"), Some(1));
        assert_eq!(jit_series(&snapshot), 1, "no email_conflict series");
    }

    /// U2: an email-like claim that fails `Email::parse` (a second `@`). `invalid`, and the
    /// capture holds no part of the claim.
    #[tokio::test]
    async fn jit_invalid_email_claim_writes_one_warn_line_without_the_claim() {
        let (logs, _logs_guard) = capture_logs();
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _metrics_guard = metrics::set_default_local_recorder(&recorder);
        let store = AuthnStore::default();
        let uc = jit_use_case(
            FakeAuthenticator::ok(claims(ISSUER, "sub-u2-distinct", Some("alice.distinct@example.com@x"), Some("Otto Distinctname"))),
            &store,
        );

        let err = uc.resolve("bearer-u2-secret", Provisioning::Enabled).await.unwrap_err();

        assert!(matches!(err, AuthnError::ProvisioningFailed(ProvisioningDefect::MissingEmail)), "got {err:?}");
        let text = logs.text();
        let lines = jit_lines(&text);
        assert_eq!(lines.len(), 1, "exactly one helper line expected:\n{text}");
        let line = lines[0];
        assert!(line.contains("WARN"), "the helper logs at warn: {line}");
        assert!(line.contains(MISSING_EMAIL_TEXT), "the fixed missing_email message: {line}");
        assert!(has_field(line, "defect", "missing_email"), "names the defect: {line}");
        assert!(has_field(line, "email_claim", "invalid"), "names the claim state: {line}");
        assert!(has_field(line, "issuer", ISSUER), "names the issuer: {line}");
        assert_no_secrets(&text, &["alice.distinct", "sub-u2-distinct", "Otto Distinctname", "bearer-u2-secret"]);
        let snapshot = snapshotter.snapshot().into_vec();
        assert_eq!(jit_failures(&snapshot, "missing_email"), Some(1));
    }

    /// U3: another principal already has the email, and the identity is absent.
    #[tokio::test]
    async fn jit_email_conflict_writes_one_warn_line_without_the_email() {
        let (logs, _logs_guard) = capture_logs();
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _metrics_guard = metrics::set_default_local_recorder(&recorder);
        let store = AuthnStore::default();
        seed_user(&store, 7, "taken.distinct@example.com");
        let uc = jit_use_case(
            FakeAuthenticator::ok(claims(ISSUER, "sub-u3-distinct", Some("taken.distinct@example.com"), Some("Paula Distinctname"))),
            &store,
        );

        let err = uc.resolve("bearer-u3-secret", Provisioning::Enabled).await.unwrap_err();

        assert!(matches!(err, AuthnError::ProvisioningFailed(ProvisioningDefect::EmailConflict)), "got {err:?}");
        let text = logs.text();
        let lines = jit_lines(&text);
        assert_eq!(lines.len(), 1, "exactly one helper line expected:\n{text}");
        let line = lines[0];
        assert!(line.contains("WARN"), "the helper logs at warn: {line}");
        assert!(line.contains(EMAIL_CONFLICT_TEXT), "the fixed email_conflict message: {line}");
        assert!(has_field(line, "defect", "email_conflict"), "names the defect: {line}");
        assert!(has_field(line, "issuer", ISSUER), "names the issuer: {line}");
        assert!(!line.contains("email_claim"), "email_claim belongs to missing_email only: {line}");
        assert_no_secrets(&text, &["taken.distinct", "sub-u3-distinct", "Paula Distinctname", "bearer-u3-secret"]);
        let snapshot = snapshotter.snapshot().into_vec();
        assert_eq!(jit_failures(&snapshot, "email_conflict"), Some(1));
        assert_eq!(jit_series(&snapshot), 1, "no missing_email series");
    }

    /// U4: a successful JIT provision writes no helper line and makes no series.
    #[tokio::test]
    async fn jit_success_writes_no_line_and_no_series() {
        let (logs, _logs_guard) = capture_logs();
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _metrics_guard = metrics::set_default_local_recorder(&recorder);
        let store = AuthnStore::default();
        let uc = jit_use_case(FakeAuthenticator::ok(claims(ISSUER, "sub-u4", Some("u4@example.com"), Some("U Four"))), &store);

        uc.resolve("token", Provisioning::Enabled).await.unwrap();

        let text = logs.text();
        assert!(jit_lines(&text).is_empty(), "a success writes no helper line:\n{text}");
        assert_eq!(jit_series(&snapshotter.snapshot().into_vec()), 0);
    }

    /// U7: `provision` fails with a repository error that is not a conflict. `Backend`, no helper
    /// line, no series (the adapters already log `Backend`).
    #[tokio::test]
    async fn jit_backend_error_writes_no_line_and_no_series() {
        let (logs, _logs_guard) = capture_logs();
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _metrics_guard = metrics::set_default_local_recorder(&recorder);
        let uc = AuthenticateToken::new(
            FakeAuthenticator::ok(claims(ISSUER, "sub-u7", Some("u7@example.com"), None)),
            FailingProvisionIdentities,
            PanicIfCalledPrincipals,
            InMemoryMemberships::default(),
            Arc::new(InMemoryRoleGrants::default()),
            SeqIds::default(),
            FixedClock::default(),
            JitPolicy::from_issuers(&[(Issuer::parse(ISSUER).unwrap(), true)]),
        );

        let err = uc.resolve("token", Provisioning::Enabled).await.unwrap_err();

        assert!(matches!(err, AuthnError::Backend(_)), "got {err:?}");
        let text = logs.text();
        assert!(jit_lines(&text).is_empty(), "a backend error writes no helper line:\n{text}");
        assert_eq!(jit_series(&snapshotter.snapshot().into_vec()), 0);
    }

    /// U9 (spec S1, D2): three `missing_email` failures for one issuer in one window. One line,
    /// three counts: the rate limit does not apply to the counter.
    #[tokio::test]
    async fn jit_repeated_failures_log_once_and_count_each() {
        let (logs, _logs_guard) = capture_logs();
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _metrics_guard = metrics::set_default_local_recorder(&recorder);
        let store = AuthnStore::default();
        let uc = jit_use_case(
            QueueAuthenticator::new(vec![claims(ISSUER, "sub-u9", None, None), claims(ISSUER, "sub-u9", None, None), claims(ISSUER, "sub-u9", None, None)]),
            &store,
        );

        for _ in 0..3 {
            let err = uc.resolve("token", Provisioning::Enabled).await.unwrap_err();
            assert!(matches!(err, AuthnError::ProvisioningFailed(ProvisioningDefect::MissingEmail)), "got {err:?}");
        }

        let text = logs.text();
        assert_eq!(jit_lines(&text).len(), 1, "one line per (issuer, defect) in the window:\n{text}");
        assert_no_secrets(&text, &["sub-u9"]);
        assert_eq!(jit_failures(&snapshotter.snapshot().into_vec(), "missing_email"), Some(3));
    }

    /// U11 (Review Focus R1): the limiter key holds the issuer. Two issuers, two lines.
    #[tokio::test]
    async fn jit_failures_from_two_issuers_log_one_line_each() {
        let (logs, _logs_guard) = capture_logs();
        let store = AuthnStore::default();
        let uc = AuthenticateToken::new(
            QueueAuthenticator::new(vec![claims(ISSUER, "sub-u11-a", None, None), claims(OTHER_ISSUER, "sub-u11-b", None, None)]),
            InMemoryIdentities(store.clone()),
            InMemoryPrincipals(store.clone()),
            InMemoryMemberships::default(),
            Arc::new(InMemoryRoleGrants::default()),
            SeqIds::default(),
            FixedClock::default(),
            JitPolicy::from_issuers(&[(Issuer::parse(ISSUER).unwrap(), true), (Issuer::parse(OTHER_ISSUER).unwrap(), true)]),
        );

        uc.resolve("token", Provisioning::Enabled).await.unwrap_err();
        uc.resolve("token", Provisioning::Enabled).await.unwrap_err();

        let text = logs.text();
        let lines = jit_lines(&text);
        assert_eq!(lines.len(), 2, "one line for each issuer:\n{text}");
        assert!(lines.iter().any(|line| has_field(line, "issuer", ISSUER)), "{text}");
        assert!(lines.iter().any(|line| has_field(line, "issuer", OTHER_ISSUER)), "{text}");
        assert_no_secrets(&text, &["sub-u11-a", "sub-u11-b"]);
    }

    /// U12 (Review Focus R2): the limiter key holds the defect. Two defects for one issuer, two lines.
    #[tokio::test]
    async fn jit_two_defects_for_one_issuer_log_one_line_each() {
        let (logs, _logs_guard) = capture_logs();
        let store = AuthnStore::default();
        seed_user(&store, 8, "taken.u12@example.com");
        let uc = jit_use_case(
            QueueAuthenticator::new(vec![claims(ISSUER, "sub-u12-a", None, None), claims(ISSUER, "sub-u12-b", Some("taken.u12@example.com"), None)]),
            &store,
        );

        let err = uc.resolve("token", Provisioning::Enabled).await.unwrap_err();
        assert!(matches!(err, AuthnError::ProvisioningFailed(ProvisioningDefect::MissingEmail)), "got {err:?}");
        let err = uc.resolve("token", Provisioning::Enabled).await.unwrap_err();
        assert!(matches!(err, AuthnError::ProvisioningFailed(ProvisioningDefect::EmailConflict)), "got {err:?}");

        let text = logs.text();
        let lines = jit_lines(&text);
        assert_eq!(lines.len(), 2, "one line for each defect:\n{text}");
        assert!(lines.iter().any(|line| has_field(line, "defect", "missing_email")), "{text}");
        assert!(lines.iter().any(|line| has_field(line, "defect", "email_conflict")), "{text}");
        assert_no_secrets(&text, &["sub-u12-a", "sub-u12-b", "taken.u12"]);
    }

    /// U13 (Review Focus R3, spec 4.5): `AppState` clones the use case for each request. The clone
    /// shares the limiter through its `Arc`, so a failure on the clone inside the window is suppressed.
    #[tokio::test]
    async fn a_cloned_use_case_shares_the_rate_limiter() {
        let (logs, _logs_guard) = capture_logs();
        let store = AuthnStore::default();
        let uc = AuthenticateToken::new(
            QueueAuthenticator::new(vec![claims(ISSUER, "sub-u13-a", None, None), claims(ISSUER, "sub-u13-b", None, None)]),
            InMemoryIdentities(store.clone()),
            InMemoryPrincipals(store.clone()),
            InMemoryMemberships::default(),
            Arc::new(InMemoryRoleGrants::default()),
            KernelIdGenerator,
            FixedClock::default(),
            JitPolicy::from_issuers(&[(Issuer::parse(ISSUER).unwrap(), true)]),
        );
        let twin = uc.clone();

        uc.resolve("token", Provisioning::Enabled).await.unwrap_err();
        twin.resolve("token", Provisioning::Enabled).await.unwrap_err();

        let text = logs.text();
        assert_eq!(jit_lines(&text).len(), 1, "the clone must share the limiter:\n{text}");
        assert_no_secrets(&text, &["sub-u13-a", "sub-u13-b"]);
    }

    /// U14 (Review Focus R4): an `email` claim that is present but empty is `invalid`, not `absent`.
    #[tokio::test]
    async fn jit_empty_email_claim_is_invalid_not_absent() {
        let (logs, _logs_guard) = capture_logs();
        let store = AuthnStore::default();
        let uc = jit_use_case(FakeAuthenticator::ok(claims(ISSUER, "sub-u14", Some(""), None)), &store);

        let err = uc.resolve("token", Provisioning::Enabled).await.unwrap_err();

        assert!(matches!(err, AuthnError::ProvisioningFailed(ProvisioningDefect::MissingEmail)), "got {err:?}");
        let text = logs.text();
        let lines = jit_lines(&text);
        assert_eq!(lines.len(), 1, "exactly one helper line expected:\n{text}");
        assert!(has_field(lines[0], "email_claim", "invalid"), "a present, empty claim is invalid: {}", lines[0]);
        assert_no_secrets(&text, &["sub-u14"]);
    }

    /// U15 (Review Focus R5): the next admitted line carries the limiter's `suppressed` count.
    /// The test module can reach the private `provisioning_log`, so it injects two earlier
    /// failures in an old window instead of waiting 10 s.
    #[tokio::test]
    async fn the_next_admitted_line_carries_the_suppressed_count() {
        let (logs, _logs_guard) = capture_logs();
        let store = AuthnStore::default();
        let uc = jit_use_case(FakeAuthenticator::ok(claims(ISSUER, "sub-u15", None, None)), &store);
        let old = Instant::now().checked_sub(std::time::Duration::from_secs(30)).expect("the monotonic clock is older than 30 s");
        assert_eq!(uc.provisioning_log.admit_at(ISSUER, "missing_email", old), Some(0));
        assert_eq!(uc.provisioning_log.admit_at(ISSUER, "missing_email", old + std::time::Duration::from_secs(1)), None);

        uc.resolve("token", Provisioning::Enabled).await.unwrap_err();

        let text = logs.text();
        let lines = jit_lines(&text);
        assert_eq!(lines.len(), 1, "the window is over, so the line is admitted:\n{text}");
        assert!(has_field(lines[0], "suppressed", "1"), "the line reports the one suppressed failure: {}", lines[0]);
        assert_no_secrets(&text, &["sub-u15"]);
    }

    /// U6 (spec 4.6, D1): `Conflict(EmailTaken)` with the identity present at the re-read is a
    /// lost race, not a conflict. The loser resolves to the winner, with no line and no count.
    #[tokio::test]
    async fn jit_email_taken_with_the_identity_present_resolves_to_the_winner() {
        let (logs, _logs_guard) = capture_logs();
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _metrics_guard = metrics::set_default_local_recorder(&recorder);
        let store = AuthnStore::default();
        let issuer = Issuer::parse(ISSUER).unwrap();
        let winner_id = principal_id(43);
        seed_user(&store, 43, "racer.u6@example.com");
        let winner = ExternalIdentity {
            id: Uuid::from_u128(4343),
            principal_id: winner_id.clone(),
            issuer: issuer.clone(),
            subject: "sub-u6-race".into(),
            created_at: epoch(),
            updated_at: epoch(),
        };
        let uc = AuthenticateToken::new(
            FakeAuthenticator::ok(claims(ISSUER, "sub-u6-race", Some("racer.u6@example.com"), None)),
            EmailTakenRaceIdentities {
                inner: InMemoryIdentities(store.clone()),
                winner,
            },
            InMemoryPrincipals(store.clone()),
            InMemoryMemberships::default(),
            Arc::new(InMemoryRoleGrants::default()),
            SeqIds::default(),
            FixedClock::default(),
            JitPolicy::from_issuers(&[(issuer, true)]),
        );

        let resolved = uc.resolve("token", Provisioning::Enabled).await.unwrap();

        assert_eq!(resolved.principal_id, winner_id, "the loser resolves to the winner");
        let text = logs.text();
        assert!(jit_lines(&text).is_empty(), "a lost race writes no helper line:\n{text}");
        assert_eq!(jit_series(&snapshotter.snapshot().into_vec()), 0, "a lost race is not counted");
    }

    // ---- SMA-707: the JIT-disabled refusal line ----------------------------------------------

    /// The fixed prefix of the SMA-707 line. Tests select the line with it.
    const JIT_DISABLED_LINE: &str = "request refused: the identity is not provisioned";
    /// The whole fixed SMA-707 message (spec 4.3).
    const JIT_DISABLED_TEXT: &str = "request refused: the identity is not provisioned, and just-in-time provisioning is disabled for this issuer; IAM creates an identity only by just-in-time provisioning, so set jit_provisioning = true for the issuer to let new users of this issuer sign in";

    /// The SMA-707 lines only.
    fn jit_disabled_lines(text: &str) -> Vec<&str> {
        text.lines().filter(|line| line.contains(JIT_DISABLED_LINE)).collect()
    }

    /// A use case over `store`, with JIT OFF for `ISSUER`.
    fn jit_disabled_use_case<A: Authenticator>(authenticator: A, store: &AuthnStore) -> AuthenticateToken<A, InMemoryIdentities, InMemoryPrincipals, InMemoryMemberships, SeqIds, FixedClock> {
        AuthenticateToken::new(
            authenticator,
            InMemoryIdentities(store.clone()),
            InMemoryPrincipals(store.clone()),
            InMemoryMemberships::default(),
            Arc::new(InMemoryRoleGrants::default()),
            SeqIds::default(),
            FixedClock::default(),
            JitPolicy::from_issuers(&[(Issuer::parse(ISSUER).unwrap(), false)]),
        )
    }

    /// SMA-707 U1: `resolve(.., Enabled)`, a JIT-disabled issuer and an unknown identity. One
    /// `info` line with the issuer, `reason` and `suppressed = 0`. The whole capture holds no
    /// claim value and no token. No line matches the SMA-698 selector, and there is no JIT
    /// failure series (the checks of the old U8).
    #[tokio::test]
    async fn jit_disabled_unknown_identity_writes_one_info_line_without_claims() {
        let (logs, _logs_guard) = capture_logs();
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _metrics_guard = metrics::set_default_local_recorder(&recorder);
        let store = AuthnStore::default();
        let uc = jit_disabled_use_case(
            FakeAuthenticator::ok(claims_with_profile(
                ISSUER,
                "sub-jd-u1-distinct",
                Some("vera.jd-distinct@example.com"),
                Some("Vera Jdname"),
                Some("jd-XQ-locale"),
                Some("Jd/Distinct_Zone"),
            )),
            &store,
        );

        let err = uc.resolve("bearer-jd-u1-secret", Provisioning::Enabled).await.unwrap_err();

        assert!(matches!(err, AuthnError::IdentityNotProvisioned), "got {err:?}");
        let text = logs.text();
        let lines = jit_disabled_lines(&text);
        assert_eq!(lines.len(), 1, "exactly one JIT-disabled line expected:\n{text}");
        let line = lines[0];
        assert!(line.contains("INFO"), "the line is at info: {line}");
        assert!(line.contains(JIT_DISABLED_TEXT), "the fixed message: {line}");
        assert!(has_field(line, "issuer", ISSUER), "names the issuer: {line}");
        assert!(has_field(line, "reason", "jit_disabled"), "names the reason: {line}");
        assert!(has_field(line, "suppressed", "0"), "the first line suppressed nothing: {line}");
        assert_no_secrets(
            &text,
            &[
                "sub-jd-u1-distinct",
                "vera.jd-distinct@example.com",
                "vera.jd-distinct",
                "Vera Jdname",
                "jd-XQ-locale",
                "Jd/Distinct_Zone",
                "bearer-jd-u1-secret",
            ],
        );
        assert!(jit_lines(&text).is_empty(), "the line must not match the SMA-698 selector:\n{text}");
        assert!(store.identities.lock().unwrap().is_empty(), "JIT off never provisions");
        assert_eq!(jit_series(&snapshotter.snapshot().into_vec()), 0, "no JIT failure series");
    }

    /// SMA-707 U2: `introspect` for an unknown identity of the SAME JIT-disabled issuer writes no
    /// line. With a JIT-enabled issuer, a misplaced line would not appear either, so the issuer
    /// must be JIT-disabled. Control: the same token on `resolve(.., Enabled)` then writes one
    /// line, so the capture can see the line on every run.
    #[tokio::test]
    async fn jit_disabled_introspect_writes_no_line() {
        let (logs, _logs_guard) = capture_logs();
        let store = AuthnStore::default();
        let uc = jit_disabled_use_case(QueueAuthenticator::new(vec![claims(ISSUER, "sub-jd-u2", None, None), claims(ISSUER, "sub-jd-u2", None, None)]), &store);

        let err = uc.introspect("bearer-jd-u2").await.unwrap_err();

        assert!(matches!(err, AuthnError::IdentityNotProvisioned), "got {err:?}");
        let text = logs.text();
        assert!(jit_disabled_lines(&text).is_empty(), "introspect must not write the line:\n{text}");

        let err = uc.resolve("bearer-jd-u2", Provisioning::Enabled).await.unwrap_err();

        assert!(matches!(err, AuthnError::IdentityNotProvisioned), "got {err:?}");
        let text = logs.text();
        assert_eq!(jit_disabled_lines(&text).len(), 1, "control: the protected path writes one line:\n{text}");
    }

    /// SMA-707 U3: `resolve(.., Disabled)` with a JIT-disabled issuer writes no line. The same
    /// guard as U2, one level down, with the same control.
    #[tokio::test]
    async fn jit_disabled_resolve_disabled_writes_no_line() {
        let (logs, _logs_guard) = capture_logs();
        let store = AuthnStore::default();
        let uc = jit_disabled_use_case(QueueAuthenticator::new(vec![claims(ISSUER, "sub-jd-u3", None, None), claims(ISSUER, "sub-jd-u3", None, None)]), &store);

        let err = uc.resolve("bearer-jd-u3", Provisioning::Disabled).await.unwrap_err();

        assert!(matches!(err, AuthnError::IdentityNotProvisioned), "got {err:?}");
        let text = logs.text();
        assert!(jit_disabled_lines(&text).is_empty(), "resolve(.., Disabled) must not write the line:\n{text}");

        let err = uc.resolve("bearer-jd-u3", Provisioning::Enabled).await.unwrap_err();

        assert!(matches!(err, AuthnError::IdentityNotProvisioned), "got {err:?}");
        let text = logs.text();
        assert_eq!(jit_disabled_lines(&text).len(), 1, "control: the protected path writes one line:\n{text}");
    }

    /// SMA-707 U4: a KNOWN identity of a JIT-disabled issuer resolves and writes no line. Control:
    /// an UNKNOWN subject of the same issuer, resolved next in the same capture, then writes
    /// exactly one line — proving the capture would have caught a line had the known-identity
    /// path wrongly written one.
    #[tokio::test]
    async fn jit_disabled_known_identity_resolves_and_writes_no_line() {
        let (logs, _logs_guard) = capture_logs();
        let store = AuthnStore::default();
        let issuer = Issuer::parse(ISSUER).unwrap();
        let pid = seeded_principal(&store, &issuer, "sub-jd-u4");
        let uc = jit_disabled_use_case(
            QueueAuthenticator::new(vec![claims(ISSUER, "sub-jd-u4", None, None), claims(ISSUER, "sub-jd-u4-unknown", None, None)]),
            &store,
        );

        let resolved = uc.resolve("token", Provisioning::Enabled).await.unwrap();

        assert_eq!(resolved.principal_id, pid);
        let text = logs.text();
        assert!(jit_disabled_lines(&text).is_empty(), "a known identity writes no line:\n{text}");

        let err = uc.resolve("token2", Provisioning::Enabled).await.unwrap_err();

        assert!(matches!(err, AuthnError::IdentityNotProvisioned), "got {err:?}");
        let text = logs.text();
        assert_eq!(jit_disabled_lines(&text).len(), 1, "control: an unknown subject writes one line:\n{text}");
    }

    /// SMA-707 U5a (and Review Focus R2): two refusals for one issuer in one window write one line.
    /// The two subjects differ, so a limiter keyed by the subject would write two lines.
    #[tokio::test]
    async fn jit_disabled_refusals_for_one_issuer_log_once() {
        let (logs, _logs_guard) = capture_logs();
        let store = AuthnStore::default();
        let uc = jit_disabled_use_case(
            QueueAuthenticator::new(vec![claims(ISSUER, "sub-jd-u5a-first", None, None), claims(ISSUER, "sub-jd-u5a-second", None, None)]),
            &store,
        );

        for _ in 0..2 {
            let err = uc.resolve("token", Provisioning::Enabled).await.unwrap_err();
            assert!(matches!(err, AuthnError::IdentityNotProvisioned), "got {err:?}");
        }

        let text = logs.text();
        let lines = jit_disabled_lines(&text);
        assert_eq!(lines.len(), 1, "one line per issuer in the window:\n{text}");
        assert!(has_field(lines[0], "suppressed", "0"), "the first line suppressed nothing: {}", lines[0]);
        assert_no_secrets(&text, &["sub-jd-u5a-first", "sub-jd-u5a-second"]);
    }

    /// SMA-707 U5b: the next admitted line carries the limiter's `suppressed` count. The test
    /// module can reach the private `not_provisioned_log`, so it injects two earlier refusals in
    /// an old window instead of waiting 10 s.
    #[tokio::test]
    async fn jit_disabled_next_admitted_line_carries_the_suppressed_count() {
        let (logs, _logs_guard) = capture_logs();
        let store = AuthnStore::default();
        let uc = jit_disabled_use_case(FakeAuthenticator::ok(claims(ISSUER, "sub-jd-u5b", None, None)), &store);
        let old = Instant::now().checked_sub(std::time::Duration::from_secs(30)).expect("the monotonic clock is older than 30 s");
        assert_eq!(uc.not_provisioned_log.admit_at(ISSUER, (), old), Some(0));
        assert_eq!(uc.not_provisioned_log.admit_at(ISSUER, (), old + std::time::Duration::from_secs(1)), None);

        uc.resolve("token", Provisioning::Enabled).await.unwrap_err();

        let text = logs.text();
        let lines = jit_disabled_lines(&text);
        assert_eq!(lines.len(), 1, "the window is over, so the line is admitted:\n{text}");
        assert!(has_field(lines[0], "suppressed", "1"), "the line reports the one suppressed refusal: {}", lines[0]);
        assert_no_secrets(&text, &["sub-jd-u5b"]);
    }

    /// SMA-707 U6: the limiter key holds the issuer. A refusal for a second JIT-disabled issuer,
    /// just after one for the first issuer, writes its own line, and each line names its issuer.
    #[tokio::test]
    async fn jit_disabled_refusals_from_two_issuers_log_one_line_each() {
        let (logs, _logs_guard) = capture_logs();
        let store = AuthnStore::default();
        let uc = AuthenticateToken::new(
            QueueAuthenticator::new(vec![claims(ISSUER, "sub-jd-u6-a", None, None), claims(OTHER_ISSUER, "sub-jd-u6-b", None, None)]),
            InMemoryIdentities(store.clone()),
            InMemoryPrincipals(store.clone()),
            InMemoryMemberships::default(),
            Arc::new(InMemoryRoleGrants::default()),
            SeqIds::default(),
            FixedClock::default(),
            JitPolicy::from_issuers(&[(Issuer::parse(ISSUER).unwrap(), false), (Issuer::parse(OTHER_ISSUER).unwrap(), false)]),
        );

        uc.resolve("token", Provisioning::Enabled).await.unwrap_err();
        uc.resolve("token", Provisioning::Enabled).await.unwrap_err();

        let text = logs.text();
        let lines = jit_disabled_lines(&text);
        assert_eq!(lines.len(), 2, "one line for each issuer:\n{text}");
        assert!(has_field(lines[0], "issuer", ISSUER), "the first line names the first issuer: {}", lines[0]);
        assert!(has_field(lines[1], "issuer", OTHER_ISSUER), "the second line names the second issuer: {}", lines[1]);
        assert_no_secrets(&text, &["sub-jd-u6-a", "sub-jd-u6-b"]);
    }

    /// SMA-707 U7 (spec 4.4): `AppState` clones the use case for each request. The clone shares
    /// the limiter through its `Arc`, so a refusal on the clone inside the window is suppressed.
    /// `KernelIdGenerator` is the only `Clone` id generator in the crate.
    #[tokio::test]
    async fn jit_disabled_cloned_use_case_shares_the_rate_limiter() {
        let (logs, _logs_guard) = capture_logs();
        let store = AuthnStore::default();
        let uc = AuthenticateToken::new(
            QueueAuthenticator::new(vec![claims(ISSUER, "sub-jd-u7-a", None, None), claims(ISSUER, "sub-jd-u7-b", None, None)]),
            InMemoryIdentities(store.clone()),
            InMemoryPrincipals(store.clone()),
            InMemoryMemberships::default(),
            Arc::new(InMemoryRoleGrants::default()),
            KernelIdGenerator,
            FixedClock::default(),
            JitPolicy::from_issuers(&[(Issuer::parse(ISSUER).unwrap(), false)]),
        );
        let twin = uc.clone();

        uc.resolve("token", Provisioning::Enabled).await.unwrap_err();
        twin.resolve("token", Provisioning::Enabled).await.unwrap_err();

        let text = logs.text();
        assert_eq!(jit_disabled_lines(&text).len(), 1, "the clone must share the limiter:\n{text}");
        assert_no_secrets(&text, &["sub-jd-u7-a", "sub-jd-u7-b"]);
    }

    /// SMA-707 U8 (Review Focus R1): a JIT-ENABLED issuer never writes the line, for a JIT
    /// success and for a JIT failure. Control: the JIT failure writes the SMA-698 line, so the
    /// capture works.
    #[tokio::test]
    async fn jit_disabled_line_is_absent_for_a_jit_enabled_issuer() {
        let (logs, _logs_guard) = capture_logs();
        let store = AuthnStore::default();
        let uc = jit_use_case(
            QueueAuthenticator::new(vec![claims(ISSUER, "sub-jd-u8-ok", Some("jd-u8@example.com"), None), claims(ISSUER, "sub-jd-u8-no-email", None, None)]),
            &store,
        );

        uc.resolve("token", Provisioning::Enabled).await.unwrap();
        let err = uc.resolve("token", Provisioning::Enabled).await.unwrap_err();

        assert!(matches!(err, AuthnError::ProvisioningFailed(ProvisioningDefect::MissingEmail)), "got {err:?}");
        let text = logs.text();
        assert!(jit_disabled_lines(&text).is_empty(), "a JIT-enabled issuer never writes the line:\n{text}");
        assert_eq!(jit_lines(&text).len(), 1, "control: the SMA-698 line appears:\n{text}");
    }

    // ---- SMA-700: resolve_dpop (D18) -------------------------------------------------------

    /// Checks 1-9 scripted: `Ok` gives a proof for `POST https://gw.example.test/v1/chat/completions`
    /// at the epoch (`FixedClock::default()`), so checks 10-13 pass.
    struct ScriptedChecker(Result<ProofClaims, ProofDefect>);

    impl DpopProofChecker for ScriptedChecker {
        fn check(&self, _proof: &str, _token: &str, _jkt: &Jkt) -> Result<ProofClaims, ProofDefect> {
            self.0.clone()
        }
        fn follow_up_claims(&self, _proof: &str) -> Result<FollowUpClaims, ProofDefect> {
            Err(ProofDefect::Malformed)
        }
        fn ath_matches(&self, _ath: &str, _token: &str) -> bool {
            false
        }
    }

    fn good_proof() -> Result<ProofClaims, ProofDefect> {
        Ok(ProofClaims {
            jti: "jti-1".into(),
            iat: 0,
            htm: "POST".into(),
            htu: "https://gw.example.test/v1/chat/completions".into(),
        })
    }

    fn verifier(check: Result<ProofClaims, ProofDefect>) -> Arc<DpopProofVerifier> {
        let bases = vec!["https://gw.example.test".to_string()];
        Arc::new(
            DpopProofVerifier::new(
                Arc::new(ScriptedChecker(check)),
                Arc::new(InMemoryReplayStore::new(10, 10, 10)),
                Arc::new(FixedClock::default()),
                &bases,
                60,
            )
            .expect("verifier"),
        )
    }

    fn dpop_request() -> DpopRequest {
        DpopRequest {
            proof: "p.r.oof".into(),
            method: "POST".into(),
            path: "/v1/chat/completions".into(),
        }
    }

    fn bound_claims(subject: &str) -> ValidatedClaims {
        ValidatedClaims {
            key_binding: Some(Jkt::new("jkt-1")),
            ..claims("https://idp.example.com", subject, Some("x@example.com"), None)
        }
    }

    /// Counts calls to the identity port, so a test proves the proof check came first (D18).
    struct CountingIdentities {
        calls: Arc<AtomicUsize>,
        inner: InMemoryIdentities,
    }

    #[async_trait]
    impl ExternalIdentityRepository for CountingIdentities {
        async fn find_by_issuer_subject(&self, issuer: &Issuer, subject: &str) -> Result<Option<ExternalIdentity>, RepositoryError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.inner.find_by_issuer_subject(issuer, subject).await
        }
        async fn provision(&self, principal: &Principal, user: &User, identity: &ExternalIdentity) -> Result<(), RepositoryError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.inner.provision(principal, user, identity).await
        }
    }

    fn dpop_use_case(
        claims: ValidatedClaims,
        calls: Arc<AtomicUsize>,
        store: AuthnStore,
        check: Result<ProofClaims, ProofDefect>,
    ) -> AuthenticateToken<FakeAuthenticator, CountingIdentities, InMemoryPrincipals, InMemoryMemberships, SeqIds, FixedClock> {
        let issuer = Issuer::parse("https://idp.example.com").unwrap();
        AuthenticateToken::new(
            FakeAuthenticator::ok(claims),
            CountingIdentities {
                calls,
                inner: InMemoryIdentities(store.clone()),
            },
            InMemoryPrincipals(store),
            InMemoryMemberships::default(),
            Arc::new(InMemoryRoleGrants::default()),
            SeqIds::default(),
            FixedClock::default(),
            JitPolicy::from_issuers(&[(issuer, true)]),
        )
        .with_dpop(verifier(check))
    }

    #[tokio::test]
    async fn no_identity_lookup_runs_before_the_proof_check() {
        // D18 / AC 4, both directions: a bad proof makes NO identity call; a good proof makes one.
        let calls = Arc::new(AtomicUsize::new(0));
        let uc = dpop_use_case(bound_claims("sub-d1"), calls.clone(), AuthnStore::default(), Err(ProofDefect::Signature));
        let err = uc.resolve_dpop("token", DpopInput::Introspect(dpop_request()), Provisioning::Disabled).await.unwrap_err();
        assert!(matches!(err, AuthnError::InvalidDpopProof(ProofDefect::Signature)), "got {err:?}");
        assert_eq!(calls.load(Ordering::SeqCst), 0, "the identity port must not run before the proof check");

        let calls = Arc::new(AtomicUsize::new(0));
        let uc = dpop_use_case(bound_claims("sub-d1"), calls.clone(), AuthnStore::default(), good_proof());
        let err = uc.resolve_dpop("token", DpopInput::Introspect(dpop_request()), Provisioning::Disabled).await.unwrap_err();
        assert!(matches!(err, AuthnError::IdentityNotProvisioned), "got {err:?}");
        assert_eq!(calls.load(Ordering::SeqCst), 1, "after a good proof, the lookup runs once");
    }

    #[tokio::test]
    async fn an_unprovisioned_identity_with_a_bad_proof_is_an_invalid_proof() {
        // § 5.1: InvalidDpopProof, not IdentityNotProvisioned — else a stolen token passes the
        // gateway's service-info, which accepts identity-not-provisioned (challenge 2).
        let uc = dpop_use_case(bound_claims("sub-unknown"), Arc::new(AtomicUsize::new(0)), AuthnStore::default(), Err(ProofDefect::Htu));
        let err = uc.introspect_dpop("token", dpop_request()).await.unwrap_err();
        assert!(matches!(err, AuthnError::InvalidDpopProof(ProofDefect::Htu)), "got {err:?}");
    }

    #[tokio::test]
    async fn a_bad_request_is_refused_before_the_token_is_verified() {
        // Decision P5: the request checks run before `authenticate`.
        let uc = AuthenticateToken::new(
            PanicIfCalledAuthenticator,
            PanicIfCalledIdentities,
            PanicIfCalledPrincipals,
            InMemoryMemberships::default(),
            Arc::new(InMemoryRoleGrants::default()),
            SeqIds::default(),
            FixedClock::default(),
            JitPolicy::from_issuers(&[]),
        )
        .with_dpop(verifier(good_proof()));
        let request = DpopRequest { path: "x".into(), ..dpop_request() };
        let err = uc.resolve_dpop("token", DpopInput::Introspect(request), Provisioning::Disabled).await.unwrap_err();
        assert!(matches!(err, AuthnError::InvalidDpopProof(ProofDefect::Malformed)), "got {err:?}");
    }

    #[tokio::test]
    async fn with_dpop_off_a_dpop_context_is_a_malformed_token() {
        // D11: the answer is the one a malformed token gets today.
        let uc = AuthenticateToken::new(
            PanicIfCalledAuthenticator,
            PanicIfCalledIdentities,
            PanicIfCalledPrincipals,
            InMemoryMemberships::default(),
            Arc::new(InMemoryRoleGrants::default()),
            SeqIds::default(),
            FixedClock::default(),
            JitPolicy::from_issuers(&[]),
        );
        assert!(!uc.dpop_enabled());
        let err = uc.resolve_dpop("token", DpopInput::Introspect(dpop_request()), Provisioning::Disabled).await.unwrap_err();
        assert!(matches!(err, AuthnError::InvalidToken(TokenDefect::Malformed)), "got {err:?}");
    }

    #[tokio::test]
    async fn a_provisioned_identity_with_a_good_proof_resolves() {
        let store = AuthnStore::default();
        let issuer = Issuer::parse("https://idp.example.com").unwrap();
        let pid = seeded_principal(&store, &issuer, "sub-ok");
        let uc = dpop_use_case(bound_claims("sub-ok"), Arc::new(AtomicUsize::new(0)), store, good_proof());
        let ctx = uc.introspect_dpop("token", dpop_request()).await.expect("resolves");
        assert_eq!(ctx.principal.principal_id, pid);
    }
}
