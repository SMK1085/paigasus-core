// SPDX-License-Identifier: Apache-2.0

//! The detector port (SMA-855 § 6.6.1). A detector screens a batch of subjects (tool calls or tool
//! results) and returns one vector of findings per subject. The built-in stage is a synchronous
//! [`BuiltInStage`], because it also returns the hosts of each call (plan P-1). Webhooks and the
//! judge are async [`Detector`]s.
//!
//! Every type here that holds a subject, a host or a finding has a hand-written `Debug` that prints
//! counts and fixed names only (§ 6.2.4, A2).

use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use paigasus_kernel::detect::{Finding, HostSet};
use paigasus_kernel::tool_policy::ToolInvocation;
use serde_json::Value;
use uuid::Uuid;

/// The side of the relay that screens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Side {
    /// The request: tool results that the caller sends.
    Request,
    /// The response: tool calls of the model.
    Response,
}

/// One thing to screen. It borrows nothing: the pipeline runs inside a `'static` decider future.
#[derive(Clone)]
#[non_exhaustive]
pub enum Subject {
    /// A tool call of the model (response side).
    Call {
        /// The position of the call.
        index: usize,
        /// The call.
        invocation: ToolInvocation,
        /// The JSON Schema of the matching tool definition.
        schema: Option<Arc<Value>>,
    },
    /// A tool result in the request (request side). `text` lives only for the request side.
    Result {
        /// The position of the result.
        index: usize,
        /// The name of the tool that gave the result, when it is known.
        tool: Option<String>,
        /// The result text.
        text: Arc<str>,
    },
}

impl Subject {
    /// Whether the subject is a tool call.
    #[must_use]
    pub fn is_call(&self) -> bool {
        matches!(self, Self::Call { .. })
    }
}

impl fmt::Debug for Subject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Call { index, .. } => write!(f, "Subject::Call {{ index: {index}, .. }}"),
            Self::Result { index, .. } => write!(f, "Subject::Result {{ index: {index}, .. }}"),
        }
    }
}

/// The input of one detector run.
#[non_exhaustive]
pub struct DetectBatch {
    /// The side that screens.
    pub side: Side,
    /// The organization of the request.
    pub org: Option<Uuid>,
    /// The request id.
    pub request_id: Option<String>,
    /// The subjects, in order.
    pub subjects: Vec<Subject>,
    /// The hosts of the seen message sources. `None`: no request screen ran.
    pub seen_hosts: Option<Arc<HostSet>>,
    /// The findings of the earlier stages, per subject, with evidence. Empty for the built-ins.
    pub earlier: Vec<Vec<Finding>>,
}

impl DetectBatch {
    /// A batch with no earlier findings.
    #[must_use]
    pub fn new(side: Side, org: Option<Uuid>, request_id: Option<String>, subjects: Vec<Subject>, seen_hosts: Option<Arc<HostSet>>) -> Self {
        Self {
            side,
            org,
            request_id,
            subjects,
            seen_hosts,
            earlier: Vec::new(),
        }
    }
}

impl fmt::Debug for DetectBatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "DetectBatch {{ side: {:?}, org: {}, request_id: {}, subjects: {}, seen_hosts: {}, earlier: {} }}",
            self.side,
            self.org.is_some(),
            self.request_id.is_some(),
            self.subjects.len(),
            self.seen_hosts.as_ref().map_or_else(|| "none".to_owned(), |h| h.len().to_string()),
            self.earlier.len()
        )
    }
}

/// Why a detector gave no findings. Fixed variants, no free text (§ 6.2.4).
#[derive(Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DetectorError {
    /// The detector failed.
    Failed,
    /// The detector returned a number of finding vectors that is not the number of subjects.
    WrongShape,
    /// The detector did not finish before the deadline.
    Timeout,
    /// The detector panicked.
    Panicked,
}

impl fmt::Debug for DetectorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Failed => "Failed",
            Self::WrongShape => "WrongShape",
            Self::Timeout => "Timeout",
            Self::Panicked => "Panicked",
        })
    }
}

/// An async detector: a webhook (SMA-856) or the judge (SMA-857).
#[async_trait]
pub trait Detector: Send + Sync {
    /// The name in `Finding.detector`. It must be a known name (D37).
    fn name(&self) -> &str;
    /// One vector of findings per subject, in subject order.
    async fn detect(&self, batch: Arc<DetectBatch>) -> Result<Vec<Vec<Finding>>, DetectorError>;
}

/// The built-in stage (plan P-1). It is synchronous, and it gives the hosts of each call too.
pub trait BuiltInStage: Send + Sync {
    /// One vector of findings and one host set per subject, in subject order.
    fn run(&self, batch: &DetectBatch) -> PipelineOutput;
}

/// The output of a detector run: per subject, the findings and the hosts. The hosts are `Some`
/// only for a call that the built-in stage scanned in full.
#[derive(Default, Clone, PartialEq, Eq)]
pub struct PipelineOutput {
    /// The findings, per subject.
    pub findings: Vec<Vec<Finding>>,
    /// The hosts, per subject.
    pub hosts: Vec<Option<HostSet>>,
}

impl fmt::Debug for PipelineOutput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let findings: usize = self.findings.iter().map(Vec::len).sum();
        let hosts = self.hosts.iter().filter(|h| h.is_some()).count();
        write!(f, "PipelineOutput {{ subjects: {}, findings: {findings}, hosts: {hosts} }}", self.findings.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use paigasus_kernel::detect::{Declared, Evidence, Severity, ToolInput};

    const CANARY: &str = "canary-7f3e.example";

    fn subjects() -> Vec<Subject> {
        let input = ToolInput::Json(format!(r#"{{"u":"https://{CANARY}/"}}"#));
        vec![
            Subject::Call {
                index: 0,
                invocation: ToolInvocation::new(format!("tool-{CANARY}"), input, Declared::Yes),
                schema: Some(Arc::new(serde_json::json!({ "description": CANARY }))),
            },
            Subject::Result {
                index: 1,
                tool: Some(CANARY.to_owned()),
                text: Arc::from(format!("result {CANARY}")),
            },
        ]
    }

    #[test]
    fn debug_prints_no_content() {
        let subjects = subjects();
        assert_eq!(format!("{:?}", subjects[0]), "Subject::Call { index: 0, .. }");
        assert_eq!(format!("{:?}", subjects[1]), "Subject::Result { index: 1, .. }");
        let mut seen = HostSet::new();
        seen.insert(CANARY);
        let mut batch = DetectBatch::new(Side::Response, Some(Uuid::nil()), Some(CANARY.to_owned()), subjects, Some(Arc::new(seen.clone())));
        batch.earlier = vec![vec![Finding::new("unseen_host", "unseen_host", Severity::Medium, "v1").with_evidence(Evidence::host(
            "r",
            None,
            CANARY.to_owned(),
        ))]];
        let text = format!("{batch:?}");
        assert_eq!(text, "DetectBatch { side: Response, org: true, request_id: true, subjects: 2, seen_hosts: 1, earlier: 1 }");
        let out = PipelineOutput {
            findings: batch.earlier.clone(),
            hosts: vec![Some(seen), None],
        };
        let text = format!("{out:?}");
        assert_eq!(text, "PipelineOutput { subjects: 1, findings: 1, hosts: 1 }");
        assert!(!text.contains(CANARY));
    }

    #[test]
    fn detector_errors_print_only_their_variant() {
        let all = [DetectorError::Failed, DetectorError::WrongShape, DetectorError::Timeout, DetectorError::Panicked];
        assert_eq!(all.map(|e| format!("{e:?}")), ["Failed", "WrongShape", "Timeout", "Panicked"]);
    }

    #[test]
    fn only_a_call_is_a_call() {
        let s = subjects();
        assert_eq!((s[0].is_call(), s[1].is_call()), (true, false));
    }
}
