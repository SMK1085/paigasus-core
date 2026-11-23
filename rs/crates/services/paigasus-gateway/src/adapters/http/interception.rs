// SPDX-License-Identifier: Apache-2.0
//! SMA-749: drives `paigasus-gateway-core` inside the chat relay (spec § 5.2).
//!
//! The interceptor is synchronous; this module owns the I/O around it: the upstream stream, one
//! pending decider future, and one timer. Nothing is spawned. Dropping the response body drops
//! the unfold state, and with it the upstream request (cancel-on-drop, D25) and any pending
//! decision.
//!
//! Like the plain relay, this runs with `current_ids() == None` (SMA-504 § 4.3): the request id
//! reaches the decider through `DecisionContext`, captured in the handler.
//!
//! The decider future and the timer are driven only while the client polls the body, so a client
//! that stalls long enough can see a hold end as `decider-error` (or `on_failure` in observe mode).
//!
//! On a client disconnect mid-hold, billing follows what the relay had read from the upstream so
//! far, read-ahead included, exactly as in the plain relay (SMA-730 D3).

use std::collections::VecDeque;
use std::convert::Infallible;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;

use axum::body::Bytes;
use futures::{Stream, StreamExt};
use metrics::{counter, histogram};
use paigasus_gateway_core::{
    Action, Basis, BlockCode, DecideFuture, DecidedCall, DeciderError, Decision, DecisionContext, Format, HoldKey, Interceptor, Limits, Metric, OnFailure, RequestEntry, ToolCall, ToolCallDecider,
    ToolTurn, parse_body, report_header,
};
use paigasus_observability::names;

use crate::adapters::http::relay::{feed_scan, finish_scan};
use crate::adapters::http::usage::StreamUsage;
use crate::adapters::openai::OpenAiByteStream;
use crate::application::charge_guard::ChargeGuard;
use crate::application::decision_log::DecisionRecorder;

/// A response chunk. The alias keeps the body type `Bytes` out of a function parameter list: the
/// `repo:http-extractor-envelope` gate bans `Bytes` as a REQUEST extractor there, and this is a
/// response stream item, not an extractor.
type Chunk = Bytes;

/// What `[tool_interception]` builds at boot.
pub struct ToolInterception {
    pub limits: Limits,
    pub decider: Arc<dyn ToolCallDecider>,
    /// The shared decision recorder (SMA-755, P-8). `None` with the tool policy off: nothing is recorded.
    pub recorder: Option<Arc<DecisionRecorder>>,
}

/// Map one core metric to the registered names (P-9).
pub(crate) fn record(m: Metric) {
    match m {
        Metric::RecordDropped(reason) => counter!(names::GATEWAY_TOOL_INTERCEPTION_RECORDS_DROPPED_TOTAL, "reason" => reason).increment(1),
        Metric::Decision { decision, basis } => counter!(names::GATEWAY_TOOL_INTERCEPTION_DECISIONS_TOTAL, "decision" => decision, "basis" => basis).increment(1),
        Metric::OnFailure(code) => counter!(names::GATEWAY_TOOL_INTERCEPTION_ON_FAILURE_TOTAL, "code" => code).increment(1),
        Metric::HoldSeconds(s) => histogram!(names::GATEWAY_TOOL_INTERCEPTION_HOLD_SECONDS).record(s),
    }
}

/// What the non-stream path does with a body (spec § 5.2).
pub(crate) enum BodyOutcome {
    /// `rewritten` is `None` when the caller forwards the original body it still owns.
    Forward { rewritten: Option<Bytes>, report: Option<String> },
    /// D16 block on a body that cannot be checked: the 502 `tool-interception-malformed-response`.
    Refuse,
}

/// The `paigasus-tool-decisions` header of an answer with no call entries (SMA-752 § 6.5.3): the
/// request entries alone, or `None` when there are none (then the header is absent, as before).
pub(crate) fn request_report(requests: &[RequestEntry]) -> Option<String> {
    (!requests.is_empty()).then(|| report_header(&[], requests))
}

/// Keep `keep` alive until the stream `s` is dropped: at its end, or on a client disconnect. The
/// pipeline uses it to hold the request's overhead sample inside a stream body (preflight F14).
pub(crate) fn keep_until_end<S, K>(s: S, keep: K) -> impl Stream<Item = S::Item> + Send + 'static
where
    S: Stream + Send + 'static,
    K: Send + 'static,
{
    s.map(move |item| {
        // Load-bearing: a `move` closure captures only what it uses.
        let _keep = &keep;
        item
    })
}

/// Parse, decide and rewrite one complete 2xx body. The usage read happened on the ORIGINAL body
/// before this runs (D24), so nothing here changes the charge. `requests` are the request-side
/// entries of the tool policy (SMA-752); they go into the header after the call entries.
pub(crate) async fn intercept_body(ti: &ToolInterception, limits: Limits, format: Format, ctx: Arc<DecisionContext>, turn: Arc<ToolTurn>, body: &[u8], requests: &[RequestEntry]) -> BodyOutcome {
    // The body cap is `max_held_bytes` (spec § 5.2).
    let calls = match parse_body(format, body, limits.max_held_bytes) {
        Ok(c) => c,
        Err(_) => {
            record(Metric::OnFailure("malformed"));
            return match limits.on_failure {
                OnFailure::Block => BodyOutcome::Refuse,
                OnFailure::Allow => BodyOutcome::Forward {
                    rewritten: None,
                    report: request_report(requests),
                },
            };
        }
    };
    if calls.calls.is_empty() && calls.unenforceable.is_empty() {
        return BodyOutcome::Forward {
            rewritten: None,
            report: request_report(requests),
        };
    }
    // SMA-769 (ruling F7, R15): a Responses item that is not `"completed"` is a truncation. The
    // stream counts it and, under `on_failure = allow`, passes the hold through unchanged; the
    // body does the same. Under `block` the forced `Truncated` wins over the decision below.
    if calls.is_truncated() {
        record(Metric::OnFailure("truncated"));
        if limits.on_failure == OnFailure::Allow {
            return BodyOutcome::Forward {
                rewritten: None,
                report: request_report(requests),
            };
        }
    }
    let n = calls.calls.len();
    let (decisions, basis) = if n == 0 {
        (Vec::new(), Basis::Decider)
    } else {
        // The body is the hold: its decide_timeout starts now (D35).
        let fut = ti.decider.decide_by(Arc::clone(&ctx), turn, calls.calls.clone(), Instant::now());
        match tokio::time::timeout(limits.decide_timeout, fut).await {
            Ok(Ok(d)) if d.len() == n => (d, Basis::Decider),
            _ => {
                record(Metric::OnFailure("decider_error"));
                match limits.on_failure {
                    OnFailure::Block => (vec![DecidedCall::from(Decision::Block { code: BlockCode::DeciderError }); n], Basis::OnFailure),
                    OnFailure::Allow => (vec![DecidedCall::from(Decision::Allow); n], Basis::OnFailure),
                }
            }
        }
    };
    for d in &decisions {
        let decision = if d.decision.is_allow() { "allow" } else { "block" };
        record(Metric::Decision { decision, basis: basis.as_str() });
    }
    // One record per report entry, at the same place as the header (SMA-755 P-1).
    let reported = calls.reported(&decisions, basis);
    if let Some(recorder) = &ti.recorder {
        for r in &reported {
            recorder.record_response(&ctx, r);
        }
    }
    let entries: Vec<_> = reported.into_iter().map(|r| r.entry).collect();
    let report = report_header(&entries, requests);
    let plain: Vec<Decision> = decisions.iter().map(|d| d.decision).collect();
    let rewritten = calls.apply(&plain).map(Bytes::from);
    BodyOutcome::Forward { rewritten, report: Some(report) }
}

/// The unfold state. It owns the upstream, the charge guard and the pending decision, so a
/// client disconnect drops all three (D25), exactly as the plain relay drops the first two.
struct Relay {
    /// `None` once the upstream has ended, failed, or been drained.
    inner: Option<OpenAiByteStream>,
    scan: StreamUsage,
    guard: Option<ChargeGuard>,
    icpt: Interceptor,
    ti: Arc<ToolInterception>,
    ctx: Arc<DecisionContext>,
    turn: Arc<ToolTurn>,
    out: VecDeque<Bytes>,
    /// Completed holds waiting for the decider, in completion order (P-5).
    decide_queue: VecDeque<(HoldKey, Vec<ToolCall>)>,
    /// The one decision in flight (P-5).
    pending: Option<(HoldKey, DecideFuture)>,
    sleep: Pin<Box<tokio::time::Sleep>>,
}

/// What woke the relay.
enum Event {
    Decided(Result<Vec<DecidedCall>, DeciderError>),
    Wake,
    Upstream(Option<Result<Bytes, reqwest::Error>>),
}

impl Relay {
    fn apply(&mut self, actions: Vec<Action>) {
        for a in actions {
            match a {
                Action::Emit(b) => self.out.push_back(b),
                Action::Decide { hold, calls } => self.decide_queue.push_back((hold, calls)),
                // The core never returns a wake in the past, so this cannot spin.
                Action::Wake(at) => self.sleep.as_mut().reset(tokio::time::Instant::from_std(at)),
                Action::Count(m) => record(m),
                // The decision log records at the report entry (SMA-755 P-1).
                Action::Reported(r) => {
                    if let Some(recorder) = &self.ti.recorder {
                        recorder.record_response(&self.ctx, &r);
                    }
                }
            }
        }
        // A hold that is no longer deciding (a timeout ruled on it) needs no answer. Drop
        // its future so the next hold is not stuck behind it.
        if let Some((hold, _)) = &self.pending
            && !self.icpt.is_deciding(*hold)
        {
            self.pending = None;
        }
    }

    /// Start the next decision if none is in flight. Holds ruled on while they queued are skipped.
    fn start_next_decision(&mut self) {
        while self.pending.is_none() {
            let Some((hold, calls)) = self.decide_queue.pop_front() else { return };
            if self.icpt.is_deciding(hold) {
                // The deadline of the hold counts from its completion, not from now: it can have
                // waited behind an earlier hold (D35, P-10).
                let hold_start = self.icpt.deciding_since(hold).unwrap_or_else(Instant::now);
                let fut = self.ti.decider.decide_by(Arc::clone(&self.ctx), Arc::clone(&self.turn), calls, hold_start);
                self.pending = Some((hold, fut));
            }
        }
    }

    /// Biased in the spec's order (§ 5.2): decider, timer, upstream.
    async fn next_event(&mut self) -> Event {
        let Relay { inner, pending, sleep, .. } = self;
        tokio::select! {
            biased;
            r = async {
                match pending.as_mut() {
                    Some((_, f)) => f.as_mut().await,
                    None => std::future::pending().await,
                }
            } => Event::Decided(r),
            () = sleep.as_mut() => Event::Wake,
            c = async {
                match inner.as_mut() {
                    Some(s) => s.next().await,
                    None => std::future::pending().await,
                }
            } => Event::Upstream(c),
        }
    }

    /// The interceptor ended the client stream (an upstream error record mid-hold) while the
    /// upstream is still open. Today's relay would read the upstream to its end, so with a guard
    /// the rest is read for billing only (the usage record usually follows), and the charge stays
    /// what it is with interception off (D24). Without a guard there is nothing to bill, and the
    /// upstream is dropped at once.
    async fn drain_for_billing(&mut self) {
        let Some(mut inner) = self.inner.take() else { return };
        if self.guard.is_some() {
            while let Some(Ok(chunk)) = inner.next().await {
                feed_scan(&mut self.scan, &mut self.guard, &chunk);
            }
        }
        finish_scan(&mut self.scan, &mut self.guard);
    }
}

/// The streaming relay with interception on (spec § 5.2, D17, D24, D25).
///
/// 1. Each upstream chunk goes to the usage scanner first, exactly as in the plain relay, and
///    then to [`Interceptor::push`].
/// 2. On the first upstream `Err`: `finish_scan`, then [`Interceptor::abort`], which emits the
///    terminal frame. The relay does not emit it a second time.
/// 3. On the upstream end: `finish_scan`, then [`Interceptor::finish`].
/// 4. Decisions run one at a time (P-5).
/// 5. Nothing is spawned.
///
/// `prelude` holds the request-side `: paigasus-decision` comments of the tool policy (SMA-752
/// § 6.5.3). They go out before the first upstream frame.
#[allow(clippy::too_many_arguments)]
pub(crate) fn intercepted_stream(
    format: Format,
    limits: Limits,
    inner: OpenAiByteStream,
    guard: Option<ChargeGuard>,
    usage: StreamUsage,
    ti: Arc<ToolInterception>,
    ctx: Arc<DecisionContext>,
    turn: Arc<ToolTurn>,
    terminal: &'static [u8],
    prelude: Vec<Chunk>,
) -> impl Stream<Item = Result<Bytes, Infallible>> + Send + 'static {
    relay(format, limits, inner, guard, usage, ti, ctx, turn, terminal, prelude)
}

/// [`intercepted_stream`] for any wire format. The unit tests drive an Anthropic stream through
/// it, because only that format has more than one hold per stream (an OpenAI chat stream
/// carries tool calls in choice 0 only), and P-5 and the dropped stale decision are about the
/// second hold.
#[allow(clippy::too_many_arguments)]
fn relay(
    format: Format,
    limits: Limits,
    inner: OpenAiByteStream,
    guard: Option<ChargeGuard>,
    usage: StreamUsage,
    ti: Arc<ToolInterception>,
    ctx: Arc<DecisionContext>,
    turn: Arc<ToolTurn>,
    terminal: &'static [u8],
    prelude: Vec<Chunk>,
) -> impl Stream<Item = Result<Bytes, Infallible>> + Send + 'static {
    let now = Instant::now();
    let icpt = Interceptor::new(format, limits, Bytes::from_static(terminal), now);
    let sleep = Box::pin(tokio::time::sleep(limits.keepalive));
    let start = Relay {
        inner: Some(inner),
        scan: usage,
        guard,
        icpt,
        ti,
        ctx,
        turn,
        // The prelude is queued before any upstream chunk is read, so it goes out first.
        out: VecDeque::from(prelude),
        decide_queue: VecDeque::new(),
        pending: None,
        sleep,
    };
    futures::stream::unfold(start, |mut st| async move {
        loop {
            if let Some(b) = st.out.pop_front() {
                return Some((Ok(b), st));
            }
            // Checked before `select!`, so an elapsed timer after the end cannot spin.
            if st.icpt.is_finished() {
                st.drain_for_billing().await;
                return None;
            }
            st.start_next_decision();
            let acts = match st.next_event().await {
                Event::Decided(r) => {
                    let (hold, _) = st.pending.take().expect("the resolved decision was pending");
                    st.icpt.decided(hold, r, Instant::now())
                }
                Event::Wake => st.icpt.tick(Instant::now()),
                Event::Upstream(Some(Ok(chunk))) => {
                    feed_scan(&mut st.scan, &mut st.guard, &chunk);
                    st.icpt.push(&chunk, Instant::now())
                }
                Event::Upstream(Some(Err(_))) => {
                    finish_scan(&mut st.scan, &mut st.guard);
                    st.inner = None;
                    st.icpt.abort()
                }
                Event::Upstream(None) => {
                    finish_scan(&mut st.scan, &mut st.guard);
                    st.inner = None;
                    st.icpt.finish(Instant::now())
                }
            };
            st.apply(acts);
        }
    })
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use metrics_util::debugging::{DebugValue, DebuggingRecorder};

    use super::*;
    use crate::adapters::http::usage::UsageScanner;

    /// A metric name, its labels and its value.
    type Sample = (String, Vec<(String, String)>, DebugValue);

    /// P-9: every core metric lands on its registered name with its closed label set. One
    /// snapshot: the debugging recorder drains counters when it is read.
    #[test]
    fn core_metrics_map_to_the_registered_names() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _local = metrics::set_default_local_recorder(&recorder);
        record(Metric::RecordDropped("malformed"));
        record(Metric::Decision { decision: "block", basis: "decider" });
        record(Metric::OnFailure("hold_limit"));
        record(Metric::HoldSeconds(0.25));
        let mut seen: Vec<Sample> = snapshotter
            .snapshot()
            .into_vec()
            .into_iter()
            .map(|(k, _, _, v)| (k.key().name().to_owned(), k.key().labels().map(|l| (l.key().to_owned(), l.value().to_owned())).collect(), v))
            .collect();
        seen.sort_by(|a, b| a.0.cmp(&b.0));
        let labels = |pairs: &[(&str, &str)]| pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect::<Vec<_>>();
        assert_eq!(
            seen,
            vec![
                (
                    names::GATEWAY_TOOL_INTERCEPTION_DECISIONS_TOTAL.to_owned(),
                    labels(&[("decision", "block"), ("basis", "decider")]),
                    DebugValue::Counter(1)
                ),
                (names::GATEWAY_TOOL_INTERCEPTION_HOLD_SECONDS.to_owned(), labels(&[]), DebugValue::Histogram(vec![0.25.into()])),
                (names::GATEWAY_TOOL_INTERCEPTION_ON_FAILURE_TOTAL.to_owned(), labels(&[("code", "hold_limit")]), DebugValue::Counter(1)),
                (
                    names::GATEWAY_TOOL_INTERCEPTION_RECORDS_DROPPED_TOTAL.to_owned(),
                    labels(&[("reason", "malformed")]),
                    DebugValue::Counter(1)
                ),
            ]
        );
    }

    const TERMINAL: &[u8] = b"\n\ndata: {\"error\":\"terminal\"}\n\n";

    /// Two complete `tool_use` blocks: two holds, `Block(0)` and `Block(1)`, completed in order,
    /// with `gap` between the first block's end and the second block's start.
    fn two_tool_blocks(first: &str, second: &str, gap: Duration) -> OpenAiByteStream {
        tool_blocks(&[first, second], gap)
    }

    /// One complete `tool_use` block per name, with `gap` before the second one starts.
    fn tool_blocks(names: &[&str], gap: Duration) -> OpenAiByteStream {
        let ev = |name: &str, data: String| Ok(Bytes::from(format!("event: {name}\ndata: {data}\n\n")));
        let block = |i: u32, name: &str| {
            vec![
                ev(
                    "content_block_start",
                    format!(r#"{{"type":"content_block_start","index":{i},"content_block":{{"type":"tool_use","id":"toolu_{i}","name":"{name}","input":{{}}}}}}"#),
                ),
                ev(
                    "content_block_delta",
                    format!(r#"{{"type":"content_block_delta","index":{i},"delta":{{"type":"input_json_delta","partial_json":"{{\"p\":{i}}}"}}}}"#),
                ),
                ev("content_block_stop", format!(r#"{{"type":"content_block_stop","index":{i}}}"#)),
            ]
        };
        let mut chunks = vec![ev(
            "message_start",
            r#"{"type":"message_start","message":{"id":"msg_1","type":"message","role":"assistant","content":[],"model":"claude-x","stop_reason":null}}"#.to_owned(),
        )];
        let mut second_starts = usize::MAX;
        for (i, name) in names.iter().enumerate() {
            if i == 1 {
                second_starts = chunks.len();
            }
            chunks.extend(block(u32::try_from(i).expect("small"), name));
        }
        chunks.push(ev(
            "message_delta",
            r#"{"type":"message_delta","delta":{"stop_reason":"tool_use","stop_sequence":null},"usage":{"output_tokens":7}}"#.to_owned(),
        ));
        chunks.push(ev("message_stop", r#"{"type":"message_stop"}"#.to_owned()));
        futures::stream::iter(chunks.into_iter().enumerate())
            .then(move |(i, c)| async move {
                if i == second_starts {
                    tokio::time::sleep(gap).await;
                }
                c
            })
            .boxed()
    }

    async fn run(decider: Arc<dyn ToolCallDecider>, limits: Limits, upstream: OpenAiByteStream) -> String {
        let ti = Arc::new(ToolInterception { limits, decider, recorder: None });
        let ctx = Arc::new(DecisionContext::new("principal", "scope", None));
        run_ctx(ti, ctx, upstream).await
    }

    async fn run_ctx(ti: Arc<ToolInterception>, ctx: Arc<DecisionContext>, upstream: OpenAiByteStream) -> String {
        let turn = Arc::new(ToolTurn::Unavailable("unit test"));
        let out: Vec<Bytes> = relay(
            Format::AnthropicMessages,
            ti.limits,
            upstream,
            None,
            StreamUsage::Chat(UsageScanner::new()),
            ti,
            ctx,
            turn,
            TERMINAL,
            Vec::new(),
        )
        .map(|r| r.expect("infallible"))
        .collect()
        .await;
        String::from_utf8(out.concat()).expect("utf-8")
    }

    /// Allows everything after a short wait, and records the most decisions ever in flight.
    #[derive(Default)]
    struct SlowCounting {
        in_flight: Arc<AtomicUsize>,
        max_in_flight: Arc<AtomicUsize>,
        calls: Arc<AtomicUsize>,
    }

    impl ToolCallDecider for SlowCounting {
        fn decide(&self, _ctx: Arc<DecisionContext>, _turn: Arc<ToolTurn>, calls: Vec<ToolCall>) -> DecideFuture {
            let (in_flight, max, count) = (Arc::clone(&self.in_flight), Arc::clone(&self.max_in_flight), Arc::clone(&self.calls));
            Box::pin(async move {
                let now = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                max.fetch_max(now, Ordering::SeqCst);
                count.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(30)).await;
                in_flight.fetch_sub(1, Ordering::SeqCst);
                Ok(vec![DecidedCall::from(Decision::Allow); calls.len()])
            })
        }
    }

    /// Never answers for a call named `hang_*`; allows everything else at once.
    struct HangNamed;

    impl ToolCallDecider for HangNamed {
        fn decide(&self, _ctx: Arc<DecisionContext>, _turn: Arc<ToolTurn>, calls: Vec<ToolCall>) -> DecideFuture {
            if calls.iter().any(|c| c.name.starts_with("hang_")) {
                return Box::pin(std::future::pending());
            }
            Box::pin(std::future::ready(Ok(vec![DecidedCall::from(Decision::Allow); calls.len()])))
        }
    }

    /// SMA-730 R1 with interception on: a transport error after an unterminated usage record runs
    /// `finish_scan` before `abort()`, so the reported total is charged, and the core's terminal
    /// frame is the only one.
    #[tokio::test]
    async fn a_transport_error_after_an_unterminated_usage_record_charges_the_reported_tokens() {
        use crate::adapters::http::chat::TERMINAL_SSE_ERROR;
        use crate::adapters::http::chat::tests::{UNTERMINATED_USAGE, admit, make_reqwest_error, sent_guard};
        use crate::test_support::ScriptedStore;
        let store = ScriptedStore::new(admit);
        let upstream = futures::stream::iter(vec![Ok(Bytes::from_static(UNTERMINATED_USAGE))])
            .chain(futures::stream::once(async { Err(make_reqwest_error().await) }))
            .boxed();
        let ti = Arc::new(ToolInterception {
            limits: Limits::defaults(),
            decider: Arc::new(paigasus_gateway_core::AllowAll),
            recorder: None,
        });
        let ctx = Arc::new(DecisionContext::new("principal", "scope", None));
        let turn = Arc::new(ToolTurn::Unavailable("unit test"));
        let out: Vec<Bytes> = intercepted_stream(
            Format::OpenAiChat,
            ti.limits,
            upstream,
            Some(sent_guard(&store)),
            StreamUsage::Chat(UsageScanner::new()),
            ti,
            ctx,
            turn,
            TERMINAL_SSE_ERROR.as_bytes(),
            Vec::new(),
        )
        .map(|r| r.expect("infallible"))
        .collect()
        .await;
        let text = String::from_utf8(out.concat()).expect("utf-8");
        assert_eq!(text.matches(TERMINAL_SSE_ERROR.trim()).count(), 1, "{text}");
        assert_eq!(store.charges(), vec![7]);
    }

    /// P-5: two holds, decided one after the other, never both at once.
    #[tokio::test]
    async fn the_decider_runs_one_hold_at_a_time() {
        let decider = Arc::new(SlowCounting::default());
        let text = run(decider.clone(), Limits::defaults(), two_tool_blocks("read_file", "list_dir", Duration::ZERO)).await;
        assert!(text.contains(r#""decision":"allow","basis":"decider""#), "{text}");
        assert_eq!(decider.calls.load(Ordering::SeqCst), 2);
        assert_eq!(decider.max_in_flight.load(Ordering::SeqCst), 1);
    }

    /// When a timeout rules on the hold being decided, its future is dropped and the next
    /// hold is decided. The second hold completes 600 ms after the first, so its own deadline
    /// (1.6 s) lies 600 ms after the first one's (1 s): kept, the stale future would hold it until
    /// that deadline and it would be blocked too. Wide margins, for a loaded CI runner.
    #[tokio::test]
    async fn a_timed_out_decision_does_not_block_the_next_hold() {
        let limits = Limits {
            decide_timeout: Duration::from_secs(1),
            ..Limits::defaults()
        };
        let text = tokio::time::timeout(
            Duration::from_secs(5),
            run(Arc::new(HangNamed), limits, two_tool_blocks("hang_x", "read_file", Duration::from_millis(600))),
        )
        .await
        .expect("the stream ends");
        assert!(
            text.contains(r#""name":"hang_x","name_sha256_8""#) && text.contains(&format!(r#""code":"{}""#, BlockCode::DeciderError.as_kebab())),
            "{text}"
        );
        assert!(text.contains(r#""name":"read_file","name_sha256_8""#), "{text}");
        let read_file_report = text
            .lines()
            .find(|l| l.contains(r#""name":"read_file""#) && l.starts_with(": paigasus-decision"))
            .expect("a report for read_file");
        assert!(read_file_report.contains(r#""decision":"allow","basis":"decider""#), "{read_file_report}");
        assert!(text.contains(r#""name":"read_file","input":{}"#), "the allowed call is forwarded: {text}");
    }

    /// Records the hold start that the relay gives and the instant of the call, and allows
    /// everything after `wait`.
    struct HoldStarts {
        wait: Duration,
        seen: Arc<std::sync::Mutex<Vec<(Instant, Instant)>>>,
    }

    impl ToolCallDecider for HoldStarts {
        fn decide(&self, _ctx: Arc<DecisionContext>, _turn: Arc<ToolTurn>, _calls: Vec<ToolCall>) -> DecideFuture {
            unreachable!("the relay calls decide_by")
        }

        fn decide_by(&self, _ctx: Arc<DecisionContext>, _turn: Arc<ToolTurn>, calls: Vec<ToolCall>, hold_start: Instant) -> DecideFuture {
            self.seen.lock().expect("lock").push((hold_start, Instant::now()));
            let wait = self.wait;
            Box::pin(async move {
                tokio::time::sleep(wait).await;
                Ok(vec![DecidedCall::from(Decision::Allow); calls.len()])
            })
        }
    }

    /// SMA-855 D35, P-10: the second hold of a stream waits behind the first decision, and the
    /// relay still gives its own completion instant as the hold start, not the instant its
    /// decision starts. The first decision takes 300 ms; the assertion needs only 200 ms of the
    /// wait, so a loaded runner (which only makes the wait longer) cannot red it.
    #[tokio::test]
    async fn a_queued_hold_is_decided_by_its_own_completion() {
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let decider = Arc::new(HoldStarts {
            wait: Duration::from_millis(300),
            seen: Arc::clone(&seen),
        });
        let text = run(decider, Limits::defaults(), two_tool_blocks("read_file", "list_dir", Duration::ZERO)).await;
        assert!(text.contains(r#""name":"list_dir","input":{}"#), "{text}");
        let seen = seen.lock().expect("lock").clone();
        assert_eq!(seen.len(), 2, "two holds, two decide_by calls");
        let (hold_start, called) = seen[1];
        assert!(
            called.saturating_duration_since(hold_start) >= Duration::from_millis(200),
            "{:?}",
            called.saturating_duration_since(hold_start)
        );
    }

    /// Counts the decide calls of the Cedar decider.
    #[derive(Default)]
    struct CountingCedar(Arc<AtomicUsize>);

    impl ToolCallDecider for CountingCedar {
        fn decide(&self, ctx: Arc<DecisionContext>, turn: Arc<ToolTurn>, calls: Vec<ToolCall>) -> DecideFuture {
            self.0.fetch_add(1, Ordering::SeqCst);
            crate::application::tool_policy::CedarDecider::new(Arc::new(DecisionRecorder::tracing())).decide(ctx, turn, calls)
        }
    }

    /// Preflight F14: a stream with two holds decides twice and records ONE overhead sample, and
    /// that sample holds the request-side time plus both decide calls.
    #[tokio::test]
    async fn a_stream_with_two_holds_records_one_overhead_sample() {
        use crate::application::tool_policy::OverheadSample;
        use crate::application::tool_policy::testing::{self, FakeSource, ORG_A_PRN, PRINCIPAL, Snap, chat_request, settings};
        use paigasus_gateway_core::{TenantMode, parse_request};

        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _local = metrics::set_default_local_recorder(&recorder);
        let source = FakeSource::new(testing::FLOOR, testing::PERMIT_ALL);
        let policy = testing::policy(&source, settings(TenantMode::Enforce));
        let body = chat_request(&[], "");
        let turn = parse_request(Format::OpenAiChat, &body);
        let ctx = DecisionContext::new(PRINCIPAL, ORG_A_PRN, None);
        let out = policy.apply(&ctx, "m", Format::OpenAiChat, &turn, body).await.expect("apply");
        let snapshot = Arc::clone(&out.snapshot);
        let ctx = Arc::new(ctx.with_policy(out.snapshot));
        let calls = Arc::new(AtomicUsize::new(0));
        let ti = Arc::new(ToolInterception {
            limits: Limits::defaults(),
            decider: Arc::new(CountingCedar(Arc::clone(&calls))),
            recorder: None,
        });
        let s = relay(
            Format::AnthropicMessages,
            ti.limits,
            two_tool_blocks("read_file", "list_dir", Duration::ZERO),
            None,
            StreamUsage::Chat(UsageScanner::new()),
            ti,
            ctx,
            Arc::new(turn),
            TERMINAL,
            Vec::new(),
        );
        let s = keep_until_end(s, OverheadSample::new(Arc::clone(&snapshot)));
        let text = String::from_utf8(s.map(|r| r.expect("infallible")).collect::<Vec<Bytes>>().await.concat()).expect("utf-8");
        assert!(text.contains(r#""name":"list_dir","input":{}"#), "both calls are allowed: {text}");
        assert_eq!(calls.load(Ordering::SeqCst), 2, "two holds, two decide calls");
        let samples = Snap::take(&snapshotter).histogram(names::GATEWAY_TOOL_POLICY_OVERHEAD_SECONDS);
        assert_eq!(samples.len(), 1, "one sample per request, not per hold: {samples:?}");
        assert!(snapshot.decide_elapsed() > Duration::ZERO);
        assert_eq!(samples[0], snapshot.overhead().as_secs_f64(), "the request side plus both decide calls");
    }

    /// SMA-752 § 6.5.3: the prelude goes out before the first upstream frame.
    #[tokio::test]
    async fn the_prelude_goes_out_before_the_first_upstream_frame() {
        let ti = Arc::new(ToolInterception {
            limits: Limits::defaults(),
            decider: Arc::new(paigasus_gateway_core::AllowAll),
            recorder: None,
        });
        let ctx = Arc::new(DecisionContext::new("principal", "scope", None));
        let turn = Arc::new(ToolTurn::Unavailable("unit test"));
        let prelude = vec![Bytes::from_static(b": p1\n\n"), Bytes::from_static(b": p2\n\n")];
        let out: Vec<Bytes> = relay(
            Format::AnthropicMessages,
            ti.limits,
            two_tool_blocks("a", "b", Duration::ZERO),
            None,
            StreamUsage::Chat(UsageScanner::new()),
            ti,
            ctx,
            turn,
            TERMINAL,
            prelude,
        )
        .map(|r| r.expect("infallible"))
        .collect()
        .await;
        assert_eq!(&out[..2], [Bytes::from_static(b": p1\n\n"), Bytes::from_static(b": p2\n\n")]);
        assert!(String::from_utf8_lossy(&out[2]).starts_with("event: message_start"), "{:?}", out[2]);
    }

    // ---- the decision log (SMA-755) ------------------------------------------------------------

    use paigasus_gateway_core::decision_log::{AppliedEffect, DecisionRecord, RecordBasis, Side};
    use paigasus_kernel::tool_policy::Effect;

    use crate::application::decision_log::DigestKey;
    use crate::application::tool_policy::testing::{self, FakeSource, ORG_A_PRN, PRINCIPAL, chat_request, settings};
    use crate::domain::decision_log::{MemoryDecisionLog, OsEntropy};
    use crate::domain::limits::SystemClock;

    /// A recorder over a memory log. `key` gives it a digest key.
    fn memory_recorder(wants_digest: bool, key: bool) -> (Arc<MemoryDecisionLog>, Arc<DecisionRecorder>) {
        let log = Arc::new(MemoryDecisionLog::new(wants_digest));
        let key = key.then(|| DigestKey::new("k1".into(), "k".repeat(32).into()).expect("key"));
        let rec = Arc::new(DecisionRecorder::new(log.clone(), Arc::new(SystemClock), Arc::new(OsEntropy), key));
        (log, rec)
    }

    fn ti_with_recorder(limits: Limits, decider: Arc<dyn ToolCallDecider>, rec: &Arc<DecisionRecorder>) -> Arc<ToolInterception> {
        Arc::new(ToolInterception {
            limits,
            decider,
            recorder: Some(Arc::clone(rec)),
        })
    }

    /// Blocks a call named `rm`, allows the rest. It adds no detail.
    struct BlockRm;

    impl ToolCallDecider for BlockRm {
        fn decide(&self, _ctx: Arc<DecisionContext>, _turn: Arc<ToolTurn>, calls: Vec<ToolCall>) -> DecideFuture {
            Box::pin(std::future::ready(Ok(calls
                .iter()
                .map(|c| DecidedCall::from(if c.name == "rm" { Decision::Block { code: BlockCode::Policy } } else { Decision::Allow }))
                .collect())))
        }
    }

    /// The Cedar decider, after a short wait.
    struct SlowCedar(crate::application::tool_policy::CedarDecider);

    impl ToolCallDecider for SlowCedar {
        fn decide(&self, ctx: Arc<DecisionContext>, turn: Arc<ToolTurn>, calls: Vec<ToolCall>) -> DecideFuture {
            let inner = self.0.decide(ctx, turn, calls);
            Box::pin(async move {
                tokio::time::sleep(Duration::from_millis(100)).await;
                inner.await
            })
        }
    }

    fn plain_ctx() -> Arc<DecisionContext> {
        Arc::new(DecisionContext::new("principal", "scope", None))
    }

    /// A context with the request snapshot of the Cedar policy: the floor forbids `rm`.
    async fn cedar_ctx(mode: paigasus_gateway_core::TenantMode) -> Arc<DecisionContext> {
        use paigasus_gateway_core::{Format as Fmt, parse_request};
        let source = FakeSource::new(testing::FLOOR, testing::PERMIT_ALL);
        let policy = testing::policy(&source, settings(mode));
        let req = chat_request(&[], "");
        let turn = parse_request(Fmt::OpenAiChat, &req);
        let ctx = DecisionContext::new(PRINCIPAL, ORG_A_PRN, None);
        let out = policy.apply(&ctx, "m", Fmt::OpenAiChat, &turn, req).await.expect("apply");
        Arc::new(ctx.with_policy(out.snapshot))
    }

    fn names_of(records: &[DecisionRecord]) -> Vec<String> {
        records.iter().map(|r| r.tool.name.clone()).collect()
    }

    #[tokio::test]
    async fn the_stream_path_records_once_per_report_entry() {
        let (log, rec) = memory_recorder(false, false);
        let ti = ti_with_recorder(Limits::defaults(), Arc::new(BlockRm), &rec);
        let text = run_ctx(ti, plain_ctx(), tool_blocks(&["a", "rm", "b"], Duration::ZERO)).await;
        assert!(text.contains(r#""name":"b","input":{}"#), "{text}");
        let records = log.records();
        assert_eq!(records.len(), 3, "one record per report entry: {records:?}");
        assert_eq!(records.iter().map(|r| r.sequence).collect::<Vec<_>>(), [1, 2, 3]);
        assert_eq!(names_of(&records), ["a", "rm", "b"]);
        assert_eq!(
            records.iter().map(|r| r.applied).collect::<Vec<_>>(),
            [AppliedEffect::Allow, AppliedEffect::Block, AppliedEffect::Allow]
        );
        assert!(records.iter().all(|r| r.side == Side::Response));
    }

    #[tokio::test]
    async fn the_body_path_records_once_per_report_entry() {
        let (log, rec) = memory_recorder(false, false);
        let ti = ti_with_recorder(Limits::defaults(), Arc::new(BlockRm), &rec);
        let calls: Vec<serde_json::Value> = ["a", "rm", "b"]
            .iter()
            .enumerate()
            .map(|(i, n)| serde_json::json!({"id": format!("c{i}"), "type": "function", "function": {"name": n, "arguments": "{}"}}))
            .collect();
        let body = serde_json::json!({"object": "chat.completion", "choices": [{"index": 0, "message": {"role": "assistant", "tool_calls": calls}, "finish_reason": "tool_calls"}]}).to_string();
        let out = intercept_body(&ti, ti.limits, Format::OpenAiChat, plain_ctx(), Arc::new(ToolTurn::Unavailable("unit test")), body.as_bytes(), &[]).await;
        assert!(matches!(out, BodyOutcome::Forward { report: Some(_), .. }));
        let records = log.records();
        assert_eq!(records.len(), 3, "{records:?}");
        assert_eq!(records.iter().map(|r| r.sequence).collect::<Vec<_>>(), [1, 2, 3]);
        assert_eq!(names_of(&records), ["a", "rm", "b"]);
        assert_eq!(records[1].applied, AppliedEffect::Block);
    }

    #[tokio::test]
    async fn an_observed_hold_records_allow_with_a_cedar_block() {
        let (log, rec) = memory_recorder(false, false);
        let limits = Limits {
            on_failure: OnFailure::Allow,
            max_held_bytes: 330,
            ..Limits::defaults()
        };
        // The decider is slow: the frames after the call make the hold limit fire while the hold is deciding.
        let ti = ti_with_recorder(limits, Arc::new(SlowCedar(crate::application::tool_policy::CedarDecider::new(Arc::clone(&rec)))), &rec);
        let ctx = cedar_ctx(paigasus_gateway_core::TenantMode::Enforce).await;
        let text = run_ctx(ti, ctx, tool_blocks(&["rm"], Duration::ZERO)).await;
        assert!(text.contains(r#""name":"rm","input":{}"#), "an observed hold is released: {text}");
        let records = log.records();
        assert_eq!(records.len(), 1, "{records:?}");
        let r = &records[0];
        assert_eq!(r.applied, AppliedEffect::Observe);
        assert_eq!(r.block_code, Some(BlockCode::Policy));
        assert_eq!(r.basis, RecordBasis::OnFailure);
        assert_eq!(r.cedar.as_ref().map(|c| c.effect), Some(Effect::Deny));
    }

    #[tokio::test]
    async fn a_truncation_under_block_records_on_failure() {
        let (log, rec) = memory_recorder(false, false);
        let ti = ti_with_recorder(Limits::defaults(), Arc::new(paigasus_gateway_core::AllowAll), &rec);
        let ev = |name: &str, data: &str| Ok(Bytes::from(format!("event: {name}\ndata: {data}\n\n")));
        let chunks = vec![
            ev(
                "message_start",
                r#"{"type":"message_start","message":{"id":"msg_1","type":"message","role":"assistant","content":[],"model":"claude-x","stop_reason":null}}"#,
            ),
            ev(
                "content_block_start",
                r#"{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_0","name":"rm","input":{}}}"#,
            ),
            ev(
                "content_block_delta",
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"p\":"}}"#,
            ),
        ];
        let _ = run_ctx(ti, plain_ctx(), futures::stream::iter(chunks).boxed()).await;
        let records = log.records();
        assert!(!records.is_empty(), "the truncated call is recorded");
        for r in &records {
            assert_eq!((r.block_code, r.basis, r.applied), (Some(BlockCode::Truncated), RecordBasis::OnFailure, AppliedEffect::Block), "{r:?}");
            assert!(r.cedar.is_none());
        }
    }

    #[tokio::test]
    async fn a_hold_limit_block_has_no_cedar_detail() {
        let (log, rec) = memory_recorder(false, false);
        let limits = Limits {
            max_held_bytes: 1,
            ..Limits::defaults()
        };
        let ti = ti_with_recorder(limits, Arc::new(paigasus_gateway_core::AllowAll), &rec);
        let _ = run_ctx(ti, plain_ctx(), tool_blocks(&["x"], Duration::ZERO)).await;
        let records = log.records();
        assert!(!records.is_empty());
        for r in &records {
            assert_eq!((r.block_code, r.basis, r.applied), (Some(BlockCode::HoldLimit), RecordBasis::OnFailure, AppliedEffect::Block), "{r:?}");
            assert!(r.cedar.is_none());
        }
    }

    #[tokio::test]
    async fn tenant_off_records_a_floor_block() {
        use paigasus_gateway_core::TenantMode;
        let (log, rec) = memory_recorder(false, false);
        let ti = ti_with_recorder(Limits::defaults(), Arc::new(crate::application::tool_policy::CedarDecider::new(Arc::clone(&rec))), &rec);
        let _ = run_ctx(ti, cedar_ctx(TenantMode::Off).await, tool_blocks(&["rm"], Duration::ZERO)).await;
        let records = log.records();
        assert_eq!(records.len(), 1, "{records:?}");
        let r = &records[0];
        assert_eq!((r.applied, r.mode), (AppliedEffect::Block, Some(TenantMode::Off)));
        assert!(r.cedar.as_ref().is_some_and(|c| c.floor_denied), "{r:?}");
    }

    #[tokio::test]
    async fn an_org_less_scope_records_no_org() {
        let (log, rec) = memory_recorder(false, false);
        let ti = ti_with_recorder(Limits::defaults(), Arc::new(paigasus_gateway_core::AllowAll), &rec);
        let ctx = Arc::new(DecisionContext::new("principal", "prn:pgs:iam:::root/0190a1e5-0000-7000-8000-000000000004", None));
        let _ = run_ctx(ti, ctx, tool_blocks(&["a"], Duration::ZERO)).await;
        let records = log.records();
        assert_eq!(records.len(), 1);
        assert_eq!((records[0].org, records[0].instance_id, records[0].sequence), (None, rec.instance_id(), 1));
    }

    #[tokio::test]
    async fn the_digest_is_set_only_when_the_log_wants_it() {
        use paigasus_gateway_core::TenantMode;
        for wants in [true, false] {
            let (log, rec) = memory_recorder(wants, true);
            let ti = ti_with_recorder(Limits::defaults(), Arc::new(crate::application::tool_policy::CedarDecider::new(Arc::clone(&rec))), &rec);
            let _ = run_ctx(ti, cedar_ctx(TenantMode::Enforce).await, tool_blocks(&["ok"], Duration::ZERO)).await;
            let records = log.records();
            assert_eq!(records.len(), 1, "{records:?}");
            if wants {
                let d = records[0].arguments.as_ref().expect("a digest");
                assert_eq!(d.key_id, "k1");
                assert_eq!(Some(d), rec.digest(r#"{"p":0}"#).as_ref(), "the HMAC of the call arguments");
            } else {
                assert!(records[0].arguments.is_none(), "the log does not want it");
            }
        }
    }

    /// SMA-855 D35, S3 (Task 23, the Task 21 carry), end to end through the relay: two holds, the
    /// real Cedar decider with the detector pipeline, and a slow webhook that takes most of the
    /// hold budget. Only an Anthropic stream has two holds, and the gateway has no Anthropic route
    /// yet, so this test drives the relay itself. Time is paused, so the values are exact.
    ///
    /// Both holds complete at t = 0. The first run ends at 600 ms, inside its deadline of 750 ms.
    /// The second run starts at 600 ms and ends at the absolute deadline `hold start +
    /// decide_timeout - margin` = 750 ms: the decider gives `detector_error` (record basis
    /// `RecordBasis::Policy`), not `RecordBasis::OnFailure`. With a relative deadline (`now + timeout_ms` = 1350 ms) the relay would
    /// rule `on_failure` at 1000 ms. Keep-alive comments go out while the holds wait.
    #[tokio::test(start_paused = true)]
    async fn two_holds_with_a_slow_detector_use_the_absolute_deadline() {
        use crate::adapters::detectors::BuiltInDetector;
        use crate::application::detectors::DetectorPipeline;
        use crate::application::tool_policy::CedarDecider;
        use crate::application::tool_policy::testing::SlowDetector;
        use crate::config::{DETECTOR_MARGIN_MS, DetectorSettings};
        use paigasus_gateway_core::TenantMode;

        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _local = metrics::set_default_local_recorder(&recorder);
        let decide_timeout = Duration::from_secs(1);
        let settings = DetectorSettings {
            enabled: true,
            timeout: Duration::from_millis(750),
            max_scan_bytes: 16 << 20,
            margin: Duration::from_millis(DETECTOR_MARGIN_MS),
        };
        assert!(settings.timeout + settings.margin <= decide_timeout, "the config rule holds");
        let pipeline = Arc::new(DetectorPipeline::new(
            Arc::new(BuiltInDetector::new(settings.max_scan_bytes)),
            vec![Arc::new(SlowDetector("hook_slow", Duration::from_millis(600)))],
            None,
            settings.timeout,
        ));
        let (log, rec) = memory_recorder(false, false);
        let decider = CedarDecider::new(Arc::clone(&rec)).with_detectors(pipeline, decide_timeout, settings);
        let limits = Limits {
            decide_timeout,
            keepalive: Duration::from_millis(100),
            ..Limits::defaults()
        };
        let ti = ti_with_recorder(limits, Arc::new(decider), &rec);
        let text = run_ctx(ti, cedar_ctx(TenantMode::Enforce).await, tool_blocks(&["fetch_a", "fetch_b"], Duration::ZERO)).await;

        let records = log.records();
        assert_eq!(names_of(&records), ["fetch_a", "fetch_b"], "{records:?}");
        let has_error = |r: &DecisionRecord| r.cedar.as_ref().is_some_and(|c| c.findings.iter().any(|f| f.kind == "detector_error" && f.detector == "hook_slow"));
        assert!(records.iter().all(|r| r.basis == RecordBasis::Policy && r.applied == AppliedEffect::Allow), "{records:?}");
        assert!(!has_error(&records[0]), "the first hold finished inside its run: {:?}", records[0]);
        assert!(has_error(&records[1]), "the absolute deadline cut the second hold: {:?}", records[1]);
        assert!(text.contains(r#""name":"fetch_b","input":{}"#), "the second call is allowed and forwarded: {text}");
        assert!(!text.contains(r#""basis":"on_failure""#), "{text}");
        assert!(text.matches(": keep-alive\n\n").count() >= 2, "keep-alives while the holds wait: {text}");
        let on_failure = snapshotter
            .snapshot()
            .into_vec()
            .into_iter()
            .filter(|(k, _, _, _)| k.key().name() == names::GATEWAY_TOOL_INTERCEPTION_ON_FAILURE_TOTAL)
            .count();
        assert_eq!(on_failure, 0, "no on_failure increment");
    }
}
