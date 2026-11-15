// SPDX-License-Identifier: Apache-2.0
//! The policy snapshot of one request (SMA-752 D30): the compiled policies, the tenant mode and
//! the effective `on_policy_unavailable`. The gateway builds it once per request; the decider
//! reads it from `DecisionContext` and never loads policies itself.

use std::sync::Arc;

use paigasus_kernel::tool_policy::{Facts, ToolPolicies};

/// The tenant mode (§ 6.4). It applies only to a deny that tenant policies decided: a floor deny
/// is always enforced (D33).
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TenantMode {
    /// The tenant set is not evaluated. Only the floor applies.
    Off,
    /// A tenant deny is reported as `Observe`, and the call goes through.
    LogOnly,
    /// A tenant deny blocks the call.
    Enforce,
}

impl TenantMode {
    /// The config and metric label spelling.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            TenantMode::Off => "off",
            TenantMode::LogOnly => "log_only",
            TenantMode::Enforce => "enforce",
        }
    }
}

/// The effective `on_policy_unavailable` of the tenant (D34: the per-org override, else the
/// global value).
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnUnavailable {
    /// Allow as `Observe { would: PolicyUnavailable }` (P-5).
    Open,
    /// Block with `PolicyUnavailable`.
    Closed,
}

/// What the tenant part of an available set is.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TenantPart {
    /// The policies hold the floor and the tenant set.
    Evaluated,
    /// The tenant mode is `off`: the policies are the floor alone (P-4).
    Off,
    /// The tenant set is unavailable: the policies are the floor alone (P-4), and the tenant part
    /// follows `on_policy_unavailable` (D34).
    Unavailable,
}

/// The policies of a request.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub enum PolicySet {
    /// The floor is available. With a `tenant` other than `Evaluated`, `policies` must be the
    /// floor alone, `ToolPolicies::compile(floor, "", &[])` (P-4).
    Available { policies: Arc<ToolPolicies>, tenant: TenantPart, facts: Facts },
    /// The floor is unavailable, or a PRN of the request does not parse (P-6).
    Unavailable,
}

/// One request's policy snapshot (D30).
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct PolicySnapshot {
    pub set: PolicySet,
    pub mode: TenantMode,
    pub on_unavailable: OnUnavailable,
}

impl PolicySnapshot {
    #[must_use]
    pub fn new(set: PolicySet, mode: TenantMode, on_unavailable: OnUnavailable) -> Self {
        PolicySnapshot { set, mode, on_unavailable }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tenant_mode_spellings() {
        assert_eq!(TenantMode::Off.as_str(), "off");
        assert_eq!(TenantMode::LogOnly.as_str(), "log_only");
        assert_eq!(TenantMode::Enforce.as_str(), "enforce");
    }
}
