// SPDX-License-Identifier: Apache-2.0

//! SeaORM entity for the `role_grant` table. Persistence representation only — mapped
//! to/from the pure-core `RoleGrant`/`GrantScope` in the repository adapter (never derives on
//! core types). Exactly one of `scope_org_id`/`scope_team_id`/`scope_project_id` is set when
//! `scope_kind` names a tenancy node kind, and all three are NULL for `scope_kind == "root"` —
//! enforced in Postgres by `ck_role_grant_scope`, not here.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "role_grant")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub principal_id: Uuid,
    pub role_key: String,
    pub scope_kind: String,
    pub scope_node_prn: String,
    pub scope_org_id: Option<Uuid>,
    pub scope_team_id: Option<Uuid>,
    pub scope_project_id: Option<Uuid>,
    #[sea_orm(unique)]
    pub linked_policy_id: String,
    pub created_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
