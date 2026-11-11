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
    Action, Basis, BlockCode, DecideFuture, DeciderError, Decision, DecisionContext, Format, HoldKey, Interceptor, Limits, Metric, OnFailure, ToolCall, ToolCallDecider, ToolTurn, parse_body,
};
use paigasus_observability::names;

use crate::adapters::http::chat::{feed_scan, finish_scan};
use crate::adapters::http::usage::UsageScanner;
use crate::adapters::openai::OpenAiByteStream;
use crate::application::charge_guard::ChargeGuard;

/// What `[tool_interception]` builds at boot.
pub struct ToolInterception {
    pub limits: Limits,
    pub decider: Arc<dyn ToolCallDecider>,
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

/// Parse, decide and rewrite one complete 2xx body. The usage read happened on the ORIGINAL body
/// before this runs (D24), so nothing here changes the charge.
pub(crate) async fn intercept_body(ti: &ToolInterception, ctx: Arc<DecisionContext>, turn: Arc<ToolTurn>, body: &[u8]) -> BodyOutcome {
    // D15: the body cap is `max_held_bytes` (spec § 5.2).
    let calls = match parse_body(Format::OpenAiChat, body, ti.limits.max_held_bytes) {
        Ok(c) => c,
        Err(_) => {
            record(Metric::OnFailure(BlockCode::Malformed.as_kebab()));
            return match ti.limits.on_failure {
                OnFailure::Block => BodyOutcome::Refuse,
                OnFailure::Allow => BodyOutcome::Forward { rewritten: None, report: None },
            };
        }
    };
    if calls.calls.is_empty() && calls.unenforceable.is_empty() {
        return BodyOutcome::Forward { rewritten: None, report: None };
    }
    let n = calls.calls.len();
    let (decisions, basis) = if n == 0 {
        (Vec::new(), Basis::Decider)
    } else {
        let fut = ti.decider.decide(ctx, turn, calls.calls.clone());
        match tokio::time::timeout(ti.limits.decide_timeout, fut).await {
            Ok(Ok(d)) if d.len() == n => (d, Basis::Decider),
            _ => {
                record(Metric::OnFailure("decider_error"));
                match ti.limits.on_failure {
                    OnFailure::Block => (vec![Decision::Block { code: BlockCode::DeciderError }; n], Basis::OnFailure),
                    OnFailure::Allow => (vec![Decision::Allow; n], Basis::OnFailure),
                }
            }
        }
    };
    for d in &decisions {
        let decision = if d.is_allow() { "allow" } else { "block" };
        record(Metric::Decision { decision, basis: basis.as_str() });
    }
    let report = calls.report(&decisions, basis);
    let rewritten = calls.apply(&decisions).map(Bytes::from);
    BodyOutcome::Forward { rewritten, report: Some(report) }
}

/// The unfold state. It owns the upstream, the charge guard and the pending decision, so a
/// client disconnect drops all three (D25), exactly as the plain relay drops the first two.
struct Relay {
    /// `None` once the upstream has ended, failed, or been drained.
    inner: Option<OpenAiByteStream>,
    scan: UsageScanner,
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
    Decided(Result<Vec<Decision>, DeciderError>),
    Wake,
    Upstream(Option<Result<Bytes, reqwest::Error>>),
}

impl Relay {
    fn apply(&mut self, actions: Vec<Action>) {
        for a in actions {
            match a {
                Action::Emit(b) => self.out.push_back(b),
                Action::Decide { hold, calls } => self.decide_queue.push_back((hold, calls)),
                // D7: the core never returns a wake in the past, so this cannot spin.
                Action::Wake(at) => self.sleep.as_mut().reset(tokio::time::Instant::from_std(at)),
                Action::Count(m) => record(m),
            }
        }
        // D22: a hold that is no longer deciding (a timeout ruled on it) needs no answer. Drop
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
                let fut = self.ti.decider.decide(Arc::clone(&self.ctx), Arc::clone(&self.turn), calls);
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

    /// D23: the interceptor ended the client stream (an upstream error record mid-hold) while the
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
pub(crate) fn intercepted_stream(
    inner: OpenAiByteStream,
    guard: Option<ChargeGuard>,
    ti: Arc<ToolInterception>,
    ctx: Arc<DecisionContext>,
    turn: Arc<ToolTurn>,
    terminal: &'static [u8],
) -> impl Stream<Item = Result<Bytes, Infallible>> + Send + 'static {
    relay(Format::OpenAiChat, inner, guard, ti, ctx, turn, terminal)
}

/// [`intercepted_stream`] for any wire format. Only OpenAI chat is wired in SMA-749 (D26); the
/// unit tests drive an Anthropic stream through it, because only that format has more than one
/// hold per stream (an OpenAI stream carries tool calls in choice 0 only), and P-5 and D22 are
/// about the second hold.
fn relay(
    format: Format,
    inner: OpenAiByteStream,
    guard: Option<ChargeGuard>,
    ti: Arc<ToolInterception>,
    ctx: Arc<DecisionContext>,
    turn: Arc<ToolTurn>,
    terminal: &'static [u8],
) -> impl Stream<Item = Result<Bytes, Infallible>> + Send + 'static {
    let now = Instant::now();
    let icpt = Interceptor::new(format, ti.limits, Bytes::from_static(terminal), now);
    let sleep = Box::pin(tokio::time::sleep(ti.limits.keepalive));
    let start = Relay {
        inner: Some(inner),
        scan: UsageScanner::new(),
        guard,
        icpt,
        ti,
        ctx,
        turn,
        out: VecDeque::new(),
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

    /// A metric name, its labels and its value.
    type Sample = (String, Vec<(String, String)>, DebugValue);

    /// P-9: every core metric lands on its registered name with its closed label set. One
    /// snapshot: the debugging recorder drains counters when it is read.
    #[test]
    fn core_metrics_map_to_the_registered_names() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _local = metrics::set_default_local_recorder(&recorder);
        record(Metric::RecordDropped(BlockCode::Malformed.as_kebab()));
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
                    labels(&[("reason", BlockCode::Malformed.as_kebab())]),
                    DebugValue::Counter(1)
                ),
            ]
        );
    }

    const TERMINAL: &[u8] = b"\n\ndata: {\"error\":\"terminal\"}\n\n";

    /// Two complete `tool_use` blocks: two holds, `Block(0)` and `Block(1)`, completed in order,
    /// with `gap` between the first block's end and the second block's start.
    fn two_tool_blocks(first: &str, second: &str, gap: Duration) -> OpenAiByteStream {
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
        chunks.extend(block(0, first));
        let second_starts = chunks.len();
        chunks.extend(block(1, second));
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
        let ti = Arc::new(ToolInterception { limits, decider });
        let ctx = Arc::new(DecisionContext::new("principal", "scope", None));
        let turn = Arc::new(ToolTurn::Unavailable("unit test"));
        let out: Vec<Bytes> = relay(Format::AnthropicMessages, upstream, None, ti, ctx, turn, TERMINAL)
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
                Ok(vec![Decision::Allow; calls.len()])
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
            Box::pin(std::future::ready(Ok(vec![Decision::Allow; calls.len()])))
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
        });
        let ctx = Arc::new(DecisionContext::new("principal", "scope", None));
        let turn = Arc::new(ToolTurn::Unavailable("unit test"));
        let out: Vec<Bytes> = intercepted_stream(upstream, Some(sent_guard(&store)), ti, ctx, turn, TERMINAL_SSE_ERROR.as_bytes())
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

    /// D22: when a timeout rules on the hold being decided, its future is dropped and the next
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
}
