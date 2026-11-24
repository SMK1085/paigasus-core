// SPDX-License-Identifier: Apache-2.0

//! The built-in stage (SMA-855 § 6.6.2 step 1, plan P-1): the kernel built-ins, synchronous. One
//! `detect_call` per call subject and one `injection_markers` per result subject, so the work is
//! linear in the input.

use paigasus_kernel::detect::{CallSubject, DETECTOR_BUILT_IN, Evidence, Finding, ToolInput, detect_call, detector_error, injection_markers};

use crate::domain::detectors::{BuiltInStage, DetectBatch, PipelineOutput, Subject};

pub use crate::domain::detectors::SCAN_LIMIT;

/// The kernel built-ins. `max_scan_bytes` bounds the argument bytes that one run scans.
#[derive(Debug, Clone, Copy)]
pub struct BuiltInDetector {
    max_scan_bytes: usize,
}

impl BuiltInDetector {
    /// A stage that scans at most `max_scan_bytes` of call arguments per run.
    #[must_use]
    pub fn new(max_scan_bytes: usize) -> Self {
        Self { max_scan_bytes }
    }
}

/// The bytes of the arguments. An input shape that this crate does not know counts as too large,
/// so it fails closed.
fn input_len(input: &ToolInput) -> usize {
    match input {
        ToolInput::Json(s) | ToolInput::Text(s) => s.len(),
        _ => usize::MAX,
    }
}

/// The finding for a call that the budget does not cover.
fn scan_limit() -> Finding {
    detector_error(DETECTOR_BUILT_IN).with_evidence(Evidence::text_input(SCAN_LIMIT))
}

impl BuiltInStage for BuiltInDetector {
    fn run(&self, batch: &DetectBatch) -> PipelineOutput {
        let mut out = PipelineOutput {
            findings: Vec::with_capacity(batch.subjects.len()),
            hosts: Vec::with_capacity(batch.subjects.len()),
            stages: Vec::new(),
        };
        // The budget runs over the argument bytes in subject order. A call that does not fit in
        // full is not scanned, and a later call that fits is still scanned.
        let mut left = self.max_scan_bytes;
        for subject in &batch.subjects {
            match subject {
                Subject::Call { invocation, schema, .. } => {
                    let len = input_len(&invocation.input);
                    if len > left {
                        out.findings.push(vec![scan_limit()]);
                        out.hosts.push(None);
                        continue;
                    }
                    left -= len;
                    let mut call = CallSubject::new(&invocation.name, &invocation.input, invocation.declared);
                    if let Some(schema) = schema {
                        call = call.with_schema(schema);
                    }
                    if let Some(seen) = &batch.seen_hosts {
                        call = call.with_seen_hosts(seen);
                    }
                    let found = detect_call(&call);
                    out.findings.push(found.findings);
                    out.hosts.push(Some(found.hosts));
                }
                Subject::Result { index, text, .. } => {
                    out.findings.push(injection_markers(text, *index).into_iter().collect());
                    out.hosts.push(None);
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::detectors::Side;
    use paigasus_kernel::detect::{DETECTOR_ERROR, Declared, Severity};
    use paigasus_kernel::tool_policy::ToolInvocation;

    fn call(index: usize, args: String) -> Subject {
        Subject::Call {
            index,
            invocation: ToolInvocation::new("t", ToolInput::Json(args), Declared::Yes),
            schema: None,
        }
    }

    /// Arguments of exactly `len` bytes.
    fn args_of(len: usize) -> String {
        format!(r#"{{"a":"{}"}}"#, "x".repeat(len - 8))
    }

    #[test]
    fn the_call_budget_gives_scan_limit() {
        let subjects = vec![call(0, args_of(40 * 1024)), call(1, args_of(40 * 1024)), call(2, args_of(10 * 1024))];
        assert_eq!(args_of(40 * 1024).len(), 40 * 1024);
        let batch = DetectBatch::new(Side::Response, None, None, subjects, None);
        let out = BuiltInDetector::new(65536).run(&batch);
        assert_eq!(out.findings.len(), 3);
        assert_eq!(out.findings[0], Vec::<Finding>::new());
        let f = &out.findings[1];
        assert_eq!(f.len(), 1, "{f:?}");
        assert_eq!((f[0].detector.as_str(), f[0].kind.as_str(), f[0].severity), (DETECTOR_BUILT_IN, DETECTOR_ERROR, Severity::High));
        assert_eq!(f[0].evidence.iter().map(|e| e.rule).collect::<Vec<_>>(), [SCAN_LIMIT]);
        assert_eq!(out.findings[2], Vec::<Finding>::new(), "a later call that fits is still scanned");
        assert_eq!(out.hosts.iter().map(Option::is_some).collect::<Vec<_>>(), [true, false, true]);
    }

    #[test]
    fn a_call_that_fills_the_budget_exactly_is_scanned() {
        let batch = DetectBatch::new(Side::Response, None, None, vec![call(0, args_of(65536))], None);
        let out = BuiltInDetector::new(65536).run(&batch);
        assert_eq!((out.findings, out.hosts.len(), out.hosts[0].is_some()), (vec![vec![]], 1, true));
    }
}
