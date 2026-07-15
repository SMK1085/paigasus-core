// SPDX-License-Identifier: Apache-2.0

//! SeaORM entity for the `audit_log` table. Persistence representation only — mapped to/from
//! the pure-core `AuditEntry` in the repository adapter (never derives on core types).
//! `determining_policies`/`detail` are serialized TEXT (mirrors `api_key.scope_actions`/
//! `scope_roles`, m0005): `determining_policies` is a JSON-encoded `Vec<String>` when set,
//! `detail` is a JSON string (`"{}"` for denials with no extra detail) — never native
//! Postgres `text[]`/`jsonb` (Slice A convention, avoids a new SeaORM column feature).
//!
//! **DB vs entity primary key (SMA-467):** the physical table is partitioned
//! `LIST(outcome)→RANGE(occurred_at)`, so its Postgres PK is the composite `(id, occurred_at,
//! outcome)` (a partitioned table's PK must include every partition-key column). This entity
//! keeps `id` as its sole `primary_key`: `id` is a per-entry UUIDv7 (the logical identity), the
//! adapter only inserts (routing) and filters, and `Entity::find_by_id(id)` still resolves a row
//! from the correct leaf. The composite PK is a partitioning requirement, not a logical key.

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
