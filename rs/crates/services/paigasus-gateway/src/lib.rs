// SPDX-License-Identifier: Apache-2.0

//! paigasus-gateway library surface (for integration tests + the binary): the AI Gateway
//! M0 walking skeleton — an axum service fronting the OpenAI chat-completions endpoint,
//! authenticating callers against `paigasus-iam` (G4/G5), forwarding requests upstream
//! (G6/G7), and reporting liveness/readiness (this task; G8 completes `/readyz`).
//!
//! Config reference + defaults: `gateway.toml.example` (crate root).
//!
//! ## Limits (SMA-677)
//! An optional `[limits]` table adds a per-principal and a per-org request rate (a 60 s sliding
//! window) and a per-org token budget (UTC daily, ISO-weekly or monthly). The refusals are `429`
//! with the registry codes `rate-limited` (retryable, `Retry-After`) and `budget-exhausted`
//! (`x-should-retry: false`). `backend = "memory"` keeps per-process counts; `backend = "redis"`
//! shares them across replicas and is fail-open when Redis is unavailable. A stream is charged its
//! reported usage only when the client sends `stream_options: {"include_usage": true}`.

pub mod adapters;
pub mod application;
pub mod config;
pub mod domain;
pub mod runtime;
pub mod service_info;
#[cfg(test)]
mod test_support;
