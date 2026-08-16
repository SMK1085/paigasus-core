// SPDX-License-Identifier: Apache-2.0

//! End-to-end coverage for `#[derive(Auditable)]` (SMA-438).
//!
//! A hand-written struct proves the re-export, the absolute paths, and `extern crate self`
//! all line up; the rest of the file exercises the derive against the generated messages.

use paigasus_proto::audit::{AuditMetadata, Auditable};

#[derive(Auditable, Default)]
struct HandWritten {
    #[allow(dead_code)]
    prn: String,
    audit: Option<AuditMetadata>,
}

#[test]
fn derived_impl_reads_through_to_embedded_metadata() {
    let dto = HandWritten {
        prn: "p".to_string(),
        audit: Some(AuditMetadata {
            created_by: "svc".to_string(),
            ..Default::default()
        }),
    };
    // A sentinel value, not just `is_some()` — a derive emitting `{ None }` must fail here.
    assert_eq!(dto.created_by(), Some("svc"));
    // Empty actor is a meaningful value (unknown/system), distinct from absent audit.
    assert_eq!(dto.modified_by(), Some(""));
    assert_eq!(dto.created_at(), None);
}

#[test]
fn absent_audit_yields_none_accessors() {
    let dto = HandWritten::default();
    assert_eq!(dto.audit(), None);
    assert_eq!(dto.created_by(), None);
    assert_eq!(dto.modified_at(), None);
}

// ─── Generated messages (SMA-438) ────────────────────────────────────────────────────────────
//
// Each type is built with a DISTINCT sentinel in `created_by` and asserted to return exactly
// that. A bare `fn assert_auditable<T: Auditable>()` bound would prove only that an impl
// EXISTS — a derive emitting `{ None }` would satisfy it for six of the seven types.

use paigasus_proto::paigasus::common::v1::AuditableExample;
use paigasus_proto::paigasus::iam::v1::{ApiKey, Membership, Organization, Project, ServiceAccount, Team};

fn stamped(actor: &str) -> Option<AuditMetadata> {
    Some(AuditMetadata {
        created_by: actor.to_string(),
        ..Default::default()
    })
}

macro_rules! generated_type_reads_through {
    ($($name:ident => $ty:ty),+ $(,)?) => {$(
        #[test]
        fn $name() {
            let sentinel = stringify!($ty);
            let mut dto = <$ty>::default();
            dto.audit = stamped(sentinel);
            assert_eq!(dto.created_by(), Some(sentinel), "derived impl did not read the audit field");
            assert_eq!(dto.audit().map(|a| a.created_by.as_str()), Some(sentinel));

            let empty = <$ty>::default();
            assert_eq!(empty.audit(), None, "absent audit must yield None");
            assert_eq!(empty.created_by(), None);
            assert_eq!(empty.modified_at(), None);
        }
    )+};
}

generated_type_reads_through! {
    auditable_example_reads_through => AuditableExample,
    organization_reads_through      => Organization,
    team_reads_through              => Team,
    project_reads_through           => Project,
    membership_reads_through        => Membership,
    service_account_reads_through   => ServiceAccount,
    api_key_reads_through           => ApiKey,
}
