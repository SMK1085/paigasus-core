// SPDX-License-Identifier: Apache-2.0
//! Asserts every metric family referenced in the committed Grafana dashboards + Prometheus
//! alert rules is a registered family (after suffix-normalisation) — so ops artifacts can't
//! reference a metric we don't emit. `promtool` covers PromQL *validity*; this covers *name
//! drift*. The dashboard and rules files are discovered by globbing their directories (not
//! hardcoded), and each glob's recursion matches that directory's real consumer: the dashboards
//! glob walks subdirectories because Grafana's file provisioner does too, while the rules glob
//! stays flat because Prometheus's `rule_files` glob and the `repo:promtool` gate are both
//! non-recursive.
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

/// Every entry under `dir_rel` whose file name ends with `suffix`, as `(repo_relative, absolute)`
/// pairs sorted for a deterministic panic order. Reads use the absolute path; failure messages use
/// the repo-relative one, so output stays clean (`root` is an unnormalized `../../../..` chain).
///
/// `recursive` picks whether subdirectories are walked, so each call site can match what its real
/// consumer actually loads: Prometheus's `rule_files: .../*.rules.yml` glob and the
/// `repo:promtool` gate are both non-recursive (flat), while Grafana's dashboard file provisioner
/// walks the whole `path` tree — `foldersFromFilesStructure` only controls folder *mapping*, not
/// *discovery* — so a nested dashboard is live in Grafana and must be recursed into, or this guard
/// would read zero of it and go green (SMA-466).
///
/// Fail-closed by construction: an unreadable directory or entry — including a non-UTF-8 file
/// name, or a `read_dir`/`file_type` I/O error — panics naming the path rather than being
/// swallowed into an empty list, which would make this whole test pass vacuously. A directory
/// whose own name happens to end with `suffix` is never treated as a matching file
/// (`file_type().is_dir()` is checked first). Callers additionally assert a known-good sentinel
/// file is present.
fn files_in(root: &str, dir_rel: &str, suffix: &str, recursive: bool) -> Vec<(String, String)> {
    let dir_abs = format!("{root}/{dir_rel}");
    let mut out = Vec::new();
    walk_dir(&dir_abs, dir_rel, suffix, recursive, &mut out);
    out.sort();
    out
}

/// Recursion helper for [`files_in`]; see its doc comment for the fail-closed contract.
fn walk_dir(dir_abs: &str, dir_rel: &str, suffix: &str, recursive: bool, out: &mut Vec<(String, String)>) {
    for entry in std::fs::read_dir(dir_abs).unwrap_or_else(|e| panic!("read_dir {dir_rel}: {e}")) {
        let entry = entry.unwrap_or_else(|e| panic!("read_dir entry in {dir_rel}: {e}"));
        // NB: `Path::ends_with` matches whole path COMPONENTS, so `path.ends_with(".rules.yml")`
        // compiles and is false for every file. Match the file name as a &str instead — as an
        // owned `String` via `into_string`, not `Path::to_str`, which would silently drop a
        // non-UTF-8 name from the list instead of failing closed.
        let name = entry.file_name().into_string().unwrap_or_else(|n| panic!("non-UTF-8 file name in {dir_rel}: {n:?}"));
        let file_type = entry.file_type().unwrap_or_else(|e| panic!("file_type {dir_rel}/{name}: {e}"));
        let child_rel = format!("{dir_rel}/{name}");
        if file_type.is_dir() {
            if recursive {
                walk_dir(&format!("{dir_abs}/{name}"), &child_rel, suffix, recursive, out);
            }
            continue;
        }
        if name.ends_with(suffix) {
            out.push((child_rel, format!("{dir_abs}/{name}")));
        }
    }
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

    // recursive: Grafana's dashboard file provisioner walks the whole `path` tree, so a nested
    // dashboard is live in Grafana and must be visible to this guard too.
    let dashboards = files_in(root, "ops/observability/grafana/dashboards", ".json", true);
    assert!(
        dashboards.iter().any(|(rel, _)| rel.ends_with("/iam.json")),
        "dashboard glob found {} file(s) but not the known-good iam.json — wrong directory?\n{dashboards:#?}",
        dashboards.len()
    );
    for (path, full) in &dashboards {
        let text = std::fs::read_to_string(full).unwrap_or_else(|e| panic!("read {path}: {e}"));
        let json: serde_json::Value = serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse {path}: {e}"));
        for expr in collect_exprs_from_dashboard(&json) {
            for id in metric_idents(&expr) {
                if !is_known(&id) {
                    unknown.insert(format!("{path}: {id}"));
                }
            }
        }
    }

    // recursive: false — Prometheus's `rule_files: .../*.rules.yml` glob and the `repo:promtool`
    // gate are both non-recursive, so a flat read matches what actually gets loaded.
    let rule_files = files_in(root, "ops/observability/prometheus/rules", ".rules.yml", false);
    assert!(
        rule_files.iter().any(|(rel, _)| rel.ends_with("/iam.rules.yml")),
        "rules glob found {} file(s) but not the known-good iam.rules.yml — wrong directory?\n{rule_files:#?}",
        rule_files.len()
    );
    for (path, full) in &rule_files {
        let text = std::fs::read_to_string(full).unwrap_or_else(|e| panic!("read {path}: {e}"));
        let doc: serde_norway::Value = serde_norway::from_str(&text).unwrap_or_else(|e| panic!("parse {path}: {e}"));
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
