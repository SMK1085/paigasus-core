// SPDX-License-Identifier: Apache-2.0
//! What the two wire protocols share (spec § 4, D12): the `Protocol` trait, verdicts and frames.

use std::collections::BTreeMap;

use bytes::Bytes;
use serde_json::Value;
use uuid::Uuid;

use crate::json::Malformed;
use crate::report::{Blocked, ReportEntry};
use crate::sse::Record;
use crate::types::{BlockCode, Decision, HoldKey, ToolCall};

/// A violation of the canonical assembly form (spec § 4.3). Always a stream-level `on_failure`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Deviation(pub &'static str);

/// The longest tool-call id or name accepted, in bytes. Real ids and names are under 100 bytes. A
/// longer one is `Deviation("long_identifier")`: a completed block or call keeps its name for the
/// stream's life, so an unbounded one would be retained outside the held-byte accounting.
pub(crate) const MAX_IDENTIFIER_BYTES: usize = 1024;

/// What a hold's calls are allowed to do once released.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// Every call allowed: frames are released unchanged.
    AllowAll,
    /// One decision per call, in the hold's call order, each with the decision id of the decider.
    PerCall(Vec<(Decision, Option<Uuid>)>),
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
            Verdict::PerCall(d) => d.iter().any(|(d, _)| !d.is_allow()),
        }
    }

    /// The block of the call in `slot` (its index within the hold), or `None` when allowed. Only
    /// a block that the decider made carries a reference (spec D1).
    pub(crate) fn blocked(&self, slot: usize) -> Option<Blocked> {
        match self {
            Verdict::AllowAll | Verdict::PassThrough => None,
            Verdict::BlockAll(c) => Some(Blocked { code: *c, reference: None }),
            Verdict::PerCall(d) => match d.get(slot) {
                Some((Decision::Block { code }, reference)) => Some(Blocked { code: *code, reference: *reference }),
                Some(_) => None,
                // D18: a missing decision is a block
                None => Some(Blocked {
                    code: BlockCode::DeciderError,
                    reference: None,
                }),
            },
        }
    }

    /// The block code of the call in `slot`, or `None` when allowed.
    pub(crate) fn block_code(&self, slot: usize) -> Option<BlockCode> {
        self.blocked(slot).map(|b| b.code)
    }
}

pub(crate) type Verdicts = BTreeMap<HoldKey, Verdict>;

/// One classified record, waiting in the queue or about to be released.
pub(crate) struct Frame {
    /// The validated event name: the Anthropic event, or the Responses `data.type`. `None` for
    /// OpenAI chat (P-1).
    pub event: Option<&'static str>,
    /// The original payload bytes, re-framed unchanged when nothing in it is blocked (D23).
    pub payload: Vec<u8>,
    /// The parsed payload; `None` only for `[DONE]`.
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
    /// A Responses terminal event (SMA-769 R3). The interceptor completes each listed hold, then
    /// truncates every hold still in [`Protocol::incomplete`], then enqueues the frame. Only
    /// `[DONE]` may follow; the protocol reports every other later record as
    /// [`StepError::AfterEnd`].
    Terminal { frame: Frame, completed: Vec<(HoldKey, Vec<ToolCall>)> },
    /// The end marker; every later record is dropped (D4).
    End(Frame),
    /// An upstream error record (D17).
    UpstreamError(Frame),
}

pub(crate) enum StepError {
    Malformed,
    Deviation(&'static str),
    /// A record after the terminal event (SMA-769 R3): dropped and counted, never a failure.
    AfterEnd,
}

impl From<Malformed> for StepError {
    fn from(_: Malformed) -> Self {
        StepError::Malformed
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
    /// Frames still owed when the stream aborts (D17), emitted before the terminal frame. Only
    /// frames that close what the client already holds; never a queued frame.
    fn closing_on_abort(&mut self, _verdicts: &Verdicts) -> Vec<Bytes> {
        Vec::new()
    }
    /// The bytes sent while a hold is open and nothing else was sent for a while (spec D12).
    fn keep_alive(&self) -> &'static [u8] {
        crate::sse::KEEP_ALIVE
    }
    /// Whether an upstream `error` record is itself the terminal frame of an aborted stream
    /// (spec D17), instead of the configured terminal frame.
    fn upstream_error_is_terminal(&self) -> bool {
        false
    }
    /// (position, name) of the calls known so far for a hold, for reports of a `BlockAll` hold.
    fn known_calls(&self, hold: HoldKey) -> Vec<(u32, String)>;
    /// Pass-through framing, with no classification (P-4).
    fn unframed(&self, rec: Record) -> Bytes;
    /// The argument and raw-entry bytes the protocol still holds, measured exactly. Tests only.
    #[cfg(test)]
    fn retained(&self) -> usize;
}

/// The serialized size of one raw entry, as the decider would see it.
pub(crate) fn raw_len(raw: &Value) -> usize {
    serde_json::to_vec(raw).expect("a Value always serializes").len()
}

/// The serialized size of retained raw entries.
#[cfg(test)]
pub(crate) fn raws_len(raws: &[Value]) -> usize {
    raws.iter().map(raw_len).sum()
}

/// A `PerCall` verdict whose decisions carry no decision id. Tests only.
#[cfg(test)]
pub(crate) fn per_call(d: Vec<Decision>) -> Verdict {
    Verdict::PerCall(d.into_iter().map(|d| (d, None)).collect())
}
