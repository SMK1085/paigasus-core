// SPDX-License-Identifier: Apache-2.0

//! Postgres-backed `AuditLog` (SeaORM). `record_out_of_band` is a single autocommit insert
//! (no txn — the port doc contract: the audit write happens after the triggering
//! transaction already committed, or after a denial, so it is never part of that
//! transaction's atomicity). `record` (SMA-446, Slice B) is its in-txn twin: it recovers the
//! caller's SeaORM transaction via `uow::recover_txn` (B3) and inserts the SAME
//! `entry_to_model`-built row on it, so the audit row only becomes visible if the caller's own
//! transaction commits — used when the audit write must be atomic with the mutation it
//! describes (and typically the same mutation's `Outbox::enqueue`, `pg_outbox.rs`). `query`
//! builds a keyset-paginated read: `ORDER BY id DESC` (UUIDv7 ids, so this doubles as
//! occurred-at-descending) with `WHERE id < cursor` for the next page, an equality filter per
//! present `AuditFilter` field, and `LIMIT filter.capped_limit()`.
//!
//! `determining_policies`/`detail` are TEXT columns (m0006, no native `text[]`/`jsonb` —
//! mirrors `entities::audit_log`'s doc and `pg_api_keys.rs::roles_to_column`'s precedent):
//! `determining_policies` is always stored as a JSON-encoded `Vec<String>` (never `NULL` —
//! simplest of the two options the column supports); `detail` is the `serde_json::Value`'s
//! plain `to_string()`. Both serializations are infallible (`Vec<String>`/`Value` always
//! serialize); only the READ side can fail, when a stored value doesn't parse back — a
//! data-integrity break surfaced as `RepositoryError::Backend`, never a silent default,
//! mirroring `pg_repository.rs::map_principal_row`'s posture for a bad enum.

use super::entities::audit_log;
use super::map_err;
use super::uow::recover_txn;
use async_trait::async_trait;
use paigasus_iam_core::{AuditEntry, AuditFilter, AuditLog, AuditOutcome, RepositoryError, Transaction};
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, QuerySelect, Set};

// `Clone` lets the composition root hold a sink handle inside a `#[derive(Clone)]` service
// (mirrors `PgPrincipalRepository`'s precedent) — cheap: `DatabaseConnection` clones an
// `Arc`-backed pool handle, not a connection.
#[derive(Clone)]
pub struct PgAuditLog {
    db: DatabaseConnection,
}

impl PgAuditLog {
    #[must_use]
    pub fn new(db: DatabaseConnection) -> Self {
        PgAuditLog { db }
    }
}

fn backend_err(msg: impl std::fmt::Display) -> RepositoryError {
    RepositoryError::Backend(Box::new(std::io::Error::other(msg.to_string())))
}

/// Builds the insertable `audit_log` row from a domain `AuditEntry`. `determining_policies`
/// is always `Some(json)` (never `NULL`, even for an empty vec — simplest of the two
/// column-storage options and infallible either way).
fn entry_to_model(e: &AuditEntry) -> audit_log::ActiveModel {
    audit_log::ActiveModel {
        id: Set(e.id),
        occurred_at: Set(e.occurred_at),
        actor_prn: Set(e.actor_prn.clone()),
        action: Set(e.action.clone()),
        resource_prn: Set(e.resource_prn.clone()),
        outcome: Set(e.outcome.as_str().to_string()),
        determining_policies: Set(Some(serde_json::to_string(&e.determining_policies).expect("Vec<String> always serializes"))),
        detail: Set(e.detail.to_string()),
        correlation_id: Set(e.correlation_id),
    }
}

/// Inverse of [`entry_to_model`]'s `determining_policies` half: `NULL` or an empty string is
/// an empty vec; otherwise the stored JSON array is parsed back. Unparseable JSON is a
/// `Backend` error — the row was written by this same adapter, so a failure here means the
/// data is corrupt.
fn policies_from_column(raw: Option<&str>) -> Result<Vec<String>, RepositoryError> {
    match raw {
        None | Some("") => Ok(Vec::new()),
        Some(s) => serde_json::from_str(s).map_err(backend_err),
    }
}

/// Inverse of [`entry_to_model`]'s `detail` half: an empty string falls back to `{}` (mirrors
/// the migration test's seed row, m0006); otherwise the stored JSON string is parsed back.
/// Unparseable JSON is a `Backend` error, same posture as [`policies_from_column`].
fn detail_from_column(raw: &str) -> Result<serde_json::Value, RepositoryError> {
    if raw.is_empty() {
        return Ok(serde_json::json!({}));
    }
    serde_json::from_str(raw).map_err(backend_err)
}

/// Reconstructs the domain `AuditEntry` from a stored row. `outcome` is a plain unconstrained
/// TEXT column (m0006 has no CHECK constraint on it) — this parse is the safety net: the
/// adapter only ever writes `AuditOutcome::as_str()` values, so a stored `outcome` outside
/// `{committed, denied}` would mean external tampering or corruption. Surfaced as `Backend`,
/// never a silent default, mirroring `pg_repository.rs::map_principal_row`'s posture for a bad
/// enum.
fn model_to_entry(m: audit_log::Model) -> Result<AuditEntry, RepositoryError> {
    let outcome = AuditOutcome::parse(&m.outcome).ok_or_else(|| backend_err(format!("bad audit outcome: {}", m.outcome)))?;
    let determining_policies = policies_from_column(m.determining_policies.as_deref())?;
    let detail = detail_from_column(&m.detail)?;
    Ok(AuditEntry {
        id: m.id,
        occurred_at: m.occurred_at,
        actor_prn: m.actor_prn,
        action: m.action,
        resource_prn: m.resource_prn,
        outcome,
        determining_policies,
        detail,
        correlation_id: m.correlation_id,
    })
}

#[async_trait]
impl AuditLog for PgAuditLog {
    async fn record_out_of_band(&self, e: &AuditEntry) -> Result<(), RepositoryError> {
        entry_to_model(e).insert(&self.db).await.map_err(map_err)?;
        Ok(())
    }

    async fn record(&self, tx: &dyn Transaction, e: &AuditEntry) -> Result<(), RepositoryError> {
        let txn = recover_txn(tx)?;
        entry_to_model(e).insert(txn).await.map_err(map_err)?;
        Ok(())
    }

    async fn query(&self, f: &AuditFilter) -> Result<Vec<AuditEntry>, RepositoryError> {
        let mut q = audit_log::Entity::find();
        if let Some(actor_prn) = &f.actor_prn {
            q = q.filter(audit_log::Column::ActorPrn.eq(actor_prn.clone()));
        }
        if let Some(resource_prn) = &f.resource_prn {
            q = q.filter(audit_log::Column::ResourcePrn.eq(resource_prn.clone()));
        }
        if let Some(action) = &f.action {
            q = q.filter(audit_log::Column::Action.eq(action.clone()));
        }
        if let Some(outcome) = f.outcome {
            q = q.filter(audit_log::Column::Outcome.eq(outcome.as_str()));
        }
        if let Some(from) = f.from {
            q = q.filter(audit_log::Column::OccurredAt.gte(from));
        }
        if let Some(to) = f.to {
            q = q.filter(audit_log::Column::OccurredAt.lte(to));
        }
        // Keyset paging (port doc contract): the next page is every row strictly older
        // (lower id) than the last row of the previous page.
        if let Some(cursor) = f.cursor {
            q = q.filter(audit_log::Column::Id.lt(cursor));
        }

        let models = q.order_by_desc(audit_log::Column::Id).limit(f.capped_limit()).all(&self.db).await.map_err(map_err)?;
        models.into_iter().map(model_to_entry).collect()
    }
}
