// SPDX-License-Identifier: Apache-2.0

//! The `POST /v1/chat/completions` proxy handler — the integration crux of the M0 slice.
//!
//! This module holds only the parts that are specific to chat: the [`CHAT`] descriptor, the
//! request parse ([`ChatCompletionRequest`], read from a COPY of the body for `model` and
//! `stream` only), and the terminal SSE frame `TERMINAL_SSE_ERROR`. The handler gives
//! [`CHAT`] to [`pipeline::proxy`], which holds the flow and its security rules (egress hygiene
//! included). The body-size limit and its `413` inside the OpenAI envelope come from
//! [`EnvelopeBytes`] (SMA-588). The mid-stream terminal frame and the cancel-on-drop contract are
//! described in [`relay`](super::relay).

use axum::Extension;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use paigasus_gateway_core::Format;

use super::AppState;
use super::error::GatewayError;
use super::pipeline::{self, Endpoint, RequestFacts};
use crate::adapters::http::bytes::EnvelopeBytes;
use crate::adapters::http::dto::ChatCompletionRequest;
use crate::adapters::http::usage::{self, StreamUsage, UsageScanner};
use crate::adapters::openai::UpstreamRoute;
use crate::domain::CallerContext;

/// The single terminal SSE event emitted when a stream fails mid-flight. Static, caller-safe (no
/// upstream detail), and shaped like the OpenAI error envelope wrapped in an SSE `data:` frame so
/// SDKs that parse the stream see a well-formed terminal error. `code` is the registry's kebab
/// `upstream-error` (SMA-504).
///
/// This frame carries NO `paigasus-retryable` header: by the time it is emitted, the `200 OK`
/// head is already committed, so no header can change at this point. Nor does its JSON gain a
/// `retryable` key — that would add a field the OpenAI envelope shape (AC 3) does not have, on
/// this one frame only. Retryability is derivable anyway without either mechanism: `upstream-error`
/// is by construction the transient mid-stream case (the request already reached the upstream and
/// was streaming successfully before it failed), so a client that recognizes this code already
/// knows it may retry.
///
/// The frame STARTS with a blank line (SMA-635 spec §4.6). An upstream failure can fall inside a
/// record; without the blank line the frame would join that partial record and no SSE parser
/// could read it. After a clean record boundary the extra blank line is an empty record, which
/// dispatches nothing, so a client that reads a clean boundary sees no change.
pub(crate) const TERMINAL_SSE_ERROR: &str = "\n\ndata: {\"error\":{\"message\":\"upstream stream error\",\"type\":\"api_error\",\"param\":null,\"code\":\"upstream-error\"}}\n\n";

/// Proxy a chat-completion request to the OpenAI upstream.
///
/// Extractor order matters: [`State`] and [`Extension`] are `FromRequestParts` (they read only the
/// head), while [`EnvelopeBytes`] is `FromRequest` (it consumes the body) and so must come LAST.
/// [`EnvelopeBytes`] wraps [`Bytes`](axum::body::Bytes), so it honours the
/// [`DefaultBodyLimit`](axum::extract::DefaultBodyLimit) layer the same way and returns `413`
/// for an over-limit body before this function body runs — rendered through the OpenAI envelope
/// rather than as axum's plain text (SMA-588). It is still `FromRequest`, so it must come LAST.
///
/// `CallerContext` is taken as `Option<Extension<_>>` for defence in depth: the G5 middleware
/// guarantees its presence on any request routed here, so its absence is an unreachable internal
/// bug surfaced as a `500` (rendered through the OpenAI envelope) rather than axum's default
/// extension-missing response.
///
/// The flow itself is [`pipeline::proxy`]; this handler only supplies the [`CHAT`] descriptor.
pub(crate) async fn chat_completions(State(state): State<AppState>, caller: Option<Extension<CallerContext>>, body: EnvelopeBytes) -> Response {
    let Some(Extension(caller)) = caller else {
        // Unreachable in practice — the auth middleware always attaches a CallerContext.
        return GatewayError::Internal.into_response();
    };
    pipeline::proxy(&CHAT, state, caller, body).await
}

/// The descriptor of `POST /v1/chat/completions`.
pub static CHAT: Endpoint = Endpoint {
    name: "chat_completions",
    route: UpstreamRoute::ChatCompletions,
    format: Format::OpenAiChat,
    parse: parse_chat,
    non_stream_usage: usage::non_stream_usage,
    stream_usage: || StreamUsage::Chat(UsageScanner::new()),
    terminal: TERMINAL_SSE_ERROR,
    limits: |ti, _state| ti.limits,
};

/// Parse a COPY of the body only to read `model` and `stream`; the ORIGINAL bytes flow upstream
/// verbatim. SMA-588 splits the failure: `serde_json` classifies its own errors, so a body that
/// PARSED but did not match the type is reported distinctly from one that could not be read at
/// all. `Category::Io` cannot arise from a `&[u8]`, but is grouped with the unreadable cases so
/// the match is exhaustive without a wildcard.
fn parse_chat(body: &[u8], _interception: bool) -> Result<RequestFacts, GatewayError> {
    let dto: ChatCompletionRequest = match serde_json::from_slice(body) {
        Ok(dto) => dto,
        Err(e) => {
            return Err(match e.classify() {
                serde_json::error::Category::Data => GatewayError::InvalidRequestSchema,
                serde_json::error::Category::Syntax | serde_json::error::Category::Eof | serde_json::error::Category::Io => GatewayError::BadRequestBody,
            });
        }
    };
    Ok(RequestFacts {
        request_estimate: usage::request_tokens_estimate(&dto.messages, body.len()),
        model: dto.model,
        stream: dto.stream,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use std::convert::Infallible;

    use axum::body::Bytes;
    use futures::{Stream, StreamExt};

    use super::*;
    use crate::adapters::http::relay::terminal_sse_error_stream;
    use crate::adapters::upstream::UpstreamByteStream;

    /// The plain relay as the pipeline builds it for chat (no interception).
    fn plain_relay(inner: UpstreamByteStream, guard: Option<ChargeGuard>) -> impl Stream<Item = Result<Bytes, Infallible>> + Send + 'static {
        terminal_sse_error_stream(inner, guard, StreamUsage::Chat(UsageScanner::new()), TERMINAL_SSE_ERROR)
    }

    /// Section 4.6: the relay never changes a forwarded byte, with or without a scanner (A5).
    #[tokio::test]
    async fn the_relay_forwards_bytes_unchanged() {
        let chunks = vec![Ok(Bytes::from_static(b"data: {\"usage\":{\"total_tokens\":3}}\r\n")), Ok(Bytes::from_static(b"\r\ndata: [DONE]\n\n"))];
        let out: Vec<Bytes> = plain_relay(futures::stream::iter(chunks).boxed(), None).map(|r| r.unwrap()).collect().await;
        assert_eq!(out.concat(), b"data: {\"usage\":{\"total_tokens\":3}}\r\n\r\ndata: [DONE]\n\n".to_vec());
    }

    #[tokio::test]
    async fn terminal_stream_forwards_chunks_then_ends_cleanly() {
        // A clean upstream stream (no error) is forwarded chunk-for-chunk with no terminal event.
        let inner = futures::stream::iter(vec![Ok(Bytes::from_static(b"data: a\n\n")), Ok(Bytes::from_static(b"data: b\n\n"))]).boxed();
        let out: Vec<Bytes> = plain_relay(inner, None).map(|r| r.unwrap()).collect().await;
        let assembled: Vec<u8> = out.iter().flat_map(|b| b.to_vec()).collect();
        assert_eq!(String::from_utf8(assembled).unwrap(), "data: a\n\ndata: b\n\n");
    }

    #[tokio::test]
    async fn terminal_stream_emits_terminal_event_on_first_error_then_stops() {
        // After the first upstream error the adapter emits exactly one terminal event and ends —
        // any items the upstream would have produced afterwards are never observed.
        let inner = futures::stream::iter(vec![Ok(Bytes::from_static(b"data: a\n\n"))])
            .chain(futures::stream::once(async { Err(make_reqwest_error().await) }))
            .boxed();
        let out: Vec<Bytes> = plain_relay(inner, None).map(|r| r.unwrap()).collect().await;
        let assembled: Vec<u8> = out.iter().flat_map(|b| b.to_vec()).collect();
        let text = String::from_utf8(assembled).unwrap();
        assert!(text.starts_with("data: a\n\n"), "the pre-error chunk is forwarded: {text}");
        assert!(text.ends_with(TERMINAL_SSE_ERROR), "the stream ends with exactly the terminal SSE error event: {text}");
        assert_eq!(text.matches("\"code\":\"upstream-error\"").count(), 1, "exactly one terminal error event");
    }

    /// SMA-635 spec §4.6: an upstream failure INSIDE a record must not join the terminal frame to
    /// the partial record. The frame starts with a blank line, so it is a record of its own and
    /// parses as JSON by itself.
    #[tokio::test]
    async fn the_terminal_frame_is_its_own_record_after_a_partial_one() {
        let inner = futures::stream::iter(vec![Ok(Bytes::from_static(b"data: {\"choices\":[{\"delta\":{\"content\":\"par"))])
            .chain(futures::stream::once(async { Err(make_reqwest_error().await) }))
            .boxed();
        let out: Vec<Bytes> = plain_relay(inner, None).map(|r| r.unwrap()).collect().await;
        let text = String::from_utf8(out.iter().flat_map(|b| b.to_vec()).collect()).unwrap();
        let records: Vec<&str> = text.split("\n\n").filter(|record| !record.is_empty()).collect();
        let last = records.last().expect("at least one record");
        let payload = last.strip_prefix("data: ").expect("the last record is a data record of its own");
        let parsed: serde_json::Value = serde_json::from_str(payload).expect("the terminal record parses as JSON on its own");
        assert_eq!(parsed["error"]["code"], "upstream-error");
    }

    /// AC 6 for the terminal SSE frame. Parses the frame's JSON and resolves the `code` field
    /// rather than string-comparing the same literal the constant is built from — a comparison
    /// against the literal would pass even if the code were never registered.
    #[test]
    fn the_terminal_sse_frame_carries_a_registered_code() {
        use paigasus_proto::paigasus::common::v1::ErrorReason;

        let payload = TERMINAL_SSE_ERROR.trim_start_matches('\n').strip_prefix("data: ").expect("an SSE data frame").trim_end();
        let parsed: serde_json::Value = serde_json::from_str(payload).expect("the frame must be valid JSON");
        let code = parsed["error"]["code"].as_str().expect("a code");
        assert!(ErrorReason::from_wire_reason(code).is_some(), "{code} is not declared in common/v1/error.proto");
        assert_eq!(parsed["error"]["type"], "api_error", "the frame keeps the OpenAI envelope shape");
    }

    use crate::application::charge_guard::{ChargeGuard, NoTicket};
    use crate::application::limits::Limits;
    use crate::domain::limits::{BudgetPeriod, LimitDecision, LimitRules, LimitStoreError, LimitTicket};
    use crate::test_support::{FixedClock, ORG_A, ScriptedStore, at};
    use std::sync::Arc;

    pub(crate) fn admit() -> Result<LimitDecision, LimitStoreError> {
        Ok(LimitDecision::Admit(None))
    }

    /// A sent guard with a ticket and a request estimate of 1, charging into `store`.
    pub(crate) fn sent_guard(store: &Arc<ScriptedStore>) -> ChargeGuard {
        let now = at("2026-10-02T12:00:00Z");
        let limits = Arc::new(Limits::new(LimitRules::default(), store.clone(), Arc::new(FixedClock(now))));
        let ticket = LimitTicket {
            org: ORG_A,
            period: BudgetPeriod::Monthly.key_at(now),
        };
        let mut guard = ChargeGuard::new(limits, Some(ticket), Some(ORG_A), NoTicket::NoBudget);
        guard.set_request_estimate(1);
        guard.mark_sent();
        guard
    }

    pub(crate) const UNTERMINATED_USAGE: &[u8] = b"data: {\"choices\":[],\"usage\":{\"total_tokens\":7}}";

    /// SMA-730 R1 (spec D2): an upstream error after a usage record with no blank line charges
    /// the reported total. The terminal frame closes that record for the client too.
    #[tokio::test]
    async fn an_error_after_an_unterminated_usage_record_charges_the_reported_tokens() {
        let store = ScriptedStore::new(admit);
        let inner = futures::stream::iter(vec![Ok(Bytes::from_static(UNTERMINATED_USAGE))])
            .chain(futures::stream::once(async { Err(make_reqwest_error().await) }))
            .boxed();
        let out: Vec<Bytes> = plain_relay(inner, Some(sent_guard(&store))).map(|r| r.unwrap()).collect().await;
        assert!(out.concat().ends_with(TERMINAL_SSE_ERROR.as_bytes()));
        assert_eq!(store.charges(), vec![7]);
    }

    /// SMA-730 R2 (spec D3): a client that disconnects drops the relay. The unterminated record
    /// does not count, and the guard charges the estimate.
    #[tokio::test]
    async fn a_dropped_relay_does_not_finish_the_last_record() {
        let store = ScriptedStore::new(admit);
        let inner = futures::stream::iter(vec![Ok(Bytes::from_static(UNTERMINATED_USAGE))]).chain(futures::stream::pending()).boxed();
        let mut relay = Box::pin(plain_relay(inner, Some(sent_guard(&store))));
        assert_eq!(relay.next().await.expect("one chunk").unwrap(), Bytes::from_static(UNTERMINATED_USAGE));
        assert!(store.charges().is_empty(), "nothing is charged while the stream is open");
        drop(relay);
        assert_eq!(store.charges(), vec![1], "the request estimate only");
    }

    /// Review Focus 3: without a guard an unterminated tail is forwarded unchanged.
    #[tokio::test]
    async fn the_relay_forwards_an_unterminated_tail_unchanged_without_a_guard() {
        let chunks = vec![Ok(Bytes::from_static(b"data: a\n\n")), Ok(Bytes::from_static(UNTERMINATED_USAGE))];
        let out: Vec<Bytes> = plain_relay(futures::stream::iter(chunks).boxed(), None).map(|r| r.unwrap()).collect().await;
        assert_eq!(out.concat(), [b"data: a\n\n".as_slice(), UNTERMINATED_USAGE].concat());
    }

    /// Produce a real `reqwest::Error` (there is no public constructor) by forcing a connection
    /// failure against an unroutable address.
    pub(crate) async fn make_reqwest_error() -> reqwest::Error {
        let _ = rustls::crypto::ring::default_provider().install_default();
        reqwest::Client::new()
            .get("http://127.0.0.1:1")
            .timeout(std::time::Duration::from_millis(50))
            .send()
            .await
            .expect_err("connection to a dead port fails")
    }
}
