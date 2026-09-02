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
use metrics::counter;
use paigasus_iam_core::{DomainEvent, Outbox, RepositoryError, Transaction};
use paigasus_observability::names;
use sea_orm::{ActiveModelTrait, ConnectionTrait, DbBackend, Set, Statement};

/// `enqueue` never touches `&self` beyond reading [`Self::notify`] — all writes go to the
/// caller-supplied transaction — but the port is injected as `Arc<dyn Outbox>` (mirroring every
/// other adapter here), so a tiny value type is the simplest shape satisfying that convention.
///
/// Deliberately NOT `Default`: the only sensible default for `notify` is `true`, and a
/// `Default` that silently shipped `false` would disable SMA-489 with no diagnostic.
#[derive(Clone, Copy)]
pub struct PgOutbox {
    notify: bool,
}

impl PgOutbox {
    /// `notify` mirrors `[outbox].wake_on_commit` (SMA-489 D11).
    #[must_use]
    pub fn new(notify: bool) -> Self {
        PgOutbox { notify }
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
        parked_at: Set(None),
        last_error: Set(None),
    }
}

/// The Postgres channel the relay's `PgOutboxListener` subscribes to (SMA-489 D3). Lowercase
/// on purpose: sqlx emits `LISTEN "iam_outbox_event"` (quoted, case-preserving) while
/// `pg_notify` takes the channel as a VALUE — the two agree only while the name has no
/// uppercase.
const WAKE_CHANNEL: &str = "iam_outbox_event";

#[async_trait]
impl Outbox for PgOutbox {
    async fn enqueue(&self, tx: &dyn Transaction, ev: &DomainEvent) -> Result<(), RepositoryError> {
        let txn = recover_txn(tx)?;
        event_to_model(ev).insert(txn).await.map_err(map_err)?;
        if self.notify {
            // SMA-489 D2. Emitted INSIDE the caller's transaction on purpose: Postgres buffers
            // the notification and delivers it ONLY if that transaction commits, discarding it
            // on rollback. That is what makes "signal after commit" structural here rather than
            // a rule every call site has to remember.
            //
            // The payload is empty (D3): the relay re-queries for work anyway, and an empty
            // payload means a hostile session that LISTENs on this channel — they are
            // database-wide and unprivileged — learns only that SOME mutation happened, never
            // which principal or event type.
            //
            // NOTE (D4): if Postgres's async notification queue is FULL this does not fail
            // here — the transaction fails at COMMIT instead, surfacing from
            // `SeaOrmTransaction::commit` as an opaque backend error. That is why
            // `[outbox].wake_on_commit` gates this writer and not only the listener.
            txn.execute_raw(Statement::from_string(DbBackend::Postgres, format!("SELECT pg_notify('{WAKE_CHANNEL}', '')")))
                .await
                .map_err(map_err)?;
            // SMA-495. AFTER the `?`, so a `pg_notify` that failed to execute is never counted.
            // This is the control term `IamOutboxNotificationsAbsent` gates on: it means "a nudge
            // was emitted", which `iam_outbox_relay_drained_total` only approximated — a drain
            // also counts SMA-469 dead-letter replays, which emit no notification at all.
            //
            // Counted PRE-COMMIT: there is no post-commit hook on a recovered transaction, so a
            // rolled-back mutation increments this while delivering nothing. That is absorbed by
            // the alert's separate `drained` term. Do NOT move this increment out of the `notify`
            // branch or below the transaction boundary without re-reading that rule.
            counter!(names::IAM_OUTBOX_NOTIFYING_ENQUEUES_TOTAL).increment(1);
        }
        Ok(())
    }
}
