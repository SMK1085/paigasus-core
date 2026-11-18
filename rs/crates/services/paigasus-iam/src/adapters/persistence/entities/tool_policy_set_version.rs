// SPDX-License-Identifier: Apache-2.0

//! SeaORM entity for the `tool_policy_set_version` table. Persistence representation only.
//!
//! The table has no primary key. Its identity is the unique index `uq_tool_policy_set_version`,
//! which uses `COALESCE(org_id, …)`. SeaORM needs a declared key, so `kind` and `version` are
//! declared here. Do not use `find_by_id` on this entity. Filter by column.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "tool_policy_set_version")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub kind: String,
    pub org_id: Option<Uuid>,
    #[sea_orm(primary_key, auto_increment = false)]
    pub version: i64,
    pub source: String,
    /// `None` only for a held or discarded row (a CHECK enforces it).
    pub compiled: Option<Vec<u8>>,
    pub compiler_generation: i32,
    pub schema_revision: i32,
    pub catalog_revision: Option<i64>,
    pub status: String,
    pub origin: String,
    pub base_version: Option<i64>,
    pub report: Option<Json>,
    pub created_by: String,
    pub created_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
