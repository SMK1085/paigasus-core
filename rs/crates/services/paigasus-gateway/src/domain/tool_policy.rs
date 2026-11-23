// SPDX-License-Identifier: Apache-2.0

//! SMA-752: the port that supplies tool-policy text to the gateway. The file source is the first
//! adapter (`adapters::policy_source`). The IAM source (SMA-751) returns the same three things for
//! a tenant and for the floor: policy text, catalog names and a version. It also returns the
//! tenant settings (mode and fail mode, spec §9.2, §9.3). The file source has no settings.

use async_trait::async_trait;
use paigasus_gateway_core::{OnUnavailable, TenantMode};
use uuid::Uuid;

/// The floor policy text and its version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyText {
    /// The Cedar policy text. An empty string is an empty floor.
    pub text: String,
    /// A token that changes when the content changes.
    pub version: String,
}

/// The tenant settings from the source (spec §9.2, §9.3). A source with no settings (the file
/// source) returns `None` in [`TenantPolicies::settings`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TenantSettings {
    /// The tenant mode.
    pub mode: TenantMode,
    /// What the gateway does when the tenant set is unavailable.
    pub on_unavailable: OnUnavailable,
}

/// The policy text, the tool catalog and the version of one tenant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TenantPolicies {
    /// The Cedar policy text of the tenant. An empty string is an empty set.
    pub text: String,
    /// The tool names that the tenant knows.
    pub catalog: Vec<String>,
    /// A token that changes when the text, the catalog or the settings change.
    pub version: String,
    /// The tenant settings. `None` from a source with no settings.
    pub settings: Option<TenantSettings>,
    /// The source refused the set. The cache then keeps the settings of the answer, but it never
    /// compiles the set. So the tenant set is unavailable, and the tenant fail mode decides.
    ///
    /// No source sets this field now. The SMA-775 R5 refusal refused a set with a history
    /// condition. SMA-753 D16 replaced that rule: such a set loads, and its slots stay empty. The
    /// port keeps the field for a later refusal rule.
    pub refused: bool,
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
