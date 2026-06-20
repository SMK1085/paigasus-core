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

/// Serialize the corpus byte-stably: a JSON array with exactly one compact case object per line and
/// a trailing newline. Stable across runs/toolchains so the drift guard's `git diff` is meaningful
/// (`serde_json::to_string_pretty` would also be stable, but one-line-per-case gives cleaner diffs).
#[must_use]
pub fn serialize(cases: &[Case]) -> String {
    let mut out = String::from("[\n");
    for (i, c) in cases.iter().enumerate() {
        let comma = if i + 1 < cases.len() { "," } else { "" };
        out.push_str("  ");
        out.push_str(&serde_json::to_string(c).expect("Case serializes infallibly"));
        out.push_str(comma);
        out.push('\n');
    }
    out.push_str("]\n");
    out
}

/// Absolute path to the committed corpus, resolved against this crate's directory so it is
/// independent of the process working directory (the generator and the replay both use it).
#[must_use]
pub fn corpus_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("vectors/sum.json")
}

/// Load and parse the committed corpus. Panics (i.e. fails a test red) if it is missing or invalid.
#[must_use]
pub fn load_corpus() -> Vec<Case> {
    let text = std::fs::read_to_string(corpus_path()).expect("read committed corpus");
    serde_json::from_str(&text).expect("parse committed corpus")
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
