# SMA-466 — Move `TargetDown` to a shared Prometheus rules file

**Status:** approved (design) — **rev 2**, revised after adversarial challenge
**Date:** 2026-08-01
**Linear:** [SMA-466](https://linear.app/smaschek/issue/SMA-466/observability-move-targetdown-alert-to-a-shared-prometheus-rules-file)
**Follows up:** SMA-446 #3 (observability, PR #89)

## Problem

`TargetDown` (`up == 0`, `for: 2m`, critical) applies to **every** scrape target — the `iam`
job and the `gateway` job alike — but it lives in a second rule group named `targets` inside
`ops/observability/prometheus/rules/gateway.rules.yml` (lines 20–26).

It works today only incidentally: Prometheus loads `rules/*.rules.yml`, so the file it happens
to sit in does not matter at runtime. The placement is misleading to a reader, and the latent
risk named in the issue is that **deleting or replacing the gateway rules would silently drop
IAM's `TargetDown`** — the only critical alert that fires before any request-level metric
exists.

The test fixture already concedes the problem in a comment
(`rules/tests/gateway.test.yml:55`): *"shared alert, exercised here with a gateway-shaped
target."*

### The same bug class, twice more, found while exploring

**(a) `drift.rs` hardcodes what everyone else globs.**
`rs/crates/libs/paigasus-observability/tests/drift.rs` — the metric-name-drift guard —
enumerates its inputs with **two** hardcoded arrays: rules files at line 105 and Grafana
dashboards at line 91.

```rust
let dashboards  = [".../grafana/dashboards/iam.json", ".../grafana/dashboards/gateway.json"];   // :91
let rule_files  = [".../rules/iam.rules.yml",         ".../rules/gateway.rules.yml"];           // :105
```

Every runtime consumer of both directories globs: `prometheus.yml:5`
(`/etc/prometheus/rules/*.rules.yml`), `moon.yml:135-136` (the `repo:promtool` gate),
`docker-compose.yml:9` and `:19` (bind-mounts both dirs), and Grafana's provisioner
(`grafana/provisioning/dashboards/dashboards.yml` → `path: /var/lib/grafana/dashboards`). So a
new rules file *or a new dashboard* is picked up in production but is invisible to the drift
guard — structurally the same trap this issue exists to remove.

The cost is zero **today**: `up == 0` contains no `iam_`/`gateway_`-prefixed token, so
`metric_idents` extracts nothing from it either way. The fix is about the next rules file, not
this one.

**(b) The drift guard does not run on the PRs that would introduce the drift.** — *verified
empirically, see D4.* Globbing the file list is worthless if the test itself is never selected.

## Goal

`TargetDown` lives in a file whose name says it belongs to no single service; its promtool
fixture proves it is service-agnostic *and* discriminating; and the drift guard covers both ops
directories **and actually executes** when those directories change.

## Decisions

### D1 — Filename: `targets.rules.yml`, not `common.rules.yml`

The Linear description originally specified `common.rules.yml`. **Deviating deliberately:** the
file is named after what it contains, matching the existing group name `targets`. `common.*` is
an open invitation to become a dumping ground; `targets.*` is self-describing and resists scope
creep. If a genuinely shared non-target alert ever appears it gets its own named file — the
same discipline this issue is enforcing.

Two sentences go into `ops/observability/README.md`'s Layout section recording the convention
("one file per service; cross-cutting alerts get their own scope-named file, never a
`common`/`misc` catch-all"). Without that, the convention lives only in a dated spec and the
next author creates `common.rules.yml` anyway.

The Linear description has been updated to name `targets.rules.yml`.

### D2 — Fixture asserts both jobs *and* discriminates

The moved fixture currently drives a single `up{job="gateway", instance="localhost:8088"}`
series, leaving the shared-ness untested. Two changes, both **empirically verified against
promtool 3.13.1** (the pinned version):

1. **One series per job**, both held at `0`, expecting **two** alerts at `eval_time: 2m`.
   Verified: promtool sorts collected and expected alert sets before comparing, so multiple
   `exp_alerts` under one `alertname` at one `eval_time` is supported and **order-insensitive**
   (the fixture passed with `gateway` listed before `iam`). `alertname` is injected
   automatically, and the alerting rule strips `__name__`, so `up` needs no special handling.
2. **A third, healthy series** (`up{job="iam", instance="host.docker.internal:9999"}` at `1`)
   that must *not* alert. Without it, both series sit at `0` and a broken expr such as
   `up >= 0` or `up != 1` still produces exactly the two expected alerts — the test would be
   green against a rule that alerts on everything. Verified by mutation: with the healthy
   series present, flipping the expr to `up >= 0` makes promtool **FAIL**; without it, it
   passes.

Instance addresses use `host.docker.internal:8080` / `:8088` to match the real scrape targets
at `prometheus.yml:9,12`.

### D3 — `drift.rs` globs both ops directories, fail-closed

Replace both hardcoded arrays with directory reads via one shared helper. The helper returns
`(repo_relative, absolute)` pairs: reads use the absolute path, **failure messages keep the
clean repo-relative path** that `drift.rs:99,112` prints today (`root` is the unnormalized
`concat!(env!("CARGO_MANIFEST_DIR"), "/../../../..")`, so echoing `entry.path()` would emit
machine-specific `../../../..` noise).

**The glob must not be able to fail open.** A path typo would make the test read zero files and
pass vacuously — strictly worse than the hardcoded list it replaces. Four guards:

- **Match on the file name, not the `Path`.** `Path::ends_with` compares whole path
  *components*, so the obvious `entry.path().ends_with(".rules.yml")` **compiles and returns
  `false` for every file** (verified: it evaluates to `false` for `/a/b/iam.rules.yml`;
  `Path::extension()` is likewise `"yml"`, not `"rules.yml"`). Use
  `entry.file_name().to_str().is_some_and(|n| n.ends_with(".rules.yml"))`.
- `read_dir` **and** each per-entry `io::Result<DirEntry>` panic with the offending path rather
  than being swallowed by `filter_map`.
- Assert the collected set is non-empty **and** contains a known-good sentinel
  (`iam.rules.yml`, `iam.json`), so a directory typo that happens to find *something* still
  fails.
- Sort for determinism — of *which file panics first* on a parse error. (Note: the failure
  output is already sorted, since `drift.rs:89` collects into a `BTreeSet`.)

### D4 — Make the drift guard actually run on ops-only changes *(new in rev 2)*

**Verified empirically.** Adding a probe file at
`ops/observability/prometheus/rules/zz-probe.rules.yml` and running
`moon query projects --affected` / `moon query tasks --affected` selects exactly one project
(`repo`) and one task (`repo:promtool`). `paigasus-observability-rs:test` is **not** selected.

The cause: `.moon/tasks/rust.yml:22-24` gives `test` the inputs `@group(sources)` (`src/**/*`),
`@group(tests)` (`tests/**/*`) and `Cargo.toml` — all *project-relative* to
`rs/crates/libs/paigasus-observability/`. `ops/` has no `moon.yml`, so it belongs to the root
`repo` project only (`ci/affected-graph/run.sh:26` — "root `.`, so it owns every file").

So D3 alone fixes *which files the test reads* but not *whether the test runs*. A future
ops-only PR adding a rules file or a dashboard would sail past the drift guard — the exact
failure mode this spec is written to eliminate, and a **stronger** false sense of coverage than
the hardcoded list it replaces (that list at least lived in the crate's own files, so updating
it re-keyed the test).

**Adding the ops paths to the crate's own `test` inputs does not fix this.** The repo already
documents that cross-project inputs are task-hash inputs only and confer no affectedness —
`py/packages/paigasus-kernel/moon.yml:53-55` and `ts/packages/paigasus-kernel/moon.yml:70-71`
both say so, and `ci/affected-graph/run.sh` asserts it as `parity-oneway`.

**Fix — follow the `repo:parity-corpus-drift` precedent** (`moon.yml:89-105`): a `repo`-scoped
task with deliberately narrow inputs (because `repo` owns the whole tree, unnarrowed inputs
would run it on every change).

```yaml
observability-drift:
  description: 'Assert the committed Grafana dashboards + Prometheus rules reference only registered
    metric families. Duplicates paigasus-observability-rs:test on purpose — `ops/` belongs to the
    root `repo` project, so an ops-only change does not make the crate affected (SMA-466).'
  script: '( cd rs && cargo nextest run --no-tests=pass -p paigasus-observability --test drift )'
  toolchain: 'system'
  inputs:
    - 'ops/observability/prometheus/rules/**/*'
    - 'ops/observability/grafana/dashboards/**/*'
    - 'rs/crates/libs/paigasus-observability/**/*'
```

`:observability-drift` joins the CI target array at `.github/workflows/ci.yml:184`.

Running the test twice on a Rust-side change is accepted, matching `parity-corpus-drift`. The
`cd rs` mirrors that task's comment about `rs/.cargo/config.toml` scope; `--no-tests=pass` is
the standing nextest gotcha from CLAUDE.md.

### D5 — The fixture doubles as a cross-file duplicate guard *(new in rev 2)*

"Considered and rejected" below notes that duplicating `TargetDown` into `iam.rules.yml` would
double-fire — but nothing detects it: `promtool check rules` only reports duplicates *within* a
file, and each fixture loads exactly one rules file.

promtool resolves **and globs** `rule_files` relative to the test file, so
`rule_files: ['../*.rules.yml']` in `targets.test.yml` loads all three rules files while
`alert_rule_test` still filters by `alertname`. The `eval_time: 2m` assertion of *exactly* two
`TargetDown` alerts then becomes a free cross-file duplicate guard.

**Verified:** the glob form passes as-is, and planting a duplicate `TargetDown` in a second
rules file makes promtool **FAIL**. `iam.test.yml` and `gateway.test.yml` keep their
single-file `rule_files` — only the shared fixture needs the whole-directory view, and a
comment in `targets.test.yml` records why the asymmetry is deliberate.

### D6 — Correct the stale `IamAuditPartitionMaintenanceStalled` docs *(new in rev 2, approved at the gate)*

Adjacent to the D-inventory RUNBOOK edit, and approved for inclusion despite being unrelated to
`TargetDown` (pre-existing SMA-467 drift). Three defects in one alert's documentation, all
verified against the code:

1. **`RUNBOOK:193`** (alert table) — documents
   `rate(iam_audit_partition_maintenance_ticks_total[1h]) == 0` for 2h. The rule
   (`iam.rules.yml:26-28`) is `sum without (result) (increase(...[2d])) == 0` for **1h**.
2. **`RUNBOOK:371`** (prose section) — repeats the same stale expr and "for 2 hours".
3. **`RUNBOOK:380-382`** — states that `audit.retention.enabled = false` "fires this alert
   forever, by design … this alert fires and stays firing". **This is inverted.** The rule's own
   annotation says the opposite ("the series is absent and this alert stays silent — expected"),
   and the annotation is right: `main.rs:233` gates the `PgPartitionMaintainer` spawn on
   `config.audit.retention.enabled`, so with retention off `counter!` is never called and the
   series never exists. `increase()` over an absent series yields empty, so the alert cannot
   fire. (`describe_counter!` at `main.rs:366` attaches HELP/TYPE metadata only — it does not
   materialize a series.)

Defect 3 is the operationally dangerous one: it promises an operator a signal that can never
arrive. Disabling retention is in fact **unalerted** — the RUNBOOK should say so plainly. There is
no metric-based fallback either: `iam_audit_default_partition_rows` is set from the same
retention-gated task (`pg_partition_maintainer.rs:87`), so it is equally absent when retention is
disabled — the only signal is the one-time startup log line at `main.rs:264`.

## Change inventory

| File | Change |
|---|---|
| `ops/observability/prometheus/rules/targets.rules.yml` | **new** — the `targets` group, rule body byte-identical to today's |
| `ops/observability/prometheus/rules/gateway.rules.yml` | delete lines 20–26 (the `targets` group); one `gateway` group with 3 alerts remains |
| `ops/observability/prometheus/rules/tests/targets.test.yml` | **new** — globbing `rule_files` (D5), dual-job + healthy-third-target case (D2) |
| `ops/observability/prometheus/rules/tests/gateway.test.yml` | delete lines 55–68 (the `TargetDown` case); 3 cases remain |
| `rs/crates/libs/paigasus-observability/tests/drift.rs` | lines 91 + 105 — both hardcoded arrays → one guarded directory-glob helper (D3) |
| `moon.yml` | **new** `repo:observability-drift` task (D4) |
| `.github/workflows/ci.yml` | line 184 — add `:observability-drift` to the target array (D4) |
| `ops/observability/README.md` | Layout section — record the rules-file naming convention (D1) |
| `docs/ops/RUNBOOK-observability.md` | lines 181–183 — replace the `{iam,gateway}` enumeration with a glob-shaped description; plus the D6 partition-alert corrections |

### New rule file

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

### New fixture (D2 + D5) — verified green against promtool 3.13.1

```yaml
# SPDX-License-Identifier: Apache-2.0
# rule_files globs the whole rules dir on purpose (unlike iam/gateway.test.yml, which each load
# only their own file): TargetDown is shared, so asserting EXACTLY two alerts below also proves no
# other rules file defines a duplicate TargetDown that would double-fire (SMA-466 D5).
rule_files: ['../*.rules.yml']
evaluation_interval: 1m
tests:
  # TargetDown: up == 0, for: 2m — service-agnostic, so both jobs are exercised. The third series
  # stays UP: without it both inputs sit at 0 and a broken expr (`up >= 0`) would still produce the
  # two expected alerts.
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

Both `evaluation_interval: 1m` (rule cadence) and the per-test `interval: 1m` (input sample
spacing) are kept, matching both existing fixtures. They mean different things and must stay
aligned — promtool **skips** an `eval_time` that does not land on an evaluation step rather
than failing, so dropping one would silently weaken the test.

## What changes, and what does not

**Unchanged:** the alert's identity — name, `expr`, `for`, labels, annotations. No edit is
needed to `prometheus.yml` (globs `rules/*.rules.yml`), `docker-compose.yml` (bind-mounts the
directory), or `moon.yml`'s `promtool` task (globs both `rules/*.rules.yml` and
`rules/tests/*.test.yml`).

**Changed, contrary to what a "pure move" implies:** Prometheus keys rule-group state on
`(file, name)`, and `Manager.Update` only calls `CopyState` when *both* match. Moving the
`targets` group to a new file therefore makes it a **brand-new group on the next reload**: a
pending or firing `TargetDown` loses its `activeAt` and must serve the full `for: 2m` again
(up to 2 minutes of extra detection delay, once). The group's evaluation offset also shifts,
since it derives from `hash(name, file) % interval`.

For the local dev stack this is a non-event. It is recorded because it is exactly the kind of
claim a future reader would rely on when repeating this refactor against a live Prometheus.
Note the compose service runs **without** `--web.enable-lifecycle`, so `POST /-/reload` is
unavailable and a rules change needs `docker compose restart prometheus`.

There is no Alertmanager in play — `prometheus.yml` has no `alerting:` block, `docker-compose.yml`
runs only prometheus + grafana, and `RUNBOOK:748-750` records routing as unimplemented.

## Verification

1. `moon run repo:promtool` — `check config`, `check rules` (glob now matches 3 files),
   `test rules` (glob now matches 3 fixtures). Confirm the output names `targets.rules.yml` and
   `targets.test.yml`.
2. `moon run repo:observability-drift` **and** `cargo nextest run -p paigasus-observability`.
3. **Fail-closed check on D3.** Temporarily point the glob at a nonexistent directory and
   confirm the test **fails**; revert. A guard that cannot fail is not a guard.
4. **Affectedness check on D4.** Re-run the probe: add a throwaway
   `ops/observability/prometheus/rules/zz-probe.rules.yml`, `git add -N` it, and confirm
   `moon query tasks --affected` now lists `repo:observability-drift` alongside
   `repo:promtool`. Delete the probe. This is the only thing that proves D4 worked.
5. **Runtime wiring check** (the one claim `promtool` cannot make — verified that
   `promtool check config` reports SUCCESS while reading **zero** rule files, because
   `prometheus.yml:5`'s container-absolute `/etc/prometheus/rules/*.rules.yml` matches nothing
   on the host and promtool tolerates a zero-match glob):
   `cd ops/observability && docker compose up -d prometheus`, then
   `curl -s localhost:9090/api/v1/rules | jq -r '.data.groups[].file' | sort -u` and assert all
   three files appear.
6. Full gate, matching `.github/workflows/ci.yml:184` verbatim:
   `moon ci :build :test :lint :fmt :deny :machete :typecheck :breaking :affected-smoke
   :parity-corpus-drift :wasm-getrandom-free :promtool :observability-drift :release-parity
   :release-parity-py :release-parity-ts --base origin/main --include-relations`
   (CI additionally runs the codegen-drift and CODEOWNERS steps at `ci.yml:194-212`, outside
   `moon ci`; neither is touched by this diff.)

## Considered and rejected

- **Assert every `*.rules.yml` has a paired `tests/*.test.yml`.** A rules file with no fixture
  is simply untested and `promtool test rules` will not complain. D5 partly covers it — the
  shared fixture now loads every rules file — but pairing is still unasserted. Deferred: a
  third guard in a tidy-up PR.
- **Adding a `TargetDown` copy to `iam.rules.yml`.** Would load the rule twice and double-fire.
  D5 now actively guards against exactly this.
- **Leaving `drift.rs` alone and spinning it out.** Rejected: the fix is small and the gap is
  the identical bug class to the one this issue closes; splitting them would ship a PR that
  fixes the symptom while stepping over its twin.
- ~~**Fixing `RUNBOOK:193`**~~ — flagged at the approval gate and **accepted into scope**; see
  D6, which also covers the two further defects found in the same section.
