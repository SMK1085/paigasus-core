// SPDX-License-Identifier: Apache-2.0

//! Postgres-backed `ServiceAccountRepository` (SeaORM). `create` mirrors `pg_repository.rs::
//! create_user`'s two-insert-one-transaction pattern (principal row first, then the
//! `service_account` subtype row): a lone `principal` row left behind by a failed
//! `service_account` insert (e.g. a duplicate name for the owner) would orphan permanently
//! and turn a caller's retry into a spurious `Conflict` on the principal insert.
//! `set_principal_status` updates the LIFECYCLE status on the `principal` row, not
//! `service_account` — the table has no `status` column of its own (D16).
//!
//! **SMA-446, Slice B Task B7:** `create`/`set_principal_status` are now thin one-shot-
//! `UnitOfWork` wrappers around [`ServiceAccountRepository::create_in`]/[`ServiceAccountRepository::
//! set_principal_status_in`] — the exact reference pattern `PgRoleGrantStore::grant`/`grant_in`
//! establish (B4): open a `SeaOrmTransaction`, drive the write on it, commit. Kept for callers
//! that don't (yet) drive their own `UnitOfWork` (`tests/service_accounts.rs`'s Docker
//! integration coverage calls both directly); `create_in`/`set_principal_status_in` are the
//! txn-scoped primitives `ServiceAccountService::create`/`archive` (the reference pattern)
//! actually drive.
//!
//! **`find`/`list_by_owner` also read the principal's status** (CodeRabbit finding on the
//! SMA-445 PR: the read paths need to answer "is this SA active or disabled" without a second,
//! out-of-band caller-side query). Both use `find_also_related` (a single LEFT JOIN against
//! `principal`, via the `Related` impl on the `service_account` entity) rather than a per-row
//! second query — `list_by_owner` in particular would otherwise be N+1.

use super::entities::{principal, project, service_account, team};
use super::map_err;
use super::uow::{SeaOrmTransaction, recover_txn};
use async_trait::async_trait;
use paigasus_iam_core::{
    OrganizationId, Principal, PrincipalId, PrincipalStatus, ProjectId, RepositoryError, ServiceAccount, ServiceAccountRecord, ServiceAccountRepository, TeamId, TenancyNodeRef, Transaction,
};
use paigasus_kernel::Prn;
use sea_orm::{ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, IntoActiveModel, QueryFilter, QueryOrder, QuerySelect, Set, TransactionTrait};
use uuid::Uuid;

// `Clone` lets the composition root hold a repo handle inside a `#[derive(Clone)]` service —
// cheap, `DatabaseConnection` clones an `Arc`-backed pool handle, not a connection (mirrors
// `PgPrincipalRepository`/`PgOrganizationRepository`'s precedent).
#[derive(Clone)]
pub struct PgServiceAccountRepository {
    db: DatabaseConnection,
}

impl PgServiceAccountRepository {
    #[must_use]
    pub fn new(db: DatabaseConnection) -> Self {
        PgServiceAccountRepository { db }
    }
}

/// Splits a [`TenancyNodeRef`] owner into the row's (at most one non-null)
/// `owner_org_id`/`owner_team_id`/`owner_project_id` columns — the write-side half of
/// [`owner_from_columns`], mirrors `pg_role_grants.rs::scope_columns`'s column-split shape
/// (no `Root` arm here: a service account's owner is always a concrete tenancy node).
fn owner_columns(owner: &TenancyNodeRef) -> (Option<Uuid>, Option<Uuid>, Option<Uuid>) {
    match owner {
        TenancyNodeRef::Organization(id) => (Some(id.uuid()), None, None),
        TenancyNodeRef::Team(id) => (None, Some(id.uuid()), None),
        TenancyNodeRef::Project(id) => (None, None, Some(id.uuid())),
    }
}

/// A missing owner row for an existing service_account is a data-integrity break (the FK
/// should prevent it) — surfaced as `Backend`, never a silent default (mirrors
/// `pg_projects.rs::missing_ancestor`'s posture).
fn missing_owner(kind: &str, owner_id: Uuid, sa_id: Uuid) -> RepositoryError {
    RepositoryError::Backend(Box::new(std::io::Error::other(format!("{kind} {owner_id} missing for service_account {sa_id}"))))
}

/// Reconstructs the domain `TenancyNodeRef` for a stored `service_account` row's owner column
/// triple (exactly one set, `ck_service_account_owner`). An organization owner needs no
/// further lookup — `OrganizationId` carries no ancestor context of its own. A team/project
/// owner DOES: unlike `pg_role_grants.rs` (which stores the scope's full canonical PRN
/// alongside the id columns, so no join is ever needed to reconstruct it), `service_account`
/// only stores the bare uuid, so recovering the `TeamId`/`ProjectId`'s embedded org segment
/// needs one join back to the owning `team`/`project` row (mirrors `pg_projects.rs::
/// model_to_project`'s `TeamId::from_parts(model.org_id, ..)` read, one hop further since
/// `service_account` doesn't itself denormalize `org_id`).
async fn owner_from_columns<C: ConnectionTrait>(
    db: &C,
    sa_id: Uuid,
    owner_org_id: Option<Uuid>,
    owner_team_id: Option<Uuid>,
    owner_project_id: Option<Uuid>,
) -> Result<TenancyNodeRef, RepositoryError> {
    if let Some(id) = owner_org_id {
        return Ok(TenancyNodeRef::Organization(OrganizationId::from_uuid(id)));
    }
    if let Some(id) = owner_team_id {
        let Some(team_model) = team::Entity::find_by_id(id).one(db).await.map_err(map_err)? else {
            return Err(missing_owner("team", id, sa_id));
        };
        return Ok(TenancyNodeRef::Team(TeamId::from_parts(team_model.org_id, id)));
    }
    if let Some(id) = owner_project_id {
        let Some(project_model) = project::Entity::find_by_id(id).one(db).await.map_err(map_err)? else {
            return Err(missing_owner("project", id, sa_id));
        };
        return Ok(TenancyNodeRef::Project(ProjectId::from_parts(project_model.org_id, id)));
    }
    // Unreachable in practice: `ck_service_account_owner` guarantees exactly one column is
    // set. Surfaced as `Backend` (never a silent default) rather than a `panic!`/`unreachable!`
    // in case that invariant is ever broken by a future migration or a hand-edited row.
    Err(RepositoryError::Backend(Box::new(std::io::Error::other(format!("service_account {sa_id} has no owner column set")))))
}

/// Builds the insertable `service_account` row from a domain `ServiceAccount`.
fn sa_to_model(sa: &ServiceAccount) -> service_account::ActiveModel {
    let (owner_org_id, owner_team_id, owner_project_id) = owner_columns(&sa.owner);
    service_account::ActiveModel {
        principal_id: Set(sa.principal_id.uuid()),
        owner_org_id: Set(owner_org_id),
        owner_team_id: Set(owner_team_id),
        owner_project_id: Set(owner_project_id),
        name: Set(sa.name.clone()),
        created_at: Set(sa.created_at),
        updated_at: Set(sa.updated_at),
    }
}

/// Re-parses a stored `service_account` row back into the pure-core `ServiceAccount`: a parse
/// failure on stored data becomes a `Backend` error (never a silent default) — the row was
/// written by this same adapter, so a failure here means the data is corrupt or the domain's
/// parsing rules changed underneath it (mirrors `pg_organizations.rs::model_to_org`). A
/// service account's PRN is synthesized directly from its uuid rather than stored (mirrors
/// `pg_role_grants.rs::model_to_grant`'s `principal_id` handling): a principal's PRN shape is
/// fully deterministic (`iam`, no org, `principal`, the uuid).
async fn model_to_sa<C: ConnectionTrait>(db: &C, model: service_account::Model) -> Result<ServiceAccount, RepositoryError> {
    let backend = |msg: String| RepositoryError::Backend(Box::new(std::io::Error::other(msg)));
    let prn = Prn::build("iam", "", None, "principal", model.principal_id).map_err(|e| backend(e.to_string()))?;
    let owner = owner_from_columns(db, model.principal_id, model.owner_org_id, model.owner_team_id, model.owner_project_id).await?;

    Ok(ServiceAccount {
        principal_id: PrincipalId::from_prn(prn),
        owner,
        name: model.name,
        created_at: model.created_at,
        updated_at: model.updated_at,
    })
}

/// Extracts the principal's lifecycle status from the LEFT-JOINed `principal` row a
/// `find_also_related` query returns alongside a `service_account` row. Both a missing row
/// (`None`) and an unparseable stored string are data-integrity breaks — `fk_service_account_
/// principal` guarantees the row exists, so either means the data is corrupt or the domain's
/// parsing rules changed underneath it — surfaced as `Backend`, never a silent default (mirrors
/// `missing_owner`/`model_to_sa`'s posture).
fn principal_status(sa_id: Uuid, principal: Option<principal::Model>) -> Result<PrincipalStatus, RepositoryError> {
    let backend = |msg: String| RepositoryError::Backend(Box::new(std::io::Error::other(msg)));
    let model = principal.ok_or_else(|| backend(format!("principal missing for service_account {sa_id}")))?;
    PrincipalStatus::parse(&model.status).ok_or_else(|| backend(format!("service_account {sa_id}'s principal has invalid status {:?}", model.status)))
}

#[async_trait]
impl ServiceAccountRepository for PgServiceAccountRepository {
    async fn create(&self, principal: &Principal, sa: &ServiceAccount) -> Result<(), RepositoryError> {
        // Thin one-shot-`UnitOfWork` wrapper (module docs), mirroring `PgRoleGrantStore::
        // grant`/`grant_in`: open a `SeaOrmTransaction`, insert via `create_in`, commit.
        let txn = self.db.begin().await.map_err(map_err)?;
        let tx: Box<dyn Transaction> = Box::new(SeaOrmTransaction { txn });
        self.create_in(&*tx, principal, sa).await?;
        tx.commit().await?;
        Ok(())
    }

    async fn create_in(&self, tx: &dyn Transaction, principal: &Principal, sa: &ServiceAccount) -> Result<(), RepositoryError> {
        let txn = recover_txn(tx)?;

        let principal_model = principal::ActiveModel {
            id: Set(principal.id.uuid()),
            prn: Set(principal.id.canonical()),
            kind: Set(principal.kind.as_str().to_string()),
            status: Set(principal.status.as_str().to_string()),
            created_at: Set(principal.created_at),
            updated_at: Set(principal.updated_at),
        };
        principal_model.insert(txn).await.map_err(map_err)?;

        sa_to_model(sa).insert(txn).await.map_err(map_err)?;

        Ok(())
    }

    async fn find(&self, id: &PrincipalId) -> Result<Option<ServiceAccountRecord>, RepositoryError> {
        let Some((model, principal_model)) = service_account::Entity::find_by_id(id.uuid())
            .find_also_related(principal::Entity)
            .one(&self.db)
            .await
            .map_err(map_err)?
        else {
            return Ok(None);
        };
        let status = principal_status(model.principal_id, principal_model)?;
        let account = model_to_sa(&self.db, model).await?;
        Ok(Some(ServiceAccountRecord { account, status }))
    }

    async fn list_by_owner(&self, owner: &TenancyNodeRef, limit: u64, offset: u64) -> Result<Vec<ServiceAccountRecord>, RepositoryError> {
        let query = service_account::Entity::find();
        let query = match owner {
            TenancyNodeRef::Organization(id) => query.filter(service_account::Column::OwnerOrgId.eq(id.uuid())),
            TenancyNodeRef::Team(id) => query.filter(service_account::Column::OwnerTeamId.eq(id.uuid())),
            TenancyNodeRef::Project(id) => query.filter(service_account::Column::OwnerProjectId.eq(id.uuid())),
        };
        let rows = query
            .find_also_related(principal::Entity)
            .order_by_asc(service_account::Column::CreatedAt)
            .order_by_asc(service_account::Column::PrincipalId)
            .limit(limit)
            .offset(offset)
            .all(&self.db)
            .await
            .map_err(map_err)?;

        // Every row's owner is, by construction, the filtered-on `owner` itself — reused
        // directly rather than re-derived per row (which would cost a team/project join per
        // row, `owner_from_columns`-style, for no benefit).
        let backend = |msg: String| RepositoryError::Backend(Box::new(std::io::Error::other(msg)));
        rows.into_iter()
            .map(|(m, principal_model)| {
                let status = principal_status(m.principal_id, principal_model)?;
                let prn = Prn::build("iam", "", None, "principal", m.principal_id).map_err(|e| backend(e.to_string()))?;
                Ok(ServiceAccountRecord {
                    account: ServiceAccount {
                        principal_id: PrincipalId::from_prn(prn),
                        owner: owner.clone(),
                        name: m.name,
                        created_at: m.created_at,
                        updated_at: m.updated_at,
                    },
                    status,
                })
            })
            .collect()
    }

    async fn set_principal_status(&self, id: &PrincipalId, status: PrincipalStatus) -> Result<(), RepositoryError> {
        // Thin one-shot-`UnitOfWork` wrapper (module docs), mirroring `create` above.
        let txn = self.db.begin().await.map_err(map_err)?;
        let tx: Box<dyn Transaction> = Box::new(SeaOrmTransaction { txn });
        self.set_principal_status_in(&*tx, id, status).await?;
        tx.commit().await?;
        Ok(())
    }

    async fn set_principal_status_in(&self, tx: &dyn Transaction, id: &PrincipalId, status: PrincipalStatus) -> Result<(), RepositoryError> {
        let txn = recover_txn(tx)?;
        let Some(model) = principal::Entity::find_by_id(id.uuid()).one(txn).await.map_err(map_err)? else {
            return Err(RepositoryError::NotFound);
        };
        let mut active = model.into_active_model();
        active.status = Set(status.as_str().to_string());
        active.update(txn).await.map_err(map_err)?;
        Ok(())
    }
}
