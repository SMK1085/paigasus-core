// SPDX-License-Identifier: Apache-2.0

//! The stored form of a compiled set, and the helpers that turn a compile result into rejections
//! (spec sections 4.3, 5.4 and 8.1).
//!
//! [`encode_compiled`] writes the `CompiledToolPolicySet` protobuf bytes that a version stores.
//! [`decode_compiled`] reads them back to a [`CompiledOutput`]. The catalog coupling compares the
//! decoded live output with a new one through `outputs_identical` only. It never compares bytes.

use paigasus_iam_core::tool_policy::{CompiledOutput, ProtoError, ToolEntry, ToolPolicySetKey, condition_from_pb, condition_to_pb};
use paigasus_proto::paigasus::iam::v1 as pb;
use prost::Message;

use super::rejection::{RejectionReason, ToolPolicyRejection};
use crate::application::error::TenancyError;

/// One rejection per policy id that has a history condition, sorted by policy id. Empty when
/// `tool_policies.history_conditions` is on (spec P13).
///
/// A live set that PR 1 stored with a history rule is not changed. While the setting is off, it
/// blocks every later catalog write for that organization, because the catalog coupling compiles
/// it again and gets this rejection (re-tagged `tool-catalog-breaks-policy`). The tenant must
/// remove the temporal block first.
pub(crate) fn history_rejections(out: &CompiledOutput, enabled: bool) -> Vec<ToolPolicyRejection> {
    if enabled {
        return Vec::new();
    }
    let ids: std::collections::BTreeSet<&str> = out.conditions.iter().map(|c| c.policy_id.as_str()).collect();
    ids.into_iter()
        .map(|id| {
            let mut rejection = ToolPolicyRejection::new(
                RejectionReason::HistoryUnsupported,
                format!(
                    "policy {id} uses a history rule (formerly, count_within or sum_within). History rules are off in this IAM (tool_policies.history_conditions = false), so this set cannot be saved. Remove the temporal block, or ask the operator to turn history rules on."
                ),
            );
            rejection.policy_id = Some(id.to_owned());
            rejection
        })
        .collect()
}

/// The values of a compiled set that do not come from the compiler output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CompiledMeta {
    pub key: ToolPolicySetKey,
    pub version: u64,
    pub compiler_generation: u32,
    pub schema_revision: u32,
    /// `None` for the floor. The proto field is then 0.
    pub catalog_revision: Option<u64>,
    pub max_window_ms: u64,
    /// The sorted `name_key` list of the catalog. Empty for the floor.
    pub catalog_names: Vec<String>,
}

/// The `CompiledToolPolicySet` bytes of one version. The message has no map fields, so the bytes
/// are deterministic.
pub(crate) fn encode_compiled(meta: &CompiledMeta, out: &CompiledOutput) -> Vec<u8> {
    let (kind, org_id) = match meta.key {
        ToolPolicySetKey::Tenant(org) => (pb::ToolPolicySetKind::Tenant, org.to_string()),
        ToolPolicySetKey::Floor => (pb::ToolPolicySetKind::Floor, String::new()),
    };
    pb::CompiledToolPolicySet {
        kind: kind as i32,
        org_id,
        version: meta.version,
        compiler_generation: meta.compiler_generation,
        schema_revision: meta.schema_revision,
        catalog_revision: meta.catalog_revision.unwrap_or(0),
        cedar_policies: out.cedar_policies.clone(),
        cedar_schema: out.cedar_schema_json.to_string(),
        conditions: out.conditions.iter().map(condition_to_pb).collect(),
        max_window_ms: meta.max_window_ms,
        catalog_names: meta.catalog_names.clone(),
    }
    .encode_to_vec()
}

/// The compiler output of stored bytes. Bytes that do not decode, or that hold an unknown enum
/// value or an empty `oneof`, are a storage defect: `Internal`.
pub(crate) fn decode_compiled(bytes: &[u8]) -> Result<CompiledOutput, TenancyError> {
    let set = pb::CompiledToolPolicySet::decode(bytes).map_err(|error| corrupt(&format!("the compiled bytes do not decode: {error}")))?;
    let cedar_schema_json = serde_json::from_str(&set.cedar_schema).map_err(|error| corrupt(&format!("the stored schema is not JSON: {error}")))?;
    let conditions = set
        .conditions
        .into_iter()
        .map(condition_from_pb)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|ProtoError::Malformed(reason)| corrupt(reason))?;
    Ok(CompiledOutput {
        cedar_policies: set.cedar_policies,
        cedar_schema_json,
        conditions,
    })
}

/// The stored bytes with the `version` field set to `version`. Every other field stays as it is.
/// Bytes that do not decode are a storage defect: `Internal` (as [`decode_compiled`]).
pub(crate) fn restamp_version(bytes: &[u8], version: u64) -> Result<Vec<u8>, TenancyError> {
    let mut set = pb::CompiledToolPolicySet::decode(bytes).map_err(|error| corrupt(&format!("the compiled bytes do not decode: {error}")))?;
    set.version = version;
    Ok(set.encode_to_vec())
}

fn corrupt(what: &str) -> TenancyError {
    tracing::error!(reason = %what, "a stored tool policy set version is corrupt");
    TenancyError::Internal
}

/// Adds the `dropped_fields` reason to a rejection whose message names a dropped path of a
/// catalog tool (spec section 4.3). A tenant reads why the field is not available, not only that
/// it is missing.
///
/// A dropped path is dotted (`a.b`), and a Cedar message often names only the leaf attribute
/// (`b`). So the full path is matched first, and then the leaf. Each match is on a whole word, so
/// `price` does not match `unit_price`.
///
/// The leaf match runs only when the leaf is unambiguous in that tool: no attribute of the mapped
/// input type (at any depth) has the same name, and no other dropped path of the tool ends in it.
/// Otherwise a message about a field that exists, for example a top-level `price` next to a
/// dropped `order.price`, would get a wrong note. A wrong note misleads the tenant; a missing note
/// only leaves the plain Cedar message. So an ambiguous leaf adds no note, and only a full-path
/// match can add one.
pub(crate) fn add_dropped_field_reasons(rejection: &mut ToolPolicyRejection, catalog: &[ToolEntry]) {
    let mut notes = Vec::new();
    for tool in catalog {
        let mut mapped = std::collections::BTreeSet::new();
        attribute_names(&tool.cedar_input_type, &mut mapped);
        for dropped in &tool.dropped_fields {
            let leaf = dropped.path.rsplit('.').next().unwrap_or(&dropped.path);
            let leaf_is_unique = leaf != dropped.path && !mapped.contains(leaf) && !tool.dropped_fields.iter().any(|other| other.path != dropped.path && other.path.rsplit('.').next() == Some(leaf));
            if names_path(&rejection.message, &dropped.path) || (leaf_is_unique && names_path(&rejection.message, leaf)) {
                notes.push(format!("the field `{}` of the tool `{}` is not in the schema: {}", dropped.path, tool.name.display, dropped.reason));
            }
        }
    }
    for note in notes {
        rejection.message.push_str("; ");
        rejection.message.push_str(&note);
    }
}

/// Every attribute name of a Cedar JSON type, at any depth (records and set elements).
fn attribute_names(ty: &serde_json::Value, out: &mut std::collections::BTreeSet<String>) {
    if let Some(attributes) = ty.get("attributes").and_then(serde_json::Value::as_object) {
        for (name, inner) in attributes {
            out.insert(name.clone());
            attribute_names(inner, out);
        }
    }
    if let Some(element) = ty.get("element") {
        attribute_names(element, out);
    }
}

fn names_path(message: &str, path: &str) -> bool {
    if path.is_empty() {
        return false;
    }
    let is_word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    message.match_indices(path).any(|(start, _)| {
        let before = message[..start].chars().next_back();
        let after = message[start + path.len()..].chars().next();
        !before.is_some_and(is_word) && !after.is_some_and(is_word)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::tool_policies::rejection::RejectionReason;
    use paigasus_iam_core::tool_policy::protocol::{CmpOp, EventKind, HistoryCondition, Literal, Predicate, Scope, Shape};
    use paigasus_iam_core::tool_policy::{CatalogLimits, ToolKind, build_manual, outputs_identical};
    use serde_json::json;
    use uuid::Uuid;

    #[test]
    fn the_history_check_is_empty_when_the_gateway_enforces_history() {
        let out = CompiledOutput {
            cedar_policies: String::new(),
            cedar_schema_json: serde_json::Value::Null,
            conditions: vec![HistoryCondition {
                slot: "s0".into(),
                policy_id: "p".into(),
                scope: Scope::Session,
                window_ms: 1,
                shape: Shape::CountWithin {
                    predicate: Predicate::AnyCatalogTool {},
                    op: CmpOp::Ge,
                    threshold: 1,
                },
            }],
        };
        assert_eq!(history_rejections(&out, false).len(), 1);
        assert!(history_rejections(&out, true).is_empty());
    }

    fn condition(slot: &str, scope: Scope, window_ms: u64, shape: Shape) -> HistoryCondition {
        HistoryCondition {
            slot: slot.into(),
            policy_id: "p".into(),
            scope,
            window_ms,
            shape,
        }
    }

    #[test]
    fn restamp_changes_only_the_version() {
        let org = Uuid::from_u128(7);
        let out = CompiledOutput {
            cedar_policies: "@id(\"p\")\npermit(principal, action, resource);\n".to_owned(),
            cedar_schema_json: json!({"Pgs::Gateway": {}}),
            conditions: vec![HistoryCondition {
                slot: "c1".into(),
                policy_id: "p".into(),
                scope: Scope::Principal,
                window_ms: 1000,
                shape: Shape::Formerly {
                    predicate: Predicate::EventKind { kind: EventKind::Request },
                },
            }],
        };
        let meta = CompiledMeta {
            key: ToolPolicySetKey::Tenant(org),
            version: 2,
            compiler_generation: 3,
            schema_revision: 4,
            catalog_revision: Some(5),
            max_window_ms: 6,
            catalog_names: vec!["a".into(), "b".into()],
        };
        let bytes = encode_compiled(&meta, &out);
        let restamped = restamp_version(&bytes, 9).unwrap();
        let before = pb::CompiledToolPolicySet::decode(bytes.as_slice()).unwrap();
        let after = pb::CompiledToolPolicySet::decode(restamped.as_slice()).unwrap();
        assert_eq!((before.version, after.version), (2, 9));
        assert_eq!(pb::CompiledToolPolicySet { version: 9, ..before }, after);
        assert!(matches!(restamp_version(&[0xff, 0xff], 1), Err(TenancyError::Internal)));
    }

    #[test]
    fn decode_compiled_inverts_encode_compiled() {
        let field = |value: Literal, op: CmpOp| Predicate::Field {
            path: vec!["a".into(), "b".into()],
            op,
            value,
        };
        let tree = Predicate::AllOf {
            left: Box::new(Predicate::EventKind { kind: EventKind::Request }),
            right: Box::new(Predicate::AnyOf {
                left: Box::new(Predicate::Tool { name_key: "gh.search".into() }),
                right: Box::new(Predicate::Negation {
                    inner: Box::new(field(Literal::Decimal("1.50".into()), CmpOp::Ne)),
                }),
            }),
        };
        let out = CompiledOutput {
            cedar_policies: "@id(\"p\")\nforbid(principal, action, resource) when { context.s0 };".into(),
            cedar_schema_json: json!({"Pgs::Gateway": {"entityTypes": {}, "actions": {"a": {"memberOf": [], "appliesTo": null}}}}),
            conditions: vec![
                condition("s0", Scope::Session, 1, Shape::Formerly { predicate: tree }),
                condition(
                    "s1",
                    Scope::Principal,
                    60_000,
                    Shape::CountWithin {
                        predicate: Predicate::AnyCatalogTool {},
                        op: CmpOp::Le,
                        threshold: 3,
                    },
                ),
                condition(
                    "s2",
                    Scope::Key,
                    3_600_000,
                    Shape::SumWithin {
                        predicate: field(Literal::Long(-5), CmpOp::Lt),
                        field_path: vec!["amount".into()],
                        op: CmpOp::Gt,
                        threshold: 500,
                    },
                ),
                condition(
                    "s3",
                    Scope::Session,
                    2,
                    Shape::Formerly {
                        predicate: field(Literal::String("x".into()), CmpOp::Eq),
                    },
                ),
                condition(
                    "s4",
                    Scope::Session,
                    3,
                    Shape::Formerly {
                        predicate: Predicate::AllOf {
                            left: Box::new(field(Literal::Bool(true), CmpOp::Ge)),
                            right: Box::new(Predicate::EventKind { kind: EventKind::Response }),
                        },
                    },
                ),
            ],
        };
        let meta = CompiledMeta {
            key: ToolPolicySetKey::Tenant(Uuid::from_u128(7)),
            version: 3,
            compiler_generation: 1,
            schema_revision: 1,
            catalog_revision: Some(9),
            max_window_ms: 3_600_000,
            catalog_names: vec!["gh.search".into()],
        };
        let bytes = encode_compiled(&meta, &out);
        let decoded = decode_compiled(&bytes).expect("the bytes decode");
        assert_eq!(decoded, out);
        assert!(outputs_identical(&decoded, &out));
        let set = pb::CompiledToolPolicySet::decode(bytes.as_slice()).unwrap();
        assert_eq!(set.kind, pb::ToolPolicySetKind::Tenant as i32);
        assert_eq!(set.org_id, Uuid::from_u128(7).to_string());
        assert_eq!((set.version, set.catalog_revision, set.max_window_ms), (3, 9, 3_600_000));
        assert_eq!(set.catalog_names, vec!["gh.search".to_string()]);

        let floor = encode_compiled(
            &CompiledMeta {
                key: ToolPolicySetKey::Floor,
                catalog_revision: None,
                catalog_names: Vec::new(),
                ..meta
            },
            &out,
        );
        let set = pb::CompiledToolPolicySet::decode(floor.as_slice()).unwrap();
        assert_eq!((set.kind, set.org_id.as_str(), set.catalog_revision), (pb::ToolPolicySetKind::Floor as i32, "", 0));
    }

    /// A compiled set with one condition whose predicate is a left-deep `all_of` chain of
    /// `depth` nodes.
    fn deep_set(depth: usize) -> Vec<u8> {
        let mut predicate = Predicate::EventKind { kind: EventKind::Request };
        for _ in 1..depth {
            predicate = Predicate::AllOf {
                left: Box::new(predicate),
                right: Box::new(Predicate::Tool { name_key: "t".into() }),
            };
        }
        let out = CompiledOutput {
            cedar_policies: String::new(),
            cedar_schema_json: json!({}),
            conditions: vec![condition("s0", Scope::Session, 1, Shape::Formerly { predicate })],
        };
        let meta = CompiledMeta {
            key: ToolPolicySetKey::Floor,
            version: 1,
            compiler_generation: 1,
            schema_revision: 1,
            catalog_revision: None,
            max_window_ms: 1,
            catalog_names: Vec::new(),
        };
        encode_compiled(&meta, &out)
    }

    /// The reason for `MAX_PREDICATE_DEPTH`: prost refuses a message nested more than 100
    /// levels, and each `all_of` adds two. A set at the limit decodes again, with room for a
    /// wrapper message. A set of 50 nodes (49 `all_of`) encodes, but never decodes again.
    #[test]
    fn a_set_at_the_predicate_depth_limit_decodes_and_a_deeper_one_does_not() {
        use paigasus_iam_core::tool_policy::MAX_PREDICATE_DEPTH;
        assert!(decode_compiled(&deep_set(MAX_PREDICATE_DEPTH)).is_ok());
        assert!(decode_compiled(&deep_set(49)).is_ok());
        assert_eq!(decode_compiled(&deep_set(50)), Err(TenancyError::Internal));
    }

    #[test]
    fn bytes_that_are_not_a_compiled_set_are_internal() {
        assert_eq!(decode_compiled(&[0xff, 0xff, 0xff]), Err(TenancyError::Internal));
        let bad_scope = pb::CompiledToolPolicySet {
            cedar_schema: "{}".into(),
            conditions: vec![pb::HistoryCondition { scope: 99, ..Default::default() }],
            ..Default::default()
        };
        assert_eq!(decode_compiled(&bad_scope.encode_to_vec()), Err(TenancyError::Internal));
    }

    #[test]
    fn the_drop_reason_is_added_only_for_a_whole_word_match() {
        let tool = build_manual(
            "search",
            ToolKind::Function,
            Some(json!({"type": "object", "properties": {"q": {"type": "string"}, "price": {"type": "number"}}})),
            &CatalogLimits::default(),
        )
        .unwrap();
        let mut hit = ToolPolicyRejection::new(RejectionReason::PolicyInvalid, "attribute `price` not found");
        add_dropped_field_reasons(&mut hit, std::slice::from_ref(&tool));
        assert!(hit.message.contains("the field `price` of the tool `search` is not in the schema"), "{}", hit.message);
        let mut miss = ToolPolicyRejection::new(RejectionReason::PolicyInvalid, "attribute `unit_price` not found");
        add_dropped_field_reasons(&mut miss, &[tool]);
        assert_eq!(miss.message, "attribute `unit_price` not found");
    }

    fn order_tool(properties: serde_json::Value) -> ToolEntry {
        build_manual("orders", ToolKind::Function, Some(json!({"type": "object", "properties": properties})), &CatalogLimits::default()).unwrap()
    }

    #[test]
    fn a_nested_drop_is_named_when_the_message_names_only_the_leaf() {
        let tool = order_tool(json!({"order": {"type": "object", "properties": {"id": {"type": "string"}, "total": {"type": "number"}}}}));
        assert_eq!(tool.dropped_fields.iter().map(|d| d.path.as_str()).collect::<Vec<_>>(), vec!["order.total"]);
        let mut leaf_only = ToolPolicyRejection::new(RejectionReason::PolicyInvalid, "attribute `total` on the record is not found");
        add_dropped_field_reasons(&mut leaf_only, std::slice::from_ref(&tool));
        assert!(leaf_only.message.contains("the field `order.total` of the tool `orders` is not in the schema"), "{}", leaf_only.message);
        // The full path also matches, and adds one note, not two.
        let mut full = ToolPolicyRejection::new(RejectionReason::PolicyInvalid, "context.input.order.total is not found");
        add_dropped_field_reasons(&mut full, &[tool]);
        assert_eq!(full.message.matches("is not in the schema").count(), 1, "{}", full.message);
    }

    #[test]
    fn a_leaf_that_is_also_a_mapped_field_adds_no_note() {
        // `total` is a mapped top-level field AND the leaf of the dropped `order.total`. A message
        // that names `total` may be about the mapped field, so no note is added (see the doc).
        let tool = order_tool(json!({
            "total": {"type": "integer"},
            "order": {"type": "object", "properties": {"total": {"type": "number"}}}
        }));
        assert_eq!(tool.dropped_fields.iter().map(|d| d.path.as_str()).collect::<Vec<_>>(), vec!["order.total"]);
        let mut ambiguous = ToolPolicyRejection::new(RejectionReason::PolicyInvalid, "attribute `total` has the wrong type");
        add_dropped_field_reasons(&mut ambiguous, std::slice::from_ref(&tool));
        assert_eq!(ambiguous.message, "attribute `total` has the wrong type");
        // The full path still names the drop.
        let mut full = ToolPolicyRejection::new(RejectionReason::PolicyInvalid, "order.total is not found");
        add_dropped_field_reasons(&mut full, &[tool]);
        assert!(full.message.contains("the field `order.total`"), "{}", full.message);
    }
}
