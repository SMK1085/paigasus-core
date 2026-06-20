// SPDX-License-Identifier: Apache-2.0
//! The Rust side of the 4-way replay (the kernel as its own "binding") + the corpus-integrity
//! guard shared by every language replay (SMA-433). The integrity guard is the load-bearing part:
//! a replay that only iterates-and-asserts goes GREEN if the corpus fails to load or is empty.

use paigasus_kernel_parity::{build_corpus, load_corpus};

#[test]
fn committed_corpus_is_non_empty() {
    // Integrity guard: a missing/empty corpus must fail RED, not pass having compared nothing.
    assert!(!load_corpus().is_empty(), "committed parity corpus is empty");
}

#[test]
fn committed_corpus_matches_a_fresh_generation() {
    // The committed vectors must equal what the current kernel produces. Catches a kernel edit
    // landed without regenerating the corpus (an in-process complement to repo:parity-corpus-drift),
    // and self-validates the file against a hand-edit. Full Vec equality subsumes count + per-case
    // value + domain.
    assert_eq!(load_corpus(), build_corpus());
}
