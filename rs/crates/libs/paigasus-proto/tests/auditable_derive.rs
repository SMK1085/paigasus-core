// SPDX-License-Identifier: Apache-2.0

//! End-to-end coverage for `#[derive(Auditable)]` (SMA-438).
//!
//! Task 2 covers a hand-written struct — proving the re-export, the absolute paths, and
//! `extern crate self` all line up. Task 3 extends this file to the generated messages.

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
