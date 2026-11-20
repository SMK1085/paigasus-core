// SPDX-License-Identifier: Apache-2.0

//! The provisional ports of the agent host (spec section 6.2).
//!
//! Each port is a trait with one owner issue. All ports are provisional: the owner may change the
//! methods, not the responsibility. M0 has no adapter for any port.

use async_trait::async_trait;

use crate::agent::ResolvedAgent;
use crate::ids::{InboxId, OutboxMessageId, TimerId};
use crate::inbox::{Inbox, InboxEvent};
use crate::memory::{CapError, MemoryCaps, MemoryCommitSet, MemoryDir, MemoryPath, MemoryScope, MemoryVersion};
use crate::outbox::OutboxMessage;
use crate::run::{CommitError, Committed, RunHandle, RunOutcome};
use crate::timer::Timer;

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
    Inserted,
    /// An event with the same key exists. `payload_matches` tells if the payload is the same.
    Duplicate {
        /// True when the stored payload equals the new payload.
        payload_matches: bool,
    },
}

/// The store of inboxes and their events. Owner: SMA-787.
/// Used outside a run: by ingress and the console.
/// Provisional: the owner may change the methods, not the responsibility.
#[async_trait]
pub trait InboxRepository: Send + Sync {
    /// Read an inbox by its ID.
    async fn get(&self, id: &InboxId) -> Result<Option<Inbox>, RepositoryError>;
    /// Add an event to an inbox. A repeated key gives `Duplicate`.
    async fn insert_event(&self, event: InboxEvent) -> Result<InsertOutcome, RepositoryError>;
}

/// The store of timers. Owner: SMA-788.
/// Used outside a run: by the scheduler and the console. A run adds or cancels a timer only in the
/// buffer of the `RunCommitter`, which writes it in the one commit.
/// Provisional: the owner may change the methods, not the responsibility.
#[async_trait]
pub trait TimerRepository: Send + Sync {
    /// Add a timer.
    async fn insert(&self, timer: Timer) -> Result<(), RepositoryError>;
    /// Cancel a timer.
    async fn cancel(&self, id: &TimerId) -> Result<(), RepositoryError>;
}

/// The scheduler that fires due timers. Owner: SMA-788.
/// Used outside a run.
/// Provisional: the owner may change the methods, not the responsibility.
#[async_trait]
pub trait Scheduler: Send + Sync {
    /// Fire up to `limit` due timers. Returns the number of timers that fired.
    async fn fire_due(&self, limit: u32) -> Result<usize, RepositoryError>;
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
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MemoryApplyError {
    /// The version of a file is not the expected version.
    #[error("memory conflict at {path}")]
    Conflict { path: MemoryPath },
    /// A cap refuses the change set.
    #[error("{0}")]
    CapExceeded(CapError),
    /// Some changes are applied and one failed. Only a store without a transaction returns this.
    #[error("memory change set partly applied: {applied} applied, failed at {failed_at}: {cause}")]
    Partial { applied: usize, failed_at: MemoryPath, cause: Box<MemoryApplyError> },
    /// The store failed.
    #[error("memory store error: {0}")]
    Store(String),
}

/// The memory store of an agent: a versioned file store. Owner: SMA-789.
/// Used inside a run: reads go direct, and writes go to the buffer of the workspace. The
/// `RunCommitter` writes them in the one commit. Used outside a run: the console reads and writes.
/// Every method takes the scope. The org is part of every key and every query.
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
/// There is no `Transient` outcome. A transient failure rolls back and does not commit.
/// A database error is `Store`, never `LeaseLost`. SMA-790 adds an "outcome unknown" case. The
/// shape of the buffered changes is the design of SMA-790 (spec sections 6.2 and 7).
#[async_trait]
pub trait RunCommitter: Send + Sync {
    /// Commit the outcome of a run in one transaction.
    async fn commit(&self, handle: RunHandle, outcome: RunOutcome) -> Result<Committed, CommitError>;
}
