// SPDX-License-Identifier: Apache-2.0

//! The two checks that IAM runs on an accepted compile result in its own process, with the
//! cedar-policy 4.14 that this crate links (spec section 5.3, steps 9 and 10). The compiler
//! validates with Dogwood's cedar 4.11. These checks catch a policy that passes under 4.11 but
//! fails under the gateway's Cedar version.

use cedar_policy::{Effect, PolicyId, PolicySet, Schema, ValidationMode, ValidationWarning, Validator};

use super::protocol::{CompiledOutput, Rejection, SetKind};

/// The reason code of a Cedar parse or validation failure (spec section 7).
pub const REASON_INVALID: &str = "tool-policy-invalid";
/// The reason code of a `permit` in a floor set (spec section 7).
pub const REASON_FLOOR_PERMIT: &str = "tool-policy-floor-permit";

/// Check a compiled output under cedar 4.14.
///
/// Step 9: parse `cedar_policies`, load `cedar_schema_json` with `Schema::from_json_value`,
/// and validate in strict mode. Each validation error is a `tool-policy-invalid` rejection.
/// An `ImpossiblePolicy` warning is also a `tool-policy-invalid` rejection: strict validation
/// types a `has` guard on a field that the schema does not declare (for example a field that
/// the JSON Schema mapping dropped) as `false`, so the policy can never apply. The compiler
/// does the same under 4.11.
///
/// Step 10: for `Floor`, each `permit` is a `tool-policy-floor-permit` rejection.
///
/// A rejection names the policy by its `@id` annotation. A parse or schema failure has no
/// policy id. The result lists every finding, in policy text order for each kind.
pub fn check_compiled(kind: SetKind, out: &CompiledOutput) -> Result<(), Vec<Rejection>> {
    let policies: PolicySet = out
        .cedar_policies
        .parse()
        .map_err(|error| vec![rejection(None, REASON_INVALID, format!("the compiled Cedar does not parse under cedar 4.14: {error}"))])?;
    let schema =
        Schema::from_json_value(out.cedar_schema_json.clone()).map_err(|error| vec![rejection(None, REASON_INVALID, format!("the compiled schema does not load under cedar 4.14: {error}"))])?;

    let mut rejections = Vec::new();
    // The compiler writes static policies only (Dogwood refuses a non-static policy). A
    // template in the text is a defect, and the floor effect rule below reads only
    // `policies()`, so a `permit` template would pass it. Each template is invalid.
    for template in policies.templates() {
        let id = template.annotation("id").map(str::to_owned);
        let message = format!(
            "the compiled Cedar holds the template {:?}; only static policies are allowed",
            id.as_deref().unwrap_or(template.id().as_ref())
        );
        rejections.push(rejection(id, REASON_INVALID, message));
    }
    let result = Validator::new(schema).validate(&policies, ValidationMode::Strict);
    for error in result.validation_errors() {
        rejections.push(rejection(source_id(&policies, error.policy_id()), REASON_INVALID, format!("Cedar 4.14 validation: {error}")));
    }
    for warning in result.validation_warnings() {
        if let ValidationWarning::ImpossiblePolicy(_) = warning {
            rejections.push(rejection(
                source_id(&policies, warning.policy_id()),
                REASON_INVALID,
                format!("Cedar 4.14 validation: {warning}; a guarded field may be missing from the schema"),
            ));
        }
    }
    if kind == SetKind::Floor {
        for policy in policies.policies().filter(|policy| policy.effect() == Effect::Permit) {
            let id = source_id(&policies, policy.id());
            let message = format!("the floor policy {:?} is a permit; a floor policy must be a forbid", id.as_deref().unwrap_or(policy.id().as_ref()));
            rejections.push(rejection(id, REASON_FLOOR_PERMIT, message));
        }
    }
    if rejections.is_empty() { Ok(()) } else { Err(rejections) }
}

/// The `@id` annotation of the policy with the Cedar id `id`. The parser gives each policy an
/// id of its own (`policy0`, ...); the source id is the annotation that the compiler writes.
fn source_id(policies: &PolicySet, id: &PolicyId) -> Option<String> {
    policies.policy(id).and_then(|policy| policy.annotation("id")).map(str::to_owned)
}

fn rejection(policy_id: Option<String>, reason: &str, message: String) -> Rejection {
    Rejection {
        policy_id,
        line: None,
        column: None,
        reason: reason.to_owned(),
        message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool_policy::{CatalogLimits, ToolKind, base_schema_json, build_manual, tenant_schema_json};
    use serde_json::{Value, json};

    fn out(policies: &str, schema: Value) -> CompiledOutput {
        CompiledOutput {
            cedar_policies: policies.to_owned(),
            cedar_schema_json: schema,
            conditions: Vec::new(),
        }
    }

    /// A catalog with one tool, `search`. Its `price` field is a JSON Schema `number`, which
    /// the mapping drops (spec section 4.3).
    fn search_schema() -> Value {
        let tool = build_manual(
            "search",
            ToolKind::Function,
            Some(json!({"type": "object", "properties": {"q": {"type": "string"}, "price": {"type": "number"}}, "required": ["q", "price"]})),
            &CatalogLimits::default(),
        )
        .expect("the tool builds");
        assert_eq!(tool.dropped_fields.len(), 1, "the mapping drops price");
        assert_eq!(tool.dropped_fields[0].path, "price");
        tenant_schema_json(&[tool])
    }

    #[test]
    fn a_valid_tenant_policy_passes() {
        let policy = "@id(\"ok\")\npermit(principal, action == Pgs::Gateway::Action::\"search\", resource) when { context.input.q == \"x\" };";
        assert_eq!(check_compiled(SetKind::Tenant, &out(policy, search_schema())), Ok(()));
    }

    #[test]
    fn a_floor_with_a_permit_fails_the_second_check() {
        let policies = "@id(\"floor-deny\")\nforbid(principal, action in Pgs::Gateway::Action::\"Unregistered\", resource);\n@id(\"floor-allow\")\npermit(principal, action in Pgs::Gateway::Action::\"InvokeTool\", resource);";
        let rejections = check_compiled(SetKind::Floor, &out(policies, base_schema_json())).expect_err("a floor permit fails");
        assert_eq!(rejections.len(), 1, "{rejections:?}");
        assert_eq!(rejections[0].policy_id.as_deref(), Some("floor-allow"));
        assert_eq!(rejections[0].reason, REASON_FLOOR_PERMIT);
        // The same text is a valid tenant set: only the floor rule rejects it.
        assert_eq!(check_compiled(SetKind::Tenant, &out(policies, base_schema_json())), Ok(()));
    }

    #[test]
    fn a_policy_that_fails_4_14_validation_is_rejected() {
        let policy = "@id(\"bad\")\npermit(principal, action == Pgs::Gateway::Action::\"search\", resource) when { context.input.zz == 1 };";
        let rejections = check_compiled(SetKind::Tenant, &out(policy, search_schema())).expect_err("an undeclared attribute fails");
        assert_eq!(rejections.len(), 1, "{rejections:?}");
        assert_eq!(rejections[0].policy_id.as_deref(), Some("bad"));
        assert_eq!(rejections[0].reason, REASON_INVALID);
    }

    #[test]
    fn a_has_guarded_read_of_a_dropped_field_is_invalid() {
        let guarded = "@id(\"price-cap\")\nforbid(principal, action == Pgs::Gateway::Action::\"search\", resource) when { context.input has price && context.input.price > 100 };";
        let rejections = check_compiled(SetKind::Tenant, &out(guarded, search_schema())).expect_err("an impossible policy fails");
        assert_eq!(rejections.len(), 1, "{rejections:?}");
        assert_eq!(rejections[0].policy_id.as_deref(), Some("price-cap"));
        assert_eq!(rejections[0].reason, REASON_INVALID);
        // The control: a guard on a declared field stays valid.
        let declared = "@id(\"q-guard\")\nforbid(principal, action == Pgs::Gateway::Action::\"search\", resource) when { context.input has q && context.input.q == \"x\" };";
        assert_eq!(check_compiled(SetKind::Tenant, &out(declared, search_schema())), Ok(()));
    }

    #[test]
    fn text_or_a_schema_that_does_not_parse_is_invalid() {
        let rejections = check_compiled(SetKind::Tenant, &out("permit(", base_schema_json())).expect_err("bad text fails");
        assert_eq!(rejections.len(), 1, "{rejections:?}");
        assert_eq!(rejections[0].reason, REASON_INVALID);
        assert_eq!(rejections[0].policy_id, None);
        let rejections = check_compiled(SetKind::Tenant, &out("", json!({"Pgs::Gateway": {"entityTypes": 5}}))).expect_err("a bad schema fails");
        assert_eq!(rejections.len(), 1, "{rejections:?}");
        assert_eq!(rejections[0].reason, REASON_INVALID);
        assert_eq!(rejections[0].policy_id, None);
    }

    #[test]
    fn a_floor_permit_template_is_invalid() {
        let policies = "@id(\"floor-deny\")\nforbid(principal, action in Pgs::Gateway::Action::\"Unregistered\", resource);\n@id(\"floor-tmpl\")\npermit(principal == ?principal, action in Pgs::Gateway::Action::\"InvokeTool\", resource);";
        let rejections = check_compiled(SetKind::Floor, &out(policies, base_schema_json())).expect_err("a template fails");
        assert_eq!(rejections.len(), 1, "{rejections:?}");
        assert_eq!(rejections[0].policy_id.as_deref(), Some("floor-tmpl"));
        assert_eq!(rejections[0].reason, REASON_INVALID);
    }

    #[test]
    fn a_tenant_template_is_invalid() {
        let policies = "@id(\"tmpl\")\nforbid(principal, action == Pgs::Gateway::Action::\"search\", resource == ?resource);";
        let rejections = check_compiled(SetKind::Tenant, &out(policies, search_schema())).expect_err("a template fails");
        assert_eq!(rejections.len(), 1, "{rejections:?}");
        assert_eq!(rejections[0].policy_id.as_deref(), Some("tmpl"));
        assert_eq!(rejections[0].reason, REASON_INVALID);
        // A template with no @id is rejected with no policy id.
        let anonymous = "forbid(principal == ?principal, action, resource);";
        let rejections = check_compiled(SetKind::Tenant, &out(anonymous, search_schema())).expect_err("a template fails");
        assert_eq!(rejections.len(), 1, "{rejections:?}");
        assert_eq!(rejections[0].policy_id, None);
    }
}
