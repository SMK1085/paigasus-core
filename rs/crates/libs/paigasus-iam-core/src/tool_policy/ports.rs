// SPDX-License-Identifier: Apache-2.0

//! The port to the policy compiler (spec section 5.2). The adapter in the service crate runs
//! the `paigasus-policy-compiler` binary as a child process.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::model::{HeldVersion, StoredTool, ToolEntry, ToolPolicySetKey, ToolPolicySettings, ToolPolicyState, ToolPolicyVersion};
use super::protocol::{CompileRequest, CompiledOutput, Rejection};
use crate::{RepositoryError, Transaction};

/// A compile that gave no result. Neither case stores anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompilerFailure {
    /// The compiler semaphore is full (`tool-policy-compiler-busy`, retryable).
    Busy,
    /// The compiler crashed, timed out or broke the protocol (`tool-policy-compiler-unavailable`).
    /// The string is for logs only and is never returned to a caller.
    Unavailable(String),
}

/// The result of a compile that the compiler answered.
#[derive(Debug, Clone, PartialEq)]
pub enum CompileOutcome {
    Accepted(CompiledOutput),
    Rejected(Vec<Rejection>),
}

/// Compiles one tool policy set.
#[async_trait]
pub trait PolicyCompiler: Send + Sync {
    /// The compiler generation that this compiler reported at boot.
    fn generation(&self) -> u32;

    /// Compile one request.
    async fn compile(&self, req: &CompileRequest) -> Result<CompileOutcome, CompilerFailure>;
}

/// Storage of the tool catalog and the tool policy set versions (spec sections 4.4 and 4.5).
///
/// The `_in` methods run on the caller's transaction. A writer calls `lock_state_in` first.
#[async_trait]
pub trait ToolPolicyRepository: Send + Sync {
    /// The state of a set. `(0, 0)` when the set has no row.
    async fn state(&self, key: &ToolPolicySetKey) -> Result<ToolPolicyState, RepositoryError>;
    /// The catalog of an organization, sorted by `name_key`.
    async fn catalog(&self, org: Uuid) -> Result<Vec<StoredTool>, RepositoryError>;
    /// The catalog of an organization, sorted by `name_key`, and its catalog revision, from one
    /// consistent read. A catalog write that commits during the call is in both values or in
    /// neither. The revision is 0 when the set has no state row.
    async fn catalog_snapshot(&self, org: Uuid) -> Result<(Vec<StoredTool>, u64), RepositoryError>;
    /// The live version of a set.
    async fn live(&self, key: &ToolPolicySetKey) -> Result<Option<ToolPolicyVersion>, RepositoryError>;
    /// One version of a set. Only a live or superseded version is returned, never a held or
    /// discarded row.
    async fn version(&self, key: &ToolPolicySetKey, version: u64) -> Result<Option<ToolPolicyVersion>, RepositoryError>;
    /// Make the state row if it is missing, then lock it with `SELECT … FOR UPDATE`.
    async fn lock_state_in(&self, tx: &dyn Transaction, key: &ToolPolicySetKey) -> Result<ToolPolicyState, RepositoryError>;
    /// Upsert and delete catalog entries. Returns the new catalog revision (old + 1).
    async fn apply_catalog_in(&self, tx: &dyn Transaction, org: Uuid, upserts: &[ToolEntry], deletes: &[String], now: DateTime<Utc>) -> Result<u64, RepositoryError>;
    /// Store a new live version. The old live version becomes superseded. The held row of the
    /// set, if any, becomes discarded, whatever the origin of the new version (plan H7).
    ///
    /// `v.status` must be `Live`. Any other status is an error (`Backend`), because this method
    /// always writes a live row. `v.version` must be the larger of the stored `live_version` and
    /// `last_version`, plus 1 (`ToolPolicyState::next_version`). If it is not, the method returns
    /// `Conflict(ConflictKind::ToolPolicyVersion)` and changes nothing. That check is a
    /// compare-and-set on the state row. It is a backstop. The caller must still call
    /// `lock_state_in` first: the row lock is what makes writers run one after the other. The
    /// method sets `live_version` and `last_version` to `v.version`.
    async fn insert_live_version_in(&self, tx: &dyn Transaction, v: &ToolPolicyVersion) -> Result<(), RepositoryError>;
    /// Live versions whose generation or schema revision differs from the arguments. The floor
    /// first, then tenants by org UUID.
    async fn stale_live_versions(&self, compiler_generation: u32, schema_revision: u32) -> Result<Vec<ToolPolicyVersion>, RepositoryError>;
    /// The largest `compiler_generation` and `schema_revision` over every row of every status.
    /// `(0, 0)` when there is no row.
    async fn max_stored_revisions(&self) -> Result<(u32, u32), RepositoryError>;
    /// The held row of a set (status held only).
    async fn held(&self, key: &ToolPolicySetKey) -> Result<Option<HeldVersion>, RepositoryError>;
    /// One page of the held rows (status held only): of one tenant, or of every set when `org`
    /// is `None`. The order is the floor first, then by org UUID. The method skips `offset` rows
    /// and returns at most `limit` rows. The store applies the page, so a page does not read
    /// every held row.
    async fn list_held(&self, org: Option<Uuid>, limit: u64, offset: u64) -> Result<Vec<HeldVersion>, RepositoryError>;
    /// True when a held or discarded row of the set has this base version, generation and schema
    /// revision (plan H6).
    async fn proposal_exists(&self, key: &ToolPolicySetKey, base_version: u64, compiler_generation: u32, schema_revision: u32) -> Result<bool, RepositoryError>;
    /// Discard the held row of the set, if any, then insert `h` (its status must be `Held`). The
    /// check is a compare-and-set on the state row: `h.version` must be `next_version()` of the
    /// stored state, else `Conflict(ConflictKind::ToolPolicyVersion)`. Sets `last_version` and
    /// leaves `live_version` as it is.
    async fn insert_held_in(&self, tx: &dyn Transaction, h: &HeldVersion) -> Result<(), RepositoryError>;
    /// Set the held row `version` of the set to discarded. `false` when no held row has that
    /// number.
    async fn discard_held_in(&self, tx: &dyn Transaction, key: &ToolPolicySetKey, version: u64) -> Result<bool, RepositoryError>;
    /// The settings of an organization. `None` when the organization has no settings row.
    async fn settings(&self, org: Uuid) -> Result<Option<ToolPolicySettings>, RepositoryError>;
    /// Insert the settings of an organization, or replace them. `s.updated_at` and
    /// `s.updated_by` must be set: a `None` is an error (`Backend`), not a panic.
    /// `first_enabled_at` is written once: a stored non-null value is kept, whatever `s` holds.
    /// `s.webhooks` is ignored. `webhooks` `Some` replaces the stored opt-in list. `None` keeps
    /// it, and a new row gets an empty list. Returns the stored list after the write.
    async fn upsert_settings_in(&self, tx: &dyn Transaction, org: Uuid, s: &ToolPolicySettings, webhooks: Option<Vec<String>>) -> Result<Vec<String>, RepositoryError>;
    /// The stored webhook opt-in list of an organization, read inside `tx` under a row lock
    /// (`SELECT ... FOR UPDATE`). The lock lasts until `tx` ends, so a second settings write for
    /// the same organization waits, and the list that this call returns is the list that the
    /// write of `tx` replaces. An organization with no settings row gives an empty list (and no
    /// row to lock).
    async fn webhooks_for_update_in(&self, tx: &dyn Transaction, org: Uuid) -> Result<Vec<String>, RepositoryError>;
}

/// A lock that lets one re-compile job run at a time across replicas (spec section 8).
#[async_trait]
pub trait RecompileLock: Send + Sync {
    /// `None` when another holder has the lock. Never waits.
    async fn try_acquire(&self) -> Result<Option<Box<dyn RecompileLockGuard>>, RepositoryError>;
}

/// A held re-compile lock. The guard is `Send`, so a spawned task can own it.
///
/// Call `release` on every path. A guard that is only dropped also frees the lock, but that is a
/// backstop: the drop of a database transaction can panic under connection contention.
#[async_trait]
pub trait RecompileLockGuard: Send {
    /// End the lock. An error means the end of the lock is not confirmed.
    async fn release(self: Box<Self>) -> Result<(), RepositoryError>;
}
