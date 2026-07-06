// SPDX-License-Identifier: Apache-2.0

//! SeaORM entity for the `team` table. Persistence representation only — mapped to/from
//! the pure-core `Team` in the repository adapter (never derives on core types).

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "team")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub org_id: Uuid,
    #[sea_orm(unique)]
    pub prn: String,
    pub slug: String,
    pub name: String,
    pub status: String,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
