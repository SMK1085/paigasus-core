// SPDX-License-Identifier: Apache-2.0
#![cfg(feature = "tool-policy")]
//! The `decide` function of the tool policy (SMA-752, Task 3).

use std::sync::Arc;

use paigasus_kernel::tool_policy::{Declared, Effect, Facts, Findings, PolicyOrigin, PolicyRef, RequestInfo, SessionInfo, ToolInput, ToolInvocation, ToolPolicies, UNPARSED_ARGUMENTS, decide};
use paigasus_kernel::{CedarUid, Prn, to_cedar_uid};

const ORG: &str = "0190a100-0000-7000-8000-0000000000aa";

fn facts() -> Facts {
    let prn = Prn::parse("prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-000000000004").expect("valid PRN");
    Facts::new(
        to_cedar_uid(&prn),
        ORG.parse().expect("uuid"),
        RequestInfo::new("prn:pgs:gateway:::scope/x", "chat_completions", "m"),
        SessionInfo::placeholder(),
    )
}

fn call(name: &str, json: &str) -> ToolInvocation {
    ToolInvocation::new(name, ToolInput::Json(json.into()), Declared::Yes)
}

fn policies(floor: &str, tenant: &str, catalog: &[&str]) -> ToolPolicies {
    let floor = Arc::new(ToolPolicies::compile_floor(floor).expect("floor"));
    let catalog: Vec<String> = catalog.iter().map(|s| (*s).to_owned()).collect();
    ToolPolicies::compile(floor, tenant, &catalog).expect("tenant")
}

const BIG: &str = r#"@id("big") forbid(principal, action, resource) when { context.input.amount > 1000 };"#;
const ALL: &str = r#"@id("all") permit(principal, action, resource);"#;

fn pref(set: PolicyOrigin, id: &str) -> PolicyRef {
    PolicyRef::new(set, id)
}

#[test]
fn an_errored_forbid_denies() {
    let p = policies(BIG, ALL, &["pay"]);
    let v = decide(&call("pay", r#"{"amount": 1500.5}"#), &facts(), &Findings::new(), &p);
    assert_eq!(v.decision, Effect::Deny);
    assert!(v.errored_forbid);
    assert!(v.floor_denied);
    assert_eq!(v.errored, vec![pref(PolicyOrigin::Floor, "big")]);
}

#[test]
fn an_integer_amount_is_evaluated_not_errored() {
    let p = policies(BIG, ALL, &["pay"]);
    let v = decide(&call("pay", r#"{"amount": 1500}"#), &facts(), &Findings::new(), &p);
    assert_eq!((v.decision, v.errored_forbid, v.floor_denied), (Effect::Deny, false, true));
    assert_eq!(v.determining, vec![pref(PolicyOrigin::Floor, "big")]);
    let v = decide(&call("pay", r#"{"amount": 5}"#), &facts(), &Findings::new(), &p);
    assert_eq!((v.decision, v.floor_denied), (Effect::Allow, false));
    assert_eq!(v.determining, vec![pref(PolicyOrigin::Tenant, "all")]);
}

#[test]
fn every_permit_errored_is_a_normal_deny() {
    let tenant = r#"@id("p") permit(principal, action, resource) when { context.input.amount > 1000 };"#;
    let p = policies("", tenant, &["pay"]);
    let v = decide(&call("pay", r#"{"amount": 1500.5}"#), &facts(), &Findings::new(), &p);
    assert_eq!(v.decision, Effect::Deny);
    assert!(!v.errored_forbid);
    assert!(!v.floor_denied);
    assert_eq!(v.errored, vec![pref(PolicyOrigin::Tenant, "p")]);
}

#[test]
fn an_errored_tenant_forbid_denies_but_is_not_a_floor_deny() {
    let tenant = r#"@id("t") forbid(principal, action, resource) when { context.input.amount > 1000 };"#;
    let p = policies("", &format!("{ALL} {tenant}"), &["pay"]);
    let v = decide(&call("pay", r#"{"amount": 1500.5}"#), &facts(), &Findings::new(), &p);
    assert_eq!((v.decision, v.errored_forbid, v.floor_denied), (Effect::Deny, true, false));
}

#[test]
fn a_floor_forbid_wins_over_a_tenant_permit() {
    let floor = r#"@id("x") forbid(principal, action == Pgs::Gateway::Action::"rm", resource);"#;
    let p = policies(floor, ALL, &["rm"]);
    let v = decide(&call("RM", "{}"), &facts(), &Findings::new(), &p);
    assert_eq!(v.decision, Effect::Deny);
    assert_eq!(v.determining, vec![pref(PolicyOrigin::Floor, "x")]);
    assert!(v.floor_denied && !v.errored_forbid);
}

#[test]
fn a_floor_forbid_that_names_a_tool_applies_outside_the_catalog() {
    let floor = r#"@id("x") forbid(principal, action == Pgs::Gateway::Action::"rm", resource);"#;
    let p = policies(floor, ALL, &[]);
    let v = decide(&call("rm", "{}"), &facts(), &Findings::new(), &p);
    assert_eq!((v.decision, v.floor_denied), (Effect::Deny, true));
}

#[test]
fn unregistered_is_denied_by_default_and_allowed_by_a_permit() {
    let none = policies("", "", &["known"]);
    let v = decide(&call("ghost", "{}"), &facts(), &Findings::new(), &none);
    assert_eq!((v.decision, v.floor_denied), (Effect::Deny, false));
    assert!(v.determining.is_empty());

    let tenant = r#"@id("u") permit(principal, action in Pgs::Gateway::Action::"Unregistered", resource);"#;
    let p = policies("", tenant, &["known"]);
    assert_eq!(decide(&call("ghost", "{}"), &facts(), &Findings::new(), &p).decision, Effect::Allow);
    assert_eq!(decide(&call("known", "{}"), &facts(), &Findings::new(), &p).decision, Effect::Deny);

    let floor = r#"@id("g") forbid(principal, action == Pgs::Gateway::Action::"ghost", resource);"#;
    let p = policies(floor, tenant, &["known"]);
    let v = decide(&call("ghost", "{}"), &facts(), &Findings::new(), &p);
    assert_eq!((v.decision, v.floor_denied), (Effect::Deny, true));
}

#[test]
fn the_floor_alone_has_no_floor_denied_default_deny() {
    let floor = Arc::new(ToolPolicies::compile_floor(BIG).expect("floor"));
    let p = ToolPolicies::compile(floor, "", &[]).expect("floor only");
    let v = decide(&call("pay", r#"{"amount": 5}"#), &facts(), &Findings::new(), &p);
    assert_eq!((v.decision, v.floor_denied, v.errored_forbid), (Effect::Deny, false, false));
}

#[test]
fn context_shape() {
    let tenant = format!(
        r#"@id("c") permit(principal is Pgs::Iam::Principal, action, resource == Pgs::Iam::Organization::"{ORG}") when {{
            context.request.ingress == "chat_completions" && context.request.model == "m"
            && context.request.scope == "prn:pgs:gateway:::scope/x"
            && context.call.declared == "yes" && context.session.turn == 0 && context.session.id == ""
            && context.findings.contains("name_collision") && context.input.a == 1
        }};"#
    );
    let p = policies("", &tenant, &["t"]);
    let mut findings = Findings::new();
    findings.insert("name_collision");
    let v = decide(&call("t", r#"{"a": 1}"#), &facts(), &findings, &p);
    assert_eq!(v.decision, Effect::Allow, "{v:?}");
    let v = decide(&call("t", r#"{"a": 1}"#), &facts(), &Findings::new(), &p);
    assert_eq!(v.decision, Effect::Deny);
    let no = ToolInvocation::new("t", ToolInput::Json(r#"{"a": 1}"#.into()), Declared::No);
    assert_eq!(decide(&no, &facts(), &findings, &p).decision, Effect::Deny);
}

#[test]
fn an_argument_policy_on_a_path() {
    let tenant = r#"@id("r") permit(principal, action == Pgs::Gateway::Action::"read_file", resource)
        when { context.input has path && context.input.path like "/workspace/*" };"#;
    let p = policies("", tenant, &["read_file"]);
    let ok = decide(&call("read_file", r#"{"path": "/workspace/a"}"#), &facts(), &Findings::new(), &p);
    assert_eq!(ok.decision, Effect::Allow);
    let bad = decide(&call("read_file", r#"{"path": "/etc/passwd"}"#), &facts(), &Findings::new(), &p);
    assert_eq!(bad.decision, Effect::Deny);
}

#[test]
fn unparsed_arguments_reach_the_findings() {
    let tenant = format!(
        r#"@id("u") permit(principal, action, resource) when {{
            context.findings.contains("{UNPARSED_ARGUMENTS}") && context has input_text && context.input_text == "*** patch"
        }};"#
    );
    let p = policies("", &tenant, &["apply_patch"]);
    let text = ToolInvocation::new("apply_patch", ToolInput::Text("*** patch".into()), Declared::Yes);
    assert_eq!(decide(&text, &facts(), &Findings::new(), &p).decision, Effect::Allow);
    // Invalid JSON is unparsed too, but it has no input_text.
    let bad = call("apply_patch", "{not json");
    assert_eq!(decide(&bad, &facts(), &Findings::new(), &p).decision, Effect::Deny);
    let tenant2 = format!(r#"@id("u") permit(principal, action, resource) when {{ context.findings.contains("{UNPARSED_ARGUMENTS}") }};"#);
    let p2 = policies("", &tenant2, &["apply_patch"]);
    assert_eq!(decide(&bad, &facts(), &Findings::new(), &p2).decision, Effect::Allow);
}

#[test]
fn a_context_slot_the_gateway_does_not_fill_fails_closed() {
    let forbid = r#"@id("h") forbid(principal, action, resource) when { context.history.count > 3 };"#;
    let p = policies(forbid, ALL, &["t"]);
    let v = decide(&call("t", "{}"), &facts(), &Findings::new(), &p);
    assert_eq!((v.decision, v.errored_forbid, v.floor_denied), (Effect::Deny, true, true));
    let permit = r#"@id("h") permit(principal, action, resource) when { context.history.count > 3 };"#;
    let p = policies("", permit, &["t"]);
    let v = decide(&call("t", "{}"), &facts(), &Findings::new(), &p);
    assert_eq!((v.decision, v.errored_forbid), (Effect::Deny, false));
}

#[test]
fn the_name_is_normalised_before_the_action_lookup() {
    let tenant = r#"@id("r") permit(principal, action == Pgs::Gateway::Action::"read_file", resource);"#;
    let p = policies("", tenant, &["Read_File"]);
    assert_eq!(decide(&call(" READ_FILE ", "{}"), &facts(), &Findings::new(), &p).decision, Effect::Allow);
}

#[test]
fn a_request_that_cannot_be_built_is_floor_denied() {
    let bad = Facts::new(
        CedarUid {
            entity_type: "not a type".into(),
            entity_id: "x".into(),
        },
        ORG.parse().expect("uuid"),
        RequestInfo::new("s", "responses", "m"),
        SessionInfo::placeholder(),
    );
    let p = policies(BIG, ALL, &["pay"]);
    let v = decide(&call("pay", "{}"), &bad, &Findings::new(), &p);
    assert_eq!((v.decision, v.floor_denied, v.errored_forbid), (Effect::Deny, true, true));
}
