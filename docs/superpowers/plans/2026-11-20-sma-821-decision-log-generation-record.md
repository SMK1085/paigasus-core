<!-- moon-diagnosis:ok -->
# SMA-821 Decision Log Generation Record Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A relay that starts after a Redis reset, or after Redis went back to an older state, counts the loss
exactly once over all relays, with the last stored `XLEN` sample as the estimate.

**Architecture:** Redis holds a generation id and an epoch next to the decision stream. A NATS KV bucket
(`GATEWAY_DECISIONS_GEN`) holds the last generation, epoch and sample. A KV step beside the sampler reads the
KV entry, runs one Redis detection script, calls the pure function `decide`, and writes at most once with a
compare-and-set. `NOGROUP` only makes the group again; it no longer counts.

**Tech Stack:** Rust 2024, `redis` 1 (Lua scripts), `async-nats` 0.53 (`jetstream` + `kv`), `metrics`,
`testcontainers` (Redis 7.4, NATS), promtool, nsc and the `nats` CLI.

**Spec:** `docs/superpowers/specs/2026-11-20-sma-821-decision-log-generation-record-design.md`

## Global Constraints

- Rust edition 2024, rust-version 1.95. `cargo clippy --workspace -- -D warnings` and `cargo fmt --check` pass.
  The workspace lints deny warnings, so dead code is a compile error.
- Every new source file starts with `// SPDX-License-Identifier: Apache-2.0` (`#` for shell and YAML).
- No new crate. Put the `kv` feature on the gateway's own `async-nats` line, NOT on the workspace line
  (`rs/Cargo.toml:298`). `Cargo.lock` must not change. UUIDs come from `uuid::Builder::from_random_bytes(rand::random())`
  (the workspace `uuid` has no `v4` feature, `rs/Cargo.toml:76-81`).
- Bucket `GATEWAY_DECISIONS_GEN`, stream `KV_GATEWAY_DECISIONS_GEN`: history 1, file storage, replicas 1, no TTL,
  `allow_direct = true`, `max_bytes` = 1 MiB (1 048 576).
- Redis keys: `<prefix>:decisions:gen` (a UUID v4 string) and `<prefix>:decisions:epoch` (an integer). No hash tag
  (the workspace `redis` crate and `paigasus-redis` have no cluster feature; checked).
- KV value JSON: `{"generation": "<id>", "epoch": <u64>, "sample": <u64 or null>, "sampled_at": <unix ms>,
  "claim": <null or {"nonce": "<id>", "lost": <u64 or null>}>}`.
- `generation_check_errors_total{kind}`: `kind` is exactly `timeout`, `permission`, `no_bucket`, `redis`,
  `foreign_writer`, `other`, each primed at 0.
- A KV failure never stops or slows a publish, an ack, a write or the backlog gauge. The KV step is single-flight
  and has one timeout (5 s in production).
- Logs name error kinds and classes only. They never hold an event, a payload, a URL or a KV value.
- Redis commands in tests run through `redis-cli` in the container (`repo:redis-connect-single-site`).
- Tests run from `rs/` with `cargo nextest run`. A FILTERED Docker run sets `PAIGASUS_REQUIRE_DOCKER=1`, so a
  skip is a failure. Log assertions use `paigasus_logging::test_support`.
- "Exactly 1" in a test means: wait for the count, then wait for at least 3 more KV steps, then assert 1.
- Proof rule: for each new counting behaviour, a step deletes or disables the feature line, shows the named tests
  red, then restores it. The mutation must COMPILE under the workspace lints; run with `--no-fail-fast` and read
  the output to confirm that tests ran (not a compile error). Restore by deleting your marked change, not with
  `git checkout --` (that also reverts the fix under test).
- Commits: conventional, with a workspace scope (`feat(rs): …`, `docs(ops): …`). The message ends with
  `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. No `#NNN` line in a commit body.
- Shell: prefix tool commands with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.
  `ops/nats/check-subjects.sh` needs bash >= 4.3: run it with `/opt/homebrew/bin/bash`.

## Review Focus

- A Redis at its memory limit refuses the `SET NX` and `INCR` of the detection script. Expect a `kind="redis"`
  error, a backlog gauge that still updates, and no count. Test: Task 9, step 9.
- A KV value that does not parse (an operator `nats kv put`). Expect an `update` with its revision, no count and
  no error on the next step. Test: Task 5, step 1.
- A KV delete or purge marker (an operator `nats kv del`). Expect a new entry and no count. Test: Task 6, step 1.
- A relay shutdown while a KV step hangs. Expect the relay to end within the drain budget plus the KV timeout.
  Test: Task 8, step 1.
- Two fresh relays on a first deploy create the entry at the same moment. Expect one create to win, the other to
  see `AlreadyExists` as a lost CAS, no error and no count. Test: Task 9, step 10.

---

## File map

- Create `rs/crates/services/paigasus-gateway/src/adapters/decision_log/generation.rs`: the pure `decide`, the
  value types, the KV port trait, and the KV step (`Generation::step`). No Redis, no NATS.
- Create `rs/crates/services/paigasus-gateway/src/adapters/decision_log/gen_kv.rs`: the NATS KV adapter of the port.
- Create `rs/crates/services/paigasus-gateway/src/adapters/decision_log/gen_redis.rs`: the three Lua scripts (group,
  sample, detection) and their calls.
- Modify `relay.rs`: the group script replaces `create_group`; the sampler runs the KV step; the NOGROUP count goes.
- Modify `jetstream.rs`: the two violation counters; the bucket constants.
- Modify `mod.rs`, `runtime.rs`, `tests/decision_log_redis.rs`, the observability names, the rules, the NATS files,
  the IAM permission test, and the docs.

---

### Task 1: Metric names, priming and alerts

**Files:**
- Modify: `rs/crates/libs/paigasus-observability/src/names.rs:93-100` (and `ALL` at ~375-383, the test at ~486-498)
- Modify: `rs/crates/services/paigasus-gateway/src/adapters/decision_log/mod.rs:79-92` (`prime_decision_log_metrics`)
  and its test `prime_primes_every_reason_at_zero` (~211-233)
- Modify: `ops/observability/prometheus/rules/gateway.rules.yml:42-46`
- Test: `ops/observability/prometheus/rules/tests/gateway.test.yml:187-212`

**Interfaces:**
- Produces: `names::GATEWAY_DECISION_LOG_GENERATION_CHECK_ERRORS_TOTAL = "gateway_decision_log_generation_check_errors_total"`,
  `names::GATEWAY_DECISION_LOG_LAST_RESET_TIMESTAMP_SECONDS = "gateway_decision_log_last_reset_timestamp_seconds"`.
- Produces: `mod.rs` constant `pub const CHECK_ERROR_KINDS: [&str; 6] = ["timeout", "permission", "no_bucket", "redis", "foreign_writer", "other"];`
  Task 4 makes `CheckErrorKind::as_label` return exactly these strings, and a Task 4 test pins the two lists equal.

- [ ] **Step 1: Write the failing tests**

In `names.rs` `the_gateway_decision_log_families_are_registered`, add both new names to the checked list. In
`mod.rs` `prime_primes_every_reason_at_zero`, add:

```rust
for kind in CHECK_ERROR_KINDS {
    assert_eq!(snapshot_counter(&snap, names::GATEWAY_DECISION_LOG_GENERATION_CHECK_ERRORS_TOTAL, &[("kind", kind)]), Some(0), "{kind}");
}
assert_eq!(snap.len(), 9 + 2 + 2 + 3 + 6, "the last-reset gauge is not primed");
```

In `gateway.test.yml`, change the expected annotations of the existing `GatewayDecisionLogRedisReset` case to the
new text (step 3), and add three cases:

```yaml
  # GatewayDecisionLogRedisReset: a fresh pod counts at its first tick, so the counter series starts at 1 and
  # increase() reads 0. The last-reset gauge fires the alert (SMA-821 § 5.3a).
  - interval: 1m
    input_series:
      - series: 'gateway_decision_log_redis_resets_total'
        values: '_ _ _ 1 1 1 1'
      - series: 'gateway_decision_log_last_reset_timestamp_seconds'
        values: '_ _ _ 180 180 180 180'
    alert_rule_test:
      - eval_time: 2m
        alertname: GatewayDecisionLogRedisReset
        exp_alerts: []
      - eval_time: 4m
        alertname: GatewayDecisionLogRedisReset
        exp_alerts:
          - exp_labels: { severity: warning }
            exp_annotations: { summary: "...", description: "..." }  # the exact copy of step 3
  # GatewayDecisionLogRedisReset quiet: the last reset is older than 900 s.
  - interval: 1m
    input_series:
      - series: 'gateway_decision_log_redis_resets_total'
        values: '1x25'
      - series: 'gateway_decision_log_last_reset_timestamp_seconds'
        values: '60x25'
    alert_rule_test:
      - eval_time: 20m
        alertname: GatewayDecisionLogRedisReset
        exp_alerts: []
  # GatewayDecisionLogGenerationCheckFailing: errors for 15m fire; a flat series is quiet.
```

For `GatewayDecisionLogGenerationCheckFailing` add one firing case (`kind="timeout"` values `0+1x40`, no alert at
10m, an alert at 32m) and one quiet case (`0x40`, no alert at 32m).

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd rs && cargo nextest run -p paigasus-observability -p paigasus-gateway --lib -E 'test(registered) | test(prime_primes)'`
Expected: FAIL (the names and `CHECK_ERROR_KINDS` do not exist).
Run: `moon run repo:promtool`
Expected: FAIL (the new cases do not match the rules).

- [ ] **Step 3: Implement**

- `names.rs`: add the two constants after `GATEWAY_DECISION_LOG_BACKLOG`, with doc comments; add both to `ALL`.
  Change the doc of `GATEWAY_DECISION_LOG_REDIS_RESETS_TOTAL` to: "Losses of entries of the Redis decision stream
  that the relays detected: a lost stream, or a return to an older state (SMA-821). One per incident over all
  relays." Change the doc of `..._LOST_ESTIMATE_TOTAL` to say the estimate is the last stored `XLEN` sample.
- `mod.rs`: add `CHECK_ERROR_KINDS`; prime `GENERATION_CHECK_ERRORS_TOTAL` at 0 for each kind. Do not prime the
  gauge.
- `gateway.rules.yml`: `GatewayDecisionLogRedisReset` gets
  `expr: sum(increase(gateway_decision_log_redis_resets_total[15m])) > 0 or max(gateway_decision_log_last_reset_timestamp_seconds) > time() - 900`
  and this copy:
  - summary: `Gateway decision log lost entries of its Redis stream`
  - description: `The relay found that the Redis decision stream lost entries: Redis lost the stream, or went back to an older state. Events in the stream were lost. See RUNBOOK-observability.md, section "Gateway decision log".`
- Add `GatewayDecisionLogGenerationCheckFailing`: `expr: sum(increase(gateway_decision_log_generation_check_errors_total[15m])) > 0`,
  `for: 15m`, `severity: warning`, with this copy:
  - summary: `Gateway decision-log Redis reset check is failing`
  - description: `The relay could not run the Redis reset check against the NATS KV bucket GATEWAY_DECISIONS_GEN for 15m. While it fails, a Redis reset is not counted. Read gateway_decision_log_generation_check_errors_total by kind. See RUNBOOK-observability.md, section "Gateway decision log".`

No annotation prints `$value`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: the two commands of step 2, then `moon run repo:observability-drift`.
Expected: PASS.

- [ ] **Step 5: Prove the alert branch bites**

Delete ` or max(gateway_decision_log_last_reset_timestamp_seconds) > time() - 900` from the rule. Run
`moon run repo:promtool --force`. Expected: FAIL in the "starts at 1" case only. Put the branch back and run it
again. Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add rs/crates/libs/paigasus-observability/src/names.rs rs/crates/services/paigasus-gateway/src/adapters/decision_log/mod.rs ops/observability/prometheus/rules/gateway.rules.yml ops/observability/prometheus/rules/tests/gateway.test.yml
git commit -m "feat(rs): add the decision log generation check metrics and alerts (SMA-821)"
```

---

### Task 2: NATS grants and the bucket, measured

**Files:**
- Modify: `ops/nats/subjects.env` (the decision-log block and `GW_DEC_PUBLISHER_PUB`)
- Modify: `ops/nats/test/accounts.conf.tmpl:57-61`
- Modify: `ops/nats/provision-decisions.sh`
- Modify: `rs/crates/services/paigasus-iam/Cargo.toml` (`[dev-dependencies]`)
- Modify: `rs/crates/services/paigasus-gateway/src/adapters/decision_log/jetstream.rs` (constants and the test
  `the_stream_config_matches_subjects_env`)
- Modify: `ops/nats/permissions.md` §11, `ops/nats/README.md`
- Test: `rs/crates/services/paigasus-iam/tests/nats_permissions.rs` (after line ~909)

**Interfaces:**
- Produces in `jetstream.rs`: `pub const GEN_BUCKET: &str = "GATEWAY_DECISIONS_GEN";`,
  `pub const GEN_MAX_BYTES: i64 = 1_048_576;`, and
  `pub fn gen_kv_config() -> jetstream::kv::Config` (bucket `GEN_BUCKET`, history 1, `StorageType::File`,
  `num_replicas` 1, `max_age` zero, `max_bytes` `GEN_MAX_BYTES`, the rest default). `gen_kv_config` needs the `kv`
  feature, so put `async-nats = { workspace = true, features = ["kv"] }` on the gateway's line in this task.
- Produces in `subjects.env`: `DECISIONS_GEN_BUCKET="GATEWAY_DECISIONS_GEN"`, `DECISIONS_GEN_MAX_BYTES=1048576`.

- [ ] **Step 1: Write the failing permission tests**

In `nats_permissions.rs`, add `gw_gen_config()` (a literal copy of the values above, like `gw_stream_config`) and:

```rust
#[tokio::test]
async fn the_decision_publisher_uses_the_generation_bucket() {
    // The calls of the relay, in its order: get the bucket, make it on a 404, read, create, update.
    let js = /* the gw_publisher client, as in the_decision_publisher_ensures_the_stream_... */;
    assert!(js.get_key_value("GATEWAY_DECISIONS_GEN").await.is_err(), "no bucket yet");
    let kv = js.create_key_value(gw_gen_config()).await.expect("STREAM.CREATE.KV_… and $JS.API.INFO");
    let kv = js.get_key_value("GATEWAY_DECISIONS_GEN").await.expect("STREAM.INFO.KV_…");
    assert_eq!(kv.entry("k1").await.expect("direct get of a missing key"), None);
    let rev = kv.create("k1", "a".into()).await.expect("$KV.GATEWAY_DECISIONS_GEN.>");
    let rev2 = kv.update("k1", "b".into(), rev).await.expect("CAS update");
    assert!(kv.update("k1", "c".into(), rev).await.is_err(), "a stale revision fails");
    assert_eq!(kv.entry("k1").await.expect("direct get").map(|e| e.revision), Some(rev2));
}

#[tokio::test]
async fn the_decision_publisher_cannot_reach_beyond_the_generation_bucket() {
    // expect_publish_denied for each:
    // "$KV.OTHER.k", "$JS.API.STREAM.DELETE.KV_GATEWAY_DECISIONS_GEN", "$JS.API.STREAM.PURGE.KV_GATEWAY_DECISIONS_GEN",
    // "$JS.API.STREAM.MSG.GET.KV_GATEWAY_DECISIONS_GEN", "$JS.API.DIRECT.GET.GATEWAY_DECISIONS",
    // "$JS.API.STREAM.MSG.GET.GATEWAY_DECISIONS", "$JS.API.CONSUMER.CREATE.KV_GATEWAY_DECISIONS_GEN"
}
```

Add `async-nats = { workspace = true, features = ["kv"] }` to `paigasus-iam` `[dev-dependencies]` with a comment
(the KV calls of the gateway relay in `tests/nats_permissions.rs`, SMA-821).

- [ ] **Step 2: Run them to verify they fail**

Run: `cd rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test nats_permissions -E 'test(generation_bucket)'`
Expected: `the_decision_publisher_uses_the_generation_bucket` FAILS at `create_key_value`. The other test passes.

- [ ] **Step 3: Add the expected grants**

In `subjects.env`, add the two variables after `DECISIONS_MAX_MSG_SIZE`, and add to `GW_DEC_PUBLISHER_PUB`:

```bash
  '$JS.API.STREAM.INFO.KV_'"$DECISIONS_GEN_BUCKET"
  '$JS.API.STREAM.CREATE.KV_'"$DECISIONS_GEN_BUCKET"
  '$JS.API.DIRECT.GET.KV_'"$DECISIONS_GEN_BUCKET"'.>'
  '$KV.'"$DECISIONS_GEN_BUCKET"'.>'
```

Extend the comment above `GW_DEC_PUBLISHER_PUB`: the KV grants touch only the generation bucket; no read, delete
or purge right on `GATEWAY_DECISIONS`; the direct get is the only read path. Put the four literal subjects in the
publisher line of `accounts.conf.tmpl`.

- [ ] **Step 4: Run the tests and the gate script**

Run: the command of step 2, then `/opt/homebrew/bin/bash ops/nats/check-subjects.sh`.
Expected: PASS, and `accounts.conf.tmpl grants exactly the subjects declared in subjects.env`.

- [ ] **Step 5: Measure each grant (leave-one-out)**

For each of the four new grants: delete it from BOTH `subjects.env` and `accounts.conf.tmpl`, run the step-2
command, and record which call fails. Expected: each removal reds `the_decision_publisher_uses_the_generation_bucket`.
If a removal stays green, that grant is not needed: remove it for good and record that in the commit message.
If the positive test needs a subject that is not in the list, add the subject that the failure names. Restore
after each run, and run check-subjects.sh again.

- [ ] **Step 6: Pin the gateway constants to subjects.env**

In `jetstream.rs`, add `GEN_BUCKET`, `GEN_MAX_BYTES` and `gen_kv_config()`. In `the_stream_config_matches_subjects_env`
add: `assert_eq!(GEN_BUCKET, var(&env, "DECISIONS_GEN_BUCKET"))`, `assert_eq!(GEN_MAX_BYTES.to_string(), var(&env, "DECISIONS_GEN_MAX_BYTES"))`,
and the `gen_kv_config()` values (history 1, file, replicas 1, `max_bytes == GEN_MAX_BYTES`). Then check that
`cargo metadata --locked --format-version 1 >/dev/null` (from `rs/`) exits 0, so `Cargo.lock` did not change.

Run: `cd rs && cargo nextest run -p paigasus-gateway --lib -E 'test(the_stream_config_matches_subjects_env)'`
Expected: PASS.

- [ ] **Step 7: Provision the bucket and the grants of an existing publisher**

In `provision-decisions.sh`:
- After section 1, fail with a message when `DECISIONS_ACCOUNT_DISK_BYTES - DECISIONS_MAX_BYTES < DECISIONS_GEN_MAX_BYTES`
  (the 10 % rest must hold the bucket; the smallest disk is then about 10 MiB).
- In `add_user`, an optional fourth argument `edit`. With it, after `nsc add user`, run
  `nsc edit user --account "$GATEWAY_ACCOUNT" --name "$name" "${args[@]}"`, before `nsc generate creds`. Pass
  `edit` for `gateway-decision-publisher` only (D14). The new creds file holds the new grants.
- In section 3, after the stream: when `nats … kv info "$DECISIONS_GEN_BUCKET"` fails, run
  `nats … kv add "$DECISIONS_GEN_BUCKET" --history 1 --storage file --replicas 1 --max-bytes "$DECISIONS_GEN_MAX_BYTES"`
  with the provisioner creds and inbox prefix. If the `nats` CLI is installed, check the flag names with
  `nats kv add --help`; do not install it.
- Update the header comment: the script also makes the bucket; the account allows 2 streams, and the decision
  stream and `KV_GATEWAY_DECISIONS_GEN` use both.

Run: `/opt/homebrew/bin/bash -n ops/nats/provision-decisions.sh`
Expected: exit 0.

- [ ] **Step 8: Update the NATS documents**

`permissions.md` §11: the four new grants in the publisher row, and bullets for: the reason (the reset record of
SMA-821), the bucket only; no right on `GATEWAY_DECISIONS`; the 2-stream limit (a third stream needs a higher
`--js-streams`); the `nsc edit user` step; the upgrade order (new grants, new creds and the bucket first, then the
new gateway binary); the deployment rule of spec §5.2 (two deployments with their own Redis, or another Redis DB
index, must not share one NATS account AND one Redis prefix; the default prefix `paigasus:gateway` collides).
`README.md`: the `provision-decisions.sh` row names the bucket.

- [ ] **Step 9: Commit**

```bash
git add ops/nats rs/crates/services/paigasus-iam rs/crates/services/paigasus-gateway/Cargo.toml rs/crates/services/paigasus-gateway/src/adapters/decision_log/jetstream.rs
git commit -m "feat(ops): grant the decision publisher the generation KV bucket (SMA-821)"
```

---

### Task 3: Two violation counters

**Files:**
- Modify: `rs/crates/services/paigasus-gateway/src/adapters/decision_log/jetstream.rs:42-52, 256-298`
- Modify: `relay.rs` (`State::violations`, `publish_one`, `run_relay`), `runtime.rs:177-202`
- Modify: `tests/decision_log_redis.rs` (`relay_js`, `Relay::start` and every caller)

**Interfaces:**
- Produces in `jetstream.rs`:
  - `#[derive(Debug, Clone, Default)] pub struct Violations { pub publish: Arc<AtomicU64>, pub kv: Arc<AtomicU64> }`
  - `pub fn violation_subject(text: &str) -> Option<&str>`: the text between the first pair of double quotes.
  - `pub fn route_violation(text: &str, v: &Violations) -> ViolationRoute` with
    `pub enum ViolationRoute { Publish, Kv }`. A quoted subject that starts with `gateway.decision.` increases
    `v.publish`. Any other subject, AND a text with no quoted subject, increases `v.kv` (a KV refusal must never
    delete an event, so an unknown text goes to the side that deletes nothing).
  - `pub async fn connect(cfg: &DecisionLogNatsConfig, violations: Violations) -> Result<jetstream::Context, NatsError>`
  - In `relay.rs`: `pub async fn run_relay(slot, js, s: RelaySettings, violations: Violations, shutdown, budget)`.

- [ ] **Step 1: Write the failing unit tests**

```rust
#[test]
fn a_violation_is_routed_by_its_quoted_subject() {
    let v = Violations::default();
    assert_eq!(route_violation(r#"nats: Permissions Violation for Publish to "gateway.decision.0190a100-0000-7000-8000-0000000000a1""#, &v), ViolationRoute::Publish);
    assert_eq!(route_violation(r#"nats: Permissions Violation for Publish to "$JS.API.STREAM.INFO.KV_GATEWAY_DECISIONS_GEN""#, &v), ViolationRoute::Kv);
    assert_eq!(route_violation(r#"Permissions Violation for Publish to "$KV.GATEWAY_DECISIONS_GEN.ab""#, &v), ViolationRoute::Kv);
    assert_eq!(route_violation("Permissions Violation for Publish to gateway.decision.x", &v), ViolationRoute::Kv, "no quotes: never the publish side");
    assert_eq!(route_violation(r#"Permissions Violation for Publish to "gateway.decisionx""#, &v), ViolationRoute::Kv);
    assert_eq!(v.publish.load(Ordering::SeqCst), 1);
    assert_eq!(v.kv.load(Ordering::SeqCst), 4);
}

#[test]
fn a_kv_violation_does_not_make_a_publish_timeout_permanent() {
    let v = Violations::default();
    let before = v.publish.load(Ordering::SeqCst);
    route_violation(r#"Permissions Violation for Publish to "$KV.GATEWAY_DECISIONS_GEN.ab""#, &v);
    assert_eq!(classify(PublishErrorKind::TimedOut, None, violation_since(&v.publish, before), false), PublishClass::Transient);
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cd rs && cargo nextest run -p paigasus-gateway --lib -E 'test(violation)'`
Expected: FAIL to compile (`Violations` is not defined).

- [ ] **Step 3: Implement**

The event callback calls `route_violation` for each `ServerError` that `is_publish_violation` accepts. It logs the
publish side as today. It logs the KV side at most once per 60 s (keep the last log time in an `AtomicU64` of unix
seconds in the closure), with the text "the decision-log NATS publisher has no permission for the generation
bucket". Change `connect`, `run_relay`, `runtime.rs` and every test caller mechanically: tests use
`let v = Violations::default();`, pass `v.clone()`, and read `v.publish.load(...)` where they read `v.load(...)`
today. The relay's `publish_one` reads `self.violations.publish`. Update the doc comment of `connect`.

- [ ] **Step 4: Run the unit tests and the existing Docker suite**

Run: `cd rs && cargo nextest run -p paigasus-gateway --lib -E 'test(violation) | test(classify)'`
Expected: PASS.
Run: `cd rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-gateway --test decision_log_redis`
Expected: PASS (no behaviour change yet).

- [ ] **Step 5: Prove the routing bites**

In `route_violation`, make the publish branch also take the `Kv` case (route every subject to `v.publish`). Run
step 4's first command with `--no-fail-fast`. Expected: both new tests FAIL. Restore.

- [ ] **Step 6: Commit**

```bash
git add rs/crates/services/paigasus-gateway
git commit -m "feat(rs): split the decision relay NATS violation counter by subject (SMA-821)"
```

---

### Task 4: The pure `decide`

**Files:**
- Create: `rs/crates/services/paigasus-gateway/src/adapters/decision_log/generation.rs`
- Modify: `mod.rs` (`pub mod generation;`)
- Test: unit tests in `generation.rs`

**Interfaces:**
- Consumes: `names::*` and `CHECK_ERROR_KINDS` (Task 1).
- Produces (all `pub` in `generation.rs`):
  - `fn entry_key(stream_key: &str) -> String`: the lower-case hex of the UTF-8 bytes. It is deterministic,
    collision-free and valid for a NATS KV key.
  - `fn new_id() -> String`: a UUID v4 string from `uuid::Builder::from_random_bytes(rand::random()).into_uuid()`.
  - `#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)] struct Claim { nonce: String, lost: Option<u64> }`
  - `#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)] struct GenRecord { generation: String, epoch: u64, sample: Option<u64>, sampled_at: u64, claim: Option<Claim> }`
    with `fn parse(bytes: &[u8]) -> Option<GenRecord>` and `fn to_bytes(&self) -> bytes::Bytes`.
  - `#[derive(Debug, Clone, PartialEq, Eq)] struct LocalTuple { generation: String, epoch: Option<u64>, sample: u64, sampled_at: u64 }`
  - `#[derive(Debug, Clone, Default)] struct LocalRecord` with `fn observe(&mut self, t: LocalTuple)` (same
    generation as `current`: replace it; else `previous = current`, `current = t`) and
    `fn of(&self, generation: &str) -> Option<&LocalTuple>`.
  - `#[derive(Debug, Clone, PartialEq, Eq)] enum RedisView { NoGroup, Live { generation: String, epoch: u64, xlen: u64 } }`
  - `#[derive(Debug, Clone, PartialEq, Eq)] enum KvView { Missing { revision: Option<u64> }, Record { record: GenRecord, revision: u64 } }`
  - `#[derive(Debug, Clone, Copy, PartialEq, Eq)] enum LossKind { Reset, Rewind }`
  - `#[derive(Debug, Clone, PartialEq, Eq)] enum Action { Nothing, Foreign, Write { record: GenRecord, revision: Option<u64>, count: Option<(LossKind, Claim)> } }`.
    `revision: None` means "create"; `Some(r)` means "update with revision r".
  - `fn decide(redis: &RedisView, kv: &KvView, last_own: Option<&str>, local: &LocalRecord, now_ms: u64, nonce: &str) -> Action`
  - `fn resolve_pending(pending: Option<Claim>, kv: &KvView) -> Option<Claim>`: `Some(claim)` only when the KV record
    holds a claim with the same nonce.
  - `fn count_loss(kind: LossKind, lost: Option<u64>, now: std::time::SystemTime)`: resets +1, lost estimate +n
    (or a log that the loss is unknown), and the last-reset gauge set to the unix seconds of `now`. Log texts:
    `Reset` keeps the two SMA-755 texts of `count_reset` (relay.rs:271-273); `Rewind` says "the Redis decision stream
    went back to an older state" with the same two endings.
  - `#[derive(Debug, Clone, Copy, PartialEq, Eq)] enum CheckErrorKind { Timeout, Permission, NoBucket, Redis, ForeignWriter, Other }`
    with `const ALL: [CheckErrorKind; 6]`, `fn as_label(self) -> &'static str`, and `fn count_check_error(kind: CheckErrorKind)`.

**The rules of `decide`** (spec §5.3 table, §5.3b, D13):

| `redis` | `kv` | Action |
|---|---|---|
| `NoGroup` | any | `Nothing` |
| `Live{R,E,x}` | `Missing{rev}` | `Write{ {R,E,Some(x),now,None}, rev, None }` |
| `Live{R,E,x}` | `Record{{R,e,..,claim},rev}`, `E > e` | `Write{ {R,E,Some(x),now,claim}, Some(rev), None }` (claim carried unchanged) |
| `Live{R,E,x}` | `Record{{R,e,s,..},rev}`, `E <= e` | `Write{ {R,E,Some(x),now,Some(c)}, Some(rev), Some((Rewind, c)) }`, `c = {nonce, lost: s}` |
| `Live{R,..}` | `Record{{K,..},..}`, `K != R`, `last_own == Some(R)` | `Foreign` |
| `Live{R,E,x}` | `Record{{K,e,s,t,..},rev}`, `K != R`, other | `Write{ {R,E,Some(x),now,Some(c)}, Some(rev), Some((Reset, c)) }`, `c.lost` = the sample of the newer of `(s, t)` and `local.of(K)` by `sampled_at` |

No write takes a maximum or carries a value of the old generation, except the claim on an `E > e` write.

- [ ] **Step 1: Write the failing unit tests**

One test per row, plus these (T14 and D13):

```rust
#[test]
fn an_epoch_above_the_stored_one_writes_and_counts_nothing() { /* E=5, e=4: Write, count None, claim carried */ }
#[test]
fn an_epoch_that_went_back_counts_a_loss_with_the_stored_sample() {
    // Record {R, e=10, s=Some(7)}, local.of(R) holds sample 0 (the XLEN after the loss): lost == Some(7).
}
#[test]
fn an_equal_epoch_counts_a_loss() { /* E == e */ }
#[test]
fn a_new_generation_stores_the_epoch_of_redis_as_it_is() {
    // Record {K, e=50}, Redis {R, E=1}: the write holds epoch 1. A second decide on {R, E=2} and that record counts nothing.
}
#[test]
fn a_new_generation_counts_the_newer_of_the_kv_and_the_local_sample() {
    // KV {K, s=Some(0), t=1_000}, local.of(K) {sample 3, t=2_000}: lost == Some(3).
    // KV {K, s=Some(5), t=3_000}, local.of(K) {sample 3, t=2_000}: lost == Some(5).
    // KV {K, s=None}, no local: lost == None.
}
#[test]
fn an_unchanged_own_generation_with_a_foreign_entry_is_a_foreign_writer() { /* last_own == Some(R) */ }
#[test]
fn a_changed_own_generation_with_another_entry_is_a_reset() { /* last_own == Some(OLD) != R */ }
#[test]
fn no_group_writes_nothing() {}
#[test]
fn a_pending_claim_counts_only_when_the_entry_holds_its_nonce() {}
#[test]
fn the_entry_key_is_hex_and_collision_free() {
    assert_eq!(entry_key("paigasus:gateway:decisions"), hex_of("paigasus:gateway:decisions"));
    assert_ne!(entry_key("a:b"), entry_key("a.b"));
    assert!(entry_key("x:y").chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
}
#[test]
fn the_record_round_trips_and_garbage_does_not_parse() {
    // The JSON field names are exactly generation, epoch, sample, sampled_at, claim, nonce, lost.
}
#[test]
fn count_loss_sets_the_counters_and_the_gauge() {
    // DebuggingRecorder + metrics::with_local_recorder: resets 1, lost 3, gauge == now in unix seconds.
}
#[test]
fn the_check_error_labels_match_the_primed_kinds() {
    assert_eq!(CheckErrorKind::ALL.map(CheckErrorKind::as_label), super::super::CHECK_ERROR_KINDS);
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cd rs && cargo nextest run -p paigasus-gateway --lib -E 'test(/generation::/)'`
Expected: FAIL to compile.

- [ ] **Step 3: Implement the types and functions above**

`serde` and `serde_json` are already gateway dependencies. `KvView` gets no `from_entry` yet; Task 5 adds it.

- [ ] **Step 4: Run the tests to verify they pass**

Run: the command of step 2, and `cd rs && cargo clippy -p paigasus-gateway --all-targets -- -D warnings`.
Expected: PASS.

- [ ] **Step 5: Prove each counting branch bites**

Mutation A: replace the guard of the `K != R` counting row with `false`. Mutation B: replace the `E <= e` guard
with `false`. rustc accepts both under the workspace lints (clippy is not part of the test run). Run step 2's
command with `--no-fail-fast` after each. Expected: A reds the `K != R` tests; B reds `an_epoch_that_went_back…` and
`an_equal_epoch…`. Restore each.

- [ ] **Step 6: Commit**

```bash
git add rs/crates/services/paigasus-gateway/src/adapters/decision_log
git commit -m "feat(rs): add the pure decision log generation decide (SMA-821)"
```

---

### Task 5: The KV step and its port

**Files:**
- Modify: `rs/crates/services/paigasus-gateway/src/adapters/decision_log/generation.rs`
- Test: unit tests in `generation.rs` (fake port)

**Interfaces:**
- Consumes: Task 4.
- Produces:
  - `#[derive(Debug, Clone)] pub struct KvEntry { pub value: bytes::Bytes, pub revision: u64, pub live: bool }`
    (`live` is false for a delete or purge marker).
  - `#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum KvError { WrongRevision, Timeout, NoBucket, Other }`
    (`WrongRevision` also covers a create that finds the key, `AlreadyExists`).
  - `pub trait GenKv: Send + Sync { fn get(&self) -> impl Future<Output = Result<Option<KvEntry>, KvError>> + Send;
    fn create(&self, value: Bytes) -> impl Future<Output = Result<u64, KvError>> + Send;
    fn update(&self, value: Bytes, revision: u64) -> impl Future<Output = Result<u64, KvError>> + Send; }`
  - `impl KvView { pub fn from_entry(e: Option<KvEntry>) -> KvView }`: `None` gives `Missing{None}`; a marker or a
    value that does not parse gives `Missing{Some(revision)}`.
  - `#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum StepOutcome { NoGroup, Wrote, Counted(LossKind), CasLost, Foreign, Failed(CheckErrorKind) }`
  - `pub struct Generation` (one per relay) with `pub fn new() -> Self`, `pub fn observe(&self, t: LocalTuple)`, and
    `pub async fn step<K, F, Fut>(&self, kv: &K, detect: F, kv_violations: &AtomicU64, timeout: Duration) -> StepOutcome
    where K: GenKv, F: FnOnce() -> Fut + Send, Fut: Future<Output = Result<RedisView, redis::RedisError>> + Send`.
    Its state is a `std::sync::Mutex<GenState>` with `pending: Option<Claim>`, `last_own: Option<String>`,
    `local: LocalRecord`. Never hold the lock across an await.

**The order inside `step`** (D17), all inside one `tokio::time::timeout(timeout, …)`:
1. Read `kv_violations` as `before`. `kv.get()`. On error, go to "error".
2. `view = KvView::from_entry(..)`. `resolve_pending(state.pending.take(), &view)`; on `Some(c)`, `count_loss`
   with the kind stored with the pending claim (store `(LossKind, Claim)` as pending).
3. `detect().await`. A Redis error is `Failed(Redis)`.
4. `decide(..)` with `new_id()` as the nonce and the unix ms of now.
5. `Nothing` gives `NoGroup`. `Foreign` gives `count_check_error(ForeignWriter)`, an error log, and `Foreign`.
6. `Write` with a count: store the pending claim BEFORE the write await. Then `create` (revision `None`) or
   `update`. On `Ok`: count at once (`count_loss`), clear the pending claim, set `last_own = Some(R)`, give
   `Counted(kind)`. Without a count: on `Ok`, set `last_own`, give `Wrote`.
7. `Err(WrongRevision)` gives `CasLost`: no count, no error, keep the pending claim (the next get drops it).
8. "error": `Timeout` (and the outer timeout firing) gives `Permission` when `kv_violations != before`, else
   `Timeout`. `NoBucket` gives `NoBucket`. `Other` gives `Other`. Count with `count_check_error` and log a
   warning with the kind at most once per 60 s per kind.
9. Every `Failed(_)` outcome (a KV error, a Redis error, the outer timeout) sets `last_own = None`
   (coordinator fix, spec §5.2). A relay with failing KV steps can miss a whole generation. A stale
   `last_own` would then turn a restore to that old generation into `Foreign`, and the reset would not
   be counted. `CasLost` and `Foreign` keep `last_own`.

- [ ] **Step 1: Write the failing unit tests**

A `FakeKv` holds `StdMutex<Option<(Bytes, u64, bool)>>`, a revision counter, and switches:
`update_then_fail: Option<KvError>` (apply the write, then return the error), `update_then_hang: bool` (apply,
then `std::future::pending()`), `get_error: Option<KvError>`, `race: Option<Bytes>` (a write by "another relay"
that runs inside `update` before the revision check). Use `#[tokio::test(start_paused = true)]` and a local
`DebuggingRecorder` (`metrics::set_default_local_recorder`).

```rust
#[tokio::test(start_paused = true)]
async fn an_ambiguous_counting_write_counts_once_on_the_next_get() {        // T7b case 1
    // KV {K}, Redis {R}. update applies, then returns Timeout. step 1: Failed(Timeout), resets 0.
    // step 2 (no failure switch, Redis {R, E+1}): resets 1, lost == the claim's lost. step 3, 4, 5: resets still 1.
}
#[tokio::test(start_paused = true)]
async fn a_cancelled_counting_write_counts_once_on_the_next_get() {         // T7b case 2
    // update applies, then hangs; the step timeout cancels it: Failed(Timeout). Next steps: resets exactly 1.
}
#[tokio::test(start_paused = true)]
async fn a_pending_nonce_that_the_entry_does_not_hold_counts_nothing() {    // T7b case 3
    // update returns Timeout and does NOT apply; another write replaces the entry. Next step: resets 0.
}
#[tokio::test(start_paused = true)]
async fn a_concurrent_write_loses_the_cas_and_counts_nothing() {            // T10
    // race: another relay writes {R} between the get and the CAS. step: CasLost, resets 0, errors 0.
    // next step: Wrote, resets 0.
}
#[tokio::test(start_paused = true)]
async fn an_unparsable_value_is_replaced_with_its_revision() {              // Review Focus 2
    // entry b"not json" at revision 3: the write is update(.., 3); Wrote; resets 0; errors 0 for every kind.
}
#[tokio::test(start_paused = true)]
async fn a_foreign_entry_counts_a_foreign_writer_error_and_writes_nothing() { // T15 unit
}
#[tokio::test(start_paused = true)]
async fn a_timeout_with_a_new_kv_violation_is_a_permission_error() {
    // get_error Timeout, and the fake bumps kv_violations inside get: errors{kind="permission"} == 1.
    // Without the bump: errors{kind="timeout"} == 1.
}
#[tokio::test(start_paused = true)]
async fn a_failed_step_forgets_last_own_so_a_restore_still_counts() {
    // step on Redis {G0}, KV {G0}: Wrote (last_own = G0). Another relay writes KV {G1}. step with
    // get_error Timeout: Failed(Timeout). step on Redis {G0} (restored), KV {G1}: Counted(Reset), resets 1,
    // errors{kind="foreign_writer"} == 0. Without the failed step in the middle: Foreign, resets 0.
}
#[tokio::test(start_paused = true)]
async fn a_redis_error_in_the_detection_is_a_redis_error() {}
#[tokio::test(start_paused = true)]
async fn a_success_counts_at_once_and_forgets_the_nonce() {}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cd rs && cargo nextest run -p paigasus-gateway --lib -E 'test(/generation::/)'`
Expected: FAIL to compile.

- [ ] **Step 3: Implement `KvEntry`, `KvError`, `GenKv`, `KvView::from_entry`, `StepOutcome` and `Generation`**

- [ ] **Step 4: Run the tests to verify they pass**

Run: the command of step 2.
Expected: PASS.

- [ ] **Step 5: Prove the claim recovery bites**

Mutation A: make `resolve_pending` always return `None`. Expected: the two "counts once on the next get" tests
FAIL. Mutation B: store the pending claim AFTER the write await instead of before. Expected: the cancel test
FAILS. Run with `--no-fail-fast`; restore each.

- [ ] **Step 6: Commit**

```bash
git add rs/crates/services/paigasus-gateway/src/adapters/decision_log/generation.rs
git commit -m "feat(rs): add the decision log KV step with a claim nonce (SMA-821)"
```

---

### Task 6: The NATS KV adapter

**Files:**
- Create: `rs/crates/services/paigasus-gateway/src/adapters/decision_log/gen_kv.rs`
- Modify: `mod.rs` (`pub mod gen_kv;`)
- Test: `rs/crates/services/paigasus-gateway/tests/decision_log_redis.rs`

**Interfaces:**
- Consumes: `GenKv`, `KvEntry`, `KvError` (Task 5); `GEN_BUCKET`, `gen_kv_config` (Task 2).
- Produces: `pub struct NatsGenKv` with `pub fn new(js: jetstream::Context, entry_key: String) -> Self`, and
  `impl GenKv for NatsGenKv`. It keeps `tokio::sync::Mutex<Option<jetstream::kv::Store>>`. It makes the store
  lazily: `js.get_key_value(GEN_BUCKET)`; on a "stream not found" (JetStream code 10059 in the `GetStreamError`
  source) it calls `js.create_key_value(gen_kv_config())`. It drops the store on every error except
  `WrongRevision` (D9). It reads with `Store::entry` only (direct get, `allow_direct` is on).

**Error mapping** (map from the error kind; keep only the kind):

| call | async-nats kind | `KvError` |
|---|---|---|
| `get_key_value` | `GetBucket`, source `JetStream` with code 10059 | try `create_key_value` |
| `get_key_value` | `GetBucket`, source `Request` (a timeout) | `Timeout` |
| `create_key_value` | `TimedOut` | `Timeout` |
| `create_key_value` | any other | `NoBucket` |
| `entry` | `TimedOut` / other | `Timeout` / `Other` |
| `create` | `AlreadyExists` / `Publish` / other | `WrongRevision` / `Timeout` / `Other` |
| `update` | `WrongLastRevision` / `TimedOut` / other | `WrongRevision` / `Timeout` / `Other` |

A `get_key_value` or `create_key_value` error not in the table is `Other`.

- [ ] **Step 1: Write the failing Docker test**

```rust
#[tokio::test]
async fn the_kv_adapter_makes_the_bucket_and_does_a_cas() {
    // start_nats(None, …). kv = NatsGenKv::new(relay_js(..).await, entry_key(KEY)).
    // get() == Ok(None) and the bucket now exists with the gen_kv_config values (admin get_key_value + stream info:
    //   max_bytes 1_048_576, history 1, file, allow_direct true).
    // create(b"a") == Ok(r1); create(b"b") == Err(WrongRevision); update(b"c", r1) == Ok(r2); update(b"d", r1) == Err(WrongRevision).
    // Review Focus 3: admin `kv.delete(entry_key(KEY))`; get() gives live == false; create(b"e") == Ok(_).
}
#[tokio::test]
async fn the_kv_adapter_makes_the_handle_again_after_an_outage() {
    // start_nats(Some(port)). get() ok. nats.stop(). get() == Err(Timeout | Other). nats.start(). wait_for get() ok.
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cd rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-gateway --test decision_log_redis -E 'test(kv_adapter)'`
Expected: FAIL to compile.

- [ ] **Step 3: Implement `NatsGenKv`**

- [ ] **Step 4: Run the tests to verify they pass**

Run: the command of step 2.
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add rs/crates/services/paigasus-gateway
git commit -m "feat(rs): add the NATS KV adapter of the decision log generation record (SMA-821)"
```

---

### Task 7: The Redis scripts

**Files:**
- Create: `rs/crates/services/paigasus-gateway/src/adapters/decision_log/gen_redis.rs`
- Modify: `mod.rs` (`pub mod gen_redis;`), `relay.rs:42-52` (move `SAMPLE_SCRIPT` out)
- Test: `tests/decision_log_redis.rs`

**Interfaces:**
- Consumes: `RedisView` (Task 4).
- Produces in `gen_redis.rs`:
  - `pub fn gen_key(stream_key: &str) -> String` (`<key>:gen`), `pub fn epoch_key(stream_key: &str) -> String` (`<key>:epoch`).
  - `#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum GroupOutcome { Created, Exists }`
  - `pub async fn ensure_group(conn: &mut paigasus_redis::RedisHandle, key: &str, group: &str, fresh_id: &str) -> Result<GroupOutcome, redis::RedisError>`
  - `#[derive(Debug, Clone, PartialEq, Eq)] pub struct Sample { pub xlen: u64, pub has_group: bool, pub generation: Option<String>, pub epoch: Option<u64> }`
  - `pub async fn sample(conn: &mut paigasus_redis::RedisHandle, key: &str, group: &str) -> Result<Sample, redis::RedisError>` (read-only)
  - `pub async fn detect(conn: &mut paigasus_redis::RedisHandle, key: &str, group: &str, fresh_id: &str) -> Result<RedisView, redis::RedisError>`
  - Each script is a `static LazyLock<redis::Script>`.

The group script (KEYS: stream, gen; ARGV: group, fresh id):

```lua
local r = redis.pcall('XGROUP', 'CREATE', KEYS[1], ARGV[1], '0', 'MKSTREAM')
if type(r) == 'table' and r.err then
  if string.sub(r.err, 1, 9) == 'BUSYGROUP' then
    redis.call('SET', KEYS[2], ARGV[2], 'NX')
    return 'exists'
  end
  return r
end
redis.call('SET', KEYS[2], ARGV[2])
return 'created'
```

The sample script keeps the SMA-755 body and returns `{n, has_group, GET gen, GET epoch}` (KEYS: stream, gen,
epoch). The detection script (KEYS: stream, gen, epoch; ARGV: group, fresh id) returns `{0, false, false, n}`
without the group and writes NOTHING then. With the group, it runs `SET gen id NX` and `INCR epoch` through
`redis.pcall`, returns any error reply as it is, and returns `{1, GET gen, epoch, n}`. (Spec §5.3 does not say what
the script does without a group; this plan writes nothing, so a stream that no relay has found yet gets no key.)

- [ ] **Step 1: Write the failing Docker tests**

```rust
#[tokio::test]
async fn the_group_script_makes_a_new_generation_on_create() {
    // ensure_group(.., "id-1") == Created, GET gen == "id-1". DEL stream; ensure_group(.., "id-2") == Created, GET gen == "id-2".
}
#[tokio::test]
async fn the_group_script_keeps_the_generation_on_busygroup_and_repairs_a_missing_one() {
    // after Created with "id-1": ensure_group(.., "id-2") == Exists, GET gen == "id-1".
    // DEL gen; ensure_group(.., "id-3") == Exists, GET gen == "id-3".
}
#[tokio::test]
async fn the_group_script_returns_another_error() {
    // ACL SETUSER default -xgroup: ensure_group is Err, and the error is not BUSYGROUP. GET gen is empty. Restore +xgroup.
}
#[tokio::test]
async fn the_detection_script_repairs_the_generation_and_increments_the_epoch() {
    // redis-cli XGROUP CREATE KEY relay 0 MKSTREAM (an SMA-755 group). detect("id-1") == Live{"id-1", 1, 0};
    // detect("id-2") == Live{"id-1", 2, 0}.
}
#[tokio::test]
async fn the_detection_script_writes_nothing_without_a_group() {
    // empty Redis: detect == NoGroup, EXISTS gen epoch == 0. XADD without group: NoGroup, xlen kept in no key.
}
#[tokio::test]
async fn the_sample_script_reads_the_keys_and_writes_nothing() {
    // DBSIZE before == DBSIZE after; Sample has the values of gen and epoch.
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cd rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-gateway --test decision_log_redis -E 'test(_script_)'`
Expected: FAIL to compile.

- [ ] **Step 3: Implement `gen_redis.rs`**

- [ ] **Step 4: Run the tests to verify they pass**

Run: the command of step 2.
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add rs/crates/services/paigasus-gateway
git commit -m "feat(rs): add the decision log generation Redis scripts (SMA-821)"
```

---

### Task 8: Wire the KV step into the relay

**Files:**
- Modify: `rs/crates/services/paigasus-gateway/src/adapters/decision_log/relay.rs` (module doc 3-8; `Inner` 124-135;
  `State` 137-145; 232-301; `iteration` 498-515; `sampler` 642-661; `run_relay` 666-704)
- Test: `tests/decision_log_redis.rs` (`flushall_with_two_relays_counts_one_reset` at 725,
  `a_group_made_after_a_failed_first_create_counts_one_reset` at 1033, new tests)

**Interfaces:**
- Consumes: Tasks 3-7.
- Produces: `RelaySettings` gains `pub kv_timeout: Duration` (5 s from `new`; `fast()` in the tests sets 1 s).
  Test helpers that Task 9 uses (in `tests/decision_log_redis.rs`):
  - `async fn kv_record(admin: &jetstream::Context) -> Option<GenRecord>` (reads `entry_key(KEY)` in `GEN_BUCKET`).
  - `async fn wait_for_kv_steps(admin: &jetstream::Context, n: u64)`: waits until the stored epoch grew by `n`.
  - `async fn block_publishes(admin: &jetstream::Context)` / `unblock_publishes(..)`: make or delete a stream
    `BLOCK` on `gateway.decision.>` (64 MiB). While it exists, `get_or_create_stream` for `GATEWAY_DECISIONS` fails,
    so each relay keeps its batch in Redis and the KV step still runs.
  - `fn resets(snap: &Tally) -> u64`, `fn lost(snap: &Tally) -> u64`, `fn check_errors(snap: &Tally, kind: &str) -> u64`.

Changes in `relay.rs`:
- Remove `Inner::recovering`, `State::sample`, `last_sample`, `set_sample`, `count_reset`, `create_group`.
- `handle_nogroup` and the start arm of `iteration` call `gen_redis::ensure_group(.., &new_id())`. They set
  `group_ready` and count nothing. Keep the warning text "the relay cannot make the consumer group again".
- `State` gains `generation: Generation`, `kv: NatsGenKv` (from `js.clone()` and `entry_key(&s.key)`), and
  `kv_in_flight: AtomicBool`.
- `sampler(self: Arc<Self>, shutdown)`: each tick, (1) `gen_redis::sample`, set the gauge, and when `has_group` and
  both keys are set, `generation.observe(LocalTuple{..})`; (2) when `kv_in_flight` was false (compare-exchange),
  spawn the step: `generation.step(&kv, || gen_redis::detect(..), &violations.kv, s.kv_timeout)`, with a guard
  that clears `kv_in_flight` on drop. A tick with no Redis handle skips the step. On shutdown, abort the spawned
  step task.
- The module doc: the relay no longer counts on `NOGROUP`; the sampler runs the generation check (SMA-821).

- [ ] **Step 1: Write and rewrite the Docker tests**

- T2, rewrite `flushall_with_two_relays_counts_one_reset`: NATS up the whole time. `block_publishes`, xadd 3, two
  relays, wait for the gauge 3, both hold the entries (delivery count 2), and `kv_record(..).sample == Some(3)`.
  FLUSHALL. Sleep 1 s: resets 0. `unblock_publishes`. Wait for resets >= 1, `wait_for_kv_steps(3)`. Assert resets
  == 1, lost == 3, and the `acked=0` / `settled=3` log.
- T7, rewrite `a_group_made_after_a_failed_first_create_counts_one_reset` with `start_nats(None, ..)` (not a dead
  URL). Wait for a KV record, then `-xgroup`, FLUSHALL, wait for the "cannot make the consumer group again" log,
  resets 0, `+xgroup`, then exactly 1.
- T3 `a_restart_with_no_loss_counts_nothing`: relay i1, wait for a KV record, shut down, start i1 again,
  `wait_for_kv_steps(3)`: resets 0, every check error 0.
- T4 `the_upgrade_from_sma_755_counts_nothing`: `XGROUP CREATE` by redis-cli, no gen or epoch key, empty KV. Relay:
  `wait_for_kv_steps(3)` from the first record: resets 0, `GET gen` is a UUID, `GET epoch` >= 3, the KV record
  holds that generation.
- T11 `the_backlog_gauge_updates_while_the_kv_step_hangs`: relay with a KV record, `block_publishes`, then
  `nats.pause()`. xadd 1, 2, 3 entries in turn; after each, the gauge reaches the new XLEN within 2 s. `unpause`.
- Review Focus 4 `a_shutdown_during_a_hung_kv_step_ends_in_time`: `nats.pause()`, wait 300 ms, then
  `tokio::time::timeout(Duration::from_secs(2 + 1 + 2), relay.shut_down())` completes (budget 2 s, `kv_timeout` 1 s).

- [ ] **Step 2: Run them to verify they fail**

Run: `cd rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-gateway --test decision_log_redis --no-fail-fast`
Expected: FAIL to compile (`kv_timeout` does not exist yet).

- [ ] **Step 3: Implement the relay changes**

- [ ] **Step 4: Run the whole suite and the unit tests**

Run: `cd rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-gateway --test decision_log_redis` and
`cd rs && cargo nextest run -p paigasus-gateway --lib`.
Expected: PASS, the existing tests included (`the_relay_sizes_the_stream_to_a_smaller_account_limit` still has
`v.publish == 0`).

- [ ] **Step 5: Commit**

```bash
git add rs/crates/services/paigasus-gateway
git commit -m "feat(rs): detect a decision log Redis reset from the generation record (SMA-821)"
```

---

### Task 9: The scenario tests and the end-to-end proof

**Files:**
- Test: `rs/crates/services/paigasus-gateway/tests/decision_log_redis.rs`

**Interfaces:**
- Consumes: the helpers of Task 8.

- [ ] **Step 1: T1 `a_fresh_relay_after_flushall_counts_one_reset_with_the_stored_sample`**

`block_publishes`, xadd 3, relay i1, wait for `kv_record.sample == Some(3)`, shut i1 down, FLUSHALL, start a NEW
relay i2 (new consumer name, no memory). Exactly 1 reset, lost == 3.

- [ ] **Step 2: T5 `a_restore_of_an_older_generation_counts_one_reset`**

Relay with a KV record. `SET <KEY>:gen <new_id()>` by redis-cli. Exactly 1 reset.

- [ ] **Step 3: T6 `a_reset_during_a_nats_outage_counts_after_nats_returns`**

`start_nats(Some(port))`. Relay with a KV record (assert it is there). `nats.stop()`. FLUSHALL. Wait for
`sum over CHECK_ERROR_KINDS` of check errors > 0. Resets 0. `nats.start()`. Assert the KV record still exists
(the store survives a container stop; if it does not, the test fails here with that message). Exactly 1 reset.

- [ ] **Step 4: T6b `a_publisher_without_kv_grants_still_relays_every_event`**

The `SMALL_ACCOUNT_CONF` broker (SMA-755 grants only). xadd 20 events in 4 batches with 300 ms between them. Wait
for published == 20 and XLEN 0. Assert `publish_rejected` == 0, `v.publish` == 0, `v.kv` > 0, and
`check_errors(.., "permission")` > 0. Then `nats.pause()` for 3 s with 2 more events, `unpause`: published == 22 and
`publish_rejected` == 0.

- [ ] **Step 5: T8 `an_xack_of_zero_and_a_group_made_by_another_relay_counts_one_reset`**

`block_publishes`, xadd 3, relay A holds the batch, wait for `kv_record.sample == Some(3)`. FLUSHALL. Start relay B
(empty carry): it makes the group. Exactly 1 reset, lost == 3. `unblock_publishes`; A settles; the log has
`acked=0`. Still exactly 1.

- [ ] **Step 6: T9 `a_group_made_by_an_sma_755_relay_is_repaired_and_counts_one_reset`**

Relay with a KV record. One `EVAL`: `DEL` the stream, the gen key and the epoch key, then
`XGROUP CREATE KEY relay 0 MKSTREAM` (so the relay never sees `NOGROUP`). Within one KV step `GET gen` is set.
Exactly 1 reset.

- [ ] **Step 7: T13 `a_return_to_an_older_state_of_the_same_generation_counts_one_loss`**

`block_publishes`, xadd 2, relay, wait for `kv_record.sample == Some(2)`. One `EVAL` copies the three keys into
side keys with `DUMP`:
`for i = 1, 3 do redis.call('SET', KEYS[i + 3], redis.call('DUMP', KEYS[i])) end return 1`.
`wait_for_kv_steps(3)`. One `EVAL` restores all three:
`for i = 1, 3 do redis.call('RESTORE', KEYS[i], 0, redis.call('GET', KEYS[i + 3]), 'REPLACE') end return 1`.
Exactly 1 reset, lost == 2, and the group and the generation are unchanged.

- [ ] **Step 8: T15 `a_foreign_writer_counts_errors_and_no_reset`**

Relay with a KV record. The admin client writes a record with another generation (`update` with the current
revision). `wait_for` `check_errors(.., "foreign_writer") >= 3`. Resets 0. The error log names the foreign writer.

- [ ] **Step 9: Review Focus 1 `a_redis_at_its_memory_limit_counts_a_redis_error_and_keeps_the_gauge`**

Relay with a KV record. `CONFIG SET maxmemory 1` (and `maxmemory-policy noeviction`). Wait for
`check_errors(.., "redis") > 0`. xadd by redis-cli is refused, so read the gauge: it still equals XLEN on the next
ticks. Resets 0. `CONFIG SET maxmemory 0`; the next steps give no new `redis` error.

- [ ] **Step 10: Review Focus 5 `two_fresh_relays_on_a_first_deploy_count_nothing`**

Empty Redis, empty KV. Start two relays in the same moment. `wait_for_kv_steps(3)`. Resets 0, and every check error
kind is 0.

- [ ] **Step 11: Run all tests**

Run: `cd rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-gateway --test decision_log_redis`
Expected: PASS. If T1-T15 pass on the first run, still do step 12.

- [ ] **Step 12: Prove the counting bites end to end**

Mutation A: in `decide`, replace the guard of the `K != R` counting row with `false` (as in Task 4). Run
`cd rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-gateway --test decision_log_redis --no-fail-fast -E 'test(counts_one) | test(flushall)'`.
Expected: T1, T2, T5, T7, T8 and T9 FAIL. Mutation B: replace the `E <= e` guard with `false`. Expected: T13 FAILS.
Restore each and run the suite once more.

- [ ] **Step 13: Commit**

```bash
git add rs/crates/services/paigasus-gateway/tests/decision_log_redis.rs
git commit -m "test(rs): cover the decision log reset cases of the generation record (SMA-821)"
```

---

### Task 10: Documents and the full gate graph

**Files:**
- Modify: `docs/ops/RUNBOOK-observability.md` (metric rows ~151-159, the alerts table ~270-274, the
  `GatewayDecisionLogRedisReset` entry ~1865-1886, a new `GatewayDecisionLogGenerationCheckFailing` entry after it)
- Modify: `docs/superpowers/specs/2026-11-18-sma-755-tool-decision-log-design.md` (§5.8 "Known limits", ~588-600)

- [ ] **Step 1: Update the runbook**

- Rows: `redis_resets_total` (the new meaning: the stream lost entries, a lost stream or a return to an older
  state; one per incident over all relays; counted at the next KV step, up to one sample period late);
  `lost_estimate_total` (the last stored sample; for a reset the newer of the KV sample and the sample of the
  relay; not a bound); the two new metrics (the gauge is not primed and is per instance).
- The alerts table: the new `GatewayDecisionLogRedisReset` expression and the new alert.
- `GatewayDecisionLogRedisReset`: it now also fires on a return to an older state; a restore from a snapshot from
  before a reset is detected and its estimate means less; it fires up to one sample period after the loss;
  during a NATS outage it fires when NATS is back; during the rolling deploy from SMA-755 one incident can count
  twice. Remove "A restore … does not detect it". The generation key and the epoch key need
  `maxmemory-policy noeviction`; deleting only one of them counts a false reset or loss.
- `GatewayDecisionLogGenerationCheckFailing`: each `kind` and what to do (`permission`: the publisher creds lack
  the KV grants, rerun `provision-decisions.sh` and redeploy the new creds; `no_bucket`: make the bucket with
  `PUSH=1`, check the 2-stream limit; `timeout`: NATS health; `redis`: Redis memory or ACL; `foreign_writer`: the
  deployment rule of spec §5.2 is broken, give the second deployment its own NATS account or Redis prefix;
  `other`: the logs). State: while it fires, a Redis reset is not counted. State the upgrade order.
- The limits of spec §5.5 that operators must know.

- [ ] **Step 2: Update the SMA-755 spec §5.8**

Replace the "reset count is best-effort" bullet: cases (a), (b) and (c) are now counted (SMA-821, link the
SMA-821 spec). Replace "A Redis failover that keeps the group but loses the last writes is not seen live" with the
§5.5 limit of SMA-821 (a return to a state newer than the last KV write is missed). Add the other §5.5 limits.

- [ ] **Step 3: Run the full gate graph**

Run the `moon ci` command between the `ci-targets` markers of the root `CLAUDE.md`, with `--base origin/main
--include-relations`. Then re-run the bash-4 gates directly as that file says, in particular
`/opt/homebrew/bin/bash ops/nats/check-subjects.sh` and the `repo:nats-permissions` nextest half.
Expected: every target passes. Read `.moon/cache/ciReport.json` for any failure before a re-run (root
`CLAUDE.md`, "Diagnosing an unattributed `moon ci` failure").

- [ ] **Step 4: Commit**

```bash
git add docs/ops/RUNBOOK-observability.md docs/superpowers/specs/2026-11-18-sma-755-tool-decision-log-design.md
git commit -m "docs(ops): document the decision log generation record (SMA-821)"
```
