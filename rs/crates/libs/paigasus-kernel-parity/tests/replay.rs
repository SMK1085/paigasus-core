// SPDX-License-Identifier: Apache-2.0
//! The Rust side of the multi-way replay (the kernel as its own "binding") + the corpus-integrity
//! guards shared by every language replay. Each corpus gets a non-empty guard (a missing/empty
//! corpus must fail RED, not pass having compared nothing) and a committed==fresh-generation guard
//! (a kernel edit landed without regenerating fails here) (SMA-433, SMA-448).

use paigasus_kernel_parity::{Case, PrnCanonicalCase, PrnCedarCase, Uuid7Case, build_corpus, build_prn_canonical_corpus, build_prn_cedar_corpus, build_uuid7_corpus, load_corpus};

#[test]
fn sum_corpus_present_and_fresh() {
    let committed = load_corpus::<Case>("sum");
    assert!(!committed.is_empty(), "sum corpus is empty");
    assert_eq!(committed, build_corpus());
}

#[test]
fn uuid7_corpus_present_and_fresh() {
    let committed = load_corpus::<Uuid7Case>("uuid7");
    assert!(!committed.is_empty(), "uuid7 corpus is empty");
    assert_eq!(committed, build_uuid7_corpus());
}

#[test]
fn prn_canonical_corpus_present_and_fresh() {
    let committed = load_corpus::<PrnCanonicalCase>("prn_canonical");
    assert!(!committed.is_empty(), "prn_canonical corpus is empty");
    assert_eq!(committed, build_prn_canonical_corpus());
}

#[test]
fn prn_cedar_corpus_present_and_fresh() {
    let committed = load_corpus::<PrnCedarCase>("prn_cedar");
    assert!(!committed.is_empty(), "prn_cedar corpus is empty");
    assert_eq!(committed, build_prn_cedar_corpus());
}
