# SMA-466 — Shared `TargetDown` Rules File Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move the service-agnostic `TargetDown` alert into its own shared rules file, and make the observability drift guard both read the right files and actually run when they change.

**Architecture:** Four independent deliverables. Task 1 is ops YAML only (the move itself, plus a fixture that proves the alert is shared, discriminating, and un-duplicated). Task 2 replaces two hardcoded file lists in the Rust drift test with a fail-closed directory glob. Task 3 adds a `repo`-scoped Moon task so that glob actually executes on ops-only PRs. Task 4 is documentation. Each task ends green and committable on its own.

**Tech Stack:** Prometheus alerting rules (YAML) validated by `promtool` 3.13.1 (proto-pinned); Rust 2024 / 1.95 integration test using only `std::fs`; Moon 2.3.2 task graph; GitHub Actions.

**Spec:** `docs/superpowers/specs/2026-08-01-sma-466-targetdown-shared-rules-file-design.md` (rev 2 + D6)

## Global Constraints

- Every new source file opens with an SPDX header: `# SPDX-License-Identifier: Apache-2.0` for YAML, `// SPDX-License-Identifier: Apache-2.0` for Rust.
- Conventional commits with a workspace scope. **Subject must start lowercase** and the whole header must be **≤100 chars**. Never put a bare `#NNN` in the commit body — it breaks commitlint's `footer-leading-blank`. Body lines ≤100 chars.
- Do **not** bypass the commit hook with `--no-verify`.
- Shell commands need the proto shims first on PATH: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.
- `cargo nextest` exits non-zero on a target with no tests — pass `--no-tests=pass`.
- The alert's identity must not change: name `TargetDown`, `expr: up == 0`, `for: 2m`, `labels: { severity: critical }`, and the exact annotation `"Scrape target {{ $labels.job }}/{{ $labels.instance }} is down"`.
- New file is named `targets.rules.yml` / `targets.test.yml` — **not** `common.*` (spec D1).
- No new Cargo dependencies. Task 2 uses `std::fs` only.
- Branch is already created: `feature/sma-466-observability-move-targetdown-alert-to-a-shared-prometheus`.

---

### Task 1: Move `TargetDown` into a shared rules file

**Files:**
- Create: `ops/observability/prometheus/rules/targets.rules.yml`
- Create: `ops/observability/prometheus/rules/tests/targets.test.yml`
- Modify: `ops/observability/prometheus/rules/gateway.rules.yml:20-26` (delete the `targets` group)
- Modify: `ops/observability/prometheus/rules/tests/gateway.test.yml:55-68` (delete the `TargetDown` case)

**Interfaces:**
- Consumes: nothing.
- Produces: the file `ops/observability/prometheus/rules/targets.rules.yml`, which Task 2's glob will pick up and Task 4's docs will describe. No Rust symbols.

**Why the step order matters:** the new fixture globs `rule_files: ['../*.rules.yml']`, so while `gateway.rules.yml` *still* defines `TargetDown` the fixture sees **two** definitions and reports four alerts instead of two. That failure is the point — it is the cross-file duplicate guard (spec D5) proving itself before the move completes.

- [ ] **Step 1: Create the new rules file**

`ops/observability/prometheus/rules/targets.rules.yml`:

```yaml
# SPDX-License-Identifier: Apache-2.0
groups:
  - name: targets
    rules:
      - alert: TargetDown
        expr: up == 0
        for: 2m
        labels: { severity: critical }
        annotations: { summary: "Scrape target {{ $labels.job }}/{{ $labels.instance }} is down" }
```

- [ ] **Step 2: Create the new fixture**

`ops/observability/prometheus/rules/tests/targets.test.yml`:

```yaml
# SPDX-License-Identifier: Apache-2.0
# rule_files globs the whole rules dir on purpose (unlike iam/gateway.test.yml, which each load
# only their own file): TargetDown is shared, so asserting EXACTLY two alerts below also proves no
# other rules file defines a duplicate TargetDown that would double-fire (SMA-466 D5).
rule_files: ['../*.rules.yml']
evaluation_interval: 1m
tests:
  # TargetDown: up == 0, for: 2m — service-agnostic, so both real jobs are exercised. The third
  # series stays UP on purpose: without it both inputs sit at 0 and a broken expr (`up >= 0`,
  # `up != 1`) would still produce exactly the two expected alerts and pass.
  - interval: 1m
    input_series:
      - series: 'up{job="iam", instance="host.docker.internal:8080"}'
        values: '0+0x4'
      - series: 'up{job="gateway", instance="host.docker.internal:8088"}'
        values: '0+0x4'
      - series: 'up{job="iam", instance="host.docker.internal:9999"}'
        values: '1+0x4'
    alert_rule_test:
      - eval_time: 1m
        alertname: TargetDown
        exp_alerts: []
      - eval_time: 2m
        alertname: TargetDown
        exp_alerts:
          - exp_labels: { severity: critical, job: iam, instance: "host.docker.internal:8080" }
            exp_annotations: { summary: "Scrape target iam/host.docker.internal:8080 is down" }
          - exp_labels: { severity: critical, job: gateway, instance: "host.docker.internal:8088" }
            exp_annotations: { summary: "Scrape target gateway/host.docker.internal:8088 is down" }
```

- [ ] **Step 3: Run the fixture and verify it FAILS on the duplicate**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
promtool test rules ops/observability/prometheus/rules/tests/targets.test.yml
```

Expected: **FAILED**, listing four `TargetDown` alerts at `time: 2m` where two were expected — two from `targets.rules.yml` and two from the copy still in `gateway.rules.yml`. If this PASSES, the `rule_files` glob is not resolving; stop and fix that before continuing.

- [ ] **Step 4: Delete the `targets` group from `gateway.rules.yml`**

Remove lines 20-26 entirely — the `- name: targets` group and its single rule. The file must end after the `GatewayUpstreamErrors` annotations line, leaving exactly one group (`gateway`) with three alerts.

- [ ] **Step 5: Run the fixture and verify it PASSES**

```bash
promtool test rules ops/observability/prometheus/rules/tests/targets.test.yml
```

Expected: `SUCCESS`

- [ ] **Step 6: Verify the discrimination assertion actually bites**

Temporarily change `expr: up == 0` to `expr: up >= 0` in `targets.rules.yml`, re-run Step 5's command, and confirm it now reports **FAILED**. Revert to `up == 0` and confirm `SUCCESS` again. A fixture that cannot fail is not a test.

- [ ] **Step 7: Delete the `TargetDown` case from `gateway.test.yml`**

Remove lines 55-68 — the trailing comment block and its test entry. Three test entries remain (`GatewayHighErrorRate`, `GatewayIamDependencyUnavailable`, `GatewayUpstreamErrors`).

- [ ] **Step 8: Run the whole promtool gate**

```bash
moon run repo:promtool
```

Expected: all three sub-commands succeed. Confirm the `check rules` and `test rules` output names `targets.rules.yml` and `targets.test.yml` — that proves the existing globs picked the new files up with no config change.

- [ ] **Step 9: Commit**

```bash
git add ops/observability/prometheus/rules/
git commit -m "refactor(repo): move TargetDown to a shared targets.rules.yml

TargetDown (up == 0) applies to every scrape target but lived in a second group
inside gateway.rules.yml, so deleting the gateway rules would have silently
dropped IAM's coverage.

The fixture gains two things beyond the move: a third, healthy target series, so
a broken expr (up >= 0) can no longer pass with both inputs at zero; and a
globbing rule_files, which makes the exactly-two-alerts assertion double as a
cross-file duplicate-TargetDown guard.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Make the drift test glob both ops directories, fail-closed

**Files:**
- Modify: `rs/crates/libs/paigasus-observability/tests/drift.rs:91` (hardcoded dashboards array) and `:105` (hardcoded rules array)

**Interfaces:**
- Consumes: `ops/observability/prometheus/rules/targets.rules.yml` from Task 1 (the glob must find three rules files, not two).
- Produces: a private test helper `fn files_in(root: &str, dir_rel: &str, suffix: &str) -> Vec<(String, String)>` returning `(repo_relative, absolute)` pairs. Test-local only; nothing outside `drift.rs` consumes it.

**Key trap:** `Path::ends_with` matches whole path *components*, not string suffixes — `Path::new("/a/b/iam.rules.yml").ends_with(".rules.yml")` is **`false`**. `Path::extension()` is `"yml"`, not `"rules.yml"`. The filter must run on the file name as a `&str`.

- [ ] **Step 1: Plant two probe files that the current test cannot see**

```bash
cat > ops/observability/prometheus/rules/zz-probe.rules.yml <<'EOF'
# SPDX-License-Identifier: Apache-2.0
groups:
  - name: zz-probe
    rules:
      - alert: ZzProbe
        expr: iam_bogus_metric_total > 0
        labels: { severity: warning }
        annotations: { summary: "probe" }
EOF
cat > ops/observability/grafana/dashboards/zz-probe.json <<'EOF'
{"panels":[{"targets":[{"expr":"rate(gateway_bogus_metric_total[5m])"}]}]}
EOF
```

Neither `iam_bogus_metric_total` nor `gateway_bogus_metric_total` is in `paigasus_observability::names::ALL`, so a working guard must reject both.

- [ ] **Step 2: Run the drift test and verify it wrongly PASSES**

```bash
cd rs && cargo nextest run --no-tests=pass -p paigasus-observability --test drift; cd ..
```

Expected: **PASS**. This is the bug — the hardcoded arrays at `:91` and `:105` never look at the probe files. Do not proceed until you have seen this pass.

- [ ] **Step 3: Add the glob helper**

Insert into `rs/crates/libs/paigasus-observability/tests/drift.rs`, after the existing `metric_idents` function:

```rust
/// Every entry in `dir_rel` whose FILE NAME ends with `suffix`, as `(repo_relative, absolute)`
/// pairs sorted for a deterministic panic order. Reads use the absolute path; failure messages use
/// the repo-relative one, so output stays clean (`root` is an unnormalized `../../../..` chain).
///
/// Globbing rather than hardcoding is deliberate: `prometheus.yml`, the `repo:promtool` gate, the
/// compose bind-mounts and Grafana's dashboard provisioner all glob these directories, so a
/// hardcoded list here let a new rules file or dashboard ship unchecked (SMA-466).
///
/// Fail-closed by construction: an unreadable directory or entry panics naming the path rather
/// than being swallowed into an empty list, which would make this whole test pass vacuously.
/// Callers additionally assert a known-good sentinel file is present.
fn files_in(root: &str, dir_rel: &str, suffix: &str) -> Vec<(String, String)> {
    let dir_abs = format!("{root}/{dir_rel}");
    let mut out: Vec<(String, String)> = std::fs::read_dir(&dir_abs)
        .unwrap_or_else(|e| panic!("read_dir {dir_rel}: {e}"))
        .map(|entry| entry.unwrap_or_else(|e| panic!("read_dir entry in {dir_rel}: {e}")))
        .filter_map(|entry| {
            // NB: `Path::ends_with` matches whole path COMPONENTS, so `path.ends_with(".rules.yml")`
            // compiles and is false for every file. Match the file name as a &str instead.
            let name = entry.file_name().to_str()?.to_owned();
            name.ends_with(suffix).then(|| (format!("{dir_rel}/{name}"), format!("{dir_abs}/{name}")))
        })
        .collect();
    out.sort();
    out
}
```

- [ ] **Step 4: Replace the two hardcoded arrays**

Replace the `let dashboards = [...]` line (`:91`) and its `for path in dashboards` loop header, and the `let rule_files = [...]` line (`:105`) and its loop header, so both iterate the glob. The body of each loop keeps using `path` for messages and switches to the absolute path for reading:

```rust
    let dashboards = files_in(root, "ops/observability/grafana/dashboards", ".json");
    assert!(
        dashboards.iter().any(|(rel, _)| rel.ends_with("/iam.json")),
        "dashboard glob found {} file(s) but not the known-good iam.json — wrong directory?\n{dashboards:#?}",
        dashboards.len()
    );
    for (path, full) in &dashboards {
        let text = std::fs::read_to_string(full).unwrap_or_else(|e| panic!("read {full}: {e}"));
        let json: serde_json::Value = serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse {full}: {e}"));
        for expr in collect_exprs_from_dashboard(&json) {
            for id in metric_idents(&expr) {
                if !is_known(&id) {
                    unknown.insert(format!("{path}: {id}"));
                }
            }
        }
    }

    let rule_files = files_in(root, "ops/observability/prometheus/rules", ".rules.yml");
    assert!(
        rule_files.iter().any(|(rel, _)| rel.ends_with("/iam.rules.yml")),
        "rules glob found {} file(s) but not the known-good iam.rules.yml — wrong directory?\n{rule_files:#?}",
        rule_files.len()
    );
    for (path, full) in &rule_files {
        let text = std::fs::read_to_string(full).unwrap_or_else(|e| panic!("read {full}: {e}"));
        let doc: serde_norway::Value = serde_norway::from_str(&text).unwrap_or_else(|e| panic!("parse {full}: {e}"));
        for expr in collect_exprs_from_rules(&doc) {
            for id in metric_idents(&expr) {
                if !is_known(&id) {
                    unknown.insert(format!("{path}: {id}"));
                }
            }
        }
    }
```

Also update the module doc comment at `drift.rs:2-5` to say the file list is globbed, not enumerated.

- [ ] **Step 5: Run the drift test and verify it now FAILS on both probes**

```bash
cd rs && cargo nextest run --no-tests=pass -p paigasus-observability --test drift; cd ..
```

Expected: **FAIL**, with both `…/zz-probe.rules.yml: iam_bogus_metric_total` and `…/zz-probe.json: gateway_bogus_metric_total` in the message, each prefixed by a clean repo-relative path (no `../../../..`). If only one appears, only one array was converted.

- [ ] **Step 6: Verify the glob is fail-closed**

Temporarily change the rules directory string to `ops/observability/prometheus/rulez`, re-run, and confirm the test **fails** with a `read_dir ops/observability/prometheus/rulez` panic rather than passing. Then change it to `ops/observability/prometheus` (a real directory that contains no `.rules.yml`) and confirm it fails on the `iam.rules.yml` sentinel assertion. Restore the correct path.

- [ ] **Step 7: Delete the probes and verify green**

```bash
rm ops/observability/prometheus/rules/zz-probe.rules.yml ops/observability/grafana/dashboards/zz-probe.json
cd rs && cargo nextest run --no-tests=pass -p paigasus-observability --test drift; cd ..
git status --short   # must show no stray zz-probe entries
```

Expected: **PASS**, and a clean status. If `git status` shows a deleted `zz-probe` path, run `git reset HEAD <path>` — a prior `git add -N` can leave an index entry.

- [ ] **Step 8: Lint and format**

```bash
moon run paigasus-observability-rs:lint paigasus-observability-rs:fmt
```

Expected: both pass. Clippy runs with `-D warnings`.

- [ ] **Step 9: Commit**

```bash
git add rs/crates/libs/paigasus-observability/tests/drift.rs
git commit -m "test(rs): glob the ops dirs in the observability drift guard

Both file lists in drift.rs were hardcoded while every runtime consumer globs:
prometheus.yml, the repo:promtool gate, the compose bind-mounts and Grafana's
dashboard provisioner. A new rules file or dashboard was therefore live but
invisible to the name-drift check.

The glob is fail-closed on purpose — an unreadable dir or entry panics with the
path instead of collapsing to an empty list, and each call asserts a known-good
sentinel file is present, so a mistyped directory that still finds something
cannot pass. Note the filter matches on the file NAME: Path::ends_with compares
whole path components and is false for every file here.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Make the drift guard run on ops-only changes

**Files:**
- Modify: `moon.yml` (add an `observability-drift` task after `parity-corpus-drift`, which ends at line 105)
- Modify: `.github/workflows/ci.yml:184` (the `T=(...)` target array)

**Interfaces:**
- Consumes: the globbing `drift.rs` from Task 2 — without it this task would run a test that still reads a hardcoded list.
- Produces: the Moon target `repo:observability-drift`, referenced by `ci.yml` and by Task 4's documentation.

**Why this task exists:** `.moon/tasks/rust.yml:22-24` gives `test` project-relative inputs (`src/**/*`, `tests/**/*`, `Cargo.toml`), and `ops/` has no `moon.yml` so it belongs to the root `repo` project. An ops-only change therefore never makes `paigasus-observability-rs` affected. Adding the ops paths to the *crate's* inputs would not help — the repo documents that cross-project inputs are task-hash inputs only and confer no affectedness (`py/packages/paigasus-kernel/moon.yml:53-55`).

- [ ] **Step 1: Prove the gap still exists**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
printf '# SPDX-License-Identifier: Apache-2.0\ngroups: []\n' > ops/observability/prometheus/rules/zz-probe.rules.yml
git add -N ops/observability/prometheus/rules/zz-probe.rules.yml
moon query tasks --affected | grep -o '"target": "[^"]*"' | sort -u
```

Expected: `repo:promtool` appears; **`paigasus-observability-rs:test` does not**. That is the bug this task closes. Leave the probe in place for Step 3.

- [ ] **Step 2: Add the Moon task**

Append to `moon.yml`, after the `parity-corpus-drift` task's `inputs` block (which ends at line 105) and before `wasm-getrandom-free`:

```yaml
  observability-drift:
    description: 'Assert the committed Grafana dashboards + Prometheus rules reference only registered metric families (SMA-466).'
    # Duplicates paigasus-observability-rs:test ON PURPOSE. `ops/` has no moon.yml, so it belongs to
    # the root `repo` project, while the crate's `test` inputs are project-relative — an ops-only
    # change never makes the crate affected, so without this task the drift guard would not run on
    # exactly the PRs that introduce drift. Same shape as parity-corpus-drift above.
    script: '( cd rs && cargo nextest run --no-tests=pass -p paigasus-observability --test drift )'
    toolchain: 'system'
    # Narrow inputs — `repo` owns the whole tree, so without these the guard would run on every change.
    inputs:
      - 'ops/observability/prometheus/rules/**/*'
      - 'ops/observability/grafana/dashboards/**/*'
      - 'rs/crates/libs/paigasus-observability/**/*'
```

- [ ] **Step 3: Verify the task is now selected by an ops-only change**

```bash
moon query tasks --affected | grep -o '"target": "[^"]*"' | sort -u
```

Expected: **both** `repo:promtool` and `repo:observability-drift` now appear. This is the only evidence that this task works — do not skip it.

- [ ] **Step 4: Remove the probe**

```bash
rm ops/observability/prometheus/rules/zz-probe.rules.yml
git reset -q HEAD ops/observability/prometheus/rules/zz-probe.rules.yml
git status --short   # must not mention zz-probe
```

- [ ] **Step 5: Run the new task standalone**

```bash
moon run repo:observability-drift
```

Expected: passes. If Moon or nextest rejects `--test drift`, fall back to `-E 'binary(drift)'` in the script and re-run.

- [ ] **Step 6: Add the target to CI**

In `.github/workflows/ci.yml:184`, insert `:observability-drift` into the `T=(...)` array immediately after `:promtool`:

```bash
          T=(:build :test :lint :fmt :deny :machete :typecheck :breaking :affected-smoke :parity-corpus-drift :wasm-getrandom-free :promtool :observability-drift :release-parity :release-parity-py :release-parity-ts)
```

- [ ] **Step 7: Commit**

```bash
git add moon.yml .github/workflows/ci.yml
git commit -m "ci(repo): run the observability drift guard on ops-only changes

moon query tasks --affected on a probe rules file selects only repo:promtool —
paigasus-observability-rs:test is never selected, because ops/ has no moon.yml
and so belongs to the root repo project while the crate's test inputs are
project-relative. The drift guard therefore did not run on the very PRs that
would introduce drift.

Adds a repo-scoped task with narrow inputs, following the parity-corpus-drift
precedent. Adding the ops paths to the crate's own inputs would not work:
cross-project inputs are task-hash inputs only and confer no affectedness.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: Documentation

**Files:**
- Modify: `ops/observability/README.md:34-35` (Layout section — record the naming convention)
- Modify: `docs/ops/RUNBOOK-observability.md:181-183` (glob-shaped rule-file description)
- Modify: `docs/ops/RUNBOOK-observability.md:193` (stale alert-table row — D6.1)
- Modify: `docs/ops/RUNBOOK-observability.md:371` (stale prose expr — D6.2)
- Modify: `docs/ops/RUNBOOK-observability.md:380-385` and the two follow-on references at ~`:387` and ~`:394` (inverted retention-disabled semantics — D6.3)
- Modify: `CLAUDE.md` (the full-graph gate command, which is missing both `:promtool` and the new target)

**Interfaces:**
- Consumes: the `targets.rules.yml` filename from Task 1 and the `repo:observability-drift` target name from Task 3. Both must already exist so the docs describe reality.
- Produces: nothing consumed by other tasks.

- [ ] **Step 1: Record the naming convention in the ops README**

In `ops/observability/README.md`, replace the `prometheus/rules/` bullet (lines 34-35) with:

```markdown
- `prometheus/rules/` — alerting/recording rules (`*.rules.yml`), with
  `promtool`-driven unit tests under `prometheus/rules/tests/`. **One file per
  service** (`iam`, `gateway`); an alert that applies across services gets its own
  scope-named file (`targets.rules.yml` for scrape-target health) rather than
  living in one service's file or in a `common`/`misc` catch-all. Every file is
  picked up by a glob — `prometheus.yml`, the `repo:promtool` gate and the
  compose bind-mount all glob this directory — so adding one needs no config change.
```

- [ ] **Step 2: Make the RUNBOOK rule-file reference glob-shaped**

Replace `docs/ops/RUNBOOK-observability.md:181-183` (which enumerates `{iam,gateway}` and would go stale again) with:

```markdown
Alert rules live in `ops/observability/prometheus/rules/*.rules.yml` — one file per service plus
`targets.rules.yml` for cross-service scrape-target health — and each is unit tested against
synthetic series via `promtool test rules` using the paired `rules/tests/*.test.yml` as part of
CI. Thresholds
```

Keep the rest of the sentence (`below are **starting points** …`) unchanged.

- [ ] **Step 3: Fix the stale alert-table row (D6.1)**

`docs/ops/RUNBOOK-observability.md:193` currently reads:

```markdown
| `IamAuditPartitionMaintenanceStalled` | `rate(iam_audit_partition_maintenance_ticks_total[1h]) == 0` for 2h | warning |
```

The rule (`iam.rules.yml:26-28`) is a `sum without (result) (increase(...[2d]))` with `for: 1h`. Replace with:

```markdown
| `IamAuditPartitionMaintenanceStalled` | `sum without (result) (increase(iam_audit_partition_maintenance_ticks_total[2d])) == 0` for 1h | warning |
```

- [ ] **Step 4: Fix the stale prose expr (D6.2)**

At `docs/ops/RUNBOOK-observability.md:371`, replace `` `rate(iam_audit_partition_maintenance_ticks_total[1h]) == 0` for 2 hours `` with `` `sum without (result) (increase(iam_audit_partition_maintenance_ticks_total[2d])) == 0` for 1 hour `` and adjust the following clause so it reads "no successful **or failed** tick in ~2 days" (the `sum without (result)` covers both label values).

- [ ] **Step 5: Correct the inverted retention-disabled claim (D6.3)**

This is the operationally dangerous one. `main.rs:233` gates the `PgPartitionMaintainer` spawn on `config.audit.retention.enabled`, so with retention off `counter!` is never called, the series never exists, `increase()` over an absent series yields empty, and the alert **cannot fire** — matching the rule's own annotation. (`describe_counter!` at `main.rs:366` attaches HELP/TYPE metadata only; it does not materialize a series.)

Replace the `**NOTE — ... fires this alert forever, by design.**` paragraph (lines 380-385) with:

```markdown
**NOTE — `audit.retention.enabled = false` makes this alert go SILENT, not fire.** When
`[audit.retention].enabled = false`, IAM does not spawn the maintenance task at all
(`main.rs`), so `iam_audit_partition_maintenance_ticks_total` is never incremented and the
series does not exist. `increase()` over an absent series returns empty, so the alert has
nothing to evaluate and stays silent for as long as the config stays that way. **Disabling
retention is therefore unalerted** — nothing will tell you that create-ahead and pruning have
stopped. Track `iam_audit_default_partition_rows` instead: it is the indirect signal that
create-ahead has stopped keeping up. If you rely on retention being on, assert
`[audit.retention].enabled` at deploy time rather than expecting this alert to catch it.
```

Then fix the two follow-on references that inherit the wrong premise:
- In **Likely causes**, delete `` `audit.retention.enabled = false` (check this first); `` — a disabled config cannot produce this alert.
- In **Confirm**, replace item 1 (`Check [audit.retention].enabled in the running config first — false fully explains this alert and is not an incident.`) with: `Confirm ` `` `[audit.retention].enabled` `` ` is true — if it is false this alert cannot be firing at all, so you are looking at the wrong alert.`

- [ ] **Step 6: Update the CLAUDE.md gate command**

`CLAUDE.md:64-66` documents the full-graph command but already lags `.github/workflows/ci.yml:184` — it is missing `:promtool` and all three `:release-parity*` targets. Replace those three lines:

```markdown
  crates/deps/proto, run the full graph like CI does: `moon ci :build :test :lint :fmt :deny
  :machete :typecheck :breaking :affected-smoke :parity-corpus-drift :wasm-getrandom-free
  --base origin/main --include-relations`.
```

with the array from `ci.yml:184` verbatim, plus the new target:

```markdown
  crates/deps/proto, run the full graph like CI does: `moon ci :build :test :lint :fmt :deny
  :machete :typecheck :breaking :affected-smoke :parity-corpus-drift :wasm-getrandom-free
  :promtool :observability-drift :release-parity :release-parity-py :release-parity-ts
  --base origin/main --include-relations`.
```

- [ ] **Step 7: Verify no doc claim is now stale**

```bash
grep -n "iam,gateway" docs/ops/RUNBOOK-observability.md ops/observability/README.md
grep -rn "rate(iam_audit_partition_maintenance" docs/
grep -n "fires this alert forever" docs/ops/RUNBOOK-observability.md
```

Expected: **no output** from any of the three. Each would indicate an edit was missed.

- [ ] **Step 8: Commit**

```bash
git add ops/observability/README.md docs/ops/RUNBOOK-observability.md CLAUDE.md
git commit -m "docs(repo): describe the shared rules file and fix stale partition-alert docs

Records the one-file-per-service naming convention in the ops README, and makes
the RUNBOOK's rule-file reference glob-shaped so it cannot go stale again.

Also corrects three defects in the IamAuditPartitionMaintenanceStalled docs
(pre-existing drift from the audit-partitioning work): the alert table and the
prose both quoted a rate(...[1h]) expr with a 2h window where the rule is
sum without (result) (increase(...[2d])) with 1h, and the retention-disabled
note had the semantics inverted. It claimed the alert fires forever when
retention is off; in fact the maintainer task is never spawned, so the series
never exists and the alert stays silent. Disabling retention is unalerted, and
the RUNBOOK now says so and points at the default-partition-rows gauge.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: Full-gate verification

**Files:** none modified — this task only runs gates and fixes whatever they surface.

**Interfaces:**
- Consumes: everything from Tasks 1-4.
- Produces: a branch known to be green under the same command CI runs.

- [ ] **Step 1: Run the full affected graph exactly as CI does**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon ci :build :test :lint :fmt :deny :machete :typecheck :breaking :affected-smoke \
  :parity-corpus-drift :wasm-getrandom-free :promtool :observability-drift \
  :release-parity :release-parity-py :release-parity-ts \
  --base origin/main --include-relations
```

Expected: all actions pass.

- [ ] **Step 2: If Moon reports an unattributed failure, find it**

Moon's summary can report "N failed" without naming the task. Resolve it with:

```bash
jq '.actions[] | select(.status=="failed") | {label, status}' .moon/cache/ciReport.json
```

- [ ] **Step 3: Optional runtime wiring check**

`promtool check config` reports SUCCESS while reading **zero** rule files — `prometheus.yml:5` is the container-absolute `/etc/prometheus/rules/*.rules.yml`, which matches nothing on the host, and promtool tolerates a zero-match glob. So the only real proof that Prometheus loads the new file is to run it. If Docker is available:

```bash
cd ops/observability && docker compose up -d prometheus
sleep 5
curl -s localhost:9090/api/v1/rules | jq -r '.data.groups[].file' | sort -u
docker compose down
```

Expected: three distinct files, including `/etc/prometheus/rules/targets.rules.yml`. Note the compose service runs without `--web.enable-lifecycle`, so a rules change needs `docker compose restart prometheus` rather than `POST /-/reload`. Skip this step if Docker is unavailable and say so in the PR body rather than claiming it passed.

- [ ] **Step 4: Confirm the working tree is clean**

```bash
git status --short
```

Expected: no modified or untracked files under `ops/`, `rs/`, `docs/`, `moon.yml`, or `.github/` — in particular no leftover `zz-probe` artifacts from Tasks 2 and 3.
