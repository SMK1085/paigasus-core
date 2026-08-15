// SPDX-License-Identifier: Apache-2.0

//! paigasus-gateway library surface (for integration tests + the binary): the AI Gateway
//! M0 walking skeleton — an axum service fronting the OpenAI chat-completions endpoint,
//! authenticating callers against `paigasus-iam` (G4/G5), forwarding requests upstream
//! (G6/G7), and reporting liveness/readiness (this task; G8 completes `/readyz`).
//!
//! Config reference + defaults: `gateway.toml.example` (crate root).

pub mod adapters;
pub mod config;
pub mod domain;
pub mod runtime;
pub mod service_info;
