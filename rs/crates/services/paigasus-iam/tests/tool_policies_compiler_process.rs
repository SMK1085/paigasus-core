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
use paigasus_iam::adapters::policy_compiler::{ProcessPolicyCompiler, probe, probe_with_delay};
use paigasus_iam::config::ToolPoliciesConfig;
use paigasus_iam_core::tool_policy::{CompileOutcome, CompileRequest, CompilerFailure, PROTOCOL_VERSION, PolicyCompiler, REASON_FLOOR_PERMIT};
use serde_json::Value;
use tempfile::TempDir;

/// The answer of a good fake to `revision`.
const REVISION_OK: &str = r#"echo '{"compiler_generation":1,"protocol_version":1,"revision":"1+dogwood.000000000000+proto.1"}'"#;

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
    let compiler = probed(&fake_compiler(&dir, REVISION_OK, r#"echo '{"outcome":"accepted","protocol_version":1,"compiler_generation":1}'"#)).await;
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
        r#"echo '{"outcome":"accepted","protocol_version":2,"compiler_generation":1,"a_new_field":true}'"#,
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

#[cfg(target_os = "linux")]
#[tokio::test]
async fn the_child_inherits_only_the_three_pipes() {
    let dir = TempDir::new().expect("tempdir");
    // A file descriptor that the test process holds open must not reach the child.
    let _held = std::fs::File::open(dir.path()).expect("open a descriptor");
    let path = script(&dir, "fds", "exec /bin/ls /proc/self/fd");
    let output = run_child(&path, &[], b"", &limits(10_000)).await.expect("ls runs");
    assert!(output.status.success());
    let fds: Vec<String> = String::from_utf8_lossy(&output.stdout).split_whitespace().map(str::to_owned).collect();
    // 0, 1 and 2 are the pipes. 3 is the directory that `ls` opens to list it.
    assert_eq!(fds, ["0", "1", "2", "3"], "the child holds other descriptors");
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
        std::fs::write(&result_file, serde_json::to_vec(&fixture(name)).expect("serialize")).expect("write the result");
        let compiler = probed(&fake_compiler(&dir, REVISION_OK, &format!("/bin/cat > /dev/null; exec /bin/cat {}", result_file.display()))).await;
        assert_eq!(compiler.generation(), 1);
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
        compile_timeout_ms: 300,
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
    assert_eq!(compiler.generation(), 1);
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
