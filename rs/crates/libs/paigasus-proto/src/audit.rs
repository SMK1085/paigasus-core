// SPDX-License-Identifier: Apache-2.0
use crate::paigasus::common::v1::AuditMetadata;

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

    // Conformance impl on the *generated* embedding fixture — proves the trait works
    // over `AuditableExample.audit: Option<AuditMetadata>` produced by codegen. The
    // orphan rule blocks this from an integration test crate (neither item is local
    // there), so it lives in-crate under cfg(test).
    impl Auditable for AuditableExample {
        fn audit(&self) -> Option<&AuditMetadata> {
            self.audit.as_ref()
        }
    }

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
