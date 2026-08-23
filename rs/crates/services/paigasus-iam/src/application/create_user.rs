// SPDX-License-Identifier: Apache-2.0

//! `CreateUser` use case: mint an identity, build a `User` principal, persist it.
//!
//! **SMA-446 Slice B Task B7 — the Unit-of-Work reference pattern, OUTBOX-ONLY (copied from
//! `RoleService::grant`, `application::roles`'s module docs, minus the audit/gen-bump halves):**
//! principal creation is NOT in the AC audit set (spec), so `execute` drives the principal+user
//! insert and its `iam.principal.created` [`DomainEvent`] through ONE [`UnitOfWork`]-scoped
//! transaction (`repo.create_user_in`, `outbox.enqueue`, then `tx.commit()`) — no `AuditEntry`,
//! no `PolicyGenBumper` (`CreateUserDeps` deliberately has neither field). A mid-txn failure
//! (e.g. the `uq_user_email` unique-violation `CreateUser`'s duplicate-email 409 is built on)
//! rolls the whole unit of work back before `Outbox::enqueue` is ever reached, so a rejected
//! create emits nothing — the existing `Conflict(ConflictKind::EmailTaken)` -> `TenancyError::
//! EmailConflict` mapping (`application::error`) is unchanged. `CreateUser::execute` still
//! takes no `actor: &Prn` parameter, so the emitted event's `actor_prn` is always `None` —
//! that is now a KNOWN GAP, not a domain property: as of SMA-584 both callers
//! (`adapters::http::users`, `adapters::grpc::users`) have already authenticated AND
//! authorized the caller (`Action::CreateUser` at Root, a `platform_admin` under the starter
//! role set) before invoking `execute`, so a successful create is currently UNATTRIBUTABLE —
//! no `audit_log` row, no actor on the event. This is a deliberately deferred follow-up
//! (design doc D2, "accepted cost 1"): threading `actor: &Prn` into `execute` would fix
//! attribution independently of where the authorization check lives. The payload stays
//! deliberately PII-minimal: `principal_id` + `kind` only, never the email address.
use std::sync::Arc;

use paigasus_iam_core::{Clock, DomainEvent, Email, EventType, IdGenerator, Outbox, Principal, PrincipalId, PrincipalKind, PrincipalRepository, PrincipalStatus, RepositoryError, UnitOfWork, User};

use crate::application::error::TenancyError;

/// Input to create a user principal.
#[derive(Debug, Clone)]
pub struct NewUser {
    pub email: String,
    pub display_name: String,
    pub locale: Option<String>,
    pub timezone: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum CreateUserError {
    #[error("invalid email: {0}")]
    InvalidEmail(String),
    #[error(transparent)]
    Repository(#[from] RepositoryError),
}

/// Lets the `/v1/users` handler use `?` against `ApiError` (which requires
/// `Into<TenancyError>`) the same way the tenancy-service handlers do.
impl From<CreateUserError> for TenancyError {
    fn from(err: CreateUserError) -> Self {
        match err {
            CreateUserError::InvalidEmail(e) => TenancyError::InvalidEmail(e),
            CreateUserError::Repository(r) => r.into(),
        }
    }
}

// `Clone` lets the composition root (`http::AppState`, Task 14) hold a `UserSvc` handle
// inside its own `#[derive(Clone)] AppState`, mirroring the tenancy services' shape. `uow`/
// `outbox` are `Arc<dyn ...>` shared handles (SMA-446 Slice B Task B7, module docs); `repo`/
// `id_gen`/`clock` stay generic-DI, mirroring `ServiceAccountService`'s `repo`.
#[derive(Clone)]
pub struct CreateUser<R, I, C> {
    repo: R,
    uow: Arc<dyn UnitOfWork>,
    outbox: Arc<dyn Outbox>,
    id_gen: I,
    clock: C,
}

/// Named-field constructor params for [`CreateUser::new`] (SMA-446 Slice B Task B7) — copies
/// `application::roles::RoleServiceDeps`'s DI-params idiom (module docs there): one field per
/// dependency, built with struct syntax at the call site so each argument is self-labeling.
/// Deliberately has NO `audit`/`gen_bumper` field — module docs: principal creation is
/// OUTBOX-ONLY, not in the AC audit set.
pub struct CreateUserDeps<R, I, C> {
    pub repo: R,
    pub uow: Arc<dyn UnitOfWork>,
    pub outbox: Arc<dyn Outbox>,
    pub id_gen: I,
    pub clock: C,
}

impl<R, I, C> CreateUser<R, I, C>
where
    R: PrincipalRepository,
    I: IdGenerator,
    C: Clock,
{
    pub fn new(deps: CreateUserDeps<R, I, C>) -> Self {
        CreateUser {
            repo: deps.repo,
            uow: deps.uow,
            outbox: deps.outbox,
            id_gen: deps.id_gen,
            clock: deps.clock,
        }
    }

    /// Validates `cmd.email` and mints the identity BEFORE ever opening a transaction (an
    /// invalid email must never consume an id or touch the UoW). Then drives the principal+
    /// user insert and its `iam.principal.created` `DomainEvent` through ONE UoW transaction
    /// (module docs — OUTBOX-ONLY, no audit row, no gen bump): `repo.create_user_in`, `outbox.
    /// enqueue`, `tx.commit()`. A duplicate-email unique-violation inside `create_user_in`
    /// rolls the whole unit of work back before the event is ever enqueued — `CreateUserError::
    /// Repository` -> `TenancyError::EmailConflict` unchanged (`application::error`), and no
    /// event is emitted for a create that never actually committed.
    pub async fn execute(&self, cmd: NewUser) -> Result<PrincipalId, CreateUserError> {
        let email = Email::parse(&cmd.email).map_err(|_| CreateUserError::InvalidEmail(cmd.email.clone()))?;
        let id = self.id_gen.new_principal_id();
        let now = self.clock.now();

        let principal = Principal::new(id.clone(), PrincipalKind::User, PrincipalStatus::Active, now, now);
        let user = User::new(id.clone(), email, cmd.display_name, cmd.locale, cmd.timezone, now, now);

        // `actor_prn: None` — `execute` has no `actor: &Prn` parameter (module docs): a known
        // attribution gap (design doc D2, "accepted cost 1"), not a property of the domain —
        // both current callers already have an authorized caller identity. The payload is
        // PII-minimal: `principal_id` + `kind` only, never the email address.
        let event = DomainEvent {
            id: self.id_gen.new_event_id(),
            event_type: EventType::PrincipalCreated,
            schema_version: 1,
            aggregate_prn: id.canonical(),
            actor_prn: None,
            occurred_at: now,
            payload: serde_json::json!({"principal_id": id.uuid(), "kind": "user"}),
            correlation_id: Some(self.id_gen.new_correlation_id()),
        };

        let tx = self.uow.begin().await?;
        self.repo.create_user_in(&*tx, &principal, &user).await?;
        self.outbox.enqueue(&*tx, &event).await?;
        tx.commit().await?;

        Ok(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::fakes::{FakeOutbox, FakeUnitOfWork};
    use async_trait::async_trait;
    use chrono::{DateTime, TimeZone, Utc};
    use paigasus_iam_core::{ApiKeyId, ConflictKind, OrganizationId, ProjectId, TeamId, Transaction};
    use paigasus_kernel::Prn;
    use std::collections::HashMap;
    use std::sync::Mutex;
    use uuid::Uuid;

    #[derive(Default)]
    struct InMemoryPrincipalRepository {
        rows: Mutex<HashMap<Uuid, (Principal, User)>>,
    }

    #[async_trait]
    impl PrincipalRepository for InMemoryPrincipalRepository {
        async fn create_user(&self, p: &Principal, u: &User) -> Result<(), RepositoryError> {
            let mut rows = self.rows.lock().unwrap();
            if rows.contains_key(&p.id.uuid()) {
                // A duplicate principal id is a UUIDv7 collision, not a genuine business
                // conflict — `conflict_kind` maps that case to `Other` (see pg_repository.rs).
                return Err(RepositoryError::Conflict(ConflictKind::Other));
            }
            if rows.values().any(|(_, existing)| existing.email == u.email) {
                return Err(RepositoryError::Conflict(ConflictKind::EmailTaken));
            }
            rows.insert(p.id.uuid(), (p.clone(), u.clone()));
            Ok(())
        }

        // Txn-scoped twin (SMA-446, Slice B Task B7 — the `CreateUser::execute` reference
        // pattern): this fake has no real backing transaction, so `tx` is ignored and the
        // mutation applies immediately — mirrors `application::fakes::InMemoryRoleGrants::
        // grant`/`grant_in`.
        async fn create_user_in(&self, _tx: &dyn Transaction, p: &Principal, u: &User) -> Result<(), RepositoryError> {
            self.create_user(p, u).await
        }

        async fn find_user(&self, id: &PrincipalId) -> Result<Option<(Principal, User)>, RepositoryError> {
            Ok(self.rows.lock().unwrap().get(&id.uuid()).cloned())
        }
        async fn find_principal(&self, id: &PrincipalId) -> Result<Option<Principal>, RepositoryError> {
            Ok(self.rows.lock().unwrap().get(&id.uuid()).map(|(p, _)| p.clone()))
        }
    }

    // `CreateUser<R, ..>` takes `R` by value, but the tests below pass `&repo` so they can
    // still call `repo.find_user(..)` after `execute` — there's no blanket
    // `impl<T: PrincipalRepository> PrincipalRepository for &T` upstream in
    // `paigasus-iam-core`, so this local fake forwards the port for its own reference type.
    #[async_trait]
    impl PrincipalRepository for &InMemoryPrincipalRepository {
        async fn create_user(&self, p: &Principal, u: &User) -> Result<(), RepositoryError> {
            InMemoryPrincipalRepository::create_user(self, p, u).await
        }
        async fn create_user_in(&self, tx: &dyn Transaction, p: &Principal, u: &User) -> Result<(), RepositoryError> {
            InMemoryPrincipalRepository::create_user_in(self, tx, p, u).await
        }
        async fn find_user(&self, id: &PrincipalId) -> Result<Option<(Principal, User)>, RepositoryError> {
            InMemoryPrincipalRepository::find_user(self, id).await
        }
        async fn find_principal(&self, id: &PrincipalId) -> Result<Option<Principal>, RepositoryError> {
            InMemoryPrincipalRepository::find_principal(self, id).await
        }
    }

    struct FixedIdGenerator(Uuid);
    impl IdGenerator for FixedIdGenerator {
        fn new_principal_id(&self) -> PrincipalId {
            PrincipalId::from_prn(Prn::build("iam", "", None, "principal", self.0).unwrap())
        }
        fn new_organization_id(&self) -> OrganizationId {
            OrganizationId::from_uuid(self.0)
        }
        fn new_team_id(&self, org: Uuid) -> TeamId {
            TeamId::from_parts(org, self.0)
        }
        fn new_project_id(&self, org: Uuid) -> ProjectId {
            ProjectId::from_parts(org, self.0)
        }
        fn new_membership_id(&self) -> Uuid {
            self.0
        }
        fn new_external_identity_id(&self) -> Uuid {
            self.0
        }
        fn new_service_account_id(&self) -> PrincipalId {
            PrincipalId::from_prn(Prn::build("iam", "", None, "principal", self.0).unwrap())
        }
        fn new_api_key_id(&self) -> ApiKeyId {
            ApiKeyId::from_uuid(self.0)
        }
        fn new_audit_id(&self) -> Uuid {
            self.0
        }
        fn new_event_id(&self) -> Uuid {
            self.0
        }
        fn new_correlation_id(&self) -> Uuid {
            self.0
        }
    }

    struct FixedClock(DateTime<Utc>);
    impl Clock for FixedClock {
        fn now(&self) -> DateTime<Utc> {
            self.0
        }
    }

    /// Builds a `CreateUser` over `repo` (by reference, mirroring the pre-existing tests'
    /// posture) and a fresh, unshared `FakeUnitOfWork`, returning the `FakeOutbox` alongside it
    /// so a test can assert exactly what — and how many — events `execute` emitted (SMA-446
    /// Slice B Task B7, mirrors `application::roles::tests::new_service_with_fakes`).
    fn new_uc(repo: &InMemoryPrincipalRepository, uuid: Uuid, now: DateTime<Utc>) -> (CreateUser<&InMemoryPrincipalRepository, FixedIdGenerator, FixedClock>, FakeOutbox) {
        let outbox = FakeOutbox::default();
        let uc = CreateUser::new(CreateUserDeps {
            repo,
            uow: Arc::new(FakeUnitOfWork::default()),
            outbox: Arc::new(outbox.clone()),
            id_gen: FixedIdGenerator(uuid),
            clock: FixedClock(now),
        });
        (uc, outbox)
    }

    #[tokio::test]
    async fn create_user_persists_and_round_trips_through_the_port() {
        let uuid = Uuid::parse_str("0192f1c0-0000-7000-8000-000000000001").unwrap();
        let repo = InMemoryPrincipalRepository::default();
        let (uc, _outbox) = new_uc(&repo, uuid, Utc.timestamp_opt(1_700_000_000, 0).unwrap());

        let id = uc
            .execute(NewUser {
                email: "alice@example.com".into(),
                display_name: "Alice".into(),
                locale: None,
                timezone: None,
            })
            .await
            .unwrap();

        assert_eq!(id.uuid(), uuid);
        let (p, u) = repo.find_user(&id).await.unwrap().unwrap();
        assert_eq!(p.kind, PrincipalKind::User);
        assert_eq!(p.status, PrincipalStatus::Active);
        assert_eq!(u.email.as_str(), "alice@example.com");
    }

    #[tokio::test]
    async fn create_user_rejects_a_bad_email() {
        let repo = InMemoryPrincipalRepository::default();
        let (uc, outbox) = new_uc(&repo, Uuid::nil(), Utc.timestamp_opt(0, 0).unwrap());
        let err = uc
            .execute(NewUser {
                email: "nope".into(),
                display_name: "X".into(),
                locale: None,
                timezone: None,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, CreateUserError::InvalidEmail(_)));
        assert!(outbox.0.lock().unwrap().is_empty(), "an invalid-email rejection must never reach the UoW/outbox");
    }

    /// SMA-446 Slice B Task B7 — the UoW reference pattern's core contract, OUTBOX-ONLY (module
    /// docs): `execute` enqueues exactly one `iam.principal.created` `DomainEvent` atomically
    /// with the principal+user insert. `actor_prn` is `None` (`CreateUser` has no `actor`
    /// parameter — documented on `execute`), and the payload is PII-minimal: `principal_id` +
    /// `kind` only, never the email address.
    #[tokio::test]
    async fn create_user_emits_one_principal_created_event_atomically() {
        let uuid = Uuid::parse_str("0192f1c0-0000-7000-8000-000000000002").unwrap();
        let repo = InMemoryPrincipalRepository::default();
        let (uc, outbox) = new_uc(&repo, uuid, Utc.timestamp_opt(1_700_000_000, 0).unwrap());

        let id = uc
            .execute(NewUser {
                email: "bob@example.com".into(),
                display_name: "Bob".into(),
                locale: None,
                timezone: None,
            })
            .await
            .unwrap();

        let events = outbox.0.lock().unwrap();
        assert_eq!(events.len(), 1, "execute must enqueue exactly one domain event");
        assert_eq!(events[0].event_type, EventType::PrincipalCreated);
        assert_eq!(events[0].aggregate_prn, id.canonical());
        assert_eq!(events[0].actor_prn, None, "CreateUser has no actor — the event must carry actor_prn = None");
        assert_eq!(
            events[0].payload,
            serde_json::json!({"principal_id": id.uuid(), "kind": "user"}),
            "the payload must be PII-minimal: principal_id + kind only, never the email"
        );
    }

    /// SMA-446 Slice B Task B7: a duplicate-email conflict inside `create_user_in` must still
    /// surface as the pre-existing `Conflict(EmailTaken)` -> `EmailConflict` mapping — the
    /// whole unit of work rolls back before `Outbox::enqueue` is ever reached, so the rejected
    /// second create must not enqueue a second event.
    #[tokio::test]
    async fn create_user_duplicate_email_is_conflict_with_no_event() {
        let repo = InMemoryPrincipalRepository::default();
        let now = Utc.timestamp_opt(0, 0).unwrap();

        let (first, outbox) = new_uc(&repo, Uuid::from_u128(1), now);
        first
            .execute(NewUser {
                email: "dupe@example.com".into(),
                display_name: "First".into(),
                locale: None,
                timezone: None,
            })
            .await
            .unwrap();
        assert_eq!(outbox.0.lock().unwrap().len(), 1, "sanity: the first create enqueued its own event");

        // A second `CreateUser` sharing the SAME repo (so the email collides) AND the SAME
        // outbox (so this test can assert the second, rejected call enqueued nothing).
        let second = CreateUser::new(CreateUserDeps {
            repo: &repo,
            uow: Arc::new(FakeUnitOfWork::default()),
            outbox: Arc::new(outbox.clone()),
            id_gen: FixedIdGenerator(Uuid::from_u128(2)),
            clock: FixedClock(now),
        });
        let err = second
            .execute(NewUser {
                email: "dupe@example.com".into(),
                display_name: "Second".into(),
                locale: None,
                timezone: None,
            })
            .await
            .unwrap_err();
        assert!(
            matches!(err, CreateUserError::Repository(RepositoryError::Conflict(ConflictKind::EmailTaken))),
            "expected Conflict(EmailTaken), got {err:?}"
        );
        assert_eq!(TenancyError::from(err).code(), "email-conflict", "must still surface as the stable email-conflict code");

        assert_eq!(outbox.0.lock().unwrap().len(), 1, "a rolled-back duplicate-email create must not enqueue a second event");
    }
}
