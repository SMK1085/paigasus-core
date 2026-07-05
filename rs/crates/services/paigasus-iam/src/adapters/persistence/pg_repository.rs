// SPDX-License-Identifier: Apache-2.0

//! Postgres-backed `PrincipalRepository` (SeaORM). Maps domain <-> entity models and
//! backend errors into the core's `RepositoryError`.

use super::entities::{principal, user};
use async_trait::async_trait;
use paigasus_iam_core::{ConflictKind, Email, Principal, PrincipalId, PrincipalKind, PrincipalRepository, PrincipalStatus, RepositoryError, User};
use paigasus_kernel::Prn;
use sea_orm::{ActiveModelTrait, DatabaseConnection, DbErr, EntityTrait, Set, SqlErr, TransactionTrait};

pub struct PgPrincipalRepository {
    db: DatabaseConnection,
}

impl PgPrincipalRepository {
    #[must_use]
    pub fn new(db: DatabaseConnection) -> Self {
        PgPrincipalRepository { db }
    }
}

fn map_err(e: DbErr) -> RepositoryError {
    match e.sql_err() {
        Some(SqlErr::UniqueConstraintViolation(msg)) => RepositoryError::Conflict(conflict_kind(&msg)),
        Some(SqlErr::ForeignKeyConstraintViolation(_)) => RepositoryError::NotFound,
        _ => RepositoryError::Backend(Box::new(e)),
    }
}

/// D7: attribute conflicts by constraint name only; never surface PG text (e.g. the raw
/// message embeds `DETAIL: Key (email)=(...)` — PII).
pub(crate) fn conflict_kind(msg: &str) -> ConflictKind {
    const SLUG: [&str; 3] = ["uq_organization_slug", "uq_team_org_slug", "uq_project_team_slug"];
    if SLUG.iter().any(|c| msg.contains(c)) {
        ConflictKind::SlugTaken
    } else if msg.contains("uq_membership_") {
        ConflictKind::DuplicateMembership
    } else if msg.contains("user_email_key") {
        ConflictKind::EmailTaken
    } else {
        ConflictKind::Other // includes uq_*_prn: a UUIDv7 collision is an internal error
    }
}

#[async_trait]
impl PrincipalRepository for PgPrincipalRepository {
    async fn create_user(&self, p: &Principal, u: &User) -> Result<(), RepositoryError> {
        // Both inserts must commit-or-rollback together: a lone `principal` row left behind
        // by a failed `user` insert (e.g. duplicate email) would orphan permanently and turn
        // a caller's retry into a spurious `Conflict` on the principal insert.
        let txn = self.db.begin().await.map_err(map_err)?;

        let principal = principal::ActiveModel {
            id: Set(p.id.uuid()),
            prn: Set(p.id.canonical()),
            kind: Set(p.kind.as_str().to_string()),
            status: Set(p.status.as_str().to_string()),
            created_at: Set(p.created_at),
            updated_at: Set(p.updated_at),
        };
        principal.insert(&txn).await.map_err(map_err)?;

        let user = user::ActiveModel {
            principal_id: Set(u.principal_id.uuid()),
            email: Set(u.email.as_str().to_string()),
            display_name: Set(u.display_name.clone()),
            locale: Set(u.locale.clone()),
            timezone: Set(u.timezone.clone()),
            created_at: Set(u.created_at),
            updated_at: Set(u.updated_at),
        };
        user.insert(&txn).await.map_err(map_err)?;

        txn.commit().await.map_err(map_err)?;
        Ok(())
    }

    async fn find_user(&self, id: &PrincipalId) -> Result<Option<(Principal, User)>, RepositoryError> {
        let uuid = id.uuid();
        let Some(pm) = principal::Entity::find_by_id(uuid).one(&self.db).await.map_err(map_err)? else {
            return Ok(None);
        };
        let Some(um) = user::Entity::find_by_id(uuid).one(&self.db).await.map_err(map_err)? else {
            return Ok(None);
        };

        let prn = Prn::parse(&pm.prn).map_err(|e| RepositoryError::Backend(Box::new(std::io::Error::other(e.to_string()))))?;
        let pid = PrincipalId::from_prn(prn);
        let kind = PrincipalKind::parse(&pm.kind).ok_or_else(|| RepositoryError::Backend(Box::new(std::io::Error::other("bad kind"))))?;
        let status = PrincipalStatus::parse(&pm.status).ok_or_else(|| RepositoryError::Backend(Box::new(std::io::Error::other("bad status"))))?;
        let email = Email::parse(&um.email).map_err(|e| RepositoryError::Backend(Box::new(std::io::Error::other(format!("{e}")))))?;

        let principal = Principal::new(pid.clone(), kind, status, pm.created_at, pm.updated_at);
        let user = User::new(pid, email, um.display_name, um.locale, um.timezone, um.created_at, um.updated_at);
        Ok(Some((principal, user)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Fast, Docker-independent coverage of the constraint-name -> ConflictKind mapping;
    // the roundtrip test (tests/roundtrip.rs) additionally proves it end-to-end against
    // real Postgres, but only runs when Docker is available.
    #[test]
    fn conflict_kind_maps_constraint_names() {
        assert_eq!(conflict_kind("uq_organization_slug"), ConflictKind::SlugTaken);
        assert_eq!(conflict_kind("uq_team_org_slug"), ConflictKind::SlugTaken);
        assert_eq!(conflict_kind("uq_project_team_slug"), ConflictKind::SlugTaken);
        assert_eq!(conflict_kind("uq_membership_principal_node"), ConflictKind::DuplicateMembership);
        assert_eq!(conflict_kind("user_email_key"), ConflictKind::EmailTaken);
        assert_eq!(conflict_kind("uq_organization_prn"), ConflictKind::Other);
        assert_eq!(conflict_kind("some other constraint"), ConflictKind::Other);
    }
}
