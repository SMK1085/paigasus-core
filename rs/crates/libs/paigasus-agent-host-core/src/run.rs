// SPDX-License-Identifier: Apache-2.0

//! Runs, leases and run outcomes.

use std::time::Duration;

use crate::ids::{EventId, InboxId, RunId};
use crate::memory::MemoryCommitSet;
use crate::ports::MemoryApplyError;

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
    Screened,
    Discarded,
}

impl RunStatus {
    /// The text of the status in the `status` column of the run table.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Deferred => "deferred",
            Self::Stopped => "stopped",
            Self::Failed => "failed",
            Self::Screened => "screened",
            Self::Discarded => "discarded",
        }
    }
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

/// The token usage of a run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RunUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_input_tokens: u64,
    pub reasoning_tokens: u64,
}

/// One row to append to the session of a run.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionRow {
    pub kind: String,
    pub payload: serde_json::Value,
}

/// An event that returns to the inbox after a delay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventDelay {
    pub event: EventId,
    pub delay: Duration,
}

/// How a run ended, as the committer records it.
#[derive(Debug, Clone, PartialEq)]
pub enum RunClass {
    Completed { output: Option<String>, usage: RunUsage },
    Stopped { output: Option<String>, reason: String, usage: RunUsage },
    Deterministic { error: String, retry: Vec<EventDelay> },
}

/// Everything that one run commit writes.
#[derive(Debug, Clone, PartialEq)]
pub struct RunRecord {
    pub run: RunId,
    pub token: LeaseToken,
    pub passed: Vec<EventId>,
    pub class: RunClass,
    pub session: Vec<SessionRow>,
    pub nonce: String,
    pub close_thread: bool,
    pub memory: MemoryCommitSet,
}

/// An event that the screen flagged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlaggedEvent {
    pub event: EventId,
    pub detector: String,
    pub reason: String,
}

impl FlaggedEvent {
    /// The `last_error` text of the event.
    pub fn last_error(&self) -> String {
        format!("screened: {}: {}", self.detector, self.reason)
    }
}

/// The events that the screen flagged in one run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenOut {
    pub run: RunId,
    pub token: LeaseToken,
    pub flagged: Vec<FlaggedEvent>,
    /// True when every event of the run is flagged.
    pub all: bool,
}

/// The result of `RunCommitter::screen_out`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenResult {
    /// Every event is flagged. The run is closed.
    AllScreened,
    /// Some events remain. The run continues with them.
    Continue,
}

/// A run that goes back to the inbox with a delay for its events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackOff {
    pub run: RunId,
    pub token: LeaseToken,
    pub error: String,
    pub retry: Vec<EventDelay>,
}

/// The result of a commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Committed {
    pub run: RunId,
    pub done: Vec<EventId>,
}

/// A commit failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CommitError {
    /// The lease token is not the current token.
    #[error("the lease is lost")]
    LeaseLost,
    /// The connection broke during the commit. The store may or may not have committed.
    #[error("the commit outcome is unknown")]
    OutcomeUnknown,
    /// The memory store refused the change set.
    #[error("memory refused: {0}")]
    MemoryRejected(MemoryApplyError),
    /// The store failed.
    #[error("store error: {0}")]
    Store(String),
}

/// The prefix of a `CommitError::Store` text for a text that the store cannot encode.
pub const UNENCODABLE_PREFIX: &str = "unencodable text: ";

impl CommitError {
    /// True when the store refused the text as not encodable.
    pub fn is_unencodable(&self) -> bool {
        matches!(self, Self::Store(text) if text.starts_with(UNENCODABLE_PREFIX))
    }
}

/// Replace every U+0000 with U+FFFD. Postgres `text` cannot hold U+0000.
pub fn replace_nul(text: &str) -> String {
    text.replace('\u{0}', "\u{FFFD}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::MemoryPath;
    use uuid::Uuid;

    #[test]
    fn run_status_texts_are_the_column_values() {
        use RunStatus::*;
        let all = [Running, Completed, Deferred, Stopped, Failed, Screened, Discarded];
        let texts: Vec<_> = all.iter().map(RunStatus::as_str).collect();
        assert_eq!(texts, ["running", "completed", "deferred", "stopped", "failed", "screened", "discarded"]);
    }

    #[test]
    fn a_memory_rejection_text_holds_no_path() {
        let path = MemoryPath::parse("/people/secret-name.md").unwrap();
        let e = CommitError::MemoryRejected(MemoryApplyError::Conflict { path });
        assert_eq!(e.to_string(), "memory refused: memory conflict");
    }

    #[test]
    fn only_the_unencodable_prefix_is_unencodable() {
        assert!(CommitError::Store(format!("{UNENCODABLE_PREFIX}22P05")).is_unencodable());
        assert!(!CommitError::Store("connection reset".into()).is_unencodable());
        assert!(!CommitError::LeaseLost.is_unencodable());
    }

    #[test]
    fn replace_nul_changes_only_nul() {
        assert_eq!(replace_nul("a\u{0}b"), "a\u{FFFD}b");
        assert_eq!(replace_nul("plain <x>"), "plain <x>");
    }

    #[test]
    fn a_flagged_event_names_its_detector_in_last_error() {
        let f = FlaggedEvent {
            event: EventId::from_uuid(Uuid::from_u128(1)),
            detector: "pii".into(),
            reason: "an email".into(),
        };
        assert_eq!(f.last_error(), "screened: pii: an email");
    }
}
