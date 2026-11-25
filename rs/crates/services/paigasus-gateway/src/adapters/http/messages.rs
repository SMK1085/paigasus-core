// SPDX-License-Identifier: Apache-2.0

//! The `POST /v1/messages` proxy handler (SMA-750 D4, D13, D15, D19, § 8).
//!
//! The route group is mounted only when `[messages] enabled = true`; otherwise the paths answer
//! the Anthropic 404. The flow is the generic [`pipeline::proxy`] with the Anthropic upstream;
//! this module gives the [`MESSAGES`] descriptor:
//!
//! - **Forward** (D5, I2): the handler picks the raw query and the `anthropic-*` headers with
//!   [`AnthropicClient::forward`](crate::adapters::anthropic::AnthropicClient::forward). No other
//!   caller header goes upstream.
//! - **Parse** (§ 8): a COPY of the body is read into [`MessagesRequest`]. The failure split is
//!   SMA-588. D19: a duplicate top-level key is refused in both modes; with interception on, a
//!   duplicate key at any depth is refused. The pipeline calls the parse before the limits, so a
//!   refused request uses no quota.
//! - **Usage** (D10): the readers of [`anthropic_usage`](super::anthropic_usage).
//! - **Terminal frame** (D13): [`MESSAGES_TERMINAL_SSE_ERROR`].
//! - **Limits** (D15): the interception limits with the record, hold and time caps of
//!   `[messages]`.
//!
//! Streaming obeys the shared `stream_enabled` switch (D15): the pipeline reads it.

use axum::Extension;
use axum::extract::{OriginalUri, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use paigasus_gateway_core::{Format, Limits};
use serde::Deserialize;
use serde_json::Value;

use super::AppState;
use super::anthropic_usage::{self, AnthropicUsageScanner};
use super::duplicates::duplicates;
use super::error::GatewayError;
use super::interception::ToolInterception;
use super::pipeline::{self, Endpoint, RequestFacts, Upstream};
use super::usage::StreamUsage;
use crate::adapters::anthropic::AnthropicRoute;
use crate::adapters::http::bytes::EnvelopeBytes;
use crate::adapters::http::dto::MessagesRequest;
use crate::domain::CallerContext;

/// D13: the terminal SSE frame of a Messages stream that fails mid-flight. It is an Anthropic
/// `error` event with the registry's kebab `upstream-error`. The leading blank line ends a partly
/// sent record, so the frame is a record of its own.
pub const MESSAGES_TERMINAL_SSE_ERROR: &str = "\n\nevent: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"api_error\",\"message\":\"upstream stream error\",\"code\":\"upstream-error\"}}\n\n";

/// Proxy a Messages request to the Anthropic upstream. [`EnvelopeBytes`] consumes the body, so it
/// comes last. The [`Forward`](crate::adapters::anthropic::Forward) value is empty when the Anthropic client is absent; the pipeline
/// then answers `500` before any egress.
pub(crate) async fn create_message(State(state): State<AppState>, caller: Option<Extension<CallerContext>>, headers: HeaderMap, uri: OriginalUri, body: EnvelopeBytes) -> Response {
    let Some(Extension(caller)) = caller else {
        // Unreachable in practice — the auth middleware always attaches a CallerContext.
        return GatewayError::Internal.into_response();
    };
    let fwd = state.anthropic.as_ref().map(|client| client.forward(&headers, uri.query())).unwrap_or_default();
    pipeline::proxy(&MESSAGES, state, caller, fwd, body).await
}

/// The descriptor of `POST /v1/messages`.
pub static MESSAGES: Endpoint = Endpoint {
    name: "messages",
    upstream: Upstream::Anthropic(AnthropicRoute::Messages),
    format: Format::AnthropicMessages,
    parse: parse_messages,
    non_stream_usage: anthropic_usage::non_stream_usage,
    stream_usage: || StreamUsage::Anthropic(AnthropicUsageScanner::new()),
    terminal: MESSAGES_TERMINAL_SSE_ERROR,
    limits: messages_limits,
};

/// D15: the interception limits with the record, hold and time caps of `[messages]`. The route
/// is mounted only when `state.messages` is `Some`, so the fallback to the shared limits is not
/// reached.
fn messages_limits(ti: &ToolInterception, state: &AppState) -> Limits {
    match &state.messages {
        Some(settings) => Limits {
            max_record_bytes: settings.max_record_bytes,
            max_held_bytes: settings.max_held_bytes,
            max_hold: settings.max_hold,
            ..ti.limits
        },
        None => ti.limits,
    }
}

/// Parse a COPY of the body (§ 8). A body that cannot be read is `BadRequestBody`; a body that is
/// JSON but not the request shape (not an object, no string `model`, `messages` not an array,
/// `stream` not a bool) is `InvalidRequestSchema`. D19 runs between the two: serde keeps the last
/// of two duplicate keys and Anthropic may keep the first, so the gateway could read
/// `stream:false` while the upstream reads `stream:true`.
fn parse_messages(body: &[u8], interception: bool) -> Result<RequestFacts, GatewayError> {
    let value: Value = serde_json::from_slice(body).map_err(|e| match e.classify() {
        serde_json::error::Category::Data => GatewayError::InvalidRequestSchema,
        serde_json::error::Category::Syntax | serde_json::error::Category::Eof | serde_json::error::Category::Io => GatewayError::BadRequestBody,
    })?;
    let dups = duplicates(body).ok_or(GatewayError::BadRequestBody)?;
    if !dups.top.is_empty() {
        return Err(GatewayError::BadRequestBody);
    }
    if interception && dups.any {
        return Err(GatewayError::InvalidRequestSchema);
    }
    let dto = MessagesRequest::deserialize(&value).map_err(|_| GatewayError::InvalidRequestSchema)?;
    Ok(RequestFacts {
        request_estimate: anthropic_usage::request_tokens_estimate(&value, body.len()),
        model: dto.model,
        stream: dto.stream.unwrap_or(false),
    })
}

#[cfg(test)]
mod tests {
    use axum::body::Bytes;
    use futures::StreamExt;
    use serde_json::json;

    use super::*;
    use crate::adapters::http::chat::tests::make_reqwest_error;
    use crate::adapters::http::relay::terminal_sse_error_stream;

    /// The `data:` JSON of the D13 frame: strip the leading blank lines and the `event:` line.
    fn frame_data(frame: &str) -> Value {
        let rest = frame.trim_start_matches('\n');
        let rest = rest.strip_prefix("event: error\n").expect("the frame names its event");
        let payload = rest.strip_prefix("data: ").expect("a data line").trim_end();
        serde_json::from_str(payload).expect("the frame's data is JSON")
    }

    /// D13 and the registry gate: the frame's `code` resolves in the registry (not a comparison
    /// with the literal it is built from), and the event is the Anthropic `error` shape.
    #[test]
    fn the_messages_terminal_frame_carries_a_registered_code() {
        use paigasus_proto::paigasus::common::v1::ErrorReason;

        let parsed = frame_data(MESSAGES_TERMINAL_SSE_ERROR);
        let code = parsed["error"]["code"].as_str().expect("a code");
        assert!(ErrorReason::from_wire_reason(code).is_some(), "{code} is not declared in common/v1/error.proto");
        assert_eq!(parsed["type"], "error");
        assert_eq!(parsed["error"]["type"], "api_error");
    }

    /// D13: after a partial record, the frame is a record of its own and parses by itself.
    #[tokio::test]
    async fn the_terminal_frame_is_its_own_record_after_a_partial_one() {
        let inner = futures::stream::iter(vec![Ok(Bytes::from_static(
            b"event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"par",
        ))])
        .chain(futures::stream::once(async { Err(make_reqwest_error().await) }))
        .boxed();
        let out: Vec<Bytes> = terminal_sse_error_stream(inner, None, StreamUsage::Anthropic(AnthropicUsageScanner::new()), MESSAGES_TERMINAL_SSE_ERROR)
            .map(|r| r.expect("infallible"))
            .collect()
            .await;
        let text = String::from_utf8(out.concat()).expect("utf-8");
        let last = text.split("\n\n").filter(|record| !record.is_empty()).last().expect("at least one record");
        assert_eq!(frame_data(last)["error"]["code"], "upstream-error", "{text}");
    }

    fn parse(raw: &str, interception: bool) -> Result<RequestFacts, GatewayError> {
        parse_messages(raw.as_bytes(), interception)
    }

    /// SMA-588: a syntax failure is `BadRequestBody`; a body of the wrong shape is
    /// `InvalidRequestSchema`. The facts come from the DTO.
    #[test]
    fn the_parse_splits_syntax_and_type_failures() {
        for raw in ["{not json", "", r#"{"model":"m","messages":[]"#] {
            assert!(matches!(parse(raw, false), Err(GatewayError::BadRequestBody)), "{raw:?}");
        }
        for body in [
            json!({"messages":[]}),
            json!({"model":"m","messages":{"role":"user"}}),
            json!({"model":"m","messages":[],"stream":"yes"}),
            json!({"model":1,"messages":[]}),
            json!([1]),
        ] {
            assert!(matches!(parse(&body.to_string(), false), Err(GatewayError::InvalidRequestSchema)), "{body}");
        }
        let facts = parse(r#"{"model":"claude-x","max_tokens":1,"messages":[{"role":"user","content":"hi"}]}"#, true).expect("a minimal body");
        assert_eq!((facts.model.as_str(), facts.stream), ("claude-x", false));
        assert!(facts.request_estimate >= 1);
        let facts = parse(r#"{"model":"claude-x","messages":[],"stream":true}"#, false).expect("a stream body");
        assert!(facts.stream);
        assert!(!parse(r#"{"model":"claude-x","messages":[],"stream":null}"#, false).expect("null is absent").stream);
    }

    /// D19: a duplicate top-level key is a 400 `BadRequestBody` with interception on and off.
    #[test]
    fn a_duplicate_top_level_key_is_refused_in_both_modes() {
        for raw in [
            r#"{"model":"m","messages":[],"stream":false,"stream":true}"#,
            r#"{"model":"m","messages":[],"stream":true,"stream":false}"#,
            r#"{"model":"m","model":"m","messages":[]}"#,
            r#"{"model":"m","messages":[],"max_tokens":1,"max_tokens":2}"#,
        ] {
            for interception in [true, false] {
                assert!(matches!(parse(raw, interception), Err(GatewayError::BadRequestBody)), "{raw}, interception {interception}");
            }
        }
    }

    /// D19: a duplicate key below the top level is a 400 `InvalidRequestSchema` with interception
    /// on, and passes with interception off. The same body without the duplicate passes.
    #[test]
    fn a_nested_duplicate_is_refused_only_with_interception() {
        for raw in [
            r#"{"model":"m","messages":[{"role":"user","content":"a","content":"b"}]}"#,
            r#"{"model":"m","messages":[],"tools":[{"name":"t","input_schema":{"type":"object","type":"string"}}]}"#,
            r#"{"model":"m","messages":[],"metadata":{"user_id":"a","user_id":"b"}}"#,
        ] {
            assert!(matches!(parse(raw, true), Err(GatewayError::InvalidRequestSchema)), "{raw}");
            assert!(parse(raw, false).is_ok(), "{raw}: interception off passes the body");
        }
        assert!(parse(r#"{"model":"m","messages":[{"role":"user","content":"a"}],"metadata":{"user_id":"a","User_id":"b"}}"#, true).is_ok());
    }
}
