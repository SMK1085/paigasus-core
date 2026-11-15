// SPDX-License-Identifier: Apache-2.0

//! The `POST /v1/chat/completions` proxy handler — the integration crux of the M0 slice.
//!
//! By the time this runs, the G5 [`require_iam_auth`](super::auth::require_iam_auth) middleware has
//! already authenticated + authorized the caller and attached a [`CallerContext`] to the request
//! extensions. This handler's job is narrow and security-sensitive:
//!
//! 1. Read the full request body as raw [`EnvelopeBytes`] — the body-size limit is enforced by
//!    the [`DefaultBodyLimit`](axum::extract::DefaultBodyLimit) layer, which `EnvelopeBytes`
//!    honours and reports as a `413` INSIDE the OpenAI envelope (SMA-588); before that it was
//!    axum's plain text, outside the error contract.
//! 2. Parse a COPY of the body into [`ChatCompletionRequest`] ONLY to read `model` (logging) and
//!    `stream` (which egress path to take). A JSON parse failure → `400`.
//! 3. **Egress hygiene (load-bearing):** forward ONLY the raw body bytes to the OpenAI client —
//!    NEVER the caller's inbound headers. The caller's `Authorization: Bearer <paigasus-key>` is
//!    still on the inbound request; it must never reach OpenAI. The client builds fresh upstream
//!    headers from its own real key, so simply never handing it caller headers guarantees this.
//! 4. Forward the upstream response verbatim: a full body (non-stream) or an UNBUFFERED SSE stream.
//!    A non-2xx upstream (OpenAI's own error envelope) passes through unchanged.
//!
//! ## Mid-stream terminal SSE error (spec §5)
//! Once a stream's `200 OK` head has been sent and `data:` frames are flowing, the HTTP status can
//! no longer change — so a mid-stream upstream failure cannot become a `502`. Instead the stream is
//! adapted so that on the FIRST inner error it emits one terminal
//! `data: {"error":…}` SSE event and then ends. The adapted stream is returned DIRECTLY (never
//! spawned onto a detached task) so reqwest's cancel-on-drop stays intact: if the downstream client
//! disconnects, axum drops the response body, which drops the inner reqwest stream and cancels the
//! upstream request (G8 proves this abort).

use std::convert::Infallible;

use axum::Extension;
use axum::body::Bytes;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use futures::{Stream, StreamExt};
use paigasus_gateway_core::Format;

use super::AppState;
use super::error::GatewayError;
use super::pipeline::{self, Endpoint, RequestFacts};
use crate::adapters::http::bytes::EnvelopeBytes;
use crate::adapters::http::dto::ChatCompletionRequest;
use crate::adapters::http::usage::{self, StreamUsage, UsageScanner};
use crate::adapters::openai::{OpenAiByteStream, UpstreamRoute};
use crate::application::charge_guard::ChargeGuard;
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
/// [`EnvelopeBytes`] wraps [`Bytes`], so it honours the
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
pub(crate) async fn chat_completions(State(state): State<AppState>, caller: Option<Extension<CallerContext>>, EnvelopeBytes(body): EnvelopeBytes) -> Response {
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

/// The small state machine [`terminal_sse_error_stream`] unfolds over: still forwarding upstream
/// chunks, or terminated (after emitting the terminal error event, or after the upstream ended).
enum StreamState {
    Streaming(OpenAiByteStream),
    Done,
}

/// The unfold state. The charge guard is a FIELD here, not a `Drop` on `StreamState`: the closure
/// destructures the state by move, which a type with `Drop` forbids (E0509). When the stream ends,
/// fails, or is dropped, this value drops once, and the guard with it (SMA-677 § 4.6). The clean
/// end and the first error call `UsageScanner::finish()` first; a drop (client disconnect) does
/// not, so a last record with no blank line counts only when the upstream ended (SMA-730 D3).
struct Relay {
    phase: StreamState,
    scan: StreamUsage,
    guard: Option<ChargeGuard>,
}

/// One upstream chunk, read for billing before anything else sees it (SMA-677, SMA-749 D24).
/// Without a guard (no `[limits]`) nothing is parsed at all. Shared with the intercepting relay.
pub(crate) fn feed_scan(scan: &mut StreamUsage, guard: &mut Option<ChargeGuard>, chunk: &[u8]) {
    if let Some(guard) = guard.as_mut() {
        scan.feed(chunk);
        guard.set_stream_progress(scan.completion_records(), scan.reported_total());
    }
}

/// SMA-730: the upstream ended or failed. A last record with no blank line after it still
/// counts. Without a guard the scanner reads nothing, as in the chunk arm. Shared with the
/// intercepting relay (SMA-749 D24).
pub(crate) fn finish_scan(scan: &mut StreamUsage, guard: &mut Option<ChargeGuard>) {
    if let Some(guard) = guard.as_mut() {
        scan.finish();
        guard.set_stream_progress(scan.completion_records(), scan.reported_total());
    }
}

/// Adapt an [`OpenAiByteStream`] into a body stream that forwards each upstream chunk UNBUFFERED
/// and, on the FIRST upstream error, emits one terminal [`TERMINAL_SSE_ERROR`] event and ends.
///
/// Items are `Result<Bytes, Infallible>`: the outgoing stream never errors (a mid-stream upstream
/// failure becomes a data event, not a transport error), which keeps `Body::from_stream` from
/// aborting the response body. The inner stream is owned by the unfold state and dropped when this
/// stream is dropped — preserving reqwest's cancel-on-drop of the upstream request.
///
/// Runs with `paigasus_observability::current_ids() == None` (SMA-504 spec §4.3, situation 3):
/// this function is polled by hyper after the request-head future — the one the
/// `CorrelationLayer` task-local scope covers — has already resolved. See the call site's
/// comment in `chat_completions`.
pub(crate) fn terminal_sse_error_stream(
    inner: OpenAiByteStream,
    guard: Option<ChargeGuard>,
    usage: StreamUsage,
    terminal: &'static str,
) -> impl Stream<Item = Result<Bytes, Infallible>> + Send + 'static {
    let start = Relay {
        phase: StreamState::Streaming(inner),
        scan: usage,
        guard,
    };
    futures::stream::unfold(start, move |Relay { phase, mut scan, mut guard }| async move {
        match phase {
            StreamState::Streaming(mut inner) => match inner.next().await {
                // Forward each chunk exactly as it arrived (unbuffered). The scanner reads a copy,
                // and only when a guard exists (no `[limits]`: no parsing at all).
                Some(Ok(chunk)) => {
                    feed_scan(&mut scan, &mut guard, &chunk);
                    Some((
                        Ok(chunk),
                        Relay {
                            phase: StreamState::Streaming(inner),
                            scan,
                            guard,
                        },
                    ))
                }
                // First upstream error: count the last record (the frame's leading blank line
                // closes it for the client too), emit the terminal event, then end.
                Some(Err(_)) => {
                    finish_scan(&mut scan, &mut guard);
                    Some((
                        Ok(Bytes::from_static(terminal.as_bytes())),
                        Relay {
                            phase: StreamState::Done,
                            scan,
                            guard,
                        },
                    ))
                }
                // Clean upstream end: count the last record, then drop the relay and the guard.
                None => {
                    finish_scan(&mut scan, &mut guard);
                    None
                }
            },
            StreamState::Done => None,
        }
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// The plain relay as the pipeline builds it for chat (no interception).
    fn plain_relay(inner: OpenAiByteStream, guard: Option<ChargeGuard>) -> impl Stream<Item = Result<Bytes, Infallible>> + Send + 'static {
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
        reqwest::Client::new()
            .get("http://127.0.0.1:1")
            .timeout(std::time::Duration::from_millis(50))
            .send()
            .await
            .expect_err("connection to a dead port fails")
    }
}
