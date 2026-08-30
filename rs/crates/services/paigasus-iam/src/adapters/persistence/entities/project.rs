// SPDX-License-Identifier: Apache-2.0

//! SeaORM entity for the `project` table. Persistence representation only — mapped to/from
//! the pure-core `Project` in the repository adapter (never derives on core types).

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "project")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub team_id: Uuid,
    pub org_id: Uuid,
    #[sea_orm(unique)]
    pub prn: String,
    pub slug: String,
    pub name: String,
    pub status: String,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
    pub created_by: Option<String>,
    pub modified_by: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
