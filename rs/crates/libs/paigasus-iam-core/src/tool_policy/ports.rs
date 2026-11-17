// SPDX-License-Identifier: Apache-2.0

//! The port to the policy compiler (spec section 5.2). The adapter in the service crate runs
//! the `paigasus-policy-compiler` binary as a child process.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::model::{StoredTool, ToolEntry, ToolPolicySetKey, ToolPolicySettings, ToolPolicyState, ToolPolicyVersion};
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
    /// One version of a set. Only a live or superseded version is returned.
    async fn version(&self, key: &ToolPolicySetKey, version: u64) -> Result<Option<ToolPolicyVersion>, RepositoryError>;
    /// Make the state row if it is missing, then lock it with `SELECT … FOR UPDATE`.
    async fn lock_state_in(&self, tx: &dyn Transaction, key: &ToolPolicySetKey) -> Result<ToolPolicyState, RepositoryError>;
    /// Upsert and delete catalog entries. Returns the new catalog revision (old + 1).
    async fn apply_catalog_in(&self, tx: &dyn Transaction, org: Uuid, upserts: &[ToolEntry], deletes: &[String], now: DateTime<Utc>) -> Result<u64, RepositoryError>;
    /// Store a new live version. The old live version becomes superseded.
    ///
    /// `v.status` must be `Live`. Any other status is an error (`Backend`), because this method
    /// always writes a live row. `v.version` must be the stored `live_version` plus 1. If it is
    /// not, the method returns `Conflict(ConflictKind::ToolPolicyVersion)` and changes nothing.
    /// That check is a compare-and-set on the state row. It is a backstop. The caller must still
    /// call `lock_state_in` first: the row lock is what makes writers run one after the other.
    async fn insert_live_version_in(&self, tx: &dyn Transaction, v: &ToolPolicyVersion) -> Result<(), RepositoryError>;
    /// The settings of an organization. `None` when the organization has no settings row.
    async fn settings(&self, org: Uuid) -> Result<Option<ToolPolicySettings>, RepositoryError>;
    /// Insert the settings of an organization, or replace them. `s.updated_at` and
    /// `s.updated_by` must be set: a `None` is an error (`Backend`), not a panic.
    /// `first_enabled_at` is written once: a stored non-null value is kept, whatever `s` holds.
    async fn upsert_settings_in(&self, tx: &dyn Transaction, org: Uuid, s: &ToolPolicySettings) -> Result<(), RepositoryError>;
}
