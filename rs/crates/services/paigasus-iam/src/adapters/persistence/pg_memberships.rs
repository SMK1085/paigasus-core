// SPDX-License-Identifier: Apache-2.0

//! Postgres-backed `MembershipRepository` (SeaORM). The most safety-critical persistence
//! adapter in the tenancy slice: `attach` runs every guard (principal exists, node exists,
//! prn byte-match, effective status, org-membership invariant, duplicate) inside one
//! transaction with row locks (D8, port doc contract), so a concurrent archive/detach can
//! never race past them. `detach` cascades an org membership's removal onto the same
//! principal's team/project memberships in that org (rule 5), also in one transaction.

use super::entities::{membership, organization, principal, project, team};
use super::map_err;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use paigasus_iam_core::{Membership, MembershipRecord, MembershipRepository, NodeStatus, PreconditionKind, RepositoryError, TenancyNodeRef};
use sea_orm::{
    ActiveModelTrait, ActiveValue::NotSet, ColumnTrait, ConnectionTrait, DatabaseConnection, DbBackend, EntityTrait, FromQueryResult, QueryFilter, QuerySelect, Set, Statement, TransactionTrait,
};
use uuid::Uuid;

// `Clone` lets the composition root (`http::AppState::new`) hold a repo handle inside a
// `#[derive(Clone)] MembershipService` — cheap, `DatabaseConnection` clones an `Arc`-backed
// pool handle, not a connection.
#[derive(Clone)]
pub struct PgMembershipRepository {
    db: DatabaseConnection,
}

impl PgMembershipRepository {
    #[must_use]
    pub fn new(db: DatabaseConnection) -> Self {
        PgMembershipRepository { db }
    }
}

/// Parses a stored `status` column value — shared across org/team/project ancestor lookups
/// (mirrors `pg_teams.rs`/`pg_projects.rs`'s helper of the same name): a corrupt value is a
/// `Backend` error, never a silent default.
fn parse_status(raw: &str) -> Result<NodeStatus, RepositoryError> {
    NodeStatus::parse(raw).ok_or_else(|| RepositoryError::Backend(Box::new(std::io::Error::other(format!("bad node status: {raw}")))))
}

/// A missing ancestor row for an existing child is a data-integrity break (the FK should
/// prevent it) — surfaced as `Backend`, never as a silent `NotFound` on the child's own read.
fn missing_ancestor(kind: &str, ancestor_id: Uuid, child_kind: &str, child_id: Uuid) -> RepositoryError {
    RepositoryError::Backend(Box::new(std::io::Error::other(format!("{kind} {ancestor_id} missing for {child_kind} {child_id}"))))
}

/// Row shape shared by every raw-SQL membership listing (`find`/`list_by_principal`/
/// `list_by_node`): each variant of the union joins `membership` to its own target's table
/// for that node's prn (D5: memberships carry no prn of their own, only the FK uuid).
#[derive(Debug, Clone, FromQueryResult)]
struct MembershipRow {
    id: Uuid,
    principal_prn: String,
    node_prn: String,
    created_at: DateTime<Utc>,
}

impl From<MembershipRow> for MembershipRecord {
    fn from(row: MembershipRow) -> Self {
        MembershipRecord {
            id: row.id,
            principal_prn: row.principal_prn,
            node_prn: row.node_prn,
            created_at: row.created_at,
        }
    }
}

/// `find`'s UNION-ALL shape (binding SQL), filtered by `m.id = $1`: at most one row can match
/// since `membership.id` is the primary key.
const FIND_SQL: &str = r#"
SELECT m.id, pr.prn AS principal_prn, o.prn AS node_prn, m.created_at
  FROM "membership" m JOIN "principal" pr ON pr.id = m.principal_id
  JOIN "organization" o ON o.id = m.org_id
 WHERE m.id = $1
UNION ALL
SELECT m.id, pr.prn, t.prn, m.created_at FROM "membership" m
  JOIN "principal" pr ON pr.id = m.principal_id JOIN "team" t ON t.id = m.team_id
 WHERE m.id = $1
UNION ALL
SELECT m.id, pr.prn, pj.prn, m.created_at FROM "membership" m
  JOIN "principal" pr ON pr.id = m.principal_id JOIN "project" pj ON pj.id = m.project_id
 WHERE m.id = $1"#;

/// `list_by_principal`'s UNION-ALL shape (binding SQL), `ORDER BY created_at, id` (rule 9)
/// with `LIMIT`/`OFFSET` bind params.
const LIST_BY_PRINCIPAL_SQL: &str = r#"
SELECT m.id, pr.prn AS principal_prn, o.prn AS node_prn, m.created_at
  FROM "membership" m JOIN "principal" pr ON pr.id = m.principal_id
  JOIN "organization" o ON o.id = m.org_id
 WHERE m.principal_id = $1
UNION ALL
SELECT m.id, pr.prn, t.prn, m.created_at FROM "membership" m
  JOIN "principal" pr ON pr.id = m.principal_id JOIN "team" t ON t.id = m.team_id
 WHERE m.principal_id = $1
UNION ALL
SELECT m.id, pr.prn, pj.prn, m.created_at FROM "membership" m
  JOIN "principal" pr ON pr.id = m.principal_id JOIN "project" pj ON pj.id = m.project_id
 WHERE m.principal_id = $1
ORDER BY created_at, id LIMIT $2 OFFSET $3"#;

/// `list_by_node`'s single-target-table shape (no UNION needed — the node kind is already
/// known from the resolved ref), same ordering/pagination as `list_by_principal`.
const LIST_BY_ORG_SQL: &str = r#"
SELECT m.id, pr.prn AS principal_prn, o.prn AS node_prn, m.created_at
  FROM "membership" m JOIN "principal" pr ON pr.id = m.principal_id
  JOIN "organization" o ON o.id = m.org_id
 WHERE m.org_id = $1
 ORDER BY m.created_at, m.id LIMIT $2 OFFSET $3"#;

const LIST_BY_TEAM_SQL: &str = r#"
SELECT m.id, pr.prn AS principal_prn, t.prn AS node_prn, m.created_at
  FROM "membership" m JOIN "principal" pr ON pr.id = m.principal_id
  JOIN "team" t ON t.id = m.team_id
 WHERE m.team_id = $1
 ORDER BY m.created_at, m.id LIMIT $2 OFFSET $3"#;

const LIST_BY_PROJECT_SQL: &str = r#"
SELECT m.id, pr.prn AS principal_prn, pj.prn AS node_prn, m.created_at
  FROM "membership" m JOIN "principal" pr ON pr.id = m.principal_id
  JOIN "project" pj ON pj.id = m.project_id
 WHERE m.project_id = $1
 ORDER BY m.created_at, m.id LIMIT $2 OFFSET $3"#;

/// `detach`'s cascade delete (binding SQL): removing an org membership also removes that
/// principal's team/project memberships scoped to that org (rule 5), in the same transaction
/// as the org row's own delete. NULL `team_id`/`project_id` columns (every non-matching row)
/// never satisfy `IN (...)`, so this can never touch the org row itself.
const DETACH_CASCADE_SQL: &str = r#"DELETE FROM "membership" m
 WHERE m.principal_id = $1
   AND (m.team_id    IN (SELECT id FROM "team"    WHERE org_id = $2)
     OR m.project_id IN (SELECT id FROM "project" WHERE org_id = $2))"#;

#[async_trait]
impl MembershipRepository for PgMembershipRepository {
    async fn attach(&self, membership: &Membership) -> Result<MembershipRecord, RepositoryError> {
        let txn = self.db.begin().await.map_err(map_err)?;
        let principal_uuid = membership.principal_id.uuid();

        // 1. Principal exists (no lock — principals aren't archived in M1; the FK backstops
        // referential integrity), and the caller's prn byte-matches the stored one.
        let Some(principal_model) = principal::Entity::find_by_id(principal_uuid).one(&txn).await.map_err(map_err)? else {
            return Err(RepositoryError::NotFound);
        };
        if principal_model.prn != membership.principal_id.canonical() {
            return Err(RepositoryError::PrnMismatch);
        }
        let principal_prn = principal_model.prn;

        // 2 & 3. Node row (`lock_shared`) exists and byte-matches (the forged-org-slot
        // defense: org always comes from the persisted row, never the caller's prn); its
        // ancestors (`lock_shared`) fold into the effective status (D1/D10). `parent_org` is
        // `Some` for team/project targets — step 4's org-membership invariant is scoped to it.
        let (node_prn, effective_status, parent_org) = match &membership.node {
            TenancyNodeRef::Organization(node_id) => {
                let Some(org) = organization::Entity::find_by_id(node_id.uuid()).lock_shared().one(&txn).await.map_err(map_err)? else {
                    return Err(RepositoryError::NotFound);
                };
                if org.prn != membership.node.canonical() {
                    return Err(RepositoryError::PrnMismatch);
                }
                let status = parse_status(&org.status)?;
                (org.prn, NodeStatus::effective(status, &[]), None)
            }
            TenancyNodeRef::Team(node_id) => {
                let Some(team_model) = team::Entity::find_by_id(node_id.uuid()).lock_shared().one(&txn).await.map_err(map_err)? else {
                    return Err(RepositoryError::NotFound);
                };
                if team_model.prn != membership.node.canonical() {
                    return Err(RepositoryError::PrnMismatch);
                }
                let Some(org) = organization::Entity::find_by_id(team_model.org_id).lock_shared().one(&txn).await.map_err(map_err)? else {
                    return Err(missing_ancestor("organization", team_model.org_id, "team", team_model.id));
                };
                let team_status = parse_status(&team_model.status)?;
                let org_status = parse_status(&org.status)?;
                (team_model.prn, NodeStatus::effective(team_status, &[org_status]), Some(team_model.org_id))
            }
            TenancyNodeRef::Project(node_id) => {
                let Some(project_model) = project::Entity::find_by_id(node_id.uuid()).lock_shared().one(&txn).await.map_err(map_err)? else {
                    return Err(RepositoryError::NotFound);
                };
                if project_model.prn != membership.node.canonical() {
                    return Err(RepositoryError::PrnMismatch);
                }
                let Some(team_model) = team::Entity::find_by_id(project_model.team_id).lock_shared().one(&txn).await.map_err(map_err)? else {
                    return Err(missing_ancestor("team", project_model.team_id, "project", project_model.id));
                };
                let Some(org) = organization::Entity::find_by_id(project_model.org_id).lock_shared().one(&txn).await.map_err(map_err)? else {
                    return Err(missing_ancestor("organization", project_model.org_id, "project", project_model.id));
                };
                let project_status = parse_status(&project_model.status)?;
                let team_status = parse_status(&team_model.status)?;
                let org_status = parse_status(&org.status)?;
                (project_model.prn, NodeStatus::effective(project_status, &[team_status, org_status]), Some(project_model.org_id))
            }
        };

        if effective_status == NodeStatus::Archived {
            return Err(RepositoryError::Precondition(PreconditionKind::NodeArchived));
        }

        // 4. Team/project targets require an existing org membership for the node's
        // PERSISTED org, locked so a concurrent detach can't race this check.
        if let Some(org_uuid) = parent_org {
            let has_org_membership = membership::Entity::find()
                .filter(membership::Column::PrincipalId.eq(principal_uuid))
                .filter(membership::Column::OrgId.eq(org_uuid))
                .lock_shared()
                .one(&txn)
                .await
                .map_err(map_err)?
                .is_some();
            if !has_org_membership {
                return Err(RepositoryError::Precondition(PreconditionKind::MissingOrgMembership));
            }
        }

        // 5. Insert exactly one target column; a `uq_membership_*` violation maps to
        // `Conflict(DuplicateMembership)` (mod.rs's `conflict_kind`).
        let (org_id, team_id, project_id) = match &membership.node {
            TenancyNodeRef::Organization(id) => (Some(id.uuid()), None, None),
            TenancyNodeRef::Team(id) => (None, Some(id.uuid()), None),
            TenancyNodeRef::Project(id) => (None, None, Some(id.uuid())),
        };
        let active = membership::ActiveModel {
            id: Set(membership.id),
            principal_id: Set(principal_uuid),
            org_id: Set(org_id),
            team_id: Set(team_id),
            project_id: Set(project_id),
            created_at: Set(membership.created_at),
            // `created_by` is Task 5's job (SMA-440) — the domain/port surface for membership
            // actor stamping isn't wired up yet, so the column is left NotSet and the DB
            // default (NULL) applies, same as any pre-migration row.
            created_by: NotSet,
        };
        active.insert(&txn).await.map_err(map_err)?;

        txn.commit().await.map_err(map_err)?;

        Ok(MembershipRecord {
            id: membership.id,
            principal_prn,
            node_prn,
            created_at: membership.created_at,
        })
    }

    async fn find(&self, id: Uuid) -> Result<Option<MembershipRecord>, RepositoryError> {
        let stmt = Statement::from_sql_and_values(DbBackend::Postgres, FIND_SQL, [id.into()]);
        let row = MembershipRow::find_by_statement(stmt).one(&self.db).await.map_err(map_err)?;
        Ok(row.map(MembershipRecord::from))
    }

    async fn detach(&self, id: Uuid) -> Result<(), RepositoryError> {
        let txn = self.db.begin().await.map_err(map_err)?;

        let Some(model) = membership::Entity::find_by_id(id).lock_exclusive().one(&txn).await.map_err(map_err)? else {
            return Err(RepositoryError::NotFound);
        };

        // An org membership cascades onto the same principal's team/project memberships in
        // that org (rule 5) before the org row itself is deleted below.
        if let Some(org_id) = model.org_id {
            let stmt = Statement::from_sql_and_values(DbBackend::Postgres, DETACH_CASCADE_SQL, [model.principal_id.into(), org_id.into()]);
            txn.execute(stmt).await.map_err(map_err)?;
        }

        membership::Entity::delete_by_id(id).exec(&txn).await.map_err(map_err)?;

        txn.commit().await.map_err(map_err)?;
        Ok(())
    }

    async fn list_by_principal(&self, principal: Uuid, limit: u64, offset: u64) -> Result<Vec<MembershipRecord>, RepositoryError> {
        let stmt = Statement::from_sql_and_values(DbBackend::Postgres, LIST_BY_PRINCIPAL_SQL, [principal.into(), limit.into(), offset.into()]);
        let rows = MembershipRow::find_by_statement(stmt).all(&self.db).await.map_err(map_err)?;
        Ok(rows.into_iter().map(MembershipRecord::from).collect())
    }

    async fn list_by_node(&self, node: &TenancyNodeRef, limit: u64, offset: u64) -> Result<Vec<MembershipRecord>, RepositoryError> {
        // Read path: resolves the node by uuid with no lock (unlike attach's step 2, this is
        // a plain listing, not a guarded mutation), but the same NotFound/PrnMismatch guard.
        let (sql, node_uuid): (&str, Uuid) = match node {
            TenancyNodeRef::Organization(id) => {
                let Some(org) = organization::Entity::find_by_id(id.uuid()).one(&self.db).await.map_err(map_err)? else {
                    return Err(RepositoryError::NotFound);
                };
                if org.prn != node.canonical() {
                    return Err(RepositoryError::PrnMismatch);
                }
                (LIST_BY_ORG_SQL, id.uuid())
            }
            TenancyNodeRef::Team(id) => {
                let Some(team_model) = team::Entity::find_by_id(id.uuid()).one(&self.db).await.map_err(map_err)? else {
                    return Err(RepositoryError::NotFound);
                };
                if team_model.prn != node.canonical() {
                    return Err(RepositoryError::PrnMismatch);
                }
                (LIST_BY_TEAM_SQL, id.uuid())
            }
            TenancyNodeRef::Project(id) => {
                let Some(project_model) = project::Entity::find_by_id(id.uuid()).one(&self.db).await.map_err(map_err)? else {
                    return Err(RepositoryError::NotFound);
                };
                if project_model.prn != node.canonical() {
                    return Err(RepositoryError::PrnMismatch);
                }
                (LIST_BY_PROJECT_SQL, id.uuid())
            }
        };

        let stmt = Statement::from_sql_and_values(DbBackend::Postgres, sql, [node_uuid.into(), limit.into(), offset.into()]);
        let rows = MembershipRow::find_by_statement(stmt).all(&self.db).await.map_err(map_err)?;
        Ok(rows.into_iter().map(MembershipRecord::from).collect())
    }
}
