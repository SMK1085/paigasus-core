// SPDX-License-Identifier: Apache-2.0

//! The inbound chat-completion request DTO.
//!
//! G7 parses the caller's JSON body into [`ChatCompletionRequest`] for exactly two reasons: to
//! read `model` (request logging) and `stream` (choosing the streaming vs non-streaming egress
//! path). It is **not** the wire body sent upstream — the OpenAI client forwards the caller's
//! ORIGINAL raw bytes verbatim (see [`crate::adapters::openai`]), so passthrough is byte-lossless
//! regardless of anything this DTO does or does not model.
//!
//! `#[serde(flatten)] extra` therefore exists as a belt-and-braces guarantee: every field this
//! struct does not name is preserved rather than dropped, so a re-serialization would still be
//! lossless (a secondary safety net, not the primary passthrough mechanism).

use serde::{Deserialize, Serialize};

/// An OpenAI-compatible `POST /v1/chat/completions` request body, modelled just enough for the
/// gateway's needs. `model` and `messages` are the required core; `stream` selects the egress
/// path (defaults to `false` when absent); every other top-level field (`temperature`, `top_p`,
/// `tools`, `response_format`, …) is captured losslessly in [`extra`](Self::extra).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ChatCompletionRequest {
    /// The target model id (read for request logging; the upstream body carries it verbatim).
    pub model: String,
    /// Whether the caller requested a streamed (SSE) completion. Absent → `false`.
    #[serde(default)]
    pub stream: bool,
    /// The conversation turns, kept as opaque JSON — the gateway never inspects message content
    /// (no prompt is ever logged or transformed).
    pub messages: Vec<serde_json::Value>,
    /// Every unmodelled top-level field, preserved verbatim so a round-trip is lossless.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_core_fields_and_captures_unknowns_in_extra() {
        // A realistic request carrying `model`/`stream`/`messages` plus several unmodelled
        // top-level fields — the exact shape a caller's OpenAI SDK emits.
        let raw = json!({
            "model": "gpt-4o-mini",
            "stream": true,
            "messages": [{ "role": "user", "content": "hi" }],
            "temperature": 0.7,
            "top_p": 0.9,
            "tools": [{ "type": "function", "function": { "name": "f" } }],
            "response_format": { "type": "json_object" }
        });

        let req: ChatCompletionRequest = serde_json::from_value(raw).expect("valid request parses");

        assert_eq!(req.model, "gpt-4o-mini");
        assert!(req.stream);
        assert_eq!(req.messages.len(), 1);

        // Every unknown top-level field landed in `extra` — and NOTHING that was named leaked
        // into it.
        assert_eq!(req.extra.get("temperature"), Some(&json!(0.7)));
        assert_eq!(req.extra.get("top_p"), Some(&json!(0.9)));
        assert_eq!(req.extra.get("tools"), Some(&json!([{ "type": "function", "function": { "name": "f" } }])));
        assert_eq!(req.extra.get("response_format"), Some(&json!({ "type": "json_object" })));
        assert!(!req.extra.contains_key("model"), "named fields must not bleed into `extra`");
        assert!(!req.extra.contains_key("stream"));
        assert!(!req.extra.contains_key("messages"));
    }

    #[test]
    fn round_trip_preserves_the_extra_fields_losslessly() {
        let raw = json!({
            "model": "gpt-4o",
            "messages": [{ "role": "system", "content": "be terse" }],
            "temperature": 0.2,
            "seed": 42,
            "metadata": { "team": "a", "nested": { "k": "v" } }
        });

        let req: ChatCompletionRequest = serde_json::from_value(raw.clone()).expect("valid request parses");
        let reserialized = serde_json::to_value(&req).expect("re-serializes");

        // The extra fields survive the round-trip byte-for-value (order-independent JSON equality).
        assert_eq!(reserialized.get("temperature"), Some(&json!(0.2)));
        assert_eq!(reserialized.get("seed"), Some(&json!(42)));
        assert_eq!(reserialized.get("metadata"), Some(&json!({ "team": "a", "nested": { "k": "v" } })));
        // Core fields survive too, at the top level (flatten does not nest them).
        assert_eq!(reserialized.get("model"), Some(&json!("gpt-4o")));
        assert_eq!(reserialized.get("messages"), raw.get("messages"));
    }

    #[test]
    fn stream_defaults_to_false_when_absent() {
        let raw = json!({
            "model": "gpt-4o",
            "messages": []
        });
        let req: ChatCompletionRequest = serde_json::from_value(raw).expect("valid request parses");
        assert!(!req.stream, "`stream` must default to false when the field is absent");
    }
}
