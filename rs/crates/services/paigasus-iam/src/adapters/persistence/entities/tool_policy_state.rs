// SPDX-License-Identifier: Apache-2.0

//! SeaORM entity for the `tool_policy_state` table. Persistence representation only.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "tool_policy_state")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub set_key: String,
    pub kind: String,
    pub org_id: Option<Uuid>,
    pub catalog_revision: i64,
    pub live_version: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
