// SPDX-License-Identifier: Apache-2.0

//! Postgres-backed `IdentityLinkStore` (SMA-712, SeaORM). Every `_in` method runs on the
//! caller's own transaction, recovered through `uow::recover_txn`. Every read that decides an
//! audit value or a no-op runs inside that transaction, under a row lock:
//!
//! - `lock_user_in` locks the `"user"` row FOR SHARE. This makes a link or an unlink run in
//!   series with a concurrent `change_email_in` (FOR UPDATE) of the same user.
//! - `unlink_in` locks the identity row FOR UPDATE, then deletes it. The returned row is the
//!   deleted row, so the audit names what was removed, not what the request said.
//! - `change_email_in` locks the `"user"` row FOR UPDATE before it compares, so two changes to
//!   one user serialize and each reads the value the other committed.
//!
//! The unique constraints `user_email_key` and `uq_external_identity_issuer_subject` report
//! conflicts through the shared `map_err`. No migration is necessary.

use super::entities::{external_identity, principal, user};
use super::map_err;
use super::uow::recover_txn;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use paigasus_iam_core::{Email, EmailChange, ExternalIdentity, IdentityLinkStore, Issuer, Mutated, PrincipalId, PrincipalStatus, RepositoryError, Transaction, User, UserWithIdentities};
use paigasus_kernel::Prn;
use sea_orm::{ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, IntoActiveModel, QueryFilter, QueryOrder, QuerySelect, Set};
use uuid::Uuid;

// `Clone` mirrors the other Pg adapters: `DatabaseConnection` is an `Arc`-backed pool handle.
#[derive(Clone)]
pub struct PgIdentityLinkStore {
    db: DatabaseConnection,
}

impl PgIdentityLinkStore {
    #[must_use]
    pub fn new(db: DatabaseConnection) -> Self {
        PgIdentityLinkStore { db }
    }
}

/// A backend error with a static message. It never carries a stored email or subject.
fn backend(message: &'static str) -> RepositoryError {
    RepositoryError::Backend(Box::new(std::io::Error::other(message)))
}

/// A user principal's id from its bare uuid. A principal PRN is `prn:pgs:iam:::principal/<uuid>`.
fn principal_id_of(uuid: Uuid) -> Result<PrincipalId, RepositoryError> {
    Prn::build("iam", "", None, "principal", uuid)
        .map(PrincipalId::from_prn)
        .map_err(|_| backend("a stored principal id does not build a prn"))
}

fn to_user(id: &PrincipalId, m: user::Model) -> Result<User, RepositoryError> {
    let email = Email::parse(&m.email).map_err(|_| backend("a stored user email does not parse"))?;
    Ok(User::new(id.clone(), email, m.display_name, m.locale, m.timezone, m.created_at, m.updated_at))
}

fn to_identity(id: &PrincipalId, m: external_identity::Model) -> Result<ExternalIdentity, RepositoryError> {
    let issuer = Issuer::parse(&m.issuer).map_err(|_| backend("a stored issuer does not parse"))?;
    Ok(ExternalIdentity {
        id: m.id,
        principal_id: id.clone(),
        issuer,
        subject: m.subject,
        created_at: m.created_at,
        updated_at: m.updated_at,
    })
}

/// The user, its principal status, and its identities in `(created_at, id)` order.
async fn load_view<C: ConnectionTrait>(conn: &C, um: user::Model) -> Result<UserWithIdentities, RepositoryError> {
    let id = principal_id_of(um.principal_id)?;
    let pm = principal::Entity::find_by_id(um.principal_id)
        .one(conn)
        .await
        .map_err(map_err)?
        .ok_or_else(|| backend("a user row references a missing principal"))?;
    let status = PrincipalStatus::parse(&pm.status).ok_or_else(|| backend("a stored principal status is unknown"))?;
    let rows = external_identity::Entity::find()
        .filter(external_identity::Column::PrincipalId.eq(um.principal_id))
        .order_by_asc(external_identity::Column::CreatedAt)
        .order_by_asc(external_identity::Column::Id)
        .all(conn)
        .await
        .map_err(map_err)?;
    let identities = rows.into_iter().map(|m| to_identity(&id, m)).collect::<Result<Vec<_>, _>>()?;
    Ok(UserWithIdentities {
        user: to_user(&id, um)?,
        status,
        identities,
    })
}

#[async_trait]
impl IdentityLinkStore for PgIdentityLinkStore {
    async fn find_user_by_email(&self, email: &Email) -> Result<Option<UserWithIdentities>, RepositoryError> {
        let Some(um) = user::Entity::find().filter(user::Column::Email.eq(email.as_str())).one(&self.db).await.map_err(map_err)? else {
            return Ok(None);
        };
        load_view(&self.db, um).await.map(Some)
    }

    async fn lock_user_in(&self, tx: &dyn Transaction, id: &PrincipalId) -> Result<Option<User>, RepositoryError> {
        let txn = recover_txn(tx)?;
        let Some(um) = user::Entity::find_by_id(id.uuid()).lock_shared().one(txn).await.map_err(map_err)? else {
            return Ok(None);
        };
        to_user(id, um).map(Some)
    }

    async fn find_identity_in(&self, tx: &dyn Transaction, issuer: &Issuer, subject: &str) -> Result<Option<ExternalIdentity>, RepositoryError> {
        let txn = recover_txn(tx)?;
        let Some(m) = external_identity::Entity::find()
            .filter(external_identity::Column::Issuer.eq(issuer.as_str()))
            .filter(external_identity::Column::Subject.eq(subject))
            .one(txn)
            .await
            .map_err(map_err)?
        else {
            return Ok(None);
        };
        let owner = principal_id_of(m.principal_id)?;
        to_identity(&owner, m).map(Some)
    }

    async fn link_in(&self, tx: &dyn Transaction, identity: &ExternalIdentity) -> Result<(), RepositoryError> {
        let txn = recover_txn(tx)?;
        external_identity::ActiveModel {
            id: Set(identity.id),
            principal_id: Set(identity.principal_id.uuid()),
            issuer: Set(identity.issuer.as_str().to_string()),
            subject: Set(identity.subject.clone()),
            created_at: Set(identity.created_at),
            updated_at: Set(identity.updated_at),
        }
        .insert(txn)
        .await
        .map_err(map_err)?;
        Ok(())
    }

    async fn unlink_in(&self, tx: &dyn Transaction, user: &PrincipalId, identity_id: Uuid) -> Result<Option<ExternalIdentity>, RepositoryError> {
        let txn = recover_txn(tx)?;
        let Some(m) = external_identity::Entity::find_by_id(identity_id)
            .filter(external_identity::Column::PrincipalId.eq(user.uuid()))
            .lock_exclusive()
            .one(txn)
            .await
            .map_err(map_err)?
        else {
            return Ok(None);
        };
        external_identity::Entity::delete_by_id(identity_id).exec(txn).await.map_err(map_err)?;
        to_identity(user, m).map(Some)
    }

    async fn change_email_in(&self, tx: &dyn Transaction, user: &PrincipalId, email: &Email, now: DateTime<Utc>) -> Result<Option<Mutated<EmailChange>>, RepositoryError> {
        let txn = recover_txn(tx)?;
        let Some(um) = user::Entity::find_by_id(user.uuid()).lock_exclusive().one(txn).await.map_err(map_err)? else {
            return Ok(None);
        };
        let old = Email::parse(&um.email).map_err(|_| backend("a stored user email does not parse"))?;
        if old == *email {
            // SMA-606 D1: a write that changes nothing stamps nothing.
            return Ok(Some(Mutated {
                value: EmailChange { old: old.clone(), new: old },
                changed: false,
            }));
        }
        let mut active = um.into_active_model();
        active.email = Set(email.as_str().to_string());
        active.updated_at = Set(now);
        active.update(txn).await.map_err(map_err)?;
        Ok(Some(Mutated {
            value: EmailChange { old, new: email.clone() },
            changed: true,
        }))
    }

    async fn user_view_in(&self, tx: &dyn Transaction, id: &PrincipalId) -> Result<Option<UserWithIdentities>, RepositoryError> {
        let txn = recover_txn(tx)?;
        let Some(um) = user::Entity::find_by_id(id.uuid()).one(txn).await.map_err(map_err)? else {
            return Ok(None);
        };
        load_view(txn, um).await.map(Some)
    }
}
