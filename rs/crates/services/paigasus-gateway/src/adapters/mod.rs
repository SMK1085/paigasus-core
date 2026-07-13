// SPDX-License-Identifier: Apache-2.0

//! Hexagonal adapters. `http` is the inbound HTTP surface (this task); G4 adds outbound
//! IAM/OpenAI clients as sibling adapter modules.

pub mod http;
