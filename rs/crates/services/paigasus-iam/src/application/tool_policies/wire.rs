// SPDX-License-Identifier: Apache-2.0

//! The `Tool` wire message of a stored tool, and the encoded size of a whole catalog as one
//! `ListToolsResponse` (spec section 4.2, the catalog size cap).
//!
//! The gRPC adapter converts each tool with [`tool_to_pb`], and the catalog write measures the
//! resulting catalog with [`catalog_response_len`]. Both use this one conversion, so the cap
//! measures the bytes that `ListTools` sends. This crate already keeps the stored compiled set
//! as `paigasus-proto` bytes (`sets.rs`), so the application layer already depends on the wire
//! types. A size port would only hide the same conversion behind a trait.

use chrono::{DateTime, Utc};
use paigasus_iam_core::tool_policy::{DroppedField, StoredTool, ToolKind, ToolSource};
use paigasus_proto::paigasus::iam::v1 as pb;
use prost::Message;

/// A timestamp as the protobuf well-known type.
fn timestamp(dt: DateTime<Utc>) -> prost_types::Timestamp {
    prost_types::Timestamp {
        seconds: dt.timestamp(),
        nanos: i32::try_from(dt.timestamp_subsec_nanos()).unwrap_or(i32::MAX),
    }
}

fn dropped_to_pb(d: &DroppedField) -> pb::DroppedField {
    pb::DroppedField {
        path: d.path.clone(),
        reason: d.reason.clone(),
    }
}

/// The `Tool` wire message of a stored tool.
pub fn tool_to_pb(tool: &StoredTool) -> pb::Tool {
    let entry = &tool.entry;
    pb::Tool {
        name_key: entry.name.name_key.clone(),
        display_name: entry.name.display.clone(),
        kind: match entry.kind {
            ToolKind::Function => pb::ToolKind::Function,
            ToolKind::Custom => pb::ToolKind::Custom,
        } as i32,
        input_schema_json: entry.input_schema.as_ref().map(ToString::to_string).unwrap_or_default(),
        cedar_input_type_json: entry.cedar_input_type.to_string(),
        dropped_fields: entry.dropped_fields.iter().map(dropped_to_pb).collect(),
        source: match entry.source {
            ToolSource::Manual => pb::ToolSource::Manual,
            ToolSource::OpenAi => pb::ToolSource::Openai,
            ToolSource::Mcp => pb::ToolSource::Mcp,
        } as i32,
        created_at: Some(timestamp(tool.created_at)),
        updated_at: Some(timestamp(tool.updated_at)),
    }
}

/// The encoded size in bytes of `tools` and `catalog_revision` as one `ListToolsResponse`: the
/// largest response that `ListTools` can send for this catalog. A page holds a part of it.
pub fn catalog_response_len(tools: &[StoredTool], catalog_revision: u64) -> usize {
    pb::ListToolsResponse {
        tools: tools.iter().map(tool_to_pb).collect(),
        catalog_revision,
    }
    .encoded_len()
}
