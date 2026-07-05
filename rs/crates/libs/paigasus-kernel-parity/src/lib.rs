// SPDX-License-Identifier: Apache-2.0
//! Cross-binding behavioral-parity corpus for the Paigasus kernel (ADR-0005, SMA-433).
//!
//! The corpus is a set of `{a, b, expected}` cases where `expected = paigasus_kernel::sum(a, b)` —
//! the kernel is the single oracle. Every binding (Python/PyO3, Node/napi, browser/wasm) and the
//! Rust impl replay this same file and must reproduce `expected`. The sample is a deterministic
//! enumeration (no PRNG) over the i32-safe parity domain, so regeneration is byte-stable and the
//! drift guard's `git diff` is meaningful. See the design spec for why the domain is i32-safe.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// One parity case: inputs `a`, `b` and the kernel-computed `expected`. `a`/`b` are `i32` (the
/// narrowest binding surface — napi/wasm); `expected` is the kernel's `i64` result, constrained
/// to the i32 range by [`build_corpus`] so every binding agrees.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Case {
    pub a: i32,
    pub b: i32,
    pub expected: i64,
}

/// Representative i32 values, including the boundaries. Pairs are taken across the full lattice and
/// filtered to the parity domain; the order is fixed, so the generated corpus is deterministic.
const SAMPLE_VALUES: [i32; 9] = [i32::MIN, -1_000_000, -1_000, -1, 0, 1, 1_000, 1_000_000, i32::MAX];

/// Build the deterministic corpus: every `(a, b)` from the lattice whose kernel sum stays within
/// `i32` (the parity domain — outside it napi/wasm would wrap while Python would not). `expected`
/// is computed FROM the kernel, making the kernel the single oracle.
#[must_use]
pub fn build_corpus() -> Vec<Case> {
    let mut cases = Vec::new();
    for &a in &SAMPLE_VALUES {
        for &b in &SAMPLE_VALUES {
            let expected = paigasus_kernel::sum(a as i64, b as i64);
            if i32::try_from(expected).is_ok() {
                cases.push(Case { a, b, expected });
            }
        }
    }
    cases
}

/// Serialize any corpus byte-stably: one compact case object per line, trailing newline.
#[must_use]
pub fn serialize<T: Serialize>(cases: &[T]) -> String {
    let mut out = String::from("[\n");
    for (i, c) in cases.iter().enumerate() {
        let comma = if i + 1 < cases.len() { "," } else { "" };
        out.push_str("  ");
        out.push_str(&serde_json::to_string(c).expect("case serializes infallibly"));
        out.push_str(comma);
        out.push('\n');
    }
    out.push_str("]\n");
    out
}

/// Absolute path to a committed corpus by stem (`sum`, `uuid7`, `prn_canonical`, `prn_cedar`).
#[must_use]
pub fn corpus_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("vectors/{name}.json"))
}

/// Load and parse a committed corpus by stem. Panics (fails a test red) if missing/invalid.
#[must_use]
pub fn load_corpus<T: serde::de::DeserializeOwned>(name: &str) -> Vec<T> {
    let text = std::fs::read_to_string(corpus_path(name)).expect("read committed corpus");
    serde_json::from_str(&text).expect("parse committed corpus")
}

/// Lowercase hex of the 10 random bytes (the FFI `rand_hex` wire format).
fn hex10(rand: &[u8; 10]) -> String {
    let mut s = String::with_capacity(20);
    for b in rand {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// One UUIDv7-minting case: injected `(unix_ms, rand_hex)` and the kernel's `expected_uuid`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Uuid7Case {
    pub unix_ms: u64,
    pub rand_hex: String,
    pub expected_uuid: String,
}

/// One PRN parse/canonicalize case. Valid inputs: `error_kind == ""`, `canonical == Some(..)`.
/// Invalid inputs: `error_kind == <token>`, `canonical == None` (serialized as JSON `null`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrnCanonicalCase {
    pub input: String,
    pub error_kind: String,
    pub canonical: Option<String>,
}

/// One PRN → Cedar case.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrnCedarCase {
    pub prn: String,
    pub entity_type: String,
    pub entity_id: String,
}

/// Deterministic UUIDv7 corpus: boundary timestamps × representative random patterns.
#[must_use]
pub fn build_uuid7_corpus() -> Vec<Uuid7Case> {
    const TIMESTAMPS: [u64; 5] = [0, 1, 1_000, 1_700_000_000_000, (1u64 << 48) - 1];
    const RANDS: [[u8; 10]; 4] = [
        [0x00; 10],
        [0xFF; 10],
        [0xAA, 0x55, 0xAA, 0x55, 0xAA, 0x55, 0xAA, 0x55, 0xAA, 0x55],
        [0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF, 0x10, 0x32],
    ];
    let mut cases = Vec::new();
    for &unix_ms in &TIMESTAMPS {
        for rand in &RANDS {
            let expected_uuid = paigasus_kernel::mint_uuid7(unix_ms, *rand).as_hyphenated().to_string();
            cases.push(Uuid7Case {
                unix_ms,
                rand_hex: hex10(rand),
                expected_uuid,
            });
        }
    }
    cases
}

/// Deterministic PRN parse/canonicalize corpus: valid round-trips + one case per rejection path.
#[must_use]
pub fn build_prn_canonical_corpus() -> Vec<PrnCanonicalCase> {
    let mut inputs: Vec<String> = [
        // valid
        "prn:pgs:iam:::organization/0190a1e5-0000-7000-8000-000000000000",
        "prn:pgs:iam::0190a100-0000-7000-8000-0000000000aa:team/0190a1b2-0000-7000-8000-000000000001",
        "prn:pgs:iam::0190a100-0000-7000-8000-0000000000aa:project/0190a1c3-0000-7000-8000-000000000002",
        "prn:pgs:iam::0190a100-0000-7000-8000-0000000000aa:service-account/0190a1d4-0000-7000-8000-000000000003",
        "prn:pgs:iam:::user/0190a1e5-0000-7000-8000-000000000004",
        "prn:pgs:gateway::0190a100-0000-7000-8000-0000000000aa:api-key/0190a1f6-0000-7000-8000-000000000005",
        // mixed-case UUID → canonicalized lowercase
        "prn:pgs:iam:::user/0190A1E5-0000-7000-8000-00000000ABCD",
        // invalid — one per PrnError variant
        "",
        "prn:pgs:iam:::user/0190a1e5-0000-7000-8000-000000000004:extra",
        "xrn:pgs:iam:::user/0190a1e5-0000-7000-8000-000000000004",
        "prn:pgz:iam:::user/0190a1e5-0000-7000-8000-000000000004",
        "prn:pgs:IAM:::user/0190a1e5-0000-7000-8000-000000000004",
        "prn:pgs:api--key:::user/0190a1e5-0000-7000-8000-000000000004",
        "prn:pgs:iam:US-EAST:0190a100-0000-7000-8000-0000000000aa:team/0190a1b2-0000-7000-8000-000000000001",
        "prn:pgs:iam::not-a-uuid:team/0190a1b2-0000-7000-8000-000000000001",
        "prn:pgs:iam:::userwithoutslash",
        "prn:pgs:iam:::user/a/b",
        "prn:pgs:iam:::/0190a1e5-0000-7000-8000-000000000004",
        "prn:pgs:iam::0190a100-0000-7000-8000-0000000000aa:team-1/0190a1b2-0000-7000-8000-000000000001",
        "prn:pgs:iam:::UPPER/0190a1e5-0000-7000-8000-000000000004",
        "prn:pgs:iam:::user/not-a-uuid",
        "prn:pgs:iam:::user/",
    ]
    .iter()
    .map(|s| (*s).to_string())
    .collect();
    inputs.push(format!("prn:pgs:iam:::user/{}", "a".repeat(600))); // too-long
    inputs
        .into_iter()
        .map(|input| match paigasus_kernel::Prn::parse(&input) {
            Ok(p) => PrnCanonicalCase {
                input,
                error_kind: String::new(),
                canonical: Some(p.canonical()),
            },
            Err(e) => PrnCanonicalCase {
                input,
                error_kind: e.kind().to_string(),
                canonical: None,
            },
        })
        .collect()
}

/// One PRN field-accessor + build round-trip case: the canonical PRN and each accessor's expected
/// output (`org` is `""` when the tenant slot is empty). Covers the FFI accessors AND `prn_build`,
/// which must reproduce `prn` from the fields — mapping `""` org → None (spec §5 marshalling).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrnFieldsCase {
    pub prn: String,
    pub service: String,
    pub region: String,
    pub org: String,
    pub resource_type: String,
    pub resource_id: String,
}

/// Deterministic PRN field/build corpus: org-scoped and empty-tenant-slot (organization/user) PRNs.
#[must_use]
pub fn build_prn_fields_corpus() -> Vec<PrnFieldsCase> {
    const PRNS: [&str; 6] = [
        "prn:pgs:iam:::organization/0190a1e5-0000-7000-8000-000000000000",
        "prn:pgs:iam::0190a100-0000-7000-8000-0000000000aa:team/0190a1b2-0000-7000-8000-000000000001",
        "prn:pgs:iam::0190a100-0000-7000-8000-0000000000aa:project/0190a1c3-0000-7000-8000-000000000002",
        "prn:pgs:iam::0190a100-0000-7000-8000-0000000000aa:service-account/0190a1d4-0000-7000-8000-000000000003",
        "prn:pgs:iam:::user/0190a1e5-0000-7000-8000-000000000004",
        "prn:pgs:gateway::0190a100-0000-7000-8000-0000000000aa:api-key/0190a1f6-0000-7000-8000-000000000005",
    ];
    PRNS.iter()
        .map(|s| {
            let p = paigasus_kernel::Prn::parse(s).expect("prn_fields corpus PRN parses");
            PrnFieldsCase {
                prn: (*s).to_string(),
                service: p.service().to_string(),
                region: p.region().to_string(),
                org: p.org().map(|u| u.as_hyphenated().to_string()).unwrap_or_default(),
                resource_type: p.resource_type().to_string(),
                resource_id: p.resource_id().as_hyphenated().to_string(),
            }
        })
        .collect()
}

/// Deterministic PRN → Cedar corpus across services, types, and multi-dash types.
#[must_use]
pub fn build_prn_cedar_corpus() -> Vec<PrnCedarCase> {
    const PRNS: [&str; 7] = [
        "prn:pgs:iam:::organization/0190a1e5-0000-7000-8000-000000000000",
        "prn:pgs:iam::0190a100-0000-7000-8000-0000000000aa:team/0190a1b2-0000-7000-8000-000000000001",
        "prn:pgs:iam::0190a100-0000-7000-8000-0000000000aa:project/0190a1c3-0000-7000-8000-000000000002",
        "prn:pgs:iam::0190a100-0000-7000-8000-0000000000aa:service-account/0190a1d4-0000-7000-8000-000000000003",
        "prn:pgs:iam:::user/0190a1e5-0000-7000-8000-000000000004",
        "prn:pgs:gateway::0190a100-0000-7000-8000-0000000000aa:api-key/0190a1f6-0000-7000-8000-000000000005",
        "prn:pgs:gateway::0190a100-0000-7000-8000-0000000000aa:oauth2-client/0190a1f7-0000-7000-8000-000000000006",
    ];
    PRNS.iter()
        .map(|s| {
            let p = paigasus_kernel::Prn::parse(s).expect("cedar corpus PRN parses");
            let c = paigasus_kernel::to_cedar_uid(&p);
            PrnCedarCase {
                prn: (*s).to_string(),
                entity_type: c.entity_type,
                entity_id: c.entity_id,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corpus_is_non_empty_and_in_domain() {
        let cases = build_corpus();
        assert!(!cases.is_empty(), "generated corpus is empty");
        for c in &cases {
            // Every case lies in the parity domain: `expected` fits i32 (so napi/wasm don't wrap)…
            assert!(i32::try_from(c.expected).is_ok(), "case out of i32 parity domain: {c:?}");
            // …and equals the kernel for that input (the corpus is kernel-derived by construction).
            assert_eq!(c.expected, paigasus_kernel::sum(c.a as i64, c.b as i64));
        }
    }

    #[test]
    fn serialize_is_stable_and_round_trips() {
        let cases = build_corpus();
        let text = serialize(&cases);
        // Byte-stability contract: one case per line, trailing newline.
        assert!(text.ends_with("]\n"), "corpus must end with a newline");
        // Pin the byte-level format (not just structure): a serializer whitespace regression
        // (extra indent, a space after a colon) fails HERE, before the corpus is committed — the
        // Task 8 drift guard is the on-disk gate; this is the pre-commit one.
        assert!(text.starts_with("[\n  {\"a\":"), "unexpected corpus byte preamble: {:?}", &text[..text.len().min(24)]);
        assert_eq!(text.matches('\n').count(), cases.len() + 2, "expect one line per case + brackets");
        let parsed: Vec<Case> = serde_json::from_str(&text).expect("round-trips");
        assert_eq!(parsed, cases);
    }
}
