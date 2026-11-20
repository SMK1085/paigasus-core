// SPDX-License-Identifier: Apache-2.0

//! The claim of a batch of events.

use std::time::Duration;

use crate::ids::{RunId, ThreadId};
use crate::inbox::InboxEvent;
use crate::run::LeaseToken;

/// The settings of one claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClaimConfig {
    /// The maximum number of events in one claim.
    pub batch_limit: u32,
    /// The attempts after which an event is dead.
    pub max_attempts: u32,
    /// The time to live of a thread.
    pub thread_ttl: Duration,
}

/// A claimed batch of events. One claim makes one run.
#[derive(Debug, Clone, PartialEq)]
pub struct Claim {
    /// The run that the claim opened.
    pub run: RunId,
    /// The thread of the run.
    pub thread: ThreadId,
    /// The fencing token after the claim.
    pub lease_token: LeaseToken,
    /// True when the claim made a new thread.
    pub fresh_thread: bool,
    /// The claimed events.
    pub events: Vec<InboxEvent>,
}
