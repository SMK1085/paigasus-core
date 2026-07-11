// SPDX-License-Identifier: Apache-2.0

//! SeaORM entity for the `api_key` table (own UUID PK). Persistence representation only —
//! mapped to/from the pure-core API-key type in the repository adapter (never derives on core
//! types). Exactly one of `scope_org_id`/`scope_team_id`/`scope_project_id` is set — enforced
//! in Postgres by `ck_api_key_scope`, not here. `key_hash`/`scope_actions`/`scope_roles` never
//! carry the plaintext secret — only its hash and the JSON-encoded scope narrowing.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "api_key")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub service_account_id: Uuid,
    pub scope_org_id: Option<Uuid>,
    pub scope_team_id: Option<Uuid>,
    pub scope_project_id: Option<Uuid>,
    pub prefix: String,
    #[sea_orm(unique)]
    pub key_hash: String,
    pub status: String,
    pub expires_at: Option<DateTimeUtc>,
    pub last_used_at: Option<DateTimeUtc>,
    pub created_at: DateTimeUtc,
    pub revoked_at: Option<DateTimeUtc>,
    pub scope_actions: Option<String>,
    pub scope_roles: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
