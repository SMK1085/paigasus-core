// SPDX-License-Identifier: Apache-2.0

//! The [`PolicyCompiler`] adapter: it runs `paigasus-policy-compiler` as a child process
//! (SMA-774, spec sections 5.2 and 6.2).
//!
//! [`probe`] runs `revision` at boot, with a bounded retry for a transient failure. A missing binary, a failed call or another protocol
//! version gives `None`: IAM still boots, and the tool policy RPCs report `capability-disabled`.
//! Each [`ProcessPolicyCompiler::compile`] checks that the result has the protocol version and
//! the compiler generation of the boot probe.

pub mod process;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

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

/// The attempts of the boot probe. A transient failure (the child does not start, an I/O error,
/// a timeout) gets up to this many tries. A permanent failure gets one.
pub const PROBE_ATTEMPTS: u32 = 3;
/// The wait between two attempts of the boot probe.
pub const PROBE_RETRY_DELAY: Duration = Duration::from_secs(1);

/// Runs `revision` on the configured binary. `None`, with an `error!` log, when the binary is
/// missing, the call fails, or the compiler speaks another protocol version. A transient failure
/// is tried again, at most [`PROBE_ATTEMPTS`] times in all, with [`PROBE_RETRY_DELAY`] between the
/// attempts. A non-zero exit, a protocol version mismatch and a parse failure are not tried again.
pub async fn probe(cfg: &ToolPoliciesConfig) -> Option<ProcessPolicyCompiler> {
    probe_with_delay(cfg, PROBE_RETRY_DELAY).await
}

/// [`probe`] with its own wait between attempts. Tests use a short wait.
#[doc(hidden)]
pub async fn probe_with_delay(cfg: &ToolPoliciesConfig, retry_delay: Duration) -> Option<ProcessPolicyCompiler> {
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
    let mut attempt = 0;
    let revision: RevisionOutput = loop {
        attempt += 1;
        let (reason, transient) = match run_child(path, &["revision"], b"", &limits).await {
            // The child did not run to the end (start, I/O, timeout): transient.
            Err(failure) => (failure_text(failure), true),
            // The child ran. A non-zero exit or a bad answer is permanent.
            Ok(raw) => match check_output::<RevisionOutput>(path, raw) {
                Ok(revision) => break revision,
                Err(failure) => (failure_text(failure), false),
            },
        };
        if transient && attempt < PROBE_ATTEMPTS {
            warn!(compiler = %path.display(), attempt, attempts = PROBE_ATTEMPTS, reason = %reason, "the policy compiler boot probe failed; it will be tried again");
            tokio::time::sleep(retry_delay).await;
            continue;
        }
        error!(compiler = %path.display(), attempt, reason = %reason, transient, "the policy compiler failed its boot probe; the tool policy feature is disabled");
        return None;
    };
    info!(
        compiler = %path.display(),
        attempts = attempt,
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
    check_output(path, run_child(path, args, stdin, limits).await?)
}

fn failure_text(failure: CompilerFailure) -> String {
    match failure {
        CompilerFailure::Unavailable(message) => message,
        CompilerFailure::Busy => "busy".to_string(),
    }
}

/// Requires exit 0, checks `protocol_version`, then parses the typed result.
fn check_output<T: DeserializeOwned>(path: &Path, output: RawOutput) -> Result<T, CompilerFailure> {
    let RawOutput { status, stdout, stderr_truncated } = output;
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
