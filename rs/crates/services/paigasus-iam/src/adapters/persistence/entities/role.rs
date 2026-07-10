// SPDX-License-Identifier: Apache-2.0

//! SeaORM entity for the `role` table. Persistence representation only — mapped to/from the
//! pure-core `Role` in the repository adapter (never derives on core types). `scope_kinds` is
//! stored as a JSON array of node-kind strings (e.g. `["organization"]`); the repository
//! adapter (de)serializes it, not this entity.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "role")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub key: String,
    pub template_id: String,
    pub scope_kinds: String,
    pub description: Option<String>,
    pub system: bool,
    pub created_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
