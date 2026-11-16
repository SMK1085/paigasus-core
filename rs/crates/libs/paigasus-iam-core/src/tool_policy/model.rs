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

/// The key of one tool policy set: one set for each organization, and one platform floor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolPolicySetKey {
    Tenant(uuid::Uuid),
    Floor,
}

/// The status of a stored version. This release writes only these two. The database also
/// allows `held` and `discarded`, for later releases.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionStatus {
    Live,
    Superseded,
}

/// What made a stored version. This release writes only these three. The database also allows
/// `recompile` and `accept`, for later releases.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionOrigin {
    Save,
    /// The starter set that the first enable of a tenant stores.
    Starter,
    CatalogChange,
}

/// The revision counters of one set. Both are 0 when the set has no row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolPolicyState {
    pub catalog_revision: u64,
    pub live_version: u64,
}

/// One stored version of a tool policy set.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolPolicyVersion {
    pub key: ToolPolicySetKey,
    pub version: u64,
    pub source: String,
    /// The `CompiledToolPolicySet` protobuf bytes. The adapter does not decode them.
    pub compiled: Vec<u8>,
    pub compiler_generation: u32,
    pub schema_revision: u32,
    /// `None` for the floor.
    pub catalog_revision: Option<u64>,
    pub status: VersionStatus,
    pub origin: VersionOrigin,
    pub created_by: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// A catalog entry with its stored times.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredTool {
    pub entry: ToolEntry,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// How the gateway applies tool policies for one tenant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolPolicyMode {
    Off,
    LogOnly,
    Enforce,
}

impl ToolPolicyMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::LogOnly => "log_only",
            Self::Enforce => "enforce",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "off" => Some(Self::Off),
            "log_only" => Some(Self::LogOnly),
            "enforce" => Some(Self::Enforce),
            _ => None,
        }
    }
}

/// What the gateway does when it cannot read the policies of a tenant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailMode {
    Open,
    Closed,
}

impl FailMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Closed => "closed",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "open" => Some(Self::Open),
            "closed" => Some(Self::Closed),
            _ => None,
        }
    }
}

/// The tool policy settings of one tenant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolPolicySettings {
    pub mode: ToolPolicyMode,
    pub fail_mode: FailMode,
    pub updated_at: Option<chrono::DateTime<chrono::Utc>>,
    pub updated_by: Option<String>,
}

impl ToolPolicySettings {
    /// The value for a tenant that has no settings row: policies off, fail closed.
    pub fn default_for_absent_row() -> Self {
        Self {
            mode: ToolPolicyMode::Off,
            fail_mode: FailMode::Closed,
            updated_at: None,
            updated_by: None,
        }
    }
}

#[cfg(test)]
mod settings_tests {
    use super::*;

    #[test]
    fn mode_and_fail_mode_round_trip_their_wire_strings() {
        for m in [ToolPolicyMode::Off, ToolPolicyMode::LogOnly, ToolPolicyMode::Enforce] {
            assert_eq!(ToolPolicyMode::parse(m.as_str()), Some(m));
        }
        for f in [FailMode::Open, FailMode::Closed] {
            assert_eq!(FailMode::parse(f.as_str()), Some(f));
        }
        for (m, s) in [(ToolPolicyMode::Off, "off"), (ToolPolicyMode::LogOnly, "log_only"), (ToolPolicyMode::Enforce, "enforce")] {
            assert_eq!(m.as_str(), s);
            assert_eq!(ToolPolicyMode::parse(s), Some(m));
        }
        for (f, s) in [(FailMode::Open, "open"), (FailMode::Closed, "closed")] {
            assert_eq!(f.as_str(), s);
            assert_eq!(FailMode::parse(s), Some(f));
        }
        assert_eq!(ToolPolicyMode::parse("Enforce"), None);
        assert_eq!(FailMode::parse(""), None);
    }

    #[test]
    fn an_absent_row_reads_as_off_and_closed() {
        let s = ToolPolicySettings::default_for_absent_row();
        assert_eq!((s.mode, s.fail_mode), (ToolPolicyMode::Off, FailMode::Closed));
        assert!(s.updated_at.is_none() && s.updated_by.is_none());
    }
}
