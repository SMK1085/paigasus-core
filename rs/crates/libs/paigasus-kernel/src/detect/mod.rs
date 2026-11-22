// SPDX-License-Identifier: Apache-2.0
//! The tool call detectors (SMA-855). Feature `detectors`, off by default. This module has no
//! Cedar type, so a build with only this feature never gets Cedar.

// The strict JSON reading has a caller only in the tool policy for now. A detector takes it
// over in a later task, and the gate on `tool-policy` goes away then.
#[cfg(feature = "tool-policy")]
pub(crate) mod input;
mod types;

pub use types::{
    DETECTOR_BUILT_IN, DETECTOR_ERROR, DETECTOR_KERNEL, Declared, Evidence, Finding, INJECTION_MARKERS, INVALID_ARGS, Location, NAME_COLLISION, NO_SEVERITY, SECRET_IN_ARGS, Severity, ToolInput,
    UNDECLARED_TOOL, UNPARSED_ARGUMENTS, UNSEEN_HOST, detector_error,
};
