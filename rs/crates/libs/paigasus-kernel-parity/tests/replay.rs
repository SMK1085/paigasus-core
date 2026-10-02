// SPDX-License-Identifier: Apache-2.0
//! The Rust side of the multi-way replay (the kernel as its own "binding") + the corpus-integrity
//! guards shared by every language replay. Each corpus gets a non-empty guard (a missing/empty
//! corpus must fail RED, not pass having compared nothing) and a committed==fresh-generation guard
//! (a kernel edit landed without regenerating fails here) (SMA-433, SMA-448).

use paigasus_kernel_parity::{
    Case, PrnCanonicalCase, PrnCedarCase, PrnFieldsCase, PrnParseCase, Uuid7Case, build_corpus, build_prn_canonical_corpus, build_prn_cedar_corpus, build_prn_fields_corpus, build_prn_parse_corpus,
    build_uuid7_corpus, load_corpus,
};

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

#[test]
fn prn_fields_corpus_present_and_fresh() {
    let committed = load_corpus::<PrnFieldsCase>("prn_fields");
    assert!(!committed.is_empty(), "prn_fields corpus is empty");
    assert_eq!(committed, build_prn_fields_corpus());
}

#[test]
fn prn_parse_corpus_present_and_fresh() {
    let committed = load_corpus::<PrnParseCase>("prn_parse");
    assert!(!committed.is_empty(), "prn_parse corpus is empty");
    assert_eq!(committed, build_prn_parse_corpus());
}

/// The Rust "binding" of the one-call parse: every row through the kernel's wire function. The
/// expected values come from the typed accessors (`build_prn_parse_corpus`), not from the function
/// under test, so a reorder in `wire::prn_parse_fields` reds here (SMA-673 L2).
#[test]
fn prn_parse_corpus_replays_through_the_wire_function() {
    let committed = load_corpus::<PrnParseCase>("prn_parse");
    assert!(!committed.is_empty(), "prn_parse corpus is empty");
    for c in &committed {
        let expected = vec![c.error_kind.clone(), c.service.clone(), c.region.clone(), c.org.clone(), c.resource_type.clone(), c.resource_id.clone()];
        assert_eq!(paigasus_kernel::wire::prn_parse_fields(&c.input), expected, "input {:?}", c.input);
    }
}

/// A marshalling defect shared by the corpus helper and the wire function is invisible to the
/// replay above. The older `prn_fields` corpus is a second oracle for the PRNs both corpora hold.
#[test]
fn prn_parse_rows_agree_with_prn_fields_rows() {
    let parse = load_corpus::<PrnParseCase>("prn_parse");
    let fields = load_corpus::<PrnFieldsCase>("prn_fields");
    assert!(!fields.is_empty(), "prn_fields corpus is empty");
    for f in &fields {
        let p = parse
            .iter()
            .find(|p| p.input == f.prn)
            .unwrap_or_else(|| panic!("prn_parse has no row for the prn_fields PRN {:?}", f.prn));
        assert_eq!(p.error_kind, "", "{:?}", f.prn);
        assert_eq!(
            (&p.service, &p.region, &p.org, &p.resource_type, &p.resource_id),
            (&f.service, &f.region, &f.org, &f.resource_type, &f.resource_id),
            "{:?}",
            f.prn
        );
    }
}
