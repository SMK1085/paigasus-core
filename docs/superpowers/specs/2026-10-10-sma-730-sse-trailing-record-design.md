# SMA-730 — gateway: dispatch the last SSE record when a stream ends without a blank line

- Linear: SMA-730 (follow-up of SMA-677, final review, Minor 6).
- Status: approved on 2026-10-11, after the spec challenge (§ 7). Bounded change.
- Scope: the crate `paigasus-gateway` only (`rs/crates/services/paigasus-gateway`).

## 1. Problem

`UsageScanner` (`src/adapters/http/usage.rs`) reads a copy of a forwarded SSE stream. It handles
a record only when a blank line ends the record (`end_line` with `line_len == 0` calls
`end_record`).

The relay `terminal_sse_error_stream` (`src/adapters/http/chat.rs`) does not tell the scanner
that the upstream stream ended. A last record with no blank line after it stays in the scanner
and is never dispatched.

If that record holds the usage object, the guard charges the estimate (request estimate plus one
token for each record) and not the reported total. OpenAI always ends a stream with a blank line.
Some OpenAI-compatible upstreams (SMA-558: vLLM, LiteLLM) possibly do not. We have no captured
stream that shows this. The change is defensive.

## 2. Decisions

| Id | Question | Decision |
|----|----------|----------|
| D1 | What `finish()` closes | The pending line, then the pending record, as `feed(b"\n\n")` does. This includes a last line with NO line terminator. |
| D2 | Where `finish()` is called | In the relay, when a guard exists, in two arms: the clean end (`None`) and the first upstream error (`Some(Err(_))`). |
| D3 | A dropped relay (client disconnect) | No `finish()`. The guard charges the progress of the records that a blank line closed. |
| D4 | The bytes to the client | No change (A5). `finish()` reads scanner state only. |
| D5 | An operator signal when `finish()` dispatches a record | None. No log line and no metric. |

Reasons:

- D1, billing: the usage object is the upstream's own count of the tokens that it produced. The
  gateway reads it when it arrived, whether or not the client dispatches the record. This does
  NOT follow WHATWG HTML § 9.2.6, which discards an event with no final blank line. That is on
  purpose: the standard is about event delivery, and the charge is about tokens produced. The
  console parser closes the last record the same way (`createChatStreamParser().end()` appends
  `\n\n`, `ts/apps/gateway-console/lib/chat-stream.ts`).
- D1, a truncated record: a record cut inside its JSON does not parse, so the scanner counts it as
  one completion record (the existing rule in `dispatch`). `data: [DO` also counts one record. The
  effect is one token, and the case needs a broken upstream. A "clean end" can also contain a cut
  record: an HTTP/1.x body framed only by the connection close ends with `None` after an upstream
  crash. D1 covers that case with the same rule.
- D2, the error arm: the terminal frame `TERMINAL_SSE_ERROR` starts with `\n\n` (SMA-635 § 4.6).
  So the relay itself closes the pending record, and the client receives it as a closed record.
  The charge then counts what the client receives. This also covers an upstream that sends a
  complete usage record with no blank line and then stops without closing: the stream path has
  only the idle `read_timeout` (`src/adapters/openai/client.rs`), so that stream ends in
  `Some(Err(_))`. Without `finish()` there, the reported total is lost.
- D2, no guard: when no guard exists (no `[limits]`), the scanner reads nothing today. That stays.
- D3: when the client disconnects, the relay is dropped and no arm runs. The client did not
  receive a closed last record, and the gateway cannot know if more bytes of it were coming. The
  estimate is the correct charge. A `Drop` on `Relay` that calls `finish()` is NOT allowed; such a
  `Drop` does not compile (E0509, because the relay closure destructures `Relay` by move), and test
  R2 fails for any other way to finish on drop.
- D5: YAGNI. A counter needs a new name in `names.rs`, priming and `repo:observability-drift`. The
  stream has no request ids (SMA-504 § 4.3), so a log line cannot be correlated. Open an issue if
  an operator asks for it.

## 3. Design

### 3.1 `UsageScanner::finish(&mut self)`

```rust
/// The upstream stream ended, or failed. Close a pending line and a pending record, as a blank
/// line does. A second call does nothing.
pub fn finish(&mut self) {
    self.feed(b"\n\n");
}
```

The body is `feed(b"\n\n")`, so the equivalence with a blank line is true by construction. No
state table is needed, and the function has no line that a mutation can remove with no effect.
A second call does nothing: after the first call the line and the record are empty, and a blank
line on an empty state dispatches nothing.

The `UsageScanner` doc comment gets one sentence: at the end of the stream, `finish()` closes the
last record, so "complete" also means "the stream ended after it".

### 3.2 The relay

In `terminal_sse_error_stream`, both arms call one helper before they return:

```rust
/// SMA-730: a last record with no blank line after it still counts.
fn finish_scan(scan: &mut UsageScanner, guard: &mut Option<ChargeGuard>) {
    if let Some(guard) = guard.as_mut() {
        scan.finish();
        guard.set_stream_progress(scan.completion_records(), scan.reported_total());
    }
}
```

- `Some(Err(_))`: call `finish_scan`, then yield the terminal frame (no change to the frame).
- `None`: call `finish_scan`, then return `None`. The closure owns `scan` and `guard`. Returning
  `None` drops them, and the guard's `Drop` charges with the new progress.

The `Relay` doc comment gets one sentence: the clean end and the first error call `finish()`; a
drop does not (D3).

## 4. Tests

Unit tests in `usage.rs`:

- U1: a usage record at the end of the stream, for each ending `""`, `"\n"`, `"\r"`, `"\r\n"`,
  gives `reported_total() == Some(N)` after `finish()`, and `None` before `finish()`.
- U2: these tails give these results after `finish()`:
  - `data: a` → 1 record.
  - `data: a\ndata: b` (a multi-line record, no terminator) → 1 record.
  - `data: a\n: keep-alive` (a comment after a data line) → 1 record.
  - `: keep-alive` → 0 records.
  - a usage record cut inside its JSON → 1 record, `reported_total() == None`.
  - `data: [DO` → 1 record.
- U3: `finish()` after a stream that ends with a blank line changes nothing, and a second
  `finish()` changes nothing.
- U4: an oversized partial record at the end counts one record after `finish()`.

Relay unit tests in `chat.rs`, with a real guard (`ChargeGuard::new` is `pub(crate)`;
`crate::test_support::ScriptedStore` records charges):

- R1 (D2, error arm): the upstream sends `usage_chunk(7)` with no trailing `\n\n`, then an error
  (`make_reqwest_error()`). The charge is 7. The output ends with `TERMINAL_SSE_ERROR`.
- R2 (D3): the upstream sends `usage_chunk(7)` with no trailing `\n\n`, then stays pending. The
  test reads that chunk, then drops the relay stream. The charge is the estimate, not 7.

Integration test in `tests/limits_http.rs`:

- I1: `a_stream_whose_last_usage_record_has_no_blank_line_charges_the_reported_tokens`. The exact
  fixture is
  `vec![chunk("Hel", true), chunk("lo", true), usage_chunk(7).trim_end_matches('\n').to_owned()]`
  (helpers in `tests/support/limits.rs`). The fixture has no `data: [DONE]`: after a record with
  no line terminator, `[DONE]` would join the same line. The test asserts:
  - the charge is `vec![7]` (without the fix it is `vec![1 + 2]`);
  - the client body is byte-identical to the concatenated fixture (D4).

Mutation checks (not only red-first): delete the `finish_scan` call in the `None` arm → I1
fails. Delete it in the `Some(Err(_))` arm → R1 fails. Add a `Drop` on `Relay` that calls
`finish()` → R2 fails. Each mutant must compile (`warnings = "deny"`).

## 5. Out of scope

- An operator signal (D5).
- The console parser. It already closes the last record.
- The non-stream path. It parses the whole body.

## 6. Risks

- R2 needs an upstream stream that stays pending. Use `futures::stream::pending()` chained after
  the chunk. No sleep and no timer.

## 7. Spec challenge changelog (2026-10-10)

Verdict: APPROVE WITH CHANGES. No BLOCKER.

Folded in:

- MAJOR, D3 reason wrong: the old D3 kept the error arm without `finish()`. The challenger showed
  that the terminal frame closes the pending record for the client, and that an idle timeout ends
  a complete-but-unterminated stream in the error arm. Both claims checked in the code. New D2
  calls `finish()` in the error arm.
- MAJOR, no test enforces D2-D4: added R1, R2, the byte-identity assertion in I1, and the
  mutation checks.
- MINOR, equivalent mutants in `finish()`: the body is now `self.feed(b"\n\n")`.
- MINOR, missing tails: added U2.
- MINOR, D1 reason relied on the console and contradicted WHATWG: the reason is now billing, and
  the WHATWG difference is stated.
- MINOR, I1 fixture not exact: the fixture is now written out, with the reason for no `[DONE]`.
- MINOR, a clean end can hold a cut record: one sentence added to D1.
- MINOR, two doc comments incomplete: both updates are in § 3.
- QUESTION, captured stream: none. § 1 now says the change is defensive.
- QUESTION, disconnect with an open upstream: intended. Now D3, with test R2.

Rejected:

- QUESTION, operator signal: rejected as D5 (YAGNI, cost of a new metric).
- MINOR (optional), a consuming `finish(self)`: not used. The `&mut self` form is simpler at the
  two call sites, and a second call is already harmless.
