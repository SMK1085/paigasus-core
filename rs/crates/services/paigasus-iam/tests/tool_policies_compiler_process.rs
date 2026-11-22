// SPDX-License-Identifier: Apache-2.0

//! The policy compiler process adapter (SMA-774, spec sections 5.2 and 6.2). Each fake compiler
//! is a `#!/bin/sh` script in a temporary directory. The child gets no environment and so no
//! `PATH`: every script names its programs with an absolute path.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

mod support;

use paigasus_iam::adapters::policy_compiler::process::{ProcessLimits, STDERR_KEEP_BYTES, run_child};
use paigasus_iam::adapters::policy_compiler::{PROBE_ATTEMPT_TIMEOUT, PROBE_ATTEMPTS, PROBE_RETRY_DELAY, ProcessPolicyCompiler, probe, probe_with, probe_with_delay};
use paigasus_iam::config::ToolPoliciesConfig;
use paigasus_iam_core::tool_policy::{CompileOutcome, CompileRequest, CompilerFailure, PROTOCOL_VERSION, PolicyCompiler, REASON_FLOOR_PERMIT};
use serde_json::Value;
use tempfile::TempDir;

/// The answer of a good fake to `revision`.
const REVISION_OK: &str = r#"echo '{"compiler_generation":2,"protocol_version":1,"revision":"2+dogwood.000000000000+proto.1"}'"#;

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../tools/paigasus-policy-compiler/protocol/fixtures")
}

fn fixture(name: &str) -> Value {
    let path = fixtures_dir().join(name);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("parse {}: {error}", path.display()))
}

fn request(name: &str) -> CompileRequest {
    serde_json::from_value(fixture(name)).expect("a fixture request parses")
}

/// Writes an executable `#!/bin/sh` script into `dir` and returns its path.
fn script(dir: &TempDir, name: &str, body: &str) -> PathBuf {
    let path = dir.path().join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write the script");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod the script");
    path
}

/// A fake compiler: `revision` answers with `revision`, `compile` runs `compile`.
fn fake_compiler(dir: &TempDir, revision: &str, compile: &str) -> PathBuf {
    script(dir, "compiler", &format!("case \"$1\" in\n  revision) {revision} ;;\n  compile) {compile} ;;\n  *) exit 64 ;;\nesac"))
}

fn config(path: &Path) -> ToolPoliciesConfig {
    ToolPoliciesConfig {
        enabled: true,
        compiler_path: path.to_path_buf(),
        ..ToolPoliciesConfig::default()
    }
}

fn limits(timeout_ms: u64) -> ProcessLimits {
    ProcessLimits::from_config(&ToolPoliciesConfig {
        compile_timeout_ms: timeout_ms,
        ..ToolPoliciesConfig::default()
    })
}

async fn probed(path: &Path) -> ProcessPolicyCompiler {
    probe(&config(path)).await.expect("the fake compiler passes the boot probe")
}

fn assert_unavailable(result: Result<CompileOutcome, CompilerFailure>) -> String {
    match result {
        Err(CompilerFailure::Unavailable(message)) => message,
        other => panic!("expected Unavailable, got {other:?}"),
    }
}

#[tokio::test]
async fn a_crashing_compiler_is_unavailable() {
    let dir = TempDir::new().expect("tempdir");
    let compiler = probed(&fake_compiler(&dir, REVISION_OK, "exit 3")).await;
    assert_unavailable(compiler.compile(&request("request_tenant_accepted.json")).await);
}

#[tokio::test]
async fn a_hanging_compiler_is_killed_at_the_timeout() {
    let dir = TempDir::new().expect("tempdir");
    let pid_file = dir.path().join("pid");
    let path = script(&dir, "hang", &format!("echo $$ > {}\nexec /bin/sleep 60", pid_file.display()));
    let started = Instant::now();
    let result = run_child(&path, &["compile"], b"{}", &limits(500)).await;
    let elapsed = started.elapsed();
    match result {
        Err(CompilerFailure::Unavailable(message)) => assert!(message.contains("timeout"), "{message}"),
        other => panic!("expected Unavailable(timeout), got {other:?}"),
    }
    assert!(elapsed < Duration::from_secs(2), "the call took {elapsed:?}");
    let pid = std::fs::read_to_string(&pid_file).expect("the script wrote its pid");
    let alive = std::process::Command::new("/bin/kill").args(["-0", pid.trim()]).status().expect("run kill -0");
    assert!(!alive.success(), "the child {} is still alive or not reaped", pid.trim());
}

#[tokio::test]
async fn too_much_output_is_unavailable() {
    let dir = TempDir::new().expect("tempdir");
    let path = script(&dir, "flood", "exec /usr/bin/head -c 4000000 /dev/zero");
    let limits = limits(10_000);
    let message = match run_child(&path, &["compile"], b"", &limits).await {
        Err(CompilerFailure::Unavailable(message)) => message,
        other => panic!("expected Unavailable, got {:?}", other.map(|o| o.stdout.len())),
    };
    assert!(message.contains("3145728"), "{message}");

    // The adapter maps it in the same way.
    let compiler = probed(&fake_compiler(&dir, REVISION_OK, "exec /usr/bin/head -c 4000000 /dev/zero")).await;
    assert_unavailable(compiler.compile(&request("request_tenant_accepted.json")).await);
}

#[tokio::test]
async fn bad_json_is_unavailable() {
    let dir = TempDir::new().expect("tempdir");
    let compiler = probed(&fake_compiler(&dir, REVISION_OK, "echo nope")).await;
    assert_unavailable(compiler.compile(&request("request_tenant_accepted.json")).await);
}

#[tokio::test]
async fn a_malformed_result_with_the_right_version_is_unavailable() {
    let dir = TempDir::new().expect("tempdir");
    let compiler = probed(&fake_compiler(&dir, REVISION_OK, r#"echo '{"outcome":"accepted","protocol_version":1,"compiler_generation":2}'"#)).await;
    let message = assert_unavailable(compiler.compile(&request("request_tenant_accepted.json")).await);
    assert!(!message.contains("protocol version"), "a malformed result is not a version mismatch: {message}");
}

/// The version check runs before the typed parse. A result of a newer protocol with a field that
/// IAM does not know must report the version, not a parse error.
#[tokio::test]
async fn a_result_of_another_protocol_version_names_both_versions() {
    let dir = TempDir::new().expect("tempdir");
    let compiler = probed(&fake_compiler(
        &dir,
        REVISION_OK,
        r#"echo '{"outcome":"accepted","protocol_version":2,"compiler_generation":2,"a_new_field":true}'"#,
    ))
    .await;
    let message = assert_unavailable(compiler.compile(&request("request_tenant_accepted.json")).await);
    assert!(message.contains("protocol version 2") && message.contains(&format!("expects {PROTOCOL_VERSION}")), "{message}");

    // A result that is valid in every other way: only the version check refuses it.
    let mut result = fixture("result_tenant_accepted.json");
    result["protocol_version"] = 2.into();
    let result_file = dir.path().join("result.json");
    std::fs::write(&result_file, serde_json::to_vec(&result).expect("serialize")).expect("write the result");
    let compiler = probed(&fake_compiler(&dir, REVISION_OK, &format!("exec /bin/cat {}", result_file.display()))).await;
    let message = assert_unavailable(compiler.compile(&request("request_tenant_accepted.json")).await);
    assert!(message.contains("protocol version 2"), "{message}");
}

#[tokio::test]
async fn a_large_stdin_and_a_large_stdout_do_not_deadlock() {
    let dir = TempDir::new().expect("tempdir");
    let path = script(&dir, "cat", "exec /bin/cat");
    let input = vec![b'x'; 2 * 1024 * 1024];
    let output = run_child(&path, &["compile"], &input, &limits(10_000)).await.expect("cat answers");
    assert!(output.status.success());
    assert_eq!(output.stdout.len(), input.len());
}

#[tokio::test]
async fn the_child_gets_an_empty_environment() {
    // SAFETY: this test binary sets the variable once and no other test in it reads the
    // environment; nextest runs each test in its own process.
    unsafe { std::env::set_var("PAIGASUS_TEST_CANARY", "secret") };
    // `/usr/bin/env` is the child itself, not a `/bin/sh` script: a shell sets `PWD` and
    // `SHLVL` on its own, and those do not come from IAM.
    let output = run_child(Path::new("/usr/bin/env"), &[], b"", &limits(10_000)).await.expect("env runs");
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), "", "the child saw an environment");
}

#[tokio::test]
async fn the_child_runs_in_an_empty_directory() {
    let dir = TempDir::new().expect("tempdir");
    let path = script(&dir, "ls", "exec /bin/ls -A");
    let output = run_child(&path, &[], b"", &limits(10_000)).await.expect("ls runs");
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), "", "the working directory is not empty");
}

#[tokio::test]
async fn stderr_is_truncated_to_4_kib() {
    let dir = TempDir::new().expect("tempdir");
    let path = script(&dir, "noisy", "/usr/bin/head -c 100000 /dev/zero | /usr/bin/tr '\\0' e 1>&2\nexit 1");
    let output = run_child(&path, &[], b"", &limits(10_000)).await.expect("the child runs");
    assert!(!output.status.success());
    assert_eq!(output.stderr_truncated.len(), STDERR_KEEP_BYTES);
}

/// Spec section 5.2: stderr is captured, truncated and logged, also when the run fails. A child
/// that writes to stderr and then hangs past the timeout gets its stderr logged, bounded at
/// `STDERR_KEEP_BYTES`, and the failure message does not hold it.
#[tokio::test]
async fn the_stderr_of_a_timed_out_child_is_logged() {
    let dir = TempDir::new().expect("tempdir");
    let marker = "stderr-marker-before-the-hang";
    let path = script(
        &dir,
        "noisy-hang",
        &format!("echo {marker} 1>&2\n/usr/bin/head -c 100000 /dev/zero | /usr/bin/tr '\\0' e 1>&2\nexec /bin/sleep 60"),
    );
    let (logs, _guard) = paigasus_logging::test_support::capture_logs();
    let message = match run_child(&path, &[], b"", &limits(500)).await {
        Err(CompilerFailure::Unavailable(message)) => message,
        other => panic!("expected Unavailable, got {:?}", other.map(|o| o.status)),
    };
    assert!(message.contains("timeout") && !message.contains(marker), "{message}");
    let text = logs.text();
    assert!(text.contains(marker), "the stderr was not logged: {text}");
    // The kept bytes are the marker line, then `e` up to the bound.
    let es = STDERR_KEEP_BYTES - (marker.len() + 1);
    assert!(text.contains(&"e".repeat(es)), "the log holds less stderr than the bound");
    assert!(!text.contains(&"e".repeat(es + 1)), "the log holds more stderr than the bound");
}

/// The same for a child that is killed at the stdout limit.
#[tokio::test]
async fn the_stderr_of_a_child_over_the_stdout_limit_is_logged() {
    let dir = TempDir::new().expect("tempdir");
    let path = script(&dir, "noisy-flood", "echo stderr-marker-before-the-flood 1>&2\nexec /usr/bin/head -c 4000000 /dev/zero");
    let (logs, _guard) = paigasus_logging::test_support::capture_logs();
    assert!(matches!(run_child(&path, &[], b"", &limits(10_000)).await, Err(CompilerFailure::Unavailable(_))));
    let text = logs.text();
    assert!(text.contains("stderr-marker-before-the-flood"), "the stderr was not logged: {text}");
}

#[tokio::test]
async fn the_child_inherits_only_the_three_pipes() {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

    let dir = TempDir::new().expect("tempdir");
    // A file descriptor that the test process holds open must not reach the child. `F_DUPFD` gives
    // the copy no `FD_CLOEXEC`, as a descriptor from outside Rust can lack it (CI held two such).
    // It picks the lowest unused number from 200 up, so it never closes a descriptor that the test
    // process already holds. The high number keeps it apart from what `ls` opens itself.
    let file = std::fs::File::open(dir.path()).expect("open a descriptor");
    // SAFETY: `fcntl` with `F_DUPFD` writes no memory. The test owns the new descriptor, and
    // `_held` closes it.
    let raw = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_DUPFD, 200) };
    assert!(raw >= 200, "F_DUPFD");
    let _held = unsafe { OwnedFd::from_raw_fd(raw) };
    let flags = unsafe { libc::fcntl(raw, libc::F_GETFD) };
    assert_eq!(flags & libc::FD_CLOEXEC, 0, "the held descriptor must lack FD_CLOEXEC");
    let listing = if cfg!(target_os = "linux") { "/proc/self/fd" } else { "/dev/fd" };
    let path = script(&dir, "fds", &format!("exec /bin/ls {listing}"));
    let output = run_child(&path, &[], b"", &limits(10_000)).await.expect("ls runs");
    assert!(output.status.success());
    let fds: Vec<String> = String::from_utf8_lossy(&output.stdout).split_whitespace().map(str::to_owned).collect();
    assert!(!fds.contains(&raw.to_string()), "the child holds the descriptor of the test: {fds:?}");
    // On Linux 0, 1 and 2 are the pipes and 3 is the directory that `ls` opens to list it. On
    // macOS `ls /dev/fd` opens two descriptors of its own, so only the check above applies.
    if cfg!(target_os = "linux") {
        assert_eq!(fds, ["0", "1", "2", "3"], "the child holds other descriptors");
    }
}

/// The child marks its descriptors `FD_CLOEXEC` and does not close them, so `std` still reports
/// an `exec` failure through its own pipe as a spawn error.
#[tokio::test]
async fn an_exec_failure_is_a_spawn_error() {
    let dir = TempDir::new().expect("tempdir");
    let missing = dir.path().join("no-such-compiler");
    match run_child(&missing, &[], b"", &limits(10_000)).await {
        Err(CompilerFailure::Unavailable(message)) => assert!(message.starts_with("start "), "{message}"),
        other => panic!("expected Unavailable, got {:?}", other.map(|o| o.status)),
    }
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn rlimit_as_stops_a_large_allocation() {
    assert!(Path::new("/usr/bin/python3").exists(), "this test needs /usr/bin/python3; it must not skip");
    let dir = TempDir::new().expect("tempdir");
    let path = script(&dir, "alloc", "exec /usr/bin/python3 -c 'b = bytearray(2 * 1024 ** 3); print(len(b))'");
    let output = run_child(&path, &[], b"", &limits(10_000)).await.expect("python runs");
    assert!(!output.status.success(), "a 2 GiB allocation under a 1 GiB RLIMIT_AS succeeded");

    let compiler = probed(&fake_compiler(&dir, REVISION_OK, "exec /usr/bin/python3 -c 'b = bytearray(2 * 1024 ** 3); print(len(b))'")).await;
    assert_unavailable(compiler.compile(&request("request_tenant_accepted.json")).await);
}

/// I2 (SMA-774 final review): `RLIMIT_CPU` ends a busy child before the wall-clock timeout. The
/// child spins with a 1 s CPU limit under a 60 s timeout. Without `setrlimit(RLIMIT_CPU)` it runs
/// to the timeout, and the message says "timeout".
#[tokio::test]
async fn rlimit_cpu_stops_a_busy_child_before_the_timeout() {
    use std::os::unix::process::ExitStatusExt;
    let dir = TempDir::new().expect("tempdir");
    let spin = "while :; do :; done";
    let cpu_limited = |timeout_ms| {
        ProcessLimits::from_config(&ToolPoliciesConfig {
            compile_timeout_ms: timeout_ms,
            rlimit_cpu_secs: 1,
            ..ToolPoliciesConfig::default()
        })
    };
    let path = script(&dir, "spin", spin);
    let started = Instant::now();
    let output = run_child(&path, &["compile"], b"", &cpu_limited(60_000)).await.expect("the child ends by a signal, not by the timeout");
    let elapsed = started.elapsed();
    // Linux sends SIGKILL when the soft and the hard limit are equal; macOS sends SIGXCPU.
    let signal = output.status.signal();
    assert!(matches!(signal, Some(libc::SIGXCPU) | Some(libc::SIGKILL)), "expected SIGXCPU or SIGKILL, got {:?}", output.status);
    assert!(elapsed < Duration::from_secs(45), "the CPU limit did not fire before the timeout: {elapsed:?}");

    // The adapter fails closed: the compile is unavailable, and the reason is the exit, not a timeout.
    let compiler = probe(&ToolPoliciesConfig {
        compile_timeout_ms: 60_000,
        rlimit_cpu_secs: 1,
        ..config(&fake_compiler(&dir, REVISION_OK, spin))
    })
    .await
    .expect("the fake compiler passes the boot probe");
    let message = assert_unavailable(compiler.compile(&request("request_tenant_accepted.json")).await);
    assert!(message.contains("exited") && !message.contains("timeout"), "{message}");
}

#[tokio::test]
async fn the_semaphore_answers_busy() {
    let dir = TempDir::new().expect("tempdir");
    let started = dir.path().join("started");
    let path = fake_compiler(&dir, REVISION_OK, &format!("echo > {}; exec /bin/sleep 2", started.display()));
    let compiler = Arc::new(probe(&ToolPoliciesConfig { compiler_permits: 1, ..config(&path) }).await.expect("the probe passes"));
    let first = {
        let compiler = Arc::clone(&compiler);
        tokio::spawn(async move { compiler.compile(&request("request_tenant_accepted.json")).await })
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    while !started.exists() {
        assert!(Instant::now() < deadline, "the first compile did not start");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let before = Instant::now();
    assert_eq!(compiler.compile(&request("request_tenant_accepted.json")).await, Err(CompilerFailure::Busy));
    assert!(before.elapsed() < Duration::from_millis(500), "Busy was not immediate");
    assert_unavailable(first.await.expect("the first task joins"));
}

#[tokio::test]
async fn a_result_with_a_foreign_generation_is_unavailable() {
    let dir = TempDir::new().expect("tempdir");
    let mut result = fixture("result_tenant_accepted.json");
    result["compiler_generation"] = 99.into();
    let result_file = dir.path().join("result.json");
    std::fs::write(&result_file, serde_json::to_vec(&result).expect("serialize")).expect("write the result");
    let compiler = probed(&fake_compiler(&dir, REVISION_OK, &format!("exec /bin/cat {}", result_file.display()))).await;
    let message = assert_unavailable(compiler.compile(&request("request_tenant_accepted.json")).await);
    assert!(message.contains("99"), "{message}");
}

/// The control for the failure cases: a fixture result passes through unchanged, and so do the
/// reason codes of a rejection.
#[tokio::test]
async fn fixture_results_pass_through() {
    let dir = TempDir::new().expect("tempdir");
    for (name, accepted) in [("result_tenant_accepted.json", true), ("result_floor_rejected.json", false)] {
        let result_file = dir.path().join(name);
        // The fixture is the output of the real compiler. The scripted fake keeps generation 2
        // (SMA-855 P-7), so the replayed result carries the generation of the fake.
        let mut result = fixture(name);
        result["compiler_generation"] = 2.into();
        std::fs::write(&result_file, serde_json::to_vec(&result).expect("serialize")).expect("write the result");
        let compiler = probed(&fake_compiler(&dir, REVISION_OK, &format!("/bin/cat > /dev/null; exec /bin/cat {}", result_file.display()))).await;
        assert_eq!(compiler.generation(), 2);
        match compiler.compile(&request("request_tenant_accepted.json")).await {
            Ok(CompileOutcome::Accepted(compiled)) if accepted => assert!(!compiled.cedar_policies.is_empty()),
            Ok(CompileOutcome::Rejected(rejections)) if !accepted => {
                let expected: Vec<String> = fixture(name)["rejections"]
                    .as_array()
                    .expect("rejections")
                    .iter()
                    .map(|r| r["reason"].as_str().expect("reason").to_owned())
                    .collect();
                let got: Vec<String> = rejections.iter().map(|r| r.reason.clone()).collect();
                assert_eq!(got, expected);
                assert!(got.iter().any(|r| r == REASON_FLOOR_PERMIT), "{got:?}");
            }
            other => panic!("{name}: unexpected {other:?}"),
        }
    }
}

#[tokio::test]
async fn a_source_over_the_limit_is_rejected_before_the_process_starts() {
    let dir = TempDir::new().expect("tempdir");
    let marker = dir.path().join("ran");
    let compiler = probed(&fake_compiler(&dir, REVISION_OK, &format!("echo > {}; exit 1", marker.display()))).await;
    let mut req = request("request_tenant_accepted.json");
    req.source = "x".repeat(262_145);
    match compiler.compile(&req).await {
        Ok(CompileOutcome::Rejected(rejections)) => {
            assert_eq!(rejections.len(), 1);
            assert_eq!(rejections[0].reason, "tool-policy-invalid");
            assert!(rejections[0].message.contains("262144"), "{}", rejections[0].message);
        }
        other => panic!("expected one rejection, got {other:?}"),
    }
    assert!(!marker.exists(), "the compiler started");
    // The control: a source at the limit reaches the compiler.
    req.source = "x".repeat(262_144);
    assert_unavailable(compiler.compile(&req).await);
    assert!(marker.exists(), "the compiler did not start for a source at the limit");
}

#[tokio::test]
async fn probe_returns_none_for_a_missing_binary() {
    let dir = TempDir::new().expect("tempdir");
    assert!(probe(&config(&dir.path().join("absent"))).await.is_none());
}

#[tokio::test]
async fn probe_returns_none_for_a_failing_revision() {
    let dir = TempDir::new().expect("tempdir");
    assert!(probe(&config(&fake_compiler(&dir, "exit 2", "exit 1"))).await.is_none());
}

// ---- the boot probe retry (SMA-774 ruling 2) --------------------------------------------------

/// A config with a short timeout, so that a hanging `revision` fails fast.
fn quick_config(path: &Path) -> ToolPoliciesConfig {
    ToolPoliciesConfig {
        compile_timeout_ms: 1_000,
        ..config(path)
    }
}

/// The number of `revision` calls that a script counted: one line each in `counter`.
fn attempts(counter: &Path) -> usize {
    std::fs::read_to_string(counter).map_or(0, |text| text.lines().count())
}

const SHORT_WAIT: Duration = Duration::from_millis(20);

#[tokio::test]
async fn the_probe_retries_a_transient_failure_and_then_succeeds() {
    let dir = TempDir::new().expect("tempdir");
    let counter = dir.path().join("counter");
    // The first two calls hang past the timeout. The third answers.
    let body = format!(
        "echo x >> {c}\nn=$(/usr/bin/wc -l < {c})\nif [ \"$n\" -lt 3 ]; then exec /bin/sleep 5; fi\n{REVISION_OK}",
        c = counter.display()
    );
    let path = script(&dir, "flaky", &body);
    let compiler = probe_with_delay(&quick_config(&path), SHORT_WAIT).await.expect("the third attempt passes");
    assert_eq!(compiler.generation(), 2);
    assert_eq!(attempts(&counter), 3);
}

#[tokio::test]
async fn three_transient_failures_give_none() {
    let dir = TempDir::new().expect("tempdir");
    let counter = dir.path().join("counter");
    let path = script(&dir, "hang", &format!("echo x >> {}\nexec /bin/sleep 5", counter.display()));
    assert!(probe_with_delay(&quick_config(&path), SHORT_WAIT).await.is_none());
    assert_eq!(attempts(&counter), 3, "the probe makes exactly three attempts");
}

/// M5 (SMA-774 final review): the probe bounds each attempt by its own short limit, not by the
/// 10 s `compile_timeout_ms`. With a 200 ms attempt limit, three hanging attempts end in well
/// under one compile timeout.
#[tokio::test]
async fn the_probe_attempt_limit_bounds_a_hanging_binary() {
    let dir = TempDir::new().expect("tempdir");
    let counter = dir.path().join("counter");
    let path = script(&dir, "hang", &format!("echo x >> {}\nexec /bin/sleep 60", counter.display()));
    let cfg = config(&path);
    assert_eq!(cfg.compile_timeout_ms, 10_000, "the test needs the default compile timeout");
    let started = Instant::now();
    assert!(probe_with(&cfg, SHORT_WAIT, Duration::from_millis(200)).await.is_none());
    let elapsed = started.elapsed();
    assert_eq!(attempts(&counter), 3);
    assert!(elapsed < Duration::from_secs(5), "the probe took {elapsed:?}; the compile timeout bounded it");
}

/// The default per-attempt limit of the probe, and the boot delay it gives on a hanging binary.
#[tokio::test]
async fn the_default_probe_attempt_limit_is_3_s() {
    assert_eq!(PROBE_ATTEMPT_TIMEOUT, Duration::from_secs(3));
    assert!(PROBE_ATTEMPT_TIMEOUT < Duration::from_millis(ToolPoliciesConfig::default().compile_timeout_ms));
    let worst = PROBE_ATTEMPT_TIMEOUT * PROBE_ATTEMPTS + PROBE_RETRY_DELAY * (PROBE_ATTEMPTS - 1);
    assert_eq!(worst, Duration::from_secs(11));

    // probe_with_delay uses the default limit: three hanging attempts take about 9 s, not 30 s.
    let dir = TempDir::new().expect("tempdir");
    let path = script(&dir, "hang", "exec /bin/sleep 60");
    let started = Instant::now();
    assert!(probe_with_delay(&config(&path), SHORT_WAIT).await.is_none());
    let elapsed = started.elapsed();
    assert!(elapsed >= PROBE_ATTEMPT_TIMEOUT * PROBE_ATTEMPTS, "the probe took {elapsed:?}");
    assert!(elapsed < PROBE_ATTEMPT_TIMEOUT * PROBE_ATTEMPTS + Duration::from_secs(3), "the probe took {elapsed:?}");
}

#[tokio::test]
async fn a_permanent_failure_is_not_retried() {
    let dir = TempDir::new().expect("tempdir");
    // A non-zero exit, a foreign protocol version and a bad answer: one attempt each.
    for (name, body) in [
        ("exit", "exit 2"),
        ("version", r#"echo '{"compiler_generation":1,"protocol_version":2,"revision":"x"}'"#),
        ("garbage", "echo nope"),
    ] {
        let counter = dir.path().join(format!("counter-{name}"));
        let path = script(&dir, name, &format!("echo x >> {}\n{body}", counter.display()));
        assert!(probe_with_delay(&quick_config(&path), SHORT_WAIT).await.is_none(), "{name}");
        assert_eq!(attempts(&counter), 1, "{name} was tried again");
    }
}

#[tokio::test]
async fn probe_returns_none_for_a_foreign_protocol_version() {
    let dir = TempDir::new().expect("tempdir");
    // With a new field (the typed parse alone would refuse it) and without one (only the version
    // check refuses it).
    for revision in [
        r#"echo '{"compiler_generation":1,"protocol_version":2,"revision":"x","a_new_field":1}'"#,
        r#"echo '{"compiler_generation":1,"protocol_version":2,"revision":"x"}'"#,
    ] {
        assert!(probe(&config(&fake_compiler(&dir, revision, "exit 1"))).await.is_none(), "{revision}");
    }
}

/// The real compiler, end to end. It runs when `PAIGASUS_POLICY_COMPILER_BIN` is set, which the
/// Moon `test` task of this crate does after it builds the binary. It skips with a printed reason
/// when the variable is not set, and it panics when the variable names a file that is absent.
#[tokio::test]
async fn the_real_compiler_answers_every_fixture_request() {
    let Some(path) = support::real_compiler_path("the_real_compiler_answers_every_fixture_request") else {
        return;
    };
    let compiler = probed(&path).await;
    for (req, res) in [
        ("request_tenant_accepted.json", "result_tenant_accepted.json"),
        ("request_tenant_rejected.json", "result_tenant_rejected.json"),
        ("request_floor_accepted.json", "result_floor_accepted.json"),
        ("request_floor_rejected.json", "result_floor_rejected.json"),
    ] {
        let expected = fixture(res);
        let got = match compiler.compile(&request(req)).await.unwrap_or_else(|e| panic!("{req}: {e:?}")) {
            CompileOutcome::Accepted(compiled) => serde_json::to_value(compiled).expect("serialize")["cedar_policies"].clone(),
            CompileOutcome::Rejected(rejections) => serde_json::to_value(rejections).expect("serialize"),
        };
        let want = if expected["outcome"] == "accepted" {
            expected["compiled"]["cedar_policies"].clone()
        } else {
            expected["rejections"].clone()
        };
        assert_eq!(got, want, "{req}");
    }
}

// ---- The catalog size cap (spec section 4.2) against the real compiler --------------------------

/// The largest catalog of `make(i)` tools whose `ListToolsResponse` is at most `cap` bytes.
fn catalog_at_cap(cap: usize, max_tools: usize, make: impl Fn(usize) -> paigasus_iam_core::tool_policy::ToolEntry) -> Vec<paigasus_iam_core::tool_policy::StoredTool> {
    use paigasus_iam::application::tool_policies::wire::catalog_response_len;
    let at = chrono::DateTime::from_timestamp(1_800_000_000, 999_999_999).expect("a time");
    let mut tools = Vec::new();
    for i in 0..max_tools {
        tools.push(paigasus_iam_core::tool_policy::StoredTool {
            entry: make(i),
            created_at: at,
            updated_at: at,
        });
        if catalog_response_len(&tools, 1) > cap {
            tools.pop();
            break;
        }
    }
    tools
}

/// A catalog at the default `max_catalog_response_bytes` gives a compile request and a compiled
/// result that the default limits accept. Five catalog forms: four with schemas near the 64 KiB
/// limit (string, boolean, array and nested properties, so the Cedar type is a large part of the
/// wire size), and one with the most tools of a small schema (the per-action overhead of the
/// schema is the largest part). It prints the sizes and the wall time of each run.
#[tokio::test]
async fn a_catalog_at_the_size_cap_compiles_within_the_default_limits() {
    use paigasus_iam::application::tool_policies::wire::catalog_response_len;
    use paigasus_iam_core::tool_policy::{CatalogLimits, SetKind, ToolKind, build_manual, tenant_schema_json};
    let Some(path) = support::real_compiler_path("a_catalog_at_the_size_cap_compiles_within_the_default_limits") else {
        return;
    };
    let defaults = ToolPoliciesConfig::default();
    let limits = CatalogLimits::default();
    // One schema of `p00000: string` plus as many `property` values as fit in 60 000 bytes.
    let filled = |property: Value| {
        let mut props = serde_json::Map::new();
        props.insert("p00000".to_string(), serde_json::json!({"type": "string"}));
        for i in 1.. {
            props.insert(format!("p{i:05}"), property.clone());
            if serde_json::json!({"type": "object", "properties": props}).to_string().len() > 60_000 {
                props.remove(&format!("p{i:05}"));
                break;
            }
        }
        serde_json::json!({"type": "object", "properties": props})
    };
    let forms = [
        ("strings", filled(serde_json::json!({"type": "string"}))),
        ("booleans", filled(serde_json::json!({"type": "boolean"}))),
        ("arrays", filled(serde_json::json!({"type": "array", "items": {"type": "string"}}))),
        (
            "nested",
            filled(serde_json::json!({"type": "object", "properties": {"a": {"type": "object", "properties": {"b": {"type": "string"}}}}})),
        ),
        ("small", serde_json::json!({"type": "object", "properties": {"p00000": {"type": "string"}, "n": {"type": "integer"}}})),
    ];
    // The image ships a release build (`rs/Dockerfile`), and the Moon task builds a debug binary.
    // A debug build is about 14 times slower, so it runs only the form with the largest result
    // (`strings`), and it does not check the default CPU and wall-clock limits.
    let release = path.components().any(|c| c.as_os_str() == "release");
    for (form, schema) in forms {
        if !release && form != "strings" {
            continue;
        }
        let tools = catalog_at_cap(defaults.max_catalog_response_bytes, limits.max_tools, |i| {
            build_manual(&format!("t{i:03}"), ToolKind::Function, Some(schema.clone()), &limits).expect("a valid tool")
        });
        let wire = catalog_response_len(&tools, 1);
        let entries: Vec<_> = tools.iter().map(|t| t.entry.clone()).collect();
        // The source names a tool and holds a temporal block, so the result schema has a slot.
        let source = "@id(\"t-rate\")\npermit (principal, action == Pgs::Gateway::Action::\"t000\", resource)\nwhen temporal { count_within(1h, Pgs::Gateway::Action::\"t000\"::request{ input.p00000: \"x\" }) <= 2 };\n".to_string();
        let request = CompileRequest {
            protocol_version: PROTOCOL_VERSION,
            kind: SetKind::Tenant,
            source,
            schema_json: tenant_schema_json(&entries),
            max_window_ms: defaults.max_window_secs * 1000,
        };
        let stdin = serde_json::to_vec(&request).expect("serialize");
        // Measure the real stdout with high limits: a debug build of the compiler needs more than
        // the default 8 s of CPU for a catalog near the cap.
        let measure = ProcessLimits::from_config(&ToolPoliciesConfig {
            max_result_bytes: 64 * 1024 * 1024,
            compile_timeout_ms: 120_000,
            rlimit_cpu_secs: 110,
            ..defaults.clone()
        });
        let started = Instant::now();
        let raw = run_child(&path, &["compile"], &stdin, &measure).await.expect("the compiler runs");
        let elapsed_ms = started.elapsed().as_millis();
        assert!(raw.status.success(), "{form}: status {:?}, stdin {} bytes: {}", raw.status, stdin.len(), raw.stderr_truncated);
        let result: Value = serde_json::from_slice(&raw.stdout).expect("JSON");
        assert_eq!(result["outcome"], "accepted", "{form}: {result}");
        let schema_len = result["compiled"]["cedar_schema_json"].to_string().len();
        println!(
            "CAP_MEASURE form={form} tools={} list_tools_response={wire} compile_request={} result={} result_schema={schema_len} wall_ms={elapsed_ms} cap={} max_result={}",
            tools.len(),
            stdin.len(),
            raw.stdout.len(),
            defaults.max_catalog_response_bytes,
            defaults.max_result_bytes
        );
        assert!(raw.stdout.len() <= defaults.max_result_bytes, "{form}: the result is {} bytes", raw.stdout.len());
        if release {
            let compiler = probed(&path).await;
            assert!(
                matches!(compiler.compile(&request).await, Ok(CompileOutcome::Accepted(_))),
                "{form}: the default limits refuse the compile"
            );
        } else {
            println!("CAP_MEASURE form={form}: not a release build, so the default CPU and time limits are not checked");
        }
    }
}

// ---- The parent environment (spec section 5.2): `make_parent_non_dumpable` -------------------
//
// `PR_SET_DUMPABLE 0` cannot be undone in a useful way, so each case runs in a new process of
// this test binary: `parent_environ_case` below. The outer test starts it once as a dumpable
// parent (the positive control) and once as a non-dumpable parent (the check).

/// The variable that selects the case of [`parent_environ_case`]. Without it the case returns.
#[cfg(target_os = "linux")]
const ENVIRON_CASE: &str = "PAIGASUS_TEST_ENVIRON_CASE";
/// The marker that the case process holds in its own environment.
#[cfg(target_os = "linux")]
const ENVIRON_MARKER: &str = "PAIGASUS_TEST_PARENT_SECRET=marker-7f3c1d";

/// One case process. It is a no-op unless the outer test set [`ENVIRON_CASE`]. For
/// `non-dumpable` it calls the production `make_parent_non_dumpable` first. Then it runs a fake
/// compiler through the production `run_child`. The fake prints `/proc/$PPID/environ`, which is
/// the environment of this process. The verdict line goes to stdout for the outer test.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn parent_environ_case() {
    use paigasus_iam::adapters::policy_compiler::process::{ParentHardening, make_parent_non_dumpable};
    let Ok(case) = std::env::var(ENVIRON_CASE) else { return };
    match case.as_str() {
        "dumpable" => {}
        "non-dumpable" => assert_eq!(make_parent_non_dumpable().expect("prctl"), ParentHardening::Applied),
        other => panic!("unknown case {other}"),
    }
    let dir = TempDir::new().expect("tempdir");
    // The child has no PATH, so `/bin/cat` is absolute. A refused read prints nothing to stdout.
    let path = script(&dir, "peek", "/bin/cat /proc/$PPID/environ 2>/dev/null\nexit 0");
    let output = run_child(&path, &["compile"], b"", &limits(10_000)).await.expect("the fake runs");
    assert!(output.status.success());
    let marker = ENVIRON_MARKER.as_bytes();
    let read = output.stdout.windows(marker.len()).any(|w| w == marker);
    println!("ENVIRON_VERDICT case={case} bytes={} marker_read={read}", output.stdout.len());
}

/// Runs [`parent_environ_case`] in a new process of this test binary. Returns its verdict line.
#[cfg(target_os = "linux")]
fn run_environ_case(case: &str) -> String {
    let exe = std::env::current_exe().expect("the test binary");
    let (key, value) = ENVIRON_MARKER.split_once('=').expect("the marker");
    let out = std::process::Command::new(exe)
        .args(["--exact", "parent_environ_case", "--nocapture", "--test-threads", "1"])
        .env(ENVIRON_CASE, case)
        .env(key, value)
        .output()
        .expect("start the case process");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(out.status.success(), "the {case} case failed:\n{stdout}\n{}", String::from_utf8_lossy(&out.stderr));
    stdout
        .lines()
        .find(|l| l.contains("ENVIRON_VERDICT"))
        .unwrap_or_else(|| panic!("the {case} case printed no verdict:\n{stdout}"))
        .to_string()
}

/// Linux only. The positive control proves that the fake really reads the environ of a dumpable
/// parent, so the check is not empty.
#[cfg(target_os = "linux")]
#[test]
fn a_compiler_child_cannot_read_the_environ_of_a_non_dumpable_parent() {
    let control = run_environ_case("dumpable");
    assert!(control.contains("marker_read=true"), "the positive control did not read the environ of the parent: {control}");
    let check = run_environ_case("non-dumpable");
    println!("{control}\n{check}");
    assert!(
        check.contains("marker_read=false") && check.contains("bytes=0"),
        "the child read the environ of a non-dumpable parent: {check}"
    );
}

/// Off Linux, `make_parent_non_dumpable` changes nothing and says so.
#[cfg(not(target_os = "linux"))]
#[test]
fn make_parent_non_dumpable_is_not_supported_off_linux() {
    use paigasus_iam::adapters::policy_compiler::process::{ParentHardening, make_parent_non_dumpable};
    assert_eq!(make_parent_non_dumpable().expect("no error"), ParentHardening::NotSupported);
}
