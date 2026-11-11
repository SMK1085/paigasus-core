// SPDX-License-Identifier: Apache-2.0
//! The interceptor: one stream's ordering, limits and failure handling (spec D7, D10–D17, § 5.3,
//! § 6).
//!
//! Synchronous and I/O-free (spec D1). The caller feeds bytes and decisions, passes the current
//! instant to every call, and performs the returned actions in order. The interceptor never
//! reads a clock and never spawns anything.

use std::collections::{BTreeMap, VecDeque};
use std::time::Instant;

use bytes::Bytes;

use crate::anthropic::stream::AnthropicStream;
use crate::limits::{Limits, OnFailure};
use crate::openai::stream::OpenAiStream;
use crate::protocol::{Frame, Protocol, Step, StepError, Verdict, Verdicts};
use crate::report::{ReportDecision, ReportEntry, report_comment};
use crate::sse::{self, SseItem, SseSplitter};
use crate::types::{Basis, BlockCode, Decision, Format, HoldKey, ToolCall};

/// The decider failed, timed out, or broke its contract (D18).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("tool-call decider failed: {reason}")]
pub struct DeciderError {
    pub reason: &'static str,
}

/// A counter or histogram sample for the gateway to record (P-9). Labels are closed sets.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Metric {
    /// `reason` = `malformed` | `oversized` | `after_done` | `late_fragment`.
    RecordDropped(&'static str),
    /// `decision` = `allow` | `block`; `basis` = `decider` | `on_failure`.
    Decision { decision: &'static str, basis: &'static str },
    /// `code` = `hold_limit` | `truncated` | `decider_error` | `malformed`.
    OnFailure(&'static str),
    /// Seconds from a hold's first frame to its completion.
    HoldSeconds(f64),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Bytes for the client, in order.
    Emit(Bytes),
    /// Ask the decider about one completed hold; answer with [`Interceptor::decided`].
    Decide {
        hold: HoldKey,
        calls: Vec<ToolCall>,
    },
    /// Call [`Interceptor::tick`] no later than this instant.
    Wake(Instant),
    Count(Metric),
}

#[derive(Clone)]
enum Track {
    Assembling { opened: Instant, observe: bool },
    Deciding { since: Instant, calls: Vec<ToolCall>, observe: bool },
    Decided,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Normal,
    Failed(BlockCode),
    PassThrough,
}

pub struct Interceptor {
    proto: Box<dyn Protocol>,
    sse: SseSplitter,
    limits: Limits,
    terminal: Bytes,
    queue: VecDeque<Frame>,
    queued: usize,
    verdicts: Verdicts,
    holds: BTreeMap<HoldKey, Track>,
    mode: Mode,
    after_end: bool,
    input_done: bool,
    ended: bool,
    last_emit: Instant,
    out: Vec<Action>,
}

fn choice_of(k: HoldKey) -> u32 {
    match k {
        HoldKey::Choice(c) => c,
        HoldKey::Block(_) => 0,
    }
}

impl Interceptor {
    #[must_use]
    pub fn new(format: Format, limits: Limits, terminal: Bytes, now: Instant) -> Self {
        let proto: Box<dyn Protocol> = match format {
            Format::OpenAiChat => Box::new(OpenAiStream::default()),
            Format::AnthropicMessages => Box::new(AnthropicStream::default()),
        };
        Interceptor {
            proto,
            sse: SseSplitter::new(limits.max_record_bytes),
            limits,
            terminal,
            queue: VecDeque::new(),
            queued: 0,
            verdicts: Verdicts::new(),
            holds: BTreeMap::new(),
            mode: Mode::Normal,
            after_end: false,
            input_done: false,
            ended: false,
            last_emit: now,
            out: Vec::new(),
        }
    }

    /// Feed upstream bytes.
    pub fn push(&mut self, chunk: &[u8], now: Instant) -> Vec<Action> {
        if self.ended || self.input_done {
            return Vec::new();
        }
        for item in self.sse.push(chunk) {
            self.item(item, now);
            if self.ended {
                break;
            }
        }
        self.settle(now)
    }

    /// The upstream ended cleanly. The stream ends once nothing is queued or deciding.
    pub fn finish(&mut self, now: Instant) -> Vec<Action> {
        if self.ended || self.input_done {
            return Vec::new();
        }
        for item in self.sse.finish() {
            self.item(item, now);
            if self.ended {
                break;
            }
        }
        self.input_done = true;
        if !self.ended {
            self.truncate_open();
        }
        self.settle(now)
    }

    /// The upstream failed (D17): drop the queue, emit the terminal frame, and end.
    pub fn abort(&mut self) -> Vec<Action> {
        if self.ended {
            return Vec::new();
        }
        self.queue.clear();
        self.queued = 0;
        self.ended = true;
        let mut out = std::mem::take(&mut self.out);
        out.push(Action::Emit(self.terminal.clone()));
        out
    }

    /// The decider's answer for `hold`. An answer for a hold that is no longer deciding is ignored.
    pub fn decided(&mut self, hold: HoldKey, result: Result<Vec<Decision>, DeciderError>, now: Instant) -> Vec<Action> {
        if self.ended {
            return Vec::new();
        }
        self.apply_decision(hold, result, now);
        self.settle(now)
    }

    /// Enforce the deadlines (D14) and send a keep-alive when one is due (D15).
    pub fn tick(&mut self, now: Instant) -> Vec<Action> {
        if self.ended {
            return Vec::new();
        }
        let due: Vec<(HoldKey, bool)> = self
            .holds
            .iter()
            .filter_map(|(k, t)| match t {
                Track::Assembling { opened, observe: false } if now.saturating_duration_since(*opened) >= self.limits.max_hold => Some((*k, false)),
                Track::Deciding { since, .. } if now.saturating_duration_since(*since) >= self.limits.decide_timeout => Some((*k, true)),
                _ => None,
            })
            .collect();
        for (k, deciding) in due {
            if deciding {
                self.apply_decision(k, Err(DeciderError { reason: "timeout" }), now);
            } else {
                self.count(Metric::OnFailure("hold_limit"));
                self.hold_limit(k);
            }
        }
        if self.holding() && now.saturating_duration_since(self.last_emit) >= self.limits.keepalive {
            self.emit(Bytes::from_static(sse::KEEP_ALIVE), now);
        }
        self.settle(now)
    }

    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.ended
    }

    /// Whether `hold` still waits for its decision. The relay drops a decider future whose hold
    /// is no longer deciding, for example after a timeout.
    #[must_use]
    pub fn is_deciding(&self, hold: HoldKey) -> bool {
        matches!(self.holds.get(&hold), Some(Track::Deciding { .. }))
    }

    #[doc(hidden)]
    #[must_use]
    pub fn queued_bytes(&self) -> usize {
        self.queued
    }

    // ---- input ---------------------------------------------------------------------------------

    fn item(&mut self, item: SseItem, now: Instant) {
        // D4: after the end marker every record is dropped, an oversized one included. It must not
        // become a stream failure that rules on a hold still waiting for the decider.
        if self.after_end {
            self.count(Metric::RecordDropped("after_done"));
            return;
        }
        let rec = match item {
            SseItem::Oversized => {
                self.count(Metric::RecordDropped("oversized"));
                self.stream_failure(now);
                return;
            }
            SseItem::Record(r) => r,
        };
        if self.mode == Mode::PassThrough {
            let b = self.proto.unframed(rec);
            self.emit(b, now);
            return;
        }
        match self.proto.step(rec, &self.verdicts) {
            Err(StepError::Malformed) => {
                self.count(Metric::RecordDropped("malformed"));
                if !self.queue.is_empty() {
                    self.stream_failure(now);
                }
            }
            Err(StepError::Deviation(why)) => {
                self.count(Metric::RecordDropped(if why == "late_fragment" { "late_fragment" } else { "malformed" }));
                self.stream_failure(now);
            }
            Ok(Step::UpstreamError(frame)) => {
                if !self.queue.is_empty() || self.any_open() {
                    let acts = self.abort();
                    self.out.extend(acts);
                } else {
                    self.enqueue(frame, now);
                }
            }
            Ok(Step::End(frame)) => {
                self.after_end = true;
                self.truncate_open();
                self.enqueue(frame, now);
            }
            Ok(Step::Frame { frame, opened, completed, reports }) => {
                for r in &reports {
                    self.emit(report_comment(r), now);
                }
                for k in opened {
                    self.open(k, now);
                }
                self.enqueue(frame, now);
                for (k, calls) in completed {
                    self.complete(k, calls, now);
                }
            }
        }
    }

    fn open(&mut self, k: HoldKey, now: Instant) {
        match self.mode {
            Mode::Failed(code) => self.block_all(k, code),
            _ => {
                self.holds.insert(k, Track::Assembling { opened: now, observe: false });
            }
        }
    }

    fn complete(&mut self, k: HoldKey, calls: Vec<ToolCall>, now: Instant) {
        if let Some(Track::Assembling { opened, observe }) = self.holds.get(&k).cloned() {
            self.count(Metric::HoldSeconds(now.saturating_duration_since(opened).as_secs_f64()));
            self.holds.insert(
                k,
                Track::Deciding {
                    since: now,
                    calls: calls.clone(),
                    observe,
                },
            );
            self.out.push(Action::Decide { hold: k, calls });
        }
    }

    fn enqueue(&mut self, frame: Frame, now: Instant) {
        if self.queue.is_empty() && self.releasable(&frame) {
            for b in self.proto.release(frame, &self.verdicts) {
                self.emit(b, now);
            }
            return;
        }
        self.queued += frame.payload.len();
        self.queue.push_back(frame);
        if self.queued > self.limits.max_held_bytes {
            let over = self.undecided();
            if !over.is_empty() {
                self.count(Metric::OnFailure("hold_limit"));
                for k in over {
                    self.hold_limit(k);
                }
            }
            // The limited holds have verdicts now; release at once so the queue stays bounded
            // within one push.
            self.release_ready(now);
        }
    }

    // ---- decisions and failures ----------------------------------------------------------------

    fn apply_decision(&mut self, hold: HoldKey, result: Result<Vec<Decision>, DeciderError>, now: Instant) {
        let Some(Track::Deciding { calls, observe, .. }) = self.holds.get(&hold).cloned() else { return };
        let checked = match result {
            Ok(d) if d.len() == calls.len() => Ok(d),
            Ok(_) => Err(DeciderError { reason: "wrong_length" }),
            Err(e) => Err(e),
        };
        let (verdict, decisions, mut basis) = match (checked, self.limits.on_failure) {
            (Ok(d), _) => {
                let v = if d.iter().all(|d| d.is_allow()) { Verdict::AllowAll } else { Verdict::PerCall(d.clone()) };
                (v, d, Basis::Decider)
            }
            (Err(_), OnFailure::Block) => {
                self.count(Metric::OnFailure("decider_error"));
                (
                    Verdict::BlockAll(BlockCode::DeciderError),
                    vec![Decision::Block { code: BlockCode::DeciderError }; calls.len()],
                    Basis::OnFailure,
                )
            }
            (Err(_), OnFailure::Allow) => {
                self.count(Metric::OnFailure("decider_error"));
                (Verdict::AllowAll, vec![Decision::Allow; calls.len()], Basis::OnFailure)
            }
        };
        if observe {
            basis = Basis::OnFailure;
        }
        for (c, d) in calls.iter().zip(&decisions) {
            let (decision, code) = match d {
                Decision::Block { code } => (ReportDecision::Block, Some(*code)),
                _ => (ReportDecision::Allow, None),
            };
            let label = if decision == ReportDecision::Block { "block" } else { "allow" };
            self.count(Metric::Decision {
                decision: label,
                basis: basis.as_str(),
            });
            let entry = ReportEntry {
                choice: c.choice,
                position: c.position,
                name: c.name.clone(),
                decision,
                basis: Some(basis),
                code,
            };
            self.emit(report_comment(&entry), now);
        }
        if !observe {
            self.verdicts.insert(hold, verdict);
        }
        self.holds.insert(hold, Track::Decided);
    }

    /// D13/D14: one hold over a limit.
    fn hold_limit(&mut self, k: HoldKey) {
        match self.limits.on_failure {
            OnFailure::Block => self.block_all(k, BlockCode::HoldLimit),
            OnFailure::Allow => self.observe(k),
        }
    }

    /// A hold blocked as a whole, with one report per call known so far.
    fn block_all(&mut self, k: HoldKey, code: BlockCode) {
        self.verdicts.insert(k, Verdict::BlockAll(code));
        self.holds.insert(k, Track::Decided);
        let known = self.proto.known_calls(k);
        let known = if known.is_empty() { vec![(0, "unknown".to_owned())] } else { known };
        for (position, name) in known {
            let entry = ReportEntry {
                choice: choice_of(k),
                position,
                name,
                decision: ReportDecision::Block,
                basis: Some(Basis::OnFailure),
                code: Some(code),
            };
            self.count(Metric::Decision {
                decision: "block",
                basis: Basis::OnFailure.as_str(),
            });
            self.out.push(Action::Emit(report_comment(&entry)));
        }
    }

    /// Observe mode: released as is, still decided for the report once complete (§ 5.3).
    fn observe(&mut self, k: HoldKey) {
        self.verdicts.insert(k, Verdict::PassThrough);
        let next = match self.holds.get(&k) {
            Some(Track::Assembling { opened, .. }) => Track::Assembling { opened: *opened, observe: true },
            Some(Track::Deciding { since, calls, .. }) => Track::Deciding {
                since: *since,
                calls: calls.clone(),
                observe: true,
            },
            _ => return,
        };
        self.holds.insert(k, next);
    }

    fn stream_failure(&mut self, now: Instant) {
        if self.mode != Mode::Normal {
            return;
        }
        self.count(Metric::OnFailure("malformed"));
        match self.limits.on_failure {
            OnFailure::Block => {
                self.mode = Mode::Failed(BlockCode::Malformed);
                for k in self.undecided() {
                    self.block_all(k, BlockCode::Malformed);
                }
            }
            OnFailure::Allow => {
                self.mode = Mode::PassThrough;
                for k in self.undecided() {
                    self.observe(k);
                }
                // The queued frames go out before any later record is forwarded unclassified.
                self.release_ready(now);
                self.emit(sse::comment("paigasus-interception pass-through"), now);
            }
        }
    }

    /// D10/D11: the stream ended with these holds still assembling.
    fn truncate_open(&mut self) {
        for k in self.proto.incomplete() {
            match self.holds.get(&k) {
                Some(Track::Assembling { observe: false, .. }) => {
                    self.count(Metric::OnFailure("truncated"));
                    match self.limits.on_failure {
                        OnFailure::Block => self.block_all(k, BlockCode::Truncated),
                        OnFailure::Allow => {
                            self.verdicts.insert(k, Verdict::PassThrough);
                            self.holds.insert(k, Track::Decided);
                        }
                    }
                }
                Some(Track::Assembling { observe: true, .. }) => {
                    self.holds.insert(k, Track::Decided);
                }
                _ => {}
            }
        }
    }

    // ---- output --------------------------------------------------------------------------------

    /// D12: pop from the head while every hold the head touches has a verdict.
    fn release_ready(&mut self, now: Instant) {
        while let Some(front) = self.queue.front() {
            if !self.releasable(front) {
                break;
            }
            let frame = self.queue.pop_front().expect("front exists");
            self.queued -= frame.payload.len();
            for b in self.proto.release(frame, &self.verdicts) {
                self.emit(b, now);
            }
        }
    }

    fn settle(&mut self, now: Instant) -> Vec<Action> {
        if !self.ended {
            self.release_ready(now);
            if self.input_done && self.queue.is_empty() && !self.any_deciding() {
                for b in self.proto.closing(&self.verdicts) {
                    self.emit(b, now);
                }
                self.ended = true;
            }
        }
        if !self.ended {
            let at = self.next_wake(now);
            self.out.push(Action::Wake(at));
        }
        std::mem::take(&mut self.out)
    }

    fn releasable(&self, frame: &Frame) -> bool {
        frame.touches.iter().all(|k| self.verdicts.contains_key(k))
    }

    fn emit(&mut self, b: Bytes, now: Instant) {
        self.out.push(Action::Emit(b));
        self.last_emit = now;
    }

    fn count(&mut self, m: Metric) {
        self.out.push(Action::Count(m));
    }

    /// The next deadline. With nothing held no keep-alive is owed, so the base moves with `now`
    /// and a wake never lies in the past.
    fn next_wake(&self, now: Instant) -> Instant {
        let mut at = if self.holding() { self.last_emit + self.limits.keepalive } else { now + self.limits.keepalive };
        for t in self.holds.values() {
            let d = match t {
                Track::Assembling { opened, observe: false } => *opened + self.limits.max_hold,
                Track::Deciding { since, .. } => *since + self.limits.decide_timeout,
                _ => continue,
            };
            at = at.min(d);
        }
        at
    }

    /// D15: a keep-alive is owed while the queue is non-empty or a hold is deciding.
    fn holding(&self) -> bool {
        !self.queue.is_empty() || self.any_deciding()
    }

    fn undecided(&self) -> Vec<HoldKey> {
        self.holds
            .iter()
            .filter(|(_, t)| matches!(t, Track::Assembling { observe: false, .. } | Track::Deciding { observe: false, .. }))
            .map(|(k, _)| *k)
            .collect()
    }

    fn any_open(&self) -> bool {
        self.holds.values().any(|t| matches!(t, Track::Assembling { .. } | Track::Deciding { .. }))
    }

    fn any_deciding(&self) -> bool {
        self.holds.values().any(|t| matches!(t, Track::Deciding { .. }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::limits::OnFailure;
    use crate::types::{BlockCode, Decision};
    use serde_json::{Value, json};
    use std::time::Duration;

    const TERMINAL: &[u8] = b"\n\ndata: {\"error\":\"terminal\"}\n\n";

    fn limits() -> Limits {
        Limits {
            max_record_bytes: 4096,
            max_held_bytes: 8192,
            max_hold: Duration::from_secs(300),
            decide_timeout: Duration::from_secs(5),
            keepalive: Duration::from_secs(10),
            on_failure: OnFailure::Block,
        }
    }
    fn data(v: Value) -> String {
        format!("data: {v}\n\n")
    }
    fn text(c: u32, s: &str) -> String {
        data(json!({"id":"x","object":"chat.completion.chunk","choices":[{"index":c,"delta":{"content":s},"finish_reason":null}]}))
    }
    fn call(c: u32, i: u32, name: &str, args: &str) -> String {
        data(
            json!({"id":"x","object":"chat.completion.chunk","choices":[{"index":c,"delta":{"tool_calls":[{"index":i,"id":format!("call_{c}_{i}"),"type":"function","function":{"name":name,"arguments":args}}]},"finish_reason":null}]}),
        )
    }
    fn args(c: u32, i: u32, a: &str) -> String {
        data(json!({"id":"x","object":"chat.completion.chunk","choices":[{"index":c,"delta":{"tool_calls":[{"index":i,"function":{"arguments":a}}]},"finish_reason":null}]}))
    }
    fn finish(c: u32) -> String {
        data(json!({"id":"x","object":"chat.completion.chunk","choices":[{"index":c,"delta":{},"finish_reason":"tool_calls"}]}))
    }
    fn ev(v: Value) -> String {
        format!("event: {}\ndata: {v}\n\n", v["type"].as_str().unwrap())
    }
    fn tool_start(i: u32, name: &str) -> String {
        ev(json!({"type":"content_block_start","index":i,"content_block":{"type":"tool_use","id":format!("t{i}"),"name":name,"input":{}}}))
    }
    fn json_delta(i: u32, pj: &str) -> String {
        ev(json!({"type":"content_block_delta","index":i,"delta":{"type":"input_json_delta","partial_json":pj}}))
    }
    fn block_stop(i: u32) -> String {
        ev(json!({"type":"content_block_stop","index":i}))
    }
    fn emitted(acts: &[Action]) -> String {
        acts.iter()
            .filter_map(|a| if let Action::Emit(b) = a { Some(String::from_utf8_lossy(b).into_owned()) } else { None })
            .collect()
    }

    /// Run a whole stream. `decide` answers each Decide at once; `None` leaves it pending.
    struct Run {
        out: String,
        decides: Vec<(HoldKey, Vec<ToolCall>)>,
        counts: Vec<Metric>,
    }
    type Decider<'a> = dyn Fn(&[ToolCall]) -> Option<Result<Vec<Decision>, DeciderError>> + 'a;
    fn run_format(format: Format, limits: Limits, chunks: &[String], decide: &Decider<'_>) -> (Run, Interceptor) {
        let t0 = Instant::now();
        let mut i = Interceptor::new(format, limits, Bytes::from_static(TERMINAL), t0);
        let mut r = Run {
            out: String::new(),
            decides: Vec::new(),
            counts: Vec::new(),
        };
        let mut pending: Vec<Action> = Vec::new();
        for c in chunks {
            pending.extend(i.push(c.as_bytes(), t0));
            drain(&mut i, &mut pending, &mut r, decide, t0);
        }
        pending.extend(i.finish(t0));
        drain(&mut i, &mut pending, &mut r, decide, t0);
        (r, i)
    }
    fn run_with(limits: Limits, chunks: &[String], decide: impl Fn(&[ToolCall]) -> Option<Result<Vec<Decision>, DeciderError>>) -> Run {
        run_format(Format::OpenAiChat, limits, chunks, &decide).0
    }
    fn drain(i: &mut Interceptor, pending: &mut Vec<Action>, r: &mut Run, decide: &Decider<'_>, now: Instant) {
        while !pending.is_empty() {
            for a in std::mem::take(pending) {
                match a {
                    Action::Emit(b) => r.out.push_str(std::str::from_utf8(&b).unwrap()),
                    Action::Decide { hold, calls } => {
                        r.decides.push((hold, calls.clone()));
                        if let Some(res) = decide(&calls) {
                            pending.extend(i.decided(hold, res, now));
                        }
                    }
                    Action::Count(m) => r.counts.push(m),
                    Action::Wake(_) => {}
                }
            }
        }
    }
    fn allow(c: &[ToolCall]) -> Option<Result<Vec<Decision>, DeciderError>> {
        Some(Ok(vec![Decision::Allow; c.len()]))
    }
    fn block_named(prefix: &'static str) -> impl Fn(&[ToolCall]) -> Option<Result<Vec<Decision>, DeciderError>> {
        move |c| {
            Some(Ok(c
                .iter()
                .map(|t| {
                    if t.name.starts_with(prefix) {
                        Decision::Block { code: BlockCode::Policy }
                    } else {
                        Decision::Allow
                    }
                })
                .collect()))
        }
    }

    #[test]
    fn text_before_any_hold_streams_at_once() {
        let t0 = Instant::now();
        let mut i = Interceptor::new(Format::OpenAiChat, limits(), Bytes::from_static(TERMINAL), t0);
        let acts = i.push(text(0, "hi").as_bytes(), t0);
        assert!(matches!(&acts[0], Action::Emit(b) if b.starts_with(b"data: {")));
    }

    #[test]
    fn allow_all_preserves_every_payload() {
        let chunks = vec![text(0, "a"), call(0, 0, "read", "{}"), text(0, "b"), finish(0), "data: [DONE]\n\n".into()];
        let r = run_with(limits(), &chunks, allow);
        let payloads: Vec<&str> = r.out.split("\n\n").filter(|f| f.starts_with("data: ")).collect();
        let want: Vec<String> = chunks.iter().map(|c| c.trim_end().to_owned()).collect();
        assert_eq!(payloads, want);
        assert!(r.out.contains(": paigasus-decision {"));
    }

    #[test]
    fn a_blocked_call_never_reaches_the_output() {
        let chunks = vec![call(0, 0, "rm_all", "{\"p\":"), args(0, 0, "\"CANARY\"}"), finish(0), "data: [DONE]\n\n".into()];
        let r = run_with(limits(), &chunks, block_named("rm"));
        assert!(!r.out.contains("CANARY"), "{}", r.out);
        assert!(r.out.contains("[paigasus]"));
    }

    #[test]
    fn a_frame_waits_for_every_hold_it_touches() {
        // Tool calls live only in OpenAI choice 0 (multi_choice_tool_calls), so the two-hold case
        // is an Anthropic stream: block 0 completes, block 1 sends a whole call that sits behind
        // it, and the message_delta touches both. Deciding block 0 must not ship block 1's frames
        // or the shared message_delta.
        let t0 = Instant::now();
        let mut i = Interceptor::new(Format::AnthropicMessages, limits(), Bytes::from_static(TERMINAL), t0);
        for c in [
            tool_start(0, "read"),
            json_delta(0, "{}"),
            block_stop(0),
            tool_start(1, "rm"),
            json_delta(1, "\"CANARY\""),
            block_stop(1),
        ] {
            i.push(c.as_bytes(), t0);
        }
        i.push(
            ev(json!({"type":"message_delta","delta":{"stop_reason":"tool_use","stop_sequence":null},"usage":{"output_tokens":9}})).as_bytes(),
            t0,
        );
        let acts = i.decided(HoldKey::Block(0), Ok(vec![Decision::Allow]), t0);
        let out = emitted(&acts);
        assert!(out.contains("\"read\""), "block 0's frames are released: {out}");
        assert!(!out.contains("CANARY"), "block 1's undecided call stays queued: {out}");
        assert!(!out.contains("message_delta"), "a frame touching an undecided hold stays queued: {out}");
        let out = emitted(&i.decided(HoldKey::Block(1), Ok(vec![Decision::Block { code: BlockCode::Policy }]), t0));
        assert!(!out.contains("CANARY") && out.contains("[paigasus]") && out.contains("message_delta"), "{out}");
    }

    #[test]
    fn eof_with_an_open_hold_is_truncated_not_complete() {
        let chunks = vec![call(0, 0, "read", "{\"CANARY\":")];
        let r = run_with(limits(), &chunks, allow);
        assert!(r.decides.is_empty(), "a truncated hold is never decided as complete");
        assert!(!r.out.contains("CANARY"));
        assert!(r.out.contains("(truncated)"));
        assert!(r.counts.contains(&Metric::OnFailure("truncated")));
    }

    #[test]
    fn done_with_an_open_hold_is_truncated_and_later_records_are_dropped() {
        let chunks = vec![call(0, 0, "read", "CANARY"), "data: [DONE]\n\n".into(), call(0, 1, "late", "LATE")];
        let r = run_with(limits(), &chunks, allow);
        assert!(!r.out.contains("CANARY") && !r.out.contains("LATE"));
        assert!(r.out.ends_with("data: [DONE]\n\n"), "{}", r.out);
        assert!(r.counts.contains(&Metric::RecordDropped("after_done")));
    }

    #[test]
    fn an_oversized_record_after_done_is_dropped_as_after_done() {
        // D4: everything after the end marker is dropped and counted, oversized or not. It must
        // not become a stream failure that rules on a hold the decider still owns.
        let t0 = Instant::now();
        let mut i = Interceptor::new(Format::OpenAiChat, limits(), Bytes::from_static(TERMINAL), t0);
        let mut acts = i.push(call(0, 0, "rm", "X1").as_bytes(), t0);
        acts.extend(i.push(finish(0).as_bytes(), t0));
        acts.extend(i.push(b"data: [DONE]\n\n", t0));
        acts.extend(i.push(args(0, 0, &"x".repeat(5000)).as_bytes(), t0));
        assert!(i.is_deciding(HoldKey::Choice(0)), "the hold is still the decider's");
        assert!(acts.contains(&Action::Count(Metric::RecordDropped("after_done"))), "{acts:?}");
        assert!(!acts.contains(&Action::Count(Metric::RecordDropped("oversized"))), "{acts:?}");
        assert!(!acts.contains(&Action::Count(Metric::OnFailure("malformed"))), "{acts:?}");
        let out = emitted(&acts) + &emitted(&i.decided(HoldKey::Choice(0), Ok(vec![Decision::Block { code: BlockCode::Policy }]), t0));
        assert!(!out.contains("(malformed)") && !out.contains("pass-through"), "{out}");
        assert!(out.contains("(policy)") && !out.contains("X1"), "{out}");
        assert!(out.ends_with("data: [DONE]\n\n"), "{out}");
    }

    #[test]
    fn records_after_message_stop_are_dropped() {
        let chunks = vec![
            ev(json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}})),
            ev(json!({"type":"content_block_stop","index":0})),
            ev(json!({"type":"message_stop"})),
            tool_start(1, "late"),
            json_delta(1, "\"LATE\""),
        ];
        let (r, i) = run_format(Format::AnthropicMessages, limits(), &chunks, &allow);
        assert!(!r.out.contains("LATE") && !r.out.contains("late"), "{}", r.out);
        assert!(r.out.ends_with("event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"), "{}", r.out);
        assert_eq!(r.counts.iter().filter(|m| **m == Metric::RecordDropped("after_done")).count(), 2);
        assert!(r.decides.is_empty());
        assert!(i.is_finished());
    }

    #[test]
    fn abort_drops_the_queue_and_emits_the_terminal_frame() {
        let t0 = Instant::now();
        let mut i = Interceptor::new(Format::OpenAiChat, limits(), Bytes::from_static(TERMINAL), t0);
        i.push(call(0, 0, "read", "CANARY").as_bytes(), t0);
        let acts = i.abort();
        assert_eq!(acts, vec![Action::Emit(Bytes::from_static(TERMINAL))]);
        assert!(i.is_finished());
        assert!(i.push(text(0, "x").as_bytes(), t0).is_empty());
    }

    #[test]
    fn an_upstream_error_record_mid_hold_aborts() {
        let chunks = vec![call(0, 0, "read", "CANARY"), data(json!({"error":{"message":"boom"}}))];
        let r = run_with(limits(), &chunks, allow);
        assert_eq!(r.out.as_bytes(), TERMINAL);
    }

    #[test]
    fn an_upstream_error_record_with_nothing_held_is_forwarded() {
        let chunks = vec![text(0, "a"), data(json!({"error":{"message":"boom"}}))];
        let r = run_with(limits(), &chunks, allow);
        assert!(r.out.contains("\"boom\""));
    }

    #[test]
    fn a_short_decision_vector_blocks_every_call() {
        let chunks = vec![call(0, 0, "a", "X1"), call(0, 1, "b", "X2"), finish(0)];
        let r = run_with(limits(), &chunks, |_| Some(Ok(vec![])));
        assert!(!r.out.contains("X1") && !r.out.contains("X2"), "{}", r.out);
        assert!(r.out.contains("(decider-error)"));
    }

    #[test]
    fn a_decider_error_follows_on_failure() {
        let chunks = vec![call(0, 0, "a", "X1"), finish(0)];
        let err = |_: &[ToolCall]| Some(Err(DeciderError { reason: "boom" }));
        let blocked = run_with(limits(), &chunks, err);
        assert!(!blocked.out.contains("X1"));
        let allowed = run_with(
            Limits {
                on_failure: OnFailure::Allow,
                ..limits()
            },
            &chunks,
            err,
        );
        assert!(allowed.out.contains("X1"));
        assert!(allowed.out.contains("\"basis\":\"on_failure\""));
    }

    #[test]
    fn a_malformed_record_with_an_empty_queue_is_dropped() {
        let chunks = vec![text(0, "a"), "data: {\"choices\":NaN}\n\n".into(), text(0, "b")];
        let r = run_with(limits(), &chunks, allow);
        assert!(!r.out.contains("NaN"));
        assert!(r.out.contains("\"b\""));
        assert!(r.counts.contains(&Metric::RecordDropped("malformed")));
    }

    #[test]
    fn a_malformed_record_mid_hold_blocks_the_stream_or_passes_it_through() {
        let chunks = vec![call(0, 0, "read", "X1"), "data: {\"a\":1,\"a\":2}\n\n".into(), call(0, 1, "late", "X2"), finish(0)];
        let blocked = run_with(limits(), &chunks, allow);
        assert!(!blocked.out.contains("X1") && !blocked.out.contains("X2"), "{}", blocked.out);
        assert!(blocked.out.contains("(malformed)"));
        let observed = run_with(
            Limits {
                on_failure: OnFailure::Allow,
                ..limits()
            },
            &chunks,
            allow,
        );
        assert!(observed.out.contains("X1") && observed.out.contains("X2"));
        assert!(observed.out.contains(": paigasus-interception pass-through"));
    }

    #[test]
    fn pass_through_keeps_the_queued_frames_before_later_records() {
        // D8: the queued frames are released at the switch, before any later record of the same
        // push goes out unclassified.
        let chunks = [
            tool_start(0, "read"),
            json_delta(0, "{\"a\":"),
            "event: ping\ndata: {\"type\":\"ping\",\"type\":\"ping\"}\n\n".into(),
            json_delta(0, "1}"),
            block_stop(0),
        ];
        let t0 = Instant::now();
        let mut i = Interceptor::new(
            Format::AnthropicMessages,
            Limits {
                on_failure: OnFailure::Allow,
                ..limits()
            },
            Bytes::from_static(TERMINAL),
            t0,
        );
        let out = emitted(&i.push(chunks.concat().as_bytes(), t0));
        let start = out.find("content_block_start").expect("start released");
        let first = out.find("{\\\"a\\\":").expect("first delta released");
        let pass = out.find(": paigasus-interception pass-through").expect("switch reported");
        let second = out.find("1}").expect("later delta forwarded");
        assert!(start < first && first < pass && pass < second, "{out}");
    }

    #[test]
    fn an_oversized_record_is_a_stream_failure() {
        let big = args(0, 0, &"x".repeat(5000));
        let r = run_with(limits(), &[call(0, 0, "first", "X1"), big], allow);
        assert!(r.counts.contains(&Metric::RecordDropped("oversized")));
        assert!(!r.out.contains("X1"));
    }

    #[test]
    fn the_queue_never_exceeds_the_cap_by_more_than_one_record() {
        let t0 = Instant::now();
        let l = Limits { max_held_bytes: 600, ..limits() };
        let mut i = Interceptor::new(Format::OpenAiChat, l, Bytes::from_static(TERMINAL), t0);
        let mut acts = i.push(call(0, 0, "read", "{").as_bytes(), t0);
        for _ in 0..50 {
            acts.extend(i.push(args(0, 0, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").as_bytes(), t0));
            assert!(i.queued_bytes() <= 600 + 200, "{}", i.queued_bytes());
        }
        assert!(acts.contains(&Action::Count(Metric::OnFailure("hold_limit"))));
    }

    #[test]
    fn a_hold_past_max_hold_is_blocked_on_tick() {
        let t0 = Instant::now();
        let mut i = Interceptor::new(Format::OpenAiChat, limits(), Bytes::from_static(TERMINAL), t0);
        i.push(call(0, 0, "read", "X1").as_bytes(), t0);
        let acts = i.tick(t0 + Duration::from_secs(301));
        assert!(acts.contains(&Action::Count(Metric::OnFailure("hold_limit"))));
        let acts = i.push(finish(0).as_bytes(), t0 + Duration::from_secs(302));
        let out = emitted(&acts);
        assert!(!out.contains("X1") && out.contains("(hold-limit)"), "{out}");
    }

    #[test]
    fn a_slow_decider_times_out_on_tick() {
        let t0 = Instant::now();
        let mut i = Interceptor::new(Format::OpenAiChat, limits(), Bytes::from_static(TERMINAL), t0);
        i.push(call(0, 0, "read", "X1").as_bytes(), t0);
        i.push(finish(0).as_bytes(), t0);
        let out = emitted(&i.tick(t0 + Duration::from_secs(6)));
        assert!(out.contains("(decider-error)"), "{out}");
        // The late answer is ignored.
        assert!(
            i.decided(HoldKey::Choice(0), Ok(vec![Decision::Allow]), t0 + Duration::from_secs(7))
                .iter()
                .all(|a| !matches!(a, Action::Emit(_)))
        );
    }

    #[test]
    fn keep_alives_flow_while_holding_and_wake_is_always_set() {
        let t0 = Instant::now();
        let mut i = Interceptor::new(Format::OpenAiChat, limits(), Bytes::from_static(TERMINAL), t0);
        let acts = i.push(call(0, 0, "read", "{").as_bytes(), t0);
        assert!(acts.iter().any(|a| matches!(a, Action::Wake(w) if *w == t0 + Duration::from_secs(10))));
        let acts = i.tick(t0 + Duration::from_secs(10));
        assert!(acts.contains(&Action::Emit(Bytes::from_static(crate::sse::KEEP_ALIVE))));
        let acts = i.tick(t0 + Duration::from_secs(11));
        assert!(!acts.contains(&Action::Emit(Bytes::from_static(crate::sse::KEEP_ALIVE))), "one per interval");
    }

    #[test]
    fn an_idle_stream_never_wakes_in_the_past() {
        // D7: with nothing held, no keep-alive is sent, so the wake must move forward from `now`.
        let t0 = Instant::now();
        let mut i = Interceptor::new(Format::OpenAiChat, limits(), Bytes::from_static(TERMINAL), t0);
        let now = t0 + Duration::from_secs(10);
        let acts = i.tick(now);
        assert!(!acts.iter().any(|a| matches!(a, Action::Emit(_))), "{acts:?}");
        let wakes: Vec<Instant> = acts.iter().filter_map(|a| if let Action::Wake(w) = a { Some(*w) } else { None }).collect();
        assert_eq!(wakes.len(), 1, "{acts:?}");
        assert!(wakes[0] > now);
    }

    #[test]
    fn is_deciding_tracks_one_hold() {
        let t0 = Instant::now();
        let mut i = Interceptor::new(Format::OpenAiChat, limits(), Bytes::from_static(TERMINAL), t0);
        let k = HoldKey::Choice(0);
        assert!(!i.is_deciding(k));
        i.push(call(0, 0, "read", "{}").as_bytes(), t0);
        assert!(!i.is_deciding(k), "assembling is not deciding");
        i.push(finish(0).as_bytes(), t0);
        assert!(i.is_deciding(k));
        i.tick(t0 + Duration::from_secs(6));
        assert!(!i.is_deciding(k), "a timed-out hold is no longer deciding");
    }

    #[test]
    fn finish_waits_for_a_pending_decision() {
        let t0 = Instant::now();
        let mut i = Interceptor::new(Format::OpenAiChat, limits(), Bytes::from_static(TERMINAL), t0);
        i.push(call(0, 0, "read", "{}").as_bytes(), t0);
        i.push(finish(0).as_bytes(), t0);
        i.finish(t0);
        assert!(!i.is_finished());
        i.decided(HoldKey::Choice(0), Ok(vec![Decision::Allow]), t0);
        assert!(i.is_finished());
    }

    #[test]
    fn anthropic_streams_are_driven_the_same_way() {
        let chunks = [tool_start(0, "rm"), json_delta(0, "\"CANARY\""), block_stop(0), ev(json!({"type":"message_stop"}))];
        let (r, i) = run_format(Format::AnthropicMessages, limits(), &chunks, &block_named("rm"));
        assert!(!r.out.contains("CANARY"));
        assert!(r.out.contains("[paigasus]"));
        assert!(i.is_finished());
    }

    /// F3: a record that deviates after it would open a hold (its first fragment, or a later
    /// choice of the same record) must leave no hold behind that the interceptor never registered.
    /// Such a hold would wait for a verdict for ever, and the stream would only keep alive.
    #[test]
    fn a_deviation_while_opening_a_hold_still_ends_the_stream() {
        let o = |v: Value| data(json!({"id":"x","object":"chat.completion.chunk","choices":v}));
        let gap = o(json!([{"index":0,"delta":{"tool_calls":[{"index":9,"id":"c","type":"function","function":{"name":"blocked_x","arguments":"{}"}}]}}]));
        let later_choice = o(json!([
            {"index":0,"delta":{"tool_calls":[{"index":0,"id":"c","type":"function","function":{"name":"read","arguments":"{}"}}]}},
            {"index":1,"delta":{"tool_calls":[{"index":0,"id":"d","type":"function","function":{"name":"rm","arguments":"{}"}}]}}
        ]));
        let stop = o(json!([{"index":0,"delta":{"content":"hi"},"finish_reason":"stop"}]));
        for first in [gap, later_choice] {
            for on_failure in [OnFailure::Block, OnFailure::Allow] {
                let chunks = [first.clone(), stop.clone(), "data: [DONE]\n\n".to_owned()];
                let l = Limits { on_failure, ..limits() };
                let (r, mut i) = run_format(Format::OpenAiChat, l, &chunks, &allow);
                let later = Instant::now() + Duration::from_secs(400);
                let tail = emitted(&i.tick(later));
                assert!(i.is_finished(), "{on_failure:?}: the stream never ended:\n{}{tail}", r.out);
                assert!(r.out.contains("\"hi\"") && r.out.ends_with("data: [DONE]\n\n"), "{on_failure:?}:\n{}", r.out);
                assert!(!r.out.contains("blocked_x\",\"arguments") && !r.out.contains("\"rm\""), "{on_failure:?}:\n{}", r.out);
            }
        }
    }

    /// F3 for Anthropic: a stop rejected as `incomplete_arguments` leaves the block open, so the
    /// blocked block still gets its notice and its stop frame.
    #[test]
    fn a_rejected_stop_still_closes_the_blocked_block() {
        let chunks = [tool_start(0, "rm"), json_delta(0, "{\"a\":"), block_stop(0), ev(json!({"type":"message_stop"}))];
        let (r, i) = run_format(Format::AnthropicMessages, limits(), &chunks, &allow);
        assert!(i.is_finished(), "{}", r.out);
        assert!(r.out.contains("(malformed)") && r.out.contains("{\"type\":\"content_block_stop\",\"index\":0}"), "{}", r.out);
        assert!(r.out.find("content_block_stop") < r.out.find("message_stop"), "{}", r.out);
    }
}
