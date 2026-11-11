// SPDX-License-Identifier: Apache-2.0
//! What the two wire protocols share (spec § 4, D12). Task 6 adds the `Protocol` trait.
#![allow(dead_code)] // Removed in Task 8, when the interceptor uses every item.

/// A violation of the canonical assembly form (spec § 4.3). Always a stream-level `on_failure`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Deviation(pub &'static str);

use std::collections::BTreeMap;

use bytes::Bytes;
use serde_json::Value;

use crate::json::Malformed;
use crate::report::ReportEntry;
use crate::sse::Record;
use crate::types::{BlockCode, Decision, HoldKey, ToolCall};

/// What a hold's calls are allowed to do once released.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// Every call allowed: frames are released unchanged.
    AllowAll,
    /// One decision per call, in the hold's call order.
    PerCall(Vec<Decision>),
    /// Every call of the hold blocked, including calls not assembled yet.
    BlockAll(BlockCode),
    /// Observe mode: released unchanged and not enforced (§ 5.3).
    PassThrough,
}

impl Verdict {
    pub(crate) fn blocks_any(&self) -> bool {
        match self {
            Verdict::AllowAll | Verdict::PassThrough => false,
            Verdict::BlockAll(_) => true,
            Verdict::PerCall(d) => d.iter().any(|d| !d.is_allow()),
        }
    }

    /// The block code of the call in `slot` (its index within the hold), or `None` when allowed.
    pub(crate) fn block_code(&self, slot: usize) -> Option<BlockCode> {
        match self {
            Verdict::AllowAll | Verdict::PassThrough => None,
            Verdict::BlockAll(c) => Some(*c),
            Verdict::PerCall(d) => match d.get(slot) {
                Some(Decision::Block { code }) => Some(*code),
                Some(_) => None,
                None => Some(BlockCode::DeciderError), // D18: a missing decision is a block
            },
        }
    }
}

pub(crate) type Verdicts = BTreeMap<HoldKey, Verdict>;

/// One classified record, waiting in the queue or about to be released.
pub(crate) struct Frame {
    /// The validated Anthropic event name; `None` for OpenAI (P-1).
    pub event: Option<&'static str>,
    /// The original payload bytes, re-framed unchanged when nothing in it is blocked (D23).
    pub payload: Vec<u8>,
    /// The parsed payload; `None` only for OpenAI's `[DONE]`.
    pub value: Option<Value>,
    /// The holds this frame carries data of (D12). Sorted and without duplicates.
    pub touches: Vec<HoldKey>,
}

pub(crate) enum Step {
    Frame {
        frame: Frame,
        opened: Vec<HoldKey>,
        completed: Vec<(HoldKey, Vec<ToolCall>)>,
        reports: Vec<ReportEntry>,
    },
    /// The end marker; every later record is dropped (D4).
    End(Frame),
    /// An upstream error record (D17).
    UpstreamError(Frame),
}

pub(crate) enum StepError {
    Malformed(&'static str),
    Deviation(&'static str),
}

impl From<Malformed> for StepError {
    fn from(m: Malformed) -> Self {
        StepError::Malformed(m.0)
    }
}

impl From<Deviation> for StepError {
    fn from(d: Deviation) -> Self {
        StepError::Deviation(d.0)
    }
}

/// One wire protocol. The interceptor owns ordering, limits and failure handling; a protocol owns
/// classification, assembly and rewriting.
pub(crate) trait Protocol: Send {
    fn step(&mut self, rec: Record, verdicts: &Verdicts) -> Result<Step, StepError>;
    /// Holds that opened and did not complete. At the end of the stream these are truncated.
    fn incomplete(&self) -> Vec<HoldKey>;
    /// The bytes for one released frame under the verdicts. May be empty (a dropped frame) or
    /// several frames (a notice first).
    fn release(&mut self, frame: Frame, verdicts: &Verdicts) -> Vec<Bytes>;
    /// Notices still owed at the end of a stream that has no end marker.
    fn closing(&mut self, verdicts: &Verdicts) -> Vec<Bytes>;
    /// (position, name) of the calls known so far for a hold, for reports of a `BlockAll` hold.
    fn known_calls(&self, hold: HoldKey) -> Vec<(u32, String)>;
    /// Pass-through framing, with no classification (P-4).
    fn unframed(&self, rec: Record) -> Bytes;
}
