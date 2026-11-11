// SPDX-License-Identifier: Apache-2.0
//! The gateway's tool-call interception core (SMA-749, ADR-0027).
//!
//! A synchronous state machine with no I/O and no async runtime (spec D1). The gateway feeds it
//! upstream bytes, decisions and instants, and performs the [`Action`]s it returns.

mod limits;
mod sse;
mod types;

pub use limits::{Limits, OnFailure};
pub use types::{Basis, BlockCode, Decision, Format, HoldKey, ToolCall, ToolCallKind};
