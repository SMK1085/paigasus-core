// SPDX-License-Identifier: Apache-2.0

//! One run of the policy compiler as a hardened child process (spec section 5.2).
//!
//! - `env_clear()`: the child gets no environment of its own. This does not hide the environment
//!   of IAM: on Linux a process under the same UID can read `/proc/<pid>/environ` and
//!   `/proc/<pid>/mem` of a dumpable process. So `main` calls [`make_parent_non_dumpable`] before
//!   the boot probe starts the first child. Then the kernel refuses these reads to the child.
//! - An empty, temporary working directory. The child inherits only the three pipes. Rust opens
//!   its own descriptors with `O_CLOEXEC`, but a descriptor from another source can lack the flag
//!   (for example a jobserver pipe from the parent of IAM). So `pre_exec` sets `FD_CLOEXEC` on
//!   every descriptor from 3 up, and `exec` closes them all. See [`cloexec_from_3`].
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

    /// The same limits with another wall-clock timeout. The boot probe uses it.
    #[must_use]
    pub fn with_timeout(&self, timeout: Duration) -> Self {
        ProcessLimits { timeout, ..self.clone() }
    }

    /// The wall-clock timeout of one run.
    pub fn timeout(&self) -> Duration {
        self.timeout
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

/// The result of [`make_parent_non_dumpable`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParentHardening {
    /// Linux: IAM is not dumpable now. A child under the same UID cannot read
    /// `/proc/<IAM pid>/environ` or `/proc/<IAM pid>/mem`.
    Applied,
    /// This OS has no `PR_SET_DUMPABLE`. IAM changed nothing.
    NotSupported,
}

/// Makes the IAM process not dumpable (`prctl(PR_SET_DUMPABLE, 0)`), on Linux only.
///
/// The kernel then refuses `/proc/<pid>/environ`, `/proc/<pid>/mem` and `ptrace` to a process
/// that has no `CAP_SYS_PTRACE`, also under the same UID. The kernel also gives the
/// `/proc/<pid>` files to root and writes no core dump of IAM. The call cannot be undone in a
/// useful way, so a test calls it in a separate process only.
///
/// # Errors
///
/// The `prctl` error, or an error when `PR_GET_DUMPABLE` does not report 0 after the call.
pub fn make_parent_non_dumpable() -> std::io::Result<ParentHardening> {
    #[cfg(target_os = "linux")]
    {
        // SAFETY: `prctl` with `PR_SET_DUMPABLE` and `PR_GET_DUMPABLE` reads and writes no memory
        // of this process. The unused arguments are 0, as the man page requires.
        let set = unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0 as libc::c_ulong, 0 as libc::c_ulong, 0 as libc::c_ulong, 0 as libc::c_ulong) };
        if set != 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: as above.
        let now = unsafe { libc::prctl(libc::PR_GET_DUMPABLE, 0 as libc::c_ulong, 0 as libc::c_ulong, 0 as libc::c_ulong, 0 as libc::c_ulong) };
        if now != 0 {
            return Err(std::io::Error::other(format!("PR_GET_DUMPABLE reports {now} after PR_SET_DUMPABLE 0")));
        }
        Ok(ParentHardening::Applied)
    }
    #[cfg(not(target_os = "linux"))]
    {
        Ok(ParentHardening::NotSupported)
    }
}

/// The highest descriptor number plus 1 that [`cloexec_from_3`] must cover on the fallback path:
/// the `RLIMIT_NOFILE` soft limit, at most [`MAX_FD_SCAN`].
fn descriptor_limit() -> libc::c_int {
    let mut nofile = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
    // SAFETY: `getrlimit` writes only into `nofile`, which is a valid `rlimit`.
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut nofile) } != 0 {
        return MAX_FD_SCAN;
    }
    libc::c_int::try_from(nofile.rlim_cur).map_or(MAX_FD_SCAN, |limit| limit.min(MAX_FD_SCAN))
}

/// The upper bound of the fallback loop in [`cloexec_from_3`]. An unlimited `RLIMIT_NOFILE`
/// would otherwise make the loop run for minutes. 2^20 is the default Linux `nr_open`.
const MAX_FD_SCAN: libc::c_int = 1 << 20;

/// Sets `FD_CLOEXEC` on every descriptor from 3 up, so that `exec` closes each of them. The
/// child then keeps only stdin, stdout and stderr, which `std` has moved to 0, 1 and 2 already.
///
/// The function sets the flag and does not close the descriptors: `std` keeps an internal
/// `O_CLOEXEC` pipe open until `exec`, and it reports an `exec` error through that pipe.
///
/// Linux uses one `close_range(3, ~0, CLOSE_RANGE_CLOEXEC)` call. A kernel before 5.11 refuses
/// it (`ENOSYS` or `EINVAL`), and every other OS has no such call. Then a loop calls `fcntl` on
/// each number below `limit`. The loop ignores `EBADF` for a number that is not open.
///
/// It runs in the forked child, so it is async-signal-safe: it allocates nothing and takes no
/// lock.
fn cloexec_from_3(limit: libc::c_int) {
    #[cfg(target_os = "linux")]
    {
        // SAFETY: `close_range` takes integers only and touches no memory of this process.
        let done = unsafe { libc::syscall(libc::SYS_close_range, 3 as libc::c_uint, libc::c_uint::MAX, libc::CLOSE_RANGE_CLOEXEC) };
        if done == 0 {
            return;
        }
    }
    for fd in 3..limit {
        // SAFETY: `fcntl` with `F_SETFD` changes only the descriptor flags of `fd`. A number
        // that is not open gives `EBADF` and changes nothing.
        unsafe {
            libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
        }
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
    // Read before the fork: the child must not call more than it needs.
    let fd_limit = descriptor_limit();
    // SAFETY: the closure runs in the forked child before `exec`. It only calls `setrlimit`,
    // `fcntl` and `close_range`, which are async-signal-safe, and it allocates nothing.
    unsafe {
        command.pre_exec(move || {
            cloexec_from_3(fd_limit);
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
