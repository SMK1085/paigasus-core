# SMA-713 `IamOutboxPublishFailures` single-failure fix Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the `IamOutboxPublishFailures` rule, its promtool test and the runbook agree on
option 3 (`increase(...[2m]) > 0`, `for: 2m`), and prime the publish-failure counter at zero in
`main` when the relay is enabled.

**Architecture:** One Prometheus rule changes window, hold and annotations. One promtool block
replaces the old block with three 15 s series (control, one step, a sustained spell). One gated
`increment(0)` goes into the metrics block of `serve()`. The built binary proves it through the
existing `boot_lifecycle_pg.rs` suite. The runbook text follows the rule.

**Tech Stack:** Prometheus rules and promtool 3.13.1 (via proto), Rust 2024 (`metrics` crate,
`metrics-exporter-prometheus`), tokio integration tests with testcontainers Postgres, cargo
nextest, Moon 2.5.3.

**Spec:** `docs/superpowers/specs/2026-09-27-sma-713-publish-failures-single-failure-design.md`
(approved by Sven on 2026-09-27; option 3 and the prime are the approved design).

## Global Constraints

- Rule: `expr: increase(iam_outbox_relay_publish_failures_total[2m]) > 0`, `for: 2m`, severity
  `warning`. No `sum by`. The `job` and `instance` labels stay on the alert.
- The `summary` stays exactly `IAM outbox publishes are failing (broker unreachable or rejecting)`.
- The new `description` is exactly: `The publish-failure counter increased in each 2-minute
  window for 2 minutes. One failure does not fire this alert. A failure spell of about 90 s or more fires it
  about 2 to 3 minutes after onset. Each IAM replica gives its own alert. Check
  iam_nats_connected. See RUNBOOK section 4.` (one line in the YAML).
- The prime is gated on `config.outbox.relay_enabled` and sits inside `if metrics_handle.is_some()`
  in `serve()` (`rs/crates/services/paigasus-iam/src/main.rs:92-110`).
- Every source file keeps its SPDX header. Rust uses edition 2024, rust-version 1.95.
- Conventional commits with a workspace scope. Every commit message ends with
  `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Never use `git commit --amend`, `git reset`, `--no-verify` or `--no-gpg-sign`. Restore a mutant
  with an edit, never with `git checkout --` (a checkout also reverts uncommitted work).
- Prefix every shell with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`. Run commands
  from the worktree root
  `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-713-publish-failures-single-failure`.
- Do not change `IamOutboxEventsParked`, `IamOutboxBacklogAgeHigh`, any other rule, a dashboard, the
  Helm chart or CI. Do not change the dated SMA-471 plan or spec.
- User-facing prose (runbook, comments, PR text) is in ASD-STE100 Simplified Technical English.

## Review Focus

1. The sample parser in the Rust test matches a `# HELP` or `# TYPE` line, or a longer metric name
   with the same prefix. Expected: it reads only a sample line whose name is exactly
   `iam_outbox_relay_publish_failures_total`. Task 2 pins this with a plain `#[test]` over a fixed
   exposition text.
2. T1 passes because the relay ran, not because of the prime. Expected: T1 reads `/metrics` while
   the migration lock is held, before the unlock, so no relay tick can exist. Task 2 places the
   assertion before the `pg_advisory_unlock` call and the prime-deletion mutant must red T1.
3. The flat control series fires under a `>= 0` mutant, or a `sum by (job)` mutant drops the
   `instance` label. Expected: the promtool block reds. Task 1 runs both mutants.
4. The runbook alert table drifts from the rule again (no gate exists until SMA-720). Expected: the
   table row states `[2m]` and `for 2m`. Task 3 has a grep check that compares both files.
5. A runbook sentence elsewhere still says that one failure fires the alert. Expected: every such
   sentence names the 90 s spell. Task 3 has a grep step over `docs/ops/` for the known phrasings.

---

### Task 1: The rule and its promtool block

**Files:**
- Modify: `ops/observability/prometheus/rules/iam.rules.yml:20-28`
- Test: `ops/observability/prometheus/rules/tests/iam.test.yml:50-66` (replace the block)

**Interfaces:**
- Consumes: nothing.
- Produces: the rule shape and the exact `description` string above. Task 3 copies the
  expression into the runbook table.

- [ ] **Step 1: Write the failing test (replace the old block)**

Replace the whole block at `iam.test.yml:50-66` (from the comment line
`# IamOutboxPublishFailures: increase(...[5m]) > 0 for 5m.` to the end of its `exp_annotations`
line) with:

```yaml
  # IamOutboxPublishFailures (SMA-713): increase(...[2m]) > 0 for 2m.
  # interval 15s is the repo scrape interval (prometheus.yml:3); evaluation_interval 1m (above)
  # is the Prometheus default. The zero seed is faithful: main primes the counter at zero when
  # the relay is enabled, and relay.rs increments it by 0 on every tick.
  #   control:8080  flat at 0. A `>= 0` rule fires on it.
  #   a:8080        exactly one step 0 -> 1 at 10m00s, then flat. It must NEVER fire.
  #   s:8080        12 steps from 10m00s to 12m45s, then flat. It fires 12m-14m.
  # The block pins the window to exactly [2m] and the hold to exactly for: 2m (spec 5.2).
  - interval: 15s
    input_series:
      - series: 'iam_outbox_relay_publish_failures_total{job="iam", instance="control:8080"}'
        values: '0x200'
      - series: 'iam_outbox_relay_publish_failures_total{job="iam", instance="a:8080"}'
        values: '0x39 1x160'
      - series: 'iam_outbox_relay_publish_failures_total{job="iam", instance="s:8080"}'
        values: '0x39 1+1x11 12x150'
    alert_rule_test:
      - eval_time: 10m
        alertname: IamOutboxPublishFailures
        exp_alerts: []
      - eval_time: 11m
        alertname: IamOutboxPublishFailures
        exp_alerts: []
      - eval_time: 12m
        alertname: IamOutboxPublishFailures
        exp_alerts:
          - exp_labels: { severity: warning, job: "iam", instance: "s:8080" }
            exp_annotations:
              summary: "IAM outbox publishes are failing (broker unreachable or rejecting)"
              description: "The publish-failure counter increased in each 2-minute window for 2 minutes. One failure does not fire this alert. A failure spell of about 90 s or more fires it about 2 to 3 minutes after onset. Each IAM replica gives its own alert. Check iam_nats_connected. See RUNBOOK section 4."
      - eval_time: 14m
        alertname: IamOutboxPublishFailures
        exp_alerts:
          - exp_labels: { severity: warning, job: "iam", instance: "s:8080" }
            exp_annotations:
              summary: "IAM outbox publishes are failing (broker unreachable or rejecting)"
              description: "The publish-failure counter increased in each 2-minute window for 2 minutes. One failure does not fire this alert. A failure spell of about 90 s or more fires it about 2 to 3 minutes after onset. Each IAM replica gives its own alert. Check iam_nats_connected. See RUNBOOK section 4."
      - eval_time: 15m
        alertname: IamOutboxPublishFailures
        exp_alerts: []
```

Keep the two-space indent of the other blocks in `tests:`.

- [ ] **Step 2: Run the test to verify it fails against the old rule**

Run: `promtool test rules ops/observability/prometheus/rules/tests/iam.test.yml`
Expected: FAIL. The `IamOutboxPublishFailures` rows at `time: 12m`, `time: 14m` and `time: 15m`
fail (the old rule has no `description`, fires later, and still fires at 15m). This is the
old-rule row of the mutation table (acceptance criterion 3). Copy the output to
`<scratchpad>/sma713/mutations/00-old-rule.txt`. No other alert block may fail.

- [ ] **Step 3: Change the rule and its comment**

In `iam.rules.yml`, replace lines 20-28 (the SMA-471 comment and the rule) with:

```yaml
      # SMA-471: with a real broker, publish failures are the earliest signal that delivery is
      # broken — earlier than parking (max_attempts × poll_interval, about 5 min) and earlier than
      # backlog age. `main` primes the counter at zero when the relay is enabled (SMA-713), and
      # relay.rs increments it by 0 on every tick, so `increase()` sees the first failure.
      #
      # SMA-713. `[2m]` with `for: 2m`: a failure spell of about 90 s or more fires this alert about
      # 2 to 3 minutes after onset. One isolated failure does NOT fire it, on purpose: SMA-471 D9
      # absorbs a short broker restart with no operator action. The old `[5m]` with `for: 5m` fired
      # only 5 to 6 minutes after onset, which is not earlier than parking.
      #
      # The 2m window must hold at least 2 samples, so this rule needs a scrape interval of 1m or
      # less. The repo scrapes every 15s.
      - alert: IamOutboxPublishFailures
        expr: increase(iam_outbox_relay_publish_failures_total[2m]) > 0
        for: 2m
        labels: { severity: warning }
        annotations: { summary: "IAM outbox publishes are failing (broker unreachable or rejecting)", description: "The publish-failure counter increased in each 2-minute window for 2 minutes. One failure does not fire this alert. A failure spell of about 90 s or more fires it about 2 to 3 minutes after onset. Each IAM replica gives its own alert. Check iam_nats_connected. See RUNBOOK section 4." }
```

Do not touch the SMA-706 comment near `iam.rules.yml:319-321` ("see SMA-713"). It stays true.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `promtool check rules ops/observability/prometheus/rules/iam.rules.yml && promtool test rules ops/observability/prometheus/rules/tests/iam.test.yml`
Expected: `SUCCESS` for the rule check and for the test file.

Then run the gates:
`moon run repo:promtool repo:observability-drift`
Expected: both tasks pass.

- [ ] **Step 5: Run the mutation battery (acceptance criterion 3)**

For each row, change only the `IamOutboxPublishFailures` rule in `iam.rules.yml` with an edit,
run `promtool test rules ops/observability/prometheus/rules/tests/iam.test.yml`, save the output
to `<scratchpad>/sma713/mutations/NN-<name>.txt`, record the failing `time:` values, then restore
the rule with an edit. Expected results (measured in the spec, section 5.2):

| # | Mutant | Must fail at |
|---|---|---|
| 00 | old rule (`[5m]`, `for: 5m`, no description) | 12m, 14m, 15m (Step 2) |
| 01 | `> 0` → `>= 0` | 10m, 11m, 12m, 14m, 15m |
| 02 | `sum by (job) (increase(...[2m])) > 0` | 12m, 14m |
| 03 | `[2m]` → `[1m]` | 14m |
| 04 | `[2m]` → `[3m]` | 12m, 15m |
| 05 | `for: 2m` → `for: 1m` | 11m |
| 06 | `for: 2m` → `for: 3m` | 12m |
| 07 | `for: 2m` → `for: 0m` | 10m, 11m |
| 08 | option 1 (`[15m]`, `for: 0m`) | 10m, 11m, 12m, 14m, 15m |
| 09 | delete `description` from annotations | 12m, 14m |

A mutant whose row passes is a finding. Stop and report it; do not weaken the table. After the
last mutant, run Step 4 again and confirm `git diff ops/observability/prometheus/rules/iam.rules.yml`
shows only the Step 3 change. Keep the table with the measured values for the PR description.

- [ ] **Step 6: Commit**

```bash
git add ops/observability/prometheus/rules/iam.rules.yml ops/observability/prometheus/rules/tests/iam.test.yml
git commit -m "fix(repo): fire IamOutboxPublishFailures on a 90 s failure spell with [2m] for 2m (SMA-713)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: Prime the publish-failure counter in `main`, proved through the binary

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/main.rs:92-110` (inside `if metrics_handle.is_some()`)
- Test: `rs/crates/services/paigasus-iam/tests/boot_lifecycle_pg.rs`

**Interfaces:**
- Consumes: `names::IAM_OUTBOX_RELAY_PUBLISH_FAILURES_TOTAL`
  (`rs/crates/libs/paigasus-observability/src/names.rs:101`, value
  `"iam_outbox_relay_publish_failures_total"`), `config.outbox.relay_enabled: bool`
  (`config.rs:437`, default `true`, env `IAM_OUTBOX__RELAY_ENABLED`).
- Produces (test-file local only):
  - `fn spawn_iam_with(db_url: &str, http_port: u16, grpc_port: u16, extra_env: &[(&str, &str)]) -> Child`
  - `fn spawn_iam(db_url: &str, http_port: u16, grpc_port: u16) -> Child` (unchanged signature,
    now calls `spawn_iam_with(.., &[])`)
  - `fn sample_value<'a>(exposition: &'a str, name: &str) -> Option<&'a str>`
  - `async fn wait_for_healthz(port: u16, child: &Child)`

Decision (recorded for the PR): T2 is a separate test. It needs its own process with a different
environment, so it cannot share the process of T1. T1 goes into the existing
`a_lock_blocked_replica_is_bound_and_reports_migrating` test, which saves one container start.

- [ ] **Step 1: Write the parser helper and its unit test**

In `boot_lifecycle_pg.rs`, after `http_status`, add:

```rust
/// The value of the UNLABELLED sample `name` in a Prometheus text exposition, or `None`.
///
/// Reads sample lines only. A `# HELP`/`# TYPE` line is skipped: `describe_iam_metrics()` can
/// write those with no series, so a `contains(name)` would pass with the SMA-713 prime deleted.
/// The name must match exactly, so `<name>_created` or a labelled `<name>{…}` does not count.
fn sample_value<'a>(exposition: &'a str, name: &str) -> Option<&'a str> {
    exposition.lines().filter(|line| !line.starts_with('#')).find_map(|line| {
        let (metric, value) = line.split_once(' ')?;
        (metric == name).then(|| value.trim())
    })
}

#[test]
fn sample_value_reads_only_the_exact_unlabelled_sample() {
    let body = "# HELP iam_outbox_relay_publish_failures_total Outbox rows.\n\
                # TYPE iam_outbox_relay_publish_failures_total counter\n\
                iam_outbox_relay_publish_failures_total_created 17\n\
                iam_outbox_relay_publish_failures_total 0\n";
    assert_eq!(sample_value(body, "iam_outbox_relay_publish_failures_total"), Some("0"));

    let header_only = "# HELP iam_outbox_relay_publish_failures_total Outbox rows.\n\
                       # TYPE iam_outbox_relay_publish_failures_total counter\n";
    assert_eq!(sample_value(header_only, "iam_outbox_relay_publish_failures_total"), None);

    let other = "iam_outbox_relay_publish_failures_total_created 0\n";
    assert_eq!(sample_value(other, "iam_outbox_relay_publish_failures_total"), None);
}
```

- [ ] **Step 2: Run the unit test**

Run: `cd rs && cargo nextest run -p paigasus-iam --test boot_lifecycle_pg sample_value_reads_only_the_exact_unlabelled_sample`
Expected: PASS (it tests the helper, not the product code). Then change `metric == name` to
`metric.starts_with(name)` and run again. Expected: FAIL on the `_created` assertion. Restore
`metric == name` with an edit.

- [ ] **Step 3: Add `spawn_iam_with` and `wait_for_healthz`**

Change `spawn_iam` so that the env list is extendable. Replace the head of the function:

```rust
fn spawn_iam(db_url: &str, http_port: u16, grpc_port: u16) -> Child {
    spawn_iam_with(db_url, http_port, grpc_port, &[])
}

/// [`spawn_iam`] with extra `IAM_*` environment entries, for example
/// `("IAM_OUTBOX__RELAY_ENABLED", "false")` (`Env::prefixed("IAM_").split("__")`, config.rs:998).
fn spawn_iam_with(db_url: &str, http_port: u16, grpc_port: u16, extra_env: &[(&str, &str)]) -> Child {
    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_paigasus-iam"));
    cmd.env("IAM_DATABASE_URL", db_url)
        .env("IAM_HTTP_ADDR", format!("127.0.0.1:{http_port}"))
        .env("IAM_GRPC_ADDR", format!("127.0.0.1:{grpc_port}"))
        .env("IAM_AUTHN__ISSUERS", r#"[{issuer="https://idp.example.com",audiences=["paigasus"]}]"#)
        .env("IAM_API_KEYS__PEPPER", "cGFpZ2FzdXMtc21va2UtcGVwcGVyLW5vdC1hLXJlYWwtc2VjcmV0LTAwMA==")
        .env("IAM_MIGRATION__LOCK_WAIT_SECS", "60")
        .envs(extra_env.iter().copied())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // … the rest of the old spawn_iam body stays unchanged from `let mut child = cmd.spawn()`.
```

Add, after `connect_grpc`:

```rust
/// Polls `/healthz` until the listener answers 200. Panics with the child log otherwise.
async fn wait_for_healthz(port: u16, child: &Child) {
    for _ in 0..100 {
        if let Some((200, _)) = http_status(port, "/healthz").await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the listener must bind while the migration lock is held; log:\n{}", child.tail());
}
```

Do not change the existing poll loop in `a_lock_blocked_replica_is_bound_and_reports_migrating`;
it also asserts the status value. Use `wait_for_healthz` only in the new test.

- [ ] **Step 4: Write T1 (positive) into the existing test**

In `a_lock_blocked_replica_is_bound_and_reports_migrating`, directly after the `/readyz` asserts
(`assert!(body.contains("migrating"), ...)`) and before the app-route check, add:

```rust
    // SMA-713 T1: the publish-failure counter is primed at zero BEFORE the migration. The lock is
    // held, so no relay tick has run and only the prime in `serve()` can make this sample.
    // `/metrics` is on the HTTP port because `metrics.addr` is unset (the default).
    let (status, metrics) = http_status(http_port, "/metrics").await.expect("metrics while migrating");
    assert_eq!(status, 200, "SMA-713: /metrics answers while migrating");
    assert_eq!(
        sample_value(&metrics, "iam_outbox_relay_publish_failures_total"),
        Some("0"),
        "SMA-713 T1: main must prime the publish-failure counter at zero when the relay is enabled; /metrics:\n{metrics}"
    );
```

This must stay BEFORE the `pg_advisory_unlock` call of the test.

- [ ] **Step 5: Write T2 (negative) as a new test**

After `a_lock_blocked_replica_is_bound_and_reports_migrating`, add:

```rust
/// SMA-713 T2: the prime is gated like the relay. With `outbox.relay_enabled = false` no relay
/// runs, so `main` must not make the publish-failure series. The lock is held, as in T1, so the
/// assertion reads the state before any migration.
#[tokio::test]
async fn a_relay_disabled_replica_does_not_prime_the_publish_failure_counter() {
    let Some((node, _pinned)) = support::start_raw_postgres().await else {
        eprintln!("skipping boot lifecycle test: Docker unavailable");
        return;
    };
    let url = support::connection_url(&node).await;
    let holder = connect_pinned(&url).await;
    assert!(
        scalar_bool(&holder, &format!("SELECT pg_try_advisory_lock({MIGRATION_LOCK_KEY}) AS v")).await,
        "the holder must actually acquire the lock"
    );

    let (http_port, grpc_port) = (free_port(), free_port());
    let child = spawn_iam_with(&url, http_port, grpc_port, &[("IAM_OUTBOX__RELAY_ENABLED", "false")]);
    wait_for_healthz(http_port, &child).await;

    let (status, metrics) = http_status(http_port, "/metrics").await.expect("metrics while migrating");
    assert_eq!(status, 200, "SMA-713: /metrics answers while migrating");
    // A control in the same body: the JIT defect series is primed unconditionally
    // (`prime_jit_provisioning_failures`), so an empty or wrong body cannot pass this test.
    assert!(
        metrics.lines().any(|l| !l.starts_with('#') && l.starts_with("iam_jit_provisioning_failures_total{")),
        "control: the unconditional JIT prime must be visible; /metrics:\n{metrics}"
    );
    assert_eq!(
        sample_value(&metrics, "iam_outbox_relay_publish_failures_total"),
        None,
        "SMA-713 T2: with the relay disabled, main must not prime the publish-failure counter; /metrics:\n{metrics}"
    );

    assert!(
        scalar_bool(&holder, &format!("SELECT pg_advisory_unlock({MIGRATION_LOCK_KEY}) AS v")).await,
        "the holder must actually release the lock"
    );
}
```

The control name is checked: `names::IAM_JIT_PROVISIONING_FAILURES_TOTAL` is
`"iam_jit_provisioning_failures_total"` (`names.rs:49`), and `prime_jit_provisioning_failures`
(`authenticate_token.rs:90-94`) writes it with a `defect` label, so its sample line starts with
`iam_jit_provisioning_failures_total{`. `main.rs` calls that prime with no config gate.

- [ ] **Step 6: Run T1 and T2 to verify T1 fails**

Docker must be up (`docker info` exits 0). Run:
`cd rs && cargo nextest run -p paigasus-iam --test boot_lifecycle_pg --no-fail-fast`
Expected: `a_lock_blocked_replica_is_bound_and_reports_migrating` FAILS with
`SMA-713 T1: main must prime ...` (left `None`). `a_relay_disabled_replica_does_not_prime_...`
PASSES. `sigterm_during_the_deferred_phase_exits_promptly` passes. If a test prints
`skipping boot lifecycle test: Docker unavailable`, the run proves nothing: start Docker and run
again.

- [ ] **Step 7: Add the prime in `main.rs`**

In `serve()`, inside `if metrics_handle.is_some() { … }`, directly after the
`if config.outbox.wake_on_commit { … }` block (`main.rs:106-108`), add:

```rust
        // SMA-713: the relay's first counter increment happens only after the migration. Prime the
        // series at zero before the listener binds, so Prometheus has a zero baseline and
        // `increase()` sees the first publish failure. Gated like the relay itself.
        if config.outbox.relay_enabled {
            metrics::counter!(names::IAM_OUTBOX_RELAY_PUBLISH_FAILURES_TOTAL).increment(0);
        }
```

- [ ] **Step 8: Run the tests to verify they pass**

Run: `cd rs && cargo nextest run -p paigasus-iam --test boot_lifecycle_pg --no-fail-fast`
Expected: all 4 tests PASS (3 Docker tests and the parser test), with no "skipping" line.

- [ ] **Step 9: Run the prime mutants (spec 5.3)**

Each mutant must COMPILE. The workspace denies warnings, so a mutant that leaves an unused name
dies in rustc and proves nothing. For each row: edit `main.rs`, `touch rs/crates/services/paigasus-iam/src/main.rs`,
run `cd rs && cargo nextest run -p paigasus-iam --test boot_lifecycle_pg --no-fail-fast`, record
the failing test names, restore with an edit, and `touch` the file again.

| Mutant | Must fail |
|---|---|
| delete the `metrics::counter!(names::IAM_OUTBOX_RELAY_PUBLISH_FAILURES_TOTAL).increment(0);` line (keep the `if` with an empty body) | T1 (`a_lock_blocked_replica_is_bound_and_reports_migrating`) |
| remove the gate: the counter line alone, outside any `if` | T2 (`a_relay_disabled_replica_does_not_prime_the_publish_failure_counter`) |
| invert the gate: `if !config.outbox.relay_enabled` | T1 and T2 |

The build must reach the test run for each mutant. If rustc rejects a mutant, the row proves
nothing: change the mutant until it compiles, and record the change. After the last mutant, run
Step 8 again and confirm that `git diff rs/crates/services/paigasus-iam/src/main.rs` shows only
the Step 7 change.

- [ ] **Step 10: Lint and format**

Run: `cd rs && cargo fmt --check && cargo clippy -p paigasus-iam --all-targets -- -D warnings`
Expected: no output from fmt, clippy clean. If `cargo fmt --check` reports a diff, run
`cargo fmt` and check the diff.

- [ ] **Step 11: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/main.rs rs/crates/services/paigasus-iam/tests/boot_lifecycle_pg.rs
git commit -m "feat(rs): prime the IAM publish-failure counter at zero when the relay is enabled (SMA-713)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: The runbook

**Files:**
- Modify: `docs/ops/RUNBOOK-observability.md:222` (alert table row)
- Modify: `docs/ops/RUNBOOK-observability.md:516-563` (the `IamOutboxPublishFailures` section)
- Modify, only where a sentence says that one failure fires the alert:
  `docs/ops/RUNBOOK-nats.md:35`, `docs/ops/RUNBOOK-observability.md:623`
- Read and leave unchanged unless they state the one-failure claim:
  `docs/ops/RUNBOOK-nats.md:53`, `:66`, `docs/ops/RUNBOOK-observability.md:128`, `:300`, `:580`

**Interfaces:**
- Consumes: the rule of Task 1 (`[2m]`, `for: 2m`) and the prime of Task 2.
- Produces: nothing that code uses.

Decision (recorded for the PR): `RUNBOOK-nats.md:35` ("eventually fires once a tick actually
errors") and `RUNBOOK-observability.md:623` ("still fires once a tick's publish times out") both
state that one failing tick fires the alert. Under option 3 that is false, so both change. Lines
53, 66, 128, 300 and 580 only say that the alert fires or is a signal, so they stay.

- [ ] **Step 1: Write the failing check**

Save as `<scratchpad>/sma713/runbook-check.sh` (a scratch file, not committed):

```bash
#!/bin/bash
set -u
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-713-publish-failures-single-failure
rc=0
grep -q 'increase(iam_outbox_relay_publish_failures_total\[2m\]) > 0' ops/observability/prometheus/rules/iam.rules.yml || { echo "rule is not [2m]"; rc=1; }
grep -qF '| `IamOutboxPublishFailures` | `increase(iam_outbox_relay_publish_failures_total[2m]) > 0` for 2m | warning |' docs/ops/RUNBOOK-observability.md || { echo "table row does not match the rule"; rc=1; }
for s in 'scrape interval of 1 m or less' 'When the alert is silent' 'one alert for each replica' 'about 90 s'; do
  grep -qF "$s" docs/ops/RUNBOOK-observability.md || { echo "missing: $s"; rc=1; }
done
if grep -nE 'very first failure|once a tick actually|once a tick.s publish times out|last 5 minutes and stayed' docs/ops/RUNBOOK-observability.md docs/ops/RUNBOOK-nats.md; then
  echo "stale one-failure or 5m wording"; rc=1
fi
exit $rc
```

- [ ] **Step 2: Run it to verify it fails**

Run: `/bin/bash <scratchpad>/sma713/runbook-check.sh; echo rc=$?`
Expected: `rc=1`, with "table row does not match the rule", the four "missing:" lines and the
stale-wording lines.

- [ ] **Step 3: Change the table row (line 222)**

```markdown
| `IamOutboxPublishFailures` | `increase(iam_outbox_relay_publish_failures_total[2m]) > 0` for 2m | warning |
```

- [ ] **Step 4: Replace the "Meaning" paragraph (lines 518-525)**

```markdown
**Meaning.** `iam_outbox_relay_publish_failures_total` increased in each 2-minute window for the
`for: 2m` hold. A row's `EventPublisher::publish` call failed during relay ticks. A failure spell
of about 90 s or more fires the alert about 2 to 3 minutes after onset. One isolated failure does
NOT fire it, on purpose: SMA-471 D9 absorbs a short broker restart with no operator action. The
alert resolves about 2 to 3 minutes after the last failure (SMA-713).

This is the **earliest** outbox signal. It can fire before `IamOutboxBacklogAgeHigh` (which needs
the backlog to age past 5 minutes) and before `IamOutboxEventsParked` (which needs a row to exhaust
`[outbox].max_attempts`, about 5 minutes at the default `poll_interval_secs`, see above). `main`
primes the counter at zero when `outbox.relay_enabled` is true, and `relay.rs` increments it by 0
on every tick. So `increase()` has a zero baseline before the first failure.

**Scrape interval.** This rule **needs a scrape interval of 1 m or less**. The 2-minute window must
hold at least 2 samples. At a longer interval `increase()` has too few samples and the alert does
not fire. The repo scrapes every 15 s (`ops/observability/prometheus/prometheus.yml:3`).

**One alert for each replica.** The rule is per series, so each IAM replica gives its own alert
with its `instance` label. A broker outage gives one alert for each replica. This repo has no
Alertmanager routing. If your Alertmanager does not group by `alertname` or `job`, expect one
notification for each replica.

**When the alert is silent:**
- one isolated failure, or a failure spell shorter than about 45 s (by design);
- a failure spell of about 45 to 75 s, for some phases against the 1-minute evaluation tick;
- a scrape interval longer than 1 minute;
- `outbox.relay_enabled = false` (no relay runs, and `main` does not prime the series);
- `outbox.publisher.backend = "tracing"` (a publish almost never fails);
- `metrics.enabled = false` (no series);
- a broker that is down at boot: `NatsEventPublisher::connect` fails, `boot_deferred` returns
  `Err` and the process drains and exits. No relay tick runs. Look for `CrashLoopBackOff` and the
  log line `boot failed after the listeners were bound`;
- a tick that returns `Err` before `relay.rs` counts its failures (the row query,
  `active.update` or `txn.commit` fails). The failures of that tick are not counted.

A short spell that does not fire this alert is still visible in the counter, in the relay log
(`outbox event publish failed; will retry`) and in `event_outbox.last_error`. If such a spell
parks a row, `IamOutboxEventsParked` fires.
```

Keep "Likely causes" and "Confirm" as they are.

- [ ] **Step 5: Extend the "Broker down" remediation item**

At the end of the first "Remediation" bullet (the one that starts "Broker down or unreachable"),
add:

```markdown
  When this alert fires, the outage lasted at least about 45 s, and usually 90 s or more. That is
  longer than a routine restart blip. If `iam_nats_connected` is back at 1 and the counter is flat
  again, the alert resolves about 2 to 3 minutes later.
```

- [ ] **Step 6: Correct the two one-failure sentences**

`RUNBOOK-nats.md:35-36`: replace "`IamOutboxPublishFailures` (`RUNBOOK-observability.md` §4)
eventually fires once a tick actually errors, and before that," with
"`IamOutboxPublishFailures` (`RUNBOOK-observability.md` §4) fires when publishes keep failing for
about 90 s or more, and before that,". Keep the rest of the sentence.

`RUNBOOK-observability.md:623`: replace "`IamOutboxPublishFailures` (§4 above) still fires once a
tick's publish times out." with "`IamOutboxPublishFailures` (§4 above) still fires when publishes
keep timing out for about 90 s or more."

Read lines 53, 66, 128, 300 and 580 of the two files again against option 3 and confirm that
none of them states that one failure fires the alert.

- [ ] **Step 7: Run the check to verify it passes**

Run: `/bin/bash <scratchpad>/sma713/runbook-check.sh; echo rc=$?`
Expected: `rc=0` with no output lines. (No formatter gate covers `docs/`: `ts:fmt` formats
only the `ts/` tree.)

- [ ] **Step 8: Commit**

```bash
git add docs/ops/RUNBOOK-observability.md docs/ops/RUNBOOK-nats.md
git commit -m "docs(repo): align the IamOutboxPublishFailures runbook with [2m] for 2m (SMA-713)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: Full gate graph and PR evidence

**Files:**
- No source change. The output goes into the PR description (the open-pr stage writes it).

**Interfaces:**
- Consumes: the commits of Tasks 1-3, the mutation table of Task 1 Step 5, the mutant table of
  Task 2 Step 9.
- Produces: the evidence block for the PR description.

- [ ] **Step 1: Run the targeted gates**

Run: `moon run repo:promtool repo:observability-drift paigasus-iam-rs:test`
Expected: all pass. If `paigasus-iam-rs:test` fails in a Docker-gated suite that this branch does
not touch (for example `authz_policy_store.rs`), re-run that suite alone once and record both runs.

- [ ] **Step 2: Run the full gate graph from the root `CLAUDE.md`**

Run the command between the `ci-targets` markers of the root `CLAUDE.md` (`moon ci :build :test
... :test-e2e --base origin/main --include-relations`). Expected: pass. For a failure, follow the
root `CLAUDE.md` "Diagnosing an unattributed `moon ci` failure" procedure (Step 0 first). A gate
in the bash-version list of the root `CLAUDE.md` that fails with `mapfile`/`declare -A` errors
or a small-pipe rc 2 is a host artifact: re-run it directly with the correct bash and record that
result.

- [ ] **Step 3: Collect the PR evidence**

Write the evidence into a scratch file `<scratchpad>/sma713/pr-evidence.md` (not committed):
- the Task 1 mutation table with the measured failing times (acceptance criterion 3);
- the Task 2 mutant table with the failing test names (acceptance criterion 4);
- the T1/T2 run line that shows no `skipping boot lifecycle test` output;
- the decisions: T2 as a separate test; `RUNBOOK-nats.md:35` and `RUNBOOK-observability.md:623`
  changed, the other mentions unchanged;
- the follow-up issues SMA-720 (runbook-table drift gate) and SMA-721 (out-of-date NATS boot
  section at `RUNBOOK-observability.md:565-573`).

- [ ] **Step 4: Confirm the branch state**

Run: `git status --short && git log --oneline origin/main..HEAD`
Expected: a clean tree (the scratch files are outside the repo) and five commits on the branch:
the spec, the plan, then Tasks 1, 2 and 3.
