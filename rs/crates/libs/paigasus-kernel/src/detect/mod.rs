// SPDX-License-Identifier: Apache-2.0
//! The tool call detectors (SMA-855). Feature `detectors`, off by default. This module has no
//! Cedar type, so a build with only this feature never gets Cedar.

mod call;
mod hosts;
mod injection;
pub(crate) mod input;
mod schema;
mod secrets;
mod types;
mod walk;

pub use call::{CallDetection, detect_call, undeclared_tool, unseen_host};
pub use hosts::{HOSTS_CAP, HOSTS_OVERFLOW, HostSet, call_hosts, hosts_in_text};
pub use injection::{INJECTION_MARKERS_VERSION, injection_markers};
pub use schema::invalid_args;
pub use secrets::{SECRET_PATTERNS_VERSION, secret_in_args};

pub use types::{
    CallSubject, DETECTOR_BUILT_IN, DETECTOR_ERROR, DETECTOR_KERNEL, Declared, Evidence, FORGED_HISTORY, Finding, HISTORY_TRUNCATED, INJECTION_MARKERS, INVALID_ARGS, Location, MIXED_SESSIONS,
    NAME_COLLISION, NO_SEVERITY, SECRET_IN_ARGS, Severity, ToolInput, UNDECLARED_TOOL, UNPARSED_ARGUMENTS, UNSEEN_HOST, detector_error,
};

/// Kinds that the response side can give with no request screen (§ 6.8.0, D27). Every other kind
/// is screen-dependent: `unseen_host`, `injection_markers`, `detector_error`, and every kind of
/// SMA-856 and SMA-857. The three history kinds of SMA-753 are request-level kinds, like
/// `name_collision`.
pub const SCREEN_INDEPENDENT_KINDS: &[&str] = &[
    NAME_COLLISION,
    UNPARSED_ARGUMENTS,
    UNDECLARED_TOOL,
    INVALID_ARGS,
    SECRET_IN_ARGS,
    MIXED_SESSIONS,
    FORGED_HISTORY,
    HISTORY_TRUNCATED,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_screen_independent_kinds() {
        assert_eq!(
            SCREEN_INDEPENDENT_KINDS,
            [
                "name_collision",
                "unparsed_arguments",
                "undeclared_tool",
                "invalid_args",
                "secret_in_args",
                "mixed_sessions",
                "forged_history",
                "history_truncated"
            ]
        );
        for dependent in [UNSEEN_HOST, INJECTION_MARKERS, DETECTOR_ERROR] {
            assert!(!SCREEN_INDEPENDENT_KINDS.contains(&dependent), "{dependent}");
        }
    }
}
