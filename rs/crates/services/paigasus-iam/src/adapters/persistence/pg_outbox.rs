// SPDX-License-Identifier: Apache-2.0

//! Postgres-backed `Outbox` (SeaORM). `enqueue` writes on the CALLER's transaction (recovered
//! via `uow::recover_txn`, B3) — it never opens or commits anything itself, so the outbox row
//! only becomes visible if the caller's own transaction commits (the whole point of the
//! transactional-outbox pattern, SMA-446 Slice B).
//!
//! `payload` is a TEXT column (m0007, mirrors `audit_log.detail`, m0006): the domain event's
//! `serde_json::Value` written via its plain `to_string()` — infallible, `Value` always
//! serializes. `event_type` is the [`EventType::as_wire`](paigasus_iam_core::EventType::as_wire)
//! stable wire string, not the enum's
//! Rust name (renaming a variant must not change what's stored). `schema_version` narrows
//! `u16` -> `i32` (the entity's SeaORM column type, mirrors Postgres `integer`) — always
//! lossless, `u16::MAX` fits `i32`.

use super::entities::event_outbox;
use super::map_err;
use super::uow::recover_txn;
use async_trait::async_trait;
use paigasus_iam_core::{DomainEvent, Outbox, RepositoryError, Transaction};
use sea_orm::{ActiveModelTrait, Set};

/// Stateless: `enqueue` never touches `&self`, only the caller-supplied transaction — but the
/// port is injected as `Arc<dyn Outbox>` (mirrors every other adapter in this module), so a
/// unit struct is the simplest shape that satisfies that composition-root convention.
#[derive(Clone, Copy, Default)]
pub struct PgOutbox;

impl PgOutbox {
    #[must_use]
    pub fn new() -> Self {
        PgOutbox
    }
}

/// Builds the insertable `event_outbox` row from a domain `DomainEvent`. `published_at`/
/// `attempts`/`parked` start `NULL`/`0`/`false` — the relay (a later task) is the only writer
/// that ever transitions them.
fn event_to_model(ev: &DomainEvent) -> event_outbox::ActiveModel {
    event_outbox::ActiveModel {
        id: Set(ev.id),
        occurred_at: Set(ev.occurred_at),
        event_type: Set(ev.event_type.as_wire().to_string()),
        schema_version: Set(i32::from(ev.schema_version)),
        aggregate_prn: Set(ev.aggregate_prn.clone()),
        actor_prn: Set(ev.actor_prn.clone()),
        payload: Set(ev.payload.to_string()),
        correlation_id: Set(ev.correlation_id),
        published_at: Set(None),
        attempts: Set(0),
        parked: Set(false),
    }
}

#[async_trait]
impl Outbox for PgOutbox {
    async fn enqueue(&self, tx: &dyn Transaction, ev: &DomainEvent) -> Result<(), RepositoryError> {
        let txn = recover_txn(tx)?;
        event_to_model(ev).insert(txn).await.map_err(map_err)?;
        Ok(())
    }
}
