// SPDX-License-Identifier: Apache-2.0

//! `UserIdentityService` (SMA-712): the operator calls that find a user by email, link and
//! unlink an external identity, and change a user's email.
//!
//! **This service authorizes, not the adapter.** The first statement of each method is
//! `self.authorize.check(actor, Action::X, &root_prn())`. There is no `enforce_tenancy` gate:
//! with that toggle off, an adapter-level check would not run, and any authenticated principal
//! could link its own identity to a `platform_admin` user (spec 5.1). The check runs before any
//! validation of a body value and before any row read. So a denied caller learns nothing about
//! the rows: not whether a user, an identity or an email exists.
//!
//! One exception is syntax only (code review F4). The transports parse the id that names the
//! user or the identity BEFORE they call this service: the HTTP path extractor parses the
//! `{id}` uuid, and the gRPC adapter parses `user_prn` and `external_identity_id` (plan
//! deviation 5). So a denied caller with a malformed id gets `invalid-uuid` or `invalid-prn`,
//! not `forbidden`. This tells the caller only that the id is malformed. It never tells whether
//! a row exists.
//!
//! Root-only-ness comes from the resource (`root_prn()`), not from the Cedar schema, exactly
//! like `DeadLetterService`.
//!
//! **The stored PRN (SMA-649, code review F1).** The store compares the caller's user PRN with
//! the stored `principal.prn`: absent is `not-found`, different is `prn-mismatch`. The audit
//! `resource_prn` is the STORED PRN that the store returns, never the caller's value.
//!
//! Each write runs in ONE unit of work: the change, then its audit entry only when the change
//! did something, then the commit. A failed audit write rolls the change back. There is no
//! outbox event (spec 6.5).
//!
//! The audit `detail` holds an email and a subject. These are personal data. `ListAuditLog`
//! restricts the audit log. No log line of this service contains either value.

use std::sync::Arc;

use paigasus_iam_core::authz::model::root_prn;
use paigasus_iam_core::{
    Action, AuditEntry, AuditLog, AuditOutcome, AuditReason, Clock, ConflictKind, Credential, Email, ExternalIdentity, ExternalSubject, IdGenerator, IdentityLinkStore, Issuer, Mutated, PrincipalId,
    RepositoryError, UnitOfWork, UserWithIdentities,
};
use paigasus_kernel::Prn;
use uuid::Uuid;

use crate::application::authorize::Authorize;
use crate::application::error::TenancyError;

/// Constructor bag, mirroring `DeadLetterDeps`. `issuers` is the configured
/// `authn.issuers[].issuer` set, parsed once at boot.
pub struct UserIdentityDeps {
    pub authorize: Authorize,
    pub links: Arc<dyn IdentityLinkStore>,
    pub uow: Arc<dyn UnitOfWork>,
    pub audit: Arc<dyn AuditLog>,
    pub issuers: Vec<Issuer>,
    pub ids: Arc<dyn IdGenerator>,
    pub clock: Arc<dyn Clock>,
}

#[derive(Clone)]
pub struct UserIdentityService {
    authorize: Authorize,
    links: Arc<dyn IdentityLinkStore>,
    uow: Arc<dyn UnitOfWork>,
    audit: Arc<dyn AuditLog>,
    issuers: Arc<[Issuer]>,
    ids: Arc<dyn IdGenerator>,
    clock: Arc<dyn Clock>,
}

/// Parses an email and drops the raw input from the error. `DomainError::InvalidEmail` keeps
/// the raw value, and this service keeps no caller value in an error.
fn parse_email(raw: &str) -> Result<Email, TenancyError> {
    Email::parse(raw).map_err(|_| TenancyError::InvalidEmail(String::new()))
}

impl UserIdentityService {
    #[must_use]
    pub fn new(deps: UserIdentityDeps) -> Self {
        UserIdentityService {
            authorize: deps.authorize,
            links: deps.links,
            uow: deps.uow,
            audit: deps.audit,
            issuers: deps.issuers.into(),
            ids: deps.ids,
            clock: deps.clock,
        }
    }

    /// The committed-outcome audit entry of one write. The resource is the user, not Root.
    fn audit_entry(&self, actor: &Prn, action: Action, user: &PrincipalId, detail: serde_json::Value) -> AuditEntry {
        AuditEntry {
            id: self.ids.new_audit_id(),
            occurred_at: self.clock.now(),
            actor_prn: Some(actor.canonical()),
            action: action.as_wire().to_string(),
            resource_prn: Some(user.canonical()),
            outcome: AuditOutcome::Committed,
            determining_policies: Vec::new(),
            detail,
            correlation_id: Some(self.ids.new_correlation_id()),
        }
    }

    /// Spec 5.2: the issuer must equal a configured issuer exactly, after trim. The stored value
    /// is the configured value.
    fn configured_issuer(&self, raw: &str) -> Result<Issuer, TenancyError> {
        let wanted = raw.trim();
        self.issuers.iter().find(|issuer| issuer.as_str() == wanted).cloned().ok_or(TenancyError::UnknownIssuer)
    }

    /// `FindUserByEmail`, `POST /v1/users/find-by-email`. A read: no audit record (spec 13).
    pub async fn find_by_email(&self, actor: &Prn, email: &str) -> Result<UserWithIdentities, TenancyError> {
        self.authorize.check(actor, Action::GetUser, &root_prn()).await?;
        let email = parse_email(email)?;
        self.links.find_user_by_email(&email).await?.ok_or(TenancyError::NotFound)
    }

    /// `LinkExternalIdentity`. `changed == false` means the same user already held the pair:
    /// the call returns that identity and writes nothing (a safe retry, spec 5.2).
    pub async fn link(&self, actor: &Prn, user: &PrincipalId, issuer: &str, subject: &str, reason: &str) -> Result<Mutated<ExternalIdentity>, TenancyError> {
        self.authorize.check(actor, Action::LinkExternalIdentity, &root_prn()).await?;
        let issuer = self.configured_issuer(issuer)?;
        let subject = ExternalSubject::parse(subject)?;
        let reason = AuditReason::parse(reason)?;

        let tx = self.uow.begin().await?;
        let Some(stored) = self.links.lock_user_in(&*tx, user).await? else {
            return Err(TenancyError::NotFound);
        };
        if let Some(existing) = self.links.find_identity_in(&*tx, &issuer, subject.as_str()).await? {
            // Dropping `tx` releases the share lock. Nothing was written.
            return Self::held_by(existing, &stored);
        }
        let now = self.clock.now();
        let identity = ExternalIdentity {
            id: self.ids.new_external_identity_id(),
            principal_id: stored.clone(),
            issuer,
            subject: subject.into_string(),
            created_at: now,
            updated_at: now,
        };
        // A concurrent writer (a JIT login, or a second link of the same pair) can insert the
        // pair after the read above. The unique constraint then raises
        // `Conflict(ExternalIdentityExists)`.
        match self.links.link_in(&*tx, &identity).await {
            Ok(()) => {}
            Err(RepositoryError::Conflict(ConflictKind::ExternalIdentityExists)) => {
                // Code review F2. The unique violation aborted the Postgres transaction, so no
                // read can run in it. Drop it (a rollback), then read the pair again in a new
                // transaction. The same user: the safe retry. Another user, or no row: 409.
                drop(tx);
                let read = self.uow.begin().await?;
                let winner = self.links.find_identity_in(&*read, &identity.issuer, &identity.subject).await?;
                drop(read);
                return match winner {
                    Some(existing) => Self::held_by(existing, &stored),
                    None => Err(TenancyError::ExternalIdentityConflict),
                };
            }
            Err(e) => return Err(e.into()),
        }
        let detail = serde_json::json!({
            "reason": reason.as_str(),
            "identity_id": identity.id.to_string(),
            "issuer": identity.issuer.as_str(),
            "subject": identity.subject,
        });
        let entry = self.audit_entry(actor, Action::LinkExternalIdentity, &stored, detail);
        self.audit.record(&*tx, &entry).await?;
        tx.commit().await?;
        Ok(Mutated { value: identity, changed: true })
    }

    /// The answer for a pair that a stored identity already holds: the safe retry when `user`
    /// holds it (`changed: false`, no audit), else 409. Both PRNs come from storage.
    fn held_by(existing: ExternalIdentity, user: &PrincipalId) -> Result<Mutated<ExternalIdentity>, TenancyError> {
        if existing.principal_id == *user {
            Ok(Mutated { value: existing, changed: false })
        } else {
            Err(TenancyError::ExternalIdentityConflict)
        }
    }

    /// `UnlinkExternalIdentity`. `caller` is the credential that authenticated this request. The
    /// call refuses to remove that credential's own identity (spec 5.3). An API key has no such
    /// identity, so the guard does not apply to it.
    pub async fn unlink(&self, actor: &Prn, caller: &Credential, user: &PrincipalId, identity_id: Uuid, reason: &str) -> Result<(), TenancyError> {
        self.authorize.check(actor, Action::UnlinkExternalIdentity, &root_prn()).await?;
        let reason = AuditReason::parse(reason)?;

        let tx = self.uow.begin().await?;
        let Some(stored) = self.links.lock_user_in(&*tx, user).await? else {
            return Err(TenancyError::NotFound);
        };
        // Code review F5: the guard runs BEFORE any delete, so a refusal writes nothing. The
        // identity row is immutable, so the id read here is the row that `unlink_in` would delete.
        if let Credential::Oidc { issuer, subject, .. } = caller
            && let Some(own) = self.links.find_identity_in(&*tx, issuer, subject).await?
            && own.id == identity_id
            && own.principal_id == stored
        {
            return Err(TenancyError::CannotUnlinkOwnIdentity);
        }
        let Some(removed) = self.links.unlink_in(&*tx, &stored, identity_id).await? else {
            return Err(TenancyError::NotFound);
        };
        // `issuer` and `subject` come from the deleted row, not from the request (spec 6.4).
        let detail = serde_json::json!({
            "reason": reason.as_str(),
            "identity_id": removed.id.to_string(),
            "issuer": removed.issuer.as_str(),
            "subject": removed.subject,
        });
        let entry = self.audit_entry(actor, Action::UnlinkExternalIdentity, &stored, detail);
        self.audit.record(&*tx, &entry).await?;
        tx.commit().await?;
        Ok(())
    }

    /// `ChangeUserEmail`. The no-op decision and `old_email` come from the row that the
    /// transaction locked (spec 5.4, 6.3).
    pub async fn change_email(&self, actor: &Prn, user: &PrincipalId, email: &str, reason: &str) -> Result<UserWithIdentities, TenancyError> {
        self.authorize.check(actor, Action::ChangeUserEmail, &root_prn()).await?;
        let email = parse_email(email)?;
        let reason = AuditReason::parse(reason)?;

        let tx = self.uow.begin().await?;
        // The store applies the stored-PRN rule before it locks (`PrnMismatch`).
        let Some(change) = self.links.change_email_in(&*tx, user, &email, self.clock.now()).await? else {
            return Err(TenancyError::NotFound);
        };
        // The row is locked, so it cannot vanish between the change and this read. The view
        // carries the stored PRN, which the audit entry below names.
        let view = self.links.user_view_in(&*tx, user).await?.ok_or(TenancyError::Internal)?;
        if change.changed {
            // `old_email` is the raw stored value. It can be a value that does not parse, when
            // this call repairs a hand-edited row (code review F6).
            let detail = serde_json::json!({
                "reason": reason.as_str(),
                "old_email": change.value.old,
                "new_email": change.value.new.as_str(),
            });
            let entry = self.audit_entry(actor, Action::ChangeUserEmail, &view.user.principal_id, detail);
            self.audit.record(&*tx, &entry).await?;
        }
        tx.commit().await?;
        Ok(view)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::fakes::{FakeAuditLog, FakeAuthorizer, FakeUnitOfWork, FixedClock, InMemoryIdentityLinks, SeqIds};
    use async_trait::async_trait;
    use chrono::{DateTime, TimeZone, Utc};
    use paigasus_iam_core::{ApiKeyId, AuditFilter, RepositoryError, Transaction, User};
    use serde_json::json;

    const ISSUER: &str = "https://idp.example.com/realms/main";
    const SECOND_ISSUER: &str = "https://second.example.com";
    const ALL_FOUR: [Action; 4] = [Action::GetUser, Action::LinkExternalIdentity, Action::UnlinkExternalIdentity, Action::ChangeUserEmail];

    fn at(secs: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(secs, 0).unwrap()
    }

    fn now() -> DateTime<Utc> {
        at(1_750_000_000)
    }

    fn pid(n: u128) -> PrincipalId {
        PrincipalId::from_prn(Prn::build("iam", "", None, "principal", Uuid::from_u128(n)).unwrap())
    }

    fn actor() -> Prn {
        pid(1).prn().clone()
    }

    fn iss(raw: &str) -> Issuer {
        Issuer::parse(raw).unwrap()
    }

    fn user(n: u128, email: &str) -> User {
        User::new(pid(n), Email::parse(email).unwrap(), format!("User {n}"), None, None, at(1_700_000_000), at(1_700_000_000))
    }

    fn identity(id: u128, owner: u128, issuer: &str, subject: &str, created: i64) -> ExternalIdentity {
        ExternalIdentity {
            id: Uuid::from_u128(id),
            principal_id: pid(owner),
            issuer: iss(issuer),
            subject: subject.to_string(),
            created_at: at(created),
            updated_at: at(created),
        }
    }

    fn oidc(issuer: &str, subject: &str) -> Credential {
        Credential::Oidc {
            issuer: iss(issuer),
            subject: subject.to_string(),
            expires_at: at(1_800_000_000),
        }
    }

    fn api_key() -> Credential {
        Credential::ApiKey {
            key_id: ApiKeyId::from_uuid(Uuid::from_u128(77)),
            expires_at: None,
            scope_prn: root_prn().canonical(),
        }
    }

    struct Fixture {
        svc: UserIdentityService,
        links: InMemoryIdentityLinks,
        audit: FakeAuditLog,
        uow: FakeUnitOfWork,
        authz: FakeAuthorizer,
    }

    fn fixture_with(allow: &[Action], audit_port: Option<Arc<dyn AuditLog>>) -> Fixture {
        let authz = FakeAuthorizer::default();
        for a in allow {
            authz.allow(*a, &root_prn());
        }
        let links = InMemoryIdentityLinks::default();
        let audit = FakeAuditLog::default();
        let uow = FakeUnitOfWork::default();
        let clock = FixedClock::default();
        clock.set(now());
        let svc = UserIdentityService::new(UserIdentityDeps {
            authorize: Authorize::new(Arc::new(authz.clone())),
            links: Arc::new(links.clone()),
            uow: Arc::new(uow.clone()),
            audit: audit_port.unwrap_or_else(|| Arc::new(audit.clone()) as Arc<dyn AuditLog>),
            issuers: vec![iss(ISSUER), iss(SECOND_ISSUER)],
            ids: Arc::new(SeqIds::default()),
            clock: Arc::new(clock),
        });
        Fixture { svc, links, audit, uow, authz }
    }

    fn fixture(allow: &[Action]) -> Fixture {
        fixture_with(allow, None)
    }

    /// An `AuditLog` whose in-transaction write always fails, to prove that a failed audit write
    /// fails the call and never commits.
    struct FailingRecord;

    #[async_trait]
    impl AuditLog for FailingRecord {
        async fn record_out_of_band(&self, _e: &AuditEntry) -> Result<(), RepositoryError> {
            unimplemented!("the identity calls audit inside the transaction")
        }

        async fn record(&self, _tx: &dyn Transaction, _e: &AuditEntry) -> Result<(), RepositoryError> {
            Err(RepositoryError::Backend(Box::new(std::io::Error::other("audit sink down"))))
        }

        async fn query(&self, _f: &AuditFilter) -> Result<Vec<AuditEntry>, RepositoryError> {
            unimplemented!("the identity calls never query")
        }
    }

    /// Spec 5.1 and 11: each method asks for ITS OWN action, at Root, and asks first. The fake
    /// denies everything, so each call must stop at the check with no store call.
    #[tokio::test]
    async fn every_method_checks_its_own_action_at_root_before_any_store_call() {
        let root = root_prn().canonical();

        let f = fixture(&[]);
        assert_eq!(f.svc.find_by_email(&actor(), "a@example.com").await.unwrap_err(), TenancyError::Forbidden);
        assert_eq!(f.authz.checks(), vec![(Action::GetUser, root.clone())]);
        assert_eq!(f.links.calls(), 0);

        let f = fixture(&[]);
        assert_eq!(f.svc.link(&actor(), &pid(10), ISSUER, "sub-1", "reason").await.unwrap_err(), TenancyError::Forbidden);
        assert_eq!(f.authz.checks(), vec![(Action::LinkExternalIdentity, root.clone())]);
        assert_eq!(f.links.calls(), 0);

        let f = fixture(&[]);
        assert_eq!(f.svc.unlink(&actor(), &api_key(), &pid(10), Uuid::from_u128(5), "reason").await.unwrap_err(), TenancyError::Forbidden);
        assert_eq!(f.authz.checks(), vec![(Action::UnlinkExternalIdentity, root.clone())]);
        assert_eq!(f.links.calls(), 0);

        let f = fixture(&[]);
        assert_eq!(f.svc.change_email(&actor(), &pid(10), "b@example.com", "reason").await.unwrap_err(), TenancyError::Forbidden);
        assert_eq!(f.authz.checks(), vec![(Action::ChangeUserEmail, root)]);
        assert_eq!(f.links.calls(), 0);
        assert_eq!(f.uow.commits(), 0);
    }

    /// A caller that holds the other three actions, but not this method's own, is denied.
    #[tokio::test]
    async fn holding_the_other_three_actions_does_not_open_a_method() {
        for (index, action) in ALL_FOUR.iter().enumerate() {
            let others: Vec<Action> = ALL_FOUR.iter().copied().filter(|a| a != action).collect();
            let f = fixture(&others);
            f.links.seed_user(user(10, "u@example.com"));
            let err = match index {
                0 => f.svc.find_by_email(&actor(), "u@example.com").await.map(|_| ()).unwrap_err(),
                1 => f.svc.link(&actor(), &pid(10), ISSUER, "sub-1", "reason").await.map(|_| ()).unwrap_err(),
                2 => f.svc.unlink(&actor(), &api_key(), &pid(10), Uuid::from_u128(5), "reason").await.unwrap_err(),
                _ => f.svc.change_email(&actor(), &pid(10), "v@example.com", "reason").await.map(|_| ()).unwrap_err(),
            };
            assert_eq!(err, TenancyError::Forbidden, "{} must need its own action", action.as_wire());
        }
    }

    /// Spec 5.1: the check runs before any validation, so a denied caller cannot use a 400 to
    /// learn anything.
    #[tokio::test]
    async fn a_denied_caller_gets_forbidden_even_when_every_value_is_invalid() {
        let f = fixture(&[]);
        assert_eq!(f.svc.find_by_email(&actor(), "not-an-email").await.unwrap_err(), TenancyError::Forbidden);
        assert_eq!(
            f.svc.link(&actor(), &pid(10), "https://unknown.example.com/", " padded", "").await.unwrap_err(),
            TenancyError::Forbidden
        );
        assert_eq!(f.svc.unlink(&actor(), &api_key(), &pid(10), Uuid::from_u128(5), "   ").await.unwrap_err(), TenancyError::Forbidden);
        assert_eq!(f.svc.change_email(&actor(), &pid(10), "@", "").await.unwrap_err(), TenancyError::Forbidden);
        assert_eq!(f.links.calls(), 0, "a denied caller must never reach the store");
    }

    #[tokio::test]
    async fn find_by_email_returns_the_user_and_its_identities_in_creation_order() {
        let f = fixture(&[Action::GetUser]);
        f.links.seed_user(user(10, "u@example.com"));
        f.links.seed_identity(identity(21, 10, SECOND_ISSUER, "later", 1_700_000_200));
        f.links.seed_identity(identity(20, 10, ISSUER, "earlier", 1_700_000_100));
        let view = f.svc.find_by_email(&actor(), "  u@example.com ").await.unwrap();
        assert_eq!(view.user.principal_id, pid(10));
        let subjects: Vec<&str> = view.identities.iter().map(|i| i.subject.as_str()).collect();
        assert_eq!(subjects, vec!["earlier", "later"]);
        assert!(f.audit.0.lock().unwrap().is_empty(), "a find writes no audit record (spec 13, question 1)");
    }

    #[tokio::test]
    async fn find_by_email_rejects_an_invalid_email_and_reports_an_unknown_one_as_not_found() {
        let f = fixture(&[Action::GetUser]);
        assert!(matches!(f.svc.find_by_email(&actor(), "not-an-email").await, Err(TenancyError::InvalidEmail(_))));
        assert_eq!(f.svc.find_by_email(&actor(), "nobody@example.com").await.unwrap_err(), TenancyError::NotFound);
    }

    #[tokio::test]
    async fn link_creates_the_identity_and_writes_one_audit_entry() {
        let f = fixture(&[Action::LinkExternalIdentity]);
        f.links.seed_user(user(10, "u@example.com"));
        let out = f.svc.link(&actor(), &pid(10), &format!("  {ISSUER} "), "sub-1", "  INC-7: second issuer  ").await.unwrap();
        assert!(out.changed);
        assert_eq!(out.value.issuer.as_str(), ISSUER, "the stored issuer is the configured value, not the padded input");
        assert_eq!(out.value.subject, "sub-1");
        assert_eq!(out.value.principal_id, pid(10));
        assert_eq!(out.value.created_at, now());
        assert_eq!(f.links.identities(), vec![out.value.clone()]);
        assert_eq!(f.uow.commits(), 1);
        let entries = f.audit.0.lock().unwrap();
        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(entry.action, "LinkExternalIdentity");
        assert_eq!(entry.outcome, AuditOutcome::Committed);
        assert_eq!(entry.actor_prn, Some(actor().canonical()));
        assert_eq!(entry.resource_prn, Some(pid(10).canonical()));
        assert_eq!(
            entry.detail,
            json!({"reason": "INC-7: second issuer", "identity_id": out.value.id.to_string(), "issuer": ISSUER, "subject": "sub-1"})
        );
    }

    /// Spec 5.2: a retry after a lost response is safe. The same user already holds the pair,
    /// so the call returns the stored identity, writes no audit record and commits nothing.
    #[tokio::test]
    async fn linking_an_identity_the_user_already_holds_returns_it_unchanged() {
        let f = fixture(&[Action::LinkExternalIdentity]);
        f.links.seed_user(user(10, "u@example.com"));
        let held = identity(20, 10, ISSUER, "sub-1", 1_700_000_100);
        f.links.seed_identity(held.clone());
        let out = f.svc.link(&actor(), &pid(10), ISSUER, "sub-1", "retry").await.unwrap();
        assert!(!out.changed);
        assert_eq!(out.value, held);
        assert!(f.audit.0.lock().unwrap().is_empty(), "a same-user link writes no audit record");
        assert_eq!(f.uow.commits(), 0);
    }

    #[tokio::test]
    async fn linking_an_identity_that_another_user_holds_is_a_conflict() {
        let f = fixture(&[Action::LinkExternalIdentity]);
        f.links.seed_user(user(10, "u@example.com"));
        f.links.seed_user(user(11, "v@example.com"));
        f.links.seed_identity(identity(20, 11, ISSUER, "sub-1", 1_700_000_100));
        assert_eq!(f.svc.link(&actor(), &pid(10), ISSUER, "sub-1", "r").await.unwrap_err(), TenancyError::ExternalIdentityConflict);
        assert_eq!(f.links.identities().len(), 1, "there is no implicit move");
        assert!(f.audit.0.lock().unwrap().is_empty());
        assert_eq!(f.uow.commits(), 0);
    }

    /// Spec 6.3 step 3: a JIT login can insert the pair between the read and the insert. The
    /// unique violation then gives 409, not 500.
    #[tokio::test]
    async fn a_unique_violation_at_insert_time_is_a_conflict() {
        let f = fixture(&[Action::LinkExternalIdentity]);
        f.links.seed_user(user(10, "u@example.com"));
        f.links.fail_next_link_with_conflict();
        assert_eq!(f.svc.link(&actor(), &pid(10), ISSUER, "sub-1", "r").await.unwrap_err(), TenancyError::ExternalIdentityConflict);
        assert!(f.audit.0.lock().unwrap().is_empty());
        assert_eq!(f.uow.commits(), 0);
    }

    /// Code review F2: a concurrent link of the same pair to the SAME user commits first. The
    /// insert then fails on the unique constraint. The service reads the pair again, finds this
    /// user, and answers the safe retry: the stored identity, `changed: false`, no audit.
    #[tokio::test]
    async fn a_unique_violation_from_a_same_user_link_is_an_unchanged_retry() {
        let f = fixture(&[Action::LinkExternalIdentity]);
        f.links.seed_user(user(10, "u@example.com"));
        let winner = identity(20, 10, ISSUER, "sub-1", 1_700_000_100);
        f.links.race_next_link_with(winner.clone());
        let out = f.svc.link(&actor(), &pid(10), ISSUER, "sub-1", "r").await.unwrap();
        assert!(!out.changed);
        assert_eq!(out.value, winner);
        assert!(f.audit.0.lock().unwrap().is_empty());
        assert_eq!(f.uow.commits(), 0);
    }

    /// Code review F2, the other half: when ANOTHER user won the race, the answer stays 409.
    #[tokio::test]
    async fn a_unique_violation_from_another_users_link_stays_a_conflict() {
        let f = fixture(&[Action::LinkExternalIdentity]);
        f.links.seed_user(user(10, "u@example.com"));
        f.links.seed_user(user(11, "v@example.com"));
        f.links.race_next_link_with(identity(20, 11, ISSUER, "sub-1", 1_700_000_100));
        assert_eq!(f.svc.link(&actor(), &pid(10), ISSUER, "sub-1", "r").await.unwrap_err(), TenancyError::ExternalIdentityConflict);
        assert!(f.audit.0.lock().unwrap().is_empty());
        assert_eq!(f.uow.commits(), 0);
    }

    #[tokio::test]
    async fn link_to_an_unknown_principal_is_not_found() {
        let f = fixture(&[Action::LinkExternalIdentity]);
        assert_eq!(f.svc.link(&actor(), &pid(10), ISSUER, "sub-1", "r").await.unwrap_err(), TenancyError::NotFound);
        assert!(f.audit.0.lock().unwrap().is_empty());
    }

    /// Review focus 2: the issuer match is exact after trim.
    #[tokio::test]
    async fn link_matches_the_issuer_exactly_after_trim() {
        let f = fixture(&[Action::LinkExternalIdentity]);
        f.links.seed_user(user(10, "u@example.com"));
        for bad in [format!("{ISSUER}/"), ISSUER.to_uppercase(), "https://idp.example.com/realms/other".to_string(), String::new()] {
            assert_eq!(f.svc.link(&actor(), &pid(10), &bad, "sub-1", "r").await.unwrap_err(), TenancyError::UnknownIssuer, "{bad:?}");
        }
        assert_eq!(f.links.calls(), 0, "an unknown issuer is refused before any store call");
        let out = f.svc.link(&actor(), &pid(10), &format!("\t{SECOND_ISSUER}\n"), "sub-2", "r").await.unwrap();
        assert_eq!(out.value.issuer.as_str(), SECOND_ISSUER);
    }

    #[tokio::test]
    async fn link_validates_issuer_then_subject_then_reason() {
        let f = fixture(&[Action::LinkExternalIdentity]);
        f.links.seed_user(user(10, "u@example.com"));
        assert_eq!(f.svc.link(&actor(), &pid(10), "https://nope.example.com", " bad", "").await.unwrap_err(), TenancyError::UnknownIssuer);
        assert_eq!(f.svc.link(&actor(), &pid(10), ISSUER, " bad", "").await.unwrap_err(), TenancyError::InvalidSubject);
        assert_eq!(f.svc.link(&actor(), &pid(10), ISSUER, "good", "  ").await.unwrap_err(), TenancyError::InvalidReason);
        assert_eq!(f.links.calls(), 0);
        assert_eq!(f.uow.commits(), 0);
    }

    /// Spec 6.4: the audit `issuer` and `subject` of an unlink come from the deleted row.
    #[tokio::test]
    async fn unlink_removes_the_identity_and_audits_the_deleted_row() {
        let f = fixture(&[Action::UnlinkExternalIdentity]);
        f.links.seed_user(user(10, "u@example.com"));
        f.links.seed_identity(identity(20, 10, ISSUER, "old-sub", 1_700_000_100));
        f.svc.unlink(&actor(), &oidc(ISSUER, "operator-sub"), &pid(10), Uuid::from_u128(20), " INC-9: dead sub ").await.unwrap();
        assert!(f.links.identities().is_empty());
        assert_eq!(f.uow.commits(), 1);
        let entries = f.audit.0.lock().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].action, "UnlinkExternalIdentity");
        assert_eq!(entries[0].resource_prn, Some(pid(10).canonical()));
        assert_eq!(
            entries[0].detail,
            json!({"reason": "INC-9: dead sub", "identity_id": Uuid::from_u128(20).to_string(), "issuer": ISSUER, "subject": "old-sub"})
        );
    }

    #[tokio::test]
    async fn unlink_of_an_unknown_identity_or_another_users_identity_is_not_found() {
        let f = fixture(&[Action::UnlinkExternalIdentity]);
        f.links.seed_user(user(10, "u@example.com"));
        f.links.seed_user(user(11, "v@example.com"));
        f.links.seed_identity(identity(20, 11, ISSUER, "sub-1", 1_700_000_100));
        assert_eq!(f.svc.unlink(&actor(), &api_key(), &pid(10), Uuid::from_u128(20), "r").await.unwrap_err(), TenancyError::NotFound);
        assert_eq!(f.svc.unlink(&actor(), &api_key(), &pid(10), Uuid::from_u128(99), "r").await.unwrap_err(), TenancyError::NotFound);
        assert_eq!(f.svc.unlink(&actor(), &api_key(), &pid(12), Uuid::from_u128(20), "r").await.unwrap_err(), TenancyError::NotFound);
        assert_eq!(f.links.identities().len(), 1);
        assert!(f.audit.0.lock().unwrap().is_empty());
        assert_eq!(f.uow.commits(), 0);
    }

    /// Spec 5.3: the caller cannot unlink the identity that authenticated the request.
    #[tokio::test]
    async fn unlinking_the_identity_that_authenticated_the_request_is_refused() {
        let f = fixture(&[Action::UnlinkExternalIdentity]);
        f.links.seed_user(user(10, "u@example.com"));
        f.links.seed_identity(identity(20, 10, ISSUER, "me", 1_700_000_100));
        let err = f.svc.unlink(&actor(), &oidc(ISSUER, "me"), &pid(10), Uuid::from_u128(20), "r").await.unwrap_err();
        assert_eq!(err, TenancyError::CannotUnlinkOwnIdentity);
        assert!(f.audit.0.lock().unwrap().is_empty());
        assert_eq!(f.uow.commits(), 0, "the refused delete must not commit");
        // Code review F5: the guard runs BEFORE any delete. The fake does not roll back, so a
        // delete that ran before the guard would leave the identity gone here.
        assert_eq!(f.links.identities(), vec![identity(20, 10, ISSUER, "me", 1_700_000_100)], "the guard must refuse before any delete");
    }

    /// Code review F5: the guard refuses only the caller's own identity OF THIS USER. When the
    /// caller's identity belongs to another user, the unlink of this user finds no such row.
    #[tokio::test]
    async fn the_own_identity_guard_does_not_hide_a_not_found() {
        let f = fixture(&[Action::UnlinkExternalIdentity]);
        f.links.seed_user(user(10, "u@example.com"));
        f.links.seed_user(user(11, "v@example.com"));
        f.links.seed_identity(identity(20, 11, ISSUER, "me", 1_700_000_100));
        let err = f.svc.unlink(&actor(), &oidc(ISSUER, "me"), &pid(10), Uuid::from_u128(20), "r").await.unwrap_err();
        assert_eq!(err, TenancyError::NotFound);
        assert_eq!(f.links.identities().len(), 1);
    }

    /// Code review F1: SMA-649's stored-PRN rule. A `PrincipalId` whose PRN carries a region or an
    /// organization slot names a stored principal by uuid, but its PRN differs from the stored
    /// one. Each write answers `PrnMismatch` and changes nothing.
    #[tokio::test]
    async fn a_user_prn_that_differs_from_the_stored_prn_is_a_prn_mismatch() {
        let uuid = Uuid::from_u128(10);
        let regional = PrincipalId::from_prn(Prn::build("iam", "eu-west-1", None, "principal", uuid).unwrap());
        let org_slot = PrincipalId::from_prn(Prn::build("iam", "", Some(Uuid::from_u128(99)), "principal", uuid).unwrap());
        for forged in [regional, org_slot] {
            let f = fixture(&ALL_FOUR);
            f.links.seed_user(user(10, "u@example.com"));
            f.links.seed_identity(identity(20, 10, ISSUER, "held", 1_700_000_100));
            assert_eq!(f.svc.link(&actor(), &forged, ISSUER, "new-sub", "r").await.unwrap_err(), TenancyError::PrnMismatch, "{forged:?}");
            assert_eq!(
                f.svc.unlink(&actor(), &api_key(), &forged, Uuid::from_u128(20), "r").await.unwrap_err(),
                TenancyError::PrnMismatch,
                "{forged:?}"
            );
            assert_eq!(f.svc.change_email(&actor(), &forged, "v@example.com", "r").await.unwrap_err(), TenancyError::PrnMismatch, "{forged:?}");
            assert_eq!(f.links.identities(), vec![identity(20, 10, ISSUER, "held", 1_700_000_100)]);
            assert_eq!(f.links.user(&pid(10)).unwrap().email.as_str(), "u@example.com");
            assert!(f.audit.0.lock().unwrap().is_empty());
            assert_eq!(f.uow.commits(), 0);
        }
    }

    /// Review focus 5: the guard compares issuer AND subject, and an API key has no identity.
    #[tokio::test]
    async fn the_own_identity_guard_matches_issuer_and_subject_together() {
        for caller in [api_key(), oidc(ISSUER, "someone-else"), oidc(SECOND_ISSUER, "me")] {
            let f = fixture(&[Action::UnlinkExternalIdentity]);
            f.links.seed_user(user(10, "u@example.com"));
            f.links.seed_identity(identity(20, 10, ISSUER, "me", 1_700_000_100));
            f.svc
                .unlink(&actor(), &caller, &pid(10), Uuid::from_u128(20), "r")
                .await
                .unwrap_or_else(|e| panic!("{caller:?}: {e:?}"));
            assert_eq!(f.uow.commits(), 1, "{caller:?}");
        }
    }

    #[tokio::test]
    async fn unlink_rejects_an_invalid_reason_before_any_store_call() {
        let f = fixture(&[Action::UnlinkExternalIdentity]);
        assert_eq!(f.svc.unlink(&actor(), &api_key(), &pid(10), Uuid::from_u128(20), "\n").await.unwrap_err(), TenancyError::InvalidReason);
        assert_eq!(f.links.calls(), 0);
    }

    #[tokio::test]
    async fn change_email_updates_the_row_and_audits_old_and_new() {
        let f = fixture(&[Action::ChangeUserEmail]);
        f.links.seed_user(user(10, "old@example.com"));
        let view = f.svc.change_email(&actor(), &pid(10), " new@example.com ", "INC-3: moved").await.unwrap();
        assert_eq!(view.user.email.as_str(), "new@example.com");
        assert_eq!(view.user.updated_at, now());
        assert_eq!(f.uow.commits(), 1);
        let entries = f.audit.0.lock().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].action, "ChangeUserEmail");
        assert_eq!(entries[0].resource_prn, Some(pid(10).canonical()));
        assert_eq!(entries[0].detail, json!({"reason": "INC-3: moved", "old_email": "old@example.com", "new_email": "new@example.com"}));
    }

    /// Spec 5.4: a change to the current email is a no-op. SMA-606 D1: it does not restamp.
    #[tokio::test]
    async fn changing_to_the_current_email_is_a_no_op_with_no_audit_record() {
        let f = fixture(&[Action::ChangeUserEmail]);
        f.links.seed_user(user(10, "same@example.com"));
        let view = f.svc.change_email(&actor(), &pid(10), " same@example.com ", "r").await.unwrap();
        assert_eq!(view.user.updated_at, at(1_700_000_000), "a no-op must not restamp");
        assert!(f.audit.0.lock().unwrap().is_empty());
    }

    /// Review focus 3: the match is exact and case-sensitive, as in JIT and the unique key.
    #[tokio::test]
    async fn an_email_that_differs_only_in_letter_case_is_a_change_not_a_conflict() {
        let f = fixture(&[Action::ChangeUserEmail]);
        f.links.seed_user(user(10, "person@example.com"));
        f.links.seed_user(user(11, "other@example.com"));
        let view = f.svc.change_email(&actor(), &pid(10), "Person@Example.com", "case fix").await.unwrap();
        assert_eq!(view.user.email.as_str(), "Person@Example.com");
        assert_eq!(f.audit.0.lock().unwrap().len(), 1, "a case change is a real change");
        let other = f.svc.change_email(&actor(), &pid(11), "PERSON@EXAMPLE.COM", "r").await.unwrap();
        assert_eq!(other.user.email.as_str(), "PERSON@EXAMPLE.COM", "a case variant of another user's email is not a conflict");
    }

    #[tokio::test]
    async fn change_email_reports_a_conflict_an_unknown_user_and_invalid_input() {
        let f = fixture(&[Action::ChangeUserEmail]);
        f.links.seed_user(user(10, "a@example.com"));
        f.links.seed_user(user(11, "b@example.com"));
        assert_eq!(f.svc.change_email(&actor(), &pid(10), "b@example.com", "r").await.unwrap_err(), TenancyError::EmailConflict);
        assert_eq!(f.svc.change_email(&actor(), &pid(12), "c@example.com", "r").await.unwrap_err(), TenancyError::NotFound);
        assert!(matches!(f.svc.change_email(&actor(), &pid(10), "no-at-sign", "r").await, Err(TenancyError::InvalidEmail(_))));
        assert_eq!(f.svc.change_email(&actor(), &pid(10), "c@example.com", " ").await.unwrap_err(), TenancyError::InvalidReason);
        assert!(f.audit.0.lock().unwrap().is_empty());
    }

    /// Spec 6.1: if the audit write fails, the change must not commit.
    #[tokio::test]
    async fn a_failed_audit_write_fails_the_call_and_never_commits() {
        let f = fixture_with(&ALL_FOUR, Some(Arc::new(FailingRecord)));
        f.links.seed_user(user(10, "u@example.com"));
        f.links.seed_identity(identity(20, 10, ISSUER, "old", 1_700_000_100));
        assert_eq!(f.svc.link(&actor(), &pid(10), ISSUER, "new", "r").await.unwrap_err(), TenancyError::Internal);
        assert_eq!(f.svc.unlink(&actor(), &api_key(), &pid(10), Uuid::from_u128(20), "r").await.unwrap_err(), TenancyError::Internal);
        assert_eq!(f.svc.change_email(&actor(), &pid(10), "v@example.com", "r").await.unwrap_err(), TenancyError::Internal);
        assert_eq!(f.uow.commits(), 0);
    }
}
