// SPDX-License-Identifier: Apache-2.0
//! Names, notices and decision reports (spec § 4.5, § 4.6, D21, D22).
//!
//! Tool names come from the model, so they are hostile (T1). They reach a notice, a report and a
//! header only after [`sanitize_name`]. The notice text is built from a closed set (the block code
//! and the sanitized name), never from argument or detector content: the model reads the notice
//! on its next turn, so echoing content would reflect attacker text into the prompt (D21).

use bytes::Bytes;
use sha2::{Digest, Sha256};

use crate::sse;
use crate::types::{Basis, BlockCode, ToolCall};

const MAX_NAME_BYTES: usize = 128;
const MAX_HEADER_BYTES: usize = 2048;
const TRUNCATED: &str = r#"{"truncated":true}"#;

/// The first 128 bytes of `name`, every byte outside `[A-Za-z0-9_.:-]` replaced by `_`.
#[must_use]
pub(crate) fn sanitize_name(name: &str) -> String {
    name.bytes()
        .take(MAX_NAME_BYTES)
        .map(|b| if b.is_ascii_alphanumeric() || b"_.:-".contains(&b) { b as char } else { '_' })
        .collect()
}

/// The first 4 bytes of SHA-256 over the raw name, as 8 lower-case hex digits.
#[must_use]
pub(crate) fn name_sha256_8(name: &str) -> String {
    Sha256::digest(name.as_bytes())[..4].iter().map(|b| format!("{b:02x}")).collect()
}

/// The fixed-shape notice for a blocked call.
#[must_use]
pub fn notice_line(name: &str, code: BlockCode) -> String {
    format!("[paigasus] tool call \"{}\" was blocked ({}).", sanitize_name(name), code.as_kebab())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportDecision {
    Allow,
    Block,
    /// A provider-executed tool (§ 4.2): reported, never decided (P-6).
    Unenforceable,
}

impl ReportDecision {
    fn as_str(self) -> &'static str {
        match self {
            ReportDecision::Allow => "allow",
            ReportDecision::Block => "block",
            ReportDecision::Unenforceable => "unenforceable",
        }
    }
}

/// The registry reason a block code reports (SMA-752 D10), or `None` when it has none.
#[must_use]
pub fn reason_label(code: BlockCode) -> Option<&'static str> {
    match code {
        BlockCode::Policy => Some("tool-call-denied"),
        BlockCode::PolicyUnavailable => Some("tool-policy-unavailable"),
        BlockCode::Detector | BlockCode::HoldLimit | BlockCode::Malformed | BlockCode::Truncated | BlockCode::DeciderError => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportEntry {
    pub choice: u32,
    pub position: u32,
    /// The raw name. Serialized only through `sanitize_name` and `name_sha256_8`.
    pub name: String,
    pub decision: ReportDecision,
    pub basis: Option<Basis>,
    pub code: Option<BlockCode>,
    /// `Some(true)` for a block, `Some(false)` for an observed call, `None` otherwise.
    pub enforced: Option<bool>,
    /// The registry reason for `code`, when it has one.
    pub reason: Option<&'static str>,
}

impl ReportEntry {
    /// The § 4.6 schema, version 2. Every field is ASCII by construction.
    #[must_use]
    pub fn to_json(&self) -> String {
        serde_json::json!({
            "v": 2,
            "side": "response",
            "choice": self.choice,
            "position": self.position,
            "name": sanitize_name(&self.name),
            "name_sha256_8": name_sha256_8(&self.name),
            "decision": self.decision.as_str(),
            "basis": self.basis.map(Basis::as_str),
            "code": self.code.map(BlockCode::as_kebab),
            "enforced": self.enforced,
            "reason": self.reason,
        })
        .to_string()
    }
}

/// What the gateway did to the request (SMA-752).
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestAction {
    RemoveDefinition,
    ReplaceResult,
    ResetToolChoice,
}

impl RequestAction {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            RequestAction::RemoveDefinition => "remove-definition",
            RequestAction::ReplaceResult => "replace-result",
            RequestAction::ResetToolChoice => "reset-tool-choice",
        }
    }
}

/// Why the gateway changed the request.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestReason {
    ToolCallDenied,
    ToolResultBlocked,
}

impl RequestReason {
    /// Every reason, for tests that must cover them all.
    pub const ALL: [RequestReason; 2] = [RequestReason::ToolCallDenied, RequestReason::ToolResultBlocked];

    /// The registry reason string.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            RequestReason::ToolCallDenied => "tool-call-denied",
            RequestReason::ToolResultBlocked => "tool-result-blocked",
        }
    }
}

/// One change to the request, for the report (P-11).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestEntry {
    /// The raw tool name. Serialized only through `sanitize_name` and `name_sha256_8`.
    pub tool: String,
    pub action: RequestAction,
    pub enforced: bool,
    pub reason: RequestReason,
}

impl RequestEntry {
    #[must_use]
    pub fn new(tool: String, action: RequestAction, enforced: bool, reason: RequestReason) -> Self {
        RequestEntry { tool, action, enforced, reason }
    }

    /// The request-side report entry.
    #[must_use]
    pub fn to_json(&self) -> String {
        serde_json::json!({
            "v": 2,
            "side": "request",
            "tool": sanitize_name(&self.tool),
            "name_sha256_8": name_sha256_8(&self.tool),
            "action": self.action.as_str(),
            "enforced": self.enforced,
            "reason": self.reason.as_str(),
        })
        .to_string()
    }
}

fn omitted_summary(n: usize) -> String {
    serde_json::json!({"v": 2, "side": "request", "omitted": n}).to_string()
}

/// The `paigasus-tool-decisions` header value (D22). Call entries come first. Request entries
/// follow and are cut first when the header is full, with one summary entry.
#[must_use]
pub fn report_header(calls: &[ReportEntry], requests: &[RequestEntry]) -> String {
    // Room kept at the end for the cut marker and the closing bracket.
    let reserve = if requests.is_empty() {
        TRUNCATED.len()
    } else {
        TRUNCATED.len().max(omitted_summary(requests.len()).len())
    };
    let mut out = String::from("[");
    for (i, e) in calls.iter().enumerate() {
        let json = e.to_json();
        let sep = usize::from(i > 0);
        if out.len() + sep + json.len() + 1 + reserve + 1 > MAX_HEADER_BYTES {
            if i > 0 {
                out.push(',');
            }
            out.push_str(TRUNCATED);
            out.push(']');
            return out;
        }
        if i > 0 {
            out.push(',');
        }
        out.push_str(&json);
    }
    for (i, r) in requests.iter().enumerate() {
        let json = r.to_json();
        let sep = usize::from(!out.ends_with('['));
        if out.len() + sep + json.len() + 1 + reserve + 1 > MAX_HEADER_BYTES {
            if sep == 1 {
                out.push(',');
            }
            out.push_str(&omitted_summary(requests.len() - i));
            out.push(']');
            return out;
        }
        if sep == 1 {
            out.push(',');
        }
        out.push_str(&json);
    }
    out.push(']');
    debug_assert!(out.is_ascii());
    out
}

/// One `: paigasus-decision` SSE comment per request entry.
#[must_use]
pub fn request_comments(requests: &[RequestEntry]) -> Vec<Bytes> {
    requests.iter().map(|r| sse::comment(&format!("paigasus-decision {}", r.to_json()))).collect()
}

/// The report entry for one call. The name is the qualified `display_name()`. A `Block` is
/// enforced. An `Allow` that carries a code is an observed call and is not enforced.
pub(crate) fn report_entry(c: &ToolCall, decision: ReportDecision, basis: Basis, code: Option<BlockCode>) -> ReportEntry {
    let enforced = match (decision, code) {
        (ReportDecision::Block, _) => Some(true),
        (ReportDecision::Allow, Some(_)) => Some(false),
        _ => None,
    };
    ReportEntry {
        choice: c.choice,
        position: c.position,
        name: c.display_name(),
        decision,
        basis: Some(basis),
        code,
        enforced,
        reason: code.and_then(reason_label),
    }
}

/// One `: paigasus-decision` SSE comment (D22).
pub(crate) fn report_comment(entry: &ReportEntry) -> Bytes {
    sse::comment(&format!("paigasus-decision {}", entry.to_json()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str) -> ReportEntry {
        ReportEntry {
            choice: 0,
            position: 1,
            name: name.to_owned(),
            decision: ReportDecision::Block,
            basis: Some(Basis::Decider),
            code: Some(BlockCode::Policy),
            enforced: Some(true),
            reason: Some("tool-call-denied"),
        }
    }

    fn call_entry(name: &str) -> ReportEntry {
        entry(name)
    }

    fn many_request_entries(n: usize) -> Vec<RequestEntry> {
        (0..n)
            .map(|i| RequestEntry::new(format!("tool_{i}"), RequestAction::RemoveDefinition, true, RequestReason::ToolCallDenied))
            .collect()
    }

    fn tc() -> ToolCall {
        ToolCall {
            choice: 0,
            position: 0,
            id: None,
            name: "x".to_owned(),
            namespace: None,
            kind: crate::types::ToolCallKind::Function,
            arguments: String::new(),
        }
    }

    #[test]
    fn names_are_sanitized_and_truncated() {
        assert_eq!(sanitize_name("read_file.v2:x-y"), "read_file.v2:x-y");
        assert_eq!(sanitize_name("rm -rf /"), "rm_-rf__");
        assert_eq!(sanitize_name("é"), "__"); // two UTF-8 bytes, two underscores
        assert_eq!(sanitize_name(&"a".repeat(300)).len(), 128);
    }

    #[test]
    fn the_hash_tells_names_apart_that_sanitize_alike() {
        assert_eq!(sanitize_name("a b"), sanitize_name("a\tb"));
        assert_ne!(name_sha256_8("a b"), name_sha256_8("a\tb"));
        assert_eq!(name_sha256_8("").len(), 8);
        assert_eq!(name_sha256_8(""), "e3b0c442"); // SHA-256 of the empty string starts e3b0c442
    }

    #[test]
    fn the_notice_has_the_fixed_shape() {
        assert_eq!(notice_line("send mail", BlockCode::HoldLimit), "[paigasus] tool call \"send_mail\" was blocked (hold-limit).");
    }

    #[test]
    fn an_entry_serializes_to_the_schema() {
        let v: serde_json::Value = serde_json::from_str(&entry("x y").to_json()).unwrap();
        assert_eq!(
            v,
            serde_json::json!({"v":2,"side":"response","choice":0,"position":1,"name":"x_y","name_sha256_8":name_sha256_8("x y"),"decision":"block","basis":"decider","code":"policy","enforced":true,"reason":"tool-call-denied"})
        );
        let u = ReportEntry {
            decision: ReportDecision::Unenforceable,
            basis: None,
            code: None,
            enforced: None,
            reason: None,
            ..entry("web_search")
        };
        let v: serde_json::Value = serde_json::from_str(&u.to_json()).unwrap();
        assert_eq!(v["decision"], "unenforceable");
        assert!(v["basis"].is_null() && v["code"].is_null());
    }

    #[test]
    fn the_header_is_ascii_and_capped() {
        let one = report_header_calls(&[entry("é")]);
        assert!(one.is_ascii());
        assert!(one.starts_with('[') && one.ends_with(']'));
        let many: Vec<ReportEntry> = (0..200).map(|i| entry(&format!("tool_{i}"))).collect();
        let h = report_header_calls(&many);
        assert!(h.len() <= 2048, "{}", h.len());
        assert!(h.ends_with(r#"{"truncated":true}]"#));
        let parsed: Vec<serde_json::Value> = serde_json::from_str(&h).unwrap();
        assert!(parsed.len() < 200);
        assert_eq!(report_header_calls(&[]), "[]");
    }

    fn report_header_calls(e: &[ReportEntry]) -> String {
        report_header(e, &[])
    }

    #[test]
    fn v2_call_entries() {
        let j = |d, code| {
            let e = report_entry(&tc(), d, Basis::Decider, code);
            serde_json::from_str::<serde_json::Value>(&e.to_json()).unwrap()
        };
        let v = j(ReportDecision::Block, Some(BlockCode::Policy));
        assert_eq!(
            (v["v"].as_u64(), v["side"].as_str(), v["decision"].as_str(), v["code"].as_str()),
            (Some(2), Some("response"), Some("block"), Some("policy"))
        );
        assert_eq!((v["enforced"].as_bool(), v["reason"].as_str()), (Some(true), Some("tool-call-denied")));
        let v = j(ReportDecision::Allow, Some(BlockCode::Policy));
        assert_eq!((v["decision"].as_str(), v["code"].as_str(), v["enforced"].as_bool()), (Some("allow"), Some("policy"), Some(false)));
        assert_eq!(v["reason"].as_str(), Some("tool-call-denied"));
        let v = j(ReportDecision::Block, Some(BlockCode::PolicyUnavailable));
        assert_eq!(v["reason"].as_str(), Some("tool-policy-unavailable"));
        let v = j(ReportDecision::Block, Some(BlockCode::HoldLimit));
        assert!(v["reason"].is_null());
        let v = j(ReportDecision::Allow, None);
        assert!(v["enforced"].is_null() && v["reason"].is_null() && v["code"].is_null());
    }

    #[test]
    fn request_entries_go_after_call_entries_and_are_cut_first() {
        let h = report_header(&[call_entry("a")], &many_request_entries(60));
        let v: Vec<serde_json::Value> = serde_json::from_str(&h).unwrap();
        assert!(h.len() <= 2048, "{}", h.len());
        assert_eq!(v[0]["side"], "response");
        let last = v.last().unwrap();
        assert_eq!(last["side"], "request");
        assert!(last["omitted"].as_u64().unwrap() > 0);
        let few = report_header(&[call_entry("a")], &many_request_entries(2));
        let v: Vec<serde_json::Value> = serde_json::from_str(&few).unwrap();
        assert_eq!(v.len(), 3);
        assert_eq!(v[2]["action"], "remove-definition");
    }

    #[test]
    fn many_calls_still_fit_with_requests_present() {
        let calls: Vec<ReportEntry> = (0..200).map(|i| entry(&format!("tool_{i}"))).collect();
        let h = report_header(&calls, &many_request_entries(5));
        assert!(h.len() <= 2048);
        serde_json::from_str::<Vec<serde_json::Value>>(&h).unwrap();
    }

    #[test]
    fn request_actions_have_stable_labels() {
        let a = [RequestAction::RemoveDefinition, RequestAction::ReplaceResult, RequestAction::ResetToolChoice];
        assert_eq!(a.map(RequestAction::as_str), ["remove-definition", "replace-result", "reset-tool-choice"]);
    }

    #[test]
    fn request_comments_are_sse_comments() {
        let cs = request_comments(&many_request_entries(3));
        assert_eq!(cs.len(), 3);
        for c in cs {
            assert!(c.starts_with(b": paigasus-decision {"));
            assert!(c.ends_with(b"\n\n"));
        }
    }

    #[test]
    fn every_report_reason_is_declared_in_the_canonical_registry() {
        use paigasus_proto::paigasus::common::v1::ErrorReason;
        let from_codes = BlockCode::ALL.iter().filter_map(|c| reason_label(*c));
        let from_requests = RequestReason::ALL.iter().map(|r| r.as_str());
        let mut seen = 0;
        for r in from_codes.chain(from_requests) {
            seen += 1;
            assert!(ErrorReason::from_wire_reason(r).is_some(), "{r} is not declared in common/v1/error.proto");
        }
        assert!(seen >= 4, "the loops must cover the reasons: {seen}");
    }

    #[test]
    fn the_comment_is_one_sse_comment() {
        let c = report_comment(&entry("x"));
        assert!(c.starts_with(b": paigasus-decision {"));
        assert!(c.ends_with(b"}\n\n"));
        assert_eq!(c.iter().filter(|b| **b == b'\n').count(), 2);
    }
}
