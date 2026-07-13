// SPDX-License-Identifier: Apache-2.0
//! Asserts every metric family referenced in the committed Grafana dashboards + Prometheus
//! alert rules is a registered family (after suffix-normalisation) — so ops artifacts can't
//! reference a metric we don't emit. `promtool` covers PromQL *validity*; this covers *name
//! drift*.
//!
//! Extraction is prefix-anchored rather than a blanket identifier scan: a PromQL `expr` mixes
//! metric names with label keys (`status_class`, `grpc_status`, `decision`, ...), function names
//! (`rate`, `sum`, `histogram_quantile`, `by`, `le`, ...), and template vars
//! (`$__rate_interval`). Every real metric in this repo is prefixed `iam_` or `gateway_`, so a
//! token only counts as a metric-name reference when it matches `^(iam|gateway)_[a-z0-9_]+$` —
//! that automatically excludes all of the above without needing a label/function blocklist.

use paigasus_observability::names::ALL;

/// Strip a trailing histogram/summary suffix so `foo_seconds_bucket` matches the registered
/// family `foo_seconds` (Prometheus expands a histogram into `_bucket`/`_sum`/`_count` series).
fn normalize(id: &str) -> &str {
    for suffix in ["_bucket", "_sum", "_count"] {
        if let Some(base) = id.strip_suffix(suffix) {
            return base;
        }
    }
    id
}

fn is_known(id: &str) -> bool {
    let normalized = normalize(id);
    ALL.contains(&normalized) || ALL.contains(&id)
}

/// Extract every `iam_`/`gateway_`-prefixed token from a PromQL expression. Tokenizing by
/// splitting on any char outside `[a-z0-9_]` and filtering to the two real metric prefixes
/// discards label keys, function names, and template vars (`$__rate_interval` splits into
/// `__rate_interval`, which fails the prefix check) without needing to enumerate them.
fn metric_idents(expr: &str) -> Vec<String> {
    expr.split(|c: char| !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'))
        .filter(|token| !token.is_empty() && (token.starts_with("iam_") || token.starts_with("gateway_")))
        .map(str::to_owned)
        .collect()
}

/// Walk a Grafana dashboard JSON doc and collect every `targets[].expr` string, recursing into
/// `panels[].panels[]` (Grafana nests panels inside "row" panels).
fn collect_exprs_from_dashboard(json: &serde_json::Value) -> Vec<String> {
    let mut exprs = Vec::new();
    if let Some(panels) = json.get("panels").and_then(serde_json::Value::as_array) {
        collect_panel_exprs(panels, &mut exprs);
    }
    exprs
}

fn collect_panel_exprs(panels: &[serde_json::Value], exprs: &mut Vec<String>) {
    for panel in panels {
        if let Some(targets) = panel.get("targets").and_then(serde_json::Value::as_array) {
            for target in targets {
                if let Some(expr) = target.get("expr").and_then(serde_json::Value::as_str) {
                    exprs.push(expr.to_owned());
                }
            }
        }
        if let Some(nested) = panel.get("panels").and_then(serde_json::Value::as_array) {
            collect_panel_exprs(nested, exprs);
        }
    }
}

/// Walk a Prometheus alert-rules YAML doc and collect every `groups[].rules[].expr` string.
fn collect_exprs_from_rules(doc: &serde_norway::Value) -> Vec<String> {
    let mut exprs = Vec::new();
    if let Some(groups) = doc.get("groups").and_then(serde_norway::Value::as_sequence) {
        for group in groups {
            if let Some(rules) = group.get("rules").and_then(serde_norway::Value::as_sequence) {
                for rule in rules {
                    if let Some(expr) = rule.get("expr").and_then(serde_norway::Value::as_str) {
                        exprs.push(expr.to_owned());
                    }
                }
            }
        }
    }
    exprs
}

#[test]
fn dashboards_and_rules_reference_only_known_metrics() {
    // repo root from crate dir: paigasus-observability -> libs -> crates -> rs -> repo root
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../..");
    let mut unknown: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();

    let dashboards = ["ops/observability/grafana/dashboards/iam.json", "ops/observability/grafana/dashboards/gateway.json"];
    for path in dashboards {
        let full = format!("{root}/{path}");
        let text = std::fs::read_to_string(&full).unwrap_or_else(|e| panic!("read {full}: {e}"));
        let json: serde_json::Value = serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse {full}: {e}"));
        for expr in collect_exprs_from_dashboard(&json) {
            for id in metric_idents(&expr) {
                if !is_known(&id) {
                    unknown.insert(format!("{path}: {id}"));
                }
            }
        }
    }

    let rule_files = ["ops/observability/prometheus/rules/iam.rules.yml", "ops/observability/prometheus/rules/gateway.rules.yml"];
    for path in rule_files {
        let full = format!("{root}/{path}");
        let text = std::fs::read_to_string(&full).unwrap_or_else(|e| panic!("read {full}: {e}"));
        let doc: serde_norway::Value = serde_norway::from_str(&text).unwrap_or_else(|e| panic!("parse {full}: {e}"));
        for expr in collect_exprs_from_rules(&doc) {
            for id in metric_idents(&expr) {
                if !is_known(&id) {
                    unknown.insert(format!("{path}: {id}"));
                }
            }
        }
    }

    assert!(
        unknown.is_empty(),
        "dashboards/rules reference unknown metrics (not in paigasus_observability::names::ALL):\n{}",
        unknown.into_iter().collect::<Vec<_>>().join("\n")
    );
}
