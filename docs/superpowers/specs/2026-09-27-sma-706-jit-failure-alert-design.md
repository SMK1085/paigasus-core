# SMA-706: alert rule and dashboard panel for JIT provisioning failures

- Linear: SMA-706 (blocked by SMA-698, which is merged as `da89b79a`)
- Date: 2026-09-27
- Status: draft

## 1. Problem

SMA-698 added the counter `iam_jit_provisioning_failures_total{defect}`. The label `defect` has
two values: `missing_email` and `email_conflict`. IAM primes both series at zero when metrics
are on (`prime_jit_provisioning_failures`, `authenticate_token.rs:89`, called from
`main.rs:95`). SMA-698 added no alert rule and no dashboard panel (SMA-698 spec, section 10).
So an operator sees a refused JIT login only when a user reports it.

## 2. Goal

An operator gets a `warning` alert on the first refused JIT provisioning attempt after start-up,
split by `defect`. The alert and the runbook tell the operator what to do for each `defect`
value. The IAM dashboard shows the failure rate by `defect`.

## 3. Non-goals

1. No change to the Rust service, to the counter, or to its labels. The counter has no `issuer`
   label. The operator reads the issuer from the `warn` log line (SMA-698).
2. No Alertmanager routing. The repo has no Alertmanager configuration.
3. No fix for the start-up residual in section 7.
4. No threshold from production data. There is no baseline yet. Section 4.2 gives the reason
   for the chosen values.

## 4. Alert rule

### 4.1 The rule

Add one rule to the `iam` group in `ops/observability/prometheus/rules/iam.rules.yml`:

```yaml
- alert: IamJitProvisioningFailures
  expr: sum by (defect) (increase(iam_jit_provisioning_failures_total[15m])) > 0
  labels: { severity: warning }
  annotations: { summary: "...", description: "..." }
```

The rule has no `for:` key, so it has the Prometheus default `for: 0s`.

Put a comment above the rule. The comment gives the reasons in section 4.2, in the style of the
comments above `IamOutboxPublishFailures` and `IamAuthzGenerationRewound`.

### 4.2 Decisions

- **`sum by (defect)`, not a bare `sum()`.** The grouping gives one alert for each defect
  value, over all replicas. It also puts `defect` on the alert, where triage needs it. It is
  also what makes a control series possible in the test (section 5). A bare `sum()` folds every
  series into one total, so no series can act as a control. This is the same reason as for
  `IamAuthzGenerationRewound` (`iam.rules.yml:302-307`).
- **No `for:`.** One refused request is enough to fire. With `for: 0s`, the alert fires at the
  first rule evaluation that sees the step. A `for:` value near the window length can let a
  single failure resolve before the alert fires. Then the first acceptance criterion fails.
  `IamOutboxPublishFailures` uses `[5m]` with `for: 5m`, and it has this problem for an isolated
  failure. This rule does not copy that shape.
- **Window `15m`.** After one failure, `increase()` stays above zero for about 15 minutes. So
  the alert stays active for about 15 minutes after the last failure. A shorter window makes an
  isolated failure visible only for a short time. A longer window keeps a resolved defect active
  for longer. 15 minutes matches `IamAuthzGenerationRewound`.
- **Severity `warning`.** A refusal blocks one user's first login. It is a configuration or data
  defect, not an outage of IAM. The repo uses `critical` only for an active outage or a
  correctness risk. Sven chose `15m`, no `for:`, `warning` on 2026-09-27.
- **The counter counts requests, not identities.** One user with a bad token makes several
  refused requests, because a console session makes several IAM calls. This does not change the
  rule: the rule fires on any increase. The description says that the value is not a count of
  users.

### 4.3 Annotations

- `summary`: `IAM refused just-in-time provisioning`. It is a static string, as in all other IAM
  rules.
- `description`: one paragraph. It says:
  - that IAM refused JIT provisioning for a new identity and returned 403 `provisioning-failed`;
  - that the value counts requests, not users;
  - `defect=missing_email`: the access token has no valid `email` claim. The operator must make
    the issuer put a valid `email` claim into the ACCESS token (not only the ID token).
  - `defect=email_conflict`: another IAM user already has this email. IAM does not link
    identities by email. The operator must find the two identities and decide by hand which
    one keeps the email.
  - that the `warn` log line that starts with `just-in-time provisioning failed` names the
    issuer;
  - `See RUNBOOK section 4.`, as the other IAM rules do.

The plan fixes the exact text. The test asserts the text, so the rule and the test must agree.

## 5. Promtool test

Add one block to `ops/observability/prometheus/rules/tests/iam.test.yml`. The `repo:promtool`
task (`moon.yml:560-575`) runs `promtool test rules ops/observability/prometheus/rules/tests/*.test.yml`,
so the block needs no registration.

### 5.1 Input

`interval: 1m`, two series, both seeded at `0` from t=0. The zero seed is faithful to
production only because IAM primes both series. The comment above the block says this.

- The control: `iam_jit_provisioning_failures_total{defect="missing_email"}`, flat at `0` for
  the whole block.
- The signal: `iam_jit_provisioning_failures_total{defect="email_conflict"}`, `0` until t=10m,
  then `1` for the rest of the block. This is exactly one failure, which is the shape of the
  first acceptance criterion.

### 5.2 Assertions

Each assertion must kill one named mutant of the rule. The plan fixes the exact `eval_time`
values. Prometheus 3 range windows are left-open, `(t - 15m, t]`, so each time needs a margin of
at least one sample from the window edge.

| # | When | Expected | Kills |
|---|------|----------|-------|
| A1 | before the step (t=5m) | no alert | `> 0` changed to `>= 0` (both groups fire) |
| A2 | the first evaluation that includes the step | exactly one alert, `defect=email_conflict`, `severity=warning`, full annotations | `for: 5m` (not yet active); also proves that the rule file loads and the rule evaluates |
| A3 | about 8 minutes after the step | still exactly one alert, `defect=email_conflict` | window `[5m]` (the step has left a 5-minute window) |
| A4 | more than 15 minutes after the step | no alert | window `[30m]` or longer; also proves that the alert resolves |

The control series stays silent in A2 and A3. That is what pins `> 0` against `>= 0` while the
signal is active.

### 5.3 Proof that the assertions bite

A green test run is not enough. For each row in 5.2, apply the mutant to the rule, run
`promtool test rules`, and record that the test fails at that row. Then restore the rule. Record
the results in the PR description. Restore each mutant by an edit, not by `git checkout --`,
because a checkout also reverts uncommitted work.

## 6. Dashboard panel

Add one panel to `ops/observability/grafana/dashboards/iam.json`:

- `id: 21`, `type: timeseries`, `title: "JIT provisioning failures"`.
- `gridPos: { h: 8, w: 12, x: 12, y: 72 }`. This fills the empty half of the row that holds
  panel 19.
- `datasource: { type: prometheus, uid: prometheus }`.
- One target: `sum(rate(iam_jit_provisioning_failures_total[$__rate_interval])) by (defect)`,
  `legendFormat: "{{defect}}"`.
- `fieldConfig.defaults.unit: "ops"`.
- Copy every other field from panel 16 ("Bootstrap-admin seed failures"), which has the same
  shape with the label `stage`.

`repo:observability-drift` (`moon.yml:292-307`, `paigasus-observability/tests/drift.rs`)
checks every metric name in rule and panel expressions against `names::ALL`. The metric is
already in `names::ALL` (`names.rs:49`), so the gate stays green without a change.

## 7. Known residual: a failure before the first scrape

IAM primes the series at start-up. Prometheus scrapes every 15 s
(`ops/observability/prometheus/prometheus.yml`). If a failure happens after start-up but before
the first scrape, the first sample is already `1`. Then `increase()` uses that sample as its
baseline, and the alert does not fire for that failure. A second failure fires it as usual.

This spec does not fix it. A fix belongs in the service start-up order or in the Prometheus
configuration, not in this rule. The runbook section states the residual.

## 8. Runbook

Change `docs/ops/RUNBOOK-observability.md` in three places:

1. **§2.2 metric catalog, line 125.** Replace "No alert ships for it yet (SMA-706)." with a
   sentence that names `IamJitProvisioningFailures`.
2. **§4 index table.** Add one row: alert name, expression, `warning`.
3. **§4 alert section.** Add `### `IamJitProvisioningFailures` — ... (warning)` in the same
   structure as the other sections: **Meaning.**, **Likely causes:**, **Confirm:**,
   **Remediation:**. Give the causes and the remediation for each `defect` value separately.
   - `missing_email`, causes: the issuer's client does not map the `email` claim into the
     access token; the user has no email in the identity provider; the email fails
     `Email::parse` (the log line has `email_claim=invalid`).
   - `email_conflict`, causes: the same person signs in through a second issuer; an email moved
     from one person to another in the identity provider; an old IAM user still holds the email.
   - Confirm: read the `warn` line that starts with `just-in-time provisioning failed`. It
     names the issuer and the defect. The log rate limit writes at most one line per issuer and
     defect in 10 s, so the line count is lower than the counter.
   - Also state the residual from section 7.

## 9. Files

| File | Change |
|------|--------|
| `ops/observability/prometheus/rules/iam.rules.yml` | new rule and comment |
| `ops/observability/prometheus/rules/tests/iam.test.yml` | new test block |
| `ops/observability/grafana/dashboards/iam.json` | new panel id 21 |
| `docs/ops/RUNBOOK-observability.md` | catalog row, index row, new section |

No other file lists alerts. There is no Helm copy of the rules and no Alertmanager routing.

## 10. Verification

- `moon run repo:promtool`: `check config`, `check rules` and `test rules` pass.
- The mutation runs in 5.3 each fail at the expected row.
- `moon run repo:observability-drift` passes.
- `iam.json` is valid JSON, and no two panels share an `id`.
- The full gate graph from the root `CLAUDE.md` passes before the push.

## 11. Acceptance criteria (from SMA-706)

- The alert fires on the first failure after start-up: A2, with the residual in section 7.
- A promtool test covers the rule, and a control series proves that the rule file loads: 5.1,
  5.2.
- `repo:observability-drift` stays green: section 6.
- The dashboard shows the failure rate by `defect`: section 6.
