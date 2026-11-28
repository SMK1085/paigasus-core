// SPDX-License-Identifier: Apache-2.0
//! The InvokeTool decision of ADR-0027 (e) (SMA-752). Feature `tool-policy`, off by default.
//!
//! Pure functions only: no I/O, no clock, no random numbers, no global state. This file holds the
//! types, the name normal form and the mapping of tool arguments into a Cedar context. The Cedar
//! types stay inside the crate, so the bindings never get the `cedar-policy` dependency.

mod compile;
mod decide;
pub mod history;
pub(crate) mod input;
mod normalise;
mod optimistic;
mod types;
pub(crate) mod walk;

pub use compile::{ActionGroup, CompileError, FloorPolicies, PolicyOrigin, PolicyRef, ToolPolicies};
pub use decide::{Effect, RESERVED_CONTEXT_NAMES, Verdict, decide, decide_with_slots};
pub use normalise::normalise;
pub use optimistic::{ToolVerdict, decide_tool};
pub use types::{Declared, FORGED_HISTORY, Facts, Findings, HISTORY_TRUNCATED, MIXED_SESSIONS, NAME_COLLISION, RequestInfo, SessionInfo, ToolInput, ToolInvocation, UNPARSED_ARGUMENTS};
pub use walk::parses_strictly;
