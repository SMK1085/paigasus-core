// SPDX-License-Identifier: Apache-2.0

//! IAM's own serde types for the compiler protocol (spec 2026-11-15 iam-tool-policy-sets,
//! section 5.2). Each side defines the JSON types once: these mirror the compiler's
//! `src/protocol.rs`, and the tests read the shared fixtures in
//! `rs/tools/paigasus-policy-compiler/protocol/fixtures/`, which are the drift control.
//!
//! Every struct and every tagged enum refuses an unknown field or an unknown tag. A result that
//! IAM cannot read completely fails closed: the caller reports `tool-policy-compiler-unavailable`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The protocol version that IAM writes and accepts.
pub const PROTOCOL_VERSION: u32 = 1;

/// The kind of a tool policy set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SetKind {
    Tenant,
    Floor,
}

/// One compile request (stdin of `paigasus-policy-compiler compile`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompileRequest {
    pub protocol_version: u32,
    pub kind: SetKind,
    /// The Dogwood source of the whole set.
    pub source: String,
    /// The generated action schema in the Cedar JSON schema form.
    pub schema_json: Value,
    /// The longest window that a temporal block may use.
    pub max_window_ms: u64,
}

/// One compile result (stdout). A rejection is a normal result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case", deny_unknown_fields)]
pub enum CompileResult {
    Accepted {
        protocol_version: u32,
        compiler_generation: u32,
        compiled: CompiledOutput,
    },
    Rejected {
        protocol_version: u32,
        compiler_generation: u32,
        rejections: Vec<Rejection>,
    },
}

impl CompileResult {
    /// The protocol version of the result. IAM checks it on every result.
    pub fn protocol_version(&self) -> u32 {
        match self {
            CompileResult::Accepted { protocol_version, .. } | CompileResult::Rejected { protocol_version, .. } => *protocol_version,
        }
    }

    /// The compiler generation that wrote the result.
    pub fn compiler_generation(&self) -> u32 {
        match self {
            CompileResult::Accepted { compiler_generation, .. } | CompileResult::Rejected { compiler_generation, .. } => *compiler_generation,
        }
    }
}

/// The compiled set (spec section 5.4): the lowered Cedar text, the lowered schema with the
/// slot attributes, and one history condition for each slot, sorted by `slot`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompiledOutput {
    pub cedar_policies: String,
    pub cedar_schema_json: Value,
    pub conditions: Vec<HistoryCondition>,
}

/// One rejection. `reason` is the wire reason code (spec section 7) as a string, so that the
/// caller decides what an unknown code means. `policy_id`, `line` and `column` are `null` on
/// the wire when they have no value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rejection {
    pub policy_id: Option<String>,
    pub line: Option<u32>,
    pub column: Option<u32>,
    pub reason: String,
    pub message: String,
}

/// The output of `paigasus-policy-compiler revision` (spec section 6.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RevisionOutput {
    pub compiler_generation: u32,
    pub protocol_version: u32,
    pub revision: String,
}

/// One temporal slot of the lowered set, as the gateway evaluates it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryCondition {
    /// The slot attribute in the lowered Cedar context.
    pub slot: String,
    /// The `@id` of the policy the slot came from.
    pub policy_id: String,
    pub scope: Scope,
    pub window_ms: u64,
    pub shape: Shape,
}

/// The history that a condition counts over.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Session,
    Principal,
    Key,
}

/// A comparison operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CmpOp {
    Lt,
    Le,
    Eq,
    Ge,
    Gt,
    Ne,
}

/// The kind of a recorded event: a call (`request`) or a result (`response`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    Request,
    Response,
}

/// A constant in a field comparison. On the wire: `{"type": "long", "value": 500}`. A decimal
/// keeps its text form (`"1.50"`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case", deny_unknown_fields)]
pub enum Literal {
    Long(i64),
    String(String),
    Bool(bool),
    Decimal(String),
}

/// A predicate on one recorded event. On the wire: an object with a `"type"` and the named
/// fields of the node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Predicate {
    EventKind {
        kind: EventKind,
    },
    /// The event is a call of the tool with this match key.
    Tool {
        name_key: String,
    },
    /// The event is a call of any tool in the catalog (`InvokeTool`).
    AnyCatalogTool,
    /// The argument field at `path` (below `input`) compares with `value`.
    Field {
        path: Vec<String>,
        op: CmpOp,
        value: Literal,
    },
    AllOf {
        left: Box<Predicate>,
        right: Box<Predicate>,
    },
    AnyOf {
        left: Box<Predicate>,
        right: Box<Predicate>,
    },
    Negation {
        inner: Box<Predicate>,
    },
}

/// The shape of a history condition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Shape {
    Formerly {
        predicate: Predicate,
    },
    CountWithin {
        predicate: Predicate,
        op: CmpOp,
        threshold: i64,
    },
    SumWithin {
        predicate: Predicate,
        field_path: Vec<String>,
        op: CmpOp,
        threshold: i64,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::{Path, PathBuf};

    fn fixtures_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../tools/paigasus-policy-compiler/protocol/fixtures")
    }

    fn read_json(path: &Path) -> Value {
        let text = std::fs::read_to_string(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        serde_json::from_str(&text).unwrap_or_else(|error| panic!("parse {}: {error}", path.display()))
    }

    #[test]
    fn every_compiler_fixture_round_trips() {
        let mut requests = 0;
        let mut results = 0;
        let mut entries: Vec<PathBuf> = std::fs::read_dir(fixtures_dir())
            .expect("the fixture directory exists")
            .map(|e| e.expect("a directory entry").path())
            .collect();
        entries.sort();
        for path in entries {
            let name = path.file_name().and_then(|n| n.to_str()).expect("a UTF-8 file name").to_owned();
            let original = read_json(&path);
            let round_trip = if name.starts_with("request_") {
                requests += 1;
                let request: CompileRequest = serde_json::from_value(original.clone()).unwrap_or_else(|error| panic!("{name}: {error}"));
                serde_json::to_value(&request).expect("a request serializes")
            } else if name.starts_with("result_") {
                results += 1;
                let result: CompileResult = serde_json::from_value(original.clone()).unwrap_or_else(|error| panic!("{name}: {error}"));
                serde_json::to_value(&result).expect("a result serializes")
            } else {
                continue;
            };
            assert_eq!(round_trip, original, "{name} does not round-trip");
        }
        assert_eq!((requests, results), (4, 4), "the fixture directory must hold four requests and four results");
    }

    #[test]
    fn the_result_fixtures_cover_both_outcomes_and_two_shapes() {
        let mut shapes = std::collections::BTreeSet::new();
        let mut outcomes = std::collections::BTreeSet::new();
        for name in ["result_tenant_accepted.json", "result_tenant_rejected.json", "result_floor_accepted.json", "result_floor_rejected.json"] {
            let result: CompileResult = serde_json::from_value(read_json(&fixtures_dir().join(name))).expect("a result parses");
            assert_eq!(result.protocol_version(), PROTOCOL_VERSION);
            match result {
                CompileResult::Accepted { compiled, .. } => {
                    outcomes.insert("accepted");
                    for condition in compiled.conditions {
                        shapes.insert(match condition.shape {
                            Shape::Formerly { .. } => "formerly",
                            Shape::CountWithin { .. } => "count_within",
                            Shape::SumWithin { .. } => "sum_within",
                        });
                    }
                }
                CompileResult::Rejected { rejections, .. } => {
                    outcomes.insert("rejected");
                    assert!(!rejections.is_empty());
                }
            }
        }
        assert_eq!(outcomes.len(), 2);
        assert!(shapes.contains("formerly") && shapes.contains("count_within"), "{shapes:?}");
    }

    #[test]
    fn an_unknown_shape_or_enum_value_does_not_parse() {
        let condition = |shape: Value| json!({"slot": "s", "policy_id": "p", "scope": "session", "window_ms": 1, "shape": shape});
        let good = condition(json!({"type": "formerly", "predicate": {"type": "any_catalog_tool"}}));
        serde_json::from_value::<HistoryCondition>(good).expect("the control parses");
        for bad in [
            condition(json!({"type": "avg_within", "predicate": {"type": "any_catalog_tool"}})),
            condition(json!({"type": "formerly", "predicate": {"type": "error"}})),
            condition(json!({"type": "count_within", "predicate": {"type": "any_catalog_tool"}, "op": "like", "threshold": 1})),
            condition(json!({"type": "formerly", "predicate": {"type": "event_kind", "kind": "error"}})),
            json!({"slot": "s", "policy_id": "p", "scope": "tenant", "window_ms": 1, "shape": {"type": "formerly", "predicate": {"type": "any_catalog_tool"}}}),
            json!({"slot": "s", "policy_id": "p", "scope": "session", "window_ms": 1, "extra": 1, "shape": {"type": "formerly", "predicate": {"type": "any_catalog_tool"}}}),
        ] {
            assert!(serde_json::from_value::<HistoryCondition>(bad.clone()).is_err(), "{bad} parsed");
        }
    }

    #[test]
    fn every_literal_type_round_trips() {
        for literal in [
            json!({"type": "long", "value": 500}),
            json!({"type": "string", "value": "x"}),
            json!({"type": "bool", "value": true}),
            json!({"type": "decimal", "value": "1.50"}),
        ] {
            let parsed: Literal = serde_json::from_value(literal.clone()).expect("a literal parses");
            assert_eq!(serde_json::to_value(&parsed).expect("serializes"), literal);
        }
    }

    #[test]
    fn the_revision_output_parses() {
        let text = r#"{"compiler_generation":1,"protocol_version":1,"revision":"1+dogwood.ff158506cfd9+proto.1"}"#;
        let revision: RevisionOutput = serde_json::from_str(text).expect("the revision parses");
        assert_eq!(revision.compiler_generation, 1);
        assert_eq!(serde_json::to_string(&revision).expect("serializes"), text);
    }
}
