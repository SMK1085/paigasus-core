<!-- moon-diagnosis:ok -->
# SMA-677 PR 2: gateway rate limit and token budget — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an optional per-principal and per-org request rate limit and a per-org token budget to `POST /v1/chat/completions` in `paigasus-gateway`, with a memory store and a shared Redis store, two new registry codes, metrics, two alerts, and the SDK and console changes that the new codes need.

**Architecture:** Hexagonal. The domain (`src/domain/limits.rs`) holds every pure rule (the D3 sliding window, the D6 budget periods, the D8 precedence) and the `LimitStore` port. The application layer (`src/application/`) holds the `Limits` service, which the handler calls once, and the `ChargeGuard`, which charges a request at most once from `Drop`. Two adapters (`src/adapters/limits/{memory,redis}.rs`) implement the port; the Redis one uses `paigasus-redis` (from PR 1) and one Lua script. The clock is injected (`Clock`), so every test uses fixed instants.

**Tech Stack:** Rust 2024 (axum, tokio, tokio-util `TaskTracker`, redis-rs `Script`, chrono, metrics), testcontainers through `paigasus-test-docker` (PR 1), Prometheus rules with `promtool`, TypeScript (vitest) in `ts/packages/paigasus-sdk`, `ts/apps/gateway-console`, `ts/apps/iam-console`.

**Spec:** `docs/superpowers/specs/2026-09-27-sma-677-gateway-rate-limit-and-token-budget-design.md` (revision 5, approved by Sven on 2026-10-02). This plan covers **PR 2 only** (spec § 12). PR 1 (SMA-726, branch `feature/sma-726-redis-and-test-docker-libs`) has its own plan.

## Recommendations relied on

Sven did not answer these questions one by one. The spec header says the plan uses the § 11 recommendation for each and names it here, so that Sven can change it at plan review.

| Question | Recommendation used | Where the plan depends on it |
|---|---|---|
| Q3 | The budget is in tokens. A currency budget is a follow-up. | Task 3 (`Budget { tokens }`), Task 5 (`tokens_per_period`) |
| Q5 | Limits are off by default. No `[limits]` table means no limit (A6). | Task 5 (`limits: Option<LimitsConfig>`), Task 13 (`build_limits`) |
| Q8 | The rate refusal uses OpenAI `type: "requests"`; the budget refusal uses `type: "insufficient_quota"`. Task 6 Step 1 confirms the value against the OpenAI API reference before the code is written. | Task 6 |
| Q10 | No per-principal token cap. One member can use the whole org budget. | Task 3 (`LimitPolicy` has no principal budget) |
| Q11 | A request whose scope names no org gets `500 internal` when an org rate or a budget applies to it (fail-closed, D2). | Task 7 (`LimitRefusal::Unscoped`), Task 9 test |
| Q12 | No per-org cap on requests in flight. The D7 overshoot stays a documented soft edge. | Task 3, Task 15 (docs) |

Q14 to Q20 were approved with the spec. Q14 (boot fails when Redis is down), Q15 (`SecretString` for `redis_url`), Q16 (no Redis Cluster), Q17 (dedicated Redis recommended) and Q19 (no console switch for `stream_options`) shape Tasks 5, 11, 13, 15 and 16.

## Interfaces consumed from PR 1

PR 2 does not re-implement these. The names and signatures are as the spec states them (D15, D22, § 4.10), and were cross-checked on 2026-10-02 against `docs/superpowers/plans/2026-10-02-sma-726-redis-and-test-docker-libs.md` (its Task 1 and Task 3 Interfaces blocks). One item differs from the spec's split: the PR 1 plan defers `start_redis_image_or_skip` to PR 2, so **this plan adds it** (Task 12).

**Crate `paigasus-redis`** (`rs/crates/libs/paigasus-redis`, Moon id `paigasus-redis-rs`, workspace dependency `paigasus-redis = { path = "crates/libs/paigasus-redis", version = "0.0.0" }`):

```rust
pub struct BreakerMetrics { pub state: &'static str, pub transitions: &'static str, pub role: &'static str }   // Clone, Copy, Debug, PartialEq, Eq
pub async fn connect(redis_url: &str, metrics: impl Into<BreakerMetrics>) -> redis::RedisResult<RedisHandle>;
#[derive(Clone, Debug)] pub struct RedisHandle;            // impl redis::aio::ConnectionLike (req_packed_command + req_packed_commands, both through the breaker)
pub const BREAKER_OPEN_MESSAGE: &str;                      // the short-circuit error is ErrorKind::Io with this text
pub fn connection_manager_config() -> redis::aio::ConnectionManagerConfig;
// Under #[cfg(any(test, feature = "test-support"))], feature `test-support = []`:
pub fn new_lazy_for_tests(redis_url: &str, metrics: impl Into<BreakerMetrics>) -> redis::RedisResult<RedisHandle>;
pub fn with_open_breaker_for_tests(redis_url: &str, metrics: impl Into<BreakerMetrics>) -> redis::RedisResult<RedisHandle>;
pub mod test_support {
    pub struct Blackhole { pub url: String, /* private */ }
    impl Blackhole { pub fn start_responding(&self); pub fn accepted(&self) -> usize; }
    pub async fn start() -> Blackhole;                     // PR 1 plan, Task 1: today's `pub(crate)` items made `pub`
}
```

The breaker emits `metrics.state` (gauge, label `role`) and `metrics.transitions` (counter, labels `role`, `to` ∈ `closed|half_open|open`), and its constructor sets the gauge to 0. The `redis` dependency line in `paigasus-redis/Cargo.toml` does **not** yet carry the `script` feature; Task 11 adds it (spec § 12).

**Crate `paigasus-test-docker`** (`rs/crates/libs/paigasus-test-docker`, Moon id `paigasus-test-docker-rs`, dev-only, workspace dependency `paigasus-test-docker = { path = "crates/libs/paigasus-test-docker", version = "0.0.0" }`):

```rust
pub fn skip_docker() -> bool;
pub fn require_docker() -> bool;
pub fn is_daemon_unreachable(e: &testcontainers::TestcontainersError) -> bool;
pub async fn start_or_skip<T, I>(image: T, what: &str) -> Option<ContainerAsync<I>>
where T: Into<ContainerRequest<I>> + Send, I: Image;
pub async fn start_redis_or_skip(what: &str) -> Option<(ContainerAsync<Redis>, String)>;           // Redis::default(), returns (node, "redis://127.0.0.1:<port>")
pub async fn mapped_port(src: &impl PortSource, port: u16, what: &str) -> u16;
// NOT in PR 1 (its plan defers it): Task 12 of THIS plan adds, in paigasus-test-docker/src/lib.rs,
pub async fn start_redis_image_or_skip(tag: &str, what: &str) -> Option<(ContainerAsync<Redis>, String)>;
```

`Redis` and `ContainerAsync` are `testcontainers_modules::redis::Redis` and `testcontainers_modules::testcontainers::ContainerAsync`, version 0.15.

**Gates (PR 1):** `repo:redis-connect-single-site` already scans `services/paigasus-gateway/{src,tests}` and allows a Redis constructor only in `libs/paigasus-redis/src`. `repo:iam-docker-policy-single-site` already scans `services/paigasus-gateway/tests` and allows the Docker-skip policy only in `libs/paigasus-test-docker/src`. PR 2 adds gateway code that both gates scan; neither gate's definition changes in PR 2.

## Global Constraints

- Every new source file opens with `// SPDX-License-Identifier: Apache-2.0` (`#` for YAML, TOML, Python and shell).
- Rust crates use edition 2024 and rust-version 1.95 (inherited through `edition.workspace` and `rust-version.workspace`).
- `[workspace.lints.rust] warnings = "deny"`: dead code, an unused import and an unused variable are hard compile errors. Every task leaves the crate compiling with no warning. A gateway item that no task uses yet is `pub` (the gateway is a library crate, so a `pub` item is not dead), never `pub(crate)`. Never add `#[allow(dead_code)]` to production code.
- `repo:machete` reds on a Cargo dependency that no code uses. Add each dependency in the task whose code first uses it.
- `rustfmt.toml` sets `max_width = 200`. Run `cargo fmt` in `rs/` after every Rust edit.
- Conventional commits with a scope from `rs`, `py`, `ts`, `contracts`, `ci`, `docs`, `deps`, `release`, `repo`, `claude`, `workspace`. Do not put a `#NNN` line or a `token: value` line in a commit body (commitlint `footer-leading-blank`). End every commit message with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Never use `--no-verify`. Never use `--no-gpg-sign`: if signing fails with "failed to fill whole buffer", 1Password is locked; stop and ask.
- Never amend, reset or rebase a commit that an earlier task made. Commit only your own task's files.
- Do not install software on the host (`brew`, `npm -g`, `cargo install`). If a tool is missing, stop and report.
- Prefix every shell with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` so `moon`, `cargo`, `pnpm` and `buf` resolve to the pinned versions.
- After an edit to a `.proto` file, run `buf format -w` in `contracts/`, then `moon run contracts:generate`. Commit the regenerated Rust, Python and TypeScript bindings in the same commit.
- After any TypeScript edit, run `moon run ts:fmt` (Prettier, whole tree) and the separate typecheck targets (`paigasus-sdk-ts:typecheck`, `gateway-console-ts:typecheck`, `iam-console-ts:typecheck`). A green vitest run does not prove that `tsc` passes.
- A Prometheus counter is primed at zero (`counter!(…).increment(0)`) so `increase()` sees the first event. A test reads the NUMBER of a series and compares it; never assert with `contains()` on a `# TYPE` line or on a series name alone.
- A mutation must compile to prove anything. Run mutation checks with `cargo nextest run -p paigasus-gateway --no-fail-fast` and read the failing test names.
- Docker-gated tests (`limits_store_redis`, `limits_redis_e2e`) skip with exit 0 when Docker is unreachable, and nextest hides a passing test's stderr. To prove they ran, run them with `PAIGASUS_REQUIRE_DOCKER=1` (a skip then panics) or run the `docker_preflight` canary in the same invocation. Never record a Docker test as passed from a run that did not prove Docker was reachable.
- Hexagonal layering in the gateway: `domain` depends on no adapter, no `tokio`, no `redis`, no `metrics`, no config type. `application` depends on `domain` (and `metrics`, `tracing`). Adapters depend inward. Every time-dependent rule takes `now: SystemTime` or a `Clock`; no code under `src/domain` or `src/application` calls `SystemTime::now()` except `SystemClock`.
- `Drop` of `ChargeGuard` never calls `unwrap`, `expect` or anything that can panic (D10).
- Verify the gateway with `cargo nextest run -p paigasus-gateway --locked` from `rs/`, and with Moon targets (`moon run paigasus-gateway-rs:test`, `paigasus-gateway-rs:lint`, `paigasus-gateway-rs:fmt`).
- Before the push (Task 17), run the full gate graph from the root `CLAUDE.md` `ci-targets` block. On the development Mac no single bash runs every gate: `repo:affected-smoke` needs `/bin/bash` 3.2; `repo:ruff-ci`, `repo:next-public-free`, `repo:publish-metadata`, `repo:version-lockstep` and `repo:nats-permissions` need bash 4+; `repo:actionlint` needs bash 5 and a pipe that holds at least 8192 bytes (its preflight prints the measured value). Re-run the gates that need the other bash directly (`<bash> ci/<gate>/run.sh`) and record those results.
- Registry codes: the only `src/` file that may spell `"rate-limited"` or `"budget-exhausted"` is `src/adapters/http/error.rs` (an `emits` row in `ci/error-registry/check.py`). New `src/` files must also not spell the single-word registry codes `"internal"` or `"forbidden"`. The gate scans `rs/crates/**/src/**/*.rs` only, so `tests/` files may spell codes.
- Log lines never contain the prompt, the messages, a body, the OpenAI key or the Redis URL.

## Review Focus

The spec does not test these five inputs directly. Each one is likely to hurt a real operator or caller. Each line names the task that adds its test.

1. **A misspelt `[limits]` key** (for example `token_per_period`). A reasonable operator expects the boot to fail. With plain serde the key is ignored and the budget is silently off. Task 5 adds `#[serde(deny_unknown_fields)]` and the test `a_misspelt_limits_key_fails_extraction`.
2. **An upper-case org UUID** in `[[limits.org]] id` while the scope PRN carries the lower-case form (or the reverse). The override must still apply. Task 5 adds `an_upper_case_override_id_applies_to_the_canonical_org`.
3. **A CRLF record delimiter split across two chunks** (`\r` at the end of one chunk, `\n` at the start of the next). The scanner must count one record, not two. Task 8 adds `a_crlf_split_across_chunks_is_one_terminator`.
4. **A body whose `messages` has an unusual shape**: `content: null` with `tool_calls`, a part with no `type`, a number as `content`. The estimate must not panic and must follow D14 (null counts 0; an unknown shape uses the capped fallback). Task 8 adds `request_estimate_shapes`.
5. **A usage record with an absurd `total_tokens`** (`u64::MAX`, from a broken upstream). The charge must clamp to 10^9 (D23) and nothing may overflow. Task 7 adds `an_absurd_usage_is_clamped_to_the_charge_cap`.

---

## File structure

| File | Task | Responsibility |
|---|---|---|
| `contracts/proto/paigasus/common/v1/error.proto` | 1 | `ERROR_REASON_RATE_LIMITED = 311`, `ERROR_REASON_BUDGET_EXHAUSTED = 312` |
| `rs/crates/libs/paigasus-proto/src/generated/**`, `py/packages/paigasus-proto/**/generated/**`, `ts/packages/paigasus-proto/src/generated/**` | 1 | regenerated |
| `rs/crates/libs/paigasus-proto/src/error.rs`, `ts/packages/paigasus-proto/src/error.test.ts` | 1 | counts 65 → 67 |
| `ts/packages/paigasus-sdk/src/errors/{types,presentation,transport-status}.ts` (+ tests) | 1 | `quota-exhausted` presentation |
| `ts/apps/{gateway,iam}-console/app/_components/error-copy.ts` (+ `tests/unit/error-views.test.tsx`) | 1 | copy row |
| `rs/crates/services/paigasus-gateway/src/domain/mod.rs` (moved from `src/domain.rs`) | 2 | caller identity, `resolve_org`, `org_of` |
| `rs/crates/services/paigasus-gateway/src/domain/limits.rs` | 2, 3 | pure limit rules, the port, the clock |
| `rs/crates/libs/paigasus-observability/src/names.rs` | 4 | seven metric names |
| `rs/crates/services/paigasus-gateway/src/adapters/limits/{mod,memory}.rs` | 4 | memory store |
| `rs/crates/services/paigasus-gateway/tests/support/limits_contract.rs`, `tests/limits_store_memory.rs` | 4 | store contract suite |
| `rs/crates/services/paigasus-gateway/src/config.rs` | 5 | `[limits]` table and validation |
| `rs/crates/services/paigasus-gateway/src/adapters/http/error.rs` | 6 | two `GatewayError` variants and headers |
| `rs/crates/services/paigasus-gateway/src/application/{mod,limits,charge_guard}.rs`, `src/test_metrics.rs` | 7 | service, guard, `prime_metrics`, log limiter |
| `rs/crates/services/paigasus-gateway/src/adapters/http/usage.rs` | 8 | SSE usage scanner, D14 estimates |
| `rs/crates/services/paigasus-gateway/src/adapters/http/{mod,chat}.rs`, `tests/support/{mod,limits}.rs`, `tests/limits_http.rs` | 9, 10 | wiring and HTTP tests |
| `rs/crates/services/paigasus-gateway/src/adapters/limits/redis.rs`, `rs/crates/libs/paigasus-redis/Cargo.toml`, gateway `Cargo.toml`/`moon.yml`, `ts/apps/gateway-console/moon.yml` | 11 | Redis store |
| `rs/crates/services/paigasus-gateway/tests/{docker_preflight,limits_store_redis,limits_redis_e2e}.rs`, `rs/.config/nextest.toml` | 12 | Docker tests |
| `rs/crates/services/paigasus-gateway/src/{main,runtime}.rs`, `src/adapters/limits/mod.rs`, `tests/limits_metrics*.rs` | 13 | boot wiring, shutdown drain, metric tests |
| `ops/observability/prometheus/rules/gateway.rules.yml`, `rules/tests/gateway.test.yml` | 14 | two alerts |
| `gateway.toml.example`, `docs/ops/RUNBOOK-observability.md`, `charts/paigasus/values.yaml`, `src/lib.rs` | 15 | docs |
| `ts/apps/gateway-console/lib/chat-route.ts` (+ tests) | 16 | `include_usage` |


---

### Task 0: Stack the branch on PR 1 and verify the two libs

**Files:** none changed (a rebase only).

**Interfaces:**
- Consumes: the PR 1 branch `feature/sma-726-redis-and-test-docker-libs` (merged into `main`, or pushed).
- Produces: a branch `feature/sma-677-gateway-rate-limit-and-token-budget` whose history holds PR 1's commits, then the SMA-677 spec commit, then this plan.

- [ ] **Step 1: Check the working tree and find the PR 1 base**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-677-limits
git status --short
git fetch origin
git log --oneline -3 origin/feature/sma-726-redis-and-test-docker-libs || true
git log --oneline origin/main | grep -i "sma-726" | head -3 || true
```

Expected: the working tree holds only this plan (uncommitted or committed by the coordinator). If PR 1 is merged, `origin/main` names it; use `origin/main` as BASE. If not, use `origin/feature/sma-726-redis-and-test-docker-libs` as BASE. If neither exists, STOP and report: PR 2 cannot start before PR 1 is pushed.

- [ ] **Step 2: Rebase onto BASE**

```bash
git rebase BASE   # replace BASE with the ref from Step 1
git log --oneline -5
```

Expected: the SMA-677 spec commit (and the plan commit, if any) sit on top of PR 1's commits. If the rebase conflicts, STOP and report; do not resolve a conflict in PR 1's files.

- [ ] **Step 3: Verify the PR 1 interfaces exist**

```bash
test -f rs/crates/libs/paigasus-redis/Cargo.toml && test -f rs/crates/libs/paigasus-test-docker/Cargo.toml && echo libs-present
grep -rn "pub struct BreakerMetrics\|pub async fn connect\|pub struct RedisHandle\|pub const BREAKER_OPEN_MESSAGE\|pub fn new_lazy_for_tests\|pub fn with_open_breaker_for_tests\|pub mod test_support\|pub async fn start\b\|pub fn accepted" rs/crates/libs/paigasus-redis/src
grep -rn "pub fn skip_docker\|pub async fn start_or_skip\|pub async fn start_redis_or_skip\|pub async fn mapped_port" rs/crates/libs/paigasus-test-docker/src
grep -n "paigasus-redis\|paigasus-test-docker" rs/Cargo.toml
grep -n "^test-support\|\[features\]" rs/crates/libs/paigasus-redis/Cargo.toml
cd rs && cargo nextest run --locked --no-tests=pass -p paigasus-redis -p paigasus-test-docker
```

Expected: `libs-present`; every symbol of "Interfaces consumed from PR 1" is found, except `start_redis_image_or_skip`, which Task 12 adds; `rs/Cargo.toml` has both literal `members` lines and both workspace dependency entries; `paigasus-redis` has a `test-support` feature; the two crates' tests pass. If a name or a signature differs from this plan, STOP and report the difference to the coordinator. Do not adapt the plan silently.

- [ ] **Step 4: Record the base**

No commit. Report BASE and the `git log --oneline -5` output to the coordinator.

---

### Task 1: The two registry codes across the stack

The new enum values make the TypeScript `PRESENTATION` table (a total `Record`) fail to compile, so the proto change, the SDK presentation and both console copy tables land in ONE commit. Nothing in between is green.

**Files:**
- Modify: `contracts/proto/paigasus/common/v1/error.proto` (after `ERROR_REASON_ORG_REQUIRED = 310;`, line 240)
- Regenerate: `rs/crates/libs/paigasus-proto/src/generated/**`, `py/packages/paigasus-proto/**/generated/**`, `ts/packages/paigasus-proto/src/generated/**`
- Modify: `rs/crates/libs/paigasus-proto/src/error.rs` (`EXPECTED_REASONS` gateway block near line 210; count at line 239)
- Modify: `ts/packages/paigasus-proto/src/error.test.ts:57-60`
- Modify: `ts/packages/paigasus-sdk/src/errors/types.ts:22`, `src/errors/presentation.ts:11-12` and `:80-81`, `src/errors/transport-status.ts:14-19`
- Modify: `ts/packages/paigasus-sdk/tests/transport-status.test.ts:8`, `tests/presentation.test.ts:13`, `tests/map-error.test.ts:152-155` (+ two new cases)
- Modify: `ts/apps/gateway-console/app/_components/error-copy.ts:12`, `:19-29`; `ts/apps/iam-console/app/_components/error-copy.ts:7`, `:15-25`, `:42`
- Modify: `ts/apps/gateway-console/tests/unit/error-views.test.tsx:61`; `ts/apps/iam-console/tests/unit/error-views.test.tsx:63`

**Interfaces:**
- Consumes: nothing.
- Produces: `ErrorReason::RateLimited` (311, wire `rate-limited`) and `ErrorReason::BudgetExhausted` (312, wire `budget-exhausted`) in Rust (`paigasus_proto::paigasus::common::v1::ErrorReason`); `ErrorReason.RATE_LIMITED` and `ErrorReason.BUDGET_EXHAUSTED` in `@paigasus/proto`; the TS `Presentation` member `'quota-exhausted'`.

- [ ] **Step 1: Write the failing count tests**

In `rs/crates/libs/paigasus-proto/src/error.rs`, extend the gateway block of `EXPECTED_REASONS` (after `"org-required",`) and the count:

```rust
        "invalid-org-header",
        "org-required",
        // Gateway limits (SMA-677)
        "rate-limited",
        "budget-exhausted",
```

```rust
        assert_eq!(actual.len(), 67, "the registry should hold 67 reasons");
```

In `ts/packages/paigasus-proto/src/error.test.ts` lines 57-60:

```ts
    // Cardinality guard. The Rust mirror asserts 67 at
    // rs/crates/libs/paigasus-proto/src/error.rs:241; the two must agree,
    // because both derive from the same proto.
    expect(values).toHaveLength(67);
```

(Re-read the Rust line number after the edit with `grep -n "should hold 67" rs/crates/libs/paigasus-proto/src/error.rs` and put that number in the comment.)

- [ ] **Step 2: Run them to verify they fail**

```bash
cd rs && cargo nextest run --locked -p paigasus-proto the_registry_contains_exactly_the_expected_reasons
```

Expected: FAIL with `declared in the test but not in the registry: ["budget-exhausted", "rate-limited"]`.

- [ ] **Step 3: Add the two reasons to the proto and regenerate**

In `contracts/proto/paigasus/common/v1/error.proto`, after `ERROR_REASON_ORG_REQUIRED = 310;`:

```proto
  // "rate-limited" — the principal or its organization sent more chat requests
  // in the current 60 s window than its configured limit (429, OpenAI type
  // "requests"). Retryable: the answer carries Retry-After (SMA-677).
  ERROR_REASON_RATE_LIMITED = 311;
  // "budget-exhausted" — the organization used its token budget for the
  // current UTC period (429, OpenAI type "insufficient_quota"). Not retryable
  // until the period resets; the answer carries x-should-retry: false (SMA-677).
  ERROR_REASON_BUDGET_EXHAUSTED = 312;
```

```bash
cd contracts && buf format -w && cd ..
moon run contracts:generate
git status --short contracts rs/crates/libs/paigasus-proto py/packages/paigasus-proto ts/packages/paigasus-proto
```

Expected: the three generated trees change. If `ts/packages/paigasus-proto/src/generated/google/rpc/error_details_pb.ts` shows as deleted, the BSR rate limit hit `contracts:generate`: restore it with `git checkout -- ts/packages/paigasus-proto/src/generated/google/rpc/error_details_pb.ts` and run `moon run contracts:generate` again.

- [ ] **Step 4: Run the Rust and TS proto tests**

```bash
cd rs && cargo nextest run --locked -p paigasus-proto && cd ..
moon run paigasus-proto-ts:test
```

Expected: PASS.

- [ ] **Step 5: Add the SDK presentation**

`ts/packages/paigasus-sdk/src/errors/types.ts:22`:

```ts
export type Presentation = 'relogin' | 'forbidden' | 'not-found' | 'degraded' | 'rate-limited' | 'quota-exhausted' | 'invalid-input' | 'conflict' | 'disabled' | 'generic';
```

`ts/packages/paigasus-sdk/src/errors/presentation.ts` lines 11-12:

```ts
// The `Exclude` is load-bearing: UNSPECIFIED is the zero sentinel, the test skips it, and
// demanding an entry for it would make the table 68 keys rather than 67.
```

and after `[ErrorReason.ORG_REQUIRED]: 'from-transport',`:

```ts
  // SMA-677. The gateway's own 429s. A rate refusal keeps the transport table's `rate-limited`
  // answer ("try again soon"). A used-up budget cannot succeed for hours, so it gets its own
  // presentation, whose copy says to ask an administrator instead of to try again.
  [ErrorReason.RATE_LIMITED]: 'from-transport',
  [ErrorReason.BUDGET_EXHAUSTED]: 'quota-exhausted',
```

`ts/packages/paigasus-sdk/src/errors/transport-status.ts` lines 14-19 become:

```ts
 * `ResourceExhausted` gets its OWN `rate-limited` state rather than sharing `degraded`. A quota
 * refusal and a sick service want different copy. Since SMA-677 the gateway emits 429 itself, with
 * a registry reason: `rate-limited` keeps this table's answer, and `budget-exhausted` overrides it
 * with `quota-exhausted` (presentation.ts). An upstream 429 forwarded through the chat passthrough
 * carries no registry reason, so it still takes this table's answer.
```

- [ ] **Step 6: Update the SDK tests**

`ts/packages/paigasus-sdk/tests/transport-status.test.ts:8`:

```ts
const PRESENTATIONS: readonly Presentation[] = ['relogin', 'forbidden', 'not-found', 'degraded', 'rate-limited', 'quota-exhausted', 'invalid-input', 'conflict', 'disabled', 'generic'];
```

`ts/packages/paigasus-sdk/tests/presentation.test.ts:13`: `expect(reasons).toHaveLength(67);`

`ts/packages/paigasus-sdk/tests/map-error.test.ts` lines 152-155 (the comment inside `degrades an upstream passthrough with an underscore code`):

```ts
    // `rate-limited`, not `degraded`. This is THE case that state exists for: an upstream quota
    // refusal forwarded verbatim through the chat passthrough, which wants different copy from a
    // sick service. The gateway's OWN 429s carry a registry reason (SMA-677, cases below); this
    // fixture has none, so it takes the transport table's answer.
```

and, inside `describe("arm 3 — the gateway's OpenAI envelope", …)`, after that test:

```ts
  // SMA-677. The gateway's own budget refusal: never retryable, and its own presentation.
  it('maps budget-exhausted to quota-exhausted, not retryable', () => {
    const result = mapError({
      kind: 'http',
      status: 429,
      headers: new Headers({ 'paigasus-retryable': 'false' }),
      body: { error: { message: 'The organization token budget for 2026-10 is used up. It resets at 2026-11-01T00:00:00Z.', type: 'insufficient_quota', param: null, code: 'budget-exhausted' } },
    });
    expect(result.reason).toBe(ErrorReason.BUDGET_EXHAUSTED);
    expect(result.presentation).toBe('quota-exhausted');
    expect(result.retryable).toBe(false);
  });

  // SMA-677. The gateway's own rate refusal keeps the transport table's `rate-limited`.
  it('maps rate-limited to rate-limited, retryable', () => {
    const result = mapError({
      kind: 'http',
      status: 429,
      headers: new Headers({ 'paigasus-retryable': 'true' }),
      body: { error: { message: 'Too many requests.', type: 'requests', param: null, code: 'rate-limited' } },
    });
    expect(result.reason).toBe(ErrorReason.RATE_LIMITED);
    expect(result.presentation).toBe('rate-limited');
    expect(result.retryable).toBe(true);
  });
```

- [ ] **Step 7: Add the console copy rows**

In BOTH `ts/apps/gateway-console/app/_components/error-copy.ts` and `ts/apps/iam-console/app/_components/error-copy.ts`, in `PRESENTATION_COPY`, after the `'rate-limited'` row:

```ts
  'quota-exhausted': { title: 'Usage limit reached', body: 'Your organization used its token budget for this period. Ask an administrator, or wait until the budget resets.' },
```

In both files change "so a tenth presentation fails the type-check" to "so an eleventh presentation fails the type-check".

In `ts/apps/iam-console/app/_components/error-copy.ts:42` the line-number citation is already stale (CAPABILITY_DISABLED is not at `:74`). Replace that comment line with:

```ts
  // The SDK maps TWO reasons to the `disabled` presentation (presentation.ts: PRINCIPAL_INACTIVE and CAPABILITY_DISABLED).
```

- [ ] **Step 8: Extend the console view tests**

`ts/apps/gateway-console/tests/unit/error-views.test.tsx:61` and `ts/apps/iam-console/tests/unit/error-views.test.tsx:63`:

```tsx
  it.each(['degraded', 'generic', 'conflict', 'invalid-input', 'rate-limited', 'quota-exhausted'] as const)('renders ErrorState with the correlation id for %s', async (presentation) => {
```

and after that `it.each` block, in both files:

```tsx
  it('renders the quota-exhausted copy, which never says "try again"', async () => {
    const html = render(await PageError({ error: errorWith('quota-exhausted') }));
    expect(html).toContain('Usage limit reached');
    expect(html).toContain('Ask an administrator');
    expect(html).not.toContain('try again');
  });
```

- [ ] **Step 9: Run the TS checks**

```bash
moon run ts:fmt paigasus-sdk-ts:typecheck paigasus-sdk-ts:test gateway-console-ts:typecheck iam-console-ts:typecheck
cd ts/apps/gateway-console && pnpm exec vitest run tests/unit/error-views.test.tsx && cd ../iam-console && pnpm exec vitest run tests/unit/error-views.test.tsx && cd ../../..
```

Expected: PASS. If `ts:fmt` fails, run `cd ts && pnpm exec prettier --write <files>` on the files of this task only, then re-run.

- [ ] **Step 10: Run the gateway registry test and the breaking check**

```bash
cd rs && cargo nextest run --locked -p paigasus-gateway every_gateway_code_is_declared_in_the_canonical_registry && cd ..
moon run contracts:breaking
```

Expected: PASS (no gateway code uses the new reasons yet; an added enum value is not a breaking change). If `contracts:breaking` fails inside a worktree with a gitlink error, run it as memory `buf-breaking-worktree-gitlink` says: `--against "$(git rev-parse --git-common-dir)#branch=main"`.

- [ ] **Step 11: Commit**

```bash
git add contracts/proto/paigasus/common/v1/error.proto rs/crates/libs/paigasus-proto py/packages/paigasus-proto ts/packages/paigasus-proto ts/packages/paigasus-sdk ts/apps/gateway-console/app/_components/error-copy.ts ts/apps/iam-console/app/_components/error-copy.ts ts/apps/gateway-console/tests/unit/error-views.test.tsx ts/apps/iam-console/tests/unit/error-views.test.tsx
git commit -m "feat(contracts): add the rate-limited and budget-exhausted gateway reasons

Two gateway registry reasons for SMA-677, regenerated for Rust, Python
and TypeScript. The SDK gains the quota-exhausted presentation, and both
console copy tables gain its row.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: Domain module and the D3 rate-window arithmetic

**Files:**
- Move: `rs/crates/services/paigasus-gateway/src/domain.rs` → `src/domain/mod.rs` (`git mv`)
- Modify: `src/domain/mod.rs` (`org_of` and `parse_org_uuid` become `pub(crate)`; `pub mod limits;`)
- Create: `src/domain/limits.rs`

**Interfaces:**
- Consumes: nothing new.
- Produces (all in `paigasus_gateway::domain::limits`, all `pub`):
  - `const RATE_WINDOW_MS: u64 = 60_000`, `RATE_KEY_TTL_SECS: u32 = 180`, `MAX_REQUESTS_PER_MINUTE: u64 = 1_000_000_000`, `MAX_TOKENS_PER_PERIOD: u64 = 1_000_000_000_000`, `MAX_TOKENS_PER_CHARGE: u64 = 1_000_000_000`
  - `struct WindowIndex { pub index: u64, pub elapsed_ms: u64 }` with `fn at(now: SystemTime) -> WindowIndex`
  - `fn admits(previous: u64, current: u64, elapsed_ms: u64, limit: u64) -> bool`
  - `fn retry_after(previous: u64, current: u64, elapsed_ms: u64, limit: u64) -> u32` (seconds, at least 1)
  - `struct RateCounts { pub previous: u64, pub current: u64, pub elapsed_ms: u64, pub limit: u64 }` (`Copy`, `Eq`) with `fn admits(&self) -> bool`, `fn retry_after_secs(&self) -> u32`
  - `struct SlidingWindow` (`Copy`, `Default`, `Eq`) with `fn counts(&self, now: SystemTime, limit: NonZeroU64) -> RateCounts`, `fn check(&self, now: SystemTime, limit: NonZeroU64) -> Result<(), RateCounts>`, `fn commit(&mut self, now: SystemTime)`
  - `pub(crate) fn org_of(raw: &str) -> Option<Uuid>` and `pub(crate) fn parse_org_uuid(value: &str) -> Option<Uuid>` in `crate::domain`

Deviation from the spec's wording, on purpose: spec D4 writes `check(now, limit) -> Result<(), RetryAfterSecs>`. Here `check` returns the counts (`RateCounts`), because the refusal needs the counts for the D8 `Retry-After` maximum and the Redis adapter builds the same `RateCounts` from the script reply. `RateCounts::retry_after_secs()` gives the spec's value.

- [ ] **Step 1: Move the module**

```bash
cd rs/crates/services/paigasus-gateway
mkdir -p src/domain
git mv src/domain.rs src/domain/mod.rs
```

In `src/domain/mod.rs`, change `fn parse_org_uuid(` to `pub(crate) fn parse_org_uuid(` and `fn org_of(` to `pub(crate) fn org_of(`, and add after the module doc comment:

```rust
pub mod limits;
```

- [ ] **Step 2: Write the failing tests**

Create `src/domain/limits.rs` with only the doc, the imports and the tests:

```rust
// SPDX-License-Identifier: Apache-2.0

//! SMA-677: the pure rules of the gateway's rate limit and token budget, and the store port.
//!
//! No dependency on `redis`, `tokio`, `metrics` or the config types (spec § 4.2). Both store
//! adapters compute every decision with the functions in this file, so they agree to the request
//! (D3). No function here reads a clock: every instant is a parameter.

use std::num::NonZeroU64;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-10-02T12:00:00Z, which is a window start (1_790_942_400 is a multiple of 60).
    const T0_MS: u64 = 1_790_942_400_000;

    fn at_ms(ms: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_millis(ms)
    }

    fn nz(n: u64) -> NonZeroU64 {
        NonZeroU64::new(n).expect("a non-zero test limit")
    }

    #[test]
    fn the_window_index_splits_unix_milliseconds() {
        assert_eq!(WindowIndex::at(at_ms(T0_MS)), WindowIndex { index: T0_MS / 60_000, elapsed_ms: 0 });
        assert_eq!(WindowIndex::at(at_ms(T0_MS + 59_999)), WindowIndex { index: T0_MS / 60_000, elapsed_ms: 59_999 });
        assert_eq!(WindowIndex::at(at_ms(T0_MS + 60_000)), WindowIndex { index: T0_MS / 60_000 + 1, elapsed_ms: 0 });
        // A clock before the epoch reads as the epoch, not a panic.
        assert_eq!(WindowIndex::at(UNIX_EPOCH - Duration::from_secs(5)), WindowIndex { index: 0, elapsed_ms: 0 });
    }

    /// (previous, current, elapsed_ms, limit, admitted) — the D3 integer form, row by row.
    const ROWS: &[(u64, u64, u64, u64, bool)] = &[
        (0, 0, 0, 1, true),                // the first request ever
        (0, 1, 0, 1, false),               // a limit of 1, second request
        (1, 0, 0, 1, false),               // window edge: the previous count weighs fully at 0 ms
        (1, 0, 59_999, 1, false),          // and still weighs 1/60000 at the last millisecond
        (2, 0, 30_000, 2, true),           // 2 × 30000 + 60000 = 120000 ≤ 120000
        (2, 0, 29_999, 2, false),          // one millisecond earlier it is over
        (0, 2, 59_999, 2, false),          // the current count alone is at the limit
        (10, 4, 30_000, 10, true),         // 300000 + 240000 + 60000 = 600000
        (10, 5, 30_000, 10, false),
        (0, MAX_REQUESTS_PER_MINUTE - 1, 0, MAX_REQUESTS_PER_MINUTE, true), // the D23 maximum
        (0, MAX_REQUESTS_PER_MINUTE, 0, MAX_REQUESTS_PER_MINUTE, false),
        (u64::MAX, u64::MAX, 0, MAX_REQUESTS_PER_MINUTE, false),            // saturates, no panic
    ];

    #[test]
    fn admits_follows_the_d3_integer_form() {
        for &(previous, current, elapsed, limit, want) in ROWS {
            assert_eq!(admits(previous, current, elapsed, limit), want, "row {:?}", (previous, current, elapsed, limit));
        }
    }

    /// Admission `wait_ms` after the row's instant, assuming no other request arrives: the counts
    /// shift by one window per 60 s, exactly as `SlidingWindow` does.
    fn admitted_after(previous: u64, current: u64, elapsed: u64, limit: u64, wait_ms: u64) -> bool {
        let t = elapsed + wait_ms;
        match t / RATE_WINDOW_MS {
            0 => admits(previous, current, t, limit),
            1 => admits(current, 0, t - RATE_WINDOW_MS, limit),
            _ => admits(0, 0, t % RATE_WINDOW_MS, limit),
        }
    }

    #[test]
    fn retry_after_is_the_first_whole_second_that_admits() {
        for &(previous, current, elapsed, limit, admitted) in ROWS {
            if admitted || previous == u64::MAX {
                continue;
            }
            let wait = retry_after(previous, current, elapsed, limit);
            assert!(wait >= 1, "Retry-After is at least 1 s");
            assert!(admitted_after(previous, current, elapsed, limit, u64::from(wait) * 1000), "admitted after {wait} s: {:?}", (previous, current, elapsed, limit));
            if wait > 1 {
                assert!(!admitted_after(previous, current, elapsed, limit, u64::from(wait - 1) * 1000), "not admitted one second earlier: {:?}", (previous, current, elapsed, limit));
            }
        }
    }

    #[test]
    fn retry_after_is_one_second_even_for_an_admitted_request() {
        assert_eq!(retry_after(0, 0, 0, 1), 1);
    }

    #[test]
    fn a_limit_of_one_waits_for_the_window_after_next_when_both_windows_are_full() {
        // previous 0, current 1 at 0 ms, limit 1: the next window still weighs the 1 fully at
        // 0 ms and by 1/60000 at its end, so only the window after next admits: 120 s.
        assert_eq!(retry_after(0, 1, 0, 1), 120);
    }

    #[test]
    fn the_first_request_ever_is_admitted_and_counted() {
        let mut w = SlidingWindow::default();
        assert_eq!(w.check(at_ms(T0_MS), nz(1)), Ok(()));
        w.commit(at_ms(T0_MS));
        assert_eq!(w.counts(at_ms(T0_MS + 1), nz(1)), RateCounts { previous: 0, current: 1, elapsed_ms: 1, limit: 1 });
    }

    #[test]
    fn a_failed_check_does_not_change_the_state() {
        let mut w = SlidingWindow::default();
        w.commit(at_ms(T0_MS));
        let before = w;
        let refused = w.check(at_ms(T0_MS + 10), nz(1)).expect_err("a second request over a limit of 1");
        assert_eq!(refused, RateCounts { previous: 0, current: 1, elapsed_ms: 10, limit: 1 });
        assert_eq!(w, before, "check is pure (D4)");
    }

    #[test]
    fn the_next_window_reads_the_old_current_count_as_previous() {
        let mut w = SlidingWindow::default();
        for _ in 0..3 {
            w.commit(at_ms(T0_MS + 5_000));
        }
        assert_eq!(w.counts(at_ms(T0_MS + 60_000 + 15_000), nz(10)), RateCounts { previous: 3, current: 0, elapsed_ms: 15_000, limit: 10 });
        w.commit(at_ms(T0_MS + 60_000 + 15_000));
        assert_eq!(w.counts(at_ms(T0_MS + 60_000 + 16_000), nz(10)), RateCounts { previous: 3, current: 1, elapsed_ms: 16_000, limit: 10 });
    }

    #[test]
    fn a_gap_of_more_than_two_windows_reads_zero() {
        let mut w = SlidingWindow::default();
        w.commit(at_ms(T0_MS));
        assert_eq!(w.counts(at_ms(T0_MS + 2 * 60_000), nz(5)), RateCounts { previous: 0, current: 0, elapsed_ms: 0, limit: 5 });
        w.commit(at_ms(T0_MS + 2 * 60_000));
        assert_eq!(w.counts(at_ms(T0_MS + 2 * 60_000), nz(5)), RateCounts { previous: 0, current: 1, elapsed_ms: 0, limit: 5 });
    }

    #[test]
    fn a_backward_clock_step_clamps_the_elapsed_time_to_zero() {
        let mut w = SlidingWindow::default();
        w.commit(at_ms(T0_MS + 30_000));
        // Five minutes back: an earlier window. Read as the stored window at 0 ms; no panic.
        assert_eq!(w.counts(at_ms(T0_MS - 300_000), nz(5)), RateCounts { previous: 0, current: 1, elapsed_ms: 0, limit: 5 });
        w.commit(at_ms(T0_MS - 300_000));
        assert_eq!(w.counts(at_ms(T0_MS + 30_000), nz(5)), RateCounts { previous: 0, current: 2, elapsed_ms: 30_000, limit: 5 });
    }

    #[test]
    fn saturated_counts_never_panic() {
        let mut w = SlidingWindow { window: T0_MS / 60_000, previous: u64::MAX, current: u64::MAX };
        w.commit(at_ms(T0_MS));
        assert!(w.check(at_ms(T0_MS), nz(MAX_REQUESTS_PER_MINUTE)).is_err());
        assert!(!admits(u64::MAX, u64::MAX, 0, u64::MAX));
    }

    #[test]
    fn check_agrees_with_the_pure_functions_on_every_row() {
        for &(previous, current, elapsed, limit, want) in ROWS {
            if limit == 0 || previous == u64::MAX {
                continue;
            }
            let w = SlidingWindow { window: T0_MS / 60_000, previous, current };
            let got = w.check(at_ms(T0_MS + elapsed), nz(limit));
            assert_eq!(got.is_ok(), want, "row {:?}", (previous, current, elapsed, limit));
            if let Err(counts) = got {
                assert_eq!(counts.retry_after_secs(), retry_after(previous, current, elapsed, limit));
            }
        }
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail**

```bash
cd rs && cargo nextest run --locked -p paigasus-gateway domain::limits
```

Expected: compile FAIL (`cannot find type WindowIndex`, `cannot find function admits`, …).

- [ ] **Step 4: Write the implementation**

Insert above `#[cfg(test)]` in `src/domain/limits.rs`:

```rust
/// The D3 rate window, in milliseconds.
pub const RATE_WINDOW_MS: u64 = 60_000;
/// D18: a rate key lives 180 s after its last increment: two windows plus one window of margin
/// for clock skew between replicas.
pub const RATE_KEY_TTL_SECS: u32 = 180;
/// D23: the largest `principal_requests_per_minute` and `org_requests_per_minute`.
pub const MAX_REQUESTS_PER_MINUTE: u64 = 1_000_000_000;
/// D23: the largest `tokens_per_period`.
pub const MAX_TOKENS_PER_PERIOD: u64 = 1_000_000_000_000;
/// D23: one charge is clamped to this many tokens. A larger usage record is a bad record.
pub const MAX_TOKENS_PER_CHARGE: u64 = 1_000_000_000;

/// Milliseconds since the epoch. A clock before the epoch reads as 0, never a panic (D10).
fn unix_ms(now: SystemTime) -> u64 {
    let ms = now.duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO).as_millis();
    u64::try_from(ms).unwrap_or(u64::MAX)
}

/// Where `now` falls in the D3 windows: `index = floor(unix_ms / 60000)`, and the milliseconds
/// elapsed in that window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowIndex {
    pub index: u64,
    pub elapsed_ms: u64,
}

impl WindowIndex {
    pub fn at(now: SystemTime) -> Self {
        let ms = unix_ms(now);
        WindowIndex { index: ms / RATE_WINDOW_MS, elapsed_ms: ms % RATE_WINDOW_MS }
    }
}

/// D3: `previous × (60000 − elapsed_ms) + current × 60000 + 60000 ≤ limit × 60000`. The Lua
/// script (Task 11) evaluates the same form. Saturating, so no input can panic (D10).
pub fn admits(previous: u64, current: u64, elapsed_ms: u64, limit: u64) -> bool {
    let elapsed = elapsed_ms.min(RATE_WINDOW_MS - 1);
    let weighted = previous
        .saturating_mul(RATE_WINDOW_MS - elapsed)
        .saturating_add(current.saturating_mul(RATE_WINDOW_MS))
        .saturating_add(RATE_WINDOW_MS);
    weighted <= limit.saturating_mul(RATE_WINDOW_MS)
}

/// Milliseconds until `admits` holds, if no other request arrives in between.
fn retry_after_ms(previous: u64, current: u64, elapsed_ms: u64, limit: u64) -> u64 {
    let elapsed = elapsed_ms.min(RATE_WINDOW_MS - 1);
    if admits(previous, current, elapsed, limit) {
        return 0;
    }
    // Same window: the previous count's weight falls as `elapsed` grows. With
    // budget = (limit − current − 1) × 60000, admission needs elapsed ≥ 60000 − ⌊budget / previous⌋.
    if previous > 0 && current < limit {
        let budget = (limit - current - 1).saturating_mul(RATE_WINDOW_MS);
        let needed = RATE_WINDOW_MS.saturating_sub(budget / previous);
        if needed < RATE_WINDOW_MS {
            return needed.saturating_sub(elapsed);
        }
    }
    // Next window: `current` becomes the previous count and the new current count is 0. A needed
    // offset of a full window means the window after next, where both counts read 0.
    let to_next = RATE_WINDOW_MS - elapsed;
    if current == 0 {
        return to_next;
    }
    let budget = limit.saturating_sub(1).saturating_mul(RATE_WINDOW_MS);
    to_next.saturating_add(RATE_WINDOW_MS.saturating_sub(budget / current))
}

/// The `Retry-After` for a refused rate check, in whole seconds, at least 1 (D8).
pub fn retry_after(previous: u64, current: u64, elapsed_ms: u64, limit: u64) -> u32 {
    let ms = retry_after_ms(previous, current, elapsed_ms, limit);
    u32::try_from(ms.div_ceil(1000).max(1)).unwrap_or(u32::MAX)
}

/// The counts one rate check saw. A refusal carries them, so `Retry-After` and the D8 precedence
/// are computed in Rust for both adapters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateCounts {
    pub previous: u64,
    pub current: u64,
    pub elapsed_ms: u64,
    pub limit: u64,
}

impl RateCounts {
    pub fn admits(&self) -> bool {
        admits(self.previous, self.current, self.elapsed_ms, self.limit)
    }

    pub fn retry_after_secs(&self) -> u32 {
        retry_after(self.previous, self.current, self.elapsed_ms, self.limit)
    }
}

/// The D3 state of one key: the counts of the last window that saw a request and of the window
/// before it. Two integers, O(1) memory per key.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SlidingWindow {
    window: u64,
    previous: u64,
    current: u64,
}

impl SlidingWindow {
    /// The counts as seen at `now`. A backward clock step (an earlier window) reads the stored
    /// window at 0 ms elapsed (D10).
    pub fn counts(&self, now: SystemTime, limit: NonZeroU64) -> RateCounts {
        let at = WindowIndex::at(now);
        let (previous, current, elapsed_ms) = if at.index == self.window {
            (self.previous, self.current, at.elapsed_ms)
        } else if at.index == self.window.saturating_add(1) {
            (self.current, 0, at.elapsed_ms)
        } else if at.index > self.window {
            (0, 0, at.elapsed_ms)
        } else {
            (self.previous, self.current, 0)
        };
        RateCounts { previous, current, elapsed_ms, limit: limit.get() }
    }

    /// D4: pure. `Err` carries the counts that refused the request.
    pub fn check(&self, now: SystemTime, limit: NonZeroU64) -> Result<(), RateCounts> {
        let counts = self.counts(now, limit);
        if counts.admits() { Ok(()) } else { Err(counts) }
    }

    /// D4: count one admitted request at `now`. Never moves the window backward.
    pub fn commit(&mut self, now: SystemTime) {
        let at = WindowIndex::at(now);
        if at.index > self.window {
            self.previous = if at.index == self.window.saturating_add(1) { self.current } else { 0 };
            self.current = 0;
            self.window = at.index;
        }
        self.current = self.current.saturating_add(1);
    }
}
```

- [ ] **Step 5: Run the tests to verify they pass**

```bash
cd rs && cargo fmt && cargo nextest run --locked -p paigasus-gateway domain:: && cargo clippy --locked -p paigasus-gateway --all-targets -- -D warnings
```

Expected: PASS, including the existing `resolve_org_table` and `parse_org_uuid_matches_kernel_uuid_rule` (spec § 5.1 "org_of" row).

- [ ] **Step 6: Commit**

```bash
git add rs/crates/services/paigasus-gateway/src/domain
git commit -m "feat(rs): add the gateway rate-window arithmetic

The D3 sliding-window counter of SMA-677 as pure domain functions, with
the integer form that both limit stores share. src/domain.rs moves to
src/domain/mod.rs.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: Domain budget periods, policy, decisions and the store port

**Files:**
- Modify: `rs/crates/services/paigasus-gateway/Cargo.toml` (`chrono = { workspace = true }` under `[dependencies]`)
- Modify: `rs/crates/services/paigasus-gateway/src/domain/limits.rs`

**Interfaces:**
- Consumes: Task 2's `RateCounts`, `RATE_WINDOW_MS`.
- Produces (all `pub` in `paigasus_gateway::domain::limits`):
  - `const LATE_CHARGE_GRACE_SECS: i64 = 86_400`
  - `enum BudgetPeriod { Daily, Weekly, Monthly }` — `Copy + Default` (default `Monthly`), `Serialize + Deserialize` with `rename_all = "lowercase"`; `fn key_at(self, now: SystemTime) -> PeriodKey`; `fn charge_ttl(key: PeriodKey, now: SystemTime) -> Option<u32>`; `fn label_at_reset(self, resets_at_unix: i64) -> String`
  - `enum PeriodKey { Day(NaiveDate), Week { year: i32, week: u32 }, Month { year: i32, month: u32 } }` — `Copy + Hash + Eq`; `fn start(self) -> DateTime<Utc>`, `fn end(self) -> DateTime<Utc>`, `fn resets_at_unix(self) -> i64`, `fn label(self) -> String` (`2026-10-02`, `2026-W40`, `2026-10`)
  - `struct Budget { pub tokens: NonZeroU64, pub period: BudgetPeriod }`
  - `struct LimitPolicy { pub principal_requests_per_minute: Option<NonZeroU64>, pub org_requests_per_minute: Option<NonZeroU64>, pub budget: Option<Budget> }` — `Copy + Default`; `fn is_empty(&self) -> bool`, `fn has_org_dimension(&self) -> bool`
  - `struct OrgOverride { pub org_requests_per_minute: Option<NonZeroU64>, pub tokens_per_period: Option<NonZeroU64>, pub exempt: bool }`
  - `struct LimitRules { pub principal_requests_per_minute, pub org_requests_per_minute, pub tokens_per_period: Option<NonZeroU64>, pub budget_period: BudgetPeriod, pub overrides: HashMap<Uuid, OrgOverride> }` — `fn policy_for(&self, org: Option<Uuid>) -> LimitPolicy`, `fn every_policy_is_empty(&self) -> bool`
  - `enum FailedCheck { PrincipalRate(RateCounts), OrgRate(RateCounts), Budget { period: BudgetPeriod, resets_at_unix: i64 } }`; `fn budget_refusal(budget: Budget, key: PeriodKey) -> FailedCheck`
  - `struct LimitTicket { pub org: Uuid, pub period: PeriodKey }` — NOT `Clone` (given back exactly once)
  - `enum LimitDecision { Admit(Option<LimitTicket>), Refused(Vec<FailedCheck>) }`
  - `enum RefusalReason { PrincipalRate, OrgRate, OrgBudget }` with `const ALL: [RefusalReason; 3]`, `fn as_label(self) -> &'static str` (`principal_rate`, `org_rate`, `org_budget`)
  - `enum LimitRefusal { RateLimited { retry_after_secs: u32, reason: RefusalReason }, BudgetExhausted { period: BudgetPeriod, resets_at_unix: i64 }, Unscoped }` with `fn reason(&self) -> Option<RefusalReason>`, `fn label(&self) -> &'static str`
  - `fn choose_refusal(failed: &[FailedCheck]) -> Option<LimitRefusal>` (D8 precedence)
  - `enum ChargeDropReason { NoRuntime, Shutdown, PeriodExpired }` with `const ALL`, `fn as_label(self)` (`no_runtime`, `shutdown`, `period_expired`)
  - `enum UnavailableKind { Io, Server, Decode }` with `const ALL`, `fn as_label(self)` (`io`, `server`, `decode`)
  - `enum LimitStoreError { Unavailable { kind: UnavailableKind, detail: String } }` (`thiserror::Error`)
  - `#[async_trait] trait LimitStore: Send + Sync { async fn check_and_admit(&self, principal: &str, org: Option<Uuid>, policy: &LimitPolicy, now: SystemTime) -> Result<LimitDecision, LimitStoreError>; fn charge(&self, ticket: LimitTicket, tokens: u64, now: SystemTime); }`
  - `trait Clock: Send + Sync { fn now(&self) -> SystemTime; }`, `struct SystemClock` (`Copy + Default`)

Two documented choices where the spec is silent or short:
- `LimitStoreError::Unavailable` carries `detail: String` beside `kind`. Spec D10 shows `Unavailable { kind }` only, but also says the log line "carries the kind and the error text". The adapter fills `detail` with the error's `Display`, which never holds the URL.
- A `[[limits.org]]` entry that sets only one of `org_requests_per_minute` and `tokens_per_period` keeps the table default for the other (D12 says an unlisted org gets the defaults; it does not say what a half-filled entry means). `exempt = true` removes both.

- [ ] **Step 1: Add `chrono`**

In `rs/crates/services/paigasus-gateway/Cargo.toml`, after the `uuid` line in `[dependencies]`:

```toml
# SMA-677 D6: the UTC budget periods (`iso_week()` for the ISO week-year) and the RFC 3339 reset
# instant in the budget refusal message. Already a workspace dependency.
chrono = { workspace = true }
```

- [ ] **Step 2: Write the failing tests**

Add to the `tests` module of `src/domain/limits.rs`:

```rust
    // `HashMap` and `Uuid` come from `super::*` (the parent's own imports).

    fn instant(rfc3339: &str) -> SystemTime {
        SystemTime::from(chrono::DateTime::parse_from_rfc3339(rfc3339).expect("a fixture instant"))
    }

    fn unix(rfc3339: &str) -> i64 {
        chrono::DateTime::parse_from_rfc3339(rfc3339).expect("a fixture instant").timestamp()
    }

    /// (period, instant, label, start, end) — spec § 5.1.
    #[test]
    fn period_keys_and_reset_instants() {
        let rows: &[(BudgetPeriod, &str, &str, &str, &str)] = &[
            (BudgetPeriod::Daily, "2026-10-02T23:59:59.999Z", "2026-10-02", "2026-10-02T00:00:00Z", "2026-10-03T00:00:00Z"),
            (BudgetPeriod::Daily, "2026-10-03T00:00:00Z", "2026-10-03", "2026-10-03T00:00:00Z", "2026-10-04T00:00:00Z"),
            (BudgetPeriod::Daily, "2028-02-29T12:00:00Z", "2028-02-29", "2028-02-29T00:00:00Z", "2028-03-01T00:00:00Z"),
            (BudgetPeriod::Weekly, "2026-10-04T23:59:59Z", "2026-W40", "2026-09-28T00:00:00Z", "2026-10-05T00:00:00Z"),
            (BudgetPeriod::Weekly, "2026-10-05T00:00:00Z", "2026-W41", "2026-10-05T00:00:00Z", "2026-10-12T00:00:00Z"),
            (BudgetPeriod::Weekly, "2026-12-31T12:00:00Z", "2026-W53", "2026-12-28T00:00:00Z", "2027-01-04T00:00:00Z"),
            (BudgetPeriod::Weekly, "2027-01-01T00:00:00Z", "2026-W53", "2026-12-28T00:00:00Z", "2027-01-04T00:00:00Z"),
            (BudgetPeriod::Monthly, "2026-10-31T23:59:59.999Z", "2026-10", "2026-10-01T00:00:00Z", "2026-11-01T00:00:00Z"),
            (BudgetPeriod::Monthly, "2026-12-31T23:59:59Z", "2026-12", "2026-12-01T00:00:00Z", "2027-01-01T00:00:00Z"),
            (BudgetPeriod::Monthly, "2027-01-01T00:00:00Z", "2027-01", "2027-01-01T00:00:00Z", "2027-02-01T00:00:00Z"),
            (BudgetPeriod::Monthly, "2028-02-29T00:00:00Z", "2028-02", "2028-02-01T00:00:00Z", "2028-03-01T00:00:00Z"),
        ];
        for &(period, now, label, start, end) in rows {
            let key = period.key_at(instant(now));
            assert_eq!(key.label(), label, "{period:?} at {now}");
            assert_eq!(key.start().timestamp(), unix(start), "{period:?} at {now}: start");
            assert_eq!(key.end().timestamp(), unix(end), "{period:?} at {now}: end");
            assert_eq!(key.resets_at_unix(), unix(end));
        }
    }

    #[test]
    fn the_default_period_is_monthly() {
        assert_eq!(BudgetPeriod::default(), BudgetPeriod::Monthly);
    }

    #[test]
    fn the_refusal_label_is_the_period_one_second_before_the_reset() {
        assert_eq!(BudgetPeriod::Monthly.label_at_reset(unix("2026-11-01T00:00:00Z")), "2026-10");
        assert_eq!(BudgetPeriod::Weekly.label_at_reset(unix("2026-10-05T00:00:00Z")), "2026-W40");
        assert_eq!(BudgetPeriod::Daily.label_at_reset(unix("2026-10-03T00:00:00Z")), "2026-10-02");
    }

    /// Spec § 5.1 `charge_ttl` rows. October 2026 has 31 days = 2_678_400 s.
    #[test]
    fn the_charge_ttl_is_relative_to_now() {
        let october = BudgetPeriod::Monthly.key_at(instant("2026-10-15T00:00:00Z"));
        assert_eq!(BudgetPeriod::charge_ttl(october, instant("2026-10-01T00:00:01Z")), Some(2_678_400 + 86_400 - 1));
        assert_eq!(BudgetPeriod::charge_ttl(october, instant("2026-11-01T00:00:00Z")), Some(86_400));
        assert_eq!(BudgetPeriod::charge_ttl(october, instant("2026-11-01T23:59:59Z")), Some(1));
        assert_eq!(BudgetPeriod::charge_ttl(october, instant("2026-11-02T00:00:00Z")), None);
    }

    const ORG_A: Uuid = Uuid::from_u128(0x0190a100_0000_7000_8000_0000000000a1);
    const ORG_B: Uuid = Uuid::from_u128(0x0190a100_0000_7000_8000_0000000000b2);
    const ORG_C: Uuid = Uuid::from_u128(0x0190a100_0000_7000_8000_0000000000c3);

    fn rules() -> LimitRules {
        let mut overrides = HashMap::new();
        overrides.insert(ORG_A, OrgOverride { org_requests_per_minute: Some(nz(1200)), tokens_per_period: None, exempt: false });
        overrides.insert(ORG_B, OrgOverride { exempt: true, ..OrgOverride::default() });
        LimitRules {
            principal_requests_per_minute: Some(nz(60)),
            org_requests_per_minute: Some(nz(600)),
            tokens_per_period: Some(nz(5_000_000)),
            budget_period: BudgetPeriod::Weekly,
            overrides,
        }
    }

    #[test]
    fn policy_for_applies_overrides_exemptions_and_defaults() {
        let budget = Some(Budget { tokens: nz(5_000_000), period: BudgetPeriod::Weekly });
        // An override that sets only the org rate keeps the default budget.
        assert_eq!(rules().policy_for(Some(ORG_A)), LimitPolicy { principal_requests_per_minute: Some(nz(60)), org_requests_per_minute: Some(nz(1200)), budget });
        // An exempt org keeps only the principal rate (D11).
        assert_eq!(rules().policy_for(Some(ORG_B)), LimitPolicy { principal_requests_per_minute: Some(nz(60)), org_requests_per_minute: None, budget: None });
        // An unlisted org and a request with no org get the table defaults.
        let defaults = LimitPolicy { principal_requests_per_minute: Some(nz(60)), org_requests_per_minute: Some(nz(600)), budget };
        assert_eq!(rules().policy_for(Some(ORG_C)), defaults);
        assert_eq!(rules().policy_for(None), defaults);
    }

    #[test]
    fn is_empty_and_every_policy_is_empty() {
        assert!(LimitPolicy::default().is_empty());
        assert!(!LimitPolicy::default().has_org_dimension());
        let exempt_only = LimitRules { overrides: HashMap::from([(ORG_B, OrgOverride { exempt: true, ..OrgOverride::default() })]), ..LimitRules::default() };
        assert!(exempt_only.policy_for(Some(ORG_B)).is_empty(), "an exempt org with no principal rate gives an empty policy");
        assert!(exempt_only.every_policy_is_empty());
        let one_override = LimitRules { overrides: HashMap::from([(ORG_A, OrgOverride { tokens_per_period: Some(nz(10)), ..OrgOverride::default() })]), ..LimitRules::default() };
        assert!(!one_override.every_policy_is_empty(), "an override with a value can give a non-empty policy");
        assert!(one_override.policy_for(None).is_empty());
        assert!(!rules().every_policy_is_empty());
    }

    fn rate(previous: u64, current: u64, elapsed_ms: u64, limit: u64) -> RateCounts {
        RateCounts { previous, current, elapsed_ms, limit }
    }

    /// D8 precedence: budget, then principal rate, then org rate; Retry-After = max, at least 1.
    #[test]
    fn the_refusal_follows_the_d8_precedence() {
        let budget = FailedCheck::Budget { period: BudgetPeriod::Monthly, resets_at_unix: unix("2026-11-01T00:00:00Z") };
        let principal = FailedCheck::PrincipalRate(rate(0, 2, 59_999, 2)); // 31 s
        let org = FailedCheck::OrgRate(rate(10, 5, 30_000, 10)); // 6 s
        assert_eq!(choose_refusal(&[org, principal, budget]), Some(LimitRefusal::BudgetExhausted { period: BudgetPeriod::Monthly, resets_at_unix: unix("2026-11-01T00:00:00Z") }));
        assert_eq!(choose_refusal(&[org, principal]), Some(LimitRefusal::RateLimited { retry_after_secs: 31, reason: RefusalReason::PrincipalRate }));
        assert_eq!(choose_refusal(&[org]), Some(LimitRefusal::RateLimited { retry_after_secs: 6, reason: RefusalReason::OrgRate }));
        assert_eq!(choose_refusal(&[]), None);
        // The minimum is 1 s even when the counts would admit at once.
        assert_eq!(choose_refusal(&[FailedCheck::OrgRate(rate(0, 0, 0, 1))]), Some(LimitRefusal::RateLimited { retry_after_secs: 1, reason: RefusalReason::OrgRate }));
    }

    #[test]
    fn labels_are_the_bounded_metric_values() {
        assert_eq!(RefusalReason::ALL.map(RefusalReason::as_label), ["principal_rate", "org_rate", "org_budget"]);
        assert_eq!(UnavailableKind::ALL.map(UnavailableKind::as_label), ["io", "server", "decode"]);
        assert_eq!(ChargeDropReason::ALL.map(ChargeDropReason::as_label), ["no_runtime", "shutdown", "period_expired"]);
        assert_eq!(LimitRefusal::Unscoped.reason(), None);
        assert_eq!(LimitRefusal::Unscoped.label(), "unscoped");
        assert_eq!(budget_refusal(Budget { tokens: nz(1), period: BudgetPeriod::Daily }, BudgetPeriod::Daily.key_at(instant("2026-10-02T12:00:00Z"))), FailedCheck::Budget { period: BudgetPeriod::Daily, resets_at_unix: unix("2026-10-03T00:00:00Z") });
    }

    #[test]
    fn the_period_serde_spelling_is_lowercase() {
        assert_eq!(serde_json::to_string(&BudgetPeriod::Weekly).expect("serializes"), "\"weekly\"");
        assert_eq!(serde_json::from_str::<BudgetPeriod>("\"daily\"").expect("deserializes"), BudgetPeriod::Daily);
        assert!(serde_json::from_str::<BudgetPeriod>("\"yearly\"").is_err());
    }
```

- [ ] **Step 3: Run the tests to verify they fail**

```bash
cd rs && cargo nextest run --locked -p paigasus-gateway domain::limits
```

Expected: compile FAIL (`cannot find type BudgetPeriod`, …).

- [ ] **Step 4: Write the implementation**

Replace the `use` block at the top of `src/domain/limits.rs` with:

```rust
use std::collections::HashMap;
use std::num::NonZeroU64;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Datelike, Days, Months, NaiveDate, NaiveTime, Utc, Weekday};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
```

Append below `impl SlidingWindow` (above `#[cfg(test)]`):

```rust
/// D7/D18: a budget period accepts a late charge for one day after it ends.
pub const LATE_CHARGE_GRACE_SECS: i64 = 86_400;

/// D6: the UTC calendar period of a token budget.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BudgetPeriod {
    Daily,
    Weekly,
    #[default]
    Monthly,
}

/// One concrete budget period. Carried as `Copy` values; formatted only for the Redis key and the
/// refusal message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PeriodKey {
    Day(NaiveDate),
    /// The ISO 8601 week-year and week (`iso_week()`), not the calendar year.
    Week { year: i32, week: u32 },
    Month { year: i32, month: u32 },
}

impl BudgetPeriod {
    /// The period that holds `now`. Every replica derives the same key from the instant alone.
    pub fn key_at(self, now: SystemTime) -> PeriodKey {
        let date = DateTime::<Utc>::from(now).date_naive();
        match self {
            BudgetPeriod::Daily => PeriodKey::Day(date),
            BudgetPeriod::Weekly => {
                let week = date.iso_week();
                PeriodKey::Week { year: week.year(), week: week.week() }
            }
            BudgetPeriod::Monthly => PeriodKey::Month { year: date.year(), month: date.month() },
        }
    }

    /// D18: the relative TTL of the budget key for a charge at `now`: `period_end + 86400 − now`
    /// in whole seconds. `None` when that is below 1 s: the charge arrives more than one day after
    /// its period ended, and is dropped and counted (`period_expired`).
    pub fn charge_ttl(key: PeriodKey, now: SystemTime) -> Option<u32> {
        let now_secs = i64::try_from(now.duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO).as_secs()).unwrap_or(i64::MAX);
        let ttl = key.resets_at_unix().saturating_add(LATE_CHARGE_GRACE_SECS).saturating_sub(now_secs);
        if ttl < 1 { None } else { u32::try_from(ttl).ok() }
    }

    /// D8: the label of the period that resets at `resets_at_unix`, from the instant one second
    /// before the reset ("2026-10", "2026-W40", "2026-10-02").
    pub fn label_at_reset(self, resets_at_unix: i64) -> String {
        let secs = u64::try_from(resets_at_unix.saturating_sub(1)).unwrap_or(0);
        self.key_at(UNIX_EPOCH + Duration::from_secs(secs)).label()
    }
}

impl PeriodKey {
    fn start_date(self) -> NaiveDate {
        match self {
            PeriodKey::Day(date) => Some(date),
            PeriodKey::Week { year, week } => NaiveDate::from_isoywd_opt(year, week, Weekday::Mon),
            PeriodKey::Month { year, month } => NaiveDate::from_ymd_opt(year, month, 1),
        }
        .unwrap_or(NaiveDate::MIN)
    }

    fn end_date(self) -> NaiveDate {
        let start = self.start_date();
        match self {
            PeriodKey::Day(_) => start.checked_add_days(Days::new(1)),
            PeriodKey::Week { .. } => start.checked_add_days(Days::new(7)),
            PeriodKey::Month { .. } => start.checked_add_months(Months::new(1)),
        }
        .unwrap_or(NaiveDate::MAX)
    }

    pub fn start(self) -> DateTime<Utc> {
        self.start_date().and_time(NaiveTime::MIN).and_utc()
    }

    /// The reset instant: the first instant of the next period (exclusive end).
    pub fn end(self) -> DateTime<Utc> {
        self.end_date().and_time(NaiveTime::MIN).and_utc()
    }

    pub fn resets_at_unix(self) -> i64 {
        self.end().timestamp()
    }

    pub fn label(self) -> String {
        match self {
            PeriodKey::Day(date) => date.format("%Y-%m-%d").to_string(),
            PeriodKey::Week { year, week } => format!("{year:04}-W{week:02}"),
            PeriodKey::Month { year, month } => format!("{year:04}-{month:02}"),
        }
    }
}

/// An org's token budget for one request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budget {
    pub tokens: NonZeroU64,
    pub period: BudgetPeriod,
}

/// The resolved limits of one request. `None` on a dimension means no limit on it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LimitPolicy {
    pub principal_requests_per_minute: Option<NonZeroU64>,
    pub org_requests_per_minute: Option<NonZeroU64>,
    pub budget: Option<Budget>,
}

impl LimitPolicy {
    /// D11: an empty policy skips the store.
    pub fn is_empty(&self) -> bool {
        self.principal_requests_per_minute.is_none() && !self.has_org_dimension()
    }

    /// D2: an org rate or a budget applies.
    pub fn has_org_dimension(&self) -> bool {
        self.org_requests_per_minute.is_some() || self.budget.is_some()
    }
}

/// One `[[limits.org]]` entry (D12). A field that is `None` keeps the table default.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OrgOverride {
    pub org_requests_per_minute: Option<NonZeroU64>,
    pub tokens_per_period: Option<NonZeroU64>,
    /// D11: no org rate and no budget for this org; the principal rate still applies.
    pub exempt: bool,
}

/// The table defaults and the per-org overrides, read once at boot. Domain-owned, so the config
/// type stays out of the domain (`LimitsConfig::rules` converts).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LimitRules {
    pub principal_requests_per_minute: Option<NonZeroU64>,
    pub org_requests_per_minute: Option<NonZeroU64>,
    pub tokens_per_period: Option<NonZeroU64>,
    pub budget_period: BudgetPeriod,
    pub overrides: HashMap<Uuid, OrgOverride>,
}

impl LimitRules {
    pub fn policy_for(&self, org: Option<Uuid>) -> LimitPolicy {
        let entry = org.and_then(|id| self.overrides.get(&id)).copied().unwrap_or_default();
        if entry.exempt {
            return LimitPolicy { principal_requests_per_minute: self.principal_requests_per_minute, ..LimitPolicy::default() };
        }
        LimitPolicy {
            principal_requests_per_minute: self.principal_requests_per_minute,
            org_requests_per_minute: entry.org_requests_per_minute.or(self.org_requests_per_minute),
            budget: entry.tokens_per_period.or(self.tokens_per_period).map(|tokens| Budget { tokens, period: self.budget_period }),
        }
    }

    /// D11: true when no table default is set and no override sets a value, so no request can
    /// ever have a non-empty policy. `main.rs` then builds no store.
    pub fn every_policy_is_empty(&self) -> bool {
        self.principal_requests_per_minute.is_none()
            && self.org_requests_per_minute.is_none()
            && self.tokens_per_period.is_none()
            && self.overrides.values().all(|entry| entry.org_requests_per_minute.is_none() && entry.tokens_per_period.is_none())
    }
}

/// One check that refused a request. A refusal carries every failed check (D8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailedCheck {
    PrincipalRate(RateCounts),
    OrgRate(RateCounts),
    Budget { period: BudgetPeriod, resets_at_unix: i64 },
}

/// The failed budget check for `budget` in the period `key`.
pub fn budget_refusal(budget: Budget, key: PeriodKey) -> FailedCheck {
    FailedCheck::Budget { period: budget.period, resets_at_unix: key.resets_at_unix() }
}

/// The right to charge one admitted request. It exists only when a budget applies, names the
/// period the request started in (D7), and is not `Clone`: the charge guard gives it back to
/// `LimitStore::charge` exactly once.
#[derive(Debug, PartialEq, Eq)]
pub struct LimitTicket {
    pub org: Uuid,
    pub period: PeriodKey,
}

#[derive(Debug, PartialEq, Eq)]
pub enum LimitDecision {
    Admit(Option<LimitTicket>),
    Refused(Vec<FailedCheck>),
}

/// The `reason` label of `gateway_limit_refusals_total`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefusalReason {
    PrincipalRate,
    OrgRate,
    OrgBudget,
}

impl RefusalReason {
    pub const ALL: [RefusalReason; 3] = [RefusalReason::PrincipalRate, RefusalReason::OrgRate, RefusalReason::OrgBudget];

    pub fn as_label(self) -> &'static str {
        match self {
            RefusalReason::PrincipalRate => "principal_rate",
            RefusalReason::OrgRate => "org_rate",
            RefusalReason::OrgBudget => "org_budget",
        }
    }
}

/// Why the limits refused a request. The HTTP adapter maps it to a `GatewayError`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitRefusal {
    RateLimited { retry_after_secs: u32, reason: RefusalReason },
    BudgetExhausted { period: BudgetPeriod, resets_at_unix: i64 },
    /// D2: the scope names no org, and an org rate or a budget applies (fail-closed, Q11).
    Unscoped,
}

impl LimitRefusal {
    /// The refusal-metric label; `None` for `Unscoped`, which has its own counter.
    pub fn reason(&self) -> Option<RefusalReason> {
        match self {
            LimitRefusal::RateLimited { reason, .. } => Some(*reason),
            LimitRefusal::BudgetExhausted { .. } => Some(RefusalReason::OrgBudget),
            LimitRefusal::Unscoped => None,
        }
    }

    /// A short name for the refusal log line.
    pub fn label(&self) -> &'static str {
        self.reason().map_or("unscoped", RefusalReason::as_label)
    }
}

/// D8: the org budget first, then the principal rate, then the org rate. `Retry-After` is the
/// largest wait over every failed rate check, at least 1 s. `None` for an empty list.
pub fn choose_refusal(failed: &[FailedCheck]) -> Option<LimitRefusal> {
    if let Some((period, resets_at_unix)) = failed.iter().find_map(|check| match check {
        FailedCheck::Budget { period, resets_at_unix } => Some((*period, *resets_at_unix)),
        _ => None,
    }) {
        return Some(LimitRefusal::BudgetExhausted { period, resets_at_unix });
    }
    let retry_after_secs = failed
        .iter()
        .filter_map(|check| match check {
            FailedCheck::PrincipalRate(counts) | FailedCheck::OrgRate(counts) => Some(counts.retry_after_secs()),
            FailedCheck::Budget { .. } => None,
        })
        .max()?;
    let reason = if failed.iter().any(|check| matches!(check, FailedCheck::PrincipalRate(_))) { RefusalReason::PrincipalRate } else { RefusalReason::OrgRate };
    Some(LimitRefusal::RateLimited { retry_after_secs: retry_after_secs.max(1), reason })
}

/// The `reason` label of `gateway_limit_charges_dropped_total` (D16, D18).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChargeDropReason {
    NoRuntime,
    Shutdown,
    PeriodExpired,
}

impl ChargeDropReason {
    pub const ALL: [ChargeDropReason; 3] = [ChargeDropReason::NoRuntime, ChargeDropReason::Shutdown, ChargeDropReason::PeriodExpired];

    pub fn as_label(self) -> &'static str {
        match self {
            ChargeDropReason::NoRuntime => "no_runtime",
            ChargeDropReason::Shutdown => "shutdown",
            ChargeDropReason::PeriodExpired => "period_expired",
        }
    }
}

/// D10: why a store call failed. `Copy`, and a bounded metric label.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnavailableKind {
    /// Redis is down, the connection failed, or the breaker is open.
    Io,
    /// Redis answered with an error (for example `OOM` under `noeviction`, or a script error).
    Server,
    /// Redis answered with a reply of the wrong type or shape.
    Decode,
}

impl UnavailableKind {
    pub const ALL: [UnavailableKind; 3] = [UnavailableKind::Io, UnavailableKind::Server, UnavailableKind::Decode];

    pub fn as_label(self) -> &'static str {
        match self {
            UnavailableKind::Io => "io",
            UnavailableKind::Server => "server",
            UnavailableKind::Decode => "decode",
        }
    }
}

/// D10: the port's error. It names no `redis` type; the Redis adapter maps its errors here.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LimitStoreError {
    /// `detail` is the store error's text. It never contains the store URL.
    #[error("limit store unavailable ({kind:?}): {detail}")]
    Unavailable { kind: UnavailableKind, detail: String },
}

/// D1/D16: the limit store port. Consumed as `dyn LimitStore`, hence `async_trait`
/// (`rs/Cargo.toml:102-104`).
#[async_trait::async_trait]
pub trait LimitStore: Send + Sync {
    /// D4: check every dimension of `policy`, then count the request on every rate key only when
    /// every check passed. A ticket is issued only when a budget applies and `org` is `Some`.
    async fn check_and_admit(&self, principal: &str, org: Option<Uuid>, policy: &LimitPolicy, now: SystemTime) -> Result<LimitDecision, LimitStoreError>;

    /// D7/D16: add `tokens` to the ticket's period. Must not block and must not panic: the charge
    /// guard calls it from `Drop`. An adapter that needs I/O spawns it.
    fn charge(&self, ticket: LimitTicket, tokens: u64, now: SystemTime);
}

/// The injected clock (spec § 4.3a).
pub trait Clock: Send + Sync {
    fn now(&self) -> SystemTime;
}

/// The production clock. The only place in the domain and application layers that reads the
/// system time.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }
}
```

- [ ] **Step 5: Run the tests to verify they pass**

```bash
cd rs && cargo fmt && cargo nextest run --locked -p paigasus-gateway domain:: && cargo clippy --locked -p paigasus-gateway --all-targets -- -D warnings
```

Expected: PASS. (`DateTime<Utc>: From<SystemTime>` and `NaiveDateTime::and_utc` need chrono's default `std`/`clock` features, which the workspace line keeps.)

- [ ] **Step 6: Commit**

```bash
git add rs/crates/services/paigasus-gateway/Cargo.toml rs/crates/services/paigasus-gateway/src/domain/limits.rs rs/Cargo.lock
git commit -m "feat(rs): add the gateway budget periods, policy and limit store port

UTC daily, ISO-weekly and monthly budget periods with a relative charge
TTL, the per-request policy with org overrides and exemptions, the D8
refusal precedence, and the async LimitStore port with its fail-open
error kinds (SMA-677).

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: Metric names, the memory store and the store contract suite

**Files:**
- Modify: `rs/crates/libs/paigasus-observability/src/names.rs` (after line 13; `ALL` after line 252)
- Create: `rs/crates/services/paigasus-gateway/src/adapters/limits/mod.rs`, `src/adapters/limits/memory.rs`
- Modify: `rs/crates/services/paigasus-gateway/src/adapters/mod.rs`
- Create: `rs/crates/services/paigasus-gateway/tests/support/limits_contract.rs`, `tests/limits_store_memory.rs`
- Modify: `rs/crates/services/paigasus-gateway/tests/support/mod.rs` (`pub mod limits_contract;`)

**Interfaces:**
- Consumes: Task 3's port, policy, decision and period types.
- Produces:
  - `paigasus_observability::names::{GATEWAY_LIMIT_REFUSALS_TOTAL, GATEWAY_TOKENS_CHARGED_TOTAL, GATEWAY_LIMIT_UNSCOPED_REQUESTS_TOTAL, GATEWAY_LIMIT_STORE_UNAVAILABLE_TOTAL, GATEWAY_LIMIT_CHARGES_DROPPED_TOTAL, GATEWAY_REDIS_BREAKER_STATE, GATEWAY_REDIS_BREAKER_TRANSITIONS_TOTAL}` (all in `ALL`)
  - `paigasus_gateway::adapters::limits::MemoryLimitStore` with `fn new() -> MemoryLimitStore` (`Default`), `impl LimitStore`
  - Test support (`tests/support/limits_contract.rs`): `#[async_trait] trait Harness: Send + Sync { async fn store(&self) -> Arc<dyn LimitStore>; async fn charge_now(&self, ticket: LimitTicket, tokens: u64, now: SystemTime); }`; `fn at(rfc3339: &str) -> SystemTime`; `fn fresh_ids() -> (String, Uuid)`; the case functions `org_refusal_does_not_grow_the_principal_count`, `principal_refusal_does_not_grow_the_org_count`, `budget_refusal_grows_neither_rate_count`, `budget_admits_below_and_refuses_at_the_limit`, `a_new_period_starts_at_zero_and_a_late_charge_keeps_its_period`, `the_window_edge_gives_the_d3_estimate`, `concurrent_admissions_never_pass_the_limit`, and `run_all(h: &dyn Harness)`.

D13 eviction rule (the spec says "older than the current budget period" for the org map but does not say which period kind, and one store serves every kind): a principal entry is evicted when its last touch is two rate windows (120 s) old. An org entry is evicted when its rate state is equally idle AND no budget period it holds can still take a late charge (`charge_ttl` is `None` for each). Budget periods past their late-charge day are pruned. This keeps a period's total stable while a late charge can still land, as D7 requires.

- [ ] **Step 1: Add the seven metric names**

In `rs/crates/libs/paigasus-observability/src/names.rs`, after `GATEWAY_UPSTREAM_REQUEST_DURATION_SECONDS`:

```rust
// Gateway limits (SMA-677). No label carries a principal or an org id (unbounded).
/// Chat requests refused before egress, one per request; `reason` = `principal_rate` |
/// `org_rate` | `org_budget`, chosen by the D8 precedence.
pub const GATEWAY_LIMIT_REFUSALS_TOTAL: &str = "gateway_limit_refusals_total";
/// Tokens the charge guard sent to the limit store; `source` = `reported` | `estimated`.
pub const GATEWAY_TOKENS_CHARGED_TOTAL: &str = "gateway_tokens_charged_total";
/// Chat requests whose scope PRN names no organization (D2). Expected 0.
pub const GATEWAY_LIMIT_UNSCOPED_REQUESTS_TOTAL: &str = "gateway_limit_unscoped_requests_total";
/// Limit-store calls that failed or met an open breaker; `op` = `check` | `charge`, `kind` = `io` |
/// `server` | `decode`. A `check` here is a request admitted with no limit (fail-open, D10).
pub const GATEWAY_LIMIT_STORE_UNAVAILABLE_TOTAL: &str = "gateway_limit_store_unavailable_total";
/// Charges that were not sent; `reason` = `no_runtime` | `shutdown` | `period_expired`.
pub const GATEWAY_LIMIT_CHARGES_DROPPED_TOTAL: &str = "gateway_limit_charges_dropped_total";
/// The gateway's limits Redis circuit breaker: 0 closed, 1 half-open, 2 open; `role` = `limits`.
/// Emitted by `paigasus-redis` with this name. Set by every replica: aggregate
/// `max by (job, role)`, never `sum`.
pub const GATEWAY_REDIS_BREAKER_STATE: &str = "gateway_redis_breaker_state";
/// One increment per breaker transition; `role` = `limits`, `to` = `closed` | `half_open` | `open`.
pub const GATEWAY_REDIS_BREAKER_TRANSITIONS_TOTAL: &str = "gateway_redis_breaker_transitions_total";
```

In `ALL`, after `GATEWAY_UPSTREAM_REQUEST_DURATION_SECONDS,`:

```rust
    GATEWAY_LIMIT_REFUSALS_TOTAL,
    GATEWAY_TOKENS_CHARGED_TOTAL,
    GATEWAY_LIMIT_UNSCOPED_REQUESTS_TOTAL,
    GATEWAY_LIMIT_STORE_UNAVAILABLE_TOTAL,
    GATEWAY_LIMIT_CHARGES_DROPPED_TOTAL,
    GATEWAY_REDIS_BREAKER_STATE,
    GATEWAY_REDIS_BREAKER_TRANSITIONS_TOTAL,
```

and in its `tests` module:

```rust
    #[test]
    fn the_gateway_limit_families_are_registered() {
        for name in [
            GATEWAY_LIMIT_REFUSALS_TOTAL,
            GATEWAY_TOKENS_CHARGED_TOTAL,
            GATEWAY_LIMIT_UNSCOPED_REQUESTS_TOTAL,
            GATEWAY_LIMIT_STORE_UNAVAILABLE_TOTAL,
            GATEWAY_LIMIT_CHARGES_DROPPED_TOTAL,
            GATEWAY_REDIS_BREAKER_STATE,
            GATEWAY_REDIS_BREAKER_TRANSITIONS_TOTAL,
        ] {
            assert!(ALL.contains(&name), "{name} is missing from ALL");
        }
    }
```

```bash
cd rs && cargo nextest run --locked -p paigasus-observability names
```

Expected: PASS.

- [ ] **Step 2: Write the contract suite**

Create `rs/crates/services/paigasus-gateway/tests/support/limits_contract.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0

//! The limit-store contract suite (SMA-677 spec § 5.2). One set of cases that BOTH adapters must
//! pass, through the port only: `tests/limits_store_memory.rs` runs it on the memory store and
//! `tests/limits_store_redis.rs` on a real Redis. A difference between the adapters' arithmetic
//! (D3) or their check-then-commit order (D4) reds one of the two runs.
//!
//! Every case takes its own principal and org ids, so cases can share one Redis. Every instant is
//! fixed; nothing sleeps or polls.

use std::num::NonZeroU64;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use paigasus_gateway::domain::limits::{Budget, BudgetPeriod, FailedCheck, LimitDecision, LimitPolicy, LimitStore, LimitTicket, RateCounts};
use uuid::Uuid;

/// How a case reaches one backend.
#[async_trait::async_trait]
pub trait Harness: Send + Sync {
    /// A store on the backend. Redis: a NEW `RedisLimitStore` per call, which models one replica.
    /// Memory: the one shared store.
    async fn store(&self) -> Arc<dyn LimitStore>;
    /// Record a charge and return only when it is recorded (memory: `charge`; Redis:
    /// `apply_charge`, awaited). So no case polls.
    async fn charge_now(&self, ticket: LimitTicket, tokens: u64, now: SystemTime);
}

/// A fixed instant from RFC 3339.
pub fn at(rfc3339: &str) -> SystemTime {
    SystemTime::from(chrono::DateTime::parse_from_rfc3339(rfc3339).expect("a fixture instant"))
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// A principal PRN and an org id that no other case uses. A counter, not a random UUID: the
/// gateway's `uuid` dependency has no `v4`/`v7` feature.
pub fn fresh_ids() -> (String, Uuid) {
    let n = u128::from(NEXT_ID.fetch_add(1, Ordering::SeqCst));
    let principal = Uuid::from_u128(0x0190_a1e5_0000_7000_8000_0000_0000_0000 | n);
    let org = Uuid::from_u128(0x0190_a100_0000_7000_8000_0000_0000_0000 | n);
    (format!("prn:pgs:iam:::principal/{principal}"), org)
}

fn nz(n: u64) -> NonZeroU64 {
    NonZeroU64::new(n).expect("a non-zero limit")
}

fn rates(principal: Option<u64>, org: Option<u64>) -> LimitPolicy {
    LimitPolicy { principal_requests_per_minute: principal.map(nz), org_requests_per_minute: org.map(nz), budget: None }
}

fn budget(tokens: u64, period: BudgetPeriod) -> LimitPolicy {
    LimitPolicy { budget: Some(Budget { tokens: nz(tokens), period }), ..LimitPolicy::default() }
}

fn rate(previous: u64, current: u64, elapsed_ms: u64, limit: u64) -> RateCounts {
    RateCounts { previous, current, elapsed_ms, limit }
}

async fn admit(store: &Arc<dyn LimitStore>, principal: &str, org: Uuid, policy: &LimitPolicy, now: SystemTime) -> LimitDecision {
    store.check_and_admit(principal, Some(org), policy, now).await.expect("the contract suite runs against a healthy store")
}

async fn ticket(store: &Arc<dyn LimitStore>, principal: &str, org: Uuid, policy: &LimitPolicy, now: SystemTime) -> LimitTicket {
    match admit(store, principal, org, policy, now).await {
        LimitDecision::Admit(Some(ticket)) => ticket,
        other => panic!("expected an admission with a ticket, got {other:?}"),
    }
}

const NOON: &str = "2026-10-02T12:00:00Z";

/// D4: an org refusal does not use the principal's quota.
pub async fn org_refusal_does_not_grow_the_principal_count(h: &dyn Harness) {
    let store = h.store().await;
    let (p, org) = fresh_ids();
    let t = at(NOON);
    let both = rates(Some(2), Some(1));
    assert_eq!(admit(&store, &p, org, &both, t).await, LimitDecision::Admit(None));
    assert_eq!(admit(&store, &p, org, &both, t).await, LimitDecision::Refused(vec![FailedCheck::OrgRate(rate(0, 1, 0, 1))]));
    // If the refusal had counted the principal, its count would be 2 and this would be refused.
    let principal_only = rates(Some(2), None);
    assert_eq!(admit(&store, &p, org, &principal_only, t).await, LimitDecision::Admit(None));
    assert_eq!(admit(&store, &p, org, &principal_only, t).await, LimitDecision::Refused(vec![FailedCheck::PrincipalRate(rate(0, 2, 0, 2))]));
}

/// D4, the reverse: a principal refusal does not use the org's quota.
pub async fn principal_refusal_does_not_grow_the_org_count(h: &dyn Harness) {
    let store = h.store().await;
    let (p1, org) = fresh_ids();
    let (p2, _) = fresh_ids();
    let (p3, _) = fresh_ids();
    let t = at(NOON);
    let both = rates(Some(1), Some(2));
    assert_eq!(admit(&store, &p1, org, &both, t).await, LimitDecision::Admit(None));
    assert_eq!(admit(&store, &p1, org, &both, t).await, LimitDecision::Refused(vec![FailedCheck::PrincipalRate(rate(0, 1, 0, 1))]));
    // If the refusal had counted the org, the org would be full and p2 would be refused.
    assert_eq!(admit(&store, &p2, org, &both, t).await, LimitDecision::Admit(None));
    assert_eq!(admit(&store, &p3, org, &both, t).await, LimitDecision::Refused(vec![FailedCheck::OrgRate(rate(0, 2, 0, 2))]));
}

/// D4: a budget refusal grows neither rate count.
pub async fn budget_refusal_grows_neither_rate_count(h: &dyn Harness) {
    let store = h.store().await;
    let (p, org) = fresh_ids();
    let t = at(NOON);
    let monthly = budget(10, BudgetPeriod::Monthly);
    let first = ticket(&store, &p, org, &monthly, t).await;
    h.charge_now(first, 10, t).await;
    let all = LimitPolicy { principal_requests_per_minute: Some(nz(1)), org_requests_per_minute: Some(nz(1)), ..monthly };
    assert_eq!(admit(&store, &p, org, &all, t).await, LimitDecision::Refused(vec![FailedCheck::Budget { period: BudgetPeriod::Monthly, resets_at_unix: 1_793_491_200 }]));
    // Both rate counts are still 0: one request passes both limits of 1, the next fails both.
    let rates_only = rates(Some(1), Some(1));
    assert_eq!(admit(&store, &p, org, &rates_only, t).await, LimitDecision::Admit(None));
    assert_eq!(
        admit(&store, &p, org, &rates_only, t).await,
        LimitDecision::Refused(vec![FailedCheck::PrincipalRate(rate(0, 1, 0, 1)), FailedCheck::OrgRate(rate(0, 1, 0, 1))])
    );
}

/// D7: used < budget admits; the overshoot of one request is allowed and visible.
pub async fn budget_admits_below_and_refuses_at_the_limit(h: &dyn Harness) {
    let store = h.store().await;
    let (p, org) = fresh_ids();
    let (_, full_org) = fresh_ids();
    let t = at(NOON);
    let monthly = budget(10, BudgetPeriod::Monthly);
    let refused = LimitDecision::Refused(vec![FailedCheck::Budget { period: BudgetPeriod::Monthly, resets_at_unix: 1_793_491_200 }]);

    // 10 of 10 → refused.
    let t1 = ticket(&store, &p, full_org, &monthly, t).await;
    h.charge_now(t1, 10, t).await;
    assert_eq!(admit(&store, &p, full_org, &monthly, t).await, refused);

    // 9 of 10 → admitted; its charge of 50 is recorded (59 > 10), so the next one is refused.
    let t2 = ticket(&store, &p, org, &monthly, t).await;
    h.charge_now(t2, 9, t).await;
    let t3 = ticket(&store, &p, org, &monthly, t).await;
    h.charge_now(t3, 50, t).await;
    assert_eq!(admit(&store, &p, org, &monthly, t).await, refused);
}

/// D6/D7: each period starts at zero, and a late charge lands in its ticket's period.
pub async fn a_new_period_starts_at_zero_and_a_late_charge_keeps_its_period(h: &dyn Harness) {
    let store = h.store().await;
    let rows = [
        (BudgetPeriod::Daily, "2026-10-02T23:59:59Z", "2026-10-03T00:00:00Z", 1_790_985_600_i64),
        (BudgetPeriod::Weekly, "2026-10-04T23:59:59Z", "2026-10-05T00:00:00Z", 1_791_158_400),
        (BudgetPeriod::Monthly, "2026-10-31T23:59:59Z", "2026-11-01T00:00:00Z", 1_793_491_200),
    ];
    for (period, before, after, old_reset) in rows {
        let (p, org) = fresh_ids();
        let policy = budget(10, period);
        let old = ticket(&store, &p, org, &policy, at(before)).await;
        // The request ends after the period rolled; its charge goes to the OLD period.
        h.charge_now(old, 10, at(after)).await;
        match admit(&store, &p, org, &policy, at(after)).await {
            LimitDecision::Admit(Some(new)) => assert_eq!(new.period, period.key_at(at(after)), "{period:?}: the new period starts at zero"),
            other => panic!("{period:?}: the new period must admit, got {other:?}"),
        }
        assert_eq!(
            admit(&store, &p, org, &policy, at(before)).await,
            LimitDecision::Refused(vec![FailedCheck::Budget { period, resets_at_unix: old_reset }]),
            "{period:?}: the old period holds the late charge"
        );
    }
}

/// D3 at the window edge: 59.999 s and 60.000 s, the same on both adapters.
pub async fn the_window_edge_gives_the_d3_estimate(h: &dyn Harness) {
    let store = h.store().await;
    let (p, org) = fresh_ids();
    let two = rates(Some(2), None);
    assert_eq!(admit(&store, &p, org, &two, at("2026-10-02T12:00:00.000Z")).await, LimitDecision::Admit(None));
    assert_eq!(admit(&store, &p, org, &two, at("2026-10-02T12:00:00.000Z")).await, LimitDecision::Admit(None));
    assert_eq!(admit(&store, &p, org, &two, at("2026-10-02T12:00:59.999Z")).await, LimitDecision::Refused(vec![FailedCheck::PrincipalRate(rate(0, 2, 59_999, 2))]));
    assert_eq!(admit(&store, &p, org, &two, at("2026-10-02T12:01:00.000Z")).await, LimitDecision::Refused(vec![FailedCheck::PrincipalRate(rate(2, 0, 0, 2))]));
    // 2 × 30000 + 0 + 60000 = 120000 ≤ 120000.
    assert_eq!(admit(&store, &p, org, &two, at("2026-10-02T12:01:30.000Z")).await, LimitDecision::Admit(None));
    assert_eq!(admit(&store, &p, org, &two, at("2026-10-02T12:01:30.000Z")).await, LimitDecision::Refused(vec![FailedCheck::PrincipalRate(rate(2, 1, 30_000, 2))]));
}

/// D4 under concurrency: 64 tasks over 4 stores (4 replicas on Redis), limit 10, one instant.
pub async fn concurrent_admissions_never_pass_the_limit(h: &dyn Harness) {
    let mut stores = Vec::new();
    for _ in 0..4 {
        stores.push(h.store().await);
    }
    let (p, _) = fresh_ids();
    let policy = rates(Some(10), None);
    let t = at(NOON);
    let mut tasks = Vec::new();
    for i in 0..64 {
        let store = Arc::clone(&stores[i % 4]);
        let principal = p.clone();
        tasks.push(tokio::spawn(async move { store.check_and_admit(&principal, None, &policy, t).await.expect("a healthy store") }));
    }
    let mut admitted = 0;
    for task in tasks {
        if matches!(task.await.expect("the task does not panic"), LimitDecision::Admit(_)) {
            admitted += 1;
        }
    }
    assert_eq!(admitted, 10, "exactly the limit is admitted");
}

/// Every case, in order. The Redis binary runs this once per image on one container.
pub async fn run_all(h: &dyn Harness) {
    org_refusal_does_not_grow_the_principal_count(h).await;
    principal_refusal_does_not_grow_the_org_count(h).await;
    budget_refusal_grows_neither_rate_count(h).await;
    budget_admits_below_and_refuses_at_the_limit(h).await;
    a_new_period_starts_at_zero_and_a_late_charge_keeps_its_period(h).await;
    the_window_edge_gives_the_d3_estimate(h).await;
    concurrent_admissions_never_pass_the_limit(h).await;
}
```

In `tests/support/mod.rs`, after the `#![allow(dead_code)]` line and its comment:

```rust
pub mod limits_contract;
```

- [ ] **Step 3: Write the memory binary**

Create `rs/crates/services/paigasus-gateway/tests/limits_store_memory.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0

//! The limit-store contract suite (spec § 5.2) on `MemoryLimitStore`.

mod support;

use std::sync::Arc;
use std::time::SystemTime;

use paigasus_gateway::adapters::limits::MemoryLimitStore;
use paigasus_gateway::domain::limits::{LimitStore, LimitTicket};
use support::limits_contract::{self as contract, Harness};

struct MemoryHarness {
    store: Arc<MemoryLimitStore>,
}

#[async_trait::async_trait]
impl Harness for MemoryHarness {
    async fn store(&self) -> Arc<dyn LimitStore> {
        self.store.clone()
    }

    async fn charge_now(&self, ticket: LimitTicket, tokens: u64, now: SystemTime) {
        // Synchronous: the memory adapter applies a charge under its lock and returns (D16).
        self.store.charge(ticket, tokens, now);
    }
}

fn harness() -> MemoryHarness {
    MemoryHarness { store: Arc::new(MemoryLimitStore::new()) }
}

#[tokio::test]
async fn org_refusal_does_not_grow_the_principal_count() {
    contract::org_refusal_does_not_grow_the_principal_count(&harness()).await;
}

#[tokio::test]
async fn principal_refusal_does_not_grow_the_org_count() {
    contract::principal_refusal_does_not_grow_the_org_count(&harness()).await;
}

#[tokio::test]
async fn budget_refusal_grows_neither_rate_count() {
    contract::budget_refusal_grows_neither_rate_count(&harness()).await;
}

#[tokio::test]
async fn budget_admits_below_and_refuses_at_the_limit() {
    contract::budget_admits_below_and_refuses_at_the_limit(&harness()).await;
}

#[tokio::test]
async fn a_new_period_starts_at_zero_and_a_late_charge_keeps_its_period() {
    contract::a_new_period_starts_at_zero_and_a_late_charge_keeps_its_period(&harness()).await;
}

#[tokio::test]
async fn the_window_edge_gives_the_d3_estimate() {
    contract::the_window_edge_gives_the_d3_estimate(&harness()).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_admissions_never_pass_the_limit() {
    contract::concurrent_admissions_never_pass_the_limit(&harness()).await;
}
```

- [ ] **Step 4: Run it to verify it fails**

```bash
cd rs && cargo nextest run --locked -p paigasus-gateway --test limits_store_memory
```

Expected: compile FAIL (`could not find limits in adapters`).

- [ ] **Step 5: Write the memory store**

Create `src/adapters/limits/mod.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0

//! The two `LimitStore` adapters (SMA-677 spec § 4.3).

pub mod memory;

pub use memory::MemoryLimitStore;
```

In `src/adapters/mod.rs`, update the doc line and add the module:

```rust
//! Hexagonal adapters. `http` is the inbound HTTP surface (G3); `iam` is the outbound IAM gRPC
//! client (G4); `openai` is the outbound OpenAI egress client (G6); `limits` holds the limit-store
//! adapters (SMA-677).

pub mod http;
pub mod iam;
pub mod limits;
pub mod openai;
```

Create `src/adapters/limits/memory.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0

//! The in-process `LimitStore` (SMA-677 D1, D4, D10, D13). For development and a single replica:
//! with N replicas behind a balancer every replica keeps its own counts, so the effective limits
//! are about N times the configured ones, and a restart resets every budget (spec § 6).
//!
//! One `std::sync::Mutex` guards both maps. It is held for the arithmetic only and never across
//! an `.await` (`check_and_admit` has none inside), so every key is checked and then committed
//! under one lock (D4). A poisoned lock is recovered: every update is a whole-integer write, so
//! the state stays consistent (D10).

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime};

use metrics::counter;
use paigasus_observability::names;
use uuid::Uuid;

use crate::domain::limits::{
    BudgetPeriod, ChargeDropReason, FailedCheck, LimitDecision, LimitPolicy, LimitStore, LimitStoreError, LimitTicket, MAX_TOKENS_PER_CHARGE, PeriodKey, SlidingWindow, budget_refusal,
};

/// D13: the sweep runs on the check path, once per this many checks. No background task.
const SWEEP_EVERY: u64 = 1024;
/// D13: two rate windows.
const RATE_IDLE: Duration = Duration::from_secs(120);

struct OrgState {
    window: SlidingWindow,
    used: HashMap<PeriodKey, u64>,
    last_touch: SystemTime,
}

impl OrgState {
    fn new(now: SystemTime) -> Self {
        OrgState { window: SlidingWindow::default(), used: HashMap::new(), last_touch: now }
    }
}

#[derive(Default)]
struct State {
    principals: HashMap<String, (SlidingWindow, SystemTime)>,
    orgs: HashMap<Uuid, OrgState>,
    checks: u64,
}

/// A backward clock step reads as no idle time (D10).
fn idle(last: SystemTime, now: SystemTime) -> Duration {
    now.duration_since(last).unwrap_or(Duration::ZERO)
}

impl State {
    /// D13. See this task's eviction rule in the plan.
    fn sweep(&mut self, now: SystemTime) {
        self.principals.retain(|_, (_, last)| idle(*last, now) < RATE_IDLE);
        self.orgs.retain(|_, org| {
            org.used.retain(|key, _| BudgetPeriod::charge_ttl(*key, now).is_some());
            idle(org.last_touch, now) < RATE_IDLE || !org.used.is_empty()
        });
    }
}

#[derive(Default)]
pub struct MemoryLimitStore {
    state: Mutex<State>,
}

impl MemoryLimitStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[async_trait::async_trait]
impl LimitStore for MemoryLimitStore {
    async fn check_and_admit(&self, principal: &str, org: Option<Uuid>, policy: &LimitPolicy, now: SystemTime) -> Result<LimitDecision, LimitStoreError> {
        let mut state = self.lock();
        state.checks = state.checks.wrapping_add(1);
        if state.checks.is_multiple_of(SWEEP_EVERY) {
            state.sweep(now);
        }

        // Check every dimension first (pure reads) …
        let mut failed = Vec::new();
        if let Some(limit) = policy.principal_requests_per_minute {
            let window = state.principals.get(principal).map(|(window, _)| *window).unwrap_or_default();
            if let Err(counts) = window.check(now, limit) {
                failed.push(FailedCheck::PrincipalRate(counts));
            }
        }
        if let (Some(limit), Some(id)) = (policy.org_requests_per_minute, org) {
            let window = state.orgs.get(&id).map(|o| o.window).unwrap_or_default();
            if let Err(counts) = window.check(now, limit) {
                failed.push(FailedCheck::OrgRate(counts));
            }
        }
        let budget = policy.budget.zip(org).map(|(budget, id)| (budget, id, budget.period.key_at(now)));
        if let Some((budget, id, key)) = budget {
            let used = state.orgs.get(&id).and_then(|o| o.used.get(&key)).copied().unwrap_or(0);
            if used >= budget.tokens.get() {
                failed.push(budget_refusal(budget, key));
            }
        }
        if !failed.is_empty() {
            return Ok(LimitDecision::Refused(failed));
        }

        // … then commit every rate key, all or nothing (D4).
        if policy.principal_requests_per_minute.is_some() {
            let entry = state.principals.entry(principal.to_owned()).or_insert((SlidingWindow::default(), now));
            entry.0.commit(now);
            entry.1 = now;
        }
        if let (Some(_), Some(id)) = (policy.org_requests_per_minute, org) {
            let entry = state.orgs.entry(id).or_insert_with(|| OrgState::new(now));
            entry.window.commit(now);
            entry.last_touch = now;
        }
        Ok(LimitDecision::Admit(budget.map(|(_, org, period)| LimitTicket { org, period })))
    }

    fn charge(&self, ticket: LimitTicket, tokens: u64, now: SystemTime) {
        if tokens == 0 {
            return;
        }
        if BudgetPeriod::charge_ttl(ticket.period, now).is_none() {
            counter!(names::GATEWAY_LIMIT_CHARGES_DROPPED_TOTAL, "reason" => ChargeDropReason::PeriodExpired.as_label()).increment(1);
            return;
        }
        let mut state = self.lock();
        let org = state.orgs.entry(ticket.org).or_insert_with(|| OrgState::new(now));
        let used = org.used.entry(ticket.period).or_insert(0);
        *used = used.saturating_add(tokens.min(MAX_TOKENS_PER_CHARGE));
        org.last_touch = now;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::num::NonZeroU64;
    use std::sync::Arc;

    fn at(rfc3339: &str) -> SystemTime {
        SystemTime::from(chrono::DateTime::parse_from_rfc3339(rfc3339).expect("a fixture instant"))
    }

    fn principal_limit(n: u64) -> LimitPolicy {
        LimitPolicy { principal_requests_per_minute: NonZeroU64::new(n), ..LimitPolicy::default() }
    }

    const ORG: Uuid = Uuid::from_u128(0x0190a100_0000_7000_8000_0000000000a1);

    /// D13: the sweep drops an idle principal and keeps an active one.
    #[tokio::test]
    async fn the_sweep_evicts_an_idle_principal_and_keeps_an_active_one() {
        let store = MemoryLimitStore::new();
        let policy = principal_limit(1_000_000);
        store.check_and_admit("idle", None, &policy, at("2026-10-02T12:00:00Z")).await.expect("memory never fails");
        // 1023 more checks: the 1024th check overall runs the sweep, 200 s after "idle" was seen.
        for _ in 0..(SWEEP_EVERY - 1) {
            store.check_and_admit("active", None, &policy, at("2026-10-02T12:03:20Z")).await.expect("memory never fails");
        }
        let state = store.lock();
        assert!(!state.principals.contains_key("idle"), "an idle principal is evicted");
        assert!(state.principals.contains_key("active"), "an active principal stays");
    }

    /// D13/D7: an org whose budget period can still take a late charge is kept.
    #[tokio::test]
    async fn the_sweep_keeps_a_budget_that_can_still_take_a_late_charge() {
        let store = MemoryLimitStore::new();
        store.charge(LimitTicket { org: ORG, period: BudgetPeriod::Daily.key_at(at("2026-10-02T12:00:00Z")) }, 5, at("2026-10-02T12:00:00Z"));
        let policy = principal_limit(1_000_000);
        for _ in 0..SWEEP_EVERY {
            store.check_and_admit("p", None, &policy, at("2026-10-03T12:00:00Z")).await.expect("memory never fails");
        }
        assert!(store.lock().orgs.contains_key(&ORG), "the day after the period, a late charge can still land");
        for _ in 0..SWEEP_EVERY {
            store.check_and_admit("p", None, &policy, at("2026-10-04T00:00:00Z")).await.expect("memory never fails");
        }
        assert!(!store.lock().orgs.contains_key(&ORG), "a day after the period's end, the org is evicted");
    }

    /// D10: a thread panics while it holds the lock; the store still works.
    #[tokio::test]
    async fn a_poisoned_lock_is_recovered() {
        let store = Arc::new(MemoryLimitStore::new());
        let poisoner = Arc::clone(&store);
        let joined = std::thread::spawn(move || {
            let _held = poisoner.state.lock().expect("not yet poisoned");
            panic!("poison the limit store lock on purpose");
        })
        .join();
        assert!(joined.is_err(), "the thread panicked");
        assert!(store.state.is_poisoned());
        let decision = store.check_and_admit("p", None, &principal_limit(1), at("2026-10-02T12:00:00Z")).await.expect("memory never fails");
        assert_eq!(decision, LimitDecision::Admit(None));
    }

    /// D18 parity: a charge more than a day late is dropped, not booked.
    #[tokio::test]
    async fn a_charge_more_than_a_day_late_is_dropped() {
        let store = MemoryLimitStore::new();
        let october = BudgetPeriod::Monthly.key_at(at("2026-10-15T00:00:00Z"));
        store.charge(LimitTicket { org: ORG, period: october }, 5, at("2026-11-02T00:00:00Z"));
        assert!(!store.lock().orgs.contains_key(&ORG), "nothing is booked");
    }
}
```

- [ ] **Step 6: Run the tests to verify they pass**

```bash
cd rs && cargo fmt && cargo nextest run --locked -p paigasus-gateway && cargo clippy --locked -p paigasus-gateway --all-targets -- -D warnings
```

Expected: PASS; `limits_store_memory` runs seven tests and the four memory unit tests pass.

- [ ] **Step 7: Prove the D4 rows bite (mutation 2 on the memory adapter)**

Temporarily move the principal commit block in `check_and_admit` ABOVE `if !failed.is_empty() { return … }`. Run:

```bash
cd rs && cargo nextest run --locked -p paigasus-gateway --test limits_store_memory --no-fail-fast
```

Expected: FAIL in `org_refusal_does_not_grow_the_principal_count`. Restore the block with the editor (do not use `git checkout --`, which would also discard this task's uncommitted work), then re-run and see PASS.

- [ ] **Step 8: Commit**

```bash
git add rs/crates/libs/paigasus-observability/src/names.rs rs/crates/services/paigasus-gateway/src/adapters rs/crates/services/paigasus-gateway/tests/support rs/crates/services/paigasus-gateway/tests/limits_store_memory.rs
git commit -m "feat(rs): add the memory limit store and the store contract suite

The in-process LimitStore with the D4 check-then-commit order, poison
recovery and the D13 sweep, the seven SMA-677 metric names, and one
contract suite that every limit store must pass.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: The `[limits]` config table

**Files:**
- Modify: `rs/crates/services/paigasus-gateway/src/config.rs` (imports; `GatewayConfig` gains `limits`; new types after `MetricsConfig` (line 144); `validate` gains one call before `Ok(())` at line 309; tests)

**Interfaces:**
- Consumes: Task 3's `BudgetPeriod`, `LimitRules`, `OrgOverride`, `MAX_REQUESTS_PER_MINUTE`, `MAX_TOKENS_PER_PERIOD`; `crate::domain::parse_org_uuid`.
- Produces (in `paigasus_gateway::config`):
  - `GatewayConfig.limits: Option<LimitsConfig>`
  - `struct LimitsConfig { pub backend: LimitsBackend, pub redis_url: Option<SecretString>, pub principal_requests_per_minute: Option<u64>, pub org_requests_per_minute: Option<u64>, pub tokens_per_period: Option<u64>, pub budget_period: BudgetPeriod, pub org: Vec<OrgLimitsConfig> }` — `Debug + Clone + Default + Deserialize + Serialize`, `deny_unknown_fields`; `fn validate(&self) -> Result<(), String>`; `fn rules(&self) -> LimitRules`
  - `struct OrgLimitsConfig { pub id: String, pub org_requests_per_minute: Option<u64>, pub tokens_per_period: Option<u64>, pub exempt: bool }`
  - `enum LimitsBackend { Memory, Redis }` — `Copy + Default` (`Memory`), `rename_all = "lowercase"`

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module of `src/config.rs`:

```rust
    use crate::domain::limits::{BudgetPeriod, OrgOverride};
    use std::num::NonZeroU64;

    const ORG_A: &str = "0190a100-0000-7000-8000-0000000000a1";

    fn limits_toml(body: &str) -> String {
        format!("{}\n[limits]\n{body}\n", valid_toml())
    }

    #[test]
    fn no_limits_table_means_no_limits() {
        figment::Jail::expect_with(|jail| {
            jail.create_file("gateway.toml", valid_toml())?;
            let cfg: GatewayConfig = GatewayConfig::figment().extract()?;
            assert!(cfg.limits.is_none(), "A6/Q5: limits are off unless configured");
            assert!(cfg.validate().is_ok());
            Ok(())
        });
    }

    #[test]
    fn an_empty_limits_table_gives_only_empty_policies() {
        figment::Jail::expect_with(|jail| {
            jail.create_file("gateway.toml", &limits_toml(""))?;
            let cfg: GatewayConfig = GatewayConfig::figment().extract()?;
            let limits = cfg.limits.as_ref().expect("an empty [limits] table is Some");
            assert_eq!(limits.backend, LimitsBackend::Memory, "the backend defaults to memory");
            assert_eq!(limits.budget_period, BudgetPeriod::Monthly, "the period defaults to monthly");
            assert!(cfg.validate().is_ok());
            assert!(limits.rules().every_policy_is_empty());
            Ok(())
        });
    }

    #[test]
    fn a_full_limits_table_loads_and_converts_to_rules() {
        figment::Jail::expect_with(|jail| {
            let body = format!(
                "principal_requests_per_minute = 60\norg_requests_per_minute = 600\ntokens_per_period = 5000000\nbudget_period = \"weekly\"\n\n[[limits.org]]\nid = \"{ORG_A}\"\norg_requests_per_minute = 1200\ntokens_per_period = 20000000\n\n[[limits.org]]\nid = \"0190a100-0000-7000-8000-0000000000b2\"\nexempt = true\n"
            );
            jail.create_file("gateway.toml", &limits_toml(&body))?;
            let cfg: GatewayConfig = GatewayConfig::figment().extract()?;
            assert!(cfg.validate().is_ok(), "{:?}", cfg.validate());
            let rules = cfg.limits.as_ref().expect("the table is set").rules();
            assert_eq!(rules.principal_requests_per_minute, NonZeroU64::new(60));
            assert_eq!(rules.org_requests_per_minute, NonZeroU64::new(600));
            assert_eq!(rules.tokens_per_period, NonZeroU64::new(5_000_000));
            assert_eq!(rules.budget_period, BudgetPeriod::Weekly);
            let a = uuid::Uuid::try_parse(ORG_A).expect("a uuid");
            assert_eq!(rules.overrides[&a], OrgOverride { org_requests_per_minute: NonZeroU64::new(1200), tokens_per_period: NonZeroU64::new(20_000_000), exempt: false });
            let b = uuid::Uuid::try_parse("0190a100-0000-7000-8000-0000000000b2").expect("a uuid");
            assert!(rules.overrides[&b].exempt);
            Ok(())
        });
    }

    #[test]
    fn validate_rejects_each_bad_limits_value_and_names_the_key() {
        let cases: &[(&str, &str)] = &[
            ("principal_requests_per_minute = 0", "limits.principal_requests_per_minute must be at least 1"),
            ("org_requests_per_minute = 0", "limits.org_requests_per_minute must be at least 1"),
            ("tokens_per_period = 0", "limits.tokens_per_period must be at least 1"),
            ("principal_requests_per_minute = 1000000001", "limits.principal_requests_per_minute must be at most 1000000000"),
            ("org_requests_per_minute = 1000000001", "limits.org_requests_per_minute must be at most 1000000000"),
            ("tokens_per_period = 1000000000001", "limits.tokens_per_period must be at most 1000000000000"),
            ("[[limits.org]]\nid = \"acme\"", "limits.org[0].id must be one organization UUID"),
            ("[[limits.org]]\nid = \"0190a1000000700080000000000000a1\"", "limits.org[0].id must be one organization UUID"),
            (
                "[[limits.org]]\nid = \"0190a100-0000-7000-8000-0000000000a1\"\n[[limits.org]]\nid = \"0190A100-0000-7000-8000-0000000000A1\"",
                "limits.org[1].id",
            ),
            ("[[limits.org]]\nid = \"0190a100-0000-7000-8000-0000000000a1\"\norg_requests_per_minute = 0", "limits.org[0].org_requests_per_minute must be at least 1"),
            ("[[limits.org]]\nid = \"0190a100-0000-7000-8000-0000000000a1\"\ntokens_per_period = 1000000000001", "limits.org[0].tokens_per_period must be at most"),
            ("[[limits.org]]\nid = \"0190a100-0000-7000-8000-0000000000a1\"\nexempt = true\ntokens_per_period = 5", "limits.org[0] sets exempt = true together with a limit"),
            ("backend = \"redis\"", "limits.backend = \"redis\" requires limits.redis_url"),
            ("backend = \"redis\"\nredis_url = \"  \"", "limits.backend = \"redis\" requires limits.redis_url"),
            ("redis_url = \"redis://127.0.0.1:6379\"", "limits.redis_url is set but limits.backend is \"memory\""),
        ];
        for (body, want) in cases {
            figment::Jail::expect_with(|jail| {
                jail.create_file("gateway.toml", &limits_toml(body))?;
                let cfg: GatewayConfig = GatewayConfig::figment().extract()?;
                let err = cfg.validate().expect_err(body);
                assert!(err.contains(want), "{body}: got {err}");
                Ok(())
            });
        }
    }

    #[test]
    fn an_unknown_budget_period_fails_extraction() {
        figment::Jail::expect_with(|jail| {
            jail.create_file("gateway.toml", &limits_toml("budget_period = \"yearly\""))?;
            assert!(GatewayConfig::figment().extract::<GatewayConfig>().is_err());
            Ok(())
        });
    }

    /// Review Focus 1: a typo must fail the boot, not switch a budget off silently.
    #[test]
    fn a_misspelt_limits_key_fails_extraction() {
        for body in ["token_per_period = 5", "[[limits.org]]\nid = \"0190a100-0000-7000-8000-0000000000a1\"\nexmpt = true"] {
            figment::Jail::expect_with(|jail| {
                jail.create_file("gateway.toml", &limits_toml(body))?;
                let err = GatewayConfig::figment().extract::<GatewayConfig>().expect_err(body);
                assert!(err.to_string().contains("unknown field"), "{body}: {err}");
                Ok(())
            });
        }
    }

    /// Review Focus 2: the override id and the scope PRN may use different letter case.
    #[test]
    fn an_upper_case_override_id_applies_to_the_canonical_org() {
        figment::Jail::expect_with(|jail| {
            jail.create_file("gateway.toml", &limits_toml("org_requests_per_minute = 10\n[[limits.org]]\nid = \"0190A100-0000-7000-8000-0000000000A1\"\norg_requests_per_minute = 99"))?;
            let cfg: GatewayConfig = GatewayConfig::figment().extract()?;
            assert!(cfg.validate().is_ok());
            let lower = uuid::Uuid::try_parse(ORG_A).expect("a uuid");
            assert_eq!(cfg.limits.as_ref().expect("set").rules().policy_for(Some(lower)).org_requests_per_minute, NonZeroU64::new(99));
            Ok(())
        });
    }

    #[test]
    fn every_limits_key_reads_from_the_environment() {
        figment::Jail::expect_with(|jail| {
            jail.create_file("gateway.toml", valid_toml())?;
            jail.set_env("GATEWAY_LIMITS__BACKEND", "redis");
            jail.set_env("GATEWAY_LIMITS__REDIS_URL", "redis://:hunter2@127.0.0.1:6379/2");
            jail.set_env("GATEWAY_LIMITS__PRINCIPAL_REQUESTS_PER_MINUTE", "60");
            jail.set_env("GATEWAY_LIMITS__ORG_REQUESTS_PER_MINUTE", "600");
            jail.set_env("GATEWAY_LIMITS__TOKENS_PER_PERIOD", "5000000");
            jail.set_env("GATEWAY_LIMITS__BUDGET_PERIOD", "daily");
            let cfg: GatewayConfig = GatewayConfig::figment().extract()?;
            assert!(cfg.validate().is_ok(), "{:?}", cfg.validate());
            let limits = cfg.limits.as_ref().expect("the env vars create the table");
            assert_eq!(limits.backend, LimitsBackend::Redis);
            assert_eq!(limits.redis_url.as_ref().map(|u| u.expose_secret().to_owned()), Some("redis://:hunter2@127.0.0.1:6379/2".to_owned()));
            assert_eq!((limits.principal_requests_per_minute, limits.org_requests_per_minute, limits.tokens_per_period), (Some(60), Some(600), Some(5_000_000)));
            assert_eq!(limits.budget_period, BudgetPeriod::Daily);
            Ok(())
        });
    }

    /// D17: the Redis URL can carry a password, and `GatewayConfig` derives Debug and Serialize.
    #[test]
    fn the_redis_url_never_reaches_debug_or_serialize() {
        figment::Jail::expect_with(|jail| {
            jail.create_file("gateway.toml", valid_toml())?;
            jail.set_env("GATEWAY_LIMITS__BACKEND", "redis");
            jail.set_env("GATEWAY_LIMITS__REDIS_URL", "rediss://:hunter2@redis.internal:6380/2");
            let cfg: GatewayConfig = GatewayConfig::figment().extract()?;
            let serialized = serde_json::to_string(&cfg).expect("the config serializes");
            assert!(!serialized.contains("hunter2") && !serialized.contains("redis.internal"), "{serialized}");
            let debug = format!("{cfg:?}");
            assert!(!debug.contains("hunter2") && !debug.contains("redis.internal"), "{debug}");
            Ok(())
        });
    }
```

- [ ] **Step 2: Run them to verify they fail**

```bash
cd rs && cargo nextest run --locked -p paigasus-gateway config::
```

Expected: compile FAIL (`no field limits on type GatewayConfig`, `cannot find type LimitsBackend`).

- [ ] **Step 3: Write the implementation**

Add to the imports of `src/config.rs`:

```rust
use std::collections::HashSet;
use std::num::NonZeroU64;

use crate::domain::limits::{BudgetPeriod, LimitRules, MAX_REQUESTS_PER_MINUTE, MAX_TOKENS_PER_PERIOD, OrgOverride};
use crate::domain::parse_org_uuid;
```

Add a field at the end of `GatewayConfig`:

```rust
    /// SMA-677: the optional rate limit and token budget on `POST /v1/chat/completions`. `None`
    /// (no `[limits]` table, the default) means no limit at all and no Redis connection (A6, Q5).
    /// The `Defaults` layer has no `limits` entry on purpose.
    #[serde(default)]
    pub limits: Option<LimitsConfig>,
```

Add after `impl Default for MetricsConfig`:

```rust
/// `[limits]` (SMA-677 § 4.1). An unset key means no limit on that dimension (D11). A misspelt
/// key fails extraction (`deny_unknown_fields`): a typo must not switch a budget off silently.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct LimitsConfig {
    /// D17: `memory` (one replica, the default) or `redis` (shared across replicas).
    pub backend: LimitsBackend,
    /// D17: required for `backend = "redis"`, rejected for `memory`. It can carry a password, so
    /// it is skipped on `Serialize`, and `SecretString` redacts it on `Debug` — the same pattern
    /// as `upstream.openai.api_key` (Q15). Set it through `GATEWAY_LIMITS__REDIS_URL`.
    #[serde(skip_serializing)]
    pub redis_url: Option<SecretString>,
    pub principal_requests_per_minute: Option<u64>,
    pub org_requests_per_minute: Option<u64>,
    pub tokens_per_period: Option<u64>,
    /// D6: `daily`, `weekly` (ISO week) or `monthly` (default), in UTC.
    pub budget_period: BudgetPeriod,
    /// D12: `[[limits.org]]`, TOML only (figment's environment provider cannot express an array
    /// of tables). With `backend = "redis"`, every replica must carry the same list.
    pub org: Vec<OrgLimitsConfig>,
}

/// One `[[limits.org]]` entry (D12).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OrgLimitsConfig {
    /// The org UUID in the 36-character form (the `domain::parse_org_uuid` rule).
    pub id: String,
    #[serde(default)]
    pub org_requests_per_minute: Option<u64>,
    #[serde(default)]
    pub tokens_per_period: Option<u64>,
    /// D11: no org rate and no budget; the principal rate still applies.
    #[serde(default)]
    pub exempt: bool,
}

/// D17: which limit store backs `[limits]`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LimitsBackend {
    #[default]
    Memory,
    Redis,
}

/// D11/D23: `0` refuses every request (never what an operator means), and a value above the cap
/// would break the D23 arithmetic bound.
fn check_limit(name: &str, value: Option<u64>, cap: u64) -> Result<(), String> {
    match value {
        Some(0) => Err(format!("{name} must be at least 1 (unset it for no limit; 0 would refuse every request)")),
        Some(v) if v > cap => Err(format!("{name} must be at most {cap}; got {v}")),
        _ => Ok(()),
    }
}

impl LimitsConfig {
    /// Every `[limits]` rule of spec § 4.1. Each message names the key.
    pub fn validate(&self) -> Result<(), String> {
        check_limit("limits.principal_requests_per_minute", self.principal_requests_per_minute, MAX_REQUESTS_PER_MINUTE)?;
        check_limit("limits.org_requests_per_minute", self.org_requests_per_minute, MAX_REQUESTS_PER_MINUTE)?;
        check_limit("limits.tokens_per_period", self.tokens_per_period, MAX_TOKENS_PER_PERIOD)?;

        let mut seen = HashSet::new();
        for (i, entry) in self.org.iter().enumerate() {
            let Some(id) = parse_org_uuid(&entry.id) else {
                return Err(format!("limits.org[{i}].id must be one organization UUID in the 36-character form; got {:?}", entry.id));
            };
            if !seen.insert(id) {
                return Err(format!("limits.org[{i}].id {:?} names an org that an earlier [[limits.org]] entry already lists", entry.id));
            }
            check_limit(&format!("limits.org[{i}].org_requests_per_minute"), entry.org_requests_per_minute, MAX_REQUESTS_PER_MINUTE)?;
            check_limit(&format!("limits.org[{i}].tokens_per_period"), entry.tokens_per_period, MAX_TOKENS_PER_PERIOD)?;
            if entry.exempt && (entry.org_requests_per_minute.is_some() || entry.tokens_per_period.is_some()) {
                return Err(format!("limits.org[{i}] sets exempt = true together with a limit; an exempt org has no org rate and no budget"));
            }
        }

        let url_set = self.redis_url.as_ref().is_some_and(|url| !url.expose_secret().trim().is_empty());
        match self.backend {
            LimitsBackend::Redis if !url_set => Err("limits.backend = \"redis\" requires limits.redis_url (set GATEWAY_LIMITS__REDIS_URL)".to_string()),
            LimitsBackend::Memory if self.redis_url.is_some() => Err("limits.redis_url is set but limits.backend is \"memory\": the counts would not be shared; set limits.backend = \"redis\" or unset the URL".to_string()),
            _ => Ok(()),
        }
    }

    /// The domain rules. Call after `validate`: an entry with a bad id is skipped, and a `0` value
    /// reads as "no limit" rather than panicking.
    pub fn rules(&self) -> LimitRules {
        LimitRules {
            principal_requests_per_minute: self.principal_requests_per_minute.and_then(NonZeroU64::new),
            org_requests_per_minute: self.org_requests_per_minute.and_then(NonZeroU64::new),
            tokens_per_period: self.tokens_per_period.and_then(NonZeroU64::new),
            budget_period: self.budget_period,
            overrides: self
                .org
                .iter()
                .filter_map(|entry| {
                    parse_org_uuid(&entry.id).map(|id| {
                        (
                            id,
                            OrgOverride {
                                org_requests_per_minute: entry.org_requests_per_minute.and_then(NonZeroU64::new),
                                tokens_per_period: entry.tokens_per_period.and_then(NonZeroU64::new),
                                exempt: entry.exempt,
                            },
                        )
                    })
                })
                .collect(),
        }
    }
}
```

In `GatewayConfig::validate`, before the final `Ok(())`:

```rust
        if let Some(limits) = &self.limits {
            limits.validate()?;
        }
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cd rs && cargo fmt && cargo nextest run --locked -p paigasus-gateway config:: && cargo clippy --locked -p paigasus-gateway --all-targets -- -D warnings
```

Expected: PASS, and every existing config test still passes.

- [ ] **Step 5: Commit**

```bash
git add rs/crates/services/paigasus-gateway/src/config.rs
git commit -m "feat(rs): add the gateway [limits] config table

backend, a redacted redis_url, the three limits with their D23 caps,
budget_period and [[limits.org]] overrides, with validation that names
each key and rejects a misspelt one (SMA-677).

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: The two `GatewayError` refusals

**Files:**
- Modify: `rs/crates/services/paigasus-gateway/src/adapters/http/error.rs` (enum at lines 51-105; `parts` at 137-208; `retryable` at 214-228; `into_response` at 231-246; tests)

**Interfaces:**
- Consumes: Task 1's registry codes; Task 3's `BudgetPeriod`, `LimitRefusal`.
- Produces: `GatewayError::RateLimited { retry_after_secs: u32 }` (429, `type: "requests"`, code `rate-limited`, `Retry-After`, `paigasus-retryable: true`, `x-should-retry: true`); `GatewayError::BudgetExhausted { resets_at_unix: i64, period: BudgetPeriod }` (429, `type: "insufficient_quota"`, code `budget-exhausted`, `paigasus-retryable: false`, `x-should-retry: false`); `impl From<LimitRefusal> for GatewayError` (`Unscoped` → `Internal`); `pub const SHOULD_RETRY_HEADER: &str = "x-should-retry"`.

`GatewayError` stays `Copy` and keeps `strum::EnumIter` in tests, so the new fields are `Copy + Default` (spec § 4.5). `parts()` keeps returning `&'static str`; a new private `message()` formats the budget message.

- [ ] **Step 1: Confirm the OpenAI `type` values (Q8)**

Open the OpenAI API reference (error codes guide, "429 - Rate limit reached for requests" and "429 - You exceeded your current quota"). Confirm that a request-rate 429 body uses `"type": "requests"` and a quota 429 body uses `"type": "insufficient_quota"`. If either differs, STOP and report the documented value to the coordinator (Q8 is Sven's decision). Record the URL and the date you read it in the task report.

- [ ] **Step 2: Write the failing tests**

Add to the `tests` module of `src/adapters/http/error.rs`:

```rust
    use crate::domain::limits::{BudgetPeriod, LimitRefusal, RefusalReason};

    /// 2026-11-01T00:00:00Z.
    const NOV_1: i64 = 1_793_491_200;

    #[tokio::test]
    async fn a_rate_refusal_is_429_requests_with_retry_after() {
        let resp = GatewayError::RateLimited { retry_after_secs: 17 }.into_response();
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(resp.headers()["retry-after"], "17");
        assert_eq!(resp.headers()["paigasus-retryable"], "true");
        assert_eq!(resp.headers()["x-should-retry"], "true");
        let body = body_json(resp).await;
        assert_eq!(body["error"]["type"], "requests");
        assert_eq!(body["error"]["code"], "rate-limited");
        assert!(body["error"]["param"].is_null());
    }

    #[tokio::test]
    async fn retry_after_is_never_zero() {
        let resp = GatewayError::RateLimited { retry_after_secs: 0 }.into_response();
        assert_eq!(resp.headers()["retry-after"], "1");
    }

    #[tokio::test]
    async fn a_budget_refusal_is_429_insufficient_quota_and_not_retryable() {
        let resp = GatewayError::BudgetExhausted { resets_at_unix: NOV_1, period: BudgetPeriod::Monthly }.into_response();
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(resp.headers()["paigasus-retryable"], "false");
        assert_eq!(resp.headers()["x-should-retry"], "false");
        assert!(resp.headers().get("retry-after").is_none(), "a retry in seconds cannot succeed");
        let body = body_json(resp).await;
        assert_eq!(body["error"]["type"], "insufficient_quota");
        assert_eq!(body["error"]["code"], "budget-exhausted");
        assert_eq!(body["error"]["message"], "The organization token budget for 2026-10 is used up. It resets at 2026-11-01T00:00:00Z.");
    }

    #[tokio::test]
    async fn the_budget_message_names_the_period_kind() {
        let weekly = body_json(GatewayError::BudgetExhausted { resets_at_unix: 1_791_158_400, period: BudgetPeriod::Weekly }.into_response()).await;
        assert_eq!(weekly["error"]["message"], "The organization token budget for 2026-W40 is used up. It resets at 2026-10-05T00:00:00Z.");
        let daily = body_json(GatewayError::BudgetExhausted { resets_at_unix: 1_790_985_600, period: BudgetPeriod::Daily }.into_response()).await;
        assert_eq!(daily["error"]["message"], "The organization token budget for 2026-10-02 is used up. It resets at 2026-10-03T00:00:00Z.");
    }

    #[test]
    fn a_limit_refusal_maps_one_to_one() {
        assert_eq!(GatewayError::from(LimitRefusal::RateLimited { retry_after_secs: 3, reason: RefusalReason::OrgRate }), GatewayError::RateLimited { retry_after_secs: 3 });
        assert_eq!(
            GatewayError::from(LimitRefusal::BudgetExhausted { period: BudgetPeriod::Daily, resets_at_unix: NOV_1 }),
            GatewayError::BudgetExhausted { resets_at_unix: NOV_1, period: BudgetPeriod::Daily }
        );
        assert_eq!(GatewayError::from(LimitRefusal::Unscoped), GatewayError::Internal, "D2/Q11: fail-closed as 500 internal");
    }

    #[tokio::test]
    async fn only_the_two_refusals_carry_x_should_retry() {
        use strum::IntoEnumIterator;
        for err in GatewayError::iter() {
            let has = err.into_response().headers().contains_key("x-should-retry");
            assert_eq!(has, matches!(err, GatewayError::RateLimited { .. } | GatewayError::BudgetExhausted { .. }), "{err:?}");
        }
    }
```

In `each_case_maps_to_its_bound_status`, add:

```rust
        assert_eq!(GatewayError::RateLimited { retry_after_secs: 1 }.into_response().status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(GatewayError::BudgetExhausted { resets_at_unix: 0, period: BudgetPeriod::Monthly }.into_response().status(), StatusCode::TOO_MANY_REQUESTS);
```

In `retryability_matches_the_documented_table`, replace the `match` with:

```rust
            let want = match err {
                GatewayError::IamUnavailable | GatewayError::UpstreamUnavailable | GatewayError::UpstreamTimeout | GatewayError::RateLimited { .. } => Retryable::Yes,
                GatewayError::Internal | GatewayError::MissingScope => Retryable::Unknown,
                GatewayError::BudgetExhausted { .. } => Retryable::No,
                _ => Retryable::No,
            };
```

- [ ] **Step 3: Run them to verify they fail**

```bash
cd rs && cargo nextest run --locked -p paigasus-gateway adapters::http::error
```

Expected: compile FAIL (`no variant named RateLimited`).

- [ ] **Step 4: Write the implementation**

Imports at the top of `error.rs`:

```rust
use axum::Json;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, SecondsFormat, Utc};
use paigasus_observability::Retryable;
use paigasus_observability::correlation::RETRYABLE_HEADER;
use serde::Serialize;

use crate::adapters::openai::OpenAiError;
use crate::domain::limits::{BudgetPeriod, LimitRefusal};

/// D8: the OpenAI SDKs retry a 429 twice unless this header says `false`, and they ignore
/// `paigasus-retryable`. Sent only on the two limit refusals.
pub const SHOULD_RETRY_HEADER: &str = "x-should-retry";
```

Add at the end of `enum GatewayError`:

```rust
    // ---- limits (SMA-677) --------------------------------------------------------------------
    /// The principal or its org is over its request rate (D3) → 429, `type: "requests"`,
    /// `Retry-After`, `x-should-retry: true`. Retryable.
    RateLimited { retry_after_secs: u32 },
    /// The org used its token budget for the current period (D6) → 429,
    /// `type: "insufficient_quota"`, `x-should-retry: false`. Not retryable: a retry in seconds
    /// cannot succeed, and each retry costs 2-3 IAM RPCs before the refusal.
    BudgetExhausted { resets_at_unix: i64, period: BudgetPeriod },
```

After `impl From<crate::domain::OrgResolutionError> for GatewayError`:

```rust
/// Map a limit refusal (SMA-677 § 4.5). `Unscoped` is the D2 fail-closed case: a scope with no
/// org is a parse mismatch or an IAM defect, so it is a plain `500 internal`.
impl From<LimitRefusal> for GatewayError {
    fn from(refusal: LimitRefusal) -> Self {
        match refusal {
            LimitRefusal::RateLimited { retry_after_secs, .. } => GatewayError::RateLimited { retry_after_secs },
            LimitRefusal::BudgetExhausted { period, resets_at_unix } => GatewayError::BudgetExhausted { resets_at_unix, period },
            LimitRefusal::Unscoped => GatewayError::Internal,
        }
    }
}
```

In `parts()`, add two arms before the closing brace of the `match`:

```rust
            GatewayError::RateLimited { .. } => (
                StatusCode::TOO_MANY_REQUESTS,
                "requests",
                Some("rate-limited"),
                None,
                "Too many requests. Wait for the number of seconds in the Retry-After header, then try again.",
            ),
            GatewayError::BudgetExhausted { .. } => (
                StatusCode::TOO_MANY_REQUESTS,
                "insufficient_quota",
                Some("budget-exhausted"),
                None,
                "The organization token budget for this period is used up.",
            ),
```

Add a method in `impl GatewayError`:

```rust
    /// The caller-safe message. Static for every case except the budget refusal, which names the
    /// period and the reset instant (D8) — never a count and never the org id.
    fn message(self) -> String {
        match self {
            GatewayError::BudgetExhausted { resets_at_unix, period } => {
                let resets_at = DateTime::<Utc>::from_timestamp(resets_at_unix, 0).map(|t| t.to_rfc3339_opts(SecondsFormat::Secs, true)).unwrap_or_default();
                format!("The organization token budget for {} is used up. It resets at {resets_at}.", period.label_at_reset(resets_at_unix))
            }
            other => other.parts().4.to_owned(),
        }
    }
```

In `retryable()`:

```rust
            Self::IamUnavailable | Self::UpstreamUnavailable | Self::UpstreamTimeout | Self::RateLimited { .. } => Retryable::Yes,
```

and add `| Self::BudgetExhausted { .. }` to the `Retryable::No` arm.

Replace `into_response`:

```rust
impl IntoResponse for GatewayError {
    fn into_response(self) -> Response {
        let (status, r#type, code, param, _) = self.parts();
        let envelope = ErrorEnvelope {
            error: ErrorBody {
                message: self.message(),
                r#type: r#type.to_owned(),
                param: param.map(str::to_owned),
                code: code.map(str::to_owned),
            },
        };
        let mut response = (status, Json(envelope)).into_response();
        let headers = response.headers_mut();
        headers.insert(RETRYABLE_HEADER, HeaderValue::from_static(self.retryable().as_wire()));
        match self {
            GatewayError::RateLimited { retry_after_secs } => {
                headers.insert(header::RETRY_AFTER, HeaderValue::from(retry_after_secs.max(1)));
                headers.insert(SHOULD_RETRY_HEADER, HeaderValue::from_static("true"));
            }
            GatewayError::BudgetExhausted { .. } => {
                headers.insert(SHOULD_RETRY_HEADER, HeaderValue::from_static("false"));
            }
            _ => {}
        }
        response
    }
}
```

- [ ] **Step 5: Run the tests to verify they pass**

```bash
cd rs && cargo fmt && cargo nextest run --locked -p paigasus-gateway adapters::http::error && cargo clippy --locked -p paigasus-gateway --all-targets -- -D warnings
moon run repo:error-code-single-site
```

Expected: PASS. `every_gateway_code_is_declared_in_the_canonical_registry` now covers both new codes (it iterates `GatewayError::iter()`). `repo:error-code-single-site` passes with no `MANIFEST` change: `error.rs` is already an `emits` row.

- [ ] **Step 6: Commit**

```bash
git add rs/crates/services/paigasus-gateway/src/adapters/http/error.rs
git commit -m "feat(rs): add the gateway rate-limited and budget-exhausted refusals

Two 429 GatewayError cases with the D8 types, Retry-After and the
x-should-retry header that the OpenAI SDKs read (SMA-677).

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: The limits application service and the charge guard

**Files:**
- Create: `rs/crates/services/paigasus-gateway/src/application/mod.rs`, `src/application/limits.rs`, `src/application/charge_guard.rs`, `src/test_support.rs`
- Modify: `rs/crates/services/paigasus-gateway/src/lib.rs` (`pub mod application;`, `#[cfg(test)] mod test_support;`)
- Modify: `rs/crates/services/paigasus-gateway/Cargo.toml` (dev-dependency `metrics-util`)

**Interfaces:**
- Consumes: Task 3's domain types; Task 4's metric names; `crate::domain::{CallerContext, org_of}`.
- Produces:
  - `paigasus_gateway::application::limits::Limits` with `fn new(rules: LimitRules, store: Arc<dyn LimitStore>, clock: Arc<dyn Clock>) -> Limits` and `async fn admit(self: &Arc<Self>, caller: &CallerContext) -> Result<ChargeGuard, LimitRefusal>`
  - `enum StoreOp { Check, Charge }` (`as_label`: `check`, `charge`); `enum StoreBackend { Memory, Redis }`; `const LIMITS_BREAKER_ROLE: &str = "limits"`; `const STORE_LOG_INTERVAL: Duration` (10 s)
  - `struct StoreLogLimiter` with `fn new(interval: Duration)`, `fn admit_at(&self, op: StoreOp, kind: UnavailableKind, now: Instant) -> Option<u64>`
  - `fn report_store_unavailable(op: StoreOp, kind: UnavailableKind, detail: &str)` (metric + rate-limited log; used by the Redis adapter in Task 11)
  - `fn prime_metrics(backend: StoreBackend)`
  - `paigasus_gateway::application::charge_guard::{ChargeGuard, ChargeSource, NoTicket}`; `ChargeGuard` methods: `has_ticket(&self) -> bool`, `set_request_estimate(&mut self, tokens: u64)`, `set_ids(&mut self, ids: Option<RequestIds>)`, `mark_sent(&mut self)`, `mark_no_charge(&mut self)`, `set_reported(&mut self, total_tokens: u64)`, `set_completion_estimate(&mut self, tokens: u64)`, `set_stream_progress(&mut self, completion_records: u64, reported: Option<u64>)`, `tokens(&self) -> (u64, ChargeSource)`; `pub(crate) fn new(limits: Arc<Limits>, ticket: Option<LimitTicket>, org: Option<Uuid>, no_ticket: NoTicket) -> ChargeGuard`
  - `crate::test_support` (cfg(test) only): `counter`, `gauge`, `at`, `FixedClock`, `ScriptedStore`, `caller`, `ORG_A`, `ORG_A_PRN`, `TEAM_IN_A`

The A4 charge rules, as `ChargeGuard::tokens` implements them: never sent (`Admitted`) or `NoCharge` (a connect failure, a non-`2xx` answer) → 0. Sent with a usage record → `total_tokens` (`reported`). Sent with no usage (a `2xx` without usage, a timeout, a transport failure, a disconnect) → request estimate + completion estimate (`estimated`). Every charge is clamped to `MAX_TOKENS_PER_CHARGE` (D23). The guard charges only with a ticket and more than 0 tokens, exactly once, in `Drop`.

- [ ] **Step 1: Add the dev-dependency and the shared test helpers**

`rs/crates/services/paigasus-gateway/Cargo.toml`, in `[dev-dependencies]`:

```toml
# SMA-677: unit tests read the limit metrics from a local DebuggingRecorder instead of the
# process-global Prometheus recorder. Same line as paigasus-iam's.
metrics-util = { version = "0.20.4", default-features = false, features = ["debugging"] }
```

Create `src/test_support.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0

//! Unit-test helpers shared by `application` and `adapters::limits` (cfg(test) only).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use metrics_util::debugging::{DebugValue, Snapshotter};
use uuid::Uuid;

use crate::domain::limits::{Clock, LimitDecision, LimitPolicy, LimitStore, LimitStoreError, LimitTicket};
use crate::domain::{CallerContext, Credential};

pub(crate) const ORG_A: Uuid = Uuid::from_u128(0x0190a100_0000_7000_8000_0000000000a1);
pub(crate) const ORG_A_PRN: &str = "prn:pgs:iam:::organization/0190a100-0000-7000-8000-0000000000a1";
pub(crate) const TEAM_IN_A: &str = "prn:pgs:iam::0190a100-0000-7000-8000-0000000000a1:team/0190a1b2-0000-7000-8000-0000000000c3";

fn value(snapshotter: &Snapshotter, name: &str, labels: &[(&str, &str)]) -> Option<DebugValue> {
    snapshotter.snapshot().into_vec().into_iter().find_map(|(key, _, _, value)| {
        let key = key.key();
        let have: Vec<(String, String)> = key.labels().map(|l| (l.key().to_owned(), l.value().to_owned())).collect();
        let same = key.name() == name && have.len() == labels.len() && labels.iter().all(|(k, v)| have.iter().any(|(hk, hv)| hk == k && hv == v));
        same.then_some(value)
    })
}

/// The value of the counter `name` with exactly `labels`, or `None` when it was never emitted.
pub(crate) fn counter(snapshotter: &Snapshotter, name: &str, labels: &[(&str, &str)]) -> Option<u64> {
    match value(snapshotter, name, labels)? {
        DebugValue::Counter(n) => Some(n),
        _ => None,
    }
}

pub(crate) fn gauge(snapshotter: &Snapshotter, name: &str, labels: &[(&str, &str)]) -> Option<f64> {
    match value(snapshotter, name, labels)? {
        DebugValue::Gauge(v) => Some(v.into_inner()),
        _ => None,
    }
}

pub(crate) fn at(rfc3339: &str) -> SystemTime {
    SystemTime::from(chrono::DateTime::parse_from_rfc3339(rfc3339).expect("a fixture instant"))
}

pub(crate) struct FixedClock(pub(crate) SystemTime);

impl Clock for FixedClock {
    fn now(&self) -> SystemTime {
        self.0
    }
}

/// A store that answers every check with `reply()` and records what it was asked.
pub(crate) struct ScriptedStore {
    reply: fn() -> Result<LimitDecision, LimitStoreError>,
    pub(crate) checks: AtomicUsize,
    pub(crate) orgs: Mutex<Vec<Option<Uuid>>>,
    pub(crate) charges: Mutex<Vec<u64>>,
}

impl ScriptedStore {
    pub(crate) fn new(reply: fn() -> Result<LimitDecision, LimitStoreError>) -> Arc<Self> {
        Arc::new(ScriptedStore { reply, checks: AtomicUsize::new(0), orgs: Mutex::new(Vec::new()), charges: Mutex::new(Vec::new()) })
    }

    pub(crate) fn charges(&self) -> Vec<u64> {
        self.charges.lock().expect("not poisoned").clone()
    }
}

#[async_trait::async_trait]
impl LimitStore for ScriptedStore {
    async fn check_and_admit(&self, _principal: &str, org: Option<Uuid>, _policy: &LimitPolicy, _now: SystemTime) -> Result<LimitDecision, LimitStoreError> {
        self.checks.fetch_add(1, Ordering::SeqCst);
        self.orgs.lock().expect("not poisoned").push(org);
        (self.reply)()
    }

    fn charge(&self, _ticket: LimitTicket, tokens: u64, _now: SystemTime) {
        self.charges.lock().expect("not poisoned").push(tokens);
    }
}

/// An API-key caller in `scope`.
pub(crate) fn caller(scope: &str) -> CallerContext {
    CallerContext {
        principal_prn: "prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-0000000000e0".to_owned(),
        scope_prn: scope.to_owned(),
        credential: Credential::ApiKey { key_id: "key-1".to_owned() },
    }
}
```

`src/lib.rs`, after `pub mod adapters;`:

```rust
pub mod application;
```

and after the last `pub mod` line:

```rust
#[cfg(test)]
mod test_support;
```

Create `src/application/mod.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0

//! The application layer (SMA-677 § 4.3a): the use cases between the HTTP adapter and the domain.
//! Flat files with no subdirectories, like `paigasus-iam/src/application/`.

pub mod charge_guard;
pub mod limits;
```

- [ ] **Step 2: Write the failing tests for the service**

Create `src/application/limits.rs` with only the doc comment and this test module (the implementation follows in Step 4):

```rust
// SPDX-License-Identifier: Apache-2.0

//! The limits service (SMA-677 § 4.3a). The chat handler calls `Limits::admit` once, just before
//! egress (D9). It resolves the org (D2), builds the policy, skips the store for an empty policy
//! (D11), calls the store, applies the D8 precedence, counts the refusal, and returns a
//! `ChargeGuard`. A store failure admits the request with no ticket (fail-open, D10).

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::limits::{BudgetPeriod, FailedCheck, OrgOverride, RateCounts, RefusalReason};
    use crate::test_support::{FixedClock, ORG_A, ORG_A_PRN, ScriptedStore, TEAM_IN_A, at, caller, counter, gauge};
    use metrics_util::debugging::DebuggingRecorder;
    use std::collections::HashMap;
    use std::num::NonZeroU64;
    use std::sync::atomic::Ordering;

    fn nz(n: u64) -> NonZeroU64 {
        NonZeroU64::new(n).expect("non-zero")
    }

    fn limits(rules: LimitRules, store: Arc<ScriptedStore>) -> Arc<Limits> {
        Arc::new(Limits::new(rules, store, Arc::new(FixedClock(at("2026-10-02T12:00:00Z")))))
    }

    fn budget_rules() -> LimitRules {
        LimitRules { tokens_per_period: Some(nz(100)), ..LimitRules::default() }
    }

    fn admit_without_ticket() -> Result<LimitDecision, LimitStoreError> {
        Ok(LimitDecision::Admit(None))
    }

    #[tokio::test]
    async fn an_empty_policy_never_calls_the_store() {
        let store = ScriptedStore::new(admit_without_ticket);
        let rules = LimitRules { overrides: HashMap::from([(ORG_A, OrgOverride { exempt: true, ..OrgOverride::default() })]), tokens_per_period: Some(nz(5)), ..LimitRules::default() };
        let guard = limits(rules, store.clone()).admit(&caller(ORG_A_PRN)).await.expect("admitted");
        assert!(!guard.has_ticket());
        assert_eq!(store.checks.load(Ordering::SeqCst), 0, "D11: an exempt org with no principal rate makes no store call");
    }

    #[tokio::test]
    async fn the_store_sees_the_org_of_a_team_scope() {
        let store = ScriptedStore::new(admit_without_ticket);
        limits(budget_rules(), store.clone()).admit(&caller(TEAM_IN_A)).await.expect("admitted");
        assert_eq!(*store.orgs.lock().expect("not poisoned"), vec![Some(ORG_A)], "D2: a team scope counts against its org");
    }

    #[tokio::test]
    async fn a_refusal_counts_one_reason_by_the_d8_precedence() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _local = metrics::set_default_local_recorder(&recorder);
        let store = ScriptedStore::new(|| {
            let counts = RateCounts { previous: 0, current: 2, elapsed_ms: 0, limit: 2 };
            Ok(LimitDecision::Refused(vec![FailedCheck::OrgRate(counts), FailedCheck::PrincipalRate(counts)]))
        });
        let refusal = limits(LimitRules { principal_requests_per_minute: Some(nz(2)), org_requests_per_minute: Some(nz(2)), ..LimitRules::default() }, store)
            .admit(&caller(ORG_A_PRN))
            .await
            .expect_err("refused");
        assert!(matches!(refusal, LimitRefusal::RateLimited { reason: RefusalReason::PrincipalRate, .. }), "{refusal:?}");
        assert_eq!(counter(&snapshotter, names::GATEWAY_LIMIT_REFUSALS_TOTAL, &[("reason", "principal_rate")]), Some(1));
        assert_eq!(counter(&snapshotter, names::GATEWAY_LIMIT_REFUSALS_TOTAL, &[("reason", "org_rate")]), None, "only one refusal is counted");
    }

    #[tokio::test]
    async fn a_budget_refusal_wins_and_is_counted_as_org_budget() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _local = metrics::set_default_local_recorder(&recorder);
        let store = ScriptedStore::new(|| Ok(LimitDecision::Refused(vec![FailedCheck::Budget { period: BudgetPeriod::Monthly, resets_at_unix: 1_793_491_200 }])));
        let refusal = limits(budget_rules(), store).admit(&caller(ORG_A_PRN)).await.expect_err("refused");
        assert_eq!(refusal, LimitRefusal::BudgetExhausted { period: BudgetPeriod::Monthly, resets_at_unix: 1_793_491_200 });
        assert_eq!(counter(&snapshotter, names::GATEWAY_LIMIT_REFUSALS_TOTAL, &[("reason", "org_budget")]), Some(1));
    }

    /// A12/D10: a failed check admits with no ticket and counts `op="check"` with its kind.
    #[tokio::test]
    async fn a_store_failure_admits_without_a_ticket_and_counts_the_kind() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _local = metrics::set_default_local_recorder(&recorder);
        let store = ScriptedStore::new(|| Err(LimitStoreError::Unavailable { kind: UnavailableKind::Server, detail: "OOM command not allowed".to_owned() }));
        let guard = limits(budget_rules(), store).admit(&caller(ORG_A_PRN)).await.expect("fail-open admits");
        assert!(!guard.has_ticket(), "no ticket, so no charge");
        assert_eq!(counter(&snapshotter, names::GATEWAY_LIMIT_STORE_UNAVAILABLE_TOTAL, &[("op", "check"), ("kind", "server")]), Some(1));
    }

    /// D2/Q11: a scope with no org is refused when an org dimension applies …
    #[tokio::test]
    async fn an_unscoped_request_is_refused_when_a_budget_applies() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _local = metrics::set_default_local_recorder(&recorder);
        let store = ScriptedStore::new(admit_without_ticket);
        let refusal = limits(budget_rules(), store.clone()).admit(&caller("prn:paigasus:iam:default:scope/team-a")).await.expect_err("fail-closed");
        assert_eq!(refusal, LimitRefusal::Unscoped);
        assert_eq!(store.checks.load(Ordering::SeqCst), 0);
        assert_eq!(counter(&snapshotter, names::GATEWAY_LIMIT_UNSCOPED_REQUESTS_TOTAL, &[]), Some(1));
    }

    /// … and admitted under the principal rate alone when no org dimension applies.
    #[tokio::test]
    async fn an_unscoped_request_keeps_the_principal_rate() {
        let store = ScriptedStore::new(admit_without_ticket);
        let rules = LimitRules { principal_requests_per_minute: Some(nz(5)), ..LimitRules::default() };
        limits(rules, store.clone()).admit(&caller("not-a-prn")).await.expect("admitted");
        assert_eq!(*store.orgs.lock().expect("not poisoned"), vec![None]);
    }

    #[test]
    fn the_store_log_limiter_admits_one_line_per_interval_and_counts_the_rest() {
        let limiter = StoreLogLimiter::new(Duration::from_secs(10));
        let t0 = Instant::now();
        assert_eq!(limiter.admit_at(StoreOp::Check, UnavailableKind::Io, t0), Some(0));
        assert_eq!(limiter.admit_at(StoreOp::Check, UnavailableKind::Io, t0 + Duration::from_secs(1)), None);
        assert_eq!(limiter.admit_at(StoreOp::Check, UnavailableKind::Io, t0 + Duration::from_secs(2)), None);
        // Another (operation, kind) is independent.
        assert_eq!(limiter.admit_at(StoreOp::Charge, UnavailableKind::Io, t0 + Duration::from_secs(2)), Some(0));
        assert_eq!(limiter.admit_at(StoreOp::Check, UnavailableKind::Server, t0 + Duration::from_secs(2)), Some(0));
        assert_eq!(limiter.admit_at(StoreOp::Check, UnavailableKind::Io, t0 + Duration::from_secs(11)), Some(2));
    }

    #[test]
    fn io_logs_at_warn_and_server_logs_at_error() {
        let (logs, _guard) = paigasus_logging::test_support::capture_logs_at(tracing::Level::WARN);
        let limiter = StoreLogLimiter::new(STORE_LOG_INTERVAL);
        let now = Instant::now();
        report_store_unavailable_with(&limiter, now, StoreOp::Check, UnavailableKind::Io, "connection refused");
        report_store_unavailable_with(&limiter, now, StoreOp::Charge, UnavailableKind::Server, "OOM command not allowed");
        report_store_unavailable_with(&limiter, now, StoreOp::Charge, UnavailableKind::Server, "suppressed");
        let text = logs.text();
        let io = text.lines().find(|l| l.contains("connection refused")).expect("the io line");
        assert!(io.contains("WARN"), "{io}");
        let server = text.lines().find(|l| l.contains("OOM command not allowed")).expect("the server line");
        assert!(server.contains("ERROR"), "{server}");
        assert!(!text.contains("suppressed"), "a second line inside 10 s is suppressed");
    }

    /// A7: every series of § 4.8 at zero; the breaker series only for Redis.
    #[test]
    fn prime_metrics_primes_every_series_at_zero() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        metrics::with_local_recorder(&recorder, || prime_metrics(StoreBackend::Redis));
        for reason in ["principal_rate", "org_rate", "org_budget"] {
            assert_eq!(counter(&snapshotter, names::GATEWAY_LIMIT_REFUSALS_TOTAL, &[("reason", reason)]), Some(0), "{reason}");
        }
        for source in ["reported", "estimated"] {
            assert_eq!(counter(&snapshotter, names::GATEWAY_TOKENS_CHARGED_TOTAL, &[("source", source)]), Some(0), "{source}");
        }
        assert_eq!(counter(&snapshotter, names::GATEWAY_LIMIT_UNSCOPED_REQUESTS_TOTAL, &[]), Some(0));
        for op in ["check", "charge"] {
            for kind in ["io", "server", "decode"] {
                assert_eq!(counter(&snapshotter, names::GATEWAY_LIMIT_STORE_UNAVAILABLE_TOTAL, &[("op", op), ("kind", kind)]), Some(0), "{op}/{kind}");
            }
        }
        for reason in ["no_runtime", "shutdown", "period_expired"] {
            assert_eq!(counter(&snapshotter, names::GATEWAY_LIMIT_CHARGES_DROPPED_TOTAL, &[("reason", reason)]), Some(0), "{reason}");
        }
        assert_eq!(gauge(&snapshotter, names::GATEWAY_REDIS_BREAKER_STATE, &[("role", "limits")]), Some(0.0));
        for to in ["closed", "half_open", "open"] {
            assert_eq!(counter(&snapshotter, names::GATEWAY_REDIS_BREAKER_TRANSITIONS_TOTAL, &[("role", "limits"), ("to", to)]), Some(0), "{to}");
        }
    }

    #[test]
    fn prime_metrics_for_memory_has_no_breaker_series() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        metrics::with_local_recorder(&recorder, || prime_metrics(StoreBackend::Memory));
        assert_eq!(counter(&snapshotter, names::GATEWAY_LIMIT_UNSCOPED_REQUESTS_TOTAL, &[]), Some(0));
        assert_eq!(gauge(&snapshotter, names::GATEWAY_REDIS_BREAKER_STATE, &[("role", "limits")]), None);
        assert_eq!(counter(&snapshotter, names::GATEWAY_REDIS_BREAKER_TRANSITIONS_TOTAL, &[("role", "limits"), ("to", "open")]), None);
    }
}
```

- [ ] **Step 3: Write the failing tests for the guard**

Create `src/application/charge_guard.rs` with the doc comment and this test module:

```rust
// SPDX-License-Identifier: Apache-2.0

//! The charge guard (SMA-677 § 4.6, A4). One guard per admitted request. Its `Drop` charges the
//! request at most once, on the stream path and on the non-stream path, also when the client
//! disconnects (axum then drops the handler future or the response body, and the guard with it).
//! `Drop` never panics (D10): a panic during unwinding aborts the process.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::limits::MemoryLimitStore;
    use crate::domain::limits::{BudgetPeriod, LimitDecision, LimitPolicy, LimitRules, LimitStore, LimitStoreError, LimitTicket};
    use crate::test_support::{FixedClock, ORG_A, ScriptedStore, at, counter};
    use metrics_util::debugging::DebuggingRecorder;
    use std::num::NonZeroU64;

    fn ticket() -> LimitTicket {
        LimitTicket { org: ORG_A, period: BudgetPeriod::Monthly.key_at(at("2026-10-02T12:00:00Z")) }
    }

    fn admit() -> Result<LimitDecision, LimitStoreError> {
        Ok(LimitDecision::Admit(None))
    }

    fn guard_with(store: Arc<dyn LimitStore>, ticket: Option<LimitTicket>, no_ticket: NoTicket) -> ChargeGuard {
        let limits = Arc::new(Limits::new(LimitRules::default(), store, Arc::new(FixedClock(at("2026-10-02T12:00:00Z")))));
        ChargeGuard::new(limits, ticket, Some(ORG_A), no_ticket)
    }

    #[test]
    fn a_reported_usage_is_charged_once_and_counted() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let store = ScriptedStore::new(admit);
        metrics::with_local_recorder(&recorder, || {
            let mut guard = guard_with(store.clone(), Some(ticket()), NoTicket::NoBudget);
            guard.set_request_estimate(9);
            guard.mark_sent();
            guard.set_reported(42);
        });
        assert_eq!(store.charges(), vec![42]);
        assert_eq!(counter(&snapshotter, names::GATEWAY_TOKENS_CHARGED_TOTAL, &[("source", "reported")]), Some(42));
    }

    #[test]
    fn the_tokens_follow_the_a4_rules() {
        let store = ScriptedStore::new(admit);
        let mut g = guard_with(store.clone(), Some(ticket()), NoTicket::NoBudget);
        g.set_request_estimate(10);
        assert_eq!(g.tokens(), (0, ChargeSource::Estimated), "never sent: zero");
        g.mark_sent();
        assert_eq!(g.tokens(), (10, ChargeSource::Estimated), "a timeout or a disconnect: the request estimate");
        g.set_completion_estimate(5);
        assert_eq!(g.tokens(), (15, ChargeSource::Estimated), "a 2xx without usage: request + completion");
        g.set_stream_progress(7, None);
        assert_eq!(g.tokens(), (17, ChargeSource::Estimated), "the stream's record count replaces the completion estimate");
        g.set_stream_progress(8, Some(30));
        assert_eq!(g.tokens(), (30, ChargeSource::Reported), "a usage record wins");
        g.mark_no_charge();
        assert_eq!(g.tokens(), (0, ChargeSource::Estimated), "a connect failure or a non-2xx: zero");
        drop(g);
        assert!(store.charges().is_empty(), "zero tokens charge nothing");
    }

    #[test]
    fn a_guard_without_a_ticket_never_charges() {
        let store = ScriptedStore::new(admit);
        let mut guard = guard_with(store.clone(), None, NoTicket::StoreUnavailable);
        guard.mark_sent();
        guard.set_reported(50);
        drop(guard);
        assert!(store.charges().is_empty(), "A12: fail-open requests are not charged");
    }

    /// Review Focus 5.
    #[test]
    fn an_absurd_usage_is_clamped_to_the_charge_cap() {
        let store = ScriptedStore::new(admit);
        let mut guard = guard_with(store.clone(), Some(ticket()), NoTicket::NoBudget);
        guard.mark_sent();
        guard.set_reported(u64::MAX);
        drop(guard);
        assert_eq!(store.charges(), vec![MAX_TOKENS_PER_CHARGE]);
        let mut estimated = guard_with(store.clone(), Some(ticket()), NoTicket::NoBudget);
        estimated.set_request_estimate(u64::MAX);
        estimated.mark_sent();
        estimated.set_completion_estimate(u64::MAX);
        assert_eq!(estimated.tokens(), (MAX_TOKENS_PER_CHARGE, ChargeSource::Estimated), "saturating, then clamped");
    }

    #[test]
    fn the_metered_line_names_the_outcome_and_the_ids() {
        let (logs, _g) = paigasus_logging::test_support::capture_logs_at(tracing::Level::INFO);
        let ids = paigasus_observability::RequestIds { request_id: uuid::Uuid::from_u128(1), correlation_id: uuid::Uuid::from_u128(2) };
        let store = ScriptedStore::new(admit);
        let mut charged = guard_with(store.clone(), Some(ticket()), NoTicket::NoBudget);
        charged.set_ids(Some(ids));
        charged.mark_sent();
        charged.set_reported(42);
        drop(charged);
        let mut fail_open = guard_with(store.clone(), None, NoTicket::StoreUnavailable);
        fail_open.set_request_estimate(3);
        fail_open.mark_sent();
        drop(fail_open);
        let mut zero = guard_with(store, Some(ticket()), NoTicket::NoBudget);
        zero.mark_sent();
        zero.mark_no_charge();
        drop(zero);
        let text = logs.text();
        let lines: Vec<&str> = text.lines().filter(|l| l.contains("chat completion metered")).collect();
        assert_eq!(lines.len(), 2, "a zero-token line is debug, below INFO: {text}");
        assert!(lines[0].contains("outcome=\"charged\"") && lines[0].contains("tokens=42") && lines[0].contains("source=\"reported\""), "{}", lines[0]);
        assert!(lines[0].contains("00000000-0000-0000-0000-000000000001") && lines[0].contains("00000000-0000-0000-0000-000000000002"), "the ids: {}", lines[0]);
        assert!(lines[0].contains("0190a100-0000-7000-8000-0000000000a1"), "the org: {}", lines[0]);
        assert!(lines[1].contains("outcome=\"store_unavailable\"") && lines[1].contains("tokens=3"), "{}", lines[1]);
    }

    /// Spec § 5.8, memory: a guard dropped on a plain thread charges at once (no spawn).
    #[tokio::test]
    async fn a_memory_charge_from_a_plain_thread_applies_at_once() {
        let store = Arc::new(MemoryLimitStore::new());
        let t = at("2026-10-02T12:00:00Z");
        let policy = LimitPolicy { budget: Some(crate::domain::limits::Budget { tokens: NonZeroU64::new(10).expect("non-zero"), period: BudgetPeriod::Monthly }), ..LimitPolicy::default() };
        let LimitDecision::Admit(Some(issued)) = store.check_and_admit("p", Some(ORG_A), &policy, t).await.expect("memory never fails") else {
            panic!("a budget admission issues a ticket");
        };
        let mut guard = guard_with(store.clone(), Some(issued), NoTicket::NoBudget);
        guard.mark_sent();
        guard.set_reported(10);
        std::thread::spawn(move || drop(guard)).join().expect("Drop does not panic");
        assert!(matches!(store.check_and_admit("p", Some(ORG_A), &policy, t).await.expect("memory never fails"), LimitDecision::Refused(_)), "the 10 tokens are booked");
    }
}
```

- [ ] **Step 4: Run them to verify they fail**

```bash
cd rs && cargo nextest run --locked -p paigasus-gateway application::
```

Expected: compile FAIL (`cannot find type Limits`, `ChargeGuard`, …).

- [ ] **Step 5: Write the service**

Insert above `#[cfg(test)]` in `src/application/limits.rs`:

```rust
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex, PoisonError};
use std::time::{Duration, Instant};

use metrics::{counter, gauge};
use paigasus_observability::names;

use crate::application::charge_guard::{ChargeGuard, ChargeSource, NoTicket};
use crate::domain::limits::{ChargeDropReason, Clock, LimitDecision, LimitRefusal, LimitRules, LimitStore, LimitStoreError, RefusalReason, UnavailableKind, choose_refusal};
use crate::domain::{CallerContext, org_of};

/// D10: at most one store-failure log line per (operation, kind) in this interval — the interval
/// of IAM's `LOG_RATE_LIMIT_INTERVAL` (`paigasus-iam/src/application/log_rate_limit.rs:11`).
pub const STORE_LOG_INTERVAL: Duration = Duration::from_secs(10);

/// The `role` label of the gateway's Redis breaker series (spec § 4.8).
pub const LIMITS_BREAKER_ROLE: &str = "limits";

/// The `op` label of `gateway_limit_store_unavailable_total`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StoreOp {
    Check,
    Charge,
}

impl StoreOp {
    pub fn as_label(self) -> &'static str {
        match self {
            StoreOp::Check => "check",
            StoreOp::Charge => "charge",
        }
    }
}

/// Which store backs the limits; decides whether the breaker series are primed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreBackend {
    Memory,
    Redis,
}

/// The limits service. `Limits` is shared as `Arc<Limits>`: every `ChargeGuard` holds one.
pub struct Limits {
    rules: LimitRules,
    store: Arc<dyn LimitStore>,
    clock: Arc<dyn Clock>,
}

impl Limits {
    pub fn new(rules: LimitRules, store: Arc<dyn LimitStore>, clock: Arc<dyn Clock>) -> Self {
        Limits { rules, store, clock }
    }

    pub(crate) fn store(&self) -> &dyn LimitStore {
        self.store.as_ref()
    }

    pub(crate) fn clock(&self) -> &dyn Clock {
        self.clock.as_ref()
    }

    /// Admit or refuse one chat request (spec § 4.3a). `Err` is a refusal the handler maps to a
    /// `GatewayError`; `Ok` carries the guard that charges the request when it drops.
    pub async fn admit(self: &Arc<Self>, caller: &CallerContext) -> Result<ChargeGuard, LimitRefusal> {
        let org = org_of(&caller.scope_prn);
        let policy = self.rules.policy_for(org);
        if org.is_none() {
            // D2: a valid request always has an org, so this is a parse mismatch or an IAM defect.
            counter!(names::GATEWAY_LIMIT_UNSCOPED_REQUESTS_TOTAL).increment(1);
            tracing::warn!(principal = %caller.principal_prn, scope = %caller.scope_prn, "chat request scope names no organization");
            if policy.has_org_dimension() {
                return Err(LimitRefusal::Unscoped);
            }
        }
        if policy.is_empty() {
            return Ok(ChargeGuard::new(Arc::clone(self), None, org, NoTicket::NoBudget));
        }
        match self.store.check_and_admit(&caller.principal_prn, org, &policy, self.clock.now()).await {
            Ok(LimitDecision::Admit(ticket)) => Ok(ChargeGuard::new(Arc::clone(self), ticket, org, NoTicket::NoBudget)),
            Ok(LimitDecision::Refused(failed)) => match choose_refusal(&failed) {
                Some(refusal) => {
                    if let Some(reason) = refusal.reason() {
                        counter!(names::GATEWAY_LIMIT_REFUSALS_TOTAL, "reason" => reason.as_label()).increment(1);
                    }
                    Err(refusal)
                }
                // A refusal with no failed check is an adapter defect; treat it as a store fault.
                None => {
                    report_store_unavailable(StoreOp::Check, UnavailableKind::Decode, "the store refused a request with no failed check");
                    Ok(ChargeGuard::new(Arc::clone(self), None, org, NoTicket::StoreUnavailable))
                }
            },
            Err(LimitStoreError::Unavailable { kind, detail }) => {
                report_store_unavailable(StoreOp::Check, kind, &detail);
                Ok(ChargeGuard::new(Arc::clone(self), None, org, NoTicket::StoreUnavailable))
            }
        }
    }
}

/// D10: one store-failure log line per (operation, kind) per interval. IAM's `LogRateLimiter` is
/// `pub(crate)` there, so the gateway holds this small copy with the same policy: a suppressed
/// event is still counted by the metric, and the next line reports how many were suppressed.
pub struct StoreLogLimiter {
    interval: Duration,
    last: Mutex<HashMap<(StoreOp, UnavailableKind), (Instant, u64)>>,
}

impl StoreLogLimiter {
    pub fn new(interval: Duration) -> Self {
        StoreLogLimiter { interval, last: Mutex::new(HashMap::new()) }
    }

    /// `Some(suppressed)` when a line may be written at `now`; `None` when it is suppressed.
    pub fn admit_at(&self, op: StoreOp, kind: UnavailableKind, now: Instant) -> Option<u64> {
        let mut last = self.last.lock().unwrap_or_else(PoisonError::into_inner);
        match last.get_mut(&(op, kind)) {
            Some((at, suppressed)) if now.saturating_duration_since(*at) < self.interval => {
                *suppressed = suppressed.saturating_add(1);
                None
            }
            Some((at, suppressed)) => {
                let count = *suppressed;
                *at = now;
                *suppressed = 0;
                Some(count)
            }
            None => {
                last.insert((op, kind), (now, 0));
                Some(0)
            }
        }
    }
}

static STORE_LOG: LazyLock<StoreLogLimiter> = LazyLock::new(|| StoreLogLimiter::new(STORE_LOG_INTERVAL));

/// D10: count a failed store call and log it (rate-limited). `Io` logs at `warn`: Redis is down,
/// and fail-open is the designed answer. `Server` and `Decode` log at `error`: Redis answered, so
/// the fault is a misconfiguration or a defect that fail-open would otherwise hide.
pub fn report_store_unavailable(op: StoreOp, kind: UnavailableKind, detail: &str) {
    report_store_unavailable_with(&STORE_LOG, Instant::now(), op, kind, detail);
}

fn report_store_unavailable_with(limiter: &StoreLogLimiter, now: Instant, op: StoreOp, kind: UnavailableKind, detail: &str) {
    counter!(names::GATEWAY_LIMIT_STORE_UNAVAILABLE_TOTAL, "op" => op.as_label(), "kind" => kind.as_label()).increment(1);
    let Some(suppressed) = limiter.admit_at(op, kind, now) else {
        return;
    };
    match kind {
        UnavailableKind::Io => tracing::warn!(op = op.as_label(), kind = kind.as_label(), suppressed, error = %detail, "limit store unavailable; limits not applied (fail-open)"),
        UnavailableKind::Server | UnavailableKind::Decode => {
            tracing::error!(op = op.as_label(), kind = kind.as_label(), suppressed, error = %detail, "limit store unavailable; limits not applied (fail-open)")
        }
    }
}

/// A7: register every limit series at zero, so `increase()` sees the first event (memory
/// `prometheus-new-counter-first-sample-trap`). The two breaker series only for Redis; the
/// breaker's constructor also sets its gauge. `build_limits` (Task 13) and the metric tests call
/// this one function.
pub fn prime_metrics(backend: StoreBackend) {
    for reason in RefusalReason::ALL {
        counter!(names::GATEWAY_LIMIT_REFUSALS_TOTAL, "reason" => reason.as_label()).increment(0);
    }
    for source in ChargeSource::ALL {
        counter!(names::GATEWAY_TOKENS_CHARGED_TOTAL, "source" => source.as_label()).increment(0);
    }
    counter!(names::GATEWAY_LIMIT_UNSCOPED_REQUESTS_TOTAL).increment(0);
    for op in [StoreOp::Check, StoreOp::Charge] {
        for kind in UnavailableKind::ALL {
            counter!(names::GATEWAY_LIMIT_STORE_UNAVAILABLE_TOTAL, "op" => op.as_label(), "kind" => kind.as_label()).increment(0);
        }
    }
    for reason in ChargeDropReason::ALL {
        counter!(names::GATEWAY_LIMIT_CHARGES_DROPPED_TOTAL, "reason" => reason.as_label()).increment(0);
    }
    if backend == StoreBackend::Redis {
        gauge!(names::GATEWAY_REDIS_BREAKER_STATE, "role" => LIMITS_BREAKER_ROLE).set(0.0);
        for to in ["closed", "half_open", "open"] {
            counter!(names::GATEWAY_REDIS_BREAKER_TRANSITIONS_TOTAL, "role" => LIMITS_BREAKER_ROLE, "to" => to).increment(0);
        }
    }
}
```

- [ ] **Step 6: Write the guard**

Insert above `#[cfg(test)]` in `src/application/charge_guard.rs`:

```rust
use std::sync::Arc;

use metrics::counter;
use paigasus_observability::{RequestIds, names};
use uuid::Uuid;

use crate::application::limits::Limits;
use crate::domain::limits::{LimitTicket, MAX_TOKENS_PER_CHARGE};

/// The `source` label of `gateway_tokens_charged_total`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChargeSource {
    Reported,
    Estimated,
}

impl ChargeSource {
    pub const ALL: [ChargeSource; 2] = [ChargeSource::Reported, ChargeSource::Estimated];

    pub fn as_label(self) -> &'static str {
        match self {
            ChargeSource::Reported => "reported",
            ChargeSource::Estimated => "estimated",
        }
    }
}

/// Why a guard has no ticket: no budget applies, or the store failed (fail-open, D10).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoTicket {
    NoBudget,
    StoreUnavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Admitted,
    Sent,
    NoCharge,
}

pub struct ChargeGuard {
    limits: Arc<Limits>,
    ticket: Option<LimitTicket>,
    org: Option<Uuid>,
    no_ticket: NoTicket,
    request_estimate: u64,
    completion_estimate: u64,
    reported: Option<u64>,
    phase: Phase,
    ids: Option<RequestIds>,
}

impl ChargeGuard {
    pub(crate) fn new(limits: Arc<Limits>, ticket: Option<LimitTicket>, org: Option<Uuid>, no_ticket: NoTicket) -> Self {
        ChargeGuard { limits, ticket, org, no_ticket, request_estimate: 0, completion_estimate: 0, reported: None, phase: Phase::Admitted, ids: None }
    }

    pub fn has_ticket(&self) -> bool {
        self.ticket.is_some()
    }

    /// D14: the request-side estimate, charged whenever the request was sent and no usage arrives.
    pub fn set_request_estimate(&mut self, tokens: u64) {
        self.request_estimate = tokens;
    }

    /// The ids for the `chat completion metered` line. Read by the handler, where they exist; the
    /// stream runs with `current_ids() == None` (SMA-504 § 4.3 situation 3).
    pub fn set_ids(&mut self, ids: Option<RequestIds>) {
        self.ids = ids;
    }

    /// Just before the egress call.
    pub fn mark_sent(&mut self) {
        self.phase = Phase::Sent;
    }

    /// A connect failure or a non-`2xx` answer: no tokens (A4).
    pub fn mark_no_charge(&mut self) {
        self.phase = Phase::NoCharge;
    }

    /// A `2xx` answer with `usage.total_tokens`.
    pub fn set_reported(&mut self, total_tokens: u64) {
        self.reported = Some(total_tokens);
    }

    /// D14: the completion estimate of a non-stream `2xx` body without usage.
    pub fn set_completion_estimate(&mut self, tokens: u64) {
        self.completion_estimate = tokens;
    }

    /// D14: the stream so far: one token per complete `data:` record, and the last usage seen.
    pub fn set_stream_progress(&mut self, completion_records: u64, reported: Option<u64>) {
        self.completion_estimate = completion_records;
        if reported.is_some() {
            self.reported = reported;
        }
    }

    /// The tokens and source the drop would charge now (A4, D23).
    pub fn tokens(&self) -> (u64, ChargeSource) {
        match (self.phase, self.reported) {
            (Phase::Admitted | Phase::NoCharge, _) => (0, ChargeSource::Estimated),
            (Phase::Sent, Some(total)) => (total.min(MAX_TOKENS_PER_CHARGE), ChargeSource::Reported),
            (Phase::Sent, None) => (self.request_estimate.saturating_add(self.completion_estimate).min(MAX_TOKENS_PER_CHARGE), ChargeSource::Estimated),
        }
    }
}

/// Manual: `Limits` holds trait objects and has no `Debug`. `Result::expect_err` on
/// `admit`'s result needs one.
impl std::fmt::Debug for ChargeGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChargeGuard").field("ticket", &self.ticket).field("org", &self.org).field("phase", &self.phase).finish_non_exhaustive()
    }
}

impl Drop for ChargeGuard {
    /// Never panics: no `unwrap`, no `expect`, and `LimitStore::charge` must not panic.
    fn drop(&mut self) {
        let (tokens, source) = self.tokens();
        let outcome = if tokens == 0 {
            "zero"
        } else if self.ticket.is_some() {
            "charged"
        } else {
            match self.no_ticket {
                NoTicket::NoBudget => "no_budget",
                NoTicket::StoreUnavailable => "store_unavailable",
            }
        };
        if tokens > 0
            && let Some(ticket) = self.ticket.take()
        {
            counter!(names::GATEWAY_TOKENS_CHARGED_TOTAL, "source" => source.as_label()).increment(tokens);
            self.limits.store().charge(ticket, tokens, self.limits.clock().now());
        }
        let org = self.org.map(|org| org.to_string()).unwrap_or_default();
        let (request_id, correlation_id) = self.ids.map(|ids| (ids.request_id.to_string(), ids.correlation_id.to_string())).unwrap_or_default();
        // The per-org spend record for both paths, fail-open spend included (§ 4.6).
        if tokens > 0 {
            tracing::info!(org = %org, tokens, source = source.as_label(), outcome, request_id = %request_id, correlation_id = %correlation_id, "chat completion metered");
        } else {
            tracing::debug!(org = %org, tokens, source = source.as_label(), outcome, request_id = %request_id, correlation_id = %correlation_id, "chat completion metered");
        }
    }
}
```

- [ ] **Step 7: Run the tests to verify they pass**

```bash
cd rs && cargo fmt && cargo nextest run --locked -p paigasus-gateway application:: && cargo clippy --locked -p paigasus-gateway --all-targets -- -D warnings
```

Expected: PASS. If `the_metered_line_names_the_outcome_and_the_ids` or `io_logs_at_warn_and_server_logs_at_error` fails only on the quoting of a field or the spelling of the level (for example `outcome=charged` without quotes), read the captured text and match the formatter's real form; do not weaken the assertion to a bare substring of the message.

- [ ] **Step 8: Commit**

```bash
git add rs/crates/services/paigasus-gateway/Cargo.toml rs/Cargo.lock rs/crates/services/paigasus-gateway/src/lib.rs rs/crates/services/paigasus-gateway/src/application rs/crates/services/paigasus-gateway/src/test_support.rs
git commit -m "feat(rs): add the gateway limits service and the charge guard

Limits::admit resolves the org, skips the store for an empty policy,
applies the D8 precedence and fails open on a store error. ChargeGuard
charges an admitted request at most once from Drop and writes the
chat completion metered line (SMA-677).

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 8: The SSE usage scanner and the D14 estimates

**Files:**
- Create: `rs/crates/services/paigasus-gateway/src/adapters/http/usage.rs`
- Modify: `rs/crates/services/paigasus-gateway/src/adapters/http/mod.rs` (`pub mod usage;` after `pub mod service_info;`)

**Interfaces:**
- Consumes: nothing from earlier tasks (pure OpenAI wire parsing).
- Produces (in `paigasus_gateway::adapters::http::usage`, all `pub`):
  - `const MAX_RECORD_BYTES: usize = 64 * 1024`
  - `fn request_tokens_estimate(messages: &[serde_json::Value], body_len: usize) -> u64`
  - `enum BodyUsage { Reported(u64), Completion(u64) }`; `fn non_stream_usage(body: &[u8]) -> BodyUsage`
  - `struct UsageScanner` (`Default`) with `fn new() -> UsageScanner`, `fn feed(&mut self, chunk: &[u8])`, `fn completion_records(&self) -> u64`, `fn reported_total(&self) -> Option<u64>`

The scanner parses SSE per its grammar (a line ends at CRLF, LF or CR; a blank line ends a record), so it accepts `\n\n`, `\r\n\r\n` and `\r\r` and any mix, also when a terminator is split across two chunks. It reads a copy: it never changes the forwarded bytes (A5). It lives in the HTTP adapter because it parses the OpenAI wire format (spec § 4.6).

- [ ] **Step 1: Write the failing tests**

Create `src/adapters/http/usage.rs` with the doc comment and the tests:

```rust
// SPDX-License-Identifier: Apache-2.0

//! The OpenAI wire readers for the charge (SMA-677 D14, § 4.6): the request estimate, the
//! non-stream completion estimate, and `UsageScanner`, which reads a COPY of a forwarded SSE
//! stream. None of them changes a byte that reaches the client (A5).

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn scan(chunks: &[&str]) -> UsageScanner {
        let mut scanner = UsageScanner::new();
        for chunk in chunks {
            scanner.feed(chunk.as_bytes());
        }
        scanner
    }

    /// A real-shaped OpenAI chunk (spec § 5.4), optionally with `"usage":null`.
    fn chunk(content: &str, usage_null: bool) -> String {
        let usage = if usage_null { ",\"usage\":null" } else { "" };
        format!(
            "data: {{\"id\":\"chatcmpl-1\",\"object\":\"chat.completion.chunk\",\"created\":1790942400,\"model\":\"gpt-4o-mini\",\"system_fingerprint\":\"fp_1\",\"choices\":[{{\"index\":0,\"delta\":{{\"content\":\"{content}\"}},\"finish_reason\":null}}]{usage}}}\n\n"
        )
    }

    fn usage_chunk(total: u64) -> String {
        format!("data: {{\"id\":\"chatcmpl-1\",\"object\":\"chat.completion.chunk\",\"choices\":[],\"usage\":{{\"prompt_tokens\":3,\"completion_tokens\":4,\"total_tokens\":{total}}}}}\n\n")
    }

    #[test]
    fn records_end_at_a_blank_line_with_every_line_ending() {
        for input in ["data: a\n\ndata: b\n\n", "data: a\r\n\r\ndata: b\r\n\r\n", "data: a\r\rdata: b\r\r", "data: a\r\n\ndata: b\n\r\n"] {
            assert_eq!(scan(&[input]).completion_records(), 2, "{input:?}");
        }
    }

    /// Review Focus 3.
    #[test]
    fn a_crlf_split_across_chunks_is_one_terminator() {
        assert_eq!(scan(&["data: a\r", "\n\r", "\n"]).completion_records(), 1);
        assert_eq!(scan(&["data: a\r\n\r", "\ndata: b\r\n\r\n"]).completion_records(), 2);
        // A lone CR followed by a non-LF byte is a terminator of its own.
        assert_eq!(scan(&["data: a\r", "\rdata: b\r\r"]).completion_records(), 2);
    }

    #[test]
    fn a_record_split_byte_by_byte_counts_once() {
        let record = chunk(" the", true);
        let mut scanner = UsageScanner::new();
        for byte in record.as_bytes() {
            scanner.feed(std::slice::from_ref(byte));
        }
        assert_eq!(scanner.completion_records(), 1);
    }

    #[test]
    fn done_comments_and_empty_records_are_not_counted() {
        assert_eq!(scan(&["\n\n: keep-alive\n\ndata: [DONE]\n\n"]).completion_records(), 0);
    }

    #[test]
    fn a_null_usage_is_a_record_and_a_usage_object_is_reported() {
        let stream = format!("{}{}{}{}data: [DONE]\n\n", chunk("Hel", true), chunk("lo", true), chunk("!", false), usage_chunk(7));
        let scanner = scan(&[stream.as_str()]);
        assert_eq!(scanner.completion_records(), 3, "`\"usage\":null` must not be read as a usage record");
        assert_eq!(scanner.reported_total(), Some(7));
    }

    #[test]
    fn the_last_usage_wins_and_whitespace_after_the_colon_matches() {
        let stream = format!("{}data: {{\"choices\":[],\"usage\": {{\"total_tokens\":9}}}}\n\n", usage_chunk(7));
        assert_eq!(scan(&[stream.as_str()]).reported_total(), Some(9));
    }

    #[test]
    fn a_usage_record_that_does_not_parse_counts_as_a_record() {
        let scanner = scan(&["data: {\"usage\":{\"total_tokens\":\n\n"]);
        assert_eq!((scanner.completion_records(), scanner.reported_total()), (1, None));
    }

    #[test]
    fn an_oversized_record_counts_once_and_is_not_buffered() {
        let mut scanner = UsageScanner::new();
        scanner.feed(b"data: ");
        let filler = vec![b'x'; 8 * 1024];
        for _ in 0..10 {
            scanner.feed(&filler);
        }
        assert!(scanner.line.len() <= MAX_RECORD_BYTES && scanner.data.len() <= MAX_RECORD_BYTES, "the tail stays bounded");
        scanner.feed(b"\n\n");
        scanner.feed(chunk("next", true).as_bytes());
        assert_eq!(scanner.completion_records(), 2, "the oversized record counts as one, and the next record still parses");
    }

    /// Review Focus 4 and D14.
    #[test]
    fn request_estimate_shapes() {
        let est = |messages: serde_json::Value, body_len: usize| request_tokens_estimate(messages.as_array().expect("an array"), body_len);
        // String contents: 5 + 2 bytes → ceil(7 / 4) = 2.
        assert_eq!(est(json!([{"role": "system", "content": "hello"}, {"role": "user", "content": "hi"}]), 999), 2);
        // A text part and an image part: ceil(4 / 4) + 85.
        assert_eq!(est(json!([{"role": "user", "content": [{"type": "text", "text": "abcd"}, {"type": "image_url", "image_url": {"url": "data:image/png;base64,AAAA"}}]}]), 999), 86);
        // `content: null` with tool calls counts 0, and is not a reason for the fallback.
        assert_eq!(est(json!([{"role": "assistant", "content": null, "tool_calls": [{"id": "c1"}]}, {"role": "user", "content": "abcd"}]), 999), 1);
        // An unknown shape uses ceil(body_len / 4), capped at 32768.
        assert_eq!(est(json!([{"role": "user", "content": 42}]), 400), 100);
        assert_eq!(est(json!(["not an object"]), 400), 100);
        assert_eq!(est(json!([{"role": "user", "content": [{"text": "no type"}]}]), 401), 101);
        assert_eq!(est(json!([{"role": "user", "content": 42}]), 1_000_000), 32_768);
        assert_eq!(est(json!([]), 10), 0);
    }

    #[test]
    fn non_stream_usage_reads_usage_or_estimates_the_completion() {
        assert_eq!(non_stream_usage(br#"{"choices":[],"usage":{"total_tokens":12}}"#), BodyUsage::Reported(12));
        // 8 content bytes + 7 argument bytes (`{"a":1}`) → ceil(15 / 4) = 4.
        let body = br#"{"choices":[{"message":{"content":"abcdefgh","tool_calls":[{"function":{"name":"f","arguments":"{\"a\":1}"}}]}}]}"#;
        assert_eq!(non_stream_usage(body), BodyUsage::Completion(4));
        assert_eq!(non_stream_usage(b"not json"), BodyUsage::Completion(0));
        assert_eq!(non_stream_usage(br#"{"usage":{"total_tokens":"many"},"choices":[]}"#), BodyUsage::Completion(0));
    }
}
```

In `src/adapters/http/mod.rs`, after `pub mod service_info;`:

```rust
pub mod usage;
```

- [ ] **Step 2: Run them to verify they fail**

```bash
cd rs && cargo nextest run --locked -p paigasus-gateway adapters::http::usage
```

Expected: compile FAIL (`cannot find type UsageScanner`).

- [ ] **Step 3: Write the implementation**

Insert above `#[cfg(test)]` in `usage.rs`:

```rust
use serde_json::Value;

/// § 4.6: the bound on one partial SSE record. A larger record is skipped, not buffered, and
/// still counts as one record. The same bound as the console parser (`chat-stream.ts`).
pub const MAX_RECORD_BYTES: usize = 64 * 1024;
/// D14: the fallback request estimate is capped.
const FALLBACK_REQUEST_CAP: u64 = 32_768;
/// D14: an image, audio or other non-text part counts OpenAI's low-detail image cost.
const NON_TEXT_PART_TOKENS: u64 = 85;

fn bytes_to_tokens(bytes: usize) -> u64 {
    u64::try_from(bytes).unwrap_or(u64::MAX).div_ceil(4)
}

/// D14: `ceil(text_bytes / 4)` over every string `content` and every `text` part, plus 85 per
/// non-text part. A `null` or absent `content` counts 0. Any other shape falls back to
/// `ceil(body_len / 4)`, capped at 32768.
pub fn request_tokens_estimate(messages: &[Value], body_len: usize) -> u64 {
    let fallback = bytes_to_tokens(body_len).min(FALLBACK_REQUEST_CAP);
    let mut text_bytes = 0usize;
    let mut other_parts = 0u64;
    for message in messages {
        let Some(message) = message.as_object() else { return fallback };
        match message.get("content") {
            None | Some(Value::Null) => {}
            Some(Value::String(text)) => text_bytes = text_bytes.saturating_add(text.len()),
            Some(Value::Array(parts)) => {
                for part in parts {
                    match (part.get("type").and_then(Value::as_str), part.get("text").and_then(Value::as_str)) {
                        (Some("text"), Some(text)) => text_bytes = text_bytes.saturating_add(text.len()),
                        (Some(_), _) => other_parts = other_parts.saturating_add(1),
                        (None, _) => return fallback,
                    }
                }
            }
            Some(_) => return fallback,
        }
    }
    bytes_to_tokens(text_bytes).saturating_add(other_parts.saturating_mul(NON_TEXT_PART_TOKENS))
}

/// What a non-stream `2xx` body says about its tokens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyUsage {
    /// `usage.total_tokens`.
    Reported(u64),
    /// D14: `ceil(len / 4)` of every `choices[].message.content` and tool-call `arguments`.
    Completion(u64),
}

pub fn non_stream_usage(body: &[u8]) -> BodyUsage {
    let Ok(value) = serde_json::from_slice::<Value>(body) else {
        return BodyUsage::Completion(0);
    };
    if let Some(total) = value.pointer("/usage/total_tokens").and_then(Value::as_u64) {
        return BodyUsage::Reported(total);
    }
    let mut bytes = 0usize;
    for choice in value.get("choices").and_then(Value::as_array).into_iter().flatten() {
        if let Some(content) = choice.pointer("/message/content").and_then(Value::as_str) {
            bytes = bytes.saturating_add(content.len());
        }
        for call in choice.pointer("/message/tool_calls").and_then(Value::as_array).into_iter().flatten() {
            if let Some(arguments) = call.pointer("/function/arguments").and_then(Value::as_str) {
                bytes = bytes.saturating_add(arguments.len());
            }
        }
    }
    BodyUsage::Completion(bytes_to_tokens(bytes))
}

/// Reads a copy of a forwarded SSE stream (§ 4.6). Every complete `data:` record that is not
/// `[DONE]` and holds no usage object adds one to the completion estimate (an OpenAI chunk
/// usually carries one token). A record that holds `"usage":{` is parsed, and the last
/// `usage.total_tokens` seen wins.
#[derive(Debug, Default)]
pub struct UsageScanner {
    /// The current partial line, bounded by `MAX_RECORD_BYTES`.
    line: Vec<u8>,
    /// Bytes of the current line, also when they were not buffered.
    line_len: usize,
    /// The last byte was CR: an LF right after it belongs to the same terminator.
    after_cr: bool,
    /// The current record's `data` value (lines joined with LF), bounded.
    data: Vec<u8>,
    has_data: bool,
    record_bytes: usize,
    oversized: bool,
    records: u64,
    reported: Option<u64>,
}

impl UsageScanner {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn feed(&mut self, chunk: &[u8]) {
        for &byte in chunk {
            if self.after_cr {
                self.after_cr = false;
                if byte == b'\n' {
                    continue;
                }
            }
            match byte {
                b'\n' => self.end_line(),
                b'\r' => {
                    self.end_line();
                    self.after_cr = true;
                }
                _ => {
                    self.line_len = self.line_len.saturating_add(1);
                    self.record_bytes = self.record_bytes.saturating_add(1);
                    if self.record_bytes > MAX_RECORD_BYTES {
                        self.oversized = true;
                        self.line.clear();
                        self.data.clear();
                    } else {
                        self.line.push(byte);
                    }
                }
            }
        }
    }

    pub fn completion_records(&self) -> u64 {
        self.records
    }

    pub fn reported_total(&self) -> Option<u64> {
        self.reported
    }

    fn end_line(&mut self) {
        if self.line_len == 0 {
            self.end_record();
            return;
        }
        self.line_len = 0;
        let line = std::mem::take(&mut self.line);
        if self.oversized {
            return;
        }
        if let Some(value) = line.strip_prefix(b"data:") {
            let value = value.strip_prefix(b" ").unwrap_or(value);
            if self.has_data {
                self.data.push(b'\n');
            }
            self.data.extend_from_slice(value);
            self.has_data = true;
        }
    }

    fn end_record(&mut self) {
        if self.oversized {
            self.records = self.records.saturating_add(1);
        } else if self.has_data {
            self.dispatch();
        }
        self.data.clear();
        self.has_data = false;
        self.oversized = false;
        self.record_bytes = 0;
    }

    fn dispatch(&mut self) {
        if self.data.as_slice() == b"[DONE]" {
            return;
        }
        if has_usage_object(&self.data)
            && let Some(total) = serde_json::from_slice::<Value>(&self.data).ok().and_then(|v| v.pointer("/usage/total_tokens").and_then(Value::as_u64))
        {
            self.reported = Some(total);
            return;
        }
        self.records = self.records.saturating_add(1);
    }
}

/// `"usage"` + optional whitespace + `:` + optional whitespace + `{`. With `include_usage`,
/// OpenAI sends `"usage":null` in every chunk, so a match on `"usage"` alone would parse every
/// record.
fn has_usage_object(data: &[u8]) -> bool {
    const KEY: &[u8] = b"\"usage\"";
    let mut rest = data;
    while let Some(pos) = rest.windows(KEY.len()).position(|w| w == KEY) {
        rest = &rest[pos + KEY.len()..];
        if let Some(after_colon) = trim_start(rest).strip_prefix(b":")
            && trim_start(after_colon).first() == Some(&b'{')
        {
            return true;
        }
    }
    false
}

fn trim_start(bytes: &[u8]) -> &[u8] {
    let start = bytes.iter().position(|b| !b.is_ascii_whitespace()).unwrap_or(bytes.len());
    &bytes[start..]
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cd rs && cargo fmt && cargo nextest run --locked -p paigasus-gateway adapters::http::usage && cargo clippy --locked -p paigasus-gateway --all-targets -- -D warnings
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add rs/crates/services/paigasus-gateway/src/adapters/http/usage.rs rs/crates/services/paigasus-gateway/src/adapters/http/mod.rs
git commit -m "feat(rs): add the gateway SSE usage scanner and token estimates

UsageScanner reads a copy of a forwarded stream with all three SSE line
endings, a bounded record buffer and the usage-object match. The D14
request and completion estimates for the charge (SMA-677).

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 9: Wire the limits into the chat handler (non-stream charge)

**Files:**
- Modify: `rs/crates/services/paigasus-gateway/src/adapters/http/mod.rs` (`AppState` at line 43; the test `state_with_iam` at 245-252)
- Modify: `rs/crates/services/paigasus-gateway/src/adapters/http/chat.rs` (handler lines 94-160; `StreamState`/`terminal_sse_error_stream` at 206-239; the three unit tests that call `terminal_sse_error_stream`)
- Modify: `rs/crates/services/paigasus-gateway/src/main.rs:87-92` (`limits: None`; Task 13 wires it)
- Modify: `rs/crates/services/paigasus-gateway/tests/chat_proxy.rs` (line 34 import, `AppState` literal at 165, `spawn_truncated_sse_server` at 545-571 moves to support)
- Modify: `rs/crates/services/paigasus-gateway/tests/metrics.rs` (three `AppState` literals: 103, 155, 227), `tests/service_info.rs:156`
- Modify: `rs/crates/services/paigasus-gateway/tests/support/mod.rs` (request counter, two mock modes, the truncated server, `pub mod limits;`)
- Create: `rs/crates/services/paigasus-gateway/tests/support/limits.rs`, `tests/limits_http.rs`

**Interfaces:**
- Consumes: Task 7's `Limits::admit`, `ChargeGuard`; Task 8's `usage::{request_tokens_estimate, non_stream_usage, BodyUsage}`; Task 6's `GatewayError::from(LimitRefusal)`; Task 4's `MemoryLimitStore`.
- Produces:
  - `AppState.limits: Option<Arc<Limits>>` (`None` = no `[limits]`, A6)
  - `terminal_sse_error_stream(inner: OpenAiByteStream, guard: Option<ChargeGuard>)` with the unfold state `struct Relay { phase: StreamState, guard: Option<ChargeGuard> }` (Task 10 adds the scanner)
  - Test support: `MockOpenAi::{request_count, spawn_delayed_json, spawn_raw_sse}`, `support::spawn_truncated_sse() -> String`; `support::limits::{ScopedIam, FixedClock, RecordingStore, FailingStore, Sent, at, rules, limits, app, app_with, post, send, chunk, usage_chunk, sample, CALLER_KEY, ORG_A, ORG_A_PRN, PROJECT_IN_A, UNSCOPED, PRINCIPAL_1, PRINCIPAL_2, NOON, NON_STREAM_BODY, STREAM_BODY, USAGE_BODY}`

Deviation from spec § 5.4, on purpose: the new HTTP tests live in a new binary `tests/limits_http.rs`, not in `tests/chat_proxy.rs`. They need a fake IAM whose scope is a canonical tenancy PRN (the existing `CALLER_SCOPE` does not parse as one), and a separate binary leaves every existing `chat_proxy.rs` assertion untouched (A6).

- [ ] **Step 1: Extend the mock upstream and add the limit fixtures**

In `tests/support/mod.rs`:

- Add `use std::sync::atomic::AtomicUsize;` and `use std::time::Duration;` to the imports (keep `AtomicBool, Ordering`).
- Add to `enum MockResponse`:

```rust
    /// Non-stream, after `delay`: drives the first-byte timeout (SMA-677 A4).
    DelayedJson { delay: Duration, status: StatusCode, body: String },
    /// Stream: emit each element VERBATIM as one frame — no `data:` prefix and no terminator are
    /// added, so a test controls line endings and record shape exactly (SMA-677).
    RawSse { chunks: Vec<String> },
```

- `struct MockState` gains `requests: AtomicUsize`, initialised with `AtomicUsize::new(0)` in `spawn`.
- Add to `impl MockOpenAi`:

```rust
    /// Start a mock that answers after `delay` (the non-stream path).
    pub async fn spawn_delayed_json(delay: Duration, status: StatusCode, body: impl Into<String>) -> Self {
        Self::spawn(MockResponse::DelayedJson { delay, status, body: body.into() }).await
    }

    /// Start a mock that streams each chunk verbatim (the stream path).
    pub async fn spawn_raw_sse(chunks: Vec<String>) -> Self {
        Self::spawn(MockResponse::RawSse { chunks }).await
    }

    /// How many requests reached the upstream (SMA-677: a refused request makes none).
    pub fn request_count(&self) -> usize {
        self.state.requests.load(Ordering::SeqCst)
    }
```

- At the top of `async fn handle`, before recording: `state.requests.fetch_add(1, Ordering::SeqCst);`
- Add two arms to its `match`:

```rust
        MockResponse::DelayedJson { delay, status, body } => {
            tokio::time::sleep(*delay).await;
            (*status, [(axum::http::header::CONTENT_TYPE, "application/json")], body.clone()).into_response()
        }
        MockResponse::RawSse { chunks } => {
            let stream = futures::stream::iter(chunks.clone().into_iter().map(|c| Ok::<Bytes, std::convert::Infallible>(Bytes::from(c))));
            Response::builder()
                .status(StatusCode::OK)
                .header(axum::http::header::CONTENT_TYPE, "text/event-stream")
                .body(Body::from_stream(stream))
                .expect("build sse response")
        }
```

- Move `spawn_truncated_sse_server` from `tests/chat_proxy.rs` (lines 545-571, doc comment included) to the end of `tests/support/mod.rs`, renamed `pub async fn spawn_truncated_sse() -> String`, with `use tokio::io::{AsyncReadExt, AsyncWriteExt};` as the first line of its body. In `tests/chat_proxy.rs`, delete line 34 (`use tokio::io::{AsyncReadExt, AsyncWriteExt};`) and replace the call at line 383 with `support::spawn_truncated_sse().await`.
- Add after `pub mod limits_contract;`: `pub mod limits;`

Create `tests/support/limits.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0

//! Fixtures for the gateway limit tests (SMA-677 spec § 5.4): a fake IAM with a chosen scope,
//! canonical tenancy PRNs, a fixed clock, a recording store and a failing store, real-shaped
//! OpenAI chunks, and a Prometheus text reader that parses NUMBERS.

use std::collections::HashMap;
use std::num::NonZeroU64;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use axum::Router;
use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, Request, StatusCode, header};
use secrecy::SecretString;
use tonic::Status;
use tower::ServiceExt;
use uuid::Uuid;

use paigasus_gateway::adapters::http::{AppState, router};
use paigasus_gateway::adapters::iam::{Iam, IamError};
use paigasus_gateway::adapters::limits::MemoryLimitStore;
use paigasus_gateway::adapters::openai::OpenAiClient;
use paigasus_gateway::application::limits::Limits;
use paigasus_gateway::config::OpenAiConfig;
use paigasus_gateway::domain::limits::{BudgetPeriod, Clock, LimitDecision, LimitPolicy, LimitRules, LimitStore, LimitStoreError, LimitTicket, OrgOverride, UnavailableKind};
use paigasus_gateway::service_info::Capabilities;
use paigasus_proto::paigasus::iam::v1::{IntrospectApiKeyResponse, IntrospectResponse};

pub const CALLER_KEY: &str = "sk-caller-secret";
pub const ORG_A: &str = "0190a100-0000-7000-8000-0000000000a1";
pub const ORG_A_PRN: &str = "prn:pgs:iam:::organization/0190a100-0000-7000-8000-0000000000a1";
pub const PROJECT_IN_A: &str = "prn:pgs:iam::0190a100-0000-7000-8000-0000000000a1:project/0190a1c3-0000-7000-8000-0000000000d4";
/// The pre-SMA-677 fixture scope: it does not parse as a tenancy PRN, so it names no org (D2).
pub const UNSCOPED: &str = "prn:paigasus:iam:default:scope/team-a";
pub const PRINCIPAL_1: &str = "prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-000000000001";
pub const PRINCIPAL_2: &str = "prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-000000000002";
pub const NOON: &str = "2026-10-02T12:00:00Z";
pub const NON_STREAM_BODY: &str = r#"{"model":"gpt-4o-mini","messages":[{"role":"user","content":"hi"}]}"#;
pub const STREAM_BODY: &str = r#"{"model":"gpt-4o-mini","stream":true,"messages":[{"role":"user","content":"hi"}]}"#;
/// A non-stream answer that reports 5 tokens.
pub const USAGE_BODY: &str = r#"{"id":"chatcmpl-1","object":"chat.completion","created":1790942400,"model":"gpt-4o-mini","choices":[{"index":0,"message":{"role":"assistant","content":"hello"},"finish_reason":"stop"}],"usage":{"prompt_tokens":2,"completion_tokens":3,"total_tokens":5}}"#;

/// An active API-key caller with a chosen principal and scope.
pub struct ScopedIam {
    principal: String,
    scope: String,
}

impl ScopedIam {
    pub fn new(principal: &str, scope: &str) -> Arc<dyn Iam> {
        Arc::new(ScopedIam { principal: principal.to_owned(), scope: scope.to_owned() })
    }
}

#[async_trait::async_trait]
impl Iam for ScopedIam {
    async fn introspect_api_key(&self, _token: &str) -> Result<IntrospectApiKeyResponse, IamError> {
        Ok(IntrospectApiKeyResponse {
            principal_prn: self.principal.clone(),
            status: "active".to_owned(),
            key_id: "key-limits".to_owned(),
            expires_at: None,
            memberships: Vec::new(),
            role_grants: Vec::new(),
            scope_prn: self.scope.clone(),
        })
    }

    async fn is_authorized_self(&self, _caller_key: &str, _principal_prn: &str, _action: &str, _resource_prn: &str) -> Result<bool, IamError> {
        Ok(true)
    }

    async fn introspect_token(&self, _token: &str) -> Result<IntrospectResponse, IamError> {
        Err(IamError::Rpc(Status::unauthenticated("not a token")))
    }
}

pub fn at(rfc3339: &str) -> SystemTime {
    SystemTime::from(chrono::DateTime::parse_from_rfc3339(rfc3339).expect("a fixture instant"))
}

pub struct FixedClock(pub SystemTime);

impl Clock for FixedClock {
    fn now(&self) -> SystemTime {
        self.0
    }
}

/// The memory store, plus a record of every `check_and_admit` and every `charge`.
pub struct RecordingStore {
    inner: MemoryLimitStore,
    checks: AtomicUsize,
    charges: Mutex<Vec<u64>>,
}

impl RecordingStore {
    pub fn new() -> Arc<Self> {
        Arc::new(RecordingStore { inner: MemoryLimitStore::new(), checks: AtomicUsize::new(0), charges: Mutex::new(Vec::new()) })
    }

    pub fn checks(&self) -> usize {
        self.checks.load(Ordering::SeqCst)
    }

    pub fn charges(&self) -> Vec<u64> {
        self.charges.lock().expect("not poisoned").clone()
    }
}

#[async_trait::async_trait]
impl LimitStore for RecordingStore {
    async fn check_and_admit(&self, principal: &str, org: Option<Uuid>, policy: &LimitPolicy, now: SystemTime) -> Result<LimitDecision, LimitStoreError> {
        self.checks.fetch_add(1, Ordering::SeqCst);
        self.inner.check_and_admit(principal, org, policy, now).await
    }

    fn charge(&self, ticket: LimitTicket, tokens: u64, now: SystemTime) {
        self.charges.lock().expect("not poisoned").push(tokens);
        self.inner.charge(ticket, tokens, now);
    }
}

/// A store whose every check fails with `kind` (A12), and that counts charges.
pub struct FailingStore {
    kind: UnavailableKind,
    charges: AtomicUsize,
}

impl FailingStore {
    pub fn new(kind: UnavailableKind) -> Arc<Self> {
        Arc::new(FailingStore { kind, charges: AtomicUsize::new(0) })
    }

    pub fn charges(&self) -> usize {
        self.charges.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl LimitStore for FailingStore {
    async fn check_and_admit(&self, _principal: &str, _org: Option<Uuid>, _policy: &LimitPolicy, _now: SystemTime) -> Result<LimitDecision, LimitStoreError> {
        Err(LimitStoreError::Unavailable { kind: self.kind, detail: "test failure".to_owned() })
    }

    fn charge(&self, _ticket: LimitTicket, _tokens: u64, _now: SystemTime) {
        self.charges.fetch_add(1, Ordering::SeqCst);
    }
}

/// Table defaults only, monthly budget.
pub fn rules(principal: Option<u64>, org: Option<u64>, tokens: Option<u64>) -> LimitRules {
    LimitRules {
        principal_requests_per_minute: principal.and_then(NonZeroU64::new),
        org_requests_per_minute: org.and_then(NonZeroU64::new),
        tokens_per_period: tokens.and_then(NonZeroU64::new),
        budget_period: BudgetPeriod::Monthly,
        overrides: HashMap::new(),
    }
}

/// `rules` plus one exempt org.
pub fn rules_with_exempt(rules: LimitRules, org: &str) -> LimitRules {
    let id = Uuid::try_parse(org).expect("a fixture uuid");
    LimitRules { overrides: HashMap::from([(id, OrgOverride { exempt: true, ..OrgOverride::default() })]), ..rules }
}

/// The limits service over `store`, with a clock fixed at `NOON`.
pub fn limits(rules: LimitRules, store: Arc<dyn LimitStore>) -> Arc<Limits> {
    Arc::new(Limits::new(rules, store, Arc::new(FixedClock(at(NOON)))))
}

pub fn openai(base_url: &str, first_byte: Duration) -> OpenAiClient {
    let cfg = OpenAiConfig { base_url: base_url.to_owned(), api_key: SecretString::from("sk-real-openai-key".to_owned()), extra_ca_bundle_path: None };
    OpenAiClient::new(&cfg, Duration::from_secs(10), first_byte, Duration::from_secs(300)).expect("client builds")
}

pub fn app(iam: Arc<dyn Iam>, base_url: &str, limits: Option<Arc<Limits>>) -> Router {
    app_with(iam, base_url, limits, Duration::from_secs(30), true)
}

pub fn app_with(iam: Arc<dyn Iam>, base_url: &str, limits: Option<Arc<Limits>>, first_byte: Duration, chat_stream: bool) -> Router {
    router(AppState { iam, openai: Arc::new(openai(base_url, first_byte)), max_request_bytes: 1_048_576, capabilities: Capabilities { chat_stream }, limits })
}

pub fn post(body: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, format!("Bearer {CALLER_KEY}"))
        .body(Body::from(body.to_owned()))
        .expect("build request")
}

/// One answer, read whole.
pub struct Sent {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Bytes,
}

impl Sent {
    pub fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).expect("a JSON body")
    }

    pub fn header(&self, name: &str) -> &str {
        self.headers.get(name).and_then(|v| v.to_str().ok()).unwrap_or_default()
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

pub async fn send(app: &Router, body: &str) -> Sent {
    let resp = app.clone().oneshot(post(body)).await.expect("the router answers");
    let status = resp.status();
    let headers = resp.headers().clone();
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.expect("the body reads");
    Sent { status, headers, body }
}

/// A real-shaped OpenAI chunk record (spec § 5.4), with `\n\n` after it.
pub fn chunk(content: &str, usage_null: bool) -> String {
    let usage = if usage_null { ",\"usage\":null" } else { "" };
    format!(
        "data: {{\"id\":\"chatcmpl-1\",\"object\":\"chat.completion.chunk\",\"created\":1790942400,\"model\":\"gpt-4o-mini\",\"system_fingerprint\":\"fp_1\",\"choices\":[{{\"index\":0,\"delta\":{{\"content\":\"{content}\"}},\"finish_reason\":null}}]{usage}}}\n\n"
    )
}

/// The final usage record of an `include_usage` stream: empty `choices`.
pub fn usage_chunk(total: u64) -> String {
    format!("data: {{\"id\":\"chatcmpl-1\",\"object\":\"chat.completion.chunk\",\"choices\":[],\"usage\":{{\"prompt_tokens\":2,\"completion_tokens\":3,\"total_tokens\":{total}}}}}\n\n")
}

/// The numeric value of the series `name` with exactly `labels`, parsed from Prometheus text.
/// Never `contains()` on a name or a `# TYPE` line (memory `prometheus-type-line-vacuous-assertion`).
pub fn sample(rendered: &str, name: &str, labels: &[(&str, &str)]) -> Option<f64> {
    let want: Vec<String> = labels.iter().map(|(k, v)| format!("{k}=\"{v}\"")).collect();
    rendered.lines().filter(|line| !line.starts_with('#')).find_map(|line| {
        let (series, value) = line.rsplit_once(' ')?;
        let (metric, label_text) = match series.split_once('{') {
            Some((metric, rest)) => (metric, rest.strip_suffix('}')?),
            None => (series, ""),
        };
        let have: Vec<&str> = if label_text.is_empty() { Vec::new() } else { label_text.split(',').collect() };
        (metric == name && have.len() == want.len() && want.iter().all(|w| have.contains(&w.as_str()))).then(|| value.parse().ok()).flatten()
    })
}
```

- [ ] **Step 2: Write the failing HTTP tests**

Create `tests/limits_http.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0

//! HTTP integration for the gateway limits (SMA-677 spec § 5.4): the real router and auth
//! middleware with a fake IAM, the real handler and OpenAI client against the in-process mock
//! upstream, and a memory, recording or failing limit store.

mod support;

use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use paigasus_gateway::adapters::limits::MemoryLimitStore;
use paigasus_gateway::domain::limits::UnavailableKind;
use paigasus_logging::test_support::capture_logs_at;
use support::MockOpenAi;
use support::limits::*;

fn memory() -> Arc<MemoryLimitStore> {
    Arc::new(MemoryLimitStore::new())
}

/// A1.
#[tokio::test]
async fn the_request_over_the_principal_rate_is_429_rate_limited() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, "{}").await;
    let app = app(ScopedIam::new(PRINCIPAL_1, ORG_A_PRN), &mock.base_url, Some(limits(rules(Some(2), None, None), memory())));
    for _ in 0..2 {
        assert_eq!(send(&app, NON_STREAM_BODY).await.status, StatusCode::OK);
    }
    let refused = send(&app, NON_STREAM_BODY).await;
    assert_eq!(refused.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(refused.json()["error"]["code"], "rate-limited");
    assert_eq!(refused.json()["error"]["type"], "requests");
    let retry_after: u32 = refused.header("retry-after").parse().expect("an integer Retry-After");
    assert!(retry_after >= 1);
    assert_eq!(refused.header("paigasus-retryable"), "true");
    assert_eq!(refused.header("x-should-retry"), "true");
    assert_eq!(mock.request_count(), 2, "the refused request made no upstream call");
}

/// A2 and D2: the org count is summed over principals, and a project scope counts against its org.
#[tokio::test]
async fn the_org_rate_is_shared_by_every_principal_of_the_org() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, "{}").await;
    let shared = limits(rules(None, Some(2), None), memory());
    let by_org = app(ScopedIam::new(PRINCIPAL_1, ORG_A_PRN), &mock.base_url, Some(shared.clone()));
    let by_project = app(ScopedIam::new(PRINCIPAL_2, PROJECT_IN_A), &mock.base_url, Some(shared));
    assert_eq!(send(&by_org, NON_STREAM_BODY).await.status, StatusCode::OK);
    assert_eq!(send(&by_project, NON_STREAM_BODY).await.status, StatusCode::OK);
    let refused = send(&by_org, NON_STREAM_BODY).await;
    assert_eq!(refused.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(refused.json()["error"]["code"], "rate-limited");
    assert_eq!(mock.request_count(), 2);
}

/// A3.
#[tokio::test]
async fn the_request_after_the_budget_is_used_is_429_budget_exhausted() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, USAGE_BODY).await;
    let app = app(ScopedIam::new(PRINCIPAL_1, ORG_A_PRN), &mock.base_url, Some(limits(rules(None, None, Some(1)), memory())));
    assert_eq!(send(&app, NON_STREAM_BODY).await.status, StatusCode::OK);
    let refused = send(&app, NON_STREAM_BODY).await;
    assert_eq!(refused.status, StatusCode::TOO_MANY_REQUESTS);
    let body = refused.json();
    assert_eq!(body["error"]["code"], "budget-exhausted");
    assert_eq!(body["error"]["type"], "insufficient_quota");
    assert_eq!(body["error"]["message"], "The organization token budget for 2026-10 is used up. It resets at 2026-11-01T00:00:00Z.");
    assert_eq!(refused.header("paigasus-retryable"), "false");
    assert_eq!(refused.header("x-should-retry"), "false");
    assert_eq!(refused.header("retry-after"), "", "no Retry-After on a budget refusal");
    assert_eq!(mock.request_count(), 1);
}

/// Send one non-stream request through a recording store with a large budget; return the status
/// and the charges.
async fn charged(base_url: &str, first_byte: Duration) -> (StatusCode, Vec<u64>) {
    let store = RecordingStore::new();
    let app = app_with(ScopedIam::new(PRINCIPAL_1, ORG_A_PRN), base_url, Some(limits(rules(None, None, Some(1_000_000)), store.clone())), first_byte, true);
    let sent = send(&app, NON_STREAM_BODY).await;
    (sent.status, store.charges())
}

/// A4, non-stream rows. The request estimate of `NON_STREAM_BODY` is ceil(2 / 4) = 1.
#[tokio::test]
async fn a_non_stream_answer_with_usage_charges_the_reported_tokens() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, USAGE_BODY).await;
    assert_eq!(charged(&mock.base_url, Duration::from_secs(30)).await, (StatusCode::OK, vec![5]));
}

#[tokio::test]
async fn a_non_stream_answer_without_usage_charges_the_estimate() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, r#"{"choices":[{"index":0,"message":{"role":"assistant","content":"abcdefgh"}}]}"#).await;
    assert_eq!(charged(&mock.base_url, Duration::from_secs(30)).await, (StatusCode::OK, vec![1 + 2]));
}

#[tokio::test]
async fn a_non_stream_timeout_charges_the_request_estimate() {
    let mock = MockOpenAi::spawn_delayed_json(Duration::from_secs(3), StatusCode::OK, USAGE_BODY).await;
    assert_eq!(charged(&mock.base_url, Duration::from_secs(1)).await, (StatusCode::GATEWAY_TIMEOUT, vec![1]));
}

#[tokio::test]
async fn a_connect_failure_charges_nothing() {
    assert_eq!(charged("http://127.0.0.1:1", Duration::from_secs(30)).await, (StatusCode::BAD_GATEWAY, vec![]));
}

#[tokio::test]
async fn an_upstream_429_charges_nothing() {
    let mock = MockOpenAi::spawn_json(StatusCode::TOO_MANY_REQUESTS, r#"{"error":{"message":"quota","type":"insufficient_quota","param":null,"code":"insufficient_quota"}}"#).await;
    assert_eq!(charged(&mock.base_url, Duration::from_secs(30)).await, (StatusCode::TOO_MANY_REQUESTS, vec![]));
}

/// A5, non-stream: the body is byte-identical with and without limits, and a success carries no
/// limit header.
#[tokio::test]
async fn a_non_stream_body_is_byte_identical_with_limits() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, USAGE_BODY).await;
    let plain = send(&app(ScopedIam::new(PRINCIPAL_1, ORG_A_PRN), &mock.base_url, None), NON_STREAM_BODY).await;
    let limited = send(&app(ScopedIam::new(PRINCIPAL_1, ORG_A_PRN), &mock.base_url, Some(limits(rules(Some(100), Some(100), Some(1_000_000)), memory()))), NON_STREAM_BODY).await;
    assert_eq!((plain.status, &plain.body), (limited.status, &limited.body));
    assert_eq!(limited.header("x-should-retry"), "");
    assert_eq!(limited.header("retry-after"), "");
}

/// A12: a failing store admits, reaches the upstream, never charges, and logs the fail-open spend.
#[tokio::test]
async fn a_failing_store_admits_and_logs_store_unavailable() {
    let (logs, _guard) = capture_logs_at(tracing::Level::INFO);
    let mock = MockOpenAi::spawn_json(StatusCode::OK, USAGE_BODY).await;
    let store = FailingStore::new(UnavailableKind::Server);
    let app = app(ScopedIam::new(PRINCIPAL_1, ORG_A_PRN), &mock.base_url, Some(limits(rules(Some(1), Some(1), Some(1)), store.clone())));
    for _ in 0..2 {
        assert_eq!(send(&app, NON_STREAM_BODY).await.status, StatusCode::OK, "fail-open: no limit applies");
    }
    assert_eq!(mock.request_count(), 2);
    assert_eq!(store.charges(), 0, "no ticket, no charge");
    let text = logs.text();
    let line = text.lines().find(|l| l.contains("chat completion metered")).expect("the metered line");
    assert!(line.contains("outcome=\"store_unavailable\"") && line.contains("tokens=5"), "{line}");
}

/// D11: an exempt org with no principal rate makes no store call.
#[tokio::test]
async fn an_exempt_org_makes_no_store_call() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, USAGE_BODY).await;
    let store = RecordingStore::new();
    let app = app(ScopedIam::new(PRINCIPAL_1, ORG_A_PRN), &mock.base_url, Some(limits(rules_with_exempt(rules(None, Some(1), Some(1)), ORG_A), store.clone())));
    for _ in 0..3 {
        assert_eq!(send(&app, NON_STREAM_BODY).await.status, StatusCode::OK);
    }
    assert_eq!((store.checks(), store.charges()), (0, vec![]));
}

/// D9: a refused body and a refused stream do not use quota.
#[tokio::test]
async fn a_refused_body_or_stream_does_not_use_quota() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, "{}").await;
    let app = app_with(ScopedIam::new(PRINCIPAL_1, ORG_A_PRN), &mock.base_url, Some(limits(rules(Some(1), None, None), memory())), Duration::from_secs(30), false);
    assert_eq!(send(&app, "{not json").await.status, StatusCode::BAD_REQUEST);
    assert_eq!(send(&app, STREAM_BODY).await.json()["error"]["code"], "streaming-disabled");
    assert_eq!(send(&app, NON_STREAM_BODY).await.status, StatusCode::OK, "the one slot is still free");
    assert_eq!(send(&app, NON_STREAM_BODY).await.status, StatusCode::TOO_MANY_REQUESTS);
}

/// D2/Q11: a scope with no org and a budget configured is `500 internal`, with no upstream call.
#[tokio::test]
async fn an_unscoped_request_with_a_budget_is_500_internal() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, USAGE_BODY).await;
    let app = app(ScopedIam::new(PRINCIPAL_1, UNSCOPED), &mock.base_url, Some(limits(rules(None, None, Some(10)), memory())));
    let refused = send(&app, NON_STREAM_BODY).await;
    assert_eq!(refused.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(refused.json()["error"]["code"], "internal");
    assert_eq!(mock.request_count(), 0);
}
```

- [ ] **Step 3: Run them to verify they fail**

```bash
cd rs && cargo nextest run --locked -p paigasus-gateway --test limits_http
```

Expected: compile FAIL (`struct AppState has no field named limits`).

- [ ] **Step 4: Add `AppState.limits` and fix every struct literal**

In `src/adapters/http/mod.rs`, add `use crate::application::limits::Limits;` and, at the end of `AppState`:

```rust
    /// SMA-677: the rate limit and token budget, or `None` when `[limits]` is absent — then the
    /// handler skips them entirely and behaves exactly as before (A6).
    pub limits: Option<Arc<Limits>>,
```

Add `limits: None,` to every `AppState { … }` literal: `src/adapters/http/mod.rs` (`state_with_iam`), `src/main.rs:87-92`, `tests/chat_proxy.rs:165`, `tests/metrics.rs` (three), `tests/service_info.rs:156`. Then run `cd rs && cargo build --locked -p paigasus-gateway --tests` and fix any literal the compiler still names. Change no assertion.

- [ ] **Step 5: Wire the handler**

In `src/adapters/http/chat.rs`, extend the imports:

```rust
use crate::adapters::http::usage::{self, BodyUsage};
use crate::adapters::openai::{ChatResponse, OpenAiByteStream, OpenAiError};
use crate::application::charge_guard::ChargeGuard;
```

After `let stream = dto.stream;` add `let messages = dto.messages;`. After the streaming check (`chat.rs:109-111`) and before `let started = Instant::now();`:

```rust
    // SMA-677 D9: the limits run after the body and streaming checks and just before egress, so a
    // refused body never uses quota. `None` (no `[limits]` table) skips them entirely (A6).
    let mut guard = match &state.limits {
        None => None,
        Some(limits) => match limits.admit(&caller).await {
            Ok(guard) => Some(guard),
            Err(refusal) => {
                // The refusal metric is the operator's signal; this line stays at debug.
                tracing::debug!(principal = %caller.principal_prn, scope = %caller.scope_prn, reason = refusal.label(), "chat completion refused by a limit");
                return GatewayError::from(refusal).into_response();
            }
        },
    };
    if let Some(guard) = guard.as_mut() {
        guard.set_request_estimate(usage::request_tokens_estimate(&messages, body.len()));
        // The ids exist here; inside the stream `current_ids()` is `None` (SMA-504 § 4.3).
        guard.set_ids(paigasus_observability::current_ids());
        guard.mark_sent();
    }
```

Replace the `match result { … }` arms:

```rust
    let (response, status) = match result {
        Ok(ChatResponse::Full { status, body }) => {
            record_upstream_call(status, started);
            // A4: a 2xx reads a COPY of the body for its usage; a non-2xx charges nothing.
            if let Some(guard) = guard.as_mut() {
                if status.is_success() {
                    match usage::non_stream_usage(&body) {
                        BodyUsage::Reported(total) => guard.set_reported(total),
                        BodyUsage::Completion(tokens) => guard.set_completion_estimate(tokens),
                    }
                } else {
                    guard.mark_no_charge();
                }
            }
            let resp = (status, [(header::CONTENT_TYPE, "application/json")], body).into_response();
            (resp, status)
        }
        Ok(ChatResponse::Stream { status, stream }) => {
            record_upstream_call(status, started);
            let resp = if status.is_success() {
                // The guard moves into the stream, so it drops — and charges — when the stream ends,
                // fails, or is dropped by a client disconnect.
                (status, [(header::CONTENT_TYPE, "text/event-stream")], Body::from_stream(terminal_sse_error_stream(stream, guard.take()))).into_response()
            } else {
                if let Some(guard) = guard.as_mut() {
                    guard.mark_no_charge();
                }
                (status, [(header::CONTENT_TYPE, "application/json")], Body::from_stream(stream)).into_response()
            };
            (resp, status)
        }
        Err(err) => {
            // A4: a connect failure never reached the upstream; any other failure after the send
            // (timeout, transport) keeps the request estimate.
            if matches!(err, OpenAiError::Connect(_))
                && let Some(guard) = guard.as_mut()
            {
                guard.mark_no_charge();
            }
            let resp = GatewayError::from(err).into_response();
            let status = resp.status();
            record_upstream_call(status, started);
            (resp, status)
        }
    };
```

Keep every existing comment of those arms (the TTFB notes and the SMA-504 situation-3 note) in place above the matching lines.

Replace `StreamState` and `terminal_sse_error_stream`:

```rust
/// The small state machine [`terminal_sse_error_stream`] unfolds over: still forwarding upstream
/// chunks, or terminated (after emitting the terminal error event, or after the upstream ended).
enum StreamState {
    Streaming(OpenAiByteStream),
    Done,
}

/// The unfold state. The charge guard is a FIELD here, not a `Drop` on `StreamState`: the closure
/// destructures the state by move, which a type with `Drop` forbids (E0509). When the stream ends,
/// fails, or is dropped, this value drops once, and the guard with it (SMA-677 § 4.6).
struct Relay {
    phase: StreamState,
    guard: Option<ChargeGuard>,
}

fn terminal_sse_error_stream(inner: OpenAiByteStream, guard: Option<ChargeGuard>) -> impl Stream<Item = Result<Bytes, Infallible>> + Send + 'static {
    let start = Relay { phase: StreamState::Streaming(inner), guard };
    futures::stream::unfold(start, |Relay { phase, guard }| async move {
        match phase {
            StreamState::Streaming(mut inner) => match inner.next().await {
                // Forward each chunk exactly as it arrived (unbuffered).
                Some(Ok(chunk)) => Some((Ok(chunk), Relay { phase: StreamState::Streaming(inner), guard })),
                // First upstream error: emit the terminal event, then end.
                Some(Err(_)) => Some((Ok(Bytes::from_static(TERMINAL_SSE_ERROR.as_bytes())), Relay { phase: StreamState::Done, guard })),
                // Clean upstream end.
                None => None,
            },
            StreamState::Done => None,
        }
    })
}
```

Keep the existing doc comment of `terminal_sse_error_stream` above it. In its three unit tests, change `terminal_sse_error_stream(inner)` to `terminal_sse_error_stream(inner, None)`.

- [ ] **Step 6: Run the tests to verify they pass**

```bash
cd rs && cargo fmt && cargo nextest run --locked -p paigasus-gateway && cargo clippy --locked -p paigasus-gateway --all-targets -- -D warnings
```

Expected: PASS — the new `limits_http` tests and every existing test with no assertion change (A6). `git diff --stat -- rs/crates/services/paigasus-gateway/tests/chat_proxy.rs rs/crates/services/paigasus-gateway/tests/metrics.rs rs/crates/services/paigasus-gateway/tests/service_info.rs` shows only `limits: None` lines and the moved helper.

- [ ] **Step 7: Prove the wiring bites (mutations 1 and 15)**

1. Replace `Some(limits) => match limits.admit(&caller).await {` … with `Some(_) => None,` (delete the admit call). Run `cargo nextest run --locked -p paigasus-gateway --test limits_http --no-fail-fast`. Expected: FAIL in `the_request_over_the_principal_rate_is_429_rate_limited`, `the_org_rate_is_shared_by_every_principal_of_the_org` and `the_request_after_the_budget_is_used_is_429_budget_exhausted`. Restore.
2. In `Limits::admit` (Task 7), change the `Err(LimitStoreError::Unavailable { kind, detail }) =>` arm to `return Err(LimitRefusal::Unscoped)` after the report call. Expected: FAIL in `a_failing_store_admits_and_logs_store_unavailable` and in `application::limits::tests::a_store_failure_admits_without_a_ticket_and_counts_the_kind`. Restore.

Restore each mutation with the editor, not `git checkout --`.

- [ ] **Step 8: Commit**

```bash
git add rs/crates/services/paigasus-gateway/src rs/crates/services/paigasus-gateway/tests
git commit -m "feat(rs): apply the gateway limits in the chat handler

The handler admits through Limits after the body and streaming checks,
maps a refusal to its 429 or 500, and gives the charge guard the request
estimate, the usage of a non-stream answer and the stream (SMA-677).

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 10: Charge a stream from its usage record

**Files:**
- Modify: `rs/crates/services/paigasus-gateway/src/adapters/http/chat.rs` (`Relay`, `terminal_sse_error_stream`; one new unit test)
- Modify: `rs/crates/services/paigasus-gateway/tests/limits_http.rs` (stream rows)

**Interfaces:**
- Consumes: Task 8's `UsageScanner`; Task 9's `Relay` and `ChargeGuard::set_stream_progress` (Task 7).
- Produces: `struct Relay { phase: StreamState, scan: UsageScanner, guard: Option<ChargeGuard> }`. The scanner runs only when a guard exists, so a deployment without `[limits]` parses nothing (A6).

- [ ] **Step 1: Write the failing stream tests**

Add to the imports of `tests/limits_http.rs`:

```rust
use futures::StreamExt;
use tower::ServiceExt;
```

Append:

```rust
/// Send `STREAM_BODY` through a recording store with a large budget; read the whole stream.
async fn stream_charged(base_url: &str) -> (Sent, Vec<u64>) {
    let store = RecordingStore::new();
    let app = app(ScopedIam::new(PRINCIPAL_1, ORG_A_PRN), base_url, Some(limits(rules(None, None, Some(1_000_000)), store.clone())));
    let sent = send(&app, STREAM_BODY).await;
    (sent, store.charges())
}

/// A4: a stream with a usage record charges the reported total.
#[tokio::test]
async fn a_stream_with_a_usage_record_charges_the_reported_tokens() {
    let mock = MockOpenAi::spawn_raw_sse(vec![chunk("Hel", true), chunk("lo", true), usage_chunk(7), "data: [DONE]\n\n".to_owned()]).await;
    let (sent, charges) = stream_charged(&mock.base_url).await;
    assert_eq!(sent.status, StatusCode::OK);
    assert_eq!(charges, vec![7]);
}

/// A4/D14: no usage → request estimate (1) + one token per record, with and without `"usage":null`.
#[tokio::test]
async fn a_stream_without_usage_charges_one_token_per_record() {
    let mock = MockOpenAi::spawn_raw_sse(vec![chunk("a", false), chunk("b", true), chunk("c", true), "data: [DONE]\n\n".to_owned()]).await;
    assert_eq!(stream_charged(&mock.base_url).await.1, vec![1 + 3]);
}

/// § 4.6: CRLF record delimiters (vLLM, LiteLLM; SMA-558).
#[tokio::test]
async fn crlf_records_are_counted_and_a_crlf_usage_record_is_read() {
    let crlf = |record: String| record.replace("\n\n", "\r\n\r\n");
    let estimated = MockOpenAi::spawn_raw_sse(vec![crlf(chunk("a", true)), crlf(chunk("b", true)), "data: [DONE]\r\n\r\n".to_owned()]).await;
    assert_eq!(stream_charged(&estimated.base_url).await.1, vec![1 + 2]);
    let reported = MockOpenAi::spawn_raw_sse(vec![crlf(chunk("a", true)), crlf(usage_chunk(9)), "data: [DONE]\r\n\r\n".to_owned()]).await;
    assert_eq!(stream_charged(&reported.base_url).await.1, vec![9]);
}

/// A4: a mid-stream upstream error charges exactly once (two records forwarded + the estimate).
#[tokio::test]
async fn a_mid_stream_error_charges_exactly_once() {
    let base_url = support::spawn_truncated_sse().await;
    let (sent, charges) = stream_charged(&base_url).await;
    assert!(sent.text().contains("\"code\":\"upstream-error\""), "{}", sent.text());
    assert_eq!(charges, vec![1 + 2]);
}

/// A4: a client that disconnects mid-stream is charged the estimate, once, when the body drops.
#[tokio::test]
async fn a_client_disconnect_mid_stream_charges_the_estimate_once() {
    let (mock, _cancelled) = MockOpenAi::spawn_abortable_stream().await;
    let store = RecordingStore::new();
    let app = app(ScopedIam::new(PRINCIPAL_1, ORG_A_PRN), &mock.base_url, Some(limits(rules(None, None, Some(1_000_000)), store.clone())));
    let resp = app.oneshot(post(STREAM_BODY)).await.expect("the router answers");
    let mut body = resp.into_body().into_data_stream();
    let first = body.next().await.expect("a first frame").expect("not a transport error");
    assert!(first.starts_with(b"data: hold"), "{first:?}");
    assert!(store.charges().is_empty(), "nothing is charged while the stream is open");
    drop(body);
    assert_eq!(store.charges(), vec![1 + 1], "the request estimate plus the one forwarded record");
}

/// A5, stream: the SSE bytes are byte-identical with and without limits.
#[tokio::test]
async fn the_sse_bytes_are_byte_identical_with_limits() {
    let records = vec![chunk("Hel", true), chunk("lo", true), usage_chunk(7), "data: [DONE]\n\n".to_owned()];
    let mock = MockOpenAi::spawn_raw_sse(records.clone()).await;
    let plain = send(&app(ScopedIam::new(PRINCIPAL_1, ORG_A_PRN), &mock.base_url, None), STREAM_BODY).await;
    let limited = send(&app(ScopedIam::new(PRINCIPAL_1, ORG_A_PRN), &mock.base_url, Some(limits(rules(Some(100), Some(100), Some(1_000_000)), memory()))), STREAM_BODY).await;
    assert_eq!(plain.body, limited.body);
    assert_eq!(limited.text(), records.concat());
}
```

- [ ] **Step 2: Run them to verify they fail**

```bash
cd rs && cargo nextest run --locked -p paigasus-gateway --test limits_http --no-fail-fast
```

Expected: `a_stream_with_a_usage_record_charges_the_reported_tokens` FAILS (charges `[1]`, the request estimate only), and so do the two record-count rows, the CRLF test, the mid-stream error test (`[1]`) and the disconnect test (`[1]`). The byte-identity test passes already.

- [ ] **Step 3: Feed the scanner from the relay**

In `src/adapters/http/chat.rs`, extend the import to `use crate::adapters::http::usage::{self, BodyUsage, UsageScanner};` and replace `Relay` and `terminal_sse_error_stream`:

```rust
/// The unfold state. The charge guard is a FIELD here, not a `Drop` on `StreamState`: the closure
/// destructures the state by move, which a type with `Drop` forbids (E0509). When the stream ends,
/// fails, or is dropped, this value drops once, and the guard with it (SMA-677 § 4.6).
struct Relay {
    phase: StreamState,
    scan: UsageScanner,
    guard: Option<ChargeGuard>,
}

fn terminal_sse_error_stream(inner: OpenAiByteStream, guard: Option<ChargeGuard>) -> impl Stream<Item = Result<Bytes, Infallible>> + Send + 'static {
    let start = Relay { phase: StreamState::Streaming(inner), scan: UsageScanner::new(), guard };
    futures::stream::unfold(start, |Relay { phase, mut scan, mut guard }| async move {
        match phase {
            StreamState::Streaming(mut inner) => match inner.next().await {
                // Forward each chunk exactly as it arrived (unbuffered). The scanner reads a copy,
                // and only when a guard exists (no `[limits]`: no parsing at all).
                Some(Ok(chunk)) => {
                    if let Some(guard) = guard.as_mut() {
                        scan.feed(&chunk);
                        guard.set_stream_progress(scan.completion_records(), scan.reported_total());
                    }
                    Some((Ok(chunk), Relay { phase: StreamState::Streaming(inner), scan, guard }))
                }
                // First upstream error: emit the terminal event, then end.
                Some(Err(_)) => Some((Ok(Bytes::from_static(TERMINAL_SSE_ERROR.as_bytes())), Relay { phase: StreamState::Done, scan, guard })),
                // Clean upstream end.
                None => None,
            },
            StreamState::Done => None,
        }
    })
}
```

Add a unit test to the `tests` module of `chat.rs`:

```rust
    /// § 4.6: the relay never changes a forwarded byte, with or without a scanner (A5).
    #[tokio::test]
    async fn the_relay_forwards_bytes_unchanged() {
        let chunks = vec![Ok(Bytes::from_static(b"data: {\"usage\":{\"total_tokens\":3}}\r\n")), Ok(Bytes::from_static(b"\r\ndata: [DONE]\n\n"))];
        let out: Vec<Bytes> = terminal_sse_error_stream(futures::stream::iter(chunks).boxed(), None).map(|r| r.unwrap()).collect().await;
        assert_eq!(out.concat(), b"data: {\"usage\":{\"total_tokens\":3}}\r\n\r\ndata: [DONE]\n\n".to_vec());
    }
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cd rs && cargo fmt && cargo nextest run --locked -p paigasus-gateway && cargo clippy --locked -p paigasus-gateway --all-targets -- -D warnings
```

Expected: PASS, every existing chat test included.

- [ ] **Step 5: Prove it bites (mutations 3 and 4)**

1. In `ChargeGuard::drop` (Task 7), delete the line `self.limits.store().charge(ticket, tokens, self.limits.clock().now());`. Run `cargo nextest run --locked -p paigasus-gateway --no-fail-fast`. Expected: FAIL in `a_client_disconnect_mid_stream_charges_the_estimate_once` and `a_non_stream_timeout_charges_the_request_estimate`. Restore.
2. In `terminal_sse_error_stream`, delete `scan.feed(&chunk);`. Expected: FAIL in `a_stream_with_a_usage_record_charges_the_reported_tokens` (it sees `[1]`, an estimate). Restore.

- [ ] **Step 6: Commit**

```bash
git add rs/crates/services/paigasus-gateway/src/adapters/http/chat.rs rs/crates/services/paigasus-gateway/tests/limits_http.rs
git commit -m "feat(rs): charge a gateway stream from its usage record

The stream relay feeds a copy of every chunk to the usage scanner, so a
stream is charged its reported total, or one token per record, once,
also on a mid-stream error or a client disconnect (SMA-677).

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 11: The Redis limit store

**Files:**
- Modify: `rs/crates/libs/paigasus-redis/Cargo.toml` (its `redis` line gains `features = ["script"]`, D19)
- Modify: `rs/crates/services/paigasus-gateway/Cargo.toml` (`paigasus-redis`, `redis`, `tokio-util`; dev: `paigasus-redis` with `test-support`)
- Modify: `rs/Cargo.lock` (gains `sha1_smol`)
- Modify: `rs/crates/services/paigasus-gateway/moon.yml` (`dependsOn`, `fileGroups.upstreams`)
- Modify: `ts/apps/gateway-console/moon.yml` (the e2e input list, after line 450)
- Modify: `ci/affected-graph/run.sh`, `ci/affected-graph/README.md` (the `redis->services` case from PR 1 gains the gateway and the console)
- Create: `rs/crates/services/paigasus-gateway/src/adapters/limits/redis.rs`
- Modify: `rs/crates/services/paigasus-gateway/src/adapters/limits/mod.rs`

**Interfaces:**
- Consumes: PR 1's `paigasus_redis::{connect, RedisHandle, BreakerMetrics, BREAKER_OPEN_MESSAGE}` and, in tests, `paigasus_redis::{new_lazy_for_tests, with_open_breaker_for_tests, test_support::start}`; Task 7's `report_store_unavailable`, `StoreOp`, `LIMITS_BREAKER_ROLE`, `Limits`, `ChargeGuard::new`.
- Produces (in `paigasus_gateway::adapters::limits::redis`, re-exported as `paigasus_gateway::adapters::limits::RedisLimitStore`):
  - `const KEY_PREFIX: &str = "paigasus:gateway:limits:v1:"`; `fn principal_rate_key(principal: &str, window: u64) -> String`; `fn org_rate_key(org: Uuid, window: u64) -> String`; `fn budget_key(org: Uuid, period: PeriodKey) -> String` (`…:budget:<org>:d2026-10-02` / `w2026-W40` / `m2026-10`)
  - `fn breaker_metrics() -> BreakerMetrics` (gateway names, `role = "limits"`)
  - `#[derive(Clone)] struct RedisLimitStore` with `async fn connect(redis_url: &str) -> redis::RedisResult<RedisLimitStore>` (eager), `fn from_handle(handle: RedisHandle) -> RedisLimitStore`, `async fn apply_charge(&self, ticket: LimitTicket, tokens: u64, now: SystemTime) -> Result<(), LimitStoreError>`, `fn tracker(&self) -> TaskTracker`, `impl LimitStore`

No gateway-side `tokio::time::timeout` around a store call: a dropped `ProbePermit` records nothing, so the breaker would never open (D20).

- [ ] **Step 1: Add the dependencies and the Moon edges**

`rs/crates/libs/paigasus-redis/Cargo.toml` — change only the `redis` line (keep PR 1's comment above it, and add one line):

```toml
# SMA-677 D19: `script` gives `redis::Script` (EVALSHA with a NOSCRIPT → SCRIPT LOAD retry) to the
# gateway's limit store. Added here, not in the workspace table, so the workspace comment that
# trims `script` stays true for every other consumer. Cargo unions features, so IAM builds it too;
# that changes no IAM behavior.
redis = { workspace = true, features = ["script"] }
```

`rs/crates/services/paigasus-gateway/Cargo.toml`, `[dependencies]`:

```toml
# SMA-677 D15: the one Redis connection site (tuned ConnectionManager + the SMA-476 breaker).
# `{ workspace = true }`, so `moon.yml` hand-declares the `dependsOn` edge (ci/CLAUDE.md).
paigasus-redis = { workspace = true }
# SMA-677: the limit store names `redis::Script`, `redis::pipe` and `redis::RedisError`. The
# `script` feature comes from paigasus-redis's own line (D19).
redis = { workspace = true }
# SMA-677 D16: `TaskTracker` for the spawned charges and the 5 s shutdown drain. `rt` adds only
# tokio/rt, tokio/sync and futures-util, all already in Cargo.lock.
tokio-util = { version = "0.7", features = ["rt"] }
```

`[dev-dependencies]`:

```toml
# SMA-677: the blackhole listener and the lazy / open-breaker handles for the fail-open tests.
# Test builds only, like paigasus-logging/test-support above.
paigasus-redis = { workspace = true, features = ["test-support"] }
```

`rs/crates/services/paigasus-gateway/moon.yml`: add `- 'paigasus-redis-rs'` to `dependsOn` (after `paigasus-proto-rs`, with a one-line comment `# SMA-677: { workspace = true } in Cargo.toml, so this edge is hand-written.`), and add to `fileGroups.upstreams`, after the two `paigasus-proto/` lines:

```yaml
    - '/rs/crates/libs/paigasus-redis/src/**/*'
    - '/rs/crates/libs/paigasus-redis/Cargo.toml'
```

`ts/apps/gateway-console/moon.yml`, after `- '/rs/crates/libs/paigasus-proto/Cargo.toml'` (line 450) in the list that mirrors the gateway's upstreams:

```yaml
      - '/rs/crates/libs/paigasus-redis/src/**/*'
      - '/rs/crates/libs/paigasus-redis/Cargo.toml'
```

(`paigasus-test-docker` is a dev-dependency only; the e2e tier runs the gateway binary, which does not link it, so the console list does not get its lines. Task 12 adds it to the gateway's own `moon.yml`, where A6 asserts the closure.)

`ci/affected-graph/run.sh`: PR 1 (SMA-726) added the `redis->services` case with only `"paigasus-redis-rs,paigasus-iam-rs"`, and its comment says that SMA-677 PR 2 adds the gateway. After the edges above, an edit of `paigasus-redis/src/lib.rs` also selects the gateway and the console, so the old expected set makes `repo:affected-smoke` fail. Change the case's expected set, and the comment above it, to:

```bash
  # paigasus-redis edit -> the lib + every service that opens Redis through it (SMA-726): IAM, and
  # the gateway's limit store (SMA-677), plus gateway-console-ts, whose playground project runs the
  # real gateway binary. One-directional: the lib has no in-tree dependency.
  run_case "redis->services" "rs/crates/libs/paigasus-redis/src/lib.rs" \
    "paigasus-redis-rs,paigasus-iam-rs,paigasus-gateway-rs,gateway-console-ts"
```

In `ci/affected-graph/README.md`, change the **redis lib edit** bullet to `→ paigasus-redis-rs + paigasus-iam-rs + paigasus-gateway-rs + gateway-console-ts (SMA-726, SMA-677).` Measure the case before you trust the list: run `/bin/bash ci/affected-graph/run.sh` (system bash 3.2; Homebrew bash 5 deadlocks on this host) and read the `redis->services` row. If the measured set differs from the list above, write the measured set and record the difference in the commit body. Add both files to this task's `git add`.

```bash
cd rs && cargo check -p paigasus-gateway   # no --locked here: this run records sha1_smol in Cargo.lock
grep -n 'name = "sha1_smol"' Cargo.lock
```

Expected: `sha1_smol` is now in `rs/Cargo.lock` (the `script` feature pulls it in, D19).

- [ ] **Step 2: Write the failing unit tests (no Docker)**

Create `src/adapters/limits/redis.rs` with the doc comment and the tests:

```rust
// SPDX-License-Identifier: Apache-2.0

//! `RedisLimitStore` (SMA-677 § 4.3, D16, D18, D19, D20): the limit counts on a shared Redis, so
//! every replica that points at it shares one set of counts, and a gateway restart keeps them.
//!
//! One Lua script per request reads every count, decides, and increments the rate keys only when
//! every check passes (D4); Redis runs a script atomically, so two replicas cannot both take the
//! last slot. A charge is `MULTI INCRBY EXPIRE EXEC`, spawned from `charge` on a `TaskTracker`.
//! Every instant comes from the gateway's clock; the script never calls `TIME` (D18 "Clock").
//! The connection is `paigasus_redis::connect`, with the SMA-473 budget and the SMA-476 breaker;
//! its series carry the gateway's names and `role="limits"`. A failure maps to
//! `LimitStoreError::Unavailable` and the request is admitted (fail-open, D10).

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::charge_guard::{ChargeGuard, NoTicket};
    use crate::application::limits::Limits;
    use crate::domain::limits::{BudgetPeriod, LimitRules};
    use crate::test_support::{FixedClock, ORG_A, ORG_A_PRN, at, caller, counter};
    use metrics_util::debugging::DebuggingRecorder;

    fn nz(n: u64) -> NonZeroU64 {
        NonZeroU64::new(n).expect("non-zero")
    }

    #[test]
    fn errors_map_to_their_kinds() {
        let io = redis::RedisError::from(std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "connection refused"));
        let short_circuit = redis::RedisError::from((redis::ErrorKind::Io, paigasus_redis::BREAKER_OPEN_MESSAGE));
        let server = redis::RedisError::from((redis::ErrorKind::Server(redis::ServerErrorKind::ResponseError), "OOM command not allowed when used memory > 'maxmemory'"));
        let decode = redis::RedisError::from((redis::ErrorKind::UnexpectedReturnType, "not an array of integers"));
        let parse = redis::RedisError::from((redis::ErrorKind::Parse, "bad reply"));
        let kinds = [io, short_circuit, server, decode, parse].map(|err| unavailable_kind(&err));
        assert_eq!(kinds, [UnavailableKind::Io, UnavailableKind::Io, UnavailableKind::Server, UnavailableKind::Decode, UnavailableKind::Decode]);
    }

    #[test]
    fn the_keys_follow_the_v1_format() {
        let org = ORG_A;
        assert_eq!(principal_rate_key("prn:pgs:iam:::principal/p", 7), "paigasus:gateway:limits:v1:rate:p:prn:pgs:iam:::principal/p:7");
        assert_eq!(org_rate_key(org, 7), "paigasus:gateway:limits:v1:rate:o:0190a100-0000-7000-8000-0000000000a1:7");
        let noon = at("2026-10-02T12:00:00Z");
        assert_eq!(budget_key(org, BudgetPeriod::Daily.key_at(noon)), "paigasus:gateway:limits:v1:budget:0190a100-0000-7000-8000-0000000000a1:d2026-10-02");
        assert_eq!(budget_key(org, BudgetPeriod::Weekly.key_at(noon)), "paigasus:gateway:limits:v1:budget:0190a100-0000-7000-8000-0000000000a1:w2026-W40");
        assert_eq!(budget_key(org, BudgetPeriod::Monthly.key_at(noon)), "paigasus:gateway:limits:v1:budget:0190a100-0000-7000-8000-0000000000a1:m2026-10");
    }

    #[test]
    fn a_script_reply_becomes_a_decision() {
        let at_noon = WindowIndex::at(at("2026-10-02T12:00:30Z"));
        let budget = Budget { tokens: nz(10), period: BudgetPeriod::Monthly };
        let key = BudgetPeriod::Monthly.key_at(at("2026-10-02T12:00:30Z"));
        let admitted = decision_from_reply(&[1, 1, 0, 0, 1, 0, 0, 1, 3], at_noon, Some(nz(5)), Some(nz(5)), Some((ORG_A, budget, key))).expect("a valid reply");
        assert_eq!(admitted, LimitDecision::Admit(Some(LimitTicket { org: ORG_A, period: key })));
        let refused = decision_from_reply(&[0, 0, 2, 5, 1, 0, 0, 0, 12], at_noon, Some(nz(5)), Some(nz(5)), Some((ORG_A, budget, key))).expect("a valid reply");
        assert_eq!(
            refused,
            LimitDecision::Refused(vec![
                FailedCheck::PrincipalRate(RateCounts { previous: 2, current: 5, elapsed_ms: 30_000, limit: 5 }),
                FailedCheck::Budget { period: BudgetPeriod::Monthly, resets_at_unix: 1_793_491_200 },
            ])
        );
    }

    #[test]
    fn a_reply_of_the_wrong_shape_is_a_decode_failure() {
        let at_noon = WindowIndex::at(at("2026-10-02T12:00:00Z"));
        for reply in [&[1_i64, 1, 0][..], &[0, 0, -1, 5, 1, 0, 0, 1, 0][..]] {
            match decision_from_reply(reply, at_noon, Some(nz(5)), None, None) {
                Err(LimitStoreError::Unavailable { kind: UnavailableKind::Decode, .. }) => {}
                other => panic!("{reply:?} must be a decode failure, got {other:?}"),
            }
        }
    }

    fn budget_rules() -> LimitRules {
        LimitRules { tokens_per_period: Some(nz(100)), ..LimitRules::default() }
    }

    /// A12 on an open breaker: admitted at once, no ticket, no dial, `op="check",kind="io"`.
    #[tokio::test]
    async fn an_open_breaker_admits_without_a_ticket_and_never_dials() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _local = metrics::set_default_local_recorder(&recorder);
        let blackhole = paigasus_redis::test_support::start().await;
        let store = RedisLimitStore::from_handle(paigasus_redis::with_open_breaker_for_tests(&blackhole.url, breaker_metrics()).expect("a lazy handle"));
        let limits = Arc::new(Limits::new(budget_rules(), Arc::new(store), Arc::new(FixedClock(at("2026-10-02T12:00:00Z")))));
        let guard = limits.admit(&caller(ORG_A_PRN)).await.expect("fail-open admits");
        assert!(!guard.has_ticket());
        assert_eq!(blackhole.accepted(), 0, "an open breaker short-circuits with no dial (SMA-702)");
        assert_eq!(counter(&snapshotter, names::GATEWAY_LIMIT_STORE_UNAVAILABLE_TOTAL, &[("op", "check"), ("kind", "io")]), Some(1));
    }

    /// D20: a blackhole with a closed breaker. Each of the first three checks waits for the dial
    /// budget (about 2.1 s) and fails open; then the breaker is open and the fourth dials nothing.
    #[tokio::test]
    async fn a_blackholed_redis_fails_open_until_the_breaker_stops_the_dials() {
        let blackhole = paigasus_redis::test_support::start().await;
        let store = RedisLimitStore::from_handle(paigasus_redis::new_lazy_for_tests(&blackhole.url, breaker_metrics()).expect("a lazy handle"));
        let policy = LimitPolicy { principal_requests_per_minute: Some(nz(5)), ..LimitPolicy::default() };
        let now = at("2026-10-02T12:00:00Z");
        for attempt in 1..=3 {
            match store.check_and_admit("p", None, &policy, now).await {
                Err(LimitStoreError::Unavailable { kind: UnavailableKind::Io, .. }) => {}
                other => panic!("attempt {attempt}: expected an io failure, got {other:?}"),
            }
        }
        let dialled = blackhole.accepted();
        assert!(dialled >= 3, "the closed breaker dialled on every attempt: {dialled}");
        assert!(matches!(store.check_and_admit("p", None, &policy, now).await, Err(LimitStoreError::Unavailable { kind: UnavailableKind::Io, .. })));
        assert_eq!(blackhole.accepted(), dialled, "after three failures the breaker is open: no new dial");
    }

    /// D10: a charge against an open breaker is counted as `op="charge",kind="io"` and never panics.
    #[tokio::test]
    async fn a_charge_against_an_open_breaker_is_counted() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _local = metrics::set_default_local_recorder(&recorder);
        let blackhole = paigasus_redis::test_support::start().await;
        let store = RedisLimitStore::from_handle(paigasus_redis::with_open_breaker_for_tests(&blackhole.url, breaker_metrics()).expect("a lazy handle"));
        let now = at("2026-10-02T12:00:00Z");
        store.charge(LimitTicket { org: ORG_A, period: BudgetPeriod::Monthly.key_at(now) }, 5, now);
        let tracker = store.tracker();
        tracker.close();
        tracker.wait().await;
        assert_eq!(counter(&snapshotter, names::GATEWAY_LIMIT_STORE_UNAVAILABLE_TOTAL, &[("op", "charge"), ("kind", "io")]), Some(1));
    }

    /// D16, spec § 5.8: a guard with a ticket dropped on a plain thread (no runtime) counts
    /// `reason="no_runtime"` and does not panic.
    #[tokio::test]
    async fn a_guard_dropped_without_a_runtime_counts_no_runtime() {
        let blackhole = paigasus_redis::test_support::start().await;
        let store = RedisLimitStore::from_handle(paigasus_redis::with_open_breaker_for_tests(&blackhole.url, breaker_metrics()).expect("a lazy handle"));
        let now = at("2026-10-02T12:00:00Z");
        let limits = Arc::new(Limits::new(budget_rules(), Arc::new(store), Arc::new(FixedClock(now))));
        let mut guard = ChargeGuard::new(limits, Some(LimitTicket { org: ORG_A, period: BudgetPeriod::Monthly.key_at(now) }), Some(ORG_A), NoTicket::NoBudget);
        guard.mark_sent();
        guard.set_reported(5);
        let dropped = std::thread::spawn(move || {
            let recorder = DebuggingRecorder::new();
            let snapshotter = recorder.snapshotter();
            metrics::with_local_recorder(&recorder, || drop(guard));
            counter(&snapshotter, names::GATEWAY_LIMIT_CHARGES_DROPPED_TOTAL, &[("reason", "no_runtime")])
        })
        .join()
        .expect("Drop must not panic without a runtime");
        assert_eq!(dropped, Some(1));
    }
}
```

In `src/adapters/limits/mod.rs` add `pub mod redis;` and `pub use self::redis::RedisLimitStore;`. The `self::` is required: a child module named `redis` next to the `redis` crate makes a bare `use redis::…` ambiguous (E0659).

- [ ] **Step 3: Run them to verify they fail**

```bash
cd rs && cargo nextest run --locked -p paigasus-gateway adapters::limits::redis
```

Expected: compile FAIL (`cannot find function unavailable_kind`, `RedisLimitStore`, …).

- [ ] **Step 4: Write the store**

Insert above `#[cfg(test)]` in `redis.rs`:

```rust
use std::num::NonZeroU64;
use std::sync::Arc;
use std::time::SystemTime;

use metrics::counter;
use paigasus_observability::names;
use paigasus_redis::{BreakerMetrics, RedisHandle};
use tokio_util::task::TaskTracker;
use uuid::Uuid;

use crate::application::limits::{LIMITS_BREAKER_ROLE, StoreOp, report_store_unavailable};
use crate::domain::limits::{
    Budget, BudgetPeriod, ChargeDropReason, FailedCheck, LimitDecision, LimitPolicy, LimitStore, LimitStoreError, LimitTicket, MAX_TOKENS_PER_CHARGE, PeriodKey, RATE_KEY_TTL_SECS, RateCounts, UnavailableKind,
    WindowIndex, budget_refusal,
};

/// D18: every key starts with this. `v1` lets a later format use new keys with no migration.
pub const KEY_PREFIX: &str = "paigasus:gateway:limits:v1:";

/// D18: the admission script. Redis 5.0 commands only (`GET`, `INCR`, `EXPIRE`; no `EXPIRE NX`).
/// It evaluates the same D3 integer form as `domain::limits::admits`; the contract suite catches a
/// difference. D23 bounds every product below 2^53, so Lua's doubles hold it exactly.
const ADMISSION_SCRIPT: &str = r#"
-- KEYS: principal current, principal previous, org current, org previous, budget.
-- ARGV: elapsed_ms, principal limit, org limit, budget limit ('' = no limit), rate TTL.
local elapsed = tonumber(ARGV[1])
local function count(key)
  local value = redis.call('GET', key)
  if value then return tonumber(value) end
  return 0
end
local function rate(current_key, previous_key, limit_arg)
  if limit_arg == '' then return 1, 0, 0 end
  local limit = tonumber(limit_arg)
  local current = count(current_key)
  local previous = count(previous_key)
  if previous * (60000 - elapsed) + current * 60000 + 60000 <= limit * 60000 then
    return 1, previous, current
  end
  return 0, previous, current
end
local p_ok, p_prev, p_cur = rate(KEYS[1], KEYS[2], ARGV[2])
local o_ok, o_prev, o_cur = rate(KEYS[3], KEYS[4], ARGV[3])
local b_ok, b_used = 1, 0
if ARGV[4] ~= '' then
  b_used = count(KEYS[5])
  if b_used >= tonumber(ARGV[4]) then b_ok = 0 end
end
local admit = 0
if p_ok == 1 and o_ok == 1 and b_ok == 1 then
  admit = 1
  local ttl = tonumber(ARGV[5])
  if ARGV[2] ~= '' then
    redis.call('INCR', KEYS[1])
    redis.call('EXPIRE', KEYS[1], ttl)
  end
  if ARGV[3] ~= '' then
    redis.call('INCR', KEYS[3])
    redis.call('EXPIRE', KEYS[3], ttl)
  end
end
return {admit, p_ok, p_prev, p_cur, o_ok, o_prev, o_cur, b_ok, b_used}
"#;

pub fn principal_rate_key(principal: &str, window: u64) -> String {
    format!("{KEY_PREFIX}rate:p:{principal}:{window}")
}

pub fn org_rate_key(org: Uuid, window: u64) -> String {
    format!("{KEY_PREFIX}rate:o:{org}:{window}")
}

/// D6/D18: `…:budget:<org>:d2026-10-02`, `…:w2026-W40` or `…:m2026-10`.
pub fn budget_key(org: Uuid, period: PeriodKey) -> String {
    let kind = match period {
        PeriodKey::Day(_) => 'd',
        PeriodKey::Week { .. } => 'w',
        PeriodKey::Month { .. } => 'm',
    };
    format!("{KEY_PREFIX}budget:{org}:{kind}{}", period.label())
}

/// D15/§ 4.8: the breaker series carry the gateway's names, never IAM's.
pub fn breaker_metrics() -> BreakerMetrics {
    BreakerMetrics { state: names::GATEWAY_REDIS_BREAKER_STATE, transitions: names::GATEWAY_REDIS_BREAKER_TRANSITIONS_TOTAL, role: LIMITS_BREAKER_ROLE }
}

/// D10: `Io` covers every connection failure and the breaker's short-circuit (an `ErrorKind::Io`
/// error); `Decode` a reply of the wrong type; `Server` every other answer (an `OOM` refusal under
/// `noeviction`, a script error, `READONLY` from a replica).
fn unavailable_kind(err: &redis::RedisError) -> UnavailableKind {
    if err.is_io_error() {
        return UnavailableKind::Io;
    }
    match err.kind() {
        redis::ErrorKind::UnexpectedReturnType | redis::ErrorKind::Parse => UnavailableKind::Decode,
        _ => UnavailableKind::Server,
    }
}

/// The error text never holds the URL: `BREAKER_OPEN_MESSAGE` and redis-rs's own texts do not.
fn unavailable(err: &redis::RedisError) -> LimitStoreError {
    LimitStoreError::Unavailable { kind: unavailable_kind(err), detail: err.to_string() }
}

fn decode(detail: String) -> LimitStoreError {
    LimitStoreError::Unavailable { kind: UnavailableKind::Decode, detail }
}

fn non_negative(value: i64) -> Result<u64, LimitStoreError> {
    u64::try_from(value).map_err(|_| decode(format!("the admission script returned a negative count {value}")))
}

fn limit_arg(limit: Option<NonZeroU64>) -> String {
    limit.map(|value| value.get().to_string()).unwrap_or_default()
}

/// Build the decision from the script's nine integers. The script decides; Rust only maps its
/// pass flags and counts to `FailedCheck`s, so the D8 precedence runs in one place for both
/// adapters.
fn decision_from_reply(
    reply: &[i64],
    at: WindowIndex,
    principal_limit: Option<NonZeroU64>,
    org_limit: Option<NonZeroU64>,
    budget: Option<(Uuid, Budget, PeriodKey)>,
) -> Result<LimitDecision, LimitStoreError> {
    let &[admit, p_ok, p_prev, p_cur, o_ok, o_prev, o_cur, b_ok, _b_used] = reply else {
        return Err(decode(format!("the admission script returned {} values, not 9", reply.len())));
    };
    if admit == 1 {
        return Ok(LimitDecision::Admit(budget.map(|(org, _, period)| LimitTicket { org, period })));
    }
    let counts = |previous: i64, current: i64, limit: NonZeroU64| -> Result<RateCounts, LimitStoreError> {
        Ok(RateCounts { previous: non_negative(previous)?, current: non_negative(current)?, elapsed_ms: at.elapsed_ms, limit: limit.get() })
    };
    let mut failed = Vec::new();
    if let (0, Some(limit)) = (p_ok, principal_limit) {
        failed.push(FailedCheck::PrincipalRate(counts(p_prev, p_cur, limit)?));
    }
    if let (0, Some(limit)) = (o_ok, org_limit) {
        failed.push(FailedCheck::OrgRate(counts(o_prev, o_cur, limit)?));
    }
    if let (0, Some((_, budget, key))) = (b_ok, budget) {
        failed.push(budget_refusal(budget, key));
    }
    Ok(LimitDecision::Refused(failed))
}

/// The shared-Redis store. `Clone` shares the handle (one breaker), the script and the tracker.
#[derive(Clone)]
pub struct RedisLimitStore {
    handle: RedisHandle,
    script: Arc<redis::Script>,
    tracker: TaskTracker,
}

impl RedisLimitStore {
    /// D17: eager. A Redis that is down at boot fails the boot, so a wrong URL or password is
    /// found at deploy time (Q14). Call after `paigasus_observability::init`: the breaker sets its
    /// gauge in its constructor.
    pub async fn connect(redis_url: &str) -> redis::RedisResult<Self> {
        Ok(Self::from_handle(paigasus_redis::connect(redis_url, breaker_metrics()).await?))
    }

    pub fn from_handle(handle: RedisHandle) -> Self {
        RedisLimitStore { handle, script: Arc::new(redis::Script::new(ADMISSION_SCRIPT)), tracker: TaskTracker::new() }
    }

    /// D16: the spawned charges, for the shutdown drain (`runtime::supervise`).
    pub fn tracker(&self) -> TaskTracker {
        self.tracker.clone()
    }

    /// D18: `MULTI INCRBY EXPIRE EXEC` through the breaker (`RedisHandle` implements
    /// `req_packed_commands`). The TTL is relative: `period_end + 86400 − now`. A charge more than
    /// a day late writes nothing and is counted (`period_expired`). Zero tokens send nothing.
    pub async fn apply_charge(&self, ticket: LimitTicket, tokens: u64, now: SystemTime) -> Result<(), LimitStoreError> {
        let tokens = tokens.min(MAX_TOKENS_PER_CHARGE);
        if tokens == 0 {
            return Ok(());
        }
        let Some(ttl) = BudgetPeriod::charge_ttl(ticket.period, now) else {
            counter!(names::GATEWAY_LIMIT_CHARGES_DROPPED_TOTAL, "reason" => ChargeDropReason::PeriodExpired.as_label()).increment(1);
            return Ok(());
        };
        let key = budget_key(ticket.org, ticket.period);
        let mut con = self.handle.clone();
        redis::pipe()
            .atomic()
            .incr(&key, tokens)
            .ignore()
            .expire(&key, i64::from(ttl))
            .ignore()
            .query_async::<()>(&mut con)
            .await
            .map_err(|err| unavailable(&err))
    }
}

#[async_trait::async_trait]
impl LimitStore for RedisLimitStore {
    async fn check_and_admit(&self, principal: &str, org: Option<Uuid>, policy: &LimitPolicy, now: SystemTime) -> Result<LimitDecision, LimitStoreError> {
        let at = WindowIndex::at(now);
        let previous = at.index.saturating_sub(1);
        let org_limit = org.and(policy.org_requests_per_minute);
        let budget = policy.budget.zip(org).map(|(budget, id)| (id, budget, budget.period.key_at(now)));
        // A dimension with no limit passes an empty limit; the script never touches its key.
        let (org_current, org_previous) = match org {
            Some(id) => (org_rate_key(id, at.index), org_rate_key(id, previous)),
            None => (format!("{KEY_PREFIX}rate:o:none:{}", at.index), format!("{KEY_PREFIX}rate:o:none:{previous}")),
        };
        let budget_name = budget.map_or_else(|| format!("{KEY_PREFIX}budget:none"), |(id, _, key)| budget_key(id, key));
        let mut invocation = self.script.prepare_invoke();
        invocation
            .key(principal_rate_key(principal, at.index))
            .key(principal_rate_key(principal, previous))
            .key(org_current)
            .key(org_previous)
            .key(budget_name)
            .arg(at.elapsed_ms)
            .arg(limit_arg(policy.principal_requests_per_minute))
            .arg(limit_arg(org_limit))
            .arg(limit_arg(budget.map(|(_, budget, _)| budget.tokens)))
            .arg(RATE_KEY_TTL_SECS);
        // `invoke_async` sends EVALSHA, and on NOSCRIPT sends SCRIPT LOAD and EVALSHA again (D19).
        let mut con = self.handle.clone();
        let reply: Vec<i64> = invocation.invoke_async(&mut con).await.map_err(|err| unavailable(&err))?;
        decision_from_reply(&reply, at, policy.principal_requests_per_minute, org_limit, budget)
    }

    /// D16: never blocks, never panics. With a runtime, the charge runs on the tracker; the task
    /// holds a clone of the handle only, never request state. With no runtime (a drop on a plain
    /// thread), it is counted as `no_runtime` and dropped.
    fn charge(&self, ticket: LimitTicket, tokens: u64, now: SystemTime) {
        if tokens == 0 {
            return;
        }
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            counter!(names::GATEWAY_LIMIT_CHARGES_DROPPED_TOTAL, "reason" => ChargeDropReason::NoRuntime.as_label()).increment(1);
            return;
        };
        let store = self.clone();
        self.tracker.spawn_on(
            async move {
                // No retry: a retry after an ambiguous failure would charge twice (D10).
                if let Err(LimitStoreError::Unavailable { kind, detail }) = store.apply_charge(ticket, tokens, now).await {
                    report_store_unavailable(StoreOp::Charge, kind, &detail);
                }
            },
            &runtime,
        );
    }
}
```

- [ ] **Step 5: Run the tests to verify they pass**

```bash
cd rs && cargo fmt && cargo nextest run --locked -p paigasus-gateway adapters::limits && cargo clippy --locked -p paigasus-gateway --all-targets -- -D warnings
```

Expected: PASS. `a_blackholed_redis_fails_open_until_the_breaker_stops_the_dials` takes about 7 s (three dials of about 2.1 s). If `BreakerMetrics` has private fields in PR 1, or `test_support::start` has another name, STOP and report (an "inferred" interface differs).

- [ ] **Step 6: Prove the mapping and the runtime check bite (mutations 18 and 21)**

1. Replace the `let Ok(runtime) = … else { … };` block and `spawn_on` with a bare `tokio::spawn(async move { … });`. Run `cargo nextest run --locked -p paigasus-gateway adapters::limits::redis --no-fail-fast`. Expected: FAIL in `a_guard_dropped_without_a_runtime_counts_no_runtime` (the thread panics). Restore.
2. Make `unavailable_kind` return `UnavailableKind::Io` for every error. Expected: FAIL in `errors_map_to_their_kinds`. Restore.

- [ ] **Step 7: Run the dependency gates**

```bash
moon run repo:deny repo:osv repo:machete repo:redis-connect-single-site
```

Expected: PASS. Record in the task report: `sha1_smol` licence (BSD-3-Clause, allowed by `rs/deny.toml:23-28`) and that `repo:osv` reports no advisory for it (D19 asks the plan to record both). `repo:redis-connect-single-site` passes because the gateway names no Redis constructor; `RedisLimitStore::connect` calls `paigasus_redis::connect`.

- [ ] **Step 8: Commit**

```bash
git add rs/crates/libs/paigasus-redis/Cargo.toml rs/crates/services/paigasus-gateway/Cargo.toml rs/Cargo.lock rs/crates/services/paigasus-gateway/moon.yml ts/apps/gateway-console/moon.yml ci/affected-graph/run.sh ci/affected-graph/README.md rs/crates/services/paigasus-gateway/src/adapters/limits
git commit -m "feat(rs): add the Redis limit store to the gateway

One Lua admission script that checks every key and then counts, a
MULTI INCRBY EXPIRE charge with a relative TTL, spawned on a TaskTracker,
and the D10 error kinds over the paigasus-redis breaker (SMA-677).

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 12: The Redis tests on Docker, the canary and the nextest policy

**Files:**
- Modify: `rs/crates/libs/paigasus-test-docker/src/lib.rs` (add `start_redis_image_or_skip` after `start_redis_or_skip`; the PR 1 plan leaves it to PR 2)
- Modify: `rs/crates/services/paigasus-gateway/Cargo.toml` (dev: `paigasus-test-docker`, `testcontainers-modules`)
- Modify: `rs/crates/services/paigasus-gateway/moon.yml` (`dependsOn` + `upstreams` for `paigasus-test-docker`; `test.options.mutex`)
- Modify: `rs/.config/nextest.toml` (two overrides, before `[profile.iam]`)
- Create: `rs/crates/services/paigasus-gateway/tests/docker_preflight.rs`, `tests/limits_store_redis.rs`, `tests/limits_redis_e2e.rs`

**Interfaces:**
- Consumes: PR 1's `paigasus_test_docker::{skip_docker, start_or_skip, mapped_port, start_redis_or_skip}`; Task 11's `RedisLimitStore`, key functions; Task 4's contract suite; Task 9's `support::limits`.
- Produces: `paigasus_test_docker::start_redis_image_or_skip(tag: &str, what: &str) -> Option<(ContainerAsync<Redis>, String)>`; the gateway Docker binaries `docker_preflight`, `limits_store_redis`, `limits_redis_e2e` (names that `nextest.toml` and the runbook use).

How to know these tests RAN: a Docker-less local run skips them with exit 0, and nextest hides a passing test's stderr. Run them with `PAIGASUS_REQUIRE_DOCKER=1` (a skip then panics), or include `--test docker_preflight` in the same run (it reds without Docker). Never report a Docker test as passed without one of the two.

- [ ] **Step 1: Add the pinned-image helper, the dev-dependencies, the Moon edges and the nextest overrides**

`rs/crates/libs/paigasus-test-docker/src/lib.rs`: add `use testcontainers::ImageExt;` to the imports, and after `start_redis_or_skip`:

```rust
/// An ephemeral Redis of a pinned image `tag`, plus its URL (SMA-677 D22). `start_redis_or_skip`
/// keeps `Redis::default()`, so IAM's tests keep the image they use today (A13); the gateway pins
/// `6.2-alpine` (the stated minimum) and `7.4-alpine` (the version kind runs) with this one. The
/// skip-versus-fail decision is `start_or_skip`'s, unchanged.
pub async fn start_redis_image_or_skip(tag: &str, what: &str) -> Option<(ContainerAsync<Redis>, String)> {
    let node = start_or_skip(Redis::default().with_tag(tag), what).await?;
    let port = mapped_port(&node, 6379, "redis").await;
    Some((node, format!("redis://127.0.0.1:{port}")))
}
```

```bash
cd rs && cargo clippy --locked -p paigasus-test-docker --all-targets -- -D warnings && cargo nextest run --locked --no-tests=pass -p paigasus-test-docker
```

Expected: PASS (the crate's own tests are Docker-free; Step 3 exercises the new function on both tags).

`rs/crates/services/paigasus-gateway/Cargo.toml`, `[dev-dependencies]`:

```toml
# SMA-677 D22: the one Docker-skip policy (SMA-538), shared with paigasus-iam. Dev-only.
paigasus-test-docker = { workspace = true }
# SMA-677: the Redis image type, `ContainerAsync` and `ExecCommand` for the raw `redis-cli` reads
# (the single-site gate bans a second Rust Redis client in tests). Already in Cargo.lock via IAM.
testcontainers-modules = { version = "0.15", features = ["redis"] }
```

`rs/crates/services/paigasus-gateway/moon.yml`: add `- 'paigasus-test-docker-rs'` to `dependsOn`, and to `fileGroups.upstreams` after the `paigasus-service-info/` lines:

```yaml
    - '/rs/crates/libs/paigasus-test-docker/src/**/*'
    - '/rs/crates/libs/paigasus-test-docker/Cargo.toml'
```

and give the `test` task a mutex:

```yaml
  test:
    deps: ['^:build']
    # SMA-677 D22. This task now starts Redis containers. It shares one CI runner with the two
    # console e2e tasks, and SMA-718 showed that such overlap starves the browser, so it takes the
    # same mutex as paigasus-iam-rs:test.
    options:
      mutex: 'heavy-integration'
```

`rs/.config/nextest.toml`, after the `package(=paigasus-iam) and kind(test)` block and before the `[profile.iam]` comment:

```toml
[[profile.default.overrides]]
# SMA-677 D22: the gateway's Docker canary, in the form of paigasus-iam's above. One retry; it
# inherits `test-group` from the block below (per-setting precedence, specific before general).
filter = 'package(=paigasus-gateway) and binary(docker_preflight)'
retries = 1

[[profile.default.overrides]]
# SMA-677 D22: the gateway's Redis-backed binaries, named one by one so the crate's pure unit
# tests keep retries = 0. Same retry budget and container cap as paigasus-iam's suites.
filter = 'package(=paigasus-gateway) and (binary(docker_preflight) or binary(limits_store_redis) or binary(limits_redis_e2e))'
retries = { backoff = "exponential", count = 2, delay = "15s", max-delay = "60s", jitter = true }
test-group = 'docker-containers'
```

```bash
cd rs && cargo nextest list --locked -p paigasus-gateway -E 'package(=paigasus-gateway) and (binary(docker_preflight) or binary(limits_store_redis) or binary(limits_redis_e2e))' 2>&1 | tail -20
```

Run this after Step 2 creates the files. Expected: exactly the tests of the three new binaries.

- [ ] **Step 2: Write the canary, the store tests and the end-to-end test**

Create `tests/docker_preflight.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0

//! The canary that makes a Docker-less run of this crate impossible to miss (SMA-538, SMA-677
//! D22). `limits_store_redis` and `limits_redis_e2e` each return early when Docker is unavailable,
//! reporting PASS having executed nothing, and nextest and Moon both discard a passing test's
//! output. So this test FAILS instead: one red, named for the actual problem.

#[tokio::test]
async fn docker_backed_suites_can_actually_run() {
    if paigasus_test_docker::skip_docker() {
        eprintln!("SKIP[docker-unavailable] docker_preflight: PAIGASUS_SKIP_DOCKER is set");
        return;
    }
    assert!(
        paigasus_test_docker::start_redis_or_skip("docker_preflight").await.is_some(),
        "Docker is unreachable, so this crate's 2 Redis-backed test binaries (limits_store_redis, \
         limits_redis_e2e) will report PASS having executed nothing.\n  \
         Start the daemon, or re-run with PAIGASUS_SKIP_DOCKER=1 to accept the skips."
    );
}
```

Create `tests/limits_store_redis.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0

//! `RedisLimitStore` against a real Redis (SMA-677 spec § 5.2, § 5.3, § 5.8). The contract suite
//! runs once on `redis:6.2-alpine` (the stated minimum) and once on `redis:7.4-alpine` (the
//! version kind runs), one container per image; its cases use their own ids. The adapter cases
//! start their own 7.4 container. Raw reads exec `redis-cli` in the container, as IAM's tests do:
//! `repo:redis-connect-single-site` bans a second Rust Redis client.

mod support;

use std::sync::Arc;
use std::time::SystemTime;

use metrics_util::debugging::{DebugValue, DebuggingRecorder, Snapshotter};
use paigasus_gateway::adapters::limits::RedisLimitStore;
use paigasus_gateway::adapters::limits::redis::{budget_key, org_rate_key, principal_rate_key};
use paigasus_gateway::application::limits::Limits;
use paigasus_gateway::domain::limits::{BudgetPeriod, LimitDecision, LimitPolicy, LimitStore, LimitTicket, WindowIndex};
use paigasus_gateway::domain::{CallerContext, Credential};
use support::limits::{FixedClock, ORG_A, ORG_A_PRN, rules};
use support::limits_contract::{self as contract, Harness, at, fresh_ids};
use testcontainers_modules::redis::Redis;
use testcontainers_modules::testcontainers::ContainerAsync;
use testcontainers_modules::testcontainers::core::ExecCommand;
use uuid::Uuid;

struct RedisHarness {
    url: String,
    charger: RedisLimitStore,
}

impl RedisHarness {
    async fn new(url: String) -> Self {
        let charger = RedisLimitStore::connect(&url).await.expect("connect to the test Redis");
        RedisHarness { url, charger }
    }
}

#[async_trait::async_trait]
impl Harness for RedisHarness {
    async fn store(&self) -> Arc<dyn LimitStore> {
        // A new store per call: one more replica on the same Redis.
        Arc::new(RedisLimitStore::connect(&self.url).await.expect("connect to the test Redis"))
    }

    async fn charge_now(&self, ticket: LimitTicket, tokens: u64, now: SystemTime) {
        self.charger.apply_charge(ticket, tokens, now).await.expect("the charge is recorded");
    }
}

/// Runs `redis-cli <args>` INSIDE the container and returns its trimmed stdout (the pattern of
/// `paigasus-iam/tests/authz_generations_redis.rs:62-82`).
async fn redis_cli(node: &ContainerAsync<Redis>, args: &[&str]) -> String {
    let mut argv = vec!["redis-cli"];
    argv.extend_from_slice(args);
    let mut result = node.exec(ExecCommand::new(argv)).await.expect("exec redis-cli in the test container");
    // Drain stdout BEFORE the exit code, or the exit code may still be `None`.
    let out = result.stdout_to_vec().await.expect("redis-cli stdout");
    let code = result.exit_code().await.expect("redis-cli exit status");
    assert_eq!(code, Some(0), "redis-cli {args:?} failed (exit {code:?})");
    String::from_utf8(out).expect("redis-cli output is utf-8").trim().to_string()
}

fn counter(snapshotter: &Snapshotter, name: &str, labels: &[(&str, &str)]) -> Option<u64> {
    snapshotter.snapshot().into_vec().into_iter().find_map(|(key, _, _, value)| {
        let key = key.key();
        let have: Vec<(String, String)> = key.labels().map(|l| (l.key().to_owned(), l.value().to_owned())).collect();
        let same = key.name() == name && have.len() == labels.len() && labels.iter().all(|(k, v)| have.iter().any(|(hk, hv)| hk == k && hv == v));
        match (same, value) {
            (true, DebugValue::Counter(n)) => Some(n),
            _ => None,
        }
    })
}

fn org_a() -> Uuid {
    Uuid::try_parse(ORG_A).expect("a fixture uuid")
}

async fn redis_7_4(what: &str) -> Option<(ContainerAsync<Redis>, String)> {
    paigasus_test_docker::start_redis_image_or_skip("7.4-alpine", what).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_contract_suite_passes_on_redis_6_2() {
    let Some((_node, url)) = paigasus_test_docker::start_redis_image_or_skip("6.2-alpine", "limits_store_redis contract 6.2").await else {
        return;
    };
    contract::run_all(&RedisHarness::new(url).await).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_contract_suite_passes_on_redis_7_4() {
    let Some((_node, url)) = redis_7_4("limits_store_redis contract 7.4").await else {
        return;
    };
    contract::run_all(&RedisHarness::new(url).await).await;
}

/// D18: `EXPIRE 180` on every increment of both current rate keys.
#[tokio::test]
async fn an_admission_sets_a_180_second_ttl_on_both_current_rate_keys() {
    let Some((node, url)) = redis_7_4("limits_store_redis rate ttl").await else {
        return;
    };
    let store = RedisLimitStore::connect(&url).await.expect("connect");
    let (principal, org) = fresh_ids();
    let now = at("2026-10-02T12:00:00Z");
    let policy = LimitPolicy { principal_requests_per_minute: std::num::NonZeroU64::new(5), org_requests_per_minute: std::num::NonZeroU64::new(5), budget: None };
    assert_eq!(store.check_and_admit(&principal, Some(org), &policy, now).await.expect("admitted"), LimitDecision::Admit(None));
    let window = WindowIndex::at(now).index;
    for key in [principal_rate_key(&principal, window), org_rate_key(org, window)] {
        let ttl: i64 = redis_cli(&node, &["TTL", &key]).await.parse().expect("an integer TTL");
        assert!((175..=180).contains(&ttl), "{key}: TTL {ttl}");
    }
}

/// D18: the budget TTL is relative and comes from `now`. Both bounds: the lower catches a missing
/// `+ 86400` or a missing `EXPIRE` (TTL -1); the upper catches a TTL that does not come from `now`.
#[tokio::test]
async fn a_charge_sets_the_relative_budget_ttl() {
    let Some((node, url)) = redis_7_4("limits_store_redis budget ttl").await else {
        return;
    };
    let store = RedisLimitStore::connect(&url).await.expect("connect");
    let (_, org) = fresh_ids();
    let now = at("2026-10-02T12:00:00Z");
    let period = BudgetPeriod::Monthly.key_at(now);
    let ttl_secs = i64::from(BudgetPeriod::charge_ttl(period, now).expect("inside the period"));
    store.apply_charge(LimitTicket { org, period }, 7, now).await.expect("charged");
    let key = budget_key(org, period);
    let ttl: i64 = redis_cli(&node, &["TTL", &key]).await.parse().expect("an integer TTL");
    assert!(ttl_secs - 5 <= ttl && ttl <= ttl_secs, "TTL {ttl}, expected about {ttl_secs}");
    assert_eq!(redis_cli(&node, &["GET", &key]).await, "7");
}

/// D18: a charge 86400 s after its period end writes no key and is counted.
#[tokio::test]
async fn a_charge_a_day_late_writes_nothing_and_is_counted() {
    let Some((node, url)) = redis_7_4("limits_store_redis period expired").await else {
        return;
    };
    let recorder = DebuggingRecorder::new();
    let snapshotter = recorder.snapshotter();
    let _local = metrics::set_default_local_recorder(&recorder);
    let store = RedisLimitStore::connect(&url).await.expect("connect");
    let (_, org) = fresh_ids();
    let period = BudgetPeriod::Monthly.key_at(at("2026-10-15T00:00:00Z"));
    store.apply_charge(LimitTicket { org, period }, 7, at("2026-11-02T00:00:00Z")).await.expect("no store error");
    assert_eq!(redis_cli(&node, &["EXISTS", &budget_key(org, period)]).await, "0");
    assert_eq!(counter(&snapshotter, "gateway_limit_charges_dropped_total", &[("reason", "period_expired")]), Some(1));
}

/// D18: after one admission and one charge, exactly the expected keys exist.
#[tokio::test]
async fn the_keys_follow_the_v1_format() {
    let Some((node, url)) = redis_7_4("limits_store_redis keys").await else {
        return;
    };
    let store = RedisLimitStore::connect(&url).await.expect("connect");
    let (principal, org) = fresh_ids();
    let now = at("2026-10-02T12:00:00Z");
    let policy = rules(Some(5), Some(5), Some(100)).policy_for(Some(org));
    let LimitDecision::Admit(Some(ticket)) = store.check_and_admit(&principal, Some(org), &policy, now).await.expect("admitted") else {
        panic!("a budget admission issues a ticket");
    };
    let period = ticket.period;
    store.apply_charge(ticket, 3, now).await.expect("charged");
    let mut keys: Vec<String> = redis_cli(&node, &["KEYS", "paigasus:gateway:limits:v1:*"]).await.lines().map(str::to_owned).collect();
    keys.sort();
    let window = WindowIndex::at(now).index;
    let mut want = vec![budget_key(org, period), org_rate_key(org, window), principal_rate_key(&principal, window)];
    want.sort();
    assert_eq!(keys, want);
}

/// D19: `SCRIPT FLUSH` (or a restart, or a failover) is recovered by `invoke_async`.
#[tokio::test]
async fn a_flushed_script_is_reloaded() {
    let Some((node, url)) = redis_7_4("limits_store_redis noscript").await else {
        return;
    };
    let store = RedisLimitStore::connect(&url).await.expect("connect");
    let (principal, _) = fresh_ids();
    let now = at("2026-10-02T12:00:00Z");
    let policy = rules(Some(5), None, None).policy_for(None);
    assert_eq!(store.check_and_admit(&principal, None, &policy, now).await, Ok(LimitDecision::Admit(None)));
    assert_eq!(redis_cli(&node, &["SCRIPT", "FLUSH"]).await, "OK");
    assert_eq!(store.check_and_admit(&principal, None, &policy, now).await, Ok(LimitDecision::Admit(None)), "NOSCRIPT → SCRIPT LOAD → EVALSHA");
}

/// D10/D21: a full `noeviction` Redis refuses the script with OOM: fail-open, `kind="server"`.
#[tokio::test]
async fn a_full_noeviction_redis_fails_open_with_kind_server() {
    let Some((node, url)) = redis_7_4("limits_store_redis oom").await else {
        return;
    };
    let recorder = DebuggingRecorder::new();
    let snapshotter = recorder.snapshotter();
    let _local = metrics::set_default_local_recorder(&recorder);
    let store = RedisLimitStore::connect(&url).await.expect("connect");
    assert_eq!(redis_cli(&node, &["CONFIG", "SET", "maxmemory-policy", "noeviction"]).await, "OK");
    assert_eq!(redis_cli(&node, &["CONFIG", "SET", "maxmemory", "1"]).await, "OK");
    let limits = Arc::new(Limits::new(rules(Some(5), None, None), Arc::new(store), Arc::new(FixedClock(at("2026-10-02T12:00:00Z")))));
    let caller = CallerContext { principal_prn: fresh_ids().0, scope_prn: ORG_A_PRN.to_owned(), credential: Credential::ApiKey { key_id: "k".to_owned() } };
    let guard = limits.admit(&caller).await.expect("fail-open admits");
    assert!(!guard.has_ticket());
    assert_eq!(counter(&snapshotter, "gateway_limit_store_unavailable_total", &[("op", "check"), ("kind", "server")]), Some(1));
}

/// D16, spec § 5.8 with a runtime: a dropped guard's charge lands once the tracker drains.
#[tokio::test]
async fn a_dropped_guard_charges_through_the_tracker() {
    let Some((node, url)) = redis_7_4("limits_store_redis guard").await else {
        return;
    };
    let store = RedisLimitStore::connect(&url).await.expect("connect");
    let tracker = store.tracker();
    let now = at("2026-10-02T12:00:00Z");
    let limits = Arc::new(Limits::new(rules(None, None, Some(100)), Arc::new(store), Arc::new(FixedClock(now))));
    let caller = CallerContext { principal_prn: fresh_ids().0, scope_prn: ORG_A_PRN.to_owned(), credential: Credential::ApiKey { key_id: "k".to_owned() } };
    let mut guard = limits.admit(&caller).await.expect("admitted");
    assert!(guard.has_ticket());
    guard.set_request_estimate(7);
    guard.mark_sent();
    drop(guard);
    tracker.close();
    tracker.wait().await;
    assert_eq!(redis_cli(&node, &["GET", &budget_key(org_a(), BudgetPeriod::Monthly.key_at(now))]).await, "7");
}
```

Create `tests/limits_redis_e2e.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0

//! A11 end to end (SMA-677 spec § 5.4): two routers, each with its own `RedisLimitStore` on one
//! Redis, model two replicas. They share one org count, and a new replica (a restart) still sees it.

mod support;

use std::sync::Arc;

use axum::Router;
use axum::http::StatusCode;
use paigasus_gateway::adapters::limits::RedisLimitStore;
use support::MockOpenAi;
use support::limits::{NON_STREAM_BODY, ORG_A_PRN, PRINCIPAL_1, PRINCIPAL_2, ScopedIam, app, limits, rules, send};

/// One replica: its own `RedisLimitStore` (its own connection and breaker) on the shared Redis.
async fn replica(redis_url: &str, upstream: &str, principal: &str) -> Router {
    let store = RedisLimitStore::connect(redis_url).await.expect("connect to the test Redis");
    app(ScopedIam::new(principal, ORG_A_PRN), upstream, Some(limits(rules(None, Some(3), None), Arc::new(store))))
}

#[tokio::test]
async fn replicas_on_one_redis_share_the_org_limit() {
    let Some((_node, url)) = paigasus_test_docker::start_redis_image_or_skip("7.4-alpine", "limits_redis_e2e").await else {
        return;
    };
    let mock = MockOpenAi::spawn_json(StatusCode::OK, "{}").await;
    let a = replica(&url, &mock.base_url, PRINCIPAL_1).await;
    let b = replica(&url, &mock.base_url, PRINCIPAL_2).await;
    for app in [&a, &b, &a] {
        assert_eq!(send(app, NON_STREAM_BODY).await.status, StatusCode::OK);
    }
    let refused = send(&b, NON_STREAM_BODY).await;
    assert_eq!(refused.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(refused.json()["error"]["code"], "rate-limited");
    // A restarted replica keeps the count: it lives in Redis, not in the process.
    let restarted = replica(&url, &mock.base_url, PRINCIPAL_1).await;
    assert_eq!(send(&restarted, NON_STREAM_BODY).await.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(mock.request_count(), 3);
}
```

- [ ] **Step 3: Run them against Docker and prove they ran**

```bash
cd rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run --locked -p paigasus-gateway --test docker_preflight --test limits_store_redis --test limits_redis_e2e
```

Expected: PASS, 11 tests. `PAIGASUS_REQUIRE_DOCKER=1` turns a skip into a panic, so a PASS here proves every container started. If Docker is not running on the host, STOP and report; do not record a pass.

- [ ] **Step 4: Prove the Redis-side assertions bite (mutations 2, 11, 12, 13, 14, 20)**

One at a time; after each, run Step 3's command with `--no-fail-fast`, read the failing test names, and restore with the editor:

1. Mutation 2 (Redis side): in the script, increment the principal key before the org check, that is, move `redis.call('INCR', KEYS[1])` to just after `local p_ok, p_prev, p_cur = …`. Expected: FAIL in `the_contract_suite_passes_on_redis_6_2` and `…_7_4` (`org_refusal_does_not_grow_the_principal_count`).
2. Mutation 11: move both `INCR`/`EXPIRE` pairs above `if p_ok == 1 and …` (increment before the check). Expected: FAIL in the contract suite (D4 rows).
3. Mutation 12: delete both `redis.call('EXPIRE', …, ttl)` lines. Expected: FAIL in `an_admission_sets_a_180_second_ttl_on_both_current_rate_keys` (TTL -1).
4. Mutation 13: delete `.expire(&key, i64::from(ttl)).ignore()` in `apply_charge`. Expected: FAIL in `a_charge_sets_the_relative_budget_ttl` (TTL -1).
5. Mutation 14: in `BudgetPeriod::charge_ttl`, drop `.saturating_add(LATE_CHARGE_GRACE_SECS)`. Expected: FAIL in `a_charge_sets_the_relative_budget_ttl` (lower bound) and in `domain::limits::tests::the_charge_ttl_is_relative_to_now`.
6. Mutation 20: replace `invocation.invoke_async(&mut con)` with a bare EVALSHA that never loads the script: `redis::cmd("EVALSHA").arg(self.script.get_hash()).arg(0).query_async::<Vec<i64>>(&mut con)`. Expected: FAIL in `a_flushed_script_is_reloaded` (the first EVALSHA on a fresh Redis answers NOSCRIPT) and in the contract suite.

- [ ] **Step 5: Run the gate that owns the Docker policy**

```bash
moon run repo:iam-docker-policy-single-site repo:affected-smoke
```

Expected: PASS. The canary calls `paigasus_test_docker::skip_docker()` and never reads `CI` itself. `repo:affected-smoke` A6 asserts the gateway's `fileGroups.upstreams` equals its `dependsOn` closure; it needs `/bin/bash` 3.2 on the development Mac (Global Constraints). If A6 reds on the `paigasus-test-docker` lines, remove them from `upstreams` AND `dependsOn` together and re-run; report which form A6 accepted.

- [ ] **Step 6: Commit**

```bash
git add rs/crates/libs/paigasus-test-docker/src/lib.rs rs/crates/services/paigasus-gateway/Cargo.toml rs/Cargo.lock rs/crates/services/paigasus-gateway/moon.yml rs/.config/nextest.toml rs/crates/services/paigasus-gateway/tests/docker_preflight.rs rs/crates/services/paigasus-gateway/tests/limits_store_redis.rs rs/crates/services/paigasus-gateway/tests/limits_redis_e2e.rs
git commit -m "test(rs): run the gateway limit store against real Redis

The contract suite on redis 6.2 and 7.4 through a new pinned-image
helper in paigasus-test-docker, the TTL, key, NOSCRIPT and OOM cases, a
two-replica end-to-end test, a Docker canary, and the nextest retry and
container-cap overrides for the gateway (SMA-677).

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 13: Boot wiring, the shutdown drain and the metric tests

**Files:**
- Modify: `rs/crates/services/paigasus-gateway/src/adapters/limits/mod.rs` (`LimitsWiring`, `build_limits`, tests)
- Modify: `rs/crates/services/paigasus-gateway/src/runtime.rs` (`supervise` gains `charges`; `drain_charges`; tests)
- Modify: `rs/crates/services/paigasus-gateway/src/main.rs` (lines 68-92, 171, 174-202)
- Create: `rs/crates/services/paigasus-gateway/tests/limits_metrics.rs`, `tests/limits_metrics_prime.rs`, `tests/limits_metrics_absent.rs`

**Interfaces:**
- Consumes: Task 5's `LimitsConfig`/`LimitsBackend`; Task 7's `Limits`, `prime_metrics`, `StoreBackend`; Task 4's and Task 11's stores.
- Produces:
  - `paigasus_gateway::adapters::limits::{LimitsWiring, build_limits}`: `#[derive(Default)] pub struct LimitsWiring { pub limits: Option<Arc<Limits>>, pub charge_tasks: Option<TaskTracker> }`; `pub async fn build_limits(config: Option<&LimitsConfig>) -> anyhow::Result<LimitsWiring>`
  - `paigasus_gateway::runtime::{supervise, drain_charges, CHARGE_DRAIN_BUDGET}`: `pub async fn supervise(servers: JoinSet<anyhow::Result<()>>, shutdown: impl Future<Output = ()>, tx: watch::Sender<()>, charges: Option<TaskTracker>) -> anyhow::Result<()>`; `pub async fn drain_charges(charges: &TaskTracker, budget: Duration) -> usize`; `pub const CHARGE_DRAIN_BUDGET: Duration` (5 s)

`build_limits` lives in the library, not in `main.rs`, so the "zero after boot" test calls the same function `main.rs` calls, and deleting its `prime_metrics` call reds that test (spec § 5.11 mutation 8). The breaker series are primed only for Redis, which needs a live Redis; the unit test `prime_metrics_primes_every_series_at_zero` (Task 7) covers them with a local recorder.

- [ ] **Step 1: Write the failing tests**

Append to `src/adapters/limits/mod.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::LimitsBackend;
    use secrecy::SecretString;

    #[tokio::test]
    async fn no_limits_table_builds_nothing() {
        let wiring = build_limits(None).await.expect("no table is fine");
        assert!(wiring.limits.is_none() && wiring.charge_tasks.is_none());
    }

    #[tokio::test]
    async fn a_table_with_only_empty_policies_builds_no_store() {
        let (logs, _guard) = paigasus_logging::test_support::capture_logs_at(tracing::Level::INFO);
        let wiring = build_limits(Some(&LimitsConfig::default())).await.expect("an empty table is fine");
        assert!(wiring.limits.is_none(), "D11: no store and no Redis connection");
        assert!(logs.text().contains("no limit is configured"), "{}", logs.text());
    }

    #[tokio::test]
    async fn the_memory_backend_builds_limits_without_a_tracker() {
        let config = LimitsConfig { principal_requests_per_minute: Some(5), ..LimitsConfig::default() };
        let wiring = build_limits(Some(&config)).await.expect("memory never fails");
        assert!(wiring.limits.is_some() && wiring.charge_tasks.is_none());
    }

    /// D17/Q14: a Redis that is down at boot fails the boot, and the error never shows the URL.
    #[tokio::test]
    async fn a_redis_that_is_down_at_boot_fails_the_boot_without_the_url() {
        let config = LimitsConfig {
            backend: LimitsBackend::Redis,
            redis_url: Some(SecretString::from("redis://:hunter2@127.0.0.1:1/2".to_owned())),
            principal_requests_per_minute: Some(5),
            ..LimitsConfig::default()
        };
        let err = build_limits(Some(&config)).await.err().expect("an unreachable Redis fails the boot");
        let text = format!("{err:#}");
        assert!(text.contains("did not connect at boot"), "{text}");
        assert!(!text.contains("hunter2"), "the password never reaches the error: {text}");
    }
}
```

Append to the `tests` module of `src/runtime.rs` (and change the four existing calls `supervise(servers, …, tx)` to `supervise(servers, …, tx, None)`):

```rust
    use crate::test_support::counter;
    use metrics_util::debugging::DebuggingRecorder;
    use paigasus_observability::names;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;
    use tokio_util::task::TaskTracker;

    #[test]
    fn the_drain_budget_is_five_seconds() {
        assert_eq!(CHARGE_DRAIN_BUDGET, Duration::from_secs(5));
    }

    /// D16: a charge still running after the budget is lost and counted as `reason="shutdown"`.
    #[tokio::test]
    async fn a_charge_still_running_after_the_budget_is_counted_as_dropped() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _local = metrics::set_default_local_recorder(&recorder);
        let charges = TaskTracker::new();
        charges.spawn(std::future::pending::<()>());
        assert_eq!(drain_charges(&charges, Duration::from_millis(50)).await, 1);
        assert_eq!(counter(&snapshotter, names::GATEWAY_LIMIT_CHARGES_DROPPED_TOTAL, &[("reason", "shutdown")]), Some(1));
    }

    #[tokio::test]
    async fn supervise_drains_the_charges_after_the_servers() {
        let (tx, rx) = watch::channel(());
        let mut servers = JoinSet::new();
        spawn_until_shutdown(&mut servers, rx.clone());
        let charges = TaskTracker::new();
        let landed = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&landed);
        charges.spawn(async move {
            tokio::task::yield_now().await;
            flag.store(true, Ordering::SeqCst);
        });
        let result = supervise(servers, ready(()), tx, Some(charges)).await;
        assert!(result.is_ok());
        assert!(landed.load(Ordering::SeqCst), "supervise waits for an in-flight charge");
    }
```

Create `tests/limits_metrics_prime.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0

//! A7: every limit series reads 0 right after boot. Its own binary: the Prometheus recorder is
//! process-global, and no other test here may move a value first.

mod support;

use paigasus_gateway::adapters::limits::build_limits;
use paigasus_gateway::config::LimitsConfig;
use support::limits::sample;

#[tokio::test]
async fn every_limit_series_reads_zero_after_boot() {
    let handle = paigasus_observability::init("test-gateway-limits-prime");
    let config = LimitsConfig { principal_requests_per_minute: Some(5), ..LimitsConfig::default() };
    build_limits(Some(&config)).await.expect("the memory backend boots");
    let out = handle.render();
    let mut series: Vec<(&str, Vec<(&str, &str)>)> = Vec::new();
    for reason in ["principal_rate", "org_rate", "org_budget"] {
        series.push(("gateway_limit_refusals_total", vec![("reason", reason)]));
    }
    for source in ["reported", "estimated"] {
        series.push(("gateway_tokens_charged_total", vec![("source", source)]));
    }
    series.push(("gateway_limit_unscoped_requests_total", vec![]));
    for op in ["check", "charge"] {
        for kind in ["io", "server", "decode"] {
            series.push(("gateway_limit_store_unavailable_total", vec![("op", op), ("kind", kind)]));
        }
    }
    for reason in ["no_runtime", "shutdown", "period_expired"] {
        series.push(("gateway_limit_charges_dropped_total", vec![("reason", reason)]));
    }
    for (name, labels) in &series {
        assert_eq!(sample(&out, name, labels), Some(0.0), "{name}{labels:?} must read 0 after boot:\n{out}");
    }
    assert_eq!(sample(&out, "gateway_redis_breaker_state", &[("role", "limits")]), None, "no breaker series for the memory backend");
}
```

Create `tests/limits_metrics_absent.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0

//! A6/A7: without `[limits]`, no limit series exists at all. Its own binary, which never calls
//! `prime_metrics`.

mod support;

use axum::http::StatusCode;
use support::MockOpenAi;
use support::limits::{NON_STREAM_BODY, ORG_A_PRN, PRINCIPAL_1, ScopedIam, app, send};

#[tokio::test]
async fn no_limit_series_exists_without_a_limits_table() {
    let handle = paigasus_observability::init("test-gateway-limits-absent");
    let mock = MockOpenAi::spawn_json(StatusCode::OK, "{}").await;
    assert_eq!(send(&app(ScopedIam::new(PRINCIPAL_1, ORG_A_PRN), &mock.base_url, None), NON_STREAM_BODY).await.status, StatusCode::OK);
    let out = handle.render();
    let limit_lines: Vec<&str> = out
        .lines()
        .filter(|line| ["gateway_limit_", "gateway_tokens_charged_", "gateway_redis_breaker_"].iter().any(|prefix| line.trim_start_matches("# HELP ").trim_start_matches("# TYPE ").starts_with(prefix)))
        .collect();
    assert!(limit_lines.is_empty(), "A6: no limit series without [limits]: {limit_lines:?}");
}
```

Create `tests/limits_metrics.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0

//! A7: the limit counters move by the right NUMBER. The recorder is process-global, so each test
//! reads a delta (after − before) of one series, parsed from the Prometheus text.

mod support;

use axum::http::StatusCode;
use paigasus_gateway::adapters::limits::MemoryLimitStore;
use paigasus_gateway::domain::limits::UnavailableKind;
use std::sync::Arc;
use support::MockOpenAi;
use support::limits::{FailingStore, NON_STREAM_BODY, ORG_A_PRN, PRINCIPAL_1, ScopedIam, UNSCOPED, USAGE_BODY, app, limits, rules, sample, send};

fn read(name: &str, labels: &[(&str, &str)]) -> f64 {
    sample(&paigasus_observability::init("test-gateway-limits-metrics").render(), name, labels).unwrap_or(0.0)
}

#[tokio::test]
async fn a_budget_refusal_adds_one_org_budget_refusal() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, USAGE_BODY).await;
    let app = app(ScopedIam::new(PRINCIPAL_1, ORG_A_PRN), &mock.base_url, Some(limits(rules(None, None, Some(1)), Arc::new(MemoryLimitStore::new()))));
    assert_eq!(send(&app, NON_STREAM_BODY).await.status, StatusCode::OK);
    let before = read("gateway_limit_refusals_total", &[("reason", "org_budget")]);
    assert_eq!(send(&app, NON_STREAM_BODY).await.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(read("gateway_limit_refusals_total", &[("reason", "org_budget")]) - before, 1.0);
}

#[tokio::test]
async fn a_reported_charge_adds_its_tokens() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, USAGE_BODY).await;
    let app = app(ScopedIam::new(PRINCIPAL_1, ORG_A_PRN), &mock.base_url, Some(limits(rules(None, None, Some(1_000)), Arc::new(MemoryLimitStore::new()))));
    let before = read("gateway_tokens_charged_total", &[("source", "reported")]);
    assert_eq!(send(&app, NON_STREAM_BODY).await.status, StatusCode::OK);
    assert_eq!(read("gateway_tokens_charged_total", &[("source", "reported")]) - before, 5.0);
}

#[tokio::test]
async fn a_fail_open_admission_adds_one_check_failure_of_its_kind() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, USAGE_BODY).await;
    let app = app(ScopedIam::new(PRINCIPAL_1, ORG_A_PRN), &mock.base_url, Some(limits(rules(Some(1), None, None), FailingStore::new(UnavailableKind::Server))));
    let before = read("gateway_limit_store_unavailable_total", &[("op", "check"), ("kind", "server")]);
    assert_eq!(send(&app, NON_STREAM_BODY).await.status, StatusCode::OK);
    assert_eq!(read("gateway_limit_store_unavailable_total", &[("op", "check"), ("kind", "server")]) - before, 1.0);
}

#[tokio::test]
async fn an_unscoped_request_adds_one_unscoped_request() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, USAGE_BODY).await;
    let app = app(ScopedIam::new(PRINCIPAL_1, UNSCOPED), &mock.base_url, Some(limits(rules(None, None, Some(10)), Arc::new(MemoryLimitStore::new()))));
    let before = read("gateway_limit_unscoped_requests_total", &[]);
    assert_eq!(send(&app, NON_STREAM_BODY).await.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(read("gateway_limit_unscoped_requests_total", &[]) - before, 1.0);
}
```

- [ ] **Step 2: Run them to verify they fail**

```bash
cd rs && cargo nextest run --locked -p paigasus-gateway --no-fail-fast adapters::limits::tests runtime:: --test limits_metrics --test limits_metrics_prime --test limits_metrics_absent
```

Expected: compile FAIL (`cannot find function build_limits`; `supervise` takes 3 arguments).

- [ ] **Step 3: Write `build_limits`**

Replace the body of `src/adapters/limits/mod.rs` above the tests with:

```rust
// SPDX-License-Identifier: Apache-2.0

//! The two `LimitStore` adapters (SMA-677 spec § 4.3) and the boot wiring that picks one.

pub mod memory;
pub mod redis;

pub use memory::MemoryLimitStore;
pub use self::redis::RedisLimitStore;

use std::sync::Arc;

use secrecy::ExposeSecret;
use tokio_util::task::TaskTracker;

use crate::application::limits::{Limits, StoreBackend, prime_metrics};
use crate::config::{LimitsBackend, LimitsConfig};
use crate::domain::limits::SystemClock;

/// What `main.rs` puts into `AppState` and `runtime::supervise`.
#[derive(Default)]
pub struct LimitsWiring {
    pub limits: Option<Arc<Limits>>,
    /// The Redis store's spawned charges, drained at shutdown (D16). `None` for memory.
    pub charge_tasks: Option<TaskTracker>,
}

/// Build the limits from `[limits]` (spec § 4.4). Call it after `paigasus_observability::init`
/// (D17 "Order"): `prime_metrics` and the breaker's constructor need the installed recorder.
///
/// - No table: nothing at all (A6).
/// - A table that can only give empty policies: no store, no Redis connection, one `info` line (D11).
/// - `memory`: a `MemoryLimitStore`.
/// - `redis`: `RedisLimitStore::connect`, eager — a Redis that is down fails the boot (D17, Q14).
pub async fn build_limits(config: Option<&LimitsConfig>) -> anyhow::Result<LimitsWiring> {
    let Some(config) = config else {
        return Ok(LimitsWiring::default());
    };
    let backend = match config.backend {
        LimitsBackend::Memory => StoreBackend::Memory,
        LimitsBackend::Redis => StoreBackend::Redis,
    };
    prime_metrics(backend);
    let rules = config.rules();
    if rules.every_policy_is_empty() {
        tracing::info!(backend = ?config.backend, "[limits] is set, but no limit is configured: no store is built and no limit applies");
        return Ok(LimitsWiring::default());
    }
    let clock = Arc::new(SystemClock);
    match config.backend {
        LimitsBackend::Memory => Ok(LimitsWiring { limits: Some(Arc::new(Limits::new(rules, Arc::new(MemoryLimitStore::new()), clock))), charge_tasks: None }),
        LimitsBackend::Redis => {
            let url = config.redis_url.as_ref().ok_or_else(|| anyhow::anyhow!("limits.backend = \"redis\" requires limits.redis_url"))?;
            // The error text is redis-rs's (for example "Connection refused"); the URL, which can
            // carry a password, is never formatted into it.
            let store = RedisLimitStore::connect(url.expose_secret()).await.map_err(|err| anyhow::anyhow!("the limits Redis store did not connect at boot: {err}"))?;
            let charge_tasks = store.tracker();
            Ok(LimitsWiring { limits: Some(Arc::new(Limits::new(rules, Arc::new(store), clock))), charge_tasks: Some(charge_tasks) })
        }
    }
}
```

- [ ] **Step 4: Add the shutdown drain**

In `src/runtime.rs`, add imports and constants:

```rust
use std::time::Duration;

use metrics::counter;
use paigasus_observability::names;
use tokio_util::task::TaskTracker;

use crate::domain::limits::ChargeDropReason;

/// D16: how long shutdown waits for in-flight limit charges.
pub const CHARGE_DRAIN_BUDGET: Duration = Duration::from_secs(5);
```

Change the signature of `supervise` to take `charges: Option<TaskTracker>` as the last parameter, document it in the doc comment ("`charges`: the Redis store's spawned charges (SMA-677 D16). After the servers drain, they get at most `CHARGE_DRAIN_BUDGET`; the rest are counted as dropped."), and insert before the final `result`:

```rust
    if let Some(charges) = charges {
        drain_charges(&charges, CHARGE_DRAIN_BUDGET).await;
    }
```

Add:

```rust
/// D16: close the tracker and wait at most `budget`. Returns the number of charges still running
/// then; they are lost, and counted as `gateway_limit_charges_dropped_total{reason="shutdown"}`.
pub async fn drain_charges(charges: &TaskTracker, budget: Duration) -> usize {
    charges.close();
    if tokio::time::timeout(budget, charges.wait()).await.is_ok() {
        return 0;
    }
    let lost = charges.len();
    counter!(names::GATEWAY_LIMIT_CHARGES_DROPPED_TOTAL, "reason" => ChargeDropReason::Shutdown.as_label()).increment(u64::try_from(lost).unwrap_or(u64::MAX));
    tracing::warn!(lost, "shutdown: limit charges still running after the drain budget were dropped");
    lost
}
```

- [ ] **Step 5: Wire `main.rs`**

In `src/main.rs`:

- Imports: `use paigasus_gateway::adapters::limits::build_limits;`
- After the `if metrics_handle.is_some() { describe_gateway_metrics(); }` block:

```rust
    // SMA-677 D17 "Order": after `paigasus_observability::init`, so `prime_metrics` and the Redis
    // breaker's constructor reach the installed recorder. Eager: with `backend = "redis"`, a Redis
    // that is down fails the boot here (Q14).
    let limits = build_limits(config.limits.as_ref()).await?;
```

- In the `AppState` literal: `limits: limits.limits,` (replacing `limits: None`).
- The last line of `serve`: `runtime::supervise(servers, shutdown_signal(), tx, limits.charge_tasks).await`
- `describe_gateway_metrics`: the doc comment says "the 14 metric families"; append:

```rust
    describe_counter!(names::GATEWAY_LIMIT_REFUSALS_TOTAL, "Chat requests refused by a limit before egress, labeled by reason (principal_rate, org_rate, org_budget).");
    describe_counter!(names::GATEWAY_TOKENS_CHARGED_TOTAL, "Tokens the charge guard sent to the limit store, labeled by source (reported, estimated).");
    describe_counter!(names::GATEWAY_LIMIT_UNSCOPED_REQUESTS_TOTAL, "Chat requests whose scope names no organization. Expected 0.");
    describe_counter!(
        names::GATEWAY_LIMIT_STORE_UNAVAILABLE_TOTAL,
        "Limit-store calls that failed or met an open breaker, labeled by op (check, charge) and kind (io, server, decode). A check here is a request admitted with no limit (fail-open)."
    );
    describe_counter!(names::GATEWAY_LIMIT_CHARGES_DROPPED_TOTAL, "Limit charges that were not sent, labeled by reason (no_runtime, shutdown, period_expired).");
    describe_gauge!(names::GATEWAY_REDIS_BREAKER_STATE, "The limits Redis circuit breaker: 0 closed, 1 half-open, 2 open. Aggregate max by (job, role), never sum.");
    describe_counter!(names::GATEWAY_REDIS_BREAKER_TRANSITIONS_TOTAL, "Transitions of the limits Redis circuit breaker, labeled by role and to.");
```

- [ ] **Step 6: Run the tests to verify they pass**

```bash
cd rs && cargo fmt && cargo nextest run --locked -p paigasus-gateway && cargo clippy --locked -p paigasus-gateway --all-targets -- -D warnings && cargo build --locked -p paigasus-gateway --bin paigasus-gateway
```

Expected: PASS. Then run the binary once without `[limits]` and once with a memory table, and read the boot log:

```bash
cd rs && GATEWAY_UPSTREAM__OPENAI__API_KEY=sk-dummy GATEWAY_HTTP_ADDR=127.0.0.1:18088 GATEWAY_IAM__GRPC_ADDR=http://127.0.0.1:9090 GATEWAY_IAM__TLS__MODE=loopback_insecure timeout 3 ./target/debug/paigasus-gateway; echo "exit $?"
cd rs && GATEWAY_LIMITS__PRINCIPAL_REQUESTS_PER_MINUTE=60 GATEWAY_UPSTREAM__OPENAI__API_KEY=sk-dummy GATEWAY_HTTP_ADDR=127.0.0.1:18088 GATEWAY_IAM__GRPC_ADDR=http://127.0.0.1:9090 GATEWAY_IAM__TLS__MODE=loopback_insecure timeout 3 ./target/debug/paigasus-gateway; echo "exit $?"
```

macOS has no `timeout` binary: if the command is missing, start the binary in the background, wait 3 s with `sleep 3` in a script file, and send `kill -TERM`. Expected: both runs log `paigasus-gateway started`; neither logs an error. Report the two log tails.

- [ ] **Step 7: Prove the priming bites (mutation 8)**

Delete `prime_metrics(backend);` in `build_limits`. Run `cargo nextest run --locked -p paigasus-gateway --test limits_metrics_prime --no-fail-fast`. Expected: FAIL in `every_limit_series_reads_zero_after_boot`. Restore.

- [ ] **Step 8: Commit**

```bash
git add rs/crates/services/paigasus-gateway/src rs/crates/services/paigasus-gateway/tests
git commit -m "feat(rs): boot the gateway limits and drain charges at shutdown

build_limits primes the limit series and builds the memory or the eager
Redis store after the metrics recorder exists; supervise drains the
spawned charges for at most 5 s and counts the rest (SMA-677).

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 14: The two fail-open alerts

**Files:**
- Modify: `ops/observability/prometheus/rules/gateway.rules.yml` (append two rules)
- Modify: `ops/observability/prometheus/rules/tests/gateway.test.yml` (append four test groups)

**Interfaces:**
- Consumes: Task 4's metric names (`gateway_redis_breaker_state`, `gateway_limit_store_unavailable_total`), Task 7's priming at zero.
- Produces: alerts `GatewayLimitsRedisBreakerOpen` and `GatewayLimitStoreUnavailable`; Task 15's RUNBOOK section is the one their descriptions name.

- [ ] **Step 1: Write the failing promtool tests**

Append to `ops/observability/prometheus/rules/tests/gateway.test.yml`:

```yaml
  # GatewayLimitsRedisBreakerOpen: max by (job, role) (gateway_redis_breaker_state) != 0, for: 2m.
  # Firing: the breaker reads 2 (open) for 5 minutes.
  - interval: 1m
    input_series:
      - series: 'gateway_redis_breaker_state{job="gateway",instance="gw-1",role="limits"}'
        values: '2x5'
    alert_rule_test:
      - eval_time: 1m
        alertname: GatewayLimitsRedisBreakerOpen
        exp_alerts: []
      - eval_time: 3m
        alertname: GatewayLimitsRedisBreakerOpen
        exp_alerts:
          - exp_labels: { severity: warning, job: gateway, role: limits }
            exp_annotations:
              summary: "Gateway limits Redis circuit breaker is not closed (role limits)"
              description: "The gateway's limits Redis circuit breaker has been open or half-open for 2m. The rate limits and token budgets are NOT applied (fail-open), and the tokens used now are not charged to any budget. See RUNBOOK-observability.md, section \"Gateway limits: fail-open and Redis requirements\"."

  # Quiet: a closed breaker (primed at 0) never fires.
  - interval: 1m
    input_series:
      - series: 'gateway_redis_breaker_state{job="gateway",instance="gw-1",role="limits"}'
        values: '0x5'
    alert_rule_test:
      - eval_time: 3m
        alertname: GatewayLimitsRedisBreakerOpen
        exp_alerts: []

  # GatewayLimitStoreUnavailable: sum by (job, op, kind) (increase(…[10m])) > 0, for: 0m.
  # Firing: one failed check after a primed 0 — increase() sees it only because of the 0 sample.
  - interval: 1m
    input_series:
      - series: 'gateway_limit_store_unavailable_total{job="gateway",instance="gw-1",op="check",kind="io"}'
        values: '0 0 1 1 1 1'
    alert_rule_test:
      - eval_time: 3m
        alertname: GatewayLimitStoreUnavailable
        exp_alerts:
          - exp_labels: { severity: warning, job: gateway, op: check, kind: io }
            exp_annotations:
              summary: "Gateway limit store calls are failing (op check, kind io)"
              description: "Limit-store calls failed in the last 10m. A failed check admits the request with no limit, and a failed charge is not counted against any budget (fail-open). kind=io: Redis is down or unreachable. kind=server or decode: Redis answered with an error (for example OOM under noeviction, or READONLY from a replica), which never opens the breaker. See RUNBOOK-observability.md, section \"Gateway limits: fail-open and Redis requirements\"."

  # Quiet: a primed series that never moves.
  - interval: 1m
    input_series:
      - series: 'gateway_limit_store_unavailable_total{job="gateway",instance="gw-1",op="check",kind="io"}'
        values: '0x5'
    alert_rule_test:
      - eval_time: 3m
        alertname: GatewayLimitStoreUnavailable
        exp_alerts: []
```

- [ ] **Step 2: Run them to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
promtool test rules ops/observability/prometheus/rules/tests/gateway.test.yml
```

Expected: FAIL (`no alert named GatewayLimitsRedisBreakerOpen` / an unexpected empty alert list). If `promtool` is not on the PATH, run `moon run repo:promtool` instead and read `.moon/cache/states/repo/promtool/stdout.log`.

- [ ] **Step 3: Add the two rules**

Append to the `rules:` list of `ops/observability/prometheus/rules/gateway.rules.yml`:

```yaml
      - alert: GatewayLimitsRedisBreakerOpen
        # SMA-677 § 4.8, in the form of IamRedisBreakerOpen. While the breaker is open or half-open
        # the limits are NOT applied: every request is admitted and its tokens are not charged
        # (fail-open, D10). `max by (job, role)`: every replica sets its own gauge, so sum() is wrong.
        expr: max by (job, role) (gateway_redis_breaker_state) != 0
        for: 2m
        labels: { severity: warning }
        annotations: { summary: "Gateway limits Redis circuit breaker is not closed (role {{ $labels.role }})", description: "The gateway's limits Redis circuit breaker has been open or half-open for 2m. The rate limits and token budgets are NOT applied (fail-open), and the tokens used now are not charged to any budget. See RUNBOOK-observability.md, section \"Gateway limits: fail-open and Redis requirements\"." }
      - alert: GatewayLimitStoreUnavailable
        # SMA-677 § 4.8. Also fires on an outage shorter than a scrape interval, which the gauge
        # misses, and on `server` and `decode` errors, which never open the breaker
        # (`counts_as_failure`). The series is primed at 0, so increase() sees the first event.
        expr: sum by (job, op, kind) (increase(gateway_limit_store_unavailable_total[10m])) > 0
        for: 0m
        labels: { severity: warning }
        annotations: { summary: "Gateway limit store calls are failing (op {{ $labels.op }}, kind {{ $labels.kind }})", description: "Limit-store calls failed in the last 10m. A failed check admits the request with no limit, and a failed charge is not counted against any budget (fail-open). kind=io: Redis is down or unreachable. kind=server or decode: Redis answered with an error (for example OOM under noeviction, or READONLY from a replica), which never opens the breaker. See RUNBOOK-observability.md, section \"Gateway limits: fail-open and Redis requirements\"." }
```

- [ ] **Step 4: Run the alert gates**

```bash
promtool test rules ops/observability/prometheus/rules/tests/gateway.test.yml
moon run repo:promtool repo:observability-drift
```

Expected: PASS. `repo:observability-drift` passes because both names are in `names::ALL` (Task 4).

- [ ] **Step 5: Prove each rule's firing case bites (mutation 22)**

Delete the `GatewayLimitsRedisBreakerOpen` rule; run Step 2's command; expected: FAIL on its firing case. Restore. Repeat for `GatewayLimitStoreUnavailable`. Then change its `[10m]` data to start at 1 instead of 0 (`values: '1 1 1 1 1 1'`) and confirm the firing case FAILS — this is the first-sample trap the priming exists for. Restore.

- [ ] **Step 6: Commit**

```bash
git add ops/observability/prometheus/rules/gateway.rules.yml ops/observability/prometheus/rules/tests/gateway.test.yml
git commit -m "feat(repo): alert on a gateway limits fail-open outage

GatewayLimitsRedisBreakerOpen watches the limits breaker gauge, and
GatewayLimitStoreUnavailable catches short outages and server or decode
errors that never open the breaker, with promtool firing and quiet
cases (SMA-677).

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 15: Documentation — example config, RUNBOOK, chart comment, crate doc

**Files:**
- Modify: `rs/crates/services/paigasus-gateway/gateway.toml.example` (lines 4-6; new `[limits]` section after the `[metrics]` block)
- Modify: `docs/ops/RUNBOOK-observability.md` (§ 2.3 table after line 138; § 4 alert table after line 240; two alert sections and one posture section after `GatewayUpstreamErrors` (line ~1720); the "Gateway deployment posture" paragraph at ~1740)
- Modify: `charts/paigasus/values.yaml:104-106` (the `replicas` comment)
- Modify: `rs/crates/services/paigasus-gateway/src/lib.rs` (crate doc)

**Interfaces:**
- Consumes: every name above (config keys, metric names, alert names, log line `chat completion metered`).
- Produces: the RUNBOOK section title "Gateway limits: fail-open and Redis requirements" that the alert descriptions (Task 14) name.

- [ ] **Step 1: Rewrite the M0 note and add the `[limits]` section to `gateway.toml.example`**

Replace lines 4-6:

```toml
# LIMITS NOTE (SMA-677): with no [limits] table (the default) there is no rate limit and no spend
# limit on the OpenAI upstream. The [limits] table below adds a per-principal and a per-org request
# rate and a per-org token budget. Keep a hard OpenAI spend cap even then: the budget counts tokens,
# not money; requests in flight when it runs out still complete and are charged; and with
# backend = "redis" no limit applies while Redis is unavailable (fail-open).
```

Append after the `[metrics]` block:

```toml
# --- Limits (SMA-677): request rate and token budget on POST /v1/chat/completions ---
#
# Off unless this table exists. An unset key means no limit on that dimension; 0 is rejected.
# A misspelt key fails the boot. Refusals: 429 "rate-limited" (Retry-After, retryable) and
# 429 "budget-exhausted" (type "insufficient_quota", x-should-retry: false).
#
# [limits]
# backend = "memory"                     # "memory" (default) | "redis"
#   # memory: one process only. With N replicas the limits are about N times the values below,
#   # and a restart sets every budget to zero. Use "redis" for more than one replica.
# # redis_url: set GATEWAY_LIMITS__REDIS_URL, never this file (it can carry a password).
# # Required for backend = "redis"; rejected for "memory". Use rediss:// outside a private
# # network. Requirements: Redis 6.2 or later; a URL that always reaches the primary (no replica,
# # no Redis Cluster); a dedicated instance, or at least a dedicated logical database, for limits;
# # maxmemory-policy noeviction (or enough headroom that eviction never runs: an evicted budget
# # key silently resets that org's budget); appendonly yes if budgets must survive a Redis
# # restart. Redis must be reachable at boot: a gateway with backend = "redis" does not start
# # while Redis is down. After boot, a Redis outage is fail-open: no limit applies and no
# # tokens are charged until it recovers (see RUNBOOK-observability.md, "Gateway limits:
# # fail-open and Redis requirements").
# principal_requests_per_minute = 60     # unset: no principal rate limit; max 1000000000
# org_requests_per_minute       = 600    # unset: no org rate limit (summed over the org's principals); max 1000000000
# tokens_per_period             = 5000000 # unset: no token budget; max 1000000000000
# budget_period                 = "monthly" # "daily" | "weekly" (ISO week) | "monthly" (default), UTC
#
# Per-org overrides, by org UUID. TOML only: environment variables cannot set an array of
# tables. With backend = "redis", give EVERY replica the same list, or two replicas apply
# different limits to one shared count. A field you leave out keeps the table default.
# [[limits.org]]
# id                      = "0190a100-0000-7000-8000-0000000000a1"
# org_requests_per_minute = 1200
# tokens_per_period       = 20000000
#
# [[limits.org]]
# id     = "0190a100-0000-7000-8000-0000000000b2"
# exempt = true                          # no org rate and no budget; the principal rate still applies
#
# Environment: GATEWAY_LIMITS__BACKEND, GATEWAY_LIMITS__REDIS_URL,
# GATEWAY_LIMITS__PRINCIPAL_REQUESTS_PER_MINUTE, GATEWAY_LIMITS__ORG_REQUESTS_PER_MINUTE,
# GATEWAY_LIMITS__TOKENS_PER_PERIOD, GATEWAY_LIMITS__BUDGET_PERIOD.
#
# Charging: a stream is charged its reported usage only when the client sends
# stream_options: {"include_usage": true} (the console playground does). Otherwise the gateway
# charges an estimate: about one token per streamed record plus the prompt text / 4.
```

- [ ] **Step 2: Update the RUNBOOK**

In § 2.3, after the `gateway_upstream_request_duration_seconds` row, add:

```markdown
| `gateway_limit_refusals_total` | counter | `reason` | SMA-677. Chat requests refused by a limit before egress, one per request. `reason` ∈ `principal_rate` / `org_rate` / `org_budget` (the D8 precedence picks one). Primed at 0 when `[limits]` is set; absent without it. |
| `gateway_tokens_charged_total` | counter | `source` | Tokens the charge guard sent to the limit store. `source` ∈ `reported` (the answer's `usage.total_tokens`) / `estimated` (no usage record). Requests admitted under fail-open are NOT counted here; see the `chat completion metered` log line. |
| `gateway_limit_unscoped_requests_total` | counter | — | Chat requests whose scope PRN names no organization. Expected 0; non-zero is a scope parse mismatch or an IAM defect. With an org rate or a budget configured, these requests get `500 internal`. |
| `gateway_limit_store_unavailable_total` | counter | `op`, `kind` | Limit-store calls that failed or met an open breaker. `op` ∈ `check` (the request was admitted with NO limit) / `charge` (the tokens were NOT counted). `kind` ∈ `io` (Redis down or unreachable) / `server` (Redis answered with an error: OOM under `noeviction`, `READONLY`, a script error) / `decode` (a reply of the wrong shape). |
| `gateway_limit_charges_dropped_total` | counter | `reason` | Charges that were not sent. `reason` ∈ `no_runtime` (a guard dropped outside the async runtime) / `shutdown` (still running after the 5 s shutdown drain) / `period_expired` (more than one day after its budget period ended). |
| `gateway_redis_breaker_state` | gauge | `role` | The limits Redis circuit breaker (`role="limits"`): 0 closed, 1 half-open, 2 open. Only with `backend = "redis"`. Per replica: aggregate `max by (job, role)`, never `sum`. |
| `gateway_redis_breaker_transitions_total` | counter | `role`, `to` | One per transition of that breaker; `to` ∈ `closed` / `half_open` / `open`. Catches a flapping breaker that the gauge misses between scrapes. |

**The `chat completion metered` log line** (`info` when tokens > 0, `debug` otherwise) is the
per-org spend record: fields `org`, `tokens`, `source`, `outcome` (`charged` / `store_unavailable`
/ `no_budget` / `zero`), `request_id`, `correlation_id`. No metric label carries an org id
(unbounded); use this line for per-org use, and its `outcome="store_unavailable"` lines for the
spend that fail-open did not charge.
```

In § 4's alert table, after the `GatewayUpstreamErrors` row:

```markdown
| `GatewayLimitsRedisBreakerOpen` | `max by (job, role) (gateway_redis_breaker_state) != 0` for 2m | warning |
| `GatewayLimitStoreUnavailable` | `sum by (job, op, kind) (increase(gateway_limit_store_unavailable_total[10m])) > 0` | warning |
```

After the `GatewayUpstreamErrors` section (before `### TargetDown`), add:

```markdown
### `GatewayLimitsRedisBreakerOpen` — the limits Redis breaker is not closed (warning)

**Meaning.** The gateway's limits Redis circuit breaker (`gateway_redis_breaker_state{role="limits"}`)
has been open or half-open for 2m. While it is, every chat request is admitted with no rate
limit and no budget, and its tokens are not charged (fail-open, SMA-677 D10). The exposure is
"the upstream's own rate limit × the outage duration".

**Confirm:** is the limits Redis up and reachable from the gateway? Does `limits.redis_url`
reach the primary? Read `gateway_limit_store_unavailable_total` by `kind`.

**Remediation:** see "Gateway limits: fail-open and Redis requirements" below.

### `GatewayLimitStoreUnavailable` — limit store calls are failing (warning)

**Meaning.** At least one limit-store call failed in the last 10m. It also fires on an outage
shorter than a scrape interval, and on `kind="server"` and `kind="decode"`, which never open the
breaker. `op="check"`: requests were admitted with no limit. `op="charge"`: tokens were not counted.

**Confirm:** the `kind` label. `io` — Redis is down or unreachable (also logged at `warn`).
`server` — Redis answered with an error, for example `OOM` under `noeviction` (the instance is
full) or `READONLY` (the URL reaches a replica); logged at `error`. `decode` — a reply of the
wrong shape; logged at `error`, and a defect. The log lines are rate-limited to one per
(operation, kind) per 10 s and carry the count of suppressed events.

**Remediation:** see the next section.

### Gateway limits: fail-open and Redis requirements

**Fail-open (SMA-677 D10).** With `[limits] backend = "redis"`, a Redis that does not answer,
answers with an error, or sits behind an open breaker does not stop chat traffic: the request is
admitted, and no tokens are charged. Readiness (`/readyz`) does not check Redis, on purpose: a
Redis check there would take every replica out of the balancer during a Redis outage. To find
the spend that fail-open did not charge, search the `chat completion metered` lines with
`outcome="store_unavailable"`; their `org` and `tokens` fields hold it.

**Boot needs Redis (D17).** A gateway with `backend = "redis"` connects at boot and does not
start while Redis is down, so a wrong URL or password shows at deploy time. During a Redis
outage a scale-up or a rollout stalls.

**Redis requirements (D21).** A lost or evicted budget key reads as zero, which silently resets
that org's budget. So:
1. Use a dedicated Redis instance for limits (strongest), or at least a dedicated logical
   database (`redis://host:6379/2`; this separates the keys but not the memory pool).
2. Set `maxmemory-policy noeviction`, or keep enough headroom that eviction never runs. Under
   `noeviction` a full instance refuses writes with `OOM`, which shows as `kind="server"`. Under an
   `allkeys-*` or `volatile-*` policy, Redis evicts budget keys silently (every limit key has a TTL).
3. Turn on AOF (`appendonly yes`) if budgets must survive a Redis restart. With no persistence a
   restart resets every budget and rate count; with RDB only, it loses the charges since the last
   snapshot.
4. Replication is asynchronous: a failover can lose the last charges (an under-count).
5. The URL must always reach the primary (a single node, or a primary behind a service address
   that moves on failover). Redis Cluster is not supported. Minimum Redis 6.2.

**Other known limits.** Concurrent requests can overshoot a budget by the use of the requests in
flight (D7). Replicas with different `[[limits.org]]` lists apply different limits to one shared
count. Clock skew between replicas moves a rate estimate by about `skew / 60 s` of one window.
A client that does not send `stream_options.include_usage` is charged an estimate.

**Hot-path latency during a blackhole (D20).** A few requests per 2 s window wait about 2.1 s
(the dial budget); after three failures the breaker opens and the rest pay nothing.
```

Replace the first paragraph of "Gateway deployment posture" ("**M0 is internal-only or spend-capped.** … enforces.") with:

```markdown
**Keep a spend cap, even with `[limits]`.** Without a `[limits]` table the gateway has no rate
limit and no budget, so a leaked API key means effectively unbounded OpenAI spend. With
`[limits]` (SMA-677) the gateway enforces a request rate and a token budget, but the budget counts
tokens, not money, requests in flight still complete when it runs out, and a Redis outage is
fail-open. So run the gateway in **one** of two postures:
1. **Internal/non-production** — not reachable from untrusted networks, or
2. **Behind a hard OpenAI account-level spend cap**, so a worst case is bounded by that cap.
```

- [ ] **Step 3: Update the chart comment and the crate doc**

`charts/paigasus/values.yaml` lines 104-106:

```yaml
      # Genuinely missing from the original schema (SMA-513 Task 11) — the gateway backend is
      # horizontally scalable, unlike IAM's SMA-559-pinned single replica, so it gets a real knob
      # rather than a hardcoded default. Mirrors console.replicas above. With gateway [limits]
      # (SMA-677), more than one replica needs limits.backend = "redis": the memory backend keeps
      # per-replica counts, so N replicas allow about N times the configured limits.
```

`rs/crates/services/paigasus-gateway/src/lib.rs`, append to the crate doc (before `pub mod adapters;`):

```rust
//!
//! ## Limits (SMA-677)
//! An optional `[limits]` table adds a per-principal and a per-org request rate (a 60 s sliding
//! window) and a per-org token budget (UTC daily, ISO-weekly or monthly). The refusals are `429`
//! with the registry codes `rate-limited` (retryable, `Retry-After`) and `budget-exhausted`
//! (`x-should-retry: false`). `backend = "memory"` keeps per-process counts; `backend = "redis"`
//! shares them across replicas and is fail-open when Redis is unavailable. A stream is charged its
//! reported usage only when the client sends `stream_options: {"include_usage": true}`.
```

- [ ] **Step 4: Run the doc gates**

```bash
moon run repo:helm-render repo:error-code-single-site repo:observability-drift
cd rs && cargo fmt --check && cargo build --locked -p paigasus-gateway
```

Expected: PASS. If `repo:helm-render` reds because a golden under `charts/paigasus/tests/golden/` holds the `replicas` comment, regenerate the goldens with the command its README names and commit them in this task (spec § 8 "The chart"). `repo:error-code-single-site` must stay green with no `ci/error-registry/check.py` change: no new `src/` file spells `rate-limited` or `budget-exhausted` (Global Constraints). If it reds, add an `asserts` row for the named file in the form of `check.py:110-111` and say why in the commit.

- [ ] **Step 5: Commit**

```bash
git add rs/crates/services/paigasus-gateway/gateway.toml.example docs/ops/RUNBOOK-observability.md charts/paigasus/values.yaml rs/crates/services/paigasus-gateway/src/lib.rs
git commit -m "docs(docs): document the gateway limits and their Redis requirements

The [limits] section of gateway.toml.example, the seven metric families,
the metered log line, two alert entries, the fail-open and Redis
requirements section, and the chart replicas note (SMA-677).

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 16: The console playground asks for usage

**Files:**
- Modify: `ts/apps/gateway-console/lib/chat-route.ts:205-206`
- Modify: `ts/apps/gateway-console/tests/unit/chat-route.test.ts:226`, `:232`
- Modify: `ts/apps/gateway-console/tests/unit/chat-stream.test.ts` (two cases after line 47)

**Interfaces:**
- Consumes: nothing from earlier tasks (the gateway forwards the field unchanged, A5).
- Produces: every playground stream request carries `stream_options: { include_usage: true }` (A10), so its stream is charged from the reported usage.

Q19 (approved): no switch for an upstream that rejects unknown fields; vLLM and LiteLLM accept it.

- [ ] **Step 1: Write the failing tests**

`ts/apps/gateway-console/tests/unit/chat-route.test.ts` line 226 and 232:

```ts
  it('sends only model, messages, stream: true and include_usage, with the org, the correlation id, the signal, the bearer and 35 s', async () => {
```

```ts
    expect(body).toEqual({ model: 'gpt-e2e', messages: [{ role: 'user', content: 'hi' }], stream: true, stream_options: { include_usage: true } });
```

`ts/apps/gateway-console/tests/unit/chat-stream.test.ts`, after the test that ends at line 47:

```ts
  // SMA-677 A10. With include_usage, OpenAI sends `"usage": null` in every chunk and ends the stream
  // with one usage record whose `choices` is empty. Neither may change what the user sees.
  it('reads a chunk that carries usage: null', () => {
    const chunk = `data: ${JSON.stringify({ choices: [{ index: 0, delta: { content: 'b' } }], usage: null })}\n\n`;
    expect(all([chunk, 'data: [DONE]\n\n'])).toEqual([{ kind: 'delta', text: 'b' }, { kind: 'done' }]);
  });

  it('emits nothing for the final usage record', () => {
    const usage = `data: ${JSON.stringify({ id: 'chatcmpl-1', object: 'chat.completion.chunk', choices: [], usage: { prompt_tokens: 2, completion_tokens: 3, total_tokens: 5 } })}\n\n`;
    expect(all([delta('a'), usage, 'data: [DONE]\n\n'])).toEqual([{ kind: 'delta', text: 'a' }, { kind: 'done' }]);
  });
```

- [ ] **Step 2: Run them to verify the route test fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts/apps/gateway-console && pnpm exec vitest run tests/unit/chat-route.test.ts tests/unit/chat-stream.test.ts
```

Expected: the `chat-route` test FAILS (the body has no `stream_options`); the two `chat-stream` cases PASS already (spec § 4.9: the parser already ignores an empty `choices`), which pins that behavior.

- [ ] **Step 3: Send `include_usage`**

`ts/apps/gateway-console/lib/chat-route.ts` lines 205-206:

```ts
    // The gateway request is built HERE: no other client field is forwarded. `include_usage` makes
    // OpenAI end the stream with a usage record, so the gateway charges the reported tokens and not
    // an estimate (SMA-677 A10).
    const result = await client.completions(
      { model, messages, stream: true, stream_options: { include_usage: true } },
      correlationId === null ? { signal: request.signal, org } : { signal: request.signal, org, correlationId },
    );
```

- [ ] **Step 4: Run the TS checks**

```bash
cd ts/apps/gateway-console && pnpm exec vitest run tests/unit/chat-route.test.ts tests/unit/chat-stream.test.ts && cd ../../..
moon run ts:fmt ts:lint gateway-console-ts:typecheck gateway-console-ts:test
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add ts/apps/gateway-console/lib/chat-route.ts ts/apps/gateway-console/tests/unit/chat-route.test.ts ts/apps/gateway-console/tests/unit/chat-stream.test.ts
git commit -m "feat(ts): ask for stream usage from the gateway console playground

The playground sends stream_options.include_usage, so the gateway charges
a playground stream from its reported usage (SMA-677).

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 17: The full gate graph and the mutation battery

**Files:** none changed, unless a gate or a mutation finds a defect (then fix it in the owning file, in a new commit, and re-run this whole task).

**Interfaces:**
- Consumes: everything above.
- Produces: the results that the PR body records (spec § 5.9, § 5.11, D19).

- [ ] **Step 1: Run the full gate graph**

Run the command between the `ci-targets` markers in the root `CLAUDE.md`, with the base of this stack:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site \
  :http-extractor-envelope :input-liveness :promtool :observability-drift \
  :nats-permissions :release-parity :release-parity-py :release-parity-ts \
  :publish-metadata :version-lockstep :workflow-credentials :pyo3-stub-drift :ruff-ci \
  :next-public-free :helm-render :moon-diagnosis-exec :test-e2e \
  --base BASE --include-relations
```

Replace `BASE` with the ref from Task 0 (`origin/main` once PR 1 is merged). On a failure, follow the root `CLAUDE.md` "Diagnosing an unattributed `moon ci` failure" procedure (Step 0 first: copy `.moon/cache/ciReport.json` and the task's `states/` directory before any re-run). Then re-run the gates that need another bash than the one `moon ci` used, directly: `/bin/bash ci/affected-graph/run.sh` (3.2); `/opt/homebrew/bin/bash ci/ruff/run.sh`, `ci/next-public/run.sh`, `ci/publish-metadata/run.sh`, `ci/version-lockstep/run.sh`, `ci/nats-permissions/run.sh` (4+); `/opt/homebrew/bin/bash ci/actionlint/run.sh` (5; read its pipe-capacity preflight line first — below 8192 bytes it exits rc 2 and gives no verdict on this host). Record each gate's result and the bash that produced it.

- [ ] **Step 2: Prove the Docker tests ran**

```bash
cd rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run --locked -p paigasus-gateway --test docker_preflight --test limits_store_redis --test limits_redis_e2e
cd rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run --locked -p paigasus-iam --profile iam
```

Expected: PASS. The second command is A13: every IAM test still passes after the `script` feature joined `paigasus-redis` (Task 11).

- [ ] **Step 3: Run the single-site negative controls (spec § 5.9)**

One at a time; after each, run the gate, record the exit code and the named file, and restore with the editor:

1. Add `let _ = redis::Client::open("redis://x").map(|c| c.get_multiplexed_async_connection());` inside a function body in `rs/crates/services/paigasus-gateway/src/adapters/limits/redis.rs`. `moon run repo:redis-connect-single-site --force` → exit 1, names that file.
2. Add `let _ = std::env::var_os("CI");` to a test in `rs/crates/services/paigasus-gateway/tests/limits_store_redis.rs`. `moon run repo:iam-docker-policy-single-site --force` → red, names that file.

(The controls that move PR 1's files — the gate's own exit-2 control — belong to PR 1's plan; run them only if the coordinator asks.)

- [ ] **Step 4: Run the mutation battery (spec § 5.11), whole**

Each mutation must COMPILE. Apply one, run `cd rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run --locked -p paigasus-gateway --no-fail-fast` (or the narrower command named in the row), record the failing test names, restore with the editor (never `git checkout --`, which would also discard any uncommitted fix), and check `git diff --stat` is empty before the next one. After any fix found by a mutation, re-run EVERY mutation (memory `mutation-battery-rerun-whole`).

| # | Mutation | Must red |
|---|---|---|
| 1 | `chat.rs`: replace the `limits.admit(&caller).await` match with `Some(_) => None` | `limits_http::the_request_over_the_principal_rate_is_429_rate_limited`, `…org_rate_is_shared…`, `…budget_is_used_is_429_budget_exhausted` |
| 2 | Memory: commit the principal before the `failed.is_empty()` check; Redis: move the principal `INCR` above the org check | `limits_store_memory::org_refusal_does_not_grow_the_principal_count`; `limits_store_redis::the_contract_suite_passes_on_redis_6_2` and `…_7_4` |
| 3 | `ChargeGuard::drop`: delete the `store().charge(…)` line | `limits_http::a_client_disconnect_mid_stream_charges_the_estimate_once`, `…a_non_stream_timeout_charges_the_request_estimate` |
| 4 | `chat.rs` relay: delete `scan.feed(&chunk);` | `limits_http::a_stream_with_a_usage_record_charges_the_reported_tokens` |
| 5 | `error.rs`: delete the `header::RETRY_AFTER` insert | `error::tests::a_rate_refusal_is_429_requests_with_retry_after`, `limits_http::the_request_over_the_principal_rate_is_429_rate_limited` |
| 6 | `error.rs`: delete the `x-should-retry: false` insert | `error::tests::a_budget_refusal_is_429_insufficient_quota_and_not_retryable`, `limits_http::the_request_after_the_budget_is_used_is_429_budget_exhausted` |
| 7 | `error.rs`: delete the `x-should-retry: true` insert | `error::tests::a_rate_refusal_is_429_requests_with_retry_after`, `…only_the_two_refusals_carry_x_should_retry` |
| 8 | `build_limits`: delete `prime_metrics(backend);` | `limits_metrics_prime::every_limit_series_reads_zero_after_boot` |
| 9 | `choose_refusal`: check the rates before the budget | `domain::limits::tests::the_refusal_follows_the_d8_precedence`, `application::limits::tests::a_budget_refusal_wins_and_is_counted_as_org_budget` |
| 10 | `MemoryLimitStore::lock`: `self.state.lock().expect("poisoned")` | `adapters::limits::memory::tests::a_poisoned_lock_is_recovered` |
| 11 | Lua: move both `INCR`/`EXPIRE` pairs above the `if p_ok == 1 and …` check | `limits_store_redis` contract suites (D4 rows) |
| 12 | Lua: delete both `EXPIRE … ttl` calls | `limits_store_redis::an_admission_sets_a_180_second_ttl_on_both_current_rate_keys` |
| 13 | `apply_charge`: delete `.expire(&key, …).ignore()` | `limits_store_redis::a_charge_sets_the_relative_budget_ttl` |
| 14 | `charge_ttl`: drop `.saturating_add(LATE_CHARGE_GRACE_SECS)` | `domain::limits::tests::the_charge_ttl_is_relative_to_now`, `limits_store_redis::a_charge_sets_the_relative_budget_ttl` |
| 15 | `Limits::admit`: on `Err(Unavailable)` return `Err(LimitRefusal::Unscoped)` | `limits_http::a_failing_store_admits_and_logs_store_unavailable`, `application::limits::tests::a_store_failure_admits_without_a_ticket_and_counts_the_kind`, `adapters::limits::redis::tests::an_open_breaker_admits_without_a_ticket_and_never_dials` |
| 16 | `Limits::admit`: give the fail-open guard a ticket (`Some(LimitTicket { org: org.unwrap_or_default(), period: BudgetPeriod::Monthly.key_at(self.clock.now()) })`) | `limits_http::a_failing_store_admits_and_logs_store_unavailable` (it sees a charge), `application::limits::tests::a_store_failure_admits_without_a_ticket_and_counts_the_kind` |
| 17 | `Limits::admit`: delete `return Err(LimitRefusal::Unscoped);` | `limits_http::an_unscoped_request_with_a_budget_is_500_internal`, `application::limits::tests::an_unscoped_request_is_refused_when_a_budget_applies` |
| 18 | `RedisLimitStore::charge`: a bare `tokio::spawn` instead of `try_current` + `spawn_on` | `adapters::limits::redis::tests::a_guard_dropped_without_a_runtime_counts_no_runtime` |
| 19 | `paigasus-redis`: ignore the caller's `BreakerMetrics` and hard-code `gateway_redis_breaker_state` / `…_transitions_total` | PR 1's IAM name-pin test (D15). Find it with `grep -rn "with_open_breaker_for_tests" rs/crates/services/paigasus-iam/src rs/crates/services/paigasus-iam/tests` and run `cargo nextest run --locked -p paigasus-iam <its name> --no-fail-fast` |
| 20 | `check_and_admit`: a bare `EVALSHA` (`redis::cmd("EVALSHA").arg(self.script.get_hash()).arg(0)`) instead of `invoke_async` | `limits_store_redis::a_flushed_script_is_reloaded` |
| 21 | `unavailable_kind`: always `UnavailableKind::Io` | `adapters::limits::redis::tests::errors_map_to_their_kinds`, `limits_store_redis::a_full_noeviction_redis_fails_open_with_kind_server` |
| 22 | Delete each new alert rule in turn | its firing case in `promtool test rules ops/observability/prometheus/rules/tests/gateway.test.yml` |

A mutation that reds nothing is a finding: STOP, report it, and add the missing test in the owning task's file before going on.

- [ ] **Step 5: Record the results**

Write the results (the gate list with each result and bash, the Docker proof, the two negative controls, the 22 mutation rows with the tests that reddened, and the `sha1_smol` licence and advisory result from Task 11 Step 7) into the task report for the coordinator, who puts them into the PR body. Do not push; the coordinator runs the review and the PR stages.

---

## Spec coverage (checked at self-review)

| Spec item | Task |
|---|---|
| A1, A2 (rate refusals, headers, no upstream call) | 6, 9 |
| A3 (budget refusal) | 6, 9 |
| A4 (charge rules, both paths, once) | 7, 9, 10 |
| A5 (bytes unchanged) | 9, 10 |
| A6 (no `[limits]` = no change, `limits: None` only) | 5, 9, 13 |
| A7 (metrics, priming, alerts) | 4, 7, 13, 14 |
| A8 (codes, SDK presentation) | 1, 6 |
| A9 (example config) | 15 |
| A10 (include_usage) | 16 |
| A11 (shared Redis counts, restart) | 11, 12 |
| A12 (fail-open, metric, log) | 7, 9, 11, 12 |
| A13 (IAM unchanged) | 17 Step 2 |
| A14 (gates scan the gateway) | 11 Step 7, 12 Step 5, 17 Step 3 |
| D2 org resolution and fail-closed | 7, 9 |
| D3/D4 window and check-then-commit | 2, 4, 11, 12 |
| D6/D7 periods, late charge | 3, 4, 12 |
| D8 precedence, message | 3, 6 |
| D10 fail-open, kinds, log levels | 3, 7, 11 |
| D11 off by default, empty policy, exempt | 3, 5, 7, 9, 13 |
| D12 overrides | 3, 5 |
| D13 memory sweep | 4 |
| D14 estimates | 8 |
| D16 spawned charge, no runtime, shutdown drain | 11, 13 |
| D17 backend, URL redaction, eager boot | 5, 13 |
| D18 keys, TTLs, script | 11, 12 |
| D19 `script` feature, NOSCRIPT, `sha1_smol` | 11, 12 |
| D20 no gateway timeout, blackhole | 11 |
| D21 Redis requirements | 15 |
| D22 canary, nextest, mutex, 6.2 + 7.4 | 12 |
| D23 caps and clamp | 2, 5, 7 |
| § 4.9 SDK and consoles | 1, 16 |
| § 4.10 gateway `moon.yml`, gateway-console inputs | 11, 12 |
| § 5.11 mutation list | 17 |
| § 7 docs | 15 |
