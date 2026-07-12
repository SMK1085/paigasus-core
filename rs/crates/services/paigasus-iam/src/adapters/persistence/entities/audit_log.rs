// SPDX-License-Identifier: Apache-2.0

//! SeaORM entity for the `audit_log` table. Persistence representation only — mapped to/from
//! the pure-core `AuditEntry` in the repository adapter (never derives on core types).
//! `determining_policies`/`detail` are serialized TEXT (mirrors `api_key.scope_actions`/
//! `scope_roles`, m0005): `determining_policies` is a JSON-encoded `Vec<String>` when set,
//! `detail` is a JSON string (`"{}"` for denials with no extra detail) — never native
//! Postgres `text[]`/`jsonb` (Slice A convention, avoids a new SeaORM column feature).

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "audit_log")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub occurred_at: DateTimeUtc,
    pub actor_prn: Option<String>,
    pub action: String,
    pub resource_prn: Option<String>,
    pub outcome: String,
    pub determining_policies: Option<String>,
    pub detail: String,
    pub correlation_id: Option<Uuid>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
