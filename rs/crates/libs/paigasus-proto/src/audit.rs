// SPDX-License-Identifier: Apache-2.0
// Re-exported so the derive's generated code has ONE stable anchor to name
// (`::paigasus_proto::audit::AuditMetadata`) instead of the codegen module layout, which
// `clean: true` regenerates (SMA-438 D5).
pub use crate::paigasus::common::v1::{Actor, AuditMetadata};
// The derive and the trait below deliberately share a name: macros live in the macro namespace,
// traits in the type namespace, so `use paigasus_proto::audit::Auditable;` imports BOTH. This is
// the `serde::Serialize` pattern (SMA-438 F3).
pub use paigasus_proto_derive::Auditable;

/// Implemented by any DTO/entity that carries [`AuditMetadata`].
pub trait Auditable {
    /// The embedded audit metadata, if present.
    fn audit(&self) -> Option<&AuditMetadata>;

    /// Who created the entity, or `None` if unknown/system.
    ///
    /// Per `Actor`'s contract an empty or unparseable `prn` ALSO means unknown, but this
    /// accessor deliberately does not normalise that away: the rule is a producer
    /// obligation stated once in the proto, and enforcing it here — in one of three
    /// languages, on one of two access paths, since `.creator` stays readable directly —
    /// would make the trait and the field disagree (SMA-439 spec D2).
    fn creator(&self) -> Option<&Actor> {
        self.audit().and_then(|a| a.creator.as_ref())
    }
    /// Who last modified the entity, or `None` if unknown/system. See [`Auditable::creator`].
    fn modifier(&self) -> Option<&Actor> {
        self.audit().and_then(|a| a.modifier.as_ref())
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
    use crate::paigasus::common::v1::{Actor, AuditMetadata, AuditableExample};

    // No manual impl here any more: `AuditableExample` now carries #[derive(Auditable)] via
    // codegen (SMA-438), so the two tests below exercise the DERIVED impl. Re-adding a manual
    // one is an E0119 conflict. Note this makes the fixture's impl public API, reversing
    // SMA-425's decision to keep it test-only — deliberate, see SMA-438 spec D8.

    const PRN: &str = "prn:pgs:iam:::principal/0192f1c0-0000-7000-8000-000000000001";

    #[test]
    fn accessors_read_through_embedded_metadata() {
        let dto = AuditableExample {
            id: "x".to_string(),
            audit: Some(AuditMetadata {
                creator: Some(Actor { prn: PRN.to_string() }),
                ..Default::default()
            }),
        };
        assert_eq!(dto.creator().map(|a| a.prn.as_str()), Some(PRN));
        // SMA-439: an unknown actor and an absent actor are now the SAME fact about the
        // actor, so `modifier()` is None here where `modified_by()` used to be Some("").
        // The present-vs-absent distinction did not vanish — it lives on `audit()` alone
        // now, and asserting both together is what pins that collapse as intended.
        assert!(dto.audit().is_some());
        assert_eq!(dto.modifier(), None);
        // created_at was never set, so the timestamp accessor is None even though audit is Some.
        assert_eq!(dto.created_at(), None);
    }

    #[test]
    fn absent_audit_yields_none_accessors() {
        let dto = AuditableExample { id: "y".to_string(), audit: None };
        assert_eq!(dto.audit(), None);
        assert_eq!(dto.creator(), None);
        assert_eq!(dto.modifier(), None);
        assert_eq!(dto.created_at(), None);
        assert_eq!(dto.modified_at(), None);
    }
}
