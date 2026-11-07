# SMA-730 SSE trailing record — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The gateway charges the reported usage when the last SSE record of a stream has no blank line after it.

**Architecture:** `UsageScanner::finish()` closes the pending line and record, as `feed(b"\n\n")` does. The relay `terminal_sse_error_stream` calls a helper `finish_scan` in its clean-end arm and in its first-error arm, when a guard exists. A dropped relay (client disconnect) does not call it.

**Tech Stack:** Rust 2024, tokio, futures, axum, `cargo nextest`, Moon.

**Spec:** `docs/superpowers/specs/2026-10-10-sma-730-sse-trailing-record-design.md` (approved 2026-10-11). Read it before Task 1.

## Global Constraints

- Change only the crate `rs/crates/services/paigasus-gateway`.
- The bytes forwarded to the client must not change (A5, spec D4).
- No new log line, no new metric (spec D5).
- No `Drop` impl on `Relay` (spec D3).
- The workspace has `warnings = "deny"`: dead code is a compile error.
- Run all cargo commands from `rs/` with `--locked`. Put the proto shims first on `PATH`: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.
- Commit scope is `rs` (commitlint `scope-enum`). End each commit message with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Do not put a `#NNN` line in the commit body.
- If the commit fails with "failed to fill whole buffer", 1Password is locked. Stop and report. Do not disable signing.
- Do not use `git commit --amend`, `git reset`, `git stash` or `git checkout -- <file>`.
- Do not install host software (`brew`, `cargo install`).
- Match the local style: doc comments in short sentences, tests named as a sentence in snake case.

## Review Focus

1. A usage record, then `data: [DONE]` with no blank line after `[DONE]`: `finish()` dispatches `[DONE]`, which must not clear or change the reported total. Test: Task 1, `finish_after_an_unterminated_done_keeps_the_usage`.
2. A stream that ends exactly on a lone `\r` split in its own chunk: the `after_cr` state must not lose the record. Test: Task 1, U1 feeds `"\r"` as a separate chunk.
3. The no-guard relay (no `[limits]`) with an unterminated last record: the bytes stay identical and nothing panics. Test: Task 2, `the_relay_forwards_an_unterminated_tail_unchanged_without_a_guard`.
4. An upstream error with no pending record: the charge does not change. The existing test `a_mid_stream_error_charges_exactly_once` (`tests/limits_http.rs`) covers it. Task 3 runs it.
5. A comment line after the last data line, with no blank line: the record still counts once. Test: Task 1, U2 row `data: a\n: keep-alive`.

---

### Task 1: `UsageScanner::finish()`

**Files:**
- Modify: `rs/crates/services/paigasus-gateway/src/adapters/http/usage.rs` (struct doc at lines 78-81, `impl UsageScanner` at lines 99-185, test module from line 209)

**Interfaces:**
- Consumes: nothing new.
- Produces: `pub fn finish(&mut self)` on `UsageScanner`. After the call, `completion_records()` and `reported_total()` include the last record. A second call does nothing.

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `usage.rs`, after `an_oversized_record_counts_once_and_is_not_buffered`:

```rust
    fn finished(chunks: &[&str]) -> UsageScanner {
        let mut scanner = scan(chunks);
        scanner.finish();
        scanner
    }

    /// SMA-730 U1: a usage record with no blank line after it is read at the end of the stream,
    /// with every line ending, also a lone CR in its own chunk.
    #[test]
    fn finish_reads_a_last_usage_record_without_a_blank_line() {
        let record = usage_chunk(7);
        let body = record.trim_end_matches('\n');
        for ending in ["", "\n", "\r", "\r\n"] {
            let input = format!("{body}{ending}");
            assert_eq!(scan(&[input.as_str()]).reported_total(), None, "not read before finish: {input:?}");
            assert_eq!(finished(&[input.as_str()]).reported_total(), Some(7), "{input:?}");
            assert_eq!(finished(&[body, ending]).reported_total(), Some(7), "split: {input:?}");
        }
    }

    /// SMA-730 U2: the tails that `finish()` closes, and what each one counts.
    #[test]
    fn finish_closes_every_kind_of_tail() {
        let cut_usage = "data: {\"choices\":[],\"usage\":{\"total_tokens\":";
        for (tail, records) in [("data: a", 1), ("data: a\ndata: b", 1), ("data: a\n: keep-alive", 1), (": keep-alive", 0), (cut_usage, 1), ("data: [DO", 1)] {
            let scanner = finished(&[tail]);
            assert_eq!((scanner.completion_records(), scanner.reported_total()), (records, None), "{tail:?}");
        }
    }

    /// SMA-730 U3: after a blank line `finish()` changes nothing, and a second call changes nothing.
    #[test]
    fn finish_is_a_no_op_after_a_blank_line_and_when_called_twice() {
        let stream = format!("{}{}", chunk("a", true), usage_chunk(7));
        let mut scanner = finished(&[stream.as_str()]);
        assert_eq!((scanner.completion_records(), scanner.reported_total()), (1, Some(7)));
        scanner.finish();
        assert_eq!((scanner.completion_records(), scanner.reported_total()), (1, Some(7)));
        let mut once = finished(&["data: a"]);
        once.finish();
        assert_eq!(once.completion_records(), 1, "a second finish does not count the record again");
    }

    /// Review Focus 1: an unterminated `[DONE]` after the usage record keeps the usage.
    #[test]
    fn finish_after_an_unterminated_done_keeps_the_usage() {
        let stream = format!("{}data: [DONE]", usage_chunk(7));
        let scanner = finished(&[stream.as_str()]);
        assert_eq!((scanner.completion_records(), scanner.reported_total()), (0, Some(7)));
    }

    /// SMA-730 U4: an oversized partial record at the end counts once.
    #[test]
    fn finish_counts_an_oversized_partial_record_once() {
        let mut scanner = UsageScanner::new();
        scanner.feed(b"data: ");
        scanner.feed(&vec![b'x'; MAX_RECORD_BYTES + 1]);
        scanner.finish();
        assert_eq!(scanner.completion_records(), 1);
    }
```

- [ ] **Step 2: Run the tests and see them fail**

Run (from `rs/`): `cargo nextest run --locked -p paigasus-gateway --lib finish`
Expected: a compile error, `no method named finish found for struct UsageScanner`.

- [ ] **Step 3: Write the implementation**

In `impl UsageScanner`, after `feed`:

```rust
    /// The upstream stream ended, or failed. Close a pending line and a pending record, as a
    /// blank line does (SMA-730). A second call does nothing.
    pub fn finish(&mut self) {
        self.feed(b"\n\n");
    }
```

Change the struct doc comment (lines 78-81) to:

```rust
/// Reads a copy of a forwarded SSE stream (§ 4.6). Every complete `data:` record that is not
/// `[DONE]` and holds no usage object adds one to the completion estimate (an OpenAI chunk
/// usually carries one token). A record that holds `"usage":{` is parsed, and the last
/// `usage.total_tokens` seen wins. A blank line completes a record. At the end of the stream,
/// `finish()` also completes the last record (SMA-730).
```

- [ ] **Step 4: Run the tests and see them pass**

Run: `cargo nextest run --locked -p paigasus-gateway --lib usage`
Expected: PASS, every test in `adapters::http::usage::tests`. `finish` is `pub`, so it is not dead code before Task 2.

- [ ] **Step 5: Commit**

```bash
git add rs/crates/services/paigasus-gateway/src/adapters/http/usage.rs
git commit -m "feat(rs): add UsageScanner::finish to close the last SSE record (SMA-730)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: Call `finish()` from the relay

**Files:**
- Modify: `rs/crates/services/paigasus-gateway/src/adapters/http/chat.rs` (`Relay` doc at lines 262-264, `terminal_sse_error_stream` at lines 283-323, test module from line 325)

**Interfaces:**
- Consumes: `UsageScanner::finish(&mut self)` from Task 1. `ChargeGuard::set_stream_progress(&mut self, completion_records: u64, reported: Option<u64>)`.
- Produces: `fn finish_scan(scan: &mut UsageScanner, guard: &mut Option<ChargeGuard>)` (private).

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `chat.rs`, before `make_reqwest_error`:

```rust
    use crate::application::charge_guard::{ChargeGuard, NoTicket};
    use crate::application::limits::Limits;
    use crate::domain::limits::{BudgetPeriod, LimitDecision, LimitRules, LimitStoreError, LimitTicket};
    use crate::test_support::{FixedClock, ORG_A, ScriptedStore, at};
    use std::sync::Arc;

    fn admit() -> Result<LimitDecision, LimitStoreError> {
        Ok(LimitDecision::Admit(None))
    }

    /// A sent guard with a ticket and a request estimate of 1, charging into `store`.
    fn sent_guard(store: &Arc<ScriptedStore>) -> ChargeGuard {
        let now = at("2026-10-02T12:00:00Z");
        let limits = Arc::new(Limits::new(LimitRules::default(), store.clone(), Arc::new(FixedClock(now))));
        let ticket = LimitTicket { org: ORG_A, period: BudgetPeriod::Monthly.key_at(now) };
        let mut guard = ChargeGuard::new(limits, Some(ticket), Some(ORG_A), NoTicket::NoBudget);
        guard.set_request_estimate(1);
        guard.mark_sent();
        guard
    }

    const UNTERMINATED_USAGE: &[u8] = b"data: {\"choices\":[],\"usage\":{\"total_tokens\":7}}";

    /// SMA-730 R1 (spec D2): an upstream error after a usage record with no blank line charges
    /// the reported total. The terminal frame closes that record for the client too.
    #[tokio::test]
    async fn an_error_after_an_unterminated_usage_record_charges_the_reported_tokens() {
        let store = ScriptedStore::new(admit);
        let inner = futures::stream::iter(vec![Ok(Bytes::from_static(UNTERMINATED_USAGE))])
            .chain(futures::stream::once(async { Err(make_reqwest_error().await) }))
            .boxed();
        let out: Vec<Bytes> = terminal_sse_error_stream(inner, Some(sent_guard(&store))).map(|r| r.unwrap()).collect().await;
        assert!(out.concat().ends_with(TERMINAL_SSE_ERROR.as_bytes()));
        assert_eq!(store.charges(), vec![7]);
    }

    /// SMA-730 R2 (spec D3): a client that disconnects drops the relay. The unterminated record
    /// does not count, and the guard charges the estimate.
    #[tokio::test]
    async fn a_dropped_relay_does_not_finish_the_last_record() {
        let store = ScriptedStore::new(admit);
        let inner = futures::stream::iter(vec![Ok(Bytes::from_static(UNTERMINATED_USAGE))]).chain(futures::stream::pending()).boxed();
        let mut relay = Box::pin(terminal_sse_error_stream(inner, Some(sent_guard(&store))));
        assert_eq!(relay.next().await.expect("one chunk").unwrap(), Bytes::from_static(UNTERMINATED_USAGE));
        assert!(store.charges().is_empty(), "nothing is charged while the stream is open");
        drop(relay);
        assert_eq!(store.charges(), vec![1], "the request estimate only");
    }

    /// Review Focus 3: without a guard an unterminated tail is forwarded unchanged.
    #[tokio::test]
    async fn the_relay_forwards_an_unterminated_tail_unchanged_without_a_guard() {
        let chunks = vec![Ok(Bytes::from_static(b"data: a\n\n")), Ok(Bytes::from_static(UNTERMINATED_USAGE))];
        let out: Vec<Bytes> = terminal_sse_error_stream(futures::stream::iter(chunks).boxed(), None).map(|r| r.unwrap()).collect().await;
        assert_eq!(out.concat(), [b"data: a\n\n".as_slice(), UNTERMINATED_USAGE].concat());
    }
```

If a `use` path does not resolve, find the correct path with `grep -rn "pub struct Limits\b\|pub enum BudgetPeriod\|pub struct LimitTicket" rs/crates/services/paigasus-gateway/src`. `charge_guard.rs` tests use the same items (see its `guard_with` and `ticket` helpers). Do not change any production visibility.

- [ ] **Step 2: Run the tests and see R1 fail**

Run: `cargo nextest run --locked -p paigasus-gateway --lib adapters::http::chat`
Expected: `an_error_after_an_unterminated_usage_record_charges_the_reported_tokens` FAILS with `left: [1]`, `right: [7]`. The other two new tests PASS (they pin behavior that must not change).

- [ ] **Step 3: Write the implementation**

Add, above `terminal_sse_error_stream`:

```rust
/// SMA-730: the upstream ended or failed. A last record with no blank line after it still
/// counts. Without a guard the scanner reads nothing, as in the chunk arm.
fn finish_scan(scan: &mut UsageScanner, guard: &mut Option<ChargeGuard>) {
    if let Some(guard) = guard.as_mut() {
        scan.finish();
        guard.set_stream_progress(scan.completion_records(), scan.reported_total());
    }
}
```

Replace the two arms in the `unfold` closure:

```rust
                // First upstream error: count the last record (the frame's leading blank line
                // closes it for the client too), emit the terminal event, then end.
                Some(Err(_)) => {
                    finish_scan(&mut scan, &mut guard);
                    Some((
                        Ok(Bytes::from_static(TERMINAL_SSE_ERROR.as_bytes())),
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
```

Change the `Relay` doc comment (lines 262-264) to:

```rust
/// The unfold state. The charge guard is a FIELD here, not a `Drop` on `StreamState`: the closure
/// destructures the state by move, which a type with `Drop` forbids (E0509). When the stream ends,
/// fails, or is dropped, this value drops once, and the guard with it (SMA-677 § 4.6). The clean
/// end and the first error call `UsageScanner::finish()` first; a drop (client disconnect) does
/// not, so a last record with no blank line counts only when the upstream ended (SMA-730 D3).
```

- [ ] **Step 4: Run the tests and see them pass**

Run: `cargo nextest run --locked -p paigasus-gateway --lib`
Expected: PASS, all lib tests.

- [ ] **Step 5: Commit**

```bash
git add rs/crates/services/paigasus-gateway/src/adapters/http/chat.rs
git commit -m "fix(rs): charge the last SSE record when a gateway stream ends without a blank line (SMA-730)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: Integration test I1, mutation checks, full verification

**Files:**
- Modify: `rs/crates/services/paigasus-gateway/tests/limits_http.rs` (after `crlf_records_are_counted_and_a_crlf_usage_record_is_read`, near line 240)

**Interfaces:**
- Consumes: the test helpers `MockOpenAi::spawn_raw_sse(Vec<String>)`, `chunk(&str, bool) -> String`, `usage_chunk(u64) -> String`, `stream_charged(&str) -> (Sent, Vec<u64>)`, `Sent { status, headers, body: Bytes }` (all exist).
- Produces: nothing.

- [ ] **Step 1: Write the test**

```rust
/// SMA-730 I1: an upstream that ends without a blank line after its usage record is charged the
/// reported total, and the client gets the upstream bytes unchanged (A5). No `data: [DONE]`:
/// after a record with no line terminator it would join the same line.
#[tokio::test]
async fn a_stream_whose_last_usage_record_has_no_blank_line_charges_the_reported_tokens() {
    let upstream = vec![chunk("Hel", true), chunk("lo", true), usage_chunk(7).trim_end_matches('\n').to_owned()];
    let mock = MockOpenAi::spawn_raw_sse(upstream.clone()).await;
    let (sent, charges) = stream_charged(&mock.base_url).await;
    assert_eq!(sent.status, StatusCode::OK);
    assert_eq!(sent.body, upstream.concat().into_bytes(), "the client bytes are the upstream bytes");
    assert_eq!(charges, vec![7]);
}
```

If `sent.body == Vec<u8>` does not compile, compare `sent.body.as_ref()` with `upstream.concat().as_bytes()`.

- [ ] **Step 2: Run the test and see it pass**

Run: `cargo nextest run --locked -p paigasus-gateway --test limits_http`
Expected: PASS, every test in the binary.

- [ ] **Step 3: Mutation checks (each mutant must compile)**

Do each mutation by an edit, run the named test, record the result, then restore the line by an edit (do NOT use `git checkout --`). After each restore, confirm `git diff --stat` shows only the files of this task.

| Mutant | Command | Expected |
|---|---|---|
| M1: delete the `finish_scan(&mut scan, &mut guard);` line in the `None` arm | `cargo nextest run --locked -p paigasus-gateway --test limits_http a_stream_whose_last` | FAIL, `left: [3]`, `right: [7]` |
| M2: delete the same line in the `Some(Err(_))` arm | `cargo nextest run --locked -p paigasus-gateway --lib an_error_after_an_unterminated` | FAIL, `left: [1]`, `right: [7]` |
| M3: add `impl Drop for Relay { fn drop(&mut self) { finish_scan(&mut self.scan, &mut self.guard); } }` | `cargo nextest run --locked -p paigasus-gateway --lib a_dropped_relay` | FAIL to compile with E0509 (the closure destructures `Relay`), or FAIL with `left: [7]`. Both prove the design rule. Record which one. |
| M4: change the body of `finish()` to `{}` | `cargo nextest run --locked -p paigasus-gateway --lib finish` | FAIL |

If a mutant does not compile for a reason other than M3's E0509, the check proves nothing. Report it.

- [ ] **Step 4: Full verification through Moon**

Run from the worktree root, with Docker running:

```bash
moon run paigasus-gateway-rs:fmt paigasus-gateway-rs:lint paigasus-gateway-rs:test
```

Expected: all three PASS. If `fmt` fails, run `cargo fmt -p paigasus-gateway` from `rs/` and run again. If a Docker-backed suite fails, run it once more before you report it as a defect.

- [ ] **Step 5: Commit**

```bash
git add rs/crates/services/paigasus-gateway/tests/limits_http.rs
git commit -m "test(rs): pin the gateway charge for an unterminated last usage record (SMA-730)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```
