// SPDX-License-Identifier: Apache-2.0
#![cfg(feature = "tool-policy")]
//! The `decide` function of the tool policy (SMA-752, Task 3).

use std::sync::Arc;

use paigasus_kernel::detect::{HOSTS_OVERFLOW, HostSet, Severity, call_hosts};
use paigasus_kernel::tool_policy::{
    ActionGroup, Declared, Effect, Facts, Findings, PolicyOrigin, PolicyRef, RequestInfo, SessionInfo, ToolInput, ToolInvocation, ToolPolicies, UNPARSED_ARGUMENTS, decide,
};
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
    findings.add("name_collision", Severity::High);
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

fn bad_facts() -> Facts {
    Facts::new(
        CedarUid {
            entity_type: "not a type".into(),
            entity_id: "x".into(),
        },
        ORG.parse().expect("uuid"),
        RequestInfo::new("s", "responses", "m"),
        SessionInfo::placeholder(),
    )
}

fn findings(kinds: &[&str]) -> Findings {
    let mut f = Findings::new();
    for k in kinds {
        f.add(k, if *k == "name_collision" { Severity::High } else { Severity::Medium });
    }
    f
}

#[test]
fn the_verdict_holds_the_final_findings() {
    // The input is not JSON, so the kernel adds unparsed_arguments. The caller gave name_collision.
    let p = policies("", "", &[]);
    let v = decide(&call("x", "not json"), &facts(), &findings(&["name_collision"]), &p);
    assert_eq!(v.findings.iter().map(String::as_str).collect::<Vec<_>>(), ["name_collision", "unparsed_arguments"]);
}

#[test]
fn a_catalog_name_is_in_the_catalog_group_and_another_is_unregistered() {
    let p = policies("", "", &["Read_File"]);
    assert_eq!(decide(&call(" read_file", "{}"), &facts(), &Findings::new(), &p).action_group, ActionGroup::Catalog);
    assert_eq!(decide(&call("rm", "{}"), &facts(), &Findings::new(), &p).action_group, ActionGroup::Unregistered);
    // The accessor gives the same group with no decision.
    assert_eq!(p.action_group(" read_file"), ActionGroup::Catalog);
    assert_eq!(p.action_group("rm"), ActionGroup::Unregistered);
}

#[test]
fn an_unevaluable_request_keeps_the_caller_findings() {
    let p = policies("", "", &[]);
    let v = decide(&call("x", "{}"), &bad_facts(), &findings(&["name_collision"]), &p);
    assert!(v.floor_denied);
    assert!(v.findings.contains("name_collision"));
}

// SMA-855 Task 8: `context.max_severity`, `context.call.hosts` and the new `Verdict` fields.

fn p_all() -> ToolPolicies {
    policies("", ALL, &["t"])
}

/// A JSON object whose one string holds `n` distinct hosts.
fn json_with_hosts(n: usize) -> String {
    let urls: Vec<String> = (0..n).map(|i| format!("https://h{i:03}.example/")).collect();
    format!(r#"{{"u":"{}"}}"#, urls.join(" "))
}

fn json_with_257_hosts() -> String {
    json_with_hosts(257)
}

fn is_kernel_detector_error(f: &paigasus_kernel::detect::Finding) -> bool {
    f.kind == "detector_error" && f.detector == "kernel" && f.severity == Severity::High
}

#[test]
fn max_severity_is_minus_one_with_no_findings_and_the_highest_otherwise() {
    let p = policies("", r#"@id("all") permit(principal, action, resource) when { context.max_severity == -1 };"#, &["t"]);
    let v = decide(&call("t", "{}"), &facts(), &Findings::new(), &p);
    assert_eq!((v.decision, v.max_severity), (Effect::Allow, None));
    let mut f = Findings::new();
    f.add("x", Severity::Low);
    f.add("y", Severity::High);
    let v = decide(&call("t", "{}"), &facts(), &f, &p);
    assert_eq!((v.decision, v.max_severity), (Effect::Deny, Some(Severity::High)));
    let p3 = policies("", r#"@id("all") permit(principal, action, resource) when { context.max_severity == 3 };"#, &["t"]);
    assert_eq!(decide(&call("t", "{}"), &facts(), &f, &p3).decision, Effect::Allow);
}

#[test]
fn a_forbid_on_any_severity_does_not_deny_a_clean_call() {
    // A4: a clean call has -1, so `>= 0` is false and the forbid neither fires nor errors.
    let p = policies(r#"@id("f") forbid(principal, action, resource) when { context.max_severity >= 0 };"#, ALL, &["t"]);
    let v = decide(&call("t", "{}"), &facts(), &Findings::new(), &p);
    assert_eq!((v.decision, v.errored.len()), (Effect::Allow, 0));
    let mut f = Findings::new();
    f.add("x", Severity::Info);
    assert_eq!(decide(&call("t", "{}"), &facts(), &f, &p).decision, Effect::Deny);
}

#[test]
fn hosts_are_sorted_unique_and_a_precomputed_set_is_used_as_given() {
    let p = policies("", r#"@id("h") permit(principal, action, resource) when { context.call.hosts == ["a.example", "b.example"] };"#, &["t"]);
    let scanned = call("t", r#"{"u":"https://b.example/ https://a.example/ https://a.example/x"}"#);
    assert_eq!(decide(&scanned, &facts(), &Findings::new(), &p).decision, Effect::Allow);
    let mut given = HostSet::new();
    given.insert("a.example");
    given.insert("b.example");
    // The input has no host: only the given set can make the policy allow, so the kernel did not scan.
    assert_eq!(decide(&call("t", "{}").with_hosts(given), &facts(), &Findings::new(), &p).decision, Effect::Allow);
    assert_eq!(decide(&call("t", "{}"), &facts(), &Findings::new(), &p).decision, Effect::Deny);
    // A set from `call_hosts` gives the same verdict as the kernel's own scan.
    let precomputed = scanned.clone().with_hosts(call_hosts(&scanned.input));
    assert_eq!(decide(&precomputed, &facts(), &Findings::new(), &p), decide(&scanned, &facts(), &Findings::new(), &p));
    let empty = policies("", r#"@id("h") permit(principal, action, resource) when { context.call.hosts.isEmpty() };"#, &["t"]);
    assert_eq!(decide(&call("t", "{}"), &facts(), &Findings::new(), &empty).decision, Effect::Allow);
}

#[test]
fn the_same_hosts_from_either_path_for_many_inputs() {
    let p = policies(
        "",
        r#"@id("h") permit(principal, action, resource) when { context.call.hosts.contains("h000.example") && context.call.hosts.contains("<overflow>") };"#,
        &["t"],
    );
    for input in [
        ToolInput::Json(json_with_257_hosts()),
        ToolInput::Json(json_with_hosts(3)),
        ToolInput::Text("see https://x.example/ and https://y.example".into()),
        ToolInput::Json(format!(r#"{{"u":"https://{}.example/"}}"#, "a".repeat(1100))),
    ] {
        let c = ToolInvocation::new("t", input, Declared::Yes);
        let pre = c.clone().with_hosts(call_hosts(&c.input));
        assert_eq!(decide(&pre, &facts(), &Findings::new(), &p), decide(&c, &facts(), &Findings::new(), &p));
    }
    assert_eq!(decide(&call("t", &json_with_257_hosts()), &facts(), &Findings::new(), &p).decision, Effect::Allow);
}

#[test]
fn detection_off_writes_minus_one_and_no_hosts() {
    let p = policies(
        "",
        r#"@id("d") permit(principal, action, resource) when { context.max_severity == -1 && context.call.hosts.isEmpty() };"#,
        &["t"],
    );
    let mut f = Findings::new();
    f.add("x", Severity::High);
    let off = facts().with_detection(false);
    let c = call("t", r#"{"u":"https://a.example/"}"#);
    let v = decide(&c, &off, &f, &p);
    assert_eq!((v.decision, v.max_severity), (Effect::Allow, None));
    assert!(v.findings.contains("x"), "the kinds set stays as today");
    assert_eq!(decide(&c, &facts(), &f, &p).decision, Effect::Deny, "detection on: the same call is denied");
    // No hosts-cap finding with detection off, from a scan or from a given set.
    let v = decide(&call("t", &json_with_257_hosts()), &off, &Findings::new(), &p);
    assert!(!v.added.iter().any(is_kernel_detector_error));
    assert!(!v.findings.contains("detector_error"));
    let given = call_hosts(&ToolInput::Json(json_with_257_hosts()));
    assert!(given.overflowed());
    let v = decide(&call("t", "{}").with_hosts(given), &off, &Findings::new(), &p);
    assert_eq!(v.decision, Effect::Allow);
    assert!(v.added.is_empty());
}

#[test]
fn the_kernel_findings_are_in_added_with_their_severity() {
    let v = decide(&ToolInvocation::new("t", ToolInput::Text("p".into()), Declared::Yes), &facts(), &Findings::new(), &p_all());
    assert_eq!(
        (v.added[0].kind.as_str(), v.added[0].severity, v.max_severity),
        ("unparsed_arguments", Severity::Info, Some(Severity::Info))
    );
    assert_eq!(v.added[0].detector, "kernel");
    let v = decide(&call("t", r#"{"a":1,"a":2}"#), &facts(), &Findings::new(), &p_all());
    assert_eq!(
        (v.added[0].kind.as_str(), v.added[0].severity, v.max_severity),
        ("unparsed_arguments", Severity::Medium, Some(Severity::Medium))
    );
    let v = decide(&call("t", &json_with_257_hosts()), &facts(), &Findings::new(), &p_all());
    assert!(v.added.iter().any(is_kernel_detector_error), "{v:?}");
    assert_eq!(v.max_severity, Some(Severity::High));
    assert!(v.findings.contains("detector_error"));
    let v = decide(&call("t", "{}"), &facts(), &Findings::new(), &p_all());
    assert!(v.added.is_empty());
    assert_eq!(v.max_severity, None);
}

#[test]
fn the_hosts_cap_reaches_the_context_and_the_findings() {
    // D36: the deny list of § 6.12 pairs a host match with `detector_error`.
    let deny = r#"@id("d") forbid(principal, action, resource) when { context.call.hosts.containsAny(["zzz.example"]) || context.findings.contains("detector_error") };"#;
    let p = policies("", &format!("{ALL} {deny}"), &["t"]);
    // 256 hosts is at the cap: no overflow.
    let v = decide(&call("t", &json_with_hosts(256)), &facts(), &Findings::new(), &p);
    assert_eq!((v.decision, v.added.len(), v.max_severity), (Effect::Allow, 0, None));
    // 257 hosts: the first 255 and the sentinel, and a kernel `detector_error`.
    let v = decide(&call("t", &json_with_257_hosts()), &facts(), &Findings::new(), &p);
    assert_eq!(v.decision, Effect::Deny);
    assert_eq!(v.added.iter().filter(|f| is_kernel_detector_error(f)).count(), 1);
    let shape = format!(
        r#"@id("s") permit(principal, action, resource) when {{ context.call.hosts.contains("{HOSTS_OVERFLOW}") && context.call.hosts.contains("h254.example") && !context.call.hosts.contains("h255.example") }};"#
    );
    assert_eq!(
        decide(&call("t", &json_with_257_hosts()), &facts(), &Findings::new(), &policies("", &shape, &["t"])).decision,
        Effect::Allow
    );
    // An authority longer than 1024 bytes overflows the set too, with no other host.
    let long = call("t", &format!(r#"{{"u":"https://{}.example/"}}"#, "a".repeat(1100)));
    let v = decide(&long, &facts(), &Findings::new(), &p);
    assert_eq!((v.decision, v.max_severity), (Effect::Deny, Some(Severity::High)));
    assert!(v.added.iter().any(is_kernel_detector_error));
    let only = format!(r#"@id("o") permit(principal, action, resource) when {{ context.call.hosts == ["{HOSTS_OVERFLOW}"] }};"#);
    assert_eq!(decide(&long, &facts(), &Findings::new(), &policies("", &only, &["t"])).decision, Effect::Allow);
    // A given set that overflowed gives the finding as well.
    let given = call_hosts(&ToolInput::Json(json_with_257_hosts()));
    let v = decide(&call("t", "{}").with_hosts(given), &facts(), &Findings::new(), &p);
    assert_eq!(v.decision, Effect::Deny);
    assert!(v.added.iter().any(is_kernel_detector_error));
}

#[test]
fn errored_forbids_hold_only_forbids() {
    let tenant = r#"@id("p") permit(principal, action, resource) when { context.input.missing == 1 };
        @id("f") forbid(principal, action, resource) when { context.input.other == 1 };
        @id("a") forbid(principal, action, resource) when { context.input has other && context.input.other == 1 };"#;
    let p = policies("", tenant, &["t"]);
    let v = decide(&call("t", "{}"), &facts(), &Findings::new(), &p);
    assert_eq!((v.errored.len(), v.errored_forbids.clone()), (2, vec![pref(PolicyOrigin::Tenant, "f")]));
    assert!(v.errored_forbid);
    let floor = r#"@id("z") forbid(principal, action, resource) when { context.input.q == 1 };"#;
    let p = policies(floor, tenant, &["t"]);
    let v = decide(&call("t", "{}"), &facts(), &Findings::new(), &p);
    assert_eq!(v.errored_forbids, vec![pref(PolicyOrigin::Floor, "z"), pref(PolicyOrigin::Tenant, "f")]);
    let v = decide(&call("t", "{}"), &facts(), &Findings::new(), &p_all());
    assert!(v.errored_forbids.is_empty());
}

#[test]
fn determining_forbids_hold_only_forbids() {
    // A deny by an errored forbid: Cedar's reason set is the satisfied permit `all`.
    let tenant = r#"@id("all") permit(principal, action, resource);
        @id("e") forbid(principal, action, resource) when { context.input.missing == 1 };"#;
    let v = decide(&call("t", "{}"), &facts(), &Findings::new(), &policies("", tenant, &["t"]));
    assert_eq!((v.decision, v.determining.clone()), (Effect::Deny, vec![pref(PolicyOrigin::Tenant, "all")]));
    assert!(v.determining_forbids.is_empty());
    // A deny by satisfied forbids of both sets.
    let floor = r#"@id("z") forbid(principal, action, resource);"#;
    let tenant = r#"@id("all") permit(principal, action, resource); @id("f") forbid(principal, action, resource);"#;
    let v = decide(&call("t", "{}"), &facts(), &Findings::new(), &policies(floor, tenant, &["t"]));
    assert_eq!(v.determining_forbids, vec![pref(PolicyOrigin::Floor, "z"), pref(PolicyOrigin::Tenant, "f")]);
    // An allow: the determining permits are not forbids.
    let v = decide(&call("t", "{}"), &facts(), &Findings::new(), &p_all());
    assert!(!v.determining.is_empty() && v.determining_forbids.is_empty());
}

#[test]
fn a_whole_record_literal_on_call_is_false() {
    // Release note of § 6.2.3: `context.call` now has two attributes.
    let p = policies("", r#"@id("c") permit(principal, action, resource) when { context.call == {"declared": "yes"} };"#, &["t"]);
    assert_eq!(decide(&call("t", "{}"), &facts(), &Findings::new(), &p).decision, Effect::Deny);
    let p = policies("", r#"@id("c") permit(principal, action, resource) when { context.call.declared == "yes" };"#, &["t"]);
    assert_eq!(decide(&call("t", "{}"), &facts(), &Findings::new(), &p).decision, Effect::Allow);
}

#[test]
fn an_unevaluable_verdict_has_the_caller_severity_and_empty_lists() {
    let v = decide(&call("x", "not json"), &bad_facts(), &findings(&["name_collision"]), &policies("", "", &[]));
    assert_eq!(v.max_severity, Some(Severity::High));
    assert!(v.added.is_empty() && v.errored_forbids.is_empty() && v.determining_forbids.is_empty());
}
