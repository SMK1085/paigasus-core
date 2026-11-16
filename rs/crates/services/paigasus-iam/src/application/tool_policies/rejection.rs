// SPDX-License-Identifier: Apache-2.0

//! The rejections of a tool policy write (spec section 7). A rejection is a successful result,
//! not an error: the write stores nothing, and the caller gets every rejection back.
//!
//! [`RejectionReason::code`] is an emission site of the canonical error registry. The
//! `every_rejection_reason_is_declared_in_the_canonical_registry` test below is its guard.

use paigasus_iam_core::tool_policy::Rejection;

/// The machine-readable reason of one rejection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(test, derive(strum::EnumIter))]
pub enum RejectionReason {
    PolicyInvalid,
    ShapeUnsupported,
    LoweringFailed,
    FloorPermit,
    IdInvalid,
    WindowTooLong,
    /// IAM only: a catalog write would break the live tenant set (spec section 4.6).
    CatalogBreaksPolicy,
    /// IAM only: a tool name breaks the grammar, or a write breaks a catalog limit.
    CatalogInvalid,
    /// IAM only, temporary: the compiled set has a history condition, and the gateway cannot
    /// enforce one yet. SMA-753 removes this reason with the check in `sets.rs`.
    HistoryUnsupported,
}

impl RejectionReason {
    /// The reason code on the wire.
    pub fn code(&self) -> &'static str {
        match self {
            Self::PolicyInvalid => "tool-policy-invalid",
            Self::ShapeUnsupported => "tool-policy-shape-unsupported",
            Self::LoweringFailed => "tool-policy-lowering-failed",
            Self::FloorPermit => "tool-policy-floor-permit",
            Self::IdInvalid => "tool-policy-id-invalid",
            Self::WindowTooLong => "tool-policy-window-too-long",
            Self::CatalogBreaksPolicy => "tool-catalog-breaks-policy",
            Self::CatalogInvalid => "tool-catalog-invalid",
            Self::HistoryUnsupported => "tool-policy-history-unsupported",
        }
    }

    /// The reason of a compiler rejection. Only the six compiler reasons parse. A catalog
    /// reason is IAM's own, so the compiler must never send one: it gives `None`, and the
    /// caller reports `tool-policy-compiler-unavailable`.
    pub fn from_wire(s: &str) -> Option<Self> {
        [
            Self::PolicyInvalid,
            Self::ShapeUnsupported,
            Self::LoweringFailed,
            Self::FloorPermit,
            Self::IdInvalid,
            Self::WindowTooLong,
        ]
        .into_iter()
        .find(|reason| reason.code() == s)
    }
}

/// One rejection of a tool policy write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolPolicyRejection {
    pub policy_id: Option<String>,
    pub line: Option<u32>,
    pub column: Option<u32>,
    pub reason: RejectionReason,
    pub message: String,
}

impl ToolPolicyRejection {
    /// A rejection that belongs to no policy and has no location.
    pub fn new(reason: RejectionReason, message: impl Into<String>) -> Self {
        Self {
            policy_id: None,
            line: None,
            column: None,
            reason,
            message: message.into(),
        }
    }

    /// A compiler rejection with its reason parsed. `None` for a reason that is not one of the
    /// six compiler reasons.
    pub fn from_compiler(r: &Rejection) -> Option<Self> {
        Some(Self {
            policy_id: r.policy_id.clone(),
            line: r.line,
            column: r.column,
            reason: RejectionReason::from_wire(&r.reason)?,
            message: r.message.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use paigasus_proto::paigasus::common::v1::ErrorReason;
    use serde_json::Value;
    use std::collections::BTreeSet;
    use strum::IntoEnumIterator;

    #[test]
    fn every_rejection_reason_is_declared_in_the_canonical_registry() {
        let mut seen = 0;
        for reason in RejectionReason::iter() {
            let code = reason.code();
            assert!(
                ErrorReason::from_wire_reason(code).is_some(),
                "RejectionReason::{reason:?} emits {code:?}, which is not declared in common/v1/error.proto"
            );
            seen += 1;
        }
        assert_eq!(seen, 9);
    }

    #[test]
    fn only_the_six_compiler_reasons_parse_from_the_wire() {
        let parsed: Vec<RejectionReason> = RejectionReason::iter().filter(|r| RejectionReason::from_wire(r.code()) == Some(*r)).collect();
        assert_eq!(parsed.len(), 6, "{parsed:?}");
        assert_eq!(RejectionReason::from_wire(RejectionReason::CatalogBreaksPolicy.code()), None);
        assert_eq!(RejectionReason::from_wire(RejectionReason::CatalogInvalid.code()), None);
        assert_eq!(RejectionReason::from_wire(RejectionReason::HistoryUnsupported.code()), None);
        assert_eq!(RejectionReason::from_wire("TOOL-POLICY-INVALID"), None);
        assert_eq!(RejectionReason::from_wire("tool-policy-unknown"), None);
    }

    const TENANT_REJECTED: &str = include_str!("../../../../../../tools/paigasus-policy-compiler/protocol/fixtures/result_tenant_rejected.json");
    const FLOOR_REJECTED: &str = include_str!("../../../../../../tools/paigasus-policy-compiler/protocol/fixtures/result_floor_rejected.json");
    const REASON_CODES: &str = include_str!("../../../../../../tools/paigasus-policy-compiler/protocol/fixtures/reason_codes.json");

    #[test]
    fn every_compiler_reason_in_the_shared_fixture_parses() {
        let mut codes = BTreeSet::new();
        for text in [TENANT_REJECTED, FLOOR_REJECTED] {
            let result: Value = serde_json::from_str(text).expect("a fixture parses");
            for rejection in result["rejections"].as_array().expect("a rejections array") {
                let reason = rejection["reason"].as_str().expect("a reason string");
                assert!(RejectionReason::from_wire(reason).is_some(), "the compiler reason {reason:?} does not parse");
                codes.insert(reason.to_owned());
            }
        }
        let unreached: Value = serde_json::from_str(REASON_CODES).expect("reason_codes.json parses");
        for reason in unreached["unreached_by_any_source"].as_array().expect("an unreached array") {
            let reason = reason.as_str().expect("a reason string");
            assert!(RejectionReason::from_wire(reason).is_some(), "the compiler reason {reason:?} does not parse");
            codes.insert(reason.to_owned());
        }
        let six: BTreeSet<String> = RejectionReason::iter().filter_map(|r| RejectionReason::from_wire(r.code())).map(|r| r.code().to_owned()).collect();
        assert_eq!(codes, six, "the fixtures and reason_codes.json must cover exactly the six compiler codes");
    }
}
