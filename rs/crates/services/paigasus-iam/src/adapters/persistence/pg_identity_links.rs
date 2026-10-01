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
//! **The stored-PRN rule (SMA-649, code review F1 and F13).** `lock_user_in` and
//! `change_email_in` load the `principal` row by uuid and compare its stored `prn` byte for byte
//! with the caller's `PrincipalId::canonical()`, the same as `pg_memberships`'s
//! `principal_list_uuid`. An absent principal is `None`, a different PRN is `PrnMismatch`. Every
//! `PrincipalId` this store returns is built from the stored `principal.prn`.
//!
//! **A stored email that does not parse (code review F6).** `lock_user_in` does not read the
//! email, and `change_email_in` returns the raw old value. So `ChangeUserEmail` can repair a
//! hand-edited row. The two reads that return a whole user view (`find_user_by_email`,
//! `user_view_in`) still parse it: `find_user_by_email` finds a row by an email that already
//! parsed, and `user_view_in` runs after the repair has written a valid email.
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

/// The `PrincipalId` of a stored principal row, built from its stored `prn` column.
fn stored_id(pm: &principal::Model) -> Result<PrincipalId, RepositoryError> {
    Prn::parse(&pm.prn).map(PrincipalId::from_prn).map_err(|_| backend("a stored principal prn does not parse"))
}

/// The stored principal row for a uuid that a stored row references (a foreign key), so an
/// absent row is a broken invariant, not a caller error.
async fn referenced_principal<C: ConnectionTrait>(conn: &C, uuid: Uuid) -> Result<principal::Model, RepositoryError> {
    principal::Entity::find_by_id(uuid)
        .one(conn)
        .await
        .map_err(map_err)?
        .ok_or_else(|| backend("a stored row references a missing principal"))
}

/// The stored-PRN rule (module doc). `Ok(None)` when no principal has this uuid,
/// `Err(PrnMismatch)` when the stored PRN differs from the caller's canonical PRN, else the
/// stored `PrincipalId`. No lock: a principal's `prn` is written once, at insert, and never
/// changes, so a comparison outside the row lock is sound (the rule `grpc::tenancy` records).
async fn confirm_principal<C: ConnectionTrait>(conn: &C, id: &PrincipalId) -> Result<Option<PrincipalId>, RepositoryError> {
    let Some(pm) = principal::Entity::find_by_id(id.uuid()).one(conn).await.map_err(map_err)? else {
        return Ok(None);
    };
    if pm.prn != id.canonical() {
        return Err(RepositoryError::PrnMismatch);
    }
    stored_id(&pm).map(Some)
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
    let pm = referenced_principal(conn, um.principal_id).await?;
    let id = stored_id(&pm)?;
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

    async fn lock_user_in(&self, tx: &dyn Transaction, id: &PrincipalId) -> Result<Option<PrincipalId>, RepositoryError> {
        let txn = recover_txn(tx)?;
        let Some(stored) = confirm_principal(txn, id).await? else {
            return Ok(None);
        };
        // Existence and the lock only: the email is not read, so a bad stored email cannot block
        // a link, an unlink or the repair (code review F6).
        let locked = user::Entity::find_by_id(stored.uuid()).lock_shared().one(txn).await.map_err(map_err)?;
        Ok(locked.map(|_| stored))
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
        let owner = stored_id(&referenced_principal(txn, m.principal_id).await?)?;
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
        let Some(stored) = confirm_principal(txn, user).await? else {
            return Ok(None);
        };
        let Some(um) = user::Entity::find_by_id(stored.uuid()).lock_exclusive().one(txn).await.map_err(map_err)? else {
            return Ok(None);
        };
        // The raw stored value. It need not parse, so a bad row can be repaired (code review F6).
        let old = um.email.clone();
        if old == email.as_str() {
            // SMA-606 D1: a write that changes nothing stamps nothing.
            return Ok(Some(Mutated {
                value: EmailChange { old, new: email.clone() },
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
