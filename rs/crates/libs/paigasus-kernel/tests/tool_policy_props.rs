// SPDX-License-Identifier: Apache-2.0
#![cfg(feature = "tool-policy")]
//! Soundness of `decide_tool` (§ 6.1.6, § 7.1, SMA-752 Task 4): `DeniedForAllInputs` implies that
//! `decide` denies every real call of the tool, with the same principal, org and request, and any
//! input, findings, `declared` value and session. `by_floor` implies a floor deny.

use std::fmt;
use std::sync::Arc;

use paigasus_kernel::CedarUid;
use paigasus_kernel::tool_policy::{Declared, Effect, Facts, Findings, RequestInfo, SessionInfo, ToolInput, ToolInvocation, ToolPolicies, ToolVerdict, decide, decide_tool};
use proptest::prelude::*;
use proptest::sample::{select, subsequence};
use proptest::strategy::ValueTree;
use proptest::test_runner::TestRunner;

const ORG: &str = "0190a100-0000-7000-8000-0000000000aa";
const NAMES: [&str; 5] = ["read_file", "Read_File ", "rm", "send_email", "x"];
const CATALOG: [&str; 3] = ["read_file", "rm", "send_email"];
const FINDINGS: [&str; 3] = ["name_collision", "unparsed_arguments", "ok"];

/// One atom of a condition. The keys `a` and `b` and the values match the generated inputs, so
/// that conditions are often true and often false.
fn atom() -> impl Strategy<Value = String> {
    let key = select(vec!["a", "b"]);
    let value = select(vec!["1", "3", "\"s\"", "true"]);
    prop_oneof![
        key.clone().prop_map(|k| format!("context.input has {k}")),
        Just("context has input_text".to_owned()),
        (key.clone(), value).prop_map(|(k, v)| format!("context.input.{k} == {v}")),
        key.clone().prop_map(|k| format!("context.input.{k} < 2")),
        key.clone().prop_map(|k| format!("context.input.{k} like \"*s*\"")),
        Just("context.input_text like \"*rm*\"".to_owned()),
        key.clone().prop_map(|k| format!("[1, 3].contains(context.input.{k})")),
        key.clone().prop_map(|k| format!("context.input.{k}.contains(1)")),
        Just("context.input == {}".to_owned()),
        Just("context.input == {a: 1}".to_owned()),
        key.prop_map(|k| format!("context.input.{k}.lessThan(decimal(\"2.0\"))")),
        Just("decimal(\"1.5\").lessThan(decimal(\"2.0\"))".to_owned()),
        select(vec!["yes", "no", "unknown"]).prop_map(|d| format!("context.call.declared == \"{d}\"")),
        select(FINDINGS.to_vec()).prop_map(|f| format!("context.findings.contains(\"{f}\")")),
        select(vec!["m1", "m2"]).prop_map(|m| format!("context.request.model == \"{m}\"")),
        Just("context.request.ingress == \"responses\"".to_owned()),
        Just("context.session.turn > 0".to_owned()),
        Just("context.session.id == \"\"".to_owned()),
        Just("action in Pgs::Gateway::Action::\"InvokeTool\"".to_owned()),
        Just("action in Pgs::Gateway::Action::\"Unregistered\"".to_owned()),
        Just("principal == Pgs::Iam::User::\"u1\"".to_owned()),
        Just("context == {}".to_owned()),
        Just("true".to_owned()),
        Just("false".to_owned()),
    ]
}

fn expr() -> impl Strategy<Value = String> {
    atom().prop_recursive(2, 8, 3, |inner| {
        prop_oneof![
            inner.clone().prop_map(|e| format!("!({e})")),
            (inner.clone(), inner.clone()).prop_map(|(a, b)| format!("({a} && {b})")),
            (inner.clone(), inner.clone()).prop_map(|(a, b)| format!("({a} || {b})")),
            (inner.clone(), inner.clone(), inner).prop_map(|(a, b, c)| format!("(if {a} then {b} else {c})")),
        ]
    })
}

fn clauses() -> impl Strategy<Value = String> {
    prop::collection::vec((any::<bool>(), expr()), 0..=2).prop_map(|cs| cs.into_iter().map(|(when, e)| format!(" {} {{ {e} }}", if when { "when" } else { "unless" })).collect())
}

fn scope() -> impl Strategy<Value = (String, String)> {
    let principal = select(vec!["principal", "principal == Pgs::Iam::User::\"u1\"", "principal is Pgs::Iam::User"]);
    let action = select(vec![
        "action",
        "action == Pgs::Gateway::Action::\"read_file\"",
        "action == Pgs::Gateway::Action::\"rm\"",
        "action in Pgs::Gateway::Action::\"InvokeTool\"",
        "action in Pgs::Gateway::Action::\"Unregistered\"",
        "action in [Pgs::Gateway::Action::\"rm\", Pgs::Gateway::Action::\"x\"]",
    ]);
    (principal, action).prop_map(|(p, a)| (p.to_owned(), a.to_owned()))
}

/// A policy body without its `@id`: effect (`true` is permit), scope and conditions.
fn policy(permit: impl Strategy<Value = bool>) -> impl Strategy<Value = (bool, (String, String), String)> {
    (permit, scope(), clauses())
}

fn render(prefix: &str, ps: &[(bool, (String, String), String)]) -> String {
    ps.iter()
        .enumerate()
        .map(|(i, (permit, (p, a), c))| format!("@id(\"{prefix}{i}\") {}({p}, {a}, resource){c};\n", if *permit { "permit" } else { "forbid" }))
        .collect()
}

fn input() -> impl Strategy<Value = ToolInput> {
    prop_oneof![
        select(vec![
            "{}",
            r#"{"a": 1}"#,
            r#"{"a": "s"}"#,
            r#"{"a": true, "b": [1, 2]}"#,
            r#"{"b": 1.5}"#,
            r#"{"a": 3, "b": "s"}"#,
            "not json",
            r#"{"a": 1, "a": 2}"#
        ])
        .prop_map(|t| ToolInput::Json(t.to_owned())),
        select(vec!["rm -rf /", "ok"]).prop_map(|t| ToolInput::Text(t.to_owned())),
    ]
}

fn findings() -> impl Strategy<Value = Findings> {
    subsequence(FINDINGS.to_vec(), 0..=FINDINGS.len()).prop_map(|ks| {
        let mut f = Findings::new();
        ks.iter().for_each(|k| f.insert(k));
        f
    })
}

fn session() -> impl Strategy<Value = SessionInfo> {
    (select(vec!["", "s"]), 0i64..=1).prop_map(|(id, turn)| SessionInfo::new(id, turn))
}

struct Case {
    floor: String,
    tenant: String,
    catalog: Vec<String>,
    name: String,
    facts: Facts,
    request_findings: Findings,
    policies: ToolPolicies,
    call: ToolInvocation,
    facts_with_session: Facts,
    call_findings: Findings,
}

impl fmt::Debug for Case {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Case")
            .field("floor", &self.floor)
            .field("tenant", &self.tenant)
            .field("catalog", &self.catalog)
            .field("name", &self.name)
            .field("facts", &self.facts)
            .field("request_findings", &self.request_findings)
            .field("call", &self.call)
            .field("facts_with_session", &self.facts_with_session)
            .field("call_findings", &self.call_findings)
            .finish_non_exhaustive()
    }
}

fn facts(principal: &str, model: &str, session: SessionInfo) -> Facts {
    let uid = CedarUid {
        entity_type: "Pgs::Iam::User".into(),
        entity_id: principal.into(),
    };
    Facts::new(uid, ORG.parse().expect("uuid"), RequestInfo::new("prn:pgs:gateway:::scope/x", "responses", model), session)
}

fn case_strategy() -> impl Strategy<Value = Case> {
    let sets = (
        prop::collection::vec(policy(Just(false)), 0..=4),
        prop::collection::vec(policy(any::<bool>()), 0..=6),
        subsequence(CATALOG.to_vec(), 0..=CATALOG.len()),
    );
    let request = (select(NAMES.to_vec()), select(vec!["u1", "u2"]), select(vec!["m1", "m2"]), session(), findings());
    let call = (input(), select(vec![Declared::Yes, Declared::No, Declared::Unknown]), session(), findings());
    (sets, request, call).prop_map(
        |((floor, tenant, catalog), (name, principal, model, tool_session, request_findings), (input, declared, call_session, call_findings))| {
            let floor = render("f", &floor);
            let tenant = render("t", &tenant);
            let catalog: Vec<String> = catalog.into_iter().map(str::to_owned).collect();
            let compiled = Arc::new(ToolPolicies::compile_floor(&floor).expect("the generated floor compiles"));
            let policies = ToolPolicies::compile(compiled, &tenant, &catalog).expect("the generated tenant set compiles");
            Case {
                floor,
                tenant,
                catalog,
                name: name.to_owned(),
                facts: facts(principal, model, tool_session),
                request_findings,
                policies,
                call: ToolInvocation::new(name, input, declared),
                facts_with_session: facts(principal, model, call_session),
                call_findings,
            }
        },
    )
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 2048, ..ProptestConfig::default() })]
    #[test]
    fn denied_for_all_inputs_implies_deny(case in case_strategy()) {
        if let ToolVerdict::DeniedForAllInputs { by_floor } = decide_tool(&case.name, &case.facts, &case.request_findings, &case.policies) {
            let v = decide(&case.call, &case.facts_with_session, &case.call_findings, &case.policies);
            prop_assert_eq!(v.decision, Effect::Deny);
            if by_floor { prop_assert!(v.floor_denied); }
        }
    }
}

/// The property is not vacuous: the generator reaches every verdict, and calls that are allowed.
#[test]
fn the_generator_reaches_every_verdict() {
    let mut runner = TestRunner::deterministic();
    let strategy = case_strategy();
    let (mut floor, mut tenant, mut may, mut allowed) = (0, 0, 0, 0);
    for _ in 0..2048 {
        let case = strategy.new_tree(&mut runner).expect("a case").current();
        match decide_tool(&case.name, &case.facts, &case.request_findings, &case.policies) {
            ToolVerdict::DeniedForAllInputs { by_floor: true } => floor += 1,
            ToolVerdict::DeniedForAllInputs { by_floor: false } => tenant += 1,
            _ => may += 1,
        }
        if decide(&case.call, &case.facts_with_session, &case.call_findings, &case.policies).decision == Effect::Allow {
            allowed += 1;
        }
    }
    assert!(
        floor >= 50 && tenant >= 50 && may >= 50 && allowed >= 50,
        "floor {floor}, tenant {tenant}, may {may}, allowed {allowed}"
    );
}
