# SMA-856 Signed Webhook Detector Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a signed webhook detector to the gateway. An operator registers external finding providers in the gateway config. A tenant opts in through IAM. The gateway sends one HMAC-signed `POST` per tool call, checks the signed answer, and adds the findings to the call. Every provider failure gives a `detector_error` finding.

**Architecture:** IAM stores a per-org opt-in list (`tool_policy_settings.webhooks`, proto field 5 / 4). The gateway reads it through `GetCompiledToolPolicies` (or `[[tool_policy.org]] webhooks` with the file source), puts it into the `PolicySnapshot` under the D8 rule, and the decider passes it to the pipeline in `DetectBatch.caller`. The pipeline resolves each opted-in name against a `WebhookRegistry`, builds one body per call once (pure module `domain/webhook_protocol.rs`), and runs the `WebhookDetector` adapters at the same time. Each adapter bounds each call by `min(now + timeout, deadline - 10 ms)` and returns one result per call. The pipeline cut stays as a backstop.

**Tech Stack:** Rust 2024 (tokio, futures `buffer_unordered`, reqwest 0.13 HTTP/1.1, hmac 0.13, sha2 0.11, serde_json `raw_value`, secrecy, rand 0.10, sea-orm, axum for mocks, rcgen for CA fixtures), protobuf + buf, Moon.

**Spec:** `docs/superpowers/specs/2026-11-28-sma-856-webhook-detector-design.md` (revision 2). The spec is the source of truth. Read it with this plan.

## Global Constraints

- Every new source file opens with `// SPDX-License-Identifier: Apache-2.0`.
- Edition 2024, rust-version 1.95. `cargo clippy --workspace --all-targets -- -D warnings` must pass after each task. Do not add an item in one task that a later task wires, unless it is `pub` in a `pub` module and a test uses it.
- No new crate dependency (spec § 7.8). Add the feature `raw_value` to the gateway's own `serde_json` line. Hex is hand-written (§ 6.5).
- Commits: conventional, with a scope: `feat(contracts): …`, `feat(rs): …`, `docs(rs): …`. End each message with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Do not put a `#NNN` line in a commit body. Do not commit with `--no-verify`. Commit as the repository's git user.
- After a `.proto` edit: `buf format -w` in `contracts/`, then `moon run contracts:generate`. Commit the regenerated Rust, Python and TypeScript files.
- Toolchain PATH for every command: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.
- Reserved provider names: every name in `BUILT_IN_NAMES` (`kernel`, `built_in`, `undeclared_tool`, `invalid_args`, `unseen_host`, `secret_in_args`, `injection_markers`), and `judge`, `webhook`, `other`.
- Reserved kinds: every kind in `FINDING_KINDS` (`decider.rs:106-132`).
- Name rule: `[a-z0-9_]{1,64}`. Version rule (D14): `[a-z0-9._-]{0,32}`. Env name rule (D20): `[A-Z][A-Z0-9_]{0,127}`, not starting with `GATEWAY_`.
- Limits: at most 8 providers; `kinds` 1 to 16; `max_request_bytes` default 1048576 and at most the effective `[tool_policy.detectors] max_scan_bytes`; response body at most 65536 bytes; at most 16 findings per answer; `max_in_flight` default 64, 1 to 1024; `timeout_ms` default 300, 1 to `[tool_policy.detectors] timeout_ms - 50`; per batch and provider `buffer_unordered(8)`; the adapter stops at `deadline - 10 ms`; secrets at least 32 bytes.
- IAM opt-in: at most 16 distinct names, each `[a-z0-9_]{1,64}`; stored sorted and unique. Error reason `invalid-webhook-opt-in`, field `webhooks`, `ERROR_REASON_INVALID_WEBHOOK_OPT_IN = 64`.
- Failure reasons (closed set, the `version` of the failure finding): adapter `request_too_large`, `overloaded`, `transport`, `timeout`, `deadline`, `status`, `response_too_large`, `signature`, `malformed`; pipeline `deadline`, `panic`, `error`, `shape`, `check`, `unknown_provider`, `not_permitted`, `retired`.
- Wire protocol version 1: headers `Content-Type: application/json`, `Accept-Encoding: identity`, `Paigasus-Webhook-Version: 1`, `Paigasus-Nonce: <32 lower-case hex>`, `Paigasus-Signature: t=<unix seconds>,v1=<64 lower-case hex>`. Request MAC input: `"paigasus.request." || name || "." || t || "." || nonce || "." || body`. Response MAC input: `"paigasus.response." || name || "." || nonce || "." || body`. HMAC-SHA256, key = the exact bytes of the env var.
- Nothing in A4 leaks: no secret, body, argument, nonce, signature or URL path in a log event, a metric label, a `Debug`, the decision log, the proto or the readiness dump. Never log the `Display` of a `reqwest::Error`.
- No `tokio::spawn` for webhooks (A8).
- Local gate runs on the development Mac follow the root `CLAUDE.md` "This development Mac only" section (bash 3.2 versus bash 5 gates).

## Review Focus

1. **Less than 10 ms left at the webhook stage.** A hold whose cut is already less than 10 ms away (or in the past) must give `deadline` for every call, send no request, and not panic on `Instant - Duration`. Use `checked_sub` and treat `None` as "already passed". Test in Task 8: `a_cut_inside_the_10_ms_margin_sends_nothing`.
2. **Content-Type that only starts with `application/json`.** `application/jsonp`, `application/json-seq` and `text/json` are `malformed`. `Application/JSON; charset=utf-8` is accepted (media type compared case-insensitively, parameters ignored). Test in Task 6: `the_content_type_check_reads_the_media_type_only`.
3. **A duplicate key in the provider answer.** `{"findings":[],"findings":[{…}]}` and a finding with `kind` twice are `malformed`, never "the last one wins". Test in Task 6: `a_duplicate_key_in_the_answer_is_malformed`.
4. **Exact size limits.** A response of exactly 65536 bytes is read; 65537 is `response_too_large`. A request body of exactly `max_request_bytes` is sent; one byte more is `request_too_large` with no request. Tests in Task 8: `the_size_limits_are_inclusive`.
5. **A bad opt-in name from IAM with control or upper-case characters.** It becomes `unknown_provider` with detector `webhook` (not the bad text), and the cache warn writes `name = "invalid"`, never the text. Tests in Task 7: `an_invalid_opt_in_name_is_never_a_detector_name_or_a_log_field`.

---

## Plan-level decisions (where the spec leaves a choice or the code differs)

- **PD1. The body builder is pure and in `domain`.** The pipeline (application layer) builds the bodies (§ 6.5), and the adapter signs and parses. So `src/domain/webhook_protocol.rs` holds the body builder, the MAC strings, the header parse and the answer parse. The application layer does not import an adapter.
- **PD2. `WebhookRegistry` in `domain/detectors.rs`.** It holds the providers in config order with their `orgs` scope and `retired` flag. The pipeline and the policy cache share one `Arc<WebhookRegistry>`. `DetectorPipeline::new` takes `impl Into<WebhookRegistry>`, and `From<Vec<Arc<dyn Detector>>>` gives each detector `OrgScope::All` and `retired = false`, so the existing call sites compile unchanged.
- **PD3. Late-stage failure findings.** A pipeline-level failure of a webhook or of the judge is `Finding::new(name, "detector_error", d.failure_severity(), reason)`. Only the webhook stage counts `gateway_tool_policy_webhook_failures_total`. Built-in stage failures do not change (`detector_error(DETECTOR_BUILT_IN)`, version `""`).
- **PD4. The strict D37 check** (detector = own name, kind in `kinds()` when `Some`, version D14) applies to the webhook stage. The judge stage keeps today's check plus D14.
- **PD5. P7 holds.** When `hold_slots` marks the tenant part unavailable for one hold (SMA-753 P7), `call_batch` gives an empty opt-in. This applies D8 ("tenant set unavailable") at hold level.
- **PD6. The file-source opt-in** reaches the request side through `RequestToolPolicy::with_org_webhooks(HashMap<Uuid, Arc<[String]>>)`, not through a new `ToolPolicySettings` field. About ten test files build `ToolPolicySettings` literals and stay unchanged.
- **PD7. The IAM upsert returns the stored list** (`RETURNING`). The event payload, `webhooks_after` and the RPC answer use it.
- **PD8. Nonce failure.** `NonceSource::nonce16` returns `Option<[u8; 16]>`. `None` gives `transport` and sends nothing.
- **PD9. The connect timeout** of a provider client is its `timeout_ms`. The call bound covers the rest.
- **PD10. Secrets are required for every provider entry, `retired` ones too.**
- **PD11. `t` comes from `domain::limits::Clock`** (`SystemTime`, unix seconds). The nonce source is a new trait `NonceSource` in the adapter module.
- **PD12. Priming.** `gateway_tool_policy_webhook_failures_total` is primed at zero for each configured provider × the 9 adapter reasons. Pipeline reasons are not primed.
- **PD13. `runtime::build_tool_policy_with_env(config, recorder, history, env: &dyn EnvReader)`** is new. `build_tool_policy` calls it with `ProcessEnv`, so `main` and the existing tests do not change.
- **PD14. Config error text** is `tool_policy.detectors.webhook[<name>].<field>: <why>`. A name error uses the index: `tool_policy.detectors.webhook[<i>].name: <why>`.
- **PD15. Spec § 7.1 says a 25-digit number fails the strict parse. It does not.** The kernel keeps a big integer as text (`paigasus-kernel/src/tool_policy/input.rs:76-83`, `"big integer keeps its text"`). The plan tests that a 25-digit number is strict-parsed and embedded byte-equal. The duplicate key and the invalid JSON go as `args_text`.
- **PD16. Spec § 7.5 "a client built with an rcgen CA bundle trusts that CA" is not observable.** `reqwest::Client` has no trust-store accessor, and the gateway has no TLS listener on purpose (C18, `adapters/upstream.rs:366-370`). The plan tests that the bundle builds, that each bad bundle fails and names the provider and the path, and both attribution arms through the injected control build.

## File map

**Contracts**
- `contracts/proto/paigasus/iam/v1/iam.proto` — `ToolPolicySettings.webhooks = 5`, `WebhookOptIn`, `PutToolPolicySettingsRequest.webhooks = 4`.
- `contracts/proto/paigasus/common/v1/error.proto` — `ERROR_REASON_INVALID_WEBHOOK_OPT_IN = 64`.
- Generated: `rs/crates/libs/paigasus-proto/src/generated/…`, `py/packages/paigasus-proto/src/paigasus_proto/generated/…`, `ts/packages/paigasus-proto/src/generated/…`.
- `rs/crates/libs/paigasus-proto/src/error.rs` — the registry list and count.
- `ts/packages/paigasus-sdk/src/errors/presentation.ts`, `ts/packages/paigasus-sdk/tests/presentation.test.ts` — the new entry, the count 94.

**Kernel** — `rs/crates/libs/paigasus-kernel/src/tool_policy/{walk,mod}.rs` — `parses_strictly`.

**Gateway core** — `rs/crates/libs/paigasus-gateway-core/src/policy/snapshot.rs` — `PolicySnapshot.webhooks`, `with_webhooks`.

**IAM**
- `rs/crates/libs/paigasus-iam-core/src/tool_policy/{model,ports}.rs`
- `rs/crates/services/paigasus-iam/src/adapters/persistence/migration/{mod,m0016_tool_policy_webhooks}.rs`, `entities/tool_policy_settings.rs`, `pg_tool_policies.rs`
- `rs/crates/services/paigasus-iam/src/application/{error,fakes}.rs`, `application/tool_policies/{mod,settings,events}.rs`
- `rs/crates/services/paigasus-iam/src/adapters/grpc/tool_policies.rs`, `adapters/http/{tool_policies,dto}.rs`
- Tests: `tests/tool_policies_settings_pg.rs`, `tests/grpc_tool_policies.rs`, `tests/grpc_tool_policies_held.rs`, `tests/http_tool_policies.rs`

**Gateway**
- `src/config.rs`, `gateway.toml.example`, `Cargo.toml` (serde_json feature)
- `src/domain/detectors.rs`, new `src/domain/webhook_protocol.rs`, `src/domain/mod.rs`, `src/domain/tool_policy.rs`
- `src/application/detectors/{mod,pipeline}.rs`
- `src/application/tool_policy/{decider,request,cache,testing,mod}.rs`, `src/application/tool_history.rs`, `src/application/decision_log.rs`
- `src/adapters/upstream.rs`, new `src/adapters/detectors/webhook.rs`, `src/adapters/detectors/mod.rs`
- `src/adapters/policy_source/iam.rs`, `src/adapters/http/interception.rs` (tests)
- `src/runtime.rs`, `src/main.rs` (`describe_counter!`)
- Tests: `tests/tool_policy.rs`, new `tests/support/webhook_mock.rs`, `tests/support/mod.rs`, `tests/tool_policy_iam_source.rs`, `tests/tool_history.rs`

**Observability** — `rs/crates/libs/paigasus-observability/src/names.rs`.

**Docs** — `docs/ops/RUNBOOK-tool-policies.md`, `docs/ops/RUNBOOK-observability.md`, new `docs/tool-policies/webhook-protocol.md`, `docs/tool-policies/tenant-guide.md`, `rs/crates/services/paigasus-gateway/CHANGELOG.md`, `rs/crates/services/paigasus-iam/CHANGELOG.md`.

---

### Task 0: Provision the worktree and take the baseline

**Files:** none.

- [ ] **Step 1: Provision.** In the worktree root: `proto install` (run `proto trust` on the parent checkout path first if `.prototools` is untrusted), `pnpm -C ts install`, `( cd py && uv sync )`, `( cd rs && cargo fetch )`.
- [ ] **Step 2: Baseline.** Run `cd rs && cargo nextest run -p paigasus-gateway -p paigasus-gateway-core -p paigasus-iam -p paigasus-proto -p paigasus-kernel --features paigasus-kernel/tool-policy` and `cargo clippy --workspace --all-targets -- -D warnings`. Expected: PASS. Write down the test counts. If a test is red, stop and report it.

No commit.

---

### Task 1: Contracts — the opt-in fields and the new error reason

**Files:**
- Modify: `contracts/proto/paigasus/iam/v1/iam.proto:1211-1236`
- Modify: `contracts/proto/paigasus/common/v1/error.proto:264-269` (after `ERROR_REASON_TOOL_POLICY_HELD_NOT_ACCEPTABLE = 63`)
- Regenerate: Rust, Python, TypeScript bindings
- Modify: `rs/crates/libs/paigasus-proto/src/error.rs:154-275,451-462`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/grpc/tool_policies.rs:119-132` (compile fix only)
- Modify: `rs/crates/services/paigasus-iam/tests/grpc_tool_policies.rs:695-702`, `tests/grpc_tool_policies_held.rs:163` (compile fix only)
- Modify: `ts/packages/paigasus-sdk/src/errors/presentation.ts:126`, `ts/packages/paigasus-sdk/tests/presentation.test.ts:13`

**Interfaces:**
- Produces: `pb::ToolPolicySettings.webhooks: Vec<String>` (tag 5); `pb::WebhookOptIn { names: Vec<String> }`; `pb::PutToolPolicySettingsRequest.webhooks: Option<pb::WebhookOptIn>` (tag 4, no `optional` keyword); `ErrorReason::InvalidWebhookOptIn` = 64, wire `"invalid-webhook-opt-in"`.

- [ ] **Step 1: Write the failing tests.** In `paigasus-proto/src/error.rs`, add `"invalid-webhook-opt-in"` to `EXPECTED_REASONS` after `"tool-policy-held-not-acceptable"`, change the count anchor to `assert_eq!(actual.len(), 94, "the registry should hold 94 reasons")`, and extend `the_tool_policy_reasons_resolve_both_ways`:

```rust
assert_eq!(ErrorReason::from_wire_reason("invalid-webhook-opt-in"), Some(ErrorReason::InvalidWebhookOptIn));
assert_eq!(ErrorReason::InvalidWebhookOptIn as i32, 64);
```
  In `presentation.test.ts`, change `toHaveLength(93)` to `toHaveLength(94)`.
- [ ] **Step 2: Run.** `cd rs && cargo nextest run -p paigasus-proto error`. Expected: FAIL (no variant).
- [ ] **Step 3: Edit the protos.** Add the three fields exactly as spec § 6.6 shows, with the spec's comments. Add to `error.proto`:

```proto
  // "invalid-webhook-opt-in" — a webhook opt-in name breaks [a-z0-9_]{1,64}, or the list holds
  // more than 16 distinct names (SMA-856). The field is "webhooks". gRPC INVALID_ARGUMENT, HTTP 400.
  ERROR_REASON_INVALID_WEBHOOK_OPT_IN = 64;
```
  Run `cd contracts && buf format -w`, then `moon run contracts:generate`.
- [ ] **Step 4: Fix the compile sites.** `settings_to_pb` sets `webhooks: Vec::new()` (Task 2 fills it). The two test request literals set `webhooks: None`. Add `[ErrorReason.INVALID_WEBHOOK_OPT_IN]: 'from-transport',` to `presentation.ts` after the held entry.
- [ ] **Step 5: Run.** `cd rs && cargo nextest run -p paigasus-proto && cargo build -p paigasus-iam -p paigasus-gateway --tests`; `moon run paigasus-sdk:test paigasus-sdk:typecheck`; `cd contracts && buf breaking --against "$(git rev-parse --git-common-dir)#branch=main,subdir=contracts"` (worktree form). Expected: PASS, and no breaking change.
- [ ] **Step 6: Commit.** `git add contracts rs/crates/libs/paigasus-proto py/packages/paigasus-proto ts/packages/paigasus-proto ts/packages/paigasus-sdk rs/crates/services/paigasus-iam` — `feat(contracts): add the webhook opt-in list and its error reason (SMA-856)`.

---

### Task 2: IAM — store, validate, audit and serve the opt-in list

**Files:**
- Create: `rs/crates/services/paigasus-iam/src/adapters/persistence/migration/m0016_tool_policy_webhooks.rs`
- Modify: `migration/mod.rs:18-41`, `entities/tool_policy_settings.rs:9-16`, `pg_tool_policies.rs:572-619`
- Modify: `rs/crates/libs/paigasus-iam-core/src/tool_policy/model.rs:212-231`, `ports.rs:98-103`
- Modify: `src/application/error.rs` (variant, `code`, `class`, `field`, the tool-policy error test at `:513-528`)
- Modify: `src/application/tool_policies/settings.rs` (the validator), `mod.rs:374-454`, `events.rs:33-44`, `fakes.rs:2314-2331`
- Modify: `src/adapters/grpc/tool_policies.rs:119-132,392-410`, `src/adapters/http/tool_policies.rs:289-324`, `src/adapters/http/dto.rs:1128-1133`
- Test: `tests/tool_policies_settings_pg.rs`, `tests/grpc_tool_policies.rs`, `tests/http_tool_policies.rs`, unit tests in `settings.rs` and `mod.rs`

**Interfaces:**
- Consumes: Task 1 proto fields and `ErrorReason::InvalidWebhookOptIn`.
- Produces:
  - `ToolPolicySettings.webhooks: Vec<String>` (iam-core); `default_for_absent_row()` gives `Vec::new()`.
  - `async fn upsert_settings_in(&self, tx: &dyn Transaction, org: Uuid, s: &ToolPolicySettings, webhooks: Option<Vec<String>>) -> Result<Vec<String>, RepositoryError>` — returns the stored list after the write. With `None`, the column is not in the update columns; a new row gets the default `[]`. `s.webhooks` is ignored by the write.
  - `TenancyError::InvalidWebhookOptIn` — code `"invalid-webhook-opt-in"`, class `Validation`, field `Some("webhooks")`.
  - `pub(crate) fn normalise_opt_in(names: Vec<String>) -> Result<Vec<String>, TenancyError>` in `settings.rs`: sorted, unique.
  - `put_settings(&self, actor: &Prn, org: Uuid, mode: ToolPolicyMode, fail_mode: FailMode, webhooks: Option<Vec<String>>) -> Result<SettingsAccepted, TenancyError>`.

- [ ] **Step 1: Write the failing unit tests** (in `settings.rs` and the `put_settings` tests of `mod.rs`).

```rust
#[test] fn the_opt_in_list_is_sorted_unique_and_bounded() {
    assert_eq!(normalise_opt_in(vec!["b".into(), "a".into(), "b".into()]).unwrap(), ["a", "b"]);
    assert_eq!(normalise_opt_in(vec![]).unwrap(), Vec::<String>::new());
    let sixteen: Vec<String> = (0..16).map(|i| format!("p{i}")).collect();
    assert_eq!(normalise_opt_in(sixteen.iter().chain(&sixteen).cloned().collect()).unwrap().len(), 16, "duplicates do not count");
    let seventeen: Vec<String> = (0..17).map(|i| format!("p{i}")).collect();
    assert_eq!(normalise_opt_in(seventeen), Err(TenancyError::InvalidWebhookOptIn));
    for bad in ["", "Acme", "a-b", "a b", "é", &"a".repeat(65)] {
        assert_eq!(normalise_opt_in(vec![bad.to_owned()]), Err(TenancyError::InvalidWebhookOptIn), "{bad:?}");
    }
    assert!(normalise_opt_in(vec!["a".repeat(64)]).is_ok());
}
// mod.rs, with the fakes:
// put_settings_with_webhooks_stores_audits_and_publishes_the_list:
//   write Some(["b","a"]) -> accepted.settings.webhooks == ["a","b"]; audit detail["webhooks_before"] == json!([]),
//   detail["webhooks_after"] == json!(["a","b"]); event payload["webhooks"] == json!(["a","b"]).
// put_settings_without_webhooks_keeps_the_list_and_has_no_audit_keys:
//   after the write above, write None -> settings.webhooks == ["a","b"]; detail has no key "webhooks_before"
//   and no key "webhooks_after"; event payload["webhooks"] == json!(["a","b"]).
// put_settings_with_an_empty_list_clears_it: Some([]) -> [] and detail["webhooks_after"] == json!([]).
// a_bad_opt_in_writes_nothing: Some(["Bad"]) -> Err(InvalidWebhookOptIn), fake repo writes == 0.
```
  Extend `the_tool_policy_errors_have_their_codes_and_classes` with a separate assertion: `TenancyError::InvalidWebhookOptIn` → code `"invalid-webhook-opt-in"`, class `ErrorClass::Validation`, field `Some("webhooks")`.
- [ ] **Step 2: Run.** `cd rs && cargo nextest run -p paigasus-iam --lib tool_policies settings error`. Expected: FAIL (no field, no variant).
- [ ] **Step 3: Implement the domain, the error, the validator, `put_settings`, the event and the fake.** `put_settings` calls `normalise_opt_in` before step 1 (no read, no write on error). It keeps the read outside the transaction (`mod.rs:384`) and takes `webhooks_before` from it. The audit detail gets `webhooks_before` and `webhooks_after` only when the request carries the field. `settings_changed` takes the stored list and writes `"webhooks"`. The fake repo keeps the stored list when `webhooks` is `None`.
- [ ] **Step 4: Run.** Same command. Expected: PASS.
- [ ] **Step 5: Write the failing Pg tests** (`tests/tool_policies_settings_pg.rs`, Docker; set `PAIGASUS_REQUIRE_DOCKER=1`).

```rust
// the_webhooks_column_defaults_to_an_empty_list: a row written by upsert(None) reads webhooks == [].
// a_webhook_list_round_trips: upsert(Some(["a","b"])) returns ["a","b"] and settings() reads ["a","b"].
// an_absent_list_does_not_undo_a_concurrent_write: tx1 upsert(Some(["a"])) commits; tx2 upsert with a
//   stale struct (webhooks: vec![]) and None commits -> settings() reads ["a"]. (Same shape as
//   first_enabled_at_round_trips_and_survives_a_later_write.)
// a_bad_stored_list_is_corrupt: raw UPDATE webhooks = 'not json' -> settings() is Err(RepositoryError::Backend(_)).
// the_down_migration_drops_the_column: m0016 down then up keeps existing rows and gives [].
```
- [ ] **Step 6: Implement m0016 and the Pg adapter.** `up`: `SET LOCAL lock_timeout = '5s';` then `ALTER TABLE "tool_policy_settings" ADD COLUMN webhooks TEXT NOT NULL DEFAULT '[]';`. `down`: the same lock, then `ALTER TABLE "tool_policy_settings" DROP COLUMN IF EXISTS webhooks;`. The module doc names the rollback hazard (§ 6.6). Register it after m0015. The entity gets `pub webhooks: String`. `settings` parses the JSON text with `serde_json::from_str::<Vec<String>>` (as `role.rs` stores `scope_kinds`); a parse error is `corrupt(...)`. `upsert_settings_in` sets `webhooks: Set(json)` and adds `Column::Webhooks` to the update columns only when the argument is `Some`; otherwise the insert uses `NotSet`. Use `exec_with_returning` and return the parsed list of the returned row.
- [ ] **Step 7: Run.** `cd rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test tool_policies_settings_pg`. Expected: PASS.
- [ ] **Step 8: Write the failing transport tests.**

```rust
// grpc_tool_policies.rs
// the_webhook_opt_in_absent_keeps_present_replaces_empty_clears:
//   put(webhooks: Some(WebhookOptIn{names: ["b","a","a"]})) -> resp.settings.webhooks == ["a","b"];
//   put(webhooks: None) -> ["a","b"]; get_tool_policy_settings -> ["a","b"];
//   get_compiled_tool_policies -> settings.webhooks == ["a","b"];
//   put(Some(WebhookOptIn{names: []})) -> [].
// a_bad_opt_in_is_invalid_webhook_opt_in: 17 distinct names, and ["Bad-Name"] -> Code::InvalidArgument,
//   ErrorInfo reason "invalid-webhook-opt-in", metadata["field"] == "webhooks".
// http_tool_policies.rs
// the_rest_opt_in_absent_and_null_keep_the_list: PUT {"mode","fail_mode","webhooks":["a"]} -> body.settings.webhooks == ["a"];
//   PUT without the key -> ["a"]; PUT "webhooks": null -> ["a"]; PUT "webhooks": [] -> [].
// a_bad_rest_opt_in_is_400_invalid_webhook_opt_in: 17 names and ["a b"] -> 400, reason "invalid-webhook-opt-in".
```
- [ ] **Step 9: Implement the transports.** gRPC: `req.webhooks.map(|w| w.names)`; `settings_to_pb` sets `webhooks: s.webhooks.clone()`. REST: `PutToolPolicySettingsBody` gets `#[serde(default)] pub webhooks: Option<Vec<String>>` (absent and `null` both give `None`), and loses `Copy`. `settings_json` adds `"webhooks"`. Update the two existing test helpers to pass `webhooks: None`.
- [ ] **Step 10: Run.** `cd rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam`; then `moon run repo:error-code-single-site` from the repo root. Expected: PASS. If the gate names a new emission site, add the `MANIFEST` row that `ci/error-registry/README.md` asks for.
- [ ] **Step 11: Commit.** `git add rs/crates/libs/paigasus-iam-core rs/crates/services/paigasus-iam` — `feat(rs): store and serve the webhook opt-in list in IAM (SMA-856)`.

---

### Task 3: Gateway config — the provider table, its validation and the secret loader

**Files:**
- Modify: `rs/crates/services/paigasus-gateway/src/config.rs:964-1000` (`DetectorsConfig`), `:1152-1160` (`OrgToolPolicyConfig`), `:836-916` (`GatewayConfig::validate`), `:1184-1290` (`ToolPolicyConfig`)
- Modify: `rs/crates/services/paigasus-gateway/gateway.toml.example` (a commented webhook block after the detectors block)
- Modify: `src/domain/detectors.rs` (`OrgScope` only)
- Test: `src/config.rs` tests (near `:2487-2575`)

**Interfaces:**
- Produces (all `pub`, in `crate::config`):
  - `pub struct WebhookConfig { pub name: String, pub url: String, pub orgs: WebhookOrgs, pub secret_env: String, pub previous_secret_env: Option<String>, pub timeout_ms: u64, pub failure_severity: FailureSeverityConfig, pub kinds: Vec<String>, pub max_request_bytes: usize, pub max_in_flight: usize, pub retired: bool, pub extra_ca_bundle_path: Option<String> }` — `Deserialize` with `deny_unknown_fields`; defaults `timeout_ms = 300`, `failure_severity = High`, `max_request_bytes = 1_048_576`, `max_in_flight = 64`, `retired = false`. Hand-written `Serialize` (the readiness dump): `name`, `host` (the URL host only), `orgs` (`"*"` or the count), `timeout_ms`, `max_request_bytes`, `max_in_flight`, `retired`. Hand-written `Debug` with the same fields.
  - `#[serde(untagged)] pub enum WebhookOrgs { All(String), List(Vec<String>) }` — `All` is valid only as `"*"`.
  - `#[serde(rename_all = "snake_case")] pub enum FailureSeverityConfig { Info, Low, Medium, High }` and `impl From<FailureSeverityConfig> for Severity`.
  - `DetectorsConfig.webhook: Vec<WebhookConfig>` with `#[serde(default)]`.
  - `OrgToolPolicyConfig.webhooks: Vec<String>` with `#[serde(default)]`.
  - `impl WebhookConfig { pub fn org_scope(&self) -> OrgScope; pub fn parsed_url(&self) -> reqwest::Url; pub fn kind_set(&self) -> BTreeSet<String> }` (meaningful after `validate`; `OrgScope` is defined here in Task 3 as `pub enum OrgScope { All, Only(BTreeSet<Uuid>) }` in `crate::domain::detectors`).
  - `impl ToolPolicyConfig { pub fn org_webhooks(&self) -> HashMap<Uuid, Arc<[String]>> }` — sorted, unique per org.
  - `pub trait EnvReader { fn var(&self, name: &str) -> Option<String>; }`, `pub struct ProcessEnv;` (`std::env::var(name).ok()`), `impl EnvReader for HashMap<String, String>` (tests).
  - `pub struct ProviderSecrets { pub current: SecretString, pub previous: Option<SecretString> }` with a `Debug` that prints no value.
  - `pub fn load_webhook_secrets(config: &GatewayConfig, env: &dyn EnvReader) -> Result<BTreeMap<String, ProviderSecrets>, String>`.
  - `GatewayConfig::validate` calls a new `DetectorsConfig::validate_webhooks(&self, max_scan_bytes: usize) -> Result<(), String>` with `self.tool_policy.detectors.max_scan_bytes.unwrap_or(self.default_detector_scan_bytes())`, and a new org check in `ToolPolicyConfig::validate`.

- [ ] **Step 1: Write the failing tests.** One helper `wh_toml(fields: &str) -> String` builds `det_toml(5, "timeout_ms = 2000")` plus one `[[tool_policy.detectors.webhook]]` table with valid defaults (name `acme_dlp`, url `https://dlp.acme.example/v1/findings`, orgs `["6f1c0000-0000-4000-8000-000000000001"]`, secret_env `ACME_DLP_WEBHOOK_SECRET`, kinds `["pii_email","pii_iban"]`) where `fields` replaces a key. One table-driven test names every rule of § 6.2:

```rust
#[test] fn each_webhook_rule_fails_and_names_the_provider_and_the_field() {
    let cases: &[(&str, &str)] = &[
        ("name = \"Acme\"", "webhook[0].name"), ("name = \"kernel\"", "webhook[0].name"), ("name = \"webhook\"", "webhook[0].name"),
        ("name = \"judge\"", "webhook[0].name"), ("name = \"other\"", "webhook[0].name"), ("name = \"built_in\"", "webhook[0].name"),
        ("url = \"http://dlp.acme.example/x\"", "webhook[acme_dlp].url"), ("url = \"https://u:p@dlp.acme.example/x\"", "webhook[acme_dlp].url"),
        ("url = \"https://dlp.acme.example/x?a=1\"", "webhook[acme_dlp].url"), ("url = \"not a url\"", "webhook[acme_dlp].url"),
        ("orgs = []", "webhook[acme_dlp].orgs"), ("orgs = \"all\"", "webhook[acme_dlp].orgs"), ("orgs = [\"not-a-uuid\"]", "webhook[acme_dlp].orgs"),
        ("secret_env = \"GATEWAY_X_SECRET\"", "webhook[acme_dlp].secret_env"), ("secret_env = \"lower\"", "webhook[acme_dlp].secret_env"),
        ("previous_secret_env = \"1BAD\"", "webhook[acme_dlp].previous_secret_env"),
        ("timeout_ms = 0", "webhook[acme_dlp].timeout_ms"), ("timeout_ms = 1951", "webhook[acme_dlp].timeout_ms"),
        ("kinds = []", "webhook[acme_dlp].kinds"), ("kinds = [\"secret_in_args\"]", "webhook[acme_dlp].kinds"),
        ("kinds = [\"other\"]", "webhook[acme_dlp].kinds"), ("kinds = [\"a\", \"a\"]", "webhook[acme_dlp].kinds"), ("kinds = [\"Bad\"]", "webhook[acme_dlp].kinds"),
        ("max_request_bytes = 0", "webhook[acme_dlp].max_request_bytes"), ("max_request_bytes = 16777217", "webhook[acme_dlp].max_request_bytes"),
        ("max_in_flight = 0", "webhook[acme_dlp].max_in_flight"), ("max_in_flight = 1025", "webhook[acme_dlp].max_in_flight"),
        ("extra_ca_bundle_path = \"\"", "webhook[acme_dlp].extra_ca_bundle_path"), ("extra_ca_bundle_path = \" /x\"", "webhook[acme_dlp].extra_ca_bundle_path"),
    ];
    for (field, want) in cases {
        let err = tp_load(&wh_toml(field)).and_then(|c| c.validate()).expect_err(field);
        assert!(err.contains(want), "{field}: {err}");
    }
    // 17 kinds, 9 providers, duplicate names, duplicate urls: one case each with their own TOML.
}
#[test] fn http_is_accepted_only_for_a_loopback_host() { /* http://127.0.0.1:9/x and http://localhost:9/x pass; http://10.0.0.1/x fails */ }
#[test] fn orgs_star_is_a_shared_provider() { /* orgs = "*" validates; org_scope() == OrgScope::All */ }
#[test] fn the_webhook_table_is_validated_when_the_detectors_are_off() { /* [tool_policy.detectors] enabled = false + name = "Acme" -> Err */ }
#[test] fn the_timeout_bound_follows_the_detectors_timeout() { /* detectors timeout_ms = 500: provider 450 passes, 451 fails */ }
#[test] fn an_unknown_webhook_key_fails() { /* typo = 1 -> extract error names "typo" */ }
#[test] fn org_webhooks_must_name_a_registered_permitted_provider() {
    // [[tool_policy.org]] id = ORG_A, webhooks = ["nobody"] -> Err containing "tool_policy.org[0].webhooks";
    // webhooks = ["acme_dlp"] with orgs = [ORG_B] -> Err; with orgs = [ORG_A] or "*" -> Ok;
    // org_webhooks() maps ORG_A -> ["acme_dlp"] (sorted, unique for ["acme_dlp","acme_dlp"]).
}
#[test] fn the_secret_loader_checks_each_secret() {
    let ok = "k".repeat(32);
    let env = |pairs: &[(&str, &str)]| pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect::<HashMap<_, _>>();
    let cfg = tp_load(&wh_toml("previous_secret_env = \"ACME_DLP_WEBHOOK_SECRET_OLD\"")).unwrap();
    let got = load_webhook_secrets(&cfg, &env(&[("ACME_DLP_WEBHOOK_SECRET", &ok), ("ACME_DLP_WEBHOOK_SECRET_OLD", &ok)])).unwrap();
    assert_eq!(got["acme_dlp"].current.expose_secret(), ok);
    for (value, why) in [(None, "is not set"), (Some("k".repeat(31)), "at least 32 bytes"), (Some(format!("{ok}\n")), "whitespace"), (Some(format!(" {ok}")), "whitespace")] {
        let e = value.map_or_else(HashMap::new, |v| env(&[("ACME_DLP_WEBHOOK_SECRET", &v), ("ACME_DLP_WEBHOOK_SECRET_OLD", &ok)]));
        let err = load_webhook_secrets(&cfg, &e).expect_err(why);
        assert!(err.contains("webhook[acme_dlp].secret_env") && err.contains(why), "{err}");
        assert!(!err.contains(&ok), "the error holds no secret");
    }
    // A missing previous secret names previous_secret_env.
}
#[test] fn the_readiness_dump_holds_no_secret_and_no_path() {
    // url "https://dlp.acme.example/v1/canary-path-77", extra_ca_bundle_path "/etc/canary-ca.pem":
    // serde_json::to_string(&cfg) and format!("{cfg:?}") hold "acme_dlp" and "dlp.acme.example",
    // not "canary-path-77", not "canary-ca.pem", not the secret value; "orgs":1.
}
#[test] fn the_example_webhook_block_parses() {
    // As `the_example_file_parses`: take the block that starts at "# [[tool_policy.detectors.webhook]]",
    // uncomment it, add `[tool_policy.detectors] timeout_ms = 500` and the base TOML, extract, validate:
    // one provider "acme_dlp", timeout_ms 300, failure_severity High, max_in_flight 64, retired false.
}
```
- [ ] **Step 2: Run.** `cd rs && cargo nextest run -p paigasus-gateway --lib config`. Expected: FAIL (no field `webhook`).
- [ ] **Step 3: Implement.** Use `reqwest::Url::parse` for the URL (scheme, `username()`, `password()`, `query()`), and `is_loopback_host(url)` for the `http` exception. Compare two providers' URLs by their parsed `as_str()`. `orgs` entries go through `domain::parse_org_uuid`. The `extra_ca_bundle_path` doc comment is the exact text of spec § 6.2 ("This ADDS to the trust store, it does not replace it. …"). The env-name rule and the secret checks follow D20; the secret error never holds the value. Add the commented example block (spec § 6.2 TOML, with a note that `secret_env` must not start with `GATEWAY_`).
- [ ] **Step 4: Run.** Same command, then `cargo clippy -p paigasus-gateway --all-targets -- -D warnings`. Expected: PASS.
- [ ] **Step 5: Commit.** `git add rs/crates/services/paigasus-gateway` — `feat(rs): add the webhook provider config and its secret loader (SMA-856)`.

---

### Task 4: Gateway IAM source — the opt-in list in the tenant settings

**Files:**
- Modify: `src/domain/tool_policy.rs:30-38` (`TenantSettings`)
- Modify: `src/adapters/policy_source/iam.rs:129-160` and its tests
- Modify: every `TenantSettings { mode, on_unavailable }` literal (`grep -rn "TenantSettings {" rs/crates/services/paigasus-gateway`) and the `Copy` uses in `src/application/tool_policy/request.rs:351-380`

**Interfaces:**
- Produces: `TenantSettings { pub mode: TenantMode, pub on_unavailable: OnUnavailable, pub webhooks: Arc<[String]> }` — `Debug, Clone, PartialEq, Eq`, no `Copy`. The IAM source sorts and de-duplicates the list. It keeps a name that fails the name rule. The version token is `format!("t{}:{}:{}:{}", set.version, mode.as_str(), fail, webhooks.join(","))`.

- [ ] **Step 1: Write the failing tests** (in `iam.rs` tests).

```rust
#[test] fn a_webhook_list_change_alone_changes_the_token() {
    let a = tenant_from_response(org(), response(set(3), with_hooks(&["acme_dlp"]))).unwrap();
    let b = tenant_from_response(org(), response(set(3), with_hooks(&[]))).unwrap();
    assert_eq!(a.version, "t3:enforce:closed:acme_dlp");
    assert_eq!(b.version, "t3:enforce:closed:");
}
#[test] fn an_unsorted_or_doubled_list_gives_the_sorted_token() {
    let t = tenant_from_response(org(), response(set(3), with_hooks(&["b", "a", "b"]))).unwrap();
    assert_eq!(t.version, "t3:enforce:closed:a,b");
    assert_eq!(&*t.settings.unwrap().webhooks, ["a", "b"]);
}
#[test] fn a_bad_name_keeps_the_tenant_set_and_is_not_malformed() {
    let t = tenant_from_response(org(), response(set(3), with_hooks(&["Bad Name\u{7}"]))).expect("not malformed");
    assert_eq!(t.text, "permit(principal, action, resource);");
    assert_eq!(&*t.settings.unwrap().webhooks, ["Bad Name\u{7}"]);
}
```
  (`with_hooks` = `settings(Enforce, Closed)` with `webhooks` set.) Update the existing token assertions to the new form (an empty list ends with `:`).
- [ ] **Step 2: Run.** `cd rs && cargo nextest run -p paigasus-gateway --lib policy_source`. Expected: FAIL.
- [ ] **Step 3: Implement.** The list goes into the token as stored after sort and dedupe. Fix the `Copy` uses with `.clone()`. The file source keeps `settings: None`.
- [ ] **Step 4: Run.** `cd rs && cargo nextest run -p paigasus-gateway`. Expected: PASS.
- [ ] **Step 5: Commit.** `feat(rs): read the webhook opt-in list from IAM in the gateway (SMA-856)`.

---

### Task 5: The detector port and the pipeline core

**Files:**
- Modify: `src/domain/detectors.rs:79-158` (batch, error, trait), new types below
- Modify: `src/application/detectors/pipeline.rs:93-270` and its tests `:272-627`
- Modify: `src/application/detectors/mod.rs` (the failure counter function)
- Modify: `src/application/tool_policy/testing.rs:363-447` (test detectors)
- Modify: `src/application/tool_policy/decider.rs:265-281` (`call_batch` uses `for_calls`, empty `Caller` for now — Task 7 fills it), tests near `:770-790`
- Modify: `rs/crates/libs/paigasus-observability/src/names.rs:84` (new name), `ALL`, `the_gateway_tool_policy_decision_families_are_registered`
- Modify: `src/main.rs` (`describe_counter!`), `docs/ops/RUNBOOK-observability.md:151` (a new row)

**Interfaces:**
- Produces (`crate::domain::detectors`):
  - `pub struct Caller { pub principal: Arc<str>, pub session: Option<Uuid>, pub opt_in: Arc<[String]> }`, `Caller::none()` (empty principal, no session, empty list). `Debug` prints `principal: <bool>`, `session: <bool>`, `opt_in: <count>`.
  - `DetectBatch` new fields: `pub caller: Caller`, `pub deadline: tokio::time::Instant` (`new` sets `Instant::now()`; `run` sets the cut), `pub webhook_bodies: Option<Arc<[Bytes]>>` (`new` sets `None`).
  - `pub fn for_calls(org: Option<Uuid>, request_id: Option<String>, subjects: Vec<Subject>, seen_hosts: Option<Arc<HostSet>>, caller: Caller) -> DetectBatch` (side `Response`).
  - `Detector` trait: `fn failure_severity(&self) -> Severity { Severity::High }`, `fn kinds(&self) -> Option<&BTreeSet<String>> { None }`.
  - `pub enum WebhookFailure { RequestTooLarge, Overloaded, Transport, Timeout, Deadline, Status, ResponseTooLarge, Signature, Malformed, Panic, Error, Shape, Check, UnknownProvider, NotPermitted, Retired }`, `pub fn as_str(self) -> &'static str` (the snake_case reasons of the Global Constraints), `pub const ADAPTER: [WebhookFailure; 9]` (the first nine).
  - `pub fn failure_finding(detector: &str, severity: Severity, reason: WebhookFailure) -> Finding` — kind `detector_error`, version = `reason.as_str()`, no evidence.
  - `pub fn valid_version(v: &str) -> bool` (D14) and `pub fn valid_name(n: &str) -> bool` (`[a-z0-9_]{1,64}`).
- Produces (`crate::application::detectors`): `pub fn count_webhook_failure(provider: &str, reason: WebhookFailure)` — increments `GATEWAY_TOOL_POLICY_WEBHOOK_FAILURES_TOTAL{provider, reason}` by 1.
- Produces (`paigasus_observability::names`): `pub const GATEWAY_TOOL_POLICY_WEBHOOK_FAILURES_TOTAL: &str = "gateway_tool_policy_webhook_failures_total";` in `ALL`.
- Produces (`testing.rs`): `pub(crate) struct DeclaredDetector { pub name: &'static str, pub kinds: BTreeSet<String>, pub severity: Severity, pub found: Vec<Finding> }` (fixed findings for every call); `pub(crate) struct EagerPanicDetector(pub &'static str, pub &'static str)` whose `detect` is a hand-written `fn detect(..) -> Pin<Box<dyn Future…>>` that panics before it returns the future (D15); `pub(crate) struct DeadlineDetector { pub name: &'static str, pub answered: usize }` that answers the first `answered` calls at once and waits until `batch.deadline - 10 ms` for the rest, then returns `failure_finding(name, High, Deadline)` for them (D24).

- [ ] **Step 1: Write the failing pipeline tests** (§ 7.2, in `pipeline.rs`). The helper `batch(subjects)` becomes `DetectBatch::for_calls(None, None, subjects, None, Caller::none())`.

```rust
#[tokio::test] async fn a_detector_that_panics_before_its_future_gives_its_failure_severity() {
    let eager: Arc<dyn Detector> = Arc::new(EagerPanicDetector("hook_eager", CANARY));
    let fixed: Arc<dyn Detector> = Arc::new(FixedDetector("hook_fixed", vec![other("hook_fixed", "fixed_kind")]));
    let out = pipeline(built_ins(), vec![eager, fixed], None, Duration::from_millis(500)).run(batch(vec![call(0, "{}")]), far()).await;
    assert_eq!(pairs(&out.findings[0]), [("hook_eager", DETECTOR_ERROR), ("hook_fixed", "fixed_kind")]);
    assert_eq!((out.findings[0][0].severity, out.findings[0][0].version.as_str()), (Severity::High, "panic"));
}
#[tokio::test(start_paused = true)] async fn the_backstop_cut_uses_the_failure_severity() {
    // A slow DeclaredDetector-like detector with severity Low (SlowDetector gains a severity field or a
    // SlowLowDetector) -> [Finding("hook_slow", "detector_error", Low, "deadline")].
}
#[tokio::test(start_paused = true)] async fn an_adapter_that_honours_the_deadline_returns_per_call_results() {
    let d: Arc<dyn Detector> = Arc::new(DeadlineDetector { name: "hook_dl", answered: 2 });
    let out = pipeline(built_ins(), vec![d], None, Duration::from_millis(500)).run(batch(vec![call(0, "{}"), call(1, "{}"), call(2, "{}")]), far()).await;
    assert_eq!(out.findings[0].iter().map(|f| f.version.as_str()).collect::<Vec<_>>(), ["ok"]);  // DeadlineDetector answers kind "fine_kind", version "ok"
    assert_eq!(out.findings[2][0].version, "deadline");
}
#[tokio::test] async fn a_strict_check_replaces_a_finding_under_another_name_a_built_in_kind_or_an_undeclared_kind() {
    for bad in [Finding::new("kernel", "pii_email", Severity::Medium, "v1"), Finding::new("hook_a", SECRET_IN_ARGS, Severity::Medium, "v1"),
                Finding::new("hook_a", "pii_iban", Severity::Medium, "v1"), Finding::new("hook_a", "pii_email", Severity::Medium, "V1!"),
                Finding::new("hook_a", "pii_email", Severity::Medium, &"v".repeat(33))] {
        let d = DeclaredDetector { name: "hook_a", kinds: ["pii_email".to_owned()].into(), severity: Severity::Low, found: vec![bad.clone()] };
        let out = pipeline(built_ins(), vec![Arc::new(d)], None, Duration::from_millis(500)).run(batch(vec![call(0, "{}")]), far()).await;
        assert_eq!(out.findings[0], vec![failure_finding("hook_a", Severity::Low, WebhookFailure::Check)], "{bad:?}");
    }
}
#[tokio::test] async fn each_rebuild_keeps_the_caller_the_deadline_and_the_bodies() {
    // A RecordingDetector webhook and a RecordingDetector judge. The source batch has caller
    // Caller{principal "prn:x", session Some(Uuid::nil()), opt_in []} and webhook_bodies = Some(b), set by
    // the test. Both recorded batches have principal "prn:x", session Some(nil), deadline == the cut
    // (start + 500 ms, paused time), and webhook_bodies that are Arc::ptr_eq with b. (Task 7 then makes
    // `run` build the bodies itself and changes this test to compare with the built bodies.)
}
#[tokio::test] async fn a_webhook_failure_counts_its_provider_and_reason() {
    // DebuggingRecorder; ErrDetector("hook_a") on two calls -> counter
    // gateway_tool_policy_webhook_failures_total{provider="hook_a",reason="error"} == 2.
}
```
  Rewrite the C3 tests: `a_bad_kind_or_an_unknown_detector_name_is_replaced` now expects `other("nobody", "fine_kind")` and `other("kernel", 64 k)` to be replaced, and the combined run gives `[("hook_a","fine_kind"), ("hook_a", DETECTOR_ERROR)]` with the last finding's version `"check"`. Replace `same_kind_from_two_stages_keeps_both` by `a_webhook_finding_under_a_built_in_kind_is_replaced` (a `DeclaredDetector` with kinds `{pii_email}` that returns `secret_in_args` → built-in `secret_in_args` stays, the webhook gives `failure_finding("hook_a", Low, Check)`). The existing tests that compare to `detector_error("hook_x")` now compare to `failure_finding("hook_x", Severity::High, <reason>)`: `Failed`→`Error`, `WrongShape`→`Shape`, `Timeout`→`Deadline`, `Panicked`→`Panic`.
- [ ] **Step 2: Run.** `cd rs && cargo nextest run -p paigasus-gateway --lib detectors`. Expected: FAIL.
- [ ] **Step 3: Implement.**
  - `run` sets `call_batch.deadline = cut`. Both rebuilds copy `caller`, `deadline` and `webhook_bodies` (struct update with the copied fields, not `..DetectBatch::new`).
  - D15: `guarded("webhook", async move { d.detect(b).await })` and the same at the judge site.
  - `failed` becomes `fn failed(stage: Stage, d: &dyn Detector, error: DetectorError, m: usize) -> Vec<Vec<Finding>>`: each call gets `failure_finding(d.name(), d.failure_severity(), reason)`; for `Stage::Webhook` it calls `count_webhook_failure(d.name(), reason)` once per call. The built-in failure path keeps `detector_error(DETECTOR_BUILT_IN)`.
  - `check_late(stage, d, found)`: for the webhook stage, a finding is kept when `f.detector == d.name()`, `valid_kind(&f.kind)`, `d.kinds().is_none_or(|k| k.contains(&f.kind))` and `valid_version(&f.version)`. For the judge stage: today's rule plus `valid_version`. When any finding is dropped, push one `failure_finding(d.name(), d.failure_severity(), Check)` (counted for the webhook stage).
  - Add the metric name, the `ALL` entry, the registry test line, one `describe_counter!` line ("Tool calls whose webhook provider failed, labelled by the configured provider name and a fixed reason (SMA-856).") and the observability runbook row (`provider`, `reason`; the 15 reasons; `provider="unknown"` for `unknown_provider`; no tenant content).
  - `call_batch` uses `DetectBatch::for_calls(org, request_id, subjects, seen, Caller::none())`.
  - In `decider.rs`, rewrite `a_webhook_finding_that_names_the_kernel_counts_as_webhook` to `a_webhook_finding_that_names_the_kernel_is_replaced_and_counts_as_webhook`: the record holds `("detector_error", "hook_a")` with version `check`, and the counter `(detector_error, webhook, high)` is 1 and `(detector_error, kernel, high)` is 0.
- [ ] **Step 4: Run.** `cd rs && cargo nextest run -p paigasus-gateway -p paigasus-observability` and `cargo clippy -p paigasus-gateway --all-targets -- -D warnings`. Expected: PASS.
- [ ] **Step 5: Commit.** `feat(rs): give the detector port failure severities, kinds and the caller (SMA-856)`.

---

### Task 6: The wire protocol module (pure) and the kernel strict-parse check

**Files:**
- Modify: `rs/crates/libs/paigasus-kernel/src/tool_policy/walk.rs:99-123`, `tool_policy/mod.rs:17-21`
- Create: `rs/crates/services/paigasus-gateway/src/domain/webhook_protocol.rs`; Modify: `src/domain/mod.rs`
- Modify: `rs/crates/services/paigasus-gateway/Cargo.toml` (`serde_json = { workspace = true, features = ["raw_value"] }`)

**Interfaces:**
- Produces (`paigasus_kernel::tool_policy`): `pub fn parses_strictly(input: &ToolInput) -> bool` = `!parse_arguments(input).unparsed`.
- Produces (`crate::domain::webhook_protocol`):
  - `pub const PROTOCOL_VERSION: &str = "1"`, `pub const MAX_RESPONSE_BYTES: usize = 65_536`, `pub const MAX_FINDINGS: usize = 16`, header-name constants `HEADER_VERSION = "paigasus-webhook-version"`, `HEADER_NONCE = "paigasus-nonce"`, `HEADER_SIGNATURE = "paigasus-signature"`.
  - `pub fn call_body(batch: &DetectBatch, subject: &Subject, built: &[Finding]) -> Option<Bytes>` — `None` for a result subject.
  - `pub fn hex_lower(bytes: &[u8]) -> String`; `pub fn hex_decode_lower(s: &str) -> Option<Vec<u8>>` (rejects upper case and odd length).
  - `pub fn request_mac(secret: &[u8], name: &str, t: u64, nonce: &str, body: &[u8]) -> String` (64 hex).
  - `pub fn request_signature_header(t: u64, mac_hex: &str) -> String` = `t=<t>,v1=<mac>`.
  - `pub fn response_mac(secret: &[u8], name: &str, nonce: &str, body: &[u8]) -> String` (for the test mock and the doc example).
  - `pub fn verify_response(secrets: &[&[u8]], name: &str, nonce: &str, body: &[u8], header: &str) -> Result<(), WebhookFailure>` — parses `v1=<64 hex>[,v1=<64 hex>]` strictly (no spaces, lower-case only, 1 or 2 values), verifies each value with each secret through `Mac::verify_slice`. Any failure is `Signature`.
  - `pub fn content_type_ok(value: &str) -> bool` — the media type before `;`, trimmed, equals `application/json` ignoring ASCII case.
  - `pub fn parse_answer(name: &str, body: &[u8], kinds: &BTreeSet<String>) -> Result<Vec<Finding>, WebhookFailure>` — serde structs with `deny_unknown_fields` (`{ findings: Vec<{ kind, severity, version? }> }`); severity in `info|low|medium|high`; kind in `kinds`; `valid_version`; at most 16; each finding is `Finding::new(name, kind, severity, version)` with no evidence. Any failure is `Malformed`.

- [ ] **Step 1: Write the failing tests** (in `webhook_protocol.rs`). The worked example (spec § 6.9) uses these fixed values; the doc in Task 12 copies them:

```rust
const SECRET: &[u8] = b"paigasus-webhook-example-secret-0001";
const NAME: &str = "acme_dlp";
const T: u64 = 1_764_300_000;
const NONCE: &str = "00112233445566778899aabbccddeeff";
const BODY: &str = r#"{"version":1,"request_id":"req_example","subject":"prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-000000000004","org":"6f1c0000-0000-4000-8000-000000000001","session":"9a2e0000-0000-4000-8000-000000000002","tool":{"name":"send_email","declared":"yes"},"args":{"to":"x@y.example","body":"hi"},"args_text":null,"findings":[]}"#;
const RESPONSE: &str = r#"{"findings":[{"kind":"pii_email","severity":"medium","version":"2026.11"}]}"#;
const REQUEST_MAC: &str = "4929f346b96d703a5ab953c6e8981444573fa64e66795b8a5d61ba680c5ba878";
const RESPONSE_MAC: &str = "cf6147bdf8be4b7c7b4619accf9b1f592a14d6d9209e112a1a86c7e00d6f5638";

#[test] fn the_worked_example_matches_byte_for_byte() {
    // A batch for_calls(Some(org), Some("req_example"), [call send_email Json(r#"{"to":"x@y.example","body":"hi"}"#) Declared::Yes],
    // None, Caller{principal, Some(session), []}) and no built-in findings:
    assert_eq!(call_body(&batch, &batch.subjects[0], &[]).unwrap(), BODY.as_bytes());
    assert_eq!(request_mac(SECRET, NAME, T, NONCE, BODY.as_bytes()), REQUEST_MAC);
    assert_eq!(request_signature_header(T, REQUEST_MAC), format!("t={T},v1={REQUEST_MAC}"));
    assert_eq!(response_mac(SECRET, NAME, NONCE, RESPONSE.as_bytes()), RESPONSE_MAC);
    assert_eq!(verify_response(&[SECRET], NAME, NONCE, RESPONSE.as_bytes(), &format!("v1={RESPONSE_MAC}")), Ok(()));
    // The doc holds the same values: include_str!("../../../../../../docs/tool-policies/webhook-protocol.md")
    // contains REQUEST_MAC, RESPONSE_MAC, NONCE and BODY. (Add this assertion in Task 12, with the doc.)
}
#[test] fn each_bad_response_signature_is_signature() {
    let good = format!("v1={RESPONSE_MAC}");
    let other_nonce = response_mac(SECRET, NAME, "ffeeddccbbaa99887766554433221100", RESPONSE.as_bytes());
    let other_name = response_mac(SECRET, "other_dlp", NONCE, RESPONSE.as_bytes());
    let mut changed = RESPONSE.as_bytes().to_vec(); changed[3] ^= 1;
    assert_eq!(verify_response(&[SECRET], NAME, NONCE, &changed, &good), Err(WebhookFailure::Signature));
    for h in [format!("v1={other_nonce}"), format!("v1={other_name}"), good.to_uppercase(), format!("v1={}", &RESPONSE_MAC[..63]),
              format!("v1={RESPONSE_MAC}, v1={RESPONSE_MAC}"), format!("v1={RESPONSE_MAC},v1={RESPONSE_MAC},v1={RESPONSE_MAC}"),
              format!("V1={RESPONSE_MAC}"), format!("t=1,v1={RESPONSE_MAC}"), String::new()] {
        assert_eq!(verify_response(&[SECRET], NAME, NONCE, RESPONSE.as_bytes(), &h), Err(WebhookFailure::Signature), "{h}");
    }
}
#[test] fn the_previous_secret_and_a_second_value_are_accepted() {
    let old = b"paigasus-webhook-example-secret-OLD1";
    let by_old = response_mac(old, NAME, NONCE, RESPONSE.as_bytes());
    assert_eq!(verify_response(&[SECRET, old], NAME, NONCE, RESPONSE.as_bytes(), &format!("v1={by_old}")), Ok(()));
    assert_eq!(verify_response(&[SECRET], NAME, NONCE, RESPONSE.as_bytes(), &format!("v1={by_old},v1={RESPONSE_MAC}")), Ok(()));
}
#[test] fn the_content_type_check_reads_the_media_type_only() {   // Review Focus 2
    for ok in ["application/json", "application/json; charset=utf-8", "Application/JSON;charset=UTF-8"] { assert!(content_type_ok(ok), "{ok}"); }
    for bad in ["application/jsonp", "application/json-seq", "text/json", "application/problem+json", ""] { assert!(!content_type_ok(bad), "{bad}"); }
}
#[test] fn each_bad_answer_is_malformed() {
    let kinds: BTreeSet<String> = ["pii_email".to_owned()].into();
    let seventeen = format!(r#"{{"findings":[{}]}}"#, vec![r#"{"kind":"pii_email","severity":"low"}"#; 17].join(","));
    for bad in [r#"{"findings":[],"extra":1}"#, r#"{}"#, r#"{"findings":[{"kind":"pii_email","severity":"urgent"}]}"#,
                r#"{"findings":[{"kind":"pii_iban","severity":"low"}]}"#, r#"{"findings":[{"kind":"pii_email","severity":"low","version":"Bad!"}]}"#,
                r#"{"findings":[{"kind":"pii_email","severity":"low","detector":"kernel"}]}"#, r#"{"findings":[{"kind":"pii_email"}]}"#,
                "not json", &seventeen] {
        assert_eq!(parse_answer(NAME, bad.as_bytes(), &kinds), Err(WebhookFailure::Malformed), "{bad}");
    }
    assert_eq!(parse_answer(NAME, br#"{"findings":[]}"#, &kinds), Ok(vec![]));
    let one = parse_answer(NAME, br#"{"findings":[{"kind":"pii_email","severity":"medium"}]}"#, &kinds).unwrap();
    assert_eq!((one[0].detector.as_str(), one[0].version.as_str(), one[0].severity), (NAME, "", Severity::Medium));
}
#[test] fn a_duplicate_key_in_the_answer_is_malformed() {   // Review Focus 3
    let kinds: BTreeSet<String> = ["pii_email".to_owned()].into();
    for bad in [r#"{"findings":[],"findings":[{"kind":"pii_email","severity":"low"}]}"#, r#"{"findings":[{"kind":"pii_email","kind":"pii_email","severity":"low"}]}"#] {
        assert_eq!(parse_answer(NAME, bad.as_bytes(), &kinds), Err(WebhookFailure::Malformed), "{bad}");
    }
}
#[test] fn the_arguments_go_verbatim_or_as_text() {
    // batch_with(input) = the worked-example batch with this one input.
    let body = |input: ToolInput| -> Value { let b = batch_with(input); serde_json::from_slice(&call_body(&b, &b.subjects[0], &[]).unwrap()).unwrap() };
    // Duplicate key and invalid JSON -> args null, args_text == the raw text; ToolInput::Text -> args null, args_text the text.
    // A 25-digit number (PD15) and a 19-digit number are strict-parsed: the raw body bytes contain the input text
    // byte for byte (e.g. r#"{"n":1234567890123456789012345}"#), and args_text is null.
    // Exactly one of args and args_text is null in each case.
}
#[test] fn the_built_in_findings_carry_their_evidence() {
    // built = [Finding("secret_in_args","secret_in_args",High,"v1").with_evidence(Evidence::argument("github_pat","/body")),
    //          Finding("unseen_host","unseen_host",Medium,"v1").with_evidence(Evidence::host("url",Some("/u"),"a.example".into())),
    //          Finding("kernel","unparsed_arguments",Info,"").with_evidence(Evidence::text_input("free_text"))]
    // -> findings[0].evidence == [{"rule":"github_pat","location":"argument","pointer":"/body"}],
    //    findings[1].evidence[0]["host"] == "a.example", findings[2].evidence == [{"rule":"free_text","location":"text_input"}].
    // request_id and session are null when absent; tool.declared is "yes" | "no" | "unknown".
}
#[test] fn hex_round_trips_and_rejects_upper_case() { /* hex_lower([0,255,16]) == "00ff10"; decode("00FF10") == None; decode("abc") == None */ }
```
  In the kernel, add `parses_strictly_follows_the_unparsed_rule` in `walk.rs` tests: `{"a":1}` true; `{"a":1,"a":2}`, `[1]`, `not json`, `ToolInput::Text` false; `{"n":1234567890123456789012345}` true.
- [ ] **Step 2: Run.** `cd rs && cargo nextest run -p paigasus-kernel --features tool-policy parses_strictly && cargo nextest run -p paigasus-gateway --lib webhook_protocol`. Expected: FAIL.
- [ ] **Step 3: Implement.** `call_body` serialises a `#[derive(Serialize)]` struct with the field order of § 6.3 through `serde_json::to_vec`. `args` is `Option<&RawValue>` from `RawValue::from_string(text)` only when `parses_strictly(input)`; else `args_text` is `Some(text)`. Evidence: `location` is `"argument"` or `"text_input"`; an entry with `Location::Result` is left out (it never occurs on a call). Use `hmac::Hmac<sha2::Sha256>` with `new_from_slice`; the MAC input is fed in parts with `update`. Hex is a 16-byte lookup table.
- [ ] **Step 4: Run.** Same commands, plus `cargo clippy --workspace --all-targets -- -D warnings`. Expected: PASS.
- [ ] **Step 5: Commit.** `git add rs/crates/libs/paigasus-kernel rs/crates/services/paigasus-gateway rs/Cargo.lock` — `feat(rs): add the webhook wire protocol and the kernel strict-parse check (SMA-856)`.

---

### Task 7: The opt-in path, the registry and the shared bodies

**Files:**
- Modify: `src/domain/detectors.rs` (registry), `src/application/detectors/pipeline.rs:93-223`
- Modify: `rs/crates/libs/paigasus-gateway-core/src/policy/snapshot.rs:97-151,180-219`
- Modify: `src/application/tool_policy/request.rs:351-380,456-540` (D8), `decider.rs:265-281,470-500` (`call_batch`)
- Modify: `src/application/tool_policy/cache.rs:555-640` (the warn), `:252` (setter)
- Modify (tests that must opt in): `src/application/tool_policy/decider.rs` tests, `src/application/decision_log.rs:355-392`, `src/adapters/http/interception.rs:1078-1086,1249-1256` tests, `tests/tool_history.rs:804-812`, `tests/tool_policy.rs` (`Det` and the four sites at `:1854,1901,2272,2289`)

**Interfaces:**
- Consumes: Task 4 `TenantSettings.webhooks`; Task 5 `Caller`, `for_calls`, `failure_finding`, `WebhookFailure`, `count_webhook_failure`, `valid_name`; Task 6 `call_body`; Task 3 `OrgScope`.
- Produces:
  - (`domain::detectors`) `pub struct WebhookEntry { pub detector: Arc<dyn Detector>, pub orgs: OrgScope, pub retired: bool }`, `WebhookEntry::shared(detector)` (All, not retired); `pub struct WebhookRegistry` with `new(Vec<WebhookEntry>)`, `len()`, `is_empty()`, `iter()`, `pub fn resolve(&self, name: &str, org: Option<Uuid>) -> OptIn<'_>`; `pub enum OptIn<'a> { Run(&'a WebhookEntry), NotPermitted, Retired(&'a WebhookEntry), Unknown }`; `impl From<Vec<Arc<dyn Detector>>> for WebhookRegistry`. `OrgScope::Only(set)` permits `Some(org)` in the set only; `OrgScope::All` permits any batch.
  - `DetectorPipeline::new(built_in, webhooks: impl Into<WebhookRegistry>, judge, timeout)`; `pub fn registry(&self) -> &Arc<WebhookRegistry>`. `known_detector` reads the registry names.
  - (gateway-core) `PolicySnapshot.webhooks: Arc<[String]>` (empty in `new`), `pub fn with_webhooks(self, list: Arc<[String]>) -> Self`.
  - `RequestToolPolicy::with_org_webhooks(self, map: HashMap<Uuid, Arc<[String]>>) -> Self`.
  - `PolicyCache::set_webhook_registry(&self, registry: Arc<WebhookRegistry>)` (a `OnceLock`).

- [ ] **Step 1: Write the failing pipeline tests** (§ 7.2 D7/D25/D27/A5, in `pipeline.rs`). Helper `opted(subjects, org, names)` = `for_calls(org, None, subjects, None, Caller { principal: "prn:t".into(), session: None, opt_in: names.into() })`.

```rust
#[tokio::test] async fn an_unknown_a_forbidden_and_a_retired_opt_in_each_give_their_reason() {
    let hook = RecordingDetector::new("acme_dlp", vec![]);
    let registry = WebhookRegistry::new(vec![
        WebhookEntry { detector: hook.clone(), orgs: OrgScope::Only([org_b()].into()), retired: false },
        WebhookEntry { detector: Arc::new(DeclaredDetector { name: "old_dlp", severity: Severity::Low, .. }), orgs: OrgScope::All, retired: true },
    ]);
    let p = DetectorPipeline::new(built_ins(), registry, None, Duration::from_millis(500));
    let out = p.run(opted(vec![call(0, "{}"), call(1, "{}")], Some(org_a()), &["acme_dlp", "nobody", "old_dlp"]), far()).await;
    for i in 0..2 {
        assert_eq!(out.findings[i], vec![failure_finding("acme_dlp", Severity::High, WebhookFailure::NotPermitted),
                                          failure_finding("nobody", Severity::High, WebhookFailure::UnknownProvider),
                                          failure_finding("old_dlp", Severity::Low, WebhookFailure::Retired)]);
        assert_eq!(out.stages[i], vec![Stage::Webhook; 3]);
    }
    assert!(hook.batches().is_empty(), "a forbidden provider gets no call (R2)");
    // counter: {provider="acme_dlp",reason="not_permitted"} == 2, {provider="unknown",reason="unknown_provider"} == 2, {provider="old_dlp",reason="retired"} == 2.
}
#[tokio::test] async fn an_invalid_opt_in_name_is_never_a_detector_name_or_a_log_field() {   // Review Focus 5
    // opt_in ["Bad Name\u{7}"] -> out.findings[0] == [failure_finding("webhook", High, UnknownProvider)].
}
#[tokio::test] async fn a_registered_webhook_that_is_not_opted_in_gets_no_call() {
    // RecordingDetector "hook_a" registered (shared), opt_in [] -> hook.batches().is_empty() and no webhook finding.
}
#[tokio::test] async fn the_bodies_are_built_once_and_shared() {
    // Two RecordingDetectors, both opted in: both recorded batches have webhook_bodies that are Arc::ptr_eq,
    // and bodies[j] == call_body(&batch, &subjects[j], &built_findings[j]).
}
```
  In `snapshot.rs` tests: `with_webhooks_sets_the_list_and_a_clone_keeps_it`.
- [ ] **Step 2: Write the failing request-side and decider tests.**

```rust
// request.rs — D8 on both paths:
// the_snapshot_holds_the_opt_in_only_for_an_evaluated_log_only_or_enforce_tenant:
//   file path: with_org_webhooks({ORG_A: ["acme_dlp"]}); mode Enforce -> snapshot.webhooks == ["acme_dlp"]; LogOnly -> same;
//   Off -> []; a tenant text that does not compile (unavailable tenant part) -> [].
//   IAM path: FakeSource.set_settings(Some(TenantSettings{mode: Enforce, on_unavailable: Closed, webhooks: ["acme_dlp"]}))
//   with settings_from_source -> ["acme_dlp"]; mode Off -> [].
// decider.rs:
// the_call_batch_carries_the_caller: a RecordingDetector "hook_a"; ctx with principal PRINCIPAL, a session, and
//   a snapshot .with_webhooks(["hook_a"]) -> the recorded batch has principal PRINCIPAL, the session id and opt_in ["hook_a"].
// a_hold_with_an_unavailable_tenant_part_sends_no_webhook (PD5): a history policy whose window read fails
//   (as the SMA-753 P7 tests do) -> hook.batches().is_empty().
// cache.rs:
// a_new_entry_warns_once_per_unknown_or_forbidden_name: registry {acme_dlp: Only(ORG_B)}; tenant settings webhooks
//   ["acme_dlp", "nobody", "Bad\u{7}"] -> capture_logs holds one warn per name with name="acme_dlp" reason="not_permitted",
//   name="nobody" reason="unknown_provider", name="invalid" reason="unknown_provider"; no "Bad"; a second load with
//   the same token writes no new warn.
```
- [ ] **Step 3: Run.** `cd rs && cargo nextest run -p paigasus-gateway-core snapshot && cargo nextest run -p paigasus-gateway --lib`. Expected: FAIL.
- [ ] **Step 4: Implement.**
  - `late_stages` resolves `batch.caller.opt_in` in its (sorted) order. `Run` → the detector runs; `NotPermitted` → `failure_finding(name, High, NotPermitted)`; `Retired(e)` → `failure_finding(name, e.detector.failure_severity(), Retired)`; `Unknown` → `failure_finding(if valid_name(name) { name } else { "webhook" }, High, UnknownProvider)`, counted with provider `"unknown"`. Each failure gives one finding per call, `Stage::Webhook`, counted once per call.
  - Only when at least one provider runs, `run` builds `Arc<[Bytes]>` with `call_body(&batch, &batch.subjects[calls[j]], &out.findings[calls[j]])` and puts it into the call batch. Change the Task 5 rebuild test to compare with these built bodies.
  - `late` returns `Vec<(Stage, Vec<Vec<Finding>>)>`, so `run` no longer counts webhooks by index.
  - Request side: after `PolicySnapshot::new(set, mode, on)`, call `with_webhooks(opt_in_for(&set, mode, list))`, where `fn opt_in_for(set: &PolicySet, mode: TenantMode, list: Arc<[String]>) -> Arc<[String]>` gives `list` only for `mode` in `{LogOnly, Enforce}` and `PolicySet::Available { tenant: TenantPart::Evaluated, .. }`, else empty. The IAM path takes `list` from `fin.webhooks`; the file path from the `with_org_webhooks` map (empty when the org has no entry, D9).
  - `call_batch` gets `Caller { principal: Arc::from(ctx.principal.as_str()), session: ctx.session.as_ref().map(|s| s.session_id), opt_in }`, where `opt_in` is empty when `slots.tenant_unavailable` (PD5), else `snapshot.webhooks`. Move the `call_batch` call after `hold_slots`.
  - Cache: when `load` stores a new tenant entry with settings, for each name of `settings.webhooks` that `resolve` gives `Unknown` or `NotPermitted`, write `tracing::warn!(org = %org, name, reason, "a tenant opts in to a webhook provider that this gateway cannot run for it")` with `name` the name when `valid_name`, else `"invalid"`.
  - Opt the existing webhook tests in: find them with `grep -rn "DetectorPipeline::new" rs/crates/services/paigasus-gateway`. A test whose pipeline holds a webhook and asserts on its finding must opt the org in (decider tests: a helper that wraps the snapshot with `with_webhooks`; `tests/tool_policy.rs`: `Det` gets `opt_in: Option<Vec<String>>`, default `Some(names of det.webhooks)`, passed to `with_org_webhooks({org_a(): …})`). A test that asserts that a webhook does not run stays as it is.
- [ ] **Step 5: Run.** `cd rs && cargo nextest run -p paigasus-gateway-core -p paigasus-gateway` and clippy. Expected: PASS, with the Task 0 counts plus the new tests.
- [ ] **Step 6: Commit.** `feat(rs): resolve the webhook opt-in and share one body per call (SMA-856)`.

---

### Task 8: The webhook adapter and the shared CA client builder

**Files:**
- Modify: `src/adapters/upstream.rs:249-345` (split)
- Create: `src/adapters/detectors/webhook.rs`; Modify: `src/adapters/detectors/mod.rs`
- Test: unit tests in `webhook.rs` and `upstream.rs`

**Interfaces:**
- Consumes: Task 3 `WebhookConfig`, `ProviderSecrets`; Task 5 trait, `WebhookFailure`, `failure_finding`, `count_webhook_failure`; Task 6 protocol functions; `domain::limits::Clock`.
- Produces:
  - (`upstream.rs`) `pub fn build_extra_ca_client(label: &str, extra_ca_bundle_path: Option<&str>, connect_timeout: Duration, configure: impl Fn(reqwest::ClientBuilder) -> reqwest::ClientBuilder) -> Result<reqwest::Client, ExtraCaError>` and `pub(crate) fn build_extra_ca_client_with_control(…, control_build_ok: impl FnOnce() -> bool)`. `pub enum ExtraCaError { Build { label: String, source: reqwest::Error }, CaBundle { label: String, path: String, source: Box<dyn Error + Send + Sync> } }` (thiserror; the message names the label and the path). The base builder splits into `plain_builder(connect_timeout)` (provider install, no redirects, connect timeout) and `base_builder` = `plain_builder(..).read_timeout(..)`. `build_client_with_control` maps the shared core to `UpstreamError` with byte-unchanged texts (the existing tests pin them).
  - (`webhook.rs`) `pub trait NonceSource: Send + Sync { fn nonce16(&self) -> Option<[u8; 16]>; }`, `pub struct OsNonce;` (`rand::rngs::SysRng` through `try_fill_bytes`; an error gives `None`).
  - `pub struct WebhookDetector` (fields of spec § 6.5 plus `max_in_flight: usize` and the warn limiter), `impl Detector` with `name`, `failure_severity`, `kinds`, `detect`.
  - `pub fn WebhookDetector::from_config(cfg: &WebhookConfig, secrets: ProviderSecrets, clock: Arc<dyn Clock>, nonce: Arc<dyn NonceSource>) -> Result<WebhookDetector, ExtraCaError>` — label `tool_policy.detectors.webhook[<name>].extra_ca_bundle_path`; the client: `configure = |b| b.pool_max_idle_per_host(max_in_flight).pool_idle_timeout(Duration::from_secs(90))`, connect timeout = `timeout_ms` (PD9), HTTP/1.1 only (the workspace has no `http2`).
  - `pub fn prime_webhook_failure_metrics(providers: &[&str])` — `absolute(0)` for each provider × `WebhookFailure::ADAPTER`.
  - A hand-written `Debug`: `WebhookDetector { name: …, host: …, timeout: …, retired: … }` without the secret, path or URL path.

- [ ] **Step 1: Write the failing adapter tests** (§ 7.1, § 7.5). A test helper spawns an axum mock on `127.0.0.1:0` (`http://` is allowed for loopback) with a scripted handler per call index; the detector uses a fixed clock (`T`) and a fixed nonce (`NONCE`) from Task 6. Use the detector `timeout_ms = 2000` unless a test says otherwise.

```rust
#[tokio::test] async fn a_request_carries_the_signed_headers_and_the_worked_example_body() {
    // mock records headers and body: content-type application/json, accept-encoding identity,
    // paigasus-webhook-version 1, paigasus-nonce NONCE, paigasus-signature format!("t={T},v1={REQUEST_MAC}") for the
    // worked-example batch; the body == BODY. Answer RESPONSE signed with RESPONSE_MAC ->
    // [[Finding("acme_dlp","pii_email",Medium,"2026.11")]].
}
#[tokio::test] async fn each_response_failure_gives_its_reason() {
    // Scripted answers, one per case, each -> [[failure_finding("acme_dlp", High, reason)]]:
    // 500 -> Status; 302 with Location -> Status (not followed: the Location target records no request);
    // Content-Encoding gzip -> Malformed; 65537-byte body -> ResponseTooLarge; no signature header -> Signature;
    // two Paigasus-Signature headers -> Signature; a signature by the wrong secret -> Signature;
    // text/plain -> Malformed; a signed invalid JSON -> Malformed; a signed undeclared kind -> Malformed;
    // a closed port (bind then drop a listener) -> Transport.
    // Each case also checks the counter {provider="acme_dlp", reason} == 1.
}
#[tokio::test] async fn a_response_signed_with_the_previous_secret_is_accepted() { /* previous_secret set; mock signs with it -> finding */ }
#[tokio::test] async fn the_size_limits_are_inclusive() {   // Review Focus 4
    // A signed body of exactly 65536 bytes (valid JSON padded with spaces inside the object) is read;
    // max_request_bytes = BODY.len(): the call is sent; max_request_bytes = BODY.len() - 1: RequestTooLarge and
    // the mock records 0 requests.
}
#[tokio::test] async fn the_provider_timeout_and_the_deadline_give_different_reasons() {
    // mock sleeps 1 s. timeout_ms = 100, deadline far -> Timeout. timeout_ms = 1900, deadline = now + 150 ms -> Deadline.
}
#[tokio::test] async fn a_cut_inside_the_10_ms_margin_sends_nothing() {   // Review Focus 1
    // deadline = now + 5 ms, and a deadline already in the past -> every call Deadline, mock records 0 requests, no panic.
}
#[tokio::test] async fn a_slow_first_call_does_not_hold_the_others() {
    // 10 calls; the mock holds call 0 on a tokio::sync::Barrier(2) that the test passes only after calls 1-9 finished
    // (a counter in the mock reaches 9). Result: 10 vectors in subject order; calls 1-9 have their findings.
}
#[tokio::test] async fn no_free_permit_within_the_bound_is_overloaded() {
    // max_in_flight = 1, timeout_ms = 300; the mock holds call 0 until the test releases it -> call 1 Overloaded,
    // call 0 its answer.
}
#[tokio::test] async fn the_warn_is_rate_limited_and_holds_no_content() {
    // Three Status failures in one batch -> capture_logs holds one warn with detector="acme_dlp", reason="status",
    // and a later warn after 10 s (paused time) reports suppressed=2. No log holds the URL path, the body, the
    // nonce, the signature or the secret (canary strings).
}
#[test] fn debug_holds_no_secret_body_path_or_argument() { /* format!("{d:?}") holds "acme_dlp" and the host only */ }
#[test] fn prime_sets_every_adapter_reason_to_zero() { /* two providers -> 18 series == 0 */ }
// § 7.5 in upstream.rs (follow SMA-558, PD16):
#[test] fn a_webhook_client_with_an_rcgen_bundle_builds() { /* build_extra_ca_client(label, Some(path), …) is Ok */ }
#[test] fn a_bad_webhook_bundle_names_the_provider_and_the_path() {
    // missing file, a key-only PEM, an undecodable PEM -> ExtraCaError::CaBundle; the Display holds
    // "tool_policy.detectors.webhook[acme_dlp].extra_ca_bundle_path" and the path.
}
#[test] fn the_webhook_bundle_attribution_follows_the_control_build() { /* control true -> the bundle; control false -> both */ }
```
- [ ] **Step 2: Run.** `cd rs && cargo nextest run -p paigasus-gateway --lib webhook upstream`. Expected: FAIL.
- [ ] **Step 3: Implement.**
  - `detect` builds `(index, future)` pairs for the call subjects and drives them with `futures::stream::iter(..).buffer_unordered(8)`. It puts each result back by index. It never returns `Err` and spawns nothing.
  - Per call: `bound = min(now + timeout, deadline.checked_sub(10 ms))`; a `None` or past bound is `Deadline` with no request. The reason of an elapsed bound is `Timeout` when `now + timeout` was the smaller bound, else `Deadline`. A permit wait that the bound ends is `Overloaded`.
  - Inside the bound: body from `batch.webhook_bodies` (a missing body is `Error`, counted); size check; permit; nonce (PD8); `t` from the clock; sign; `POST` with `HeaderValue::set_sensitive(true)` on the signature; the response checks in the order of § 6.3 (status, encoding, the body read as a stream cut at 65536, the one signature header, verify with current then previous, content type, parse). Use the Task 6 functions.
  - Every failure: `failure_finding(name, failure_severity, reason)`, `count_webhook_failure(name, reason)`, and the rate-limited warn (`detector`, `reason`, `request_id`; one per provider and reason per 10 s; the next warn carries `suppressed`). Never log a `reqwest::Error`.
- [ ] **Step 4: Run.** Same command, then `cargo nextest run -p paigasus-gateway` and clippy. Expected: PASS.
- [ ] **Step 5: Commit.** `feat(rs): add the signed webhook detector adapter (SMA-856)`.

---

### Task 9: Injected finding labels and the overhead without webhook wait time

**Files:**
- Modify: `src/application/tool_policy/decider.rs:104-216,320-375` and tests `:695-705,830-875`
- Modify: `src/application/tool_policy/request.rs:28,548-556`, `src/application/tool_history.rs:33,387-393`, `src/application/tool_policy/mod.rs:16`
- Modify: `src/domain/detectors.rs:180-200` (`PipelineOutput.late_elapsed`), `src/application/detectors/pipeline.rs` (sets it)
- Modify: `src/runtime.rs:360,593-610`, `tests/tool_policy.rs:2253`

**Interfaces:**
- Produces:
  - `pub struct FindingLabels` with `pub fn built_in() -> Self`, `pub fn with_webhook_kinds<'a>(kinds: impl IntoIterator<Item = &'a str>) -> Self`, `pub fn labels(&self, f: &FindingRecord, stage: Option<Stage>) -> (&str, &'static str, &'static str)`, `pub fn triples(&self, side: &'static str) -> Vec<(&str, &'static str, &'static str)>` (the prime list: the const table, plus on `response` the webhook triples, plus the four `other` triples), `pub fn shared_built_in() -> &'static FindingLabels` (a `LazyLock`).
  - `count_finding(labels: &FindingLabels, side: &'static str, f: &FindingRecord, stage: Option<Stage>)`; `prime_tool_policy_metrics(labels: &FindingLabels)`.
  - `CedarDecider::with_finding_labels(self, Arc<FindingLabels>) -> Self`, `RequestToolPolicy::with_finding_labels(self, Arc<FindingLabels>) -> Self` (both default to `built_in()`).
  - `PipelineOutput.late_elapsed: Duration` — the time of the late stages of one run.

- [ ] **Step 1: Write the failing tests.**

```rust
#[test] fn webhook_kinds_get_four_triples_and_detector_error_gets_every_severity() {
    let l = FindingLabels::with_webhook_kinds(["pii_email", "pii_iban", "pii_email"]);
    assert_eq!(l.labels(&rec("pii_email", Some(Severity::Low), "acme_dlp"), Some(Stage::Webhook)), ("pii_email", "webhook", "low"));
    assert_eq!(l.labels(&rec("detector_error", Some(Severity::Info), "acme_dlp"), Some(Stage::Webhook)), ("detector_error", "webhook", "info"));
    assert_eq!(l.labels(&rec("pii_email", Some(Severity::Low), "kernel"), None), ("other", "other", "low"), "a webhook kind is a webhook triple only");
    assert_eq!(l.triples("request").len(), 20);
    assert_eq!(l.triples("response").len(), 31, "20 + 2 kinds x 4 + 3 detector_error severities; a kind of two providers is one triple");
    assert_eq!(FindingLabels::built_in().triples("response").len(), 20);
}
// decider.rs: rewrite the_overhead_includes_the_pipeline_time as the_overhead_excludes_the_webhook_time
// (paused time, an opted-in SlowDetector of 50 ms): snapshot.decide_elapsed() < 50 ms, and the
// detector_seconds{stage="webhook"} sample is about 0.05.
// runtime.rs: prime_tool_policy_metrics_primes_every_triple_at_zero reads labels.triples(side) for both sides.
```
  Move the existing `labels_follow_the_table` assertions to `FindingLabels::built_in().labels(..)` with unchanged values.
- [ ] **Step 2: Run.** `cd rs && cargo nextest run -p paigasus-gateway --lib decider runtime`. Expected: FAIL.
- [ ] **Step 3: Implement.** The new `PipelineOutput` field breaks its struct literals (`built_in.rs:54`, `domain/detectors.rs` tests, `pipeline.rs:126-135`): add `late_elapsed: Duration::ZERO` or `..Default::default()`. `decide_now` adds `started.elapsed().saturating_sub(late_elapsed)` (D28). The pipeline measures `late_elapsed` around the `late_stages` call. `tool_history.rs` uses `FindingLabels::shared_built_in()`. `tests/tool_policy.rs:2253` and `runtime.rs` read the primed list through `FindingLabels::built_in().triples(side)` until Task 10 wires the config labels.
- [ ] **Step 4: Run.** `cd rs && cargo nextest run -p paigasus-gateway` and clippy. Expected: PASS.
- [ ] **Step 5: Commit.** `feat(rs): label webhook findings from the config and keep webhook waits out of the overhead (SMA-856)`.

---

### Task 10: The runtime wiring

**Files:**
- Modify: `src/runtime.rs:300-420` (`build_tool_policy`, `build_detectors`) and tests near `:480-800`
- Create: `rs/crates/services/paigasus-gateway/tests/support/webhook_mock.rs` (its interface is in Task 11; this task makes it, Task 11 uses it more). `tests/tool_policy_iam_source.rs` has no `mod support`, so it includes the file with `#[path = "support/webhook_mock.rs"] mod webhook_mock;`.
- Test: `tests/tool_policy_iam_source.rs` (one new test)

**Interfaces:**
- Consumes: Tasks 3, 7, 8, 9.
- Produces: `pub fn build_tool_policy_with_env(config: &GatewayConfig, recorder: Arc<DecisionRecorder>, history: Option<Arc<ToolHistory>>, env: &dyn EnvReader) -> anyhow::Result<ToolPolicyRuntime>`; `build_tool_policy` calls it with `&ProcessEnv`. `build_detectors(config, secrets, labels)` builds one `WebhookEntry` per provider in config order (`WebhookDetector::from_config`, `org_scope()`, `retired`), the registry into the pipeline and `cache.set_webhook_registry(..)`. The runtime builds `FindingLabels::with_webhook_kinds(all declared kinds)` once and passes it to `prime_tool_policy_metrics`, `CedarDecider::with_finding_labels` and `RequestToolPolicy::with_finding_labels`. It calls `prime_webhook_failure_metrics(names)` and `RequestToolPolicy::with_org_webhooks(config.tool_policy.org_webhooks())`. A secret or CA-bundle error is a boot error (`anyhow`), with the provider name. With `[tool_policy.detectors] enabled = false`, no secret is read and no client is built.

- [ ] **Step 1: Write the failing tests.**

```rust
// runtime.rs
// two_providers_give_a_pipeline_with_two_webhooks_in_config_order:
//   providers "zeta_dlp" then "acme_dlp", an env map with both secrets ->
//   rt.detectors.unwrap().registry().iter().map(|e| e.detector.name()) == ["zeta_dlp", "acme_dlp"].
// a_missing_webhook_secret_stops_the_boot: env map without the secret -> Err whose text names "webhook[acme_dlp].secret_env".
// the_detectors_off_read_no_secret: enabled = false and no env -> Ok, rt.detectors.is_none().
// the_runtime_primes_the_webhook_triples_and_failures: DebuggingRecorder ->
//   findings_total{side="response",kind="pii_email",detector="webhook",severity="low"} == 0 and
//   webhook_failures_total{provider="acme_dlp",reason="signature"} == 0.
// tests/tool_policy_iam_source.rs
// an_opt_in_from_iam_reaches_the_provider_and_an_empty_list_stops_it:
//   the fake IAM answers webhooks ["acme_dlp"] (token t1:enforce:closed:acme_dlp); a gateway built by
//   build_tool_policy_with_env over the fake IAM and a MockProvider -> request side `apply`, then
//   `rt.decider.decide(..)` for one call -> the mock records 1 request. The fake IAM then answers
//   webhooks [] (new token); cache.invalidate_all(); the next hold -> the mock still records 1 request.
```
- [ ] **Step 2: Run.** `cd rs && cargo nextest run -p paigasus-gateway --lib runtime && cargo nextest run -p paigasus-gateway --test tool_policy_iam_source`. Expected: FAIL.
- [ ] **Step 3: Implement** as in the Interfaces block. The boot log names the provider count, never a URL path.
- [ ] **Step 4: Run.** `cd rs && cargo nextest run -p paigasus-gateway` and clippy. Expected: PASS.
- [ ] **Step 5: Commit.** `feat(rs): wire the webhook providers into the gateway runtime (SMA-856)`.

---

### Task 11: End-to-end tests with a mock provider (§ 7.3)

**Files:**
- Modify: `rs/crates/services/paigasus-gateway/tests/support/webhook_mock.rs` (from Task 10), `tests/support/mod.rs` (`pub mod webhook_mock;`)
- Modify: `tests/tool_policy.rs` (a new `mod webhooks` inside `mod detectors`)
- Modify: `tests/tool_policy_iam_source.rs` (the `unknown_provider` and `not_permitted` cases)

**Interfaces:**
- Produces (test support): `pub struct MockProvider { pub url: String, … }`, `MockProvider::spawn(name: &str, secret: &str, script: impl Fn(usize, &Value) -> Reply + Send + Sync + 'static) -> MockProvider`, `pub struct Reply { pub status: u16, pub body: String, pub sign_with: Option<String>, pub delay: Duration, pub headers: Vec<(String, String)> }`, `Reply::signed(body)`, `fn requests(&self) -> Vec<Seen>` with `Seen { body: Value, raw: Vec<u8>, signature_ok: bool }`, and an optional `tokio::sync::Barrier` (`with_barrier(Arc<Barrier>)`) that a request waits on before it answers. The mock checks the request MAC with `webhook_protocol::request_mac` and signs with `response_mac`.
- Consumes: the gateway built through `runtime::build_tool_policy_with_env` over a file policy directory, with `[[tool_policy.org]] id = ORG_A, mode = "enforce", webhooks = […]` (D9) and the provider URL of the mock.

Timing rule of § 7: `[tool_policy.detectors] timeout_ms = 2000`, `decide_timeout_secs = 5`; the barrier proves parallelism, not wall time.

- [ ] **Step 1: Write the tests.** Tenant policy: the starter policy plus `@id("no-email") forbid (principal, action, resource) when { context.findings.contains("pii_email") };`.

```rust
// a_good_provider_denies_by_its_finding: answers pii_email medium -> the call is blocked; the decision log record
//   lists a finding with detector "acme_dlp", kind "pii_email", severity medium; the mock saw signature_ok, subject ==
//   the principal PRN, session == the session id (or null when the stack keeps none), and the built-in findings.
// negative_control_an_empty_answer_passes: the same policy, the provider answers {"findings":[]} -> allowed.
// a_bad_signature_gives_detector_error_signature: sign_with the wrong secret -> detector_error from acme_dlp,
//   version "signature"; failure_severity high -> blocked.
// a_slow_provider_and_the_failure_severity: sleep past timeout_ms = 300 -> high: blocked; low: allowed and the
//   record holds the low detector_error with version "timeout". keepalive 1 s and an answer after 1.5 s
//   (timeout_ms 1900): the ChatSse hold sends at least one keep-alive (keep_alives(text) >= 1).
// the_deadline_stops_one_call_only: 3 calls; the mock answers calls 1 and 2 at once and holds call 3 past the cut
//   (detectors timeout_ms 500, provider timeout_ms 450) -> calls 1 and 2 keep their findings, call 3 has "deadline".
// every_other_reason_reaches_the_record_and_the_counter: invalid JSON and an undeclared kind (malformed), 500 and
//   302 (status), a 70 KiB answer (response_too_large), a request over max_request_bytes (request_too_large, the
//   mock records 0 requests), a closed port (transport). Each: the finding version and the counter label.
// two_providers_one_slow: a Barrier(3) shared by both mocks and the test; both requests are open at the same time;
//   the fast one adds its finding, the slow one gives its failure finding.
// the_opt_in_rules (file source): no opt-in -> 0 requests; mode off -> 0 requests (D8); a tenant set that does
//   not compile -> 0 requests (D8); retired = true -> 0 requests and detector_error "retired" with the
//   provider's failure_severity.
// tool_policy_iam_source.rs (IAM source, because the file source refuses these names at boot, § 6.2):
//   an_unknown_opt_in_from_iam_gives_unknown_provider: webhooks ["nobody"] -> high detector_error "nobody",
//   version "unknown_provider", 0 requests; an_opt_in_outside_orgs_gives_not_permitted: provider orgs = [ORG_B],
//   the ORG_A tenant opts in -> high detector_error "acme_dlp", version "not_permitted", 0 requests.
// no_canary_leaks: a canary in the arguments, the secret and the answer body appear in no captured tracing event,
//   no decision log record or its proto bytes, no metric label and not in serde_json::to_string(&cfg).
// the_failure_counter_is_primed_before_the_first_request: every provider x adapter reason reads 0 after the boot.
```
- [ ] **Step 2: Run.** `cd rs && cargo nextest run -p paigasus-gateway --test tool_policy --test tool_policy_iam_source webhook`. Expected: PASS. If a test fails, fix the product code, not the test.
- [ ] **Step 3: Prove the assertions bite.** Remove the `with_webhooks(..)` call in `request.rs` and run `a_good_provider_denies_by_its_finding`: it must FAIL. Change `contains("pii_email")` in the policy to `contains("pii_x")`: the good-provider test must FAIL and the negative control must still PASS. Restore both edits by deleting the inserted change (do not `git checkout` the file).
- [ ] **Step 4: Commit.** `test(rs): prove the webhook detector end to end (SMA-856)`.

---

### Task 12: Docs and the full gate run

**Files:**
- Modify: `docs/ops/RUNBOOK-tool-policies.md` (a new section "Webhook providers" after § 10, and the line at `:596`)
- Create: `docs/tool-policies/webhook-protocol.md`
- Modify: `docs/tool-policies/tenant-guide.md` (§ 4 and § 6 "Error and rejection codes")
- Modify: `rs/crates/services/paigasus-gateway/CHANGELOG.md`, `rs/crates/services/paigasus-iam/CHANGELOG.md` (`## [Unreleased]` → `### Added`)
- Modify: `src/domain/webhook_protocol.rs` (the doc assertion of the worked example test)

- [ ] **Step 1: Write the docs** in Simplified Technical English.
  - Runbook section: every bullet of spec § 6.9 (register a provider with `orgs`, the secret env, the CA bundle that adds, `failure_severity`; D5/D12 data that leaves unredacted; the reserved names and kinds; the failure reasons and the counter; Cedar sees kinds and `max_severity`, not the provider name; more than 8 calls queue; cold connections; the rollout order and `retired = true`; the secret rotation order of § 6.3; the IAM rollback hazard of § 6.6). At `:596`, add: "The 15 ms limit excludes webhook wait time. `gateway_tool_policy_detector_seconds{stage="webhook"}` measures it."
  - `webhook-protocol.md`: § 6.3 in full (headers, MAC inputs, body fields, `args` versus `args_text`, the response, the order of checks, the rotation order, the 5-minute replay window, the key-bytes rule). The worked example uses the Task 6 constants: secret `paigasus-webhook-example-secret-0001`, name `acme_dlp`, `t = 1764300000`, nonce `00112233445566778899aabbccddeeff`, the request body and the answer body verbatim, request MAC `4929f346b96d703a5ab953c6e8981444573fa64e66795b8a5d61ba680c5ba878`, response MAC `cf6147bdf8be4b7c7b4619accf9b1f592a14d6d9209e112a1a86c7e00d6f5638`.
  - Tenant guide: the opt-in (gRPC `WebhookOptIn`, REST `webhooks`; absent and `null` keep, `[]` clears); a policy example with `context.findings.contains("pii_email")`; the opt-out delay of up to `max_stale_secs` during an IAM outage under fail mode `open`; the `invalid-webhook-opt-in` row.
  - Changelogs: the gateway entry (the providers, the opt-in, the counter, the overhead change, the rollout note of § 6.6); the IAM entry (m0016, the field, the error reason, the rollout note).
- [ ] **Step 2: Bind the doc to the test.** In `the_worked_example_matches_byte_for_byte`, add `let doc = include_str!("../../../../../../docs/tool-policies/webhook-protocol.md");` and assert that `doc` contains `REQUEST_MAC`, `RESPONSE_MAC`, `NONCE`, `BODY` and `RESPONSE`. Run `cd rs && cargo nextest run -p paigasus-gateway --lib webhook_protocol`. Expected: PASS. Change one hex digit in the doc: the test FAILS. Restore it.
- [ ] **Step 3: Run the full gate graph.** From the worktree root, with the PATH export, run the command between the `ci-targets` markers of the root `CLAUDE.md`:

```bash
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site \
  :http-extractor-envelope :input-liveness :promtool :observability-drift \
  :nats-permissions :release-parity :release-parity-py :release-parity-ts \
  :publish-metadata :version-lockstep :workflow-credentials :pyo3-stub-drift :ruff-ci \
  :next-public-free :helm-render :moon-diagnosis-exec :wasm-lockstep :test-e2e \
  --base origin/main --include-relations
```
  Expected: every task passes. On this Mac, re-run the bash-split gates directly with the bash that the root `CLAUDE.md` names for each (`repo:affected-smoke` under `/bin/bash`; `repo:ruff-ci`, `repo:next-public-free`, `repo:publish-metadata`, `repo:nats-permissions` under `/opt/homebrew/bin/bash`; `repo:actionlint` only when its pipe preflight passes). For a failure, follow the root `CLAUDE.md` "Diagnosing an unattributed `moon ci` failure" steps, Step 0 first. Pay attention to `:breaking`, `:observability-drift`, `:error-code-single-site`, `:deny`, `:machete` and `ts:fmt`.
- [ ] **Step 4: Commit.** `git add docs rs/crates/services/paigasus-gateway rs/crates/services/paigasus-iam` — `docs(rs): document the webhook providers, the protocol and the opt-in (SMA-856)`.

---

## Self-review record

- **Spec coverage.** G1 → Tasks 6, 8. G2 → Task 3. G3 → Tasks 1, 2, 4, 7 (file source: Task 3 + 7). G4 → Tasks 5, 7, 8. G5 → Tasks 5, 8. G6 → Tasks 5 (D15, D16, version check, label), 7 (name check). G7 → Task 11. D1–D31 → D1/D26 Task 8; D2 Tasks 6, 8; D3/D22/D29/D30/D31 Task 2; D4 Tasks 5, 8; D5/D13 Tasks 6, 7; D6 Task 2; D7/D25/D27 Task 7 (warn in the cache) and Task 11; D8/D9/D23 Tasks 3, 4, 7; D10/D11/D14 Tasks 5, 6; D12 Task 12 (runbook); D17 Tasks 5, 9; D18/D19/D20/D21 Task 3 (and the runtime limits in Task 8); D24 Tasks 5, 8; D28 Task 9. § 7.1–7.7 → Tasks 2, 3, 4, 5, 6, 7, 8, 10, 11. § 6.9 docs → Task 12. § 7.8 → Task 12.
- **Type consistency.** `Caller`, `for_calls`, `WebhookFailure`, `failure_finding`, `count_webhook_failure` (Task 5) are the names that Tasks 7, 8 use. `WebhookRegistry`, `WebhookEntry`, `OptIn`, `OrgScope` (Tasks 3, 7) are the names that Tasks 8, 10 use. `FindingLabels` (Task 9) is the name that Task 10 wires. `build_tool_policy_with_env` and `EnvReader` (Tasks 3, 10) are the names that Task 11 uses.
- **Known spec defects that the plan works around:** PD15 (the 25-digit number) and PD16 (the CA trust test).
