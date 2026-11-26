// SPDX-License-Identifier: Apache-2.0

//! The `POST /v1/responses` proxy handler (SMA-769 D3–D5, D8, D13, D15, § 7.2).
//!
//! The route is mounted only when `[responses] enabled = true` (D8); otherwise it is a 404. The
//! flow is the generic [`pipeline::proxy`]; this module gives the [`RESPONSES`] descriptor:
//!
//! - **Parse** (§ 7.2): a COPY of the body is read into [`ResponsesRequest`]. `input` must be a
//!   string or an array. D4 (`background: true`) is refused always; D3 (a non-null
//!   `previous_response_id`, `conversation` or `prompt`, or an `item_reference` input item) is
//!   refused only when interception is on. The pipeline calls the parse before `Limits::admit`,
//!   so a refused request uses no quota and is not charged. A JSON `null` and `background: false`
//!   pass (Review Focus 2).
//! - **D5:** the request body goes upstream byte-identical. The gateway does not set `store`.
//! - **Usage** (D14): the Responses readers of [`responses_usage`](super::responses_usage).
//! - **Terminal frame** (D13): [`RESPONSES_TERMINAL_SSE_ERROR`].
//! - **Limits** (D15): the interception limits with the record and hold caps of `[responses]`.
//!
//! Streaming obeys the same `stream_enabled` switch as chat (D9): the pipeline reads the one
//! shared capability.

use axum::Extension;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use paigasus_gateway_core::{Format, Limits};
use serde::Deserialize;
use serde_json::Value;

use super::AppState;
use super::duplicates::duplicates;
use super::error::GatewayError;
use super::interception::ToolInterception;
use super::pipeline::{self, Endpoint, RequestFacts, Upstream};
use super::responses_usage::{self, ResponsesUsageScanner};
use super::usage::StreamUsage;
use crate::adapters::anthropic::Forward;
use crate::adapters::http::bytes::EnvelopeBytes;
use crate::adapters::http::dto::ResponsesRequest;
use crate::adapters::openai::UpstreamRoute;
use crate::domain::CallerContext;

/// D13: the terminal SSE frame of a Responses stream that fails mid-flight. It is an `error`
/// event in the Responses shape, with the registry's kebab `upstream-error`. As with the chat
/// frame, the leading blank line ends a partly sent record, so the frame is a record of its own.
pub const RESPONSES_TERMINAL_SSE_ERROR: &str = "\n\nevent: error\ndata: {\"type\":\"error\",\"code\":\"upstream-error\",\"message\":\"upstream stream error\",\"param\":null}\n\n";

/// Proxy a Responses request to the OpenAI upstream. The extractor rules are those of
/// [`chat_completions`](super::chat::chat_completions): [`EnvelopeBytes`] consumes the body, so it
/// comes last, and it answers an over-limit body (the `[responses] max_request_bytes` layer) with
/// a `413` inside the OpenAI envelope.
pub(crate) async fn create_response(State(state): State<AppState>, caller: Option<Extension<CallerContext>>, body: EnvelopeBytes) -> Response {
    let Some(Extension(caller)) = caller else {
        // Unreachable in practice — the auth middleware always attaches a CallerContext.
        return GatewayError::Internal.into_response();
    };
    pipeline::proxy(&RESPONSES, state, caller, Forward::default(), body).await
}

/// The descriptor of `POST /v1/responses`.
pub static RESPONSES: Endpoint = Endpoint {
    name: "responses",
    upstream: Upstream::OpenAi(UpstreamRoute::Responses),
    format: Format::OpenAiResponses,
    parse: parse_responses,
    strict_with_policy: false,
    non_stream_usage: responses_usage::non_stream_usage,
    stream_usage: || StreamUsage::Responses(ResponsesUsageScanner::new()),
    terminal: RESPONSES_TERMINAL_SSE_ERROR,
    limits: responses_limits,
};

/// D15: the interception limits with the record and hold caps of `[responses]`. The route is
/// mounted only when `state.responses` is `Some`, so the fallback to the shared limits is not
/// reached.
fn responses_limits(ti: &ToolInterception, state: &AppState) -> Limits {
    match &state.responses {
        Some(settings) => Limits {
            max_record_bytes: settings.max_record_bytes,
            max_held_bytes: settings.max_held_bytes,
            ..ti.limits
        },
        None => ti.limits,
    }
}

/// Parse a COPY of the body (§ 7.2). The failure split is that of chat (SMA-588): a body that
/// cannot be read is `BadRequestBody`; a body that is JSON but not the request shape (not an
/// object, a field of the wrong type, an `input` that is not a string or an array) is
/// `InvalidRequestSchema`. Then D4 and D3, in the order of [`refuse_stateful`].
fn parse_responses(body: &[u8], interception: bool) -> Result<RequestFacts, GatewayError> {
    let value: Value = serde_json::from_slice(body).map_err(|e| match e.classify() {
        serde_json::error::Category::Data => GatewayError::InvalidRequestSchema,
        serde_json::error::Category::Syntax | serde_json::error::Category::Eof | serde_json::error::Category::Io => GatewayError::BadRequestBody,
    })?;
    // Coordinator ruling (fix round 1): a duplicate key must not decide D3 or D4. A duplicate
    // top-level `background` is refused in both modes (D4). With interception on, a duplicate key
    // at any depth is refused, because the turn facts and D3 would read the last value and the
    // upstream may read the first. With interception off the body passes as before.
    let dups = duplicates(body).ok_or(GatewayError::BadRequestBody)?;
    if dups.top.contains("background") {
        return Err(GatewayError::ResponsesStatefulNotSupported { param: "background" });
    }
    if interception && dups.any {
        return Err(GatewayError::InvalidRequestSchema);
    }
    let dto = ResponsesRequest::deserialize(&value).map_err(|_| GatewayError::InvalidRequestSchema)?;
    if !matches!(dto.input, None | Some(Value::String(_) | Value::Array(_))) {
        return Err(GatewayError::InvalidRequestSchema);
    }
    refuse_stateful(&dto, interception)?;
    Ok(RequestFacts {
        request_estimate: responses_usage::request_tokens_estimate(&value, body.len()),
        model: dto.model.unwrap_or_else(|| "-".to_owned()),
        stream: dto.stream.unwrap_or(false),
    })
}

/// D4 always, then D3 when interception is on, in the order `background`,
/// `previous_response_id`, `conversation`, `prompt`, `input`. The first match names its `param`.
fn refuse_stateful(dto: &ResponsesRequest, interception: bool) -> Result<(), GatewayError> {
    let refuse = |param| Err(GatewayError::ResponsesStatefulNotSupported { param });
    if dto.background == Some(true) {
        return refuse("background");
    }
    if !interception {
        return Ok(());
    }
    if dto.previous_response_id.is_some() {
        return refuse("previous_response_id");
    }
    if dto.conversation.is_some() {
        return refuse("conversation");
    }
    if dto.prompt.is_some() {
        return refuse("prompt");
    }
    if let Some(Value::Array(items)) = &dto.input
        && items.iter().any(is_item_reference)
    {
        return refuse("input");
    }
    Ok(())
}

/// An `item_reference` input item. Its `type` is optional in the API (openai-python 3.48.0
/// `ItemReference`: `id` required, `type` `Optional[Literal["item_reference"]]`). The only other
/// untyped item, the easy input message, always has a string `role`. So, failing closed, an item
/// with no string `type` is a reference unless its `role` is a string.
fn is_item_reference(item: &Value) -> bool {
    let Some(obj) = item.as_object() else { return false };
    match obj.get("type") {
        Some(Value::String(ty)) => ty == "item_reference",
        _ => !matches!(obj.get("role"), Some(Value::String(_))),
    }
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

    /// D13 and AC 6: the frame's `code` resolves in the registry (not a comparison with the
    /// literal it is built from), and the event is the Responses `error` shape.
    #[test]
    fn the_responses_terminal_frame_carries_a_registered_code() {
        use paigasus_proto::paigasus::common::v1::ErrorReason;

        let parsed = frame_data(RESPONSES_TERMINAL_SSE_ERROR);
        let code = parsed["code"].as_str().expect("a code");
        assert!(ErrorReason::from_wire_reason(code).is_some(), "{code} is not declared in common/v1/error.proto");
        assert_eq!(parsed["type"], "error");
        assert_eq!(parsed["param"], Value::Null);
    }

    /// D13: after a partial record, the frame is a record of its own and parses by itself.
    #[tokio::test]
    async fn the_terminal_frame_is_its_own_record_after_a_partial_one() {
        let inner = futures::stream::iter(vec![Ok(Bytes::from_static(
            b"event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"par",
        ))])
        .chain(futures::stream::once(async { Err(make_reqwest_error().await.into()) }))
        .boxed();
        let out: Vec<Bytes> = terminal_sse_error_stream(inner, None, StreamUsage::Responses(ResponsesUsageScanner::new()), RESPONSES_TERMINAL_SSE_ERROR)
            .map(|r| r.expect("infallible"))
            .collect()
            .await;
        let text = String::from_utf8(out.concat()).expect("utf-8");
        let last = text.split("\n\n").filter(|record| !record.is_empty()).last().expect("at least one record");
        assert_eq!(frame_data(last)["code"], "upstream-error", "{text}");
    }

    fn parse(body: &Value, interception: bool) -> Result<RequestFacts, GatewayError> {
        parse_responses(body.to_string().as_bytes(), interception)
    }

    fn refused(body: &Value, interception: bool) -> Option<&'static str> {
        match parse(body, interception) {
            Err(GatewayError::ResponsesStatefulNotSupported { param }) => Some(param),
            Err(e) => panic!("{body}: not a stateful refusal: {e:?}"),
            Ok(_) => None,
        }
    }

    /// Review Focus 2: `null` and `false` pass; each non-null D3 field refuses with its `param`
    /// when interception is on and passes when it is off; `background: true` refuses always.
    #[test]
    fn stateful_fields_are_refused_only_when_non_null() {
        let nulls = json!({"model":"gpt-6","input":"hi","background":false,"previous_response_id":null,"conversation":null,"prompt":null});
        for interception in [true, false] {
            assert_eq!(refused(&nulls, interception), None, "interception {interception}");
        }
        let d3 = [
            ("previous_response_id", json!("resp_1")),
            ("conversation", json!("conv_1")),
            ("conversation", json!({"id":"conv_1"})),
            ("prompt", json!({"id":"pmpt_1"})),
        ];
        for (field, value) in d3 {
            let mut body = json!({"model":"gpt-6","input":"hi"});
            body[field] = value;
            assert_eq!(refused(&body, true), Some(field), "{body}");
            assert_eq!(refused(&body, false), None, "{body}: a plain proxy with interception off");
        }
        // An `item_reference` input item, typed or not (the SDK's `type` is optional).
        for item in [json!({"type":"item_reference","id":"msg_1"}), json!({"id":"msg_1"}), json!({"type":null,"id":"msg_1"})] {
            let body = json!({"model":"gpt-6","input":[{"role":"user","content":"hi"}, item]});
            assert_eq!(refused(&body, true), Some("input"), "{body}");
            assert_eq!(refused(&body, false), None, "{body}");
        }
        // A replayed item with an `id` and its own type is not a reference.
        let replay = json!({"model":"gpt-6","input":[
            {"role":"user","content":"hi"},
            {"type":"message","id":"msg_1","role":"assistant","content":[{"type":"output_text","text":"ok"}]},
            {"type":"function_call","id":"fc_1","call_id":"c1","name":"f","arguments":"{}"},
            {"type":"function_call_output","call_id":"c1","output":"done"}
        ]});
        assert_eq!(refused(&replay, true), None);
        for interception in [true, false] {
            let body = json!({"model":"gpt-6","input":"hi","background":true});
            assert_eq!(refused(&body, interception), Some("background"), "interception {interception}");
        }
        // The order: `background` first, then the D3 fields as listed.
        let all = json!({"input":[{"id":"msg_1"}],"prompt":{"id":"p"},"conversation":"c","previous_response_id":"r","background":true});
        assert_eq!(refused(&all, true), Some("background"));
        let no_bg = json!({"input":[{"id":"msg_1"}],"prompt":{"id":"p"},"conversation":"c","previous_response_id":"r"});
        assert_eq!(refused(&no_bg, true), Some("previous_response_id"));
        let no_prev = json!({"input":[{"id":"msg_1"}],"prompt":{"id":"p"},"conversation":"c"});
        assert_eq!(refused(&no_prev, true), Some("conversation"));
        let no_conv = json!({"input":[{"id":"msg_1"}],"prompt":{"id":"p"}});
        assert_eq!(refused(&no_conv, true), Some("prompt"));
    }

    /// § 7.2: `input` is a string or an array; a body that is not a JSON object is a 400 as in
    /// chat; `model` is `"-"` when absent.
    #[test]
    fn input_must_be_a_string_or_an_array() {
        for input in [json!(1), json!({"role":"user"}), json!(true)] {
            let body = json!({"model":"gpt-6","input":input});
            assert!(matches!(parse(&body, false), Err(GatewayError::InvalidRequestSchema)), "{body}");
        }
        for input in [json!("hi"), json!([]), Value::Null] {
            let body = json!({"model":"gpt-6","input":input});
            assert!(parse(&body, true).is_ok(), "{body}");
        }
        for not_an_object in [json!([1]), json!("hi"), json!(1)] {
            let Err(err) = parse(&not_an_object, false) else { panic!("{not_an_object}: not an object") };
            assert!(matches!(err, GatewayError::InvalidRequestSchema), "{not_an_object}: {err:?}");
            assert_eq!(err.status(), axum::http::StatusCode::BAD_REQUEST);
        }
        assert!(matches!(parse_responses(b"{not json", false), Err(GatewayError::BadRequestBody)));
        assert!(matches!(parse(&json!({"stream":"yes"}), false), Err(GatewayError::InvalidRequestSchema)));
        let facts = parse(&json!({"input":"hi"}), false).expect("a minimal body");
        assert_eq!((facts.model.as_str(), facts.stream), ("-", false));
        let facts = parse(&json!({"model":"gpt-6","input":"hi","stream":true}), false).expect("a stream body");
        assert_eq!((facts.model.as_str(), facts.stream), ("gpt-6", true));
    }

    /// The outcome of a raw body: `Ok`, the stateful `param`, or another error.
    fn outcome(raw: &str, interception: bool) -> Result<(), Result<&'static str, GatewayError>> {
        match parse_responses(raw.as_bytes(), interception) {
            Ok(_) => Ok(()),
            Err(GatewayError::ResponsesStatefulNotSupported { param }) => Err(Ok(param)),
            Err(e) => Err(Err(e)),
        }
    }

    /// Coordinator ruling (fix round 1): serde keeps the last of two duplicate keys, and the
    /// upstream may keep the first. With interception on, a duplicate key at any depth is a 400
    /// `InvalidRequestSchema`. With interception off the body passes, except a duplicate
    /// `background`, which D4 refuses in both modes. The body without the duplicate passes.
    #[test]
    fn duplicate_keys_are_refused() {
        let schema = Err(Err(GatewayError::InvalidRequestSchema));
        let on_only = [
            r#"{"input":"hi","previous_response_id":"resp_1","previous_response_id":null}"#,
            r#"{"input":"hi","previous_response_id":null,"previous_response_id":"resp_1"}"#,
            r#"{"input":[{"role":"user","content":"hi","content":"x"}]}"#,
            r#"{"input":[{"role":"user","content":[{"type":"input_text","text":"a","text":"b"}]}]}"#,
            r#"{"input":"hi","metadata":{"k":1,"k":2}}"#,
            r#"{"input":"hi","a":1,"a":2}"#,
        ];
        for raw in on_only {
            assert_eq!(outcome(raw, true), schema, "{raw}");
            assert_eq!(outcome(raw, false), Ok(()), "{raw}: interception off passes the body");
        }
        for raw in [
            r#"{"input":"hi","background":false,"background":false}"#,
            r#"{"input":"hi","background":true,"background":false}"#,
            r#"{"input":"hi","background":false,"background":true}"#,
        ] {
            for interception in [true, false] {
                assert_eq!(outcome(raw, interception), Err(Ok("background")), "{raw}, interception {interception}");
            }
        }
        // A `background` key below the top level is not D4.
        assert_eq!(outcome(r#"{"input":"hi","metadata":{"background":1,"background":2}}"#, false), Ok(()));
        for raw in [
            r#"{"input":"hi","previous_response_id":null}"#,
            r#"{"input":[{"role":"user","content":"hi"}]}"#,
            r#"{"input":"hi","metadata":{"k":1,"K":2}}"#,
            r#"{"input":"hi","background":false}"#,
        ] {
            assert_eq!(outcome(raw, true), Ok(()), "{raw}");
        }
    }

    /// Fix round 1: an input item with no string `type` is a reference unless `role` is a string.
    #[test]
    fn an_untyped_item_without_a_string_role_is_a_reference() {
        for item in [
            json!({"id":"msg_1","role":null}),
            json!({"id":"msg_1","role":5}),
            json!({"type":5,"id":"msg_1"}),
            json!({"type":null,"id":"msg_1","role":null}),
        ] {
            let body = json!({"input":[item]});
            assert_eq!(refused(&body, true), Some("input"), "{body}");
        }
        for item in [json!({"role":"user","content":"hi"}), json!({"type":null,"role":"user","content":"hi"})] {
            let body = json!({"input":[item]});
            assert_eq!(refused(&body, true), None, "{body}");
        }
    }
}
