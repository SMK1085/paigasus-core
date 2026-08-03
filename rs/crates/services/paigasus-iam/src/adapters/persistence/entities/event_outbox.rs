// SPDX-License-Identifier: Apache-2.0

//! SeaORM entity for the `event_outbox` table. Persistence representation only — mapped
//! to/from the pure-core `DomainEvent` in `PgOutbox` (never derives on core types). `payload`
//! is serialized TEXT (mirrors `audit_log.detail`, m0006): the `serde_json::Value`'s plain
//! `to_string()`, never native Postgres `jsonb`. `published_at`/`attempts`/`parked` are the
//! relay's own bookkeeping columns — `PgOutbox::enqueue` (this task) only ever writes the
//! initial unpublished row; the relay (a later task) owns transitioning them.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "event_outbox")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub occurred_at: DateTimeUtc,
    pub event_type: String,
    pub schema_version: i32,
    pub aggregate_prn: String,
    pub actor_prn: Option<String>,
    pub payload: String,
    pub correlation_id: Option<Uuid>,
    pub published_at: Option<DateTimeUtc>,
    pub attempts: i32,
    pub parked: bool,
    /// When the relay flipped `parked` to true (SMA-469). `[outbox.retention].parked_days`
    /// measures from this, never from `occurred_at`. Expected to be non-NULL for a parked row,
    /// but this is not schema-enforced (no CHECK constraint — a rolling deploy can have an old
    /// replica park a row without stamping it, and that must not hard-fail).
    pub parked_at: Option<DateTimeUtc>,
    /// The most recent publish-failure reason, rewritten on EVERY failed attempt (not only at
    /// parking) so an operator watching `attempts` climb sees the current cause. A later replay
    /// operation does NOT NULL this out, so it still names the last real cause afterward too.
    pub last_error: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
