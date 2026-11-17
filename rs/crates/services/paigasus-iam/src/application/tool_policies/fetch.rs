// SPDX-License-Identifier: Apache-2.0

//! The compiled-policy fetch for the gateway (SMA-775, spec section 9.3). Both reads return
//! stored versions as they are. They hold no lock and write no audit entry.

use paigasus_iam_core::authz::model::root_prn;
use paigasus_iam_core::tool_policy::{ToolPolicySetKey, ToolPolicySettings, ToolPolicyVersion};
use paigasus_iam_core::{Action, Clock, IdGenerator};
use paigasus_kernel::Prn;
use uuid::Uuid;

use super::ToolPolicyService;
use crate::application::error::TenancyError;

/// What the gateway reads for one tenant.
#[derive(Debug, Clone, PartialEq)]
pub struct CompiledTenant {
    /// The requested version of the tenant set. `None` when the tenant has no live version.
    pub set: Option<ToolPolicyVersion>,
    /// The settings of the tenant. The default of an absent row is `off` and `closed`.
    pub settings: ToolPolicySettings,
    /// The live version of the platform floor. 0 when the floor has no live version.
    pub floor_version: u64,
}

impl<I: IdGenerator, C: Clock> ToolPolicyService<I, C> {
    /// The compiled tenant set (`version = None` is the live version), the settings and the
    /// live floor version. Authorized as `ReadCompiledToolPolicies` at the organization.
    /// `tool-policy-version-not-found` for a version that is not stored or not readable.
    ///
    /// The three reads are not one snapshot. The settings are read first, then the set, then the
    /// floor version. A first enable writes the starter set and the new mode in one transaction.
    /// With this order, the only tear that a concurrent first enable can cause is the starter set
    /// with mode `off`. That tear is harmless. The reverse order could return no set with mode
    /// `log_only` or `enforce`. The gateway version token names exactly what this call returned,
    /// and the outbox event of the concurrent commit causes a fresh load.
    pub async fn get_compiled(&self, actor: &Prn, org: Uuid, version: Option<u64>) -> Result<CompiledTenant, TenancyError> {
        self.authorize_org(actor, org, Action::ReadCompiledToolPolicies).await?;
        let settings = self.repo.settings(org).await?.unwrap_or_else(ToolPolicySettings::default_for_absent_row);
        let set = self.read_version(&ToolPolicySetKey::Tenant(org), version).await?;
        let floor_version = self.repo.state(&ToolPolicySetKey::Floor).await?.live_version;
        Ok(CompiledTenant { set, settings, floor_version })
    }

    /// The compiled floor (`version = None` is the live version). Authorized as
    /// `ReadCompiledToolPolicies` at `root_prn()`, never at a PRN from the request.
    pub async fn get_compiled_floor(&self, actor: &Prn, version: Option<u64>) -> Result<Option<ToolPolicyVersion>, TenancyError> {
        self.authorize.check(actor, Action::ReadCompiledToolPolicies, &root_prn()).await?;
        self.read_version(&ToolPolicySetKey::Floor, version).await
    }
}
