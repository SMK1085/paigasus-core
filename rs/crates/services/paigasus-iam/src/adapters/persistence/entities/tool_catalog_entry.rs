// SPDX-License-Identifier: Apache-2.0

//! SeaORM entity for the `tool_catalog_entry` table. Persistence representation only.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "tool_catalog_entry")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub org_id: Uuid,
    #[sea_orm(primary_key, auto_increment = false)]
    pub name_key: String,
    pub display_name: String,
    pub kind: String,
    pub input_schema: Option<Json>,
    pub cedar_input_type: Json,
    pub dropped_fields: Json,
    pub source: String,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
