// SPDX-License-Identifier: Apache-2.0

//! Data types of one tool catalog entry (spec section 4.2).

use super::name::ToolName;

/// The kind of a tool. A `Function` takes a JSON object. A `Custom` tool takes free text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolKind {
    Function,
    Custom,
}

/// Where a catalog entry came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolSource {
    Manual,
    OpenAi,
    Mcp,
}

/// An argument path that did not map to Cedar, with the reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DroppedField {
    /// A dotted path, for example `a.b`.
    pub path: String,
    pub reason: String,
}

/// One tool catalog entry.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolEntry {
    pub name: ToolName,
    pub kind: ToolKind,
    /// The original JSON Schema as given. `None` for a `Custom` tool or a tool with no schema.
    pub input_schema: Option<serde_json::Value>,
    /// The mapped Cedar record type, as JSON.
    pub cedar_input_type: serde_json::Value,
    pub dropped_fields: Vec<DroppedField>,
    pub source: ToolSource,
}

/// The limits of the tool catalog (spec section 4.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CatalogLimits {
    pub max_tools: usize,
    pub max_input_schema_bytes: usize,
    pub max_depth: usize,
}

impl Default for CatalogLimits {
    fn default() -> Self {
        Self {
            max_tools: 500,
            max_input_schema_bytes: 65536,
            max_depth: 8,
        }
    }
}
