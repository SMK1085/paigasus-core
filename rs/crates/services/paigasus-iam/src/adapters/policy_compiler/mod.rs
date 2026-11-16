// SPDX-License-Identifier: Apache-2.0

//! The [`PolicyCompiler`] adapter: it runs `paigasus-policy-compiler` as a child process
//! (SMA-774, spec sections 5.2 and 6.2).
//!
//! [`probe`] runs `revision` once at boot. A missing binary, a failed call or another protocol
//! version gives `None`: IAM still boots, and the tool policy RPCs report `capability-disabled`.
//! Each [`ProcessPolicyCompiler::compile`] checks that the result has the protocol version and
//! the compiler generation of the boot probe.

pub mod process;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use paigasus_iam_core::tool_policy::{CompileOutcome, CompileRequest, CompileResult, CompilerFailure, PROTOCOL_VERSION, PolicyCompiler, REASON_INVALID, Rejection, RevisionOutput};
use serde::de::DeserializeOwned;
use tokio::sync::Semaphore;
use tracing::{error, info, warn};

use self::process::{ProcessLimits, RawOutput, run_child};
use crate::config::ToolPoliciesConfig;

/// The policy compiler as a child process.
#[derive(Debug)]
pub struct ProcessPolicyCompiler {
    path: PathBuf,
    limits: ProcessLimits,
    max_source_bytes: usize,
    permits: Arc<Semaphore>,
    generation: u32,
    /// The revision string of the boot probe. Every compile failure log names it.
    revision: String,
}

/// Runs `revision` on the configured binary. `None`, with an `error!` log, when the binary is
/// missing, the call fails, or the compiler speaks another protocol version.
pub async fn probe(cfg: &ToolPoliciesConfig) -> Option<ProcessPolicyCompiler> {
    let limits = ProcessLimits::from_config(cfg);
    let path = cfg.compiler_path.as_path();
    if ProcessLimits::rlimit_as_applied() {
        info!(
            rlimit_as_bytes = limits.rlimit_as(),
            rlimit_cpu_secs = cfg.rlimit_cpu_secs,
            "the policy compiler runs under RLIMIT_AS and RLIMIT_CPU"
        );
    } else {
        warn!(
            rlimit_as_bytes = limits.rlimit_as(),
            rlimit_cpu_secs = cfg.rlimit_cpu_secs,
            "this OS does not apply RLIMIT_AS, so the policy compiler runs under RLIMIT_CPU only"
        );
    }
    let revision: RevisionOutput = match run_checked(path, &["revision"], b"", &limits).await {
        Ok(revision) => revision,
        Err(failure) => {
            let message = match failure {
                CompilerFailure::Unavailable(message) => message,
                CompilerFailure::Busy => "busy".to_string(),
            };
            error!(compiler = %path.display(), reason = %message, "the policy compiler failed its boot probe; the tool policy feature is disabled");
            return None;
        }
    };
    info!(
        compiler = %path.display(),
        compiler_generation = revision.compiler_generation,
        revision = %revision.revision,
        "the policy compiler passed its boot probe"
    );
    Some(ProcessPolicyCompiler {
        path: path.to_path_buf(),
        limits,
        max_source_bytes: cfg.max_source_bytes,
        permits: Arc::new(Semaphore::new(cfg.compiler_permits)),
        generation: revision.compiler_generation,
        revision: revision.revision,
    })
}

impl ProcessPolicyCompiler {
    /// The revision string of the boot probe, for the boot log.
    #[must_use]
    pub fn revision(&self) -> &str {
        &self.revision
    }
}

#[async_trait]
impl PolicyCompiler for ProcessPolicyCompiler {
    fn generation(&self) -> u32 {
        self.generation
    }

    async fn compile(&self, req: &CompileRequest) -> Result<CompileOutcome, CompilerFailure> {
        // The size limit belongs to IAM, not to the binary: an oversized source never starts a
        // process. It is the caller's input, so it is a rejection, not a compiler failure.
        if req.source.len() > self.max_source_bytes {
            return Ok(CompileOutcome::Rejected(vec![Rejection {
                policy_id: None,
                line: None,
                column: None,
                reason: REASON_INVALID.to_string(),
                message: format!("the source is {} bytes; the limit is {} bytes", req.source.len(), self.max_source_bytes),
            }]));
        }
        let Ok(_permit) = self.permits.try_acquire() else {
            return Err(CompilerFailure::Busy);
        };
        let stdin = serde_json::to_vec(req).map_err(|error| CompilerFailure::Unavailable(format!("serialize the compile request: {error}")))?;
        let result: CompileResult = run_checked(&self.path, &["compile"], &stdin, &self.limits).await.inspect_err(|failure| {
            if let CompilerFailure::Unavailable(message) = failure {
                warn!(compiler = %self.path.display(), revision = %self.revision, reason = %message, "the policy compiler is unavailable");
            }
        })?;
        if result.compiler_generation() != self.generation {
            let message = format!("the result has compiler generation {}; the boot probe reported {}", result.compiler_generation(), self.generation);
            warn!(compiler = %self.path.display(), revision = %self.revision, reason = %message, "the policy compiler is unavailable");
            return Err(CompilerFailure::Unavailable(message));
        }
        Ok(match result {
            CompileResult::Accepted { compiled, .. } => CompileOutcome::Accepted(compiled),
            // The rejections pass through unchanged, reason codes included.
            CompileResult::Rejected { rejections, .. } => CompileOutcome::Rejected(rejections),
        })
    }
}

/// Runs the child, requires exit 0, checks `protocol_version`, then parses the typed result.
async fn run_checked<T: DeserializeOwned>(path: &Path, args: &[&str], stdin: &[u8], limits: &ProcessLimits) -> Result<T, CompilerFailure> {
    let RawOutput { status, stdout, stderr_truncated } = run_child(path, args, stdin, limits).await?;
    if !stderr_truncated.is_empty() {
        warn!(compiler = %path.display(), stderr = %stderr_truncated, "the policy compiler wrote to stderr");
    }
    if !status.success() {
        return Err(CompilerFailure::Unavailable(format!("the compiler exited with {status}")));
    }
    parse_versioned(&stdout)
}

/// Reads `protocol_version` before the typed parse. The typed results refuse unknown fields, so a
/// result of another protocol version must be reported as that, not as a parse error.
fn parse_versioned<T: DeserializeOwned>(stdout: &[u8]) -> Result<T, CompilerFailure> {
    let value: serde_json::Value = serde_json::from_slice(stdout).map_err(|error| CompilerFailure::Unavailable(format!("the compiler stdout is not JSON: {error}")))?;
    match value.get("protocol_version").and_then(serde_json::Value::as_u64) {
        Some(version) if version == u64::from(PROTOCOL_VERSION) => {}
        Some(version) => {
            return Err(CompilerFailure::Unavailable(format!("the compiler speaks protocol version {version}; IAM expects {PROTOCOL_VERSION}")));
        }
        None => return Err(CompilerFailure::Unavailable("the compiler stdout has no numeric protocol_version".to_string())),
    }
    serde_json::from_value(value).map_err(|error| CompilerFailure::Unavailable(format!("the compiler stdout does not match the protocol: {error}")))
}
