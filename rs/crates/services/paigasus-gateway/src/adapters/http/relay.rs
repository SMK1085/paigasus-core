// SPDX-License-Identifier: Apache-2.0

//! The stream relay helpers that every OpenAI-shaped endpoint shares (SMA-769 D6): the plain
//! relay with its mid-stream terminal frame, and the usage feed that the intercepting relay uses
//! too. They do not depend on an endpoint; the endpoint gives its terminal frame and its usage
//! reader.
//!
//! ## Mid-stream terminal SSE error (spec §5)
//! Once a stream's `200 OK` head has been sent and `data:` frames are flowing, the HTTP status can
//! no longer change — so a mid-stream upstream failure cannot become a `502`. Instead the stream is
//! adapted so that on the FIRST inner error it emits one terminal SSE event and then ends. The
//! adapted stream is returned DIRECTLY (never spawned onto a detached task) so reqwest's
//! cancel-on-drop stays intact: if the downstream client disconnects, axum drops the response
//! body, which drops the inner reqwest stream and cancels the upstream request (G8 proves this
//! abort).

use std::convert::Infallible;

use axum::body::Bytes;
use futures::{Stream, StreamExt};

use crate::adapters::http::usage::StreamUsage;
use crate::adapters::openai::OpenAiByteStream;
use crate::application::charge_guard::ChargeGuard;

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
/// and, on the FIRST upstream error, emits one `terminal` event and ends.
///
/// Items are `Result<Bytes, Infallible>`: the outgoing stream never errors (a mid-stream upstream
/// failure becomes a data event, not a transport error), which keeps `Body::from_stream` from
/// aborting the response body. The inner stream is owned by the unfold state and dropped when this
/// stream is dropped — preserving reqwest's cancel-on-drop of the upstream request.
///
/// Runs with `paigasus_observability::current_ids() == None` (SMA-504 spec §4.3, situation 3):
/// this function is polled by hyper after the request-head future — the one the
/// `CorrelationLayer` task-local scope covers — has already resolved. See the call site's
/// comment in [`pipeline::proxy`](super::pipeline::proxy).
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
