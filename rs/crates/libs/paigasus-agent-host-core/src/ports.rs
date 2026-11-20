// SPDX-License-Identifier: Apache-2.0

//! The provisional ports of the agent host (spec section 6.2).
//!
//! Each port is a trait with one owner issue. All ports are provisional: the owner may change the
//! methods, not the responsibility. M0 has no adapter for any port.

use async_trait::async_trait;

use crate::agent::ResolvedAgent;
use crate::ids::{AgentId, InboxId, OutboxMessageId, TimerId};
use crate::inbox::{Inbox, InboxEvent};
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

/// The memory store of an agent. Owner: SMA-789.
/// Used inside a run: reads go direct, and writes go to the buffer. The `RunCommitter` writes them
/// in the one commit. Used outside a run: the console reads and writes.
/// Provisional: the owner may change the methods, not the responsibility.
#[async_trait]
pub trait MemoryStore: Send + Sync {
    /// Read the content at `path`.
    async fn read(&self, agent: &AgentId, path: &str) -> Result<Option<String>, RepositoryError>;
    /// Write the content at `path`.
    async fn write(&self, agent: &AgentId, path: &str, content: String) -> Result<(), RepositoryError>;
    /// List the paths that start with `prefix`.
    async fn list(&self, agent: &AgentId, prefix: &str) -> Result<Vec<String>, RepositoryError>;
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
