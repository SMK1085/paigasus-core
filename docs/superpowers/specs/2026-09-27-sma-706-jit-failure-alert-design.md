# SMA-706: alert rule and dashboard panel for JIT provisioning failures

- Linear: SMA-706 (blocked by SMA-698, which is merged as `da89b79a`)
- Date: 2026-09-27
- Status: approved by Sven on 2026-09-27

## 1. Problem

SMA-698 added the counter `iam_jit_provisioning_failures_total{defect}`. The label `defect` has
two values: `missing_email` and `email_conflict`. IAM primes both series at zero when metrics
are on (`prime_jit_provisioning_failures`, `authenticate_token.rs:89`, called from
`main.rs:95`, before the HTTP listener binds at `main.rs:127`). SMA-698 added no alert rule and
no dashboard panel (SMA-698 spec, section 10). So an operator sees a refused JIT login only when
a user reports it.

## 2. Goal

An operator gets a `warning` alert on the first refused JIT provisioning attempt after start-up,
split by `defect`, with the limit in section 7. The alert and the runbook tell the operator what
to do for each `defect` value. The IAM dashboard shows the failure rate by `defect`.

## 3. Non-goals

1. No change to the Rust service, to the counter, or to its labels. The counter has no `issuer`
   label. The operator reads the issuer from the `warn` log line (SMA-698).
2. No Alertmanager routing. The repo has no Alertmanager configuration.
3. No operator API to link an identity or to change an email. Section 8 gives the manual path.
   SMA-712 tracks the API.
4. No threshold from production data. There is no baseline yet. Section 4.2 gives the reason
   for the chosen values.
5. No change to `IamOutboxPublishFailures`. Section 4.2 describes a defect in it. SMA-713
   tracks that defect.

## 4. Alert rule

### 4.1 The rule

Add one rule to the `iam` group in `ops/observability/prometheus/rules/iam.rules.yml`:

```yaml
- alert: IamJitProvisioningFailures
  expr: sum by (job, defect) (increase(iam_jit_provisioning_failures_total[15m])) > 0
  for: 0m
  labels: { severity: warning }
  annotations: { summary: "IAM refused just-in-time provisioning ({{ $labels.defect }})", description: "..." }
```

Put a comment above the rule. The comment gives the reasons in section 4.2, in the style of the
comments above `IamOutboxPublishFailures` and `IamAuthzGenerationRewound`.

### 4.2 Decisions

- **`sum by (job, defect)`.** The grouping gives one alert for each job and defect value, over
  all replicas of that job. It puts `defect` on the alert, where triage needs it. It keeps `job`,
  so one Prometheus that scrapes two deployments gives two alerts. The runbook keeps `job` on the
  breaker panel for the same reason (`RUNBOOK-observability.md:186-188`). The grouping also makes
  a control series possible in the test (section 5). A bare `sum()` folds every series into one
  total, so no series can act as a control (`iam.rules.yml:302-307`).
- **`for: 0m`.** One refused request is enough to fire. The alert fires at the first rule
  evaluation that sees the step. All IAM rules state `for:`. The rules without a hold write
  `for: 0m`. `IamOutboxEventsParked` (`increase([15m]) > 0`, `for: 0m`, `iam.rules.yml:15-19`) is
  the direct precedent.
- **Window `15m`.** After one failure, `increase()` stays above zero until the last sample before
  the step leaves the window, which is about 15 minutes. So the alert stays active for about 15
  minutes after the last failure.
- **Severity `warning`.** A refusal blocks one user's first login. It is a configuration or data
  defect, not an outage of IAM. The repo uses `critical` only for an active outage or a
  correctness risk.
- **Sven chose `15m`, no hold, `warning` on 2026-09-27.**
- **The counter counts requests, not identities.** One user with a bad token makes several
  refused requests, because a console session makes several IAM calls. The rule fires on any
  increase, so this does not change the rule. The description says that the value is not a count
  of users.
- **Why the rule does not copy `IamOutboxPublishFailures`.** That rule uses `[5m]` with
  `for: 5m`. After one isolated failure, the condition holds only until the pre-step sample leaves
  the 5-minute window. At a 15 s scrape that is about 4 min 45 s, which is shorter than the 5 min
  hold. So one isolated failure never fires that rule. Its runbook entry says that it "can fire
  on the very first failure" (`RUNBOOK-observability.md:520-522`). This spec does not change it
  (non-goal 5). SMA-713 tracks it.

### 4.3 Annotations

- `summary`: `IAM refused just-in-time provisioning ({{ $labels.defect }})`. The label is in the
  summary, because some notifications show only the summary. `IamRedisBreakerOpen` uses a label
  in its summary too (`iam.rules.yml:260`).
- `description`: one paragraph. It says:
  - that IAM refused JIT provisioning for a new identity: HTTP 403 `provisioning-failed`, or gRPC
    `PermissionDenied` with the reason `provisioning-failed`;
  - that the value counts requests, not users;
  - `defect=missing_email`: the access token has no valid `email` claim. The operator must make
    the issuer put a valid `email` claim into the ACCESS token (not only the ID token).
  - `defect=email_conflict`: another IAM user already has this exact email. IAM does not link
    identities by email. The operator must follow the runbook, and must not link by hand before
    the runbook's identity check.
  - that the `warn` log line that starts with `just-in-time provisioning failed` names the
    issuer;
  - `See RUNBOOK section 4.`, as the other IAM rules do.

The plan fixes the exact text. The test asserts the full text, so the rule and the test must
agree.

## 5. Promtool test

Add two blocks to `ops/observability/prometheus/rules/tests/iam.test.yml`. The `repo:promtool`
task (`moon.yml:560-575`) runs `promtool test rules ops/observability/prometheus/rules/tests/*.test.yml`,
so the blocks need no registration. promtool is 3.13.1, so range windows are left-open:
`(t - w, t]`.

### 5.1 Block 1: the primed shape

`interval: 1m`. Both series carry `job="iam", instance="a:8080"`, as the other blocks do. Both
are seeded at `0` from t=0. The zero seed is faithful to production only because IAM primes
both series. The comment above the block says this.

- The control: `{defect="missing_email"}`, values `'0x40'`. It stays flat.
- The signal: `{defect="email_conflict"}`, values `'0x9 1x30'`. It is `0` at 0–9m and `1` at
  10–40m. This is exactly one failure, which is the shape of the first acceptance criterion.

The correct rule fires at every evaluation from 10m to 23m. It is quiet at 9m and before, and at
24m and after. `increase()` is above zero only when the window holds the 9m sample and a sample
at 10m or later. Extrapolation cannot change the sign, and a flat window gives exactly 0.

| # | `eval_time` | Expected |
|---|------|----------|
| A1 | 5m | no alert (baseline) |
| A2 | 10m | exactly one alert: `job=iam`, `defect=email_conflict`, `severity=warning`, full annotations |
| A3 | 23m | the same single alert as A2, with full labels and annotations |
| A4 | 25m | no alert |

A3 and A4 each keep a margin of one sample from the window edge. So a Prometheus 2 left-closed
window gives the same result.

### 5.2 Block 2: the un-primed shape (the residual in section 7)

`interval: 1m`, one series `{job="iam", instance="a:8080", defect="email_conflict"}`, values
`'_x9 1x5 2x10'`. The series is absent at 0–8m. It appears at 9m already at `1`, which is a
failure before the first scrape of a new series. It steps to `2` at 15m.

| # | `eval_time` | Expected |
|---|------|----------|
| B1 | 12m | no alert: the first failure is invisible (section 7) |
| B2 | 16m | one alert: a second failure after the first scrape fires |

This block pins the runbook claim in section 7. If a later change fixes the residual, B1 must
change in the same commit.

### 5.3 Proof that the assertions bite

A green test run is not enough. Apply each mutant below to the rule, run `promtool test rules`,
and record every row that fails. Then restore the rule by an edit, not by `git checkout --`,
because a checkout also reverts uncommitted work. After any fix to the rule or the test, run the
whole set again.

| Mutant | Must fail at |
|--------|--------------|
| `> 0` → `>= 0` | A1, A2, A3, A4 (the flat control fires) |
| `for: 0m` → `for: 5m` | A2 |
| `[15m]` → `[14m]` | A3 |
| `[15m]` → `[17m]` | A4 |
| `sum by (job, defect)` → `sum by (defect)` | A2, A3 (no `job` label) |
| aggregation removed | A2, A3 (extra `instance` label) |

A1 kills no unique mutant. It is a baseline row. The window is pinned to the band `[15m]` to
`[16m]`. Record the results in the PR description.

## 6. Dashboard panel

Add one panel to `ops/observability/grafana/dashboards/iam.json`:

- `id: 21`, `type: timeseries`, `title: "JIT provisioning failures"`.
- `description: "iam_jit_provisioning_failures_total by defect — refused JIT provisioning
  requests (SMA-698); alert IamJitProvisioningFailures"`.
- `gridPos: { h: 8, w: 12, x: 12, y: 72 }`. This fills the empty half of the row that holds
  panel 19.
- `datasource: { type: prometheus, uid: prometheus }`.
- One target: `sum(rate(iam_jit_provisioning_failures_total[$__rate_interval])) by (defect)`,
  `legendFormat: "{{defect}}"`.
- `fieldConfig.defaults.unit: "ops"`.
- Copy the other structural fields (`options`, `fieldConfig` other than `unit`) from panel 16.
  Do not copy its title or its description.

`repo:observability-drift` (`moon.yml:292-307`, `paigasus-observability/tests/drift.rs`)
checks every metric name in rule and panel expressions against `names::ALL`. The metric is
already in `names::ALL` (`names.rs:49`), so the gate stays green without a change.

## 7. Known residual: the first failure of a new series

**The condition.** IAM primes the series before it binds its listener. But Prometheus reads the
primed `0` only at its first scrape. If a failure happens before that first scrape, and the
series is new to Prometheus, the first scraped value is already above zero. `increase()` uses
that first sample as its baseline, and the alert does not fire for that failure. A later
failure, after the first scrape, fires the alert as usual (5.2, B2).

A series is new when the window holds no earlier sample with the same labels. With a static
target (`prometheus.yml:9`), a restart keeps the labels, so the pre-restart samples stay in the
window as the baseline. A step from 0 to 1 fires, and a post-restart value below the old value
counts as a counter reset and fires. So a restart on a static target misses a failure only in
rare cases. With pod-level Kubernetes discovery, every pod start makes a new series. The chart
runs IAM with `replicas: 1` and `maxSurge: 0` (`charts/paigasus/templates/backend-deployment.yaml:23-35`),
so every rollout and every fresh install opens the gap again. The gap is one scrape interval of
the operator's Prometheus.

**Where a fix can go.** Neither the service nor the Prometheus configuration can close the gap:

- The service already primes before it serves. It cannot control when Prometheus scrapes.
- Prometheus has `created-timestamp-zero-ingestion`, but that needs a created timestamp. IAM
  serves `text/plain; version=0.0.4` (`paigasus-observability/src/lib.rs:51`), and
  metrics-exporter-prometheus 0.18.3 writes no created timestamp.
- Only the rule can close it, with a second branch, for example
  `or (sum by (job, defect) (iam_jit_provisioning_failures_total unless iam_jit_provisioning_failures_total offset 15m) > 0)`.
  That branch has its own false positive. If Prometheus has no data for more than 5 minutes
  around `t - 15m`, every series looks new, and a replica with an old failure fires.

**Decision.** This spec does not add the branch. Sven confirmed this on 2026-09-27 (section
11). The runbook states the residual.

**Other paths where the alert is silent.** The runbook lists them:

- `metrics.enabled = false`: `/metrics` is not mounted. If Prometheus still scrapes the target,
  the scrape fails and `TargetDown` fires.
- A JIT-disabled issuer returns `identity-not-provisioned` and does not count (SMA-707).
- An IAM binary older than SMA-698 has no such series.

## 8. Runbook

Change `docs/ops/RUNBOOK-observability.md` in four places:

1. **§2.2 metric catalog, line 125.** Replace "No alert ships for it yet (SMA-706)." with a
   sentence that names `IamJitProvisioningFailures`.
2. **§3 dashboard tour (lines 181-195).** Add one line for the new panel. The tour also omits
   panels 15, 16 and 20. This spec does not add them, because they are not in its scope.
3. **§4 index table.** Add one row: alert name, expression, `warning`.
4. **§4 alert section.** Add `### `IamJitProvisioningFailures` — ... (warning)` in the same
   structure as the other sections: **Meaning.**, **Likely causes:**, **Confirm:**,
   **Remediation:**. Give the causes and the remediation for each `defect` value separately.

Content of the new section:

- **Confirm (both defects).** Read the `warn` line that starts with
  `just-in-time provisioning failed`. It names the issuer and the defect. It never names the
  email or the subject. The log rate limit is per process: each replica writes at most one line
  per issuer and defect in 10 s. So the line count is lower than the counter.
- **`missing_email`, causes:**
  - the issuer's client does not map the `email` claim into the access token;
  - the user has no email in the identity provider;
  - the email fails `Email::parse` (the log line has `email_claim=invalid`);
  - a machine client sends a client-credentials token from a JIT-enabled issuer. Such a token
    has no email, so each call counts.
- **`missing_email`, remediation:** add an `email` mapper to the access token at the issuer, or
  set the user's email at the identity provider. A machine client must use an issuer that has JIT
  disabled, or must be provisioned in another way.
- **`email_conflict`, causes:**
  - the same person signs in through a second issuer;
  - a user was made with `POST /v1/users` / gRPC `CreateUser`. Such a user has no external
    identity, and JIT does not link by email (`authenticate_token.rs:312-315`), so this person's
    first login fails every time;
  - the configured issuer string changed, for example when the identity provider moved to a new
    URL. The stored `(issuer, subject)` keys then do not match, so every returning user gets
    `email_conflict` at the same time. A sudden high rate for many users is this case;
  - the identity provider gave the same person a new `sub`, for example after the user was
    deleted and made again, or after a realm import;
  - an email moved from one person to another at the identity provider.
- **`email_conflict`, the match:** the email match is exact and case-sensitive
  (`m0001_create_principal_and_user.rs:57`; `Email::parse` does not change the letter case). So
  two emails that differ only in letter case give a second user, not a conflict.
- **`email_conflict`, remediation:**
  - Get the email from the user or from the identity provider's login events. The IAM log does
    not contain it.
  - For an issuer URL change: IAM accepts only a token whose `iss` equals the configured issuer.
    Restoring the old issuer string alone makes IAM refuse every token, including users with a
    working link. Use one of these two options instead:
    - Make the identity provider issue the old `iss` again, for example with Keycloak's hostname
      or frontend URL setting. Then restore `oidc.issuer` to the old value.
    - Or keep the new issuer, and move the stored keys in one statement:
      `UPDATE external_identity SET issuer = '<new>', updated_at = now() WHERE issuer = '<old>';`.
      Also update any `zones.iam.backend.bootstrapAdmins` entries that name the old issuer.
      Use this option only when the new issuer is the same identity provider realm at a new
      URL. Before you run the statement, confirm that the identity provider keeps the same `sub`
      for every user after the change. A check of some users is not enough proof. If you cannot
      confirm it for every user, or if the new issuer is a different realm or a different
      identity provider, one `subject` value can name a different person. Then the statement
      gives that person the old account. In that case, do not run it. Use the "same person"
      case below for each user.
  - Otherwise, correct the email at the identity provider if it is wrong there.
  - IAM has no API to update a user, change an email, or link an identity. The only write call
    is `POST /v1/users` / `CreateUser`. Pick the right case:
    - Same person: a second issuer, a new `sub`, or a user made with `CreateUser`. After the
      same-person check in the Warning below, insert an `external_identity` row for the existing
      `principal_id`:
      `INSERT INTO external_identity (id, principal_id, issuer, subject, created_at, updated_at)
      VALUES (gen_random_uuid(), '<principal_id>', '<issuer>', '<subject>', now(), now());`
    - The email now belongs to a different person. Change the old user's `"user".email`. JIT
      then makes a new user for the new person at the next login.
  - **Warning.** A manual change can bring back the account-takeover risk that the "no
    auto-link by email" rule (D5) prevents. For the same-person case, first confirm that the new
    identity is the same person. For the different-person case, first confirm at the identity
    provider that the email now belongs to the new person. For both cases, confirm that the
    identity provider verifies emails.
- **When the alert is silent:** the residual and the silent paths in section 7.

Also add one line to `docs/ops/RUNBOOK-chart.md` (lines 131-142), where the counter is described
on the install path. The line names the alert and points to the new runbook section.

## 9. Files

| File | Change |
|------|--------|
| `ops/observability/prometheus/rules/iam.rules.yml` | new rule and comment |
| `ops/observability/prometheus/rules/tests/iam.test.yml` | two new test blocks |
| `ops/observability/grafana/dashboards/iam.json` | new panel id 21 |
| `docs/ops/RUNBOOK-observability.md` | catalog row, dashboard tour line, index row, new section |
| `docs/ops/RUNBOOK-chart.md` | one pointer line |

No other file lists alerts. There is no Helm copy of the rules and no Alertmanager routing.

## 10. Verification

- `moon run repo:promtool`: `check config`, `check rules` and `test rules` pass.
- Each mutant in 5.3 fails at exactly the rows in its table entry, or at a superset that the PR
  records.
- `moon run repo:observability-drift` passes.
- `iam.json` is valid JSON, and no two panels share an `id` or overlap in `gridPos`.
- The full gate graph from the root `CLAUDE.md` passes before the push.

## 11. Decisions by Sven (2026-09-27)

1. Accept the residual in section 7 without the `unless ... offset 15m` rule branch: **yes**. The
   gap is one scrape interval after each new series. The branch adds a false positive after every
   Prometheus data gap of more than 5 minutes.
2. The manual Postgres edit is the remediation for now. **SMA-712** tracks an operator API that
   links an identity or changes an email. The runbook section names SMA-712.
3. **SMA-713** tracks the `IamOutboxPublishFailures` defect in section 4.2.

## 12. Spec challenge changelog (2026-09-27)

Verdict: APPROVE WITH CHANGES. No BLOCKER.

Folded in:

- MAJOR, section 7 gave a false reason for not fixing the residual. Rewritten: only a rule branch
  can fix it, with its false positive. Decision moved to section 11.
- MAJOR, the `email_conflict` remediation could not be carried out, and it omitted causes.
  Section 8 now gives the lookup path, the missing API, the three extra causes, the case-sensitive
  match and the takeover warning.
- MINOR, the residual condition was imprecise. Now "a new series", with the rollout case and
  without the local 15 s figure.
- MINOR, the other silent paths. Added to section 7 and the runbook.
- MINOR, no guard on the aggregation. Test series now carry `job` and `instance`; two mutants
  added.
- MINOR, the window was pinned only to 10m-17m. A3 and A4 moved to 23m and 25m.
- MINOR, the values and eval times were left to the plan. Now in 5.1.
- MINOR, A1 killed no unique mutant. Now a baseline row, and 5.3 lists every failing row.
- MINOR, A3 lacked full annotations. Fixed.
- MINOR, the residual had no fixture. Block 2 added.
- MINOR, no `for:` key broke the file convention. Now `for: 0m`.
- MINOR, annotation text: gRPC reason added, `{{ $labels.defect }}` in the summary, the log rate
  limit is per replica, machine-client cause added.
- MINOR, panel 16's description would be copied. The new description is given.
- MINOR, `sum by (defect)` removed `job`. Now `sum by (job, defect)`.
- MINOR, `RUNBOOK-chart.md` and the dashboard tour were missing. Both added.
- MINOR, the `IamOutboxPublishFailures` aside filed no issue. Now non-goal 5 and question 3.

Rejected: none. The three challenger questions are section 11.
