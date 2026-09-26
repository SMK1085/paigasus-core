# SMA-698: log a JIT provisioning failure — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** When just-in-time (JIT) provisioning fails, IAM writes one rate-limited `warn` line that names the defect and the issuer, and it increments `iam_jit_provisioning_failures_total{defect}`. A lost first-login race on the email becomes a success in Postgres.

**Architecture:** The log line, the counter and the rate limit go into one private helper, `AuthenticateToken::provisioning_failed`, in the application layer. Both transports reach it through `resolve(.., Provisioning::Enabled)`. The validator's `RefusalLog` becomes one shared, generic limiter in `src/application/log_rate_limit.rs`. The `EmailTaken` arm of `jit_provision` re-reads the identity before it reports a conflict.

**Tech Stack:** Rust 2024 (rust-version 1.95), `tracing` 0.1, `tracing-subscriber` 0.3.23 (tests), `metrics` + `metrics-util` 0.20.4 `DebuggingRecorder` (tests), tonic 0.14.6, SeaORM 2 on Postgres 16 (testcontainers), cargo-nextest.

**Spec:** `docs/superpowers/specs/2026-09-26-sma-698-log-jit-provisioning-failure-design.md`

## Deviations from the spec

Each deviation keeps the intent of the spec. The code facts are verified against the tree at `b32d3ec4`.

1. **The limiter key in `AuthenticateToken` is the `defect` label, not `ProvisioningDefect`.** Spec 4.5 says the limiter is keyed by (issuer, K) with K the defect type. `ProvisioningDefect` derives `Debug, Clone, Copy, PartialEq, Eq` and not `Hash` (`rs/crates/libs/paigasus-iam-core/src/authn.rs:182`). Spec D5 forbids a change to `paigasus-iam-core`. So `AuthenticateToken` uses `LogRateLimiter<&'static str>`, keyed by `provisioning_defect_label(defect)`. The label function is an exhaustive `match`, so the label and the defect are one-to-one. The validator keeps `LogRateLimiter<TokenDefect>` (`TokenDefect` derives `Hash`).
2. **T3 is deterministic, and it is in Task 4, not in a separate task.** Spec 6.2 describes "two concurrent `resolve(.., Enabled)` calls". SMA-660 recorded that a `tokio::join!` race test in this crate passed without a race (`tests/authz_policy_store.rs:266-271`), and spec 6.2 forbids a T3 that can pass without a race. So T3 holds the winner's three rows in an open transaction, waits with `support::race::expect_racer_blocked` until the real `resolve` (the loser) is blocked inside its `user` insert, and then commits the winner. The race occurs on every run. T3 runs in Task 4 before the fix, so it MEASURES the defect red (spec 4.6: "measures this and does not assume it") and then green.
3. **Module locations.** The shared limiter is `src/application/log_rate_limit.rs` (`pub(crate)`). `adapters` already depends on `application`, so the validator can use it, and `application` does not depend on `adapters`. The shared test capture is `src/log_capture.rs`, declared in `src/lib.rs` as `#[cfg(test)] mod log_capture;`. It is at the crate root because both layers use it. The validator's `REFUSAL_LOG_INTERVAL` becomes the shared `LOG_RATE_LIMIT_INTERVAL` (10 s).
4. **The PII mutation also logs the subject.** Spec 6.3 adds `email = ?claims.email` and expects U1-U3 red. U1 has no email claim, so that mutation cannot make U1 red. The plan's mutation M4 logs `email` and `subject`, so U1, U2, U3, T1 and T2 go red.
5. **The counter-deletion mutation also reds U2.** U2 asserts `Counter(1)` too. Spec 6.3 lists U1, U3, U9.
6. **Extra tests and mutations.** The Review Focus below adds U11-U15 and mutations M9-M12.
7. **Test-only changes to existing fakes.** `InMemoryMemberships` in `authenticate_token.rs` becomes `Clone` (its field moves into an `Arc`), and U13 uses `crate::adapters::id::KernelIdGenerator`. `AuthenticateToken` derives `Clone`, so every type parameter must be `Clone`, and the fakes `SeqIds` and `InMemoryMembershipRepository` are not. `KernelIdGenerator` is the only `Clone` id generator in the crate.
8. **Line numbers.** The spec's line numbers are correct within one or two lines. The prime in `main.rs` is at line 104 (spec: 105). The metric-family comment is at `main.rs:645` and says 38. A count of the `describe_*!` calls in `describe_iam_metrics` gives 38, so the new count is 39.

## Global Constraints

- Worktree root: `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-698`, branch `feature/sma-698-log-jit-provisioning-failure`. Run every command from there or from its `rs/`.
- PATH for every shell: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.
- Every new source file starts with `// SPDX-License-Identifier: Apache-2.0`.
- Rust edition 2024, rust-version 1.95.
- The workspace sets `[workspace.lints.rust] warnings = "deny"` (`rs/Cargo.toml:275`). Dead code is a compile error. Each task must compile clean under `cargo clippy --workspace --all-targets --locked -- -D warnings`. Do not add an item in one task and use it in a later task.
- Conventional commits. Scope `rs` for code (`feat(rs)`, `fix(rs)`, `refactor(rs)`, `test(rs)`), `docs(rs)` for docs-only.
- The commit body ends with the line `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`, after one blank line.
- No `#NNN` line and no `token: value` line in the commit BODY (commitlint `footer-leading-blank`). Write "SMA-698" inside a sentence, never as `Refs: SMA-698`.
- Write each commit message to a file in your scratchpad directory with the Write tool, then run `git -C <worktree root> commit -F <that file>`.
- Never `git commit --amend`. Never `git reset`. Never `--no-verify`. Never `git stash`.
- Never restore a mutation with `git checkout --`. It also discards uncommitted work. Remove the inserted change with the Edit tool.
- If a commit fails with "failed to fill whole buffer", 1Password is locked. Ask the user to unlock it. Do not change the signing setup.
- Run `cargo fmt --all` from `rs/` after each code change (max_width is 200, and a longer line reflows).
- Run tests from `rs/` with `cargo nextest run -p paigasus-iam …`. Add `--no-tests=pass` when a filter can match nothing.
- A filtered run of a Docker-backed test needs `PAIGASUS_REQUIRE_DOCKER=1`, or a missing Docker daemon reads as a pass.
- The JIT log line must never carry the email, the `sub` claim, the `name` claim, `locale`, `zoneinfo`, token material or an `Email::parse` error (`DomainError::InvalidEmail` holds the raw claim).
- The two message texts are fixed. Tests match on them. Do not change a character:
  - `missing_email`: `just-in-time provisioning failed: the access token has no valid email claim; configure the issuer to put a valid email claim into the access token`
  - `email_conflict`: `just-in-time provisioning failed: another user already has this email address, and IAM does not link identities by email`
  - The shared prefix `just-in-time provisioning failed` selects the helper's lines. No other log line in the crate contains it (checked with `git grep`).
- Do not install host software (no `brew install`). Do not run Docker for anything other than the named test suites.
- Documentation text is in ASD-STE100 Simplified Technical English: short sentences, active voice, one idea in one sentence.

## Review Focus

These five input classes are the most likely to break the feature, and no test in the spec covers them. Each one gets a pinning test in Task 3.

| # | Input class or failure mode | Why it can break | Pinning test |
|---|---|---|---|
| R1 | Two issuers fail in the same 10 s window | A limiter keyed by the defect only would hide the second issuer, and the operator would fix the wrong IdP | U11 `jit_failures_from_two_issuers_log_one_line_each` |
| R2 | Two defects for one issuer in the same window | A limiter keyed by the issuer only would hide `email_conflict` behind `missing_email` | U12 `jit_two_defects_for_one_issuer_log_one_line_each` |
| R3 | `AppState` clones the use case for each request | A limiter that is not shared through the `Arc` resets on each clone, and the rate limit does nothing in production | U13 `a_cloned_use_case_shares_the_rate_limiter` |
| R4 | An `email` claim that is present but empty (`""`) | It must be `invalid`, not `absent`. A wrong `email_claim` sends the operator to the wrong IdP setting | U14 `jit_empty_email_claim_is_invalid_not_absent` |
| R5 | The `suppressed` count on the next admitted line | The limiter test proves the count. Nothing proves that the helper writes it into the line | U15 `the_next_admitted_line_carries_the_suppressed_count` |

## File Structure

| File | Task | Change |
|---|---|---|
| `rs/crates/services/paigasus-iam/src/application/log_rate_limit.rs` | 1 | NEW. `LOG_RATE_LIMIT_INTERVAL`, generic `LogRateLimiter<K>`, the moved limiter unit test |
| `rs/crates/services/paigasus-iam/src/log_capture.rs` | 1 | NEW, `#[cfg(test)]`. `LogBuffer` and `capture_logs` |
| `rs/crates/services/paigasus-iam/src/lib.rs` | 1 | declare `#[cfg(test)] mod log_capture;` |
| `rs/crates/services/paigasus-iam/src/application/mod.rs` | 1 | declare `pub(crate) mod log_rate_limit;` |
| `rs/crates/services/paigasus-iam/src/adapters/oidc/validator.rs` | 1 | use the shared limiter and the shared capture; remove `RefusalLog`, the local `LogBuffer` and the moved test |
| `rs/crates/libs/paigasus-observability/src/names.rs` | 2 | `IAM_JIT_PROVISIONING_FAILURES_TOTAL` and its `ALL` entry |
| `rs/crates/services/paigasus-iam/src/application/authenticate_token.rs` | 2, 3, 4 | label function, defect array, prime (2); `EmailClaim`, `JitFailure`, limiter field, helper, unit tests (3); `EmailTaken` re-read and U6 (4) |
| `rs/crates/services/paigasus-iam/src/main.rs` | 2 | `describe_counter!`, the prime call, the family count 38 to 39 |
| `rs/crates/services/paigasus-iam/tests/support/mod.rs` | 4 | the capture helper copy and `JIT_FAILURE_LINE` |
| `rs/crates/services/paigasus-iam/tests/authn_identities.rs` | 4 | T3 |
| `rs/crates/services/paigasus-iam/tests/http_authn.rs` | 5 | T1 |
| `rs/crates/services/paigasus-iam/tests/grpc_whoami.rs` | 5 | T2 |
| `docs/ops/RUNBOOK-chart.md` | 6 | § 6 item 2 and the § 9 bootstrap bullet |
| `docs/ops/RUNBOOK-observability.md` | 6 | § 2.2 row and the § 5 `defect` label key |
| `rs/crates/services/paigasus-iam/CHANGELOG.md` | 6 | `### Added` and `### Fixed` under `[Unreleased]` |
| `docs/superpowers/specs/2026-09-26-sma-698-log-jit-provisioning-failure-design.md` | 6 | § 10 names SMA-706 and SMA-707 |

---

## Task 1: Share the log rate limiter and the test log capture

A pure refactor. The validator's behavior does not change. The existing validator tests are the regression proof.

**Files:**
- Create: `rs/crates/services/paigasus-iam/src/application/log_rate_limit.rs`
- Create: `rs/crates/services/paigasus-iam/src/log_capture.rs`
- Modify: `rs/crates/services/paigasus-iam/src/lib.rs`
- Modify: `rs/crates/services/paigasus-iam/src/application/mod.rs`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/oidc/validator.rs` (lines 22-29, 57-98, 129, 149, 407-420, 950-984, 1186-1199)

**Interfaces:**
- Consumes: `paigasus_iam_core::TokenDefect` (`Copy + Eq + Hash`).
- Produces:
  - `pub(crate) const LOG_RATE_LIMIT_INTERVAL: std::time::Duration` (10 s)
  - `pub(crate) struct LogRateLimiter<K>`
  - `impl<K: Copy + Eq + Hash> LogRateLimiter<K> { pub(crate) fn new(interval: Duration) -> Self; pub(crate) fn admit_at(&self, issuer: &str, kind: K, now: Instant) -> Option<u64> }`
  - `#[cfg(test)] pub(crate) struct LogBuffer` with `pub(crate) fn text(&self) -> String`
  - `#[cfg(test)] pub(crate) fn capture_logs() -> (LogBuffer, tracing::subscriber::DefaultGuard)`

- [ ] **Step 1: Write the failing test in the new limiter module**

Create `rs/crates/services/paigasus-iam/src/application/log_rate_limit.rs` with ONLY the test module (the type does not exist yet):

```rust
// SPDX-License-Identifier: Apache-2.0

//! One rate-limit policy for diagnostic log lines (SMA-686 D14, SMA-698 D2).

#[cfg(test)]
mod tests {
    use super::*;
    use paigasus_iam_core::TokenDefect;
    use std::time::{Duration, Instant};

    #[test]
    fn refusal_log_counts_suppressed_refusals() {
        // D14 unit: admit, suppress twice within the interval, then admit with the count.
        let log = LogRateLimiter::new(Duration::from_secs(10));
        let t0 = Instant::now();
        assert_eq!(log.admit_at("iss", TokenDefect::NotAnAccessToken, t0), Some(0));
        assert_eq!(log.admit_at("iss", TokenDefect::NotAnAccessToken, t0 + Duration::from_secs(1)), None);
        assert_eq!(log.admit_at("iss", TokenDefect::NotAnAccessToken, t0 + Duration::from_secs(2)), None);
        // Independent keys: another defect and another issuer are admitted at once.
        assert_eq!(log.admit_at("iss", TokenDefect::AudienceMismatch, t0 + Duration::from_secs(2)), Some(0));
        assert_eq!(log.admit_at("other", TokenDefect::NotAnAccessToken, t0 + Duration::from_secs(2)), Some(0));
        assert_eq!(log.admit_at("iss", TokenDefect::NotAnAccessToken, t0 + Duration::from_secs(11)), Some(2));
        assert_eq!(log.admit_at("iss", TokenDefect::NotAnAccessToken, t0 + Duration::from_secs(12)), None);
    }
}
```

In `rs/crates/services/paigasus-iam/src/application/mod.rs`, add after the `pub mod fakes;` line:

```rust
pub(crate) mod log_rate_limit;
```

- [ ] **Step 2: Run the test and see it fail**

Run: `cd rs && cargo nextest run -p paigasus-iam --lib log_rate_limit`
Expected: FAIL at compile time with `error[E0433]: failed to resolve: use of undeclared type `LogRateLimiter``.

- [ ] **Step 3: Write the limiter**

In `log_rate_limit.rs`, put this between the module doc and `#[cfg(test)]`:

```rust
use std::collections::HashMap;
use std::hash::Hash;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

/// At most one log line per (issuer, kind) in this interval (SMA-686 D14).
pub(crate) const LOG_RATE_LIMIT_INTERVAL: Duration = Duration::from_secs(10);

/// Rate limit for diagnostic log lines (SMA-686 D14). One signed token can otherwise write one
/// line per request. Keyed by (issuer, kind), so the map holds at most `issuers × kinds` entries.
/// The validator uses it with `TokenDefect`, and `AuthenticateToken` with the JIT `defect` label
/// (SMA-698 D2). A suppressed event is counted, and the next admitted line reports the count.
pub(crate) struct LogRateLimiter<K> {
    interval: Duration,
    last: Mutex<HashMap<(String, K), (Instant, u64)>>,
}

impl<K: Copy + Eq + Hash> LogRateLimiter<K> {
    pub(crate) fn new(interval: Duration) -> Self {
        LogRateLimiter {
            interval,
            last: Mutex::new(HashMap::new()),
        }
    }

    /// `Some(suppressed)` when a line may be written at `now` (with the count of events
    /// suppressed since the last line); `None` when this event is suppressed and counted.
    pub(crate) fn admit_at(&self, issuer: &str, kind: K, now: Instant) -> Option<u64> {
        let mut last = self.last.lock().unwrap_or_else(PoisonError::into_inner);
        match last.get_mut(&(issuer.to_owned(), kind)) {
            Some((at, suppressed)) if now.saturating_duration_since(*at) < self.interval => {
                *suppressed += 1;
                None
            }
            Some((at, suppressed)) => {
                let count = *suppressed;
                *at = now;
                *suppressed = 0;
                Some(count)
            }
            None => {
                last.insert((issuer.to_owned(), kind), (now, 0));
                Some(0)
            }
        }
    }
}
```

In the test module, remove the line `use std::time::{Duration, Instant};` (the parent module now imports both, and `use super::*;` brings them in).

- [ ] **Step 4: Move the validator to the shared limiter**

In `rs/crates/services/paigasus-iam/src/adapters/oidc/validator.rs`:

(a) Replace lines 22-29:

```rust
use paigasus_iam_core::{Authenticator, AuthnError, Clock, Issuer, TokenDefect, ValidatedClaims};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use crate::adapters::oidc::jwks::{JwksCache, JwksFetcher, JwksProvider};
use crate::config::IssuerConfig;
```

with:

```rust
use paigasus_iam_core::{Authenticator, AuthnError, Clock, Issuer, TokenDefect, ValidatedClaims};
use serde::Deserialize;
use std::time::Instant;

use crate::adapters::oidc::jwks::{JwksCache, JwksFetcher, JwksProvider};
use crate::application::log_rate_limit::{LOG_RATE_LIMIT_INTERVAL, LogRateLimiter};
use crate::config::IssuerConfig;
```

(b) Delete lines 57-98 completely: the `/// At most one refusal log line …` doc, `const REFUSAL_LOG_INTERVAL`, the `RefusalLog` doc, `struct RefusalLog` and `impl RefusalLog`. Leave one blank line between the `BACKCHANNEL_LOGOUT_EVENT` const and the `/// What a refusal log line names …` doc.

(c) In `struct OidcAuthenticator`, replace `    refusal_log: RefusalLog,` with:

```rust
    refusal_log: LogRateLimiter<TokenDefect>,
```

(d) In `OidcAuthenticator::new`, replace `            refusal_log: RefusalLog::new(REFUSAL_LOG_INTERVAL),` with:

```rust
            refusal_log: LogRateLimiter::new(LOG_RATE_LIMIT_INTERVAL),
```

(e) Delete the test `refusal_log_counts_suppressed_refusals` (the `#[test]` at line 1186 to its closing `}` at line 1199). It now lives in `log_rate_limit.rs`.

- [ ] **Step 5: Run the limiter test and the validator tests**

Run: `cd rs && cargo nextest run -p paigasus-iam --lib log_rate_limit oidc::validator`
Expected: PASS. The limiter test passes, and every `adapters::oidc::validator::tests::*` test passes (the validator still has its own `LogBuffer` at this point).

- [ ] **Step 6: Create the shared capture module**

Create `rs/crates/services/paigasus-iam/src/log_capture.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0

//! Test-only log capture, shared by every unit test module in this crate. A test module can
//! import a `#[cfg(test)]` item from another module of the same crate (as
//! `adapters/retryable.rs`'s `tests_support` shows). Integration tests cannot, so
//! `tests/support/mod.rs` keeps its own copy.

use std::sync::{Arc, Mutex};

/// A `tracing` writer that keeps every formatted line in memory.
#[derive(Clone, Default)]
pub(crate) struct LogBuffer(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for LogBuffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogBuffer {
    type Writer = LogBuffer;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

impl LogBuffer {
    /// Everything written so far, as text.
    pub(crate) fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

/// Installs a thread-local subscriber at TRACE, so a "no personal data" assertion sees every
/// level. `#[tokio::test]` is current-thread, so the subscriber sees every task of the test.
/// Bind the guard to a named variable (`_guard`), never to `_`, or it drops at once.
pub(crate) fn capture_logs() -> (LogBuffer, tracing::subscriber::DefaultGuard) {
    let buffer = LogBuffer::default();
    let subscriber = tracing_subscriber::fmt().with_writer(buffer.clone()).with_ansi(false).with_max_level(tracing::Level::TRACE).finish();
    (buffer, tracing::subscriber::set_default(subscriber))
}
```

In `rs/crates/services/paigasus-iam/src/lib.rs`, add after `pub mod config;`:

```rust
#[cfg(test)]
mod log_capture;
```

- [ ] **Step 7: Use the shared capture in the validator tests**

In `validator.rs`, in `mod tests`, add after `use crate::adapters::oidc::jwks::{CachedJwks, InMemoryJwksCache};`:

```rust
    use crate::log_capture::capture_logs;
```

Delete the local capture block (lines 950-984 before Step 4; search for it by text): the comment `// ---- log capture (copy of the `LogBuffer` helper …` with its second line, `struct LogBuffer`, `impl std::io::Write for LogBuffer`, `impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogBuffer`, `impl LogBuffer`, and `fn capture_logs()` with its doc line. Keep `use std::sync::Arc;` in the test imports: `StubFetcher` still uses `Arc`.

- [ ] **Step 8: Run, format, lint**

Run: `cd rs && cargo fmt --all && cargo nextest run -p paigasus-iam --lib && cargo clippy --workspace --all-targets --locked -- -D warnings`
Expected: PASS. All `paigasus-iam` unit tests pass, including every `oidc::validator` test and `application::log_rate_limit::tests::refusal_log_counts_suppressed_refusals`. Clippy reports no warning.

- [ ] **Step 9: Commit**

```bash
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-698 add \
  rs/crates/services/paigasus-iam/src/application/log_rate_limit.rs \
  rs/crates/services/paigasus-iam/src/log_capture.rs \
  rs/crates/services/paigasus-iam/src/lib.rs \
  rs/crates/services/paigasus-iam/src/application/mod.rs \
  rs/crates/services/paigasus-iam/src/adapters/oidc/validator.rs
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-698 commit -F <scratchpad>/msg-task1.txt
```

Message file:

```text
refactor(rs): share the log rate limiter and the test log capture in paigasus-iam

The validator's RefusalLog becomes the generic LogRateLimiter in
application/log_rate_limit.rs, so the JIT provisioning log for SMA-698 can
use the same policy. The LogBuffer capture moves to one cfg(test) module.
The validator behavior does not change.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
```

---

## Task 2: The counter name, the label function and the prime

**Files:**
- Modify: `rs/crates/libs/paigasus-observability/src/names.rs` (after line 41, and in `ALL` after line 258)
- Modify: `rs/crates/services/paigasus-iam/src/application/authenticate_token.rs` (imports, new items after `backend_authz`, test module)
- Modify: `rs/crates/services/paigasus-iam/src/main.rs` (lines 92-93, 645-652, after line 703)

**Interfaces:**
- Consumes: `paigasus_iam_core::ProvisioningDefect` (`MissingEmail`, `EmailConflict`; `Copy + Eq`, no `Hash`).
- Produces:
  - `pub const IAM_JIT_PROVISIONING_FAILURES_TOTAL: &str = "iam_jit_provisioning_failures_total";` (in `paigasus_observability::names`, and in `ALL`)
  - `const PROVISIONING_DEFECTS: [ProvisioningDefect; 2]` (private, `authenticate_token.rs`)
  - `fn provisioning_defect_label(defect: ProvisioningDefect) -> &'static str` (private)
  - `pub fn prime_jit_provisioning_failures()` (in `paigasus_iam::application::authenticate_token`)
  - Test helpers in `authenticate_token.rs` `mod tests`: `type MetricsSnapshot`, `fn jit_failures(snapshot: &MetricsSnapshot, defect: &str) -> Option<u64>`, `fn jit_series(snapshot: &MetricsSnapshot) -> usize`

`paigasus-iam` is a lib plus a bin (`src/lib.rs`, `src/main.rs`), and `application::authenticate_token` is a `pub` module. So `pub fn prime_jit_provisioning_failures` is not dead code in the lib, and the private label function and array are used by it.

- [ ] **Step 1: Write the failing tests**

In `authenticate_token.rs`, in `mod tests`, add these imports after `use uuid::Uuid;`:

```rust
    use metrics_util::debugging::{DebugValue, DebuggingRecorder};
```

Add at the end of `mod tests` (before its closing `}`):

```rust
    type MetricsSnapshot = Vec<(metrics_util::CompositeKey, Option<metrics::Unit>, Option<metrics::SharedString>, DebugValue)>;

    /// The value of `iam_jit_provisioning_failures_total{defect}` in `snapshot`, or `None` when that
    /// series does not exist. It reads the VALUE: a series primed with `increment(0)` also has a
    /// key. `Snapshotter::snapshot` resets the counters it reads, so take ONE snapshot per test.
    fn jit_failures(snapshot: &MetricsSnapshot, defect: &str) -> Option<u64> {
        snapshot.iter().find_map(|(key, _, _, value)| {
            let key = key.key();
            let matches = key.name() == names::IAM_JIT_PROVISIONING_FAILURES_TOTAL && key.labels().any(|label| label.key() == "defect" && label.value() == defect);
            match (matches, value) {
                (false, _) => None,
                (true, DebugValue::Counter(n)) => Some(*n),
                (true, other) => panic!("expected a counter, got {other:?}"),
            }
        })
    }

    /// How many `iam_jit_provisioning_failures_total` series exist in `snapshot`, at any value.
    fn jit_series(snapshot: &MetricsSnapshot) -> usize {
        snapshot.iter().filter(|(key, ..)| key.key().name() == names::IAM_JIT_PROVISIONING_FAILURES_TOTAL).count()
    }

    /// U10 (SMA-698 spec 4.4): the prime registers both `defect` series at zero, so `increase()`
    /// sees the first failure.
    #[test]
    fn prime_registers_both_defect_series_at_zero() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        metrics::with_local_recorder(&recorder, prime_jit_provisioning_failures);
        let snapshot = snapshotter.snapshot().into_vec();
        assert_eq!(jit_failures(&snapshot, "missing_email"), Some(0));
        assert_eq!(jit_failures(&snapshot, "email_conflict"), Some(0));
        assert_eq!(jit_series(&snapshot), 2, "exactly the two defect series");
    }

    /// `PROVISIONING_DEFECTS` names every `ProvisioningDefect` once, with its own label. The
    /// `match` has no wildcard: a new variant stops this test from compiling until someone adds
    /// it here AND to `PROVISIONING_DEFECTS`.
    #[test]
    fn the_defect_array_lists_every_defect_once() {
        fn listed(defect: ProvisioningDefect) -> usize {
            match defect {
                ProvisioningDefect::MissingEmail | ProvisioningDefect::EmailConflict => PROVISIONING_DEFECTS.iter().filter(|d| **d == defect).count(),
            }
        }
        assert_eq!(listed(ProvisioningDefect::MissingEmail), 1);
        assert_eq!(listed(ProvisioningDefect::EmailConflict), 1);
        let labels: std::collections::HashSet<&str> = PROVISIONING_DEFECTS.iter().map(|d| provisioning_defect_label(*d)).collect();
        assert_eq!(labels.len(), PROVISIONING_DEFECTS.len(), "each defect needs its own label");
    }
```

- [ ] **Step 2: Run the tests and see them fail**

Run: `cd rs && cargo nextest run -p paigasus-iam --lib authenticate_token`
Expected: FAIL at compile time. The errors name the unresolved `names` module (`authenticate_token.rs` does not import it yet), `prime_jit_provisioning_failures`, `PROVISIONING_DEFECTS` and `provisioning_defect_label`.

- [ ] **Step 3: Add the metric name**

In `rs/crates/libs/paigasus-observability/src/names.rs`, add after line 41 (`pub const IAM_BOOTSTRAP_ADMIN_SEED_FAILURES_TOTAL …`):

```rust
/// SMA-698: a just-in-time provisioning attempt that IAM refused, by `defect`
/// (`missing_email` | `email_conflict`). It counts refused REQUESTS, not identities: one person
/// with a bad token makes several refused requests, because a console session makes several IAM
/// calls. The log rate limit (one `warn` line per issuer and defect in 10 s) does NOT apply to
/// this counter. A lost race between two first logins of one identity is a success and is not
/// counted. Both series are primed at zero when metrics are on
/// (`authenticate_token::prime_jit_provisioning_failures`).
pub const IAM_JIT_PROVISIONING_FAILURES_TOTAL: &str = "iam_jit_provisioning_failures_total";
```

In `ALL`, add after `    IAM_BOOTSTRAP_ADMIN_SEED_FAILURES_TOTAL,`:

```rust
    IAM_JIT_PROVISIONING_FAILURES_TOTAL,
```

- [ ] **Step 4: Add the label function, the array and the prime**

In `authenticate_token.rs`, add after the `use std::sync::Arc;` line:

```rust

use metrics::counter;
use paigasus_observability::names;
```

Add after `fn backend_authz` (before the `/// Generic-by-value over the ports …` doc):

```rust
/// Every `ProvisioningDefect` value. [`prime_jit_provisioning_failures`] registers one series for
/// each entry. `the_defect_array_lists_every_defect_once` fails to compile when a variant is added
/// and not listed.
const PROVISIONING_DEFECTS: [ProvisioningDefect; 2] = [ProvisioningDefect::MissingEmail, ProvisioningDefect::EmailConflict];

/// The `defect` label of `iam_jit_provisioning_failures_total` and the `defect` field of the JIT
/// failure log line (SMA-698 spec 4.3). One exhaustive `match`, no wildcard: the log field, the
/// counter label and the prime use this one function, so they cannot differ, and a new variant
/// does not compile until it has a label.
fn provisioning_defect_label(defect: ProvisioningDefect) -> &'static str {
    match defect {
        ProvisioningDefect::MissingEmail => "missing_email",
        ProvisioningDefect::EmailConflict => "email_conflict",
    }
}

/// Registers every `defect` series of `iam_jit_provisioning_failures_total` at zero (SMA-698
/// spec 4.4). A metrics-rs series first appears at its first increment's value, and `increase()`
/// takes the first sample as its baseline, so without this the first failure is invisible to an
/// `increase() > 0` query. `main` calls this when metrics are on, even if no issuer has JIT on.
pub fn prime_jit_provisioning_failures() {
    for defect in PROVISIONING_DEFECTS {
        counter!(names::IAM_JIT_PROVISIONING_FAILURES_TOTAL, "defect" => provisioning_defect_label(defect)).increment(0);
    }
}
```

- [ ] **Step 5: Describe and prime in `main.rs`**

In `rs/crates/services/paigasus-iam/src/main.rs`, replace:

```rust
    if metrics_handle.is_some() {
        describe_iam_metrics();
```

with:

```rust
    if metrics_handle.is_some() {
        describe_iam_metrics();
        // SMA-698: both `defect` series start at zero, so `increase()` sees the first JIT failure.
        paigasus_iam::application::authenticate_token::prime_jit_provisioning_failures();
```

Replace the doc lines 645-649:

```rust
/// Registers `# HELP`/`# TYPE` exposition text for the 38 metric families `paigasus-iam` emits
/// directly (spec §4.1; includes the SMA-467 audit partition-maintenance families, the
/// SMA-469 outbox retention/dead-letter families, the SMA-476 Redis circuit-breaker families,
/// the SMA-481 system-row-retirement family, the SMA-471 NATS publisher families, and the
/// SMA-489 commit-nudge/listener families and the SMA-495 notifying-enqueue family), via the
```

with:

```rust
/// Registers `# HELP`/`# TYPE` exposition text for the 39 metric families `paigasus-iam` emits
/// directly (spec §4.1; includes the SMA-467 audit partition-maintenance families, the
/// SMA-469 outbox retention/dead-letter families, the SMA-476 Redis circuit-breaker families,
/// the SMA-481 system-row-retirement family, the SMA-471 NATS publisher families, the
/// SMA-489 commit-nudge/listener families, the SMA-495 notifying-enqueue family and the SMA-698
/// JIT provisioning-failure family), via the
```

After the `describe_counter!(names::IAM_BOOTSTRAP_ADMIN_SEED_FAILURES_TOTAL, …);` block, add:

```rust
    describe_counter!(
        names::IAM_JIT_PROVISIONING_FAILURES_TOTAL,
        "Just-in-time provisioning attempts that IAM refused, labeled by defect (missing_email/email_conflict). Counts refused requests, not identities. The log rate limit does not apply."
    );
```

- [ ] **Step 6: Confirm the family count**

Run: `awk '/^fn describe_iam_metrics/,/^}/' rs/crates/services/paigasus-iam/src/main.rs | grep -c "describe_counter!\|describe_gauge!\|describe_histogram!"`
Expected: `39`.

- [ ] **Step 7: Run, format, lint**

Run: `cd rs && cargo fmt --all && cargo nextest run -p paigasus-iam --lib authenticate_token && cargo nextest run -p paigasus-observability && cargo clippy --workspace --all-targets --locked -- -D warnings`
Expected: PASS. `prime_registers_both_defect_series_at_zero` and `the_defect_array_lists_every_defect_once` pass; `names::tests::all_names_are_unique_and_snake_case` and `tests/drift.rs` pass; no clippy warning.

- [ ] **Step 8: Commit**

```bash
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-698 add \
  rs/crates/libs/paigasus-observability/src/names.rs \
  rs/crates/services/paigasus-iam/src/application/authenticate_token.rs \
  rs/crates/services/paigasus-iam/src/main.rs
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-698 commit -F <scratchpad>/msg-task2.txt
```

Message file:

```text
feat(rs): add the iam_jit_provisioning_failures_total counter and prime it

The counter has one label, defect, with the values missing_email and
email_conflict from one exhaustive label function. main describes the
family and primes both series at zero when metrics are on (SMA-698).

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
```

---

## Task 3: The helper: log line, counter and rate limit

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/application/authenticate_token.rs`

**Interfaces:**
- Consumes: `LogRateLimiter<&'static str>`, `LOG_RATE_LIMIT_INTERVAL` (Task 1); `provisioning_defect_label`, `names::IAM_JIT_PROVISIONING_FAILURES_TOTAL` (Task 2); `crate::log_capture::capture_logs` (Task 1, tests); `jit_failures`, `jit_series`, `MetricsSnapshot` (Task 2, tests).
- Produces (all private to the module):
  - `enum EmailClaim { Absent, Invalid }` with `fn label(self) -> &'static str`
  - `enum JitFailure { MissingEmail(EmailClaim), EmailConflict }` with `fn defect(self) -> ProvisioningDefect`
  - field `provisioning_log: Arc<LogRateLimiter<&'static str>>` on `AuthenticateToken`
  - `fn provisioning_failed(&self, issuer: &Issuer, failure: JitFailure) -> AuthnError`
  - `AuthenticateToken::new` keeps its signature.

- [ ] **Step 1: Write the failing tests**

In `mod tests`, add these imports after `use metrics_util::debugging::{DebugValue, DebuggingRecorder};`:

```rust
    use crate::adapters::id::KernelIdGenerator;
    use crate::log_capture::capture_logs;
    use std::collections::VecDeque;
```

`KernelIdGenerator` is the only `Clone` id generator in the crate; U13 needs a `Clone` use case. This is a test-only import.

Replace the `InMemoryMemberships` struct (its `#[derive(Default)]` and the struct body) with:

```rust
    #[derive(Clone, Default)]
    struct InMemoryMemberships {
        rows: Arc<Mutex<HashMap<Uuid, Vec<MembershipRecord>>>>,
    }
```

Its `seed` and `list_by_principal` bodies do not change (`self.rows.lock()` goes through the `Arc`).

Add after the `FakeAuthenticator` impl block:

```rust
    /// Yields its claims in order, one per call, for tests that resolve more than once. `Clone`
    /// shares the queue, so a cloned use case reads from the same queue (U13).
    #[derive(Clone)]
    struct QueueAuthenticator(Arc<Mutex<VecDeque<ValidatedClaims>>>);

    impl QueueAuthenticator {
        fn new(claims: Vec<ValidatedClaims>) -> Self {
            QueueAuthenticator(Arc::new(Mutex::new(claims.into())))
        }
    }

    #[async_trait]
    impl Authenticator for QueueAuthenticator {
        async fn authenticate(&self, _token: &str) -> Result<ValidatedClaims, AuthnError> {
            Ok(self.0.lock().unwrap().pop_front().expect("QueueAuthenticator ran out of claims"))
        }
    }

    /// The initial lookup misses, and `provision` fails with a repository error that is not a
    /// conflict (U7). `RepositoryError::Backend` wraps a boxed error.
    struct FailingProvisionIdentities;

    #[async_trait]
    impl ExternalIdentityRepository for FailingProvisionIdentities {
        async fn find_by_issuer_subject(&self, _issuer: &Issuer, _subject: &str) -> Result<Option<ExternalIdentity>, RepositoryError> {
            Ok(None)
        }
        async fn provision(&self, _principal: &Principal, _user: &User, _identity: &ExternalIdentity) -> Result<(), RepositoryError> {
            Err(RepositoryError::Backend("database connection lost".into()))
        }
    }
```

Add at the end of `mod tests`, after the Task 2 tests:

```rust
    const ISSUER: &str = "https://idp.example.com";
    const OTHER_ISSUER: &str = "https://other-idp.example.com";
    /// The fixed prefix of both helper messages. Tests select the helper's lines with it.
    const JIT_LINE: &str = "just-in-time provisioning failed";
    const MISSING_EMAIL_TEXT: &str =
        "just-in-time provisioning failed: the access token has no valid email claim; configure the issuer to put a valid email claim into the access token";
    const EMAIL_CONFLICT_TEXT: &str = "just-in-time provisioning failed: another user already has this email address, and IAM does not link identities by email";

    /// The helper's lines only. `AppState::new` and other code can write other `warn` lines.
    fn jit_lines(text: &str) -> Vec<&str> {
        text.lines().filter(|line| line.contains(JIT_LINE)).collect()
    }

    /// `true` when `line` carries `key` with `value`. `tracing-subscriber` writes a `&str` field
    /// as `key="value"` and a number as `key=value`; both forms are accepted.
    fn has_field(line: &str, key: &str, value: &str) -> bool {
        line.contains(&format!("{key}=\"{value}\"")) || line.contains(&format!("{key}={value}"))
    }

    /// Asserts that the WHOLE capture holds none of `secrets` (spec S2, 6.1).
    fn assert_no_secrets(text: &str, secrets: &[&str]) {
        for secret in secrets {
            assert!(!text.contains(secret), "the log must not contain {secret:?}:\n{text}");
        }
    }

    /// A user that already owns `email`, with no external identity: an email conflict for anyone else.
    fn seed_user(store: &AuthnStore, n: u128, email: &str) {
        let id = principal_id(n);
        let principal = Principal::new(id.clone(), PrincipalKind::User, PrincipalStatus::Active, epoch(), epoch());
        let user = User::new(id.clone(), Email::parse(email).unwrap(), "Existing".into(), None, None, epoch(), epoch());
        store.principals.lock().unwrap().insert(id.uuid(), (principal, user));
    }

    /// A use case over `store`, with JIT on for `ISSUER`.
    fn jit_use_case<A: Authenticator>(authenticator: A, store: &AuthnStore) -> AuthenticateToken<A, InMemoryIdentities, InMemoryPrincipals, InMemoryMemberships, SeqIds, FixedClock> {
        AuthenticateToken::new(
            authenticator,
            InMemoryIdentities(store.clone()),
            InMemoryPrincipals(store.clone()),
            InMemoryMemberships::default(),
            Arc::new(InMemoryRoleGrants::default()),
            SeqIds::default(),
            FixedClock::default(),
            JitPolicy::from_issuers(&[(Issuer::parse(ISSUER).unwrap(), true)]),
        )
    }

    /// U1: no `email` claim. One `warn` line with the defect, `absent` and the issuer; one count.
    #[tokio::test]
    async fn jit_missing_email_writes_one_warn_line_and_counts_once() {
        let (logs, _logs_guard) = capture_logs();
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _metrics_guard = metrics::set_default_local_recorder(&recorder);
        let store = AuthnStore::default();
        let uc = jit_use_case(FakeAuthenticator::ok(claims(ISSUER, "sub-u1-distinct", None, Some("Nora Distinctname"))), &store);

        let err = uc.resolve("bearer-u1-secret", Provisioning::Enabled).await.unwrap_err();

        assert!(matches!(err, AuthnError::ProvisioningFailed(ProvisioningDefect::MissingEmail)), "got {err:?}");
        let text = logs.text();
        let lines = jit_lines(&text);
        assert_eq!(lines.len(), 1, "exactly one helper line expected:\n{text}");
        let line = lines[0];
        assert!(line.contains("WARN"), "the helper logs at warn: {line}");
        assert!(line.contains(MISSING_EMAIL_TEXT), "the fixed missing_email message: {line}");
        assert!(has_field(line, "defect", "missing_email"), "names the defect: {line}");
        assert!(has_field(line, "email_claim", "absent"), "names the claim state: {line}");
        assert!(has_field(line, "issuer", ISSUER), "names the issuer: {line}");
        assert!(has_field(line, "suppressed", "0"), "the first line suppressed nothing: {line}");
        assert_no_secrets(&text, &["sub-u1-distinct", "Nora Distinctname", "bearer-u1-secret"]);
        let snapshot = snapshotter.snapshot().into_vec();
        assert_eq!(jit_failures(&snapshot, "missing_email"), Some(1));
        assert_eq!(jit_series(&snapshot), 1, "no email_conflict series");
    }

    /// U2: an email-like claim that fails `Email::parse` (a second `@`). `invalid`, and the
    /// capture holds no part of the claim.
    #[tokio::test]
    async fn jit_invalid_email_claim_writes_one_warn_line_without_the_claim() {
        let (logs, _logs_guard) = capture_logs();
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _metrics_guard = metrics::set_default_local_recorder(&recorder);
        let store = AuthnStore::default();
        let uc = jit_use_case(FakeAuthenticator::ok(claims(ISSUER, "sub-u2-distinct", Some("alice.distinct@example.com@x"), Some("Otto Distinctname"))), &store);

        let err = uc.resolve("bearer-u2-secret", Provisioning::Enabled).await.unwrap_err();

        assert!(matches!(err, AuthnError::ProvisioningFailed(ProvisioningDefect::MissingEmail)), "got {err:?}");
        let text = logs.text();
        let lines = jit_lines(&text);
        assert_eq!(lines.len(), 1, "exactly one helper line expected:\n{text}");
        let line = lines[0];
        assert!(line.contains("WARN"), "the helper logs at warn: {line}");
        assert!(line.contains(MISSING_EMAIL_TEXT), "the fixed missing_email message: {line}");
        assert!(has_field(line, "defect", "missing_email"), "names the defect: {line}");
        assert!(has_field(line, "email_claim", "invalid"), "names the claim state: {line}");
        assert!(has_field(line, "issuer", ISSUER), "names the issuer: {line}");
        assert_no_secrets(&text, &["alice.distinct", "sub-u2-distinct", "Otto Distinctname", "bearer-u2-secret"]);
        let snapshot = snapshotter.snapshot().into_vec();
        assert_eq!(jit_failures(&snapshot, "missing_email"), Some(1));
    }

    /// U3: another principal already has the email, and the identity is absent.
    #[tokio::test]
    async fn jit_email_conflict_writes_one_warn_line_without_the_email() {
        let (logs, _logs_guard) = capture_logs();
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _metrics_guard = metrics::set_default_local_recorder(&recorder);
        let store = AuthnStore::default();
        seed_user(&store, 7, "taken.distinct@example.com");
        let uc = jit_use_case(FakeAuthenticator::ok(claims(ISSUER, "sub-u3-distinct", Some("taken.distinct@example.com"), Some("Paula Distinctname"))), &store);

        let err = uc.resolve("bearer-u3-secret", Provisioning::Enabled).await.unwrap_err();

        assert!(matches!(err, AuthnError::ProvisioningFailed(ProvisioningDefect::EmailConflict)), "got {err:?}");
        let text = logs.text();
        let lines = jit_lines(&text);
        assert_eq!(lines.len(), 1, "exactly one helper line expected:\n{text}");
        let line = lines[0];
        assert!(line.contains("WARN"), "the helper logs at warn: {line}");
        assert!(line.contains(EMAIL_CONFLICT_TEXT), "the fixed email_conflict message: {line}");
        assert!(has_field(line, "defect", "email_conflict"), "names the defect: {line}");
        assert!(has_field(line, "issuer", ISSUER), "names the issuer: {line}");
        assert!(!line.contains("email_claim"), "email_claim belongs to missing_email only: {line}");
        assert_no_secrets(&text, &["taken.distinct", "sub-u3-distinct", "Paula Distinctname", "bearer-u3-secret"]);
        let snapshot = snapshotter.snapshot().into_vec();
        assert_eq!(jit_failures(&snapshot, "email_conflict"), Some(1));
        assert_eq!(jit_series(&snapshot), 1, "no missing_email series");
    }

    /// U4: a successful JIT provision writes no helper line and makes no series.
    #[tokio::test]
    async fn jit_success_writes_no_line_and_no_series() {
        let (logs, _logs_guard) = capture_logs();
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _metrics_guard = metrics::set_default_local_recorder(&recorder);
        let store = AuthnStore::default();
        let uc = jit_use_case(FakeAuthenticator::ok(claims(ISSUER, "sub-u4", Some("u4@example.com"), Some("U Four"))), &store);

        uc.resolve("token", Provisioning::Enabled).await.unwrap();

        let text = logs.text();
        assert!(jit_lines(&text).is_empty(), "a success writes no helper line:\n{text}");
        assert_eq!(jit_series(&snapshotter.snapshot().into_vec()), 0);
    }

    /// U7: `provision` fails with a repository error that is not a conflict. `Backend`, no helper
    /// line, no series (the adapters already log `Backend`).
    #[tokio::test]
    async fn jit_backend_error_writes_no_line_and_no_series() {
        let (logs, _logs_guard) = capture_logs();
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _metrics_guard = metrics::set_default_local_recorder(&recorder);
        let uc = AuthenticateToken::new(
            FakeAuthenticator::ok(claims(ISSUER, "sub-u7", Some("u7@example.com"), None)),
            FailingProvisionIdentities,
            PanicIfCalledPrincipals,
            InMemoryMemberships::default(),
            Arc::new(InMemoryRoleGrants::default()),
            SeqIds::default(),
            FixedClock::default(),
            JitPolicy::from_issuers(&[(Issuer::parse(ISSUER).unwrap(), true)]),
        );

        let err = uc.resolve("token", Provisioning::Enabled).await.unwrap_err();

        assert!(matches!(err, AuthnError::Backend(_)), "got {err:?}");
        let text = logs.text();
        assert!(jit_lines(&text).is_empty(), "a backend error writes no helper line:\n{text}");
        assert_eq!(jit_series(&snapshotter.snapshot().into_vec()), 0);
    }

    /// U8: a JIT-disabled issuer, and `introspect` for an unknown identity. Both return
    /// `IdentityNotProvisioned` before `jit_provision` runs: no helper line, no series.
    #[tokio::test]
    async fn identity_not_provisioned_writes_no_line_and_no_series() {
        let (logs, _logs_guard) = capture_logs();
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _metrics_guard = metrics::set_default_local_recorder(&recorder);
        let store = AuthnStore::default();
        let disabled = AuthenticateToken::new(
            FakeAuthenticator::ok(claims(ISSUER, "sub-u8-a", None, None)),
            InMemoryIdentities(store.clone()),
            InMemoryPrincipals(store.clone()),
            InMemoryMemberships::default(),
            Arc::new(InMemoryRoleGrants::default()),
            SeqIds::default(),
            FixedClock::default(),
            JitPolicy::from_issuers(&[(Issuer::parse(ISSUER).unwrap(), false)]),
        );
        let introspecting = jit_use_case(FakeAuthenticator::ok(claims(ISSUER, "sub-u8-b", None, None)), &store);

        let err = disabled.resolve("token", Provisioning::Enabled).await.unwrap_err();
        assert!(matches!(err, AuthnError::IdentityNotProvisioned), "got {err:?}");
        let err = introspecting.introspect("token").await.unwrap_err();
        assert!(matches!(err, AuthnError::IdentityNotProvisioned), "got {err:?}");

        let text = logs.text();
        assert!(jit_lines(&text).is_empty(), "IdentityNotProvisioned writes no helper line:\n{text}");
        assert_eq!(jit_series(&snapshotter.snapshot().into_vec()), 0);
    }

    /// U9 (spec S1, D2): three `missing_email` failures for one issuer in one window. One line,
    /// three counts: the rate limit does not apply to the counter.
    #[tokio::test]
    async fn jit_repeated_failures_log_once_and_count_each() {
        let (logs, _logs_guard) = capture_logs();
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _metrics_guard = metrics::set_default_local_recorder(&recorder);
        let store = AuthnStore::default();
        let uc = jit_use_case(
            QueueAuthenticator::new(vec![claims(ISSUER, "sub-u9", None, None), claims(ISSUER, "sub-u9", None, None), claims(ISSUER, "sub-u9", None, None)]),
            &store,
        );

        for _ in 0..3 {
            let err = uc.resolve("token", Provisioning::Enabled).await.unwrap_err();
            assert!(matches!(err, AuthnError::ProvisioningFailed(ProvisioningDefect::MissingEmail)), "got {err:?}");
        }

        let text = logs.text();
        assert_eq!(jit_lines(&text).len(), 1, "one line per (issuer, defect) in the window:\n{text}");
        assert_no_secrets(&text, &["sub-u9"]);
        assert_eq!(jit_failures(&snapshotter.snapshot().into_vec(), "missing_email"), Some(3));
    }

    /// U11 (Review Focus R1): the limiter key holds the issuer. Two issuers, two lines.
    #[tokio::test]
    async fn jit_failures_from_two_issuers_log_one_line_each() {
        let (logs, _logs_guard) = capture_logs();
        let store = AuthnStore::default();
        let uc = AuthenticateToken::new(
            QueueAuthenticator::new(vec![claims(ISSUER, "sub-u11-a", None, None), claims(OTHER_ISSUER, "sub-u11-b", None, None)]),
            InMemoryIdentities(store.clone()),
            InMemoryPrincipals(store.clone()),
            InMemoryMemberships::default(),
            Arc::new(InMemoryRoleGrants::default()),
            SeqIds::default(),
            FixedClock::default(),
            JitPolicy::from_issuers(&[(Issuer::parse(ISSUER).unwrap(), true), (Issuer::parse(OTHER_ISSUER).unwrap(), true)]),
        );

        uc.resolve("token", Provisioning::Enabled).await.unwrap_err();
        uc.resolve("token", Provisioning::Enabled).await.unwrap_err();

        let text = logs.text();
        let lines = jit_lines(&text);
        assert_eq!(lines.len(), 2, "one line for each issuer:\n{text}");
        assert!(lines.iter().any(|line| has_field(line, "issuer", ISSUER)), "{text}");
        assert!(lines.iter().any(|line| has_field(line, "issuer", OTHER_ISSUER)), "{text}");
        assert_no_secrets(&text, &["sub-u11-a", "sub-u11-b"]);
    }

    /// U12 (Review Focus R2): the limiter key holds the defect. Two defects for one issuer, two lines.
    #[tokio::test]
    async fn jit_two_defects_for_one_issuer_log_one_line_each() {
        let (logs, _logs_guard) = capture_logs();
        let store = AuthnStore::default();
        seed_user(&store, 8, "taken.u12@example.com");
        let uc = jit_use_case(
            QueueAuthenticator::new(vec![claims(ISSUER, "sub-u12-a", None, None), claims(ISSUER, "sub-u12-b", Some("taken.u12@example.com"), None)]),
            &store,
        );

        let err = uc.resolve("token", Provisioning::Enabled).await.unwrap_err();
        assert!(matches!(err, AuthnError::ProvisioningFailed(ProvisioningDefect::MissingEmail)), "got {err:?}");
        let err = uc.resolve("token", Provisioning::Enabled).await.unwrap_err();
        assert!(matches!(err, AuthnError::ProvisioningFailed(ProvisioningDefect::EmailConflict)), "got {err:?}");

        let text = logs.text();
        let lines = jit_lines(&text);
        assert_eq!(lines.len(), 2, "one line for each defect:\n{text}");
        assert!(lines.iter().any(|line| has_field(line, "defect", "missing_email")), "{text}");
        assert!(lines.iter().any(|line| has_field(line, "defect", "email_conflict")), "{text}");
        assert_no_secrets(&text, &["sub-u12-a", "sub-u12-b", "taken.u12"]);
    }

    /// U13 (Review Focus R3, spec 4.5): `AppState` clones the use case for each request. The clone
    /// shares the limiter through its `Arc`, so a failure on the clone inside the window is suppressed.
    #[tokio::test]
    async fn a_cloned_use_case_shares_the_rate_limiter() {
        let (logs, _logs_guard) = capture_logs();
        let store = AuthnStore::default();
        let uc = AuthenticateToken::new(
            QueueAuthenticator::new(vec![claims(ISSUER, "sub-u13-a", None, None), claims(ISSUER, "sub-u13-b", None, None)]),
            InMemoryIdentities(store.clone()),
            InMemoryPrincipals(store.clone()),
            InMemoryMemberships::default(),
            Arc::new(InMemoryRoleGrants::default()),
            KernelIdGenerator,
            FixedClock::default(),
            JitPolicy::from_issuers(&[(Issuer::parse(ISSUER).unwrap(), true)]),
        );
        let twin = uc.clone();

        uc.resolve("token", Provisioning::Enabled).await.unwrap_err();
        twin.resolve("token", Provisioning::Enabled).await.unwrap_err();

        let text = logs.text();
        assert_eq!(jit_lines(&text).len(), 1, "the clone must share the limiter:\n{text}");
        assert_no_secrets(&text, &["sub-u13-a", "sub-u13-b"]);
    }

    /// U14 (Review Focus R4): an `email` claim that is present but empty is `invalid`, not `absent`.
    #[tokio::test]
    async fn jit_empty_email_claim_is_invalid_not_absent() {
        let (logs, _logs_guard) = capture_logs();
        let store = AuthnStore::default();
        let uc = jit_use_case(FakeAuthenticator::ok(claims(ISSUER, "sub-u14", Some(""), None)), &store);

        let err = uc.resolve("token", Provisioning::Enabled).await.unwrap_err();

        assert!(matches!(err, AuthnError::ProvisioningFailed(ProvisioningDefect::MissingEmail)), "got {err:?}");
        let text = logs.text();
        let lines = jit_lines(&text);
        assert_eq!(lines.len(), 1, "exactly one helper line expected:\n{text}");
        assert!(has_field(lines[0], "email_claim", "invalid"), "a present, empty claim is invalid: {}", lines[0]);
        assert_no_secrets(&text, &["sub-u14"]);
    }
```

Replace the whole existing test `provision_race_loser_reuses_winner_row` (U5) with:

```rust
    /// U5: the lost race in its `ExternalIdentityExists` form resolves to the winner, and it
    /// writes no helper line and makes no series (spec 4.7).
    #[tokio::test]
    async fn provision_race_loser_reuses_winner_row() {
        let (logs, _logs_guard) = capture_logs();
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _metrics_guard = metrics::set_default_local_recorder(&recorder);
        let store = AuthnStore::default();
        let issuer = Issuer::parse("https://idp.example.com").unwrap();
        let winner_id = principal_id(42);
        let winner_principal = Principal::new(winner_id.clone(), PrincipalKind::User, PrincipalStatus::Active, epoch(), epoch());
        let winner_user = User::new(winner_id.clone(), Email::parse("racer@example.com").unwrap(), "Racer".into(), None, None, epoch(), epoch());
        store.principals.lock().unwrap().insert(winner_id.uuid(), (winner_principal, winner_user));
        store.identities.lock().unwrap().insert(
            (issuer.as_str().to_string(), "sub-race".to_string()),
            ExternalIdentity {
                id: Uuid::from_u128(4242),
                principal_id: winner_id.clone(),
                issuer: issuer.clone(),
                subject: "sub-race".into(),
                created_at: epoch(),
                updated_at: epoch(),
            },
        );

        let uc = AuthenticateToken::new(
            FakeAuthenticator::ok(claims("https://idp.example.com", "sub-race", Some("racer2@example.com"), Some("Racer Two"))),
            RaceOnceIdentities::new(InMemoryIdentities(store.clone())),
            InMemoryPrincipals(store.clone()),
            InMemoryMemberships::default(),
            Arc::new(InMemoryRoleGrants::default()),
            SeqIds::default(),
            FixedClock::default(),
            JitPolicy::from_issuers(&[(issuer.clone(), true)]),
        );

        let resolved = uc.resolve("token", Provisioning::Enabled).await.unwrap();
        assert_eq!(resolved.principal_id, winner_id);
        // The loser's own (different-email) user must never have been persisted.
        assert!(!store.principals.lock().unwrap().values().any(|(_, u)| u.email.as_str() == "racer2@example.com"));
        let text = logs.text();
        assert!(jit_lines(&text).is_empty(), "a lost race writes no helper line:\n{text}");
        assert_eq!(jit_series(&snapshotter.snapshot().into_vec()), 0, "a lost race is not counted");
    }
```

- [ ] **Step 2: Run the tests and see them fail**

Run: `cd rs && cargo nextest run -p paigasus-iam --lib --no-fail-fast authenticate_token`
Expected: FAIL. These fail with `exactly one helper line expected` (or `one line …` / `the clone must share the limiter`), because nothing logs yet: `jit_missing_email_writes_one_warn_line_and_counts_once`, `jit_invalid_email_claim_writes_one_warn_line_without_the_claim`, `jit_email_conflict_writes_one_warn_line_without_the_email`, `jit_repeated_failures_log_once_and_count_each`, `jit_failures_from_two_issuers_log_one_line_each`, `jit_two_defects_for_one_issuer_log_one_line_each`, `a_cloned_use_case_shares_the_rate_limiter`, `jit_empty_email_claim_is_invalid_not_absent`. These pass already, because they are negative tests: `jit_success_writes_no_line_and_no_series`, `jit_backend_error_writes_no_line_and_no_series`, `identity_not_provisioned_writes_no_line_and_no_series`, `provision_race_loser_reuses_winner_row`. Task 7 proves that each of them can go red.

- [ ] **Step 3: Write the helper**

In `authenticate_token.rs`, add these imports after `use std::sync::Arc;`:

```rust
use std::time::Instant;

use crate::application::log_rate_limit::{LOG_RATE_LIMIT_INTERVAL, LogRateLimiter};
```

Add after `prime_jit_provisioning_failures` (before the `/// Generic-by-value …` doc):

```rust
/// The state of the `email` claim when JIT provisioning failed. It never holds the claim value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EmailClaim {
    /// The token has no `email` claim.
    Absent,
    /// The token has an `email` claim, and `Email::parse` refused it.
    Invalid,
}

impl EmailClaim {
    fn label(self) -> &'static str {
        match self {
            EmailClaim::Absent => "absent",
            EmailClaim::Invalid => "invalid",
        }
    }
}

/// A JIT provisioning failure with its detail (SMA-698 spec 4.2). An invalid combination, such as
/// an `EmailConflict` with an `EmailClaim`, cannot be built.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JitFailure {
    MissingEmail(EmailClaim),
    EmailConflict,
}

impl JitFailure {
    fn defect(self) -> ProvisioningDefect {
        match self {
            JitFailure::MissingEmail(_) => ProvisioningDefect::MissingEmail,
            JitFailure::EmailConflict => ProvisioningDefect::EmailConflict,
        }
    }
}
```

In `struct AuthenticateToken`, add after `jit: JitPolicy,`:

```rust
    /// SMA-698 D2: at most one JIT failure line per (issuer, defect) in 10 s. An `Arc`, because
    /// `AppState` clones this use case for each request, and a plain field would reset on each
    /// clone. Keyed by the `defect` label, because `ProvisioningDefect` does not implement `Hash`.
    provisioning_log: Arc<LogRateLimiter<&'static str>>,
```

In `AuthenticateToken::new`, add after `            jit,`:

```rust
            provisioning_log: Arc::new(LogRateLimiter::new(LOG_RATE_LIMIT_INTERVAL)),
```

In `jit_provision`, replace:

```rust
        let email = match claims.email.as_deref().map(Email::parse) {
            Some(Ok(email)) => email,
            _ => return Err(AuthnError::ProvisioningFailed(ProvisioningDefect::MissingEmail)),
        };
```

with:

```rust
        // The `Email::parse` error is dropped here on purpose: `DomainError::InvalidEmail` holds
        // the raw claim, and the helper must never see it (SMA-698 spec 4.3).
        let email = match claims.email.as_deref().map(Email::parse) {
            Some(Ok(email)) => email,
            Some(Err(_)) => return Err(self.provisioning_failed(&claims.issuer, JitFailure::MissingEmail(EmailClaim::Invalid))),
            None => return Err(self.provisioning_failed(&claims.issuer, JitFailure::MissingEmail(EmailClaim::Absent))),
        };
```

and replace:

```rust
            Err(RepositoryError::Conflict(ConflictKind::EmailTaken)) => Err(AuthnError::ProvisioningFailed(ProvisioningDefect::EmailConflict)),
```

with:

```rust
            Err(RepositoryError::Conflict(ConflictKind::EmailTaken)) => Err(self.provisioning_failed(&claims.issuer, JitFailure::EmailConflict)),
```

Add after `jit_provision` (inside the same `impl` block):

```rust
    /// The only way a JIT failure arm builds its error (SMA-698 spec 4.2). It counts the failure,
    /// asks the rate limiter, writes one `warn` line when admitted, and returns
    /// `ProvisioningFailed`. It never receives the email, the subject, the name or the token, so
    /// the line cannot carry them (spec S2).
    fn provisioning_failed(&self, issuer: &Issuer, failure: JitFailure) -> AuthnError {
        let defect = failure.defect();
        let label = provisioning_defect_label(defect);
        counter!(names::IAM_JIT_PROVISIONING_FAILURES_TOTAL, "defect" => label).increment(1);
        if let Some(suppressed) = self.provisioning_log.admit_at(issuer.as_str(), label, Instant::now()) {
            match failure {
                JitFailure::MissingEmail(claim) => tracing::warn!(
                    defect = label,
                    issuer = issuer.as_str(),
                    email_claim = claim.label(),
                    suppressed,
                    "just-in-time provisioning failed: the access token has no valid email claim; configure the issuer to put a valid email claim into the access token"
                ),
                JitFailure::EmailConflict => tracing::warn!(
                    defect = label,
                    issuer = issuer.as_str(),
                    suppressed,
                    "just-in-time provisioning failed: another user already has this email address, and IAM does not link identities by email"
                ),
            }
        }
        AuthnError::ProvisioningFailed(defect)
    }
```

- [ ] **Step 4: Run the tests and see them pass**

Run: `cd rs && cargo fmt --all && cargo nextest run -p paigasus-iam --lib authenticate_token`
Expected: PASS for every `application::authenticate_token::tests::*` test, including the old `missing_email_fails_provisioning`, `unparseable_email_fails_provisioning_as_missing_email` and `email_conflict_maps_to_provisioning_failed`.

- [ ] **Step 5: Add U15 (Review Focus R5)**

Add at the end of `mod tests`:

```rust
    /// U15 (Review Focus R5): the next admitted line carries the limiter's `suppressed` count.
    /// The test module can reach the private `provisioning_log`, so it injects two earlier
    /// failures in an old window instead of waiting 10 s.
    #[tokio::test]
    async fn the_next_admitted_line_carries_the_suppressed_count() {
        let (logs, _logs_guard) = capture_logs();
        let store = AuthnStore::default();
        let uc = jit_use_case(FakeAuthenticator::ok(claims(ISSUER, "sub-u15", None, None)), &store);
        let old = Instant::now().checked_sub(std::time::Duration::from_secs(30)).expect("the monotonic clock is older than 30 s");
        assert_eq!(uc.provisioning_log.admit_at(ISSUER, "missing_email", old), Some(0));
        assert_eq!(uc.provisioning_log.admit_at(ISSUER, "missing_email", old + std::time::Duration::from_secs(1)), None);

        uc.resolve("token", Provisioning::Enabled).await.unwrap_err();

        let text = logs.text();
        let lines = jit_lines(&text);
        assert_eq!(lines.len(), 1, "the window is over, so the line is admitted:\n{text}");
        assert!(has_field(lines[0], "suppressed", "1"), "the line reports the one suppressed failure: {}", lines[0]);
        assert_no_secrets(&text, &["sub-u15"]);
    }
```

Run: `cd rs && cargo nextest run -p paigasus-iam --lib the_next_admitted_line_carries_the_suppressed_count`
Expected: PASS.

- [ ] **Step 6: Run the crate's unit tests, format, lint**

Run: `cd rs && cargo fmt --all && cargo nextest run -p paigasus-iam --lib && cargo clippy --workspace --all-targets --locked -- -D warnings`
Expected: PASS, and no clippy warning.

- [ ] **Step 7: Commit**

```bash
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-698 add rs/crates/services/paigasus-iam/src/application/authenticate_token.rs
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-698 commit -F <scratchpad>/msg-task3.txt
```

Message file:

```text
feat(rs): log and count a JIT provisioning failure (SMA-698)

A refused just-in-time provisioning now writes one warn line that names the
defect and the issuer, and for missing_email the state of the email claim.
The line holds no email, subject, name or token. One line per issuer and
defect in 10 s, with the suppressed count. iam_jit_provisioning_failures_total
counts every refused request. Both transports reach the one helper through
resolve with provisioning enabled.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
```

---

## Task 4: The lost race on the email is a success (spec 4.6, D1)

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/application/authenticate_token.rs`
- Modify: `rs/crates/services/paigasus-iam/tests/support/mod.rs` (append)
- Modify: `rs/crates/services/paigasus-iam/tests/authn_identities.rs`

**Interfaces:**
- Consumes: `ExternalIdentityRepository::find_by_issuer_subject`; `support::race::{backend_pid, expect_racer_blocked}` (`tests/support/race.rs`); `paigasus_iam::adapters::persistence::entities::{principal, user, external_identity}` (all `pub`); `paigasus_iam::adapters::http::AppState` (`authn: AuthnSvc`, `Clone`).
- Produces:
  - In `tests/support/mod.rs`: `pub struct LogBuffer`, `pub fn LogBuffer::text(&self) -> String`, `pub fn capture_logs() -> (LogBuffer, tracing::subscriber::DefaultGuard)`, `pub const JIT_FAILURE_LINE: &str = "just-in-time provisioning failed"`.
  - Behavior: `resolve(.., Enabled)` returns the winner's principal after `Conflict(EmailTaken)` when the identity exists at the re-read.

- [ ] **Step 1: Write U6 (the unit form)**

In `authenticate_token.rs` `mod tests`, add after the `RaceOnceIdentities` impl block:

```rust
    /// The Postgres form of a lost race (SMA-698 spec 4.6). The winner commits all three rows;
    /// the loser's `user` insert fails on the email first, so its `provision` returns
    /// `Conflict(EmailTaken)`. `provision` inserts the winner's identity into the shared store
    /// and then returns that error, so the identity is present at the re-read.
    struct EmailTakenRaceIdentities {
        inner: InMemoryIdentities,
        winner: ExternalIdentity,
    }

    #[async_trait]
    impl ExternalIdentityRepository for EmailTakenRaceIdentities {
        async fn find_by_issuer_subject(&self, issuer: &Issuer, subject: &str) -> Result<Option<ExternalIdentity>, RepositoryError> {
            self.inner.find_by_issuer_subject(issuer, subject).await
        }

        async fn provision(&self, _principal: &Principal, _user: &User, _identity: &ExternalIdentity) -> Result<(), RepositoryError> {
            let key = (self.winner.issuer.as_str().to_string(), self.winner.subject.clone());
            self.inner.0.identities.lock().unwrap().insert(key, self.winner.clone());
            Err(RepositoryError::Conflict(ConflictKind::EmailTaken))
        }
    }
```

Add at the end of `mod tests`:

```rust
    /// U6 (spec 4.6, D1): `Conflict(EmailTaken)` with the identity present at the re-read is a
    /// lost race, not a conflict. The loser resolves to the winner, with no line and no count.
    #[tokio::test]
    async fn jit_email_taken_with_the_identity_present_resolves_to_the_winner() {
        let (logs, _logs_guard) = capture_logs();
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _metrics_guard = metrics::set_default_local_recorder(&recorder);
        let store = AuthnStore::default();
        let issuer = Issuer::parse(ISSUER).unwrap();
        let winner_id = principal_id(43);
        seed_user(&store, 43, "racer.u6@example.com");
        let winner = ExternalIdentity {
            id: Uuid::from_u128(4343),
            principal_id: winner_id.clone(),
            issuer: issuer.clone(),
            subject: "sub-u6-race".into(),
            created_at: epoch(),
            updated_at: epoch(),
        };
        let uc = AuthenticateToken::new(
            FakeAuthenticator::ok(claims(ISSUER, "sub-u6-race", Some("racer.u6@example.com"), None)),
            EmailTakenRaceIdentities {
                inner: InMemoryIdentities(store.clone()),
                winner,
            },
            InMemoryPrincipals(store.clone()),
            InMemoryMemberships::default(),
            Arc::new(InMemoryRoleGrants::default()),
            SeqIds::default(),
            FixedClock::default(),
            JitPolicy::from_issuers(&[(issuer, true)]),
        );

        let resolved = uc.resolve("token", Provisioning::Enabled).await.unwrap();

        assert_eq!(resolved.principal_id, winner_id, "the loser resolves to the winner");
        let text = logs.text();
        assert!(jit_lines(&text).is_empty(), "a lost race writes no helper line:\n{text}");
        assert_eq!(jit_series(&snapshotter.snapshot().into_vec()), 0, "a lost race is not counted");
    }
```

`seed_user(&store, 43, …)` makes the principal `principal_id(43)`, the same id as `winner_id`, so `find_principal` finds the winner.

- [ ] **Step 2: Add the capture copy to the integration-test support**

Append to `rs/crates/services/paigasus-iam/tests/support/mod.rs`:

```rust

// --- SMA-698: log capture for the JIT provisioning-failure line ------------------------------
//
// The same helper as `src/log_capture.rs`. An integration test cannot import a `#[cfg(test)]`
// item of the library, so this is the one copy for the test binaries.

/// A `tracing` writer that keeps every formatted line in memory.
#[allow(dead_code)]
#[derive(Clone, Default)]
pub struct LogBuffer(Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for LogBuffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogBuffer {
    type Writer = LogBuffer;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

#[allow(dead_code)]
impl LogBuffer {
    /// Everything written so far, as text.
    pub fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

/// Installs a thread-local subscriber at TRACE. It sees every task of a current-thread
/// `#[tokio::test]`, spawned tasks included. Bind the guard to a named variable, never to `_`.
#[allow(dead_code)]
pub fn capture_logs() -> (LogBuffer, tracing::subscriber::DefaultGuard) {
    let buffer = LogBuffer::default();
    let subscriber = tracing_subscriber::fmt().with_writer(buffer.clone()).with_ansi(false).with_max_level(tracing::Level::TRACE).finish();
    (buffer, tracing::subscriber::set_default(subscriber))
}

/// The fixed prefix of both SMA-698 JIT failure messages. Count only lines that contain it:
/// `AppState::new` writes its own `warn` line when `accept_invalid_tls` is on, and `test_config`
/// turns it on.
#[allow(dead_code)]
pub const JIT_FAILURE_LINE: &str = "just-in-time provisioning failed";
```

(`Arc` is already imported at the top of `support/mod.rs`: `use std::sync::{Arc, RwLock};`.)

- [ ] **Step 3: Write T3 (the Postgres form, deterministic)**

In `rs/crates/services/paigasus-iam/tests/authn_identities.rs`, replace the imports:

```rust
use chrono::{SubsecRound, Utc};
use paigasus_iam::adapters::persistence::{PgExternalIdentityRepository, PgPrincipalRepository};
use paigasus_iam_core::{
    ConflictKind, Email, ExternalIdentity, ExternalIdentityRepository, Issuer, Principal, PrincipalId, PrincipalKind, PrincipalRepository, PrincipalStatus, RepositoryError, User,
};
use paigasus_kernel::{Prn, mint_uuid7};
use sea_orm::{ConnectionTrait, DbBackend, Statement};
```

with:

```rust
use chrono::{SubsecRound, Utc};
use paigasus_iam::adapters::http::AppState;
use paigasus_iam::adapters::persistence::entities::{external_identity, principal, user};
use paigasus_iam::adapters::persistence::{PgExternalIdentityRepository, PgPrincipalRepository};
use paigasus_iam::application::authenticate_token::Provisioning;
use paigasus_iam_core::{
    ConflictKind, Email, ExternalIdentity, ExternalIdentityRepository, Issuer, Principal, PrincipalId, PrincipalKind, PrincipalRepository, PrincipalStatus, RepositoryError, User,
};
use paigasus_kernel::{Prn, mint_uuid7};
use sea_orm::{ActiveModelTrait, ColumnTrait, ConnectionTrait, DbBackend, EntityTrait, PaginatorTrait, QueryFilter, Set, Statement, TransactionTrait};
```

Add at the end of the file:

```rust
/// SMA-698 T3 (spec 4.6, D1). In Postgres, `provision` inserts the `user` row before the
/// `external_identity` row, and `user.email` is unique. So when two first logins of ONE identity
/// race, the loser fails on `user_email_key` (`EmailTaken`) first. The loser must still resolve
/// to the winner's principal and write no JIT failure line.
///
/// Deterministic, as SMA-660 asks of every race test here: a `tokio::join!` of two `resolve`
/// calls also passes when one call commits before the other starts, with no race at all. The
/// winner is one open transaction that holds all three rows. The loser is the real
/// `AppState.authn.resolve`. The winner commits only when the loser is provably blocked inside its
/// `user` insert on the winner's uncommitted email.
#[tokio::test]
async fn lost_first_login_race_on_the_email_resolves_to_the_winner() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (logs, _logs_guard) = support::capture_logs();
    let idp = support::start_mock_idp().await;
    let state = AppState::new(db.clone(), &support::test_config(&idp)).await.expect("AppState::new");
    let email = "t3-racer@example.com";
    let subject = "t3-race-subject";
    let (winner_principal, winner_user, winner_identity) = build_triple(1_700_000_400_000, 0x40, email, &idp.issuer, subject);

    // The winner: all three rows in one open transaction, in `provision`'s order.
    let winner = db.begin().await.unwrap();
    principal::ActiveModel {
        id: Set(winner_principal.id.uuid()),
        prn: Set(winner_principal.id.canonical()),
        kind: Set(winner_principal.kind.as_str().to_string()),
        status: Set(winner_principal.status.as_str().to_string()),
        created_at: Set(winner_principal.created_at),
        updated_at: Set(winner_principal.updated_at),
    }
    .insert(&winner)
    .await
    .unwrap();
    user::ActiveModel {
        principal_id: Set(winner_user.principal_id.uuid()),
        email: Set(winner_user.email.as_str().to_string()),
        display_name: Set(winner_user.display_name.clone()),
        locale: Set(winner_user.locale.clone()),
        timezone: Set(winner_user.timezone.clone()),
        created_at: Set(winner_user.created_at),
        updated_at: Set(winner_user.updated_at),
    }
    .insert(&winner)
    .await
    .unwrap();
    external_identity::ActiveModel {
        id: Set(winner_identity.id),
        principal_id: Set(winner_identity.principal_id.uuid()),
        issuer: Set(winner_identity.issuer.as_str().to_string()),
        subject: Set(winner_identity.subject.clone()),
        created_at: Set(winner_identity.created_at),
        updated_at: Set(winner_identity.updated_at),
    }
    .insert(&winner)
    .await
    .unwrap();
    let winner_pid = support::race::backend_pid(&winner).await;

    // The loser: the real use case. Its lookup cannot see the uncommitted winner, so it provisions.
    let token = idp.bearer(subject, Some(email), "paigasus", 3600);
    let loser_state = state.clone();
    let mut loser = tokio::spawn(async move { loser_state.authn.resolve(&token, Provisioning::Enabled).await });

    support::race::expect_racer_blocked(&db, winner_pid, &mut loser, "insert into \"user\"%", |r| format!("{r:?}")).await;
    winner.commit().await.unwrap();

    let resolved = loser.await.unwrap().expect("the loser of a first-login race must resolve to the winner");
    assert_eq!(resolved.principal_id, winner_principal.id, "the loser resolves to the winner's principal");
    let users = user::Entity::find().filter(user::Column::Email.eq(email)).count(&db).await.unwrap();
    assert_eq!(users, 1, "exactly one user owns the email after the race");
    let text = logs.text();
    assert_eq!(
        text.lines().filter(|line| line.contains(support::JIT_FAILURE_LINE)).count(),
        0,
        "a lost race writes no JIT failure line:\n{text}"
    );
}
```

- [ ] **Step 4: Run U6 and T3 and see them fail (this MEASURES the defect)**

Run: `cd rs && cargo nextest run -p paigasus-iam --lib jit_email_taken_with_the_identity_present_resolves_to_the_winner`
Expected: FAIL with `called `Result::unwrap()` on an `Err` value: ProvisioningFailed(EmailConflict)`.

Run: `cd rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test authn_identities lost_first_login_race_on_the_email_resolves_to_the_winner`
Expected: FAIL with `the loser of a first-login race must resolve to the winner: ProvisioningFailed(EmailConflict)`. This is the spec 4.6 defect, measured on real Postgres. If it fails in another way (for example `expect_racer_blocked` reports that the loser finished, or the budget passed with a dump of the backends), stop and report the full message. Do not weaken the test.

- [ ] **Step 5: Write the re-read**

In `jit_provision`, replace:

```rust
            Err(RepositoryError::Conflict(ConflictKind::EmailTaken)) => Err(self.provisioning_failed(&claims.issuer, JitFailure::EmailConflict)),
```

with:

```rust
            // SMA-698 D1 (spec 4.6). In Postgres, `provision` inserts the `user` row before the
            // `external_identity` row, and `user.email` is unique. The loser of a race between two
            // first logins of ONE identity therefore fails on the email first. Its insert waits on
            // the unique index until the winner commits, so this re-read sees the winner's row.
            Err(RepositoryError::Conflict(ConflictKind::EmailTaken)) => match self.identities.find_by_issuer_subject(&claims.issuer, &claims.subject).await.map_err(backend)? {
                Some(winner) => Ok(winner.principal_id),
                None => Err(self.provisioning_failed(&claims.issuer, JitFailure::EmailConflict)),
            },
```

Update the `jit_provision` doc comment. Replace:

```rust
    /// a single transaction (D9). A lost race (`Conflict(ExternalIdentityExists)`) re-reads
    /// the winner's row and proceeds with it — no orphan principal/user, no auto-linking by
    /// email (D5): an email conflict fails provisioning instead.
```

with:

```rust
    /// a single transaction (D9). A lost race re-reads the winner's row and proceeds with it — no
    /// orphan principal/user. The race has two forms: `Conflict(ExternalIdentityExists)`, and
    /// `Conflict(EmailTaken)` with the identity present at the re-read (SMA-698 D1). No
    /// auto-linking by email (D5): an email conflict with the identity absent fails provisioning.
```

- [ ] **Step 6: Run U6 and T3 and see them pass; record T3**

Run: `cd rs && cargo fmt --all && cargo nextest run -p paigasus-iam --lib authenticate_token`
Expected: PASS, including `jit_email_taken_with_the_identity_present_resolves_to_the_winner` and `jit_email_conflict_writes_one_warn_line_without_the_email` (the re-read finds no identity there).

Run T3 five times: `cd rs && for i in 1 2 3 4 5; do PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test authn_identities lost_first_login_race_on_the_email_resolves_to_the_winner || break; done`. If the worktree sandbox refuses the loop, run the single command five times.
Expected: PASS on all five runs. Record the real result (five of five, or the first failure with its message) in the task report. If a run fails, report it; do not weaken T3. U6 stays the proof of the branch logic.

Run the whole suite file: `cd rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test authn_identities`
Expected: PASS (5 tests).

- [ ] **Step 7: Lint**

Run: `cd rs && cargo clippy --workspace --all-targets --locked -- -D warnings`
Expected: no warning.

- [ ] **Step 8: Commit**

```bash
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-698 add \
  rs/crates/services/paigasus-iam/src/application/authenticate_token.rs \
  rs/crates/services/paigasus-iam/tests/support/mod.rs \
  rs/crates/services/paigasus-iam/tests/authn_identities.rs
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-698 commit -F <scratchpad>/msg-task4.txt
```

Message file:

```text
fix(rs): treat a lost first-login race on the email as a success (SMA-698)

In Postgres the loser of two concurrent first logins by one identity fails
on user_email_key first and got 403 provisioning-failed. jit_provision now
re-reads the identity after EmailTaken and resolves to the winner. A new
Docker test holds the winner's rows in an open transaction, so the race
occurs on every run.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
```

---

## Task 5: Transport tests T1 (HTTP) and T2 (gRPC)

**Files:**
- Modify: `rs/crates/services/paigasus-iam/tests/http_authn.rs` (append)
- Modify: `rs/crates/services/paigasus-iam/tests/grpc_whoami.rs` (append)

**Interfaces:**
- Consumes: `support::{capture_logs, JIT_FAILURE_LINE}` (Task 4); `support::app(db) -> (Router, MockIdp)`; `support::send(&Router, method, uri, Option<Value>, Option<&str>) -> (StatusCode, Value)`; `MockIdp::bearer(&self, sub: &str, email: Option<&str>, aud: &str, exp_offset_secs: i64) -> String`; `MockIdp.issuer: String`; in `grpc_whoami.rs` the local `spawn_server`, `channel`, `authed`, `reason_of`. Fallback only: `paigasus_iam::adapters::grpc::authn::AuthLayer::new(AppState)`, `paigasus_iam::adapters::grpc::routes(AppState) -> tonic::service::Routes` (both `pub`; the production composition at `src/adapters/boot.rs:84`), `tonic::body::Body::empty()`, `tonic::Status::from_header_map(&HeaderMap) -> Option<Status>` (tonic 0.14.6).
- Produces: two Docker-backed tests.

The behavior exists since Task 3, so both tests pass on their first run. Task 7 proves that they can go red (mutations M1, M2, M4).

- [ ] **Step 1: Write T1**

Append to `rs/crates/services/paigasus-iam/tests/http_authn.rs`:

```rust

/// SMA-698 T1: a token with no `email` claim on a protected route is 403 `provisioning-failed`,
/// and IAM writes one `warn` line that names the defect and the issuer, and not the subject.
#[tokio::test]
async fn protected_route_with_a_token_without_email_logs_the_provisioning_failure() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    // Installed before `AppState::new`, which writes its own `accept_invalid_tls` warn line; the
    // filter on `JIT_FAILURE_LINE` below skips it.
    let (logs, _logs_guard) = support::capture_logs();
    let (app, idp) = support::app(db).await;
    let token = idp.bearer("t1-no-email-subject", None, "paigasus", 3600);

    let (status, body) = send(&app, "GET", "/v1/organizations", None, Some(&token)).await;

    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"]["code"], "provisioning-failed");
    let text = logs.text();
    let lines: Vec<&str> = text.lines().filter(|line| line.contains(support::JIT_FAILURE_LINE)).collect();
    assert_eq!(lines.len(), 1, "exactly one JIT failure line expected:\n{text}");
    let line = lines[0];
    assert!(line.contains("WARN"), "the line is at warn: {line}");
    assert!(line.contains("missing_email"), "the line names the defect: {line}");
    assert!(line.contains(&idp.issuer), "the line names the issuer: {line}");
    assert!(!text.contains("t1-no-email-subject"), "the log must not contain the subject:\n{text}");
}
```

- [ ] **Step 2: Run T1**

Run: `cd rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test http_authn protected_route_with_a_token_without_email_logs_the_provisioning_failure`
Expected: PASS. If the subject assertion fails, read which line holds the subject. Report it; do not remove the assertion.

- [ ] **Step 3: Write T2 in its primary form (spawned tonic server)**

Append to `rs/crates/services/paigasus-iam/tests/grpc_whoami.rs`:

```rust

/// SMA-698 T2: the same token on `WhoAmI` is `PermissionDenied` with reason `provisioning-failed`,
/// and IAM writes one `warn` line. The tests here run on the default current-thread runtime, so
/// the spawned tonic server runs on the test thread and the thread-local subscriber sees it.
#[tokio::test]
async fn who_am_i_with_a_token_without_email_logs_the_provisioning_failure() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (logs, _logs_guard) = support::capture_logs();
    let idp = support::start_mock_idp().await;
    let state = AppState::new(db, &support::test_config(&idp)).await.unwrap();
    let token = idp.bearer("t2-no-email-subject", None, "paigasus", 3600);
    let (addr, server) = spawn_server(state).await;
    let mut client = AuthnServiceClient::new(channel(addr).await);

    let status = client.who_am_i(authed(WhoAmIRequest {}, &token)).await.unwrap_err();

    assert_eq!(status.code(), Code::PermissionDenied, "{status:?}");
    assert_eq!(reason_of(&status), "provisioning-failed", "{status:?}");
    let text = logs.text();
    let lines: Vec<&str> = text.lines().filter(|line| line.contains(support::JIT_FAILURE_LINE)).collect();
    assert_eq!(lines.len(), 1, "exactly one JIT failure line expected:\n{text}");
    let line = lines[0];
    assert!(line.contains("WARN"), "the line is at warn: {line}");
    assert!(line.contains("missing_email"), "the line names the defect: {line}");
    assert!(line.contains(&idp.issuer), "the line names the issuer: {line}");
    assert!(!text.contains("t2-no-email-subject"), "the log must not contain the subject:\n{text}");

    server.abort();
}
```

- [ ] **Step 4: Measure T2 (spec 6.2)**

Run: `cd rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test grpc_whoami who_am_i_with_a_token_without_email_logs_the_provisioning_failure`

Read the result:
- PASS: the thread-local capture sees the spawned server. Record "T2 primary form measured: PASS". Go to Step 6.
- FAIL with `exactly one JIT failure line expected` while the status and reason asserts passed: the capture does not see the server task. Go to Step 5.
- Any other failure: stop and report it.

- [ ] **Step 5 (only if Step 4 showed no line): Replace T2 with the in-process fallback**

Replace the whole test from Step 3 with this version. It drives the production composition (`src/adapters/boot.rs:84`) in-process, so no task is spawned. Do not use `WithSubscriber` on the server future, and do not use `set_global_default`.

```rust

/// SMA-698 T2: the same token on `WhoAmI` is `PermissionDenied` with reason `provisioning-failed`,
/// and IAM writes one `warn` line. The measurement in the SMA-698 plan showed that the
/// thread-local subscriber does not see a spawned tonic server, so this drives the production
/// composition in-process with `oneshot`. `AuthEnforce` refuses the request before the inner
/// service runs, so an empty body is enough, and the trailers-only answer carries the status in
/// its headers.
#[tokio::test]
async fn who_am_i_with_a_token_without_email_logs_the_provisioning_failure() {
    use paigasus_iam::adapters::grpc::authn::AuthLayer;
    use tonic::codegen::http;
    use tower::{Layer, ServiceExt};

    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (logs, _logs_guard) = support::capture_logs();
    let idp = support::start_mock_idp().await;
    let state = AppState::new(db, &support::test_config(&idp)).await.unwrap();
    let token = idp.bearer("t2-no-email-subject", None, "paigasus", 3600);
    let service = AuthLayer::new(state.clone()).layer(grpc::routes(state.clone()).await);
    let request = http::Request::builder()
        .method("POST")
        .uri("/paigasus.iam.v1.AuthnService/WhoAmI")
        .header("content-type", "application/grpc")
        .header("authorization", format!("Bearer {token}"))
        .body(tonic::body::Body::empty())
        .unwrap();

    let response = service.oneshot(request).await.unwrap();
    let status = tonic::Status::from_header_map(response.headers()).expect("AuthEnforce answers trailers-only, so the status is in the headers");

    assert_eq!(status.code(), Code::PermissionDenied, "{status:?}");
    assert_eq!(reason_of(&status), "provisioning-failed", "{status:?}");
    let text = logs.text();
    let lines: Vec<&str> = text.lines().filter(|line| line.contains(support::JIT_FAILURE_LINE)).collect();
    assert_eq!(lines.len(), 1, "exactly one JIT failure line expected:\n{text}");
    let line = lines[0];
    assert!(line.contains("WARN"), "the line is at warn: {line}");
    assert!(line.contains("missing_email"), "the line names the defect: {line}");
    assert!(line.contains(&idp.issuer), "the line names the issuer: {line}");
    assert!(!text.contains("t2-no-email-subject"), "the log must not contain the subject:\n{text}");
}
```

Run the command from Step 4 again. Expected: PASS. Record "T2 primary form measured: FAIL (no line); fallback: PASS".

- [ ] **Step 6: Run both suite files, format, lint**

Run: `cd rs && cargo fmt --all && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test http_authn --test grpc_whoami && cargo clippy --workspace --all-targets --locked -- -D warnings`
Expected: PASS for every test in both files, and no clippy warning.

- [ ] **Step 7: Commit**

```bash
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-698 add \
  rs/crates/services/paigasus-iam/tests/http_authn.rs \
  rs/crates/services/paigasus-iam/tests/grpc_whoami.rs
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-698 commit -F <scratchpad>/msg-task5.txt
```

Message file (write the measured T2 form into the second paragraph):

```text
test(rs): cover the JIT provisioning-failure line on HTTP and gRPC

A token with no email claim gives 403 provisioning-failed on a protected HTTP
route and PermissionDenied provisioning-failed on WhoAmI. Each path writes
one warn line with missing_email and the issuer, and no subject (SMA-698).

T2 uses the <spawned server | in-process oneshot> form, as measured.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
```

---

## Task 6: Documentation

**Files:**
- Modify: `docs/ops/RUNBOOK-chart.md` (lines 121-122, 400-402)
- Modify: `docs/ops/RUNBOOK-observability.md` (§ 2.2 table after the `iam_bootstrap_admin_seed_failures_total` row; § 5 label list after the `reason` bullet)
- Modify: `rs/crates/services/paigasus-iam/CHANGELOG.md` (under `## [Unreleased]`)
- Modify: `docs/superpowers/specs/2026-09-26-sma-698-log-jit-provisioning-failure-design.md` (§ 10)

**Interfaces:**
- Consumes: the fixed message prefix, the field names (`defect`, `issuer`, `email_claim`, `suppressed`) and the counter name from Tasks 2-4.
- Produces: operator text only. No gate reads these files (checked: `git grep` finds no `ci/`, `charts/` or `rs/` test that reads the runbook text).

- [ ] **Step 1: RUNBOOK-chart § 6 item 2**

In `docs/ops/RUNBOOK-chart.md`, replace:

```markdown
2. **Email.** The access token must carry an `email` claim. IAM creates the principal on the first
   login from it.
```

with:

```markdown
2. **Email.** The access token must carry a valid `email` claim. IAM creates the principal on the
   first login from it. Without a valid claim, IAM answers `403 provisioning-failed`. IAM then
   writes a `warn` line that starts with `just-in-time provisioning failed`. The line has the
   fields `defect="missing_email"`, `issuer` and `email_claim`. `email_claim` is `absent` when the
   token has no `email` claim, and `invalid` when the value is not an email address. When another
   user already has the email, the line has `defect="email_conflict"`. IAM does not link
   identities by email. The line does not show the email, the subject or the token. IAM writes
   at most one line for each issuer and defect in 10 seconds. The field `suppressed` gives the
   number of failures since the last line. The counter `iam_jit_provisioning_failures_total`
   counts each refused request.
```

- [ ] **Step 2: RUNBOOK-chart § 9 bootstrap bullet**

Replace:

```markdown
- **The user must provision first.** IAM grants the role only after JIT provisioning succeeds.
  Provisioning needs an `email` claim in the access token. Without it, IAM answers
  `403 provisioning-failed` and grants nothing.
```

with:

```markdown
- **The user must provision first.** IAM grants the role only after JIT provisioning succeeds.
  Provisioning needs a valid `email` claim in the access token. Without it, IAM answers
  `403 provisioning-failed` and grants nothing. IAM also writes the `warn` line that § 6 item 2
  describes. Look for `just-in-time provisioning failed` in the IAM log.
```

- [ ] **Step 3: RUNBOOK-observability § 2.2 row**

In `docs/ops/RUNBOOK-observability.md`, add this row directly after the row that starts with `| `iam_bootstrap_admin_seed_failures_total` |`:

```markdown
| `iam_jit_provisioning_failures_total` | counter | `defect` | Just-in-time provisioning attempts that IAM refused (SMA-698). `defect` ∈ `missing_email` (the access token has no valid `email` claim) / `email_conflict` (another user already has the email, and IAM does not link identities by email). It counts refused **requests**, not identities: one user with a bad token makes several refused requests, because a console session makes several IAM calls. The log rate limit (one `warn` line per issuer and defect in 10 s) does not apply to this counter. A lost race between two first logins of one identity is a success and is not counted. Both series are **primed at zero** when metrics are on, so a flat zero is the healthy state and `increase()` sees the first failure. Each refusal also writes a `warn` line that starts with `just-in-time provisioning failed` and names the issuer — read the issuer there, because the counter has no `issuer` label. No alert ships for it yet (SMA-706). |
```

- [ ] **Step 4: RUNBOOK-observability § 5 label key**

In § 5, add after the bullet that starts with `- `reason` — how a rewind presented (SMA-474)` (it ends with `Two literals chosen at the emit site.`):

```markdown
- `defect` — why just-in-time provisioning failed (SMA-698): `missing_email`/`email_conflict`.
  One function with an exhaustive `match` on `ProvisioningDefect` makes the value. It never
  comes from a token claim.
```

- [ ] **Step 5: CHANGELOG**

In `rs/crates/services/paigasus-iam/CHANGELOG.md`, replace:

```markdown
## [Unreleased]

### Security
```

with:

```markdown
## [Unreleased]

### Added

- IAM logs a refused just-in-time provisioning at `warn`. The line starts with
  `just-in-time provisioning failed` and names the defect (`missing_email` or `email_conflict`)
  and the issuer. For `missing_email`, the field `email_claim` tells if the claim is absent or
  invalid. The line does not show the email, the subject, the name or the token. IAM writes at
  most one line for each issuer and defect in 10 seconds. Before, IAM answered
  `403 provisioning-failed` and logged nothing (SMA-698).
- The counter `iam_jit_provisioning_failures_total`, with the label `defect`. It counts each
  refused request, and the log rate limit does not apply to it. Both series start at zero when
  metrics are on (SMA-698).

### Fixed

- Two first logins of the same identity at the same time both succeed. Before, in Postgres the
  second login failed on the email and got `403 provisioning-failed` (SMA-698).

### Security
```

- [ ] **Step 6: Spec § 10**

In `docs/superpowers/specs/2026-09-26-sma-698-log-jit-provisioning-failure-design.md`, replace:

```markdown
Open these in Linear after the spec is approved:

- An alert rule on `increase(iam_jit_provisioning_failures_total[..]) > 0`, with a
  promtool test, and a dashboard panel.
- A log line for a silent `403 identity-not-provisioned` on the `Enabled` path, when the
  issuer has JIT disabled (`authenticate_token.rs:117-119`).
```

with:

```markdown
These issues are open in Linear:

- SMA-706: an alert rule on `increase(iam_jit_provisioning_failures_total[..]) > 0`, with a
  promtool test, and a dashboard panel.
- SMA-707: a log line for a silent `403 identity-not-provisioned` on the `Enabled` path, when
  the issuer has JIT disabled (`authenticate_token.rs:117-119`).
```

- [ ] **Step 7: Check the Markdown table**

Run: `grep -n "iam_jit_provisioning_failures_total" docs/ops/RUNBOOK-observability.md docs/ops/RUNBOOK-chart.md rs/crates/services/paigasus-iam/CHANGELOG.md`
Expected: one hit in the § 2.2 row, one in RUNBOOK-chart § 6 item 2, one in the CHANGELOG. The § 2.2 row has exactly four `|`-separated cells (count: 5 `|` characters outside backticks).

- [ ] **Step 8: Commit**

```bash
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-698 add \
  docs/ops/RUNBOOK-chart.md docs/ops/RUNBOOK-observability.md \
  rs/crates/services/paigasus-iam/CHANGELOG.md \
  docs/superpowers/specs/2026-09-26-sma-698-log-jit-provisioning-failure-design.md
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-698 commit -F <scratchpad>/msg-task6.txt
```

Message file:

```text
docs(rs): document the JIT provisioning-failure line and counter

The chart runbook names the warn line at the missing-email notes. The
observability runbook gets a catalog row and the defect label key. The
CHANGELOG records the line, the counter and the lost-race fix. The spec
names the follow-up issues SMA-706 and SMA-707.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
```

---

## Task 7: Prove that the tests bite (spec 6.3)

No commit. Every mutation must compile: the crate denies warnings, so a mutation that fails at `rustc` proves nothing. Apply one mutation at a time with the Edit tool, run the named tests with `--no-fail-fast`, record red or green, and restore by removing the inserted change with the Edit tool. Never use `git checkout --`. Before the first mutation, `git status --short` must be empty (Tasks 1-6 are committed). After each restore, `git status --short` must be empty again. That is the proof of the restore.

**Files:**
- Temporarily modify: `rs/crates/services/paigasus-iam/src/application/authenticate_token.rs` only.

**Interfaces:**
- Consumes: every test from Tasks 2-5.
- Produces: a mutation table in the task report.

Command templates (from `rs/`):
- Unit: `cargo nextest run -p paigasus-iam --lib --no-fail-fast <name filters>`
- Docker: `PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --no-fail-fast --test http_authn --test grpc_whoami --test authn_identities <name filters>`

- [ ] **Step 1: Confirm the clean start**

Run: `git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-698 status --short`
Expected: no output.

- [ ] **Step 2: Apply, run and restore each mutation**

| M | Mutation (exact edit in `provisioning_failed` or `jit_provision`) | Run these filters | Must go red |
|---|---|---|---|
| M1 | In both events, `tracing::warn!(` becomes `tracing::debug!(` | unit: `jit_missing_email jit_invalid_email jit_email_conflict`; Docker: `logs_the_provisioning_failure` | U1, U2, U3 (`the helper logs at warn`), T1, T2 (`the line is at warn`) |
| M2 | Replace the whole `match failure { … }` inside the `if let` with `let _ = (suppressed, if let JitFailure::MissingEmail(claim) = failure { claim.label() } else { "" });` | unit: `jit_ a_cloned the_next_admitted`; Docker: `logs_the_provisioning_failure` | U1, U2, U3, U9, U11, U12, U13, U14, U15, T1, T2 |
| M3 | Delete the line `counter!(names::IAM_JIT_PROVISIONING_FAILURES_TOTAL, "defect" => label).increment(1);` | unit: `jit_missing_email jit_invalid_email jit_email_conflict jit_repeated` | U1, U2, U3, U9 |
| M4 | Insert as the first line of `jit_provision`: `tracing::debug!(email = ?claims.email, subject = %claims.subject, "jit provisioning input");` | unit: `jit_missing_email jit_invalid_email jit_email_conflict`; Docker: `logs_the_provisioning_failure` | U1, U2, U3 (`the log must not contain`), T1, T2 |
| M5 | In the `ExternalIdentityExists` arm, wrap the body: `Err(RepositoryError::Conflict(ConflictKind::ExternalIdentityExists)) => { let _ = self.provisioning_failed(&claims.issuer, JitFailure::EmailConflict); self.identities…(unchanged chain) }` | unit: `provision_race_loser_reuses_winner_row` | U5 |
| M6 | Replace the whole `EmailTaken` arm with `Err(RepositoryError::Conflict(ConflictKind::EmailTaken)) => Err(self.provisioning_failed(&claims.issuer, JitFailure::EmailConflict)),` | unit: `jit_email_taken`; Docker: `lost_first_login_race` | U6, T3 |
| M7 | Append `.or(Some(0))` to `self.provisioning_log.admit_at(issuer.as_str(), label, Instant::now())` | unit: `jit_repeated a_cloned` | U9, U13 |
| M8 | In `prime_jit_provisioning_failures`, replace the `counter!(…).increment(0);` line with `let _ = provisioning_defect_label(defect);` | unit: `prime_registers_both_defect_series_at_zero` | U10 |
| M9 | In both events, replace the field `suppressed,` with `suppressed = suppressed.min(0),` | unit: `the_next_admitted` | U15 |
| M10 | Replace `admit_at(issuer.as_str(), label, Instant::now())` with `admit_at("", label, Instant::now())` | unit: `jit_failures_from_two_issuers` | U11 |
| M11 | Replace `admit_at(issuer.as_str(), label, Instant::now())` with `admit_at(issuer.as_str(), "", Instant::now())` | unit: `jit_two_defects` | U12 |
| M12 | In `jit_provision`, change `Some(Err(_)) => … EmailClaim::Invalid …` to `EmailClaim::Absent` | unit: `jit_invalid_email jit_empty_email` | U2, U14 |

For each row:
1. Apply the edit with the Edit tool.
2. Run `cd rs && cargo build -p paigasus-iam --tests` first. If it does not compile, the mutation is invalid: fix the mutation text so it compiles (keep its intent), and note the change in the report.
3. Run the named filters with `--no-fail-fast`. Record each named test as red or green, with its failure message.
4. Restore with the Edit tool (the reverse edit). Run `git status --short`; expected: no output.

A row passes when every test in "Must go red" is red. Report a row where a listed test stays green; do not change the test in this task.

- [ ] **Step 3: Full verification after the last restore**

Run: `cd rs && cargo fmt --all -- --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo nextest run -p paigasus-iam`
Expected: PASS, with Docker available. `cargo fmt --check` prints nothing. `git status --short` prints nothing.

---

## Task 8: Final verification with the full gate graph

**Files:** none changed.

**Interfaces:** consumes the committed branch.

- [ ] **Step 1: Prepare**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-698
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
git fetch origin
mkdir -p <scratchpad>/bash32 <scratchpad>/bash5
ln -sf /bin/bash <scratchpad>/bash32/bash
ln -sf /opt/homebrew/bin/bash <scratchpad>/bash5/bash
```

If `py/.venv` is missing, run `uv sync` in `py/` first (a fresh worktree has none).

- [ ] **Step 2: Run the full graph with bash 3.2**

`repo:affected-smoke` needs bash 3.2. Moon resolves `bash` through `PATH`, so put the bash-only shim first (never prepend `/bin`: it downgrades `python3`).

```bash
PATH="<scratchpad>/bash32:$HOME/.proto/shims:$HOME/.proto/bin:$PATH" moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site \
  :http-extractor-envelope :input-liveness :promtool :observability-drift \
  :nats-permissions :release-parity :release-parity-py :release-parity-ts \
  :publish-metadata :version-lockstep :workflow-credentials :pyo3-stub-drift :ruff-ci \
  :next-public-free :helm-render :test-e2e \
  --base origin/main --include-relations
```

Expected: every selected task passes, except gates that need bash 4+ and were selected: `repo:ruff-ci`, `repo:next-public-free`, `repo:publish-metadata`, `repo:version-lockstep`, `repo:nats-permissions`. Under bash 3.2 these fail with `mapfile: command not found`, `declare: -A: invalid option`, `unbound variable`, or a bash-version refusal. That is a bash-version artifact, not a finding.

If a task fails, copy `.moon/cache/ciReport.json` and `.moon/cache/states/<project>/<task>/` to the scratchpad BEFORE any re-run, then follow the root `CLAUDE.md` "Diagnosing an unattributed `moon ci` failure" steps.

<!-- moon-diagnosis:ok -->

- [ ] **Step 3: Re-run the bash-4+ gates under Homebrew bash**

For each bash-4+ gate that Step 2 selected, run it directly with the bash-5 shim first in `PATH`, and read this verdict instead of the Step 2 verdict:

```bash
PATH="<scratchpad>/bash5:$HOME/.proto/shims:$HOME/.proto/bin:$PATH" ./ci/ruff/run.sh
PATH="<scratchpad>/bash5:$HOME/.proto/shims:$HOME/.proto/bin:$PATH" ./ci/next-public/run.sh
PATH="<scratchpad>/bash5:$HOME/.proto/shims:$HOME/.proto/bin:$PATH" ./ci/publish-metadata/run.sh
PATH="<scratchpad>/bash5:$HOME/.proto/shims:$HOME/.proto/bin:$PATH" ./ci/version-lockstep/run.sh
PATH="<scratchpad>/bash5:$HOME/.proto/shims:$HOME/.proto/bin:$PATH" ./ops/nats/check-subjects.sh
( cd rs && cargo nextest run --locked --no-tests=pass -p paigasus-iam --test nats_permissions --test docker_preflight --profile iam-nats )
```

Expected: each exits 0.

- [ ] **Step 4: Read the actionlint verdict correctly**

`repo:actionlint` gives a local verdict only when its pipe preflight passes. Read its output for `pipe capacity … bytes (floor 8192)`. If the host pipe holds 512 bytes, the gate exits rc 2 with a `small` message. That is a host condition, not a finding, and CI is then its only verdict. Report which case occurred.

- [ ] **Step 5: Report**

Report: the Step 2 result per failed task, the Step 3 verdicts, the Step 4 case, the T2 form that Task 5 measured, the T3 five-run result from Task 4, and the Task 7 mutation table. Do not push. Do not open a PR.
