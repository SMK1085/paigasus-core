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
use crate::types::{Basis, BlockCode};

const MAX_NAME_BYTES: usize = 128;
const MAX_HEADER_BYTES: usize = 2048;
const TRUNCATED: &str = r#"{"truncated":true}"#;

/// The first 128 bytes of `name`, every byte outside `[A-Za-z0-9_.:-]` replaced by `_`.
#[must_use]
pub fn sanitize_name(name: &str) -> String {
    name.bytes()
        .take(MAX_NAME_BYTES)
        .map(|b| if b.is_ascii_alphanumeric() || b"_.:-".contains(&b) { b as char } else { '_' })
        .collect()
}

/// The first 4 bytes of SHA-256 over the raw name, as 8 lower-case hex digits.
#[must_use]
pub fn name_sha256_8(name: &str) -> String {
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
    /// A provider-executed tool (D3, § 4.2): reported, never decided (P-6).
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportEntry {
    pub choice: u32,
    pub position: u32,
    /// The raw name. Serialized only through `sanitize_name` and `name_sha256_8`.
    pub name: String,
    pub decision: ReportDecision,
    pub basis: Option<Basis>,
    pub code: Option<BlockCode>,
}

impl ReportEntry {
    /// The § 4.6 schema. Every field is ASCII by construction.
    #[must_use]
    pub fn to_json(&self) -> String {
        serde_json::json!({
            "v": 1,
            "choice": self.choice,
            "position": self.position,
            "name": sanitize_name(&self.name),
            "name_sha256_8": name_sha256_8(&self.name),
            "decision": self.decision.as_str(),
            "basis": self.basis.map(Basis::as_str),
            "code": self.code.map(BlockCode::as_kebab),
        })
        .to_string()
    }
}

/// The `paigasus-tool-decisions` header value (D22).
#[must_use]
pub fn header_value(entries: &[ReportEntry]) -> String {
    let mut out = String::from("[");
    for (i, e) in entries.iter().enumerate() {
        let json = e.to_json();
        let sep = usize::from(i > 0);
        // Room for this entry, then a separator, TRUNCATED and the closing bracket.
        if out.len() + sep + json.len() + 1 + TRUNCATED.len() + 1 > MAX_HEADER_BYTES {
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
    out.push(']');
    debug_assert!(out.is_ascii());
    out
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
            serde_json::json!({"v":1,"choice":0,"position":1,"name":"x_y","name_sha256_8":name_sha256_8("x y"),"decision":"block","basis":"decider","code":"policy"})
        );
        let u = ReportEntry {
            decision: ReportDecision::Unenforceable,
            basis: None,
            code: None,
            ..entry("web_search")
        };
        let v: serde_json::Value = serde_json::from_str(&u.to_json()).unwrap();
        assert_eq!(v["decision"], "unenforceable");
        assert!(v["basis"].is_null() && v["code"].is_null());
    }

    #[test]
    fn the_header_is_ascii_and_capped() {
        let one = header_value(&[entry("é")]);
        assert!(one.is_ascii());
        assert!(one.starts_with('[') && one.ends_with(']'));
        let many: Vec<ReportEntry> = (0..200).map(|i| entry(&format!("tool_{i}"))).collect();
        let h = header_value(&many);
        assert!(h.len() <= 2048, "{}", h.len());
        assert!(h.ends_with(r#"{"truncated":true}]"#));
        let parsed: Vec<serde_json::Value> = serde_json::from_str(&h).unwrap();
        assert!(parsed.len() < 200);
        assert_eq!(header_value(&[]), "[]");
    }

    #[test]
    fn the_comment_is_one_sse_comment() {
        let c = report_comment(&entry("x"));
        assert!(c.starts_with(b": paigasus-decision {"));
        assert!(c.ends_with(b"}\n\n"));
        assert_eq!(c.iter().filter(|b| **b == b'\n').count(), 2);
    }
}
