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
    CallSubject, DETECTOR_BUILT_IN, DETECTOR_ERROR, DETECTOR_KERNEL, Declared, Evidence, Finding, INJECTION_MARKERS, INVALID_ARGS, Location, NAME_COLLISION, NO_SEVERITY, SECRET_IN_ARGS, Severity,
    ToolInput, UNDECLARED_TOOL, UNPARSED_ARGUMENTS, UNSEEN_HOST, detector_error,
};
