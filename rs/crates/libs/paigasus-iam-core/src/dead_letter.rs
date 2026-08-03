// SPDX-License-Identifier: Apache-2.0

//! Dead-letter value types and the `DeadLetters` port (SMA-469): the operator-facing view of
//! `event_outbox` rows the relay parked (`parked = true`), plus the operations that retire
//! them. Pure/kernel-friendly — ids and timestamps are injected by the caller.

use crate::ports::{RepositoryError, Transaction};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use uuid::Uuid;

/// A parked `event_outbox` row, projected for inspection.
///
/// `event_type`, `payload`, and `schema_version` are deliberately RAW — a wire `String`, a
/// serialized-TEXT `String`, and the stored `i32` — rather than `EventType`,
/// `serde_json::Value`, and `u16`. All three of the relay's malformed-row rejection reasons
/// are an unrecognized `event_type` wire string, invalid `payload` JSON, and an out-of-range
/// `schema_version`; i.e. all three are reasons a row PARKS. Typing any of them strictly would
/// make the dead-letter surface unable to display exactly the rows it exists to explain. This
/// is a diagnostic projection of a persisted row, not a domain type.
///
/// `attempts` is a plain `u32` by contrast — it is a count, never negative, and never a park
/// reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeadLetterEntry {
    pub id: Uuid,
    pub occurred_at: DateTime<Utc>,
    pub event_type: String,
    pub schema_version: i32,
    pub aggregate_prn: String,
    pub actor_prn: Option<String>,
    pub payload: String,
    pub correlation_id: Option<Uuid>,
    pub attempts: u32,
    pub parked_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
}

/// Listing + keyset paging. `parked_from`/`parked_to` filter `parked_at` — NOT `occurred_at`.
/// The operationally meaningful question is "what parked during last night's outage", which
/// `occurred_at` cannot answer; the fields are named for the column so no call site can be
/// ambiguous about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeadLetterFilter {
    pub event_type: Option<String>,
    pub parked_from: Option<DateTime<Utc>>,
    pub parked_to: Option<DateTime<Utc>>,
    pub cursor: Option<Uuid>,
    pub limit: u64,
}

impl DeadLetterFilter {
    /// Mirrors `AuditFilter::MAX_LIMIT`.
    pub const MAX_LIMIT: u64 = 200;
    pub fn capped_limit(&self) -> u64 {
        self.limit.clamp(1, Self::MAX_LIMIT)
    }
}

/// Bulk replay. A SEPARATE type from [`DeadLetterFilter`], deliberately: reusing the paging
/// filter would put its `MAX_LIMIT` (200) in direct contradiction with `MAX_BULK_REPLAY`
/// (10_000) and leave `cursor` meaningless on a path that does not page.
///
/// `max_rows` is REQUIRED and is the guard. An "at least one filter field must be present"
/// check was considered and rejected: `parked_from = 1970-01-01T00:00:00Z` satisfies it while
/// matching every row, and that is the most natural way an operator writes "replay
/// everything". An explicit row budget cannot be satisfied by accident.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BulkReplayRequest {
    pub event_type: Option<String>,
    pub parked_from: Option<DateTime<Utc>>,
    pub parked_to: Option<DateTime<Utc>>,
    pub max_rows: u64,
}

impl BulkReplayRequest {
    pub const MAX_BULK_REPLAY: u64 = 10_000;
    /// `false` when `max_rows` is absent/zero — the caller must state its blast radius.
    pub fn is_valid(&self) -> bool {
        self.max_rows > 0
    }
    pub fn capped_max_rows(&self) -> u64 {
        self.max_rows.min(Self::MAX_BULK_REPLAY)
    }
}

/// Inspect and retire parked outbox rows.
///
/// The three mutating methods take the CALLER's transaction (like `Outbox::enqueue`) so the
/// mutation and its audit entry commit atomically on one `UnitOfWork`. They return the affected
/// row (or a count) rather than a bare bool so the caller can build a complete audit entry —
/// for `discard_in` that entry is the discarded event's ONLY remaining trace.
#[async_trait]
pub trait DeadLetters: Send + Sync {
    async fn list(&self, f: &DeadLetterFilter) -> Result<Vec<DeadLetterEntry>, RepositoryError>;
    /// `None` when no PARKED row has that id (absent, live, or already published/discarded).
    async fn replay_in(&self, tx: &dyn Transaction, id: Uuid) -> Result<Option<DeadLetterEntry>, RepositoryError>;
    /// Returns how many rows were un-parked.
    async fn replay_matching_in(&self, tx: &dyn Transaction, r: &BulkReplayRequest) -> Result<u64, RepositoryError>;
    /// `None` when no PARKED row has that id.
    async fn discard_in(&self, tx: &dyn Transaction, id: Uuid) -> Result<Option<DeadLetterEntry>, RepositoryError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    // Compile-time proof `DeadLetters` is object-safe (injected as `Arc<dyn DeadLetters>` by
    // a later task — mirrors the `assert_object_safe`/`*_is_object_safe` pattern in
    // `ports.rs`/`authz/ports.rs`).
    #[allow(dead_code)]
    fn assert_object_safe(_: &dyn DeadLetters) {}

    #[test]
    fn filter_limit_is_clamped_like_the_audit_filter() {
        let f = |limit| DeadLetterFilter {
            event_type: None,
            parked_from: None,
            parked_to: None,
            cursor: None,
            limit,
        };
        assert_eq!(f(0).capped_limit(), 1, "a zero limit floors at 1");
        assert_eq!(f(50).capped_limit(), 50);
        assert_eq!(f(10_000).capped_limit(), DeadLetterFilter::MAX_LIMIT);
    }

    #[test]
    fn bulk_replay_requires_an_explicit_max_rows_and_is_capped() {
        let r = |max_rows| BulkReplayRequest {
            event_type: None,
            parked_from: None,
            parked_to: None,
            max_rows,
        };
        // A missing/zero max_rows is invalid: the required, explicit blast radius IS the guard.
        assert!(!r(0).is_valid(), "max_rows = 0 must be rejected, not treated as unlimited");
        assert!(r(1).is_valid());
        assert_eq!(r(1_000_000).capped_max_rows(), BulkReplayRequest::MAX_BULK_REPLAY);
        assert_eq!(r(500).capped_max_rows(), 500);
    }
}
