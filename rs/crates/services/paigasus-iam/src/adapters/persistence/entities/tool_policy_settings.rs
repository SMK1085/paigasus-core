// SPDX-License-Identifier: Apache-2.0

//! SeaORM entity for the `tool_policy_settings` table. Persistence representation only.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "tool_policy_settings")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub org_id: Uuid,
    pub mode: String,
    pub fail_mode: String,
    pub updated_at: DateTimeUtc,
    pub updated_by: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
