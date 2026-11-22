// SPDX-License-Identifier: Apache-2.0
//! The domain record of one applied tool decision (SMA-755 § 5.1) and the builders that make it
//! from the report entries (§ 5.4). Pure: no I/O, no clock and no entropy. The caller passes the
//! ids, the time and the sequence in through [`RecordHeader`].

mod build;
mod record;
pub mod trace;

pub use build::{MAX_FIELD_BYTES, build_refusal, build_request, build_response, cap, response_org};
pub use record::{
    AppliedEffect, ArgumentDigest, CallFacts, CedarDetail, DecisionDetail, DecisionRecord, FindingRecord, Prevention, RecordBasis, RecordHeader, RecordedAction, ReportedCall, Side, ToolRef,
    derive_max_severity, records_of,
};
