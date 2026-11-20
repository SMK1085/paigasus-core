// SPDX-License-Identifier: Apache-2.0
//! The compile steps and the accepted temporal shapes (spec 2026-11-15 iam-tool-policy-sets,
//! section 5.3).

use paigasus_policy_compiler::compile::{Compiled, ReasonCode, Rejection, SetKind, compile_cedarschema};
use paigasus_policy_compiler::translate::{CmpOp, EventKind, HistoryCondition, Literal, Predicate, Scope, Shape};

const SCHEMA: &str = include_str!("fixtures/test_schema.cedarschema");
const DAY_MS: u64 = 86_400_000;
const GW: &str = "Pgs::Gateway::Action";

fn tenant(source: &str) -> Result<Compiled, Vec<Rejection>> {
    compile_cedarschema(SetKind::Tenant, source, SCHEMA, DAY_MS)
}

fn floor(source: &str) -> Result<Compiled, Vec<Rejection>> {
    compile_cedarschema(SetKind::Floor, source, SCHEMA, DAY_MS)
}

fn accepted(result: Result<Compiled, Vec<Rejection>>) -> Compiled {
    result.unwrap_or_else(|rejections| panic!("the set was rejected: {rejections:#?}"))
}

fn rejected(result: Result<Compiled, Vec<Rejection>>) -> Vec<Rejection> {
    match result {
        Ok(compiled) => panic!("the set compiled: {compiled:#?}"),
        Err(rejections) => rejections,
    }
}

/// The only rejection of a set, which must have `reason`.
fn only_rejection(result: Result<Compiled, Vec<Rejection>>, reason: ReasonCode) -> Rejection {
    let rejections = rejected(result);
    assert_eq!(rejections.len(), 1, "{rejections:#?}");
    assert_eq!(rejections[0].reason, reason, "{rejections:#?}");
    rejections[0].clone()
}

/// A tenant permit on `search` with one temporal block.
fn one_block(id: &str, block: &str) -> String {
    format!("@id(\"{id}\")\npermit (principal, action == {GW}::\"search\", resource)\nwhen temporal {{ {block} }};\n")
}

fn only_condition(compiled: &Compiled) -> &HistoryCondition {
    assert_eq!(compiled.conditions.len(), 1, "{:#?}", compiled.conditions);
    &compiled.conditions[0]
}

fn all_of(left: Predicate, right: Predicate) -> Predicate {
    Predicate::AllOf(Box::new(left), Box::new(right))
}

fn tool(name: &str) -> Predicate {
    Predicate::Tool { name_key: name.to_string() }
}

#[test]
fn formerly_within_is_one_formerly_condition() {
    let compiled = accepted(tenant(&one_block("f", &format!("formerly within 10m {GW}::\"send\"::response{{}}"))));
    let condition = only_condition(&compiled);
    assert_eq!(condition.window_ms, 600_000);
    assert_eq!(condition.policy_id, "f");
    assert_eq!(condition.scope, Scope::Session);
    assert_eq!(
        condition.shape,
        Shape::Formerly {
            predicate: all_of(Predicate::EventKind(EventKind::Response), tool("send"))
        }
    );
    assert!(compiled.cedar_policies.contains(&condition.slot), "{}", compiled.cedar_policies);
}

#[test]
fn count_within_with_each_comparison_is_accepted() {
    let ops = [("<", CmpOp::Lt), ("<=", CmpOp::Le), ("==", CmpOp::Eq), (">=", CmpOp::Ge), (">", CmpOp::Gt), ("!=", CmpOp::Ne)];
    for (token, op) in ops {
        let direct = format!("count_within(1h, {GW}::\"search\"::request{{ input.q: \"x\" }}) {token} 2");
        let bound = format!("bind(c, count_within(1h, {GW}::\"search\"::request{{ input.q: \"x\" }}), c {token} 2)");
        for block in [direct, bound] {
            let compiled = accepted(tenant(&one_block("c", &block)));
            let condition = only_condition(&compiled);
            assert_eq!(condition.window_ms, 3_600_000, "{block}");
            let expected_predicate = all_of(
                all_of(Predicate::EventKind(EventKind::Request), tool("search")),
                Predicate::Field {
                    path: vec!["q".to_string()],
                    op: CmpOp::Eq,
                    value: Literal::String("x".to_string()),
                },
            );
            assert_eq!(
                condition.shape,
                Shape::CountWithin {
                    predicate: expected_predicate,
                    op,
                    threshold: 2
                },
                "{block}"
            );
        }
    }
}

#[test]
fn sum_within_of_a_long_field_is_accepted() {
    let direct = format!("sum_within(a, 1h, {GW}::\"send\"::request{{ input.amount: a }}) > 100");
    let bound = format!("bind(s, sum_within(a, 1h, {GW}::\"send\"::request{{ input.amount: a, input.to: \"x\" }}), s >= 100)");
    let compiled = accepted(tenant(&one_block("s", &direct)));
    assert_eq!(
        only_condition(&compiled).shape,
        Shape::SumWithin {
            predicate: all_of(Predicate::EventKind(EventKind::Request), tool("send")),
            field_path: vec!["amount".to_string()],
            op: CmpOp::Gt,
            threshold: 100,
        }
    );
    let compiled = accepted(tenant(&one_block("s", &bound)));
    match &only_condition(&compiled).shape {
        Shape::SumWithin { predicate, field_path, op, threshold } => {
            assert_eq!(field_path, &vec!["amount".to_string()]);
            assert_eq!((*op, *threshold), (CmpOp::Ge, 100));
            let to = Predicate::Field {
                path: vec!["to".to_string()],
                op: CmpOp::Eq,
                value: Literal::String("x".to_string()),
            };
            assert_eq!(predicate, &all_of(all_of(Predicate::EventKind(EventKind::Request), tool("send")), to));
        }
        other => panic!("not a sum: {other:?}"),
    }
}

#[test]
fn a_field_comparison_inside_a_predicate_is_accepted() {
    let block = format!("formerly within 10m (exists (v: Long). ({GW}::\"search\"::request{{ input.n: v }} && v > 5))");
    let compiled = accepted(tenant(&one_block("n", &block)));
    let field = Predicate::Field {
        path: vec!["n".to_string()],
        op: CmpOp::Gt,
        value: Literal::Long(5),
    };
    assert_eq!(
        only_condition(&compiled).shape,
        Shape::Formerly {
            predicate: all_of(all_of(Predicate::EventKind(EventKind::Request), tool("search")), field)
        }
    );
}

#[test]
fn and_not_and_any_catalog_tool_are_accepted() {
    // The surface grammar has no `||`: Dogwood makes `Or` only in its pin rewrite, which the
    // compiler does not use (D2). So `AnyOf` has no source form here.
    let block = format!("formerly within 10m ({GW}::\"InvokeTool\"::request{{}} && !{GW}::\"search\"::request{{ input.q: \"z\" }})");
    let compiled = accepted(tenant(&one_block("b", &block)));
    let any = all_of(Predicate::EventKind(EventKind::Request), Predicate::AnyCatalogTool);
    let search = all_of(
        all_of(Predicate::EventKind(EventKind::Request), tool("search")),
        Predicate::Field {
            path: vec!["q".to_string()],
            op: CmpOp::Eq,
            value: Literal::String("z".to_string()),
        },
    );
    assert_eq!(
        only_condition(&compiled).shape,
        Shape::Formerly {
            predicate: all_of(any, Predicate::Negation(Box::new(search)))
        }
    );
}

#[test]
fn previous_and_since_are_shape_unsupported() {
    let previous = format!("previous within 10m {GW}::\"send\"::request{{}}");
    let since = format!("{GW}::\"send\"::request{{}} since within 10m {GW}::\"search\"::request{{}}");
    for (block, node) in [(previous, "Previous"), (since, "Since")] {
        let rejection = only_rejection(tenant(&one_block("p", &block)), ReasonCode::ToolPolicyShapeUnsupported);
        assert_eq!(rejection.policy_id.as_deref(), Some("p"));
        assert!(rejection.message.contains(node), "{}", rejection.message);
    }
}

#[test]
fn count_distinct_within_is_shape_unsupported() {
    let block = format!("count_distinct_within(k, 1h, {GW}::\"search\"::request{{ input.q: k }}) > 2");
    let rejection = only_rejection(tenant(&one_block("d", &block)), ReasonCode::ToolPolicyShapeUnsupported);
    assert!(rejection.message.contains("count_distinct_within"), "{}", rejection.message);
}

#[test]
fn a_two_shape_block_is_rejected() {
    let block = format!("formerly within 10m {GW}::\"send\"::request{{}} && count_within(1h, {GW}::\"search\"::request{{}}) > 1");
    let rejection = only_rejection(tenant(&one_block("t", &block)), ReasonCode::ToolPolicyShapeUnsupported);
    assert!(rejection.message.contains("two-shapes"), "{}", rejection.message);
}

#[test]
fn an_error_event_pattern_is_rejected() {
    let block = format!("formerly within 10m {GW}::\"send\"::error{{}}");
    let rejection = only_rejection(tenant(&one_block("e", &block)), ReasonCode::ToolPolicyShapeUnsupported);
    assert!(rejection.message.contains("error"), "{}", rejection.message);
}

#[test]
fn a_context_reference_in_a_predicate_is_rejected() {
    let block = format!("formerly within 10m {GW}::\"search\"::request{{ input.q: context.input.q }}");
    let rejection = only_rejection(tenant(&one_block("x", &block)), ReasonCode::ToolPolicyShapeUnsupported);
    assert!(rejection.message.contains("Term::ContextField"), "{}", rejection.message);
}

#[test]
fn a_missing_or_duplicate_or_malformed_id_is_rejected() {
    let plain = |annotation: &str| format!("{annotation}\npermit (principal, action == {GW}::\"search\", resource);\n");
    let missing = plain("");
    let duplicate = format!("{}{}", plain("@id(\"a\")"), plain("@id(\"a\")"));
    let malformed = plain("@id(\"Bad Id\")");
    for (source, policy_id) in [(missing, None), (duplicate, Some("a")), (malformed, None)] {
        let rejection = only_rejection(tenant(&source), ReasonCode::ToolPolicyIdInvalid);
        assert_eq!(rejection.policy_id.as_deref(), policy_id, "{source}");
    }
}

/// Dogwood keeps every annotation, but the emitted Cedar annotation map keeps only one `@id`.
/// The compiler could then check one id and emit another, so a second `@id` is rejected.
#[test]
fn a_policy_with_two_ids_is_rejected() {
    let block = format!("permit (principal, action == {GW}::\"search\", resource)\nwhen temporal {{ formerly within 1m {GW}::\"send\"::request{{}} }};\n");
    for annotations in ["@id(\"a\")\n@id(\"b\")", "@id(\"a\")\n@id(\"a\")"] {
        let source = format!("{annotations}\n{block}");
        let rejection = only_rejection(tenant(&source), ReasonCode::ToolPolicyIdInvalid);
        assert_eq!(rejection.policy_id, None, "{source}");
        assert!(rejection.message.contains("more than one @id"), "{}", rejection.message);
    }
    // The same set with one `@id` compiles, and the second policy still has its own id.
    let two_policies = format!("@id(\"a\")\n{block}@id(\"b\")\n{block}");
    assert_eq!(accepted(tenant(&two_policies)).conditions.len(), 2);
}

#[test]
fn an_empty_or_bare_id_is_rejected_like_a_malformed_one() {
    for annotation in ["@id(\"\")", "@id"] {
        let source = format!("{annotation}\npermit (principal, action == {GW}::\"search\", resource);\n");
        only_rejection(tenant(&source), ReasonCode::ToolPolicyIdInvalid);
    }
}

#[test]
fn an_id_outside_the_grammar_never_reaches_lowering() {
    // A temporal block makes the lowering use the distincter, which panics on an id outside the
    // grammar. The `@id` check must reject these before lowering.
    for annotation in ["", "@id(\"-lead\")", "@id(\"a.b\")", "@id(\"ÄÖ\")", &format!("@id(\"{}\")", "a".repeat(65))] {
        let source = format!("{annotation}\npermit (principal, action == {GW}::\"search\", resource)\nwhen temporal {{ formerly within 1m {GW}::\"send\"::request{{}} }};\n");
        only_rejection(tenant(&source), ReasonCode::ToolPolicyIdInvalid);
    }
}

#[test]
fn the_floor_prefix_rule_holds_in_both_directions() {
    let tenant_source = format!("@id(\"floor-x\")\npermit (principal, action == {GW}::\"search\", resource);\n");
    let rejection = only_rejection(tenant(&tenant_source), ReasonCode::ToolPolicyIdInvalid);
    assert_eq!(rejection.policy_id.as_deref(), Some("floor-x"));
    let floor_source = format!("@id(\"x\")\nforbid (principal, action == {GW}::\"search\", resource);\n");
    let rejection = only_rejection(floor(&floor_source), ReasonCode::ToolPolicyIdInvalid);
    assert_eq!(rejection.policy_id.as_deref(), Some("x"));
    let good_floor = format!("@id(\"floor-x\")\nforbid (principal, action == {GW}::\"search\", resource);\n");
    accepted(floor(&good_floor));
}

#[test]
fn an_unknown_scope_is_rejected() {
    for annotation in ["@scope(\"global\")", "@scope(\"\")", "@scope", "@scope(\"Session\")"] {
        let source = format!("@id(\"a\")\n{annotation}\npermit (principal, action == {GW}::\"search\", resource);\n");
        only_rejection(tenant(&source), ReasonCode::ToolPolicyIdInvalid);
    }
}

/// Dogwood keeps every annotation, but the emitted Cedar annotation map keeps only the last
/// `@scope`. The condition could then use one scope and the Cedar text another.
#[test]
fn a_policy_with_two_scopes_is_rejected() {
    let block = format!("formerly within 10m {GW}::\"send\"::response{{}}");
    for annotations in ["@scope(\"session\")\n@scope(\"principal\")", "@scope(\"key\")\n@scope(\"key\")"] {
        let source = format!("{annotations}\n{}", one_block("a", &block));
        let rejection = only_rejection(tenant(&source), ReasonCode::ToolPolicyIdInvalid);
        assert_eq!(rejection.policy_id.as_deref(), Some("a"), "{source}");
        assert!(rejection.message.contains("more than one @scope"), "{}", rejection.message);
    }
}

#[test]
fn scope_defaults_to_session() {
    let block = format!("formerly within 10m {GW}::\"send\"::response{{}}");
    let compiled = accepted(tenant(&one_block("a", &block)));
    assert_eq!(only_condition(&compiled).scope, Scope::Session);
    for (value, scope) in [("session", Scope::Session), ("principal", Scope::Principal), ("key", Scope::Key)] {
        let source = format!("@scope(\"{value}\")\n{}", one_block("a", &block));
        let compiled = accepted(tenant(&source));
        assert_eq!(only_condition(&compiled).scope, scope);
    }
}

#[test]
fn a_floor_permit_is_rejected_in_every_form() {
    let head = format!("@id(\"floor-p\")\npermit (principal, action == {GW}::\"search\", resource)");
    let forms = [
        format!("{head};\n"),
        format!("{head}\nwhen {{ context.input.n > 3 }};\n"),
        format!("{head}\nwhen temporal {{ formerly within 1m {GW}::\"send\"::request{{}} }};\n"),
    ];
    for source in forms {
        let rejection = only_rejection(floor(&source), ReasonCode::ToolPolicyFloorPermit);
        assert_eq!(rejection.policy_id.as_deref(), Some("floor-p"));
    }
}

#[test]
fn a_window_over_the_limit_is_rejected() {
    let block = format!("formerly within 2h {GW}::\"send\"::request{{}}");
    let source = one_block("w", &block);
    let rejection = only_rejection(compile_cedarschema(SetKind::Tenant, &source, SCHEMA, 3_600_000), ReasonCode::ToolPolicyWindowTooLong);
    assert_eq!(rejection.policy_id.as_deref(), Some("w"));
    accepted(compile_cedarschema(SetKind::Tenant, &source, SCHEMA, 7_200_000));
}

#[test]
fn a_non_normal_action_literal_is_rejected() {
    let in_scope = |literal: &str| format!("@id(\"a\")\npermit (principal, action == {GW}::\"{literal}\", resource);\n");
    let in_list = |literal: &str| format!("@id(\"a\")\npermit (principal, action in [{GW}::\"search\", {GW}::\"{literal}\"], resource);\n");
    let in_predicate = |literal: &str| one_block("a", &format!("formerly within 1m {GW}::\"{literal}\"::request{{}}"));
    for literal in ["Send", "invoketool", "unregistered", "sénd", "a.b.c", ""] {
        for source in [in_scope(literal), in_list(literal), in_predicate(literal)] {
            let rejection = only_rejection(tenant(&source), ReasonCode::ToolPolicyInvalid);
            assert_eq!(rejection.policy_id.as_deref(), Some("a"), "{source}");
        }
    }
}

#[test]
fn a_non_normal_action_literal_in_a_when_body_is_rejected() {
    // The pre-lowering check does not see a `when` body. Dogwood validation rejects the literal,
    // because a non-normal literal is never an action of a generated schema.
    for literal in ["Send", "invoketool", "sénd"] {
        let source = format!("@id(\"a\")\npermit (principal, action, resource)\nwhen {{ action == {GW}::\"{literal}\" }};\n");
        // Dogwood 1.2.0 (SMA-800) also reports that the policy is impossible, so there are exactly
        // two rejections. Both must be `tool-policy-invalid` and must name the policy. A change
        // in this count means that Dogwood validation changed, so read it before you edit this.
        let rejections = rejected(tenant(&source));
        assert_eq!(rejections.len(), 2, "{source}: {rejections:#?}");
        for rejection in &rejections {
            assert_eq!(rejection.reason, ReasonCode::ToolPolicyInvalid, "{rejections:#?}");
            assert_eq!(rejection.policy_id.as_deref(), Some("a"), "{source}");
        }
    }
}

#[test]
fn a_provider_invocation_is_rejected() {
    let source = format!("@id(\"a\")\npermit (principal, action == {GW}::\"search\", resource)\nwhen {{ Ns::Fn(context.input.q) }};\n");
    let rejection = only_rejection(tenant(&source), ReasonCode::ToolPolicyInvalid);
    assert_eq!(rejection.policy_id.as_deref(), Some("a"));
}

#[test]
fn a_parse_error_is_invalid_with_a_location() {
    let rejection = only_rejection(tenant("@id(\"a\")\npermit (principal, action, resource) when {"), ReasonCode::ToolPolicyInvalid);
    assert!(rejection.line.is_some() && rejection.column.is_some(), "{rejection:?}");
}

#[test]
fn a_type_error_is_invalid_for_its_policy() {
    let source =
        format!("@id(\"ok\")\npermit (principal, action == {GW}::\"search\", resource);\n@id(\"bad\")\npermit (principal, action == {GW}::\"search\", resource) when {{ context.input.q > 3 }};\n");
    let rejections = rejected(tenant(&source));
    assert!(!rejections.is_empty());
    for rejection in &rejections {
        assert_eq!(rejection.reason, ReasonCode::ToolPolicyInvalid, "{rejections:#?}");
        assert_eq!(rejection.policy_id.as_deref(), Some("bad"), "{rejections:#?}");
    }
}

#[test]
fn a_hyphenated_id_with_a_temporal_block_compiles() {
    let compiled = accepted(tenant(&one_block("rate-limit-search", &format!("count_within(1h, {GW}::\"search\"::request{{}}) < 10"))));
    let condition = only_condition(&compiled);
    assert!(condition.slot.starts_with("p_rate_hlimit_hsearch_0__temporal_"), "{}", condition.slot);
    assert_eq!(condition.policy_id, "rate-limit-search");
}

#[test]
fn slot_names_stay_when_an_earlier_policy_is_deleted() {
    let a = one_block("a", &format!("formerly within 10m {GW}::\"send\"::response{{}}"));
    let b = one_block("b", &format!("count_within(1h, {GW}::\"search\"::request{{}}) > 1"));
    let both = accepted(tenant(&format!("{b}{a}")));
    let only_a = accepted(tenant(&a));
    let of_a = |compiled: &Compiled| compiled.conditions.iter().filter(|c| c.policy_id == "a").cloned().collect::<Vec<_>>();
    assert_eq!(of_a(&both).len(), 1);
    assert_eq!(of_a(&both), of_a(&only_a));
}

#[test]
fn conditions_are_sorted_by_slot() {
    let mut source = String::new();
    for id in ["z", "m", "a"] {
        source.push_str(&format!(
            "@id(\"{id}\")\npermit (principal, action == {GW}::\"search\", resource)\nwhen temporal {{ formerly within 1m {GW}::\"send\"::request{{}} }}\nwhen temporal {{ formerly within 2m {GW}::\"send\"::response{{}} }};\n"
        ));
    }
    let compiled = accepted(tenant(&source));
    let slots: Vec<&str> = compiled.conditions.iter().map(|c| c.slot.as_str()).collect();
    assert_eq!(slots.len(), 6);
    let mut sorted = slots.clone();
    sorted.sort();
    assert_eq!(slots, sorted);
    assert!(slots[0].starts_with("p_a_0__temporal_"), "{slots:?}");
}

#[test]
fn empty_source_compiles_to_an_empty_set() {
    for source in ["", "// only a comment\n"] {
        let compiled = accepted(tenant(source));
        assert!(compiled.conditions.is_empty());
        assert_eq!(compiled.cedar_policies, "");
        let actions = &compiled.cedar_schema_json["Pgs::Gateway"]["actions"];
        assert!(actions.get("search").is_some(), "{}", compiled.cedar_schema_json);
    }
}
