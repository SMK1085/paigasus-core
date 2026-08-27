// SPDX-License-Identifier: Apache-2.0

//! End-to-end coverage for `#[derive(Auditable)]` (SMA-438).
//!
//! A hand-written struct proves the re-export, the absolute paths, and `extern crate self`
//! all line up; the rest of the file exercises the derive against the generated messages.

use paigasus_proto::audit::{Actor, AuditMetadata, Auditable};

#[derive(Auditable, Default)]
struct HandWritten {
    #[allow(dead_code)]
    prn: String,
    audit: Option<AuditMetadata>,
}

/// A distinct canonical PRN per fixture. Test fixtures obey `Actor`'s producer obligation
/// (SMA-439 spec D2) rather than modelling what it tells producers not to write — a bare
/// type name is not a parseable PRN.
fn actor(n: u32) -> Actor {
    Actor {
        prn: format!("prn:pgs:iam:::principal/0192f1c0-0000-7000-8000-{n:012}"),
    }
}

#[test]
fn derived_impl_reads_through_to_embedded_metadata() {
    let dto = HandWritten {
        prn: "p".to_string(),
        audit: Some(AuditMetadata {
            creator: Some(actor(1)),
            ..Default::default()
        }),
    };
    // A sentinel value, not just `is_some()` — a derive emitting `{ None }` must fail here.
    assert_eq!(dto.creator(), Some(&actor(1)));
    // SMA-439: an unknown actor reads as None, exactly like an absent one. The
    // present-vs-absent distinction now lives on `audit()` alone, so assert both.
    assert!(dto.audit().is_some());
    assert_eq!(dto.modifier(), None);
    assert_eq!(dto.created_at(), None);
}

#[test]
fn absent_audit_yields_none_accessors() {
    let dto = HandWritten::default();
    assert_eq!(dto.audit(), None);
    assert_eq!(dto.creator(), None);
    assert_eq!(dto.modified_at(), None);
}

// ─── Generated messages (SMA-438) ────────────────────────────────────────────────────────────
//
// Each type is built with a specific NON-DEFAULT sentinel PRN in `creator` and asserted to
// round-trip through the accessor exactly. A bare `fn assert_auditable<T: Auditable>()` bound
// would prove only that an impl EXISTS — a derive emitting `{ None }` would satisfy it for six
// of the seven types; only a real, non-default value forces the accessor to have actually read
// the field. The per-row integer isn't a uniqueness guarantee (a copy-pasted duplicate would
// not be caught) — it just keeps each row's sentinel independently identifiable in a failure
// message.

use paigasus_proto::paigasus::common::v1::AuditableExample;
use paigasus_proto::paigasus::iam::v1::{ApiKey, Membership, Organization, Project, ServiceAccount, Team};

fn stamped(who: &Actor) -> Option<AuditMetadata> {
    Some(AuditMetadata {
        creator: Some(who.clone()),
        ..Default::default()
    })
}

macro_rules! generated_type_reads_through {
    ($($name:ident => $ty:ty => $n:expr),+ $(,)?) => {$(
        #[test]
        fn $name() {
            let sentinel = actor($n);
            let mut dto = <$ty>::default();
            dto.audit = stamped(&sentinel);
            assert_eq!(dto.creator(), Some(&sentinel), "derived impl did not read the audit field");
            assert_eq!(dto.audit().and_then(|a| a.creator.as_ref()), Some(&sentinel));

            let empty = <$ty>::default();
            assert_eq!(empty.audit(), None, "absent audit must yield None");
            assert_eq!(empty.creator(), None);
            assert_eq!(empty.modified_at(), None);
        }
    )+};
}

generated_type_reads_through! {
    auditable_example_reads_through => AuditableExample  => 10,
    organization_reads_through      => Organization      => 11,
    team_reads_through              => Team              => 12,
    project_reads_through           => Project           => 13,
    membership_reads_through        => Membership        => 14,
    service_account_reads_through   => ServiceAccount    => 15,
    api_key_reads_through           => ApiKey            => 16,
}
