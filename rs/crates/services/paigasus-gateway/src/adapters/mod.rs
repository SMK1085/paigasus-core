// SPDX-License-Identifier: Apache-2.0

//! Hexagonal adapters. `http` is the inbound HTTP surface (G3); `iam` is the outbound IAM gRPC
//! client (G4); `openai` is the outbound OpenAI egress client (G6); `limits` holds the limit-store
//! adapters (SMA-677); `nats` is the tool-policy invalidation subscription (SMA-752); `decision_log`
//! writes the tool decision records to a Redis stream (SMA-755); `history` holds the session history
//! store adapters (SMA-753).

pub mod decision_log;
pub mod detectors;
pub mod history;
pub mod http;
pub mod iam;
pub mod limits;
pub mod nats;
pub mod openai;
pub mod policy_source;
