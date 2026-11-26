// SPDX-License-Identifier: Apache-2.0

//! Tool catalog building blocks: tool names, the JSON Schema to Cedar mapping and the
//! import parsers (SMA-774, spec sections 4.1 to 4.3). Also the compiler protocol types, the
//! output comparator, the cedar 4.14 checks and the `PolicyCompiler` port (spec sections 5.2,
//! 5.3 and 8.1). Pure code, no I/O.

pub mod check;
pub mod compare;
pub mod import;
pub mod json_schema;
pub mod model;
pub mod name;
pub mod ports;
pub mod proto;
pub mod protocol;
pub mod report;
pub mod schema;
pub mod starter;

pub use check::{MAX_PREDICATE_DEPTH, REASON_FLOOR_PERMIT, REASON_INVALID, check_compiled};
pub use compare::{outputs_equivalent_for_recompile, outputs_identical};
pub use import::{CatalogProblem, ImportFormat, ImportedTools, build_manual, parse_import};
pub use json_schema::{MappingError, map_input_schema};
pub use model::{
    CatalogLimits, DroppedField, FailMode, HeldStatus, HeldVersion, StoredTool, ToolEntry, ToolKind, ToolPolicyMode, ToolPolicySetKey, ToolPolicySettings, ToolPolicyState, ToolPolicyVersion,
    ToolSource, VersionOrigin, VersionStatus,
};
pub use name::{NameError, RESERVED_NAME_KEYS, ToolName, name_key, parse_qualified, parse_tool_name};
pub use ports::{CompileOutcome, CompilerFailure, PolicyCompiler, RecompileLock, RecompileLockGuard, ToolPolicyRepository};
pub use proto::{ProtoError, condition_from_pb, condition_to_pb};
pub use protocol::{CompileRequest, CompileResult, CompiledOutput, PROTOCOL_VERSION, Rejection, RevisionOutput, SetKind};
pub use report::{HeldReport, HeldResult, NameDiff, ReportRejection, diff_outputs, failed_report};
pub use schema::{ACTION_NAMESPACE, TOOL_SCHEMA_REVISION, base_schema_json, catalog_names, tenant_schema_json};
pub use starter::{STARTER_TOOL_POLICY_ID, STARTER_TOOL_POLICY_SOURCE};
