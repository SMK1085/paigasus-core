# SMA-706 JIT failure alert Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the alert `IamJitProvisioningFailures`, its promtool tests, a Grafana panel and the
runbook text for `iam_jit_provisioning_failures_total{defect}`.

**Architecture:** One new Prometheus rule in the existing `iam` group. Four new promtool blocks in
the existing IAM test file. The `repo:promtool` glob finds them. One new panel in the existing IAM
dashboard. Runbook text in two existing runbooks. No Rust change.

**Tech Stack:** Prometheus rules and promtool 3.13.1 (proto-pinned), Grafana dashboard JSON,
Markdown, Moon tasks `repo:promtool` and `repo:observability-drift`.

**Spec:** `docs/superpowers/specs/2026-09-27-sma-706-jit-failure-alert-design.md` (approved
2026-09-27). Read it before you start a task.

## Global Constraints

- Worktree root: `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-706`. Run every
  command from this directory. Check `git branch --show-current` =
  `feature/sma-706-jit-failure-alert` before each commit.
- Start every Bash command that uses a repo tool with
  `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.
- Commit scope must be one of `rs, py, ts, contracts, ci, docs, deps, release, repo, claude,
  workspace`. Use `feat(repo)` for `ops/` files and `docs(docs)` for runbooks.
- End every commit message with the line
  `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`, after one blank line.
- Do not use `--no-verify`, `--amend`, `git reset`, `git stash`, or `git checkout -- <file>`. Do not
  install host software (`brew install` and similar).
- Do not start background jobs. Run every command in the foreground.
- Rule: `sum by (job, defect) (increase(iam_jit_provisioning_failures_total[15m])) > 0`,
  `for: 0m`, `severity: warning`.
- Dashboard panel: `id: 21`, `gridPos {h: 8, w: 12, x: 12, y: 72}`.
- Write new prose in ASD-STE100 Simplified Technical English: short sentences, active voice, one
  idea per sentence, no idioms.

## Review Focus

1. **Two replicas fail at the same time.** The operator expects one alert per `job` and `defect`,
   not one per replica. Task 1 Block C pins this.
2. **Metrics are off, or the IAM binary is older than SMA-698.** The series does not exist. The
   operator expects no alert, not a permanent one. Task 1 Block D pins this.
3. **The summary template.** A notification that shows only the summary must show the defect.
   Task 1 asserts the expanded summary `IAM refused just-in-time provisioning (email_conflict)`.
4. **The dashboard JSON loads in Grafana.** A syntax error or a duplicate panel `id` breaks the
   whole dashboard. Task 2 checks both, and checks that no two panels overlap.
5. **A test that passes for the wrong reason.** Each assertion must fail against a named mutant.
   Task 1 Step 6 runs the mutation battery on a temporary copy and compares exact failing rows.

---

### Task 1: Alert rule and promtool tests

**Files:**
- Modify: `ops/observability/prometheus/rules/iam.rules.yml` (append after the last rule,
  `IamAuthzGenerationRewound`, which ends at line 311)
- Modify: `ops/observability/prometheus/rules/tests/iam.test.yml` (append after line 848, the
  end of the last block)

**Interfaces:**
- Consumes: the metric `iam_jit_provisioning_failures_total{defect}` from SMA-698. It is in
  `paigasus_observability::names::ALL`.
- Produces: the alert name `IamJitProvisioningFailures`, used by Task 2 (panel description) and
  Task 3 (runbook).

The exact description string. The rule and the test must use it byte for byte. It contains no
double quote:

```text
IAM refused just-in-time provisioning for a new identity (HTTP 403 provisioning-failed, or gRPC PermissionDenied with the reason provisioning-failed). The value counts refused requests, not users. One user with a bad token makes several refused requests. defect=missing_email: the access token has no valid email claim. Make the issuer put a valid email claim into the ACCESS token, not only into the ID token. defect=email_conflict: another IAM user already has this exact email, and IAM does not link identities by email. Follow the runbook. Do not link identities by hand before the identity check in the runbook. The warn log line that starts with 'just-in-time provisioning failed' names the issuer. See RUNBOOK section 4.
```

- [ ] **Step 1: Append the four test blocks**

Append this text to the end of `ops/observability/prometheus/rules/tests/iam.test.yml`. Keep the
two-space indent of the other blocks under `tests:`. Replace `<DESC>` in each of the four places
with the exact description string above.

```yaml

  # IamJitProvisioningFailures (SMA-706): sum by (job, defect) (increase(...[15m])) > 0, for: 0m.
  #
  # Block A, the primed shape. Both series are seeded at 0 from t=0. That is faithful to
  # production ONLY because `main` calls `prime_jit_provisioning_failures` before the listener
  # binds. `missing_email` stays flat and is the control: under the correct rule its group's
  # increase() is 0 and stays silent, so a `>= 0` mutant fires on it. `email_conflict` steps
  # 0 -> 1 once at t=10m: exactly one failure, the shape of the first acceptance criterion.
  # The correct rule fires from 10m to 23m. At 24m the 9m sample leaves the left-open window
  # (t-15m, t], so the rule is quiet. A3 (23m) and A4 (25m) keep one sample of margin, so a
  # left-closed window gives the same result. The rows pin the window to [15m]..[16m].
  - interval: 1m
    input_series:
      - series: 'iam_jit_provisioning_failures_total{job="iam", instance="a:8080", defect="missing_email"}'
        values: '0x40'
      - series: 'iam_jit_provisioning_failures_total{job="iam", instance="a:8080", defect="email_conflict"}'
        values: '0x9 1x30'
    alert_rule_test:
      # A1, baseline: nothing moved yet.
      - eval_time: 5m
        alertname: IamJitProvisioningFailures
        exp_alerts: []
      # A2: the first evaluation that sees the step. It fires at once (kills `for: 5m`). The
      # flat control stays silent (kills `>= 0`). The labels carry `job` and no `instance`
      # (kills a dropped `job` and a dropped aggregation).
      - eval_time: 10m
        alertname: IamJitProvisioningFailures
        exp_alerts:
          - exp_labels: { severity: warning, job: iam, defect: email_conflict }
            exp_annotations: { summary: "IAM refused just-in-time provisioning (email_conflict)", description: "<DESC>" }
      # A3: 13 minutes after the step, still active (kills [14m] and shorter).
      - eval_time: 23m
        alertname: IamJitProvisioningFailures
        exp_alerts:
          - exp_labels: { severity: warning, job: iam, defect: email_conflict }
            exp_annotations: { summary: "IAM refused just-in-time provisioning (email_conflict)", description: "<DESC>" }
      # A4: the step has left the window, so the alert resolved (kills [17m] and longer).
      - eval_time: 25m
        alertname: IamJitProvisioningFailures
        exp_alerts: []

  # Block B, the un-primed shape: the known residual (SMA-706 spec section 7). `_` is promtool's
  # absent-sample marker, and `_x9` gives 9 samples (0m..8m). The series first appears at 9m
  # ALREADY at 1: a failure before the first scrape of a new series. increase() takes that
  # first sample as its baseline, so B1 is silent. A second failure at 15m (1 -> 2) fires (B2).
  # If a later change fixes the residual, B1 must change in the same commit, and so must the
  # runbook section.
  - interval: 1m
    input_series:
      - series: 'iam_jit_provisioning_failures_total{job="iam", instance="a:8080", defect="email_conflict"}'
        values: '_x9 1x5 2x10'
    alert_rule_test:
      - eval_time: 12m
        alertname: IamJitProvisioningFailures
        exp_alerts: []
      - eval_time: 16m
        alertname: IamJitProvisioningFailures
        exp_alerts:
          - exp_labels: { severity: warning, job: iam, defect: email_conflict }
            exp_annotations: { summary: "IAM refused just-in-time provisioning (email_conflict)", description: "<DESC>" }

  # Block C, two replicas of one job. Both refuse an `email_conflict` login, at 10m and at 12m.
  # The operator gets ONE alert for the job and defect, not one per replica.
  - interval: 1m
    input_series:
      - series: 'iam_jit_provisioning_failures_total{job="iam", instance="a:8080", defect="email_conflict"}'
        values: '0x9 1x20'
      - series: 'iam_jit_provisioning_failures_total{job="iam", instance="b:8080", defect="email_conflict"}'
        values: '0x11 1x18'
    alert_rule_test:
      - eval_time: 13m
        alertname: IamJitProvisioningFailures
        exp_alerts:
          - exp_labels: { severity: warning, job: iam, defect: email_conflict }
            exp_annotations: { summary: "IAM refused just-in-time provisioning (email_conflict)", description: "<DESC>" }

  # Block D, no series at all: metrics are off, or the binary is older than SMA-698.
  # `sum by (...)` over an empty vector is EMPTY, not 0, so the alert stays silent. A rule
  # "hardened" with `or vector(0)` must not turn this into a permanent alert.
  - interval: 1m
    input_series:
      - series: 'iam_authz_decisions_total{cache="hit",decision="allow"}'
        values: '0+5x25'
    alert_rule_test:
      - eval_time: 20m
        alertname: IamJitProvisioningFailures
        exp_alerts: []
```

- [ ] **Step 2: Run the tests and see them fail**

Run:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
promtool test rules ops/observability/prometheus/rules/tests/*.test.yml
```

Expected: exit code 1. The output has `FAILED` rows for `IamJitProvisioningFailures` at the times
`10m`, `23m`, `16m` and `13m`, because the rule does not exist yet. All other rules still pass.
If the output shows a YAML parse error, fix the indent and run again.

- [ ] **Step 3: Append the rule**

Append this text to the end of `ops/observability/prometheus/rules/iam.rules.yml`. Keep the
six-space indent of the other rules. Replace `<DESC>` with the exact description string.

```yaml
      # SMA-706. IAM refused just-in-time provisioning for a new identity (SMA-698). The counter
      # counts refused REQUESTS, not users. `main` primes both `defect` series at zero before the
      # listener binds, so increase() can see the first failure after the first scrape.
      #
      # `sum by (job, defect)`, never a bare `sum()`: the grouping puts `defect` on the alert,
      # keeps two deployments apart by `job`, and lets the promtool fixture use a flat series as
      # a control. `[15m]` with `for: 0m`, the shape of IamOutboxEventsParked: one isolated
      # failure fires at the next evaluation and stays active for about 15 minutes. A hold near
      # the window length lets one failure resolve before it fires (see SMA-713).
      #
      # Known residual (SMA-706 spec section 7): a failure before the FIRST scrape of a NEW
      # series is invisible, because the first scraped value is already above zero. Only a rule
      # branch could close that gap, and it adds a false positive after a Prometheus data gap, so
      # the rule does not have one. Block B of the promtool test pins the residual.
      - alert: IamJitProvisioningFailures
        expr: sum by (job, defect) (increase(iam_jit_provisioning_failures_total[15m])) > 0
        for: 0m
        labels: { severity: warning }
        annotations: { summary: "IAM refused just-in-time provisioning ({{ $labels.defect }})", description: "<DESC>" }
```

- [ ] **Step 4: Run the whole promtool gate and see it pass**

Run:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
promtool check rules ops/observability/prometheus/rules/*.rules.yml
promtool test rules ops/observability/prometheus/rules/tests/*.test.yml
moon run repo:promtool
```

Expected: `check rules` reports `SUCCESS` for each file, and the IAM file has one rule more than
before. `test rules` prints `SUCCESS` for each test file. `moon run repo:promtool` exits 0.

- [ ] **Step 5: Confirm the description is identical in the rule and the test**

Run:

```bash
python3 - <<'EOF'
import re
rule = open("ops/observability/prometheus/rules/iam.rules.yml").read()
test = open("ops/observability/prometheus/rules/tests/iam.test.yml").read()
r = re.search(r'alert: IamJitProvisioningFailures.*?description: "([^"]*)"', rule, re.S).group(1)
t = re.findall(r'IAM refused just-in-time provisioning \(email_conflict\)", description: "([^"]*)"', test)
assert len(t) == 4, len(t)
assert all(x == r for x in t), "description differs"
assert "<DESC>" not in rule + test
print("ok: 4 test copies equal the rule description")
EOF
```

Expected: `ok: 4 test copies equal the rule description`.

- [ ] **Step 6: Run the mutation battery**

This step proves that each assertion fails against a named mutant. It writes each mutant into a
temporary copy, so the tracked files never change. Run:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
T=$(mktemp -d)
mkdir -p "$T/rules/tests"
cp ops/observability/prometheus/rules/tests/iam.test.yml "$T/rules/tests/"
python3 - "$T" <<'EOF'
import re, subprocess, sys
T = sys.argv[1]
src = open("ops/observability/prometheus/rules/iam.rules.yml").read()
start = src.index("- alert: IamJitProvisioningFailures")
head, block = src[:start], src[start:]
EXPR = "sum by (job, defect) (increase(iam_jit_provisioning_failures_total[15m])) > 0"
assert block.count(EXPR) == 1 and block.count("for: 0m") == 1
mutants = {
    "baseline":       (None, None, set()),
    ">= 0":           (EXPR, EXPR.replace(") > 0", ") >= 0"), {"5m", "10m", "12m", "23m", "25m"}),
    "for: 5m":        ("for: 0m", "for: 5m", {"10m", "13m", "16m"}),
    "[14m]":          (EXPR, EXPR.replace("[15m]", "[14m]"), {"23m"}),
    "[17m]":          (EXPR, EXPR.replace("[15m]", "[17m]"), {"25m"}),
    "by (defect)":    (EXPR, EXPR.replace("sum by (job, defect)", "sum by (defect)"), {"10m", "13m", "16m", "23m"}),
    "no aggregation": (EXPR, "increase(iam_jit_provisioning_failures_total[15m]) > 0", {"10m", "13m", "16m", "23m"}),
}
bad = 0
for name, (old, new, want) in mutants.items():
    body = block if old is None else block.replace(old, new, 1)
    open(f"{T}/rules/iam.rules.yml", "w").write(head + body)
    r = subprocess.run(["promtool", "test", "rules", f"{T}/rules/tests/iam.test.yml"],
                       capture_output=True, text=True)
    out = r.stdout + r.stderr
    got = set(re.findall(r"alertname: IamJitProvisioningFailures, time: ([0-9a-z]+)", out))
    other = re.findall(r"alertname: (\w+), time:", out)
    other = [a for a in other if a != "IamJitProvisioningFailures"]
    ok = got == want and not other and (r.returncode == 0) == (not want)
    bad += not ok
    print(f"{'OK ' if ok else 'BAD'} {name:15} rc={r.returncode} failed_at={sorted(got)} want={sorted(want)} other={other}")
    if not ok:
        print(out[:3000])
sys.exit(1 if bad else 0)
EOF
echo "battery rc=$?"
```

Expected: seven `OK` lines and `battery rc=0`.

If a `BAD` line shows `failed_at=[]` for a mutant whose `want` is not empty, first read the printed
promtool output. If the time format differs from `10m` (for example `10m0s`), fix the regex and run
again. Do not change `want` to make a line pass. If a real mutant survives, the test is wrong:
report it and stop.

Save the seven result lines to
`/private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/01460aca-b148-4e2e-9271-93bcdc595935/scratchpad/sma-706-mutation.txt`.
The PR description uses this file.

- [ ] **Step 7: Commit**

```bash
git add ops/observability/prometheus/rules/iam.rules.yml ops/observability/prometheus/rules/tests/iam.test.yml
git commit -m "feat(repo): alert on refused JIT provisioning in IAM (SMA-706)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git log --oneline -1
```

Expected: the commit hook prints `✓ commitlint`, and the new commit is at the top.

---

### Task 2: Dashboard panel

**Files:**
- Modify: `ops/observability/grafana/dashboards/iam.json` (append a panel after panel 20, the last
  element of `panels`)

**Interfaces:**
- Consumes: the alert name `IamJitProvisioningFailures` from Task 1 (in the description only).
- Produces: panel `id: 21`, title `JIT provisioning failures`, used by Task 3 (dashboard tour).

- [ ] **Step 1: Write the check and see it fail**

Run:

```bash
python3 - <<'EOF'
import json
d = json.load(open("ops/observability/grafana/dashboards/iam.json"))
ids = [p["id"] for p in d["panels"]]
assert len(ids) == len(set(ids)), f"duplicate ids {ids}"
cells = {}
for p in d["panels"]:
    g = p["gridPos"]
    for x in range(g["x"], g["x"] + g["w"]):
        for y in range(g["y"], g["y"] + g["h"]):
            assert (x, y) not in cells, f"panel {p['id']} overlaps panel {cells[(x, y)]}"
            cells[(x, y)] = p["id"]
p = [p for p in d["panels"] if p["id"] == 21]
assert p, "panel 21 missing"
p = p[0]
assert p["title"] == "JIT provisioning failures"
assert p["gridPos"] == {"h": 8, "w": 12, "x": 12, "y": 72}
t = p["targets"][0]
assert t["expr"] == "sum(rate(iam_jit_provisioning_failures_total[$__rate_interval])) by (defect)"
assert t["legendFormat"] == "{{defect}}"
assert p["fieldConfig"]["defaults"]["unit"] == "ops"
print("ok: panel 21 valid, ids unique, no overlap")
EOF
```

Expected: `AssertionError: panel 21 missing`.

- [ ] **Step 2: Add the panel**

In `ops/observability/grafana/dashboards/iam.json`, the file ends with panel 20, then `  ]`, then
`}`. Panel 20 ends with these lines:

```json
          "expr": "sum(rate(iam_authz_generation_rewinds_total[$__rate_interval])) by (counter, outcome)",
          "legendFormat": "{{counter}} {{outcome}}"
        }
      ]
    }
  ]
}
```

Change the `    }` that closes panel 20 to `    },` and insert the new panel after it, so the
end of the file reads:

```json
          "expr": "sum(rate(iam_authz_generation_rewinds_total[$__rate_interval])) by (counter, outcome)",
          "legendFormat": "{{counter}} {{outcome}}"
        }
      ]
    },
    {
      "id": 21,
      "type": "timeseries",
      "title": "JIT provisioning failures",
      "description": "iam_jit_provisioning_failures_total by defect — refused JIT provisioning requests (SMA-698); alert IamJitProvisioningFailures",
      "gridPos": { "h": 8, "w": 12, "x": 12, "y": 72 },
      "datasource": { "type": "prometheus", "uid": "prometheus" },
      "fieldConfig": { "defaults": { "unit": "ops" }, "overrides": [] },
      "targets": [
        {
          "refId": "A",
          "datasource": { "type": "prometheus", "uid": "prometheus" },
          "expr": "sum(rate(iam_jit_provisioning_failures_total[$__rate_interval])) by (defect)",
          "legendFormat": "{{defect}}"
        }
      ]
    }
  ]
}
```

- [ ] **Step 3: Run the check again and see it pass**

Run the script from Step 1 again.

Expected: `ok: panel 21 valid, ids unique, no overlap`.

- [ ] **Step 4: Run the drift gate**

Run:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:observability-drift
```

Expected: exit code 0. The gate checks every metric name in rule and panel expressions against
`names::ALL`, and the new name is already there (`names.rs:49`).

- [ ] **Step 5: Commit**

```bash
git add ops/observability/grafana/dashboards/iam.json
git commit -m "feat(repo): show JIT provisioning failures on the IAM dashboard (SMA-706)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git log --oneline -1
```

---

### Task 3: Runbook text

**Files:**
- Modify: `docs/ops/RUNBOOK-observability.md` (line 125 catalog row; dashboard tour at lines
  181-195; index table at line 237; new section before line 1302)
- Modify: `docs/ops/RUNBOOK-chart.md` (the paragraph at lines 131-142)

**Interfaces:**
- Consumes: the alert name `IamJitProvisioningFailures` (Task 1) and the panel title
  `JIT provisioning failures` (Task 2).
- Produces: the anchor heading `### \`IamJitProvisioningFailures\``, which the rule's
  `See RUNBOOK section 4.` points to.

- [ ] **Step 1: Write the check and see it fail**

Run:

```bash
python3 - <<'EOF'
o = open("docs/ops/RUNBOOK-observability.md").read()
c = open("docs/ops/RUNBOOK-chart.md").read()
assert "No alert ships for it yet (SMA-706)" not in o, "catalog row still says no alert"
assert "- JIT provisioning failures (SMA-706)" in o, "dashboard tour line missing"
assert "| `IamJitProvisioningFailures` | `sum by (job, defect) (increase(iam_jit_provisioning_failures_total[15m])) > 0` | warning |" in o, "index row missing"
h = "### `IamJitProvisioningFailures` — IAM refused just-in-time provisioning (warning)"
assert o.count(h) == 1, "section heading missing"
s = o.index(h); e = o.index("### `IamRedisBreakerOpen`")
assert s < e, "section is not before IamRedisBreakerOpen"
sec = o[s:e]
for needle in ["**Meaning.**", "**Confirm:**", "**Likely causes (`missing_email`):**",
               "**Likely causes (`email_conflict`):**", "**Remediation (`missing_email`):**",
               "**Remediation (`email_conflict`):**", "**Warning.**", "**When the alert is silent:**",
               "SMA-712", "external_identity", "case-sensitive", "CreateUser"]:
    assert needle in sec, f"section lacks {needle}"
assert "IamJitProvisioningFailures" in c, "chart runbook pointer missing"
print("ok: runbook text in place")
EOF
```

Expected: `AssertionError: catalog row still says no alert`.

- [ ] **Step 2: Change the catalog row (line 125)**

In `docs/ops/RUNBOOK-observability.md` line 125, replace the text
`No alert ships for it yet (SMA-706).` with:

```text
The alert `IamJitProvisioningFailures` fires on it (§4, SMA-706).
```

- [ ] **Step 3: Add the dashboard tour line**

In the **Paigasus IAM** list of the dashboard tour, after the line that ends with
`deleted by the retention sweep (rate, by `reason`).`, insert this line:

```markdown
- JIT provisioning failures (SMA-706): refused just-in-time provisioning requests per second, by
  `defect`. A flat zero is the healthy state. See `IamJitProvisioningFailures` in §4.
```

- [ ] **Step 4: Add the index row**

In the §4 table, after the row that starts with `| \`IamAuthzGenerationRewound\``, insert:

```markdown
| `IamJitProvisioningFailures` | `sum by (job, defect) (increase(iam_jit_provisioning_failures_total[15m])) > 0` | warning |
```

- [ ] **Step 5: Add the alert section**

Insert this text immediately before the line
`### \`IamRedisBreakerOpen\` — Redis circuit breaker is not closed (warning)`, with one blank line
after it:

````markdown
### `IamJitProvisioningFailures` — IAM refused just-in-time provisioning (warning)

**Meaning.** `sum by (job, defect) (increase(iam_jit_provisioning_failures_total[15m])) > 0`,
`for: 0m` (SMA-706). IAM refused to provision a new identity on its first login (SMA-698). On
HTTP, IAM answered `403 provisioning-failed`. On gRPC, IAM answered `PermissionDenied` with the
reason `provisioning-failed`. One refused request fires the alert at the next rule evaluation.
The alert stays active for about 15 minutes after the last refusal. The counter counts refused
requests, not users. One user with a bad token makes several refused requests, because a console
session makes several IAM calls. This is `warning`: a refusal blocks one user's first login, but
IAM itself is healthy.

**Confirm:**
1. Read the IAM log. Find the `warn` line that starts with `just-in-time provisioning failed`.
   It has the fields `defect` and `issuer`. For `missing_email` it also has `email_claim`
   (`absent` or `invalid`). The line never contains the email, the subject or the token.
2. The log rate limit is per replica. Each replica writes at most one line per issuer and defect
   in 10 s. The field `suppressed` gives the number of refusals since the last line. So the line
   count is lower than the counter.
3. Break down the rate:
   `sum by (job, defect) (increase(iam_jit_provisioning_failures_total[15m]))`.

**Likely causes (`missing_email`):**
- The issuer's client does not map the `email` claim into the ACCESS token. IAM reads the access
  token, not the ID token.
- The user has no email at the identity provider.
- The email is not a valid address (`email_claim=invalid`).
- A machine client sends a client-credentials token from an issuer that has JIT enabled. Such a
  token has no email, so every call counts.

**Remediation (`missing_email`):** add an `email` mapper for the access token to the issuer's
client, or set the user's email at the identity provider. A machine client must use an issuer
that has JIT disabled, or it must be provisioned in a different way.

**Likely causes (`email_conflict`):** another IAM user already has this email. IAM does not link
identities by email (the "no auto-link by email" rule, D5). The email match is exact and
case-sensitive (the unique key on `"user".email`; `Email::parse` does not change the letter
case). So two emails that differ only in letter case make a second user, not a conflict. The
causes are:
- The same person signs in through a second issuer.
- A user was made with `POST /v1/users` or gRPC `CreateUser`. Such a user has no external
  identity. JIT does not link by email, so this person's first login fails every time.
- The configured issuer string changed, for example when the identity provider moved to a new
  URL. The stored `(issuer, subject)` pairs in `external_identity` then do not match, so every
  returning user gets `email_conflict` at the same time. A sudden high rate for many users is
  this case.
- The identity provider gave the same person a new `sub`, for example after the user was deleted
  and made again, or after a realm import.
- An email moved from one person to another at the identity provider.

**Remediation (`email_conflict`):**
1. Get the email from the user or from the identity provider's login events. The IAM log does
   not contain it.
2. If the issuer URL changed, restore the old issuer string in the IAM configuration. Do not edit
   user rows.
3. If the email is wrong at the identity provider, correct it there.
4. Otherwise a link is necessary. IAM has no API to update a user, change an email or link an
   identity (SMA-712 tracks one). The only write call is `POST /v1/users` / `CreateUser`. A link
   needs a manual Postgres change: add an `external_identity` row with the new `issuer` and
   `subject` for the existing `principal_id`, or change `"user".email`.

**Warning.** A manual link or a manual email change brings back the account-takeover risk that
rule D5 prevents. Before you change a row, confirm that the new identity is the same person.
Also confirm that the identity provider verifies emails.

**When the alert is silent:**
- **The first refusal of a new series.** IAM primes both series at zero before it serves. But
  Prometheus reads the zero only at its first scrape. If a refusal happens before that scrape,
  and the series is new to Prometheus, the first scraped value is already above zero. Then
  `increase()` uses that value as its baseline, and the alert does not fire for that refusal. A
  later refusal fires it as usual. A series is new after a fresh install, and after each pod start
  with pod-level service discovery. The chart runs IAM with `replicas: 1` and `maxSurge: 0`, so
  every rollout opens this gap for one scrape interval. Only an extra rule branch could close it,
  and that branch fires falsely after a Prometheus data gap. The SMA-706 spec, section 7, has the
  details.
- **Metrics are off** (`metrics.enabled = false`). `/metrics` is not mounted. If Prometheus still
  scrapes the target, the scrape fails and `TargetDown` fires.
- **A JIT-disabled issuer.** IAM returns `identity-not-provisioned` and does not count it
  (SMA-707).
- **An IAM binary older than SMA-698.** The series does not exist.
````

- [ ] **Step 6: Add the chart runbook pointer**

In `docs/ops/RUNBOOK-chart.md`, in item 2 (**Email.**), after the sentence
`The counter `iam_jit_provisioning_failures_total` counts each refused request.`, add:

```text
 The alert `IamJitProvisioningFailures` fires on it. `RUNBOOK-observability.md` §4 gives the
   operator action for each `defect` value.
```

Keep the three-space indent of the continuation lines in that list item.

- [ ] **Step 7: Run the check again and see it pass**

Run the script from Step 1 again.

Expected: `ok: runbook text in place`.

- [ ] **Step 8: Commit**

```bash
git add docs/ops/RUNBOOK-observability.md docs/ops/RUNBOOK-chart.md
git commit -m "docs(docs): add the IamJitProvisioningFailures runbook entry (SMA-706)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git log --oneline -1
```

---

### Task 4: Final verification

**Files:** none changed.

- [ ] **Step 1: Run the two owning gates with a clean cache**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:promtool --force
moon run repo:observability-drift --force
```

Expected: both exit 0.

- [ ] **Step 2: Run the mutation battery from Task 1 Step 6 again**

The rule and the test must be unchanged since Task 1. Run the battery again and compare the seven
lines with `sma-706-mutation.txt`.

Expected: seven `OK` lines, identical to the saved file.

- [ ] **Step 3: Check the diff against the spec's file list**

```bash
git diff --stat main...HEAD
```

Expected: only these files, plus the spec and this plan:
`ops/observability/prometheus/rules/iam.rules.yml`,
`ops/observability/prometheus/rules/tests/iam.test.yml`,
`ops/observability/grafana/dashboards/iam.json`, `docs/ops/RUNBOOK-observability.md`,
`docs/ops/RUNBOOK-chart.md`.

- [ ] **Step 4: Run the affected gate graph**

Run the full command between the `ci-targets` markers in the root `CLAUDE.md`, with
`--base main` instead of `--base origin/main` if `origin/main` is not current. Run it in the
foreground with a timeout of at least 20 minutes.

Expected: every selected task passes. If a gate fails only because of the local bash version or
the 512-byte pipe (see "This development Mac only" in `CLAUDE.md`), record that, run that gate
directly with the correct bash, and report both results. Do not record such a failure as a
finding against this change.
