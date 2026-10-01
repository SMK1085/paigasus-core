# SMA-713: `IamOutboxPublishFailures` does not fire on one isolated publish failure

- Linear: SMA-713 (related: SMA-706, whose spec section 4.2 found the defect; SMA-471 added the rule)
- Path: bounded (one rule, one test block, one runbook section, one counter prime in `main` with
  its test)
- Date: 2026-09-27
- Status: **approved by Sven on 2026-09-27** with the decisions of section 3. The draft chose
  option 1. Sven chose option 3 and a prime of the counter in `main`. This version applies those
  decisions and is checked against `origin/main` `4051df5e` (section 12).

## 1. Problem

`ops/observability/prometheus/rules/iam.rules.yml:24-28` holds:

```yaml
- alert: IamOutboxPublishFailures
  expr: increase(iam_outbox_relay_publish_failures_total[5m]) > 0
  for: 5m
```

The repo scrape interval is 15 s (`ops/observability/prometheus/prometheus.yml:3`). The file sets
no `evaluation_interval`, so Prometheus evaluates rules every 1 m (the default).

Let k be the number of scrapes that see an increase in one failure spell. The condition
`increase(...[5m]) > 0` is true from the first step until the pre-step sample of the LAST step
leaves the 5 m window. That is about 4 m 45 s + (k − 1) × 15 s. The `for: 5m` hold must see the
condition true at evaluations that are 5 m apart. Section 2 gives the measured result:

- k ≤ 2 (a failure spell of up to about 30 s): the rule never fires.
- k = 3 to 5 (about 45 to 75 s): the rule fires sometimes. The result depends on the phase of the
  steps against the 1 m evaluation ticks.
- k ≥ 6 (about 90 s or more): the rule always fires.
- When it fires, it fires about 5 to 6 minutes after the first failure.

So the current rule catches a broker outage of more than about 1 to 1.5 minutes. It never fires
on one isolated step.

A failed row is retried on every `TickMode::All` poll tick, which is every 5 s at the default
`poll_interval_secs`. `TickMode::Fresh` does not retry it, because it selects only
`attempts = 0` (`relay.rs:162-163`). So a real outage gives one step at almost every scrape. In
production, "one isolated failure" means that the broker recovered within about one scrape
interval.

Three texts say something different from this behaviour:

- the rule comment, `iam.rules.yml:20-23`: publish failures are "the earliest signal", and
  "`increase()` can fire on it";
- the runbook, `docs/ops/RUNBOOK-observability.md:518-525`: the alert is "deliberately the
  **earliest** outbox signal" and "can fire on the very first failure";
- the runbook alert table, `RUNBOOK-observability.md:222`, which copies the expression.

The runbook also says that the alert fires "well before `IamOutboxEventsParked`". Parking needs
`max_attempts × poll_interval_secs`, about 5 minutes at the defaults (SMA-471 D9,
`rs/crates/services/paigasus-iam/src/config.rs:444-452`). The current rule fires about 5 to 6
minutes after onset. So it is not earlier than parking, and this claim is false too.

The promtool block `ops/observability/prometheus/rules/tests/iam.test.yml:50-66` uses a counter
that increases at every sample. So it cannot see the defect.

The path to "the rule, the runbook and the test agree" is a decision about which failures must
alert. Section 3 records that decision.

## 2. Measured facts (2026-09-27, promtool 3.13.1)

- `origin/main` is `4051df5e`. `git log origin/main --grep=SMA-713` finds nothing. The rule is
  unchanged. The issue is not fixed. No commit between `ad1c44b7` (the spec draft base) and
  `4051df5e` changes a file that this spec names (section 12).
- The counter is incremented on each tick. `relay.rs:232` calls `increment(report.failures)` on
  every tick that gets to that line, also with 0. The tick returns `Err` before that line when
  the row query, `active.update` or `txn.commit` fails (`relay.rs:170`, `:214`, `:217`). The
  failures of such a tick are never counted.
- `main` does NOT prime this counter today. The only primes in `main` are the JIT defect series
  and `iam_outbox_notifying_enqueues_total` (`main.rs:92-108`). So the publish-failure series
  first appears at the first relay tick that gets to `relay.rs:232`. That tick runs only after
  the migration and after `AppState::new` (`main.rs:475-499`).
- All promtool runs used the real binary
  (`~/.proto/tools/promtool/3.13.1/prometheus-3.13.1.darwin-arm64/promtool`) in the session
  scratchpad. The shim `~/.proto/shims/promtool` fails outside the repo with
  `proto::tool::unknown_id`, because the plugin is registered only in the repo's `.prototools`.
- **The old rule, 15 s interval, 1 m evaluation** (`evaluation_interval: 1m`, probe at each
  minute 0-40, a counter `0` for 10 m, then k steps of 1, then flat):

  | Onset (first step) | k = 1 | k = 2 | k = 3 | k = 4 | k = 5 | k = 6 | k = 8 |
  |---|---|---|---|---|---|---|---|
  | 10m00s | never | never | 15m | 15m | 15m | 15m | 15m-16m |
  | 10m15s | | | never | never | 16m | | |
  | 10m30s | | | never | 16m | 16m | | |
  | 10m45s | | | 16m | 16m | 16m | | |

- **The chosen rule** (`[2m]`, `for: 2m`, same set-up): k = 1 and k = 2 never fire. k = 3 to 5
  fire at 13m with onset 10m45s and never with onset 10m00s. k = 6 fires at 12m. k = 12 fires at
  12m-14m.
- The Block of section 4.2, run against the real rule shape and re-measured in this revision
  (`scratchpad/sma713/opt3/`), gives the mutation table of section 5.2.

## 3. Decision

**Sven chose option 3 on 2026-09-27:** `increase(iam_outbox_relay_publish_failures_total[2m]) > 0`
with `for: 2m`. One isolated failure does NOT fire the alert. A failure spell of about 90 s or more
always fires it, about 2 to 3 minutes after onset. That is earlier than parking (about 5 minutes).

The approval decisions:

1. **Rule: option 3** (`[2m]`, `for: 2m`). Section 4.1.
2. **Prime the counter at zero in `main`: yes, in this PR.** One line, gated on
   `config.outbox.relay_enabled`, in the pattern of the `wake_on_commit` prime at
   `main.rs:106-108`. A test must fail when the prime is removed. Section 4.4.
3. **Alertmanager grouping: unknown.** The runbook documents the per-replica alert behaviour.
   Section 4.3.
4. **File the two follow-up issues** of section 8 (team Sven Maschek, project Paigasus Polyglot,
   priority Low), linked to SMA-713.

Facts that support the choice:

1. SMA-471 added the rule as the **earliest** signal that delivery is broken, earlier than
   parking and backlog age. The SMA-471 plan
   (`docs/superpowers/plans/2026-08-07-sma-471-broker-event-publisher.md:1494-1500`) has this
   text and the `for: 5m` hold. Option 3 keeps "earlier than parking". The current rule does not.
2. SMA-471 D9 designed the relay to absorb a routine broker restart with no operator action
   (`config.rs:444-452`, `RUNBOOK-observability.md:360-368`). Option 3 does not alert on the
   short blips that D9 absorbs.
3. Option 3 still catches the outages of about 1.5 to 5 minutes that neither
   `IamOutboxEventsParked` nor `IamOutboxBacklogAgeHigh` catches. Such an outage parks no row, and
   it does not keep the backlog age above 300 s for 5 minutes.

Severity stays `warning`.

### 3.1 Rejected options

- **Option 1: `[15m]`, `for: 0m`** (the `IamOutboxEventsParked` and `IamJitProvisioningFailures`
  shape). It fires on one isolated failure and stays active for about 15 minutes. Rejected: it
  alerts once for each replica on every short broker blip that D9 absorbs with no operator action.
  Parking and a JIT refusal always need action. A publish failure that heals itself does not.
- **Option 2: keep `[5m]`, `for: 5m`** and correct only the texts. Rejected: it fires 5 to 6
  minutes after onset, so it is not earlier than parking. The rule then loses the purpose that
  SMA-471 gave it.

## 4. Design

### 4.1 The rule

```yaml
# SMA-471: with a real broker, publish failures are the earliest signal that delivery is
# broken — earlier than parking (max_attempts × poll_interval, about 5 min) and earlier than
# backlog age. `main` primes the counter at zero when the relay is enabled (SMA-713), and
# relay.rs increments it by 0 on every tick, so `increase()` sees the first failure.
#
# SMA-713. `[2m]` with `for: 2m`, at the repo's 15s scrape interval: a failure spell of about
# 90 s or more fires this alert about 2 to 3 minutes after onset. The 90 s figure needs that
# 15s cadence; at a 1m interval a 90 s spell can give only two true evaluations and not fire.
# One isolated failure does NOT fire it, on purpose: SMA-471 D9
# absorbs a short broker restart with no operator action. The old `[5m]` with `for: 5m` fired
# only 5 to 6 minutes after onset, which is not earlier than parking.
#
# The 2m window must hold at least 2 samples, so this rule needs a scrape interval of 1m or
# less. The repo scrapes every 15s.
- alert: IamOutboxPublishFailures
  expr: increase(iam_outbox_relay_publish_failures_total[2m]) > 0
  for: 2m
  labels: { severity: warning }
  annotations: { summary: "IAM outbox publishes are failing (broker unreachable or rejecting)", description: "The publish-failure counter increased in each 2-minute window for 2 minutes. One failure does not fire this alert. At a 15 s scrape interval, a failure spell of about 90 s or more fires it about 2 to 3 minutes after onset. Each IAM replica gives its own alert. Check iam_nats_connected. See RUNBOOK section 4." }
```

The expression stays per series. It has no `sum by`. The labels `job` and `instance` stay on the
alert, so the operator sees the failing replica. The summary does not change. The
`description` is new. It follows the newer IAM rules (for example `IamJitProvisioningFailures`).

Fire time, measured (section 2): a spell of 6 or more scrapes (about 90 s) always fires, 2 to 3
minutes after onset. A spell of 3 to 5 scrapes fires only for some phases against the 1 m
evaluation tick. A spell of 1 or 2 scrapes never fires. The alert resolves about 2 to 3 minutes
after the last failure.

### 4.2 The promtool block

Replace the block at `iam.test.yml:50-66`. The old block is not kept: it uses a counter that
steps at every sample, so it cannot tell the rules apart.

`interval: 15s` (the repo scrape interval). The file already sets `evaluation_interval: 1m`
(`iam.test.yml:3`), which is the production default. Three series, all `job="iam"`:

- the control: `instance="control:8080"`, values `'0x200'`. It stays flat.
- the single step: `instance="a:8080"`, values `'0x39 1x160'`. Exactly one step `0 → 1` at
  10m00s, then flat (acceptance criterion 2).
- the sustained spell: `instance="s:8080"`, values `'0x39 1+1x11 12x150'`. 12 steps, from 10m00s
  to 12m45s, then flat.

The zero seed is faithful to production because `main` primes the counter at zero (section 4.4)
and the relay increments it by 0 on every tick. The block comment says this.

| # | `eval_time` | Expected |
|---|---|---|
| Q1 | 10m | no alert (the single step and the spell have no hold yet) |
| Q2 | 11m | no alert |
| Q3 | 12m | one alert: `severity=warning, job=iam, instance=s:8080`, the full summary and description. The single step `a:8080` and the control do not fire |
| Q4 | 14m | the same single alert as Q3 |
| Q5 | 15m | no alert |

No residual block. The draft had a Block B that pinned "a failure before the first scrape of a
new series is invisible". Under option 3 one isolated failure never fires anyway. The prime of
section 4.4 also makes the series exist at 0 before the relay starts. So that block pins nothing
that this design depends on.

### 4.3 The runbook

`docs/ops/RUNBOOK-observability.md`:

- Line 222, the alert table: the expression becomes
  `increase(iam_outbox_relay_publish_failures_total[2m]) > 0` for 2m.
- Lines 518-525, "Meaning": replace "increased in the last 5 minutes and stayed increased for
  the `for: 5m` hold" with: the counter increased in each 2-minute window for a 2-minute hold. A
  failure spell of about 90 s or more fires the alert about 2 to 3 minutes after onset. One
  isolated failure does NOT fire it: SMA-471 D9 absorbs a short broker restart with no operator
  action. The alert resolves about 2 to 3 minutes after the last failure. Keep the "earliest
  outbox signal" sentence and the "before `IamOutboxEventsParked`" sentence. They are now true:
  2 to 3 minutes against about 5 minutes. Replace "primed at zero from boot … can fire on the
  very first failure" with: `main` primes the counter at zero when `outbox.relay_enabled` is
  true, and `relay.rs` increments it by 0 on every tick.
- Add the requirement: the rule **needs a scrape interval of 1 m or less** for reliable
  detection. At that interval, the 2 m window holds at least 2 samples at every evaluation. At a
  longer interval, detection is phase-dependent: the alert can stay silent, but it does not always
  stay silent. The repo scrapes every 15 s (`prometheus.yml:3`).
- Add the per-replica behaviour: the rule is per series, so each IAM replica gives its own alert
  with its `instance` label. A broker outage gives one alert for each replica. The repo has no
  Alertmanager routing. If your Alertmanager does not group by `alertname` or `job`, expect one
  notification for each replica.
- Add a list "When the alert is silent":
  - one isolated failure, or a failure spell shorter than about 45 s (by design);
  - a failure spell of about 45 to 75 s, for some phases against the 1 m evaluation tick;
  - a scrape interval longer than 1 m;
  - `outbox.relay_enabled = false` (no relay runs, and `main` does not prime the series);
  - `outbox.publisher.backend = "tracing"` (publish almost never fails);
  - `metrics.enabled = false` (no series);
  - a broker that is down at boot: `NatsEventPublisher::connect` fails (`main.rs:395`),
    `boot_deferred` returns `Err` and the process drains and exits (`main.rs:254-269`). No relay
    tick runs. Look for `CrashLoopBackOff` and the log line
    `boot failed after the listeners were bound`;
  - a tick that returns `Err` before `relay.rs:232` (the row query, `active.update` or
    `txn.commit` fails). The failures of that tick are never counted.
- "Remediation", the "Broker down" item: add one sentence. When this alert fires, the outage
  lasted at least about 45 s, and usually 90 s or more. That is longer than a routine restart
  blip. If `iam_nats_connected` is back at 1 and the counter is flat again, the alert resolves
  about 2 to 3 minutes later.

`docs/ops/RUNBOOK-nats.md:35` and `:66`, and `RUNBOOK-observability.md:300`, `:580`, `:623`
say only that the alert fires. `RUNBOOK-nats.md:35` says it "eventually fires once a tick actually"
fails. The implementer reads each of these lines against option 3. They change only if they
state that one failure fires the alert.

### 4.4 The counter prime in `main`

In `serve()`, inside the `if metrics_handle.is_some()` block (`main.rs:92-110`), next to the
`wake_on_commit` prime at `main.rs:106-108`:

```rust
// SMA-713: the relay's first counter increment happens only after the migration. Prime the
// series at zero before the listener binds, so Prometheus has a zero baseline and
// `increase()` sees the first publish failure. Gated like the relay itself.
if config.outbox.relay_enabled {
    metrics::counter!(names::IAM_OUTBOX_RELAY_PUBLISH_FAILURES_TOTAL).increment(0);
}
```

The prime runs before the bind (`main.rs:127-140`) and before the migration. `/metrics` is live
from the bind (SMA-571). So Prometheus can scrape the 0 while the replica migrates.

**The test.** `serve()` lives in the binary, so a unit test of a helper does not prove the call.
The test goes in `rs/crates/services/paigasus-iam/tests/boot_lifecycle_pg.rs`, the only suite
that spawns the built binary (`spawn_iam`, `boot_lifecycle_pg.rs:133`):

- **T1, positive.** Hold the migration advisory lock, as
  `a_lock_blocked_replica_is_bound_and_reports_migrating` does (`boot_lifecycle_pg.rs:183`).
  While the replica is migrating, GET `/metrics` on the HTTP port. Assert that the body holds the
  sample `iam_outbox_relay_publish_failures_total 0`. No relay tick can run while the migration
  waits for the lock, so only the prime can make this series. The assertion can go into the
  existing test (after the `/readyz` 503 check) to avoid a second container start.
- **T2, negative.** Spawn with `IAM_OUTBOX__RELAY_ENABLED=false` (the env mapping is
  `Env::prefixed("IAM_").split("__")`, `config.rs:998`) and the lock held. Assert that
  `/metrics` does NOT hold `iam_outbox_relay_publish_failures_total`. This pins the gate. If a
  second process start is too slow, the implementer can put T2 in a separate test in the same
  file. The implementer records the choice in the PR.

Parse the sample line exactly (name, a space, `0`). Do not match only the `# TYPE` or `# HELP`
line: `describe_iam_metrics()` writes those lines with no prime, so a `contains()` on the metric
name alone passes with the prime deleted.

`spawn_iam` must also make the child spawn `metrics.enabled = true`. That is the default
(`config.rs:3608`), so no new env var is needed.

The suite is Docker-gated. It prints `skipping boot lifecycle test: Docker unavailable` and
passes when Docker is absent. The implementer runs it with Docker up and records the run.

### 4.5 Rust drift and dashboards

The metric name does not change. `describe_iam_metrics()` already describes it
(`main.rs:721`). `repo:observability-drift` checks metric names only, so it stays green. The
Grafana panel (`ops/observability/grafana/dashboards/iam.json:185-193`) uses `rate()` and does
not change.

## 5. Test strategy

### 5.1 Gates

`moon run repo:promtool` runs `check config`, `check rules` and `test rules` over
`ops/observability/prometheus/rules/tests/*.test.yml` (`moon.yml:560-574`). The new block needs
no registration. `moon run repo:observability-drift` also reads the rules. `paigasus-iam-rs:test`
runs `boot_lifecycle_pg.rs`.

### 5.2 Proof that the promtool assertions bite (acceptance criterion 3)

Apply each mutant to the rule, run `promtool test rules`, and record every failing row. Restore
the rule with an edit, not with `git checkout --`, because a checkout also reverts uncommitted
work. After any later fix, run the whole set again.

Measured on 2026-09-27 in the scratchpad (a copy of the rule alone, the block of 4.2, a stub
`description`):

| Rule under test | Fails at |
|---|---|
| chosen rule (`[2m]`, `for: 2m`) | none: SUCCESS |
| **old rule** (`[5m]`, `for: 5m`) | **Q3, Q4, Q5** |
| `> 0` → `>= 0` | Q1, Q2, Q3, Q4, Q5 (the flat control fires) |
| `sum by (job)` added | Q3, Q4 (no `instance` label) |
| `[2m]` → `[1m]` | Q4 |
| `[2m]` → `[3m]` | Q3, Q5 |
| `for: 2m` → `for: 1m` | Q2 |
| `for: 2m` → `for: 3m` | Q3 |
| `for: 2m` → `for: 0m` | Q1, Q2 |
| option 1 (`[15m]`, `for: 0m`) | Q1, Q2, Q3, Q4, Q5 |

So the block pins the window to exactly `[2m]` and the hold to exactly `for: 2m`. The draft
derived the old-rule row as "Q3, Q4". The measurement gives Q3, Q4 and Q5, because the old rule
fires on the sustained spell at 15m.

The implementer re-runs this table against the real `iam.rules.yml` and the full `iam.test.yml`,
adds a row that deletes the `description` (Q3, Q4 must fail), and puts the result in the PR
description.

### 5.3 Proof that the prime test bites

- Delete the prime line in `main.rs`: T1 must fail.
- Remove the `if config.outbox.relay_enabled` gate (prime always): T2 must fail.
- Invert the gate (`!config.outbox.relay_enabled`): T1 and T2 must fail.

Each mutant must compile. The implementer runs with `--no-fail-fast` and records the failing test
names in the PR. Restore by an edit, then `touch` the file, so cargo does not reuse a stale
binary.

## 6. Files

| File | Change |
|---|---|
| `ops/observability/prometheus/rules/iam.rules.yml` | rule at lines 24-28 and its comment at lines 20-23 |
| `ops/observability/prometheus/rules/tests/iam.test.yml` | replace the block at lines 50-66 with the block of 4.2 |
| `docs/ops/RUNBOOK-observability.md` | table row line 222; the section at lines 516-563 |
| `rs/crates/services/paigasus-iam/src/main.rs` | the gated prime in the block at lines 92-110 |
| `rs/crates/services/paigasus-iam/tests/boot_lifecycle_pg.rs` | T1 and T2 (section 4.4) |

No dashboard, Helm or CI change. There is no other copy of the rule. The SMA-471 plan and
spec copies (`docs/superpowers/plans/2026-08-07-…`, `specs/2026-08-07-…`) are dated history and
do not change. The SMA-706 rule comment at `iam.rules.yml:319-321` says "A hold near the window
length lets one failure resolve before it fires (see SMA-713)". That is still true of
`IamJitProvisioningFailures` and does not change.

## 7. Acceptance criteria

1. The rule and the runbook agree that one isolated publish failure does NOT fire the alert. The
   table row at line 222, the "Meaning" paragraph, the rule comment and the rule `description`
   state `[2m]` and `for: 2m`.
2. `iam.test.yml` holds a block with exactly one step (`0 → 1`, then flat), a sustained spell and
   a flat control series, and asserts Q1-Q5 (section 4.2).
3. A mutation run shows that the new assertions fail against the old rule. The PR description
   records it with the other rows of section 5.2.
4. `main` primes `iam_outbox_relay_publish_failures_total` at zero, gated on
   `config.outbox.relay_enabled`. T1 and T2 pass, and the mutants of section 5.3 fail them.
5. The runbook states the 1 m scrape-interval requirement, the per-replica alert behaviour, and
   the "When the alert is silent" list of section 4.3.
6. `moon run repo:promtool`, `moon run repo:observability-drift` and `moon run
   paigasus-iam-rs:test` pass. The full gate graph from the root `CLAUDE.md` passes before the
   push.

## 8. Out of scope

- No aggregation change (`sum by (job, instance)`). The per-series form already gives one alert
  per replica.
- No change to `IamOutboxEventsParked`, `IamOutboxBacklogAgeHigh` or other rules.
- No Alertmanager routing. The repo has none. The grouping of the operator's Alertmanager is
  unknown (decision 3); the runbook documents the per-replica behaviour.
- Follow-up issues, filed on 2026-09-27 and linked to SMA-713 (decision 4):
  - (a) SMA-720: a gate that compares the `expr` and `for` of each runbook alert-table row with the rule.
    `rs/crates/libs/paigasus-observability/tests/drift.rs` checks only metric names, and this
    gap let the defect ship. Milestone CI & Tooling, labels `area:ci`, `area:observability`,
    Improvement.
  - (b) SMA-721: `RUNBOOK-observability.md:565-573` says that NATS connect runs "before any listener is
    spawned", with "no port bound". Since SMA-571 the listeners bind first (`main.rs:127-140`,
    the log line at `:237`; the comment at `:378-387`). The runbook section is out of date.
    Milestone IAM Gaps, labels `area:docs`, `area:observability`, Bug.

## 9. Residual risk

- **A short failure spell does not alert, by design.** A spell under about 45 s never fires. A
  spell of about 45 to 75 s fires only for some phases. The operator sees these spells only in
  the counter, the relay log (`outbox event publish failed; will retry`) and
  `event_outbox.last_error`. If such a spell parks a row, `IamOutboxEventsParked` fires.
- **The rule needs a scrape interval of 1 m or less.** At a longer interval, detection is
  phase-dependent, and the alert can be silent.
  Nothing gates the scrape interval of an operator's Prometheus. The runbook states the
  requirement.
- **A broker that is down at boot does not fire this alert.** `NatsEventPublisher::connect`
  fails, `boot_deferred` returns `Err`, and the process drains and exits
  (`main.rs:254-269`, `:395`). No relay tick runs. The operator sees a crash loop, not this
  alert.
- **A tick that returns `Err` before `relay.rs:232` is not counted** (section 2).
- **Per-replica alerts.** An outage gives one alert for each replica. The noise cost depends on
  the operator's Alertmanager grouping, which is unknown.
- **The first scrape of a new series.** With the prime, the series exists at 0 from before the
  bind. Prometheus probably scrapes that 0 during the migration. The service cannot control when
  Prometheus scrapes, so a very fast boot can still show the first scraped value above 0. Under
  option 3 this matters only for the first 2-minute window of a spell, and a sustained spell
  still fires.

## 10. Open questions

All answered by Sven on 2026-09-27.

1. **Which option: 1, 2 or 3?** ANSWERED: option 3 (`[2m]`, `for: 2m`). Section 3.
2. **May this PR include a one-line prime of the counter in `main`?** ANSWERED: yes, in this PR,
   gated on `config.outbox.relay_enabled`, with a test that fails when the prime is removed.
   Section 4.4.
3. **Does the operator's Alertmanager group alerts by `alertname` or `job`?** ANSWERED: unknown.
   The runbook documents the per-replica behaviour. Section 4.3.
4. **Must the two follow-up issues of section 8 be filed?** ANSWERED: yes, both, team Sven
   Maschek, project Paigasus Polyglot, priority Low, linked to SMA-713. Section 8.

## 11. Challenge changelog

Verdict of the spec-challenger: **APPROVE WITH CHANGES**.

Each finding was checked against the repo code and, where it made a timing claim, measured
again with promtool 3.13.1 in the scratchpad (`scratchpad/sma713/challenge/`).

Folded:

- BLOCKER, the firing condition of the current rule: confirmed. `prometheus.yml` sets no
  `evaluation_interval`. The old rule never fires for k ≤ 2, fires sometimes for k = 3 to 5, and
  always fires for k ≥ 6, 5 to 6 minutes after onset (section 2 table).
- MAJOR, a third option: added as option 3, with a measured row set and its own mutation band.
  Sven chose it.
- MAJOR, the residual analysis: confirmed. `boot_deferred` returns `Err` on a connect failure and
  the process exits (`main.rs:254-269`, `:395`). The relay starts after the migration
  (`main.rs:475-499`). Section 9 and the "When the alert is silent" list record it.
- MINOR, no fixture for the residual: a Block B was added for option 1. It is removed in this
  version, because option 3 and the prime make it irrelevant (section 4.2).
- MINOR, the value range, the citation, the `description` annotation, the fire-time wording:
  folded.
- MINOR, no drift gate between the runbook table and the rules, and the out-of-date runbook boot
  section: follow-up issues (section 8).

Rejected: none.

## 12. Approval changes and check against `origin/main` (2026-09-27)

Changes from the approval decisions:

- Sections 3, 4, 5.2, 6, 7 and 10 are rewritten for option 3. Option 1 was the design; it is now
  a rejected option (section 3.1).
- Section 4.4 is new: the counter prime in `main` and its tests T1 and T2. Section 5.3 is new.
- The runbook section (4.3) adds the 1 m scrape-interval requirement, the per-replica behaviour,
  and a "When the alert is silent" list for option 3.
- Block B (the new-series residual block) is removed (section 4.2).
- The section 5.2 table is re-measured for option 3 against the real rule shape. The old-rule row
  is Q3, Q4, Q5, not the derived Q3, Q4. New rows: `>= 0`, `sum by (job)`, option 1.
- Section 8 records the two follow-up issues as filed (SMA-720, SMA-721), not proposed.

Checked against `origin/main` `4051df5e` (the draft used `ad1c44b7`). The commits between them
are SMA-705 (TS console readiness) and SMA-695 (Helm chart Ingress). Neither touches the rule,
the test, the runbook, `relay.rs`, `main.rs` or `config.rs`. The chart has no Prometheus rules
and no scrape config. Stale references found and fixed:

- "the listeners bind first (`main.rs:237`)": line 237 is the log line
  `paigasus-iam listeners bound; migrating`. The binds are at `main.rs:127-140`. Both are now
  cited.
- "the comment at `main.rs:380-387`": the comment block is `main.rs:378-391`. Cited as `:378-387`.
- "the prime pattern at `main.rs:94-108`": the metrics block is `main.rs:92-110`, and the
  `wake_on_commit` gated prime is at `:106-108`. Cited exactly now.
- `config.rs:448-452` (D9): the doc comment is `config.rs:444-452`.
- `RUNBOOK-observability.md:518-524` ("Meaning"): the paragraph ends at line 525. The whole
  section is lines 516-563 (the draft said 516-555).
- `RUNBOOK-observability.md:360-365` (D9 text): the paragraph runs to line 368.
- The out-of-date runbook boot section starts at line 565 (heading) with the text at 567-573.
- `moon.yml:560-575`: the `promtool` task is lines 560-574.
- Unchanged and confirmed: `iam.rules.yml:20-28`, `iam.test.yml:50-66`, `prometheus.yml:3`,
  `RUNBOOK-observability.md:222`, `relay.rs:162-163`, `:170`, `:214`, `:217`, `:232`,
  `main.rs:254-269`, `:395`, `:475-499`, the SMA-471 plan lines 1494-1500.
