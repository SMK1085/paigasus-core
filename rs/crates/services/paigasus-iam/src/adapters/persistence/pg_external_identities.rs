// SPDX-License-Identifier: Apache-2.0

//! Postgres-backed `ExternalIdentityRepository` (SeaORM). `provision` spans principal + user +
//! external_identity in one transaction (D9) — a lost race (duplicate issuer/subject) or an
//! email conflict must roll back everything, leaving no orphan principal/user row.

use super::entities::{external_identity, principal, user};
use super::map_err;
use async_trait::async_trait;
use paigasus_iam_core::{ExternalIdentity, ExternalIdentityRepository, Issuer, Principal, PrincipalId, RepositoryError, User};
use paigasus_kernel::Prn;
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set, TransactionTrait};

// `Clone` mirrors `PgPrincipalRepository` — the composition root holds a repo handle inside
// `#[derive(Clone)]` use cases; `DatabaseConnection` clones an `Arc`-backed pool handle.
#[derive(Clone)]
pub struct PgExternalIdentityRepository {
    db: DatabaseConnection,
}

impl PgExternalIdentityRepository {
    #[must_use]
    pub fn new(db: DatabaseConnection) -> Self {
        PgExternalIdentityRepository { db }
    }
}

#[async_trait]
impl ExternalIdentityRepository for PgExternalIdentityRepository {
    async fn find_by_issuer_subject(&self, issuer: &Issuer, subject: &str) -> Result<Option<ExternalIdentity>, RepositoryError> {
        let Some(im) = external_identity::Entity::find()
            .filter(external_identity::Column::Issuer.eq(issuer.as_str()))
            .filter(external_identity::Column::Subject.eq(subject))
            .one(&self.db)
            .await
            .map_err(map_err)?
        else {
            return Ok(None);
        };

        // No join: reconstruct `PrincipalId` from the principal row's `prn` column via a
        // second query (the identity row itself stores only the bare uuid).
        let pm = principal::Entity::find_by_id(im.principal_id)
            .one(&self.db)
            .await
            .map_err(map_err)?
            .ok_or_else(|| RepositoryError::Backend(Box::new(std::io::Error::other("external_identity references a missing principal"))))?;

        let prn = Prn::parse(&pm.prn).map_err(|e| RepositoryError::Backend(Box::new(std::io::Error::other(e.to_string()))))?;
        let principal_id = PrincipalId::from_prn(prn);
        let issuer = Issuer::parse(&im.issuer).map_err(|e| RepositoryError::Backend(Box::new(std::io::Error::other(e.to_string()))))?;

        Ok(Some(ExternalIdentity {
            id: im.id,
            principal_id,
            issuer,
            subject: im.subject,
            created_at: im.created_at,
            updated_at: im.updated_at,
        }))
    }

    async fn provision(&self, p: &Principal, u: &User, identity: &ExternalIdentity) -> Result<(), RepositoryError> {
        // All three inserts must commit-or-rollback together (D9): a lost race on
        // (issuer, subject) or a colliding email must not leave an orphaned principal/user.
        let txn = self.db.begin().await.map_err(map_err)?;

        let principal_am = principal::ActiveModel {
            id: Set(p.id.uuid()),
            prn: Set(p.id.canonical()),
            kind: Set(p.kind.as_str().to_string()),
            status: Set(p.status.as_str().to_string()),
            created_at: Set(p.created_at),
            updated_at: Set(p.updated_at),
        };
        principal_am.insert(&txn).await.map_err(map_err)?;

        let user_am = user::ActiveModel {
            principal_id: Set(u.principal_id.uuid()),
            email: Set(u.email.as_str().to_string()),
            display_name: Set(u.display_name.clone()),
            locale: Set(u.locale.clone()),
            timezone: Set(u.timezone.clone()),
            created_at: Set(u.created_at),
            updated_at: Set(u.updated_at),
        };
        user_am.insert(&txn).await.map_err(map_err)?;

        let identity_am = external_identity::ActiveModel {
            id: Set(identity.id),
            principal_id: Set(identity.principal_id.uuid()),
            issuer: Set(identity.issuer.as_str().to_string()),
            subject: Set(identity.subject.clone()),
            created_at: Set(identity.created_at),
            updated_at: Set(identity.updated_at),
        };
        identity_am.insert(&txn).await.map_err(map_err)?;

        txn.commit().await.map_err(map_err)?;
        Ok(())
    }
}
