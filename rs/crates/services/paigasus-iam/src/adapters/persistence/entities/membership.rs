// SPDX-License-Identifier: Apache-2.0

//! SeaORM entity for the `membership` table. Persistence representation only — mapped
//! to/from the pure-core `Membership` in the repository adapter (never derives on core
//! types). Exactly one of `org_id`/`team_id`/`project_id` is set — enforced in Postgres by
//! `ck_membership_one_target`, not here.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "membership")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub principal_id: Uuid,
    pub org_id: Option<Uuid>,
    pub team_id: Option<Uuid>,
    pub project_id: Option<Uuid>,
    pub created_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
