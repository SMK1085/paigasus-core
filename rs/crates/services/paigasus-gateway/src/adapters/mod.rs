// SPDX-License-Identifier: Apache-2.0

//! Hexagonal adapters. `http` is the inbound HTTP surface (G3); `iam` is the outbound IAM gRPC
//! client (G4); `openai` is the outbound OpenAI egress client (G6).

pub mod http;
pub mod iam;
pub mod openai;
