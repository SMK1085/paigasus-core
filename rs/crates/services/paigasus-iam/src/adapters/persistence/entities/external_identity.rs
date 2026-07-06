// SPDX-License-Identifier: Apache-2.0

//! SeaORM entity for the `external_identity` table. Persistence representation only — mapped
//! to/from the pure-core `ExternalIdentity` in `pg_external_identities` (SeaORM never derives
//! on the core types). No FK cascade on `principal_id` — principals are never hard-deleted.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "external_identity")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub principal_id: Uuid,
    pub issuer: String,
    pub subject: String,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
