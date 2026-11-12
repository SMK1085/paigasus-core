<!-- moon-diagnosis:ok -->
# SMA-749: gateway tool-call assembler — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Find, hold, decide and rewrite LLM tool calls in `paigasus-gateway`'s `/v1/chat/completions` responses (streaming and non-stream), through a new sans-IO crate `paigasus-gateway-core`, wired in and off by default, with an allow-all decider.

**Architecture:** `paigasus-gateway-core` is a synchronous state machine with no I/O and no async runtime. `Interceptor::push/decided/tick/finish/abort` take bytes, decisions and instants, and return `Vec<Action>` (`Emit`, `Decide`, `Wake`, `Count`). Two `Protocol` implementations (OpenAI Chat, Anthropic Messages) classify strict-parsed SSE records, assemble tool calls, and rewrite released frames. The gateway drives the machine with `tokio::select!` inside the existing relay, in front of the upstream-error conversion. `UsageScanner` and `ChargeGuard` keep reading the raw upstream bytes.

**Tech Stack:** Rust 2024 (serde_json, bytes, thiserror, sha2 0.11, proptest; tokio and axum in the gateway only), protobuf + buf (one error reason), Python 3.12 + uv + pytest (pinned `openai` and `anthropic` SDKs), TypeScript + vitest (two existing parsers).

**Spec:** `docs/superpowers/specs/2026-11-09-sma-749-tool-call-assembler-design.md` (approved 2026-11-09). Read it before any task. Every decision id below (D1–D29, § 4.x) refers to it.

## Plan-level clarifications of the spec

These are choices the spec leaves open. Each is the narrowest reading that keeps invariant I1. Sven can change any of them at plan review.

| Id | Clarification | Where |
|---|---|---|
| P-1 | OpenAI stream records that carry an `event:` field are classified normally and re-framed **without** the event line. Every client then sees the record we classified. | Task 2, Task 6 |
| P-2 | A record whose `data` buffer is empty after the WHATWG join is not dispatched. WHATWG does the same, and we never emit it. | Task 2 |
| P-3 | An oversized record is always dropped, in both `on_failure` modes. With `allow`, the stream then switches to pass-through (§ 5.3). Keeping the record would need unbounded memory. | Task 2, Task 8 |
| P-4 | Pass-through mode (stream-level `allow` after a malformed record or deviation) re-frames every later record without classifying it. It does not forward raw upstream bytes. | Task 8 |
| P-5 | The decider is called for one hold at a time, in completion order. | Task 16 |
| P-6 | `unenforceable` report entries carry `"basis":null` and `"code":null`. Nothing decided them. | Task 4 |
| P-7 | The two first-party TS parsers do not accumulate tool calls. `chat-stream.ts` emits text deltas, and `paigasus-sdk/src/chat.ts` reads terminal error frames. So they get no Rust oracle model. Their golden-file tests (Task 19) assert that they still parse every golden stream, and that the console parser shows the notice as text. | Task 11, Task 19 |
| P-8 | The notice for an Anthropic block is its own text block, so it gets no `\n\n` prefix. The prefix applies to OpenAI choices (§ 4.4). | Task 7 |
| P-9 | Interceptor-level metrics leave the crate as `Action::Count`. The gateway maps them to the four metric names (Task 14). | Task 8, Task 16 |
| P-10 | Two additions to the canonical form (§ 4.3) and the rewrite (§ 4.4). Both close one differential. openai-python finds a stream choice by list POSITION (`choices[choice.index]`, appending on `IndexError`). So a hostile upstream that sends choice 1 before choice 0 makes Python merge the two choices' calls, for example `r` + `m` into an undecided `rm`. (a) A choice index seen for the first time must equal the number of choices seen so far, or it is `Deviation("choice_order")`. (b) The rewrite never removes a choice entry and never drops a frame. A blocked entry keeps its `index` with an emptied `delta`, so the gateway's own rewrite cannot shift Python's positions. § 4.4 says "drop a frame **only** when …", so keeping it stays within the spec. | Task 6 |

## Global Constraints

- Every new source file opens with `// SPDX-License-Identifier: Apache-2.0` (`#` for YAML, TOML, Python and shell).
- Rust crates use edition 2024 and rust-version 1.95, through `edition.workspace` and `rust-version.workspace`.
- `[workspace.lints.rust] warnings = "deny"`, so dead code, an unused import or an unused variable is a hard compile error. Every task leaves the workspace compiling with no warning. In `paigasus-gateway-core`, an item that no task uses yet is `pub` or `pub(crate)` **and used by a test in the same task**. Never add `#[allow(dead_code)]` to production code.
- `cargo clippy --workspace -- -D warnings` must pass. `clippy::all` is `warn`. `unwrap`/`expect` are not denied, but production code uses them only where the value cannot fail (for example serializing a `serde_json::Value`).
- `repo:machete` reds on a Cargo dependency that no code uses. Add each dependency in the task whose code first uses it.
- `rustfmt.toml` sets `max_width = 200`. Run `cargo fmt` in `rs/` after every Rust edit.
- `cd rs` before any `cargo` command. `--manifest-path` from the root silently uses the host toolchain, not the pinned 1.95.
- Prefix every shell with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.
- Conventional commits with a scope (`rs`, `py`, `ts`, `contracts`, `ci`, `docs`). The subject must not start with `SMA-749`; put `(SMA-749)` at the end. No body line may start with `word:`, because commitlint reads it as a footer. End every commit message with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Never use `--no-verify`. Never amend, reset or rebase a commit that an earlier task made. Commit only your own task's files.
- Do not install software on the host. If a tool is missing, stop and report.
- `paigasus-gateway-core` has **no** dependency on tokio, futures, axum, reqwest or any I/O crate (D1). Time comes in as `std::time::Instant` arguments.
- `paigasus-kernel` is not changed (D1).
- `.github/CODEOWNERS` is Moon-generated. Never hand-edit it.
- Registry codes: `ci/error-registry/check.py` scans `rs/crates/**/src/**/*.rs` for string literals equal to a registry code. No `src/` file of `paigasus-gateway-core` may spell a registry code (for example `"upstream-error"`, `"internal"`, `"forbidden"`, `"not-found"`). The terminal SSE frame stays in the gateway and is passed in as bytes. The new code `"tool-interception-malformed-response"` is spelled only in `paigasus-gateway/src/adapters/http/error.rs`.
- Log lines, metric labels and decision reports never contain arguments, prompts, messages, tool results or detector content. Tool names appear only sanitized (§ 4.5).
- `with enabled = false` the gateway behaves exactly as today (D27, AC 1). Every existing gateway test must pass unchanged at every commit.
- Bash on the development Mac: run `ci/affected-graph/run.sh` and `repo:affected-smoke` with `/bin/bash` (3.2). Run `repo:ruff-ci`, `repo:publish-metadata` and `repo:next-public-free` with `/opt/homebrew/bin/bash`. Run `repo:actionlint` only when its pipe preflight passes (see root `CLAUDE.md`).

## Review focus

The spec states these risks. Each line names the task whose test pins it.

1. **A blocked call leaks through a frame queued behind another hold** (D12). Task 8, `a_frame_waits_for_every_hold_it_touches`.
2. **The upstream error arrives mid-hold, and the half-assembled call is released at EOF** (D10, D17). Task 8, `abort_drops_the_queue_and_emits_the_terminal_frame` and `eof_with_an_open_hold_is_truncated_not_complete`.
3. **A JSON differential** (duplicate key, case-folded key, `__proto__`, float index) carries a call that serde does not see. Task 3 unit tests and Task 11 generators.
4. **A decider that returns too few decisions.** Task 8, `a_short_decision_vector_blocks_every_call`.
5. **Billing changes on a block.** Task 16, `a_blocked_stream_charges_like_the_upstream`, and Task 17, `a_blocked_body_charges_like_the_upstream`.
6. **Choices arrive out of order** (P-10). Task 6, `choices_must_first_appear_in_order`.

---

## File structure

| File | Task | Responsibility |
|---|---|---|
| `rs/crates/libs/paigasus-gateway-core/{Cargo.toml,moon.yml,README.md}` | 1 | the crate |
| `rs/crates/libs/paigasus-gateway-core/src/lib.rs` | 1, then each task adds its `mod` line | crate root and the public re-exports |
| `src/types.rs` | 1 | `ToolCall`, `ToolCallKind`, `Decision`, `BlockCode`, `Basis`, `HoldKey`, `Format` (§ 4.1) |
| `src/limits.rs` | 1 | `Limits`, `OnFailure` |
| `src/sse.rs` | 2 | input splitter (D2, D4), output framing (D3), keep-alive |
| `src/json.rs` | 3 | strict parse (D5) |
| `src/report.rs` | 4 | name sanitizing, notice text, decision report, header (§ 4.5, § 4.6, D21, D22) |
| `src/openai/{mod,classify,assembly}.rs` | 5 | OpenAI classification (§ 4.2) and canonical assembly (§ 4.3) |
| `src/protocol.rs` | 6 | `Protocol` trait, `Verdict`, `Frame`, `Step`, `Deviation` |
| `src/openai/stream.rs` | 6 | the OpenAI streaming protocol and its rewrite (§ 4.4) |
| `src/anthropic/{mod,stream}.rs` | 7 | Anthropic classification, assembly and rewrite |
| `src/interceptor.rs` | 8 | `Interceptor`, `Action`, `Metric` (D10–D17, § 5.3, § 6) |
| `src/body.rs`, `src/openai/body.rs`, `src/anthropic/body.rs` | 9 | non-stream extraction and rewrite |
| `src/turn.rs`, `src/decider.rs` | 10 | `ToolTurn` (D19), `DecisionContext`, `ToolCallDecider`, `AllowAll` (D18) |
| `tests/support/{mod,driver,oracle,gen}.rs`, `tests/properties.rs`, `tests/fixtures.rs`, `tests/fixtures/**` | 11 | differential oracle, generators, fixture replays (§ 8.1, § 8.3) |
| `tests/golden.rs`, `tests/golden/**` | 12 | golden streams and their manifest (§ 8.2) |
| `contracts/proto/paigasus/common/v1/error.proto` (+ generated trees, registry mirrors, SDK) | 13 | `ERROR_REASON_TOOL_INTERCEPTION_MALFORMED_RESPONSE = 313` |
| `rs/crates/services/paigasus-gateway/src/adapters/http/error.rs` | 13 | the `GatewayError` variant |
| `rs/crates/libs/paigasus-observability/src/names.rs` (+ RUNBOOK) | 14 | four metric names |
| `rs/crates/services/paigasus-gateway/src/config.rs`, `gateway.toml.example` | 15 | `[tool_interception]` (D26) |
| `rs/crates/services/paigasus-gateway/src/adapters/http/{mod,chat,interception}.rs`, `src/main.rs`, `Cargo.toml`, `moon.yml` | 16, 17 | wiring (§ 5.2) |
| `rs/crates/services/paigasus-gateway/tests/tool_interception.rs` | 16, 17 | integration tests through the router |
| `py/packages/paigasus-gateway-conformance/**`, `py/scripts/assert_test_floor.py`, `py/uv.lock` | 18 | Python SDK conformance (§ 8.2) |
| `ts/apps/gateway-console/tests/unit/chat-stream-golden.test.ts`, `ts/packages/paigasus-sdk/tests/terminal-frame-golden.test.ts`, both `moon.yml` | 19 | first-party parser goldens |
| `rs/Cargo.toml`, `rs/release-plz.toml`, `ci/affected-graph/{run.sh,README.md}` | 1, 16 | registration |
| `rs/crates/libs/paigasus-gateway-core/tests/perf.rs` | 20 | T-perf |

All paths below are relative to the worktree root `/Users/sven/dev/paigasus/paigasus-core-sma-749` unless they start with `rs/…`. In that case, run `cargo` from `rs/`.

---

### Task 0: Provision the worktree and take the baseline

**Files:** none committed.

- [ ] **Step 1: Provision**

```bash
cd /Users/sven/dev/paigasus/paigasus-core-sma-749
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
proto install
pnpm -C ts install
(cd py && uv sync)
(cd rs && cargo fetch --locked)
git status --short
```

Expected: `git status` prints nothing. If `pnpm install` rewrote `.git/hooks`, that is expected (the hooks are shared).

- [ ] **Step 2: Baseline the gateway tests**

```bash
cd rs && cargo nextest run --locked -p paigasus-gateway 2>&1 | tail -3
```

Expected: every test passes. Record the passed count in your task report. Later tasks compare against it (AC 1).

---

### Task 1: The `paigasus-gateway-core` crate with its shared types

**Files:**
- Create: `rs/crates/libs/paigasus-gateway-core/Cargo.toml`, `moon.yml`, `README.md`, `src/lib.rs`, `src/types.rs`, `src/limits.rs`
- Modify: `rs/Cargo.toml` (`members`, `[workspace.dependencies]`), `rs/release-plz.toml`, `ci/affected-graph/run.sh` (both `lockfile->all-lint` CSV lines), `rs/Cargo.lock`

**Interfaces:**
- Consumes: nothing.
- Produces (all `pub`, re-exported from the crate root):
  - `ToolCallKind { Function, Custom, LegacyFunctionCall, AnthropicToolUse, Other }` with `fn as_str(self) -> &'static str`.
  - `ToolCall { choice: u32, position: u32, id: Option<String>, name: String, kind: ToolCallKind, arguments: String }`.
  - `Decision { Allow, Block { code: BlockCode } }` with `fn is_allow(self) -> bool`.
  - `BlockCode { Policy, Detector, HoldLimit, Malformed, Truncated, DeciderError }` with `fn as_kebab(self) -> &'static str`.
  - `Basis { Decider, OnFailure }` with `fn as_str(self) -> &'static str`.
  - `HoldKey { Choice(u32), Block(u32) }` (`Copy`, `Ord`).
  - `Format { OpenAiChat, AnthropicMessages }`.
  - `Limits { max_record_bytes: usize, max_held_bytes: usize, max_hold: Duration, decide_timeout: Duration, keepalive: Duration, on_failure: OnFailure }` and `OnFailure { Block, Allow }`.

- [ ] **Step 1: Write the manifest, the Moon project and the README**

`rs/crates/libs/paigasus-gateway-core/Cargo.toml`:

```toml
[package]
name = "paigasus-gateway-core"
version = "0.0.0"
edition.workspace = true
license.workspace = true
rust-version.workspace = true
authors.workspace = true
publish = false

[dependencies]

[lints]
workspace = true
```

`rs/crates/libs/paigasus-gateway-core/moon.yml`:

```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'

id: 'paigasus-gateway-core-rs'
layer: 'library'
language: 'rust'

# SMA-528 — the transitive dependsOn closure's sources, consumed by build/test/lint via
# `@group(upstreams)` in .moon/tasks/rust.yml. Empty because this crate has no in-tree
# dependencies (SMA-749 D1: sans-IO, no kernel change). Asserted to strict equality by
# repo:affected-smoke's A6.
fileGroups:
  upstreams: []
```

`rs/crates/libs/paigasus-gateway-core/README.md`:

```markdown
# paigasus-gateway-core

The gateway's tool-call interception core (SMA-749, ADR-0027). It is a synchronous state
machine with no I/O: it reads model response bytes, finds every tool call, holds it until it
is complete, asks a decider, and rewrites blocked calls into a notice.

`paigasus-gateway` drives it. Design: `docs/superpowers/specs/2026-11-09-sma-749-tool-call-assembler-design.md`.

Not published.
```

- [ ] **Step 2: Register the crate**

In `rs/Cargo.toml` `members`, add one literal line between `"crates/libs/paigasus-iam-core",` and `"crates/libs/paigasus-kernel",`:

```toml
  "crates/libs/paigasus-gateway-core",
```

In `[workspace.dependencies]`, directly after the `paigasus-redis = { … }` entry, add:

```toml
# In-tree path dep: the gateway's sans-IO tool-call interception core (SMA-749). `version` is
# required: `rs/deny.toml` sets `wildcards = "deny"`.
paigasus-gateway-core = { path = "crates/libs/paigasus-gateway-core", version = "0.0.0" }
```

In `rs/release-plz.toml`, add after the `paigasus-redis` `[[package]]` block:

```toml
[[package]]
name = "paigasus-gateway-core"
release = false
publish = false
git_release_enable = false
```

Find the comment above `[[package]] name = "paigasus-iam-core"` that counts the packages ("all N of these carry", "the first of these N to trip"), and increase both numbers by one:

```bash
grep -n "of these" rs/release-plz.toml
```

In `ci/affected-graph/run.sh`, two lines hold the same `lockfile->all-lint` CSV:

```bash
grep -n "paigasus-gateway-rs:lint,paigasus-iam-core-rs:lint" ci/affected-graph/run.sh
```

In each line the grep prints, replace `paigasus-gateway-rs:lint,paigasus-iam-core-rs:lint,` with:

```
paigasus-gateway-core-rs:lint,paigasus-gateway-rs:lint,paigasus-iam-core-rs:lint,
```

The CSV is sorted. If the grep finds the lines in a different order, put `paigasus-gateway-core-rs:lint` in sorted position.

- [ ] **Step 3: Write the failing tests for the types**

`rs/crates/libs/paigasus-gateway-core/src/lib.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0
//! The gateway's tool-call interception core (SMA-749, ADR-0027).
//!
//! A synchronous state machine with no I/O and no async runtime (spec D1). The gateway feeds it
//! upstream bytes, decisions and instants, and performs the [`Action`]s it returns.

mod limits;
mod types;

pub use limits::{Limits, OnFailure};
pub use types::{Basis, BlockCode, Decision, Format, HoldKey, ToolCall, ToolCallKind};
```

`rs/crates/libs/paigasus-gateway-core/src/types.rs`, with tests first and an empty-bodied implementation that does not compile yet:

```rust
// SPDX-License-Identifier: Apache-2.0
//! The shapes the SMA-747 specs share (spec § 4.1).
//!
//! Every type is `#[non_exhaustive]`: SMA-752 and SMA-754 add variants, and that must not be a
//! breaking change. These types move to `paigasus-kernel` with SMA-752, when a binding needs
//! them (spec D1).

/// Which wire shape a tool call came from.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolCallKind {
    /// OpenAI `tool_calls[]` of type `function`.
    Function,
    /// OpenAI `tool_calls[]` of type `custom`.
    Custom,
    /// OpenAI's legacy `function_call` field (the `functions` API).
    LegacyFunctionCall,
    /// An Anthropic `tool_use` content block.
    AnthropicToolUse,
    /// Any other tool-call shape. Held and decided, never passed unchecked (fail closed).
    Other,
}

impl ToolCallKind {
    /// A stable lower-snake label, for reports and tests.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            ToolCallKind::Function => "function",
            ToolCallKind::Custom => "custom",
            ToolCallKind::LegacyFunctionCall => "legacy_function_call",
            ToolCallKind::AnthropicToolUse => "tool_use",
            ToolCallKind::Other => "other",
        }
    }
}

/// One complete tool call, as the decider sees it.
///
/// `arguments` holds (spec § 4.1): the concatenated `function.arguments` string (Function,
/// LegacyFunctionCall); `custom.input` (Custom); the concatenated `partial_json` of an Anthropic
/// stream, or the compact JSON of a non-stream `input` (AnthropicToolUse); and the raw JSON of the
/// whole entry, as a JSON array of its fragments (Other).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCall {
    /// OpenAI choice index. Always 0 for Anthropic.
    pub choice: u32,
    /// The tool-call index (OpenAI) or the content-block index (Anthropic), as the upstream sent it.
    pub position: u32,
    /// The upstream's call id. `None` for a legacy `function_call`.
    pub id: Option<String>,
    pub name: String,
    pub kind: ToolCallKind,
    pub arguments: String,
}

/// The decider's answer for one call.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Block { code: BlockCode },
}

impl Decision {
    #[must_use]
    pub fn is_allow(self) -> bool {
        matches!(self, Decision::Allow)
    }
}

/// Why a call was blocked.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BlockCode {
    /// A policy refused it (SMA-752).
    Policy,
    /// A detector finding refused it (SMA-754).
    Detector,
    /// A hold exceeded `max_held_bytes` or `max_hold_secs` (D13, D14).
    HoldLimit,
    /// A record or the assembly was not in canonical form (D5–D9).
    Malformed,
    /// The stream ended before the call was complete (D10, D11).
    Truncated,
    /// The decider failed, timed out, or returned the wrong number of decisions (D18).
    DeciderError,
}

impl BlockCode {
    /// The kebab-case label used in the notice and in reports (§ 4.5).
    #[must_use]
    pub fn as_kebab(self) -> &'static str {
        match self {
            BlockCode::Policy => "policy",
            BlockCode::Detector => "detector",
            BlockCode::HoldLimit => "hold-limit",
            BlockCode::Malformed => "malformed",
            BlockCode::Truncated => "truncated",
            BlockCode::DeciderError => "decider-error",
        }
    }
}

/// How a decision was reached (§ 5.3).
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Basis {
    Decider,
    OnFailure,
}

impl Basis {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Basis::Decider => "decider",
            Basis::OnFailure => "on_failure",
        }
    }
}

/// One hold: an OpenAI choice, or an Anthropic content block (D10, D11).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum HoldKey {
    Choice(u32),
    Block(u32),
}

/// Which provider wire format a stream or body uses.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    OpenAiChat,
    AnthropicMessages,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_codes_have_stable_kebab_labels() {
        let all = [BlockCode::Policy, BlockCode::Detector, BlockCode::HoldLimit, BlockCode::Malformed, BlockCode::Truncated, BlockCode::DeciderError];
        let labels: Vec<&str> = all.iter().map(|c| c.as_kebab()).collect();
        assert_eq!(labels, ["policy", "detector", "hold-limit", "malformed", "truncated", "decider-error"]);
    }

    #[test]
    fn only_allow_is_allow() {
        assert!(Decision::Allow.is_allow());
        assert!(!Decision::Block { code: BlockCode::Policy }.is_allow());
    }

    #[test]
    fn kinds_and_bases_have_labels() {
        assert_eq!(ToolCallKind::LegacyFunctionCall.as_str(), "legacy_function_call");
        assert_eq!(ToolCallKind::AnthropicToolUse.as_str(), "tool_use");
        assert_eq!(Basis::OnFailure.as_str(), "on_failure");
    }

    #[test]
    fn hold_keys_order_choices_before_blocks() {
        let mut keys = vec![HoldKey::Block(0), HoldKey::Choice(1), HoldKey::Choice(0)];
        keys.sort();
        assert_eq!(keys, [HoldKey::Choice(0), HoldKey::Choice(1), HoldKey::Block(0)]);
    }
}
```

`rs/crates/libs/paigasus-gateway-core/src/limits.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0
//! The interceptor's limits (spec D8, D13–D16). The gateway builds them from `[tool_interception]`.

use std::time::Duration;

/// What a failure does (D16): block every affected call (the default), or let it through and
/// report it (observe mode, § 5.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnFailure {
    Block,
    Allow,
}

/// The bounds of one stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// D8: a record over this size is never skipped; it is a stream-level failure.
    pub max_record_bytes: usize,
    /// D13: the per-stream total of queued payload bytes.
    pub max_held_bytes: usize,
    /// D14: from a hold's first frame to its completion.
    pub max_hold: Duration,
    /// D14: how long the decider may take for one hold.
    pub decide_timeout: Duration,
    /// D15: the keep-alive interval while anything is held.
    pub keepalive: Duration,
    pub on_failure: OnFailure,
}

impl Limits {
    /// The spec's defaults (§ 5.2). The gateway config uses the same numbers.
    #[must_use]
    pub fn defaults() -> Self {
        Limits {
            max_record_bytes: 1_048_576,
            max_held_bytes: 4 * 1_048_576,
            max_hold: Duration::from_secs(300),
            decide_timeout: Duration::from_secs(5),
            keepalive: Duration::from_secs(10),
            on_failure: OnFailure::Block,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_match_the_spec() {
        let l = Limits::defaults();
        assert_eq!(l.max_record_bytes, 1 << 20);
        assert_eq!(l.max_held_bytes, 4 << 20);
        assert_eq!(l.max_hold, Duration::from_secs(300));
        assert_eq!(l.decide_timeout, Duration::from_secs(5));
        assert_eq!(l.keepalive, Duration::from_secs(10));
        assert_eq!(l.on_failure, OnFailure::Block);
    }
}
```

- [ ] **Step 4: Run the tests**

```bash
cd rs && cargo fmt && cargo nextest run --locked -p paigasus-gateway-core
```

Expected: PASS (5 tests). `cargo` updates `rs/Cargo.lock` with `+name = "paigasus-gateway-core"`. With `--locked` the first run fails with "the lock file needs to be updated". In that case run `cargo nextest run -p paigasus-gateway-core` once without `--locked`, then re-run with `--locked`.

- [ ] **Step 5: Run the registration gates**

```bash
cd rs && cargo clippy --locked -p paigasus-gateway-core --all-targets -- -D warnings && cd ..
/bin/bash ci/affected-graph/run.sh --negative-control && /bin/bash ci/affected-graph/run.sh
moon run repo:affected-smoke
```

Expected: clippy is clean, and the affected-graph suite ends `PASS`, with the two `lockfile->all-lint` cases now including `paigasus-gateway-core-rs:lint`. If `repo:affected-smoke` A6 or A9 reds, read its message: A9 means that the `members` line is missing, and A6 means that `fileGroups.upstreams` does not equal the crate's dependency closure (empty here).

- [ ] **Step 6: Commit**

```bash
git add rs/crates/libs/paigasus-gateway-core rs/Cargo.toml rs/Cargo.lock rs/release-plz.toml ci/affected-graph/run.sh
git commit -m "feat(rs): add the paigasus-gateway-core crate with the tool-call types (SMA-749)" -m "The sans-IO crate for gateway tool-call interception, with the shared
ToolCall, Decision, BlockCode, Basis, HoldKey and Format types and the
interceptor limits. Not published; no kernel change.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---
### Task 2: SSE framing — the input splitter and the output framer

**Files:**
- Create: `rs/crates/libs/paigasus-gateway-core/src/sse.rs`
- Modify: `rs/crates/libs/paigasus-gateway-core/src/lib.rs`, `Cargo.toml` (adds `bytes`)

**Interfaces:**
- Consumes: nothing.
- Produces (`pub(crate)`):
  - `struct Record { event: Option<String>, data: Vec<u8> }`
  - `enum SseItem { Record(Record), Oversized }`
  - `struct SseSplitter` with `fn new(max_record_bytes: usize) -> Self`, `fn push(&mut self, chunk: &[u8]) -> Vec<SseItem>`, `fn finish(&mut self) -> Vec<SseItem>`
  - `fn frame(event: Option<&str>, payload: &[u8]) -> Bytes`
  - `fn comment(text: &str) -> Bytes`
  - `const KEEP_ALIVE: &[u8] = b": keep-alive\n\n"`

Rules (spec D2–D4, P-2, P-3):
- A line ends at CR, LF or CRLF. A CR at the end of a chunk is held as "after CR", so an LF that starts the next chunk is part of the same terminator.
- One leading UTF-8 BOM (`EF BB BF`) is stripped, even when it is split across chunks.
- A line that starts with `:` is a comment and is dropped. `data` appends its value plus `\n`. `event` sets the event name. `id`, `retry` and every unknown field are dropped. A field name is everything before the first `:`. The value loses one leading space.
- A blank line dispatches the record when its data buffer, after the trailing `\n` is removed, is non-empty (P-2).
- `finish()` closes a pending line, then dispatches the pending record (D4). This is the tail-record rule of `UsageScanner::finish`.
- Once a record passes `max_record_bytes`, the splitter emits one `Oversized`, discards bytes up to the next blank line, and then carries on.

- [ ] **Step 1: Add the dependency and the module line**

In `rs/crates/libs/paigasus-gateway-core/Cargo.toml` `[dependencies]`:

```toml
# `Bytes` is the body currency at the gateway boundary (`axum::body::Bytes` is this type).
bytes = { workspace = true }
```

In `src/lib.rs`, add `mod sse;` after `mod limits;`.

- [ ] **Step 2: Write the failing tests**

Create `src/sse.rs` with the tests below, plus the type and function signatures from the Interfaces block, each with a `todo!()` body:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn records(items: Vec<SseItem>) -> Vec<(Option<String>, String)> {
        items
            .into_iter()
            .map(|i| match i {
                SseItem::Record(r) => (r.event, String::from_utf8(r.data).unwrap()),
                SseItem::Oversized => (None, "<oversized>".to_owned()),
            })
            .collect()
    }

    fn split_all(input: &[u8], max: usize) -> Vec<(Option<String>, String)> {
        let mut s = SseSplitter::new(max);
        let mut out = s.push(input);
        out.extend(s.finish());
        records(out)
    }

    #[test]
    fn all_three_line_endings_end_a_record() {
        for input in [&b"data: a\n\ndata: b\n\n"[..], b"data: a\r\n\r\ndata: b\r\n\r\n", b"data: a\r\rdata: b\r\r"] {
            assert_eq!(split_all(input, 1024), vec![(None, "a".into()), (None, "b".into())], "{input:?}");
        }
    }

    #[test]
    fn every_split_point_gives_the_same_records() {
        let input = b"\xEF\xBB\xBFevent: e\r\ndata: {\"x\":1}\r\n\r\n: c\ndata: l1\ndata: l2\n\ndata: tail";
        let whole = split_all(input, 1024);
        assert_eq!(whole, vec![(Some("e".into()), "{\"x\":1}".into()), (None, "l1\nl2".into()), (None, "tail".into())]);
        for i in 0..=input.len() {
            for j in i..=input.len() {
                let mut s = SseSplitter::new(1024);
                let mut out = s.push(&input[..i]);
                out.extend(s.push(&input[i..j]));
                out.extend(s.push(&input[j..]));
                out.extend(s.finish());
                assert_eq!(records(out), whole, "split at {i},{j}");
            }
        }
    }

    #[test]
    fn a_bom_is_stripped_once_and_only_at_the_start() {
        assert_eq!(split_all(b"\xEF\xBB\xBFdata: a\n\n", 64), vec![(None, "a".into())]);
        // A second BOM is part of the field name, so the line is an unknown field and is dropped.
        assert_eq!(split_all(b"\xEF\xBB\xBF\xEF\xBB\xBFdata: a\n\n", 64), vec![]);
        // A BOM-like prefix that is not a BOM is kept.
        assert_eq!(split_all(b"\xEF\xBBdata: a\n\n", 64), vec![]);
    }

    #[test]
    fn comments_ids_retries_and_unknown_fields_are_dropped() {
        assert_eq!(split_all(b": hi\nid: 7\nretry: 10\nfoo: bar\ndata: a\n\n", 64), vec![(None, "a".into())]);
    }

    #[test]
    fn a_lax_line_is_an_unknown_field_not_data() {
        // `  data: x` and `Data: x` are what a trimming or case-blind client would read as data.
        assert_eq!(split_all(b"  data: x\n\nData: y\n\n", 64), vec![]);
    }

    #[test]
    fn data_without_a_space_and_with_two_spaces() {
        assert_eq!(split_all(b"data:a\n\ndata:  b\n\n", 64), vec![(None, "a".into()), (None, " b".into())]);
    }

    #[test]
    fn an_empty_data_buffer_dispatches_nothing() {
        assert_eq!(split_all(b"data:\n\nevent: x\n\n", 64), vec![]);
    }

    #[test]
    fn the_unterminated_tail_is_dispatched_by_finish() {
        let mut s = SseSplitter::new(64);
        assert!(s.push(b"data: a").is_empty());
        assert_eq!(records(s.finish()), vec![(None, "a".into())]);
    }

    #[test]
    fn an_oversized_record_is_reported_once_and_the_next_record_survives() {
        let mut input = b"data: ".to_vec();
        input.extend(std::iter::repeat_n(b'x', 100));
        input.extend_from_slice(b"\ndata: more\n\ndata: ok\n\n");
        assert_eq!(split_all(&input, 32), vec![(None, "<oversized>".into()), (None, "ok".into())]);
    }

    #[test]
    fn frame_writes_one_data_line_per_payload_line() {
        assert_eq!(&frame(None, b"{}")[..], b"data: {}\n\n");
        assert_eq!(&frame(Some("ping"), b"a\nb")[..], b"event: ping\ndata: a\ndata: b\n\n");
    }

    #[test]
    fn comment_never_breaks_the_frame() {
        assert_eq!(&comment("x")[..], b": x\n\n");
        assert_eq!(&comment("a\r\nb")[..], b": a  b\n\n");
        assert_eq!(KEEP_ALIVE, b": keep-alive\n\n");
    }
}
```

- [ ] **Step 3: Run them to verify they fail**

```bash
cd rs && cargo nextest run --locked -p paigasus-gateway-core sse::
```

Expected: FAIL, with the tests panicking at `not yet implemented`.

- [ ] **Step 4: Implement**

Replace the `todo!()` bodies with this module body. It goes above the tests:

```rust
// SPDX-License-Identifier: Apache-2.0
//! SSE framing (spec D2–D4).
//!
//! Input follows WHATWG HTML § 9.2.6 line splitting, with one difference: the last record is
//! dispatched at the end of the stream even without a blank line (D4), as `UsageScanner::finish`
//! does. Output is ALWAYS re-framed by [`frame`]: no upstream byte reaches the client unless the
//! record that carried it was classified (D3). So a BOM, a lax line or an unknown field cannot
//! smuggle a record past the classifier into a client that reads SSE differently.
//!
//! Unlike `UsageScanner`, nothing is ever skipped silently. A record over the cap becomes
//! [`SseItem::Oversized`], which the interceptor treats as a stream-level failure (D8).

use bytes::Bytes;

/// One dispatched SSE record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Record {
    pub event: Option<String>,
    /// The joined `data` lines, without the final `\n`. Never empty (P-2).
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SseItem {
    Record(Record),
    /// A record passed `max_record_bytes`. Its bytes were discarded (P-3).
    Oversized,
}

const BOM: &[u8] = b"\xEF\xBB\xBF";

/// The SSE comment sent while anything is held (D15).
pub(crate) const KEEP_ALIVE: &[u8] = b": keep-alive\n\n";

enum BomState {
    /// Bytes seen so far that could still be a BOM.
    Pending(Vec<u8>),
    Done,
}

pub(crate) struct SseSplitter {
    max_record_bytes: usize,
    bom: BomState,
    line: Vec<u8>,
    /// Bytes in the current line, counted even while skipping.
    line_bytes: usize,
    after_cr: bool,
    record_bytes: usize,
    data: Vec<u8>,
    has_data: bool,
    event: Option<String>,
    skipping: bool,
}

impl SseSplitter {
    pub(crate) fn new(max_record_bytes: usize) -> Self {
        SseSplitter {
            max_record_bytes,
            bom: BomState::Pending(Vec::with_capacity(BOM.len())),
            line: Vec::new(),
            line_bytes: 0,
            after_cr: false,
            record_bytes: 0,
            data: Vec::new(),
            has_data: false,
            event: None,
            skipping: false,
        }
    }

    pub(crate) fn push(&mut self, chunk: &[u8]) -> Vec<SseItem> {
        let mut out = Vec::new();
        if let BomState::Pending(buf) = &mut self.bom {
            buf.extend_from_slice(chunk);
            if buf.len() < BOM.len() && BOM.starts_with(buf) {
                return out;
            }
            let mut head = std::mem::take(buf);
            self.bom = BomState::Done;
            if head.starts_with(BOM) {
                head.drain(..BOM.len());
            }
            for &b in &head {
                self.byte(b, &mut out);
            }
            return out;
        }
        for &b in chunk {
            self.byte(b, &mut out);
        }
        out
    }

    /// The upstream ended: close the pending line, then dispatch the pending record (D4).
    pub(crate) fn finish(&mut self) -> Vec<SseItem> {
        let mut out = Vec::new();
        if let BomState::Pending(buf) = &mut self.bom {
            let head = std::mem::take(buf);
            self.bom = BomState::Done;
            for &b in &head {
                self.byte(b, &mut out);
            }
        }
        if self.line_bytes > 0 {
            self.end_line(&mut out);
        }
        self.after_cr = false;
        if self.skipping {
            self.skipping = false;
            self.reset_record();
        } else {
            self.dispatch(&mut out);
        }
        out
    }

    fn byte(&mut self, b: u8, out: &mut Vec<SseItem>) {
        if self.after_cr {
            self.after_cr = false;
            if b == b'\n' {
                return;
            }
        }
        match b {
            b'\r' => {
                self.after_cr = true;
                self.end_line(out);
            }
            b'\n' => self.end_line(out),
            _ => {
                self.line_bytes += 1;
                self.record_bytes += 1;
                if self.skipping {
                    return;
                }
                if self.record_bytes > self.max_record_bytes {
                    self.skipping = true;
                    self.line.clear();
                    self.reset_record();
                    out.push(SseItem::Oversized);
                    return;
                }
                self.line.push(b);
            }
        }
    }

    fn end_line(&mut self, out: &mut Vec<SseItem>) {
        let blank = self.line_bytes == 0;
        self.line_bytes = 0;
        let line = std::mem::take(&mut self.line);
        if blank {
            if self.skipping {
                self.skipping = false;
                self.reset_record();
            } else {
                self.dispatch(out);
            }
            return;
        }
        if !self.skipping {
            self.field(&line);
        }
    }

    fn field(&mut self, line: &[u8]) {
        if line.first() == Some(&b':') {
            return; // a comment (dropped, D3)
        }
        let (name, value) = match line.iter().position(|&b| b == b':') {
            Some(i) => {
                let v = &line[i + 1..];
                (&line[..i], v.strip_prefix(b" ").unwrap_or(v))
            }
            None => (line, &b""[..]),
        };
        match name {
            b"data" => {
                self.data.extend_from_slice(value);
                self.data.push(b'\n');
                self.has_data = true;
            }
            b"event" => self.event = Some(String::from_utf8_lossy(value).into_owned()),
            _ => {} // `id`, `retry` and unknown fields are dropped (D3)
        }
    }

    fn dispatch(&mut self, out: &mut Vec<SseItem>) {
        if self.has_data {
            let mut data = std::mem::take(&mut self.data);
            data.pop();
            if !data.is_empty() {
                out.push(SseItem::Record(Record { event: self.event.take(), data }));
            }
        }
        self.reset_record();
    }

    fn reset_record(&mut self) {
        self.data.clear();
        self.has_data = false;
        self.event = None;
        self.record_bytes = 0;
    }
}

/// Re-frame one record (D3): an optional `event:` line, one `data:` line per payload line, and the
/// blank line. The payload cannot hold a CR: the splitter ended a line at every CR.
pub(crate) fn frame(event: Option<&str>, payload: &[u8]) -> Bytes {
    let mut out = Vec::with_capacity(payload.len() + 24);
    if let Some(e) = event {
        out.extend_from_slice(b"event: ");
        out.extend_from_slice(e.as_bytes());
        out.push(b'\n');
    }
    for line in payload.split(|&b| b == b'\n') {
        out.extend_from_slice(b"data: ");
        out.extend_from_slice(line);
        out.push(b'\n');
    }
    out.push(b'\n');
    Bytes::from(out)
}

/// One SSE comment. A CR or LF in `text` would end the comment early, so each becomes a space.
pub(crate) fn comment(text: &str) -> Bytes {
    let clean: String = text.chars().map(|c| if c == '\r' || c == '\n' { ' ' } else { c }).collect();
    Bytes::from(format!(": {clean}\n\n"))
}
```

`frame`, `comment` and `KEEP_ALIVE` are unused outside the tests until Task 6 and Task 8. A `pub(crate)` item that only tests use is dead code in a non-test build, and `warnings = "deny"` turns that into an error. So mark the module, until Task 8 removes this line, with:

```rust
#![cfg_attr(not(test), allow(dead_code))] // Removed in Task 8, when the interceptor uses every item.
```

Put it directly under the `//!` doc block. This is the one sanctioned use of the attribute in the plan. Task 8, Step 6 deletes it from every module that carries it, and the build then proves that each item is used.

- [ ] **Step 5: Run the tests**

```bash
cd rs && cargo fmt && cargo nextest run --locked -p paigasus-gateway-core sse::
```

Expected: PASS (11 tests).

- [ ] **Step 6: Commit**

```bash
git add rs/crates/libs/paigasus-gateway-core rs/Cargo.lock
git commit -m "feat(rs): add the SSE splitter and framer for tool-call interception (SMA-749)" -m "WHATWG line splitting with the three line endings, one stripped BOM,
dropped comments and unknown fields, a dispatched tail record, and an
oversized-record signal instead of a skip. Output is always re-framed.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: Strict JSON parsing

**Files:**
- Create: `rs/crates/libs/paigasus-gateway-core/src/json.rs`
- Modify: `src/lib.rs`, `Cargo.toml` (adds `serde`, `serde_json`, `thiserror`)

**Interfaces:**
- Consumes: nothing.
- Produces (`pub(crate)`):
  - `struct Malformed(pub &'static str)`
  - `fn parse_strict(bytes: &[u8]) -> Result<serde_json::Value, Malformed>`
  - `const MAX_OPENAI_INDEX: u64 = 127`, `const MAX_ANTHROPIC_INDEX: u64 = 4095`
  - `fn index_of(v: Option<&Value>, max: u64) -> Result<u32, Malformed>`: a JSON integer literal in `0..=max`. A float form (`0.0`, `1e0`), a bool, a negative number, a string or an absent value is `Malformed("index")`.
  - `fn opt_str(v: Option<&Value>, what: &'static str) -> Result<Option<String>, Malformed>`: absent or `null` is `None`, a string is `Some`, and anything else is `Malformed(what)`.
  - `fn opt_obj<'a>(v: Option<&'a Value>, what: &'static str) -> Result<Option<&'a Map<String, Value>>, Malformed>`, with the same rule for objects.

A record is malformed (D5) when any of these holds:
- it is not one complete JSON value, or has trailing bytes after it;
- it has a duplicate key at any depth;
- it has a `__proto__`, `constructor` or `prototype` key at any depth;
- it has a key that is not byte-equal to a classifier key but equals one after case folding. Folding is `char::to_lowercase`, plus `ſ` (U+017F) → `s` and `K` (U+212A, the Kelvin sign) → `k`, which simple case folding maps and Go's `EqualFold` matches.

`serde_json` already rejects `NaN`, `Infinity`, invalid UTF-8, a lone surrogate escape and nesting deeper than 128.

- [ ] **Step 1: Add the dependencies and the module line**

`Cargo.toml` `[dependencies]`:

```toml
serde = { workspace = true }
serde_json = { workspace = true }
thiserror = { workspace = true }
```

`src/lib.rs`: add `mod json;` after `mod limits;`.

- [ ] **Step 2: Write the failing tests**

Create `src/json.rs` with the signatures, `todo!()` bodies, and:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_plain_record_parses() {
        assert_eq!(parse_strict(br#"{"a":[1,"x",null,true,{"b":2.5}]}"#).unwrap(), json!({"a":[1,"x",null,true,{"b":2.5}]}));
    }

    #[test]
    fn duplicate_keys_are_malformed_at_any_depth() {
        assert!(parse_strict(br#"{"a":1,"a":2}"#).is_err());
        assert!(parse_strict(br#"{"choices":[{"delta":{"tool_calls":[]},"delta":{"content":"hi"}}]}"#).is_err());
        assert!(parse_strict(br#"[{"x":{"y":1,"y":1}}]"#).is_err());
    }

    #[test]
    fn prototype_keys_are_malformed() {
        for k in ["__proto__", "constructor", "prototype"] {
            assert!(parse_strict(format!(r#"{{"choices":[{{"{k}":{{}}}}]}}"#).as_bytes()).is_err(), "{k}");
        }
    }

    #[test]
    fn case_folded_classifier_keys_are_malformed() {
        for k in ["Tool_Calls", "TOOL_CALLS", "tool_callſ", "Delta", "Index", "\u{212A}ey_not_a_classifier_key"] {
            let r = parse_strict(format!(r#"{{"{k}":1}}"#).as_bytes());
            if k.starts_with('\u{212A}') {
                assert!(r.is_ok(), "{k} folds to no classifier key");
            } else {
                assert!(r.is_err(), "{k}");
            }
        }
        // An unrelated key with capitals is fine.
        assert!(parse_strict(br#"{"Model":"x","X-Extra":1}"#).is_ok());
    }

    #[test]
    fn non_json_and_trailing_bytes_are_malformed() {
        for bad in [&b"NaN"[..], b"{\"a\":NaN}", b"{\"a\":Infinity}", b"{} {}", b"{\"a\":1", b"", b"\"\\ud800\""] {
            assert!(parse_strict(bad).is_err(), "{:?}", String::from_utf8_lossy(bad));
        }
    }

    #[test]
    fn a_unicode_escaped_key_is_read_as_the_key_it_spells() {
        // There is no substring prefilter (spec D4): the escaped spelling IS the classifier key.
        let v = parse_strict(br#"{"tool\u005fcalls":[1]}"#).unwrap();
        assert_eq!(v, json!({"tool_calls":[1]}));
    }

    #[test]
    fn index_must_be_a_plain_integer_in_range() {
        assert_eq!(index_of(Some(&json!(0)), MAX_OPENAI_INDEX).unwrap(), 0);
        assert_eq!(index_of(Some(&json!(127)), MAX_OPENAI_INDEX).unwrap(), 127);
        let floaty = parse_strict(b"[0.0, 1e0, -1, true, \"0\", 128]").unwrap();
        for v in floaty.as_array().unwrap() {
            assert!(index_of(Some(v), MAX_OPENAI_INDEX).is_err(), "{v}");
        }
        assert!(index_of(None, MAX_OPENAI_INDEX).is_err());
    }

    #[test]
    fn opt_helpers_treat_null_as_absent() {
        assert_eq!(opt_str(None, "x").unwrap(), None);
        assert_eq!(opt_str(Some(&json!(null)), "x").unwrap(), None);
        assert_eq!(opt_str(Some(&json!("a")), "x").unwrap(), Some("a".to_owned()));
        assert!(opt_str(Some(&json!(1)), "x").is_err());
        assert!(opt_obj(Some(&json!({})), "x").unwrap().is_some());
        assert!(opt_obj(Some(&json!([])), "x").is_err());
    }
}
```

- [ ] **Step 3: Run them to verify they fail**

```bash
cd rs && cargo nextest run --locked -p paigasus-gateway-core json::
```

Expected: FAIL (`not yet implemented`).

- [ ] **Step 4: Implement**

```rust
// SPDX-License-Identifier: Apache-2.0
//! Strict JSON (spec D5).
//!
//! A record that parses everywhere but means different things in different clients is worse than
//! one that fails to parse. serde and JavaScript keep the last of two duplicate keys, Go's
//! `encoding/json` merges struct-typed duplicates, and Go and Newtonsoft match field names
//! case-insensitively. JavaScript gives `__proto__` meaning. Rejecting all of these costs nothing
//! on a conforming upstream.
#![cfg_attr(not(test), allow(dead_code))] // Removed in Task 8, when the interceptor uses every item.

use std::collections::HashSet;
use std::fmt;

use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};

/// A record the gateway cannot classify with certainty. The label is for tests and logs only;
/// it never reaches a metric label with a value from the record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("malformed record: {0}")]
pub(crate) struct Malformed(pub &'static str);

pub(crate) const MAX_OPENAI_INDEX: u64 = 127;
pub(crate) const MAX_ANTHROPIC_INDEX: u64 = 4095;

/// Keys the classifiers read, in every format. A key that folds to one of these without being
/// byte-equal to it is a case-folding differential.
const CLASSIFIER_KEYS: &[&str] = &[
    "arguments", "choices", "content", "content_block", "custom", "delta", "error", "finish_reason", "function", "function_call", "functions", "id", "index",
    "input", "input_schema", "logprobs", "message", "messages", "name", "parameters", "partial_json", "role", "stop_reason", "tool_call_id", "tool_calls",
    "tool_use_id", "tools", "type", "usage",
];

const PROTOTYPE_KEYS: &[&str] = &["__proto__", "constructor", "prototype"];

fn fold(s: &str) -> String {
    s.chars()
        .flat_map(|c| match c {
            '\u{017F}' => 's'.to_lowercase(),
            '\u{212A}' => 'k'.to_lowercase(),
            c => c.to_lowercase(),
        })
        .collect()
}

fn check_key(k: &str) -> Result<(), &'static str> {
    if PROTOTYPE_KEYS.contains(&k) {
        return Err("prototype_key");
    }
    if k.is_ascii() && k.bytes().all(|b| !b.is_ascii_uppercase()) {
        return Ok(()); // already folded; equal to a classifier key only if byte-equal
    }
    let folded = fold(k);
    if CLASSIFIER_KEYS.iter().any(|c| *c != k && folded == *c) {
        return Err("case_folded_key");
    }
    Ok(())
}

/// Parse one record strictly.
pub(crate) fn parse_strict(bytes: &[u8]) -> Result<Value, Malformed> {
    let mut de = serde_json::Deserializer::from_slice(bytes);
    let value = Strict.deserialize(&mut de).map_err(|_| Malformed("json"))?;
    de.end().map_err(|_| Malformed("trailing"))?;
    Ok(value)
}

struct Strict;

impl<'de> DeserializeSeed<'de> for Strict {
    type Value = Value;
    fn deserialize<D: de::Deserializer<'de>>(self, d: D) -> Result<Value, D::Error> {
        d.deserialize_any(StrictVisitor)
    }
}

struct StrictVisitor;

impl<'de> Visitor<'de> for StrictVisitor {
    type Value = Value;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a JSON value")
    }
    fn visit_bool<E: de::Error>(self, v: bool) -> Result<Value, E> {
        Ok(Value::Bool(v))
    }
    fn visit_i64<E: de::Error>(self, v: i64) -> Result<Value, E> {
        Ok(Value::Number(v.into()))
    }
    fn visit_u64<E: de::Error>(self, v: u64) -> Result<Value, E> {
        Ok(Value::Number(v.into()))
    }
    fn visit_f64<E: de::Error>(self, v: f64) -> Result<Value, E> {
        Number::from_f64(v).map(Value::Number).ok_or_else(|| E::custom("non-finite number"))
    }
    fn visit_str<E: de::Error>(self, v: &str) -> Result<Value, E> {
        Ok(Value::String(v.to_owned()))
    }
    fn visit_string<E: de::Error>(self, v: String) -> Result<Value, E> {
        Ok(Value::String(v))
    }
    fn visit_unit<E: de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_none<E: de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
        let mut out = Vec::new();
        while let Some(v) = seq.next_element_seed(Strict)? {
            out.push(v);
        }
        Ok(Value::Array(out))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
        let mut out = Map::new();
        let mut seen = HashSet::new();
        while let Some(k) = map.next_key::<String>()? {
            check_key(&k).map_err(de::Error::custom)?;
            if !seen.insert(k.clone()) {
                return Err(de::Error::custom("duplicate key"));
            }
            let v = map.next_value_seed(Strict)?;
            out.insert(k, v);
        }
        Ok(Value::Object(out))
    }
}

/// A JSON integer literal in `0..=max` (spec § 4.2). serde_json reads `0.0` and `1e0` as floats,
/// so `is_u64` already excludes them.
pub(crate) fn index_of(v: Option<&Value>, max: u64) -> Result<u32, Malformed> {
    match v {
        Some(Value::Number(n)) if n.is_u64() => n.as_u64().filter(|i| *i <= max).and_then(|i| u32::try_from(i).ok()).ok_or(Malformed("index")),
        _ => Err(Malformed("index")),
    }
}

pub(crate) fn opt_str(v: Option<&Value>, what: &'static str) -> Result<Option<String>, Malformed> {
    match v {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(Malformed(what)),
    }
}

pub(crate) fn opt_obj<'a>(v: Option<&'a Value>, what: &'static str) -> Result<Option<&'a Map<String, Value>>, Malformed> {
    match v {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Object(m)) => Ok(Some(m)),
        Some(_) => Err(Malformed(what)),
    }
}
```

The fast path in `check_key` is the reason it stays cheap: almost every real key is lower-case ASCII, and the only lower-case ASCII string that folds to a classifier key is the key itself.

- [ ] **Step 5: Run the tests**

```bash
cd rs && cargo fmt && cargo nextest run --locked -p paigasus-gateway-core json::
```

Expected: PASS (8 tests). If `case_folded_classifier_keys_are_malformed` fails on `tool_callſ`, check that `fold` maps U+017F before `to_lowercase` (`ſ` is already lower case, so `to_lowercase` leaves it unchanged).

- [ ] **Step 6: Commit**

```bash
git add rs/crates/libs/paigasus-gateway-core rs/Cargo.lock
git commit -m "feat(rs): add strict JSON parsing for tool-call interception (SMA-749)" -m "Rejects duplicate keys at any depth, prototype keys, and keys that only
case-fold to a classifier key, so a record cannot mean one thing to the
gateway and another to a client. Index values must be integer literals.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: Names, notices and decision reports

**Files:**
- Create: `rs/crates/libs/paigasus-gateway-core/src/report.rs`
- Modify: `src/lib.rs`, `Cargo.toml` (adds `sha2`)

**Interfaces:**
- Consumes: `BlockCode`, `Basis` (Task 1), `sse::comment` (Task 2).
- Produces (`pub`, re-exported):
  - `fn sanitize_name(name: &str) -> String`: the first 128 bytes, with every byte outside `[A-Za-z0-9_.:-]` replaced by `_`.
  - `fn name_sha256_8(name: &str) -> String`: the first 4 bytes of SHA-256 over the raw name, as 8 lower-case hex digits.
  - `fn notice_line(name: &str, code: BlockCode) -> String`, which returns `[paigasus] tool call "<sanitized>" was blocked (<kebab>).`
  - `enum ReportDecision { Allow, Block, Unenforceable }`
  - `struct ReportEntry { choice: u32, position: u32, name: String, decision: ReportDecision, basis: Option<Basis>, code: Option<BlockCode> }` with `fn to_json(&self) -> String`.
  - `fn header_value(entries: &[ReportEntry]) -> String`: a JSON array, ASCII only, at most 2048 bytes. When the entries do not fit, the array ends with `{"truncated":true}`.
  - `pub(crate) fn report_comment(entry: &ReportEntry) -> Bytes`, which returns `: paigasus-decision <json>\n\n`.

- [ ] **Step 1: Add the dependency and the module line**

`Cargo.toml` `[dependencies]`:

```toml
# `name_sha256_8` in decision reports (spec § 4.6). The same major as paigasus-iam-core's line,
# already in rs/Cargo.lock.
sha2 = "0.11"
```

`src/lib.rs`: add `mod report;` and `pub use report::{ReportDecision, ReportEntry, header_value, name_sha256_8, notice_line, sanitize_name};`.

- [ ] **Step 2: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str) -> ReportEntry {
        ReportEntry { choice: 0, position: 1, name: name.to_owned(), decision: ReportDecision::Block, basis: Some(Basis::Decider), code: Some(BlockCode::Policy) }
    }

    #[test]
    fn names_are_sanitized_and_truncated() {
        assert_eq!(sanitize_name("read_file.v2:x-y"), "read_file.v2:x-y");
        assert_eq!(sanitize_name("rm -rf /"), "rm_-rf__");
        assert_eq!(sanitize_name("é"), "__"); // two UTF-8 bytes, two underscores
        assert_eq!(sanitize_name(&"a".repeat(300)).len(), 128);
    }

    #[test]
    fn the_hash_tells_names_apart_that_sanitize_alike() {
        assert_eq!(sanitize_name("a b"), sanitize_name("a\tb"));
        assert_ne!(name_sha256_8("a b"), name_sha256_8("a\tb"));
        assert_eq!(name_sha256_8("").len(), 8);
        assert_eq!(name_sha256_8(""), "e3b0c442"); // SHA-256 of the empty string starts e3b0c442
    }

    #[test]
    fn the_notice_has_the_fixed_shape() {
        assert_eq!(notice_line("send mail", BlockCode::HoldLimit), "[paigasus] tool call \"send_mail\" was blocked (hold-limit).");
    }

    #[test]
    fn an_entry_serializes_to_the_schema() {
        let v: serde_json::Value = serde_json::from_str(&entry("x y").to_json()).unwrap();
        assert_eq!(
            v,
            serde_json::json!({"v":1,"choice":0,"position":1,"name":"x_y","name_sha256_8":name_sha256_8("x y"),"decision":"block","basis":"decider","code":"policy"})
        );
        let u = ReportEntry { decision: ReportDecision::Unenforceable, basis: None, code: None, ..entry("web_search") };
        let v: serde_json::Value = serde_json::from_str(&u.to_json()).unwrap();
        assert_eq!(v["decision"], "unenforceable");
        assert!(v["basis"].is_null() && v["code"].is_null());
    }

    #[test]
    fn the_header_is_ascii_and_capped() {
        let one = header_value(&[entry("é")]);
        assert!(one.is_ascii());
        assert!(one.starts_with('[') && one.ends_with(']'));
        let many: Vec<ReportEntry> = (0..200).map(|i| entry(&format!("tool_{i}"))).collect();
        let h = header_value(&many);
        assert!(h.len() <= 2048, "{}", h.len());
        assert!(h.ends_with(r#"{"truncated":true}]"#));
        let parsed: Vec<serde_json::Value> = serde_json::from_str(&h).unwrap();
        assert!(parsed.len() < 200);
        assert_eq!(header_value(&[]), "[]");
    }

    #[test]
    fn the_comment_is_one_sse_comment() {
        let c = report_comment(&entry("x"));
        assert!(c.starts_with(b": paigasus-decision {"));
        assert!(c.ends_with(b"}\n\n"));
        assert_eq!(c.iter().filter(|b| **b == b'\n').count(), 2);
    }
}
```

- [ ] **Step 3: Run them to verify they fail**

```bash
cd rs && cargo nextest run --locked -p paigasus-gateway-core report::
```

Expected: FAIL (compile error or `not yet implemented`).

- [ ] **Step 4: Implement**

```rust
// SPDX-License-Identifier: Apache-2.0
//! Names, notices and decision reports (spec § 4.5, § 4.6, D21, D22).
//!
//! Tool names come from the model, so they are hostile (T1). They reach a notice, a report and a
//! header only after [`sanitize_name`]. The notice text is built from a closed set (the block code
//! and the sanitized name), never from argument or detector content: the model reads the notice
//! on its next turn, so echoing content would reflect attacker text into the prompt (D21).

use bytes::Bytes;
use sha2::{Digest, Sha256};

use crate::sse;
use crate::types::{Basis, BlockCode};

const MAX_NAME_BYTES: usize = 128;
const MAX_HEADER_BYTES: usize = 2048;
const TRUNCATED: &str = r#"{"truncated":true}"#;

#[must_use]
pub fn sanitize_name(name: &str) -> String {
    name.bytes()
        .take(MAX_NAME_BYTES)
        .map(|b| if b.is_ascii_alphanumeric() || b"_.:-".contains(&b) { b as char } else { '_' })
        .collect()
}

#[must_use]
pub fn name_sha256_8(name: &str) -> String {
    Sha256::digest(name.as_bytes())[..4].iter().map(|b| format!("{b:02x}")).collect()
}

#[must_use]
pub fn notice_line(name: &str, code: BlockCode) -> String {
    format!("[paigasus] tool call \"{}\" was blocked ({}).", sanitize_name(name), code.as_kebab())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportDecision {
    Allow,
    Block,
    /// A provider-executed tool (D3, § 4.2): reported, never decided (P-6).
    Unenforceable,
}

impl ReportDecision {
    fn as_str(self) -> &'static str {
        match self {
            ReportDecision::Allow => "allow",
            ReportDecision::Block => "block",
            ReportDecision::Unenforceable => "unenforceable",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportEntry {
    pub choice: u32,
    pub position: u32,
    /// The raw name. Serialized only through `sanitize_name` and `name_sha256_8`.
    pub name: String,
    pub decision: ReportDecision,
    pub basis: Option<Basis>,
    pub code: Option<BlockCode>,
}

impl ReportEntry {
    /// The § 4.6 schema. Every field is ASCII by construction.
    #[must_use]
    pub fn to_json(&self) -> String {
        serde_json::json!({
            "v": 1,
            "choice": self.choice,
            "position": self.position,
            "name": sanitize_name(&self.name),
            "name_sha256_8": name_sha256_8(&self.name),
            "decision": self.decision.as_str(),
            "basis": self.basis.map(Basis::as_str),
            "code": self.code.map(BlockCode::as_kebab),
        })
        .to_string()
    }
}

/// The `paigasus-tool-decisions` header value (D22).
#[must_use]
pub fn header_value(entries: &[ReportEntry]) -> String {
    let mut out = String::from("[");
    for (i, e) in entries.iter().enumerate() {
        let json = e.to_json();
        let sep = usize::from(i > 0);
        // Room for this entry, then a separator, TRUNCATED and the closing bracket.
        if out.len() + sep + json.len() + 1 + TRUNCATED.len() + 1 > MAX_HEADER_BYTES {
            if i > 0 {
                out.push(',');
            }
            out.push_str(TRUNCATED);
            out.push(']');
            return out;
        }
        if i > 0 {
            out.push(',');
        }
        out.push_str(&json);
    }
    out.push(']');
    debug_assert!(out.is_ascii());
    out
}

/// One `: paigasus-decision` SSE comment (D22).
pub(crate) fn report_comment(entry: &ReportEntry) -> Bytes {
    sse::comment(&format!("paigasus-decision {}", entry.to_json()))
}
```

`report_comment` has no production user until Task 8. Add the same `#![cfg_attr(not(test), allow(dead_code))] // Removed in Task 8 …` line under the module doc.

- [ ] **Step 5: Run the tests**

```bash
cd rs && cargo fmt && cargo nextest run --locked -p paigasus-gateway-core report:: && git diff --stat rs/Cargo.lock
```

Expected: PASS (6 tests). `rs/Cargo.lock` gains only the `sha2` edge to the existing 0.11 package. If it gains a new `[[package]]`, stop: the version must match `paigasus-iam-core`'s `sha2 = "0.11"`.

- [ ] **Step 6: Commit**

```bash
git add rs/crates/libs/paigasus-gateway-core rs/Cargo.lock
git commit -m "feat(rs): add tool-call notices and decision reports (SMA-749)" -m "Sanitized names with a short hash, the fixed notice text, the report
entry schema, the capped ASCII header and the SSE report comment.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---
### Task 5: OpenAI classification and canonical assembly

**Files:**
- Create: `rs/crates/libs/paigasus-gateway-core/src/openai/mod.rs`, `src/openai/classify.rs`, `src/openai/assembly.rs`, `src/protocol.rs` (only the `Deviation` type for now; Task 6 adds the rest)
- Modify: `src/lib.rs`

**Interfaces:**
- Consumes: `parse_strict`, `index_of`, `opt_str`, `opt_obj`, `Malformed`, `MAX_OPENAI_INDEX` (Task 3); `ToolCall`, `ToolCallKind` (Task 1).
- Produces (`pub(crate)`):
  - `protocol::Deviation(pub &'static str)`, a canonical-form violation (§ 4.3).
  - `openai::classify::{Chunk, ChoiceView, Fragment, NameArgs, chunk}`:
    - `enum Chunk { Choices(Vec<ChoiceView>), UpstreamError }`
    - `struct ChoiceView { index: u32, fragments: Vec<Fragment>, legacy: Option<NameArgs>, finish_reason: Option<String>, has_text: bool }`
    - `struct Fragment { index: u32, id: Option<String>, ty: Option<String>, function: Option<NameArgs>, custom: Option<NameArgs>, raw: Value }`
    - `struct NameArgs { name: Option<String>, args: Option<String> }`
    - `fn chunk(v: &Value) -> Result<Chunk, Malformed>`
  - `openai::assembly::ChoiceAssembly` (`Default`) with:
    - `fn apply(&mut self, frags: &[Fragment], legacy: Option<&NameArgs>) -> Result<(), Deviation>`
    - `fn finish(&self, choice: u32) -> Result<Vec<ToolCall>, Deviation>`
    - `fn names(&self) -> Vec<(u32, String)>`, which returns (position, name or `"unknown"`)

Classification (§ 4.2, stream chunk):
- The payload must be an object. A top-level `error` key without `choices` is `Chunk::UpstreamError`. `error` together with `choices` is malformed.
- `choices` is absent (no choices) or an array of objects. `null` or any other type is malformed.
- In each choice:
  - `index` must be an integer literal in `0..=127`. A `message` key is malformed.
  - `delta` is absent or an object.
  - `finish_reason` is absent, `null` or a string.
  - `delta.content` is absent, `null` or a string. `has_text` is true when it is a non-empty string.
  - `delta.tool_calls` is absent, `null`, `[]`, or an array of objects.
  - `delta.function_call` is absent, `null` or an object.
- In each tool-call entry:
  - `index` must be an integer literal in `0..=127`.
  - `id` and `type` are each absent, `null` or a string.
  - `function` and `custom` are each absent, `null` or an object. Inside them, `name` and `arguments` (or `input`) are each absent, `null` or a string.
  - `function` and `custom` together are malformed.

Assembly (§ 4.3, per choice):
- A tool-call `index` equal to the number of calls so far starts a new call. A smaller index continues an existing call. A larger one is `Deviation("index_gap")`.
- `id`, `type` and `name` are set by exactly one fragment each. A second one is `repeated_*`.
- `function` versus `custom` never changes for a call (`shape_changed`).
- A legacy `function_call` and `tool_calls` never share a choice (`legacy_and_tool_calls`).
- At `finish()`, the kind is decided as follows:
  - `function` (or no type) with a `function` body is Function.
  - `custom` (or no type) with a `custom` body is Custom.
  - A known type with the other body is `type_shape_mismatch`.
  - A known type with no body is `missing_body`.
  - An unknown type is Other. Its arguments are the JSON array of its raw fragments, and its name is its `name`, else its `type`.
- A Function, Custom or legacy call with no name is `missing_name`.

- [ ] **Step 1: Write the module skeleton**

`src/protocol.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0
//! What the two wire protocols share (spec § 4, D12). Task 6 adds the `Protocol` trait.

/// A violation of the canonical assembly form (spec § 4.3). Always a stream-level `on_failure`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Deviation(pub &'static str);
```

`src/openai/mod.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0
//! OpenAI Chat Completions (spec § 4.2–§ 4.4).

pub(crate) mod assembly;
pub(crate) mod classify;
```

`src/lib.rs`: add `mod openai;` and `mod protocol;`.

Both new files carry `#![cfg_attr(not(test), allow(dead_code))] // Removed in Task 8 …` under their module doc.

- [ ] **Step 2: Write the failing classification tests** (in `src/openai/classify.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::parse_strict;

    fn c(s: &str) -> Result<Chunk, Malformed> {
        chunk(&parse_strict(s.as_bytes()).unwrap())
    }
    fn choices(s: &str) -> Vec<ChoiceView> {
        match c(s).unwrap() {
            Chunk::Choices(v) => v,
            Chunk::UpstreamError => panic!("not an error"),
        }
    }

    #[test]
    fn a_text_delta_is_tool_free() {
        let v = choices(r#"{"id":"x","choices":[{"index":0,"delta":{"role":"assistant","content":"hi"},"finish_reason":null}]}"#);
        assert_eq!(v.len(), 1);
        assert!(v[0].fragments.is_empty() && v[0].legacy.is_none() && v[0].has_text);
    }

    #[test]
    fn empty_or_null_tool_calls_are_tool_free() {
        for d in [r#"{"tool_calls":[]}"#, r#"{"tool_calls":null}"#, r#"{"function_call":null}"#, r#"{}"#] {
            let v = choices(&format!(r#"{{"choices":[{{"index":0,"delta":{d}}}]}}"#));
            assert!(v[0].fragments.is_empty() && v[0].legacy.is_none(), "{d}");
        }
    }

    #[test]
    fn a_usage_chunk_has_no_choices() {
        assert!(choices(r#"{"choices":[],"usage":{"total_tokens":3}}"#).is_empty());
        assert!(choices(r#"{"usage":{"total_tokens":3}}"#).is_empty());
    }

    #[test]
    fn a_tool_fragment_is_read() {
        let v = choices(r#"{"choices":[{"index":1,"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"rm","arguments":""}}]}}]}"#);
        let f = &v[0].fragments[0];
        assert_eq!((v[0].index, f.index, f.id.as_deref(), f.ty.as_deref()), (1, 0, Some("call_1"), Some("function")));
        assert_eq!(f.function.as_ref().unwrap().name.as_deref(), Some("rm"));
    }

    #[test]
    fn a_legacy_function_call_is_read() {
        let v = choices(r#"{"choices":[{"index":0,"delta":{"function_call":{"name":"f","arguments":"{"}}}]}"#);
        assert_eq!(v[0].legacy.as_ref().unwrap().args.as_deref(), Some("{"));
    }

    #[test]
    fn an_error_record_is_an_upstream_error() {
        assert!(matches!(c(r#"{"error":{"message":"x"}}"#).unwrap(), Chunk::UpstreamError));
        assert!(c(r#"{"error":{},"choices":[]}"#).is_err());
    }

    #[test]
    fn every_non_canonical_shape_is_malformed() {
        for bad in [
            r#"[]"#,
            r#"{"choices":null}"#,
            r#"{"choices":{"0":{}}}"#,
            r#"{"choices":[1]}"#,
            r#"{"choices":[{"delta":{}}]}"#,
            r#"{"choices":[{"index":0.0,"delta":{}}]}"#,
            r#"{"choices":[{"index":0,"message":{"tool_calls":[]}}]}"#,
            r#"{"choices":[{"index":0,"delta":"x"}]}"#,
            r#"{"choices":[{"index":0,"delta":{"tool_calls":{"0":{}}}}]}"#,
            r#"{"choices":[{"index":0,"delta":{"tool_calls":"x"}}]}"#,
            r#"{"choices":[{"index":0,"delta":{"tool_calls":[1]}}]}"#,
            r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":true}]}}]}"#,
            r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":{}}}]}}]}"#,
            r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{},"custom":{}}]}}]}"#,
            r#"{"choices":[{"index":0,"delta":{"function_call":"rm"}}]}"#,
            r#"{"choices":[{"index":0,"delta":{"content":["x"]}}]}"#,
            r#"{"choices":[{"index":0,"finish_reason":1}]}"#,
        ] {
            assert!(c(bad).is_err(), "{bad}");
        }
    }
}
```

- [ ] **Step 3: Write the failing assembly tests** (in `src/openai/assembly.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::parse_strict;
    use crate::openai::classify::{Chunk, chunk};
    use crate::types::ToolCallKind;

    /// The fragments and legacy part of choice 0 of one chunk.
    fn frags(s: &str) -> (Vec<Fragment>, Option<NameArgs>) {
        let Chunk::Choices(mut v) = chunk(&parse_strict(s.as_bytes()).unwrap()).unwrap() else { panic!() };
        let c = v.remove(0);
        (c.fragments, c.legacy)
    }
    fn tc(entries: &str) -> String {
        format!(r#"{{"choices":[{{"index":0,"delta":{{"tool_calls":[{entries}]}}}}]}}"#)
    }
    fn feed(a: &mut ChoiceAssembly, s: &str) -> Result<(), Deviation> {
        let (f, l) = frags(s);
        a.apply(&f, l.as_ref())
    }

    #[test]
    fn two_interleaved_calls_assemble() {
        let mut a = ChoiceAssembly::default();
        feed(&mut a, &tc(r#"{"index":0,"id":"c0","type":"function","function":{"name":"read","arguments":"{\"p\":"}}"#)).unwrap();
        feed(&mut a, &tc(r#"{"index":1,"id":"c1","type":"function","function":{"name":"rm","arguments":"{"}}"#)).unwrap();
        feed(&mut a, &tc(r#"{"index":0,"function":{"arguments":"1}"}},{"index":1,"function":{"arguments":"}"}}"#)).unwrap();
        let calls = a.finish(0).unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!((calls[0].name.as_str(), calls[0].arguments.as_str(), calls[0].kind), ("read", "{\"p\":1}", ToolCallKind::Function));
        assert_eq!((calls[1].position, calls[1].id.as_deref(), calls[1].arguments.as_str()), (1, Some("c1"), "{}"));
    }

    #[test]
    fn a_custom_call_assembles_from_input() {
        let mut a = ChoiceAssembly::default();
        feed(&mut a, &tc(r#"{"index":0,"id":"c","type":"custom","custom":{"name":"sql","input":"SELECT"}}"#)).unwrap();
        feed(&mut a, &tc(r#"{"index":0,"custom":{"input":" 1"}}"#)).unwrap();
        let calls = a.finish(0).unwrap();
        assert_eq!((calls[0].kind, calls[0].arguments.as_str()), (ToolCallKind::Custom, "SELECT 1"));
    }

    #[test]
    fn a_legacy_call_assembles() {
        let mut a = ChoiceAssembly::default();
        feed(&mut a, r#"{"choices":[{"index":0,"delta":{"function_call":{"name":"f","arguments":"{"}}}]}"#).unwrap();
        feed(&mut a, r#"{"choices":[{"index":0,"delta":{"function_call":{"arguments":"}"}}}]}"#).unwrap();
        let calls = a.finish(0).unwrap();
        assert_eq!((calls[0].kind, calls[0].id.clone(), calls[0].arguments.as_str()), (ToolCallKind::LegacyFunctionCall, None, "{}"));
    }

    #[test]
    fn an_unknown_type_is_other_with_its_raw_fragments() {
        let mut a = ChoiceAssembly::default();
        feed(&mut a, &tc(r#"{"index":0,"id":"c","type":"shell","shell":{"cmd":"ls"}}"#)).unwrap();
        let calls = a.finish(0).unwrap();
        assert_eq!((calls[0].kind, calls[0].name.as_str()), (ToolCallKind::Other, "shell"));
        let raw: serde_json::Value = serde_json::from_str(&calls[0].arguments).unwrap();
        assert_eq!(raw[0]["shell"]["cmd"], "ls");
    }

    #[test]
    fn every_canonical_violation_is_a_deviation() {
        let cases: &[(&[&str], &str)] = &[
            (&[r#"{"index":1,"id":"c","type":"function","function":{"name":"x"}}"#], "index_gap"),
            (&[r#"{"index":0,"id":"c","type":"function","function":{"name":"safe_"}}"#, r#"{"index":0,"function":{"name":"rm"}}"#], "repeated_name"),
            (&[r#"{"index":0,"id":"c","type":"function","function":{"name":"x"}}"#, r#"{"index":0,"id":"d"}"#], "repeated_id"),
            (&[r#"{"index":0,"id":"c","type":"function","function":{"name":"x"}}"#, r#"{"index":0,"type":"function"}"#], "repeated_type"),
            (&[r#"{"index":0,"id":"c","type":"function","function":{"name":"x"}}"#, r#"{"index":0,"custom":{"input":"y"}}"#], "shape_changed"),
        ];
        for (seq, want) in cases {
            let mut a = ChoiceAssembly::default();
            let mut got = None;
            for s in *seq {
                if let Err(d) = feed(&mut a, &tc(s)) {
                    got = Some(d.0);
                    break;
                }
            }
            assert_eq!(got, Some(*want), "{seq:?}");
        }
    }

    #[test]
    fn finish_rejects_incomplete_or_contradictory_calls() {
        let cases = [
            (r#"{"index":0,"id":"c","type":"function","function":{"arguments":"{}"}}"#, "missing_name"),
            (r#"{"index":0,"id":"c","type":"function","custom":{"name":"x"}}"#, "type_shape_mismatch"),
            (r#"{"index":0,"id":"c","type":"function"}"#, "missing_body"),
        ];
        for (s, want) in cases {
            let mut a = ChoiceAssembly::default();
            feed(&mut a, &tc(s)).unwrap();
            assert_eq!(a.finish(0).unwrap_err().0, want, "{s}");
        }
    }

    #[test]
    fn legacy_and_tool_calls_never_share_a_choice() {
        let mut a = ChoiceAssembly::default();
        feed(&mut a, &tc(r#"{"index":0,"id":"c","type":"function","function":{"name":"x"}}"#)).unwrap();
        assert_eq!(feed(&mut a, r#"{"choices":[{"index":0,"delta":{"function_call":{"name":"f"}}}]}"#).unwrap_err().0, "legacy_and_tool_calls");
    }

    #[test]
    fn names_report_unknown_until_a_name_arrives() {
        let mut a = ChoiceAssembly::default();
        feed(&mut a, &tc(r#"{"index":0,"id":"c","type":"function"}"#)).unwrap();
        assert_eq!(a.names(), vec![(0, "unknown".to_owned())]);
        feed(&mut a, &tc(r#"{"index":0,"function":{"name":"rm"}}"#)).unwrap();
        assert_eq!(a.names(), vec![(0, "rm".to_owned())]);
    }
}
```

- [ ] **Step 4: Run them to verify they fail**

```bash
cd rs && cargo nextest run --locked -p paigasus-gateway-core openai::
```

Expected: FAIL (compile errors until the types exist).

- [ ] **Step 5: Implement classification**

`src/openai/classify.rs` (above the tests):

```rust
// SPDX-License-Identifier: Apache-2.0
//! OpenAI stream-chunk classification by a positive allowlist (spec § 4.2, D6).
//!
//! A chunk is tool-free only when it matches the strict tool-free shape. A non-empty
//! `tool_calls` or a `function_call` object makes it tool-bearing. Anything else is malformed.
//! "Has a `tool_calls` key" would be wrong both ways: it misses `tool_calls` as an object, and it
//! holds every text delta of an upstream that sends `"tool_calls": []`.

use serde_json::{Map, Value};

use crate::json::{MAX_OPENAI_INDEX, Malformed, index_of, opt_obj, opt_str};

pub(crate) enum Chunk {
    Choices(Vec<ChoiceView>),
    UpstreamError,
}

pub(crate) struct ChoiceView {
    pub index: u32,
    pub fragments: Vec<Fragment>,
    pub legacy: Option<NameArgs>,
    pub finish_reason: Option<String>,
    pub has_text: bool,
}

pub(crate) struct Fragment {
    pub index: u32,
    pub id: Option<String>,
    pub ty: Option<String>,
    pub function: Option<NameArgs>,
    pub custom: Option<NameArgs>,
    /// The whole entry, for kind Other (§ 4.1).
    pub raw: Value,
}

pub(crate) struct NameArgs {
    pub name: Option<String>,
    pub args: Option<String>,
}

pub(crate) fn chunk(v: &Value) -> Result<Chunk, Malformed> {
    let Value::Object(top) = v else { return Err(Malformed("not_object")) };
    if top.contains_key("error") {
        return if top.contains_key("choices") { Err(Malformed("error_and_choices")) } else { Ok(Chunk::UpstreamError) };
    }
    let choices = match top.get("choices") {
        None => Vec::new(),
        Some(Value::Array(a)) => a.iter().map(choice).collect::<Result<_, _>>()?,
        Some(_) => return Err(Malformed("choices")),
    };
    Ok(Chunk::Choices(choices))
}

fn choice(v: &Value) -> Result<ChoiceView, Malformed> {
    let Value::Object(c) = v else { return Err(Malformed("choice")) };
    if c.contains_key("message") {
        return Err(Malformed("message_in_chunk"));
    }
    let index = index_of(c.get("index"), MAX_OPENAI_INDEX)?;
    let finish_reason = opt_str(c.get("finish_reason"), "finish_reason")?;
    let (fragments, legacy, has_text) = match c.get("delta") {
        None => (Vec::new(), None, false),
        Some(Value::Object(d)) => delta(d)?,
        Some(_) => return Err(Malformed("delta")),
    };
    Ok(ChoiceView { index, fragments, legacy, finish_reason, has_text })
}

fn delta(d: &Map<String, Value>) -> Result<(Vec<Fragment>, Option<NameArgs>, bool), Malformed> {
    let fragments = match d.get("tool_calls") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(a)) => a.iter().map(fragment).collect::<Result<_, _>>()?,
        Some(_) => return Err(Malformed("tool_calls")),
    };
    let legacy = opt_obj(d.get("function_call"), "function_call")?.map(|f| name_args(f, "arguments")).transpose()?;
    let has_text = match d.get("content") {
        None | Some(Value::Null) => false,
        Some(Value::String(s)) => !s.is_empty(),
        Some(_) => return Err(Malformed("content")),
    };
    Ok((fragments, legacy, has_text))
}

fn fragment(v: &Value) -> Result<Fragment, Malformed> {
    let Value::Object(e) = v else { return Err(Malformed("tool_call")) };
    let index = index_of(e.get("index"), MAX_OPENAI_INDEX)?;
    let id = opt_str(e.get("id"), "id")?;
    let ty = opt_str(e.get("type"), "type")?;
    let function = opt_obj(e.get("function"), "function")?.map(|f| name_args(f, "arguments")).transpose()?;
    let custom = opt_obj(e.get("custom"), "custom")?.map(|f| name_args(f, "input")).transpose()?;
    if function.is_some() && custom.is_some() {
        return Err(Malformed("function_and_custom"));
    }
    Ok(Fragment { index, id, ty, function, custom, raw: v.clone() })
}

pub(crate) fn name_args(f: &Map<String, Value>, args_key: &'static str) -> Result<NameArgs, Malformed> {
    Ok(NameArgs { name: opt_str(f.get("name"), "name")?, args: opt_str(f.get(args_key), args_key)? })
}
```

- [ ] **Step 6: Implement assembly**

`src/openai/assembly.rs` (above the tests):

```rust
// SPDX-License-Identifier: Apache-2.0
//! Canonical tool-call assembly for one OpenAI choice (spec § 4.3, D9).
//!
//! The client SDKs disagree on everything outside this form. openai-python concatenates every
//! string except `index` and `type`, so a second `name` fragment turns `safe_` into `safe_rm`.
//! openai-node replaces the name, and LangChain merges by index value. Inside the canonical form
//! they all agree, so the decider sees exactly what every client will run.

use serde_json::Value;

use crate::openai::classify::{Fragment, NameArgs};
use crate::protocol::Deviation;
use crate::types::{ToolCall, ToolCallKind};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    Function,
    Custom,
}

#[derive(Default)]
struct CallBuild {
    id: Option<String>,
    ty: Option<String>,
    shape: Option<Shape>,
    name: Option<String>,
    args: String,
    raws: Vec<Value>,
}

#[derive(Default)]
pub(crate) struct ChoiceAssembly {
    calls: Vec<CallBuild>,
    legacy: Option<CallBuild>,
}

fn set_once(slot: &mut Option<String>, v: &Option<String>, why: &'static str) -> Result<(), Deviation> {
    if let Some(v) = v {
        if slot.is_some() {
            return Err(Deviation(why));
        }
        *slot = Some(v.clone());
    }
    Ok(())
}

impl ChoiceAssembly {
    pub(crate) fn apply(&mut self, frags: &[Fragment], legacy: Option<&NameArgs>) -> Result<(), Deviation> {
        let mixes = (legacy.is_some() && (!frags.is_empty() || !self.calls.is_empty())) || (!frags.is_empty() && self.legacy.is_some());
        if mixes {
            return Err(Deviation("legacy_and_tool_calls"));
        }
        if let Some(l) = legacy {
            let b = self.legacy.get_or_insert_with(CallBuild::default);
            set_once(&mut b.name, &l.name, "repeated_name")?;
            if let Some(a) = &l.args {
                b.args.push_str(a);
            }
        }
        for f in frags {
            let pos = f.index as usize;
            if pos == self.calls.len() {
                self.calls.push(CallBuild::default());
            } else if pos > self.calls.len() {
                return Err(Deviation("index_gap"));
            }
            let b = &mut self.calls[pos];
            set_once(&mut b.id, &f.id, "repeated_id")?;
            set_once(&mut b.ty, &f.ty, "repeated_type")?;
            let (shape, na) = match (&f.function, &f.custom) {
                (Some(na), None) => (Some(Shape::Function), Some(na)),
                (None, Some(na)) => (Some(Shape::Custom), Some(na)),
                _ => (None, None),
            };
            if let Some(s) = shape {
                match b.shape {
                    None => b.shape = Some(s),
                    Some(prev) if prev != s => return Err(Deviation("shape_changed")),
                    Some(_) => {}
                }
            }
            if let Some(na) = na {
                set_once(&mut b.name, &na.name, "repeated_name")?;
                if let Some(a) = &na.args {
                    b.args.push_str(a);
                }
            }
            b.raws.push(f.raw.clone());
        }
        Ok(())
    }

    pub(crate) fn finish(&self, choice: u32) -> Result<Vec<ToolCall>, Deviation> {
        if let Some(b) = &self.legacy {
            let name = b.name.clone().ok_or(Deviation("missing_name"))?;
            return Ok(vec![ToolCall { choice, position: 0, id: None, name, kind: ToolCallKind::LegacyFunctionCall, arguments: b.args.clone() }]);
        }
        let mut out = Vec::with_capacity(self.calls.len());
        for (i, b) in self.calls.iter().enumerate() {
            let kind = match (b.ty.as_deref(), b.shape) {
                (Some("function") | None, Some(Shape::Function)) => ToolCallKind::Function,
                (Some("custom") | None, Some(Shape::Custom)) => ToolCallKind::Custom,
                (Some("function"), Some(Shape::Custom)) | (Some("custom"), Some(Shape::Function)) => return Err(Deviation("type_shape_mismatch")),
                (Some("function" | "custom"), None) => return Err(Deviation("missing_body")),
                _ => ToolCallKind::Other,
            };
            let (name, arguments) = if kind == ToolCallKind::Other {
                let name = b.name.clone().or_else(|| b.ty.clone()).unwrap_or_else(|| "unknown".to_owned());
                (name, serde_json::to_string(&b.raws).expect("a Value array always serializes"))
            } else {
                (b.name.clone().ok_or(Deviation("missing_name"))?, b.args.clone())
            };
            let position = u32::try_from(i).expect("at most 128 calls per choice");
            out.push(ToolCall { choice, position, id: b.id.clone(), name, kind, arguments });
        }
        Ok(out)
    }

    pub(crate) fn names(&self) -> Vec<(u32, String)> {
        let name = |b: &CallBuild| b.name.clone().unwrap_or_else(|| "unknown".to_owned());
        if let Some(b) = &self.legacy {
            return vec![(0, name(b))];
        }
        self.calls.iter().enumerate().map(|(i, b)| (u32::try_from(i).expect("at most 128 calls per choice"), name(b))).collect()
    }
}
```

`raws` keeps a copy of every fragment so that kind Other can report it. That doubles the memory of a large call, and the doubled amount still counts against `max_held_bytes`. Do not optimize it in this task.

- [ ] **Step 7: Run the tests**

```bash
cd rs && cargo fmt && cargo nextest run --locked -p paigasus-gateway-core openai::
```

Expected: PASS (15 tests).

- [ ] **Step 8: Commit**

```bash
git add rs/crates/libs/paigasus-gateway-core
git commit -m "feat(rs): classify OpenAI chunks and assemble tool calls canonically (SMA-749)" -m "A positive allowlist splits chunks into tool-free, tool-bearing and
malformed. Assembly enforces the canonical form the client SDKs agree
on, and reports any deviation.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: The protocol trait and the OpenAI streaming protocol

**Files:**
- Modify: `rs/crates/libs/paigasus-gateway-core/src/protocol.rs`, `src/openai/mod.rs`
- Create: `src/openai/stream.rs`

**Interfaces:**
- Consumes: Tasks 1–5.
- Produces (`pub(crate)`, in `protocol`):
  - `enum Verdict { AllowAll, PerCall(Vec<Decision>), BlockAll(BlockCode), PassThrough }`, with:
    - `fn blocks_any(&self) -> bool`
    - `fn block_code(&self, slot: usize) -> Option<BlockCode>`. `slot` is the call's index within its hold. A `PerCall` vector that is too short blocks the missing slot with `DeciderError` (D18).
  - `type Verdicts = BTreeMap<HoldKey, Verdict>`
  - `struct Frame { event: Option<&'static str>, payload: Vec<u8>, value: Option<Value>, touches: Vec<HoldKey> }`
  - `enum Step { Frame { frame: Frame, opened: Vec<HoldKey>, completed: Vec<(HoldKey, Vec<ToolCall>)>, reports: Vec<ReportEntry> }, End(Frame), UpstreamError(Frame) }`
  - `enum StepError { Malformed(&'static str), Deviation(&'static str) }`, with `From<Malformed>` and `From<Deviation>`.
  - `trait Protocol: Send` with:
    - `fn step(&mut self, rec: Record, verdicts: &Verdicts) -> Result<Step, StepError>`
    - `fn incomplete(&self) -> Vec<HoldKey>`
    - `fn release(&mut self, frame: Frame, verdicts: &Verdicts) -> Vec<Bytes>`
    - `fn closing(&mut self, verdicts: &Verdicts) -> Vec<Bytes>`
    - `fn known_calls(&self, hold: HoldKey) -> Vec<(u32, String)>`
    - `fn unframed(&self, rec: Record) -> Bytes`
- Produces: `openai::stream::OpenAiStream` (`Default`), which implements `Protocol`.

Semantics of `OpenAiStream`:
- **`step`.**
  - `[DONE]` gives `End`, with no value and no touches.
  - Otherwise the record is strict-parsed and classified. The meta keys (`id`, `object`, `created`, `model`, `system_fingerprint`) are remembered from every chunk.
  - Per choice:
    - A choice index seen for the first time must equal the number of choices seen so far (`Deviation("choice_order")`, P-10).
    - Text marks `has_text`.
    - A tool fragment after the choice's `finish_reason` is `Deviation("late_fragment")`.
    - The first tool fragment opens hold `Choice(c)`.
    - A frame touches the hold when it carries a tool fragment of that choice, or the choice's `finish_reason` while its hold is open.
    - Fragments are assembled while the hold has no verdict, or has `PassThrough`. In the `PassThrough` case errors are ignored.
    - `finish_reason` on an open hold completes it, unless the hold already has a blocking verdict.
- **`release`.**
  - `[DONE]` emits `closing()` first, then `data: [DONE]`.
  - A frame with no blocking verdict on any touched hold is re-framed with its original payload bytes (D23).
  - Otherwise the frame is rewritten per § 4.4:
    - Blocked entries are removed, the rest are renumbered, `function_call` is removed, and `logprobs` is stripped.
    - `finish_reason` `tool_calls` or `function_call` becomes `stop` when no call of the choice remains.
    - A choice entry is never removed and a frame is never dropped, even when its `delta` becomes `{}` (P-10).
    - A notice chunk (the meta of this frame, plus `{"index":c,"delta":{"content":…},"finish_reason":null}`) is emitted right before the first frame that carries the blocked choice's `finish_reason`. The notice lines are joined with `\n`, with a `\n\n` prefix when the choice already streamed text.
- **`closing`** emits the notice for every blocked choice that has not had one yet.
- **`unframed`** re-frames the payload with no event line.

- [ ] **Step 1: Write the protocol types**

Append to `src/protocol.rs`:

```rust
use std::collections::BTreeMap;

use bytes::Bytes;
use serde_json::Value;

use crate::json::Malformed;
use crate::report::ReportEntry;
use crate::sse::Record;
use crate::types::{BlockCode, Decision, HoldKey, ToolCall};

/// What a hold's calls are allowed to do once released.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// Every call allowed: frames are released unchanged.
    AllowAll,
    /// One decision per call, in the hold's call order.
    PerCall(Vec<Decision>),
    /// Every call of the hold blocked, including calls not assembled yet.
    BlockAll(BlockCode),
    /// Observe mode: released unchanged and not enforced (§ 5.3).
    PassThrough,
}

impl Verdict {
    pub(crate) fn blocks_any(&self) -> bool {
        match self {
            Verdict::AllowAll | Verdict::PassThrough => false,
            Verdict::BlockAll(_) => true,
            Verdict::PerCall(d) => d.iter().any(|d| !d.is_allow()),
        }
    }

    /// The block code of the call in `slot` (its index within the hold), or `None` when allowed.
    pub(crate) fn block_code(&self, slot: usize) -> Option<BlockCode> {
        match self {
            Verdict::AllowAll | Verdict::PassThrough => None,
            Verdict::BlockAll(c) => Some(*c),
            Verdict::PerCall(d) => match d.get(slot) {
                Some(Decision::Block { code }) => Some(*code),
                Some(_) => None,
                None => Some(BlockCode::DeciderError), // D18: a missing decision is a block
            },
        }
    }
}

pub(crate) type Verdicts = BTreeMap<HoldKey, Verdict>;

/// One classified record, waiting in the queue or about to be released.
pub(crate) struct Frame {
    /// The validated Anthropic event name; `None` for OpenAI (P-1).
    pub event: Option<&'static str>,
    /// The original payload bytes, re-framed unchanged when nothing in it is blocked (D23).
    pub payload: Vec<u8>,
    /// The parsed payload; `None` only for OpenAI's `[DONE]`.
    pub value: Option<Value>,
    /// The holds this frame carries data of (D12). Sorted and without duplicates.
    pub touches: Vec<HoldKey>,
}

pub(crate) enum Step {
    Frame { frame: Frame, opened: Vec<HoldKey>, completed: Vec<(HoldKey, Vec<ToolCall>)>, reports: Vec<ReportEntry> },
    /// The end marker; every later record is dropped (D4).
    End(Frame),
    /// An upstream error record (D17).
    UpstreamError(Frame),
}

pub(crate) enum StepError {
    Malformed(&'static str),
    Deviation(&'static str),
}

impl From<Malformed> for StepError {
    fn from(m: Malformed) -> Self {
        StepError::Malformed(m.0)
    }
}

impl From<Deviation> for StepError {
    fn from(d: Deviation) -> Self {
        StepError::Deviation(d.0)
    }
}

/// One wire protocol. The interceptor owns ordering, limits and failure handling; a protocol owns
/// classification, assembly and rewriting.
pub(crate) trait Protocol: Send {
    fn step(&mut self, rec: Record, verdicts: &Verdicts) -> Result<Step, StepError>;
    /// Holds that opened and did not complete. At the end of the stream these are truncated.
    fn incomplete(&self) -> Vec<HoldKey>;
    /// The bytes for one released frame under the verdicts. May be empty (a dropped frame) or
    /// several frames (a notice first).
    fn release(&mut self, frame: Frame, verdicts: &Verdicts) -> Vec<Bytes>;
    /// Notices still owed at the end of a stream that has no end marker.
    fn closing(&mut self, verdicts: &Verdicts) -> Vec<Bytes>;
    /// (position, name) of the calls known so far for a hold, for reports of a `BlockAll` hold.
    fn known_calls(&self, hold: HoldKey) -> Vec<(u32, String)>;
    /// Pass-through framing, with no classification (P-4).
    fn unframed(&self, rec: Record) -> Bytes;
}
```

In `src/openai/mod.rs` add `pub(crate) mod stream;`.

- [ ] **Step 2: Write the failing tests** (in `src/openai/stream.rs`)

The tests drive `OpenAiStream` directly, through a small helper that plays the interceptor's part with one fixed verdict per hold.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{Protocol, Step, Verdict, Verdicts};
    use crate::sse::Record;
    use crate::types::{BlockCode, Decision, HoldKey};
    use serde_json::{Value, json};

    fn rec(s: &str) -> Record {
        Record { event: None, data: s.as_bytes().to_vec() }
    }
    fn chunk(choices: Value) -> String {
        json!({"id":"chatcmpl-1","object":"chat.completion.chunk","created":1,"model":"m","system_fingerprint":"fp","choices":choices}).to_string()
    }

    /// Steps every record, then releases every frame under `verdict` for each completed hold.
    /// Returns the concatenated output and the completed calls.
    fn run(records: &[String], verdict: impl Fn(&[crate::types::ToolCall]) -> Verdict) -> (String, Vec<crate::types::ToolCall>) {
        let mut p = OpenAiStream::default();
        let mut verdicts = Verdicts::new();
        let mut frames = Vec::new();
        let mut calls = Vec::new();
        for r in records {
            match p.step(rec(r), &verdicts).ok().expect("canonical record") {
                Step::Frame { frame, completed, .. } => {
                    for (k, c) in completed {
                        verdicts.insert(k, verdict(&c));
                        calls.extend(c);
                    }
                    frames.push(frame);
                }
                Step::End(f) | Step::UpstreamError(f) => frames.push(f),
            }
        }
        let mut out = Vec::new();
        for f in frames {
            for b in p.release(f, &verdicts) {
                out.extend_from_slice(&b);
            }
        }
        for b in p.closing(&verdicts) {
            out.extend_from_slice(&b);
        }
        (String::from_utf8(out).unwrap(), calls)
    }

    fn two_calls() -> Vec<String> {
        vec![
            chunk(json!([{"index":0,"delta":{"role":"assistant","content":"Sure."},"finish_reason":null}])),
            chunk(json!([{"index":0,"delta":{"tool_calls":[{"index":0,"id":"c0","type":"function","function":{"name":"read","arguments":"{}"}}]},"finish_reason":null}])),
            chunk(json!([{"index":0,"delta":{"tool_calls":[{"index":1,"id":"c1","type":"function","function":{"name":"rm","arguments":"{\"p\":\"/\"}"}}]},"logprobs":{"content":[{"token":"rm"}]},"finish_reason":null}])),
            chunk(json!([{"index":0,"delta":{},"finish_reason":"tool_calls"}])),
            "[DONE]".to_owned(),
        ]
    }

    #[test]
    fn allow_all_reemits_every_payload_unchanged() {
        let records = two_calls();
        let (out, calls) = run(&records, |c| Verdict::PerCall(vec![Decision::Allow; c.len()]));
        let want: String = records.iter().map(|r| format!("data: {r}\n\n")).collect();
        assert_eq!(out, want);
        assert_eq!(calls.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["read", "rm"]);
    }

    #[test]
    fn a_blocked_second_call_is_removed_and_its_logprobs_stripped() {
        let (out, _) = run(&two_calls(), |_| Verdict::PerCall(vec![Decision::Allow, Decision::Block { code: BlockCode::Policy }]));
        assert!(!out.contains("\\\"p\\\""), "the blocked arguments leaked: {out}");
        assert!(!out.contains("\"token\":\"rm\""), "the logprobs leaked: {out}");
        assert!(out.contains("\"name\":\"read\""));
        assert!(out.contains(r#"[paigasus] tool call \"rm\" was blocked (policy)."#));
        assert!(out.contains("\"finish_reason\":\"tool_calls\""), "one call remains, so finish_reason stays");
        let notice_at = out.find("[paigasus]").unwrap();
        let finish_at = out.find("\"finish_reason\":\"tool_calls\"").unwrap();
        assert!(notice_at < finish_at, "the notice precedes the finish frame");
        assert!(out.ends_with("data: [DONE]\n\n"));
    }

    #[test]
    fn blocking_every_call_rewrites_finish_reason_to_stop_and_keeps_role() {
        let records = vec![
            chunk(json!([{"index":0,"delta":{"role":"assistant","content":null,"tool_calls":[{"index":0,"id":"c0","type":"function","function":{"name":"rm","arguments":"x"}}]},"finish_reason":null}])),
            chunk(json!([{"index":0,"delta":{},"finish_reason":"tool_calls"}])),
        ];
        let (out, _) = run(&records, |_| Verdict::BlockAll(BlockCode::Policy));
        assert!(out.contains("\"role\":\"assistant\""), "role is kept: {out}");
        assert!(!out.contains("tool_calls\":["), "no tool_calls remain: {out}");
        assert!(out.contains("\"finish_reason\":\"stop\""));
        let notice: Value = serde_json::from_str(out.lines().find(|l| l.contains("[paigasus]")).unwrap().strip_prefix("data: ").unwrap()).unwrap();
        assert_eq!(notice["id"], "chatcmpl-1");
        assert_eq!(notice["system_fingerprint"], "fp");
        assert_eq!(notice["choices"][0]["index"], 0);
        assert!(!notice["choices"][0]["delta"]["content"].as_str().unwrap().starts_with('\n'), "no text streamed before, so no prefix");
    }

    #[test]
    fn a_legacy_call_is_blocked_and_finish_reason_function_call_becomes_stop() {
        let records = vec![
            chunk(json!([{"index":0,"delta":{"role":"assistant","content":"x","function_call":{"name":"f","arguments":"SECRET"}},"finish_reason":null}])),
            chunk(json!([{"index":0,"delta":{},"finish_reason":"function_call"}])),
        ];
        let (out, _) = run(&records, |_| Verdict::BlockAll(BlockCode::Policy));
        assert!(!out.contains("SECRET"));
        assert!(out.contains("\"finish_reason\":\"stop\""));
        assert!(out.contains("\\n\\n[paigasus]"), "text streamed first, so the notice is prefixed");
    }

    #[test]
    fn closing_owes_a_notice_for_a_truncated_blocked_choice() {
        let records = vec![chunk(json!([{"index":0,"delta":{"tool_calls":[{"index":0,"id":"c0","type":"function","function":{"name":"rm","arguments":"SEC"}}]}}]))];
        let mut p = OpenAiStream::default();
        let Step::Frame { frame, opened, .. } = p.step(rec(&records[0]), &Verdicts::new()).ok().unwrap() else { panic!() };
        assert_eq!(opened, vec![HoldKey::Choice(0)]);
        assert_eq!(p.incomplete(), vec![HoldKey::Choice(0)]);
        let mut verdicts = Verdicts::new();
        verdicts.insert(HoldKey::Choice(0), Verdict::BlockAll(BlockCode::Truncated));
        let out = String::from_utf8(p.release(frame, &verdicts).concat()).unwrap();
        assert!(!out.contains("SEC") && !out.contains("tool_calls"), "the blocked call is gone: {out}");
        assert!(out.contains("\"delta\":{}"), "the choice entry is kept, emptied (P-10): {out}");
        let closing = String::from_utf8(p.closing(&verdicts).concat()).unwrap();
        assert!(closing.contains("(truncated)"));
        assert!(p.closing(&verdicts).is_empty(), "the notice is owed once");
    }

    #[test]
    fn a_fragment_after_finish_reason_is_a_deviation() {
        let mut p = OpenAiStream::default();
        let v = Verdicts::new();
        p.step(rec(&chunk(json!([{"index":0,"delta":{"tool_calls":[{"index":0,"id":"c","type":"function","function":{"name":"x"}}]}}]))), &v).ok().unwrap();
        p.step(rec(&chunk(json!([{"index":0,"delta":{},"finish_reason":"tool_calls"}]))), &v).ok().unwrap();
        let late = p.step(rec(&chunk(json!([{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"x"}}]}}]))), &v);
        assert!(matches!(late, Err(StepError::Deviation("late_fragment"))));
    }

    #[test]
    fn two_choices_in_one_frame_touch_both_holds() {
        let mut p = OpenAiStream::default();
        let v = Verdicts::new();
        let both = chunk(json!([
            {"index":0,"delta":{"tool_calls":[{"index":0,"id":"a","type":"function","function":{"name":"x"}}]}},
            {"index":1,"delta":{"tool_calls":[{"index":0,"id":"b","type":"function","function":{"name":"y"}}]}}
        ]));
        let Step::Frame { frame, .. } = p.step(rec(&both), &v).ok().unwrap() else { panic!() };
        assert_eq!(frame.touches, vec![HoldKey::Choice(0), HoldKey::Choice(1)]);
    }

    #[test]
    fn choices_must_first_appear_in_order() {
        let mut p = OpenAiStream::default();
        let v = Verdicts::new();
        let first_is_one = p.step(rec(&chunk(json!([{"index":1,"delta":{"content":"x"}}]))), &v);
        assert!(matches!(first_is_one, Err(StepError::Deviation("choice_order"))));
        let mut q = OpenAiStream::default();
        assert!(q.step(rec(&chunk(json!([{"index":0,"delta":{"content":"a"}},{"index":1,"delta":{"content":"b"}}]))), &v).is_ok());
        assert!(q.step(rec(&chunk(json!([{"index":1,"delta":{"content":"c"}}]))), &v).is_ok(), "a seen choice may come in any order later");
    }

    #[test]
    fn a_text_frame_of_a_holding_choice_touches_nothing() {
        let mut p = OpenAiStream::default();
        let v = Verdicts::new();
        p.step(rec(&chunk(json!([{"index":0,"delta":{"tool_calls":[{"index":0,"id":"a","type":"function","function":{"name":"x"}}]}}]))), &v).ok().unwrap();
        let Step::Frame { frame, .. } = p.step(rec(&chunk(json!([{"index":0,"delta":{"content":"later"}}]))), &v).ok().unwrap() else { panic!() };
        assert!(frame.touches.is_empty());
    }
}
```

`StepError` has no `Debug`, so the tests use `.ok().expect(…)` and `matches!` rather than `unwrap()`. Do not add `Debug` just for the tests.

- [ ] **Step 3: Run them to verify they fail**

```bash
cd rs && cargo nextest run --locked -p paigasus-gateway-core openai::stream::
```

Expected: FAIL (`OpenAiStream` does not exist).

- [ ] **Step 4: Implement**

`src/openai/stream.rs` (above the tests):

```rust
// SPDX-License-Identifier: Apache-2.0
//! The OpenAI Chat Completions streaming protocol (spec § 4.2–§ 4.4, D10, D12, D20).

use std::collections::BTreeMap;

use bytes::Bytes;
use serde_json::{Map, Value, json};

use crate::json::parse_strict;
use crate::openai::assembly::ChoiceAssembly;
use crate::openai::classify::{self, Chunk};
use crate::protocol::{Frame, Protocol, Step, StepError, Verdict, Verdicts};
use crate::report::notice_line;
use crate::sse::{self, Record};
use crate::types::{HoldKey, ToolCall};

const META_KEYS: [&str; 5] = ["id", "object", "created", "model", "system_fingerprint"];

#[derive(Default)]
struct ChoiceState {
    asm: ChoiceAssembly,
    hold_open: bool,
    finished: bool,
    has_text: bool,
    notice_done: bool,
    /// The calls handed to the decider, in hold order.
    calls: Vec<ToolCall>,
}

#[derive(Default)]
pub(crate) struct OpenAiStream {
    choices: BTreeMap<u32, ChoiceState>,
    meta: Map<String, Value>,
}

fn meta_of(v: &Value) -> Map<String, Value> {
    let mut m = Map::new();
    if let Value::Object(top) = v {
        for k in META_KEYS {
            if let Some(x) = top.get(k) {
                m.insert(k.to_owned(), x.clone());
            }
        }
    }
    m
}

fn notice_chunk(meta: &Map<String, Value>, choice: u32, text: &str) -> Bytes {
    let mut m = meta.clone();
    m.insert("choices".to_owned(), json!([{"index": choice, "delta": {"content": text}, "finish_reason": null}]));
    sse::frame(None, &serde_json::to_vec(&Value::Object(m)).expect("a Value always serializes"))
}

impl ChoiceState {
    /// The (position, name) of every call the hold knows.
    fn known(&self) -> Vec<(u32, String)> {
        if self.calls.is_empty() { self.asm.names() } else { self.calls.iter().map(|c| (c.position, c.name.clone())).collect() }
    }

    fn notice_text(&self, verdict: &Verdict) -> String {
        let lines: Vec<String> = self.known().iter().enumerate().filter_map(|(slot, (_, name))| verdict.block_code(slot).map(|code| notice_line(name, code))).collect();
        let body = if lines.is_empty() { notice_line("unknown", verdict.block_code(0).unwrap_or(crate::types::BlockCode::Malformed)) } else { lines.join("\n") };
        if self.has_text { format!("\n\n{body}") } else { body }
    }

    fn allowed_left(&self, verdict: &Verdict) -> usize {
        (0..self.known().len()).filter(|slot| verdict.block_code(*slot).is_none()).count()
    }
}

impl Protocol for OpenAiStream {
    fn step(&mut self, rec: Record, verdicts: &Verdicts) -> Result<Step, StepError> {
        if rec.data == b"[DONE]" {
            return Ok(Step::End(Frame { event: None, payload: rec.data, value: None, touches: Vec::new() }));
        }
        let value = parse_strict(&rec.data)?;
        let choices = match classify::chunk(&value)? {
            Chunk::UpstreamError => return Ok(Step::UpstreamError(Frame { event: None, payload: rec.data, value: Some(value), touches: Vec::new() })),
            Chunk::Choices(c) => c,
        };
        for (k, v) in meta_of(&value) {
            self.meta.insert(k, v);
        }
        let (mut touches, mut opened, mut completed) = (Vec::new(), Vec::new(), Vec::new());
        for cv in choices {
            let key = HoldKey::Choice(cv.index);
            if !self.choices.contains_key(&cv.index) && cv.index as usize != self.choices.len() {
                return Err(StepError::Deviation("choice_order"));
            }
            let st = self.choices.entry(cv.index).or_default();
            if cv.has_text {
                st.has_text = true;
            }
            if !cv.fragments.is_empty() || cv.legacy.is_some() {
                if st.finished {
                    return Err(StepError::Deviation("late_fragment"));
                }
                if !st.hold_open {
                    st.hold_open = true;
                    opened.push(key);
                }
                touches.push(key);
                match verdicts.get(&key) {
                    None => st.asm.apply(&cv.fragments, cv.legacy.as_ref())?,
                    Some(Verdict::PassThrough) => {
                        let _ = st.asm.apply(&cv.fragments, cv.legacy.as_ref());
                    }
                    Some(_) => {}
                }
            }
            if cv.finish_reason.is_some() {
                if st.finished {
                    return Err(StepError::Deviation("second_finish"));
                }
                st.finished = true;
                if st.hold_open {
                    touches.push(key);
                    let finished = match verdicts.get(&key) {
                        None => Some(st.asm.finish(cv.index)?),
                        Some(Verdict::PassThrough) => st.asm.finish(cv.index).ok(),
                        Some(_) => None,
                    };
                    if let Some(calls) = finished {
                        st.calls = calls.clone();
                        completed.push((key, calls));
                    }
                }
            }
        }
        touches.sort();
        touches.dedup();
        Ok(Step::Frame { frame: Frame { event: None, payload: rec.data, value: Some(value), touches }, opened, completed, reports: Vec::new() })
    }

    fn incomplete(&self) -> Vec<HoldKey> {
        self.choices.iter().filter(|(_, s)| s.hold_open && !s.finished).map(|(c, _)| HoldKey::Choice(*c)).collect()
    }

    fn release(&mut self, frame: Frame, verdicts: &Verdicts) -> Vec<Bytes> {
        let Some(mut value) = frame.value else {
            let mut out = self.closing(verdicts);
            out.push(sse::frame(None, &frame.payload));
            return out;
        };
        let blocking: Vec<u32> = frame
            .touches
            .iter()
            .filter_map(|k| match (k, verdicts.get(k)) {
                (HoldKey::Choice(c), Some(v)) if v.blocks_any() => Some(*c),
                _ => None,
            })
            .collect();
        if blocking.is_empty() {
            return vec![sse::frame(None, &frame.payload)];
        }
        let meta = meta_of(&value);
        let mut notices = Vec::new();
        if let Some(Value::Array(choices)) = value.get_mut("choices") {
            for ch in choices.iter_mut() {
                let idx = ch.get("index").and_then(Value::as_u64).and_then(|i| u32::try_from(i).ok()).unwrap_or(0);
                if let Some(v) = verdicts.get(&HoldKey::Choice(idx)).filter(|_| blocking.contains(&idx)) {
                    let st = self.choices.entry(idx).or_default();
                    if let Some(text) = rewrite_choice(ch, v, st) {
                        notices.push(notice_chunk(&meta, idx, &text));
                    }
                }
            }
        }
        // P-10: never drop the frame or a choice entry. openai-python finds a choice by list
        // position, so a missing first appearance would shift every later choice.
        notices.push(sse::frame(None, &serde_json::to_vec(&value).expect("a Value always serializes")));
        notices
    }

    fn closing(&mut self, verdicts: &Verdicts) -> Vec<Bytes> {
        let mut out = Vec::new();
        for (idx, st) in &mut self.choices {
            let Some(v) = verdicts.get(&HoldKey::Choice(*idx)) else { continue };
            if st.hold_open && v.blocks_any() && !st.notice_done {
                st.notice_done = true;
                out.push(notice_chunk(&self.meta, *idx, &st.notice_text(v)));
            }
        }
        out
    }

    fn known_calls(&self, hold: HoldKey) -> Vec<(u32, String)> {
        match hold {
            HoldKey::Choice(c) => self.choices.get(&c).map(ChoiceState::known).unwrap_or_default(),
            HoldKey::Block(_) => Vec::new(),
        }
    }

    fn unframed(&self, rec: Record) -> Bytes {
        sse::frame(None, &rec.data)
    }
}

/// Rewrite one choice entry in place (§ 4.4). Returns the notice text when this entry carries the
/// choice's `finish_reason` and the notice is still owed.
fn rewrite_choice(ch: &mut Value, verdict: &Verdict, st: &mut ChoiceState) -> Option<String> {
    let slots: Vec<u32> = st.known().iter().map(|(p, _)| *p).collect();
    let slot_of = |pos: u32| slots.iter().position(|p| *p == pos).unwrap_or(slots.len());
    let rank = |pos: u32| -> u32 { u32::try_from((0..slot_of(pos)).filter(|s| verdict.block_code(*s).is_none()).count()).expect("at most 128 calls per choice") };
    if let Some(Value::Object(delta)) = ch.get_mut("delta") {
        if let Some(Value::Array(tc)) = delta.get_mut("tool_calls") {
            tc.retain_mut(|e| {
                let pos = e.get("index").and_then(Value::as_u64).and_then(|i| u32::try_from(i).ok()).unwrap_or(0);
                if verdict.block_code(slot_of(pos)).is_some() {
                    return false;
                }
                e["index"] = rank(pos).into();
                true
            });
            if tc.is_empty() {
                delta.remove("tool_calls");
            }
        }
        if delta.contains_key("function_call") && verdict.block_code(0).is_some() {
            delta.remove("function_call");
        }
    }
    if let Value::Object(o) = ch {
        o.remove("logprobs");
    }
    let finish = ch.get("finish_reason").and_then(Value::as_str).map(str::to_owned)?;
    if st.allowed_left(verdict) == 0 && (finish == "tool_calls" || finish == "function_call") {
        ch["finish_reason"] = "stop".into();
    }
    if st.notice_done {
        return None;
    }
    st.notice_done = true;
    Some(st.notice_text(verdict))
}
```

- [ ] **Step 5: Run the tests**

```bash
cd rs && cargo fmt && cargo nextest run --locked -p paigasus-gateway-core openai::
```

Expected: PASS (24 tests in `openai::`). If `a_blocked_second_call_is_removed_and_its_logprobs_stripped` fails on the order assertion, check that `release` pushes the notice chunk BEFORE the rewritten frame, in the same `Vec`.

- [ ] **Step 6: Commit**

```bash
git add rs/crates/libs/paigasus-gateway-core
git commit -m "feat(rs): add the OpenAI streaming protocol with the block rewrite (SMA-749)" -m "Holds per choice, touch sets per frame, and the per-fragment rewrite:
blocked entries removed and renumbered, logprobs stripped, role kept,
finish_reason fixed, and one notice chunk before the finish frame.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---
### Task 7: The Anthropic streaming protocol

**Files:**
- Create: `rs/crates/libs/paigasus-gateway-core/src/anthropic/mod.rs`, `src/anthropic/stream.rs`
- Modify: `src/lib.rs`

**Interfaces:**
- Consumes: Tasks 1–6 (`Protocol`, `Frame`, `Step`, `Verdict`, `ReportEntry`, `parse_strict`, `index_of`, `MAX_ANTHROPIC_INDEX`, `opt_str`).
- Produces: `anthropic::stream::AnthropicStream` (`Default`), which implements `Protocol`. Hold keys are `HoldKey::Block(index)`. Each hold has exactly one call, in slot 0.

Semantics (§ 4.2–§ 4.4, D11, P-8):
- **Dispatch.** Dispatch is on `data.type`, which must be one of `message_start`, `message_delta`, `message_stop`, `ping`, `error`, `content_block_start`, `content_block_delta`, `content_block_stop`. An unknown type is malformed. An `event:` line that differs from `data.type` is malformed. Output always carries `event: <type>`.
- **`message_start`.** `message.content` must be absent or `[]` (`Deviation("prefilled_content")`).
- **`content_block_start`.**
  - `index` must equal the number of earlier starts (`Deviation("block_index")`).
  - The block type sets the block's kind:
    - `text` is Text, and `thinking` or `redacted_thinking` is Thinking. Both are tool-free.
    - `server_tool_use`, `mcp_tool_use`, and any type ending in `_tool_result` are Server. They are tool-free and produce one `Unenforceable` report.
    - `tool_use` is ToolUse. Its `input` must be `{}` (`Deviation("prefilled_input")`), and `name` must be a string. It opens a hold.
    - Any other type is Other. It opens a hold, and its arguments are the JSON array of the raw block and its deltas.
- **`content_block_delta`.** The block must be open (`Deviation("unknown_block")`). The allowed delta types per kind are:

  | Block kind | Allowed delta types |
  |---|---|
  | ToolUse | `input_json_delta` with a string `partial_json` |
  | Other | any (kept raw) |
  | Text | `text_delta`, `citations_delta` |
  | Thinking | `thinking_delta`, `signature_delta` |
  | Server | `input_json_delta` |

  Any other delta type is malformed for Text, Thinking and Server, and a deviation for ToolUse.
- **`content_block_stop`.** The block must be open. A ToolUse or Other block completes its hold, unless the hold already has a blocking verdict.
- **`message_delta`.** Touches every ToolUse and Other hold, so that its `stop_reason` is released after every decision.
- **`message_stop`.** Ends the stream (`End`).
- **`error`.** An upstream error.
- **`release` of a blocked block.**
  - Its `content_block_start` becomes `{"type":"content_block_start","index":i,"content_block":{"type":"text","text":""}}`.
  - Its deltas are dropped.
  - Its `content_block_stop` is preceded by one `text_delta` with the notice.
  - `message_delta.delta.stop_reason` `tool_use` becomes `end_turn` when no ToolUse or Other block is allowed.
  - `message_stop` emits `closing()` first.
- **`closing`.** For a blocked block that never stopped, it emits the notice `text_delta`, then `content_block_stop`.

- [ ] **Step 1: Write the module skeleton**

`src/anthropic/mod.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0
//! Anthropic Messages (spec § 4.2–§ 4.4). Library code until SMA-750 wires the ingress (D26).

pub(crate) mod stream;
```

`src/lib.rs`: add `mod anthropic;`. Put the `#![cfg_attr(not(test), allow(dead_code))] // Removed in Task 8 …` line in `stream.rs`.

- [ ] **Step 2: Write the failing tests** (in `src/anthropic/stream.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{Protocol, Step, Verdict, Verdicts};
    use crate::report::ReportDecision;
    use crate::sse::Record;
    use crate::types::{BlockCode, Decision, HoldKey, ToolCallKind};
    use serde_json::{Value, json};

    fn rec(v: Value) -> Record {
        let ty = v["type"].as_str().unwrap().to_owned();
        Record { event: Some(ty), data: v.to_string().into_bytes() }
    }

    fn tool_stream() -> Vec<Value> {
        vec![
            json!({"type":"message_start","message":{"id":"msg_1","type":"message","role":"assistant","content":[],"model":"claude","stop_reason":null}}),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Let me look."}}),
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_1","name":"rm","input":{}}}),
            json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"path\":"}}),
            json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"\"SECRET\"}"}}),
            json!({"type":"content_block_stop","index":1}),
            json!({"type":"message_delta","delta":{"stop_reason":"tool_use","stop_sequence":null},"usage":{"output_tokens":9}}),
            json!({"type":"message_stop"}),
        ]
    }

    fn run(events: Vec<Value>, verdict: Verdict) -> (String, Vec<crate::types::ToolCall>, Vec<ReportEntry>) {
        let mut p = AnthropicStream::default();
        let mut verdicts = Verdicts::new();
        let (mut frames, mut calls, mut reps) = (Vec::new(), Vec::new(), Vec::new());
        for e in events {
            match p.step(rec(e), &verdicts).ok().expect("canonical event") {
                Step::Frame { frame, completed, reports, .. } => {
                    for (k, c) in completed {
                        verdicts.insert(k, verdict.clone());
                        calls.extend(c);
                    }
                    reps.extend(reports);
                    frames.push(frame);
                }
                Step::End(f) | Step::UpstreamError(f) => frames.push(f),
            }
        }
        let mut out = Vec::new();
        for f in frames {
            out.extend(p.release(f, &verdicts).concat());
        }
        (String::from_utf8(out).unwrap(), calls, reps)
    }

    #[test]
    fn an_allowed_tool_use_is_reemitted_with_event_lines() {
        let events = tool_stream();
        let (out, calls, _) = run(events.clone(), Verdict::AllowAll);
        let want: String = events.iter().map(|e| format!("event: {}\ndata: {}\n\n", e["type"].as_str().unwrap(), e)).collect();
        assert_eq!(out, want);
        assert_eq!(calls.len(), 1);
        assert_eq!((calls[0].kind, calls[0].position, calls[0].name.as_str()), (ToolCallKind::AnthropicToolUse, 1, "rm"));
        assert_eq!(calls[0].arguments, "{\"path\":\"SECRET\"}");
        assert_eq!(calls[0].id.as_deref(), Some("toolu_1"));
    }

    #[test]
    fn a_blocked_tool_use_becomes_a_text_block_at_the_same_index() {
        let (out, _, _) = run(tool_stream(), Verdict::PerCall(vec![Decision::Block { code: BlockCode::Policy }]));
        assert!(!out.contains("SECRET"), "{out}");
        assert!(!out.contains("\"type\":\"tool_use\""), "{out}");
        assert!(out.contains(r#"{"content_block":{"text":"","type":"text"},"index":1,"type":"content_block_start"}"#) || out.contains(r#""index":1"#));
        assert!(out.contains(r#"[paigasus] tool call \"rm\" was blocked (policy)."#));
        assert!(out.contains("\"stop_reason\":\"end_turn\""));
        // The notice delta comes before the block's stop.
        let notice = out.find("[paigasus]").unwrap();
        let stop1 = out.find(r#"{"index":1,"type":"content_block_stop"}"#).or_else(|| out.find(r#""type":"content_block_stop","index":1"#)).unwrap();
        assert!(notice < stop1);
        for frame in out.split("\n\n").filter(|f| !f.is_empty()) {
            let mut lines = frame.lines();
            let ev = lines.next().unwrap().strip_prefix("event: ").expect("every frame has an event line");
            let data: Value = serde_json::from_str(lines.next().unwrap().strip_prefix("data: ").unwrap()).unwrap();
            assert_eq!(data["type"], ev);
        }
    }

    #[test]
    fn server_tools_pass_and_are_reported_unenforceable() {
        let events = vec![
            json!({"type":"message_start","message":{"content":[]}}),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"server_tool_use","id":"s1","name":"web_search","input":{}}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{}"}}),
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"content_block_start","index":1,"content_block":{"type":"web_search_tool_result","tool_use_id":"s1","content":[]}}),
            json!({"type":"content_block_stop","index":1}),
            json!({"type":"message_stop"}),
        ];
        let (out, calls, reps) = run(events, Verdict::BlockAll(BlockCode::Policy));
        assert!(calls.is_empty());
        assert_eq!(reps.len(), 2);
        assert!(reps.iter().all(|r| r.decision == ReportDecision::Unenforceable));
        assert!(out.contains("web_search"));
    }

    #[test]
    fn an_unknown_block_type_is_held_as_other() {
        let events = vec![
            json!({"type":"content_block_start","index":0,"content_block":{"type":"computer_use_v9","id":"x","name":"click"}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"coords_delta","x":1}}),
            json!({"type":"content_block_stop","index":0}),
        ];
        let (_, calls, _) = run(events, Verdict::AllowAll);
        assert_eq!((calls[0].kind, calls[0].name.as_str()), (ToolCallKind::Other, "click"));
        let raw: Value = serde_json::from_str(&calls[0].arguments).unwrap();
        assert_eq!(raw[1]["x"], 1);
    }

    #[test]
    fn every_non_canonical_event_is_rejected() {
        let mut p = AnthropicStream::default();
        let v = Verdicts::new();
        let bad = |p: &mut AnthropicStream, r: Record| p.step(r, &v).is_err();
        assert!(bad(&mut p, Record { event: Some("ping".into()), data: br#"{"type":"message_stop"}"#.to_vec() }), "event mismatch");
        assert!(bad(&mut AnthropicStream::default(), rec(json!({"type":"new_event"}))), "unknown type");
        assert!(bad(&mut AnthropicStream::default(), rec(json!({"type":"message_start","message":{"content":[{"type":"tool_use"}]}}))), "prefilled content");
        assert!(bad(&mut AnthropicStream::default(), rec(json!({"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}))), "index gap");
        assert!(bad(&mut AnthropicStream::default(), rec(json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"t","name":"rm","input":{"p":"/"}}}))), "prefilled input");
        assert!(bad(&mut AnthropicStream::default(), rec(json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"x"}}))), "delta for an unknown block");
        let mut q = AnthropicStream::default();
        q.step(rec(json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"t","name":"rm","input":{}}})), &v).ok().unwrap();
        assert!(bad(&mut q, rec(json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"x"}}))), "a text delta on a tool block");
    }

    #[test]
    fn a_block_without_a_stop_is_incomplete_and_closing_finishes_it() {
        let mut p = AnthropicStream::default();
        let v = Verdicts::new();
        let Step::Frame { opened, .. } = p.step(rec(json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"t","name":"rm","input":{}}})), &v).ok().unwrap() else { panic!() };
        assert_eq!(opened, vec![HoldKey::Block(0)]);
        assert_eq!(p.incomplete(), vec![HoldKey::Block(0)]);
        let mut verdicts = Verdicts::new();
        verdicts.insert(HoldKey::Block(0), Verdict::BlockAll(BlockCode::Truncated));
        let closing = String::from_utf8(p.closing(&verdicts).concat()).unwrap();
        assert!(closing.contains("(truncated)") && closing.contains("content_block_stop"), "{closing}");
    }
}
```

- [ ] **Step 3: Run them to verify they fail**

```bash
cd rs && cargo nextest run --locked -p paigasus-gateway-core anthropic::
```

Expected: FAIL (compile errors).

- [ ] **Step 4: Implement**

```rust
// SPDX-License-Identifier: Apache-2.0
//! The Anthropic Messages streaming protocol (spec § 4.2–§ 4.4, D11).
//!
//! The SDK selects records by the `event:` name and decodes them by `data.type`. The gateway
//! dispatches on `data.type`, rejects a disagreeing `event:`, and always writes the event line
//! itself, so every client reads the record the gateway classified.

use bytes::Bytes;
use serde_json::{Value, json};

use crate::json::{MAX_ANTHROPIC_INDEX, Malformed, index_of, opt_str, parse_strict};
use crate::protocol::{Deviation, Frame, Protocol, Step, StepError, Verdict, Verdicts};
use crate::report::{ReportDecision, ReportEntry, notice_line};
use crate::sse::{self, Record};
use crate::types::{HoldKey, ToolCall, ToolCallKind};

const EVENTS: [&str; 8] = ["message_start", "message_delta", "message_stop", "ping", "error", "content_block_start", "content_block_delta", "content_block_stop"];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Text,
    Thinking,
    Server,
    ToolUse,
    Other,
}

struct BlockState {
    kind: Kind,
    open: bool,
    id: Option<String>,
    name: String,
    args: String,
    raws: Vec<Value>,
    notice_done: bool,
}

impl BlockState {
    fn is_hold(&self) -> bool {
        matches!(self.kind, Kind::ToolUse | Kind::Other)
    }
}

#[derive(Default)]
pub(crate) struct AnthropicStream {
    blocks: Vec<BlockState>,
}

fn key(i: u32) -> HoldKey {
    HoldKey::Block(i)
}

fn block_index(v: &Value) -> Result<u32, Malformed> {
    index_of(v.get("index"), MAX_ANTHROPIC_INDEX)
}

fn plain(ev: &'static str, rec: Record, value: Value, touches: Vec<HoldKey>) -> Step {
    Step::Frame { frame: Frame { event: Some(ev), payload: rec.data, value: Some(value), touches }, opened: Vec::new(), completed: Vec::new(), reports: Vec::new() }
}

fn text_delta(i: u32, text: &str) -> Bytes {
    let v = json!({"type": "content_block_delta", "index": i, "delta": {"type": "text_delta", "text": text}});
    sse::frame(Some("content_block_delta"), v.to_string().as_bytes())
}

fn stop_frame(i: u32) -> Bytes {
    sse::frame(Some("content_block_stop"), json!({"type": "content_block_stop", "index": i}).to_string().as_bytes())
}

impl AnthropicStream {
    fn holds(&self) -> Vec<HoldKey> {
        self.blocks.iter().enumerate().filter(|(_, b)| b.is_hold()).map(|(i, _)| key(u32::try_from(i).expect("index is bounded"))).collect()
    }

    fn open_block(&mut self, i: u32) -> Result<&mut BlockState, StepError> {
        self.blocks.get_mut(i as usize).filter(|b| b.open).ok_or(StepError::Deviation("unknown_block"))
    }

    fn call_of(b: &BlockState, i: u32) -> ToolCall {
        let (kind, arguments) = match b.kind {
            Kind::ToolUse => (ToolCallKind::AnthropicToolUse, b.args.clone()),
            _ => (ToolCallKind::Other, serde_json::to_string(&b.raws).expect("a Value array always serializes")),
        };
        ToolCall { choice: 0, position: i, id: b.id.clone(), name: b.name.clone(), kind, arguments }
    }
}

impl Protocol for AnthropicStream {
    fn step(&mut self, rec: Record, verdicts: &Verdicts) -> Result<Step, StepError> {
        let value = parse_strict(&rec.data)?;
        let ty = value.get("type").and_then(Value::as_str).ok_or(Malformed("type"))?;
        let ev: &'static str = EVENTS.iter().copied().find(|e| *e == ty).ok_or(Malformed("unknown_event"))?;
        if rec.event.as_deref().is_some_and(|e| e != ev) {
            return Err(StepError::Malformed("event_mismatch"));
        }
        match ev {
            "ping" => Ok(plain(ev, rec, value, Vec::new())),
            "error" => Ok(Step::UpstreamError(Frame { event: Some(ev), payload: rec.data, value: Some(value), touches: Vec::new() })),
            "message_stop" => Ok(Step::End(Frame { event: Some(ev), payload: rec.data, value: Some(value), touches: Vec::new() })),
            "message_start" => {
                match value.pointer("/message/content") {
                    None => {}
                    Some(Value::Array(a)) if a.is_empty() => {}
                    Some(_) => return Err(Deviation("prefilled_content").into()),
                }
                Ok(plain(ev, rec, value, Vec::new()))
            }
            "message_delta" => {
                let touches = self.holds();
                Ok(plain(ev, rec, value, touches))
            }
            "content_block_start" => {
                let i = block_index(&value)?;
                if i as usize != self.blocks.len() {
                    return Err(Deviation("block_index").into());
                }
                let cb = value.get("content_block").and_then(Value::as_object).ok_or(Malformed("content_block"))?;
                let bty = cb.get("type").and_then(Value::as_str).ok_or(Malformed("block_type"))?;
                let kind = match bty {
                    "text" => Kind::Text,
                    "thinking" | "redacted_thinking" => Kind::Thinking,
                    "server_tool_use" | "mcp_tool_use" => Kind::Server,
                    t if t.ends_with("_tool_result") => Kind::Server,
                    "tool_use" => Kind::ToolUse,
                    _ => Kind::Other,
                };
                let mut b = BlockState { kind, open: true, id: None, name: String::new(), args: String::new(), raws: Vec::new(), notice_done: false };
                let (mut touches, mut opened, mut reports) = (Vec::new(), Vec::new(), Vec::new());
                match kind {
                    Kind::ToolUse => {
                        match cb.get("input") {
                            Some(Value::Object(m)) if m.is_empty() => {}
                            _ => return Err(Deviation("prefilled_input").into()),
                        }
                        b.id = opt_str(cb.get("id"), "id")?;
                        b.name = cb.get("name").and_then(Value::as_str).ok_or(Malformed("name"))?.to_owned();
                    }
                    Kind::Other => {
                        b.id = opt_str(cb.get("id"), "id")?;
                        b.name = cb.get("name").and_then(Value::as_str).unwrap_or(bty).to_owned();
                        b.raws.push(Value::Object(cb.clone()));
                    }
                    Kind::Server => {
                        let name = cb.get("name").and_then(Value::as_str).unwrap_or(bty).to_owned();
                        reports.push(ReportEntry { choice: 0, position: i, name, decision: ReportDecision::Unenforceable, basis: None, code: None });
                    }
                    Kind::Text | Kind::Thinking => {}
                }
                if b.is_hold() {
                    touches.push(key(i));
                    opened.push(key(i));
                }
                self.blocks.push(b);
                Ok(Step::Frame { frame: Frame { event: Some(ev), payload: rec.data, value: Some(value), touches }, opened, completed: Vec::new(), reports })
            }
            "content_block_delta" => {
                let i = block_index(&value)?;
                let d = value.get("delta").and_then(Value::as_object).ok_or(Malformed("delta"))?.clone();
                let dty = d.get("type").and_then(Value::as_str).ok_or(Malformed("delta_type"))?.to_owned();
                let decided = matches!(verdicts.get(&key(i)), Some(v) if *v != Verdict::PassThrough);
                let b = self.open_block(i)?;
                let mut touches = Vec::new();
                match (b.kind, dty.as_str()) {
                    (Kind::ToolUse, "input_json_delta") => {
                        let pj = d.get("partial_json").and_then(Value::as_str).ok_or(StepError::Deviation("partial_json"))?;
                        if !decided {
                            b.args.push_str(pj);
                        }
                        touches.push(key(i));
                    }
                    (Kind::ToolUse, _) => return Err(StepError::Deviation("tool_delta_type")),
                    (Kind::Other, _) => {
                        if !decided {
                            b.raws.push(Value::Object(d));
                        }
                        touches.push(key(i));
                    }
                    (Kind::Text, "text_delta" | "citations_delta") | (Kind::Thinking, "thinking_delta" | "signature_delta") | (Kind::Server, "input_json_delta") => {}
                    _ => return Err(StepError::Malformed("delta_type")),
                }
                Ok(plain(ev, rec, value, touches))
            }
            // "content_block_stop"
            _ => {
                let i = block_index(&value)?;
                let decided = matches!(verdicts.get(&key(i)), Some(v) if *v != Verdict::PassThrough);
                let b = self.open_block(i)?;
                b.open = false;
                let (mut touches, mut completed) = (Vec::new(), Vec::new());
                if b.is_hold() {
                    touches.push(key(i));
                    if !decided {
                        completed.push((key(i), vec![Self::call_of(b, i)]));
                    }
                }
                Ok(Step::Frame { frame: Frame { event: Some(ev), payload: rec.data, value: Some(value), touches }, opened: Vec::new(), completed, reports: Vec::new() })
            }
        }
    }

    fn incomplete(&self) -> Vec<HoldKey> {
        self.blocks.iter().enumerate().filter(|(_, b)| b.is_hold() && b.open).map(|(i, _)| key(u32::try_from(i).expect("index is bounded"))).collect()
    }

    fn release(&mut self, frame: Frame, verdicts: &Verdicts) -> Vec<Bytes> {
        let ev = frame.event.unwrap_or("ping");
        let original = || sse::frame(Some(ev), &frame.payload);
        let Some(value) = frame.value.as_ref() else { return vec![original()] };
        let code_of = |i: u32| verdicts.get(&key(i)).and_then(|v| v.block_code(0));
        match ev {
            "content_block_start" | "content_block_delta" | "content_block_stop" => {
                let Ok(i) = block_index(value) else { return vec![original()] };
                let Some(code) = code_of(i) else { return vec![original()] };
                match ev {
                    "content_block_start" => vec![sse::frame(Some(ev), json!({"type": ev, "index": i, "content_block": {"type": "text", "text": ""}}).to_string().as_bytes())],
                    "content_block_delta" => Vec::new(),
                    _ => {
                        let Some(b) = self.blocks.get_mut(i as usize) else { return vec![original()] };
                        b.notice_done = true;
                        vec![text_delta(i, &notice_line(&b.name, code)), original()]
                    }
                }
            }
            "message_delta" => {
                let any_blocked = self.holds().iter().any(|k| verdicts.get(k).is_some_and(Verdict::blocks_any));
                let any_allowed = self.holds().iter().any(|k| verdicts.get(k).is_none_or(|v| !v.blocks_any()));
                if any_blocked && !any_allowed && value.pointer("/delta/stop_reason").and_then(Value::as_str) == Some("tool_use") {
                    let mut v = value.clone();
                    v["delta"]["stop_reason"] = "end_turn".into();
                    return vec![sse::frame(Some(ev), v.to_string().as_bytes())];
                }
                vec![original()]
            }
            "message_stop" => {
                let mut out = self.closing(verdicts);
                out.push(original());
                out
            }
            _ => vec![original()],
        }
    }

    fn closing(&mut self, verdicts: &Verdicts) -> Vec<Bytes> {
        let mut out = Vec::new();
        for (i, b) in self.blocks.iter_mut().enumerate() {
            let i = u32::try_from(i).expect("index is bounded");
            let code = verdicts.get(&key(i)).and_then(|v| v.block_code(0));
            if let Some(code) = code
                && b.is_hold()
                && b.open
                && !b.notice_done
            {
                b.notice_done = true;
                out.push(text_delta(i, &notice_line(&b.name, code)));
                out.push(stop_frame(i));
            }
        }
        out
    }

    fn known_calls(&self, hold: HoldKey) -> Vec<(u32, String)> {
        match hold {
            HoldKey::Block(i) => self.blocks.get(i as usize).map(|b| vec![(i, b.name.clone())]).unwrap_or_default(),
            HoldKey::Choice(_) => Vec::new(),
        }
    }

    fn unframed(&self, rec: Record) -> Bytes {
        sse::frame(rec.event.as_deref(), &rec.data)
    }
}
```

`let … && …` chains in `if let` are stable in edition 2024 on 1.88+, and the pin is 1.95.

- [ ] **Step 5: Run the tests**

```bash
cd rs && cargo fmt && cargo nextest run --locked -p paigasus-gateway-core anthropic::
```

Expected: PASS (6 tests). `a_blocked_tool_use_becomes_a_text_block_at_the_same_index` accepts either key order in the rewritten start frame, because `serde_json::Map` sorts keys unless `preserve_order` is on somewhere in the workspace. If it fails on the `stop1` lookup, print `out` and adjust only the lookup string to the actual key order. Do not change the code.

- [ ] **Step 6: Commit**

```bash
git add rs/crates/libs/paigasus-gateway-core
git commit -m "feat(rs): add the Anthropic streaming protocol with the block rewrite (SMA-749)" -m "Dispatch on data.type with a validated event line, a closed allowlist of
block types, server tools reported unenforceable, unknown block types
held as Other, and blocked tool_use blocks turned into text blocks.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 8: The interceptor state machine

**Files:**
- Create: `rs/crates/libs/paigasus-gateway-core/src/interceptor.rs`
- Modify: `src/lib.rs`. Remove every `#![cfg_attr(not(test), allow(dead_code))]` line from `src/{sse,json,report,protocol}.rs`, `src/openai/{classify,assembly,stream}.rs` and `src/anthropic/stream.rs`.

**Interfaces:**
- Consumes: everything before.
- Produces (`pub`, re-exported):
  - `struct Interceptor` with:
    - `fn new(format: Format, limits: Limits, terminal: Bytes, now: Instant) -> Self`
    - `fn push(&mut self, chunk: &[u8], now: Instant) -> Vec<Action>`
    - `fn decided(&mut self, hold: HoldKey, result: Result<Vec<Decision>, DeciderError>, now: Instant) -> Vec<Action>`
    - `fn tick(&mut self, now: Instant) -> Vec<Action>`
    - `fn finish(&mut self, now: Instant) -> Vec<Action>`
    - `fn abort(&mut self) -> Vec<Action>`
    - `fn is_finished(&self) -> bool`
    - `#[doc(hidden)] fn queued_bytes(&self) -> usize` (for P2)
  - `enum Action { Emit(Bytes), Decide { hold: HoldKey, calls: Vec<ToolCall> }, Wake(Instant), Count(Metric) }`
  - `enum Metric { RecordDropped(&'static str), Decision { decision: &'static str, basis: &'static str }, OnFailure(&'static str), HoldSeconds(f64) }`
  - `struct DeciderError { pub reason: &'static str }` (`thiserror`). It is defined here rather than in Task 10, because `decided` takes it.

Rules (D7, D10–D17, § 5.3, § 6):
- **Release order (D12).** Before any hold opens, a tool-free frame is released at once. Once the queue is non-empty, every frame is queued. Release pops from the head while every hold the head touches has a verdict.
- **A hold's life.**
  - It opens Assembling.
  - When the protocol completes it, it becomes Deciding, and `Action::Decide` is emitted.
  - `decided` records the verdict, emits one report comment per call, and drains the queue.
  - The length check of D18 applies: a wrong length is a decider error.
  - A late `decided` for a hold that is no longer Deciding is ignored.
- **Time limits (D14).** `tick(now)`:
  - an Assembling hold older than `max_hold` is a hold limit;
  - a Deciding hold older than `decide_timeout` is a decider error;
  - while the queue is non-empty or a hold is Deciding, a keep-alive is emitted once `keepalive` has passed since the last emit.
- **Memory (D13).** When the queued payload bytes pass `max_held_bytes`, every undecided hold gets a hold limit.
- **Hold limit.** With `block` the verdict is `BlockAll(HoldLimit)`. With `allow` it is `PassThrough`: the hold keeps assembling, and the decider is still asked once it completes, with the decision reported as basis `on_failure` (§ 5.3).
- **Stream failure** (a malformed record while the queue is non-empty, a deviation, or an oversized record):
  - With `block`, the mode becomes Failed: every undecided hold, and every later one, gets `BlockAll(Malformed)`.
  - With `allow`, the mode becomes PassThrough: undecided holds get `PassThrough`, one `: paigasus-interception pass-through` comment is emitted, and every later record is re-framed unclassified (P-4).
- **A malformed record while the queue is empty** is dropped and counted (D7). In Failed mode every malformed or deviating record is dropped.
- **End.**
  - An `End` record sets after-end: later records are dropped as `after_done`. It truncates the incomplete holds, then queues the end frame.
  - `finish()` dispatches the tail, truncates, and ends once the queue is empty and nothing is Deciding. `closing()` runs then.
  - A truncated hold gets `BlockAll(Truncated)` with `block`, and `PassThrough` with `allow`.
- **Upstream error.** `abort()`, or an upstream error record while the queue is non-empty or a hold is open, drops the queue, emits the terminal frame, and ends (D17). An upstream error record with nothing held is released like any frame.
- **Wake.** Every call that does not end the stream returns one `Action::Wake(next deadline)`.

- [ ] **Step 1: Write the failing tests**

`src/interceptor.rs` holds the tests below. A helper drives the machine synchronously with a scripted decider.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{BlockCode, Decision};
    use serde_json::{Value, json};
    use std::time::Duration;

    const TERMINAL: &[u8] = b"\n\ndata: {\"error\":\"terminal\"}\n\n";

    fn limits() -> Limits {
        Limits { max_record_bytes: 4096, max_held_bytes: 8192, max_hold: Duration::from_secs(300), decide_timeout: Duration::from_secs(5), keepalive: Duration::from_secs(10), on_failure: OnFailure::Block }
    }
    fn data(v: Value) -> String {
        format!("data: {v}\n\n")
    }
    fn text(c: u32, s: &str) -> String {
        data(json!({"id":"x","choices":[{"index":c,"delta":{"content":s},"finish_reason":null}]}))
    }
    fn call(c: u32, i: u32, name: &str, args: &str) -> String {
        data(json!({"id":"x","choices":[{"index":c,"delta":{"tool_calls":[{"index":i,"id":format!("call_{c}_{i}"),"type":"function","function":{"name":name,"arguments":args}}]},"finish_reason":null}]}))
    }
    fn args(c: u32, i: u32, a: &str) -> String {
        data(json!({"id":"x","choices":[{"index":c,"delta":{"tool_calls":[{"index":i,"function":{"arguments":a}}]},"finish_reason":null}]}))
    }
    fn finish(c: u32) -> String {
        data(json!({"id":"x","choices":[{"index":c,"delta":{},"finish_reason":"tool_calls"}]}))
    }

    /// Run a whole stream. `decide` answers each Decide at once; `None` leaves it pending.
    struct Run {
        out: String,
        decides: Vec<(HoldKey, Vec<ToolCall>)>,
        counts: Vec<Metric>,
    }
    fn run_with(limits: Limits, chunks: &[String], decide: impl Fn(&[ToolCall]) -> Option<Result<Vec<Decision>, DeciderError>>) -> Run {
        let t0 = Instant::now();
        let mut i = Interceptor::new(Format::OpenAiChat, limits, Bytes::from_static(TERMINAL), t0);
        let mut r = Run { out: String::new(), decides: Vec::new(), counts: Vec::new() };
        let mut pending: Vec<Action> = Vec::new();
        for c in chunks {
            pending.extend(i.push(c.as_bytes(), t0));
            drain(&mut i, &mut pending, &mut r, &decide, t0);
        }
        pending.extend(i.finish(t0));
        drain(&mut i, &mut pending, &mut r, &decide, t0);
        r
    }
    fn drain(i: &mut Interceptor, pending: &mut Vec<Action>, r: &mut Run, decide: &impl Fn(&[ToolCall]) -> Option<Result<Vec<Decision>, DeciderError>>, now: Instant) {
        while !pending.is_empty() {
            for a in std::mem::take(pending) {
                match a {
                    Action::Emit(b) => r.out.push_str(std::str::from_utf8(&b).unwrap()),
                    Action::Decide { hold, calls } => {
                        r.decides.push((hold, calls.clone()));
                        if let Some(res) = decide(&calls) {
                            pending.extend(i.decided(hold, res, now));
                        }
                    }
                    Action::Count(m) => r.counts.push(m),
                    Action::Wake(_) => {}
                }
            }
        }
    }
    fn allow(c: &[ToolCall]) -> Option<Result<Vec<Decision>, DeciderError>> {
        Some(Ok(vec![Decision::Allow; c.len()]))
    }
    fn block_named(prefix: &'static str) -> impl Fn(&[ToolCall]) -> Option<Result<Vec<Decision>, DeciderError>> {
        move |c| Some(Ok(c.iter().map(|t| if t.name.starts_with(prefix) { Decision::Block { code: BlockCode::Policy } } else { Decision::Allow }).collect()))
    }

    #[test]
    fn text_before_any_hold_streams_at_once() {
        let t0 = Instant::now();
        let mut i = Interceptor::new(Format::OpenAiChat, limits(), Bytes::from_static(TERMINAL), t0);
        let acts = i.push(text(0, "hi").as_bytes(), t0);
        assert!(matches!(&acts[0], Action::Emit(b) if b.starts_with(b"data: {")));
    }

    #[test]
    fn allow_all_preserves_every_payload() {
        let chunks = vec![text(0, "a"), call(0, 0, "read", "{}"), text(0, "b"), finish(0), "data: [DONE]\n\n".into()];
        let r = run_with(limits(), &chunks, allow);
        let payloads: Vec<&str> = r.out.split("\n\n").filter(|f| f.starts_with("data: ")).collect();
        let want: Vec<String> = chunks.iter().map(|c| c.trim_end().to_owned()).collect();
        assert_eq!(payloads, want);
        assert!(r.out.contains(": paigasus-decision {"));
    }

    #[test]
    fn a_blocked_call_never_reaches_the_output() {
        let chunks = vec![call(0, 0, "rm_all", "{\"p\":"), args(0, 0, "\"CANARY\"}"), finish(0), "data: [DONE]\n\n".into()];
        let r = run_with(limits(), &chunks, block_named("rm"));
        assert!(!r.out.contains("CANARY"), "{}", r.out);
        assert!(r.out.contains("[paigasus]"));
    }

    #[test]
    fn a_frame_waits_for_every_hold_it_touches() {
        // n=2: choice 0 opens, choice 1 sends a whole call that sits behind it, then choice 0
        // finishes. Choice 1 is still undecided, so its call must not be released with A's tail.
        let t0 = Instant::now();
        let mut i = Interceptor::new(Format::OpenAiChat, limits(), Bytes::from_static(TERMINAL), t0);
        i.push(call(0, 0, "read", "{}").as_bytes(), t0);
        i.push(call(1, 0, "rm", "CANARY").as_bytes(), t0);
        let acts = i.push(finish(0).as_bytes(), t0);
        let Some(Action::Decide { hold, .. }) = acts.iter().find(|a| matches!(a, Action::Decide { .. })).cloned() else { panic!("{acts:?}") };
        assert_eq!(hold, HoldKey::Choice(0));
        let acts = i.decided(hold, Ok(vec![Decision::Allow]), t0);
        let out: String = acts.iter().filter_map(|a| if let Action::Emit(b) = a { Some(String::from_utf8_lossy(b).into_owned()) } else { None }).collect();
        assert!(out.contains("\"read\""), "choice 0's frame is released: {out}");
        assert!(!out.contains("CANARY"), "choice 1's undecided call stays queued: {out}");
    }

    #[test]
    fn eof_with_an_open_hold_is_truncated_not_complete() {
        let chunks = vec![call(0, 0, "read", "{\"CANARY\":")];
        let r = run_with(limits(), &chunks, allow);
        assert!(r.decides.is_empty(), "a truncated hold is never decided as complete");
        assert!(!r.out.contains("CANARY"));
        assert!(r.out.contains("(truncated)"));
        assert!(r.counts.contains(&Metric::OnFailure("truncated")));
    }

    #[test]
    fn done_with_an_open_hold_is_truncated_and_later_records_are_dropped() {
        let chunks = vec![call(0, 0, "read", "CANARY"), "data: [DONE]\n\n".into(), call(0, 1, "late", "LATE")];
        let r = run_with(limits(), &chunks, allow);
        assert!(!r.out.contains("CANARY") && !r.out.contains("LATE"));
        assert!(r.out.ends_with("data: [DONE]\n\n"), "{}", r.out);
        assert!(r.counts.contains(&Metric::RecordDropped("after_done")));
    }

    #[test]
    fn abort_drops_the_queue_and_emits_the_terminal_frame() {
        let t0 = Instant::now();
        let mut i = Interceptor::new(Format::OpenAiChat, limits(), Bytes::from_static(TERMINAL), t0);
        i.push(call(0, 0, "read", "CANARY").as_bytes(), t0);
        let acts = i.abort();
        assert_eq!(acts, vec![Action::Emit(Bytes::from_static(TERMINAL))]);
        assert!(i.is_finished());
        assert!(i.push(text(0, "x").as_bytes(), t0).is_empty());
    }

    #[test]
    fn an_upstream_error_record_mid_hold_aborts() {
        let chunks = vec![call(0, 0, "read", "CANARY"), data(json!({"error":{"message":"boom"}}))];
        let r = run_with(limits(), &chunks, allow);
        assert_eq!(r.out.as_bytes(), TERMINAL);
    }

    #[test]
    fn an_upstream_error_record_with_nothing_held_is_forwarded() {
        let chunks = vec![text(0, "a"), data(json!({"error":{"message":"boom"}}))];
        let r = run_with(limits(), &chunks, allow);
        assert!(r.out.contains("\"boom\""));
    }

    #[test]
    fn a_short_decision_vector_blocks_every_call() {
        let chunks = vec![call(0, 0, "a", "X1"), call(0, 1, "b", "X2"), finish(0)];
        let r = run_with(limits(), &chunks, |_| Some(Ok(vec![])));
        assert!(!r.out.contains("X1") && !r.out.contains("X2"), "{}", r.out);
        assert!(r.out.contains("(decider-error)"));
    }

    #[test]
    fn a_decider_error_follows_on_failure() {
        let chunks = vec![call(0, 0, "a", "X1"), finish(0)];
        let err = |_: &[ToolCall]| Some(Err(DeciderError { reason: "boom" }));
        let blocked = run_with(limits(), &chunks, err);
        assert!(!blocked.out.contains("X1"));
        let allowed = run_with(Limits { on_failure: OnFailure::Allow, ..limits() }, &chunks, err);
        assert!(allowed.out.contains("X1"));
        assert!(allowed.out.contains("\"basis\":\"on_failure\""));
    }

    #[test]
    fn a_malformed_record_with_an_empty_queue_is_dropped() {
        let chunks = vec![text(0, "a"), "data: {\"choices\":NaN}\n\n".into(), text(0, "b")];
        let r = run_with(limits(), &chunks, allow);
        assert!(!r.out.contains("NaN"));
        assert!(r.out.contains("\"b\""));
        assert!(r.counts.contains(&Metric::RecordDropped("malformed")));
    }

    #[test]
    fn a_malformed_record_mid_hold_blocks_the_stream_or_passes_it_through() {
        let chunks = vec![call(0, 0, "read", "X1"), "data: {\"a\":1,\"a\":2}\n\n".into(), call(0, 1, "late", "X2"), finish(0)];
        let blocked = run_with(limits(), &chunks, allow);
        assert!(!blocked.out.contains("X1") && !blocked.out.contains("X2"), "{}", blocked.out);
        assert!(blocked.out.contains("(malformed)"));
        let observed = run_with(Limits { on_failure: OnFailure::Allow, ..limits() }, &chunks, allow);
        assert!(observed.out.contains("X1") && observed.out.contains("X2"));
        assert!(observed.out.contains(": paigasus-interception pass-through"));
    }

    #[test]
    fn an_oversized_record_is_a_stream_failure() {
        let big = args(0, 0, &"x".repeat(5000));
        let r = run_with(limits(), &[call(0, 0, "first", "X1"), big], allow);
        assert!(r.counts.contains(&Metric::RecordDropped("oversized")));
        assert!(!r.out.contains("X1"));
    }

    #[test]
    fn the_queue_never_exceeds_the_cap_by_more_than_one_record() {
        let t0 = Instant::now();
        let l = Limits { max_held_bytes: 600, ..limits() };
        let mut i = Interceptor::new(Format::OpenAiChat, l, Bytes::from_static(TERMINAL), t0);
        let mut acts = i.push(call(0, 0, "read", "{").as_bytes(), t0);
        for _ in 0..50 {
            acts.extend(i.push(args(0, 0, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").as_bytes(), t0));
            assert!(i.queued_bytes() <= 600 + 200, "{}", i.queued_bytes());
        }
        assert!(acts.contains(&Action::Count(Metric::OnFailure("hold_limit"))));
    }

    #[test]
    fn a_hold_past_max_hold_is_blocked_on_tick() {
        let t0 = Instant::now();
        let mut i = Interceptor::new(Format::OpenAiChat, limits(), Bytes::from_static(TERMINAL), t0);
        i.push(call(0, 0, "read", "X1").as_bytes(), t0);
        let acts = i.tick(t0 + Duration::from_secs(301));
        assert!(acts.contains(&Action::Count(Metric::OnFailure("hold_limit"))));
        let acts = i.push(finish(0).as_bytes(), t0 + Duration::from_secs(302));
        let out: String = acts.iter().filter_map(|a| if let Action::Emit(b) = a { Some(String::from_utf8_lossy(b).into_owned()) } else { None }).collect();
        assert!(!out.contains("X1") && out.contains("(hold-limit)"), "{out}");
    }

    #[test]
    fn a_slow_decider_times_out_on_tick() {
        let t0 = Instant::now();
        let mut i = Interceptor::new(Format::OpenAiChat, limits(), Bytes::from_static(TERMINAL), t0);
        i.push(call(0, 0, "read", "X1").as_bytes(), t0);
        i.push(finish(0).as_bytes(), t0);
        let acts = i.tick(t0 + Duration::from_secs(6));
        let out: String = acts.iter().filter_map(|a| if let Action::Emit(b) = a { Some(String::from_utf8_lossy(b).into_owned()) } else { None }).collect();
        assert!(out.contains("(decider-error)"), "{out}");
        // The late answer is ignored.
        assert!(i.decided(HoldKey::Choice(0), Ok(vec![Decision::Allow]), t0 + Duration::from_secs(7)).iter().all(|a| !matches!(a, Action::Emit(_))));
    }

    #[test]
    fn keep_alives_flow_while_holding_and_wake_is_always_set() {
        let t0 = Instant::now();
        let mut i = Interceptor::new(Format::OpenAiChat, limits(), Bytes::from_static(TERMINAL), t0);
        let acts = i.push(call(0, 0, "read", "{").as_bytes(), t0);
        assert!(acts.iter().any(|a| matches!(a, Action::Wake(w) if *w == t0 + Duration::from_secs(10))));
        let acts = i.tick(t0 + Duration::from_secs(10));
        assert!(acts.contains(&Action::Emit(Bytes::from_static(crate::sse::KEEP_ALIVE))));
        let acts = i.tick(t0 + Duration::from_secs(11));
        assert!(!acts.contains(&Action::Emit(Bytes::from_static(crate::sse::KEEP_ALIVE))), "one per interval");
    }

    #[test]
    fn finish_waits_for_a_pending_decision() {
        let t0 = Instant::now();
        let mut i = Interceptor::new(Format::OpenAiChat, limits(), Bytes::from_static(TERMINAL), t0);
        i.push(call(0, 0, "read", "{}").as_bytes(), t0);
        i.push(finish(0).as_bytes(), t0);
        i.finish(t0);
        assert!(!i.is_finished());
        i.decided(HoldKey::Choice(0), Ok(vec![Decision::Allow]), t0);
        assert!(i.is_finished());
    }

    #[test]
    fn anthropic_streams_are_driven_the_same_way() {
        let ev = |v: Value| format!("event: {}\ndata: {v}\n\n", v["type"].as_str().unwrap());
        let chunks = [
            ev(json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"t","name":"rm","input":{}}})),
            ev(json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"\"CANARY\""}})),
            ev(json!({"type":"content_block_stop","index":0})),
            ev(json!({"type":"message_stop"})),
        ];
        let t0 = Instant::now();
        let mut i = Interceptor::new(Format::AnthropicMessages, limits(), Bytes::from_static(TERMINAL), t0);
        let mut r = Run { out: String::new(), decides: Vec::new(), counts: Vec::new() };
        let mut pending = Vec::new();
        for c in &chunks {
            pending.extend(i.push(c.as_bytes(), t0));
            drain(&mut i, &mut pending, &mut r, &block_named("rm"), t0);
        }
        pending.extend(i.finish(t0));
        drain(&mut i, &mut pending, &mut r, &block_named("rm"), t0);
        assert!(!r.out.contains("CANARY"));
        assert!(r.out.contains("[paigasus]"));
        assert!(i.is_finished());
    }
}
```

- [ ] **Step 2: Run them to verify they fail**

```bash
cd rs && cargo nextest run --locked -p paigasus-gateway-core interceptor::
```

Expected: FAIL (compile errors).

- [ ] **Step 3: Implement**

```rust
// SPDX-License-Identifier: Apache-2.0
//! The interceptor: one stream's ordering, limits and failure handling (spec D7, D10–D17, § 5.3,
//! § 6).
//!
//! Synchronous and I/O-free (spec D1). The caller feeds bytes and decisions, passes the current
//! instant to every call, and performs the returned actions in order. The interceptor never
//! reads a clock and never spawns anything.

use std::collections::{BTreeMap, VecDeque};
use std::time::Instant;

use bytes::Bytes;

use crate::anthropic::stream::AnthropicStream;
use crate::limits::{Limits, OnFailure};
use crate::openai::stream::OpenAiStream;
use crate::protocol::{Frame, Protocol, Step, StepError, Verdict, Verdicts};
use crate::report::{ReportDecision, ReportEntry, report_comment};
use crate::sse::{self, SseItem, SseSplitter};
use crate::types::{Basis, BlockCode, Decision, Format, HoldKey, ToolCall};

/// The decider failed, timed out, or broke its contract (D18).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("tool-call decider failed: {reason}")]
pub struct DeciderError {
    pub reason: &'static str,
}

/// A counter or histogram sample for the gateway to record (P-9). Labels are closed sets.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Metric {
    /// `reason` = `malformed` | `oversized` | `after_done` | `late_fragment`.
    RecordDropped(&'static str),
    /// `decision` = `allow` | `block`; `basis` = `decider` | `on_failure`.
    Decision { decision: &'static str, basis: &'static str },
    /// `code` = `hold_limit` | `truncated` | `decider_error` | `malformed`.
    OnFailure(&'static str),
    /// Seconds from a hold's first frame to its completion.
    HoldSeconds(f64),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Bytes for the client, in order.
    Emit(Bytes),
    /// Ask the decider about one completed hold; answer with [`Interceptor::decided`].
    Decide { hold: HoldKey, calls: Vec<ToolCall> },
    /// Call [`Interceptor::tick`] no later than this instant.
    Wake(Instant),
    Count(Metric),
}

#[derive(Clone)]
enum Track {
    Assembling { opened: Instant, observe: bool },
    Deciding { since: Instant, calls: Vec<ToolCall>, observe: bool },
    Decided,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Normal,
    Failed(BlockCode),
    PassThrough,
}

pub struct Interceptor {
    proto: Box<dyn Protocol>,
    sse: SseSplitter,
    limits: Limits,
    terminal: Bytes,
    queue: VecDeque<Frame>,
    queued: usize,
    verdicts: Verdicts,
    holds: BTreeMap<HoldKey, Track>,
    mode: Mode,
    after_end: bool,
    input_done: bool,
    ended: bool,
    last_emit: Instant,
    out: Vec<Action>,
}

fn on_failure_label(code: BlockCode) -> &'static str {
    match code {
        BlockCode::HoldLimit => "hold_limit",
        BlockCode::Truncated => "truncated",
        BlockCode::DeciderError => "decider_error",
        _ => "malformed",
    }
}

fn choice_of(k: HoldKey) -> u32 {
    match k {
        HoldKey::Choice(c) => c,
        HoldKey::Block(_) => 0,
    }
}

impl Interceptor {
    #[must_use]
    pub fn new(format: Format, limits: Limits, terminal: Bytes, now: Instant) -> Self {
        let proto: Box<dyn Protocol> = match format {
            Format::OpenAiChat => Box::new(OpenAiStream::default()),
            Format::AnthropicMessages => Box::new(AnthropicStream::default()),
        };
        Interceptor {
            proto,
            sse: SseSplitter::new(limits.max_record_bytes),
            limits,
            terminal,
            queue: VecDeque::new(),
            queued: 0,
            verdicts: Verdicts::new(),
            holds: BTreeMap::new(),
            mode: Mode::Normal,
            after_end: false,
            input_done: false,
            ended: false,
            last_emit: now,
            out: Vec::new(),
        }
    }

    pub fn push(&mut self, chunk: &[u8], now: Instant) -> Vec<Action> {
        if self.ended || self.input_done {
            return Vec::new();
        }
        for item in self.sse.push(chunk) {
            self.item(item, now);
            if self.ended {
                break;
            }
        }
        self.settle(now)
    }

    pub fn finish(&mut self, now: Instant) -> Vec<Action> {
        if self.ended || self.input_done {
            return Vec::new();
        }
        for item in self.sse.finish() {
            self.item(item, now);
        }
        self.input_done = true;
        self.truncate_open(now);
        self.settle(now)
    }

    pub fn abort(&mut self) -> Vec<Action> {
        if self.ended {
            return Vec::new();
        }
        self.queue.clear();
        self.queued = 0;
        self.ended = true;
        let mut out = std::mem::take(&mut self.out);
        out.push(Action::Emit(self.terminal.clone()));
        out
    }

    pub fn decided(&mut self, hold: HoldKey, result: Result<Vec<Decision>, DeciderError>, now: Instant) -> Vec<Action> {
        self.apply_decision(hold, result, now);
        self.settle(now)
    }

    pub fn tick(&mut self, now: Instant) -> Vec<Action> {
        if self.ended {
            return Vec::new();
        }
        let due: Vec<(HoldKey, bool)> = self
            .holds
            .iter()
            .filter_map(|(k, t)| match t {
                Track::Assembling { opened, observe: false } if now.saturating_duration_since(*opened) >= self.limits.max_hold => Some((*k, false)),
                Track::Deciding { since, .. } if now.saturating_duration_since(*since) >= self.limits.decide_timeout => Some((*k, true)),
                _ => None,
            })
            .collect();
        for (k, deciding) in due {
            if deciding {
                self.apply_decision(k, Err(DeciderError { reason: "timeout" }), now);
            } else {
                self.count(Metric::OnFailure("hold_limit"));
                self.hold_limit(k);
            }
        }
        if (!self.queue.is_empty() || self.any_deciding()) && now.saturating_duration_since(self.last_emit) >= self.limits.keepalive {
            self.emit(Bytes::from_static(sse::KEEP_ALIVE), now);
        }
        self.settle(now)
    }

    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.ended
    }

    #[doc(hidden)]
    #[must_use]
    pub fn queued_bytes(&self) -> usize {
        self.queued
    }

    // ---- input ---------------------------------------------------------------------------------

    fn item(&mut self, item: SseItem, now: Instant) {
        let rec = match item {
            SseItem::Oversized => {
                self.count(Metric::RecordDropped("oversized"));
                self.stream_failure(now);
                return;
            }
            SseItem::Record(r) => r,
        };
        if self.after_end {
            self.count(Metric::RecordDropped("after_done"));
            return;
        }
        if self.mode == Mode::PassThrough {
            let b = self.proto.unframed(rec);
            self.emit(b, now);
            return;
        }
        match self.proto.step(rec, &self.verdicts) {
            Err(StepError::Malformed(_)) => {
                self.count(Metric::RecordDropped("malformed"));
                if !self.queue.is_empty() {
                    self.stream_failure(now);
                }
            }
            Err(StepError::Deviation(why)) => {
                self.count(Metric::RecordDropped(if why == "late_fragment" { "late_fragment" } else { "malformed" }));
                self.stream_failure(now);
            }
            Ok(Step::UpstreamError(frame)) => {
                if !self.queue.is_empty() || self.any_open() {
                    let acts = self.abort();
                    self.out.extend(acts);
                } else {
                    self.enqueue(frame, now);
                }
            }
            Ok(Step::End(frame)) => {
                self.after_end = true;
                self.truncate_open(now);
                self.enqueue(frame, now);
            }
            Ok(Step::Frame { frame, opened, completed, reports }) => {
                for r in &reports {
                    self.emit(report_comment(r), now);
                }
                for k in opened {
                    self.open(k, now);
                }
                self.enqueue(frame, now);
                for (k, calls) in completed {
                    self.complete(k, calls, now);
                }
            }
        }
    }

    fn open(&mut self, k: HoldKey, now: Instant) {
        match self.mode {
            Mode::Failed(code) => self.block_all(k, code),
            _ => {
                self.holds.insert(k, Track::Assembling { opened: now, observe: false });
            }
        }
    }

    fn complete(&mut self, k: HoldKey, calls: Vec<ToolCall>, now: Instant) {
        if let Some(Track::Assembling { opened, observe }) = self.holds.get(&k).cloned() {
            self.count(Metric::HoldSeconds(now.saturating_duration_since(opened).as_secs_f64()));
            self.holds.insert(k, Track::Deciding { since: now, calls: calls.clone(), observe });
            self.out.push(Action::Decide { hold: k, calls });
        }
    }

    fn enqueue(&mut self, frame: Frame, now: Instant) {
        if self.queue.is_empty() && frame.touches.iter().all(|k| self.verdicts.contains_key(k)) {
            for b in self.proto.release(frame, &self.verdicts) {
                self.emit(b, now);
            }
            return;
        }
        self.queued += frame.payload.len();
        self.queue.push_back(frame);
        if self.queued > self.limits.max_held_bytes {
            self.count(Metric::OnFailure("hold_limit"));
            for k in self.undecided() {
                self.hold_limit(k);
            }
        }
    }

    // ---- decisions and failures ----------------------------------------------------------------

    fn apply_decision(&mut self, hold: HoldKey, result: Result<Vec<Decision>, DeciderError>, now: Instant) {
        let Some(Track::Deciding { calls, observe, .. }) = self.holds.get(&hold).cloned() else { return };
        let checked = match result {
            Ok(d) if d.len() == calls.len() => Ok(d),
            Ok(_) => Err(DeciderError { reason: "wrong_length" }),
            Err(e) => Err(e),
        };
        let (verdict, decisions, mut basis) = match (checked, self.limits.on_failure) {
            (Ok(d), _) => {
                let v = if d.iter().all(|d| d.is_allow()) { Verdict::AllowAll } else { Verdict::PerCall(d.clone()) };
                (v, d, Basis::Decider)
            }
            (Err(_), OnFailure::Block) => {
                self.count(Metric::OnFailure("decider_error"));
                (Verdict::BlockAll(BlockCode::DeciderError), vec![Decision::Block { code: BlockCode::DeciderError }; calls.len()], Basis::OnFailure)
            }
            (Err(_), OnFailure::Allow) => {
                self.count(Metric::OnFailure("decider_error"));
                (Verdict::AllowAll, vec![Decision::Allow; calls.len()], Basis::OnFailure)
            }
        };
        if observe {
            basis = Basis::OnFailure;
        }
        for (c, d) in calls.iter().zip(&decisions) {
            let (decision, code) = match d {
                Decision::Block { code } => (ReportDecision::Block, Some(*code)),
                _ => (ReportDecision::Allow, None),
            };
            let label = if decision == ReportDecision::Block { "block" } else { "allow" };
            self.count(Metric::Decision { decision: label, basis: basis.as_str() });
            let entry = ReportEntry { choice: c.choice, position: c.position, name: c.name.clone(), decision, basis: Some(basis), code };
            self.emit(report_comment(&entry), now);
        }
        if !observe {
            self.verdicts.insert(hold, verdict);
        }
        self.holds.insert(hold, Track::Decided);
    }

    /// D13/D14: one hold over a limit.
    fn hold_limit(&mut self, k: HoldKey) {
        match self.limits.on_failure {
            OnFailure::Block => self.block_all(k, BlockCode::HoldLimit),
            OnFailure::Allow => self.observe(k),
        }
    }

    /// A hold blocked as a whole, with one report per call known so far.
    fn block_all(&mut self, k: HoldKey, code: BlockCode) {
        self.verdicts.insert(k, Verdict::BlockAll(code));
        self.holds.insert(k, Track::Decided);
        let known = self.proto.known_calls(k);
        let known = if known.is_empty() { vec![(0, "unknown".to_owned())] } else { known };
        for (position, name) in known {
            let entry = ReportEntry { choice: choice_of(k), position, name, decision: ReportDecision::Block, basis: Some(Basis::OnFailure), code: Some(code) };
            self.count(Metric::Decision { decision: "block", basis: Basis::OnFailure.as_str() });
            self.out.push(Action::Emit(report_comment(&entry)));
        }
    }

    /// Observe mode: released as is, still decided for the report once complete (§ 5.3).
    fn observe(&mut self, k: HoldKey) {
        self.verdicts.insert(k, Verdict::PassThrough);
        let next = match self.holds.get(&k) {
            Some(Track::Assembling { opened, .. }) => Track::Assembling { opened: *opened, observe: true },
            Some(Track::Deciding { since, calls, .. }) => Track::Deciding { since: *since, calls: calls.clone(), observe: true },
            _ => return,
        };
        self.holds.insert(k, next);
    }

    fn stream_failure(&mut self, now: Instant) {
        if self.mode != Mode::Normal {
            return;
        }
        self.count(Metric::OnFailure("malformed"));
        match self.limits.on_failure {
            OnFailure::Block => {
                self.mode = Mode::Failed(BlockCode::Malformed);
                for k in self.undecided() {
                    self.block_all(k, BlockCode::Malformed);
                }
            }
            OnFailure::Allow => {
                self.mode = Mode::PassThrough;
                for k in self.undecided() {
                    self.observe(k);
                }
                self.emit(sse::comment("paigasus-interception pass-through"), now);
            }
        }
    }

    fn truncate_open(&mut self, _now: Instant) {
        for k in self.proto.incomplete() {
            if matches!(self.holds.get(&k), Some(Track::Assembling { observe: false, .. })) {
                self.count(Metric::OnFailure("truncated"));
                match self.limits.on_failure {
                    OnFailure::Block => self.block_all(k, BlockCode::Truncated),
                    OnFailure::Allow => {
                        self.verdicts.insert(k, Verdict::PassThrough);
                        self.holds.insert(k, Track::Decided);
                    }
                }
            } else if matches!(self.holds.get(&k), Some(Track::Assembling { observe: true, .. })) {
                self.holds.insert(k, Track::Decided);
            }
        }
    }

    // ---- output --------------------------------------------------------------------------------

    fn settle(&mut self, now: Instant) -> Vec<Action> {
        if !self.ended {
            while let Some(front) = self.queue.front() {
                if !front.touches.iter().all(|k| self.verdicts.contains_key(k)) {
                    break;
                }
                let frame = self.queue.pop_front().expect("front exists");
                self.queued -= frame.payload.len();
                for b in self.proto.release(frame, &self.verdicts) {
                    self.emit(b, now);
                }
            }
            if self.input_done && self.queue.is_empty() && !self.any_deciding() {
                for b in self.proto.closing(&self.verdicts) {
                    self.emit(b, now);
                }
                self.ended = true;
            }
        }
        if !self.ended {
            self.out.push(Action::Wake(self.next_wake()));
        }
        std::mem::take(&mut self.out)
    }

    fn emit(&mut self, b: Bytes, now: Instant) {
        self.out.push(Action::Emit(b));
        self.last_emit = now;
    }

    fn count(&mut self, m: Metric) {
        self.out.push(Action::Count(m));
    }

    fn next_wake(&self) -> Instant {
        let mut at = self.last_emit + self.limits.keepalive;
        for t in self.holds.values() {
            let d = match t {
                Track::Assembling { opened, observe: false } => *opened + self.limits.max_hold,
                Track::Deciding { since, .. } => *since + self.limits.decide_timeout,
                _ => continue,
            };
            at = at.min(d);
        }
        at
    }

    fn undecided(&self) -> Vec<HoldKey> {
        self.holds.iter().filter(|(_, t)| matches!(t, Track::Assembling { observe: false, .. } | Track::Deciding { observe: false, .. })).map(|(k, _)| *k).collect()
    }

    fn any_open(&self) -> bool {
        self.holds.values().any(|t| matches!(t, Track::Assembling { .. } | Track::Deciding { .. }))
    }

    fn any_deciding(&self) -> bool {
        self.holds.values().any(|t| matches!(t, Track::Deciding { .. }))
    }
}
```

Three details are easy to get wrong:
- **`abort()` takes `self.out` first.** Actions that `item` already produced in the same `push`, such as counts, are not lost.
- **`settle()` checks `ended` before it drains.** After an abort nothing more is released.
- **An observe-mode Assembling hold is never timed out.** It is already released (`PassThrough`), and a timeout would only emit a second report.

`src/lib.rs`: add `mod interceptor;` and `pub use interceptor::{Action, DeciderError, Interceptor, Metric};`. Then delete every `#![cfg_attr(not(test), allow(dead_code))]` line:

```bash
grep -rln 'allow(dead_code)' rs/crates/libs/paigasus-gateway-core/src
```

Delete the line from each file the grep lists.

- [ ] **Step 4: Run the tests and the dead-code proof**

```bash
cd rs && cargo fmt && cargo nextest run --locked -p paigasus-gateway-core && cargo clippy --locked -p paigasus-gateway-core --all-targets -- -D warnings
grep -rn 'allow(dead_code)' crates/libs/paigasus-gateway-core/src && echo "FAIL: an allow is left" || echo "no allow left"
```

Expected: every test passes, there are no clippy warnings, and the grep prints `no allow left`. A dead-code error here names an item no path uses. Either a later task needs it, so move it there, or nothing does, so delete it. Do not re-add the `allow`.

- [ ] **Step 5: Commit**

```bash
git add rs/crates/libs/paigasus-gateway-core
git commit -m "feat(rs): add the tool-call interceptor state machine (SMA-749)" -m "One FIFO per stream released by touch sets, hold and decider deadlines,
the per-stream memory cap, keep-alives, truncation at the end, the abort
path, and both on_failure modes, all as synchronous actions.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---
### Task 9: Non-stream bodies

**Files:**
- Create: `rs/crates/libs/paigasus-gateway-core/src/body.rs`, `src/openai/body.rs`, `src/anthropic/body.rs`
- Modify: `src/lib.rs`, `src/openai/mod.rs`, `src/anthropic/mod.rs`

**Interfaces:**
- Consumes: Tasks 1–8.
- Produces (`pub`, re-exported):
  - `struct BodyCalls`, which keeps the parsed body private, with:
    - `pub calls: Vec<ToolCall>`
    - `pub unenforceable: Vec<ReportEntry>`
    - `fn apply(self, decisions: &[Decision]) -> Option<Vec<u8>>`. `None` means nothing is blocked, so forward the original bytes. `Some` is the rewritten body.
    - `fn report(&self, decisions: &[Decision], basis: Basis) -> String`, the header value (D22). It includes the unenforceable entries.
  - `fn parse_body(format: Format, body: &[u8], max_bytes: usize) -> Result<BodyCalls, Malformed>`
  - `pub use json::Malformed`. `Malformed` becomes `pub`, because the gateway names it.

Rules (§ 4.2, § 4.4):
- **OpenAI.**
  - The payload must be an object with a `choices` array of objects. Each choice needs an integer `index` and an object `message`. A `delta` key is malformed.
  - `message.content` is absent, `null`, a string or an array.
  - `message.tool_calls` is absent, `null`, `[]`, or an array of objects. Each entry has:
    - `type` absent, `null` or a string;
    - `function{name, arguments}` or `custom{name, input}`, but not both.
  - A `function`/`custom` entry with no name is malformed. A known type with the other body is malformed. An unknown type is Other, whose arguments are the compact JSON of the whole entry.
  - A legacy `function_call` object is position 0. `tool_calls` and `function_call` together are malformed.
  - Positions are the array indices.
- **OpenAI rewrite.**
  - Remove the blocked entries, and `function_call` when it is blocked. Drop `tool_calls` when it becomes empty. Strip `logprobs`.
  - Append the notice to `message.content`:
    - a non-empty string gets `\n\n` + notice;
    - an empty string or `null` becomes the notice;
    - an array gets `{"type":"text","text":notice}` pushed.
  - `finish_reason` `tool_calls` or `function_call` becomes `stop` when no call of that choice remains.
- **Anthropic.**
  - The payload must be an object with a `content` array of objects that each carry a `type`.
  - Positions are the array indices. A `tool_use` block needs a string `name` and an object `input`; its arguments are the compact JSON of `input`.
  - `text`, `thinking` and `redacted_thinking` are content. Server types are reported as unenforceable. Any other type is Other, with the compact JSON of the block.
- **Anthropic rewrite.** A blocked block becomes `{"type":"text","text":notice}`. `stop_reason` `tool_use` becomes `end_turn` when no ToolUse or Other block remains.
- **Size.** A body over `max_bytes` is `Malformed("too_large")` (§ 5.2).
- **Decision length.** `apply` treats a missing decision as `Block(DeciderError)` (D18). The caller still runs the length check, for the report.

- [ ] **Step 1: Write the failing tests** (in `src/body.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{BlockCode, Decision, ToolCallKind};
    use serde_json::{Value, json};

    const MAX: usize = 1 << 20;
    fn block() -> Decision {
        Decision::Block { code: BlockCode::Policy }
    }
    fn openai(message: Value, finish: &str) -> Vec<u8> {
        json!({"id":"chatcmpl-1","object":"chat.completion","created":1,"model":"m","choices":[{"index":0,"message":message,"logprobs":{"content":[{"token":"SECRET"}]},"finish_reason":finish}],"usage":{"total_tokens":3}}).to_string().into_bytes()
    }
    fn two_calls() -> Vec<u8> {
        openai(
            json!({"role":"assistant","content":null,"tool_calls":[
                {"id":"c0","type":"function","function":{"name":"read","arguments":"{}"}},
                {"id":"c1","type":"function","function":{"name":"rm","arguments":"SECRET"}}
            ]}),
            "tool_calls",
        )
    }

    #[test]
    fn openai_calls_are_extracted_in_position_order() {
        let b = parse_body(Format::OpenAiChat, &two_calls(), MAX).unwrap();
        assert_eq!(b.calls.iter().map(|c| (c.position, c.name.as_str(), c.kind)).collect::<Vec<_>>(), [(0, "read", ToolCallKind::Function), (1, "rm", ToolCallKind::Function)]);
        assert_eq!(b.calls[1].arguments, "SECRET");
    }

    #[test]
    fn allow_all_forwards_the_original_bytes() {
        let b = parse_body(Format::OpenAiChat, &two_calls(), MAX).unwrap();
        assert!(b.apply(&[Decision::Allow, Decision::Allow]).is_none());
    }

    #[test]
    fn a_blocked_openai_call_is_removed_and_noticed() {
        let b = parse_body(Format::OpenAiChat, &two_calls(), MAX).unwrap();
        let out = String::from_utf8(b.apply(&[Decision::Allow, block()]).unwrap()).unwrap();
        assert!(!out.contains("SECRET"), "{out}");
        let v: Value = serde_json::from_str(&out).unwrap();
        let m = &v["choices"][0]["message"];
        assert_eq!(m["tool_calls"].as_array().unwrap().len(), 1);
        assert_eq!(m["content"], "[paigasus] tool call \"rm\" was blocked (policy).");
        assert_eq!(v["choices"][0]["finish_reason"], "tool_calls");
        assert!(v["choices"][0].get("logprobs").is_none());
    }

    #[test]
    fn blocking_every_openai_call_sets_stop_and_appends_to_text() {
        let body = openai(json!({"role":"assistant","content":"I will.","tool_calls":[{"id":"c0","type":"function","function":{"name":"rm","arguments":"SECRET"}}]}), "tool_calls");
        let out = parse_body(Format::OpenAiChat, &body, MAX).unwrap().apply(&[block()]).unwrap();
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert!(v["choices"][0]["message"].get("tool_calls").is_none());
        assert_eq!(v["choices"][0]["message"]["content"], "I will.\n\n[paigasus] tool call \"rm\" was blocked (policy).");
        assert_eq!(v["choices"][0]["finish_reason"], "stop");
    }

    #[test]
    fn a_legacy_call_and_array_content_are_handled() {
        let body = openai(json!({"role":"assistant","content":[{"type":"text","text":"x"}],"function_call":{"name":"f","arguments":"SECRET"}}), "function_call");
        let b = parse_body(Format::OpenAiChat, &body, MAX).unwrap();
        assert_eq!(b.calls[0].kind, ToolCallKind::LegacyFunctionCall);
        let v: Value = serde_json::from_slice(&b.apply(&[block()]).unwrap()).unwrap();
        assert!(v["choices"][0]["message"].get("function_call").is_none());
        assert_eq!(v["choices"][0]["message"]["content"][1]["type"], "text");
        assert_eq!(v["choices"][0]["finish_reason"], "stop");
    }

    #[test]
    fn a_short_decision_vector_blocks_the_rest() {
        let b = parse_body(Format::OpenAiChat, &two_calls(), MAX).unwrap();
        let out = String::from_utf8(b.apply(&[Decision::Allow]).unwrap()).unwrap();
        assert!(!out.contains("SECRET") && out.contains("(decider-error)"));
    }

    #[test]
    fn malformed_openai_bodies_are_rejected() {
        for bad in [
            json!({"choices":[{"index":0,"delta":{}}]}),
            json!({"choices":[{"index":0,"message":{"tool_calls":{"0":{}}}}]}),
            json!({"choices":[{"index":0,"message":{"tool_calls":[{"type":"function","function":{"arguments":"{}"}}]}}]}),
            json!({"choices":[{"index":0,"message":{"tool_calls":[{"type":"function","function":{"name":"a"}}],"function_call":{"name":"b"}}}]}),
            json!({"choices":[{"index":0,"message":{"content":1}}]}),
            json!({"choices":null}),
        ] {
            assert!(parse_body(Format::OpenAiChat, bad.to_string().as_bytes(), MAX).is_err(), "{bad}");
        }
        assert!(parse_body(Format::OpenAiChat, br#"{"choices":[],"a":1,"a":2}"#, MAX).is_err());
        assert!(parse_body(Format::OpenAiChat, &two_calls(), 10).is_err(), "too large");
    }

    fn anthropic(content: Value) -> Vec<u8> {
        json!({"id":"msg_1","type":"message","role":"assistant","model":"claude","content":content,"stop_reason":"tool_use"}).to_string().into_bytes()
    }

    #[test]
    fn an_anthropic_tool_use_is_extracted_and_blocked() {
        let body = anthropic(json!([{"type":"text","text":"Ok."},{"type":"tool_use","id":"t1","name":"rm","input":{"path":"SECRET"}}]));
        let b = parse_body(Format::AnthropicMessages, &body, MAX).unwrap();
        assert_eq!((b.calls[0].position, b.calls[0].kind, b.calls[0].arguments.as_str()), (1, ToolCallKind::AnthropicToolUse, r#"{"path":"SECRET"}"#));
        let v: Value = serde_json::from_slice(&b.apply(&[block()]).unwrap()).unwrap();
        assert_eq!(v["content"][1]["type"], "text");
        assert!(!v.to_string().contains("SECRET"));
        assert_eq!(v["stop_reason"], "end_turn");
    }

    #[test]
    fn anthropic_server_tools_are_reported_not_decided() {
        let body = anthropic(json!([{"type":"server_tool_use","id":"s","name":"web_search","input":{}},{"type":"web_search_tool_result","tool_use_id":"s","content":[]}]));
        let b = parse_body(Format::AnthropicMessages, &body, MAX).unwrap();
        assert!(b.calls.is_empty());
        assert_eq!(b.unenforceable.len(), 2);
        assert!(b.report(&[], Basis::Decider).contains("unenforceable"));
    }

    #[test]
    fn the_report_lists_each_decision() {
        let b = parse_body(Format::OpenAiChat, &two_calls(), MAX).unwrap();
        let h: Vec<Value> = serde_json::from_str(&b.report(&[Decision::Allow, block()], Basis::Decider)).unwrap();
        assert_eq!(h.len(), 2);
        assert_eq!((h[0]["decision"].as_str(), h[1]["decision"].as_str(), h[1]["code"].as_str()), (Some("allow"), Some("block"), Some("policy")));
    }
}
```

- [ ] **Step 2: Run them to verify they fail**

```bash
cd rs && cargo nextest run --locked -p paigasus-gateway-core body::
```

Expected: FAIL (compile errors).

- [ ] **Step 3: Implement `src/body.rs`**

```rust
// SPDX-License-Identifier: Apache-2.0
//! Non-stream bodies (spec § 4.2, § 4.4, § 5.2). The same strict parse and the same rules as the
//! streams, applied to `choices[].message` (OpenAI) or `content[]` (Anthropic).

use serde_json::Value;

use crate::json::{Malformed, parse_strict};
use crate::report::{ReportDecision, ReportEntry, header_value};
use crate::types::{Basis, BlockCode, Decision, Format, ToolCall};
use crate::{anthropic, openai};

pub struct BodyCalls {
    format: Format,
    value: Value,
    pub calls: Vec<ToolCall>,
    pub unenforceable: Vec<ReportEntry>,
}

pub fn parse_body(format: Format, body: &[u8], max_bytes: usize) -> Result<BodyCalls, Malformed> {
    if body.len() > max_bytes {
        return Err(Malformed("too_large"));
    }
    let value = parse_strict(body)?;
    let (calls, unenforceable) = match format {
        Format::OpenAiChat => (openai::body::calls(&value)?, Vec::new()),
        Format::AnthropicMessages => anthropic::body::calls(&value)?,
    };
    Ok(BodyCalls { format, value, calls, unenforceable })
}

impl BodyCalls {
    /// The block code of call `i`. A missing decision is a block (D18).
    fn code(decisions: &[Decision], i: usize) -> Option<BlockCode> {
        match decisions.get(i) {
            Some(Decision::Block { code }) => Some(*code),
            Some(_) => None,
            None => Some(BlockCode::DeciderError),
        }
    }

    #[must_use]
    pub fn apply(self, decisions: &[Decision]) -> Option<Vec<u8>> {
        let codes: Vec<Option<BlockCode>> = (0..self.calls.len()).map(|i| Self::code(decisions, i)).collect();
        if codes.iter().all(Option::is_none) {
            return None;
        }
        let mut value = self.value;
        match self.format {
            Format::OpenAiChat => openai::body::rewrite(&mut value, &self.calls, &codes),
            Format::AnthropicMessages => anthropic::body::rewrite(&mut value, &self.calls, &codes),
        }
        Some(serde_json::to_vec(&value).expect("a Value always serializes"))
    }

    #[must_use]
    pub fn report(&self, decisions: &[Decision], basis: Basis) -> String {
        let mut entries: Vec<ReportEntry> = self
            .calls
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let code = Self::code(decisions, i);
                let decision = if code.is_some() { ReportDecision::Block } else { ReportDecision::Allow };
                ReportEntry { choice: c.choice, position: c.position, name: c.name.clone(), decision, basis: Some(basis), code }
            })
            .collect();
        entries.extend(self.unenforceable.iter().cloned());
        header_value(&entries)
    }
}
```

In `src/json.rs`, make `Malformed` `pub` (it was `pub(crate)`). In `src/lib.rs` add `mod body;`, `pub use body::{BodyCalls, parse_body};` and `pub use json::Malformed;`. In `src/openai/mod.rs` and `src/anthropic/mod.rs` add `pub(crate) mod body;`.

- [ ] **Step 4: Implement `src/openai/body.rs`**

```rust
// SPDX-License-Identifier: Apache-2.0
//! OpenAI non-stream bodies (spec § 4.2, § 4.4).

use serde_json::{Value, json};

use crate::json::{MAX_OPENAI_INDEX, Malformed, index_of, opt_obj, opt_str};
use crate::openai::classify::name_args;
use crate::report::notice_line;
use crate::types::{BlockCode, ToolCall, ToolCallKind};

pub(crate) fn calls(v: &Value) -> Result<Vec<ToolCall>, Malformed> {
    let Value::Object(top) = v else { return Err(Malformed("not_object")) };
    let Some(Value::Array(choices)) = top.get("choices") else { return Err(Malformed("choices")) };
    let mut out = Vec::new();
    for ch in choices {
        let Value::Object(c) = ch else { return Err(Malformed("choice")) };
        if c.contains_key("delta") {
            return Err(Malformed("delta_in_body"));
        }
        let choice = index_of(c.get("index"), MAX_OPENAI_INDEX)?;
        let Some(Value::Object(m)) = c.get("message") else { return Err(Malformed("message")) };
        match m.get("content") {
            None | Some(Value::Null | Value::String(_) | Value::Array(_)) => {}
            Some(_) => return Err(Malformed("content")),
        }
        let entries: &[Value] = match m.get("tool_calls") {
            None | Some(Value::Null) => &[],
            Some(Value::Array(a)) => a,
            Some(_) => return Err(Malformed("tool_calls")),
        };
        let legacy = opt_obj(m.get("function_call"), "function_call")?;
        if legacy.is_some() && !entries.is_empty() {
            return Err(Malformed("legacy_and_tool_calls"));
        }
        if let Some(f) = legacy {
            let na = name_args(f, "arguments")?;
            out.push(ToolCall { choice, position: 0, id: None, name: na.name.ok_or(Malformed("name"))?, kind: ToolCallKind::LegacyFunctionCall, arguments: na.args.unwrap_or_default() });
        }
        for (i, e) in entries.iter().enumerate() {
            let Value::Object(o) = e else { return Err(Malformed("tool_call")) };
            let id = opt_str(o.get("id"), "id")?;
            let ty = opt_str(o.get("type"), "type")?;
            let function = opt_obj(o.get("function"), "function")?.map(|f| name_args(f, "arguments")).transpose()?;
            let custom = opt_obj(o.get("custom"), "custom")?.map(|f| name_args(f, "input")).transpose()?;
            let (kind, na) = match (ty.as_deref(), function, custom) {
                (_, Some(_), Some(_)) => return Err(Malformed("function_and_custom")),
                (Some("function") | None, Some(na), None) => (ToolCallKind::Function, Some(na)),
                (Some("custom") | None, None, Some(na)) => (ToolCallKind::Custom, Some(na)),
                (Some("function" | "custom"), _, _) => return Err(Malformed("type_shape_mismatch")),
                _ => (ToolCallKind::Other, None),
            };
            let (name, arguments) = match na {
                Some(na) => (na.name.ok_or(Malformed("name"))?, na.args.unwrap_or_default()),
                None => (ty.clone().unwrap_or_else(|| "unknown".to_owned()), e.to_string()),
            };
            let position = u32::try_from(i).map_err(|_| Malformed("too_many_calls"))?;
            out.push(ToolCall { choice, position, id, name, kind, arguments });
        }
    }
    Ok(out)
}

pub(crate) fn rewrite(v: &mut Value, calls: &[ToolCall], codes: &[Option<BlockCode>]) {
    let Some(Value::Array(choices)) = v.get_mut("choices") else { return };
    for ch in choices {
        let idx = ch.get("index").and_then(Value::as_u64).and_then(|i| u32::try_from(i).ok()).unwrap_or(0);
        let mine: Vec<(&ToolCall, Option<BlockCode>)> = calls.iter().zip(codes).filter(|(c, _)| c.choice == idx).map(|(c, k)| (c, *k)).collect();
        if mine.iter().all(|(_, k)| k.is_none()) {
            continue;
        }
        let notice: String = mine.iter().filter_map(|(c, k)| k.map(|k| notice_line(&c.name, k))).collect::<Vec<_>>().join("\n");
        let remaining = mine.iter().filter(|(_, k)| k.is_none()).count();
        let blocked: Vec<u32> = mine.iter().filter(|(_, k)| k.is_some()).map(|(c, _)| c.position).collect();
        if let Value::Object(o) = ch {
            o.remove("logprobs");
        }
        if let Some(Value::Object(m)) = ch.get_mut("message") {
            if let Some(Value::Array(tc)) = m.get_mut("tool_calls") {
                let mut pos = 0u32;
                tc.retain(|_| {
                    let keep = !blocked.contains(&pos);
                    pos += 1;
                    keep
                });
                if tc.is_empty() {
                    m.remove("tool_calls");
                }
            }
            if m.contains_key("function_call") && blocked.contains(&0) {
                m.remove("function_call");
            }
            let content = m.remove("content").unwrap_or(Value::Null);
            let new = match content {
                Value::String(s) if !s.is_empty() => Value::String(format!("{s}\n\n{notice}")),
                Value::Array(mut parts) => {
                    parts.push(json!({"type": "text", "text": notice}));
                    Value::Array(parts)
                }
                _ => Value::String(notice),
            };
            m.insert("content".to_owned(), new);
        }
        let finish = ch.get("finish_reason").and_then(Value::as_str).map(str::to_owned);
        if remaining == 0 && matches!(finish.as_deref(), Some("tool_calls" | "function_call")) {
            ch["finish_reason"] = "stop".into();
        }
    }
}
```

- [ ] **Step 5: Implement `src/anthropic/body.rs`**

```rust
// SPDX-License-Identifier: Apache-2.0
//! Anthropic non-stream bodies (spec § 4.2, § 4.4).

use serde_json::{Value, json};

use crate::json::Malformed;
use crate::report::{ReportDecision, ReportEntry, notice_line};
use crate::types::{BlockCode, ToolCall, ToolCallKind};

pub(crate) fn calls(v: &Value) -> Result<(Vec<ToolCall>, Vec<ReportEntry>), Malformed> {
    let Some(Value::Array(content)) = v.get("content") else { return Err(Malformed("content")) };
    let (mut calls, mut reports) = (Vec::new(), Vec::new());
    for (i, b) in content.iter().enumerate() {
        let position = u32::try_from(i).map_err(|_| Malformed("too_many_blocks"))?;
        let Value::Object(o) = b else { return Err(Malformed("block")) };
        let ty = o.get("type").and_then(Value::as_str).ok_or(Malformed("block_type"))?;
        let id = crate::json::opt_str(o.get("id"), "id")?;
        match ty {
            "text" | "thinking" | "redacted_thinking" => {}
            "server_tool_use" | "mcp_tool_use" => reports.push(server(o, ty, position)),
            t if t.ends_with("_tool_result") => reports.push(server(o, ty, position)),
            "tool_use" => {
                let name = o.get("name").and_then(Value::as_str).ok_or(Malformed("name"))?.to_owned();
                let Some(input @ Value::Object(_)) = o.get("input") else { return Err(Malformed("input")) };
                calls.push(ToolCall { choice: 0, position, id, name, kind: ToolCallKind::AnthropicToolUse, arguments: input.to_string() });
            }
            _ => {
                let name = o.get("name").and_then(Value::as_str).unwrap_or(ty).to_owned();
                calls.push(ToolCall { choice: 0, position, id, name, kind: ToolCallKind::Other, arguments: b.to_string() });
            }
        }
    }
    Ok((calls, reports))
}

fn server(o: &serde_json::Map<String, Value>, ty: &str, position: u32) -> ReportEntry {
    let name = o.get("name").and_then(Value::as_str).unwrap_or(ty).to_owned();
    ReportEntry { choice: 0, position, name, decision: ReportDecision::Unenforceable, basis: None, code: None }
}

pub(crate) fn rewrite(v: &mut Value, calls: &[ToolCall], codes: &[Option<BlockCode>]) {
    let remaining = codes.iter().filter(|k| k.is_none()).count();
    if let Some(Value::Array(content)) = v.get_mut("content") {
        for (c, code) in calls.iter().zip(codes) {
            if let (Some(code), Some(slot)) = (code, content.get_mut(c.position as usize)) {
                *slot = json!({"type": "text", "text": notice_line(&c.name, *code)});
            }
        }
    }
    if remaining == 0 && v.get("stop_reason").and_then(Value::as_str) == Some("tool_use") {
        v["stop_reason"] = "end_turn".into();
    }
}
```

- [ ] **Step 6: Run the tests**

```bash
cd rs && cargo fmt && cargo nextest run --locked -p paigasus-gateway-core && cargo clippy --locked -p paigasus-gateway-core --all-targets -- -D warnings
```

Expected: every test passes (10 new ones in `body::`), with no warnings.

- [ ] **Step 7: Commit**

```bash
git add rs/crates/libs/paigasus-gateway-core
git commit -m "feat(rs): extract and rewrite tool calls in non-stream bodies (SMA-749)" -m "The strict parse and allowlist applied to chat.completion and message
bodies. Allow-all returns None so the original bytes are forwarded; a
block re-serializes once, with the notice and a fixed finish reason.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 10: The request turn and the decider interface

**Files:**
- Create: `rs/crates/libs/paigasus-gateway-core/src/turn.rs`, `src/decider.rs`
- Modify: `src/lib.rs`

**Interfaces:**
- Consumes: `parse_strict`, `ToolCallKind`, `ToolCall`, `Decision`, `DeciderError` (Task 8), `Format`.
- Produces (`pub`, re-exported):
  - `enum ToolTurn { Parsed(TurnFacts), Unavailable(&'static str) }` (`#[non_exhaustive]`)
  - `struct TurnFacts { definitions: Vec<ToolDefinition>, prior_calls: Vec<PriorCall>, tool_results: Vec<ToolResultRef> }`
  - `struct ToolDefinition { name: String, kind: ToolCallKind, schema: Option<serde_json::Value> }`
  - `struct PriorCall { id: Option<String>, name: String, arguments: String }`
  - `struct ToolResultRef { call_id: Option<String>, name: Option<String> }`. It never holds content.
  - `fn parse_request(format: Format, body: &[u8]) -> ToolTurn`
  - `struct DecisionContext { principal: String, scope: String, request_id: Option<String> }` (`#[non_exhaustive]`) with `fn new(principal, scope, request_id) -> Self`
  - `type DecideFuture = Pin<Box<dyn Future<Output = Result<Vec<Decision>, DeciderError>> + Send + 'static>>`
  - `trait ToolCallDecider: Send + Sync + 'static { fn decide(&self, ctx: Arc<DecisionContext>, turn: Arc<ToolTurn>, calls: Vec<ToolCall>) -> DecideFuture; }`
  - `struct AllowAll` (`Default`, `Clone`, `Copy`), which implements `ToolCallDecider`.

Request rules (D19):
- **OpenAI.**
  - `tools[]` entries of type `function` give `{name: function.name, kind: Function, schema: function.parameters}`. Entries of type `custom` give `{name: custom.name, kind: Custom}`.
  - Legacy `functions[]` entries give `{name, kind: LegacyFunctionCall, schema: parameters}`.
  - In `messages[]`:
    - each entry of an assistant's `tool_calls` is a `PriorCall`, and an assistant's `function_call` is a `PriorCall` with no id;
    - a `role:"tool"` message is a `ToolResultRef{call_id: tool_call_id}`, and a `role:"function"` message is a `ToolResultRef{name}`.
- **Anthropic.**
  - `tools[]` entries with no `type`, or with type `custom`, give `{name, kind: AnthropicToolUse, schema: input_schema}`. Other typed tools are server tools and are skipped.
  - In `messages[].content` arrays, a `tool_use` block is a `PriorCall` whose arguments are the compact JSON of its `input`. A `tool_result` block is a `ToolResultRef{call_id: tool_use_id}`.
- **Failure.** A body that fails strict parsing, or whose fields have the wrong types, is `Unavailable("malformed")`. It is never an empty `Parsed`.

- [ ] **Step 1: Write the failing tests**

`src/turn.rs` tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn facts(format: Format, v: serde_json::Value) -> TurnFacts {
        match parse_request(format, v.to_string().as_bytes()) {
            ToolTurn::Parsed(f) => f,
            ToolTurn::Unavailable(r) => panic!("unavailable: {r}"),
        }
    }

    #[test]
    fn an_openai_request_yields_definitions_calls_and_results() {
        let f = facts(
            Format::OpenAiChat,
            json!({"model":"m","tools":[{"type":"function","function":{"name":"read","parameters":{"type":"object"}}},{"type":"custom","custom":{"name":"sql"}}],
                   "functions":[{"name":"old","parameters":{}}],
                   "messages":[{"role":"user","content":"hi"},
                               {"role":"assistant","content":null,"tool_calls":[{"id":"c1","type":"function","function":{"name":"read","arguments":"{}"}}]},
                               {"role":"tool","tool_call_id":"c1","content":"SECRET RESULT"},
                               {"role":"assistant","function_call":{"name":"old","arguments":"{}"}},
                               {"role":"function","name":"old","content":"SECRET"}]}),
        );
        assert_eq!(f.definitions.iter().map(|d| (d.name.as_str(), d.kind)).collect::<Vec<_>>(), [("read", ToolCallKind::Function), ("sql", ToolCallKind::Custom), ("old", ToolCallKind::LegacyFunctionCall)]);
        assert_eq!(f.prior_calls.len(), 2);
        assert_eq!(f.tool_results, vec![ToolResultRef { call_id: Some("c1".into()), name: None }, ToolResultRef { call_id: None, name: Some("old".into()) }]);
        assert!(!format!("{f:?}").contains("SECRET"), "tool result content is never kept");
    }

    #[test]
    fn an_anthropic_request_yields_definitions_calls_and_results() {
        let f = facts(
            Format::AnthropicMessages,
            json!({"model":"c","tools":[{"name":"rm","input_schema":{"type":"object"}},{"type":"web_search_20250305","name":"web_search"}],
                   "messages":[{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"rm","input":{"p":"/"}}]},
                               {"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"SECRET"}]}]}),
        );
        assert_eq!(f.definitions.len(), 1);
        assert_eq!(f.prior_calls[0].arguments, r#"{"p":"/"}"#);
        assert_eq!(f.tool_results[0].call_id.as_deref(), Some("t1"));
    }

    #[test]
    fn a_malformed_request_is_unavailable_never_empty() {
        for bad in [&b"{"[..], br#"{"a":1,"a":2}"#, br#"{"messages":"x"}"#, br#"{"tools":[{"type":"function","function":{"name":1}}]}"#] {
            assert!(matches!(parse_request(Format::OpenAiChat, bad), ToolTurn::Unavailable(_)), "{}", String::from_utf8_lossy(bad));
        }
    }
}
```

`src/decider.rs` tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ToolCall, ToolCallKind};

    #[test]
    fn allow_all_allows_every_call() {
        let call = ToolCall { choice: 0, position: 0, id: None, name: "x".into(), kind: ToolCallKind::Function, arguments: String::new() };
        let fut = AllowAll.decide(Arc::new(DecisionContext::new("p", "s", None)), Arc::new(ToolTurn::Unavailable("test")), vec![call.clone(), call]);
        // The future is ready at once; poll it without a runtime.
        let waker = std::task::Waker::noop();
        let mut cx = std::task::Context::from_waker(waker);
        let mut fut = fut;
        match fut.as_mut().poll(&mut cx) {
            std::task::Poll::Ready(Ok(d)) => assert_eq!(d, vec![Decision::Allow, Decision::Allow]),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_decider_is_object_safe() {
        let d: Arc<dyn ToolCallDecider> = Arc::new(AllowAll);
        let _ = d;
    }
}
```

`std::task::Waker::noop` is stable since 1.85.

- [ ] **Step 2: Run them to verify they fail**

```bash
cd rs && cargo nextest run --locked -p paigasus-gateway-core turn:: decider::
```

Expected: FAIL (compile errors).

- [ ] **Step 3: Implement `src/turn.rs`**

```rust
// SPDX-License-Identifier: Apache-2.0
//! The request side of a turn (spec D19): what tools the caller declared, what it called before,
//! and which results it sent back. Never the content of a tool result. No request rewrite.

use serde_json::{Map, Value};

use crate::json::{Malformed, opt_obj, opt_str, parse_strict};
use crate::types::{Format, ToolCallKind};

#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub enum ToolTurn {
    Parsed(TurnFacts),
    /// The request could not be read. Sequence rules (SMA-753) must treat this as unknown history,
    /// never as an empty one.
    Unavailable(&'static str),
}

#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TurnFacts {
    pub definitions: Vec<ToolDefinition>,
    pub prior_calls: Vec<PriorCall>,
    pub tool_results: Vec<ToolResultRef>,
}

#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct ToolDefinition {
    pub name: String,
    pub kind: ToolCallKind,
    pub schema: Option<Value>,
}

#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PriorCall {
    pub id: Option<String>,
    pub name: String,
    pub arguments: String,
}

#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolResultRef {
    pub call_id: Option<String>,
    pub name: Option<String>,
}

#[must_use]
pub fn parse_request(format: Format, body: &[u8]) -> ToolTurn {
    let parsed = parse_strict(body).and_then(|v| match format {
        Format::OpenAiChat => openai(&v),
        Format::AnthropicMessages => anthropic(&v),
    });
    match parsed {
        Ok(f) => ToolTurn::Parsed(f),
        Err(_) => ToolTurn::Unavailable("malformed"),
    }
}

fn arr<'a>(o: &'a Map<String, Value>, k: &'static str) -> Result<&'a [Value], Malformed> {
    match o.get(k) {
        None | Some(Value::Null) => Ok(&[]),
        Some(Value::Array(a)) => Ok(a),
        Some(_) => Err(Malformed(k)),
    }
}

fn obj(v: &Value) -> Result<&Map<String, Value>, Malformed> {
    v.as_object().ok_or(Malformed("object"))
}

fn req_str(o: &Map<String, Value>, k: &'static str) -> Result<String, Malformed> {
    opt_str(o.get(k), k)?.ok_or(Malformed(k))
}

fn openai(v: &Value) -> Result<TurnFacts, Malformed> {
    let top = obj(v)?;
    let mut f = TurnFacts::default();
    for t in arr(top, "tools")? {
        let t = obj(t)?;
        match opt_str(t.get("type"), "type")?.as_deref() {
            Some("function") => {
                let func = opt_obj(t.get("function"), "function")?.ok_or(Malformed("function"))?;
                f.definitions.push(ToolDefinition { name: req_str(func, "name")?, kind: ToolCallKind::Function, schema: func.get("parameters").cloned() });
            }
            Some("custom") => {
                let c = opt_obj(t.get("custom"), "custom")?.ok_or(Malformed("custom"))?;
                f.definitions.push(ToolDefinition { name: req_str(c, "name")?, kind: ToolCallKind::Custom, schema: None });
            }
            _ => {}
        }
    }
    for d in arr(top, "functions")? {
        let d = obj(d)?;
        f.definitions.push(ToolDefinition { name: req_str(d, "name")?, kind: ToolCallKind::LegacyFunctionCall, schema: d.get("parameters").cloned() });
    }
    for m in arr(top, "messages")? {
        let m = obj(m)?;
        match opt_str(m.get("role"), "role")?.as_deref() {
            Some("assistant") => {
                for c in arr(m, "tool_calls")? {
                    let c = obj(c)?;
                    let func = opt_obj(c.get("function"), "function")?;
                    let name = func.map(|f| req_str(f, "name")).transpose()?.unwrap_or_default();
                    let arguments = func.map(|f| opt_str(f.get("arguments"), "arguments")).transpose()?.flatten().unwrap_or_default();
                    f.prior_calls.push(PriorCall { id: opt_str(c.get("id"), "id")?, name, arguments });
                }
                if let Some(fc) = opt_obj(m.get("function_call"), "function_call")? {
                    f.prior_calls.push(PriorCall { id: None, name: req_str(fc, "name")?, arguments: opt_str(fc.get("arguments"), "arguments")?.unwrap_or_default() });
                }
            }
            Some("tool") => f.tool_results.push(ToolResultRef { call_id: opt_str(m.get("tool_call_id"), "tool_call_id")?, name: None }),
            Some("function") => f.tool_results.push(ToolResultRef { call_id: None, name: opt_str(m.get("name"), "name")? }),
            _ => {}
        }
    }
    Ok(f)
}

fn anthropic(v: &Value) -> Result<TurnFacts, Malformed> {
    let top = obj(v)?;
    let mut f = TurnFacts::default();
    for t in arr(top, "tools")? {
        let t = obj(t)?;
        match opt_str(t.get("type"), "type")?.as_deref() {
            None | Some("custom") => f.definitions.push(ToolDefinition { name: req_str(t, "name")?, kind: ToolCallKind::AnthropicToolUse, schema: t.get("input_schema").cloned() }),
            Some(_) => {} // a server tool
        }
    }
    for m in arr(top, "messages")? {
        let m = obj(m)?;
        let Some(Value::Array(blocks)) = m.get("content") else { continue };
        for b in blocks {
            let b = obj(b)?;
            match opt_str(b.get("type"), "type")?.as_deref() {
                Some("tool_use") => f.prior_calls.push(PriorCall {
                    id: opt_str(b.get("id"), "id")?,
                    name: req_str(b, "name")?,
                    arguments: b.get("input").map(Value::to_string).unwrap_or_default(),
                }),
                Some("tool_result") => f.tool_results.push(ToolResultRef { call_id: opt_str(b.get("tool_use_id"), "tool_use_id")?, name: None }),
                _ => {}
            }
        }
    }
    Ok(f)
}
```

`TurnFacts` derives `Default` but is `#[non_exhaustive]`, so callers outside the crate cannot build it with a literal. That is intended: only `parse_request` builds one.

- [ ] **Step 4: Implement `src/decider.rs`**

```rust
// SPDX-License-Identifier: Apache-2.0
//! The decider port (spec D18). SMA-752 adds the Cedar decider behind the same trait.
//!
//! The arguments are owned (`Arc`s and a `Vec`), and the future is `'static`, so the gateway can
//! keep a pending decision inside its relay state without borrowing the interceptor.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::interceptor::DeciderError;
use crate::turn::ToolTurn;
use crate::types::{Decision, ToolCall};

/// Who is asking, captured in the handler. Inside the stream `current_ids()` is `None`
/// (SMA-504 § 4.3), so the request id comes in here.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionContext {
    pub principal: String,
    pub scope: String,
    pub request_id: Option<String>,
}

impl DecisionContext {
    #[must_use]
    pub fn new(principal: impl Into<String>, scope: impl Into<String>, request_id: Option<String>) -> Self {
        DecisionContext { principal: principal.into(), scope: scope.into(), request_id }
    }
}

pub type DecideFuture = Pin<Box<dyn Future<Output = Result<Vec<Decision>, DeciderError>> + Send + 'static>>;

/// Decides one completed hold. MUST return exactly one decision per call, in order: the
/// interceptor treats any other length as a decider error (D18).
pub trait ToolCallDecider: Send + Sync + 'static {
    fn decide(&self, ctx: Arc<DecisionContext>, turn: Arc<ToolTurn>, calls: Vec<ToolCall>) -> DecideFuture;
}

/// The only decider in SMA-749.
#[derive(Debug, Clone, Copy, Default)]
pub struct AllowAll;

impl ToolCallDecider for AllowAll {
    fn decide(&self, _ctx: Arc<DecisionContext>, _turn: Arc<ToolTurn>, calls: Vec<ToolCall>) -> DecideFuture {
        let n = calls.len();
        Box::pin(std::future::ready(Ok(vec![Decision::Allow; n])))
    }
}
```

`src/lib.rs`: add `mod decider;`, `mod turn;`, `pub use decider::{AllowAll, DecideFuture, DecisionContext, ToolCallDecider};` and `pub use turn::{PriorCall, ToolDefinition, ToolResultRef, ToolTurn, TurnFacts, parse_request};`.

- [ ] **Step 5: Run the tests**

```bash
cd rs && cargo fmt && cargo nextest run --locked -p paigasus-gateway-core && cargo clippy --locked -p paigasus-gateway-core --all-targets -- -D warnings
```

Expected: every test passes, with no warnings.

- [ ] **Step 6: Commit**

```bash
git add rs/crates/libs/paigasus-gateway-core
git commit -m "feat(rs): add the request turn and the tool-call decider port (SMA-749)" -m "ToolTurn reads tool definitions, earlier calls and result references
from OpenAI and Anthropic requests, and is Unavailable rather than
empty on a bad body. The decider takes owned arguments and returns a
static future; AllowAll is the only implementation.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---
### Task 11: The differential oracle, the generators and the fixture replays

This task is the security proof of the whole feature (spec § 8.1, AC 2–5). Each Rust model below reproduces how one client SDK turns stream bytes into tool calls. The property tests generate hostile upstream streams, run them through the interceptor, and require every call that any model derives from the output to be one the decider allowed.

**Files:**
- Create: `rs/crates/libs/paigasus-gateway-core/tests/support/mod.rs`, `tests/support/driver.rs`, `tests/support/oracle.rs`, `tests/support/gen.rs`
- Create: `tests/properties.rs`, `tests/fixtures.rs`, `tests/fixtures/make_fixtures.py`, `tests/fixtures/{openai,anthropic}/*.sse` (generated, committed)
- Modify: `rs/crates/libs/paigasus-gateway-core/Cargo.toml` (`[dev-dependencies]`)

**Interfaces:**
- Consumes: the crate's public API: `Interceptor`, `Action`, `Limits`, `OnFailure`, `Format`, `Decision`, `BlockCode`, `ToolCall`, `ToolCallKind`.
- Produces (test-only; used again by Task 12):
  - `support::driver::{Outcome, drive, block_named, TERMINAL}`:
    - `drive(format, limits, chunks: &[&[u8]], decide: &dyn Fn(&ToolCall) -> Decision) -> Outcome`
    - `struct Outcome { out: Vec<u8>, decided: Vec<(ToolCall, Decision)>, max_queued: usize }`
  - `support::oracle::{ModelCall, Model, MODELS, payloads}`:
    - `MODELS: &[(&str, Model)]`
    - `type Model = fn(&[u8]) -> Result<Vec<ModelCall>, String>`
    - `payloads(bytes) -> Vec<Vec<u8>>`, a reference WHATWG splitter that is independent of the crate's own.
  - `support::gen::{openai_scenario, anthropic_scenario, Scenario}`.

- [ ] **Step 1: Add the dev-dependencies**

```toml
[dev-dependencies]
proptest = { workspace = true }
serde_json = { workspace = true }
bytes = { workspace = true }
```

- [ ] **Step 2: Write the driver**

`tests/support/mod.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0
//! Shared test support for the paigasus-gateway-core integration tests (SMA-749 § 8).
#![allow(dead_code)] // Each test binary uses a different subset of this module.

pub mod driver;
pub mod gen;
pub mod oracle;
```

A test-support `allow(dead_code)` is the established pattern for shared `tests/support` modules: each test binary compiles the module separately. It is not production code.

`tests/support/driver.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0
//! Drives an `Interceptor` synchronously, answering each `Decide` at once.

use std::time::Instant;

use bytes::Bytes;
use paigasus_gateway_core::{Action, BlockCode, Decision, Format, Interceptor, Limits, OnFailure, ToolCall};

pub const TERMINAL: &[u8] = b"\n\ndata: {\"error\":{\"message\":\"terminal\"}}\n\n";

pub struct Outcome {
    pub out: Vec<u8>,
    pub decided: Vec<(ToolCall, Decision)>,
    pub max_queued: usize,
}

pub fn limits(on_failure: OnFailure) -> Limits {
    Limits { max_record_bytes: 4096, max_held_bytes: 16 * 1024, on_failure, ..Limits::defaults() }
}

pub fn block_named(c: &ToolCall) -> Decision {
    if c.name.starts_with("blocked_") { Decision::Block { code: BlockCode::Policy } } else { Decision::Allow }
}

pub fn allow_all(_: &ToolCall) -> Decision {
    Decision::Allow
}

pub fn drive(format: Format, limits: Limits, chunks: &[&[u8]], decide: &dyn Fn(&ToolCall) -> Decision) -> Outcome {
    let now = Instant::now();
    let mut i = Interceptor::new(format, limits, Bytes::from_static(TERMINAL), now);
    let mut o = Outcome { out: Vec::new(), decided: Vec::new(), max_queued: 0 };
    let mut pending = Vec::new();
    for c in chunks {
        pending.extend(i.push(c, now));
        run(&mut i, &mut pending, &mut o, decide, now);
    }
    pending.extend(i.finish(now));
    run(&mut i, &mut pending, &mut o, decide, now);
    assert!(i.is_finished(), "the interceptor must end once every decision is in");
    o
}

fn run(i: &mut Interceptor, pending: &mut Vec<Action>, o: &mut Outcome, decide: &dyn Fn(&ToolCall) -> Decision, now: Instant) {
    while !pending.is_empty() {
        o.max_queued = o.max_queued.max(i.queued_bytes());
        for a in std::mem::take(pending) {
            match a {
                Action::Emit(b) => o.out.extend_from_slice(&b),
                Action::Decide { hold, calls } => {
                    let ds: Vec<Decision> = calls.iter().map(decide).collect();
                    o.decided.extend(calls.iter().cloned().zip(ds.iter().copied()));
                    pending.extend(i.decided(hold, Ok(ds), now));
                }
                _ => {}
            }
        }
    }
    o.max_queued = o.max_queued.max(i.queued_bytes());
}

/// Split `bytes` at the given cut points, which are taken modulo the length and sorted.
pub fn cut<'a>(bytes: &'a [u8], cuts: &[usize]) -> Vec<&'a [u8]> {
    if bytes.is_empty() {
        return vec![bytes];
    }
    let mut at: Vec<usize> = cuts.iter().map(|c| c % bytes.len()).collect();
    at.sort_unstable();
    at.dedup();
    let mut out = Vec::new();
    let mut prev = 0;
    for a in at {
        out.push(&bytes[prev..a]);
        prev = a;
    }
    out.push(&bytes[prev..]);
    out
}
```

- [ ] **Step 3: Write the oracle**

`tests/support/oracle.rs`. Every model function carries a `SOURCE` doc line naming the SDK and the source location it reproduces. Task 18 pins the exact SDK versions and fills them in. Until then the version slot says `<pinned in Task 18>`.

```rust
// SPDX-License-Identifier: Apache-2.0
//! Rust models of how client SDKs turn stream bytes into tool calls (spec § 8.1).
//!
//! The models read the interceptor's OUTPUT. They reproduce each SDK's accumulation rules,
//! including the quirks that made the canonical form necessary (spec D9). A model returns `Err`
//! when the SDK would raise. The interceptor must never emit bytes that make a model fail.

use serde_json::{Map, Value};

/// One tool call a client would run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelCall {
    /// `function`, `custom`, `legacy`, `tool_use`, or `other`.
    pub kind: &'static str,
    pub name: String,
    pub arguments: String,
}

pub type Model = fn(&[u8]) -> Result<Vec<ModelCall>, String>;

pub const MODELS: &[(&str, Model)] = &[("openai-python", openai_python), ("openai-node", openai_node), ("langchain", langchain), ("anthropic-python", anthropic_python)];

/// A reference WHATWG SSE splitter, independent of the crate's: (event, data) per record.
pub fn records(bytes: &[u8]) -> Vec<(Option<String>, Vec<u8>)> {
    let text = String::from_utf8_lossy(bytes);
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text).replace("\r\n", "\n").replace('\r', "\n");
    let mut out = Vec::new();
    let (mut data, mut has, mut event): (Vec<u8>, bool, Option<String>) = (Vec::new(), false, None);
    for line in text.split('\n') {
        if line.is_empty() {
            if has {
                data.pop();
                if !data.is_empty() {
                    out.push((event.take(), std::mem::take(&mut data)));
                }
            }
            data.clear();
            has = false;
            event = None;
            continue;
        }
        if line.starts_with(':') {
            continue;
        }
        let (name, value) = match line.find(':') {
            Some(i) => (&line[..i], line[i + 1..].strip_prefix(' ').unwrap_or(&line[i + 1..])),
            None => (line, ""),
        };
        match name {
            "data" => {
                data.extend_from_slice(value.as_bytes());
                data.push(b'\n');
                has = true;
            }
            "event" => event = Some(value.to_owned()),
            _ => {}
        }
    }
    out
}

pub fn payloads(bytes: &[u8]) -> Vec<Vec<u8>> {
    records(bytes).into_iter().map(|(_, d)| d).collect()
}

fn json_records(bytes: &[u8]) -> Result<Vec<Value>, String> {
    let mut out = Vec::new();
    for (_, d) in records(bytes) {
        if d == b"[DONE]" {
            break; // every SDK stops at [DONE]
        }
        let v: Value = serde_json::from_slice(&d).map_err(|e| format!("unparseable record: {e}: {}", String::from_utf8_lossy(&d)))?;
        if v.get("error").is_some() {
            return Err("error record".into());
        }
        out.push(v);
    }
    Ok(out)
}

/// openai-python `accumulate_delta`.
/// SOURCE: openai-python <pinned in Task 18>, src/openai/lib/streaming/chat/_completions.py.
fn py_merge(acc: &mut Map<String, Value>, delta: &Map<String, Value>) -> Result<(), String> {
    for (k, dv) in delta {
        let Some(av) = acc.get_mut(k) else {
            acc.insert(k.clone(), dv.clone());
            continue;
        };
        if av.is_null() || k == "index" || k == "type" {
            *av = dv.clone();
            continue;
        }
        match (av, dv) {
            (Value::String(a), Value::String(b)) => a.push_str(b),
            (Value::Object(a), Value::Object(b)) => py_merge(a, b)?,
            (Value::Array(a), Value::Array(b)) => {
                for e in b {
                    let Some(eo) = e.as_object() else { return Err("list entry is not a dict".into()) };
                    let idx = eo.get("index").and_then(Value::as_i64).ok_or("list entry without index")?;
                    let len = i64::try_from(a.len()).expect("small");
                    let pos = if idx < 0 { idx + len } else { idx };
                    if pos >= 0 && pos < len {
                        let target = a[usize::try_from(pos).expect("in range")].as_object_mut().ok_or("not a dict")?;
                        py_merge(target, eo)?;
                    } else {
                        let at = usize::try_from(idx.clamp(0, len)).expect("clamped");
                        a.insert(at, e.clone());
                    }
                }
            }
            (a, b) => *a = b.clone(),
        }
    }
    Ok(())
}

fn calls_of_message(m: &Map<String, Value>) -> Vec<ModelCall> {
    let mut out = Vec::new();
    if let Some(Value::Array(tc)) = m.get("tool_calls") {
        for e in tc {
            let ty = e.get("type").and_then(Value::as_str).unwrap_or("function");
            let (kind, body, args_key): (&'static str, &str, &str) = match ty {
                "function" => ("function", "function", "arguments"),
                "custom" => ("custom", "custom", "input"),
                _ => ("other", ty, ""),
            };
            let name = e.pointer(&format!("/{body}/name")).and_then(Value::as_str).or_else(|| (kind == "other").then_some(ty)).unwrap_or("").to_owned();
            let arguments = if args_key.is_empty() { String::new() } else { e.pointer(&format!("/{body}/{args_key}")).and_then(Value::as_str).unwrap_or("").to_owned() };
            out.push(ModelCall { kind, name, arguments });
        }
    }
    if let Some(Value::Object(fc)) = m.get("function_call") {
        out.push(ModelCall {
            kind: "legacy",
            name: fc.get("name").and_then(Value::as_str).unwrap_or("").to_owned(),
            arguments: fc.get("arguments").and_then(Value::as_str).unwrap_or("").to_owned(),
        });
    }
    out
}

/// openai-python `ChatCompletionStreamState._accumulate_chunk`: choices are looked up by LIST
/// POSITION (`choices[choice.index]`, appending on IndexError), deltas merged by `py_merge`.
pub fn openai_python(bytes: &[u8]) -> Result<Vec<ModelCall>, String> {
    let mut snaps: Vec<Map<String, Value>> = Vec::new();
    for v in json_records(bytes)? {
        let Some(Value::Array(choices)) = v.get("choices") else { continue };
        for c in choices {
            let idx = usize::try_from(c.get("index").and_then(Value::as_u64).ok_or("choice without index")?).expect("small");
            if idx >= snaps.len() {
                snaps.push(Map::new());
            }
            let pos = idx.min(snaps.len() - 1);
            if let Some(Value::Object(d)) = c.get("delta") {
                py_merge(&mut snaps[pos], d)?;
            }
        }
    }
    Ok(snaps.iter().flat_map(calls_of_message).collect())
}

/// openai-node `ChatCompletionStream#accumulateChatCompletion`: choices and tool calls are sparse
/// arrays by index; `id`, `type` and `function.name` are REPLACED, `arguments` appended.
/// SOURCE: openai-node <pinned in Task 18>, src/lib/ChatCompletionStream.ts.
pub fn openai_node(bytes: &[u8]) -> Result<Vec<ModelCall>, String> {
    use std::collections::BTreeMap;
    let mut choices: BTreeMap<u64, (BTreeMap<u64, (String, String, String)>, Option<(String, String)>)> = BTreeMap::new();
    for v in json_records(bytes)? {
        let Some(Value::Array(cs)) = v.get("choices") else { continue };
        for c in cs {
            let idx = c.get("index").and_then(Value::as_u64).ok_or("choice without index")?;
            let (tcs, fc) = choices.entry(idx).or_default();
            let Some(d) = c.get("delta") else { continue };
            if let Some(Value::Array(list)) = d.get("tool_calls") {
                for e in list {
                    let i = e.get("index").and_then(Value::as_u64).ok_or("tool call without index")?;
                    let slot = tcs.entry(i).or_insert_with(|| ("function".into(), String::new(), String::new()));
                    if let Some(t) = e.get("type").and_then(Value::as_str).filter(|t| !t.is_empty()) {
                        slot.0 = t.to_owned();
                    }
                    let body = if slot.0 == "custom" { "custom" } else { "function" };
                    if let Some(n) = e.pointer(&format!("/{body}/name")).and_then(Value::as_str).filter(|n| !n.is_empty()) {
                        slot.1 = n.to_owned();
                    }
                    let key = if body == "custom" { "input" } else { "arguments" };
                    if let Some(a) = e.pointer(&format!("/{body}/{key}")).and_then(Value::as_str) {
                        slot.2.push_str(a);
                    }
                }
            }
            if let Some(Value::Object(f)) = d.get("function_call") {
                let s = fc.get_or_insert_with(|| (String::new(), String::new()));
                if let Some(n) = f.get("name").and_then(Value::as_str).filter(|n| !n.is_empty()) {
                    s.0 = n.to_owned();
                }
                if let Some(a) = f.get("arguments").and_then(Value::as_str) {
                    s.1.push_str(a);
                }
            }
        }
    }
    let mut out = Vec::new();
    for (tcs, fc) in choices.into_values() {
        for (ty, name, arguments) in tcs.into_values() {
            let kind = match ty.as_str() {
                "function" => "function",
                "custom" => "custom",
                _ => "other",
            };
            out.push(ModelCall { kind, name, arguments });
        }
        if let Some((name, arguments)) = fc {
            out.push(ModelCall { kind: "legacy", name, arguments });
        }
    }
    Ok(out)
}

/// LangChain `AIMessageChunk` merging: `tool_call_chunks` merged by `index` VALUE, strings
/// (including `name`) concatenated.
/// SOURCE: langchain-core <not pinned: modelled only>, langchain_core/utils/_merge.py merge_lists.
pub fn langchain(bytes: &[u8]) -> Result<Vec<ModelCall>, String> {
    use std::collections::BTreeMap;
    let mut per: BTreeMap<u64, Vec<(u64, String, String)>> = BTreeMap::new();
    let mut legacy: BTreeMap<u64, (String, String)> = BTreeMap::new();
    for v in json_records(bytes)? {
        let Some(Value::Array(cs)) = v.get("choices") else { continue };
        for c in cs {
            let idx = c.get("index").and_then(Value::as_u64).ok_or("choice without index")?;
            let Some(d) = c.get("delta") else { continue };
            if let Some(Value::Array(list)) = d.get("tool_calls") {
                let calls = per.entry(idx).or_default();
                for e in list {
                    let i = e.get("index").and_then(Value::as_u64).ok_or("tool call without index")?;
                    let name = e.pointer("/function/name").and_then(Value::as_str).unwrap_or("");
                    let args = e.pointer("/function/arguments").and_then(Value::as_str).unwrap_or("");
                    match calls.iter_mut().find(|(j, _, _)| *j == i) {
                        Some(slot) => {
                            slot.1.push_str(name);
                            slot.2.push_str(args);
                        }
                        None => calls.push((i, name.to_owned(), args.to_owned())),
                    }
                }
            }
            if let Some(Value::Object(f)) = d.get("function_call") {
                let s = legacy.entry(idx).or_default();
                s.0.push_str(f.get("name").and_then(Value::as_str).unwrap_or(""));
                s.1.push_str(f.get("arguments").and_then(Value::as_str).unwrap_or(""));
            }
        }
    }
    let mut out: Vec<ModelCall> = per.into_values().flatten().map(|(_, name, arguments)| ModelCall { kind: "function", name, arguments }).collect();
    out.extend(legacy.into_values().map(|(name, arguments)| ModelCall { kind: "legacy", name, arguments }));
    Ok(out)
}

/// anthropic-python `MessageStream` accumulation: dispatch by the SSE EVENT name; a
/// `content_block_start` APPENDS a block regardless of its index; deltas apply by index.
/// SOURCE: anthropic-python <pinned in Task 18>, src/anthropic/lib/streaming/_messages.py accumulate_event.
pub fn anthropic_python(bytes: &[u8]) -> Result<Vec<ModelCall>, String> {
    let mut blocks: Vec<(String, String, String)> = Vec::new(); // (type, name, json_buf)
    for (event, d) in records(bytes) {
        let Some(ev) = event else { continue }; // the SDK ignores records without an event name
        let v: Value = serde_json::from_slice(&d).map_err(|e| format!("unparseable record: {e}"))?;
        match ev.as_str() {
            "content_block_start" => {
                let cb = v.get("content_block").ok_or("start without block")?;
                let ty = cb.get("type").and_then(Value::as_str).unwrap_or("").to_owned();
                let name = cb.get("name").and_then(Value::as_str).unwrap_or("").to_owned();
                blocks.push((ty, name, String::new()));
            }
            "content_block_delta" => {
                let i = usize::try_from(v.get("index").and_then(Value::as_u64).ok_or("delta without index")?).expect("small");
                let b = blocks.get_mut(i).ok_or("delta for a missing block")?;
                if v.pointer("/delta/type").and_then(Value::as_str) == Some("input_json_delta") {
                    b.2.push_str(v.pointer("/delta/partial_json").and_then(Value::as_str).unwrap_or(""));
                }
            }
            "message_stop" => break,
            "error" => return Err("error event".into()),
            _ => {}
        }
    }
    Ok(blocks
        .into_iter()
        .filter(|(t, _, _)| t != "text" && t != "thinking" && t != "redacted_thinking" && t != "server_tool_use" && !t.ends_with("_tool_result") && t != "mcp_tool_use")
        .map(|(t, name, arguments)| ModelCall { kind: if t == "tool_use" { "tool_use" } else { "other" }, name, arguments })
        .collect())
}

/// The model kind of a decided call.
pub fn kind_of(k: paigasus_gateway_core::ToolCallKind) -> &'static str {
    use paigasus_gateway_core::ToolCallKind as K;
    match k {
        K::Function => "function",
        K::Custom => "custom",
        K::LegacyFunctionCall => "legacy",
        K::AnthropicToolUse => "tool_use",
        _ => "other",
    }
}
```

The `_ => "other"` arm is required because `ToolCallKind` is `#[non_exhaustive]` outside the crate.

- [ ] **Step 4: Write the generators**

`tests/support/gen.rs` renders scenarios as upstream bytes. Hidden calls, the ones a correct interceptor must never let through, are always named `blocked_*` and carry `CANARY` in their arguments. Allowed calls never contain `CANARY`.

```rust
// SPDX-License-Identifier: Apache-2.0
//! Grammar-based generators of hostile upstream streams (spec § 8.1).

use proptest::prelude::*;
use serde_json::{Value, json};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quirk {
    DuplicateDelta,
    CaseFoldedKey,
    ProtoKey,
    FloatIndex,
    BoolIndex,
    NegativeIndex,
    RepeatedName,
    Bom,
    LaxLine,
    UnknownFields,
    AfterDone,
    NoFinalBlank,
    Oversized,
    NaNRecord,
    JsonBody,
    EventLine,
    IndexGap,
    Comments,
    Interleave,
    EscapedKey,
}

#[derive(Debug, Clone)]
pub struct Call {
    pub name: &'static str,
    pub args: String,
    pub legacy: bool,
    pub pieces: usize,
}

#[derive(Debug, Clone)]
pub struct Choice {
    pub before: Option<String>,
    pub calls: Vec<Call>,
    pub after: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Scenario {
    pub choices: Vec<Choice>,
    pub quirks: Vec<(Quirk, usize)>,
    pub cuts: Vec<usize>,
}

const ALLOWED: [&str; 2] = ["read_file", "send_mail"];
const BLOCKED: [&str; 2] = ["blocked_rm", "blocked_exfil"];

fn call_strategy(legacy_ok: bool) -> impl Strategy<Value = Call> {
    (prop::bool::ANY, 0usize..2, "[a-z]{0,12}", 1usize..4, prop::bool::ANY).prop_map(move |(blocked, n, s, pieces, legacy)| {
        let name = if blocked { BLOCKED[n] } else { ALLOWED[n] };
        let args = if blocked { format!("{{\"v\":\"CANARY-{s}\"}}") } else { format!("{{\"v\":\"{s}\"}}") };
        Call { name, args, legacy: legacy && legacy_ok, pieces }
    })
}

fn choice_strategy() -> impl Strategy<Value = Choice> {
    (prop::option::of("[a-zA-Z ]{1,10}"), prop::collection::vec(call_strategy(true), 0..4), prop::option::of("[a-z ]{1,8}")).prop_map(|(before, mut calls, after)| {
        if calls.len() > 1 {
            for c in &mut calls {
                c.legacy = false; // a legacy call is the only call of its choice
            }
        }
        Choice { before, calls, after }
    })
}

fn quirk_strategy() -> impl Strategy<Value = Quirk> {
    prop_oneof![
        Just(Quirk::DuplicateDelta),
        Just(Quirk::CaseFoldedKey),
        Just(Quirk::ProtoKey),
        Just(Quirk::FloatIndex),
        Just(Quirk::BoolIndex),
        Just(Quirk::NegativeIndex),
        Just(Quirk::RepeatedName),
        Just(Quirk::Bom),
        Just(Quirk::LaxLine),
        Just(Quirk::UnknownFields),
        Just(Quirk::AfterDone),
        Just(Quirk::NoFinalBlank),
        Just(Quirk::Oversized),
        Just(Quirk::NaNRecord),
        Just(Quirk::JsonBody),
        Just(Quirk::EventLine),
        Just(Quirk::IndexGap),
        Just(Quirk::Comments),
        Just(Quirk::Interleave),
        Just(Quirk::EscapedKey),
    ]
}

pub fn openai_scenario() -> impl Strategy<Value = Scenario> {
    (prop::collection::vec(choice_strategy(), 1..3), prop::collection::vec((quirk_strategy(), 0usize..64), 0..3), prop::collection::vec(0usize..100_000, 0..6))
        .prop_map(|(choices, quirks, cuts)| Scenario { choices, quirks, cuts })
}

fn chunk(choices: Value) -> Value {
    json!({"id":"chatcmpl-g","object":"chat.completion.chunk","created":1,"model":"m","choices":choices})
}

fn hidden_entry(index: Value) -> Value {
    json!({"index":index,"id":"call_h","type":"function","function":{"name":"blocked_hidden","arguments":"{\"v\":\"CANARY-hidden\"}"}})
}

fn split(s: &str, pieces: usize) -> Vec<String> {
    let chars: Vec<char> = s.chars().collect();
    let step = chars.len().div_ceil(pieces.max(1)).max(1);
    chars.chunks(step).map(|c| c.iter().collect()).collect()
}

/// The records of one choice, as (choice, sequence of JSON payloads).
fn choice_records(ci: usize, ch: &Choice, interleave: bool) -> Vec<String> {
    let c = ci;
    let mut out = Vec::new();
    if let Some(t) = &ch.before {
        out.push(chunk(json!([{"index":c,"delta":{"role":"assistant","content":t}}])).to_string());
    }
    // Fragments per call: the first carries id/type/name.
    let per_call: Vec<Vec<String>> = ch
        .calls
        .iter()
        .enumerate()
        .map(|(i, call)| {
            split(&call.args, call.pieces)
                .into_iter()
                .enumerate()
                .map(|(k, piece)| {
                    let delta = if call.legacy {
                        if k == 0 { json!({"function_call":{"name":call.name,"arguments":piece}}) } else { json!({"function_call":{"arguments":piece}}) }
                    } else if k == 0 {
                        json!({"tool_calls":[{"index":i,"id":format!("call_{c}_{i}"),"type":"function","function":{"name":call.name,"arguments":piece}}]})
                    } else {
                        json!({"tool_calls":[{"index":i,"function":{"arguments":piece}}]})
                    };
                    chunk(json!([{"index":c,"delta":delta}])).to_string()
                })
                .collect()
        })
        .collect();
    if interleave {
        let longest = per_call.iter().map(Vec::len).max().unwrap_or(0);
        for k in 0..longest {
            for frags in &per_call {
                if let Some(f) = frags.get(k) {
                    out.push(f.clone());
                }
            }
        }
    } else {
        out.extend(per_call.into_iter().flatten());
    }
    if let Some(t) = &ch.after {
        out.push(chunk(json!([{"index":c,"delta":{"content":t}}])).to_string());
    }
    let finish = if ch.calls.iter().any(|c| c.legacy) {
        "function_call"
    } else if ch.calls.is_empty() {
        "stop"
    } else {
        "tool_calls"
    };
    out.push(chunk(json!([{"index":c,"delta":{},"finish_reason":finish}])).to_string());
    out
}

pub fn render_openai(s: &Scenario) -> Vec<u8> {
    let interleave = s.quirks.iter().any(|(q, _)| *q == Quirk::Interleave);
    let per: Vec<Vec<String>> = s.choices.iter().enumerate().map(|(i, c)| choice_records(i, c, interleave)).collect();
    let mut recs: Vec<String> = Vec::new();
    if interleave {
        let longest = per.iter().map(Vec::len).max().unwrap_or(0);
        for k in 0..longest {
            for p in &per {
                if let Some(r) = p.get(k) {
                    recs.push(r.clone());
                }
            }
        }
    } else {
        recs = per.into_iter().flatten().collect();
    }
    let mut lines: Vec<String> = recs.iter().map(|r| format!("data: {r}\n\n")).collect();
    let mut prefix = String::new();
    let mut suffix = String::new();
    for (q, at) in &s.quirks {
        let pos = at % (lines.len() + 1);
        let insert = |lines: &mut Vec<String>, rec: String| lines.insert(pos, format!("data: {rec}\n\n"));
        match q {
            Quirk::DuplicateDelta => insert(&mut lines, format!(r#"{{"choices":[{{"index":0,"delta":{{"tool_calls":[{}]}},"delta":{{"content":"x"}}}}]}}"#, hidden_entry(json!(0)))),
            Quirk::CaseFoldedKey => insert(&mut lines, format!(r#"{{"choices":[{{"index":0,"delta":{{"Tool_Calls":[{}]}}}}]}}"#, hidden_entry(json!(0)))),
            Quirk::ProtoKey => insert(&mut lines, format!(r#"{{"choices":[{{"index":0,"delta":{{"__proto__":{{"tool_calls":[{}]}}}}}}]}}"#, hidden_entry(json!(0)))),
            Quirk::FloatIndex => insert(&mut lines, chunk(json!([{"index":0,"delta":{"tool_calls":[hidden_entry(json!(0.0))]}}])).to_string()),
            Quirk::BoolIndex => insert(&mut lines, chunk(json!([{"index":0,"delta":{"tool_calls":[hidden_entry(json!(true))]}}])).to_string()),
            Quirk::NegativeIndex => insert(&mut lines, chunk(json!([{"index":0,"delta":{"tool_calls":[hidden_entry(json!(-1))]}}])).to_string()),
            Quirk::RepeatedName => insert(&mut lines, chunk(json!([{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"name":"blocked_hidden","arguments":"CANARY-rn"}}]}}])).to_string()),
            Quirk::IndexGap => insert(&mut lines, chunk(json!([{"index":0,"delta":{"tool_calls":[hidden_entry(json!(9))]}}])).to_string()),
            Quirk::NaNRecord => insert(&mut lines, format!(r#"{{"x":NaN,"choices":[{{"index":0,"delta":{{"tool_calls":[{}]}}}}]}}"#, hidden_entry(json!(0)))),
            Quirk::EscapedKey => insert(&mut lines, r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_e","type":"function","function":{"name":"blocked_escaped","arguments":"CANARY-esc"}}]}}]}"#.to_owned()),
            Quirk::Oversized => insert(&mut lines, chunk(json!([{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":format!("CANARY-big{}", "x".repeat(5000))}}]}}])).to_string()),
            Quirk::LaxLine => lines.insert(pos, format!("  data: {}\n\nData: {}\n\n", chunk(json!([{"index":0,"delta":{"tool_calls":[hidden_entry(json!(0))]}}])), chunk(json!([{"index":0,"delta":{"tool_calls":[hidden_entry(json!(0))]}}])))),
            Quirk::UnknownFields => lines.insert(pos, "id: 7\nretry: 10\nfoo: bar\n".to_owned()),
            Quirk::Comments => lines.insert(pos, ": hello\n: data: {\"x\":\"CANARY-comment\"}\n".to_owned()),
            Quirk::EventLine => lines.insert(pos, "event: message\n".to_owned()),
            Quirk::Bom => prefix = "\u{feff}".to_owned(),
            Quirk::AfterDone => suffix = format!("data: {}\n\n", chunk(json!([{"index":0,"delta":{"tool_calls":[hidden_entry(json!(0))]}}]))),
            Quirk::NoFinalBlank | Quirk::JsonBody | Quirk::Interleave => {}
        }
    }
    let mut body = format!("{prefix}{}data: [DONE]\n\n{suffix}", lines.concat());
    if s.quirks.iter().any(|(q, _)| *q == Quirk::NoFinalBlank) {
        body.truncate(body.trim_end_matches('\n').len());
    }
    if s.quirks.iter().any(|(q, _)| *q == Quirk::JsonBody) {
        body = json!({"choices":[{"index":0,"message":{"tool_calls":[{"id":"c","type":"function","function":{"name":"blocked_json","arguments":"CANARY-json"}}]}}]}).to_string();
    }
    body.into_bytes()
}

#[derive(Debug, Clone)]
pub enum Block {
    Text(String),
    Tool { name: &'static str, args: String, pieces: usize },
    Server,
}

#[derive(Debug, Clone)]
pub struct AnthropicScenario {
    pub blocks: Vec<Block>,
    pub quirks: Vec<(AQuirk, usize)>,
    pub cuts: Vec<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AQuirk {
    EventMismatch,
    PrefilledInput,
    PrefilledMessage,
    IndexReuse,
    NoEventLines,
    Bom,
    AfterStop,
    DuplicateKey,
    Oversized,
}

pub fn anthropic_scenario() -> impl Strategy<Value = AnthropicScenario> {
    let block = prop_oneof![
        "[a-z ]{1,10}".prop_map(Block::Text),
        (prop::bool::ANY, 0usize..2, "[a-z]{0,10}", 1usize..4).prop_map(|(b, n, s, pieces)| if b {
            Block::Tool { name: BLOCKED[n], args: format!("{{\"v\":\"CANARY-{s}\"}}"), pieces }
        } else {
            Block::Tool { name: ALLOWED[n], args: format!("{{\"v\":\"{s}\"}}"), pieces }
        }),
        Just(Block::Server),
    ];
    let quirk = prop_oneof![
        Just(AQuirk::EventMismatch),
        Just(AQuirk::PrefilledInput),
        Just(AQuirk::PrefilledMessage),
        Just(AQuirk::IndexReuse),
        Just(AQuirk::NoEventLines),
        Just(AQuirk::Bom),
        Just(AQuirk::AfterStop),
        Just(AQuirk::DuplicateKey),
        Just(AQuirk::Oversized),
    ];
    (prop::collection::vec(block, 0..5), prop::collection::vec((quirk, 0usize..64), 0..3), prop::collection::vec(0usize..100_000, 0..6))
        .prop_map(|(blocks, quirks, cuts)| AnthropicScenario { blocks, quirks, cuts })
}

pub fn render_anthropic(s: &AnthropicScenario) -> Vec<u8> {
    let mut evs: Vec<Value> = vec![json!({"type":"message_start","message":{"id":"msg_g","type":"message","role":"assistant","content":[],"model":"c"}})];
    for (i, b) in s.blocks.iter().enumerate() {
        match b {
            Block::Text(t) => {
                evs.push(json!({"type":"content_block_start","index":i,"content_block":{"type":"text","text":""}}));
                evs.push(json!({"type":"content_block_delta","index":i,"delta":{"type":"text_delta","text":t}}));
            }
            Block::Tool { name, args, pieces } => {
                evs.push(json!({"type":"content_block_start","index":i,"content_block":{"type":"tool_use","id":format!("toolu_{i}"),"name":name,"input":{}}}));
                for p in split(args, *pieces) {
                    evs.push(json!({"type":"content_block_delta","index":i,"delta":{"type":"input_json_delta","partial_json":p}}));
                }
            }
            Block::Server => {
                evs.push(json!({"type":"content_block_start","index":i,"content_block":{"type":"server_tool_use","id":format!("srv_{i}"),"name":"web_search","input":{}}}));
            }
        }
        evs.push(json!({"type":"content_block_stop","index":i}));
    }
    let any_tool = s.blocks.iter().any(|b| matches!(b, Block::Tool { .. }));
    evs.push(json!({"type":"message_delta","delta":{"stop_reason": if any_tool { "tool_use" } else { "end_turn" }},"usage":{"output_tokens":1}}));
    evs.push(json!({"type":"message_stop"}));
    let no_events = s.quirks.iter().any(|(q, _)| *q == AQuirk::NoEventLines);
    let mut frames: Vec<String> = evs
        .iter()
        .map(|e| if no_events { format!("data: {e}\n\n") } else { format!("event: {}\ndata: {e}\n\n", e["type"].as_str().unwrap()) })
        .collect();
    let hidden_start = |idx: usize, input: Value| json!({"type":"content_block_start","index":idx,"content_block":{"type":"tool_use","id":"toolu_h","name":"blocked_hidden","input":input}});
    let mut prefix = String::new();
    let mut suffix = String::new();
    for (q, at) in &s.quirks {
        let pos = 1 + at % frames.len().max(1);
        let pos = pos.min(frames.len().saturating_sub(1));
        match q {
            AQuirk::EventMismatch => frames.insert(pos, format!("event: ping\ndata: {}\n\n", hidden_start(s.blocks.len(), json!({"v":"CANARY-em"})))),
            AQuirk::PrefilledInput => frames.insert(pos, format!("event: content_block_start\ndata: {}\n\n", hidden_start(s.blocks.len(), json!({"v":"CANARY-pi"})))),
            AQuirk::PrefilledMessage => {
                frames[0] = format!("event: message_start\ndata: {}\n\n", json!({"type":"message_start","message":{"content":[{"type":"tool_use","id":"t","name":"blocked_hidden","input":{"v":"CANARY-pm"}}]}}))
            }
            AQuirk::IndexReuse => frames.insert(pos, format!("event: content_block_start\ndata: {}\n\n", json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_r","name":"blocked_reuse","input":{}}}))),
            AQuirk::DuplicateKey => frames.insert(pos, format!("event: content_block_delta\ndata: {}\n\n", r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"CANARY-dup"},"delta":{"type":"text_delta","text":"x"}}"#)),
            AQuirk::Oversized => frames.insert(pos, format!("event: content_block_delta\ndata: {}\n\n", json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":format!("CANARY-big{}", "x".repeat(5000))}}))),
            AQuirk::Bom => prefix = "\u{feff}".to_owned(),
            AQuirk::AfterStop => suffix = format!("event: content_block_start\ndata: {}\n\n", hidden_start(s.blocks.len(), json!({}))),
            AQuirk::NoEventLines => {}
        }
    }
    format!("{prefix}{}{suffix}", frames.concat()).into_bytes()
}
```

The `IndexReuse` quirk can be followed by deltas for index 0 that are forwarded in observe mode. That is why the properties check the canary only under `OnFailure::Block` (Step 5).

- [ ] **Step 5: Write the properties**

`tests/properties.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0
//! P1 (invariant I1) and P2 (bounded memory) over generated hostile streams (spec § 8.1, AC 3, AC 4).

mod support;

use paigasus_gateway_core::{Format, OnFailure};
use proptest::prelude::*;
use support::driver::{allow_all, block_named, cut, drive, limits};
use support::gen::{anthropic_scenario, openai_scenario, render_anthropic, render_openai};
use support::oracle::{MODELS, ModelCall, kind_of};

/// Every call any model derives must match a distinct call the decider allowed, by kind, name and
/// arguments. A model error fails the property: the output must parse for every client.
fn check_p1(out: &[u8], decided: &[(paigasus_gateway_core::ToolCall, paigasus_gateway_core::Decision)]) -> Result<(), TestCaseError> {
    for (model, f) in MODELS {
        let calls = f(out).map_err(|e| TestCaseError::fail(format!("{model} failed on the output: {e}\n{}", String::from_utf8_lossy(out))))?;
        let mut allowed: Vec<ModelCall> = decided
            .iter()
            .filter(|(_, d)| d.is_allow())
            .map(|(c, _)| ModelCall { kind: kind_of(c.kind), name: c.name.clone(), arguments: c.arguments.clone() })
            .collect();
        for mc in calls {
            let hit = allowed.iter().position(|a| a.kind == mc.kind && a.name == mc.name && (mc.kind == "other" || a.arguments == mc.arguments));
            match hit {
                Some(i) => {
                    allowed.remove(i);
                }
                None => return Err(TestCaseError::fail(format!("{model} runs an undecided or blocked call {mc:?}\n{}", String::from_utf8_lossy(out)))),
            }
        }
    }
    Ok(())
}

fn check_canary(out: &[u8]) -> Result<(), TestCaseError> {
    prop_assert!(!String::from_utf8_lossy(out).contains("CANARY"), "a blocked call's canary leaked:\n{}", String::from_utf8_lossy(out));
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(10_000))]

    #[test]
    fn openai_p1_and_p2_hold(s in openai_scenario()) {
        let bytes = render_openai(&s);
        let l = limits(OnFailure::Block);
        let o = drive(Format::OpenAiChat, l, &cut(&bytes, &s.cuts), &block_named);
        check_canary(&o.out)?;
        check_p1(&o.out, &o.decided)?;
        prop_assert!(o.max_queued <= l.max_held_bytes + l.max_record_bytes, "queue {} over the bound", o.max_queued);
    }

    #[test]
    fn anthropic_p1_and_p2_hold(s in anthropic_scenario()) {
        let bytes = render_anthropic(&s);
        let l = limits(OnFailure::Block);
        let o = drive(Format::AnthropicMessages, l, &cut(&bytes, &s.cuts), &block_named);
        check_canary(&o.out)?;
        check_p1(&o.out, &o.decided)?;
        prop_assert!(o.max_queued <= l.max_held_bytes + l.max_record_bytes);
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2_000))]

    /// Observe mode is not a security mode, but it must never panic, must stay bounded, and must
    /// emit bytes every client can parse.
    #[test]
    fn observe_mode_is_bounded_and_parseable(s in openai_scenario(), a in anthropic_scenario()) {
        for (format, bytes, cuts) in [(Format::OpenAiChat, render_openai(&s), &s.cuts), (Format::AnthropicMessages, render_anthropic(&a), &a.cuts)] {
            let l = limits(OnFailure::Allow);
            let o = drive(format, l, &cut(&bytes, cuts), &block_named);
            prop_assert!(o.max_queued <= l.max_held_bytes + l.max_record_bytes);
            for (model, f) in MODELS {
                prop_assert!(f(&o.out).is_ok(), "{model} failed in observe mode:\n{}", String::from_utf8_lossy(&o.out));
            }
        }
    }

    /// Allow-all with no quirks: the decider sees exactly the calls every model derives.
    #[test]
    fn allow_all_models_agree_with_the_decider(mut s in openai_scenario()) {
        s.quirks.retain(|(q, _)| matches!(q, support::gen::Quirk::Interleave | support::gen::Quirk::Comments | support::gen::Quirk::UnknownFields | support::gen::Quirk::Bom));
        let bytes = render_openai(&s);
        let o = drive(Format::OpenAiChat, limits(OnFailure::Block), &cut(&bytes, &s.cuts), &allow_all);
        check_p1(&o.out, &o.decided)?;
        let derived = support::oracle::openai_node(&o.out).unwrap();
        prop_assert_eq!(derived.len(), o.decided.len(), "every decided call reaches the client");
    }
}
```

`openai_python` is not used for the last length check, because it merges choices by list position (its documented quirk). `openai_node` indexes by choice, so its count is exact.

- [ ] **Step 6: Generate the fixtures**

`tests/fixtures/make_fixtures.py` is committed, so the fixtures can be regenerated and reviewed:

```python
# SPDX-License-Identifier: Apache-2.0
"""Writes the hand-built SSE fixtures for paigasus-gateway-core (SMA-749 spec § 8.3).

Every fixture here is HAND-BUILT, not a recording. Blocked calls are named blocked_* and carry
CANARY in their arguments. Run: python3 -I make_fixtures.py <this directory>
"""

import json
import pathlib
import sys

root = pathlib.Path(sys.argv[1])
BOM = "﻿"


def dump(v):
    return json.dumps(v, separators=(",", ":"))


def d(v):
    return "data: " + (v if isinstance(v, str) else dump(v)) + "\n\n"


def ch(choices, **extra):
    return {"id": "chatcmpl-fx", "object": "chat.completion.chunk", "created": 1790942400, "model": "gpt-4o-mini", "system_fingerprint": "fp_fx", "choices": choices, **extra}


def text(c, s, role=False):
    delta = {"role": "assistant", "content": s} if role else {"content": s}
    return ch([{"index": c, "delta": delta, "finish_reason": None}])


def call(c, i, name, args, first=True):
    e = {"index": i, "id": f"call_{c}_{i}", "type": "function", "function": {"name": name, "arguments": args}} if first else {"index": i, "function": {"arguments": args}}
    return ch([{"index": c, "delta": {"tool_calls": [e]}, "finish_reason": None}])


def fin(c, reason="tool_calls"):
    return ch([{"index": c, "delta": {}, "finish_reason": reason}])


DONE = "data: [DONE]\n\n"
HIDDEN = {"index": 0, "id": "call_h", "type": "function", "function": {"name": "blocked_hidden", "arguments": "CANARY-hidden"}}


def one_blocked(canary):
    return [call(0, 0, "blocked_rm", '{"path":'), call(0, 0, "", f'"{canary}"}}', first=False), fin(0)]


OPENAI = {
    "text_only": [text(0, "Hello", role=True), text(0, " world"), fin(0, "stop")],
    "one_call_allowed": [call(0, 0, "read_file", '{"path":'), call(0, 0, "", '"/tmp/a"}', first=False), fin(0)],
    "one_call_blocked": one_blocked("CANARY-1"),
    "parallel_mixed": [call(0, 0, "read_file", '{"p":1}'), call(0, 1, "blocked_rm", '{"p":"CANARY-2"}'), fin(0)],
    "interleaved": [call(0, 0, "read_file", '{"p":'), call(0, 1, "blocked_rm", '{"p":'), call(0, 0, "", "1}", first=False), call(0, 1, "", '"CANARY-3"}', first=False), fin(0)],
    "n2_choices": [text(0, "A", role=True), call(1, 0, "blocked_exfil", '{"to":"CANARY-4"}'), call(0, 0, "read_file", "{}"), fin(1), fin(0)],
    "legacy_blocked": [ch([{"index": 0, "delta": {"role": "assistant", "content": None, "function_call": {"name": "blocked_rm", "arguments": '{"p":"CANARY-5"}'}}, "finish_reason": None}]), fin(0, "function_call")],
    "text_after_calls": [call(0, 0, "blocked_rm", '{"p":"CANARY-6"}'), text(0, "done"), fin(0)],
    "custom_tool_blocked": [ch([{"index": 0, "delta": {"tool_calls": [{"index": 0, "id": "call_c", "type": "custom", "custom": {"name": "blocked_sql", "input": "DROP CANARY-7"}}]}, "finish_reason": None}]), fin(0)],
    "usage_chunk": one_blocked("CANARY-8") + [{"id": "chatcmpl-fx", "object": "chat.completion.chunk", "choices": [], "usage": {"prompt_tokens": 2, "completion_tokens": 3, "total_tokens": 5}}],
    "event_line": one_blocked("CANARY-9"),
}

OPENAI_MALFORMED = {
    "nan_record": [text(0, "a", role=True), '{"x":NaN,"choices":[{"index":0,"delta":{"tool_calls":[' + dump(HIDDEN) + "]}}]}", text(0, "b"), fin(0, "stop")],
    "duplicate_key_mid_hold": [call(0, 0, "read_file", "{}"), '{"choices":[{"index":0,"delta":{"tool_calls":[' + dump(HIDDEN) + ']},"delta":{"content":"x"}}]}', fin(0)],
    "case_folded_key": ['{"choices":[{"index":0,"delta":{"Tool_Calls":[' + dump(HIDDEN) + "]}}]}", text(0, "ok"), fin(0, "stop")],
    "proto_key": ['{"choices":[{"index":0,"delta":{"__proto__":{"tool_calls":[' + dump(HIDDEN) + "]}}}]}", text(0, "ok"), fin(0, "stop")],
    "float_index": [ch([{"index": 0, "delta": {"tool_calls": [{**HIDDEN, "index": 0.0}]}}]), text(0, "ok"), fin(0, "stop")],
    "repeated_name": [call(0, 0, "read_file", "{}"), ch([{"index": 0, "delta": {"tool_calls": [{"index": 0, "function": {"name": "blocked_x", "arguments": "CANARY-rn"}}]}}]), fin(0)],
    "oversized": [call(0, 0, "read_file", "{"), ch([{"index": 0, "delta": {"tool_calls": [{"index": 0, "function": {"arguments": "CANARY-big" + "x" * 5000}}]}}]), fin(0)],
}


def write(path, body):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(body.encode("utf-8"))


for name, recs in {**OPENAI, **OPENAI_MALFORMED}.items():
    kind = "malformed" if name in OPENAI_MALFORMED else "conforming"
    body = "".join(d(r) for r in recs) + DONE
    if name == "event_line":
        body = body.replace("data: ", "event: message\ndata: ", 1)
    write(root / "openai" / kind / f"{name}.sse", body)

# Shapes that need raw bytes.
write(root / "openai" / "conforming" / "crlf_endings.sse", "".join(d(r) for r in one_blocked("CANARY-10")).replace("\n", "\r\n") + DONE.replace("\n", "\r\n"))
write(root / "openai" / "conforming" / "bom_prefixed.sse", BOM + "".join(d(r) for r in one_blocked("CANARY-11")) + DONE)
write(root / "openai" / "conforming" / "escaped_key.sse", d('{"choices":[{"index":0,"delta":{"tool\\u005fcalls":[{"index":0,"id":"call_e","type":"function","function":{"name":"blocked_rm","arguments":"CANARY-12"}}]}}]}') + d(fin(0)) + DONE)
write(root / "openai" / "malformed" / "after_done.sse", "".join(d(r) for r in OPENAI["one_call_allowed"]) + DONE + d(ch([{"index": 0, "delta": {"tool_calls": [HIDDEN]}}])))
write(root / "openai" / "malformed" / "error_mid_hold.sse", d(call(0, 0, "blocked_rm", "CANARY-err")) + d({"error": {"message": "boom", "type": "server_error"}}))
write(root / "openai" / "malformed" / "json_body.sse", dump({"choices": [{"index": 0, "message": {"tool_calls": [HIDDEN]}}]}))
write(root / "openai" / "malformed" / "lax_line.sse", "  data: " + dump(ch([{"index": 0, "delta": {"tool_calls": [HIDDEN]}}])) + "\n\n" + d(text(0, "ok", role=True)) + d(fin(0, "stop")) + DONE)


def ev(v):
    return f"event: {v['type']}\ndata: {dump(v)}\n\n"


START = {"type": "message_start", "message": {"id": "msg_fx", "type": "message", "role": "assistant", "content": [], "model": "claude-x", "stop_reason": None}}
STOP = {"type": "message_stop"}


def blk_text(i, s):
    return [{"type": "content_block_start", "index": i, "content_block": {"type": "text", "text": ""}}, {"type": "content_block_delta", "index": i, "delta": {"type": "text_delta", "text": s}}, {"type": "content_block_stop", "index": i}]


def blk_tool(i, name, args):
    half = len(args) // 2
    return [
        {"type": "content_block_start", "index": i, "content_block": {"type": "tool_use", "id": f"toolu_{i}", "name": name, "input": {}}},
        {"type": "content_block_delta", "index": i, "delta": {"type": "input_json_delta", "partial_json": args[:half]}},
        {"type": "content_block_delta", "index": i, "delta": {"type": "input_json_delta", "partial_json": args[half:]}},
        {"type": "content_block_stop", "index": i},
    ]


def mdelta(reason):
    return {"type": "message_delta", "delta": {"stop_reason": reason, "stop_sequence": None}, "usage": {"output_tokens": 7}}


ANTHROPIC = {
    "text_only": [START, *blk_text(0, "Hi."), mdelta("end_turn"), STOP],
    "tool_use_allowed": [START, *blk_text(0, "Reading."), *blk_tool(1, "read_file", '{"path":"/tmp/a"}'), mdelta("tool_use"), STOP],
    "tool_use_blocked": [START, *blk_text(0, "Removing."), *blk_tool(1, "blocked_rm", '{"path":"CANARY-A1"}'), mdelta("tool_use"), STOP],
    "two_tools_mixed": [START, *blk_tool(0, "read_file", '{"p":1}'), *blk_tool(1, "blocked_rm", '{"p":"CANARY-A2"}'), mdelta("tool_use"), STOP],
    "server_tool": [
        START,
        {"type": "content_block_start", "index": 0, "content_block": {"type": "server_tool_use", "id": "srv_1", "name": "web_search", "input": {}}},
        {"type": "content_block_delta", "index": 0, "delta": {"type": "input_json_delta", "partial_json": '{"q":"x"}'}},
        {"type": "content_block_stop", "index": 0},
        {"type": "content_block_start", "index": 1, "content_block": {"type": "web_search_tool_result", "tool_use_id": "srv_1", "content": []}},
        {"type": "content_block_stop", "index": 1},
        *blk_text(2, "Found."),
        mdelta("end_turn"),
        STOP,
    ],
    "thinking_then_tool_blocked": [
        START,
        {"type": "content_block_start", "index": 0, "content_block": {"type": "thinking", "thinking": ""}},
        {"type": "content_block_delta", "index": 0, "delta": {"type": "thinking_delta", "thinking": "hmm"}},
        {"type": "content_block_delta", "index": 0, "delta": {"type": "signature_delta", "signature": "sig"}},
        {"type": "content_block_stop", "index": 0},
        *blk_tool(1, "blocked_exfil", '{"to":"CANARY-A3"}'),
        mdelta("tool_use"),
        STOP,
    ],
}

for name, evs in ANTHROPIC.items():
    write(root / "anthropic" / "conforming" / f"{name}.sse", "".join(ev(e) for e in evs))

hidden_start = {"type": "content_block_start", "index": 0, "content_block": {"type": "tool_use", "id": "toolu_h", "name": "blocked_hidden", "input": {}}}
write(
    root / "anthropic" / "malformed" / "event_mismatch.sse",
    ev(START) + "event: ping\ndata: " + dump(hidden_start) + "\n\n" + ev({"type": "content_block_delta", "index": 0, "delta": {"type": "input_json_delta", "partial_json": "CANARY-em"}}) + ev({"type": "content_block_stop", "index": 0}) + ev(STOP),
)
write(root / "anthropic" / "malformed" / "prefilled_input.sse", ev(START) + ev({**hidden_start, "content_block": {**hidden_start["content_block"], "input": {"v": "CANARY-pi"}}}) + ev({"type": "content_block_stop", "index": 0}) + ev(STOP))
write(
    root / "anthropic" / "malformed" / "index_reuse.sse",
    ev(START) + "".join(ev(e) for e in blk_tool(0, "read_file", "{}")[:1]) + ev({**hidden_start, "content_block": {**hidden_start["content_block"], "name": "blocked_reuse"}}) + ev({"type": "content_block_delta", "index": 0, "delta": {"type": "input_json_delta", "partial_json": "CANARY-ir"}}) + ev({"type": "content_block_stop", "index": 0}) + ev(STOP),
)
write(root / "anthropic" / "malformed" / "prefilled_message.sse", ev({"type": "message_start", "message": {"content": [{"type": "tool_use", "id": "t", "name": "blocked_hidden", "input": {"v": "CANARY-pm"}}]}}) + ev(STOP))
```

```bash
cd rs/crates/libs/paigasus-gateway-core/tests/fixtures
python3 -I make_fixtures.py . && find . -name '*.sse' | sort | wc -l
```

Expected: `35` (14 OpenAI conforming, 11 OpenAI malformed, 6 Anthropic conforming, 4 Anthropic malformed). Check one file by eye: `cat openai/conforming/one_call_blocked.sse`.

- [ ] **Step 7: Write the fixture replays**

`tests/fixtures.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0
//! Fixture replays (spec § 8.3, AC 2, AC 3, AC 5).

mod support;

use std::path::{Path, PathBuf};

use paigasus_gateway_core::{Format, OnFailure};
use support::driver::{allow_all, block_named, drive, limits};
use support::oracle::{MODELS, payloads};

fn dir(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(rel)
}

fn files(rel: &str) -> Vec<(String, Vec<u8>)> {
    let mut v: Vec<(String, Vec<u8>)> = std::fs::read_dir(dir(rel))
        .unwrap_or_else(|e| panic!("{rel}: {e}"))
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "sse"))
        .map(|p| (p.file_stem().unwrap().to_string_lossy().into_owned(), std::fs::read(&p).unwrap()))
        .collect();
    v.sort();
    assert!(!v.is_empty(), "{rel} holds no fixtures");
    v
}

fn s(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// AC 2: with allow-all, every conforming fixture's data payloads come out byte-equal and in order.
#[test]
fn allow_all_is_payload_identical_for_every_conforming_fixture() {
    for (format, rel) in [(Format::OpenAiChat, "openai/conforming"), (Format::AnthropicMessages, "anthropic/conforming")] {
        for (name, bytes) in files(rel) {
            let o = drive(format, limits(OnFailure::Block), &[&bytes], &allow_all);
            assert_eq!(payloads(&o.out), payloads(&bytes), "{rel}/{name}");
        }
    }
}

/// AC 3: a blocked call's canary never appears, for every fixture and both on_failure modes
/// (except where observe mode passes the stream through by design).
#[test]
fn blocked_canaries_never_appear() {
    let observe_may_leak = ["index_reuse"];
    for (format, rel) in [
        (Format::OpenAiChat, "openai/conforming"),
        (Format::OpenAiChat, "openai/malformed"),
        (Format::AnthropicMessages, "anthropic/conforming"),
        (Format::AnthropicMessages, "anthropic/malformed"),
    ] {
        for (name, bytes) in files(rel) {
            let blocked = drive(format, limits(OnFailure::Block), &[&bytes], &block_named);
            assert!(!s(&blocked.out).contains("CANARY"), "{rel}/{name} (block):\n{}", s(&blocked.out));
            let observed = drive(format, limits(OnFailure::Allow), &[&bytes], &block_named);
            if !observe_may_leak.contains(&name.as_str()) {
                assert!(!s(&observed.out).contains("CANARY"), "{rel}/{name} (allow):\n{}", s(&observed.out));
            }
            // The terminal error frame makes the OpenAI SDKs raise, which is the intended client
            // behaviour for an upstream error (D17). Every other output must parse.
            if !s(&blocked.out).contains("\"terminal\"") {
                for (model, f) in MODELS {
                    assert!(f(&blocked.out).is_ok(), "{model} cannot parse {rel}/{name}");
                }
            }
        }
    }
}

/// § 8.3: every single split point gives the same output as one push, for fixtures up to 4 KiB.
#[test]
fn every_split_point_gives_the_same_output() {
    for (format, rel) in [(Format::OpenAiChat, "openai/conforming"), (Format::OpenAiChat, "openai/malformed"), (Format::AnthropicMessages, "anthropic/conforming"), (Format::AnthropicMessages, "anthropic/malformed")] {
        for (name, bytes) in files(rel) {
            if bytes.len() > 4096 {
                continue;
            }
            let whole = drive(format, limits(OnFailure::Block), &[&bytes], &block_named).out;
            for i in 0..=bytes.len() {
                let split = drive(format, limits(OnFailure::Block), &[&bytes[..i], &bytes[i..]], &block_named).out;
                assert_eq!(s(&split), s(&whole), "{rel}/{name} split at {i}");
            }
        }
    }
}

/// AC 5: each malformed shape shows the § 6 behaviour under both on_failure values.
#[test]
fn malformed_shapes_follow_section_6() {
    // (fixture, must appear with block, must appear with allow)
    let openai: &[(&str, &[&str], &[&str])] = &[
        ("nan_record", &["\"b\""], &["\"b\""]),
        ("duplicate_key_mid_hold", &["(malformed)"], &[": paigasus-interception pass-through"]),
        ("case_folded_key", &["\"ok\""], &["\"ok\""]),
        ("proto_key", &["\"ok\""], &["\"ok\""]),
        ("float_index", &["\"ok\""], &["\"ok\""]),
        ("repeated_name", &["(malformed)"], &[": paigasus-interception pass-through"]),
        ("oversized", &["(malformed)"], &[": paigasus-interception pass-through"]),
        ("after_done", &["data: [DONE]"], &["data: [DONE]"]),
        ("error_mid_hold", &["\"terminal\""], &["\"terminal\""]),
        ("json_body", &[], &[]),
        ("lax_line", &["\"ok\""], &["\"ok\""]),
    ];
    let anthropic: &[(&str, &[&str], &[&str])] = &[
        ("event_mismatch", &[], &[": paigasus-interception pass-through"]),
        ("prefilled_input", &[], &[": paigasus-interception pass-through"]),
        ("index_reuse", &["(malformed)"], &[": paigasus-interception pass-through"]),
        ("prefilled_message", &[], &[": paigasus-interception pass-through"]),
    ];
    for (format, rel, table) in [(Format::OpenAiChat, "openai/malformed", openai), (Format::AnthropicMessages, "anthropic/malformed", anthropic)] {
        let all = files(rel);
        assert_eq!(all.len(), table.len(), "every {rel} fixture has a row");
        for (name, with_block, with_allow) in table {
            let bytes = &all.iter().find(|(n, _)| n == name).unwrap_or_else(|| panic!("no fixture {name}")).1;
            let b = s(&drive(format, limits(OnFailure::Block), &[bytes], &block_named).out);
            for want in *with_block {
                assert!(b.contains(want), "{rel}/{name} (block) lacks {want:?}:\n{b}");
            }
            let a = s(&drive(format, limits(OnFailure::Allow), &[bytes], &block_named).out);
            for want in *with_allow {
                assert!(a.contains(want), "{rel}/{name} (allow) lacks {want:?}:\n{a}");
            }
        }
    }
}
```

- [ ] **Step 8: Run everything**

```bash
cd rs && cargo fmt && cargo nextest run --locked -p paigasus-gateway-core --no-fail-fast 2>&1 | tail -20
```

Expected: PASS. The two 10 000-case properties take the longest. Record their wall time in the task report. When a property fails, proptest prints a minimized scenario. Treat it as a real finding: fix the code, not the generator, unless the generator produced a stream that the spec allows the interceptor to forward. Then fix the generator and say so in the report.

A failure of `malformed_shapes_follow_section_6` on an Anthropic row whose block column is empty is expected only when the stream produced no hold. The canary check in `blocked_canaries_never_appear` is the load-bearing one. Do not weaken it.

- [ ] **Step 9: Commit**

```bash
git add rs/crates/libs/paigasus-gateway-core rs/Cargo.lock
git commit -m "test(rs): add the differential oracle and hostile-stream properties (SMA-749)" -m "Rust models of the openai-python, openai-node, LangChain and
anthropic-python accumulators, grammar-based generators of hostile
streams, the P1 and P2 properties at 10000 cases per format, and
fixture replays for payload identity, canaries, every split point and
the malformed shapes.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---
### Task 12: Golden streams and their manifest

**Files:**
- Create: `rs/crates/libs/paigasus-gateway-core/tests/golden.rs`, `tests/golden/manifest.json`, `tests/golden/{openai,anthropic}/*.sse` (generated, committed)

**Interfaces:**
- Consumes: `tests/support` (Task 11), the fixtures (Task 11).
- Produces: the golden contract that Task 18 (Python) and Task 19 (TypeScript) read.
  - `tests/golden/manifest.json` is a JSON array. Each entry is `{"file": "openai/one_call_blocked.sse", "format": "openai" | "anthropic", "allowed": [{"name": …, "arguments": …}], "notices": ["[paigasus] tool call \"…\" was blocked (policy)."]}`.
  - `allowed` lists every call the decider allowed, in decision order. `notices` lists every notice line the output must show as text.
  - The `.sse` files are the interceptor's output for the named input fixture under `block_named` with `OnFailure::Block`.

The golden set is a deliberate subset of the conforming fixtures: the shapes the real SDK stream helpers support. Legacy `function_call` and `custom` tools are left out. The Rust models cover them, and the pinned Python SDK's stream helper may not accept them (Task 18 Step 1 checks this).

| Golden | Source fixture |
|---|---|
| `openai/one_call_blocked.sse` | `fixtures/openai/conforming/one_call_blocked.sse` |
| `openai/parallel_mixed.sse` | `fixtures/openai/conforming/parallel_mixed.sse` |
| `openai/n2_choices.sse` | `fixtures/openai/conforming/n2_choices.sse` |
| `openai/text_after_calls.sse` | `fixtures/openai/conforming/text_after_calls.sse` |
| `anthropic/tool_use_blocked.sse` | `fixtures/anthropic/conforming/tool_use_blocked.sse` |
| `anthropic/two_tools_mixed.sse` | `fixtures/anthropic/conforming/two_tools_mixed.sse` |

- [ ] **Step 1: Write the drift test**

`tests/golden.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0
//! The golden streams that the real SDKs are checked against (spec § 8.2).
//!
//! `cargo nextest run -p paigasus-gateway-core --test golden` fails when a golden file is stale.
//! `UPDATE_GOLDEN=1` rewrites them. Review the diff before you commit it: a golden change is a
//! change to what every client receives.

mod support;

use std::path::{Path, PathBuf};

use paigasus_gateway_core::{Decision, Format, OnFailure, notice_line};
use serde_json::{Value, json};
use support::driver::{block_named, drive, limits};
use support::oracle::MODELS;

const SET: &[(&str, &str)] = &[
    ("openai", "one_call_blocked"),
    ("openai", "parallel_mixed"),
    ("openai", "n2_choices"),
    ("openai", "text_after_calls"),
    ("anthropic", "tool_use_blocked"),
    ("anthropic", "two_tools_mixed"),
];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests")
}

fn generate() -> (Vec<(PathBuf, Vec<u8>)>, Value) {
    let mut files = Vec::new();
    let mut manifest = Vec::new();
    for (fmt, name) in SET {
        let format = if *fmt == "openai" { Format::OpenAiChat } else { Format::AnthropicMessages };
        let input = std::fs::read(root().join(format!("fixtures/{fmt}/conforming/{name}.sse"))).expect("fixture exists");
        let o = drive(format, limits(OnFailure::Block), &[&input], &block_named);
        let allowed: Vec<Value> = o.decided.iter().filter(|(_, d)| d.is_allow()).map(|(c, _)| json!({"name": c.name, "arguments": c.arguments})).collect();
        let notices: Vec<String> = o
            .decided
            .iter()
            .filter_map(|(c, d)| match d {
                Decision::Block { code } => Some(notice_line(&c.name, *code)),
                _ => None,
            })
            .collect();
        assert!(!notices.is_empty(), "{fmt}/{name}: a golden must block at least one call");
        for (model, f) in MODELS {
            let calls = f(&o.out).unwrap_or_else(|e| panic!("{model} cannot parse golden {fmt}/{name}: {e}"));
            let relevant = (*fmt == "openai") != (*model == "anthropic-python");
            if relevant {
                assert_eq!(calls.len(), allowed.len(), "{model} on {fmt}/{name}");
            }
        }
        files.push((root().join(format!("golden/{fmt}/{name}.sse")), o.out));
        manifest.push(json!({"file": format!("{fmt}/{name}.sse"), "format": fmt, "allowed": allowed, "notices": notices}));
    }
    (files, Value::Array(manifest))
}

#[test]
fn golden_files_are_current() {
    let (files, manifest) = generate();
    let manifest_path = root().join("golden/manifest.json");
    let manifest_text = format!("{}\n", serde_json::to_string_pretty(&manifest).unwrap());
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        for (path, bytes) in &files {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes).unwrap();
        }
        std::fs::write(&manifest_path, &manifest_text).unwrap();
        return;
    }
    for (path, bytes) in &files {
        let on_disk = std::fs::read(path).unwrap_or_else(|_| panic!("{} is missing: run UPDATE_GOLDEN=1", path.display()));
        assert_eq!(String::from_utf8_lossy(&on_disk), String::from_utf8_lossy(bytes), "{} is stale: run UPDATE_GOLDEN=1 and review the diff", path.display());
    }
    assert_eq!(std::fs::read_to_string(&manifest_path).unwrap_or_default(), manifest_text, "manifest.json is stale: run UPDATE_GOLDEN=1");
}
```

The `relevant` line pairs the OpenAI models with OpenAI goldens and the Anthropic model with Anthropic goldens. A model of the other format derives no calls, so its count is not compared.

- [ ] **Step 2: Run it to verify it fails**

```bash
cd rs && cargo nextest run --locked -p paigasus-gateway-core --test golden
```

Expected: FAIL with `… is missing: run UPDATE_GOLDEN=1`.

- [ ] **Step 3: Generate and review the goldens**

```bash
cd rs && UPDATE_GOLDEN=1 cargo nextest run --locked -p paigasus-gateway-core --test golden && cargo nextest run --locked -p paigasus-gateway-core --test golden
cat crates/libs/paigasus-gateway-core/tests/golden/manifest.json
grep -l CANARY crates/libs/paigasus-gateway-core/tests/golden/*/*.sse && echo "FAIL: a canary is in a golden" || echo "no canary in any golden"
```

Expected: the second run passes, the manifest lists 6 entries, and the grep prints `no canary in any golden`. Read `golden/openai/parallel_mixed.sse` by eye. It must show `read_file` at tool-call index 0, no `blocked_rm`, and a notice chunk before the `finish_reason` frame.

- [ ] **Step 4: Commit**

```bash
git add rs/crates/libs/paigasus-gateway-core/tests
git commit -m "test(rs): add golden tool-interception streams for the SDK checks (SMA-749)" -m "Six rewritten streams and a manifest of allowed calls and notices, with
a drift test. The Python SDK and first-party parser checks read them.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 13: The `tool-interception-malformed-response` registry code

One reason across the stack, in one commit. The TS `PRESENTATION` table is a total `Record`, so the proto change and the SDK table must land together, the same way SMA-677 Task 1 did it.

**Files:**
- Modify: `contracts/proto/paigasus/common/v1/error.proto` (after `ERROR_REASON_BUDGET_EXHAUSTED = 312;`)
- Regenerate: `rs/crates/libs/paigasus-proto/src/generated/**`, `py/packages/paigasus-proto/**/generated/**`, `ts/packages/paigasus-proto/src/generated/**`
- Modify: `rs/crates/libs/paigasus-proto/src/error.rs` (`EXPECTED_REASONS`, gateway block after `"budget-exhausted",`; the count 69 → 70)
- Modify: `ts/packages/paigasus-proto/src/error.test.ts:60` (69 → 70, plus its comment's Rust line number)
- Modify: `ts/packages/paigasus-sdk/src/errors/presentation.ts` (one table row; the `Exclude` comment's key count, if it names one)
- Modify: `ts/packages/paigasus-sdk/tests/presentation.test.ts:13` (69 → 70), `tests/map-error.test.ts` (one new case)
- Modify: `rs/crates/services/paigasus-gateway/src/adapters/http/error.rs` (variant, `parts`, `retryable`, the exhaustive retryability test)

**Interfaces:**
- Produces:
  - `ErrorReason::ToolInterceptionMalformedResponse` (313, wire `tool-interception-malformed-response`).
  - `GatewayError::ToolInterceptionMalformedResponse`, which maps to `502`, type `api_error`, code `tool-interception-malformed-response`, no `param`, the message `The upstream model provider returned a response the gateway could not check.`, and `Retryable::Unknown`.

- [ ] **Step 1: Write the failing count test**

In `rs/crates/libs/paigasus-proto/src/error.rs`, after `"budget-exhausted",` in the gateway block of `EXPECTED_REASONS`, add:

```rust
        // Gateway tool interception (SMA-749)
        "tool-interception-malformed-response",
```

and change `assert_eq!(actual.len(), 69, "the registry should hold 69 reasons");` to `70` in both places on that line.

```bash
cd rs && cargo nextest run --locked -p paigasus-proto the_registry_contains_exactly_the_expected_reasons
```

Expected: FAIL with `declared in the test but not in the registry: ["tool-interception-malformed-response"]`.

- [ ] **Step 2: Add the reason and regenerate**

In `contracts/proto/paigasus/common/v1/error.proto`, after `ERROR_REASON_BUDGET_EXHAUSTED = 312;`:

```proto
  // "tool-interception-malformed-response" — tool-call interception is on and the
  // upstream answered a non-stream request with a 2xx body the gateway could not
  // classify (not strict JSON, or not the provider's shape), so it could not check
  // the tool calls in it (502, OpenAI type "api_error"). Sent only with
  // [tool_interception] on_failure = "block" (SMA-749).
  ERROR_REASON_TOOL_INTERCEPTION_MALFORMED_RESPONSE = 313;
```

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
(cd contracts && buf format -w)
moon run contracts:generate
git status --short contracts rs/crates/libs/paigasus-proto py/packages/paigasus-proto ts/packages/paigasus-proto
```

Expected: the three generated trees change. If `ts/packages/paigasus-proto/src/generated/google/rpc/error_details_pb.ts` shows as deleted, the BSR rate limit hit: run `git checkout -- ts/packages/paigasus-proto/src/generated/google/rpc/error_details_pb.ts`, then run `moon run contracts:generate` again.

- [ ] **Step 3: Update the TS mirrors**

```bash
grep -n "should hold 70" rs/crates/libs/paigasus-proto/src/error.rs
```

In `ts/packages/paigasus-proto/src/error.test.ts`, change `expect(values).toHaveLength(69);` (line 60) to `70`. Point the comment above it at the line number the grep printed.

In `ts/packages/paigasus-sdk/src/errors/presentation.ts`, after `[ErrorReason.BUDGET_EXHAUSTED]: 'quota-exhausted',`:

```ts
  // SMA-749. The gateway could not check a non-stream answer for tool calls, so it refused to
  // forward it. A 502 like an upstream failure: the transport table's `degraded` copy fits.
  [ErrorReason.TOOL_INTERCEPTION_MALFORMED_RESPONSE]: 'from-transport',
```

```bash
grep -n "rather than\|keys" ts/packages/paigasus-sdk/src/errors/presentation.ts | head
```

If a comment there states a key count (SMA-677 left "demanding an entry for it would make the table N keys rather than M"), increase both numbers by one.

In `ts/packages/paigasus-sdk/tests/presentation.test.ts:13`, change `toHaveLength(69)` to `toHaveLength(70)`.

In `ts/packages/paigasus-sdk/tests/map-error.test.ts`, directly after the `it('maps budget-exhausted to quota-exhausted, not retryable', …)` case:

```ts
  // SMA-749. The gateway refused an upstream answer it could not check for tool calls.
  it('maps tool-interception-malformed-response to degraded, retry unknown', () => {
    const result = mapError({
      kind: 'http',
      status: 502,
      headers: new Headers(),
      body: { error: { message: 'The upstream model provider returned a response the gateway could not check.', type: 'api_error', param: null, code: 'tool-interception-malformed-response' } },
    });
    expect(result.reason).toBe(ErrorReason.TOOL_INTERCEPTION_MALFORMED_RESPONSE);
    expect(result.presentation).toBe('degraded');
  });
```

Before you trust the `degraded` expectation, read how `'from-transport'` resolves a 502 in `src/errors/transport-status.ts`. If 502 maps to a different presentation there, use that value in the test, and say so in the task report.

- [ ] **Step 4: Add the gateway variant**

In `rs/crates/services/paigasus-gateway/src/adapters/http/error.rs`, after the `BudgetExhausted { … }` variant:

```rust
    // ---- tool interception (SMA-749) ---------------------------------------------------------
    /// Interception is on with `on_failure = "block"`, and a 2xx non-stream answer could not be
    /// classified, so its tool calls could not be checked → 502. Retry unknown: another answer
    /// from the same upstream may or may not be well-formed.
    ToolInterceptionMalformedResponse,
```

In `parts()`, after the `BudgetExhausted` arm:

```rust
            GatewayError::ToolInterceptionMalformedResponse => (
                StatusCode::BAD_GATEWAY,
                "api_error",
                Some("tool-interception-malformed-response"),
                None,
                "The upstream model provider returned a response the gateway could not check.",
            ),
```

In `retryable()`, add `| Self::ToolInterceptionMalformedResponse` to the `Retryable::Unknown` arm (`Self::Internal | Self::MissingScope`).

Find the exhaustive retryability test:

```bash
grep -n "D4's table, asserted exhaustively" -A30 rs/crates/services/paigasus-gateway/src/adapters/http/error.rs
```

Add the new variant to that test, with the expectation `Retryable::Unknown`, in the same style as `Internal`.

- [ ] **Step 5: Run every registry check**

```bash
cd rs && cargo nextest run --locked -p paigasus-proto && cargo nextest run --locked -p paigasus-gateway error:: && cd ..
moon run paigasus-proto-ts:test ts:fmt paigasus-sdk-ts:typecheck paigasus-sdk-ts:test gateway-console-ts:typecheck iam-console-ts:typecheck
moon run repo:error-code-single-site contracts:breaking
```

Expected: PASS. `every_gateway_code_is_declared_in_the_canonical_registry` covers the new variant through `strum`. `contracts:breaking` passes, because an added enum value is not breaking. In a worktree it can fail with a gitlink error. In that case run `buf breaking` with `--against "$(git rev-parse --git-common-dir)#branch=main"`, from `contracts/`.

- [ ] **Step 6: Commit**

```bash
git add contracts/proto/paigasus/common/v1/error.proto rs/crates/libs/paigasus-proto py/packages/paigasus-proto ts/packages/paigasus-proto ts/packages/paigasus-sdk rs/crates/services/paigasus-gateway/src/adapters/http/error.rs
git commit -m "feat(contracts): add the tool-interception-malformed-response gateway reason (SMA-749)" -m "Registry reason 313 for a non-stream answer the gateway could not check
for tool calls, regenerated for Rust, Python and TypeScript, with the
GatewayError variant (502, retry unknown) and the SDK mapping.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 14: The four metric names

**Files:**
- Modify: `rs/crates/libs/paigasus-observability/src/names.rs`

**Interfaces:**
- Produces the following names. No label carries a tool name, an argument, a principal or an org.

| Constant | Name | Type and labels |
|---|---|---|
| `GATEWAY_TOOL_INTERCEPTION_RECORDS_DROPPED_TOTAL` | `gateway_tool_interception_records_dropped_total` | counter, `reason` = `malformed` \| `oversized` \| `after_done` \| `late_fragment` |
| `GATEWAY_TOOL_INTERCEPTION_DECISIONS_TOTAL` | `gateway_tool_interception_decisions_total` | counter, `decision` = `allow` \| `block`, `basis` = `decider` \| `on_failure` |
| `GATEWAY_TOOL_INTERCEPTION_ON_FAILURE_TOTAL` | `gateway_tool_interception_on_failure_total` | counter, `code` = `hold_limit` \| `truncated` \| `decider_error` \| `malformed` |
| `GATEWAY_TOOL_INTERCEPTION_HOLD_SECONDS` | `gateway_tool_interception_hold_seconds` | histogram, no label |

- [ ] **Step 1: Write the failing test**

In `names.rs` `mod tests`, after `the_gateway_limit_families_are_registered`:

```rust
    #[test]
    fn the_gateway_tool_interception_families_are_registered() {
        for name in [
            GATEWAY_TOOL_INTERCEPTION_RECORDS_DROPPED_TOTAL,
            GATEWAY_TOOL_INTERCEPTION_DECISIONS_TOTAL,
            GATEWAY_TOOL_INTERCEPTION_ON_FAILURE_TOTAL,
            GATEWAY_TOOL_INTERCEPTION_HOLD_SECONDS,
        ] {
            assert!(ALL.contains(&name), "{name} is missing from ALL");
        }
    }
```

```bash
cd rs && cargo nextest run --locked -p paigasus-observability names::
```

Expected: FAIL to compile (the constants do not exist yet).

- [ ] **Step 2: Add the names**

After the gateway-limits block (after `GATEWAY_REDIS_BREAKER_TRANSITIONS_TOTAL`'s definition):

```rust
// Gateway tool-call interception (SMA-749). No label carries a tool name, an argument, a principal
// or an org id.
/// Upstream SSE records the interceptor did not forward; `reason` = `malformed` | `oversized` |
/// `after_done` | `late_fragment`. A conforming upstream keeps this at 0.
pub const GATEWAY_TOOL_INTERCEPTION_RECORDS_DROPPED_TOTAL: &str = "gateway_tool_interception_records_dropped_total";
/// One per decided tool call; `decision` = `allow` | `block`, `basis` = `decider` | `on_failure`.
pub const GATEWAY_TOOL_INTERCEPTION_DECISIONS_TOTAL: &str = "gateway_tool_interception_decisions_total";
/// Failures that took the `on_failure` path; `code` = `hold_limit` | `truncated` |
/// `decider_error` | `malformed`.
pub const GATEWAY_TOOL_INTERCEPTION_ON_FAILURE_TOTAL: &str = "gateway_tool_interception_on_failure_total";
/// Seconds from a hold's first tool-call frame to its completion.
pub const GATEWAY_TOOL_INTERCEPTION_HOLD_SECONDS: &str = "gateway_tool_interception_hold_seconds";
```

Add the four constants to `ALL`, directly after `GATEWAY_REDIS_BREAKER_TRANSITIONS_TOTAL,`.

- [ ] **Step 3: Run the tests and the drift gate**

```bash
cd rs && cargo fmt && cargo nextest run --locked -p paigasus-observability && cd .. && moon run repo:observability-drift
```

Expected: PASS (including `all_names_are_unique_and_snake_case`).

- [ ] **Step 4: Commit**

```bash
git add rs/crates/libs/paigasus-observability/src/names.rs
git commit -m "feat(rs): register the gateway tool-interception metric names (SMA-749)" -m "Dropped records, decisions, on_failure paths and hold time, with closed
label sets only.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 15: The `[tool_interception]` config table

**Files:**
- Modify: `rs/crates/services/paigasus-gateway/src/config.rs`, `rs/crates/services/paigasus-gateway/Cargo.toml` (adds `paigasus-gateway-core`), `rs/crates/services/paigasus-gateway/moon.yml`, `rs/crates/services/paigasus-gateway/gateway.toml.example`, `rs/Cargo.lock`

**Interfaces:**
- Consumes: `paigasus_gateway_core::{Limits, OnFailure}`.
- Produces:
  - `pub struct ToolInterceptionConfig { pub enabled: bool, pub max_record_bytes: usize, pub max_held_bytes: usize, pub max_hold_secs: u64, pub decide_timeout_secs: u64, pub keepalive_secs: u64, pub on_failure: OnFailureConfig }`, with `fn validate(&self) -> Result<(), String>` and `fn limits(&self) -> paigasus_gateway_core::Limits`.
  - `pub enum OnFailureConfig { Block, Allow }`. It deserializes from `"block"` and `"allow"`.
  - `GatewayConfig::tool_interception: ToolInterceptionConfig`, with `#[serde(default)]`.

- [ ] **Step 1: Add the dependency and the Moon edge**

`rs/crates/services/paigasus-gateway/Cargo.toml` `[dependencies]`, after `paigasus-kernel`:

```toml
# SMA-749: the sans-IO tool-call interception core. `{ workspace = true }`, so `moon.yml`
# hand-declares the `dependsOn` edge (ci/CLAUDE.md).
paigasus-gateway-core = { workspace = true }
```

`rs/crates/services/paigasus-gateway/moon.yml`: add `  - 'paigasus-gateway-core-rs'` to `dependsOn`, directly after `  - 'paigasus-kernel-rs'`. In `fileGroups.upstreams`, add these lines before the two `paigasus-kernel` lines (the list is sorted):

```yaml
    - '/rs/crates/libs/paigasus-gateway-core/src/**/*'
    - '/rs/crates/libs/paigasus-gateway-core/Cargo.toml'
```

- [ ] **Step 2: Write the failing tests**

In `src/config.rs` `mod tests`:

```rust
    fn ti_toml(body: &str) -> String {
        format!("{}\n[tool_interception]\n{body}\n", valid_toml())
    }

    #[test]
    fn tool_interception_is_off_by_default_with_the_spec_limits() {
        figment::Jail::expect_with(|jail| {
            jail.create_file("gateway.toml", valid_toml())?;
            let cfg = GatewayConfig::figment().extract::<GatewayConfig>()?;
            let ti = &cfg.tool_interception;
            assert!(!ti.enabled);
            assert_eq!((ti.max_record_bytes, ti.max_held_bytes, ti.max_hold_secs, ti.decide_timeout_secs, ti.keepalive_secs), (1_048_576, 4_194_304, 300, 5, 10));
            assert_eq!(ti.on_failure, OnFailureConfig::Block);
            assert_eq!(ti.limits(), paigasus_gateway_core::Limits::defaults());
            cfg.validate().map_err(figment::Error::from)?;
            Ok(())
        });
    }

    #[test]
    fn tool_interception_reads_the_table_and_the_environment() {
        figment::Jail::expect_with(|jail| {
            jail.create_file("gateway.toml", &ti_toml("enabled = true\non_failure = \"allow\"\nmax_hold_secs = 120"))?;
            jail.set_env("GATEWAY_TOOL_INTERCEPTION__KEEPALIVE_SECS", "15");
            let cfg = GatewayConfig::figment().extract::<GatewayConfig>()?;
            assert!(cfg.tool_interception.enabled);
            assert_eq!(cfg.tool_interception.on_failure, OnFailureConfig::Allow);
            assert_eq!(cfg.tool_interception.limits().max_hold, std::time::Duration::from_secs(120));
            assert_eq!(cfg.tool_interception.keepalive_secs, 15);
            Ok(())
        });
    }

    #[test]
    fn a_misspelt_tool_interception_key_fails_extraction() {
        figment::Jail::expect_with(|jail| {
            jail.create_file("gateway.toml", &ti_toml("enabeld = true"))?;
            let err = GatewayConfig::figment().extract::<GatewayConfig>().expect_err("a typo must not leave interception silently off");
            assert!(err.to_string().contains("unknown field"), "{err}");
            Ok(())
        });
    }

    #[test]
    fn tool_interception_validation_names_each_key() {
        for (body, key) in [
            ("max_record_bytes = 0", "tool_interception.max_record_bytes"),
            ("max_held_bytes = 0", "tool_interception.max_held_bytes"),
            ("max_hold_secs = 0", "tool_interception.max_hold_secs"),
            ("decide_timeout_secs = 0", "tool_interception.decide_timeout_secs"),
            ("keepalive_secs = 0", "tool_interception.keepalive_secs"),
            ("max_record_bytes = 10\nmax_held_bytes = 5", "tool_interception.max_record_bytes"),
            ("keepalive_secs = 300", "tool_interception.keepalive_secs"),
        ] {
            figment::Jail::expect_with(|jail| {
                jail.create_file("gateway.toml", &ti_toml(body))?;
                let cfg = GatewayConfig::figment().extract::<GatewayConfig>()?;
                let err = cfg.validate().expect_err(body);
                assert!(err.contains(key), "{body}: {err}");
                Ok(())
            });
        }
    }
```

`figment::Error::from(String)` exists, and it is the conversion that the other validation tests in this module use. If they use a different conversion, copy theirs.

```bash
cd rs && cargo nextest run --locked -p paigasus-gateway config::tests::tool_interception config::tests::a_misspelt_tool
```

Expected: FAIL to compile.

- [ ] **Step 3: Implement**

In `src/config.rs`, after `GatewayDpopConfig`:

```rust
/// `[tool_interception]` (SMA-749 D26). Off by default: with `enabled = false` the chat relay is
/// exactly the relay without interception (D27). A misspelt key fails extraction, so a typo can
/// never leave a security control silently off.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct ToolInterceptionConfig {
    pub enabled: bool,
    /// D8: one SSE record or one non-stream body larger than this is a failure, never skipped.
    pub max_record_bytes: usize,
    /// D13: the queued bytes of one stream, across every hold.
    pub max_held_bytes: usize,
    /// D14: from a hold's first frame to its completion. The default equals the stream idle
    /// timeout: a file-writing call with 40 KB of arguments takes about 200 s.
    pub max_hold_secs: u64,
    /// D14: how long the decider may take for one hold.
    pub decide_timeout_secs: u64,
    /// D15: the keep-alive comment interval while a call is held.
    pub keepalive_secs: u64,
    /// D16: `block` (default) or `allow` (observe mode).
    pub on_failure: OnFailureConfig,
}

/// D16.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OnFailureConfig {
    #[default]
    Block,
    Allow,
}

impl Default for ToolInterceptionConfig {
    fn default() -> Self {
        let l = paigasus_gateway_core::Limits::defaults();
        ToolInterceptionConfig {
            enabled: false,
            max_record_bytes: l.max_record_bytes,
            max_held_bytes: l.max_held_bytes,
            max_hold_secs: l.max_hold.as_secs(),
            decide_timeout_secs: l.decide_timeout.as_secs(),
            keepalive_secs: l.keepalive.as_secs(),
            on_failure: OnFailureConfig::Block,
        }
    }
}

impl ToolInterceptionConfig {
    /// Every rule of spec § 5.2. Each message names the key. Checked even when off, so turning it
    /// on later never meets a config that was always invalid.
    pub fn validate(&self) -> Result<(), String> {
        for (key, v) in [
            ("tool_interception.max_record_bytes", self.max_record_bytes as u64),
            ("tool_interception.max_held_bytes", self.max_held_bytes as u64),
            ("tool_interception.max_hold_secs", self.max_hold_secs),
            ("tool_interception.decide_timeout_secs", self.decide_timeout_secs),
            ("tool_interception.keepalive_secs", self.keepalive_secs),
        ] {
            if v == 0 {
                return Err(format!("{key} must be at least 1"));
            }
        }
        if self.max_record_bytes > self.max_held_bytes {
            return Err(format!("tool_interception.max_record_bytes ({}) must not exceed tool_interception.max_held_bytes ({})", self.max_record_bytes, self.max_held_bytes));
        }
        if self.keepalive_secs >= self.max_hold_secs {
            return Err(format!("tool_interception.keepalive_secs ({}) must be less than tool_interception.max_hold_secs ({})", self.keepalive_secs, self.max_hold_secs));
        }
        Ok(())
    }

    #[must_use]
    pub fn limits(&self) -> paigasus_gateway_core::Limits {
        paigasus_gateway_core::Limits {
            max_record_bytes: self.max_record_bytes,
            max_held_bytes: self.max_held_bytes,
            max_hold: std::time::Duration::from_secs(self.max_hold_secs),
            decide_timeout: std::time::Duration::from_secs(self.decide_timeout_secs),
            keepalive: std::time::Duration::from_secs(self.keepalive_secs),
            on_failure: match self.on_failure {
                OnFailureConfig::Block => paigasus_gateway_core::OnFailure::Block,
                OnFailureConfig::Allow => paigasus_gateway_core::OnFailure::Allow,
            },
        }
    }
}
```

In `GatewayConfig`, after `dpop`:

```rust
    /// SMA-749: tool-call interception on `POST /v1/chat/completions`. Off by default (D26).
    #[serde(default)]
    pub tool_interception: ToolInterceptionConfig,
```

In `GatewayConfig::validate`, directly before the final `Ok(())`:

```rust
        self.tool_interception.validate()?;
```

Every literal `GatewayConfig { … }` in the tests (if there are any) needs the new field:

```bash
grep -rn "GatewayConfig {" rs/crates/services/paigasus-gateway/src rs/crates/services/paigasus-gateway/tests
```

- [ ] **Step 4: Document the table**

In `rs/crates/services/paigasus-gateway/gateway.toml.example`, after the `[limits]` section block (before the next `# ---` heading or at the end of the file), add:

```toml
# --- Tool-call interception (SMA-749): hold, check and rewrite model tool calls ---
#
# Off by default. With enabled = true the gateway parses every response of
# POST /v1/chat/completions, holds each tool call until its arguments are complete (text keeps
# streaming), asks the decider, and replaces a blocked call with an assistant notice. This build's
# decider allows every call. Each decision is reported as an SSE comment ": paigasus-decision {…}"
# or, for a non-stream answer, in the paigasus-tool-decisions header. A misspelt key fails the boot.
#
# [tool_interception]
# enabled             = false      # env GATEWAY_TOOL_INTERCEPTION__ENABLED
# max_record_bytes    = 1048576    # one SSE record or one non-stream body; larger is a failure
# max_held_bytes      = 4194304    # all held bytes of one stream
# max_hold_secs       = 300        # from a tool call's first fragment to its completion
# decide_timeout_secs = 5
# keepalive_secs      = 10         # ": keep-alive" comment while a call is held; < max_hold_secs
# on_failure          = "block"    # "block" (default) | "allow" (observe: forward and report)
#   # A failure is: a record or body that is not strict JSON or not the provider's shape, a
#   # limit above, a decider error, or a stream that ends inside a tool call. With "block", a
#   # non-stream answer that cannot be checked is a 502 "tool-interception-malformed-response".
```

- [ ] **Step 5: Run the tests and the affected-graph suite**

```bash
cd rs && cargo fmt && cargo nextest run --locked -p paigasus-gateway && cd ..
/bin/bash ci/affected-graph/run.sh
```

Expected: every gateway test passes, including the existing ones (AC 1), and the Task 0 count goes up by exactly the four new tests. If `run.sh` reds on `kernel->consumer-tasks` or a gateway case, it is the new `dependsOn` edge. Read the case's expected set and add `paigasus-gateway-core-rs:*` only where the suite asks for it. Task 16 adds the crate's own `gateway-core->consumer-tasks` case.

- [ ] **Step 6: Commit**

```bash
git add rs/crates/services/paigasus-gateway rs/Cargo.lock
git commit -m "feat(rs): add the gateway [tool_interception] config table (SMA-749)" -m "Off by default, with the spec's limits, a misspelt key failing the
boot, validation that names each key, and the example config section.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---
### Task 16: Wire the interceptor into the streaming chat relay

**Files:**
- Create: `rs/crates/services/paigasus-gateway/src/adapters/http/interception.rs`, `rs/crates/services/paigasus-gateway/tests/tool_interception.rs`
- Modify: `src/adapters/http/mod.rs` (the `AppState` field, the module line), `src/adapters/http/chat.rs`, `src/main.rs`, `tests/support/limits.rs`, and every other `AppState { … }` literal (`tests/chat_proxy.rs` ×2, `tests/metrics.rs` ×3, `tests/service_info.rs`, `src/adapters/http/mod.rs` tests)
- Modify: `ci/affected-graph/run.sh`, `ci/affected-graph/README.md` (a `gateway-core->consumer-tasks` case)

**Interfaces:**
- Consumes: `paigasus_gateway_core::{Interceptor, Action, Metric, Format, HoldKey, ToolCall, ToolCallDecider, DecideFuture, DecisionContext, ToolTurn, parse_request, Limits, AllowAll}`; `ToolInterceptionConfig::limits` (Task 15); the metric names (Task 14).
- Produces:
  - `pub struct ToolInterception { pub limits: Limits, pub decider: Arc<dyn ToolCallDecider> }` in `adapters::http::interception`.
  - `AppState::tool_interception: Option<Arc<ToolInterception>>`. It is `None` when off (D27).
  - `pub(crate) fn intercepted_stream(inner: OpenAiByteStream, guard: Option<ChargeGuard>, ti: Arc<ToolInterception>, ctx: Arc<DecisionContext>, turn: Arc<ToolTurn>, terminal: Bytes) -> impl Stream<Item = Result<Bytes, Infallible>> + Send + 'static`
  - `pub(crate) fn record(m: Metric)`, which maps a core metric to the registered names.
  - `tests/support/limits.rs`: `pub fn app_with_interception(iam: Arc<dyn Iam>, base_url: &str, limits: Option<Arc<Limits>>, ti: Option<Arc<ToolInterception>>) -> Router`.

The order inside the relay (spec § 5.2, D17, D24, D25):
1. Each upstream chunk goes to `UsageScanner` first, exactly as today, and then to `Interceptor::push`.
2. On the first upstream `Err`, `finish_scan`, then `Interceptor::abort()`. The interceptor emits the terminal frame, so the relay does not emit it a second time.
3. On the upstream end, `finish_scan`, then `Interceptor::finish(now)`.
4. Pending decisions run one at a time (P-5). The decider future, the `tokio::time::Sleep` for `Wake`, and the upstream are polled in one `tokio::select!`, biased in that order.
5. Nothing is spawned. The unfold state owns the upstream stream, the guard and the future, so a client disconnect drops all three, exactly as today.

- [ ] **Step 1: Add the `AppState` field everywhere**

In `src/adapters/http/mod.rs`, add `pub mod interception;` next to the other `mod` lines, and add this field to `AppState`, after `dpop_enabled`:

```rust
    /// SMA-749: tool-call interception, or `None` when `[tool_interception] enabled = false`.
    /// `None` keeps the relay exactly as it was (D27).
    pub tool_interception: Option<Arc<interception::ToolInterception>>,
```

Add `tool_interception: None,` to every `AppState { … }` literal:

```bash
grep -rn "AppState {" rs/crates/services/paigasus-gateway/src rs/crates/services/paigasus-gateway/tests
```

In `src/main.rs`, in the `AppState { … }` literal:

```rust
        tool_interception: config.tool_interception.enabled.then(|| {
            tracing::info!("tool-call interception is on (SMA-749): tool calls in chat completions are held, checked and may be rewritten");
            Arc::new(ToolInterception { limits: config.tool_interception.limits(), decider: Arc::new(paigasus_gateway_core::AllowAll) })
        }),
```

with `use paigasus_gateway::adapters::http::interception::ToolInterception;` (or the crate-internal path that `main.rs` uses for `AppState`).

In `tests/support/limits.rs`, change `app_with` to delegate, and add the new builder:

```rust
pub fn app_with(iam: Arc<dyn Iam>, base_url: &str, limits: Option<Arc<Limits>>, first_byte: Duration, chat_stream: bool) -> Router {
    build(iam, base_url, limits, first_byte, chat_stream, None)
}

/// SMA-749: the router with tool-call interception set.
pub fn app_with_interception(iam: Arc<dyn Iam>, base_url: &str, limits: Option<Arc<Limits>>, ti: Option<Arc<ToolInterception>>) -> Router {
    build(iam, base_url, limits, Duration::from_secs(30), true, ti)
}

fn build(iam: Arc<dyn Iam>, base_url: &str, limits: Option<Arc<Limits>>, first_byte: Duration, chat_stream: bool, tool_interception: Option<Arc<ToolInterception>>) -> Router {
    router(AppState {
        iam,
        openai: Arc::new(openai(base_url, first_byte)),
        max_request_bytes: 1_048_576,
        capabilities: Capabilities { chat_stream },
        limits,
        dpop_enabled: false,
        tool_interception,
    })
}
```

The body of `build` is the old body of `app_with`. Keep any fields that `app_with` sets today and this sketch omits.

- [ ] **Step 2: Write the failing integration tests**

`rs/crates/services/paigasus-gateway/tests/tool_interception.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0
//! SMA-749: tool-call interception through the real router against the mock upstream.

mod support;

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use axum::http::StatusCode;
use futures::StreamExt;
use paigasus_gateway::adapters::http::interception::ToolInterception;
use paigasus_gateway_core::{AllowAll, BlockCode, DecideFuture, Decision, DecisionContext, Limits, ToolCall, ToolCallDecider, ToolTurn};
use support::MockOpenAi;
use support::limits::*;

/// Blocks every call whose name starts with `blocked_`.
struct BlockNamed;

impl ToolCallDecider for BlockNamed {
    fn decide(&self, _ctx: Arc<DecisionContext>, _turn: Arc<ToolTurn>, calls: Vec<ToolCall>) -> DecideFuture {
        let d = calls.iter().map(|c| if c.name.starts_with("blocked_") { Decision::Block { code: BlockCode::Policy } } else { Decision::Allow }).collect();
        Box::pin(std::future::ready(Ok(d)))
    }
}

fn ti(decider: Arc<dyn ToolCallDecider>) -> Option<Arc<ToolInterception>> {
    Some(Arc::new(ToolInterception { limits: Limits::defaults(), decider }))
}

fn tool_chunk(index: u32, id: Option<&str>, name: Option<&str>, args: &str) -> String {
    let mut e = serde_json::json!({"index": index, "function": {"arguments": args}});
    if let Some(id) = id {
        e["id"] = id.into();
        e["type"] = "function".into();
    }
    if let Some(n) = name {
        e["function"]["name"] = n.into();
    }
    format!(
        "data: {}\n\n",
        serde_json::json!({"id":"chatcmpl-1","object":"chat.completion.chunk","created":1790942400,"model":"gpt-4o-mini","system_fingerprint":"fp_1","choices":[{"index":0,"delta":{"tool_calls":[e]},"finish_reason":null}]})
    )
}

fn finish_chunk() -> String {
    format!("data: {}\n\n", serde_json::json!({"id":"chatcmpl-1","object":"chat.completion.chunk","created":1790942400,"model":"gpt-4o-mini","choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}))
}

fn upstream() -> Vec<String> {
    vec![
        chunk("Sure.", true),
        tool_chunk(0, Some("call_0"), Some("read_file"), "{\"p\":"),
        tool_chunk(0, None, None, "1}"),
        tool_chunk(1, Some("call_1"), Some("blocked_rm"), "{\"p\":\"CANARY\"}"),
        finish_chunk(),
        usage_chunk(11),
        "data: [DONE]\n\n".to_owned(),
    ]
}

fn payloads(body: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(body).split("\n\n").filter_map(|f| f.lines().find_map(|l| l.strip_prefix("data: ")).map(str::to_owned)).collect()
}

/// AC 1: off means today's relay, byte for byte.
#[tokio::test]
async fn interception_off_is_byte_identical() {
    let records = upstream();
    let mock = MockOpenAi::spawn_raw_sse(records.clone()).await;
    let app = app_with_interception(ScopedIam::arc(PRINCIPAL_1, ORG_A_PRN), &mock.base_url, None, None);
    let sent = send(&app, STREAM_BODY).await;
    assert_eq!(sent.status, StatusCode::OK);
    assert_eq!(sent.text(), records.concat());
}

/// AC 2: allow-all forwards every payload unchanged and adds only comments.
#[tokio::test]
async fn allow_all_is_payload_identical() {
    let records = upstream();
    let mock = MockOpenAi::spawn_raw_sse(records.clone()).await;
    let app = app_with_interception(ScopedIam::arc(PRINCIPAL_1, ORG_A_PRN), &mock.base_url, None, ti(Arc::new(AllowAll)));
    let sent = send(&app, STREAM_BODY).await;
    assert_eq!(payloads(&sent.body), payloads(records.concat().as_bytes()));
    assert!(sent.text().contains(": paigasus-decision {"));
}

/// AC 3: a blocked call never reaches the client.
#[tokio::test]
async fn a_blocked_call_never_reaches_the_client() {
    let mock = MockOpenAi::spawn_raw_sse(upstream()).await;
    let app = app_with_interception(ScopedIam::arc(PRINCIPAL_1, ORG_A_PRN), &mock.base_url, None, ti(Arc::new(BlockNamed)));
    let text = send(&app, STREAM_BODY).await.text();
    assert!(!text.contains("CANARY") && !text.contains("blocked_rm\","), "{text}");
    assert!(text.contains(r#"[paigasus] tool call \"blocked_rm\" was blocked (policy)."#), "{text}");
    assert!(text.contains("\"read_file\""));
    assert!(text.ends_with("data: [DONE]\n\n"));
}

/// D17: an upstream error record mid-hold drops the held call and ends with the terminal frame.
#[tokio::test]
async fn an_upstream_error_mid_hold_ends_with_the_terminal_frame() {
    let records = vec![tool_chunk(0, Some("call_0"), Some("read_file"), "{\"CANARY\":"), "data: {\"error\":{\"message\":\"boom\"}}\n\n".to_owned()];
    let mock = MockOpenAi::spawn_raw_sse(records).await;
    let app = app_with_interception(ScopedIam::arc(PRINCIPAL_1, ORG_A_PRN), &mock.base_url, None, ti(Arc::new(AllowAll)));
    let text = send(&app, STREAM_BODY).await.text();
    assert!(!text.contains("CANARY"), "{text}");
    assert!(text.contains("\"code\":\"upstream-error\""), "{text}");
}

/// D24 (Review focus 5): the charge follows the upstream's usage record, block or no block.
#[tokio::test]
async fn a_blocked_stream_charges_like_the_upstream() {
    let mock = MockOpenAi::spawn_raw_sse(upstream()).await;
    let store = RecordingStore::new();
    let app = app_with_interception(ScopedIam::arc(PRINCIPAL_1, ORG_A_PRN), &mock.base_url, Some(limits(rules(None, None, Some(1_000_000)), store.clone())), ti(Arc::new(BlockNamed)));
    let sent = send(&app, STREAM_BODY).await;
    assert_eq!(sent.status, StatusCode::OK);
    assert_eq!(store.charges(), vec![11]);
}

/// D25: a client disconnect still cancels the upstream with interception on.
#[tokio::test]
async fn client_abort_cancels_upstream_with_interception_on() {
    let (mock, cancelled) = MockOpenAi::spawn_abortable_stream().await;
    let app = app_with_interception(ScopedIam::arc(PRINCIPAL_1, ORG_A_PRN), &mock.base_url, None, ti(Arc::new(AllowAll)));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/v1/chat/completions"))
        .header(reqwest::header::AUTHORIZATION, format!("Bearer {CALLER_KEY}"))
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(STREAM_BODY)
        .send()
        .await
        .expect("responds");
    let mut stream = resp.bytes_stream();
    // The mock's one frame is text, so it is forwarded at once.
    let _ = tokio::time::timeout(Duration::from_secs(2), stream.next()).await.expect("a first chunk");
    drop(stream);
    let propagated = tokio::time::timeout(Duration::from_secs(2), async {
        while !cancelled.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    assert!(propagated.is_ok(), "cancel-on-drop must survive interception");
    server.abort();
}
```

Check two things before relying on these tests:
- `CALLER_KEY`, `PRINCIPAL_1`, `ORG_A_PRN`, `STREAM_BODY`, `chunk`, `usage_chunk`, `send`, `RecordingStore`, `limits` and `rules` exist in `tests/support/limits.rs` or `tests/support/mod.rs`. Read both files, and import from where each name lives.
- If `spawn_abortable_stream`'s single frame is not a plain text delta, the first-chunk read can block. Read its `MockResponse::AbortableStream` arm. If the frame is not a JSON text chunk, assert only the cancel flag, with no first-chunk read.

```bash
cd rs && cargo nextest run --locked -p paigasus-gateway --test tool_interception
```

Expected: FAIL to compile (`interception::ToolInterception` does not exist).

- [ ] **Step 3: Implement the relay**

`src/adapters/http/interception.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0
//! SMA-749: drives `paigasus-gateway-core` inside the chat relay (spec § 5.2).
//!
//! The interceptor is synchronous; this module owns the I/O around it: the upstream stream, one
//! pending decider future, and one timer. Nothing is spawned. Dropping the response body drops
//! the unfold state, and with it the upstream request (cancel-on-drop, D25) and any pending
//! decision.
//!
//! Like the plain relay, this runs with `current_ids() == None` (SMA-504 § 4.3): the request id
//! reaches the decider through `DecisionContext`, captured in the handler.

use std::collections::VecDeque;
use std::convert::Infallible;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;

use axum::body::Bytes;
use futures::{Stream, StreamExt};
use metrics::{counter, histogram};
use paigasus_gateway_core::{Action, DecideFuture, DecisionContext, Format, HoldKey, Interceptor, Limits, Metric, ToolCall, ToolCallDecider, ToolTurn};
use paigasus_observability::names;

use crate::adapters::http::usage::UsageScanner;
use crate::adapters::openai::OpenAiByteStream;
use crate::application::charge_guard::ChargeGuard;

/// What `[tool_interception]` builds at boot.
pub struct ToolInterception {
    pub limits: Limits,
    pub decider: Arc<dyn ToolCallDecider>,
}

/// Map one core metric to the registered names (P-9).
pub(crate) fn record(m: Metric) {
    match m {
        Metric::RecordDropped(reason) => counter!(names::GATEWAY_TOOL_INTERCEPTION_RECORDS_DROPPED_TOTAL, "reason" => reason).increment(1),
        Metric::Decision { decision, basis } => counter!(names::GATEWAY_TOOL_INTERCEPTION_DECISIONS_TOTAL, "decision" => decision, "basis" => basis).increment(1),
        Metric::OnFailure(code) => counter!(names::GATEWAY_TOOL_INTERCEPTION_ON_FAILURE_TOTAL, "code" => code).increment(1),
        Metric::HoldSeconds(s) => histogram!(names::GATEWAY_TOOL_INTERCEPTION_HOLD_SECONDS).record(s),
    }
}

struct Relay {
    inner: Option<OpenAiByteStream>,
    scan: UsageScanner,
    guard: Option<ChargeGuard>,
    icpt: Interceptor,
    ti: Arc<ToolInterception>,
    ctx: Arc<DecisionContext>,
    turn: Arc<ToolTurn>,
    out: VecDeque<Bytes>,
    decide_queue: VecDeque<(HoldKey, Vec<ToolCall>)>,
    pending: Option<(HoldKey, DecideFuture)>,
    sleep: Pin<Box<tokio::time::Sleep>>,
}

impl Relay {
    fn apply(&mut self, actions: Vec<Action>) {
        for a in actions {
            match a {
                Action::Emit(b) => self.out.push_back(b),
                Action::Decide { hold, calls } => self.decide_queue.push_back((hold, calls)),
                Action::Wake(at) => self.sleep.as_mut().reset(tokio::time::Instant::from_std(at)),
                Action::Count(m) => record(m),
            }
        }
    }

    /// SMA-730, as in the plain relay: a last record with no blank line still counts.
    fn finish_scan(&mut self) {
        if let Some(guard) = self.guard.as_mut() {
            self.scan.finish();
            guard.set_stream_progress(self.scan.completion_records(), self.scan.reported_total());
        }
    }
}

pub(crate) fn intercepted_stream(
    inner: OpenAiByteStream,
    guard: Option<ChargeGuard>,
    ti: Arc<ToolInterception>,
    ctx: Arc<DecisionContext>,
    turn: Arc<ToolTurn>,
    terminal: Bytes,
) -> impl Stream<Item = Result<Bytes, Infallible>> + Send + 'static {
    let now = Instant::now();
    let icpt = Interceptor::new(Format::OpenAiChat, ti.limits, terminal, now);
    let sleep = Box::pin(tokio::time::sleep(ti.limits.keepalive));
    let start = Relay { inner: Some(inner), scan: UsageScanner::new(), guard, icpt, ti, ctx, turn, out: VecDeque::new(), decide_queue: VecDeque::new(), pending: None, sleep };
    futures::stream::unfold(start, |mut st| async move {
        loop {
            if let Some(b) = st.out.pop_front() {
                return Some((Ok(b), st));
            }
            if st.icpt.is_finished() {
                return None;
            }
            if st.pending.is_none()
                && let Some((hold, calls)) = st.decide_queue.pop_front()
            {
                let fut = st.ti.decider.decide(Arc::clone(&st.ctx), Arc::clone(&st.turn), calls);
                st.pending = Some((hold, fut));
            }
            let Relay { inner, pending, sleep, .. } = &mut st;
            tokio::select! {
                biased;
                r = async {
                    match pending.as_mut() {
                        Some((_, f)) => f.as_mut().await,
                        None => std::future::pending().await,
                    }
                } => {
                    let (hold, _) = st.pending.take().expect("the resolved decision was pending");
                    let acts = st.icpt.decided(hold, r, Instant::now());
                    st.apply(acts);
                }
                () = sleep.as_mut() => {
                    let acts = st.icpt.tick(Instant::now());
                    st.apply(acts);
                }
                chunk = async {
                    match inner.as_mut() {
                        Some(s) => s.next().await,
                        None => std::future::pending().await,
                    }
                } => match chunk {
                    Some(Ok(c)) => {
                        if let Some(guard) = st.guard.as_mut() {
                            st.scan.feed(&c);
                            guard.set_stream_progress(st.scan.completion_records(), st.scan.reported_total());
                        }
                        let acts = st.icpt.push(&c, Instant::now());
                        st.apply(acts);
                    }
                    Some(Err(_)) => {
                        st.finish_scan();
                        st.inner = None;
                        let acts = st.icpt.abort();
                        st.apply(acts);
                    }
                    None => {
                        st.finish_scan();
                        st.inner = None;
                        let acts = st.icpt.finish(Instant::now());
                        st.apply(acts);
                    }
                },
            }
        }
    })
}
```

Two traps:
- **Borrowing in `select!`.** The `select!` branches borrow disjoint fields through the `let Relay { inner, pending, sleep, .. } = &mut st;` destructure. `tokio::select!` drops the branch futures before it runs a handler, so the handlers may use `st` again. If the compiler disagrees, move the three borrows into a helper `async fn next_event(&mut Relay) -> Event` that returns an enum, and handle the enum after it returns.
- **A dead loop.** When the interceptor has ended, `is_finished()` returns before `select!` is reached. So an elapsed `sleep` with no new `Wake` cannot spin.

In `src/adapters/http/chat.rs`:
- After the limits guard block, and before the egress call:

```rust
    // SMA-749: capture what the decider needs while the request ids still exist (§ 4.3 of
    // SMA-504: inside the stream they are `None`). Parsed from the same raw bytes that go upstream.
    let interception = state.tool_interception.clone().map(|ti| {
        let turn = Arc::new(paigasus_gateway_core::parse_request(paigasus_gateway_core::Format::OpenAiChat, &body));
        let request_id = paigasus_observability::current_ids().map(|ids| ids.request_id.to_string());
        let ctx = Arc::new(paigasus_gateway_core::DecisionContext::new(caller.principal_prn.clone(), caller.scope_prn.clone(), request_id));
        (ti, ctx, turn)
    });
```

- In the success stream arm, replace `Body::from_stream(terminal_sse_error_stream(stream, guard.take()))` with:

```rust
                    match interception {
                        Some((ti, ctx, turn)) => Body::from_stream(super::interception::intercepted_stream(stream, guard.take(), ti, ctx, turn, Bytes::from_static(TERMINAL_SSE_ERROR.as_bytes()))),
                        None => Body::from_stream(terminal_sse_error_stream(stream, guard.take())),
                    },
```

Add `use std::sync::Arc;` if `chat.rs` lacks it. Task 17 uses `interception` in the non-stream arm. Until then, the non-stream arm has to stop the compiler from reporting it as unused. `let _ = &interception;` is not acceptable for that. Write the non-stream use in Task 17, and in this task, bind the value only inside the stream arm, like this:

```rust
    let interception = if stream { state.tool_interception.clone().map(|ti| { /* as above */ }) } else { None };
```

Task 17 removes the `if stream` condition.

- [ ] **Step 4: Register the affected-graph case**

In `ci/affected-graph/run.sh`, directly after the `run_case "redis->services" …` case:

```bash
  # paigasus-gateway-core edit -> the lib + the gateway that drives it + the console e2e project
  # whose playground runs the real gateway binary (SMA-749). One-directional: the lib has no
  # in-tree dependency.
  run_case "gateway-core->services" "rs/crates/libs/paigasus-gateway-core/src/lib.rs" \
    "paigasus-gateway-core-rs,paigasus-gateway-rs,gateway-console-ts"
```

Measure the expected set before you trust it:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
printf '%s\n' rs/crates/libs/paigasus-gateway-core/src/lib.rs \
  | moon query projects --affected --downstream deep \
  | python3 -c 'import sys,json; print(",".join(sorted(p["id"] for p in json.load(sys.stdin)["projects"] if p["id"] != "repo")))'
```

Put the printed set in the case, in the order the other cases use. In `ci/affected-graph/README.md`, after the **redis lib edit** bullet:

```markdown
- **gateway-core lib edit** → `paigasus-gateway-core-rs` + `paigasus-gateway-rs` + `gateway-console-ts`
  (SMA-749).
```

- [ ] **Step 5: Run the tests**

```bash
cd rs && cargo fmt && cargo nextest run --locked -p paigasus-gateway && cargo clippy --locked -p paigasus-gateway --all-targets -- -D warnings && cd ..
/bin/bash ci/affected-graph/run.sh --negative-control && /bin/bash ci/affected-graph/run.sh
```

Expected: every gateway test passes. The Task 0 count goes up by the Task 15 config tests and the 6 new integration tests. The affected-graph suite passes.

- [ ] **Step 6: Commit**

```bash
git add rs/crates/services/paigasus-gateway ci/affected-graph rs/Cargo.lock
git commit -m "feat(rs): intercept tool calls in streamed chat completions (SMA-749)" -m "With [tool_interception] on, the relay feeds the upstream bytes to the
usage scanner first and the interceptor second, runs one decider future
and one timer beside the upstream, and keeps cancel-on-drop. Off, the
relay is unchanged.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 17: Intercept non-stream chat completions

**Files:**
- Modify: `rs/crates/services/paigasus-gateway/src/adapters/http/chat.rs`, `src/adapters/http/interception.rs`, `tests/tool_interception.rs`

**Interfaces:**
- Consumes: `paigasus_gateway_core::{parse_body, BodyCalls, Basis, Decision}`; `GatewayError::ToolInterceptionMalformedResponse` (Task 13).
- Produces: `pub(crate) async fn intercept_body(ti: &ToolInterception, ctx: Arc<DecisionContext>, turn: Arc<ToolTurn>, body: Bytes) -> BodyOutcome`, with `pub(crate) enum BodyOutcome { Forward { body: Bytes, report: Option<String> }, Refuse }`.

The order, per spec § 5.2:
1. `non_stream_usage` reads the ORIGINAL body (today's code, unchanged).
2. Only for a 2xx with interception on: `parse_body(Format::OpenAiChat, &body, limits.max_record_bytes)`.
   - On `Err`, `block` gives `Refuse`, which becomes the 502 `ToolInterceptionMalformedResponse`. The charge is already set from the original body and stays (D24). `allow` gives `Forward` with the original body and no report.
   - With no calls and no unenforceable entries, the body is forwarded unchanged, with no header.
   - Otherwise the decider runs under `tokio::time::timeout(decide_timeout)`. A timeout, an error, or the wrong length follows `on_failure`: with `block` every call becomes `Block(DeciderError)`, and with `allow` every call becomes `Allow` with basis `on_failure`.
   - Then `apply(decisions)`. `None` forwards the original bytes, `Some(rewritten)` forwards the rewritten bytes. The `paigasus-tool-decisions` header carries `report(decisions, basis)`.
3. A non-2xx answer is forwarded as today.

- [ ] **Step 1: Write the failing tests** (append to `tests/tool_interception.rs`)

```rust
fn body_with_calls() -> String {
    serde_json::json!({"id":"chatcmpl-1","object":"chat.completion","created":1790942400,"model":"gpt-4o-mini",
        "choices":[{"index":0,"message":{"role":"assistant","content":null,"tool_calls":[
            {"id":"c0","type":"function","function":{"name":"read_file","arguments":"{}"}},
            {"id":"c1","type":"function","function":{"name":"blocked_rm","arguments":"{\"p\":\"CANARY\"}"}}]},"finish_reason":"tool_calls"}],
        "usage":{"prompt_tokens":2,"completion_tokens":3,"total_tokens":13}}).to_string()
}

#[tokio::test]
async fn a_non_stream_allow_all_body_is_forwarded_unchanged_with_a_header() {
    let body = body_with_calls();
    let mock = MockOpenAi::spawn_json(StatusCode::OK, body.clone()).await;
    let app = app_with_interception(ScopedIam::arc(PRINCIPAL_1, ORG_A_PRN), &mock.base_url, None, ti(Arc::new(AllowAll)));
    let sent = send(&app, NON_STREAM_BODY).await;
    assert_eq!(sent.text(), body);
    let h: Vec<serde_json::Value> = serde_json::from_str(sent.header("paigasus-tool-decisions")).expect("an ASCII JSON header");
    assert_eq!(h.len(), 2);
}

#[tokio::test]
async fn a_non_stream_blocked_call_is_rewritten() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, body_with_calls()).await;
    let app = app_with_interception(ScopedIam::arc(PRINCIPAL_1, ORG_A_PRN), &mock.base_url, None, ti(Arc::new(BlockNamed)));
    let sent = send(&app, NON_STREAM_BODY).await;
    assert_eq!(sent.status, StatusCode::OK);
    assert!(!sent.text().contains("CANARY"));
    let v = sent.json();
    assert_eq!(v["choices"][0]["message"]["tool_calls"].as_array().unwrap().len(), 1);
    assert!(v["choices"][0]["message"]["content"].as_str().unwrap().contains("(policy)"));
}

#[tokio::test]
async fn a_non_stream_body_without_calls_gets_no_header() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, USAGE_BODY).await;
    let app = app_with_interception(ScopedIam::arc(PRINCIPAL_1, ORG_A_PRN), &mock.base_url, None, ti(Arc::new(AllowAll)));
    let sent = send(&app, NON_STREAM_BODY).await;
    assert_eq!(sent.text(), USAGE_BODY);
    assert!(sent.headers.get("paigasus-tool-decisions").is_none());
}

#[tokio::test]
async fn a_malformed_non_stream_body_is_a_502_with_block_and_forwarded_with_allow() {
    let bad = r#"{"choices":[{"index":0,"message":{"tool_calls":[]}}],"a":1,"a":2}"#;
    let mock = MockOpenAi::spawn_json(StatusCode::OK, bad).await;
    let block = app_with_interception(ScopedIam::arc(PRINCIPAL_1, ORG_A_PRN), &mock.base_url, None, ti(Arc::new(AllowAll)));
    let sent = send(&block, NON_STREAM_BODY).await;
    assert_eq!(sent.status, StatusCode::BAD_GATEWAY);
    assert_eq!(sent.json()["error"]["code"], "tool-interception-malformed-response");
    let observe = Some(Arc::new(ToolInterception { limits: Limits { on_failure: paigasus_gateway_core::OnFailure::Allow, ..Limits::defaults() }, decider: Arc::new(AllowAll) }));
    let allow = app_with_interception(ScopedIam::arc(PRINCIPAL_1, ORG_A_PRN), &mock.base_url, None, observe);
    let sent = send(&allow, NON_STREAM_BODY).await;
    assert_eq!(sent.status, StatusCode::OK);
    assert_eq!(sent.text(), bad);
}

#[tokio::test]
async fn a_non_2xx_answer_is_forwarded_as_today() {
    let err = r#"{"error":{"message":"bad","type":"invalid_request_error","param":null,"code":null}}"#;
    let mock = MockOpenAi::spawn_json(StatusCode::BAD_REQUEST, err).await;
    let app = app_with_interception(ScopedIam::arc(PRINCIPAL_1, ORG_A_PRN), &mock.base_url, None, ti(Arc::new(BlockNamed)));
    let sent = send(&app, NON_STREAM_BODY).await;
    assert_eq!((sent.status, sent.text().as_str()), (StatusCode::BAD_REQUEST, err));
}

/// D24 (Review focus 5): the charge is the upstream's reported usage, even when the body is rewritten.
#[tokio::test]
async fn a_blocked_body_charges_like_the_upstream() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, body_with_calls()).await;
    let store = RecordingStore::new();
    let app = app_with_interception(ScopedIam::arc(PRINCIPAL_1, ORG_A_PRN), &mock.base_url, Some(limits(rules(None, None, Some(1_000_000)), store.clone())), ti(Arc::new(BlockNamed)));
    let _ = send(&app, NON_STREAM_BODY).await;
    // The charge is sent from the guard's Drop; give a spawned charge a moment if the store is async.
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(store.charges(), vec![13]);
}
```

Read how the existing non-stream charge test in `tests/limits_http.rs` waits for the charge. If it does not sleep, drop the sleep line here too.

```bash
cd rs && cargo nextest run --locked -p paigasus-gateway --test tool_interception
```

Expected: the six new tests FAIL. The body is forwarded with no interception, so there is no header, no rewrite, no 502, and the tests fail on their assertions.

- [ ] **Step 2: Implement `intercept_body`**

Append to `src/adapters/http/interception.rs`:

```rust
use paigasus_gateway_core::{Basis, Decision, OnFailure, parse_body};

pub(crate) enum BodyOutcome {
    Forward { body: Bytes, report: Option<String> },
    /// D16 block on a body that cannot be checked: the 502 `tool-interception-malformed-response`.
    Refuse,
}

pub(crate) async fn intercept_body(ti: &ToolInterception, ctx: Arc<DecisionContext>, turn: Arc<ToolTurn>, body: Bytes) -> BodyOutcome {
    let calls = match parse_body(Format::OpenAiChat, &body, ti.limits.max_record_bytes) {
        Ok(c) => c,
        Err(_) => {
            record(Metric::OnFailure("malformed"));
            return match ti.limits.on_failure {
                OnFailure::Block => BodyOutcome::Refuse,
                OnFailure::Allow => BodyOutcome::Forward { body, report: None },
            };
        }
    };
    if calls.calls.is_empty() && calls.unenforceable.is_empty() {
        return BodyOutcome::Forward { body, report: None };
    }
    let n = calls.calls.len();
    let (decisions, basis) = if n == 0 {
        (Vec::new(), Basis::Decider)
    } else {
        let fut = ti.decider.decide(ctx, turn, calls.calls.clone());
        match tokio::time::timeout(ti.limits.decide_timeout, fut).await {
            Ok(Ok(d)) if d.len() == n => (d, Basis::Decider),
            _ => {
                record(Metric::OnFailure("decider_error"));
                match ti.limits.on_failure {
                    OnFailure::Block => (vec![Decision::Block { code: paigasus_gateway_core::BlockCode::DeciderError }; n], Basis::OnFailure),
                    OnFailure::Allow => (vec![Decision::Allow; n], Basis::OnFailure),
                }
            }
        }
    };
    for d in &decisions {
        let decision = if d.is_allow() { "allow" } else { "block" };
        record(Metric::Decision { decision, basis: basis.as_str() });
    }
    let report = calls.report(&decisions, basis);
    let body = match calls.apply(&decisions) {
        None => body,
        Some(rewritten) => Bytes::from(rewritten),
    };
    BodyOutcome::Forward { body, report: Some(report) }
}
```

In `chat.rs`:
- Drop the `if stream { … } else { None }` condition that Task 16 added. `interception` is now built for both paths.
- Replace the end of the `ChatResponse::Full` arm, from `// Forward the upstream status + body VERBATIM …` to `(resp, status)`, with:

```rust
            // SMA-749 § 5.2: the guard read the ORIGINAL body above (D24); interception runs after.
            let outcome = match &interception {
                Some((ti, ctx, turn)) if status.is_success() => super::interception::intercept_body(ti, Arc::clone(ctx), Arc::clone(turn), body).await,
                _ => super::interception::BodyOutcome::Forward { body, report: None },
            };
            match outcome {
                super::interception::BodyOutcome::Refuse => {
                    let resp = GatewayError::ToolInterceptionMalformedResponse.into_response();
                    let status = resp.status();
                    (resp, status)
                }
                super::interception::BodyOutcome::Forward { body, report } => {
                    // Forward the upstream status + body VERBATIM (or rewritten by a block),
                    // including a non-2xx OpenAI error envelope.
                    let mut resp = (status, [(header::CONTENT_TYPE, "application/json")], body).into_response();
                    if let Some(r) = report
                        && let Ok(v) = axum::http::HeaderValue::from_str(&r)
                    {
                        resp.headers_mut().insert("paigasus-tool-decisions", v);
                    }
                    (resp, status)
                }
            }
```

The arm still produces `(response, status)`, so the structured log line at the end of the handler is written for the 502 too.

`header_value` is ASCII and at most 2 KiB, so `from_str` cannot fail on it. The `if let` keeps a future bug from becoming a panic.

- [ ] **Step 3: Run the tests and the registry gate**

```bash
cd rs && cargo fmt && cargo nextest run --locked -p paigasus-gateway && cargo clippy --locked -p paigasus-gateway --all-targets -- -D warnings && cd ..
moon run repo:error-code-single-site
```

Expected: PASS. The registry gate passes because the new code is spelled only in `error.rs`.

- [ ] **Step 4: Commit**

```bash
git add rs/crates/services/paigasus-gateway
git commit -m "feat(rs): intercept tool calls in non-stream chat completions (SMA-749)" -m "A 2xx body is strictly parsed after the usage read, decided under the
decider timeout, and forwarded unchanged or rewritten once, with the
paigasus-tool-decisions header. A body that cannot be checked is a 502
with on_failure block.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 18: Python SDK conformance against the goldens

**Files:**
- Create: `py/packages/paigasus-gateway-conformance/{pyproject.toml,moon.yml,README.md,LICENSE}`, `py/packages/paigasus-gateway-conformance/tests/test_golden_streams.py`
- Modify: `py/scripts/assert_test_floor.py` (`EXPECTED_TEST_PACKAGES`), `py/uv.lock`
- Modify: `rs/crates/libs/paigasus-gateway-core/tests/support/oracle.rs` (the `SOURCE` version slots)

**Interfaces:**
- Consumes: `rs/crates/libs/paigasus-gateway-core/tests/golden/manifest.json` and the six `.sse` files (Task 12).
- Produces: a private, unpublished uv workspace member, `paigasus-gateway-conformance`, with exact pins of `openai` and `anthropic`. Its Moon task `paigasus-gateway-conformance-py:test` re-runs whenever a golden file changes.

The package is a test-only, private workspace member. It is modelled on `py/packages/paigasus-workflows`: classifier `Private :: Do Not Upload`, no `[tool.paigasus] pypi`, version `0.0.0`. `tool.uv.package = false` means it needs no `src/`.

- [ ] **Step 1: Pin the SDKs and check what their stream helpers accept**

```bash
cd py
uv pip index versions openai 2>/dev/null | head -2 || uv run --no-project --with openai python -c "import openai; print(openai.__version__)"
uv pip index versions anthropic 2>/dev/null | head -2 || uv run --no-project --with anthropic python -c "import anthropic; print(anthropic.__version__)"
```

Pick, for each SDK, the newest release that is at least two weeks old (check its release date on PyPI). Record both versions in the task report. Those versions replace `<pinned in Task 18>` in `oracle.rs` for `openai-python` and `anthropic-python`.

Then read the stream helper of the pinned versions. Paths:
- `openai/lib/streaming/chat/_completions.py`: `ChatCompletionStreamState`, `accumulate_delta`.
- `anthropic/lib/streaming/_messages.py`: `accumulate_event`.

Compare them with the Rust models in `oracle.rs`. If a model disagrees with the code you read (another merge rule, another lookup by index or position), fix the MODEL in `oracle.rs`, re-run Task 11's properties, and say so in the report. The model must follow the SDK, not the other way round.

- [ ] **Step 2: Write the package**

`py/packages/paigasus-gateway-conformance/pyproject.toml` (fill in the two versions from Step 1):

```toml
# SPDX-License-Identifier: Apache-2.0
[project]
name = "paigasus-gateway-conformance"
version = "0.0.0"
license = "Apache-2.0"
license-files = ["LICENSE"]
readme = "README.md"
description = "Checks the gateway's rewritten tool-call streams against the real OpenAI and Anthropic SDKs (SMA-749)."
classifiers = ["Private :: Do Not Upload"]
requires-python = ">=3.12"
# Exact pins: the conformance claim is about these versions (spec § 8.2). Bump deliberately, and
# re-read the stream helpers against tests/support/oracle.rs in paigasus-gateway-core.
dependencies = ["openai==<VERSION>", "anthropic==<VERSION>", "httpx"]

[tool.uv]
package = false
```

Do not commit `<VERSION>`: put in the two pinned numbers. `httpx` stays unpinned, because both SDKs constrain it and the lockfile pins it.

Copy `LICENSE` from `py/packages/paigasus-workflows/LICENSE`. `README.md`:

```markdown
# paigasus-gateway-conformance

Test-only. Checks that the golden streams from `rs/crates/libs/paigasus-gateway-core/tests/golden`
parse with the pinned OpenAI and Anthropic Python SDKs: every allowed tool call arrives, no blocked
call arrives, and the notice is text (SMA-749 spec § 8.2). Not published.
```

`py/packages/paigasus-gateway-conformance/moon.yml`:

```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'

id: 'paigasus-gateway-conformance-py'
layer: 'library'
language: 'python'

tasks:
  # SMA-749. Scoped like paigasus-kernel-py:test, so a golden-file change selects this task: the
  # goldens live under rs/, outside this project, and Moon confers affectedness only through a
  # task's own inputs.
  test:
    command: 'uv run pytest tests'
    inputs:
      - 'tests/**/*'
      - 'pyproject.toml'
      - '/py/uv.lock'
      - '/rs/crates/libs/paigasus-gateway-core/tests/golden/**/*'
```

`py/packages/paigasus-gateway-conformance/tests/test_golden_streams.py`:

```python
# SPDX-License-Identifier: Apache-2.0
"""The gateway's golden tool-call streams through the real SDK stream helpers (SMA-749 § 8.2)."""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

import anthropic
import httpx
import openai
import pytest

GOLDEN = Path(__file__).resolve().parents[4] / "rs" / "crates" / "libs" / "paigasus-gateway-core" / "tests" / "golden"
MANIFEST: list[dict[str, Any]] = json.loads((GOLDEN / "manifest.json").read_text())


def _transport(body: bytes) -> httpx.MockTransport:
    def handler(_request: httpx.Request) -> httpx.Response:
        return httpx.Response(200, headers={"content-type": "text/event-stream"}, content=body)

    return httpx.MockTransport(handler)


def _cases(fmt: str) -> list[dict[str, Any]]:
    cases = [c for c in MANIFEST if c["format"] == fmt]
    assert cases, f"no {fmt} golden in the manifest"
    return cases


@pytest.mark.parametrize("case", _cases("openai"), ids=lambda c: c["file"])
def test_openai_stream_helper_sees_only_allowed_calls(case: dict[str, Any]) -> None:
    body = (GOLDEN / case["file"]).read_bytes()
    client = openai.OpenAI(api_key="x", base_url="http://gateway.test/v1", http_client=httpx.Client(transport=_transport(body)))
    with client.chat.completions.stream(model="m", messages=[{"role": "user", "content": "hi"}]) as stream:
        final = stream.get_final_completion()
    calls = [(tc.function.name, tc.function.arguments) for ch in final.choices for tc in (ch.message.tool_calls or [])]
    assert sorted(calls) == sorted((a["name"], a["arguments"]) for a in case["allowed"])
    text = "".join(ch.message.content or "" for ch in final.choices)
    for notice in case["notices"]:
        assert notice in text
    assert "CANARY" not in body.decode()


@pytest.mark.parametrize("case", _cases("anthropic"), ids=lambda c: c["file"])
def test_anthropic_stream_helper_sees_only_allowed_calls(case: dict[str, Any]) -> None:
    body = (GOLDEN / case["file"]).read_bytes()
    client = anthropic.Anthropic(api_key="x", base_url="http://gateway.test", http_client=httpx.Client(transport=_transport(body)))
    with client.messages.stream(model="m", max_tokens=16, messages=[{"role": "user", "content": "hi"}]) as stream:
        final = stream.get_final_message()
    calls = [(b.name, json.dumps(b.input, separators=(",", ":"))) for b in final.content if b.type == "tool_use"]
    want = [(a["name"], json.dumps(json.loads(a["arguments"]), separators=(",", ":"))) for a in case["allowed"]]
    assert sorted(calls) == sorted(want)
    text = "".join(b.text for b in final.content if b.type == "text")
    for notice in case["notices"]:
        assert notice in text
```

`parents[4]` resolves from `py/packages/paigasus-gateway-conformance/tests/test_golden_streams.py` to the repo root. Verify that with `python3 -c "from pathlib import Path; print(Path('py/packages/paigasus-gateway-conformance/tests/x.py').resolve().parents[4])"` from the root.

In `py/scripts/assert_test_floor.py`, change `EXPECTED_TEST_PACKAGES = frozenset({"paigasus-kernel", "paigasus-proto"})` to:

```python
EXPECTED_TEST_PACKAGES = frozenset({"paigasus-gateway-conformance", "paigasus-kernel", "paigasus-proto"})
```

- [ ] **Step 3: Lock, run and type-check**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd py && uv lock && uv sync && uv run pytest packages/paigasus-gateway-conformance/tests -v && cd ..
moon run py:test py:typecheck py:lint py:fmt paigasus-gateway-conformance-py:test
/opt/homebrew/bin/bash ci/ruff/run.sh
moon run repo:osv repo:publish-metadata repo:version-lockstep repo:release-parity-py
```

Expected: 6 tests pass, and every gate passes. The `py:typecheck` (basedpyright, `typeCheckingMode = "all"`) is strict. When it reports `reportAny` or `reportUnknownMemberType` on SDK objects, add precise annotations or `typing.cast` calls. Do not add `# type: ignore`, because `reportUnnecessaryTypeIgnoreComment` is an error in this repo.

If `repo:osv` reports a vulnerability in a transitive dependency of the SDKs, follow the `constraint-dependencies` pattern in `py/pyproject.toml` (a floor, with a comment), rather than pinning a different SDK.

If `repo:publish-metadata`, `repo:version-lockstep` or `repo:release-parity-py` reds on the new package, it reads every `py/packages/*/pyproject.toml`. Compare the new `pyproject.toml` with `paigasus-workflows`'s line by line, and copy what the gate needs, for example its dormant `[tool.semantic_release]` block. Record which gate needed what.

If a test fails because the pinned SDK's stream helper rejects something in a golden, read the SDK's error before you change anything. A rejection means the rewrite is not SDK-compatible, which is a Task 6 or Task 7 defect. Fix the rewrite, regenerate the goldens (Task 12 Step 3) and re-run.

- [ ] **Step 4: Fill in the model versions**

In `rs/crates/libs/paigasus-gateway-core/tests/support/oracle.rs`, replace each `<pinned in Task 18>` with `openai <version>` or `anthropic <version>` from Step 1. Leave the openai-node and LangChain lines as they are: those are modelled only, as their doc lines say.

```bash
grep -n "pinned in Task 18" rs/crates/libs/paigasus-gateway-core/tests/support/oracle.rs && echo "FAIL: a slot is left" || echo "all slots filled"
```

- [ ] **Step 5: Commit**

```bash
git add py/packages/paigasus-gateway-conformance py/scripts/assert_test_floor.py py/uv.lock rs/crates/libs/paigasus-gateway-core/tests/support/oracle.rs
git commit -m "test(py): check the tool-interception goldens with the real Python SDKs (SMA-749)" -m "A private test-only workspace member with exact openai and anthropic
pins runs each golden through the SDK stream helpers: only allowed calls
arrive and the notice is text. The Rust models cite the pinned versions.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 19: The first-party TS parsers against the goldens

**Files:**
- Create: `ts/apps/gateway-console/tests/unit/chat-stream-golden.test.ts`, `ts/packages/paigasus-sdk/tests/terminal-frame-golden.test.ts`
- Modify: `ts/apps/gateway-console/moon.yml` (`test` inputs), `ts/packages/paigasus-sdk/moon.yml` (`test` inputs)

**Interfaces:**
- Consumes: the OpenAI goldens and the manifest (Task 12); `createChatStreamParser` (`ts/apps/gateway-console/lib/chat-stream.ts`); `createTerminalFrameParser` (`ts/packages/paigasus-sdk/src/chat.ts`).

- [ ] **Step 1: Write the tests**

`ts/apps/gateway-console/tests/unit/chat-stream-golden.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
// SMA-749 § 8.2: the console's stream parser reads every OpenAI golden stream. It shows the
// notice as text, ends with `done`, and raises no error.
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { createChatStreamParser } from '../../lib/chat-stream';

const golden = fileURLToPath(new URL('../../../../../rs/crates/libs/paigasus-gateway-core/tests/golden/', import.meta.url));
const manifest = JSON.parse(readFileSync(`${golden}manifest.json`, 'utf8')) as { file: string; format: string; notices: string[] }[];

describe('the console stream parser on the tool-interception goldens', () => {
  const cases = manifest.filter((c) => c.format === 'openai');
  it('has goldens to read', () => expect(cases.length).toBeGreaterThan(0));
  for (const c of cases) {
    it(c.file, () => {
      const parser = createChatStreamParser();
      const events = [...parser.push(new Uint8Array(readFileSync(`${golden}${c.file}`))), ...parser.end()];
      expect(events.filter((e) => e.kind === 'error')).toEqual([]);
      expect(events.at(-1)).toEqual({ kind: 'done' });
      const text = events.flatMap((e) => (e.kind === 'delta' ? [e.text] : [])).join('');
      for (const n of c.notices) expect(text).toContain(n);
      expect(text).not.toContain('CANARY');
    });
  }
});
```

`ts/packages/paigasus-sdk/tests/terminal-frame-golden.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
// SMA-749 § 8.2: the SDK's terminal-frame parser finds no error in any golden stream. A
// rewritten stream must never look like a failed one.
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { createTerminalFrameParser } from '../src/chat';

const golden = fileURLToPath(new URL('../../../../rs/crates/libs/paigasus-gateway-core/tests/golden/', import.meta.url));
const manifest = JSON.parse(readFileSync(`${golden}manifest.json`, 'utf8')) as { file: string; format: string }[];

describe('the terminal-frame parser on the tool-interception goldens', () => {
  for (const c of manifest.filter((m) => m.format === 'openai')) {
    it(c.file, () => {
      const parser = createTerminalFrameParser(200);
      expect(parser.push(new Uint8Array(readFileSync(`${golden}${c.file}`)))).toEqual([]);
    });
  }
});
```

Check both relative paths from the test file's directory to the repo root, by counting `..`. `ts/apps/gateway-console/tests/unit/` needs five levels, and `ts/packages/paigasus-sdk/tests/` needs four. If the console's event type uses other field names than `kind`, `text` and `done`, read `lib/chat-stream.ts:17` and adjust the test to it.

- [ ] **Step 2: Make a golden change select both tests**

In `ts/apps/gateway-console/moon.yml`, add to the `test` task's `inputs`:

```yaml
      # SMA-749: tests/unit/chat-stream-golden.test.ts reads the gateway-core goldens.
      - '/rs/crates/libs/paigasus-gateway-core/tests/golden/**/*'
```

In `ts/packages/paigasus-sdk/moon.yml`, add the same line, with the comment naming `tests/terminal-frame-golden.test.ts`, to the `test` task's `inputs`.

- [ ] **Step 3: Run**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
(cd ts/apps/gateway-console && pnpm exec vitest run tests/unit/chat-stream-golden.test.ts)
(cd ts/packages/paigasus-sdk && pnpm exec vitest run tests/terminal-frame-golden.test.ts)
moon run ts:fmt paigasus-sdk-ts:typecheck gateway-console-ts:typecheck
/bin/bash ci/affected-graph/run.sh
```

Expected: PASS. If the affected-graph suite has a case that pins the exact `inputs` of these tasks, it reds on the new line. Add the golden path to that case's expectation.

- [ ] **Step 4: Commit**

```bash
git add ts/apps/gateway-console/tests/unit/chat-stream-golden.test.ts ts/packages/paigasus-sdk/tests/terminal-frame-golden.test.ts ts/apps/gateway-console/moon.yml ts/packages/paigasus-sdk/moon.yml ci/affected-graph
git commit -m "test(ts): read the tool-interception goldens with the first-party parsers (SMA-749)" -m "The console stream parser shows the notice as text and ends cleanly, and
the SDK terminal-frame parser sees no error, on every OpenAI golden.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 20: T-perf, the crate doc, and the full gate graph

**Files:**
- Create: `rs/crates/libs/paigasus-gateway-core/tests/perf.rs`
- Modify: `rs/crates/services/paigasus-gateway/src/lib.rs` (the crate doc), `rs/CLAUDE.md` (one line, only if Step 3 shows it is needed)

- [ ] **Step 1: Write the timing test**

`tests/perf.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0
//! T-perf (spec § 8.3): 1 MiB of text-only stream, interception on, against a plain copy (off).
//! `#[ignore]`: it measures, it does not gate. Run it and record the numbers in the PR:
//! `cargo nextest run --release -p paigasus-gateway-core --test perf --run-ignored only --no-capture`

use std::time::Instant;

use bytes::Bytes;
use paigasus_gateway_core::{Action, Format, Interceptor, Limits};

#[test]
#[ignore = "a measurement, not a gate"]
fn one_mib_of_text() {
    let rec = "data: {\"id\":\"c\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"m\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"lorem ipsum dolor \"},\"finish_reason\":null}]}\n\n";
    let n = (1 << 20) / rec.len();
    let body = rec.repeat(n);
    let chunks: Vec<&[u8]> = body.as_bytes().chunks(4096).collect();

    let t = Instant::now();
    let mut copied = 0usize;
    for c in &chunks {
        copied += Bytes::copy_from_slice(c).len();
    }
    let off = t.elapsed();

    let t = Instant::now();
    let now = Instant::now();
    let mut i = Interceptor::new(Format::OpenAiChat, Limits::defaults(), Bytes::new(), now);
    let mut emitted = 0usize;
    for c in &chunks {
        for a in i.push(c, now) {
            if let Action::Emit(b) = a {
                emitted += b.len();
            }
        }
    }
    let on = t.elapsed();

    assert_eq!(copied, body.len());
    assert!(emitted >= body.len());
    println!("T-perf: {n} records, {} bytes; off {:?}, on {:?}, ratio {:.1}x", body.len(), off, on, on.as_secs_f64() / off.as_secs_f64().max(1e-9));
}
```

```bash
cd rs && cargo nextest run --release --locked -p paigasus-gateway-core --test perf --run-ignored only --no-capture
```

Record the printed line in the PR description. There is no threshold.

- [ ] **Step 2: Document the feature in the gateway crate doc**

In `rs/crates/services/paigasus-gateway/src/lib.rs`, after the SMA-677 paragraph of the crate doc:

```rust
//! **Tool-call interception (SMA-749).** With `[tool_interception] enabled = true`, the chat
//! relay runs every response through `paigasus-gateway-core`. Text streams at once. A tool call
//! is held until it is complete, then decided, then forwarded or replaced with an assistant
//! notice. This build's decider allows everything (SMA-752 adds policies). Off by default; off is
//! exactly the old relay. Decisions are reported in `: paigasus-decision` SSE comments or the
//! `paigasus-tool-decisions` header. A non-stream answer the gateway cannot check is a 502
//! `tool-interception-malformed-response` under `on_failure = "block"`.
```

- [ ] **Step 3: Run the full gate graph**

Run the command from the root `CLAUDE.md` `ci-targets` block, with `/bin/bash` 3.2 as the default shell:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/sven/dev/paigasus/paigasus-core-sma-749
sed -n '/ci-targets:begin/,/ci-targets:end/p' CLAUDE.md
```

Copy the command that the block prints and run it. Then re-run the gates that need another bash directly, and read their verdicts instead of `moon ci`'s:

```bash
/opt/homebrew/bin/bash ci/ruff/run.sh
/opt/homebrew/bin/bash ci/next-public/run.sh
/opt/homebrew/bin/bash ci/publish-metadata/run.sh
/opt/homebrew/bin/bash ci/actionlint/run.sh   # only when its pipe preflight reports >= 8192 bytes
```

Expected: every gate passes. When one fails, use the root `CLAUDE.md` "Diagnosing an unattributed `moon ci` failure" procedure. Copy `.moon/cache/ciReport.json` and the task's state directory before you re-run anything. Fix and commit each finding in its own commit, in the task it belongs to. A finding that needs a rule in `rs/CLAUDE.md` (for example a new crate-level convention a gate enforced) gets one line there, in the same commit as the fix.

- [ ] **Step 4: Confirm the acceptance criteria**

Tick each line in the task report with the test or gate that proves it:

| AC | Proof |
|---|---|
| 1 | Task 0's count plus the new tests; `interception_off_is_byte_identical` |
| 2 | `allow_all_is_payload_identical_for_every_conforming_fixture`, `allow_all_is_payload_identical` |
| 3 | `blocked_canaries_never_appear`, the canary check in both 10 000-case properties, `a_blocked_call_never_reaches_the_client` |
| 4 | `openai_p1_and_p2_hold`, `anthropic_p1_and_p2_hold` |
| 5 | `malformed_shapes_follow_section_6` |
| 6 | Task 18's 6 pytest cases; Task 19's two vitest files |
| 7 | `a_short_decision_vector_blocks_every_call` (stream), `a_short_decision_vector_blocks_the_rest` (body) |
| 8 | Step 3 |

- [ ] **Step 5: Commit**

```bash
git add rs/crates/libs/paigasus-gateway-core/tests/perf.rs rs/crates/services/paigasus-gateway/src/lib.rs
git commit -m "docs(rs): document gateway tool-call interception and add the T-perf probe (SMA-749)" -m "The crate doc names the feature, its default and its error code. The
ignored timing test measures 1 MiB of text with interception on.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## Self-review against the spec

| Spec item | Task |
|---|---|
| § 2 threat model, I1 | 11 (P1 with four client models, canaries) |
| D1 crate, sans-IO, no kernel change | 1, 8 (no tokio), Global Constraints |
| D2–D4 SSE in/out, tail, after `[DONE]` | 2, 8 |
| D5 strict JSON | 3 |
| D6 positive allowlist | 5, 7, 9 |
| D7 malformed record | 8 |
| D8 oversized | 2, 8 |
| D9 canonical assembly (+ P-10) | 5, 6, 7 |
| D10/D11 completion and truncation | 6, 7, 8 |
| D12 one FIFO, touch sets | 6, 8 |
| D13 per-stream memory | 8, 11 (P2) |
| D14 hold and decide timeouts | 8, 15, 17 |
| D15 keep-alive | 8 |
| D16 on_failure, § 5.3 observe | 8, 9, 17 |
| D17 transport error, error record | 8, 16 |
| D18 decider contract, length check | 8, 9, 10, 17 |
| D19 request turn, `Unavailable` | 10, 16 |
| D20 rewrite rules | 6, 7, 9 |
| D21 notice text | 4 |
| D22 reports and header | 4, 8, 9, 17 |
| D23 payload identity | 6, 7, 11, 16 |
| D24 billing | 16, 17 |
| D25 cancellation | 16 |
| D26 config, `deny_unknown_fields` | 15 |
| D27 off is today's relay | 16 (`interception_off_is_byte_identical`) |
| D28 no capability advertised | nothing changes `service_info.rs` (checked by its existing tests) |
| D29 proptest, not cargo-fuzz | 11 |
| § 7 gates | 1, 13, 14, 15, 16, 18, 19, 20 |
| § 8.2 SDK conformance | 12, 18, 19 |
| § 8.3 fixtures, splits, canaries, T-perf | 11, 20 |
| AC 1–8 | 20 Step 4 |
