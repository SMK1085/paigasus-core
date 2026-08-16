// SPDX-License-Identifier: Apache-2.0
// Re-exported so the derive's generated code has ONE stable anchor to name
// (`::paigasus_proto::audit::AuditMetadata`) instead of the codegen module layout, which
// `clean: true` regenerates (SMA-438 D5).
pub use crate::paigasus::common::v1::AuditMetadata;
// The derive and the trait below deliberately share a name: macros live in the macro namespace,
// traits in the type namespace, so `use paigasus_proto::audit::Auditable;` imports BOTH. This is
// the `serde::Serialize` pattern (SMA-438 F3).
pub use paigasus_proto_derive::Auditable;

/// Implemented by any DTO/entity that carries [`AuditMetadata`].
pub trait Auditable {
    /// The embedded audit metadata, if present.
    fn audit(&self) -> Option<&AuditMetadata>;

    fn created_by(&self) -> Option<&str> {
        self.audit().map(|a| a.created_by.as_str())
    }
    fn modified_by(&self) -> Option<&str> {
        self.audit().map(|a| a.modified_by.as_str())
    }
    fn created_at(&self) -> Option<&::prost_types::Timestamp> {
        self.audit().and_then(|a| a.created_at.as_ref())
    }
    fn modified_at(&self) -> Option<&::prost_types::Timestamp> {
        self.audit().and_then(|a| a.modified_at.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::Auditable;
    use crate::paigasus::common::v1::{AuditMetadata, AuditableExample};

    // No manual impl here any more: `AuditableExample` now carries #[derive(Auditable)] via
    // codegen (SMA-438), so the two tests below exercise the DERIVED impl. Re-adding a manual
    // one is an E0119 conflict. Note this makes the fixture's impl public API, reversing
    // SMA-425's decision to keep it test-only — deliberate, see SMA-438 spec D8.

    #[test]
    fn accessors_read_through_embedded_metadata() {
        let dto = AuditableExample {
            id: "x".to_string(),
            audit: Some(AuditMetadata {
                created_by: "svc".to_string(),
                ..Default::default()
            }),
        };
        assert_eq!(dto.created_by(), Some("svc"));
        // Empty actor is a meaningful value (unknown/system), distinct from absent audit.
        assert_eq!(dto.modified_by(), Some(""));
        // created_at was never set, so the timestamp accessor is None even though audit is Some.
        assert_eq!(dto.created_at(), None);
    }

    #[test]
    fn absent_audit_yields_none_accessors() {
        let dto = AuditableExample { id: "y".to_string(), audit: None };
        assert_eq!(dto.audit(), None);
        assert_eq!(dto.created_by(), None);
        assert_eq!(dto.modified_by(), None);
        assert_eq!(dto.created_at(), None);
        assert_eq!(dto.modified_at(), None);
    }
}
