# SMA-583 — gRPC audit timestamp bound Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make gRPC `ListAuditEntries` reject a present-but-unrepresentable `from`/`to` timestamp with `INVALID_ARGUMENT` instead of silently treating it as unfiltered, and make the broken shape unwritable from outside `convert.rs`.

**Architecture:** Swap two `and_then(convert::from_ts)` call sites onto the existing, already-tested `convert::parse_opt_ts` helper, which keeps absent / valid / unrepresentable distinct. Then fold the one remaining outside caller of `from_ts` onto the same helper so `from_ts` can become module-private — a compile-time check replacing the `repo:*` grep gate the issue asked us to consider. Finally document the rule in the proto, which is the source of truth.

**Tech Stack:** Rust (edition 2024, rust-version 1.95), tonic/prost, sea-orm, `cargo nextest`, Moon 2.3.2, buf.

**Spec:** `docs/superpowers/specs/2026-08-23-sma-583-grpc-audit-timestamp-bound-design.md`

## Global Constraints

- Every source file opens with `// SPDX-License-Identifier: Apache-2.0`. All files here already exist — do not add or remove headers.
- Crate root for all Rust paths below: `rs/crates/services/paigasus-iam/`.
- **PATH:** every shell command must be prefixed with
  `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` — the Bash tool's PATH lacks the proto-managed CLIs (moon, buf, nextest).
- **Working directory:** the git worktree `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-583`. Do **not** `cd` to the main checkout.
- **`[workspace.lints.rust] warnings = "deny"`** — dead code is a hard compile error. This dictates task order: Task 5 (making `from_ts` private) **must** come after Tasks 1 and 3, which remove its two outside callers. Do not reorder.
- Conventional commits with a workspace scope: `fix(rs):`, `docs(rs):`, `test(rs):`, `feat(contracts):`.
- Commit subject must **start lowercase** and be ≤100 chars. Never write a bare `#NNN` in a commit body (commitlint reads it as a footer and fails `footer-leading-blank`) — write "PR NNN" instead.
- Do **not** use `--no-verify`. The worktree is provisioned; commitlint works.
- Docker is reachable in this worktree, so the Docker-gated suites will really run. For any **filtered** test run (`-E 'test(...)'`, `--test <name>`) prefix with `PAIGASUS_REQUIRE_DOCKER=1`, because the Docker canary is not in the filter and the suites would otherwise skip silently and report a green that tested nothing.

---

### Task 1: Reject a malformed `from`/`to` in `grpc::audit::to_filter`

Implements spec D1, D2 (gRPC half), and D4's unit tier. This is the whole bug fix; everything after it is hardening and documentation.

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/grpc/audit.rs:91-98` (the `to_filter` body and the `KNOWN LATENT` comment above it)
- Test: `rs/crates/services/paigasus-iam/src/adapters/grpc/audit.rs` — the existing `#[cfg(test)] mod tests` at the bottom of the same file

**Interfaces:**
- Consumes: `convert::parse_opt_ts(t: Option<prost_types::Timestamp>, field: &str) -> Result<Option<DateTime<Utc>>, TenancyError>` — already exists at `src/adapters/grpc/convert.rs:210`, already `pub`, already unit-tested. Do not modify it in this task.
- Produces: nothing new. `to_filter`'s signature is unchanged: `fn to_filter(req: ListAuditEntriesRequest) -> Result<AuditFilter, TenancyError>`.

Note `mod tests` already has `use super::*;`, and `default_request()` is defined at the **bottom** of that module (after the tests that use it) — that is fine in Rust; follow the existing placement and add new tests above it.

- [ ] **Step 1: Write the failing tests**

In `src/adapters/grpc/audit.rs`, inside `mod tests`, add these three tests immediately after the existing `to_filter_rejects_a_malformed_cursor` test. Also extend the existing `to_filter_treats_empty_wire_fields_as_unfiltered` by adding two assertions to it.

Add to the **end** of `to_filter_treats_empty_wire_fields_as_unfiltered`'s assertion block (after `assert_eq!(filter.cursor, None);`):

```rust
        // The `from`/`to` half of "absent means unfiltered" — untested before SMA-583, which is
        // how `and_then(from_ts)` mapped an INVALID bound to the same `None` unnoticed.
        assert_eq!(filter.from, None);
        assert_eq!(filter.to, None);
```

Then add these three new tests:

```rust
    /// The case that catches a fix which rejects malformed bounds but drops VALID ones: both
    /// bounds must survive as the exact instant the wire asked for. Mirrors
    /// `http::audit::to_filter_parses_valid_rfc3339_from_and_to`, but asserts the instant
    /// rather than just `is_some()` — the gRPC side converts, it does not parse a string.
    #[test]
    fn to_filter_parses_a_present_valid_from_and_to() {
        let from = chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        let to = chrono::DateTime::from_timestamp(1_700_003_600, 500).unwrap();
        let filter = to_filter(ListAuditEntriesRequest {
            from: Some(convert::ts(from)),
            to: Some(convert::ts(to)),
            ..default_request()
        })
        .unwrap();
        assert_eq!(filter.from, Some(from));
        assert_eq!(filter.to, Some(to));
    }

    /// SMA-583: a PRESENT but unrepresentable `from` (`nanos: -1` is outside `Timestamp`'s
    /// valid `[0, 999_999_999]`) is a client error, NOT silently unfiltered. `to` is left
    /// absent deliberately — setting both would pass even if only the `from` line were fixed.
    #[test]
    fn to_filter_rejects_a_present_but_unrepresentable_from_instead_of_unfiltering() {
        let err = to_filter(ListAuditEntriesRequest {
            from: Some(prost_types::Timestamp { seconds: 0, nanos: -1 }),
            to: None,
            ..default_request()
        })
        .unwrap_err();
        assert!(matches!(err, TenancyError::InvalidPrn(_)), "{err:?}");
    }

    /// The `to` half, with `from` absent for the same reason the previous test leaves `to`
    /// absent: this is what fails if only one of the two call sites is fixed.
    #[test]
    fn to_filter_rejects_a_present_but_unrepresentable_to_instead_of_unfiltering() {
        let err = to_filter(ListAuditEntriesRequest {
            from: None,
            to: Some(prost_types::Timestamp { seconds: 0, nanos: -1 }),
            ..default_request()
        })
        .unwrap_err();
        assert!(matches!(err, TenancyError::InvalidPrn(_)), "{err:?}");
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --lib -E 'test(to_filter)'
```

Expected: `to_filter_parses_a_present_valid_from_and_to` **PASSES** already (the old code converts valid bounds correctly — that test is a regression guard, not a red-to-green one). The two `rejects_a_present_but_unrepresentable_*` tests **FAIL**, each with a panic from `.unwrap_err()` on an `Ok` value — that is the bug: the malformed bound was silently accepted as `None`.

If either rejection test passes at this point, stop — the bug is not where the spec says it is.

- [ ] **Step 3: Make the fix**

In `src/adapters/grpc/audit.rs`, replace the five-line `KNOWN LATENT` comment and the two `and_then` lines. Replace this:

```rust
        // KNOWN LATENT (design D10, SMA-501): this drops a PRESENT-but-unrepresentable bound to
        // `None`, which on a filter field means UNFILTERED — a malformed timestamp silently widens
        // the query instead of being rejected. `convert::parse_opt_ts` keeps the three cases
        // distinct and is what `grpc::dead_letters` uses. Left as-is deliberately: this is a READ,
        // so the blast radius is a wider result set rather than a wider mutation. Tracked in SMA-583.
        from: req.from.and_then(convert::from_ts),
        to: req.to.and_then(convert::from_ts),
```

with this:

```rust
        from: convert::parse_opt_ts(req.from, "from")?,
        to: convert::parse_opt_ts(req.to, "to")?,
```

Then add the positive rationale to `to_filter`'s **doc comment** (the `///` block directly above `fn to_filter`), appending this paragraph at its end — wording taken from `grpc::dead_letters::to_filter`'s doc at `src/adapters/grpc/dead_letters.rs:86-87`, which is transport-generic:

```rust
/// Timestamps go through [`convert::parse_opt_ts`], NOT `and_then(convert::from_ts)`: the
/// latter maps an unrepresentable value to `None`, which on a filter field means UNFILTERED
/// (SMA-583).
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --lib -E 'test(to_filter)'
```

Expected: PASS, all `to_filter` tests including the three new ones.

- [ ] **Step 5: Verify no `KNOWN LATENT` marker survives (AC3)**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
grep -rn "KNOWN LATENT" rs/crates/services/paigasus-iam/src/ || echo "clean"
```

Expected: `clean`.

- [ ] **Step 6: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/adapters/grpc/audit.rs
git commit -m "fix(rs): reject a malformed grpc audit timestamp bound (SMA-583)"
```

---

### Task 2: Pin the rejection over the wire

Implements spec D4's integration tier. Extends the existing seeded test rather than adding a fourth Postgres container — it already seeds a row and asserts an unfiltered list returns it, which *is* the control.

**Files:**
- Modify: `rs/crates/services/paigasus-iam/tests/grpc_audit.rs:117-154` (the test `list_audit_entries_over_grpc_returns_seeded_rows_for_a_platform_admin`)

**Interfaces:**
- Consumes: Task 1's behaviour (`InvalidArgument` on a malformed bound). Test-local fixtures already in this file: `spawn_server`, `channel`, `authed`, `default_request`, `denial`, plus `support::provision_platform_admin`.
- Produces: nothing consumed by later tasks.

- [ ] **Step 1: Write the failing assertions**

In `tests/grpc_audit.rs`, rename the test and append the malformed-bound assertions. Change the function name line:

```rust
async fn list_audit_entries_over_grpc_returns_seeded_rows_for_a_platform_admin() {
```

to:

```rust
async fn list_audit_entries_over_grpc_returns_seeded_rows_and_rejects_a_malformed_bound() {
```

Add this doc comment directly above the `#[tokio::test]` attribute of that test:

```rust
/// Two halves against one seeded fixture. The first is the CONTROL: an unfiltered request
/// returns the seeded row, proving the row is genuinely matchable. The second is SMA-583: a
/// PRESENT but unrepresentable `from`/`to` is `InvalidArgument`, not a silently widened query.
/// The control is what stops the second half being vacuous — without it these assertions would
/// pass green against an empty database, proving nothing about unfiltering.
///
/// Deliberately one test, not two: the control is already written here and a second test would
/// cost another Postgres container for no extra signal, given the fine-grained diagnosis now
/// lives in `grpc::audit`'s unit tests. Trade-off accepted: one red instead of two on failure.
```

Then, immediately before the closing `server.abort();` of that test, insert:

```rust
    // SMA-583: a present-but-unrepresentable bound is rejected, never treated as unfiltered.
    // `to` absent here, `from` absent below — setting both at once would still pass if only one
    // of the two call sites had been fixed.
    let err = audit
        .list_audit_entries(authed(
            ListAuditEntriesRequest {
                from: Some(prost_types::Timestamp { seconds: 0, nanos: -1 }),
                to: None,
                ..default_request()
            },
            &admin_token,
        ))
        .await
        .unwrap_err();
    assert_eq!(err.code(), Code::InvalidArgument, "malformed `from` must be rejected: {err:?}");

    let err = audit
        .list_audit_entries(authed(
            ListAuditEntriesRequest {
                from: None,
                to: Some(prost_types::Timestamp { seconds: 0, nanos: -1 }),
                ..default_request()
            },
            &admin_token,
        ))
        .await
        .unwrap_err();
    assert_eq!(err.code(), Code::InvalidArgument, "malformed `to` must be rejected: {err:?}");
```

`Code` is already imported in this file (`use tonic::Code;`). `prost_types` is available to this crate's tests — `tests/grpc_dead_letters.rs` uses `prost_types::Timestamp` the same way, unqualified.

- [ ] **Step 2: Run the test**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test grpc_audit
```

Expected: PASS. (Task 1 already landed the fix, so this is a green-on-green confirmation rather than a red-to-green cycle. If it fails with `InvalidArgument` expected but `Ok` received, Task 1 is incomplete.)

`PAIGASUS_REQUIRE_DOCKER=1` is required here: this is a filtered run, the Docker canary is not in the filter, and without it a Docker hiccup would report a green that tested nothing.

- [ ] **Step 3: Confirm the control actually bites**

Sanity-check that the control is not vacuous, by confirming the test still asserts a returned row:

```bash
grep -n "resp.entries.len(), 1" rs/crates/services/paigasus-iam/tests/grpc_audit.rs
```

Expected: one hit, inside the renamed test.

- [ ] **Step 4: Commit**

```bash
git add rs/crates/services/paigasus-iam/tests/grpc_audit.rs
git commit -m "test(rs): pin the grpc audit malformed-bound rejection over the wire (SMA-583)"
```

---

### Task 3: Fold `service_accounts.rs` onto `parse_opt_ts`

Implements spec D5. **Load-bearing for Task 5** — this removes `from_ts`'s last caller outside `convert.rs`. Behaviour is unchanged; the point is to make Task 5 possible.

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/grpc/service_accounts.rs:183-187`
- Test: `rs/crates/services/paigasus-iam/tests/api_keys_grpc.rs` — inside the existing `grpc_issue_and_introspect_parity` test (`:67-133`)

**Interfaces:**
- Consumes: `convert::parse_opt_ts` (same signature as Task 1).
- Produces: `from_ts` now has zero callers outside `convert.rs` — the precondition Task 5 depends on.

- [ ] **Step 1: Write the failing test**

There is currently **no** test for an out-of-range `expires_at` over gRPC — all four `IssueApiKeyRequest` sites in `tests/api_keys_grpc.rs` (`:102`, `:170`, `:198`, `:281`) send `expires_at: None`. Add one assertion to `grpc_issue_and_introspect_parity`, reusing the service account it already created (zero extra containers).

Insert this immediately after the existing `assert_eq!(api_key.service_account_prn, sa_prn);` line in that test:

```rust
    // SMA-583: `expires_at` goes through the same `parse_opt_ts` helper as the audit filters.
    // A present-but-unrepresentable value is a client error — the ONLY test of this path, and
    // what makes the refactor's "no behaviour change" claim checkable.
    let err = sa_client
        .issue_api_key(authed(
            IssueApiKeyRequest {
                service_account_prn: sa_prn.clone(),
                scope_prn: owner.canonical(),
                expires_at: Some(prost_types::Timestamp { seconds: 0, nanos: -1 }),
                scope_actions: Vec::new(),
                scope_roles: Vec::new(),
            },
            &token,
        ))
        .await
        .unwrap_err();
    assert_eq!(err.code(), Code::InvalidArgument, "an unrepresentable expires_at must be rejected: {err:?}");
```

- [ ] **Step 2: Run it to verify it passes against the CURRENT code**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test api_keys_grpc -E 'test(grpc_issue_and_introspect_parity)'
```

Expected: **PASS**. This is deliberately not a red-to-green test — the current hand-rolled code already rejects correctly. The test is a *characterization* test: it captures existing behaviour so Step 3's refactor is provably behaviour-preserving. Running it before the refactor is the whole point; do not skip this step.

- [ ] **Step 3: Refactor to the shared helper**

In `src/adapters/grpc/service_accounts.rs`, replace this:

```rust
            // `expires_at` unset means non-expiring (or the configured `default_expiry_days`
            // fallback, `ApiKeyService::issue`) — mirrors `IssueApiKeyBody::expires_at`'s HTTP
            // counterpart. A present-but-out-of-range timestamp is `InvalidPrn`-as-sentinel
            // (mirrors `DetachMembershipRequest`'s "not a uuid" posture): there is no dedicated
            // error code for "not a valid timestamp" either.
            let expires_at = req
                .expires_at
                .map(|t| convert::from_ts(t).ok_or_else(|| TenancyError::InvalidPrn("expires_at out of range".to_string())))
                .transpose()
                .map_err(convert::status_to_grpc)?;
```

with this:

```rust
            // `expires_at` unset means non-expiring (or the configured `default_expiry_days`
            // fallback, `ApiKeyService::issue`) — mirrors `IssueApiKeyBody::expires_at`'s HTTP
            // counterpart. A present-but-out-of-range timestamp is `InvalidPrn`-as-sentinel
            // (mirrors `DetachMembershipRequest`'s "not a uuid" posture): there is no dedicated
            // error code for "not a valid timestamp" either. `parse_opt_ts` is that exact
            // absent/valid/unrepresentable split, shared with the filter call sites (SMA-583).
            let expires_at = convert::parse_opt_ts(req.expires_at, "expires_at").map_err(convert::status_to_grpc)?;
```

The error detail string changes from `"expires_at out of range"` to `"invalid timestamp for expires_at"`. This is wire-invisible: `TenancyError::InvalidPrn`'s `Display` is the static `"invalid resource prn"` (`src/application/error.rs:41-42`), which is what reaches the client. No test asserts the old string — confirmed by grep.

- [ ] **Step 4: Check whether `TenancyError` is still used in this file**

The removed line was a `TenancyError::InvalidPrn` construction site. If it was the last one in the file, the `use crate::application::error::TenancyError;` import is now unused, which under `warnings = "deny"` is a **hard compile error**.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
grep -n "TenancyError" rs/crates/services/paigasus-iam/src/adapters/grpc/service_accounts.rs
```

If the only remaining hit is the `use` line, delete that import. If other uses remain, leave it.

- [ ] **Step 5: Run the test to verify behaviour is unchanged**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test api_keys_grpc
```

Expected: PASS, the whole suite — the characterization test from Step 2 still green, proving the refactor changed nothing observable.

- [ ] **Step 6: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/adapters/grpc/service_accounts.rs rs/crates/services/paigasus-iam/tests/api_keys_grpc.rs
git commit -m "refactor(rs): fold issue-api-key expires_at onto parse_opt_ts (SMA-583)"
```

---

### Task 4: Correct the two stale comments

Implements the rest of spec D2. Comment-only — no behaviour change, no test.

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/http/audit.rs:65-69`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/grpc/convert.rs:201-205`

**Interfaces:**
- Consumes: nothing. Produces: nothing.

- [ ] **Step 1: Fix the HTTP twin's factually wrong claim**

`http::audit::parse_ts`'s doc currently asserts the gRPC hazard cannot exist — the exact false belief this issue corrects. In `src/adapters/http/audit.rs`, replace:

```rust
/// Parses an RFC3339 `from`/`to` query param: absent/empty means unfiltered; a present value
/// must parse as RFC3339. `InvalidPrn`-as-sentinel, mirroring `parse_outcome`/`parse_cursor`
/// above — there is no dedicated error code for "not a valid timestamp" either, and the gRPC
/// wire has no equivalent failure mode to mirror here (its `from`/`to` are already-structured
/// `prost_types::Timestamp`, never a string that can fail to parse).
```

with:

```rust
/// Parses an RFC3339 `from`/`to` query param: absent/empty means unfiltered; a present value
/// must parse as RFC3339. `InvalidPrn`-as-sentinel, mirroring `parse_outcome`/`parse_cursor`
/// above — there is no dedicated error code for "not a valid timestamp" either.
///
/// The gRPC twin has the SAME three-case split, contrary to what this comment claimed before
/// SMA-583: a `prost_types::Timestamp` cannot fail to *parse*, but it can fail to *convert*
/// (a negative `nanos`, or a `seconds` outside `chrono`'s range), and `grpc::audit::to_filter`
/// now rejects that via `convert::parse_opt_ts` exactly as this does.
```

- [ ] **Step 2: Un-stale `parse_opt_ts`'s doc**

Task 1 deleted the call site this sentence points at. In `src/adapters/grpc/convert.rs`, replace:

```rust
/// That third case is why this exists. [`from_ts`] returns `None` for a negative `nanos` or an
/// out-of-`chrono`-range `seconds`, and on a filter field `None` means UNFILTERED — so the
/// `req.field.and_then(convert::from_ts)` shape used in `grpc::audit` silently DROPS a
/// malformed bound instead of rejecting it. On `BulkReplayDeadLetters` that turns a
/// narrowly-scoped replay into "replay everything up to `max_rows`". The HTTP twin rejects the
/// equivalent with a 400 (`http::dead_letters::parse_ts`), so this also restores parity.
```

with:

```rust
/// That third case is why this exists. `from_ts` returns `None` for a negative `nanos` or an
/// out-of-`chrono`-range `seconds`, and on a filter field `None` means UNFILTERED — so a
/// `req.field.and_then(from_ts)` shape silently DROPS a malformed bound instead of rejecting
/// it. On `BulkReplayDeadLetters` that turned a narrowly-scoped replay into "replay everything
/// up to `max_rows`"; on `ListAuditEntries` it widened the result set (SMA-583). Both surfaces
/// now use this helper, and `from_ts` is module-private so the shape cannot recur outside this
/// file. The HTTP twin rejects the equivalent with a 400 (`http::dead_letters::parse_ts`), so
/// this also restores parity.
```

Note the intra-doc link ``[`from_ts`]`` has become a plain code span `` `from_ts` ``. This is required by Task 5: once `from_ts` is private, a `pub` item's doc linking to it emits `rustdoc::private_intra_doc_links`. Making the edit here keeps Task 5 to a one-word change.

- [ ] **Step 3: Verify it still builds and nothing else links `from_ts`**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo build -p paigasus-iam --tests 2>&1 | tail -5
grep -n '\[`from_ts`\]' crates/services/paigasus-iam/src/adapters/grpc/convert.rs || echo "no intra-doc links to from_ts remain"
```

Expected: build succeeds; `no intra-doc links to from_ts remain`.

- [ ] **Step 4: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/adapters/http/audit.rs rs/crates/services/paigasus-iam/src/adapters/grpc/convert.rs
git commit -m "docs(rs): correct the stale timestamp-conversion comments (SMA-583)"
```

---

### Task 5: Make `from_ts` module-private (AC4)

Implements spec D3 — the compile-time mechanism replacing the `repo:*` grep gate the issue asked us to consider.

**MUST run after Tasks 1 and 3.** Both removed a caller of `from_ts` outside `convert.rs`; doing this first is a compile error.

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/grpc/convert.rs:192`

**Interfaces:**
- Consumes: Tasks 1 and 3 having removed both outside callers.
- Produces: `from_ts` is no longer reachable outside `convert.rs`. `parse_opt_ts` remains `pub` and is the only supported entry point for wire-timestamp conversion.

- [ ] **Step 1: Prove the precondition — no callers outside `convert.rs`**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
grep -rn "from_ts" rs/ --include="*.rs" | grep -v "grpc/convert.rs"
```

Expected: **no output**. If anything is listed, stop — Tasks 1 and/or 3 are incomplete, and this task cannot proceed.

- [ ] **Step 2: Drop the visibility modifier**

In `src/adapters/grpc/convert.rs`, change:

```rust
pub fn from_ts(t: prost_types::Timestamp) -> Option<DateTime<Utc>> {
```

to:

```rust
fn from_ts(t: prost_types::Timestamp) -> Option<DateTime<Utc>> {
```

Fully private — **not** `pub(crate)` and **not** `pub(super)`. Both of those would keep it reachable from `grpc::audit` and `grpc::service_accounts`, which are siblings under `adapters::grpc`, and would deliver none of the protection this task exists for.

Then add this to `from_ts`'s doc comment, after its existing text, so the next reader knows the visibility is load-bearing rather than incidental:

```rust
/// **Module-private on purpose (SMA-583).** On a filter field `None` means UNFILTERED, so an
/// `and_then(from_ts)` call site silently widens the query instead of rejecting a malformed
/// bound. Callers outside this module must use [`parse_opt_ts`], which keeps the three cases
/// distinct. Note this closes *that shape* only — `parse_opt_ts(..).ok().flatten()` would
/// reintroduce the same bug, and no grep gate would catch that either.
```

- [ ] **Step 3: Verify the whole crate still compiles**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo build -p paigasus-iam --tests 2>&1 | tail -5
```

Expected: `Finished`. Under `warnings = "deny"`, a now-unused `from_ts` would fail here — it does not, because `parse_opt_ts` (`convert.rs:213`) and the unit test at `convert.rs:732` still call it.

- [ ] **Step 4: Prove the guard actually bites**

This is the acceptance evidence for AC4 — confirm the protection is real rather than assumed:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs/crates/services/paigasus-iam
# Temporarily reintroduce the banned shape in a file OUTSIDE convert.rs.
sed -i.bak 's|^        from: convert::parse_opt_ts(req.from, "from")?,|        from: req.from.and_then(convert::from_ts),|' src/adapters/grpc/audit.rs
cd ../../.. && cargo build -p paigasus-iam 2>&1 | grep -c "private" || true
```

Expected: a non-zero count — the build fails with a privacy error naming `from_ts`.

Now restore, and **`touch` the file** — `mv`ing a `.bak` back rolls mtime *backwards*, so cargo would reuse the binary built from the temporary edit and every later check would be testing the wrong code:

```bash
cd rs/crates/services/paigasus-iam
mv src/adapters/grpc/audit.rs.bak src/adapters/grpc/audit.rs
touch src/adapters/grpc/audit.rs
cd ../../.. && cargo build -p paigasus-iam --tests 2>&1 | tail -3
git diff --stat   # must show NO change to audit.rs
```

Expected: build succeeds, `git diff --stat` shows `audit.rs` unchanged.

- [ ] **Step 5: Run the full crate test suite**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && env -u CI cargo nextest run -p paigasus-iam
```

Expected: PASS. `env -u CI` because a stray `CI` variable is presence-based and would change the Docker-skip policy.

- [ ] **Step 6: Commit**

```bash
git add rs/crates/services/paigasus-iam/src/adapters/grpc/convert.rs
git commit -m "refactor(rs): make convert::from_ts module-private (SMA-583)"
```

---

### Task 6: Document the rule in the proto

Implements spec D6. `contracts/` is the source of truth (ADR-0004) and this is a client-visible behaviour change. Also fixes an independently wrong claim in the existing comment.

**Files:**
- Modify: `contracts/proto/paigasus/iam/v1/iam.proto:484-486`
- Regenerate: `rs/crates/libs/paigasus-proto/src/generated/`, `py/packages/paigasus-proto/src/paigasus_proto/generated/`, `ts/packages/paigasus-proto/src/generated/`

**Interfaces:**
- Consumes: nothing. Produces: regenerated bindings whose doc comments carry the rule.

- [ ] **Step 1: Fix and extend the comment**

In `contracts/proto/paigasus/iam/v1/iam.proto`, replace lines 484-486:

```proto
// Optional scalar filters use empty-string/zero sentinels (mirrors the other
// request messages in this file): an empty string / zero timestamp means
// "unfiltered" on that field.
```

with:

```proto
// Optional scalar filters use empty-string/zero sentinels (mirrors the other
// request messages in this file): an empty string means "unfiltered" on that
// field, and limit 0 means the server default.
//
// Timestamps do NOT use a zero sentinel: a zero Timestamp is PRESENT and
// denotes the epoch, so it filters from 1970-01-01. An ABSENT timestamp means
// unfiltered; a PRESENT but unrepresentable one is INVALID_ARGUMENT and never
// silently unfiltered (SMA-583, matching ListDeadLettersRequest).
```

Two things change: the old text claimed "a zero timestamp means unfiltered", which is wrong (a zero `Timestamp` is present and denotes the epoch), and the absent/valid/unrepresentable rule is now stated — the same rule `ListDeadLettersRequest` already carries at `:622-625`, which claims to *mirror this message*.

- [ ] **Step 2: Format the proto**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd contracts && buf format -w && buf lint
```

Expected: no output from either. Skipping `buf format -w` reds `contracts:fmt` in CI, and it fails *silently* in the sense that the failure is not attributed to the proto edit.

- [ ] **Step 3: Regenerate the bindings**

Proto comments become doc comments in generated Rust/Py/TS, so the generated trees must be refreshed or the codegen-drift gate reds.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd contracts && buf generate
```

Invoke `buf generate` **directly**, not `moon run contracts:generate` — that task declares no `outputs:`, so Moon can serve stale cached output and the drift gate would then fail on CI against a locally-green run.

- [ ] **Step 4: Confirm the comment reached the generated code**

```bash
git diff --stat -- rs/crates/libs/paigasus-proto/src/generated py/packages/paigasus-proto/src/paigasus_proto/generated ts/packages/paigasus-proto/src/generated
grep -rn "never silently unfiltered" rs/crates/libs/paigasus-proto/src/generated | head -3
```

Expected: the diffstat lists changed files, and the grep finds the new doc text. If the diffstat is empty, regeneration did not run — do not proceed.

- [ ] **Step 5: Check `buf breaking` is unaffected**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd contracts && buf breaking --against '../.git#branch=main,subdir=contracts'
```

Expected: no output. Comments are not a breaking change; a failure here means the edit accidentally touched a field number, name, or type.

- [ ] **Step 6: Rebuild Rust against the regenerated bindings**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo build -p paigasus-iam --tests 2>&1 | tail -3
```

Expected: `Finished`.

- [ ] **Step 7: Commit**

```bash
git add contracts/proto/paigasus/iam/v1/iam.proto rs/crates/libs/paigasus-proto/src/generated py/packages/paigasus-proto/src/paigasus_proto/generated ts/packages/paigasus-proto/src/generated
git commit -m "feat(contracts): document the audit timestamp filter contract (SMA-583)"
```

---

### Task 7: Full-graph verification

Per-project Moon tasks do **not** run the repo-level gates. This change touches `contracts/` (drift, `:breaking`, `:fmt`) and a crate other gates key on, so the full graph is required rather than optional.

**Files:** none modified (unless a gate reds).

- [ ] **Step 1: Format and lint Rust**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings
```

Expected: both silent. If `cargo fmt --check` reports a diff, run `cargo fmt` and amend the relevant commit.

- [ ] **Step 2: Run the full CI graph exactly as CI does**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site \
  :input-liveness :promtool :observability-drift :nats-permissions :release-parity \
  :release-parity-py :release-parity-ts :publish-metadata :version-lockstep \
  --base origin/main --include-relations
```

Expected: all green.

- [ ] **Step 3: If Moon reports an unattributed failure, find it**

Moon's summary often says only "N failed" without naming the task:

```bash
jq '.actions[] | select(.status=="failed") | {label, status}' .moon/cache/ciReport.json
```

- [ ] **Step 4: Confirm the acceptance criteria**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
# AC3: the KNOWN LATENT marker is gone
grep -rn "KNOWN LATENT" rs/crates/services/paigasus-iam/src/ || echo "AC3 ok"
# AC4: no caller of from_ts outside convert.rs
grep -rn "from_ts" rs/ --include="*.rs" | grep -v "grpc/convert.rs" || echo "AC4 ok"
# AC1/AC2: the tests that prove the rejection
cd rs && env -u CI cargo nextest run -p paigasus-iam -E 'test(unrepresentable) or test(malformed_bound) or test(to_filter)'
```

Expected: `AC3 ok`, `AC4 ok`, and all listed tests passing.

- [ ] **Step 5: Review the complete diff**

```bash
git diff origin/main --stat
```

Expected files: `contracts/proto/paigasus/iam/v1/iam.proto`, three `generated/` trees, `src/adapters/grpc/{audit,convert,service_accounts}.rs`, `src/adapters/http/audit.rs`, `tests/{grpc_audit,api_keys_grpc}.rs`, and the two `docs/superpowers/` documents. Anything else is unintended — investigate before opening the PR.

---

## Self-Review

**Spec coverage:**

| Spec section | Task |
|---|---|
| D1 — `parse_opt_ts` at both `audit.rs` sites | Task 1 |
| D2 — gRPC comment replaced | Task 1 Step 3 |
| D2 — `http/audit.rs` false claim corrected | Task 4 Step 1 |
| D2 — `convert.rs:203` un-staled | Task 4 Step 2 |
| D3 — `from_ts` module-private, guard proven to bite | Task 5 |
| D3 — rustdoc private-intra-doc-link consequence | Task 4 Step 2 (link → code span) |
| D4 — unit tier (absent / valid / invalid `from` / invalid `to`) | Task 1 Step 1 |
| D4 — integration tier with control, no extra container | Task 2 |
| D5 — `service_accounts.rs` fold | Task 3 Steps 3-4 |
| D5 — the missing `IssueApiKey` assertion | Task 3 Step 1 |
| D6 — proto comment + regen | Task 6 |
| Verification section | Task 7 |

No gaps.

**Placeholder scan:** none — every code step carries the literal text to insert, and every command is runnable as written.

**Type consistency:** `convert::parse_opt_ts(Option<prost_types::Timestamp>, &str) -> Result<Option<DateTime<Utc>>, TenancyError>` is used identically in Tasks 1 and 3. `convert::ts(DateTime<Utc>) -> prost_types::Timestamp` (Task 1's valid-bound test) matches `convert.rs:186`. `to_filter`'s signature is unchanged throughout. `from_ts` is referenced as private only from Task 5 onward, after both its outside callers are gone.

**Ordering hazard, restated:** Task 5 depends on Tasks 1 **and** 3. Task 4 Step 2 must precede Task 5 or rustdoc emits a private-intra-doc-link warning. Task 6 is independent and may run any time after Task 1.
