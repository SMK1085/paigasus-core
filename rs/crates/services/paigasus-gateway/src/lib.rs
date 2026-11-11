// SPDX-License-Identifier: Apache-2.0

//! paigasus-gateway library surface (for integration tests + the binary): the AI Gateway
//! M0 walking skeleton — an axum service fronting the OpenAI chat-completions endpoint,
//! authenticating callers against `paigasus-iam` (G4/G5), forwarding requests upstream
//! (G6/G7), and reporting liveness/readiness (this task; G8 completes `/readyz`).
//!
//! Config reference + defaults: `gateway.toml.example` (crate root).
//!
//! ## Limits (SMA-677)
//! An optional `[limits]` table adds a per-principal request rate, a per-org request rate
//! (each a 60 s sliding window) and a per-org token budget (UTC daily, ISO-weekly or monthly).
//! The refusals are `429` with the registry codes `rate-limited` (retryable, `Retry-After`) and
//! `budget-exhausted` (`x-should-retry: false`). `backend = "memory"` keeps counts in one process.
//! `backend = "redis"` shares the counts across replicas. It is fail-open: when Redis is
//! unavailable, the gateway admits requests with no limit. A stream is charged its reported usage
//! only when the client sends `stream_options: {"include_usage": true}`.
//!
//! ## Tool-call interception (SMA-749)
//! With `[tool_interception] enabled = true`, the chat relay runs every response through
//! `paigasus-gateway-core`. Text streams at once. A tool call is held until it is complete, then
//! decided, then forwarded or replaced with an assistant notice. This build's decider allows
//! everything (SMA-752 adds policies). Off by default; off is exactly the old relay. Decisions are
//! reported in `: paigasus-decision` SSE comments or the `paigasus-tool-decisions` header. A
//! non-stream answer the gateway cannot check is a 502 `tool-interception-malformed-response`
//! under `on_failure = "block"`.
//!
//! The gateway only forwards a call that it has assembled in the one canonical form that every
//! client SDK agrees on. Anything else is a deviation and follows `on_failure`. Rules a reader
//! might not expect:
//! - A choice index must first appear in order (0, 1, 2, ...). Tool-call data is accepted only in
//!   the choice at array position 0 of a chunk (`multi_choice_tool_calls`, `choice_position`),
//!   because LangChain reads `choices[0]` only. So `n > 1` requests whose later choices call tools
//!   are not supported while interception is on.
//! - A chunk that carries tool data must have `object` exactly `chat.completion.chunk`. A body
//!   that carries tool calls must have `chat.completion`.
//! - An unknown `type` with a function or custom body is a deviation (`type_shape_mismatch`).
//! - Function arguments must be one complete JSON value when the call completes (empty means
//!   `{}`), else `incomplete_arguments`. Custom-tool input is free-form and exempt.
//! - An Anthropic `content_block_start` while an earlier block is open is `block_interleave`.
//!   A `tool_use` input integer beyond 2^53 is `lossy_number`.
//! - A decider that returns the wrong number of decisions blocks every call of the hold.
//! - Every `*_secs` interception limit is capped at 86 400 s.

pub mod adapters;
pub mod application;
pub mod config;
pub mod domain;
pub mod runtime;
pub mod service_info;
#[cfg(test)]
mod test_support;
