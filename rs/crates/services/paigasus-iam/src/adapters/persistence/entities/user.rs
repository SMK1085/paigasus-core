// SPDX-License-Identifier: Apache-2.0

//! SeaORM entity for the `user` table (1:1 with `principal`, shared PK).

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "user")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub principal_id: Uuid,
    #[sea_orm(unique)]
    pub email: String,
    pub display_name: String,
    pub locale: Option<String>,
    pub timezone: Option<String>,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(belongs_to = "super::principal::Entity", from = "Column::PrincipalId", to = "super::principal::Column::Id")]
    Principal,
}

impl Related<super::principal::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Principal.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
