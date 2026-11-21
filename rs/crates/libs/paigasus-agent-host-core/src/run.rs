// SPDX-License-Identifier: Apache-2.0

//! Runs, leases and run outcomes.

use crate::deferral::PendingToolCall;
use crate::ids::{EventId, InboxId, RunId};

/// The fencing token of a lease.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LeaseToken(pub i64);

/// The status of a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunStatus {
    Running,
    Completed,
    Deferred,
    Stopped,
    Failed,
}

/// A run of an agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    pub id: RunId,
    pub inbox: InboxId,
    pub token: LeaseToken,
    pub events: Vec<EventId>,
    pub status: RunStatus,
    pub final_output: Option<String>,
}

/// A handle to a run that holds a lease.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunHandle {
    pub inbox: InboxId,
    pub run: RunId,
    pub token: LeaseToken,
}

/// The outcome of a run.
#[derive(Debug, Clone, PartialEq)]
pub enum RunOutcome {
    Completed { final_output: Option<String> },
    Deferred { calls: Vec<PendingToolCall> },
    Stopped { reason: String },
    Failed { error: String },
}

/// The result of a commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Committed {
    pub run: RunId,
}

/// A commit failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CommitError {
    /// The lease token is not the current token.
    #[error("the lease is lost")]
    LeaseLost,
    /// The store failed.
    #[error("store error: {0}")]
    Store(String),
}
