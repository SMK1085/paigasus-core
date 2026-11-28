// SPDX-License-Identifier: Apache-2.0

//! SMA-752: the port that supplies tool-policy text to the gateway. The file source is the first
//! adapter (`adapters::policy_source`). The IAM source (SMA-751) returns the same three things for
//! a tenant and for the floor: policy text, catalog names and a version. It also returns the
//! tenant settings (mode and fail mode, spec §9.2, §9.3). The file source has no settings.
//!
//! SMA-753 spec P6: both sets also carry their history conditions in the proto form, and the
//! window limit that the set was compiled with. The cache maps and validates them at load. The
//! file source has no conditions.

use std::sync::Arc;

use async_trait::async_trait;
use paigasus_gateway_core::{OnUnavailable, TenantMode};
use paigasus_proto::paigasus::iam::v1 as pb;
use uuid::Uuid;

/// The floor policy text and its version.
#[derive(Debug, Clone, PartialEq)]
pub struct PolicyText {
    /// The Cedar policy text. An empty string is an empty floor.
    pub text: String,
    /// A token that changes when the content changes. The conditions change only with it.
    pub version: String,
    /// The history conditions of the floor, sorted by slot. Empty from the file source.
    pub conditions: Vec<pb::HistoryCondition>,
    /// The window limit that the floor was compiled with.
    pub max_window_ms: u64,
}

/// The tenant settings from the source (spec §9.2, §9.3). A source with no settings (the file
/// source) returns `None` in [`TenantPolicies::settings`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TenantSettings {
    /// The tenant mode.
    pub mode: TenantMode,
    /// What the gateway does when the tenant set is unavailable.
    pub on_unavailable: OnUnavailable,
    /// The webhook providers that the tenant opted in to, sorted and unique. Empty from a source
    /// with no opt-in list.
    pub webhooks: Arc<[String]>,
}

/// The policy text, the tool catalog and the version of one tenant.
#[derive(Debug, Clone, PartialEq)]
pub struct TenantPolicies {
    /// The Cedar policy text of the tenant. An empty string is an empty set.
    pub text: String,
    /// The tool names that the tenant knows.
    pub catalog: Vec<String>,
    /// A token that changes when the text, the catalog or the settings change.
    pub version: String,
    /// The tenant settings. `None` from a source with no settings.
    pub settings: Option<TenantSettings>,
    /// The history conditions of the tenant set, sorted by slot. Empty from the file source. They
    /// change only with the set version, so `version` covers them.
    pub conditions: Vec<pb::HistoryCondition>,
    /// The window limit that the tenant set was compiled with.
    pub max_window_ms: u64,
}

/// Why a source could not supply a policy set. The messages hold no policy text and no tool name.
#[derive(Debug, thiserror::Error)]
pub enum SourceError {
    /// The source has no such item (for example, the floor file is missing).
    #[error("policy source item not found: {0}")]
    NotFound(String),
    /// The source could not be read.
    #[error("policy source I/O error: {0}")]
    Io(String),
    /// The source was read, but its content has a wrong shape.
    #[error("policy source content is malformed: {0}")]
    Malformed(String),
}

/// Supplies the floor and the tenant policy sets.
#[async_trait]
pub trait PolicySource: Send + Sync {
    /// The floor. An unavailable floor is an error, not an empty floor.
    async fn floor(&self) -> Result<PolicyText, SourceError>;
    /// The set and the catalog of one organization. A tenant with no data has an empty set.
    async fn tenant(&self, org: Uuid) -> Result<TenantPolicies, SourceError>;
}
