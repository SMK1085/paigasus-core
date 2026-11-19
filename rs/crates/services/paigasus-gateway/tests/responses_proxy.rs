// SPDX-License-Identifier: Apache-2.0
//! SMA-769: `POST /v1/responses` through the real router, the auth middleware, the limits and the
//! interceptor, against the in-process mock upstream (spec § 10.6, AC 1–3, AC 10).

mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use futures::StreamExt;
use paigasus_gateway::adapters::http::interception::ToolInterception;
use paigasus_gateway::adapters::http::responses::RESPONSES_TERMINAL_SSE_ERROR;
use paigasus_gateway::adapters::http::{AppState, ResponsesSettings, router};
use paigasus_gateway::application::limits::Limits;
use paigasus_gateway::service_info::Capabilities;
use paigasus_gateway_core::{AllowAll, BlockCode, DecideFuture, Decision, DecisionContext, Limits as CoreLimits, OnFailure, ToolCall, ToolCallDecider, ToolTurn};
use serde_json::{Value, json};
use support::MockOpenAi;
use support::limits::{CALLER_KEY, ORG_A_PRN, PRINCIPAL_1, RecordingStore, ScopedIam, Sent, limits, openai, rules};
use tower::ServiceExt;

const MIB: usize = 1 << 20;
const NON_STREAM: &str = r#"{"model":"gpt-6","input":"hi"}"#;
const STREAM: &str = r#"{"model":"gpt-6","input":"hi","stream":true}"#;
/// A minimal valid non-stream answer, with no calls.
const OK_BODY: &str = r#"{"id":"resp_1","object":"response","status":"completed","output":[],"usage":{"total_tokens":2}}"#;

/// The `[responses]` values of the spec (D15).
fn settings() -> ResponsesSettings {
    ResponsesSettings {
        max_request_bytes: 16 * MIB,
        max_record_bytes: 16 * MIB,
        max_held_bytes: 32 * MIB,
    }
}

/// Blocks every call whose name starts with `blocked_`.
struct BlockNamed;

impl ToolCallDecider for BlockNamed {
    fn decide(&self, _ctx: Arc<DecisionContext>, _turn: Arc<ToolTurn>, calls: Vec<ToolCall>) -> DecideFuture {
        let d = calls
            .iter()
            .map(|c| {
                if c.name.starts_with("blocked_") {
                    Decision::Block { code: BlockCode::Policy }
                } else {
                    Decision::Allow
                }
            })
            .collect();
        Box::pin(std::future::ready(Ok(d)))
    }
}

fn ti(decider: Arc<dyn ToolCallDecider>) -> Option<Arc<ToolInterception>> {
    ti_with(OnFailure::Block, decider)
}

fn ti_with(on_failure: OnFailure, decider: Arc<dyn ToolCallDecider>) -> Option<Arc<ToolInterception>> {
    Some(Arc::new(ToolInterception {
        limits: CoreLimits { on_failure, ..CoreLimits::defaults() },
        decider,
    }))
}

/// The options of one test router.
struct Opts {
    responses: Option<ResponsesSettings>,
    limits: Option<Arc<Limits>>,
    ti: Option<Arc<ToolInterception>>,
    chat_stream: bool,
}

impl Default for Opts {
    fn default() -> Self {
        Opts {
            responses: Some(settings()),
            limits: None,
            ti: None,
            chat_stream: true,
        }
    }
}

fn app(base_url: &str, o: Opts) -> Router {
    router(AppState {
        iam: ScopedIam::arc(PRINCIPAL_1, ORG_A_PRN),
        openai: Arc::new(openai(base_url, Duration::from_secs(30))),
        max_request_bytes: MIB,
        capabilities: Capabilities { chat_stream: o.chat_stream },
        limits: o.limits,
        dpop_enabled: false,
        tool_interception: o.ti,
        tool_policy: None,
        responses: o.responses.map(Arc::new),
    })
}

fn request(path: &str, body: impl Into<Body>, bearer: bool) -> Request<Body> {
    let mut b = Request::builder().method("POST").uri(path).header(header::CONTENT_TYPE, "application/json");
    if bearer {
        b = b.header(header::AUTHORIZATION, format!("Bearer {CALLER_KEY}"));
    }
    b.body(body.into()).expect("build request")
}

async fn read(app: &Router, req: Request<Body>) -> Sent {
    let resp = app.clone().oneshot(req).await.expect("the router answers");
    let status = resp.status();
    let headers = resp.headers().clone();
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.expect("the body reads");
    Sent { status, headers, body }
}

async fn respond(app: &Router, body: &str) -> Sent {
    read(app, request("/v1/responses", body.to_owned(), true)).await
}

/// A Responses SSE stream, built event by event with increasing `sequence_number`s.
#[derive(Default)]
struct Events {
    seq: u64,
    out: Vec<String>,
}

impl Events {
    fn push(&mut self, mut v: Value) {
        v["sequence_number"] = self.seq.into();
        self.seq += 1;
        let ty = v["type"].as_str().expect("a type").to_owned();
        self.out.push(format!("event: {ty}\ndata: {v}\n\n"));
    }

    fn created() -> Self {
        let mut e = Events::default();
        let response = json!({"id":"resp_1","object":"response","status":"in_progress","output":[]});
        e.push(json!({"type":"response.created","response":response}));
        e.push(json!({"type":"response.in_progress","response":response}));
        e
    }

    /// One `function_call` item at `index`, with one argument delta. Returns the done item.
    fn function_call(&mut self, index: u32, name: &str, args: &str) -> Value {
        let (id, call_id) = (format!("fc_{index}"), format!("call_{index}"));
        let added = json!({"type":"function_call","id":id,"status":"in_progress","call_id":call_id,"name":name,"arguments":""});
        self.push(json!({"type":"response.output_item.added","item":added,"output_index":index}));
        self.push(json!({"type":"response.function_call_arguments.delta","delta":args,"item_id":id,"output_index":index}));
        self.push(json!({"type":"response.function_call_arguments.done","arguments":args,"item_id":id,"output_index":index}));
        let done = json!({"type":"function_call","id":id,"status":"completed","call_id":call_id,"name":name,"arguments":args});
        self.push(json!({"type":"response.output_item.done","item":done,"output_index":index}));
        done
    }

    /// The terminal event, with `pad` bytes of `instructions` to make it large.
    fn completed(mut self, output: Vec<Value>, total: u64, pad: usize) -> Vec<String> {
        let response = json!({"id":"resp_1","object":"response","status":"completed","instructions":"x".repeat(pad),"output":output,
            "usage":{"input_tokens":3,"output_tokens":5,"total_tokens":total}});
        self.push(json!({"type":"response.completed","response":response}));
        self.out
    }
}

/// A stream with an allowed `read_file` call at 0 and a `blocked_rm` call at 1 that carries the
/// canary, then the terminal event with `total` tokens and `pad` bytes of padding.
fn blocked_stream(total: u64, pad: usize) -> Vec<String> {
    let mut e = Events::created();
    let a = e.function_call(0, "read_file", "{\"p\":1}");
    let b = e.function_call(1, "blocked_rm", "{\"path\":\"CANARY\"}");
    e.completed(vec![a, b], total, pad)
}

fn body_with_calls(second_status: &str) -> String {
    json!({"id":"resp_1","object":"response","status":"completed","output":[
        {"type":"message","id":"msg_1","status":"completed","role":"assistant","content":[{"type":"output_text","text":"Sure.","annotations":[]}]},
        {"type":"function_call","id":"fc_1","call_id":"c1","name":"blocked_rm","arguments":"{\"p\":\"CANARY\"}","status":second_status},
        {"type":"function_call","id":"fc_2","call_id":"c2","name":"read_file","arguments":"{}","status":"completed"}
    ],"usage":{"input_tokens":2,"output_tokens":3,"total_tokens":13}})
    .to_string()
}

fn decisions(sent: &Sent) -> Vec<Value> {
    serde_json::from_str(sent.header("paigasus-tool-decisions")).expect("an ASCII JSON header")
}

/// D8 (C7): with `[responses] enabled = false` the route is a 404, as an unknown path is, and the
/// chat route still answers.
#[tokio::test]
async fn the_route_is_404_when_disabled() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, "{}").await;
    let off = app(&mock.base_url, Opts { responses: None, ..Opts::default() });
    assert_eq!(respond(&off, NON_STREAM).await.status, StatusCode::NOT_FOUND);
    assert_eq!(read(&off, request("/v1/responses", NON_STREAM, false)).await.status, StatusCode::NOT_FOUND, "no credential challenge");
    assert_eq!(mock.request_count(), 0);
    let chat = read(&off, request("/v1/chat/completions", r#"{"model":"m","messages":[]}"#, true)).await;
    assert_eq!(chat.status, StatusCode::OK);
    let on = app(&mock.base_url, Opts::default());
    assert_eq!(respond(&on, NON_STREAM).await.status, StatusCode::OK, "the same request is served when the route is on");
}

#[tokio::test]
async fn the_route_needs_a_credential() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, "{}").await;
    let sent = read(&app(&mock.base_url, Opts::default()), request("/v1/responses", NON_STREAM, false)).await;
    assert_eq!(sent.status, StatusCode::UNAUTHORIZED);
    assert_eq!(mock.request_count(), 0);
}

/// D5 and D7: the upstream path is `/v1/responses`, the body bytes are unchanged (no `store`
/// rewrite), and the caller's credential never goes upstream.
#[tokio::test]
async fn the_request_goes_to_the_responses_path() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, OK_BODY).await;
    let raw = "{ \"model\":\"gpt-6\",\n  \"input\":[{\"role\":\"user\",\"content\":\"hi\"}], \"store\":true, \"x-unknown\":1.50 }";
    for ti in [None, ti(Arc::new(AllowAll))] {
        let sent = respond(&app(&mock.base_url, Opts { ti, ..Opts::default() }), raw).await;
        assert_eq!(sent.status, StatusCode::OK);
        let rec = mock.recorded().expect("the upstream got the request");
        assert_eq!(rec.path, "/v1/responses");
        assert_eq!(rec.body.as_ref(), raw.as_bytes(), "D5: the body goes upstream byte-identical");
        assert_eq!(rec.header("authorization"), Some("Bearer sk-real-openai-key"));
        assert!(rec.headers.values().all(|v| !v.to_str().unwrap_or_default().contains(CALLER_KEY)), "the caller key never goes upstream");
    }
}

/// D15: the route's body limit is `[responses] max_request_bytes`, not the chat limit (1 MiB in
/// these tests), and an over-limit body is a 413 in the OpenAI envelope.
///
/// The mock upstream keeps axum's 2 MiB default, so a passing body is at most 2 MiB. The second
/// router's cap (1.5 MiB) is under that default: without the route's limit layer, the body of
/// `cap + 1` would pass under axum's default, so the 413 row fails.
#[tokio::test]
async fn the_body_limit_is_the_responses_limit() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, "{}").await;
    let body = |len: usize| {
        let head = r#"{"model":"gpt-6","input":""#;
        format!("{head}{}\"}}", "x".repeat(len - head.len() - 2))
    };
    let two = body(2 * MIB);
    assert_eq!(two.len(), 2 * MIB);
    let spec = app(&mock.base_url, Opts::default());
    assert_eq!(respond(&spec, &two).await.status, StatusCode::OK, "2 MiB is over the chat limit and under the responses limit");
    let cap = 3 * MIB / 2;
    let app = app(
        &mock.base_url,
        Opts {
            responses: Some(ResponsesSettings { max_request_bytes: cap, ..settings() }),
            ..Opts::default()
        },
    );
    assert_eq!(respond(&app, &body(cap)).await.status, StatusCode::OK, "a body of exactly the cap passes");
    let over = respond(&app, &body(cap + 1)).await;
    assert_eq!(over.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(over.json()["error"]["code"], "request-too-large");
    assert_eq!(over.json()["error"]["type"], "invalid_request_error");
    assert_eq!(mock.request_count(), 2, "the over-limit body never went upstream");
}

#[tokio::test]
async fn rate_limit_and_budget_refuse_with_429() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, body_with_calls("completed")).await;
    let rate = app(
        &mock.base_url,
        Opts {
            limits: Some(limits(rules(Some(1), None, None), RecordingStore::new())),
            ..Opts::default()
        },
    );
    assert_eq!(respond(&rate, NON_STREAM).await.status, StatusCode::OK);
    let refused = respond(&rate, NON_STREAM).await;
    assert_eq!(refused.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(refused.json()["error"]["code"], "rate-limited");
    let budget = app(
        &mock.base_url,
        Opts {
            limits: Some(limits(rules(None, None, Some(1)), RecordingStore::new())),
            ..Opts::default()
        },
    );
    assert_eq!(respond(&budget, NON_STREAM).await.status, StatusCode::OK);
    let refused = respond(&budget, NON_STREAM).await;
    assert_eq!(refused.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(refused.json()["error"]["code"], "budget-exhausted");
    assert_eq!(mock.request_count(), 2, "a refused request makes no upstream call");
}

/// D3, D4, AC 2 and Review Focus 2 (C6) at the HTTP level: with interception on, each stateful
/// field is a 400 that names it, before admission (the limit of one is not used). `null` values
/// and `background: false` pass.
#[tokio::test]
async fn stateful_requests_are_refused_before_admission() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, OK_BODY).await;
    let store = RecordingStore::new();
    let app = app(
        &mock.base_url,
        Opts {
            limits: Some(limits(rules(Some(1), None, None), store.clone())),
            ti: ti(Arc::new(AllowAll)),
            ..Opts::default()
        },
    );
    let cases = [
        (r#"{"model":"gpt-6","input":"hi","background":true}"#, "background"),
        (r#"{"model":"gpt-6","input":"hi","previous_response_id":"resp_1"}"#, "previous_response_id"),
        (r#"{"model":"gpt-6","input":"hi","conversation":{"id":"conv_1"}}"#, "conversation"),
        (r#"{"model":"gpt-6","input":"hi","prompt":{"id":"pmpt_1"}}"#, "prompt"),
        (r#"{"model":"gpt-6","input":[{"type":"item_reference","id":"msg_1"}]}"#, "input"),
        (r#"{"model":"gpt-6","input":[{"id":"msg_1"}]}"#, "input"),
    ];
    for (body, param) in cases {
        let sent = respond(&app, body).await;
        assert_eq!(sent.status, StatusCode::BAD_REQUEST, "{body}");
        let err = &sent.json()["error"];
        assert_eq!(err["code"], "responses-stateful-not-supported", "{body}");
        assert_eq!(err["param"], param, "{body}");
        assert_eq!(err["type"], "invalid_request_error", "{body}");
        assert_eq!(err["message"], "Stateful Responses API requests are not supported by this gateway.");
    }
    assert_eq!(store.checks(), 0, "no refusal reached the limits");
    assert_eq!(mock.request_count(), 0);
    let nulls = r#"{"model":"gpt-6","input":"hi","background":false,"previous_response_id":null,"conversation":null,"prompt":null}"#;
    assert_eq!(respond(&app, nulls).await.status, StatusCode::OK, "null values pass, and the one slot is still free");
    assert_eq!(respond(&app, NON_STREAM).await.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(mock.request_count(), 1);
}

/// Coordinator ruling (fix round 1): a duplicate key must not decide D3 or D4. With interception
/// on, a duplicate key at any depth is a 400 before admission; with interception off, only a
/// duplicate `background` is refused (D4). The same body without the duplicate passes.
#[tokio::test]
async fn duplicate_keys_are_refused_before_admission() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, OK_BODY).await;
    let store = RecordingStore::new();
    let on = app(
        &mock.base_url,
        Opts {
            limits: Some(limits(rules(Some(1), None, None), store.clone())),
            ti: ti(Arc::new(AllowAll)),
            ..Opts::default()
        },
    );
    for raw in [
        r#"{"model":"gpt-6","input":"hi","previous_response_id":"resp_1","previous_response_id":null}"#,
        r#"{"model":"gpt-6","input":"hi","previous_response_id":null,"previous_response_id":"resp_1"}"#,
        r#"{"model":"gpt-6","input":[{"role":"user","content":"hi","content":[{"type":"input_text","text":"x"}]}]}"#,
    ] {
        let sent = respond(&on, raw).await;
        assert_eq!(sent.status, StatusCode::BAD_REQUEST, "{raw}");
        assert_eq!(sent.json()["error"]["code"], "invalid-request-schema", "{raw}");
    }
    let sent = respond(&on, r#"{"model":"gpt-6","input":"hi","background":false,"background":false}"#).await;
    assert_eq!((sent.status, sent.json()["error"]["param"].clone()), (StatusCode::BAD_REQUEST, json!("background")));
    assert_eq!(store.checks(), 0, "no refusal reached the limits");
    assert_eq!(mock.request_count(), 0);
    assert_eq!(
        respond(&on, r#"{"model":"gpt-6","input":[{"role":"user","content":"hi"}],"previous_response_id":null}"#).await.status,
        StatusCode::OK
    );
    assert_eq!(respond(&on, NON_STREAM).await.status, StatusCode::TOO_MANY_REQUESTS, "the one slot was used by the clean body only");

    let store = RecordingStore::new();
    let off = app(
        &mock.base_url,
        Opts {
            limits: Some(limits(rules(Some(1), None, None), store.clone())),
            ..Opts::default()
        },
    );
    for raw in [
        r#"{"model":"gpt-6","input":"hi","background":false,"background":true}"#,
        r#"{"model":"gpt-6","input":"hi","background":true,"background":false}"#,
    ] {
        let sent = respond(&off, raw).await;
        assert_eq!(sent.status, StatusCode::BAD_REQUEST, "{raw}");
        assert_eq!(sent.json()["error"]["code"], "responses-stateful-not-supported", "{raw}");
        assert_eq!(sent.json()["error"]["param"], "background", "{raw}");
    }
    assert_eq!(store.checks(), 0);
    let dup_d3 = r#"{"model":"gpt-6","input":"hi","previous_response_id":"resp_1","previous_response_id":null,"background":false}"#;
    assert_eq!(respond(&off, dup_d3).await.status, StatusCode::OK, "interception off passes other duplicates through");
    assert_eq!(mock.recorded().expect("sent upstream").body.as_ref(), dup_d3.as_bytes());
    assert_eq!(respond(&off, NON_STREAM).await.status, StatusCode::TOO_MANY_REQUESTS);
}

/// D15: a non-stream body over the shared interception hold cap (4 MiB) and under the
/// `[responses] max_held_bytes` cap (32 MiB) is checked and rewritten, not refused as too large.
#[tokio::test]
async fn the_hold_cap_is_the_responses_limit() {
    let mut v: Value = serde_json::from_str(&body_with_calls("completed")).expect("a fixture body");
    v["instructions"] = "x".repeat(5 * MIB).into();
    let body = v.to_string();
    assert!(body.len() > CoreLimits::defaults().max_held_bytes && body.len() < settings().max_held_bytes);
    let mock = MockOpenAi::spawn_json(StatusCode::OK, body).await;
    let sent = respond(
        &app(
            &mock.base_url,
            Opts {
                ti: ti(Arc::new(BlockNamed)),
                ..Opts::default()
            },
        ),
        NON_STREAM,
    )
    .await;
    assert_eq!(sent.status, StatusCode::OK, "{}", &sent.text()[..sent.text().len().min(512)]);
    assert!(!sent.text().contains("CANARY"));
    assert_eq!(sent.json()["output"][1]["id"], "msg_paigasus_1");
}

/// D4 holds with interception off; D3 does not apply then (a plain proxy).
#[tokio::test]
async fn background_is_refused_with_interception_off() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, "{}").await;
    let app = app(&mock.base_url, Opts::default());
    let sent = respond(&app, r#"{"model":"gpt-6","input":"hi","background":true}"#).await;
    assert_eq!(sent.status, StatusCode::BAD_REQUEST);
    assert_eq!(sent.json()["error"]["code"], "responses-stateful-not-supported");
    assert_eq!(sent.json()["error"]["param"], "background");
    assert_eq!(mock.request_count(), 0);
    let d3 = r#"{"model":"gpt-6","input":[{"id":"msg_1"}],"previous_response_id":"resp_1","conversation":"conv_1","prompt":{"id":"p"},"background":false}"#;
    assert_eq!(respond(&app, d3).await.status, StatusCode::OK);
    assert_eq!(mock.request_count(), 1);
}

/// D9 (C2): the route obeys the one shared `stream_enabled` switch, with the chat refusal.
#[tokio::test]
async fn streaming_obeys_stream_enabled() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, "{}").await;
    let app = app(
        &mock.base_url,
        Opts {
            chat_stream: false,
            ..Opts::default()
        },
    );
    let sent = respond(&app, STREAM).await;
    assert_eq!(sent.status, StatusCode::BAD_REQUEST);
    assert_eq!(sent.json()["error"]["code"], "streaming-disabled");
    assert_eq!(sent.json()["error"]["param"], "stream");
    assert_eq!(mock.request_count(), 0);
    assert_eq!(respond(&app, NON_STREAM).await.status, StatusCode::OK);
}

/// Send `body` once with interception off and once on; return the two charge lists and the two
/// client bodies.
async fn charged(mock: &MockOpenAi, body: &str, decider: Arc<dyn ToolCallDecider>) -> ([Vec<u64>; 2], [Sent; 2]) {
    let mut charges = Vec::new();
    let mut sent = Vec::new();
    for ti in [None, ti(decider.clone())] {
        let store = RecordingStore::new();
        let app = app(
            &mock.base_url,
            Opts {
                limits: Some(limits(rules(None, None, Some(1_000_000_000)), store.clone())),
                ti,
                ..Opts::default()
            },
        );
        sent.push(respond(&app, body).await);
        charges.push(store.charges());
    }
    let s1 = sent.pop().expect("two");
    let s0 = sent.pop().expect("two");
    ([charges.remove(0), charges.remove(0)], [s0, s1])
}

/// § 8.2: a non-stream answer is charged its `usage.total_tokens`, rewritten or not (D14).
#[tokio::test]
async fn a_non_stream_answer_is_charged_its_usage() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, body_with_calls("completed")).await;
    let ([off, on], [_, sent]) = charged(&mock, NON_STREAM, Arc::new(BlockNamed)).await;
    assert_eq!(off, vec![13]);
    assert_eq!(on, off);
    assert!(!sent.text().contains("CANARY"), "the charge is the same for a rewritten body");
}

/// § 8.2: a stream is charged the terminal event's `response.usage.total_tokens`.
#[tokio::test]
async fn a_stream_is_charged_its_usage() {
    let records = blocked_stream(21, 0);
    let mock = MockOpenAi::spawn_raw_sse(records.clone()).await;
    let ([off, on], [plain, held]) = charged(&mock, STREAM, Arc::new(BlockNamed)).await;
    assert_eq!(off, vec![21]);
    assert_eq!(on, off);
    assert_eq!(plain.text(), records.concat(), "interception off: the bytes are unchanged");
    assert!(!held.text().contains("CANARY"));
}

/// Review Focus 5, AC 10 and C3: a terminal event over 1 MiB (the chat record limit) is charged
/// its reported usage with interception off and on, for a held and rewritten stream.
#[tokio::test]
async fn a_terminal_event_over_1_mib_is_charged_with_interception_on_and_off() {
    let records = blocked_stream(4_242, 3 * MIB / 2);
    assert!(records.last().expect("a terminal").len() > MIB);
    let mock = MockOpenAi::spawn_raw_sse(records).await;
    let ([off, on], [_, held]) = charged(&mock, STREAM, Arc::new(BlockNamed)).await;
    assert_eq!(off, vec![4_242]);
    assert_eq!(on, off);
    let text = held.text();
    assert!(!text.contains("CANARY"));
    assert!(text.contains(r#"[paigasus] tool call \"blocked_rm\" was blocked (policy)."#));
}

/// Review Focus 5 and C3: with interception on, a terminal event over the 16 MiB record limit is
/// a stream failure. The client gets the `(malformed)` rewrite and no canary, and the charge is
/// still the reported total, because the usage scanner reads the upstream bytes before the hold
/// and has no record limit.
#[tokio::test]
async fn a_terminal_event_over_the_record_limit_is_charged_and_fails_closed() {
    let records = blocked_stream(9_001, 17 * MIB);
    assert!(records.last().expect("a terminal").len() > 16 * MIB);
    let mock = MockOpenAi::spawn_raw_sse(records).await;
    let ([off, on], [_, failed]) = charged(&mock, STREAM, Arc::new(AllowAll)).await;
    assert_eq!(off, vec![9_001]);
    assert_eq!(on, off, "the over-limit terminal event is still charged");
    let text = failed.text();
    assert_eq!(failed.status, StatusCode::OK);
    assert!(!text.contains("CANARY"), "a failed stream never releases the held call");
    assert!(text.contains(r#"[paigasus] tool call \"blocked_rm\" was blocked (malformed)."#), "{}", &text[..text.len().min(4096)]);
    assert!(text.contains(r#"[paigasus] tool call \"read_file\" was blocked (malformed)."#));
    assert!(text.len() < MIB, "the over-limit record is not forwarded");
}

/// § 6.5 non-stream (AC 3): the blocked item becomes the completed message at the same index,
/// with the decisions header. C4: a call that is not `"completed"` is reported and rewritten
/// `truncated` over the decider's allow. C5: under `on_failure = allow` such a body goes through
/// unchanged, as a truncated stream does.
#[tokio::test]
async fn a_non_stream_blocked_call_is_rewritten_with_the_header() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, body_with_calls("completed")).await;
    let sent = respond(
        &app(
            &mock.base_url,
            Opts {
                ti: ti(Arc::new(BlockNamed)),
                ..Opts::default()
            },
        ),
        NON_STREAM,
    )
    .await;
    assert_eq!(sent.status, StatusCode::OK);
    assert!(!sent.text().contains("CANARY"));
    let v = sent.json();
    assert_eq!(
        v["output"][1],
        json!({"type":"message","id":"msg_paigasus_1","role":"assistant","status":"completed",
            "content":[{"type":"output_text","text":"[paigasus] tool call \"blocked_rm\" was blocked (policy).","annotations":[],"logprobs":[]}]})
    );
    assert_eq!(v["output"][0]["id"], "msg_1");
    assert_eq!(v["output"][2]["id"], "fc_2");
    assert_eq!(v["usage"]["total_tokens"], 13);
    let h = decisions(&sent);
    assert_eq!(h.len(), 2, "{h:?}");
    assert!(h.iter().any(|e| e["name"] == "blocked_rm" && e["decision"] == "block" && e["code"] == "policy"), "{h:?}");
    assert!(h.iter().any(|e| e["name"] == "read_file" && e["decision"] == "allow"), "{h:?}");

    let cut = body_with_calls("incomplete");
    let mock = MockOpenAi::spawn_json(StatusCode::OK, cut.clone()).await;
    let sent = respond(
        &app(
            &mock.base_url,
            Opts {
                ti: ti(Arc::new(AllowAll)),
                ..Opts::default()
            },
        ),
        NON_STREAM,
    )
    .await;
    assert_eq!(sent.status, StatusCode::OK);
    assert!(!sent.text().contains("CANARY"));
    assert_eq!(sent.json()["output"][1]["content"][0]["text"], "[paigasus] tool call \"blocked_rm\" was blocked (truncated).");
    let h = decisions(&sent);
    assert!(h.iter().any(|e| e["name"] == "blocked_rm" && e["decision"] == "block" && e["code"] == "truncated"), "{h:?}");
    let observe = app(
        &mock.base_url,
        Opts {
            ti: ti_with(OnFailure::Allow, Arc::new(BlockNamed)),
            ..Opts::default()
        },
    );
    let sent = respond(&observe, NON_STREAM).await;
    assert_eq!(sent.status, StatusCode::OK);
    assert_eq!(sent.text(), cut, "on_failure = allow forwards a truncated body unchanged");
    assert!(sent.headers.get("paigasus-tool-decisions").is_none());
}

/// § 9: a body that cannot be checked is a 502 under `block` and goes through under `allow`.
#[tokio::test]
async fn a_malformed_body_is_a_502_under_block() {
    let bad = r#"{"id":"resp_1","object":"response","output":[],"a":1,"a":2}"#;
    let mock = MockOpenAi::spawn_json(StatusCode::OK, bad).await;
    let sent = respond(
        &app(
            &mock.base_url,
            Opts {
                ti: ti(Arc::new(AllowAll)),
                ..Opts::default()
            },
        ),
        NON_STREAM,
    )
    .await;
    assert_eq!(sent.status, StatusCode::BAD_GATEWAY);
    assert_eq!(sent.json()["error"]["code"], "tool-interception-malformed-response");
    let sent = respond(
        &app(
            &mock.base_url,
            Opts {
                ti: ti_with(OnFailure::Allow, Arc::new(AllowAll)),
                ..Opts::default()
            },
        ),
        NON_STREAM,
    )
    .await;
    assert_eq!(sent.status, StatusCode::OK);
    assert_eq!(sent.text(), bad);
}

/// AC 3 and § 6.5: a blocked streamed call never reaches the client; its frames become the
/// message events, and the allowed call goes through.
#[tokio::test]
async fn a_streamed_blocked_call_never_reaches_the_client() {
    let mock = MockOpenAi::spawn_raw_sse(blocked_stream(8, 0)).await;
    let sent = respond(
        &app(
            &mock.base_url,
            Opts {
                ti: ti(Arc::new(BlockNamed)),
                ..Opts::default()
            },
        ),
        STREAM,
    )
    .await;
    assert_eq!(sent.status, StatusCode::OK);
    assert_eq!(sent.header("content-type"), "text/event-stream");
    let text = sent.text();
    assert!(!text.contains("CANARY"), "{text}");
    assert!(!text.contains("\"name\":\"blocked_rm\",\"arguments\""), "{text}");
    for want in [
        r#"event: response.output_item.added"#,
        r#""item":{"type":"message","id":"msg_paigasus_1","role":"assistant","status":"in_progress","content":[]}"#,
        r#"event: response.content_part.added"#,
        r#""part":{"type":"output_text","text":"","annotations":[],"logprobs":[]}"#,
        r#"event: response.output_text.delta"#,
        r#""delta":"[paigasus] tool call \"blocked_rm\" was blocked (policy).""#,
        r#"event: response.output_item.done"#,
        r#""status":"completed","content":[{"type":"output_text","text":"[paigasus] tool call \"blocked_rm\" was blocked (policy).""#,
        r#"event: response.completed"#,
        r#""name":"read_file""#,
        ": paigasus-decision {",
    ] {
        assert!(text.contains(want), "lacks {want}:\n{text}");
    }
}

/// D13: an upstream `error` event mid-hold ends the stream with the D13 frame, and the held call
/// never reaches the client. With interception off, a transport failure ends with it too.
#[tokio::test]
async fn an_upstream_error_mid_hold_ends_with_the_d13_frame() {
    let mut e = Events::created();
    e.push(json!({"type":"response.output_item.added","item":{"type":"function_call","id":"fc_0","status":"in_progress","call_id":"call_0","name":"read_file","arguments":""},"output_index":0}));
    e.push(json!({"type":"response.function_call_arguments.delta","delta":"{\"CANARY\":","item_id":"fc_0","output_index":0}));
    e.push(json!({"type":"error","code":"server_error","message":"boom","param":null}));
    let mock = MockOpenAi::spawn_raw_sse(e.out).await;
    let text = respond(
        &app(
            &mock.base_url,
            Opts {
                ti: ti(Arc::new(AllowAll)),
                ..Opts::default()
            },
        ),
        STREAM,
    )
    .await
    .text();
    assert!(!text.contains("CANARY"), "{text}");
    assert!(text.ends_with(RESPONSES_TERMINAL_SSE_ERROR), "{text}");
    assert_eq!(text.matches("\"code\":\"upstream-error\"").count(), 1, "{text}");

    let base_url = support::spawn_truncated_sse().await;
    let sent = respond(&app(&base_url, Opts::default()), STREAM).await;
    assert_eq!(sent.status, StatusCode::OK);
    assert!(sent.text().ends_with(RESPONSES_TERMINAL_SSE_ERROR), "{}", sent.text());
}

/// § 9: a non-2xx answer is forwarded verbatim, with no charge and no interception.
#[tokio::test]
async fn a_non_2xx_answer_is_forwarded_with_no_charge() {
    let err = r#"{"error":{"message":"bad","type":"invalid_request_error","param":null,"code":null}}"#;
    let mock = MockOpenAi::spawn_json(StatusCode::BAD_REQUEST, err).await;
    let ([off, on], sent) = charged(&mock, NON_STREAM, Arc::new(BlockNamed)).await;
    assert_eq!((off, on), (vec![], vec![]));
    for s in sent {
        assert_eq!((s.status, s.text().as_str()), (StatusCode::BAD_REQUEST, err));
    }
}

/// G8 for this route: a client disconnect cancels the upstream request.
#[tokio::test]
async fn client_abort_cancels_upstream() {
    let (mock, cancelled): (MockOpenAi, Arc<AtomicBool>) = MockOpenAi::spawn_abortable_stream().await;
    let app = app(&mock.base_url, Opts::default());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    let _ = rustls::crypto::ring::default_provider().install_default();
    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/v1/responses"))
        .header(reqwest::header::AUTHORIZATION, format!("Bearer {CALLER_KEY}"))
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(STREAM)
        .send()
        .await
        .expect("responds");
    assert_eq!(resp.status(), StatusCode::OK);
    let mut stream = resp.bytes_stream();
    let first = tokio::time::timeout(Duration::from_secs(2), stream.next())
        .await
        .expect("the first chunk arrives")
        .expect("a first chunk")
        .expect("not a transport error");
    assert!(first.starts_with(b"data: "), "{first:?}");
    assert!(!cancelled.load(Ordering::SeqCst));
    drop(stream);
    let propagated = tokio::time::timeout(Duration::from_secs(2), async {
        while !cancelled.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    assert!(propagated.is_ok(), "client-abort must cancel the upstream request");
    server.abort();
}
