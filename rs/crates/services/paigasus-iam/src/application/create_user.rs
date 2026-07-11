// SPDX-License-Identifier: Apache-2.0

//! `CreateUser` use case: mint an identity, build a `User` principal, persist it.

use paigasus_iam_core::{Clock, Email, IdGenerator, Principal, PrincipalId, PrincipalKind, PrincipalRepository, PrincipalStatus, RepositoryError, User};

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
// inside its own `#[derive(Clone)] AppState`, mirroring the tenancy services' shape.
#[derive(Clone)]
pub struct CreateUser<R, I, C> {
    repo: R,
    id_gen: I,
    clock: C,
}

impl<R, I, C> CreateUser<R, I, C>
where
    R: PrincipalRepository,
    I: IdGenerator,
    C: Clock,
{
    pub fn new(repo: R, id_gen: I, clock: C) -> Self {
        CreateUser { repo, id_gen, clock }
    }

    pub async fn execute(&self, cmd: NewUser) -> Result<PrincipalId, CreateUserError> {
        let email = Email::parse(&cmd.email).map_err(|_| CreateUserError::InvalidEmail(cmd.email.clone()))?;
        let id = self.id_gen.new_principal_id();
        let now = self.clock.now();

        let principal = Principal::new(id.clone(), PrincipalKind::User, PrincipalStatus::Active, now, now);
        let user = User::new(id.clone(), email, cmd.display_name, cmd.locale, cmd.timezone, now, now);

        self.repo.create_user(&principal, &user).await?;
        Ok(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use chrono::{DateTime, TimeZone, Utc};
    use paigasus_iam_core::{ApiKeyId, ConflictKind, OrganizationId, ProjectId, TeamId};
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
            rows.insert(p.id.uuid(), (p.clone(), u.clone()));
            Ok(())
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
    }

    struct FixedClock(DateTime<Utc>);
    impl Clock for FixedClock {
        fn now(&self) -> DateTime<Utc> {
            self.0
        }
    }

    #[tokio::test]
    async fn create_user_persists_and_round_trips_through_the_port() {
        let uuid = Uuid::parse_str("0192f1c0-0000-7000-8000-000000000001").unwrap();
        let clock = FixedClock(Utc.timestamp_opt(1_700_000_000, 0).unwrap());
        let repo = InMemoryPrincipalRepository::default();
        let uc = CreateUser::new(&repo, FixedIdGenerator(uuid), clock);

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
        let uc = CreateUser::new(&repo, FixedIdGenerator(Uuid::nil()), FixedClock(Utc.timestamp_opt(0, 0).unwrap()));
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
    }
}
