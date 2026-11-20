# SMA-755: the gateway tool decision log — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Keep a durable record of each tool decision as the gateway applied it: a Redis stream outbox, a relay to a JetStream stream in its own NATS account, loss metrics and alerts, and an operator CLI that exports one org's decisions as a Dogwood trace.

**Architecture:** `paigasus-kernel` exposes the final findings and the action group of a verdict. `paigasus-gateway-core` gets a pure `decision_log` module: the domain record, the record builders, and the trace encoder. The decider attaches its Cedar detail to each decision, the interceptor and the body path hand the final report entries to the gateway (`Action::Reported`, `BodyCalls::reported`), and the gateway service builds one record per report entry through one `DecisionRecorder`. The `DecisionLog` port has a tracing adapter (default) and a Redis adapter (writer task, relay task to JetStream). The CLI reads JetStream and calls the core encoder.

**Tech Stack:** Rust 2024 (prost 0.14, redis 1.x with `streams`, async-nats 0.52 JetStream, hmac 0.13 + sha2 0.11, rand 0.10, time 0.3, testcontainers 0.27), protobuf + buf, Dogwood `dogwood-language` 1.1.1 at rev `ff158506cfd9c7871385ed3a87d7435505fd24b1` (policy-compiler workspace only), bash (`ops/nats`), Prometheus rules + promtool.

**Spec:** `docs/superpowers/specs/2026-11-18-sma-755-tool-decision-log-design.md` (revision 2, approved). Read it before any task. Every decision id (D1-D17, § x.y, F1-F29, A1-A6) refers to it. Do not reopen its decisions.

## Plan-level clarifications of the spec

The spec leaves these to the builder (§ 8), or the code does not carry a spec rule as written. Sven can change any of them at plan review.

| Id | Clarification | Where |
|---|---|---|
| P-1 | **The report site.** The decider port returns `Vec<DecidedCall>` (a `Decision` plus an optional `Arc<DecisionDetail>`), not `Vec<Decision>`. The interceptor emits one `Action::Reported(Box<ReportedCall>)` next to every `report_comment` it emits: in `apply_decision` (`interceptor.rs:340-363`), in `block_all` (`:373-396`) and for each protocol report of `Step::Frame { reports }` (`:259-262`). On the body path, `BodyCalls::reported(&self, decided, basis) -> Vec<ReportedCall>` builds the entries, and `BodyCalls::report` is built from the same entries. The gateway records in `adapters/http/interception.rs`: `Relay::apply` for `Action::Reported`, and `intercept_body` after `calls.reported(..)`. These are the only two record sites on the response side. | Tasks 4, 8 |
| P-2 | `ReportedCall.call` is `Option<CallFacts>` (kind, id, namespace, name; never the arguments). It is `None` for a `block_all` entry and for an unenforceable entry, because the core has only `(position, name)` there (`interceptor.rs:376-378`, `body.rs:24`). Such a record has `tool.kind = ""` and `tool.call_id = ""`. | Tasks 3, 4 |
| P-3 | **Prevention (A5).** `applied = Block` with a known kind uses `ToolCallKind::provider_may_run()`. `applied = Block` with an unknown kind (P-2) and `ingress = "responses"` is `PROVIDER_MAY_RUN`, because a Responses hold can be a provider-run kind; with any other ingress it is `PREVENTED`. A `ReportDecision::Unenforceable` entry is `applied = Allow`, `prevention = PROVIDER_MAY_RUN` (§ 5.4 names these items explicitly). Every other `Allow` and `Observe` is `NOT_APPLICABLE`. | Task 3 |
| P-4 | `RecordBasis`: a report entry with `basis = OnFailure` is `OnFailure`. A `Decider` entry takes `DecisionDetail.basis` (`Policy`, `ErroredForbid`, `Unavailable`). An observed hold keeps its Cedar detail (`cedar = Some`, `basis = OnFailure`), because § 6 asks for "applied `Allow`, Cedar `Block`". A `block_all` entry has `cedar = None`. | Tasks 3, 4 |
| P-5 | **Request side.** Every `RequestEntry` comes from a deny-for-all-inputs on an available set (`plan.rs:213-216`, `:134-136`), so its record has `basis = Policy`, `block_code = Some(Policy)`, `applied = Block` when `enforced`, else `Observe`. `RequestEntry` has no kind (`report.rs:149-155`), so `tool.kind = ""`. A refused request (every `Err(Refused)` return of `apply_inner` and `check_turn`) adds one record with `request_action = Refuse`, `applied = Block`, `block_code = None`, `basis = Policy`, an empty `ToolRef`. A log-only "would refuse" writes no refusal record. | Task 8 |
| P-6 | **`RequestAction` proto values** match the real `report::RequestAction` (`report.rs:105-123`) plus the refusal path: `REMOVE_DEFINITION`, `REPLACE_RESULT`, `RESET_TOOL_CHOICE`, `REFUSE`. The core record type is `decision_log::RecordedAction`, so it does not clash with `report::RequestAction`. | Tasks 2, 3 |
| P-7 | `decide_calls` takes a clock closure, `now: &dyn Fn() -> Instant`, and measures `decide_duration` per call with it. `paigasus-gateway-core` stays clock-free (`interceptor.rs:5-7`). | Task 4 |
| P-8 | **One recorder.** `application::decision_log::DecisionRecorder` owns the `Arc<dyn DecisionLog>`, the clock, the entropy, the instance id, the per-org sequence map and the optional digest key. The composition root builds one `Arc<DecisionRecorder>` and gives the same `Arc` to the `CedarDecider`, the `RequestToolPolicy` and the `ToolInterception`. The recorder assigns `sequence` before it calls `DecisionLog::record`, and `record` always accepts, so "the counter increases when `record` accepts" (§ 5.8) holds. | Tasks 7, 8 |
| P-9 | `MemoryDecisionLog` is a plain `pub` type in `domain::decision_log`, with no feature gate. The gateway crate has no `[features]`, and a crate cannot dev-depend on itself, so a `test-support` gate would hide it from `tests/*.rs`. The runtime never builds it. | Task 7 |
| P-10 | **The tracing line** moves from `decide_now` into a shared `domain::decision_log::log_line(&DecisionRecord)`. Both adapters call it, only for response-side records with `basis != OnFailure`, so the set of lines stays the same as today. Its `verdict` field is the applied label (`allow`, `observe`, `block`). This changes the line of an observed hold from `block` to `observe`: the decider no longer writes the line, the report site does. The `GATEWAY_TOOL_POLICY_DECISIONS_TOTAL` counter stays in `decide_now`. | Tasks 7, 8 |
| P-11 | **Dogwood skip set** (§ 8, read in `log_parse.rs`). The head uses an unescaped `find('(')` (`:85`) and `unescape` decodes the `escape_debug` forms (`:245`, `:395-484`). Only `(` breaks a line that is escaped. The encoder keeps the spec minimum (a superset): it skips an action id that contains `(`, `)`, `"` or any char for which `char::is_control` is true. Every other string goes through `escape_debug`. | Task 5 |
| P-12 | **Property test across workspaces.** The encoder cannot be called from the compiler workspace. A gateway-core test generates 512 action ids from a fixed seed (a xorshift over an alphabet with `(`, `)`, `::"`, `\r`, NUL, `\u{202e}`, `\u{301}` at the start, ASCII and CJK), keeps the ones that `encode` does not skip, and writes them to `rs/tools/paigasus-policy-compiler/tests/fixtures/decision-trace-ids.log` (one trace line per id) and `decision-trace-ids.json` (the expected ids). A drift test fails when the committed files differ. The compiler test parses each line and asserts that the action id equals the expected id. No `proptest` dependency enters the compiler workspace. | Tasks 5, 6 |
| P-13 | **NATS reader grants** (§ 8, read in async-nats 0.52.0). The CLI uses `get_stream_no_info` (no network call) and `Stream::create_consumer(OrderedConfig { filter_subject, deliver_policy: ByStartTime, .. })`. The first create has no name (`$JS.API.CONSUMER.CREATE.<stream>`); a recreate after a gap has a name and the filter suffix (`context.rs:1561-1569`, `pull.rs:2934-2990`) and deletes the old consumer first. Reader publish grants: `$JS.API.CONSUMER.CREATE.GATEWAY_DECISIONS`, `$JS.API.CONSUMER.CREATE.GATEWAY_DECISIONS.>`, `$JS.API.CONSUMER.DELETE.GATEWAY_DECISIONS.*`, `$JS.API.CONSUMER.MSG.NEXT.GATEWAY_DECISIONS.*`. Subscribe: `_INBOX_GW_RD.>`. No `$JS.ACK` (an ordered consumer has `ack_policy: None`). The end of the read is the `num_pending` of `Consumer::cached_info()` at create time. | Tasks 13, 14 |
| P-14 | **Publish error classes.** async-nats reports a permission refusal of a publish as `PublishErrorKind::TimedOut`, not as its own kind (`context.rs:1946-1972`; the server sends an async `Permissions Violation`). The relay's `event_callback` sets an `AtomicBool` on a `ServerError` that contains `Permissions Violation for Publication`. Permanent: `MaxPayloadExceeded`, an `Other` with no JetStream API error, an `Other` whose server code rejects the message (10054, 10097, 10109 and each code not in the transient list), a decode failure, a bad `org_id`, and `TimedOut` while the flag is set. Transient: `StreamNotFound` (R9: async-nats maps NO_RESPONDERS to it; the relay re-ensures the stream), `TimedOut` without the flag, `BrokenPipe`, `MaxAckPending`, and an `Other` with a resource or availability code (`jetstream::TRANSIENT_CODES`; a full disk on the nats-server 2.10.14 publish path gives 10023 for the server limit and 10002 for the account limit; newer servers add 10158, 10167 and 10194) or a status 503 with no code. An `Other` with no JetStream code (a closed client, a closed ack semaphore, a PubAck that does not parse) stays `Permanent("other")`. CodeRabbit review: before this change each `Other` was permanent, so a full disk deleted valid events. | Task 11 |
| P-15 | **Stream creation.** `provision-decisions.sh` creates `GATEWAY_DECISIONS` with a third, operator-only identity `gateway-decision-provisioner` (the `iam-provisioner` pattern, permissions.md § 9). The `nats` CLI calls `$JS.API.INFO` (`subjects.env:56-57`). The publisher has `$JS.API.INFO` too, for the account limit (P-20), but not `$JS.API.STREAM.>`. The relay calls `get_or_create_stream` with the same values. The values live once in `ops/nats/subjects.env`, and a gateway unit test pins the Rust constants to them. | Tasks 11, 14 |
| P-16 | **Permission tests live in IAM's `tests/nats_permissions.rs`.** That file renders the committed template, so it must replace the new placeholders anyway. The new tests use raw async-nats clients with the exact calls of the relay and the CLI. `repo:nats-permissions` keeps its command; its `ops/nats/**/*` input already covers the template. The gateway's own relay and CLI code is tested against an unauthenticated NATS in gateway test binaries. | Task 14 |
| P-17 | `[decision_log.nats]` is required when `enabled = true`. Without it, events wait in Redis until the backlog cap drops them. | Task 9 |
| P-18 | Writer and relay run **outside** the server `JoinSet`, on their own shutdown watch. `supervise` gets `decision_log: Option<DecisionLogTasks>` and drains it after the servers and after the charges, with `DECISION_LOG_DRAIN_BUDGET = 2 s` (§ 5.6). | Task 12 |
| P-19 | Two samplers of `XLEN`: the writer every 1 s for the backlog cap (§ 5.6); the relay every 15 s for the gauge and for the loss estimate (§ 5.8, § 5.9). | Tasks 10, 11 |
| P-20 | The stream consts: `max_age` 30 days (2 592 000 s), `max_bytes` 19 327 352 832 (90 % of the default 20 GiB disk), `discard old`, `duplicate_window` 120 s, `max_message_size` 65 536, `num_replicas` 1, storage file, retention limits. Before it makes or checks the stream, the relay reads the account storage limit (`query_account`, `$JS.API.INFO`) and uses `max_bytes_for(limit)` = `min(MAX_BYTES, limit × 9 / 10)`, or `MAX_BYTES` with no limit or when the query fails. So a smaller disk gives no refused create and no drift warning. A disk above the default gives a drift warning after a `stream edit`, and the relay does not change the stream. CodeRabbit review: before this change the relay always used `MAX_BYTES`. | Tasks 11, 14 |
| P-21 | The config key names match `[tool_policy.nats]` (`creds_file`, `ca_file`, `config.rs:719`). The coordinator changed § 5.12 from `credentials_file` to `creds_file` for this. | Task 9 |

## Global Constraints

- Every new source file opens with `// SPDX-License-Identifier: Apache-2.0` (`#` for shell, Python, YAML and TOML). A `.proto` file opens with `// SPDX-License-Identifier: Apache-2.0`. A trace fixture (`.log`) and a JSON fixture cannot hold a comment (F25), so they have no header.
- Rust crates use edition 2024 and rust-version 1.95 through `edition.workspace` and `rust-version.workspace`.
- The workspace has `warnings = "deny"`. Dead code, an unused import or an unused dependency is a hard error. Never "add now, wire later": each task's new code is `pub` and used by a test in the same task, or used by production code in the same task. Never add `#[allow(dead_code)]`. Each commit compiles under `cd rs && cargo clippy --workspace --all-targets -- -D warnings`.
- `repo:machete` reds on an unused dependency. Add each dependency in the task whose code first uses it.
- `rustfmt.toml` sets `max_width = 200`. Run `cargo fmt` in `rs/` after every Rust edit, and in `rs/tools/paigasus-policy-compiler/` after an edit there.
- Rust commands run in `rs/`: `cargo build --workspace`, `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo nextest run -p <crate>` (or `--workspace`). The compiler workspace runs its own `cargo nextest run` in `rs/tools/paigasus-policy-compiler/`.
- Prefix every shell command with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`. Export `PROTO_REPORTER=text` before you capture the output of a `proto` call or a proto shim.
- Conventional commits with a scope from the allow-list in `ts/packages/commitlint-config/index.cjs`: use `feat(rs)`, `feat(contracts)`, `test(rs)`, `feat(repo)`, `docs(repo)`. There is no `ops` scope: `ops/nats` changes use `feat(rs)` (as SMA-752 did), `ops/observability` changes use `feat(repo)`. Put `(SMA-755)` at the end of the subject. The header is at most 100 characters, and a body line at most 100. No body line starts with `#NNN`. End every commit message with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Never use `--no-verify`. Never use `git commit --amend` with `git reset`. Never amend, reset or rebase a commit that an earlier task made. Commit only your own task's files.
- Run every command in the foreground. Do not start a background job and wait for it.
- Do not install software on the host. If a tool is missing, stop and report. Use `docker run` for a Linux tool if a task needs one.
- Proto edits: `buf format -w` in `contracts/`, then `moon run contracts:generate`. Commit the generated Rust, Python and TS code. In a worktree, `buf breaking` needs `--against "$(git rev-parse --git-common-dir)#branch=main,subdir=contracts"`. If `contracts:generate` fails on a BSR rate limit, restore `ts/packages/paigasus-proto/src/generated/google/rpc/error_details_pb.ts` and run it again.
- `paigasus-gateway-core` stays I/O-free: no tokio, futures, axum, reqwest, redis or async-nats, and no clock read (P-7).
- With `[tool_policy] enabled = false` (the default), the gateway behaves exactly as today and records nothing. Every existing gateway test passes at every commit, after its mechanical update to `DecidedCall` in Task 4.
- **Hygiene (A1).** A record, an event, a trace line, a log line or a metric label never holds `ToolCall.arguments`, a `ToolInput`, a definition schema, a tool result, or a plain hash of the arguments. No `{:?}` of a record or an event in a log line. Tool names reach a log line only through `sanitize_name`.
- **Metrics.** A new metric name gets: a `pub const` in `rs/crates/libs/paigasus-observability/src/names.rs`, an entry in `names::ALL`, a membership test in that file's test module, a `describe_*!` line in `paigasus-gateway/src/main.rs`, a prime at zero for every label value at boot, and a row in `docs/ops/RUNBOOK-observability.md` § 2.3. A Prometheus test assertion parses a number; it never `contains()` a `# TYPE` line. A promtool test primes a counter series at zero before it rises.
- **Redis** only through `paigasus_redis::connect` and `RedisHandle` (`repo:redis-connect-single-site`). Tests read Redis through `redis-cli` in the container (`ExecCommand`), never a second Rust client. A Redis test uses `start_redis_image_or_skip("7.4-alpine", ..)`: the default image is `5.0` and has no `XAUTOCLAIM`.
- **NATS grants** change only in `ops/nats/subjects.env`, `ops/nats/test/accounts.conf.tmpl` and `ops/nats/check-subjects.sh` together. `check-subjects.sh` needs bash >= 4.3: on the development Mac run `/opt/homebrew/bin/bash ops/nats/check-subjects.sh`.
- **Docker tests** use `paigasus_test_docker::start_or_skip` (or `start_redis_image_or_skip`): they skip only outside CI and fail in CI. Every new Docker-backed test binary goes into the `docker-containers` filter at `rs/.config/nextest.toml:134`.
- **Mutation proofs** (§ 6): a mutation must compile. Under `-D warnings` a mutation can die at rustc (for example an unused variable), and that proves nothing. Write the mutation so that it compiles, run `cargo nextest run --no-fail-fast -p <crate>`, and confirm that the failure is a test assertion. Restore by deleting your marked edit; never `git checkout --` a file that holds the fix.
- Moon edges: a new `{ workspace = true }` in-tree dependency needs a hand-written `dependsOn` and its `fileGroups.upstreams` lines in the crate's `moon.yml` (`repo:affected-smoke` A6). This plan adds no new in-tree edge; `prost`, `prost-types`, `hmac`, `rand`, `time` and `uuid` are external.
- The kernel version is not edited by hand. Task 1's `feat(rs)` on the kernel makes release-plz cut the next kernel minor.

## Review Focus

These inputs are implied by the spec, but no spec test names them. Each line has a test in the named task.

1. **A forged or non-canonical `org_id` in a Redis entry** (upper case, braces, `../`, a valid UUID of another org with `*` in it). The relay must reject it as `publish_rejected` and never publish to a subject built from it. Task 11 `a_non_canonical_org_id_is_rejected_and_never_published`.
2. **An event from a newer gateway with an unknown proto field.** The relay must publish the original bytes unchanged, and the CLI must decode the event and ignore the field. Task 11 `an_unknown_field_survives_the_relay_byte_for_byte`, Task 13 `the_export_ignores_an_unknown_field`.
3. **A model or a scope string with `(`, `"`, a backslash and a newline** in `request_context`. The line must stay one line, Dogwood must parse it, and the model must round-trip. Task 5 golden case `hostile_request_context_strings`, replayed in Task 6.
4. **A string cut at the 256-byte cap in the middle of a multi-byte character** (a namespace of 255 ASCII bytes and then `é`). The cut must be on a UTF-8 boundary, at most 256 bytes, and set `truncated`. Task 3 `a_cut_never_splits_a_character`.
5. **Bad CLI time arguments**: `--since` equal to or after `--until`, a time with no zone offset, an `--org` with upper case or braces. Expected: exit 1 for the first two, and the org is made canonical (lower case, hyphenated) before the subject is built. Task 13 `since_not_before_until_is_exit_1`, `a_time_without_an_offset_is_exit_1`, `an_upper_case_org_is_canonical`.

---

### Task 0: Provision the worktree and take the baseline

**Files:** none.

- [ ] **Step 1: Provision.** In the worktree: `proto install`, `pnpm -C ts install`, `( cd py && uv sync )`, `( cd rs && cargo fetch )`, `( cd rs/tools/paigasus-policy-compiler && cargo fetch )`.
- [ ] **Step 2: Baseline.** Run `cd rs && cargo nextest run -p paigasus-kernel -p paigasus-gateway-core -p paigasus-gateway -p paigasus-observability` and `cargo clippy --workspace --all-targets -- -D warnings`, then `cd tools/paigasus-policy-compiler && cargo nextest run`. Expected: PASS. Record the test counts in your report. Record `cargo tree -e features -p redis` (it is the "before" of Task 10). If anything is red, stop and report; do not fix it.

No commit.

---

### Task 1: The kernel exposes the final findings and the action group

**Files:**
- Modify: `rs/crates/libs/paigasus-kernel/src/tool_policy/decide.rs` (`Verdict`, `decide`, `Verdict::unevaluable`)
- Modify: `rs/crates/libs/paigasus-kernel/src/tool_policy/compile.rs` (`action_entity` returns the group)
- Modify: `rs/crates/libs/paigasus-kernel/src/tool_policy/mod.rs` (re-export `ActionGroup`)

**Interfaces:**
- Produces (module `paigasus_kernel::tool_policy`):
  - `#[non_exhaustive] #[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum ActionGroup { Catalog, Unregistered }` with `as_str()` = `"catalog"` / `"unregistered"`
  - `Verdict` gets `pub findings: BTreeSet<String>` (the final set, `unparsed_arguments` included) and `pub action_group: ActionGroup`
  - `pub(crate) fn action_entity(policies: &ToolPolicies, normalised: &str) -> (EntityUid, Entities, ActionGroup)`

- [ ] **Step 1: Write the failing tests** (unit tests in `decide.rs`).

```rust
#[test] fn the_verdict_holds_the_final_findings() {
    // input "not json" → the kernel adds unparsed_arguments; the caller gave name_collision.
    let v = decide(&inv("x", ToolInput::Json("not json".into())), &facts(), &findings(&["name_collision"]), &policies(&[]));
    assert_eq!(v.findings.iter().map(String::as_str).collect::<Vec<_>>(), ["name_collision", "unparsed_arguments"]);
}
#[test] fn a_catalog_name_is_in_the_catalog_group_and_another_is_unregistered() {
    let p = policies(&["Read_File"]);
    assert_eq!(decide(&inv(" read_file", json("{}")), &facts(), &Findings::new(), &p).action_group, ActionGroup::Catalog);
    assert_eq!(decide(&inv("rm", json("{}")), &facts(), &Findings::new(), &p).action_group, ActionGroup::Unregistered);
}
#[test] fn an_unevaluable_request_keeps_the_caller_findings() {
    // facts with principal entity_type "not a type" → Verdict::unevaluable
    let v = decide(&inv("x", json("{}")), &bad_facts(), &findings(&["name_collision"]), &policies(&[]));
    assert!(v.floor_denied);
    assert!(v.findings.contains("name_collision"));
}
```

- [ ] **Step 2: Run them.** `cd rs && cargo nextest run -p paigasus-kernel --features tool-policy -E 'test(verdict) | test(group) | test(unevaluable)'`. Expected: FAIL to compile (no field `findings`).
- [ ] **Step 3: Implement.** `context()` already builds `kinds`; return it to `decide` so the verdict holds the same set that Cedar saw (no second map). `unevaluable(findings, group)` takes the caller set. `optimistic.rs` uses only `request()`; give it `let (action, entities, _) = …`.
- [ ] **Step 4: Run.** The kernel tests and `cargo clippy -p paigasus-kernel --features tool-policy --all-targets -- -D warnings`. Then `moon run paigasus-kernel-ts:test`: the change is behind `tool-policy`, so the wasm glue must not change. Expected: PASS.
- [ ] **Step 5: Commit.** `feat(rs): expose the final findings and the action group of a tool-policy verdict (SMA-755)`

---

### Task 2: The `ToolDecisionEvent` proto

**Files:**
- Create: `contracts/proto/paigasus/gateway/v1/tool_decision.proto` (the § 5.2 text, with the P-6 `RequestAction` values)
- Modify: `contracts/buf.gen.yaml` (one `message_attribute` line)
- Regenerated: `rs/crates/libs/paigasus-proto/src/generated/paigasus/gateway/v1/paigasus.gateway.v1.rs`, the Python and TS generated trees

**Interfaces:**
- Produces: `paigasus_proto::paigasus::gateway::v1::{ToolDecisionEvent, CedarUid, ToolRef, CedarDetail, PolicyRef, Finding, ArgumentDigest, DecisionSide, DecisionMode, DecisionEffect, DecisionBasis, Prevention, RequestAction, PolicyOrigin, ActionGroup}` (prost types; `bytes` fields are `bytes::Bytes` because of `bytes=.`). `ToolDecisionEvent` implements `paigasus_proto::audit::Auditable`.

- [ ] **Step 1: Write the proto** exactly as § 5.2, package `paigasus.gateway.v1`, with `import "google/protobuf/duration.proto";` and `import "paigasus/common/v1/audit.proto";`. No `reserved` line in `Finding`. `paigasus.gateway.v1` already has `health.proto`, so `paigasus-proto/src/lib.rs` needs no new `include!`.
- [ ] **Step 2: Add the Auditable line** after the `ExternalIdentity` line: `- 'message_attribute=.paigasus.gateway.v1.ToolDecisionEvent=#[derive(::paigasus_proto::audit::Auditable)]'` (quoted).
- [ ] **Step 3: Generate.** `cd contracts && buf format -w && buf lint && cd .. && moon run contracts:generate`. Expected: new types in the three generated trees.
- [ ] **Step 4: Verify.** `cd rs && cargo nextest run -p paigasus-proto` (`auditable_derive_drift` must pass with the new line), `moon run contracts:lint contracts:fmt`, and `cd contracts && buf breaking --against "$(git rev-parse --git-common-dir)#branch=main,subdir=contracts"`. Expected: PASS (the file is new). Run `moon run paigasus-proto-py:test paigasus-proto-ts:test` if those projects exist (`moon query projects | grep proto`). Expected: PASS.
- [ ] **Step 5: Commit.** `feat(contracts): add the ToolDecisionEvent of the gateway decision log (SMA-755)`. Include every generated file.

---

### Task 3: The core decision record and its builders

**Files:**
- Create: `rs/crates/libs/paigasus-gateway-core/src/decision_log/mod.rs`, `record.rs`, `build.rs`
- Modify: `rs/crates/libs/paigasus-gateway-core/src/lib.rs` (`pub mod decision_log;`)
- Modify: `rs/crates/libs/paigasus-gateway-core/src/types.rs` (`ToolCallKind::provider_may_run`)
- Modify: `rs/crates/libs/paigasus-gateway-core/src/decider.rs` (`DecisionContext` fields)
- Modify: `rs/crates/libs/paigasus-gateway-core/Cargo.toml` (`uuid` from dev to normal dependency)

**Interfaces:**
- Produces (module `paigasus_gateway_core::decision_log`, every type `#[non_exhaustive]`, `Debug, Clone, PartialEq`):
  - `DecisionRecord` with the § 5.1 fields; `request_action: Option<RecordedAction>`
  - `CedarDetail { effect: Effect, determining: Vec<PolicyRef>, errored: Vec<PolicyRef>, floor_denied: bool, findings: BTreeSet<String>, declared: Declared, action_group: ActionGroup, decide_duration: Duration }` with a `new(..)` constructor
  - `ToolRef { name, namespace, action_id, truncated: bool, kind, call_id }` (all `String`)
  - `enum Side { Request, Response }`, `enum AppliedEffect { Allow, Observe, Block }`, `enum RecordBasis { Policy, ErroredForbid, Unavailable, OnFailure }` with `as_str()` = `"policy" | "errored_forbid" | "unavailable" | "on_failure"`, `enum Prevention { NotApplicable, Prevented, ProviderMayRun }`, `enum RecordedAction { RemoveDefinition, ReplaceResult, ResetToolChoice, Refuse }`
  - `ArgumentDigest { hmac_sha256: [u8; 32], key_id: String }`
  - `DecisionDetail { basis: RecordBasis, cedar: Option<CedarDetail>, arguments: Option<ArgumentDigest> }`
  - `CallFacts { kind: ToolCallKind, id: Option<String>, namespace: Option<String>, name: String }` with `CallFacts::of(&ToolCall)` (copies no arguments)
  - `ReportedCall { entry: ReportEntry, call: Option<CallFacts>, detail: Option<Arc<DecisionDetail>> }`
  - `RecordHeader { decision_id: Uuid, decided_at: SystemTime, instance_id: Uuid, sequence: u64, org: Option<Uuid> }`
  - `pub fn build_response(h: RecordHeader, ctx: &DecisionContext, r: &ReportedCall) -> DecisionRecord`
  - `pub fn build_request(h: RecordHeader, ctx: &DecisionContext, mode: Option<TenantMode>, e: &RequestEntry) -> DecisionRecord`
  - `pub fn build_refusal(h: RecordHeader, ctx: &DecisionContext, mode: Option<TenantMode>) -> DecisionRecord`
  - `pub fn response_org(ctx: &DecisionContext) -> Option<Uuid>` (`Facts.org` when the set is available, else `None`; the gateway falls back to `org_of(scope)`)
  - `pub const MAX_FIELD_BYTES: usize = 256;` and `pub fn cap(s: &str) -> (String, bool)` (cut on a UTF-8 boundary)
- Produces (`types.rs`): `pub fn provider_may_run(self) -> bool` — `true` for `Shell` and `ToolSearch` only; a `match` with every variant and no `_` arm.
- Produces (`decider.rs`): `DecisionContext` gets `pub api_key_id: Option<String>, pub ingress: String, pub model: String` (empty by default) and `pub fn with_request(self, api_key_id: Option<String>, ingress: impl Into<String>, model: impl Into<String>) -> Self`.

- [ ] **Step 1: Write the failing tests** (unit tests in `build.rs`, `types.rs`). One test per row of the § 5.4 field table, plus:

```rust
#[test] fn provider_may_run_per_kind() {
    use ToolCallKind::*;
    for (k, want) in [(Function,false),(Custom,false),(LegacyFunctionCall,false),(AnthropicToolUse,false),(Other,false),(Computer,false),
                      (LocalShell,false),(Shell,true),(ApplyPatch,false),(ToolSearch,true),(McpApprovalRequest,false)] {
        assert_eq!(k.provider_may_run(), want, "{k:?}");
    }
}
#[test] fn an_observed_hold_is_applied_allow_with_the_cedar_block() { /* entry Allow + code Policy + basis OnFailure, detail effect Deny → applied Observe, block_code Some(Policy), basis OnFailure, cedar.effect Deny, prevention NotApplicable */ }
#[test] fn a_block_all_entry_has_no_cedar_and_no_call_id() { /* call None, detail None, code HoldLimit → basis OnFailure, cedar None, tool.kind "", tool.call_id "", prevention Prevented (ingress chat_completions) */ }
#[test] fn an_unknown_kind_block_on_responses_may_run_at_the_provider() { /* call None, ingress "responses" → ProviderMayRun */ }
#[test] fn an_unenforceable_entry_may_run_at_the_provider() { /* ReportDecision::Unenforceable → applied Allow, ProviderMayRun */ }
#[test] fn a_shell_block_may_run_at_the_provider() { /* kind Shell, Block → ProviderMayRun; kind Function, Block → Prevented */ }
#[test] fn an_unavailable_set_takes_the_principal_from_the_prn() { /* ctx.policy Unavailable → principal = to_cedar_uid(Prn::parse(principal_prn)), mode None, session placeholder */ }
#[test] fn a_request_entry_record() { /* RequestEntry(RemoveDefinition, enforced false) → side Request, applied Observe, block_code Policy, basis Policy, cedar None, arguments None, kind "" */ }
#[test] fn a_refusal_record() { /* request_action Refuse, applied Block, block_code None, tool empty */ }
#[test] fn a_one_mib_name_is_capped() { /* name "a"×1<<20 → tool.name.len() == 128 (sanitize_name), action_id.len() <= 256, truncated */ }
#[test] fn a_cut_never_splits_a_character() {                                      // Review Focus 4
    let ns = format!("{}é", "a".repeat(255));
    let (out, cut) = cap(&ns);
    assert!(cut && out.len() <= MAX_FIELD_BYTES && out == "a".repeat(255));
}
```

  `action_id` is `normalise(display_name)` and is cut like the other fields; `model` and `ingress` are cut too (from `DecisionContext`). `tool.name` is `sanitize_name(display_name)` and a name over 128 bytes also sets `truncated`.

- [ ] **Step 2: Run.** `cd rs && cargo nextest run -p paigasus-gateway-core -E 'test(decision_log) | test(provider_may_run)'`. Expected: FAIL to compile.
- [ ] **Step 3: Implement** the types and builders. `mode` for a response record is `Some(snapshot.mode)` when `ctx.policy` is `Some` and the set is available, else `None`. `session` is `Facts.session` or `SessionInfo::placeholder()`. `block_code` is the entry `code`.
- [ ] **Step 4: Run.** The crate tests and clippy. Expected: PASS.
- [ ] **Step 5: Commit.** `feat(rs): add the core decision record and its builders (SMA-755)`

---

### Task 4: The Cedar detail travels to the report site

**Files:**
- Modify: `rs/crates/libs/paigasus-gateway-core/src/policy/calls.rs` (`CallOutcome.cedar`, `decide_calls(.., now)`)
- Modify: `rs/crates/libs/paigasus-gateway-core/src/decider.rs` (`DecidedCall`, `DecideFuture`, `AllowAll`)
- Modify: `rs/crates/libs/paigasus-gateway-core/src/interceptor.rs` (`decided` takes `DecidedCall`s; `Action::Reported`)
- Modify: `rs/crates/libs/paigasus-gateway-core/src/body.rs` (`BodyCalls::reported`; `report` built from it)
- Modify: `rs/crates/libs/paigasus-gateway-core/src/lib.rs` (re-exports)
- Modify (mechanical): `rs/crates/services/paigasus-gateway/src/application/tool_policy/decider.rs`, `src/adapters/http/interception.rs`, `src/runtime.rs` tests, `tests/tool_interception.rs`, `tests/responses_proxy.rs`, `tests/tool_policy.rs`, the gateway-core `tests/` that call `decided`

**Interfaces:**
- Produces:
  - `#[non_exhaustive] #[derive(Debug, Clone, PartialEq)] pub struct DecidedCall { pub decision: Decision, pub detail: Option<Arc<DecisionDetail>> }`, `impl From<Decision> for DecidedCall` (detail `None`), `DecidedCall::new(decision, detail)`
  - `pub type DecideFuture = Pin<Box<dyn Future<Output = Result<Vec<DecidedCall>, DeciderError>> + Send + 'static>>`
  - `Interceptor::decided(&mut self, hold: HoldKey, result: Result<Vec<DecidedCall>, DeciderError>, now: Instant) -> Vec<Action>`
  - `Action::Reported(Box<ReportedCall>)`: one per report comment, in the same order, emitted right after the comment
  - `CallOutcome` gets `pub cedar: Option<CedarDetail>` (`Some` whenever a kernel verdict exists)
  - `pub fn decide_calls(snapshot: &PolicySnapshot, turn: &ToolTurn, calls: &[ToolCall], now: &dyn Fn() -> Instant) -> Vec<CallOutcome>`
  - `BodyCalls::reported(&self, decided: &[DecidedCall], basis: Basis) -> Vec<ReportedCall>` (call entries in order, then the unenforceable entries with `call: None`)
- Consumes: `decision_log::{DecisionDetail, ReportedCall, CallFacts, CedarDetail}` (Task 3), `Verdict.findings`, `Verdict.action_group` (Task 1).

- [ ] **Step 1: Write the failing tests.**

```rust
// interceptor.rs
#[test] fn every_report_comment_has_one_reported_action() {
    // OpenAI chat: two calls, decider blocks "rm"; then a second stream with a hold-limit block_all;
    // then a Responses stream with a web_search_call (protocol report).
    // For each: count(Action::Emit that starts with ": paigasus-decision") == count(Action::Reported),
    // and Reported[i].entry.to_json() equals the JSON inside comment i.
}
#[test] fn the_detail_of_a_decided_call_reaches_the_reported_action() { /* DecidedCall with detail D → Reported.detail is Arc::ptr_eq D */ }
#[test] fn a_decider_error_reports_without_detail() { /* Err(DeciderError) under Block → Reported.detail None, entry basis OnFailure */ }
// body.rs
#[test] fn reported_and_report_agree() { /* report(..) == report_header(&reported(..).map(|r| r.entry), requests) for a Responses body with a forced Truncated call */ }
// calls.rs
#[test] fn decide_calls_attaches_the_cedar_detail_and_the_duration() {
    // `now` returns t, t+3ms, t+5ms, t+6ms → durations 3ms and 1ms; detail.findings == verdict.findings; declared from the turn
}
#[test] fn an_unavailable_set_has_no_cedar_detail() { /* PolicySet::Unavailable → cedar None for every outcome */ }
```

  The `CedarDecider` maps each `CallOutcome` to `DecidedCall::new(o.decision, Some(Arc::new(DecisionDetail { basis: o.basis.into(), cedar: o.cedar, arguments: None })))`, and passes `&Instant::now` to `decide_calls`. Add `impl From<PolicyBasis> for RecordBasis` in `decision_log`. Its test `the_decider_attaches_a_detail_to_every_call` asserts `detail.is_some()` for each of four calls, and `basis == Unavailable` with `cedar == None` for an unavailable set.

  The gateway relay maps `Action::Reported(_) => {}` in this task (the record site is Task 8). The body path in `intercept_body` maps its fallback vectors with `DecidedCall::from`.

- [ ] **Step 2: Run.** `cd rs && cargo nextest run -p paigasus-gateway-core -p paigasus-gateway`. Expected: FAIL to compile.
- [ ] **Step 3: Implement.** Update the test helpers once (a `fn plain(d: Vec<Decision>) -> Vec<DecidedCall>`) rather than every call site.
- [ ] **Step 4: Run.** `cargo nextest run -p paigasus-gateway-core -p paigasus-gateway` and workspace clippy. Expected: PASS; every existing test passes.
- [ ] **Step 5: Commit.** `feat(rs): carry the Cedar detail of each call to the decision report site (SMA-755)`

---

### Task 5: The trace encoder and the cross-workspace fixtures

**Files:**
- Create: `rs/crates/libs/paigasus-gateway-core/src/decision_log/trace.rs`
- Create: `rs/crates/libs/paigasus-gateway-core/tests/decision_trace.rs` (golden lines, fixture writer and drift test)
- Create: `rs/tools/paigasus-policy-compiler/tests/fixtures/decision-trace.log`, `decision-trace.dw`, `decision-trace.expected.json`, `decision-trace-ids.log`, `decision-trace-ids.json`
- Modify: `rs/crates/libs/paigasus-gateway-core/moon.yml` (test inputs: the five fixture paths)

**Interfaces:**
- Produces (module `decision_log::trace`):
  - `pub enum Encoded { Line(TraceLine), Skipped(SkipReason) }`
  - `#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)] pub enum SkipReason { RequestSide, NoOrg, NoPrincipal, Truncated, UnsafeActionId }` with `as_str()` = `"request side" | "no org" | "no principal" | "truncated" | "unsafe action id"`
  - `pub struct TraceLine { .. }`, `impl Display` (one line, no newline), `pub fn ts(&self) -> i64`
  - `pub fn encode(record: &DecisionRecord) -> Encoded`
  - `pub fn escape(s: &str) -> String` (`s.escape_debug().to_string()`)
  - `pub fn is_safe_action_id(id: &str) -> bool` (P-11)
  - `pub fn order_key(r: &DecisionRecord) -> (i64, SystemTime, Uuid, u64)` (§ 5.11 order)

- [ ] **Step 1: Write the failing tests** (`tests/decision_trace.rs`).

```rust
#[test] fn a_response_record_gives_the_golden_line() {
    assert_eq!(line(&record_a()), concat!(
        r#"@1763460000 scope(principal: Pgs::Iam::Principal::"0190a1e5-0000-7000-8000-000000000004", resource: Pgs::Iam::Organization::"0190a100-0000-7000-8000-0000000000a1") "#,
        r#"request_context(input: {}, findings: ["name_collision"], call: { declared: "yes" }, request: { scope: "prn:pgs:iam:::organization/0190a100-0000-7000-8000-0000000000a1", ingress: "chat_completions", model: "gpt-4o" }, session: { id: "", turn: 0 }) "#,
        r#"Pgs::Gateway::Action::"read_file"::request(input: {}, callerPrincipal: Pgs::Iam::Principal::"0190a1e5-0000-7000-8000-000000000004", callerResource: Pgs::Iam::Organization::"0190a100-0000-7000-8000-0000000000a1", requestId: "req-1", sessionId: "")"#));
}
#[test] fn no_cedar_detail_gives_empty_findings_and_unknown_declared() { /* contains r#"findings: [], call: { declared: "unknown" }"# */ }
#[test] fn the_second_is_rounded_down() { /* decided_at = 1763460000.999 s → "@1763460000 " */ }
#[test] fn hostile_request_context_strings() {                                    // Review Focus 3
    // model = "gpt (x) \"q\" \\ \n end" → the line holds no '\n' and contains r#"model: "gpt (x) \"q\" \\ \n end""#
}
#[test] fn each_skip_reason() { /* Request side; org None; principal None; tool.truncated; ids "a(b", "a)b", "a\"b", "a\u{0}b", "a\rb" → UnsafeActionId */ }
#[test] fn safe_ids_include_namespaces_and_unicode() { /* "ns.read_file", "caf\u{e9}", "a::b", "a b" are safe */ }
#[test] fn the_fixtures_match_the_encoder() {
    // Builds the fixture content from known records and the seeded id generator (P-12).
    // With PAIGASUS_WRITE_FIXTURES=1 it writes the five files; otherwise it asserts byte equality and
    // names the file that drifted.
}
```

  `decision-trace.log` holds five response records of one org and principal, action `search`, at `@1000`, `@1500`, `@2000`, `@2100` and `@9000`, and the hostile-model line at `@9100` with action `read_file`. `decision-trace.dw` holds the policy of Task 6. `decision-trace.expected.json` is `["ALLOW","ALLOW","DENY","DENY","ALLOW","DENY"]`. The reason: the policy permits `search` while `count_within(1h, search) <= 2`, and the Authorizer records an event before it decides it, so the counts are 1, 2, 3, 4, then 1 again (5400 s after `@2100` is outside the 3600 s window); `read_file` has no permit (default deny).

- [ ] **Step 2: Run.** `cd rs && cargo nextest run -p paigasus-gateway-core --test decision_trace`. Expected: FAIL to compile.
- [ ] **Step 3: Implement** `encode`. Then generate: `PAIGASUS_WRITE_FIXTURES=1 cargo nextest run -p paigasus-gateway-core --test decision_trace -E 'test(the_fixtures_match_the_encoder)'`. Add the five fixture paths as workspace-relative `inputs` of the `test` task in `paigasus-gateway-core/moon.yml` (`/rs/tools/paigasus-policy-compiler/tests/fixtures/decision-trace*`).
- [ ] **Step 4: Run.** The test without the env var. Expected: PASS. Mutation: change one character in `decision-trace.log`; the drift test must FAIL and name the file. Restore with the generator.
- [ ] **Step 5: Commit.** `feat(rs): encode decision records as Dogwood trace lines (SMA-755)`

---

### Task 6: Dogwood parses and replays the trace

**Files:**
- Create: `rs/tools/paigasus-policy-compiler/tests/decision_trace.rs`
- Modify: `rs/tools/paigasus-policy-compiler/moon.yml` (`build`, `build-release`, `test`, `lint` inputs: `tests/fixtures/**/*`)

**Interfaces:**
- Consumes: the fixtures of Task 5; `dogwood_language::{parse_trace, Authorizer, Decision}`; `paigasus_policy_compiler::lower::{lower_units, service_schema, split_policies}`; `compile::compile_cedarschema` and `tests/fixtures/test_schema.cedarschema` (the `dogwood_decisions` helper of `tests/corpus.rs:54-70` is the model).

- [ ] **Step 1: Write the tests.**

```rust
#[test] fn the_decision_trace_parses() { assert_eq!(parse_trace(TRACE).expect("parses").len(), 6); }
#[test] fn the_replay_gives_the_exact_verdict_sequence() {
    // decision-trace.dw: @id("search-rate") permit (principal, action == Pgs::Gateway::Action::"search", resource)
    //   when temporal { count_within(1h, Pgs::Gateway::Action::"search"::request{}) <= 2 };
    // Authorizer::new(lower_units(..)); for each event: is_authorized(&event).decision().
    // Note (tests/corpus.rs): the Authorizer records each event before it decides it.
    assert_eq!(verdicts(), expected_from_json());   // e.g. ALLOW, ALLOW, DENY at line 3
}
#[test] fn every_generated_action_id_round_trips() {
    // decision-trace-ids.log line i parses, and its action id == decision-trace-ids.json[i].
    // Assert the count is > 400 (the generator keeps most of the 512 ids) so the test is not vacuous.
}
```

- [ ] **Step 2: Run.** `cd rs/tools/paigasus-policy-compiler && cargo nextest run --test decision_trace`. If the verdict sequence differs from the fixture, read why (window arithmetic, F23-F24), change the record times in Task 5's generator, regenerate, and commit both together. The sequence must hold at least one `DENY` and one `ALLOW` after it.
- [ ] **Step 3: Moon inputs.** Add `'tests/fixtures/**/*'` to the four `mergeInputs: 'replace'` task input lists. Run `moon run paigasus-policy-compiler-rs:test`.
- [ ] **Step 4: Mutation.** In the gateway-core encoder, stop skipping `(` (comment the check out so it still compiles), regenerate the ids fixture: `every_generated_action_id_round_trips` must FAIL in the compiler workspace with a parse error. Restore and regenerate.
- [ ] **Step 5: Commit.** `test(rs): replay the decision trace fixture with Dogwood (SMA-755)`

---

### Task 7: The `DecisionLog` port, the tracing adapter and the recorder

**Files:**
- Create: `rs/crates/services/paigasus-gateway/src/domain/decision_log.rs`
- Create: `rs/crates/services/paigasus-gateway/src/application/decision_log.rs`
- Modify: `rs/crates/services/paigasus-gateway/src/domain/mod.rs`, `src/application/mod.rs`, `Cargo.toml` (`hmac = "0.13"`, `rand = { workspace = true }`)

**Interfaces:**
- Produces (`domain::decision_log`):
  - `pub trait DecisionLog: Send + Sync { fn wants_digest(&self) -> bool; fn record(&self, record: DecisionRecord); }` (§ 5.5 docs)
  - `pub struct TracingDecisionLog;` (`wants_digest` false; `record` calls `log_line`)
  - `pub struct MemoryDecisionLog` with `new(wants_digest: bool)`, `records(&self) -> Vec<DecisionRecord>` (P-9)
  - `pub fn log_line(r: &DecisionRecord)` (P-10; fields `request_id`, `tool`, `verdict`, `basis`, `mode`, `determining`, `errored`; message `"tool-policy decision"`)
  - `pub trait Entropy: Send + Sync { fn bytes10(&self) -> [u8; 10]; }`, `pub struct OsEntropy;` (`rand::random`)
- Produces (`application::decision_log`):
  - `pub struct DigestKey { id: String, key: SecretString }` with `pub fn new(id: String, key: SecretString) -> Result<Self, String>` (refuses a key under 32 bytes) and `pub fn digest(&self, arguments: &[u8]) -> ArgumentDigest` (`Hmac<Sha256>`)
  - `pub struct DecisionRecorder`; `pub fn new(log: Arc<dyn DecisionLog>, clock: Arc<dyn Clock>, entropy: Arc<dyn Entropy>, digest: Option<DigestKey>) -> Self` (mints `instance_id` with `mint_uuid7`)
  - `pub fn instance_id(&self) -> Uuid`, `pub fn log(&self) -> &Arc<dyn DecisionLog>`
  - `pub fn digest(&self, arguments: &str) -> Option<ArgumentDigest>` (`None` unless `log.wants_digest()` and a key exists)
  - `pub fn record_response(&self, ctx: &DecisionContext, r: &ReportedCall)`, `pub fn record_request(&self, ctx: &DecisionContext, mode: Option<TenantMode>, e: &RequestEntry)`, `pub fn record_refusal(&self, ctx: &DecisionContext, mode: Option<TenantMode>)`. Each builds the header (org = `response_org(ctx)` or `crate::domain::org_of(&ctx.scope)`; sequence = next value of the `(instance, org)` counter, starting at 1; `decision_id = mint_uuid7(unix_ms, entropy.bytes10())`), calls the core builder and `log.record`.

- [ ] **Step 1: Write the failing tests** (unit tests in both files).

```rust
#[test] fn sequences_start_at_1_per_org() { /* ORG_A, ORG_A, ORG_B, no org, ORG_A → 1, 2, 1, 1, 3 */ }
#[test] fn the_decision_id_is_a_uuid7_from_the_clock_and_the_entropy() { /* FixedClock + fixed bytes → expected Uuid, version 7 */ }
#[test] fn the_digest_is_hmac_not_sha256() {
    // key "k"*32, args {"secret":"s3cr3t-canary"} → equals a known HMAC-SHA256 vector; != Sha256::digest(args)
}
#[test] fn a_short_digest_key_is_refused() { assert!(DigestKey::new("id".into(), "short".into()).is_err()); }
#[test] fn no_digest_without_wants_digest() { /* MemoryDecisionLog::new(false) + a key → digest None */ }
#[test] fn the_tracing_log_line_has_no_arguments() { /* paigasus_logging test capture: one line, fields as P-10, no "s3cr3t-canary" */ }
#[test] fn an_on_failure_or_request_record_writes_no_line() { /* basis OnFailure → no line; side Request → no line */ }
```

- [ ] **Step 2: Run.** `cd rs && cargo nextest run -p paigasus-gateway -E 'test(decision_log)'`. Expected: FAIL to compile.
- [ ] **Step 3: Implement.** The sequence map is `Mutex<HashMap<Option<Uuid>, u64>>` (no eviction, § 5.8).
- [ ] **Step 4: Run.** Tests and clippy. Expected: PASS.
- [ ] **Step 5: Commit.** `feat(rs): add the DecisionLog port, the tracing log and the decision recorder (SMA-755)`

---

### Task 8: Record every report entry, the digest and the findings metric

**Files:**
- Modify: `rs/crates/services/paigasus-gateway/src/application/tool_policy/decider.rs` (`CedarDecider { recorder }`, digest, findings metric; the tracing line goes)
- Modify: `rs/crates/services/paigasus-gateway/src/application/tool_policy/request.rs` (`RequestToolPolicy::new(cache, settings, recorder)`, request records, request-side findings)
- Modify: `rs/crates/services/paigasus-gateway/src/adapters/http/interception.rs` (`ToolInterception.recorder: Option<Arc<DecisionRecorder>>`; the two record sites of P-1)
- Modify: `rs/crates/services/paigasus-gateway/src/adapters/http/pipeline.rs` (`ctx.with_request(api_key_id, ingress(endpoint.format), model)`)
- Modify: `rs/crates/services/paigasus-gateway/src/runtime.rs` (`build_tool_policy(config, recorder)`; `ToolPolicyRuntime.recorder`)
- Modify: `rs/crates/services/paigasus-gateway/src/main.rs` (build the recorder with `TracingDecisionLog`; one `describe_counter!`)
- Modify: `rs/crates/libs/paigasus-observability/src/names.rs` (`GATEWAY_TOOL_POLICY_FINDINGS_TOTAL`)
- Modify: `docs/ops/RUNBOOK-observability.md` § 2.3 (one row)
- Modify (mechanical, the new `RequestToolPolicy::new` and `CedarDecider::new`): `src/application/tool_policy/testing.rs`, `tests/tool_policy.rs`, `tests/tool_policy_nats.rs`, `tests/tool_policy_iam_source.rs`, `tests/perf.rs`, and the unit tests in `src/adapters/http/interception.rs`

**Interfaces:**
- Consumes: `DecisionRecorder` (Task 7), `Action::Reported`, `BodyCalls::reported` (Task 4).
- Produces:
  - `CedarDecider::new(recorder: Arc<DecisionRecorder>) -> Self`
  - `pub const FINDING_KINDS: [&str; 3] = ["name_collision", "unparsed_arguments", "other"];` and `pub fn finding_label(kind: &str) -> &'static str` in `application::tool_policy`
  - `pub fn prime_tool_policy_metrics()`: `gateway_tool_policy_findings_total{side, kind}` at 0 for `side ∈ {request, response}` × the three kinds; called by `build_tool_policy` when enabled
  - `ToolPolicyRuntime { policy, decider, recorder: Option<Arc<DecisionRecorder>> }`
  - `names::GATEWAY_TOOL_POLICY_FINDINGS_TOTAL = "gateway_tool_policy_findings_total"`

- [ ] **Step 1: Write the failing tests.** In `interception.rs` (unit, with a `MemoryDecisionLog` recorder):

```rust
#[tokio::test] async fn the_stream_path_records_once_per_report_entry() { /* 3 calls, one blocked → 3 response records, sequences 1..=3 */ }
#[tokio::test] async fn the_body_path_records_once_per_report_entry() { /* same body as JSON → 3 records */ }
#[tokio::test] async fn an_observed_hold_records_allow_with_a_cedar_block() { /* on_failure allow + hold limit before the decision → applied Observe, block_code Policy, cedar.effect Deny, basis OnFailure */ }
#[tokio::test] async fn a_truncation_under_block_records_on_failure() { /* stream ends mid-call → block_code Truncated, basis OnFailure, cedar None */ }
#[tokio::test] async fn a_hold_limit_block_has_no_cedar_detail() { /* basis OnFailure, cedar None */ }
#[tokio::test] async fn tenant_off_records_a_floor_block() { /* mode Off, floor forbids "rm" → applied Block, mode Some(Off), cedar.floor_denied */ }
#[tokio::test] async fn an_org_less_scope_records_no_org() { /* scope "prn:pgs:iam:::root/…" → record.org None, sequence pair (instance, None) */ }
#[tokio::test] async fn the_digest_is_set_only_when_the_log_wants_it() { /* MemoryDecisionLog::new(true) + key → arguments Some; new(false) → None */ }
```

  In `request.rs`: `a_removal_and_a_refusal_are_recorded` (one `RemoveDefinition` record and one `Refuse` record for the enforced computer-result refusal of the existing test), `a_log_only_would_refuse_writes_no_refusal_record`, `request_side_findings_are_counted` (two colliding definitions → `findings_total{side="request",kind="name_collision"}` = 1, read as a number). In `decider.rs`: `response_side_findings_are_counted` (`unparsed_arguments` → 1; an unknown kind `"zz"` from a fake verdict is not possible, so test `finding_label("zz") == "other"` directly). In `runtime.rs`: `the_decider_the_request_side_and_the_report_site_share_one_recorder` (`Arc::ptr_eq` on the three handles of `ToolPolicyRuntime` and the `ToolInterception` that `main` builds through a new `runtime::tool_interception(config, &rt)` helper), and `prime_tool_policy_metrics_primes_six_series_at_zero`.

- [ ] **Step 2: Run.** `cd rs && cargo nextest run -p paigasus-gateway`. Expected: FAIL.
- [ ] **Step 3: Implement.** The digest is computed in `decide_now`, once per call, only when `recorder.digest(..)` returns `Some` (inside the timed region, so it is part of the overhead sample, § 5.3). `api_key_id` is `Some(key_id)` for `Credential::ApiKey`, else `None`. Move the `refs` helper into `domain::decision_log` for `log_line`. Add the `describe_counter!` line ("Tool-policy findings, labeled by side (request, response) and kind (name_collision, unparsed_arguments, other).") and the runbook row.
- [ ] **Step 4: Run** the gateway tests, `cargo nextest run -p paigasus-observability`, and `moon run repo:observability-drift`. Expected: PASS.
- [ ] **Step 5: Mutations** (each must compile and fail a test, Global Constraints). (a) Delete the `record_response` call in `Relay::apply`: `the_stream_path_records_once_per_report_entry` FAILS. (b) Delete it in `intercept_body`: the body test FAILS. (c) In `runtime.rs`, give `RequestToolPolicy` a second `DecisionRecorder`: the shared-recorder test FAILS. Restore each by deleting the marked edit.
- [ ] **Step 6: Commit.** `feat(rs): record each tool decision at the decision report entry (SMA-755)`

---

### Task 9: The `[decision_log]` configuration

**Files:**
- Modify: `rs/crates/services/paigasus-gateway/src/config.rs` (`DecisionLogConfig`, `DecisionLogNatsConfig`, validation)
- Modify: `rs/crates/services/paigasus-gateway/gateway.toml.example` (the § 5.12 block, commented)

**Interfaces:**
- Produces:
  - `GatewayConfig.decision_log: DecisionLogConfig` (`#[serde(default)]`)
  - `#[serde(default, deny_unknown_fields)] pub struct DecisionLogConfig { pub enabled: bool, pub redis_url: Option<SecretString>, pub key_prefix: String, pub stream_max_len: u64, pub queue_capacity: usize, pub digest_key: Option<SecretString>, pub digest_key_id: String, pub nats: Option<DecisionLogNatsConfig> }` — defaults `false`, `None`, `"paigasus:gateway"`, `100_000`, `4096`, `None`, `""`, `None`; secrets `skip_serializing`
  - `#[serde(deny_unknown_fields)] pub struct DecisionLogNatsConfig { pub url: SecretString, pub creds_file: Option<String>, pub ca_file: Option<String> }`
  - `DecisionLogConfig::validate(&self, tp: &ToolPolicyConfig) -> Result<(), String>`, called from `GatewayConfig::validate`
  - `DecisionLogConfig::digest_key(&self) -> Option<DigestKey>`

- [ ] **Step 1: Write the failing tests** (in `config.rs`, figment `Jail`).

```rust
#[test] fn the_decision_log_is_off_by_default() { /* no table → enabled false, key_prefix "paigasus:gateway", stream_max_len 100000, queue_capacity 4096 */ }
#[test] fn enabled_without_the_tool_policy_is_refused() { /* error names "decision_log.enabled" and "tool_policy" */ }
#[test] fn enabled_needs_redis_url_digest_key_key_id_and_nats() { /* four cases, each error names the key */ }
#[test] fn a_digest_key_under_32_bytes_is_refused() {}
#[test] fn zero_capacity_and_zero_max_len_are_refused() {}
#[test] fn an_empty_or_padded_key_prefix_is_refused() {}
#[test] fn a_misspelt_key_fails_extraction() { /* [decision_log] stream_maxlen = 1 → error */ }
#[test] fn the_secrets_do_not_serialize_and_debug_is_redacted() { /* serde_json of the config holds no redis_url, digest_key, nats.url; {:?} holds no secret text */ }
```

- [ ] **Step 2: Run.** `cd rs && cargo nextest run -p paigasus-gateway -E 'test(decision_log)'`. Expected: FAIL.
- [ ] **Step 3: Implement.** `main` passes `config.decision_log.digest_key()` to the recorder of Task 8 (still `TracingDecisionLog`, so `digest` returns `None`).
- [ ] **Step 4: Run.** Tests and clippy. Expected: PASS.
- [ ] **Step 5: Commit.** `feat(rs): add the decision_log configuration section (SMA-755)`

---

### Task 10: The Redis adapter and the writer task

**Files:**
- Modify: `rs/Cargo.toml` (add `"streams"` to the workspace `redis` features, with a comment)
- Modify: `rs/crates/services/paigasus-gateway/Cargo.toml` (`prost = { workspace = true }`, `prost-types = { workspace = true }`)
- Create: `rs/crates/services/paigasus-gateway/src/adapters/decision_log/mod.rs` (`RedisDecisionLog`), `proto.rs` (mapping), `writer.rs`, `redis_conn.rs` (the connect loop)
- Modify: `rs/crates/services/paigasus-gateway/src/adapters/mod.rs`
- Modify: `names.rs` (`GATEWAY_DECISION_LOG_RECORDED_TOTAL`, `_WRITTEN_TOTAL`, `_DROPPED_TOTAL`, `_TASK_RESTARTS_TOTAL`), `main.rs` (four `describe_*!` lines), RUNBOOK § 2.3 (four rows)
- Create: `rs/crates/services/paigasus-gateway/tests/decision_log_redis.rs`
- Modify: `rs/.config/nextest.toml:134` (add `binary(decision_log_redis)`)

**Interfaces:**
- Produces:
  - `pub fn to_event(r: &DecisionRecord) -> ToolDecisionEvent` and `pub fn from_event(e: ToolDecisionEvent) -> Result<DecisionRecord, MapError>` (`proto.rs`; `MapError` is a `thiserror` enum: `BadUuid(&'static str)`, `MissingField(&'static str)`, `BadEnum(&'static str)`)
  - `pub enum DropReason { QueueFull, Closed, RedisUnavailable, WriteError, BreakerOpen, BacklogFull, PublishRejected, RedisDeleted, Shutdown }` with `as_label()` and `ALL: [DropReason; 9]` (the § 5.8 labels)
  - `pub struct RedisDecisionLog` with `pub fn new(queue_capacity: usize) -> (Self, mpsc::Receiver<Bytes>)`; `wants_digest` true; `record` = `log_line`, `to_event`, `encode_to_vec`, `recorded_total += 1`, `try_send` (full → `queue_full`, closed → `closed`)
  - `pub struct RedisSlot(Arc<OnceLock<RedisHandle>>)`; `pub async fn connect_loop(url: SecretString, slot: RedisSlot, shutdown: watch::Receiver<()>)` (backoff 1 s doubling to 30 s; breaker role `"decision_log"`)
  - `pub struct WriterSettings { key: String, max_len: u64 }` (`key` = `<prefix>:decisions`)
  - `pub async fn run_writer(rx: mpsc::Receiver<Bytes>, slot: RedisSlot, s: WriterSettings, shutdown: watch::Receiver<()>, budget: Duration)`
  - `pub fn prime_decision_log_metrics()` (all decision-log counters at 0: each of the nine `reason` values, each `task` value)
  - `pub const DECISION_LOG_BREAKER_ROLE: &str = "decision_log";`

- [ ] **Step 1: Write the failing unit tests** (`proto.rs`, `mod.rs`).

```rust
#[test] fn a_record_survives_the_proto_round_trip() { /* every field, response and request side */ }
#[test] fn audit_carries_created_at_and_creator_only() { /* audit.created_at == decided_at; creator.prn == principal_prn; modified_at, modifier None */ }
#[test] fn a_full_queue_counts_queue_full() { /* capacity 1, two records → dropped{queue_full} == 1, recorded == 2 */ }
#[test] fn a_closed_queue_counts_closed() { /* drop the receiver → dropped{closed} == 1 */ }
#[test] fn the_encoded_event_holds_no_canary() { /* arguments {"secret":"s3cr3t-canary"}: encoded bytes do not contain the canary */ }
#[test] fn prime_primes_every_reason_at_zero() { /* DebuggingRecorder; 9 dropped series + 2 restart series + recorded + written, each parsed as 0 */ }
```

  Integration tests (`tests/decision_log_redis.rs`, `7.4-alpine`; read Redis only with `redis-cli` through `ExecCommand`):

```rust
#[tokio::test] async fn n_records_give_n_stream_entries_with_the_original_bytes() { /* XLEN == 5; XRANGE field "e" == encoded bytes */ }
#[tokio::test] async fn redis_down_at_boot_counts_redis_unavailable_then_writes() { /* slot empty → 3 records dropped{redis_unavailable}; start Redis on the reserved port; later records written */ }
#[tokio::test] async fn an_open_breaker_counts_breaker_open() { /* paigasus_redis::with_open_breaker_for_tests */ }
#[tokio::test] async fn a_silent_redis_counts_write_error() { /* paigasus_redis::test_support::start() blackhole; each event of the batch → write_error */ }
#[tokio::test] async fn a_backlog_at_max_len_drops_each_new_event() { /* max_len 3, no relay, 5 records → XLEN 3, dropped{backlog_full} 2 */ }
#[tokio::test] async fn shutdown_drains_then_counts_the_rest_as_shutdown() { /* budget 0 with a blackhole → each queued event dropped{shutdown}; after shutdown, record → closed */ }
```

  The panic case is a unit test of the shared loop helper (`writer.rs` or a small `supervised.rs`), with no Redis: `pub async fn run_supervised<F, Fut>(task: &'static str, shutdown: watch::Receiver<()>, iteration: F) where F: FnMut() -> Fut, Fut: Future<Output = ()>`. Test `a_panic_in_one_iteration_counts_a_restart_and_goes_on`: an iteration that panics once, then counts → `task_restarts{task="writer"}` == 1 (parsed as a number) and the next iterations run. Both the writer and the relay use this helper.

- [ ] **Step 2: Measure the feature.** `cd rs && cargo tree -e features -p redis` and compare with Task 0: `streams` is new, and no new crate appears in `Cargo.lock` (`git diff --stat rs/Cargo.lock` is empty or touches only `redis`'s own feature lines). `deny.toml` does not change.
- [ ] **Step 3: Run the tests.** Expected: FAIL.
- [ ] **Step 4: Implement.** `XADD <key> MAXLEN ~ <2 × max_len> * e <bytes>`, up to 64 per `redis::pipe()`. Sample `XLEN` once per second; at or above `max_len`, drop with `backlog_full`. Classify a failed pipeline: `err.to_string().contains(paigasus_redis::BREAKER_OPEN_MESSAGE)` → `breaker_open`, else `write_error`. No retry. Wrap one iteration in `AssertUnwindSafe(..).catch_unwind()` (`futures::FutureExt`), log, count `task_restarts{task="writer"}`, continue. On shutdown: `rx.close()`, write what is left within `budget`, count the rest as `shutdown`.
- [ ] **Step 5: Run** the unit and Docker tests, `moon run repo:redis-connect-single-site repo:observability-drift`. Expected: PASS.
- [ ] **Step 6: Commit.** `feat(rs): write decision events to a Redis stream (SMA-755)`

---

### Task 11: The relay to JetStream and the loss accounting

**Files:**
- Create: `rs/crates/services/paigasus-gateway/src/adapters/decision_log/relay.rs`, `jetstream.rs`
- Modify: `rs/crates/services/paigasus-gateway/Cargo.toml` (no new crate: `async-nats` and `paigasus-nats-creds` exist)
- Modify: `names.rs` (`GATEWAY_DECISION_LOG_PUBLISHED_TOTAL`, `_REDIS_RESETS_TOTAL`, `_LOST_ESTIMATE_TOTAL`, `_PUBLISH_SECONDS`, `_BACKLOG`), `main.rs` (five `describe_*!`), RUNBOOK § 2.3 (five rows), `prime_decision_log_metrics` (the three new counters)
- Modify: `rs/crates/services/paigasus-gateway/tests/decision_log_redis.rs` (relay cases; a NATS container with JetStream and no auth, as `tests/tool_policy_nats.rs:37-50`)
- Modify: `ops/nats/subjects.env` (only the stream variables of Task 14's list: `GATEWAY_ACCOUNT` to `DECISIONS_MAX_MSG_SIZE`; no identity yet, so `check-subjects.sh` is unchanged)

**Interfaces:**
- Produces (`jetstream.rs`):
  - `pub const STREAM: &str = "GATEWAY_DECISIONS"; pub const SUBJECT_PREFIX: &str = "gateway.decision"; pub const NO_ORG_TOKEN: &str = "_none"; pub const INBOX_PREFIX: &str = "_INBOX_GW_DEC";`
  - `pub fn stream_config() -> jetstream::stream::Config` (P-20 values)
  - `pub fn subject_for(org_id: &str) -> Option<String>` (`""` → `gateway.decision._none`; a canonical lower-case hyphenated UUID → `gateway.decision.<uuid>`; anything else → `None`)
  - `pub async fn connect(cfg: &DecisionLogNatsConfig, violations: Arc<AtomicBool>) -> Result<jetstream::Context, NatsError>` (creds callback, `custom_inbox_prefix(INBOX_PREFIX)`, `retry_on_initial_connect()`, `max_reconnects(None)`, CA, `event_callback` sets `violations` on a `ServerError` that contains `Permissions Violation for Publication`)
  - `pub enum PublishClass { Transient, Permanent(&'static str) }` and `pub fn classify(kind: PublishErrorKind, violation_seen: bool) -> PublishClass` (P-14)
- Produces (`relay.rs`): `pub struct RelaySettings { key: String, group: &'static str /* "relay" */, consumer: String /* instance id */ }`; `pub async fn run_relay(slot: RedisSlot, js: jetstream::Context, s: RelaySettings, violations: Arc<AtomicBool>, shutdown: watch::Receiver<()>, budget: Duration)`

- [ ] **Step 1: Write the failing tests.** Unit: `subject_for` cases, `classify` table (every `PublishErrorKind` variant, with and without the flag), `the_stream_config_matches_subjects_env` (read `ops/nats/subjects.env` from `CARGO_MANIFEST_DIR/../../../../ops/nats/subjects.env`, take the `DECISIONS_*` values with the default disk, and assert name, subjects, max age, max bytes, dupe window and max message size against `stream_config()`). Add `/ops/nats/subjects.env` to the `test` inputs in `paigasus-gateway/moon.yml`. Docker (Redis + NATS):

```rust
#[tokio::test] async fn n_records_give_n_messages_with_the_right_subjects_and_bytes() { /* 2 orgs + 1 org-less → subjects, Nats-Msg-Id == decision_id, payload == XRANGE bytes; XLEN 0 after */ }
#[tokio::test] async fn nats_down_keeps_the_events_in_redis_until_it_returns() { /* stop NATS; records stay (XLEN 4); start; all published once */ }
#[tokio::test] async fn a_poison_entry_is_rejected_and_the_next_publishes() { /* XADD a 70 KiB event via redis-cli → publish_rejected 1; next 2 published */ }
#[tokio::test] async fn a_non_canonical_org_id_is_rejected_and_never_published() { /* Review Focus 1: org_id "0190A100-…", "{…}", "a.b", "*" via redis-cli → publish_rejected 4; no message on gateway.decision.> */ }
#[tokio::test] async fn an_unknown_field_survives_the_relay_byte_for_byte() { /* Review Focus 2: encoded event + field 99 (varint) → the published payload equals the stream bytes */ }
#[tokio::test] async fn flushall_with_two_relays_counts_one_reset() { /* two relays, backlog sampled; FLUSHALL → redis_resets_total sum == 1; lost_estimate == the sampled XLEN */ }
#[tokio::test] async fn a_dead_instance_pending_entries_are_claimed_once() { /* relay A reads (XREADGROUP) and is dropped before XACK; relay B XAUTOCLAIM with min idle lowered via RelaySettings → each entry published once (count by Nats-Msg-Id) */ }
#[tokio::test] async fn a_deleted_pending_id_counts_redis_deleted() { /* XDEL a pending id → XAUTOCLAIM reports it deleted → dropped{redis_deleted} 1 */ }
#[tokio::test] async fn the_relay_creates_a_bounded_stream_and_warns_on_drift() { /* no stream → created with stream_config(); a pre-made stream with max_bytes -1 → warning logged, stream unchanged */ }
```

  `RelaySettings` carries `claim_idle: Duration` (60 s), `claim_every: Duration` (30 s), `prune_every: Duration` (10 min), `prune_idle: Duration` (1 h), `sample_every: Duration` (15 s), `idle_sleep: Duration` (200 ms) as fields with those defaults, so the tests can shorten them.

- [ ] **Step 2: Run.** Expected: FAIL.
- [ ] **Step 3: Implement** the § 5.7 loop: `XGROUP CREATE <key> relay 0 MKSTREAM` (ignore `BUSYGROUP`); `XREADGROUP GROUP relay <instance> COUNT 256 STREAMS <key> >` with no `BLOCK`; decode each entry only to read `org_id` and `decision_id`; publish the original bytes with `PublishMessage::build().payload(..).message_id(decision_id)`, up to 32 in flight (`futures::stream::iter(..).buffer_unordered(32)`); after the acks, one `redis::pipe().atomic()` with `XACK` and `XDEL` of the published and the rejected ids; a transient error stops the batch, keeps the rest pending, and backs off 1 s to 30 s. `NOGROUP` (`err.code() == Some("NOGROUP")`) → `XGROUP CREATE … 0 MKSTREAM`; on `OK` count one reset and add the last 15 s `XLEN` sample to `lost_estimate` (log "unknown" when no sample). Every 10 min `XINFO CONSUMERS` and `XGROUP DELCONSUMER` for 0 pending and idle above 1 h. `publish_seconds` times publish to PubAck. `catch_unwind` per iteration as in Task 10 (`task="relay"`). Ensure the stream with `get_or_create_stream(stream_config())`, retrying with the same backoff; compare the returned config and `tracing::warn!` on drift.
- [ ] **Step 4: Run** the tests and `moon run repo:observability-drift`. Expected: PASS.
- [ ] **Step 5: Commit.** `feat(rs): relay decision events from Redis to JetStream with loss accounting (SMA-755)`

---

### Task 12: Wire the decision log into the gateway

**Files:**
- Modify: `rs/crates/services/paigasus-gateway/src/runtime.rs` (`build_decision_log`, `DecisionLogTasks`, `supervise` second drain)
- Modify: `rs/crates/services/paigasus-gateway/src/main.rs` (build order, spawn, pass the tasks to `supervise`)
- Create: `rs/crates/services/paigasus-gateway/tests/decision_log_e2e.rs`
- Modify: `rs/.config/nextest.toml:134` (add `binary(decision_log_e2e)`)

**Interfaces:**
- Produces:
  - `pub const DECISION_LOG_DRAIN_BUDGET: Duration = Duration::from_secs(2);`
  - `pub struct DecisionLogRuntime { pub recorder: Arc<DecisionRecorder>, pub tasks: Option<DecisionLogTasks> }`
  - `pub struct DecisionLogTasks { shutdown: watch::Sender<()>, writer: JoinHandle<()>, relay: JoinHandle<()>, connector: JoinHandle<()> }` with `pub async fn drain(self, budget: Duration)`
  - `pub async fn build_decision_log(config: &GatewayConfig, clock: Arc<dyn Clock>, entropy: Arc<dyn Entropy>) -> anyhow::Result<DecisionLogRuntime>` (`TracingDecisionLog` when off; `RedisDecisionLog` + the three tasks + `prime_decision_log_metrics()` when on; never waits for Redis or NATS)
  - `supervise(servers, shutdown, tx, charges, decision_log: Option<DecisionLogTasks>)`

- [ ] **Step 1: Write the failing tests.** In `runtime.rs`: `the_decision_log_off_uses_the_tracing_log` (`wants_digest` false, `tasks` None); `the_decision_log_on_shares_one_redis_log` (enabled config with an unreachable Redis and NATS: builds at once, `wants_digest` true, and the decider, the request side and the `ToolInterception` hold the same `Arc<DecisionRecorder>`); `supervise_drains_the_decision_log_after_the_servers` (a fake server task that records a decision during its own graceful stop; the writer sees it, so `written == 1`). In `tests/decision_log_e2e.rs` (Redis + NATS containers, the router through `app_with_tool_policy`-style setup with a real `build_decision_log`):

```rust
#[tokio::test] async fn a_blocked_call_through_the_router_reaches_gateway_decision_org() { /* subscribe gateway.decision.<ORG_A>; one chat completion with a floor-blocked call → one message; decode → applied Block, prevention Prevented, side Response */ }
#[tokio::test] async fn the_canary_never_leaves_the_gateway() {
    // § 5.13: a call with {"secret":"s3cr3t-canary"} on each path (allowed, blocked, observed hold, truncated, unavailable).
    // Assert the canary is not in: the published payloads, the encode() lines, the captured tracing output,
    // the rendered /metrics text (labels). Assert arguments.hmac_sha256 != Sha256::digest(arguments).
}
#[tokio::test] async fn the_tool_policy_off_writes_nothing() { /* tool_policy disabled → XLEN 0, no message */ }
```

- [ ] **Step 2: Run.** Expected: FAIL.
- [ ] **Step 3: Implement.** In `main`: build the recorder after `paigasus_observability::init` and before `build_tool_policy`; spawn the three tasks outside the `JoinSet` (P-18).
- [ ] **Step 4: Run** `cargo nextest run -p paigasus-gateway`. Expected: PASS.
- [ ] **Step 5: Mutations.** (a) In `build_decision_log`, return `TracingDecisionLog` when enabled: `the_decision_log_on_shares_one_redis_log` and the e2e test FAIL. (b) Drain the decision log before the servers in `supervise`: the drain test FAILS. Restore.
- [ ] **Step 6: Commit.** `feat(rs): wire the Redis decision log into the gateway runtime (SMA-755)`

---

### Task 13: The `decisions export` CLI

**Files:**
- Create: `rs/crates/services/paigasus-gateway/src/application/decision_export.rs` (pure: arguments, filter, dedup, order, gaps, summary)
- Create: `rs/crates/services/paigasus-gateway/src/adapters/decision_log/reader.rs` (the NATS reader)
- Modify: `rs/crates/services/paigasus-gateway/src/main.rs` (dispatch `decisions export` before `health::dispatch`)
- Modify: `rs/crates/services/paigasus-gateway/Cargo.toml` (`time = "0.3"`, already in `Cargo.lock` at 0.3.55)
- Create: `rs/crates/services/paigasus-gateway/tests/decision_export_nats.rs`; `rs/.config/nextest.toml:134` (add the binary)

**Interfaces:**
- Produces (`application::decision_export`):
  - `pub struct ExportArgs { org: Uuid, since: SystemTime, until: SystemTime, principal: Option<String>, api_key: Option<String>, max_lag: Duration /* 1 h */, max_events: usize /* 5_000_000 */, out: Option<PathBuf>, report: Option<PathBuf>, nats_url_file: Option<PathBuf>, creds: Option<PathBuf>, ca: Option<PathBuf> }`
  - `pub fn parse_args<I: IntoIterator<Item = String>>(args: I) -> Result<ExportArgs, String>` (hand-written; `--org` through `parse_org_uuid`, then lower case; RFC 3339 with an offset; `--max-lag` as `<n>s|m|h`)
  - `pub struct Exported { pub stored_at: SystemTime, pub bytes: Bytes }`
  - `pub enum ExportError { WrongOrg(String) /* exit 3 */, TooMany /* exit 4 */, Decode }`
  - `pub fn export(args: &ExportArgs, events: impl IntoIterator<Item = Exported>) -> Result<Export, ExportError>` (decode with `from_event`, org check, `[since, until)`, filters, dedup by `decision_id`, order by `order_key`, `encode` each)
  - `pub struct Export { pub lines: Vec<String>, pub summary: Summary }` and `impl Display for Summary` (the § 5.11 text, with "gaps inside the range only")
  - `pub enum Exit { Done = 0, BadArgs = 1, Nats = 2, WrongOrg = 3, TooMany = 4 }`
- Produces (`reader.rs`): `pub async fn read(args: &ExportArgs) -> Result<Vec<Exported>, ReaderError>` (URL from `--nats-url-file` or `PAIGASUS_NATS_URL`; `custom_inbox_prefix("_INBOX_GW_RD")`; `get_stream_no_info(STREAM)`; `create_consumer(OrderedConfig { filter_subject: subject_for(org), deliver_policy: ByStartTime { start_time: since - max_lag } })`; read until `num_pending` of `cached_info()` messages arrived; `stored_at` from `msg.info()`)

- [ ] **Step 1: Write the failing tests** (unit tests in `decision_export.rs`, fed with `to_event` bytes).

```rust
#[test] fn lines_are_in_order_and_until_is_exclusive() {}
#[test] fn an_event_of_another_org_is_exit_3() {}
#[test] fn late_events_inside_max_lag_are_kept_and_counted() { /* stored 11 min after decided_at → "1 decisions arrived later than 10 min" */ }
#[test] fn duplicates_are_removed_and_counted() {}
#[test] fn sequence_gaps_per_instance() { /* sequences 1,2,5 → "2 missing (3, 4)" */ }
#[test] fn more_than_max_events_is_exit_4() {}
#[test] fn the_summary_has_every_warning() { /* skip counts per reason, shared-second count, Unregistered count, the three WARNING blocks */ }
#[test] fn principal_and_api_key_filters_select_one_history() {}
#[test] fn the_export_ignores_an_unknown_field() {}                                // Review Focus 2
#[test] fn since_not_before_until_is_exit_1() {}                                   // Review Focus 5
#[test] fn a_time_without_an_offset_is_exit_1() {}                                 // Review Focus 5
#[test] fn an_upper_case_org_is_canonical() {}                                     // Review Focus 5
#[test] fn the_url_is_never_read_from_argv() { /* --nats-url → exit 1 "unknown argument" */ }
```

  Docker (`tests/decision_export_nats.rs`): publish 6 events (two orgs) to a JetStream stream made with `stream_config()`; `read` for ORG_A returns exactly its 4, in stream order, and the CLI binary (`env!("CARGO_BIN_EXE_paigasus-gateway")`) writes a trace file of `@` lines only and exits 0.

- [ ] **Step 2: Run.** Expected: FAIL.
- [ ] **Step 3: Implement.** The trace file holds only `@` lines and goes to `--out`, or to stdout without it; the summary goes to stderr and to `--report`.
- [ ] **Step 4: Run** the tests. Expected: PASS. If the ordered consumer is refused because the account has no memory storage (`OrderedConfig` sets `memory_storage: true`, async-nats `pull.rs:711-750`), switch the reader to an ephemeral `pull::Config { ack_policy: None, inactive_threshold: 30 s, memory_storage: false }` with the same grants, and say so in your report. Task 14 proves this under the real account limits.
- [ ] **Step 5: Commit.** `feat(rs): export the decisions of one org as a Dogwood trace (SMA-755)`

---

### Task 14: The NATS account, the stream and the identities

**Files:**
- Modify: `ops/nats/subjects.env` (the gateway account block, P-13, P-15, P-20)
- Modify: `ops/nats/test/accounts.conf.tmpl` (account `PAIGASUS_GATEWAY`: `jetstream: { max_mem: 0, max_file: 268435456, max_streams: 2, max_consumers: 16 }`, three users)
- Modify: `ops/nats/test/nats-server.conf`, `ops/nats/test/nats-server-tls.conf` (`max_file_store: 805306368`)
- Modify: `ops/nats/check-subjects.sh` (three `check_identity` calls; the provisioner call's last argument becomes `'{{GW_DEC_PUBLISHER_NKEY}}'`)
- Create: `ops/nats/provision-decisions.sh` (bash >= 4.3 guard, `umask 077`, idempotent)
- Modify: `ops/nats/provision.sh` (call `provision-decisions.sh` for a new environment)
- Modify: `ops/nats/permissions.md` (a new section), `docs/ops/RUNBOOK-nats.md` (account, stream, sizing formula, identities, rollout order)
- Modify: `rs/crates/services/paigasus-iam/tests/nats_permissions.rs` (three new placeholders, new tests)

**Interfaces:**
- `subjects.env` names (new, distinct from the IAM names, because the file is sourced). Task 11 added the stream variables, from `GATEWAY_ACCOUNT` to `DECISIONS_MAX_MSG_SIZE`; this task adds the rest: `GATEWAY_ACCOUNT="PAIGASUS_GATEWAY"`, `DECISIONS_STREAM_NAME="GATEWAY_DECISIONS"`, `DECISIONS_SUBJECTS="gateway.decision.>"`, `DECISIONS_MAX_AGE_SECS=2592000`, `DECISIONS_ACCOUNT_DISK_BYTES="${DECISIONS_ACCOUNT_DISK_BYTES:-21474836480}"`, `DECISIONS_MAX_BYTES=$(( DECISIONS_ACCOUNT_DISK_BYTES * 9 / 10 ))`, `DECISIONS_DUPE_WINDOW_SECS=120`, `DECISIONS_MAX_MSG_SIZE=65536`, `GW_DEC_PUBLISHER_INBOX_PREFIX="_INBOX_GW_DEC"`, `GW_DEC_READER_INBOX_PREFIX="_INBOX_GW_RD"`, `GW_DEC_PROVISIONER_INBOX_PREFIX="_INBOX_GW_PROV"`, and the arrays:
  - `GW_DEC_PUBLISHER_PUB=( "gateway.decision.>" '$JS.API.STREAM.INFO.'"$DECISIONS_STREAM_NAME" '$JS.API.STREAM.CREATE.'"$DECISIONS_STREAM_NAME" )`, `GW_DEC_PUBLISHER_SUB=( "$GW_DEC_PUBLISHER_INBOX_PREFIX.>" )`
  - `GW_DEC_READER_PUB=( '$JS.API.CONSUMER.CREATE.'"$DECISIONS_STREAM_NAME" '$JS.API.CONSUMER.CREATE.'"$DECISIONS_STREAM_NAME"'.>' '$JS.API.CONSUMER.DELETE.'"$DECISIONS_STREAM_NAME"'.*' '$JS.API.CONSUMER.MSG.NEXT.'"$DECISIONS_STREAM_NAME"'.*' )`, `GW_DEC_READER_SUB=( "$GW_DEC_READER_INBOX_PREFIX.>" )`
  - `GW_DEC_PROVISIONER_PUB=( '$JS.API.INFO' '$JS.API.STREAM.>' )`, `GW_DEC_PROVISIONER_SUB=( "$GW_DEC_PROVISIONER_INBOX_PREFIX.>" )`
- Placeholders: `{{GW_DEC_PUBLISHER_NKEY}}`, `{{GW_DEC_READER_NKEY}}`, `{{GW_DEC_PROVISIONER_NKEY}}`, in that order after the IAM users. Each user keeps its `publish:` and `subscribe:` allow-lists on one line each (`check-subjects.sh:72-73`).

- [ ] **Step 1: Write the failing tests** (`nats_permissions.rs`; extend `Fixture` with the three identities and `start_fixture` with the three `.replace` calls).

```rust
#[tokio::test] async fn the_decision_publisher_ensures_the_stream_and_publishes_with_an_ack() {
    // get_or_create_stream(stream config of subjects.env) then send_publish("gateway.decision.<uuid>") with a msg id → PubAck
}
#[tokio::test] async fn the_decision_publisher_cannot_read_delete_or_touch_iam() {
    // subscribe gateway.decision.> → violation; publish $JS.API.STREAM.DELETE.GATEWAY_DECISIONS → violation;
    // publish iam.role.granted → violation (account isolation)
}
#[tokio::test] async fn the_decision_reader_reads_with_an_ordered_consumer() {
    // get_stream_no_info + create_consumer(OrderedConfig{filter_subject, ByStartTime}) + messages() → reads 2 published events;
    // then force a recreate (delete the consumer server-side with the provisioner) and read on
}
#[tokio::test] async fn the_decision_reader_cannot_publish_or_purge() {
    // publish gateway.decision.x → violation; $JS.API.STREAM.PURGE.GATEWAY_DECISIONS → violation; $JS.API.STREAM.CREATE.X → violation
}
#[tokio::test] async fn a_gateway_account_user_cannot_see_iam_events() {
    // the reader cannot create a consumer on IAM_EVENTS (NoResponders / stream not found in its account)
}
```

  In `check-subjects.sh`, a mismatch must red: add one entry to `GW_DEC_READER_PUB` only, run the script, and see it fail; remove the entry.

- [ ] **Step 2: Run.** `/opt/homebrew/bin/bash ops/nats/check-subjects.sh` and `( cd rs && cargo nextest run --locked -p paigasus-iam --test nats_permissions --test docker_preflight --profile iam-nats )`. Expected: FAIL before the edits.
- [ ] **Step 3: Implement** the files. `provision-decisions.sh` takes the same `OUT_DIR`, `OPERATOR`, `NATS_URL` and `PUSH` inputs as `provision.sh`; it runs `nsc add account --name "$GATEWAY_ACCOUNT"` (tolerates "already exists"), `nsc edit account --js-mem-storage 0 --js-disk-storage "$DECISIONS_ACCOUNT_DISK_BYTES" --js-streams 2 --js-consumer 16`, adds the three users with `--account "$GATEWAY_ACCOUNT"`, writes the creds with `chmod 600`, and with `PUSH=1` runs `nats stream info` and, only when absent, `nats stream add "$DECISIONS_STREAM_NAME" --subjects "$DECISIONS_SUBJECTS" --storage file --retention limits --max-age "${DECISIONS_MAX_AGE_SECS}s" --max-bytes "$DECISIONS_MAX_BYTES" --discard old --dupe-window "${DECISIONS_DUPE_WINDOW_SECS}s" --max-msg-size "$DECISIONS_MAX_MSG_SIZE" --replicas 1 --defaults` with the provisioner creds. The runbook states: the reader creds are never deployed with the gateway; the provisioner creds are an operator artifact; Redis write access equals write access to the decision log; NTP on the gateway hosts; keep old digest keys 30 days; Redis 7.0+, `noeviction`, a Redis apart from the limits store, `appendonly yes`; the sizing formula and the 13 decisions per second example; the rollout order (provision, deploy creds, set `enabled = true`).
- [ ] **Step 4: Run** both commands of Step 2, `moon run repo:nats-permissions`, and Task 11's `the_stream_config_matches_subjects_env`. Expected: PASS.
- [ ] **Step 5: Commit.** `feat(rs): add the PAIGASUS_GATEWAY NATS account and the decision-log identities (SMA-755)`

---

### Task 15: The alerts

**Files:**
- Modify: `ops/observability/prometheus/rules/gateway.rules.yml` (five alerts; `{role="limits"}` on `GatewayLimitsRedisBreakerOpen`)
- Modify: `ops/observability/prometheus/rules/tests/gateway.test.yml`
- Modify: `docs/ops/RUNBOOK-observability.md` (a "Gateway decision log" section the alerts link to)

**Interfaces:**
- Produces the alert names `GatewayDecisionLogDrops`, `GatewayDecisionLogRedisReset`, `GatewayDecisionLogRelayStalled`, `GatewayDecisionLogRedisBreakerOpen`, `GatewayDecisionLogTaskRestarts`, with the § 5.9 expressions. Relay stalled: `(sum(increase(gateway_decision_log_written_total[10m])) > 0 and sum(increase(gateway_decision_log_published_total[10m])) == 0) or sum(gateway_decision_log_backlog) > 50000` (50 % of the default `max_len`; the runbook says to change it with `stream_max_len`). Breaker: `max by (job, role) (gateway_redis_breaker_state{role="decision_log"}) != 0`, `for: 2m`.

- [ ] **Step 1: Write the promtool cases.** Per alert, one firing case (series primed at `0`, then rising) and one quiet case (`0x5`). Drops: a `reason="shutdown"` rise stays quiet. Limits breaker: a `role="decision_log"` gauge at 2 does not fire `GatewayLimitsRedisBreakerOpen`.
- [ ] **Step 2: Run.** `moon run repo:promtool repo:observability-drift`. Expected: FAIL before the rules exist.
- [ ] **Step 3: Implement** the rules and the runbook section.
- [ ] **Step 4: Run.** Expected: PASS.
- [ ] **Step 5: Commit.** `feat(repo): alert on decision-log drops, resets, stalls, breaker and restarts (SMA-755)`

---

### Task 16: The digest benchmark

**Files:**
- Modify: `rs/crates/services/paigasus-gateway/tests/perf.rs`

- [ ] **Step 1: Write the test** (`#[ignore = "a measurement in release mode"]`, as the existing perf tests).

```rust
#[test]
#[ignore = "a measurement in release mode"]
fn argument_digest_at_1_kib_1_mib_and_32_mib() {
    // DigestKey of 32 bytes; median of 20 runs for each size; println! the three numbers in ms.
    assert!(median(32 << 20) < Duration::from_millis(50), "32 MiB digest {:?}: move digests above 1 MiB to spawn_blocking (§ 5.3)", median(32 << 20));
}
```

- [ ] **Step 2: Run.** `cd rs && cargo nextest run --release -p paigasus-gateway --test perf --run-ignored only --no-capture`. Record the three numbers and the machine in your report.
- [ ] **Step 3: If the 32 MiB case is above 50 ms**, stop and report to the controller with the numbers. Do not change `decide_now` in this task: moving the digest to `spawn_blocking` makes the decider future async and is a design change that the controller decides (§ 5.3).
- [ ] **Step 4: Commit.** `test(rs): measure the argument digest cost (SMA-755)`

---

### Task 17: The full gate graph

**Files:** none, unless a gate reds.

- [ ] **Step 1: Run the full graph** as CI does, the command from the root `CLAUDE.md` `ci-targets` block:

```bash
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site \
  :http-extractor-envelope :input-liveness :promtool :observability-drift \
  :nats-permissions :release-parity :release-parity-py :release-parity-ts \
  :publish-metadata :version-lockstep :workflow-credentials :pyo3-stub-drift :ruff-ci \
  :next-public-free :helm-render :moon-diagnosis-exec :wasm-lockstep :test-e2e \
  --base origin/main \
  --include-relations
```

- [ ] **Step 2: Re-run the bash-split gates directly** (root `CLAUDE.md`, "This development Mac only"). `repo:affected-smoke` with `/bin/bash` 3.2 (if it hangs under Homebrew bash, use a bash-only shim directory, not a `/bin` prepend). `ci/ruff/run.sh`, `ci/next-public/run.sh`, `ci/publish-metadata/run.sh` and `ops/nats/check-subjects.sh` with `/opt/homebrew/bin/bash`, then the `nats-permissions` nextest half: `( cd rs && cargo nextest run --locked --no-tests=pass -p paigasus-iam --test nats_permissions --test docker_preflight --profile iam-nats )` (needs Docker). `ci/actionlint/run.sh` with `/opt/homebrew/bin/bash` only when its preflight reports a pipe capacity of at least 8192 bytes; on the 512-byte state, run it in a Linux container. Read a `mapfile`, `declare -A` or `local -n` failure as the wrong bash, not as a finding.
- [ ] **Step 3: On a red gate**, take the `moon-diagnosis` Step 0 copy before any re-run, fix the cause in the task that owns the file, and commit the fix with that task's scope.
- [ ] **Step 4: Report** each gate that you could not run locally and why. No commit unless a fix was needed.
