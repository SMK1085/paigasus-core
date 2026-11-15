// SPDX-License-Identifier: Apache-2.0
//! The JSON protocol between IAM and the compiler (spec 2026-11-15 iam-tool-policy-sets,
//! section 5.2). IAM writes one `CompileRequest` to stdin; the compiler writes one
//! `CompileResult` to stdout. The shared fixture `protocol/fixtures/` holds examples of both,
//! and the tests on both sides read it.

use cedar_policy::SchemaFragment;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::compile::{Compiled, ReasonCode, Rejection, SetKind, compile_cedarschema};
use crate::{COMPILER_GENERATION, PROTOCOL_VERSION, revision_string};

/// One compile request.
#[derive(Debug, Clone, PartialEq, Deserialize)]
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

/// One compile result. A rejection is a normal result (exit 0).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum CompileResult {
    Accepted {
        protocol_version: u32,
        compiler_generation: u32,
        compiled: Compiled,
    },
    Rejected {
        protocol_version: u32,
        compiler_generation: u32,
        rejections: Vec<Rejection>,
    },
}

/// The output of `paigasus-policy-compiler revision` (spec section 6.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RevisionOutput {
    pub compiler_generation: u32,
    pub protocol_version: u32,
    pub revision: String,
}

impl RevisionOutput {
    /// The revision of this build.
    pub fn current() -> Self {
        RevisionOutput {
            compiler_generation: COMPILER_GENERATION,
            protocol_version: PROTOCOL_VERSION,
            revision: revision_string(),
        }
    }
}

/// A request that the compiler cannot answer with a result. The binary exits 2.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolError {
    /// The request is not JSON, or it does not have the request fields.
    Malformed(String),
    /// The request has a `protocol_version` other than `PROTOCOL_VERSION`.
    UnsupportedVersion(u32),
}

impl std::fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProtocolError::Malformed(detail) => write!(f, "the request is malformed: {detail}"),
            ProtocolError::UnsupportedVersion(version) => {
                write!(f, "the request has protocol_version {version}; this compiler supports only {PROTOCOL_VERSION}")
            }
        }
    }
}

impl std::error::Error for ProtocolError {}

/// Parse a request. The version is checked first, so a request of a later version that adds
/// fields gets `UnsupportedVersion`, not `Malformed`.
pub fn parse_request(text: &str) -> Result<CompileRequest, ProtocolError> {
    let value: Value = serde_json::from_str(text).map_err(|error| ProtocolError::Malformed(error.to_string()))?;
    let version = value
        .get("protocol_version")
        .ok_or_else(|| ProtocolError::Malformed("the request has no protocol_version".to_string()))?;
    let version = version
        .as_u64()
        .and_then(|v| u32::try_from(v).ok())
        .ok_or_else(|| ProtocolError::Malformed(format!("the protocol_version {version} is not a u32")))?;
    if version != PROTOCOL_VERSION {
        return Err(ProtocolError::UnsupportedVersion(version));
    }
    serde_json::from_value(value).map_err(|error| ProtocolError::Malformed(error.to_string()))
}

/// Convert a Cedar JSON schema to the cedarschema text that Dogwood's `PolicySchema` reads
/// (plan decision D6), with the cedar 4.11 API.
pub fn cedarschema_from_json(schema_json: &Value) -> Result<String, String> {
    let fragment = SchemaFragment::from_json_value(schema_json.clone()).map_err(|error| error.to_string())?;
    fragment.to_cedarschema().map_err(|error| error.to_string())
}

/// Compile one request. A schema that does not convert is a `tool-policy-invalid` rejection,
/// in the same way as a cedarschema that does not parse in `compile_cedarschema`.
pub fn compile_request(request: &CompileRequest) -> Result<CompileResult, ProtocolError> {
    if request.protocol_version != PROTOCOL_VERSION {
        return Err(ProtocolError::UnsupportedVersion(request.protocol_version));
    }
    let outcome = cedarschema_from_json(&request.schema_json)
        .map_err(|error| vec![Rejection::new(None, ReasonCode::ToolPolicyInvalid, format!("the action schema is invalid: {error}"))])
        .and_then(|cedarschema| compile_cedarschema(request.kind, &request.source, &cedarschema, request.max_window_ms));
    Ok(match outcome {
        Ok(compiled) => CompileResult::Accepted {
            protocol_version: PROTOCOL_VERSION,
            compiler_generation: COMPILER_GENERATION,
            compiled,
        },
        Err(rejections) => CompileResult::Rejected {
            protocol_version: PROTOCOL_VERSION,
            compiler_generation: COMPILER_GENERATION,
            rejections,
        },
    })
}
