// SPDX-License-Identifier: Apache-2.0

//! The outbound Anthropic egress client (SMA-750).

pub mod client;

pub use client::{AnthropicClient, AnthropicRoute, Forward};
