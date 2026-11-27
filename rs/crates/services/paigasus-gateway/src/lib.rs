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
//! decided, then forwarded or replaced with an assistant notice. Without `[tool_policy]` the
//! decider allows everything. Off by default; off is payload-identical to the old relay. Decisions are
//! reported in `: paigasus-decision` SSE comments or the `paigasus-tool-decisions` header. A
//! non-stream answer the gateway cannot check is a 502 `tool-interception-malformed-response`
//! under `on_failure = "block"`.
//!
//! ## Tool policy (SMA-752)
//! With `[tool_policy] enabled = true` (it needs tool interception), the gateway checks tools
//! against Cedar policies: a floor that is always enforced, and a tenant set per organization in
//! the mode `off`, `log_only` or `enforce`. Before the limits, the request side removes denied tool
//! definitions and replaces blocked tool results in `enforce` (a floor removal in every mode). It
//! refuses a request whose tools it cannot read in `enforce` with a 400 `invalid-request-body`.
//! It refuses a request with an enforced block of a `computer_call_output` with a 400
//! `tool-result-unreplaceable`, because that item has no field for the notice (SMA-776).
//! The Cedar decider then decides each tool call with the same policy snapshot. The request
//! entries are reported first in a stream, and in the `paigasus-tool-decisions` header of a full
//! answer. Off by default; off keeps the gateway exactly as before.
//!
//! Under `on_failure = "block"`, the gateway only forwards a call that it has assembled in the one
//! canonical form that every client SDK agrees on. Anything else is a deviation and is blocked.
//! Under `on_failure = "allow"` (observe mode) deviations are forwarded as received and not
//! enforced. The routes are `/v1/chat/completions` and `/v1/responses` (OpenAI), and `/v1/messages`
//! (Anthropic, SMA-750), with `/v1/messages/count_tokens` and `/v1/models` as pass-through routes.
//! Rules a reader might not expect:
//! - A choice index must first appear in order (0, 1, 2, ...). Tool-call data is accepted only in
//!   the choice at array position 0 of a chunk (`multi_choice_tool_calls`, `choice_position`),
//!   because LangChain reads `choices[0]` only. So `n > 1` requests whose later choices call tools
//!   are not supported while interception is on.
//! - A chunk that carries tool data must have `object` exactly `chat.completion.chunk`. A body
//!   that carries tool calls must have `chat.completion`.
//! - An unknown `type` with a function or custom body is a deviation (`type_shape_mismatch`).
//! - Function arguments must be one complete JSON value when the call completes (empty means
//!   `{}`), else `incomplete_arguments`. Custom-tool input is free-form and exempt.
//! - Anthropic: a `content_block_start` while an earlier block is open is `block_interleave`, and
//!   a `tool_use` body input integer beyond 2^53 is `lossy_number`.
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
