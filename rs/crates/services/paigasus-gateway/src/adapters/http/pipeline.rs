// SPDX-License-Identifier: Apache-2.0

//! The generic proxy pipeline of a model route (SMA-769 D6, spec § 8.1; SMA-750 § 5).
//!
//! One [`Endpoint`] descriptor per route holds every step that is specific to the route: the
//! upstream, the request parse, the non-stream usage reader, the stream usage reader, the
//! terminal SSE frame and the interception limits. [`proxy`] is the flow that is common to all of them. It is the
//! `/v1/chat/completions` handler flow, moved here step for step.
//!
//! By the time [`proxy`] runs, the [`require_iam_auth`](super::auth::require_iam_auth)
//! middleware has authenticated and authorized the caller and attached a [`CallerContext`].
//! The security rules of the chat handler apply unchanged to every route:
//!
//! - **Egress hygiene (load-bearing):** only the raw body bytes go to the OpenAI client. The
//!   caller's inbound headers never do. The caller's `Authorization: Bearer <paigasus-key>` must
//!   never reach OpenAI. The client builds fresh upstream headers from its own real key. The
//!   Anthropic client also gets the [`Forward`] value of the handler: the raw query and the
//!   `anthropic-*` headers only (SMA-750 I2). The OpenAI routes pass an empty one.
//! - The upstream response is forwarded verbatim: a full body (non-stream) or an UNBUFFERED SSE
//!   stream. A non-2xx upstream (the provider's own error envelope) passes through unchanged,
//!   with the upstream `content-type` (`application/json` when the client reports none, which is
//!   always the case for OpenAI). The D8 headers of [`ResponseHead::apply`] go on every
//!   forwarded answer; the OpenAI client reports none.
//! - The mid-stream terminal error and the cancel-on-drop contract are described on
//!   [`terminal_sse_error_stream`](super::relay::terminal_sse_error_stream).

use std::sync::Arc;
use std::time::Instant;

use axum::body::Body;
use axum::http::header;
use axum::http::{HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use metrics::{counter, histogram};
use paigasus_gateway_core::{DecisionContext, Format, Limits, RequestEntry};
use paigasus_observability::names;

use super::AppState;
use super::bytes::EnvelopeBytes;
use super::error::GatewayError;
use super::interception::{self, ToolInterception};
use super::relay::terminal_sse_error_stream;
use super::usage::{BodyUsage, StreamUsage};
use crate::adapters::anthropic::{AnthropicClient, AnthropicRoute, Forward};
use crate::adapters::openai::{OpenAiClient, UpstreamRoute};
use crate::adapters::upstream::{ResponseHead, UpstreamError, UpstreamResponse};
use crate::application::tool_policy::OverheadSample;
use crate::domain::{CallerContext, Credential};

/// What the pipeline needs from a parsed request.
pub struct RequestFacts {
    /// The model name, for the request log.
    pub model: String,
    /// Whether the caller asked for a stream.
    pub stream: bool,
    /// The token estimate for the request side of the charge.
    pub request_estimate: u64,
}

/// The upstream of one endpoint: the provider and its route (SMA-750 § 5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Upstream {
    OpenAi(UpstreamRoute),
    Anthropic(AnthropicRoute),
}

/// The descriptor of one model route.
pub struct Endpoint {
    /// The endpoint name. `"chat_completions"` for chat. It is the `endpoint` log field and
    /// metric label.
    pub name: &'static str,
    /// The upstream this endpoint forwards to.
    pub upstream: Upstream,
    /// The wire format the interceptor reads.
    pub format: Format,
    /// Parse a COPY of the body. The second argument says whether interception is enabled.
    pub parse: fn(&[u8], bool) -> Result<RequestFacts, GatewayError>,
    /// The usage a non-stream 2xx body reports.
    pub non_stream_usage: fn(&[u8]) -> BodyUsage,
    /// A new usage reader for one stream.
    pub stream_usage: fn() -> StreamUsage,
    /// The terminal SSE frame for a mid-stream upstream failure.
    pub terminal: &'static str,
    /// The interception limits for this endpoint.
    pub limits: fn(&ToolInterception, &AppState) -> Limits,
}

/// Proxy one request through `endpoint`. See the module docs for the rules that bind it.
///
/// The body arrives as the house [`EnvelopeBytes`] that the handler extracted, and is unwrapped
/// here. The wrapped bytes are the ORIGINAL request bytes, and they go upstream unchanged (D5).
/// `fwd` goes to the Anthropic client only.
pub(crate) async fn proxy(endpoint: &'static Endpoint, state: AppState, caller: CallerContext, fwd: Forward, body: EnvelopeBytes) -> Response {
    let EnvelopeBytes(body) = body;
    // Parse a COPY only to read the facts; the ORIGINAL `body` bytes flow upstream verbatim.
    // The parse function splits its failures (SMA-588): a body that PARSED but did not match the
    // type is reported distinctly from one that could not be read at all.
    let RequestFacts { model, stream, request_estimate } = match (endpoint.parse)(&body, state.tool_interception.is_some()) {
        Ok(facts) => facts,
        Err(e) => return e.into_response(),
    };

    // SMA-505 D9: streaming is a request PARAMETER, not a route, so a disabled capability is
    // enforced here rather than by unmounting. Checked before any egress call, so a refused
    // request never reaches the upstream or its rate limit.
    if stream && !state.capabilities.chat_stream {
        return GatewayError::StreamingDisabled.into_response();
    }

    // SMA-750 § 5: the client of the endpoint's upstream. An Anthropic endpoint with no Anthropic
    // client is an internal fault (boot validation makes it unreachable), refused before the
    // limits so it uses no quota.
    let client = match (endpoint.upstream, &state.anthropic) {
        (Upstream::OpenAi(route), _) => Client::OpenAi(Arc::clone(&state.openai), route),
        (Upstream::Anthropic(route), Some(anthropic)) => Client::Anthropic(Arc::clone(anthropic), route),
        (Upstream::Anthropic(_), None) => return GatewayError::Internal.into_response(),
    };

    // SMA-749: capture what the decider needs while the request ids still exist (SMA-504 § 4.3:
    // inside the stream they are `None`). `None` when interception and the tool policy are both
    // off: nothing is parsed or constructed (D27).
    //
    // SMA-752 D17, D19, § 6.5.0: this runs BEFORE the limits, so a refused request never uses
    // quota. The turn is parsed from exactly the bytes that `apply` plans from and rewrites, and
    // `body` is then `outcome.body`: nothing after this line changes the tools, the tool choice or
    // the tool results before the upstream send.
    let mut body = body;
    let mut request_entries: Vec<RequestEntry> = Vec::new();
    // Preflight F14: one overhead sample per request, recorded when this drops.
    let mut overhead: Option<OverheadSample> = None;
    // SMA-753 D4: the history write waits for the limits admission, so a refused request writes nothing.
    let mut pending_history = None;
    let prepared = if state.tool_interception.is_some() || state.tool_policy.is_some() {
        let turn = paigasus_gateway_core::parse_request(endpoint.format, &body);
        let request_id = paigasus_observability::current_ids().map(|ids| ids.request_id.to_string());
        let api_key_id = match &caller.credential {
            Credential::ApiKey { key_id } => Some(key_id.clone()),
            _ => None,
        };
        let mut ctx = DecisionContext::new(caller.principal_prn.clone(), caller.scope_prn.clone(), request_id).with_request(api_key_id, paigasus_gateway_core::ingress(endpoint.format), model.clone());
        if let Some(tp) = &state.tool_policy {
            match tp.apply(&ctx, &model, endpoint.format, &turn, body).await {
                Ok(outcome) => {
                    body = outcome.body;
                    request_entries = outcome.entries;
                    overhead = Some(OverheadSample::new(Arc::clone(&outcome.snapshot)));
                    // D30: the decider uses this snapshot; without it, every call fails closed.
                    ctx = ctx.with_policy(outcome.snapshot);
                    if let Some(session) = outcome.session {
                        ctx = ctx.with_session(session);
                    }
                    pending_history = outcome.history;
                }
                Err(refused) => {
                    // The refusal metric is the operator's signal; this line stays at debug.
                    tracing::debug!(principal = %caller.principal_prn, scope = %caller.scope_prn, reason = ?refused.refusal, endpoint = endpoint.name, "request refused by the tool policy");
                    let mut resp = GatewayError::from(refused.refusal).into_response();
                    add_request_report(&mut resp, &refused.entries);
                    return resp;
                }
            }
        }
        Some((Arc::new(ctx), Arc::new(turn)))
    } else {
        None
    };
    let interception = state.tool_interception.clone().zip(prepared).map(|(ti, (ctx, turn))| {
        let limits = (endpoint.limits)(&ti, &state);
        (ti, limits, ctx, turn)
    });

    // SMA-677 D9: the limits run after the body and streaming checks and just before egress, so a
    // refused body never uses quota. `None` (no `[limits]` table) skips them entirely (A6).
    let mut guard = match &state.limits {
        None => None,
        Some(limits) => match limits.admit_for(&caller, endpoint.name).await {
            Ok(guard) => Some(guard),
            Err(refusal) => {
                // The refusal metric is the operator's signal; this line stays at debug.
                tracing::debug!(principal = %caller.principal_prn, scope = %caller.scope_prn, reason = refusal.label(), endpoint = endpoint.name, "chat completion refused by a limit");
                // SMA-752 § 6.5.3: the header goes out whenever an entry exists, a limit refusal too.
                let mut resp = GatewayError::from(refusal).into_response();
                add_request_report(&mut resp, &request_entries);
                return resp;
            }
        },
    };
    // SMA-753 D4: the request is admitted (or has no `[limits]`), so the history write runs now.
    // Its time goes into this request's overhead sample: `apply` always sets the sample when it
    // returns a pending write.
    if let (Some(tp), Some(pending), Some(sample)) = (&state.tool_policy, pending_history.take(), overhead.as_ref()) {
        tp.commit_history(pending, sample.snapshot()).await;
    }
    if let Some(guard) = guard.as_mut() {
        guard.set_request_estimate(request_estimate);
        // The ids exist here; inside the stream `current_ids()` is `None` (SMA-504 § 4.3).
        guard.set_ids(paigasus_observability::current_ids());
        guard.set_endpoint(endpoint.name);
        guard.mark_sent();
    }

    let started = Instant::now();
    // EGRESS HYGIENE: only the raw body bytes cross the boundary — never the caller's headers.
    let result = match &client {
        Client::OpenAi(openai, route) => openai.send(*route, body, stream).await,
        Client::Anthropic(anthropic, route) => anthropic.send(*route, Method::POST, body, stream, &fwd).await,
    };

    let (response, status) = match result {
        Ok(UpstreamResponse::Full { status, head, body: upstream_body }) => {
            // Non-stream: the upstream head AND body have both landed, so this is a genuine
            // full-request latency, not just TTFB (see the NOTE on the stream arm below).
            record_upstream_call(status, started, endpoint.name);
            // A4: a 2xx reads a COPY of the body for its usage; a non-2xx charges nothing.
            if let Some(guard) = guard.as_mut() {
                if status.is_success() {
                    match (endpoint.non_stream_usage)(&upstream_body) {
                        BodyUsage::Reported(total) => guard.set_reported(total),
                        BodyUsage::Completion(tokens) => guard.set_completion_estimate(tokens),
                    }
                } else {
                    guard.mark_no_charge();
                }
            }
            // SMA-749 § 5.2: the guard read the ORIGINAL body above (D24); interception runs after.
            let outcome = match &interception {
                Some((ti, limits, ctx, turn)) if status.is_success() => {
                    interception::intercept_body(ti, *limits, endpoint.format, Arc::clone(ctx), Arc::clone(turn), &upstream_body, &request_entries).await
                }
                // § 6.5.3: the request entries also go out on a non-2xx answer.
                _ => interception::BodyOutcome::Forward {
                    rewritten: None,
                    report: interception::request_report(&request_entries),
                },
            };
            match outcome {
                interception::BodyOutcome::Refuse => {
                    let mut resp = GatewayError::ToolInterceptionMalformedResponse.into_response();
                    add_request_report(&mut resp, &request_entries);
                    let status = resp.status();
                    (resp, status)
                }
                interception::BodyOutcome::Forward { rewritten, report } => {
                    let out = rewritten.unwrap_or(upstream_body);
                    // Forward the upstream status + body VERBATIM (or rewritten by a block),
                    // including a non-2xx provider error envelope with its own content type.
                    let content_type = if status.is_success() {
                        HeaderValue::from_static("application/json")
                    } else {
                        error_content_type(&head)
                    };
                    let mut resp = (status, [(header::CONTENT_TYPE, content_type)], out).into_response();
                    head.apply(resp.headers_mut());
                    if let Some(r) = report
                        && let Ok(v) = axum::http::HeaderValue::from_str(&r)
                    {
                        resp.headers_mut().insert("paigasus-tool-decisions", v);
                    }
                    (resp, status)
                }
            }
        }
        Ok(UpstreamResponse::Stream { status, head, stream: upstream }) => {
            // NOTE: for `stream:true` this records TTFB (time-to-first-byte / response head), NOT
            // stream completion — the call is considered "done" for metrics purposes the moment
            // the upstream response head arrives. A mid-stream SSE failure (handled below by the
            // terminal-error adapter, once the 200 head is already committed) is NOT re-counted
            // here — it has no separate HTTP status to attribute.
            record_upstream_call(status, started, endpoint.name);
            let resp = if status.is_success() {
                // Success stream: SSE passthrough with the mid-stream terminal-error adapter.
                //
                // SMA-504 spec §4.3, situation 3: `current_ids() == None` from here on. The
                // `CorrelationLayer`'s task-local scope covers the request-HEAD future only, and
                // that future already resolved to this `Response<Body>` the moment this line
                // runs — hyper polls the `Body::from_stream` future (and therefore
                // `terminal_sse_error_stream` below) AFTERWARDS, outside the scope. Anything
                // added inside the stream adapter — a log line, a metric — will see no ids.
                let usage = (endpoint.stream_usage)();
                let out = match interception {
                    // SMA-749: the interceptor owns the terminal frame from here on (D17).
                    // SMA-752 § 6.5.3: the request entries go out before the first upstream frame,
                    // and the overhead sample lives as long as the stream (preflight F14).
                    Some((ti, limits, ctx, turn)) => Body::from_stream(interception::keep_until_end(
                        interception::intercepted_stream(
                            endpoint.format,
                            limits,
                            upstream,
                            guard.take(),
                            usage,
                            ti,
                            ctx,
                            turn,
                            endpoint.terminal.as_bytes(),
                            paigasus_gateway_core::request_comments(&request_entries),
                        ),
                        overhead.take(),
                    )),
                    None => Body::from_stream(terminal_sse_error_stream(upstream, guard.take(), usage, endpoint.terminal)),
                };
                let mut resp = (status, [(header::CONTENT_TYPE, "text/event-stream")], out).into_response();
                head.apply(resp.headers_mut());
                resp
            } else {
                // Non-2xx `stream:true` request: the upstream answers with a JSON error body (NOT
                // SSE), so forward it as an error passthrough — no SSE terminal-error wrapping applies.
                if let Some(guard) = guard.as_mut() {
                    guard.mark_no_charge();
                }
                let mut resp = (status, [(header::CONTENT_TYPE, error_content_type(&head))], Body::from_stream(upstream)).into_response();
                head.apply(resp.headers_mut());
                add_request_report(&mut resp, &request_entries);
                resp
            };
            (resp, status)
        }
        Err(err) => {
            // Connect/transport/build → 502; timeout → 504 (see `GatewayError::from`). No upstream
            // HTTP status exists in this case at all, so record under the MAPPED gateway status
            // instead (still a `status_class` derived from an HTTP status, per the global
            // constraints — never a free-form error string).
            // A4: a connect failure never reached the upstream; any other failure after the send
            // (timeout, transport) keeps the request estimate.
            if matches!(err, UpstreamError::Connect { .. })
                && let Some(guard) = guard.as_mut()
            {
                guard.mark_no_charge();
            }
            let mut resp = GatewayError::from(err).into_response();
            add_request_report(&mut resp, &request_entries);
            let status = resp.status();
            record_upstream_call(status, started, endpoint.name);
            (resp, status)
        }
    };

    // One structured line per request — model/stream/status/latency/principal/scope/credential
    // ONLY. NEVER the prompt, messages, body, or the OpenAI key. `principal` is a service account
    // for an API key and a USER for an OIDC token (SMA-635): a principal PRN is an opaque id, not
    // the user's name or e-mail, and it is kept deliberately for attribution and audit. The "never
    // PII" bar is about prompt/message content. `key_id` is logged only for an API key.
    // SMA-750 § 9: `anthropic_beta` is there only when the caller sent an `anthropic-beta` value
    // (a `None` field is not written), so the OpenAI lines do not change.
    let anthropic_beta = fwd.beta_values();
    match &caller.credential {
        Credential::ApiKey { key_id } => tracing::info!(
            model = %model,
            stream = stream,
            status = status.as_u16(),
            latency_ms = started.elapsed().as_millis() as u64,
            principal = %caller.principal_prn,
            scope = %caller.scope_prn,
            auth = "api_key",
            key_id = %key_id,
            endpoint = endpoint.name,
            anthropic_beta = anthropic_beta.as_deref(),
            "chat completion proxied"
        ),
        Credential::Oidc => tracing::info!(
            model = %model,
            stream = stream,
            status = status.as_u16(),
            latency_ms = started.elapsed().as_millis() as u64,
            principal = %caller.principal_prn,
            scope = %caller.scope_prn,
            auth = "oidc",
            endpoint = endpoint.name,
            anthropic_beta = anthropic_beta.as_deref(),
            "chat completion proxied"
        ),
    }

    // Preflight F14: a full answer (or a stream with no interceptor) records its sample here.
    drop(overhead);
    response
}

/// The client of one request's upstream, resolved before the limits.
enum Client {
    OpenAi(Arc<OpenAiClient>, UpstreamRoute),
    Anthropic(Arc<AnthropicClient>, AnthropicRoute),
}

/// The `content-type` of a forwarded non-2xx answer: the upstream's own, or `application/json`
/// when the client reports none (the OpenAI client never does, so its routes do not change).
fn error_content_type(head: &ResponseHead) -> HeaderValue {
    head.content_type.clone().unwrap_or_else(|| HeaderValue::from_static("application/json"))
}

/// SMA-752 § 6.5.3: put the request entries into the `paigasus-tool-decisions` header of an
/// answer that carries no call entries. No entries, no header (as before).
fn add_request_report(resp: &mut Response, requests: &[RequestEntry]) {
    if let Some(r) = interception::request_report(requests)
        && let Ok(v) = axum::http::HeaderValue::from_str(&r)
    {
        resp.headers_mut().insert("paigasus-tool-decisions", v);
    }
}

/// Record `gateway_upstream_requests_total{status_class, endpoint}` + `gateway_upstream_request_duration_seconds`
/// for one upstream call. `status` is either the upstream's OWN status (a completed response,
/// full or streamed) or, on a transport/connect/timeout failure, the GATEWAY status the error
/// was mapped to (502/504 — see `GatewayError::from<UpstreamError>`), since no upstream status
/// exists in that case. See the call sites above for the TTFB-vs-stream-vs-full timing note.
pub(crate) fn record_upstream_call(status: StatusCode, started: Instant, endpoint: &'static str) {
    let status_class = format!("{}xx", status.as_u16() / 100);
    counter!(names::GATEWAY_UPSTREAM_REQUESTS_TOTAL, "status_class" => status_class, "endpoint" => endpoint).increment(1);
    histogram!(names::GATEWAY_UPSTREAM_REQUEST_DURATION_SECONDS).record(started.elapsed().as_secs_f64());
}
