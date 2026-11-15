// SPDX-License-Identifier: Apache-2.0

//! Tool catalog building blocks: tool names, the JSON Schema to Cedar mapping and the
//! import parsers (SMA-774, spec sections 4.1 to 4.3). Pure code, no I/O.

pub mod import;
pub mod json_schema;
pub mod model;
pub mod name;
pub mod schema;
pub mod starter;

pub use import::{CatalogProblem, ImportFormat, ImportedTools, build_manual, parse_import};
pub use json_schema::{MappingError, map_input_schema};
pub use model::{CatalogLimits, DroppedField, ToolEntry, ToolKind, ToolSource};
pub use name::{NameError, RESERVED_NAME_KEYS, ToolName, name_key, parse_qualified, parse_tool_name};
pub use schema::{ACTION_NAMESPACE, TOOL_SCHEMA_REVISION, base_schema_json, catalog_names, tenant_schema_json};
pub use starter::{STARTER_TOOL_POLICY_ID, STARTER_TOOL_POLICY_SOURCE};
