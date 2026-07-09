// SPDX-License-Identifier: Apache-2.0

//! SeaORM entity for the `policy` table. Persistence representation only — mapped to/from
//! the pure-core `PolicyDocument` in the repository adapter (never derives on core types).

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "policy")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub policy_id: String,
    pub kind: String,
    pub source: String,
    pub description: Option<String>,
    pub system: bool,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
