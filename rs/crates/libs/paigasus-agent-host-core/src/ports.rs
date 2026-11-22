// SPDX-License-Identifier: Apache-2.0

//! The provisional ports of the agent host (spec section 6.2).
//!
//! Each port is a trait with one owner issue. All ports are provisional: the owner may change the
//! methods, not the responsibility. M0 has no adapter for any port.

use async_trait::async_trait;

use crate::agent::ResolvedAgent;
use std::time::Duration;

use jiff::Timestamp;
use uuid::Uuid;

use crate::claim::{Claim, ClaimConfig};
pub use crate::fault::FaultPoint;
use crate::ids::{EventId, InboxId, OutboxMessageId, TimerId};
use crate::inbox::{Inbox, NewInboxEvent};
use crate::lease::{ClaimError, HeartbeatError, LeaseHandle};
use crate::memory::{CapError, MemoryCaps, MemoryCommitSet, MemoryDir, MemoryPath, MemoryScope, MemoryVersion};
use crate::outbox::OutboxMessage;
use crate::run::{BackOff, CommitError, Committed, RunRecord, ScreenOut, ScreenResult};
pub use crate::screen::EventScreen;
use crate::timer::{TimerOrigin, TimerView};

/// The error of a repository port.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RepositoryError {
    /// The record does not exist.
    #[error("not found")]
    NotFound,
    /// The store failed. The text is the cause.
    #[error("store error: {0}")]
    Store(String),
}

/// The result of an event insert.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InsertOutcome {
    /// The store added the event.
    Inserted {
        /// The ID of the new event.
        id: EventId,
    },
    /// An event with the same key exists. `payload_matches` tells if the payload is the same.
    Duplicate {
        /// The ID of the stored event.
        id: EventId,
        /// True when the stored payload equals the new payload.
        payload_matches: bool,
    },
}

/// The error of a lease or claim store call. The text is the cause.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("store error: {0}")]
pub struct StoreError(pub String);

/// The store of inboxes and their events. Owner: SMA-787.
/// Used outside a run: by ingress and the console.
/// Provisional: the owner may change the methods, not the responsibility.
#[async_trait]
pub trait InboxRepository: Send + Sync {
    /// Read an inbox by the UUID of its ID.
    async fn get_by_uuid(&self, id: Uuid) -> Result<Option<Inbox>, RepositoryError>;
    /// Add an event to an inbox. A repeated key gives `Duplicate`.
    async fn insert_event(&self, event: NewInboxEvent) -> Result<InsertOutcome, RepositoryError>;
}

/// The store of inbox leases. Owner: SMA-787.
/// Provisional: the owner may change the methods, not the responsibility.
#[async_trait]
pub trait LeaseRepository: Send + Sync {
    /// Acquire the lease of the next inbox that has work. Returns `None` when no inbox is free.
    async fn acquire_next(&self, holder: &str, ttl: Duration) -> Result<Option<LeaseHandle>, StoreError>;
    /// Acquire the lease of one inbox. Returns `None` when another holder has it.
    async fn acquire(&self, inbox: &InboxId, holder: &str, ttl: Duration) -> Result<Option<LeaseHandle>, StoreError>;
    /// Extend the lease. A poisoned handle gives `LeaseLost` with no SQL.
    async fn heartbeat(&self, handle: &LeaseHandle, ttl: Duration) -> Result<(), HeartbeatError>;
    /// Release the lease.
    async fn release(&self, handle: LeaseHandle) -> Result<(), StoreError>;
}

/// The store of claims. Owner: SMA-787.
/// A claim ends through `RunCommitter` (SMA-790 §6.8), which replaced `complete`.
/// Provisional: the owner may change the methods, not the responsibility.
#[async_trait]
pub trait ClaimRepository: Send + Sync {
    /// Claim a batch of events. Returns `None` when there is nothing to claim.
    async fn claim(&self, handle: &mut LeaseHandle, cfg: &ClaimConfig) -> Result<Option<Claim>, ClaimError>;
}

/// The read side of the timer store. Owner: SMA-788.
/// Use it outside a run only. A run writes a timer through `TimerWorkspace` and the run commit.
/// Provisional: the owner may change the methods, not the responsibility.
#[async_trait]
pub trait TimerRepository: Send + Sync {
    /// List the active timers of an inbox with one origin, ordered by `next_fire_at`.
    async fn list_active(&self, inbox: &InboxId, origin: TimerOrigin) -> Result<Vec<TimerView>, RepositoryError>;
    /// Read one timer of an inbox. A timer of another inbox gives `None`.
    async fn get(&self, inbox: &InboxId, id: TimerId) -> Result<Option<TimerView>, RepositoryError>;
    /// The IDs of the active timers that the agent made in an inbox.
    async fn active_agent_ids(&self, inbox: &InboxId) -> Result<Vec<TimerId>, RepositoryError>;
    /// The number of timers that the agent made in an inbox after `since`.
    async fn count_agent_created_since(&self, inbox: &InboxId, since: Timestamp) -> Result<u32, RepositoryError>;
}

/// The scheduler that fires due timers. Owner: SMA-788.
/// Used outside a run.
/// Provisional: the owner may change the methods, not the responsibility.
#[async_trait]
pub trait Scheduler: Send + Sync {
    /// Process up to `limit` due timers. Returns the number of processed rows, backed-off and
    /// failed rows included.
    async fn fire_due(&self, limit: u32) -> Result<usize, RepositoryError>;
    /// The database `now` minus the oldest due active `next_fire_at`, or zero.
    async fn due_lag(&self) -> Result<Duration, RepositoryError>;
}

/// The clock of a run. The host gives the time; a run never reads the system clock itself.
pub trait RunClock: Send + Sync {
    /// The current time.
    fn now(&self) -> Timestamp;
}

/// The source of new UUIDs.
pub trait IdSource: Send + Sync {
    /// Make a new UUID.
    fn new_uuid(&self) -> Uuid;
}

/// A file as the store returns it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryFile {
    /// The content of the file.
    pub content: String,
    /// The version of the file.
    pub version: MemoryVersion,
}

/// One file in a listing. The listing has no content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryEntry {
    /// The path of the file.
    pub path: MemoryPath,
    /// The size of the content in UTF-8 bytes.
    pub size_bytes: u64,
    /// The version of the file.
    pub version: MemoryVersion,
}

/// The size of a scope: the number of files and the total content bytes.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct MemoryUsage {
    /// The number of files.
    pub files: u64,
    /// The total size of the contents in UTF-8 bytes.
    pub bytes: u64,
}

/// A store error that is not a refusal of a change.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MemoryStoreError {
    /// The store failed.
    #[error("memory store error: {0}")]
    Store(String),
}

/// The refusal or failure of `MemoryStore::apply`.
///
/// The `Display` text is for a log line. It holds no path of a principal and no cause (spec
/// §6.13). The path and the cause stay in the fields. The model texts are on `CapError`,
/// `WorkspaceError` and `MemoryCommandError`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MemoryApplyError {
    /// The version of a file is not the expected version.
    #[error("memory conflict")]
    Conflict { path: MemoryPath },
    /// A cap refuses the change set.
    #[error("memory cap exceeded")]
    CapExceeded(CapError),
    /// Some changes are applied and one failed. Only a store without a transaction returns this.
    #[error("memory change set partly applied: {applied} applied")]
    Partial { applied: usize, failed_at: MemoryPath, cause: Box<MemoryApplyError> },
    /// The store failed. The text is the cause.
    #[error("memory store error")]
    Store(String),
}

/// The memory store of an agent: a versioned file store. Owner: SMA-789.
/// Used inside a run: reads go direct, and writes go to the buffer of the workspace. The
/// `RunCommitter` writes them in the one commit. Used outside a run: the console reads and writes.
/// Every method takes the scope. The org is part of every key and every query.
///
/// Every write is expected to go through `MemoryWorkspace`, which checks the paths, the caps and
/// the content, and refuses a NUL character with a model text. An adapter refuses content with a
/// NUL character in `apply` with `MemoryApplyError::Store`, as Postgres `text` does.
#[async_trait]
pub trait MemoryStore: Send + Sync {
    /// Read a file. It returns `None` when the file does not exist.
    async fn get(&self, scope: &MemoryScope, path: &MemoryPath) -> Result<Option<MemoryFile>, MemoryStoreError>;
    /// List every file under `dir`, at any depth, in byte order of the path.
    async fn list(&self, scope: &MemoryScope, dir: &MemoryDir) -> Result<Vec<MemoryEntry>, MemoryStoreError>;
    /// The usage of a scope.
    async fn usage(&self, scope: &MemoryScope) -> Result<MemoryUsage, MemoryStoreError>;
    /// Apply a change set. The set is all or nothing, except for a store that returns `Partial`.
    async fn apply(&self, set: &MemoryCommitSet, caps: &MemoryCaps) -> Result<(), MemoryApplyError>;
    /// Remove every file of a scope.
    async fn delete_scope(&self, scope: &MemoryScope) -> Result<(), MemoryStoreError>;
}

/// The outbox of messages that leave the host. Owner: SMA-792.
/// This is the read side of the durable outbound messages. Used outside a run: by the dispatcher.
/// A run adds a message only in the buffer of the `RunCommitter`.
/// Provisional: the owner may change the methods, not the responsibility.
#[async_trait]
pub trait Outbox: Send + Sync {
    /// Read the next batch of unsent messages, up to `limit`.
    async fn next_batch(&self, limit: u32) -> Result<Vec<OutboxMessage>, RepositoryError>;
    /// Mark a message as sent.
    async fn mark_sent(&self, id: &OutboxMessageId) -> Result<(), RepositoryError>;
}

/// The resolver that finds the agent of an inbox. Owner: SMA-790.
/// Used inside a run: read only, at the start of the run.
/// Provisional: the owner may change the methods, not the responsibility.
#[async_trait]
pub trait AgentResolver: Send + Sync {
    /// Resolve the agent for an inbox.
    async fn resolve(&self, inbox: &InboxId) -> Result<ResolvedAgent, RepositoryError>;
}

/// The committer of a run outcome. Owner: SMA-790.
/// Used inside a run: this is the only writer.
/// Provisional: the owner may change the methods, not the responsibility.
///
/// The port has four methods. `commit` writes a run outcome. `screen_out` closes the events that
/// the screen flagged. `back_off` returns the events to the inbox with a delay.
/// `release_after_shutdown` gives up the lease when the worker stops.
///
/// There is no `Transient` outcome. A transient failure rolls back and does not commit.
/// A database error is `Store`, never `LeaseLost`. Only `CommitError::LeaseLost` counts as a lost
/// lease. `OutcomeUnknown` means that the store may have committed.
#[async_trait]
pub trait RunCommitter: Send + Sync {
    /// Commit the outcome of a run in one transaction.
    async fn commit(&self, h: &mut LeaseHandle, r: RunRecord) -> Result<Committed, CommitError>;
    /// Close the events that the screen flagged.
    async fn screen_out(&self, h: &mut LeaseHandle, s: ScreenOut) -> Result<ScreenResult, CommitError>;
    /// Return the events of a failed run to the inbox with a delay.
    async fn back_off(&self, h: &mut LeaseHandle, b: BackOff) -> Result<(), CommitError>;
    /// Release the lease after a shutdown.
    async fn release_after_shutdown(&self, h: LeaseHandle) -> Result<(), CommitError>;
}

/// The time zone name is not in the time zone database.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TzError {
    #[error("the time zone {name} is not known")]
    Unknown { name: String },
}

/// Resolves an IANA time zone name. Timer code gets every zone through this port. It never reads
/// the global time zone database of `jiff` (spec section 5.1).
pub trait TzResolver: Send + Sync {
    /// Return the zone of `name`, or `TzError::Unknown`.
    fn resolve(&self, name: &str) -> Result<jiff::tz::TimeZone, TzError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::MemoryPath;

    #[test]
    fn no_memory_apply_error_display_holds_the_path() {
        let path = MemoryPath::parse("/people/secret-name.md").unwrap();
        let cap = CapError::File {
            path: path.clone(),
            size: 10,
            limit: 1,
        };
        let errors = [
            MemoryApplyError::Conflict { path: path.clone() },
            MemoryApplyError::CapExceeded(cap),
            MemoryApplyError::Partial {
                applied: 2,
                failed_at: path.clone(),
                cause: Box::new(MemoryApplyError::Conflict { path: path.clone() }),
            },
            MemoryApplyError::Store(format!("db error at {path}")),
        ];
        let texts: Vec<String> = errors.iter().map(ToString::to_string).collect();
        assert_eq!(texts, ["memory conflict", "memory cap exceeded", "memory change set partly applied: 2 applied", "memory store error"]);
        for text in &texts {
            assert!(!text.contains("secret-name"), "{text}");
        }
    }
}
