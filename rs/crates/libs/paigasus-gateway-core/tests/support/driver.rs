// SPDX-License-Identifier: Apache-2.0
//! Drives an `Interceptor` synchronously, answering each `Decide` at once.

use std::time::Instant;

use bytes::Bytes;
use paigasus_gateway_core::{Action, BlockCode, Decision, Format, Interceptor, Limits, Metric, OnFailure, ToolCall};

pub const TERMINAL: &[u8] = b"\n\ndata: {\"error\":{\"message\":\"terminal\"}}\n\n";

pub struct Outcome {
    pub out: Vec<u8>,
    pub decided: Vec<(ToolCall, Decision)>,
    pub max_queued: usize,
    /// Whether the interceptor ended once the input was finished and every decision was in.
    pub finished: bool,
    pub metrics: Vec<Metric>,
}

impl Outcome {
    /// Whether the interceptor dropped a record or took an `on_failure` path. Its output may then
    /// be a stream that some client rejects, which is a fail-closed outcome for that client.
    pub fn core_failed(&self) -> bool {
        self.metrics.iter().any(|m| matches!(m, Metric::RecordDropped(_) | Metric::OnFailure(_)))
    }
}

pub fn limits(on_failure: OnFailure) -> Limits {
    Limits {
        max_record_bytes: 4096,
        max_held_bytes: 16 * 1024,
        on_failure,
        ..Limits::defaults()
    }
}

/// The block-by-name decider: every call named `blocked_*` is refused.
pub fn block_named(c: &ToolCall) -> Decision {
    if c.name.starts_with("blocked_") {
        Decision::Block { code: BlockCode::Policy }
    } else {
        Decision::Allow
    }
}

pub fn allow_all(_: &ToolCall) -> Decision {
    Decision::Allow
}

pub fn block_all(_: &ToolCall) -> Decision {
    Decision::Block { code: BlockCode::Policy }
}

pub type Decider = fn(&ToolCall) -> Decision;

/// The three deciders spec § 8.1 runs every generated stream under.
pub const DECIDERS: &[(&str, Decider)] = &[("block-by-name", block_named), ("block-all", block_all), ("allow-all", allow_all)];

pub fn drive(format: Format, limits: Limits, chunks: &[&[u8]], decide: &dyn Fn(&ToolCall) -> Decision) -> Outcome {
    let now = Instant::now();
    let mut i = Interceptor::new(format, limits, Bytes::from_static(TERMINAL), now);
    let mut o = Outcome {
        out: Vec::new(),
        decided: Vec::new(),
        max_queued: 0,
        finished: false,
        metrics: Vec::new(),
    };
    let mut pending = Vec::new();
    for c in chunks {
        pending.extend(i.push(c, now));
        run(&mut i, &mut pending, &mut o, decide, now);
    }
    pending.extend(i.finish(now));
    run(&mut i, &mut pending, &mut o, decide, now);
    o.finished = i.is_finished();
    o
}

fn run(i: &mut Interceptor, pending: &mut Vec<Action>, o: &mut Outcome, decide: &dyn Fn(&ToolCall) -> Decision, now: Instant) {
    while !pending.is_empty() {
        o.max_queued = o.max_queued.max(i.queued_bytes());
        for a in std::mem::take(pending) {
            match a {
                Action::Emit(b) => o.out.extend_from_slice(&b),
                Action::Decide { hold, calls } => {
                    let ds: Vec<Decision> = calls.iter().map(decide).collect();
                    o.decided.extend(calls.iter().cloned().zip(ds.iter().copied()));
                    pending.extend(i.decided(hold, Ok(ds), now));
                }
                Action::Count(m) => o.metrics.push(m),
                _ => {}
            }
        }
    }
    o.max_queued = o.max_queued.max(i.queued_bytes());
}

/// Split `bytes` at the given cut points, which are taken modulo the length and sorted.
pub fn cut<'a>(bytes: &'a [u8], cuts: &[usize]) -> Vec<&'a [u8]> {
    if bytes.is_empty() {
        return vec![bytes];
    }
    let mut at: Vec<usize> = cuts.iter().map(|c| c % bytes.len()).collect();
    at.sort_unstable();
    at.dedup();
    let mut out = Vec::new();
    let mut prev = 0;
    for a in at {
        out.push(&bytes[prev..a]);
        prev = a;
    }
    out.push(&bytes[prev..]);
    out
}
