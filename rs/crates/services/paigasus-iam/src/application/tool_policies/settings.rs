// SPDX-License-Identifier: Apache-2.0

//! The mode-transition rule of the tool policy settings (SMA-775, spec section 9.2).

use paigasus_iam_core::tool_policy::ToolPolicyMode;

use crate::application::error::TenancyError;

/// Decides a settings write. `has_set` is true when the organization has a live tool policy set
/// (`live_version > 0`). Returns `Ok(true)` when the write must save the starter set, `Ok(false)`
/// when it saves none, and `ToolPolicyModeTransitionInvalid` for a first enable to `enforce`.
///
/// The stored mode does not change the result. An organization with no set is treated as
/// `Off`, and a row in `log_only` or `enforce` always has a set. So the function takes no
/// current mode.
pub(crate) fn check_transition(next: ToolPolicyMode, has_set: bool) -> Result<bool, TenancyError> {
    match (next, has_set) {
        (ToolPolicyMode::Off, _) => Ok(false),
        (_, true) => Ok(false),
        (ToolPolicyMode::LogOnly, false) => Ok(true),
        (ToolPolicyMode::Enforce, false) => Err(TenancyError::ToolPolicyModeTransitionInvalid),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_transition_table_holds() {
        use ToolPolicyMode::{Enforce, LogOnly, Off};
        for has_set in [false, true] {
            assert_eq!(check_transition(Off, has_set), Ok(false), "off, has_set {has_set}");
        }
        assert_eq!(check_transition(LogOnly, false), Ok(true));
        assert_eq!(check_transition(LogOnly, true), Ok(false));
        assert_eq!(check_transition(Enforce, false), Err(TenancyError::ToolPolicyModeTransitionInvalid));
        assert_eq!(check_transition(Enforce, true), Ok(false));
    }
}
