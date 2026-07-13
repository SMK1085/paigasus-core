// SPDX-License-Identifier: Apache-2.0

//! Outbound OpenAI egress adapter: a `reqwest` client that forwards a caller's chat-completion
//! request upstream (non-stream + UNBUFFERED stream), holding the real OpenAI key and NEVER
//! leaking it or forwarding the caller's inbound headers. See [`client`] for the split-timeout
//! rationale (connect / first-byte / idle), the header/secret hygiene posture, and the
//! cancel-on-drop streaming contract.

pub mod client;

pub use client::{ChatResponse, OpenAiByteStream, OpenAiClient, OpenAiError};
