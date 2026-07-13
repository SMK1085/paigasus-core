// SPDX-License-Identifier: Apache-2.0

//! Hexagonal adapters. `http` is the inbound HTTP surface (G3); `iam` is the outbound IAM gRPC
//! client (this task, G4); G6 adds the outbound OpenAI client as a sibling adapter module.

pub mod http;
pub mod iam;
