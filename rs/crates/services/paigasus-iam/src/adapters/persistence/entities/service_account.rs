// SPDX-License-Identifier: Apache-2.0

//! SeaORM entity for the `service_account` table (1:1 with `principal`, shared PK, mirrors
//! `user.rs`). Persistence representation only — mapped to/from the pure-core `ServiceAccount`
//! in the repository adapter (never derives on core types). Exactly one of
//! `owner_org_id`/`owner_team_id`/`owner_project_id` is set — enforced in Postgres by
//! `ck_service_account_owner`, not here. **No `status` field**: SA lifecycle status lives on
//! the `principal` row (SMA-445 design D16).

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "service_account")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub principal_id: Uuid,
    pub owner_org_id: Option<Uuid>,
    pub owner_team_id: Option<Uuid>,
    pub owner_project_id: Option<Uuid>,
    pub name: String,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(belongs_to = "super::principal::Entity", from = "Column::PrincipalId", to = "super::principal::Column::Id")]
    Principal,
}

impl Related<super::principal::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Principal.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
