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

/// The most distinct names that an opt-in list may hold.
const WEBHOOK_OPT_IN_MAX: usize = 16;
/// The most characters in one provider name.
const WEBHOOK_NAME_MAX: usize = 64;

/// Validates and normalises a webhook opt-in list (SMA-856): each name matches `[a-z0-9_]{1,64}`,
/// and the list holds at most 16 distinct names. Returns the names sorted and unique. Any
/// other input is `InvalidWebhookOptIn`.
pub(crate) fn normalise_opt_in(names: Vec<String>) -> Result<Vec<String>, TenancyError> {
    let valid = |n: &str| !n.is_empty() && n.len() <= WEBHOOK_NAME_MAX && n.bytes().all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'_'));
    if !names.iter().all(|n| valid(n)) {
        return Err(TenancyError::InvalidWebhookOptIn);
    }
    let mut unique = std::collections::BTreeSet::new();
    for name in names {
        unique.insert(name);
        // Stop at the first excess entry. Duplicates do not count.
        if unique.len() > WEBHOOK_OPT_IN_MAX {
            return Err(TenancyError::InvalidWebhookOptIn);
        }
    }
    Ok(unique.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_opt_in_list_is_sorted_unique_and_bounded() {
        assert_eq!(normalise_opt_in(vec!["b".into(), "a".into(), "b".into()]).unwrap(), ["a", "b"]);
        assert_eq!(normalise_opt_in(vec![]).unwrap(), Vec::<String>::new());
        let sixteen: Vec<String> = (0..16).map(|i| format!("p{i}")).collect();
        assert_eq!(normalise_opt_in(sixteen.iter().chain(&sixteen).cloned().collect()).unwrap().len(), 16, "duplicates do not count");
        let seventeen: Vec<String> = (0..17).map(|i| format!("p{i}")).collect();
        assert_eq!(normalise_opt_in(seventeen), Err(TenancyError::InvalidWebhookOptIn));
        for bad in ["", "Acme", "a-b", "a b", "é", &"a".repeat(65)] {
            assert_eq!(normalise_opt_in(vec![bad.to_owned()]), Err(TenancyError::InvalidWebhookOptIn), "{bad:?}");
        }
        assert!(normalise_opt_in(vec!["a".repeat(64)]).is_ok());
    }

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

    /// The one client message covers both causes of the refusal: no earlier change away from
    /// `off`, and no saved set.
    #[test]
    fn the_refusal_message_names_both_causes() {
        let message = TenancyError::ToolPolicyModeTransitionInvalid.to_string();
        assert!(message.contains("log_only first"), "{message}");
        assert!(message.contains("saved tool policy set"), "{message}");
    }
}
