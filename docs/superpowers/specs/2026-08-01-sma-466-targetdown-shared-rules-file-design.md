# SMA-466 — Move `TargetDown` to a shared Prometheus rules file

**Status:** approved (design)
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

### Second instance of the same bug class, found while exploring

`rs/crates/libs/paigasus-observability/tests/drift.rs:105` — the metric-name-drift guard —
enumerates the rules files with a **hardcoded array**:

```rust
let rule_files = ["ops/observability/prometheus/rules/iam.rules.yml",
                  "ops/observability/prometheus/rules/gateway.rules.yml"];
```

Every other consumer of that directory globs: `prometheus.yml:5`
(`/etc/prometheus/rules/*.rules.yml`), `moon.yml:135-136` (the `repo:promtool` gate),
`docker-compose.yml:9` (mounts the whole dir). `drift.rs` is the lone hardcoded list, so a new
rules file escapes the drift guard while *looking* covered — structurally the same trap this
issue exists to remove.

The cost is zero **today**: `up == 0` contains no `iam_`/`gateway_`-prefixed token, so
`metric_idents` extracts nothing from it either way. The fix is about the next rules file, not
this one.

## Goal

`TargetDown` lives in a file whose name says it belongs to no single service; its promtool
fixture proves it is service-agnostic; and the drift guard covers the rules directory rather
than a list that must be remembered.

## Decisions

### D1 — Filename: `targets.rules.yml`, not `common.rules.yml`

The Linear description specifies `common.rules.yml`. **Deviating deliberately:** the file is
named after what it contains, matching the existing group name `targets`. `common.*` is an
open invitation to become a dumping ground; `targets.*` is self-describing and resists scope
creep. If a genuinely shared non-target alert ever appears, it gets its own named file — which
is the same discipline this issue is enforcing. The Linear description is updated to match.

### D2 — Fixture asserts both jobs

The moved fixture currently drives a single `up{job="gateway", instance="localhost:8088"}`
series. In a file that is now *explicitly* shared, that leaves the shared-ness untested. The
fixture takes one series per job, both held at `0`, and expects **two** alerts at
`eval_time: 2m`. This converts an incidental single-service test into a real assertion of the
premise the issue rests on.

### D3 — `drift.rs` globs the rules directory

Replace the hardcoded array with a `read_dir` over
`ops/observability/prometheus/rules`, filtered to `*.rules.yml` and sorted for deterministic
failure output.

**The glob must not be able to fail open.** A path typo would make the test read zero files and
pass vacuously — strictly worse than the hardcoded list it replaces. Two guards:

- `read_dir` failure panics with the offending path (not swallowed by `filter_map`).
- Assert the collected set is non-empty before the drift check runs.

## Change inventory

| File | Change |
|---|---|
| `ops/observability/prometheus/rules/targets.rules.yml` | **new** — the `targets` group, rule body byte-identical to today's |
| `ops/observability/prometheus/rules/gateway.rules.yml` | delete lines 20–26 (the `targets` group); one `gateway` group with 3 alerts remains |
| `ops/observability/prometheus/rules/tests/targets.test.yml` | **new** — `rule_files: [../targets.rules.yml]`, dual-job case per D2 |
| `ops/observability/prometheus/rules/tests/gateway.test.yml` | delete lines 55–68 (the `TargetDown` case); 3 cases remain |
| `rs/crates/libs/paigasus-observability/tests/drift.rs` | line 105 — hardcoded array → guarded directory glob (D3) |
| `docs/ops/RUNBOOK-observability.md` | lines 181–183 — `{iam,gateway}.rules.yml` / `{iam,gateway}.test.yml` become three-way |

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

### New fixture (D2)

```yaml
# SPDX-License-Identifier: Apache-2.0
rule_files: [../targets.rules.yml]
evaluation_interval: 1m
tests:
  # TargetDown: up == 0, for: 2m — service-agnostic, so both jobs are exercised.
  - interval: 1m
    input_series:
      - series: 'up{job="iam", instance="localhost:8080"}'
        values: '0+0x4'
      - series: 'up{job="gateway", instance="localhost:8088"}'
        values: '0+0x4'
    alert_rule_test:
      - eval_time: 1m
        alertname: TargetDown
        exp_alerts: []
      - eval_time: 2m
        alertname: TargetDown
        exp_alerts:
          - exp_labels: { severity: critical, job: iam, instance: "localhost:8080" }
            exp_annotations: { summary: "Scrape target iam/localhost:8080 is down" }
          - exp_labels: { severity: critical, job: gateway, instance: "localhost:8088" }
            exp_annotations: { summary: "Scrape target gateway/localhost:8088 is down" }
```

The instance addresses match the real scrape targets in `prometheus.yml:7-12`
(`host.docker.internal:8080` / `:8088`).

## Explicitly unchanged

No edit is needed to `prometheus.yml` (globs `rules/*.rules.yml`), `docker-compose.yml`
(bind-mounts the directory), `moon.yml`'s `promtool` task (globs both `rules/*.rules.yml` and
`rules/tests/*.test.yml`), or `ops/observability/README.md` (describes `prometheus/rules/`
generically, names no individual file). Implementation **verifies** each of these picks the new
file up rather than assuming it.

No alert semantics change — same alert name, `expr`, `for`, labels and annotations — so
Alertmanager and any downstream routing observe nothing new. This is a refactor of file
layout, not of alerting behavior.

## Verification

1. `moon run repo:promtool` — `check config`, `check rules` (glob now matches 3 files),
   `test rules` (glob now matches 3 fixtures). Confirm the output names `targets.rules.yml` and
   `targets.test.yml`, proving the globs picked them up rather than silently skipping them.
2. `cargo nextest run -p paigasus-observability` — the drift test with the new glob.
3. Negative check on D3's fail-open risk: temporarily point the glob at a nonexistent directory
   and confirm the test **fails** rather than passing vacuously; revert.
4. Full gate, since `drift.rs` is Rust and pulls in build/clippy/fmt:
   `moon ci :build :test :lint :fmt :deny :machete :typecheck :breaking :affected-smoke
   :parity-corpus-drift :wasm-getrandom-free :promtool --base origin/main --include-relations`

## Considered and rejected

- **Assert every `*.rules.yml` has a paired `tests/*.test.yml`.** A real latent gap — a rules
  file with no fixture is simply untested, and `promtool test rules` will not complain. But it
  is a third concern in a tidy-up PR. Recorded here; spin out if it ever bites.
- **Adding a `TargetDown` copy to `iam.rules.yml`.** Would duplicate the rule and cause it to
  load twice, double-firing. The shared-file move is the correct fix.
- **Leaving `drift.rs` alone and spinning it out.** Rejected because the fix is a few lines and
  the gap is the identical bug class to the one this issue closes; splitting them would ship a
  PR that fixes the symptom while stepping over its twin.
