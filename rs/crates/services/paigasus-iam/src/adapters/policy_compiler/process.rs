// SPDX-License-Identifier: Apache-2.0

//! One run of the policy compiler as a hardened child process (spec section 5.2).
//!
//! - `env_clear()`: the child gets no environment, so it cannot read the database, NATS or Redis
//!   credentials of IAM.
//! - An empty, temporary working directory. The child inherits only the three pipes: Rust opens
//!   every other descriptor with `O_CLOEXEC`.
//! - `RLIMIT_CPU` everywhere, and `RLIMIT_AS` on Linux only (macOS refuses it), set in
//!   `pre_exec`.
//! - IAM writes stdin and reads stdout and stderr at the same time, so the pipes cannot deadlock.
//! - A stdout over the limit, or a run over the timeout, kills and reaps the child.
//! - Stderr is truncated to [`STDERR_KEEP_BYTES`] and logged, also when the run fails. It never
//!   reaches an RPC caller.

use std::path::Path;
use std::process::{ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use paigasus_iam_core::tool_policy::CompilerFailure;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tracing::warn;

use crate::config::ToolPoliciesConfig;

/// The number of stderr bytes that a run keeps. The rest is read and dropped.
pub const STDERR_KEEP_BYTES: usize = 4096;

/// How long IAM waits for the end of stderr after the child exited or was killed. A grandchild
/// that holds stderr open cannot make a run wait longer than this.
const STDERR_GRACE: Duration = Duration::from_millis(250);

/// The limits of one child process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessLimits {
    timeout: Duration,
    max_stdout: usize,
    rlimit_as: u64,
    rlimit_cpu: u64,
}

impl ProcessLimits {
    pub fn from_config(cfg: &ToolPoliciesConfig) -> Self {
        ProcessLimits {
            timeout: Duration::from_millis(cfg.compile_timeout_ms),
            max_stdout: cfg.max_result_bytes,
            rlimit_as: cfg.rlimit_as_bytes,
            rlimit_cpu: cfg.rlimit_cpu_secs,
        }
    }

    /// The `RLIMIT_AS` value. The boot log reads it on every OS, also where it is not applied.
    pub fn rlimit_as(&self) -> u64 {
        self.rlimit_as
    }

    /// Whether this OS applies `RLIMIT_AS` to the child.
    pub const fn rlimit_as_applied() -> bool {
        cfg!(target_os = "linux")
    }
}

/// What a child that ran to its end gave back.
#[derive(Debug)]
pub struct RawOutput {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    /// At most [`STDERR_KEEP_BYTES`] of stderr, for the log only.
    pub stderr_truncated: String,
}

/// Runs `path` with `args`, writes `stdin` to it and reads its output under `limits`.
///
/// A spawn failure, an I/O failure, a stdout over the limit and a timeout are all
/// [`CompilerFailure::Unavailable`]. On those paths this function logs the truncated stderr
/// itself, and the failure message never holds it. The exit status is the caller's to judge.
pub async fn run_child(path: &Path, args: &[&str], stdin: &[u8], limits: &ProcessLimits) -> Result<RawOutput, CompilerFailure> {
    let workdir = tempfile::tempdir().map_err(|error| CompilerFailure::Unavailable(format!("make the compiler working directory: {error}")))?;
    let mut command = Command::new(path);
    command
        .args(args)
        .env_clear()
        .current_dir(workdir.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let rlimit_cpu = limits.rlimit_cpu;
    #[cfg(target_os = "linux")]
    let rlimit_as = limits.rlimit_as;
    // SAFETY: the closure runs in the forked child before `exec`. It only calls `setrlimit`,
    // which is async-signal-safe, and it allocates nothing.
    unsafe {
        command.pre_exec(move || {
            let cpu = libc::rlimit {
                rlim_cur: rlimit_cpu,
                rlim_max: rlimit_cpu,
            };
            if libc::setrlimit(libc::RLIMIT_CPU, &cpu) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            #[cfg(target_os = "linux")]
            {
                let address_space = libc::rlimit {
                    rlim_cur: rlimit_as,
                    rlim_max: rlimit_as,
                };
                if libc::setrlimit(libc::RLIMIT_AS, &address_space) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
            }
            Ok(())
        });
    }

    let mut child = command.spawn().map_err(|error| CompilerFailure::Unavailable(format!("start {}: {error}", path.display())))?;
    let (Some(mut child_stdin), Some(child_stdout), Some(child_stderr)) = (child.stdin.take(), child.stdout.take(), child.stderr.take()) else {
        return Err(CompilerFailure::Unavailable("the child has no pipes".to_string()));
    };
    let max_stdout = limits.max_stdout;
    // Stderr goes into a bounded buffer that a separate task fills. The buffer outlives the run
    // future, so stderr is still there to log after a timeout or a kill.
    let stderr_buffer = Arc::new(Mutex::new(Vec::with_capacity(STDERR_KEEP_BYTES)));
    let mut stderr_task = tokio::spawn(drain_bounded(child_stderr, Arc::clone(&stderr_buffer)));

    let run = async {
        let write = async move {
            match child_stdin.write_all(stdin).await {
                Ok(()) => {}
                // A child that exits without reading all of stdin is not an I/O failure here; its
                // exit status and its stdout decide.
                Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => {}
                Err(error) => return Err(CompilerFailure::Unavailable(format!("write the compiler stdin: {error}"))),
            }
            // Dropping the handle closes stdin, so the child sees the end of the request.
            drop(child_stdin);
            Ok(())
        };
        let read_stdout = async move {
            let mut stdout = Vec::new();
            let limit = u64::try_from(max_stdout).unwrap_or(u64::MAX).saturating_add(1);
            child_stdout
                .take(limit)
                .read_to_end(&mut stdout)
                .await
                .map_err(|error| CompilerFailure::Unavailable(format!("read the compiler stdout: {error}")))?;
            if stdout.len() > max_stdout {
                return Err(CompilerFailure::Unavailable(format!("the compiler stdout is larger than {max_stdout} bytes")));
            }
            Ok(stdout)
        };
        let ((), stdout) = tokio::try_join!(write, read_stdout)?;
        let status = child.wait().await.map_err(|error| CompilerFailure::Unavailable(format!("wait for the compiler: {error}")))?;
        Ok((status, stdout))
    };

    let outcome = tokio::time::timeout(limits.timeout, run).await;
    let result = match outcome {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(failure)) => Err(failure),
        Err(_elapsed) => Err(CompilerFailure::Unavailable(format!("timeout after {} ms", limits.timeout.as_millis()))),
    };
    if result.is_err() {
        // Kill and reap the child, so that no process and no zombie stays behind.
        // `kill_on_drop` is the backstop if this future is dropped first.
        let _ = child.kill().await;
    }
    // The child is gone, so stderr reaches its end at once, unless a grandchild holds it.
    let _ = tokio::time::timeout(STDERR_GRACE, &mut stderr_task).await;
    stderr_task.abort();
    let stderr = stderr_text(&stderr_buffer);
    drop(workdir);
    match result {
        Ok((status, stdout)) => Ok(RawOutput {
            status,
            stdout,
            stderr_truncated: stderr,
        }),
        Err(CompilerFailure::Unavailable(reason)) => {
            warn!(compiler = %path.display(), reason = %reason, stderr = %stderr, "the policy compiler run failed");
            Err(CompilerFailure::Unavailable(reason))
        }
        Err(failure) => Err(failure),
    }
}

/// Reads `reader` to the end. `buffer` keeps the first [`STDERR_KEEP_BYTES`] bytes, and the rest
/// is dropped. A read error ends the drain: stderr is for the log only.
async fn drain_bounded(mut reader: impl AsyncRead + Unpin, buffer: Arc<Mutex<Vec<u8>>>) {
    let mut chunk = [0u8; 8192];
    while let Ok(n) = reader.read(&mut chunk).await {
        if n == 0 {
            break;
        }
        let Ok(mut kept) = buffer.lock() else {
            break;
        };
        let room = STDERR_KEEP_BYTES.saturating_sub(kept.len());
        kept.extend_from_slice(&chunk[..n.min(room)]);
    }
}

/// The kept stderr as text, at most [`STDERR_KEEP_BYTES`] bytes.
fn stderr_text(buffer: &Mutex<Vec<u8>>) -> String {
    let kept = match buffer.lock() {
        Ok(kept) => kept.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    };
    let mut text = String::from_utf8_lossy(&kept).into_owned();
    // A replacement character is 3 bytes, so the lossy text can be longer than the bytes kept.
    while text.len() > STDERR_KEEP_BYTES {
        text.pop();
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The drain holds at most `STDERR_KEEP_BYTES` in memory, however much the child writes.
    /// `stderr_text` cuts the text again, so only this test sees the bound of the buffer itself.
    #[tokio::test]
    async fn the_stderr_drain_keeps_at_most_the_bound() {
        let input = vec![b'e'; 100_000];
        let buffer = Arc::new(Mutex::new(Vec::new()));
        drain_bounded(input.as_slice(), Arc::clone(&buffer)).await;
        assert_eq!(buffer.lock().expect("the lock").len(), STDERR_KEEP_BYTES);
        assert_eq!(stderr_text(&buffer).len(), STDERR_KEEP_BYTES);
    }
}
