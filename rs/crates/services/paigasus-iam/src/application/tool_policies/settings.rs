// SPDX-License-Identifier: Apache-2.0

//! The mode-transition rule of the tool policy settings (SMA-775, spec section 9.2).

use paigasus_iam_core::tool_policy::ToolPolicyMode;

use crate::application::error::TenancyError;

/// Decides a settings write. Spec section 9.2: "The first change away from `off` must be to
/// `log_only`. [...] A later change from `off` to `enforce` is allowed when a tenant set exists."
/// This is the strict reading (Sven, final review I-2): `enforce` needs both a live set
/// (`has_set`, `live_version > 0`) and an earlier change away from `off` (`ever_enabled`: the
/// stored `first_enabled_at` is set). A set saved while the organization is in `off` does not
/// count as a first enable. `off` -> `log_only` is always allowed.
///
/// Returns `Ok(true)` when the write must save the starter set (`log_only` with no set),
/// `Ok(false)` when it saves none, and `ToolPolicyModeTransitionInvalid` (reason 61) for an
/// `enforce` that the rule refuses. The stored mode does not change the result: a row in
/// `log_only` or `enforce` always has a set and `first_enabled_at`. So the function takes no
/// current mode.
pub(crate) fn check_transition(next: ToolPolicyMode, has_set: bool, ever_enabled: bool) -> Result<bool, TenancyError> {
    match next {
        ToolPolicyMode::Off => Ok(false),
        ToolPolicyMode::LogOnly => Ok(!has_set),
        ToolPolicyMode::Enforce if has_set && ever_enabled => Ok(false),
        ToolPolicyMode::Enforce => Err(TenancyError::ToolPolicyModeTransitionInvalid),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_transition_table_holds() {
        use ToolPolicyMode::{Enforce, LogOnly, Off};
        for has_set in [false, true] {
            for ever in [false, true] {
                assert_eq!(check_transition(Off, has_set, ever), Ok(false), "off, has_set {has_set}, ever {ever}");
            }
        }
        for ever in [false, true] {
            assert_eq!(check_transition(LogOnly, false, ever), Ok(true));
            assert_eq!(check_transition(LogOnly, true, ever), Ok(false));
            assert_eq!(check_transition(Enforce, false, ever), Err(TenancyError::ToolPolicyModeTransitionInvalid));
        }
        // I-2: a set saved in `off` is not a first enable.
        assert_eq!(check_transition(Enforce, true, false), Err(TenancyError::ToolPolicyModeTransitionInvalid));
        assert_eq!(check_transition(Enforce, true, true), Ok(false));
    }
}
