// SPDX-License-Identifier: Apache-2.0

//! Deferred tool calls and their state machine.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::error::DomainError;
use crate::ids::{CallId, DeferralId, InboxId, RunId};

/// The state of a pending tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PendingToolCallState {
    Open,
    Approved,
    Denied,
    Expired,
    Superseded,
    Resolved,
}

impl PendingToolCallState {
    fn transition(self, allowed: &[PendingToolCallState], to: PendingToolCallState) -> Result<Self, DomainError> {
        if allowed.contains(&self) {
            Ok(to)
        } else {
            Err(DomainError::InvalidTransition { from: self, to })
        }
    }

    /// Open to Approved.
    pub fn approve(self) -> Result<Self, DomainError> {
        self.transition(&[Self::Open], Self::Approved)
    }

    /// Open to Denied.
    pub fn deny(self) -> Result<Self, DomainError> {
        self.transition(&[Self::Open], Self::Denied)
    }

    /// Open to Expired.
    pub fn expire(self) -> Result<Self, DomainError> {
        self.transition(&[Self::Open], Self::Expired)
    }

    /// Open to Superseded.
    pub fn supersede(self) -> Result<Self, DomainError> {
        self.transition(&[Self::Open], Self::Superseded)
    }

    /// Approved, Denied, Expired or Superseded to Resolved.
    pub fn resolve(self) -> Result<Self, DomainError> {
        self.transition(&[Self::Approved, Self::Denied, Self::Expired, Self::Superseded], Self::Resolved)
    }
}

/// A tool call that waits for a decision.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingToolCall {
    pub deferral_id: DeferralId,
    pub call_id: CallId,
    pub tool: String,
    pub args: serde_json::Value,
    pub prompt: String,
    pub handoff_path: Vec<String>,
    pub inbox: InboxId,
    pub run: RunId,
    pub expires_at: Timestamp,
    pub state: PendingToolCallState,
}
