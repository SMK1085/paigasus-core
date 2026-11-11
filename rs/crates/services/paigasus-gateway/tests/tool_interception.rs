// SPDX-License-Identifier: Apache-2.0
//! SMA-749: tool-call interception through the real router against the mock upstream.

mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use axum::http::StatusCode;
use paigasus_gateway::adapters::http::interception::ToolInterception;
use paigasus_gateway_core::{AllowAll, BlockCode, DecideFuture, Decision, DecisionContext, Limits, ToolCall, ToolCallDecider, ToolTurn};
use support::MockOpenAi;
use support::limits::*;

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

/// Never answers for a call whose name starts with `hang_`; allows everything else.
struct HangNamed;

impl ToolCallDecider for HangNamed {
    fn decide(&self, _ctx: Arc<DecisionContext>, _turn: Arc<ToolTurn>, calls: Vec<ToolCall>) -> DecideFuture {
        if calls.iter().any(|c| c.name.starts_with("hang_")) {
            return Box::pin(std::future::pending());
        }
        let n = calls.len();
        Box::pin(std::future::ready(Ok(vec![Decision::Allow; n])))
    }
}

fn ti(decider: Arc<dyn ToolCallDecider>) -> Option<Arc<ToolInterception>> {
    ti_with(Limits::defaults(), decider)
}

fn ti_with(limits: Limits, decider: Arc<dyn ToolCallDecider>) -> Option<Arc<ToolInterception>> {
    Some(Arc::new(ToolInterception { limits, decider }))
}

/// A choice-0 tool-call delta. Every chunk carries `"object":"chat.completion.chunk"` (Task 11 F1).
fn tool_chunk(index: u32, id: Option<&str>, name: Option<&str>, args: &str) -> String {
    let mut e = serde_json::json!({"index": index, "function": {"arguments": args}});
    if let Some(id) = id {
        e["id"] = id.into();
        e["type"] = "function".into();
    }
    if let Some(n) = name {
        e["function"]["name"] = n.into();
    }
    format!(
        "data: {}\n\n",
        serde_json::json!({"id":"chatcmpl-1","object":"chat.completion.chunk","created":1790942400,"model":"gpt-4o-mini","system_fingerprint":"fp_1","choices":[{"index":0,"delta":{"tool_calls":[e]},"finish_reason":null}]})
    )
}

fn finish_chunk() -> String {
    format!(
        "data: {}\n\n",
        serde_json::json!({"id":"chatcmpl-1","object":"chat.completion.chunk","created":1790942400,"model":"gpt-4o-mini","choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]})
    )
}

fn upstream() -> Vec<String> {
    vec![
        chunk("Sure.", true),
        tool_chunk(0, Some("call_0"), Some("read_file"), "{\"p\":"),
        tool_chunk(0, None, None, "1}"),
        tool_chunk(1, Some("call_1"), Some("blocked_rm"), "{\"p\":\"CANARY\"}"),
        finish_chunk(),
        usage_chunk(11),
        "data: [DONE]\n\n".to_owned(),
    ]
}

fn payloads(body: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(body)
        .split("\n\n")
        .filter_map(|f| f.lines().find_map(|l| l.strip_prefix("data: ")).map(str::to_owned))
        .collect()
}

/// AC 1: off means today's relay, byte for byte.
#[tokio::test]
async fn interception_off_is_byte_identical() {
    let records = upstream();
    let mock = MockOpenAi::spawn_raw_sse(records.clone()).await;
    let app = app_with_interception(ScopedIam::arc(PRINCIPAL_1, ORG_A_PRN), &mock.base_url, None, None);
    let sent = send(&app, STREAM_BODY).await;
    assert_eq!(sent.status, StatusCode::OK);
    assert_eq!(sent.text(), records.concat());
}

/// AC 2: allow-all forwards every payload unchanged and adds only comments.
#[tokio::test]
async fn allow_all_is_payload_identical() {
    let records = upstream();
    let mock = MockOpenAi::spawn_raw_sse(records.clone()).await;
    let app = app_with_interception(ScopedIam::arc(PRINCIPAL_1, ORG_A_PRN), &mock.base_url, None, ti(Arc::new(AllowAll)));
    let sent = send(&app, STREAM_BODY).await;
    assert_eq!(payloads(&sent.body), payloads(records.concat().as_bytes()));
    assert!(sent.text().contains(": paigasus-decision {"));
}

/// AC 3: a blocked call never reaches the client.
#[tokio::test]
async fn a_blocked_call_never_reaches_the_client() {
    let mock = MockOpenAi::spawn_raw_sse(upstream()).await;
    let app = app_with_interception(ScopedIam::arc(PRINCIPAL_1, ORG_A_PRN), &mock.base_url, None, ti(Arc::new(BlockNamed)));
    let sent = send(&app, STREAM_BODY).await;
    let text = sent.text();
    assert!(!text.contains("CANARY"), "{text}");
    assert!(payloads(&sent.body).iter().all(|p| !p.contains("\"name\":\"blocked_rm\"")), "{text}");
    assert!(text.contains(r#"[paigasus] tool call \"blocked_rm\" was blocked (policy)."#), "{text}");
    assert!(payloads(&sent.body).iter().any(|p| p.contains("\"name\":\"read_file\"")), "{text}");
    assert!(text.ends_with("data: [DONE]\n\n"));
}

/// D17: an upstream error record mid-hold drops the held call and ends with the terminal frame.
#[tokio::test]
async fn an_upstream_error_mid_hold_ends_with_the_terminal_frame() {
    let records = vec![
        tool_chunk(0, Some("call_0"), Some("read_file"), "{\"CANARY\":"),
        "data: {\"error\":{\"message\":\"boom\"}}\n\n".to_owned(),
    ];
    let mock = MockOpenAi::spawn_raw_sse(records).await;
    let app = app_with_interception(ScopedIam::arc(PRINCIPAL_1, ORG_A_PRN), &mock.base_url, None, ti(Arc::new(AllowAll)));
    let text = send(&app, STREAM_BODY).await.text();
    assert!(!text.contains("CANARY"), "{text}");
    assert!(text.contains("\"code\":\"upstream-error\""), "{text}");
}

/// The charges for one upstream sent through the router with interception off, then on.
async fn charges_off_and_on(records: Vec<String>, decider: Arc<dyn ToolCallDecider>) -> (Vec<u64>, Vec<u64>) {
    let mock = MockOpenAi::spawn_raw_sse(records).await;
    let mut out = Vec::new();
    for ti in [None, ti(decider)] {
        let store = RecordingStore::new();
        let app = app_with_interception(
            ScopedIam::arc(PRINCIPAL_1, ORG_A_PRN),
            &mock.base_url,
            Some(limits(rules(None, None, Some(1_000_000)), store.clone())),
            ti,
        );
        let sent = send(&app, STREAM_BODY).await;
        assert_eq!(sent.status, StatusCode::OK);
        out.push(store.charges());
    }
    (out.remove(0), out.remove(0))
}

/// D24 (Review focus 5): a block does not change the charge. Differential against interception
/// off, once with the upstream's usage record (charged as reported) and once without (charged
/// from the completion records the scanner counted, which a rewrite would change if the scanner
/// read the rewritten bytes instead of the upstream's).
#[tokio::test]
async fn a_blocked_stream_charges_like_the_upstream() {
    let (off, on) = charges_off_and_on(upstream(), Arc::new(BlockNamed)).await;
    assert_eq!(off, vec![11]);
    assert_eq!(on, off);
    let no_usage: Vec<String> = upstream().into_iter().filter(|r| !r.contains("\"usage\":{")).collect();
    let (off, on) = charges_off_and_on(no_usage, Arc::new(BlockNamed)).await;
    assert_eq!(off.len(), 1, "{off:?}");
    assert_eq!(on, off);
}

/// D23/D24: the interceptor ends the client stream at an error record mid-hold, but the usage
/// record the upstream sends after it is still read, so the charge equals today's relay's.
#[tokio::test]
async fn an_error_record_mid_hold_still_charges_the_upstream_usage() {
    let records = vec![
        tool_chunk(0, Some("call_0"), Some("read_file"), "{\"p\":"),
        "data: {\"error\":{\"message\":\"boom\"}}\n\n".to_owned(),
        usage_chunk(17),
        "data: [DONE]\n\n".to_owned(),
    ];
    let mock = MockOpenAi::spawn_raw_sse(records.clone()).await;
    let today = RecordingStore::new();
    let off = app_with_interception(
        ScopedIam::arc(PRINCIPAL_1, ORG_A_PRN),
        &mock.base_url,
        Some(limits(rules(None, None, Some(1_000_000)), today.clone())),
        None,
    );
    send(&off, STREAM_BODY).await;
    let store = RecordingStore::new();
    let on = app_with_interception(
        ScopedIam::arc(PRINCIPAL_1, ORG_A_PRN),
        &mock.base_url,
        Some(limits(rules(None, None, Some(1_000_000)), store.clone())),
        ti(Arc::new(AllowAll)),
    );
    let text = send(&on, STREAM_BODY).await.text();
    assert!(text.contains("\"code\":\"upstream-error\""), "{text}");
    assert_eq!(today.charges(), vec![17]);
    assert_eq!(store.charges(), today.charges());
}

/// D14/D18 over HTTP: a decider that never answers is timed out, the call is blocked with
/// `decider-error`, and the stream still ends. (Two holds, P-5 and D22, need a format with more
/// than one hold per stream; the relay's unit tests cover them with Anthropic blocks.)
#[tokio::test]
async fn a_hanging_decider_is_timed_out_and_the_call_blocked() {
    let limits = Limits {
        decide_timeout: Duration::from_millis(100),
        ..Limits::defaults()
    };
    let records = vec![
        tool_chunk(0, Some("call_0"), Some("hang_rm"), "{\"p\":\"CANARY\"}"),
        finish_chunk(),
        usage_chunk(5),
        "data: [DONE]\n\n".to_owned(),
    ];
    let mock = MockOpenAi::spawn_raw_sse(records).await;
    let app = app_with_interception(ScopedIam::arc(PRINCIPAL_1, ORG_A_PRN), &mock.base_url, None, ti_with(limits, Arc::new(HangNamed)));
    let sent = tokio::time::timeout(Duration::from_secs(5), send(&app, STREAM_BODY)).await.expect("the stream ends");
    let text = sent.text();
    assert!(!text.contains("CANARY"), "{text}");
    assert!(payloads(&sent.body).iter().all(|p| !p.contains("\"name\":\"hang_rm\"")), "{text}");
    assert!(text.contains(r#""code":"decider-error""#), "{text}");
    assert!(text.ends_with("data: [DONE]\n\n"), "{text}");
}

/// D25: a client disconnect still cancels the upstream with interception on. The mock's one
/// frame is not JSON, so nothing is forwarded; the stream is dropped right after the head.
#[tokio::test]
async fn client_abort_cancels_upstream_with_interception_on() {
    let (mock, cancelled): (MockOpenAi, Arc<AtomicBool>) = MockOpenAi::spawn_abortable_stream().await;
    let app = app_with_interception(ScopedIam::arc(PRINCIPAL_1, ORG_A_PRN), &mock.base_url, None, ti(Arc::new(AllowAll)));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/v1/chat/completions"))
        .header(reqwest::header::AUTHORIZATION, format!("Bearer {CALLER_KEY}"))
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(STREAM_BODY)
        .send()
        .await
        .expect("responds");
    assert_eq!(resp.status(), StatusCode::OK);
    drop(resp);
    let propagated = tokio::time::timeout(Duration::from_secs(2), async {
        while !cancelled.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    assert!(propagated.is_ok(), "cancel-on-drop must survive interception");
    server.abort();
}

fn body_with_calls() -> String {
    serde_json::json!({"id":"chatcmpl-1","object":"chat.completion","created":1790942400,"model":"gpt-4o-mini",
        "choices":[{"index":0,"message":{"role":"assistant","content":null,"tool_calls":[
            {"id":"c0","type":"function","function":{"name":"read_file","arguments":"{}"}},
            {"id":"c1","type":"function","function":{"name":"blocked_rm","arguments":"{\"p\":\"CANARY\"}"}}]},"finish_reason":"tool_calls"}],
        "usage":{"prompt_tokens":2,"completion_tokens":3,"total_tokens":13}})
    .to_string()
}

#[tokio::test]
async fn a_non_stream_allow_all_body_is_forwarded_unchanged_with_a_header() {
    let body = body_with_calls();
    let mock = MockOpenAi::spawn_json(StatusCode::OK, body.clone()).await;
    let app = app_with_interception(ScopedIam::arc(PRINCIPAL_1, ORG_A_PRN), &mock.base_url, None, ti(Arc::new(AllowAll)));
    let sent = send(&app, NON_STREAM_BODY).await;
    assert_eq!(sent.text(), body);
    let h: Vec<serde_json::Value> = serde_json::from_str(sent.header("paigasus-tool-decisions")).expect("an ASCII JSON header");
    assert_eq!(h.len(), 2);
}

#[tokio::test]
async fn a_non_stream_blocked_call_is_rewritten() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, body_with_calls()).await;
    let app = app_with_interception(ScopedIam::arc(PRINCIPAL_1, ORG_A_PRN), &mock.base_url, None, ti(Arc::new(BlockNamed)));
    let sent = send(&app, NON_STREAM_BODY).await;
    assert_eq!(sent.status, StatusCode::OK);
    assert!(!sent.text().contains("CANARY"));
    let v = sent.json();
    assert_eq!(v["choices"][0]["message"]["tool_calls"].as_array().unwrap().len(), 1);
    assert!(v["choices"][0]["message"]["content"].as_str().unwrap().contains("(policy)"));
}

#[tokio::test]
async fn a_non_stream_body_without_calls_gets_no_header() {
    let mock = MockOpenAi::spawn_json(StatusCode::OK, USAGE_BODY).await;
    let app = app_with_interception(ScopedIam::arc(PRINCIPAL_1, ORG_A_PRN), &mock.base_url, None, ti(Arc::new(AllowAll)));
    let sent = send(&app, NON_STREAM_BODY).await;
    assert_eq!(sent.text(), USAGE_BODY);
    assert!(sent.headers.get("paigasus-tool-decisions").is_none());
}

#[tokio::test]
async fn a_malformed_non_stream_body_is_a_502_with_block_and_forwarded_with_allow() {
    let bad = r#"{"choices":[{"index":0,"message":{"tool_calls":[]}}],"a":1,"a":2}"#;
    let mock = MockOpenAi::spawn_json(StatusCode::OK, bad).await;
    let block = app_with_interception(ScopedIam::arc(PRINCIPAL_1, ORG_A_PRN), &mock.base_url, None, ti(Arc::new(AllowAll)));
    let sent = send(&block, NON_STREAM_BODY).await;
    assert_eq!(sent.status, StatusCode::BAD_GATEWAY);
    assert_eq!(sent.json()["error"]["code"], "tool-interception-malformed-response");
    let observe = ti_with(
        Limits {
            on_failure: paigasus_gateway_core::OnFailure::Allow,
            ..Limits::defaults()
        },
        Arc::new(AllowAll),
    );
    let allow = app_with_interception(ScopedIam::arc(PRINCIPAL_1, ORG_A_PRN), &mock.base_url, None, observe);
    let sent = send(&allow, NON_STREAM_BODY).await;
    assert_eq!(sent.status, StatusCode::OK);
    assert_eq!(sent.text(), bad);
}

#[tokio::test]
async fn a_non_2xx_answer_is_forwarded_as_today() {
    let err = r#"{"error":{"message":"bad","type":"invalid_request_error","param":null,"code":null}}"#;
    let mock = MockOpenAi::spawn_json(StatusCode::BAD_REQUEST, err).await;
    let app = app_with_interception(ScopedIam::arc(PRINCIPAL_1, ORG_A_PRN), &mock.base_url, None, ti(Arc::new(BlockNamed)));
    let sent = send(&app, NON_STREAM_BODY).await;
    assert_eq!((sent.status, sent.text().as_str()), (StatusCode::BAD_REQUEST, err));
}

/// D24 (Review focus 5): differential against interception off. The charge is the upstream's
/// reported usage, whether or not the body is rewritten, and a refused (502) body keeps it too.
#[tokio::test]
async fn a_blocked_body_charges_like_the_upstream() {
    let bad = r#"{"usage":{"prompt_tokens":2,"completion_tokens":3,"total_tokens":13},"a":1,"a":2}"#;
    for (body, decider) in [
        (body_with_calls(), Arc::new(BlockNamed) as Arc<dyn ToolCallDecider>),
        (bad.to_owned(), Arc::new(AllowAll) as Arc<dyn ToolCallDecider>),
    ] {
        let mock = MockOpenAi::spawn_json(StatusCode::OK, body).await;
        let mut charges = Vec::new();
        for ti in [None, ti(decider.clone())] {
            let store = RecordingStore::new();
            let app = app_with_interception(
                ScopedIam::arc(PRINCIPAL_1, ORG_A_PRN),
                &mock.base_url,
                Some(limits(rules(None, None, Some(1_000_000)), store.clone())),
                ti,
            );
            let _ = send(&app, NON_STREAM_BODY).await;
            charges.push(store.charges());
        }
        assert_eq!(charges[0], vec![13]);
        assert_eq!(charges[1], charges[0]);
    }
}
