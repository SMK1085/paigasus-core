// SPDX-License-Identifier: Apache-2.0

//! The starter tool policy (spec section 6.3).
//!
//! It permits every catalog tool and does not permit `Unregistered`, so a tool that is not in
//! the catalog stays blocked (ADR-0027 (d)).

/// The `@id` of the starter policy.
pub const STARTER_TOOL_POLICY_ID: &str = "starter-allow-all";

/// The starter tool policy source, byte for byte.
pub const STARTER_TOOL_POLICY_SOURCE: &str = "@id(\"starter-allow-all\")\npermit (principal, action in Pgs::Gateway::Action::\"InvokeTool\", resource);\n";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool_policy::schema::tenant_schema_json;
    use cedar_policy::{PolicySet, Schema, ValidationMode, Validator};

    #[test]
    fn the_starter_source_is_pinned() {
        assert_eq!(
            STARTER_TOOL_POLICY_SOURCE,
            "@id(\"starter-allow-all\")\npermit (principal, action in Pgs::Gateway::Action::\"InvokeTool\", resource);\n"
        );
    }

    #[test]
    fn the_starter_id_is_the_annotation() {
        let policies: PolicySet = STARTER_TOOL_POLICY_SOURCE.parse().unwrap();
        assert_eq!(policies.policies().count(), 1);
        let policy = policies.policies().next().unwrap();
        assert_eq!(policy.annotation("id"), Some(STARTER_TOOL_POLICY_ID));
    }

    #[test]
    fn the_starter_validates_against_an_empty_catalog() {
        let schema = Schema::from_json_value(tenant_schema_json(&[])).unwrap();
        let policies: PolicySet = STARTER_TOOL_POLICY_SOURCE.parse().unwrap();
        let result = Validator::new(schema).validate(&policies, ValidationMode::Strict);
        assert!(result.validation_passed(), "{:?}", result.validation_errors().map(ToString::to_string).collect::<Vec<_>>());
        assert_eq!(result.validation_warnings().count(), 0);
    }
}
