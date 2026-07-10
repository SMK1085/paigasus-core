// SPDX-License-Identifier: Apache-2.0

//! Postgres-backed `EntitySliceLoader` (SMA-444 Task 12): loads the minimal Cedar
//! `EntitySlice` needed to decide one `AccessRequest` — the synthetic `Root` singleton,
//! the resource's tenancy ancestor chain, and the principal.
//!
//! Every tenancy node's `effective_status` attr is taken straight from that node's OWN
//! `NodeView` (the M1 read adapters — `pg_organizations.rs::org_view`,
//! `pg_teams.rs::team_view`, `pg_projects.rs::project_view` — already fold every ancestor's
//! status into it, D1/D10), so this loader never re-derives the fold: it re-reads each node
//! in the chain via its own repository's `find` and takes the returned `effective_status`
//! verbatim. That also means archiving an ancestor is visible immediately on every
//! descendant's next `load` — no separate propagation step.
//!
//! `entity_gen` delegates to the shared `Generations` handle (Task 10) — this loader itself
//! does not bump it; that is Task 15's job (tenancy write adapters bump `entity_gen` on
//! create/rename/set_status), tracked separately from this read-only loader.

use super::entities::principal;
use super::{PgOrganizationRepository, PgProjectRepository, PgTeamRepository};
use crate::adapters::authz::Generations;
use async_trait::async_trait;
use paigasus_iam_core::authz::model::{ContextValue, EntitySlice, ROOT_ENTITY, SliceEntity, root_prn};
use paigasus_iam_core::{AuthzError, EntitySliceLoader, NodeStatus, OrganizationId, OrganizationRepository, ProjectRepository, RepositoryError, TeamRepository, TenancyNodeRef};
use paigasus_kernel::{Prn, to_cedar_uid};
use sea_orm::{DatabaseConnection, EntityTrait};
use std::collections::BTreeMap;
use uuid::Uuid;

/// `Clone` mirrors `PgPolicyStore`/`PgRoleGrantStore`: cheap, since `DatabaseConnection`
/// clones an `Arc`-backed pool handle and `Generations` is `Arc`-backed too.
#[derive(Clone)]
pub struct PgEntitySliceLoader {
    db: DatabaseConnection,
    gens: Generations,
}

impl PgEntitySliceLoader {
    #[must_use]
    pub fn new(db: DatabaseConnection, gens: Generations) -> Self {
        PgEntitySliceLoader { db, gens }
    }

    /// Loads the resource node's ancestor chain (organization, team, project as
    /// applicable) as `SliceEntity`s, each parented up to `root_uid`. Root itself is NOT
    /// included here — the caller already seeds it once, up front.
    async fn load_chain(&self, node: TenancyNodeRef, root_uid: &(String, String)) -> Result<Vec<SliceEntity>, AuthzError> {
        match node {
            TenancyNodeRef::Organization(id) => {
                let org_repo = PgOrganizationRepository::new(self.db.clone(), self.gens.clone());
                let view = org_repo.find(id.uuid()).await.map_err(backend)?.ok_or_else(|| missing("organization", id.uuid()))?;

                Ok(vec![SliceEntity {
                    uid: uid_pair(id.prn()),
                    parents: vec![root_uid.clone()],
                    attrs: status_attrs(view.effective_status),
                }])
            }
            TenancyNodeRef::Team(id) => {
                let team_repo = PgTeamRepository::new(self.db.clone(), self.gens.clone());
                let team_view = team_repo.find(id.uuid()).await.map_err(backend)?.ok_or_else(|| missing("team", id.uuid()))?;

                // SMA-444 cross-tenant-escalation fix: `id` is `TenancyNodeRef::from_prn`'d
                // straight from the caller-supplied resource PRN, whose org slot
                // `TeamId::from_prn` only checks is PRESENT, never that it's CORRECT
                // (`tenancy::check`) — so `id.org_uuid()` is caller-controlled. The team's
                // REAL parent org lives in `team_view.node.id` (reconstructed by
                // `pg_teams.rs::model_to_team` from the DB row's own `prn`/`org_id` columns,
                // written together in lockstep at creation and never touched by
                // rename/set_status), exactly mirroring how the `Project` branch below derives
                // its parent from `project_view.node.team_id`, never the caller's PRN.
                let org_id = OrganizationId::from_uuid(team_view.node.id.org_uuid());
                let org_repo = PgOrganizationRepository::new(self.db.clone(), self.gens.clone());
                let org_view = org_repo.find(org_id.uuid()).await.map_err(backend)?.ok_or_else(|| missing("organization", org_id.uuid()))?;

                let org_uid = uid_pair(org_id.prn());
                Ok(vec![
                    SliceEntity {
                        uid: uid_pair(id.prn()),
                        parents: vec![org_uid.clone()],
                        attrs: status_attrs(team_view.effective_status),
                    },
                    SliceEntity {
                        uid: org_uid,
                        parents: vec![root_uid.clone()],
                        attrs: status_attrs(org_view.effective_status),
                    },
                ])
            }
            TenancyNodeRef::Project(id) => {
                let project_repo = PgProjectRepository::new(self.db.clone(), self.gens.clone());
                let project_view = project_repo.find(id.uuid()).await.map_err(backend)?.ok_or_else(|| missing("project", id.uuid()))?;
                let team_id = project_view.node.team_id;

                let team_repo = PgTeamRepository::new(self.db.clone(), self.gens.clone());
                let team_view = team_repo.find(team_id.uuid()).await.map_err(backend)?.ok_or_else(|| missing("team", team_id.uuid()))?;

                let org_id = OrganizationId::from_uuid(team_id.org_uuid());
                let org_repo = PgOrganizationRepository::new(self.db.clone(), self.gens.clone());
                let org_view = org_repo.find(org_id.uuid()).await.map_err(backend)?.ok_or_else(|| missing("organization", org_id.uuid()))?;

                let team_uid = uid_pair(team_id.prn());
                let org_uid = uid_pair(org_id.prn());
                Ok(vec![
                    SliceEntity {
                        uid: uid_pair(id.prn()),
                        parents: vec![team_uid.clone()],
                        attrs: status_attrs(project_view.effective_status),
                    },
                    SliceEntity {
                        uid: team_uid,
                        parents: vec![org_uid.clone()],
                        attrs: status_attrs(team_view.effective_status),
                    },
                    SliceEntity {
                        uid: org_uid,
                        parents: vec![root_uid.clone()],
                        attrs: status_attrs(org_view.effective_status),
                    },
                ])
            }
        }
    }

    /// The principal entity: `kind = "user"` (M3 mints no other principal kind yet).
    /// `status` is read from the `principal` table when the row exists (one cheap PK
    /// lookup) and falls back to `"active"` when it doesn't — a caller only ever reaches
    /// authz after authenticating a real principal, so a missing row isn't expected in
    /// practice, but a fallback beats a hard failure on an edge this loader doesn't need
    /// to police.
    async fn principal_entity(&self, principal_prn: &Prn) -> Result<SliceEntity, AuthzError> {
        let status = match principal::Entity::find_by_id(principal_prn.resource_id())
            .one(&self.db)
            .await
            .map_err(|e| AuthzError::Backend(Box::new(e)))?
        {
            Some(model) => model.status,
            None => "active".to_string(),
        };

        Ok(SliceEntity {
            uid: uid_pair(principal_prn),
            parents: vec![],
            attrs: BTreeMap::from([("kind".to_string(), ContextValue::Str("user".to_string())), ("status".to_string(), ContextValue::Str(status))]),
        })
    }
}

#[async_trait]
impl EntitySliceLoader for PgEntitySliceLoader {
    async fn load(&self, resource: &Prn, principal: &Prn) -> Result<EntitySlice, AuthzError> {
        let root_uid = (ROOT_ENTITY.0.to_string(), ROOT_ENTITY.1.to_string());
        let mut entities = vec![SliceEntity {
            uid: root_uid.clone(),
            parents: vec![],
            attrs: BTreeMap::new(),
        }];

        // The Root scope itself has no tenancy node to load — the slice is just Root +
        // the principal.
        if *resource != root_prn() {
            let node = TenancyNodeRef::from_prn(resource.clone()).map_err(|e| AuthzError::Backend(Box::new(e)))?;
            entities.extend(self.load_chain(node, &root_uid).await?);
        }

        entities.push(self.principal_entity(principal).await?);

        Ok(EntitySlice { entities })
    }

    async fn entity_gen(&self) -> Result<u64, AuthzError> {
        self.gens.entity_gen().await
    }
}

fn status_attrs(status: NodeStatus) -> BTreeMap<String, ContextValue> {
    BTreeMap::from([("effective_status".to_string(), ContextValue::Str(status.as_str().to_string()))])
}

fn uid_pair(prn: &Prn) -> (String, String) {
    let uid = to_cedar_uid(prn);
    (uid.entity_type, uid.entity_id)
}

fn backend(e: RepositoryError) -> AuthzError {
    AuthzError::Backend(Box::new(e))
}

/// A missing node for a PRN the caller asked to slice: the request names a resource that
/// doesn't exist (or no longer does), and there is no slice to build — surfaced as
/// `AuthzError::Backend`, the only variant that fits an unexpected-state backend failure
/// (mirrors `pg_teams.rs`/`pg_projects.rs`'s `missing_ancestor` posture for a broken FK,
/// though here the row is simply absent, not a data-integrity break).
fn missing(kind: &str, id: Uuid) -> AuthzError {
    AuthzError::Backend(Box::new(std::io::Error::other(format!("{kind} {id} not found for entity-slice load"))))
}
