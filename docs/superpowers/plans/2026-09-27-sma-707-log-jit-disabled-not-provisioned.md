# SMA-707 JIT-disabled not-provisioned log line Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** When a protected IAM request (HTTP or gRPC) comes from an unknown identity of an issuer with `jit_provisioning = false`, IAM writes one rate-limited `info` line that names the issuer. `Introspect` writes no line. The HTTP status, the gRPC status and the error body do not change.

**Architecture:** One private helper, `AuthenticateToken::jit_disabled`, in the application layer. The `!self.jit.allows(..)` branch of `resolve(.., Provisioning::Enabled)` calls it. Both transports reach that branch, and `Introspect` (`Provisioning::Disabled`) cannot reach it. A new field, `not_provisioned_log: Arc<LogRateLimiter<()>>`, gives one line per issuer in 10 s. There is no counter.

**Tech Stack:** Rust 2024 (rust-version 1.95), `tracing` 0.1, `tracing-subscriber` 0.3 (tests), `metrics-util` `DebuggingRecorder` (tests), tonic 0.14, axum, SeaORM 2 on Postgres 16 (testcontainers), cargo-nextest.

**Spec:** `docs/superpowers/specs/2026-09-27-sma-707-log-jit-disabled-not-provisioned-design.md`

## Deviations from the spec

Each deviation keeps the intent of the spec. The code facts are checked against the tree at `d600b894`.

1. **The stale doc reference in `grpc_whoami.rs` names the branch by symbol, not by line.** Spec 6.2 T2 says to replace `authenticate_token.rs:107-109` with `:189-191`. Task 1 adds lines above the branch (two doc sentences and a new field), so `:189-191` is stale again after Task 1. The new doc comment names "the `!self.jit.allows` branch of `AuthenticateToken::resolve`, which calls `jit_disabled`". A symbol does not go stale when lines move.
2. **T1 and T2 get distinctive fixture values.** The existing tests use the subject `no-jit-user` and `grpc-whoami-no-jit`. Spec 6 asks for distinctive values, so a substring check cannot pass by accident. T1 and T2 change their subject and email. The status and the error-code assertions stay.
3. **T1, T3 and T4 match the issuer as the exact field `issuer="<issuer>"`.** Two mock IdPs have issuers such as `https://127.0.0.1:4000` and `https://127.0.0.1:40001`. A plain `contains(&idp.issuer)` can match the wrong one.
4. **Extra tests and one extra mutation.** The Review Focus below adds U8 (a new test, not the old U8), T4 and mutation M9.
5. **Exact mutation forms.** Spec 6.3 gives M3 and M4 in words. Task 4 gives the exact edits. M3 changes the helper signature to take `&ValidatedClaims`. M4 replaces the limiter result with `Some(0_u64)`. M6 keeps the `suppressed` binding with `let _ = suppressed;`, because the crate denies the `unused_variables` warning.
6. **T3 and T4 also assert the level and the issuer field on the control line.** So M7 and M8 also red them. The spec lists the minimum set of red tests. Task 4 records every red test.

## Global Constraints

- Worktree root: `/Users/smaschek/dev/paigasus/paigasus-core-sma-707`, branch `feature/sma-707-log-jit-disabled-not-provisioned`. Run `git -C /Users/smaschek/dev/paigasus/paigasus-core-sma-707 branch --show-current` before the first edit. Never touch `/Users/smaschek/dev/paigasus/paigasus-core` (the main checkout).
- Run every cargo command from `/Users/smaschek/dev/paigasus/paigasus-core-sma-707/rs`, after `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.
- Keep every SPDX header unchanged. This plan creates no new source file.
- Rust edition 2024, rust-version 1.95.
- The workspace sets `[workspace.lints.rust] warnings = "deny"` (`rs/Cargo.toml:274-275`). An unused item or an unused binding is a compile error. Each committed task must pass `cargo clippy --workspace --all-targets --locked -- -D warnings`.
- Run `cargo fmt --all` from `rs/` after each code change.
- A Docker-backed test run must set `PAIGASUS_REQUIRE_DOCKER=1`. Without it, `start_or_skip` skips the test and it passes under every mutation (`tests/support/docker.rs:257-279`).
- Level: `info`. Fields: `issuer` (`issuer.as_str()`), `reason` (the fixed string `"jit_disabled"`), `suppressed` (the limiter count). No other field.
- The message is fixed. Tests match on it. Do not change a character: `request refused: the identity is not provisioned, and just-in-time provisioning is disabled for this issuer; IAM creates an identity only by just-in-time provisioning, so set jit_provisioning = true for the issuer to let new users of this issuer sign in`
- The selector prefix is `request refused: the identity is not provisioned`. No other log line in the crate contains it (checked with `git grep`). The message must not contain `just-in-time provisioning failed`, the SMA-698 selector.
- The rate limit is `LOG_RATE_LIMIT_INTERVAL` (10 s), keyed by `(issuer, ())`. The field is `not_provisioned_log: Arc<LogRateLimiter<()>>`.
- No counter. No change to `paigasus-iam-core`, to the status codes, to the error codes or to the error bodies.
- The helper receives only `&Issuer`. The line never holds the subject, the email, the name, `locale`, `zoneinfo`, another claim value or the token.
- Conventional commits: `feat(rs): … (SMA-707)` for code, `test(rs): … (SMA-707)` for tests only, `docs(rs): … (SMA-707)` for docs only. The body ends with one blank line and then the line `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Write each commit message to a file in your scratchpad directory with the Write tool. Then run `git -C /Users/smaschek/dev/paigasus/paigasus-core-sma-707 commit -F <that file>`. Keep each body line at 100 characters or less. Do not put a `#NNN` line or a `token: value` line in the body (commitlint `footer-leading-blank`).
- Never `git commit --amend`. Never `git reset`. Never `--no-verify`. Never `git stash`. Never `git checkout --` to restore a file: it also discards uncommitted work. Restore a mutation with the Edit tool, by removing the inserted change.
- Commits are SSH-signed through 1Password. If a commit fails with "failed to fill whole buffer", 1Password is locked. Stop and report. Do not bypass or change the signing.
- Do not install host software (no `brew install`). Run every command in the foreground.
- Documentation text is in ASD-STE100 Simplified Technical English: short sentences, active voice, one idea in one sentence.

## Review Focus

These input classes can break the feature, and no test in spec section 6 exercises them. Each one gets a test in the task that owns it.

| # | Input class or failure mode | Why it can break | Test |
|---|---|---|---|
| R1 | An unknown identity of a JIT-ENABLED issuer (a success, and a JIT failure) | A misplaced call (for example at the top of the `Enabled` arm) writes the line for every first login. The spec tests use only JIT-disabled issuers, so they cannot see this. | U8 `jit_disabled_line_is_absent_for_a_jit_enabled_issuer` (Task 1), red under M9 |
| R2 | One issuer, two DIFFERENT subjects in one window | A limiter keyed by the subject would write one line per user, and the rate limit would not bound the lines | U5a uses two different subjects (Task 1) |
| R3 | One refusal on HTTP and one on gRPC, on the same `AppState` | Production serves both transports from one `AppState`. If a transport built its own use case, each transport would write its own line in the window | T4 `jit_disabled_refusals_on_http_and_grpc_share_one_line` (Task 2), red under M1 and M4 |
| R4 | The control half of U2, U3 and T3 | A "no line" assertion passes when the capture sees nothing at all. Each test first asserts no line, then does one protected refusal and asserts one line | U2, U3, T3 (spec), red under M1 |

## File Structure

| File | Task | Change |
|---|---|---|
| `rs/crates/services/paigasus-iam/src/application/authenticate_token.rs` | 1 | `Provisioning` doc (lines 21-24), `not_provisioned_log` field (after line 149), its init in `new` (after line 172), `resolve` doc and branch (lines 176-191), helper `jit_disabled` (after line 346), unit tests: U8 at lines 1603-1632 removed, U1-U8 added |
| `rs/crates/services/paigasus-iam/src/application/log_rate_limit.rs` | 1 | Doc comment only (lines 13-16) |
| `rs/crates/services/paigasus-iam/tests/support/mod.rs` | 2 | The constant `JIT_DISABLED_LINE` (after line 1004) |
| `rs/crates/services/paigasus-iam/tests/http_authn.rs` | 2 | T1 (replaces lines 328-346), T3 (appended) |
| `rs/crates/services/paigasus-iam/tests/grpc_whoami.rs` | 2 | T2 (replaces lines 138-160, with the doc fix), T4 (appended) |
| `rs/crates/services/paigasus-iam/iam.toml.example` | 3 | Text after line 51 (`jit_provisioning`) |
| `rs/crates/services/paigasus-iam/CHANGELOG.md` | 3 | One bullet under `## [Unreleased]` / `### Added` (after line 22) |

`RUNBOOK-chart.md`, `RUNBOOK-observability.md`, `paigasus-observability/src/names.rs`, `src/main.rs`, the chart and the Prometheus rules do not change (spec section 7).

---

## Task 1: The helper, the limiter field, the branch and the unit tests

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/application/authenticate_token.rs` (lines 21-24, 146-150, 163-173, 176-191, 320-347, 1603-1632, end of `mod tests` at line 1815)
- Modify: `rs/crates/services/paigasus-iam/src/application/log_rate_limit.rs` (lines 13-16)

**Interfaces:**
- Consumes:
  - `crate::application::log_rate_limit::{LOG_RATE_LIMIT_INTERVAL, LogRateLimiter}` with `impl<K: Copy + Eq + Hash> LogRateLimiter<K> { pub(crate) fn new(interval: Duration) -> Self; pub(crate) fn admit_at(&self, issuer: &str, kind: K, now: Instant) -> Option<u64> }` (already imported in `authenticate_token.rs:19`).
  - `std::time::Instant` (already imported, line 14).
  - Test helpers already in `mod tests`: `capture_logs() -> (LogBuffer, DefaultGuard)`, `claims(issuer, subject, email, name) -> ValidatedClaims`, `claims_with_profile(issuer, subject, email, name, locale, zoneinfo) -> ValidatedClaims`, `FakeAuthenticator::ok`, `QueueAuthenticator::new(Vec<ValidatedClaims>)`, `AuthnStore`, `InMemoryIdentities`, `InMemoryPrincipals`, `InMemoryMemberships`, `seeded_principal(&AuthnStore, &Issuer, &str) -> PrincipalId`, `jit_use_case(A, &AuthnStore)` (JIT on for `ISSUER`), `jit_lines(&str) -> Vec<&str>`, `has_field(&str, &str, &str) -> bool`, `assert_no_secrets(&str, &[&str])`, `jit_series(&MetricsSnapshot) -> usize`, `KernelIdGenerator`, consts `ISSUER`, `OTHER_ISSUER`, `JIT_LINE`.
- Produces (all private to the module):
  - field `not_provisioned_log: Arc<LogRateLimiter<()>>` on `AuthenticateToken`
  - `fn jit_disabled(&self, issuer: &Issuer) -> AuthnError`
  - `AuthenticateToken::new` keeps its signature.
  - Test items: `const JIT_DISABLED_LINE: &str`, `const JIT_DISABLED_TEXT: &str`, `fn jit_disabled_lines(text: &str) -> Vec<&str>`, `fn jit_disabled_use_case<A: Authenticator>(authenticator: A, store: &AuthnStore) -> AuthenticateToken<A, InMemoryIdentities, InMemoryPrincipals, InMemoryMemberships, SeqIds, FixedClock>`.

Every new unit test name starts with `jit_disabled_`, so the filter `jit_disabled_` selects all of them. It also selects the existing test `jit_disabled_issuer_returns_identity_not_provisioned`, which stays green.

- [ ] **Step 1: Remove the old U8 and write the failing tests**

In `authenticate_token.rs`, delete the test `identity_not_provisioned_writes_no_line_and_no_series` completely: its doc comment (`/// U8: a JIT-disabled issuer, and `introspect` for an unknown identity. Both return` and the next line), `#[tokio::test]`, and the function body to its closing `}` (lines 1603-1632). Its assertions move into the new U1 (no SMA-698 line, no JIT failure series) and U2 (introspect).

Add this block at the end of `mod tests`, directly before the closing `}` of the module (line 1815):

```rust

    // ---- SMA-707: the JIT-disabled refusal line ----------------------------------------------

    /// The fixed prefix of the SMA-707 line. Tests select the line with it.
    const JIT_DISABLED_LINE: &str = "request refused: the identity is not provisioned";
    /// The whole fixed SMA-707 message (spec 4.3).
    const JIT_DISABLED_TEXT: &str = "request refused: the identity is not provisioned, and just-in-time provisioning is disabled for this issuer; IAM creates an identity only by just-in-time provisioning, so set jit_provisioning = true for the issuer to let new users of this issuer sign in";

    /// The SMA-707 lines only.
    fn jit_disabled_lines(text: &str) -> Vec<&str> {
        text.lines().filter(|line| line.contains(JIT_DISABLED_LINE)).collect()
    }

    /// A use case over `store`, with JIT OFF for `ISSUER`.
    fn jit_disabled_use_case<A: Authenticator>(authenticator: A, store: &AuthnStore) -> AuthenticateToken<A, InMemoryIdentities, InMemoryPrincipals, InMemoryMemberships, SeqIds, FixedClock> {
        AuthenticateToken::new(
            authenticator,
            InMemoryIdentities(store.clone()),
            InMemoryPrincipals(store.clone()),
            InMemoryMemberships::default(),
            Arc::new(InMemoryRoleGrants::default()),
            SeqIds::default(),
            FixedClock::default(),
            JitPolicy::from_issuers(&[(Issuer::parse(ISSUER).unwrap(), false)]),
        )
    }

    /// SMA-707 U1: `resolve(.., Enabled)`, a JIT-disabled issuer and an unknown identity. One
    /// `info` line with the issuer, `reason` and `suppressed = 0`. The whole capture holds no
    /// claim value and no token. No line matches the SMA-698 selector, and there is no JIT
    /// failure series (the checks of the old U8).
    #[tokio::test]
    async fn jit_disabled_unknown_identity_writes_one_info_line_without_claims() {
        let (logs, _logs_guard) = capture_logs();
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _metrics_guard = metrics::set_default_local_recorder(&recorder);
        let store = AuthnStore::default();
        let uc = jit_disabled_use_case(
            FakeAuthenticator::ok(claims_with_profile(
                ISSUER,
                "sub-jd-u1-distinct",
                Some("vera.jd-distinct@example.com"),
                Some("Vera Jdname"),
                Some("jd-XQ-locale"),
                Some("Jd/Distinct_Zone"),
            )),
            &store,
        );

        let err = uc.resolve("bearer-jd-u1-secret", Provisioning::Enabled).await.unwrap_err();

        assert!(matches!(err, AuthnError::IdentityNotProvisioned), "got {err:?}");
        let text = logs.text();
        let lines = jit_disabled_lines(&text);
        assert_eq!(lines.len(), 1, "exactly one JIT-disabled line expected:\n{text}");
        let line = lines[0];
        assert!(line.contains("INFO"), "the line is at info: {line}");
        assert!(line.contains(JIT_DISABLED_TEXT), "the fixed message: {line}");
        assert!(has_field(line, "issuer", ISSUER), "names the issuer: {line}");
        assert!(has_field(line, "reason", "jit_disabled"), "names the reason: {line}");
        assert!(has_field(line, "suppressed", "0"), "the first line suppressed nothing: {line}");
        assert_no_secrets(
            &text,
            &["sub-jd-u1-distinct", "vera.jd-distinct@example.com", "vera.jd-distinct", "Vera Jdname", "jd-XQ-locale", "Jd/Distinct_Zone", "bearer-jd-u1-secret"],
        );
        assert!(jit_lines(&text).is_empty(), "the line must not match the SMA-698 selector:\n{text}");
        assert!(store.identities.lock().unwrap().is_empty(), "JIT off never provisions");
        assert_eq!(jit_series(&snapshotter.snapshot().into_vec()), 0, "no JIT failure series");
    }

    /// SMA-707 U2: `introspect` for an unknown identity of the SAME JIT-disabled issuer writes no
    /// line. With a JIT-enabled issuer, a misplaced line would not appear either, so the issuer
    /// must be JIT-disabled. Control: the same token on `resolve(.., Enabled)` then writes one
    /// line, so the capture can see the line on every run.
    #[tokio::test]
    async fn jit_disabled_introspect_writes_no_line() {
        let (logs, _logs_guard) = capture_logs();
        let store = AuthnStore::default();
        let uc = jit_disabled_use_case(QueueAuthenticator::new(vec![claims(ISSUER, "sub-jd-u2", None, None), claims(ISSUER, "sub-jd-u2", None, None)]), &store);

        let err = uc.introspect("bearer-jd-u2").await.unwrap_err();

        assert!(matches!(err, AuthnError::IdentityNotProvisioned), "got {err:?}");
        let text = logs.text();
        assert!(jit_disabled_lines(&text).is_empty(), "introspect must not write the line:\n{text}");

        let err = uc.resolve("bearer-jd-u2", Provisioning::Enabled).await.unwrap_err();

        assert!(matches!(err, AuthnError::IdentityNotProvisioned), "got {err:?}");
        let text = logs.text();
        assert_eq!(jit_disabled_lines(&text).len(), 1, "control: the protected path writes one line:\n{text}");
    }

    /// SMA-707 U3: `resolve(.., Disabled)` with a JIT-disabled issuer writes no line. The same
    /// guard as U2, one level down, with the same control.
    #[tokio::test]
    async fn jit_disabled_resolve_disabled_writes_no_line() {
        let (logs, _logs_guard) = capture_logs();
        let store = AuthnStore::default();
        let uc = jit_disabled_use_case(QueueAuthenticator::new(vec![claims(ISSUER, "sub-jd-u3", None, None), claims(ISSUER, "sub-jd-u3", None, None)]), &store);

        let err = uc.resolve("bearer-jd-u3", Provisioning::Disabled).await.unwrap_err();

        assert!(matches!(err, AuthnError::IdentityNotProvisioned), "got {err:?}");
        let text = logs.text();
        assert!(jit_disabled_lines(&text).is_empty(), "resolve(.., Disabled) must not write the line:\n{text}");

        let err = uc.resolve("bearer-jd-u3", Provisioning::Enabled).await.unwrap_err();

        assert!(matches!(err, AuthnError::IdentityNotProvisioned), "got {err:?}");
        let text = logs.text();
        assert_eq!(jit_disabled_lines(&text).len(), 1, "control: the protected path writes one line:\n{text}");
    }

    /// SMA-707 U4: a KNOWN identity of a JIT-disabled issuer resolves and writes no line.
    #[tokio::test]
    async fn jit_disabled_known_identity_resolves_and_writes_no_line() {
        let (logs, _logs_guard) = capture_logs();
        let store = AuthnStore::default();
        let issuer = Issuer::parse(ISSUER).unwrap();
        let pid = seeded_principal(&store, &issuer, "sub-jd-u4");
        let uc = jit_disabled_use_case(FakeAuthenticator::ok(claims(ISSUER, "sub-jd-u4", None, None)), &store);

        let resolved = uc.resolve("token", Provisioning::Enabled).await.unwrap();

        assert_eq!(resolved.principal_id, pid);
        let text = logs.text();
        assert!(jit_disabled_lines(&text).is_empty(), "a known identity writes no line:\n{text}");
    }

    /// SMA-707 U5a (and Review Focus R2): two refusals for one issuer in one window write one line.
    /// The two subjects differ, so a limiter keyed by the subject would write two lines.
    #[tokio::test]
    async fn jit_disabled_refusals_for_one_issuer_log_once() {
        let (logs, _logs_guard) = capture_logs();
        let store = AuthnStore::default();
        let uc = jit_disabled_use_case(
            QueueAuthenticator::new(vec![claims(ISSUER, "sub-jd-u5a-first", None, None), claims(ISSUER, "sub-jd-u5a-second", None, None)]),
            &store,
        );

        for _ in 0..2 {
            let err = uc.resolve("token", Provisioning::Enabled).await.unwrap_err();
            assert!(matches!(err, AuthnError::IdentityNotProvisioned), "got {err:?}");
        }

        let text = logs.text();
        let lines = jit_disabled_lines(&text);
        assert_eq!(lines.len(), 1, "one line per issuer in the window:\n{text}");
        assert!(has_field(lines[0], "suppressed", "0"), "the first line suppressed nothing: {}", lines[0]);
        assert_no_secrets(&text, &["sub-jd-u5a-first", "sub-jd-u5a-second"]);
    }

    /// SMA-707 U5b: the next admitted line carries the limiter's `suppressed` count. The test
    /// module can reach the private `not_provisioned_log`, so it injects two earlier refusals in
    /// an old window instead of waiting 10 s.
    #[tokio::test]
    async fn jit_disabled_next_admitted_line_carries_the_suppressed_count() {
        let (logs, _logs_guard) = capture_logs();
        let store = AuthnStore::default();
        let uc = jit_disabled_use_case(FakeAuthenticator::ok(claims(ISSUER, "sub-jd-u5b", None, None)), &store);
        let old = Instant::now().checked_sub(std::time::Duration::from_secs(30)).expect("the monotonic clock is older than 30 s");
        assert_eq!(uc.not_provisioned_log.admit_at(ISSUER, (), old), Some(0));
        assert_eq!(uc.not_provisioned_log.admit_at(ISSUER, (), old + std::time::Duration::from_secs(1)), None);

        uc.resolve("token", Provisioning::Enabled).await.unwrap_err();

        let text = logs.text();
        let lines = jit_disabled_lines(&text);
        assert_eq!(lines.len(), 1, "the window is over, so the line is admitted:\n{text}");
        assert!(has_field(lines[0], "suppressed", "1"), "the line reports the one suppressed refusal: {}", lines[0]);
        assert_no_secrets(&text, &["sub-jd-u5b"]);
    }

    /// SMA-707 U6: the limiter key holds the issuer. A refusal for a second JIT-disabled issuer,
    /// just after one for the first issuer, writes its own line, and each line names its issuer.
    #[tokio::test]
    async fn jit_disabled_refusals_from_two_issuers_log_one_line_each() {
        let (logs, _logs_guard) = capture_logs();
        let store = AuthnStore::default();
        let uc = AuthenticateToken::new(
            QueueAuthenticator::new(vec![claims(ISSUER, "sub-jd-u6-a", None, None), claims(OTHER_ISSUER, "sub-jd-u6-b", None, None)]),
            InMemoryIdentities(store.clone()),
            InMemoryPrincipals(store.clone()),
            InMemoryMemberships::default(),
            Arc::new(InMemoryRoleGrants::default()),
            SeqIds::default(),
            FixedClock::default(),
            JitPolicy::from_issuers(&[(Issuer::parse(ISSUER).unwrap(), false), (Issuer::parse(OTHER_ISSUER).unwrap(), false)]),
        );

        uc.resolve("token", Provisioning::Enabled).await.unwrap_err();
        uc.resolve("token", Provisioning::Enabled).await.unwrap_err();

        let text = logs.text();
        let lines = jit_disabled_lines(&text);
        assert_eq!(lines.len(), 2, "one line for each issuer:\n{text}");
        assert!(has_field(lines[0], "issuer", ISSUER), "the first line names the first issuer: {}", lines[0]);
        assert!(has_field(lines[1], "issuer", OTHER_ISSUER), "the second line names the second issuer: {}", lines[1]);
        assert_no_secrets(&text, &["sub-jd-u6-a", "sub-jd-u6-b"]);
    }

    /// SMA-707 U7 (spec 4.4): `AppState` clones the use case for each request. The clone shares
    /// the limiter through its `Arc`, so a refusal on the clone inside the window is suppressed.
    /// `KernelIdGenerator` is the only `Clone` id generator in the crate.
    #[tokio::test]
    async fn jit_disabled_cloned_use_case_shares_the_rate_limiter() {
        let (logs, _logs_guard) = capture_logs();
        let store = AuthnStore::default();
        let uc = AuthenticateToken::new(
            QueueAuthenticator::new(vec![claims(ISSUER, "sub-jd-u7-a", None, None), claims(ISSUER, "sub-jd-u7-b", None, None)]),
            InMemoryIdentities(store.clone()),
            InMemoryPrincipals(store.clone()),
            InMemoryMemberships::default(),
            Arc::new(InMemoryRoleGrants::default()),
            KernelIdGenerator,
            FixedClock::default(),
            JitPolicy::from_issuers(&[(Issuer::parse(ISSUER).unwrap(), false)]),
        );
        let twin = uc.clone();

        uc.resolve("token", Provisioning::Enabled).await.unwrap_err();
        twin.resolve("token", Provisioning::Enabled).await.unwrap_err();

        let text = logs.text();
        assert_eq!(jit_disabled_lines(&text).len(), 1, "the clone must share the limiter:\n{text}");
        assert_no_secrets(&text, &["sub-jd-u7-a", "sub-jd-u7-b"]);
    }

    /// SMA-707 U8 (Review Focus R1): a JIT-ENABLED issuer never writes the line, for a JIT
    /// success and for a JIT failure. Control: the JIT failure writes the SMA-698 line, so the
    /// capture works.
    #[tokio::test]
    async fn jit_disabled_line_is_absent_for_a_jit_enabled_issuer() {
        let (logs, _logs_guard) = capture_logs();
        let store = AuthnStore::default();
        let uc = jit_use_case(
            QueueAuthenticator::new(vec![claims(ISSUER, "sub-jd-u8-ok", Some("jd-u8@example.com"), None), claims(ISSUER, "sub-jd-u8-no-email", None, None)]),
            &store,
        );

        uc.resolve("token", Provisioning::Enabled).await.unwrap();
        let err = uc.resolve("token", Provisioning::Enabled).await.unwrap_err();

        assert!(matches!(err, AuthnError::ProvisioningFailed(ProvisioningDefect::MissingEmail)), "got {err:?}");
        let text = logs.text();
        assert!(jit_disabled_lines(&text).is_empty(), "a JIT-enabled issuer never writes the line:\n{text}");
        assert_eq!(jit_lines(&text).len(), 1, "control: the SMA-698 line appears:\n{text}");
    }
```

- [ ] **Step 2: Run the tests and see them fail to compile**

Run: `cd /Users/smaschek/dev/paigasus/paigasus-core-sma-707/rs && cargo nextest run -p paigasus-iam --lib jit_disabled_`
Expected: FAIL at compile time with `error[E0609]: no field `not_provisioned_log` on type `AuthenticateToken<…>`` (from U5b).

- [ ] **Step 3: Add the field and its init only**

In `struct AuthenticateToken`, after the `provisioning_log` field (line 149), add:

```rust
    /// SMA-707: at most one JIT-disabled refusal line per issuer in 10 s. An `Arc`, for the same
    /// reason as `provisioning_log`. A separate instance keyed by `()`, because `provisioning_log`
    /// is keyed by the SMA-698 `defect` label, and `jit_disabled` is not a defect (SMA-707 D4).
    not_provisioned_log: Arc<LogRateLimiter<()>>,
```

In `AuthenticateToken::new`, after the line `            provisioning_log: Arc::new(LogRateLimiter::new(LOG_RATE_LIMIT_INTERVAL)),`, add:

```rust
            not_provisioned_log: Arc::new(LogRateLimiter::new(LOG_RATE_LIMIT_INTERVAL)),
```

- [ ] **Step 4: Run the tests and see the behavior fail**

Run: `cd /Users/smaschek/dev/paigasus/paigasus-core-sma-707/rs && cargo nextest run -p paigasus-iam --lib --no-fail-fast jit_disabled_`
Expected: FAIL. These tests fail on a line-count assertion, because nothing writes the line yet: U1 (`exactly one JIT-disabled line expected`), U2 and U3 (`control: the protected path writes one line`), U5a, U5b, U6, U7. These pass: U4, U8, and `jit_disabled_issuer_returns_identity_not_provisioned`.

If this step does not compile with `field `not_provisioned_log` is never read`, the lib target was built without `cfg(test)`. Then go to Step 5, and record in the task report that the red was the compile error of Step 2 only.

- [ ] **Step 5: Add the helper, the branch and the doc comments**

(a) In `authenticate_token.rs`, replace the `Provisioning` doc (lines 21-24):

```rust
/// Whether `resolve` may just-in-time provision an unknown `(issuer, subject)` identity.
/// The middleware calls `resolve(.., Enabled)`; `Introspect` always calls `resolve(..,
/// Disabled)` (D10) — an unauthenticated, middleware-exempt endpoint must not have a
/// user-creation side effect.
```

with:

```rust
/// Whether `resolve` may just-in-time provision an unknown `(issuer, subject)` identity.
/// The middleware calls `resolve(.., Enabled)`; `Introspect` always calls `resolve(..,
/// Disabled)` (D10) — an unauthenticated, middleware-exempt endpoint must not have a
/// user-creation side effect. Only `Enabled` writes the SMA-707 `info` line for an unknown
/// identity of an issuer with JIT disabled.
```

(b) Replace the `resolve` doc and the branch (lines 176-191):

```rust
    /// Verifies `token` and resolves it to a local principal (§6.1). `provisioning`
    /// controls whether an unknown `(issuer, subject)` gets just-in-time provisioned
    /// (`Enabled`, the authenticated-request path) or rejected (`Disabled`, `Introspect`'s
    /// D10 read-only guarantee). JIT additionally requires the issuer's `JitPolicy` flag —
    /// an issuer with JIT disabled never provisions even under `Enabled`.
    pub async fn resolve(&self, token: &str, provisioning: Provisioning) -> Result<AuthnPrincipal, AuthnError> {
        let claims = self.authenticator.authenticate(token).await?;

        let principal_id = match self.identities.find_by_issuer_subject(&claims.issuer, &claims.subject).await.map_err(backend)? {
            Some(identity) => identity.principal_id,
            None => match provisioning {
                Provisioning::Disabled => return Err(AuthnError::IdentityNotProvisioned),
                Provisioning::Enabled => {
                    if !self.jit.allows(&claims.issuer) {
                        return Err(AuthnError::IdentityNotProvisioned);
                    }
```

with:

```rust
    /// Verifies `token` and resolves it to a local principal (§6.1). `provisioning`
    /// controls whether an unknown `(issuer, subject)` gets just-in-time provisioned
    /// (`Enabled`, the authenticated-request path) or rejected (`Disabled`, `Introspect`'s
    /// D10 read-only guarantee). JIT additionally requires the issuer's `JitPolicy` flag —
    /// an issuer with JIT disabled never provisions even under `Enabled`. That refusal writes one
    /// rate-limited `info` line through `jit_disabled` (SMA-707); the `Disabled` refusal writes none.
    pub async fn resolve(&self, token: &str, provisioning: Provisioning) -> Result<AuthnPrincipal, AuthnError> {
        let claims = self.authenticator.authenticate(token).await?;

        let principal_id = match self.identities.find_by_issuer_subject(&claims.issuer, &claims.subject).await.map_err(backend)? {
            Some(identity) => identity.principal_id,
            None => match provisioning {
                Provisioning::Disabled => return Err(AuthnError::IdentityNotProvisioned),
                Provisioning::Enabled => {
                    if !self.jit.allows(&claims.issuer) {
                        return Err(self.jit_disabled(&claims.issuer));
                    }
```

(c) Add the helper directly after `fn provisioning_failed` (after its closing `}` at line 346, inside the `impl` block):

```rust

    /// The only way the JIT-disabled branch of `resolve` builds its error (SMA-707 spec 4.2). It
    /// asks the rate limiter, writes one `info` line when admitted, and returns
    /// `IdentityNotProvisioned`. It receives only the issuer, so the line cannot carry the
    /// subject, a claim or the token. `info`, not `warn`: JIT off closes the user set of the
    /// issuer, so this refusal is the intended result of the operator's setting (SMA-707 D2).
    fn jit_disabled(&self, issuer: &Issuer) -> AuthnError {
        if let Some(suppressed) = self.not_provisioned_log.admit_at(issuer.as_str(), (), Instant::now()) {
            tracing::info!(
                issuer = issuer.as_str(),
                reason = "jit_disabled",
                suppressed,
                "request refused: the identity is not provisioned, and just-in-time provisioning is disabled for this issuer; IAM creates an identity only by just-in-time provisioning, so set jit_provisioning = true for the issuer to let new users of this issuer sign in"
            );
        }
        AuthnError::IdentityNotProvisioned
    }
```

(d) In `rs/crates/services/paigasus-iam/src/application/log_rate_limit.rs`, replace lines 13-16:

```rust
/// Rate limit for diagnostic log lines (SMA-686 D14). One signed token can otherwise write one
/// line per request. Keyed by (issuer, kind), so the map holds at most `issuers × kinds` entries.
/// The validator uses it with `TokenDefect`, and `AuthenticateToken` with the JIT `defect` label
/// (SMA-698 D2). A suppressed event is counted, and the next admitted line reports the count.
```

with:

```rust
/// Rate limit for diagnostic log lines (SMA-686 D14). One signed token can otherwise write one
/// line per request. Keyed by (issuer, kind), so the map holds at most `issuers × kinds` entries.
/// The validator uses it with `TokenDefect`. `AuthenticateToken` uses it with the JIT `defect`
/// label (SMA-698 D2), and with `()` for the JIT-disabled refusal line, one line per issuer
/// (SMA-707). A suppressed event is counted, and the next admitted line reports the count.
```

- [ ] **Step 6: Run the tests and see them pass**

Run: `cd /Users/smaschek/dev/paigasus/paigasus-core-sma-707/rs && cargo fmt --all && cargo nextest run -p paigasus-iam --lib --no-fail-fast jit_disabled_`
Expected: PASS for all nine `jit_disabled_*` tests (U1-U8 and `jit_disabled_issuer_returns_identity_not_provisioned`).

- [ ] **Step 7: Run the whole lib suite and lint**

Run: `cd /Users/smaschek/dev/paigasus/paigasus-core-sma-707/rs && cargo nextest run -p paigasus-iam --lib && cargo clippy --workspace --all-targets --locked -- -D warnings`
Expected: PASS. Every `paigasus-iam` unit test passes, the SMA-698 tests included. Clippy reports no warning. `admit_at(.., (), ..)` passes the unit literal, which `clippy::unit_arg` does not flag. If clippy flags it, stop and report; do not add an `#[allow]`.

- [ ] **Step 8: Commit**

```bash
git -C /Users/smaschek/dev/paigasus/paigasus-core-sma-707 add \
  rs/crates/services/paigasus-iam/src/application/authenticate_token.rs \
  rs/crates/services/paigasus-iam/src/application/log_rate_limit.rs
git -C /Users/smaschek/dev/paigasus/paigasus-core-sma-707 commit -F <scratchpad>/msg-task1.txt
```

Message file (`<scratchpad>` is your scratchpad directory):

```text
feat(rs): log the JIT-disabled identity-not-provisioned refusal (SMA-707)

When a protected request comes from an unknown identity of an issuer with
jit_provisioning = false, IAM writes one info line with the issuer,
reason="jit_disabled" and suppressed. A limiter keyed by the issuer admits one
line in 10 s. Introspect writes no line. The status and the error code do not
change.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
```

---

## Task 2: Integration tests T1-T4

**Files:**
- Modify: `rs/crates/services/paigasus-iam/tests/support/mod.rs` (append after line 1004)
- Modify: `rs/crates/services/paigasus-iam/tests/http_authn.rs` (replace lines 328-346; append at the end)
- Modify: `rs/crates/services/paigasus-iam/tests/grpc_whoami.rs` (replace lines 138-160; append at the end)
- Temporarily modify (mutation M1, restored in this task): `rs/crates/services/paigasus-iam/src/application/authenticate_token.rs`

**Interfaces:**
- Consumes: `support::{capture_logs, start_migrated_postgres, start_mock_idp, test_config_with, send}`; `MockIdp::bearer(&self, sub: &str, email: Option<&str>, aud: &str, exp_offset_secs: i64) -> String`; `MockIdp.issuer: String` (the exact string `Issuer::as_str` returns, because `Issuer::parse` only trims); `AppState::new(db, &IamConfig)`; `router(AppState)` in `http_authn.rs`; `http_router(AppState)`, `spawn_server(AppState) -> (SocketAddr, JoinHandle<()>)`, `channel(SocketAddr)`, `authed(msg, &str)`, `reason_of(&tonic::Status) -> String` in `grpc_whoami.rs`; the Task 1 line.
- Produces: `pub const JIT_DISABLED_LINE: &str` in `tests/support/mod.rs`; the tests `jit_disabled_unknown_identity_is_403` (T1, extended), `introspect_on_a_jit_disabled_issuer_writes_no_jit_disabled_line` (T3, new), `who_am_i_obeys_the_jit_policy` (T2, extended), `jit_disabled_refusals_on_http_and_grpc_share_one_line` (T4, new).

The behavior exists since Task 1, so these tests can pass on their first run. This task therefore proves the red with mutation M1 on the committed Task 1 code, and then restores it.

- [ ] **Step 1: Add the selector constant**

Append to `rs/crates/services/paigasus-iam/tests/support/mod.rs`, after the `JIT_FAILURE_LINE` constant:

```rust

/// The fixed prefix of the SMA-707 JIT-disabled refusal line. Count only lines that contain it,
/// for the same reason as `JIT_FAILURE_LINE`.
#[allow(dead_code)]
pub const JIT_DISABLED_LINE: &str = "request refused: the identity is not provisioned";
```

- [ ] **Step 2: Extend T1**

In `rs/crates/services/paigasus-iam/tests/http_authn.rs`, replace the whole test `jit_disabled_unknown_identity_is_403` (lines 328-346, from `#[tokio::test]` to its closing `}`) with:

```rust
/// A valid token from a JIT-disabled issuer for an unknown identity is 403
/// `identity-not-provisioned`. SMA-707 T1: IAM also writes one `info` line that names the issuer,
/// and not the subject or the email.
#[tokio::test]
async fn jit_disabled_unknown_identity_is_403() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    // Installed before `AppState::new`, which writes its own `accept_invalid_tls` warn line; the
    // filter on `JIT_DISABLED_LINE` below skips it.
    let (logs, _logs_guard) = support::capture_logs();
    let jit_enabled = start_mock_idp().await;
    let jit_disabled = start_mock_idp().await;
    // Two configured issuers; the second has jit_provisioning = false.
    let state = AppState::new(db, &test_config_with(&[(&jit_enabled, true), (&jit_disabled, false)], 30)).await.expect("AppState::new");
    let app = router(state);

    // A valid token from the JIT-disabled issuer for an unknown identity: the middleware
    // verifies the signature but refuses to provision (per-issuer flag, D5), so the request
    // is 403 `identity-not-provisioned` instead of a JIT success.
    let token = jit_disabled.bearer("t1-jd-subject-5e2c", Some("t1-jd-mail-5e2c@example.com"), "paigasus", 3600);
    let (status, body) = send(&app, "POST", "/v1/organizations", Some(json!({ "slug": "acme", "name": "Acme" })), Some(&token)).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"]["code"], "identity-not-provisioned");

    let text = logs.text();
    let lines: Vec<&str> = text.lines().filter(|line| line.contains(support::JIT_DISABLED_LINE)).collect();
    assert_eq!(lines.len(), 1, "exactly one JIT-disabled line expected:\n{text}");
    let line = lines[0];
    assert!(line.contains("INFO"), "the line is at info: {line}");
    assert!(line.contains(&format!("issuer=\"{}\"", jit_disabled.issuer)), "the line names the JIT-disabled issuer: {line}");
    assert!(!text.contains("t1-jd-subject-5e2c"), "the log must not contain the subject:\n{text}");
    assert!(!text.contains("t1-jd-mail-5e2c"), "the log must not contain the email:\n{text}");
}
```

- [ ] **Step 3: Write T3**

Append to `rs/crates/services/paigasus-iam/tests/http_authn.rs`:

```rust

/// SMA-707 T3: `POST /v1/authn/introspect` for an unknown identity of a JIT-disabled issuer is
/// 403 and writes no JIT-disabled line (D10). The issuer must be JIT-disabled: with JIT on, a
/// misplaced line would not appear either, so `introspect_unknown_identity_is_403_and_never_provisions`
/// cannot detect it. Control in the same test: a protected request with the same token then writes
/// exactly one line, so the capture can see the line on every run.
#[tokio::test]
async fn introspect_on_a_jit_disabled_issuer_writes_no_jit_disabled_line() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (logs, _logs_guard) = support::capture_logs();
    let idp = start_mock_idp().await;
    let state = AppState::new(db, &test_config_with(&[(&idp, false)], 30)).await.expect("AppState::new");
    let app = router(state);
    let token = idp.bearer("t3-jd-subject-8b4d", Some("t3-jd-mail-8b4d@example.com"), "paigasus", 3600);

    let (status, body) = send(&app, "POST", "/v1/authn/introspect", Some(json!({ "token": token })), None).await;

    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"]["code"], "identity-not-provisioned");
    let text = logs.text();
    let lines: Vec<&str> = text.lines().filter(|line| line.contains(support::JIT_DISABLED_LINE)).collect();
    assert!(lines.is_empty(), "introspect must not write the JIT-disabled line:\n{text}");

    let (status, body) = send(&app, "GET", "/v1/organizations", None, Some(&token)).await;

    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"]["code"], "identity-not-provisioned");
    let text = logs.text();
    let lines: Vec<&str> = text.lines().filter(|line| line.contains(support::JIT_DISABLED_LINE)).collect();
    assert_eq!(lines.len(), 1, "control: the protected request writes exactly one line:\n{text}");
    let line = lines[0];
    assert!(line.contains("INFO"), "the line is at info: {line}");
    assert!(line.contains(&format!("issuer=\"{}\"", idp.issuer)), "the line names the issuer: {line}");
    assert!(!text.contains("t3-jd-subject-8b4d"), "the log must not contain the subject:\n{text}");
    assert!(!text.contains("t3-jd-mail-8b4d"), "the log must not contain the email:\n{text}");
}
```

- [ ] **Step 4: Extend T2 and fix its stale doc reference**

In `rs/crates/services/paigasus-iam/tests/grpc_whoami.rs`, replace the whole test `who_am_i_obeys_the_jit_policy` with its doc comment (lines 138-160, from `/// Bearer enforcement is necessary but not sufficient.` to the closing `}`) with:

```rust
/// Bearer enforcement is necessary but not sufficient. `AuthenticateToken::resolve` still checks
/// the issuer's JIT flag under `Provisioning::Enabled` (the `!self.jit.allows` branch of
/// `AuthenticateToken::resolve`, which calls `jit_disabled`), so an issuer with JIT off gets the
/// same `identity-not-provisioned` Introspect gives — the console keeps a branch for it. SMA-707
/// T2: the refusal also writes one `info` line that names the issuer, and not the subject or the
/// email. The capture sees the spawned server, as
/// `who_am_i_with_a_token_without_email_logs_the_provisioning_failure` proves.
#[tokio::test]
async fn who_am_i_obeys_the_jit_policy() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (logs, _logs_guard) = support::capture_logs();
    let idp = support::start_mock_idp().await;
    let cfg = support::test_config_with(&[(&idp, false)], 30);
    let state = AppState::new(db, &cfg).await.unwrap();
    let token = idp.bearer("t2-jd-subject-9c1e", Some("t2-jd-mail-9c1e@example.com"), "paigasus", 3600);
    let (addr, server) = spawn_server(state).await;
    let ch = channel(addr).await;
    let mut client = AuthnServiceClient::new(ch);

    let err = client.who_am_i(authed(WhoAmIRequest {}, &token)).await.unwrap_err();
    assert_eq!(err.code(), Code::PermissionDenied, "{err:?}");
    assert_eq!(reason_of(&err), "identity-not-provisioned", "{err:?}");

    let text = logs.text();
    let lines: Vec<&str> = text.lines().filter(|line| line.contains(support::JIT_DISABLED_LINE)).collect();
    assert_eq!(lines.len(), 1, "exactly one JIT-disabled line expected:\n{text}");
    let line = lines[0];
    assert!(line.contains("INFO"), "the line is at info: {line}");
    assert!(line.contains(&format!("issuer=\"{}\"", idp.issuer)), "the line names the issuer: {line}");
    assert!(!text.contains("t2-jd-subject-9c1e"), "the log must not contain the subject:\n{text}");
    assert!(!text.contains("t2-jd-mail-9c1e"), "the log must not contain the email:\n{text}");

    server.abort();
}
```

- [ ] **Step 5: Write T4 (Review Focus R3)**

Append to `rs/crates/services/paigasus-iam/tests/grpc_whoami.rs`:

```rust

/// SMA-707 T4 (Review Focus R3): production serves HTTP and gRPC from ONE `AppState`, so the two
/// transports share one limiter. A refusal on HTTP and then one on gRPC, for the same
/// JIT-disabled issuer inside the 10 s window, write one line together.
#[tokio::test]
async fn jit_disabled_refusals_on_http_and_grpc_share_one_line() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (logs, _logs_guard) = support::capture_logs();
    let idp = support::start_mock_idp().await;
    let state = AppState::new(db, &support::test_config_with(&[(&idp, false)], 30)).await.unwrap();
    let token = idp.bearer("t4-jd-subject-3a7f", Some("t4-jd-mail-3a7f@example.com"), "paigasus", 3600);
    let http = http_router(state.clone());
    let (addr, server) = spawn_server(state).await;
    let mut client = AuthnServiceClient::new(channel(addr).await);

    let (status, body) = support::send(&http, "GET", "/v1/organizations", None, Some(token.as_str())).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"]["code"], "identity-not-provisioned");
    let err = client.who_am_i(authed(WhoAmIRequest {}, &token)).await.unwrap_err();
    assert_eq!(err.code(), Code::PermissionDenied, "{err:?}");
    assert_eq!(reason_of(&err), "identity-not-provisioned", "{err:?}");

    let text = logs.text();
    let lines: Vec<&str> = text.lines().filter(|line| line.contains(support::JIT_DISABLED_LINE)).collect();
    assert_eq!(lines.len(), 1, "both transports share one limiter:\n{text}");
    let line = lines[0];
    assert!(line.contains("INFO"), "the line is at info: {line}");
    assert!(line.contains(&format!("issuer=\"{}\"", idp.issuer)), "the line names the issuer: {line}");
    assert!(!text.contains("t4-jd-subject-3a7f"), "the log must not contain the subject:\n{text}");
    assert!(!text.contains("t4-jd-mail-3a7f"), "the log must not contain the email:\n{text}");

    server.abort();
}
```

- [ ] **Step 6: Confirm the start state for the mutation**

Run: `git -C /Users/smaschek/dev/paigasus/paigasus-core-sma-707 status --short -- rs/crates/services/paigasus-iam/src`
Expected: no output. Task 1 is committed, so `authenticate_token.rs` matches `HEAD`.

- [ ] **Step 7: Apply mutation M1 and see the tests fail**

In `authenticate_token.rs`, in `fn jit_disabled`, replace:

```rust
            tracing::info!(
                issuer = issuer.as_str(),
                reason = "jit_disabled",
                suppressed,
                "request refused: the identity is not provisioned, and just-in-time provisioning is disabled for this issuer; IAM creates an identity only by just-in-time provisioning, so set jit_provisioning = true for the issuer to let new users of this issuer sign in"
            );
```

with:

```rust
            let _ = (issuer, suppressed);
```

Run: `cd /Users/smaschek/dev/paigasus/paigasus-core-sma-707/rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --no-fail-fast --test http_authn --test grpc_whoami jit_disabled_unknown_identity_is_403 introspect_on_a_jit_disabled_issuer who_am_i_obeys_the_jit_policy jit_disabled_refusals_on_http_and_grpc`
Expected: FAIL for all four tests. T1, T2 and T4 fail with `exactly one JIT-disabled line expected` or `both transports share one limiter` (0 lines). T3 fails on its control, `control: the protected request writes exactly one line`. If Docker is not available, the run fails at the Docker preflight: stop and report. Do not run without `PAIGASUS_REQUIRE_DOCKER=1`.

- [ ] **Step 8: Restore M1 and confirm the restore**

With the Edit tool, replace `            let _ = (issuer, suppressed);` with the original `tracing::info!(…);` block from Step 7, character for character.

Run: `git -C /Users/smaschek/dev/paigasus/paigasus-core-sma-707 status --short -- rs/crates/services/paigasus-iam/src`
Expected: no output. This proves the restore.

- [ ] **Step 9: Run the tests and see them pass**

Run: `cd /Users/smaschek/dev/paigasus/paigasus-core-sma-707/rs && cargo fmt --all && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test http_authn --test grpc_whoami && cargo clippy --workspace --all-targets --locked -- -D warnings`
Expected: PASS for every test in both files, T1-T4 included, and no clippy warning. If a "must not contain the subject" assertion fails, read which line holds the subject. Report it. Do not remove the assertion.

- [ ] **Step 10: Commit**

```bash
git -C /Users/smaschek/dev/paigasus/paigasus-core-sma-707 add \
  rs/crates/services/paigasus-iam/tests/support/mod.rs \
  rs/crates/services/paigasus-iam/tests/http_authn.rs \
  rs/crates/services/paigasus-iam/tests/grpc_whoami.rs
git -C /Users/smaschek/dev/paigasus/paigasus-core-sma-707 commit -F <scratchpad>/msg-task2.txt
```

Message file:

```text
test(rs): cover the JIT-disabled refusal line on HTTP and gRPC (SMA-707)

A protected HTTP request and WhoAmI each write one info line for an unknown
identity of a JIT-disabled issuer, with the issuer and no subject or email.
HTTP introspect writes no line, and a control request proves the capture.
One refusal on each transport writes one line, because both share one limiter.
The tests went red under mutation M1 before the commit.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
```

---

## Task 3: Documentation

**Files:**
- Modify: `rs/crates/services/paigasus-iam/iam.toml.example` (after line 51)
- Modify: `rs/crates/services/paigasus-iam/CHANGELOG.md` (after line 22, in `## [Unreleased]` / `### Added`)

**Interfaces:**
- Consumes: the prefix, the fields (`issuer`, `reason`, `suppressed`) and the 10 s window from Task 1.
- Produces: operator text only. No gate reads these two texts.

- [ ] **Step 1: `iam.toml.example`**

Replace:

```toml
# REQUIRED — no default. At least one issuer:
# [[authn.issuers]]
# issuer = "https://idp.example.com/realms/acme"
# audiences = ["paigasus"]
# jit_provisioning = true             # default shown
```

with:

```toml
# REQUIRED — no default. At least one issuer:
# [[authn.issuers]]
# issuer = "https://idp.example.com/realms/acme"
# audiences = ["paigasus"]
# jit_provisioning = true             # default shown
#
# jit_provisioning = false closes the user set of this issuer. IAM creates an identity only by
# just-in-time provisioning. No API links an issuer and a subject to a user. So with false, only
# the identities that IAM created while the flag was true can sign in. You cannot add a new user
# of this issuer in a different way.
# When IAM refuses an unknown identity of such an issuer on a protected request (HTTP or gRPC), it
# writes one `info` line. The line starts with "request refused: the identity is not provisioned".
# It has the fields issuer, reason="jit_disabled" and suppressed. It does not show the subject,
# the email, another claim or the token. IAM writes at most one line for each issuer in 10
# seconds. The field suppressed gives the number of refusals since the last line.
# Introspect writes no line. A user that gets to IAM only through the gateway causes no line: the
# gateway calls Introspect first, and it answers 401.
```

- [ ] **Step 2: CHANGELOG**

In `rs/crates/services/paigasus-iam/CHANGELOG.md`, replace:

```markdown
- The counter `iam_jit_provisioning_failures_total` carries the label `defect`. It counts each
  refused request. The log rate limit does not apply to it. Both series start at zero when
  metrics are on (SMA-698).
```

with:

```markdown
- The counter `iam_jit_provisioning_failures_total` carries the label `defect`. It counts each
  refused request. The log rate limit does not apply to it. Both series start at zero when
  metrics are on (SMA-698).
- IAM logs a refused request at `info` when the identity is not provisioned and the issuer has
  `jit_provisioning = false`. The line starts with
  `request refused: the identity is not provisioned`. The line names the issuer and has the
  fields `reason="jit_disabled"` and `suppressed`. The line does not show the subject, the email,
  another claim or the token. IAM writes at most one line for each issuer in 10 seconds.
  `Introspect` writes no line. Before, IAM answered `403 identity-not-provisioned` and logged
  nothing (SMA-707).
```

- [ ] **Step 3: Check the text**

Run: `git -C /Users/smaschek/dev/paigasus/paigasus-core-sma-707 grep -n "request refused: the identity is not provisioned" -- rs/crates/services/paigasus-iam`
Expected: exactly these hits: `CHANGELOG.md` (1), `iam.toml.example` (1), `src/application/authenticate_token.rs` (3: the event, `JIT_DISABLED_LINE`, `JIT_DISABLED_TEXT`), `tests/support/mod.rs` (1).

- [ ] **Step 4: Commit**

```bash
git -C /Users/smaschek/dev/paigasus/paigasus-core-sma-707 add \
  rs/crates/services/paigasus-iam/iam.toml.example \
  rs/crates/services/paigasus-iam/CHANGELOG.md
git -C /Users/smaschek/dev/paigasus/paigasus-core-sma-707 commit -F <scratchpad>/msg-task3.txt
```

Message file:

```text
docs(rs): document the JIT-disabled refusal line (SMA-707)

iam.toml.example says what jit_provisioning = false means (a closed user
set), names the info line and its fields, and says that Introspect and
gateway-only users write no line. The CHANGELOG records the line.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
```

---

## Task 4: Prove that the tests bite (spec 6.3), then the final verification

No commit in this task. Each mutation must compile: the crate denies warnings, so a mutation that fails at `rustc` proves nothing. Apply one mutation at a time with the Edit tool, run the full crate, record every red test, and restore by removing the inserted change with the Edit tool. Never use `git checkout --`.

**Files:**
- Temporarily modify: `rs/crates/services/paigasus-iam/src/application/authenticate_token.rs` only.

**Interfaces:**
- Consumes: every test from Tasks 1 and 2.
- Produces: the mutation results table and the verification results, in the task report.

The run command for every mutation (from `rs/`):

```bash
PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --no-fail-fast
```

- [ ] **Step 1: Confirm the clean start and the green baseline**

Run: `git -C /Users/smaschek/dev/paigasus/paigasus-core-sma-707 status --short -- rs/`
Expected: no output (Tasks 1-3 are committed).

Run the command above with no mutation. Expected: PASS. Record any red test here as a baseline flake. The Docker suite `tests/authz_policy_store.rs` is known to flake under parallel load. A test that is red on the baseline does not count as "red" for a mutation row.

- [ ] **Step 2: Apply, run and restore each mutation**

For each row:
1. Apply the edit with the Edit tool.
2. Run `cd /Users/smaschek/dev/paigasus/paigasus-core-sma-707/rs && cargo build -p paigasus-iam --tests`. If it does not compile, the mutation is invalid. Change the mutation text so that it compiles and keeps its intent, and write the change in the report.
3. Run the command above. Record every red test with its failure message.
4. Restore with the Edit tool (the reverse edit). Run `git -C /Users/smaschek/dev/paigasus/paigasus-core-sma-707 status --short -- rs/`. Expected: no output. That is the proof of the restore.

A row passes when every test in "Must go red (spec)" and "Must go red (plan additions)" is red. If a listed test stays green, report the row. Do not change a test in this task.

| M | Exact edit (in `authenticate_token.rs`) | Must go red (spec) | Must go red (plan additions) |
|---|---|---|---|
| M1 | In `jit_disabled`, replace the whole `tracing::info!(…);` statement with `let _ = (issuer, suppressed);` | U1, U2 (control), U3 (control), U5a, U5b, U6, U7, T1, T2, T3 (control) | T4 |
| M2 | Replace the `None => match provisioning { … },` arm of `resolve` with the block below this table. It writes the line for every unknown identity of a JIT-disabled issuer, before the `match provisioning`, and the `Enabled` branch returns the plain error again | U2, U3, T3 | — |
| M3 | Change the signature to `fn jit_disabled(&self, issuer: &Issuer, claims: &ValidatedClaims) -> AuthnError`; change the call in `resolve` to `return Err(self.jit_disabled(&claims.issuer, &claims));`; in the event, add the line `subject = claims.subject.as_str(),` after `issuer = issuer.as_str(),` | U1, T1, T2 | T3, T4 (their subject check) |
| M4 | Replace `if let Some(suppressed) = self.not_provisioned_log.admit_at(issuer.as_str(), (), Instant::now()) {` with `if let Some(suppressed) = { let _ = self.not_provisioned_log.admit_at(issuer.as_str(), (), Instant::now()); Some(0_u64) } {` | U5a, U7 | U5b, T4 |
| M5 | Replace `admit_at(issuer.as_str(), (), Instant::now())` with `admit_at("", (), Instant::now())` | U6 | — |
| M6 | In the event, replace the field line `suppressed,` with `suppressed = 0_u64,`, and insert `let _ = suppressed;` as the first line inside the `if let` block, before `tracing::info!(` | U5b | — |
| M7a | Replace `tracing::info!(` with `tracing::debug!(` | U1, T1, T2 | T3, T4 |
| M7b | Replace `tracing::info!(` with `tracing::warn!(` (a separate run from M7a) | U1, T1, T2 | T3, T4 |
| M8 | Delete the field line `issuer = issuer.as_str(),` from the event | U1, U6, T1, T2 | T3, T4 |
| M9 | Review Focus R1, not in spec 6.3. In `resolve`, insert `let _ = self.jit_disabled(&claims.issuer);` as the first line inside `Provisioning::Enabled => {`, before `if !self.jit.allows(&claims.issuer) {` | — | U8 |

The M2 replacement for the `None =>` arm (the original arm ends with `},` after `self.jit_provision(&claims).await?` and its closing braces):

```rust
            None => {
                if !self.jit.allows(&claims.issuer) {
                    let _ = self.jit_disabled(&claims.issuer);
                }
                match provisioning {
                    Provisioning::Disabled => return Err(AuthnError::IdentityNotProvisioned),
                    Provisioning::Enabled => {
                        if !self.jit.allows(&claims.issuer) {
                            return Err(AuthnError::IdentityNotProvisioned);
                        }
                        self.jit_provision(&claims).await?
                    }
                }
            }
```

The test names, for the table:

| Id | Test |
|---|---|
| U1 | `application::authenticate_token::tests::jit_disabled_unknown_identity_writes_one_info_line_without_claims` |
| U2 | `…::jit_disabled_introspect_writes_no_line` |
| U3 | `…::jit_disabled_resolve_disabled_writes_no_line` |
| U4 | `…::jit_disabled_known_identity_resolves_and_writes_no_line` |
| U5a | `…::jit_disabled_refusals_for_one_issuer_log_once` |
| U5b | `…::jit_disabled_next_admitted_line_carries_the_suppressed_count` |
| U6 | `…::jit_disabled_refusals_from_two_issuers_log_one_line_each` |
| U7 | `…::jit_disabled_cloned_use_case_shares_the_rate_limiter` |
| U8 | `…::jit_disabled_line_is_absent_for_a_jit_enabled_issuer` |
| T1 | `http_authn::jit_disabled_unknown_identity_is_403` |
| T2 | `grpc_whoami::who_am_i_obeys_the_jit_policy` |
| T3 | `http_authn::introspect_on_a_jit_disabled_issuer_writes_no_jit_disabled_line` |
| T4 | `grpc_whoami::jit_disabled_refusals_on_http_and_grpc_share_one_line` |

- [ ] **Step 3: Fill in the results table**

Copy this table into the task report and fill it in:

| M | Compiled | Red tests (all) | Every listed test red? | Restore proven (`git status` empty) |
|---|---|---|---|---|
| M1 | | | | |
| M2 | | | | |
| M3 | | | | |
| M4 | | | | |
| M5 | | | | |
| M6 | | | | |
| M7a | | | | |
| M7b | | | | |
| M8 | | | | |
| M9 | | | | |

- [ ] **Step 4: Crate verification after the last restore**

Run from `/Users/smaschek/dev/paigasus/paigasus-core-sma-707/rs`:

```bash
cargo fmt --check
cargo clippy --workspace -- -D warnings
cargo clippy --workspace --all-targets --locked -- -D warnings
PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam
```

Expected: each command exits 0. `cargo fmt --check` prints nothing. Then `git -C /Users/smaschek/dev/paigasus/paigasus-core-sma-707 status --short -- rs/` prints nothing.

- [ ] **Step 5: Prepare the full gate graph**

No single local bash runs every gate on this Mac (root `CLAUDE.md`, "This development Mac only"). `repo:affected-smoke` needs system bash 3.2. `repo:ruff-ci`, `repo:next-public-free`, `repo:publish-metadata`, `repo:version-lockstep` and `repo:nats-permissions` need bash 4+. `repo:actionlint` needs bash 5 and a pipe that holds at least 8192 bytes. Moon finds `bash` through `PATH`, so a directory that holds only a `bash` link selects the bash. Never put `/bin` first in `PATH`: that also changes `python3`.

Set `S` to your scratchpad directory, then run:

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core-sma-707
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
git fetch origin
mkdir -p "$S/bash32" "$S/bash5"
ln -sf /bin/bash "$S/bash32/bash"
ln -sf /opt/homebrew/bin/bash "$S/bash5/bash"
```

- [ ] **Step 6: Run the full graph with bash 3.2**

The target list is a copy of the root `CLAUDE.md` `ci-targets` command:

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core-sma-707
PATH="$S/bash32:$HOME/.proto/shims:$HOME/.proto/bin:$PATH" moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site \
  :http-extractor-envelope :input-liveness :promtool :observability-drift \
  :nats-permissions :release-parity :release-parity-py :release-parity-ts \
  :publish-metadata :version-lockstep :workflow-credentials :pyo3-stub-drift :ruff-ci \
  :next-public-free :helm-render :test-e2e \
  --base origin/main --include-relations
```

Expected: every selected task passes, except the bash-4+ gates in the list above if moon selected them. Under bash 3.2 they fail with `mapfile: command not found`, `declare: -A: invalid option`, an `unbound variable` error, or an empty `stdout.log`. That is a bash-version artifact, not a finding. Step 7 gives their real result.

If any other task fails, copy the moon cache report file and `.moon/cache/states/<project>/<task>/` out of the repo BEFORE any re-run. Then follow the root `CLAUDE.md` section "Diagnosing an unattributed `moon ci` failure". A re-run overwrites the evidence, and a passing re-run too.

- [ ] **Step 7: Re-run the bash-4+ gates directly under Homebrew bash**

For each bash-4+ gate that Step 6 selected, run its moon script directly with the bash-5 link first in `PATH`. The result of this step replaces the Step 6 verdict for that gate.

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core-sma-707
export PATH="$S/bash5:$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
bash ci/ruff/run.sh --self-test && bash ci/ruff/run.sh --negative-control && bash ci/ruff/run.sh
bash ci/next-public/run.sh --self-test && bash ci/next-public/run.sh --negative-control && bash ci/next-public/run.sh
bash ci/publish-metadata/run.sh --negative-control && bash ci/publish-metadata/run.sh
bash ci/version-lockstep/run.sh --self-test && bash ci/version-lockstep/run.sh --negative-control && bash ci/version-lockstep/run.sh
bash ops/nats/check-subjects.sh && ( cd rs && cargo nextest run --locked --no-tests=pass -p paigasus-iam --test nats_permissions --test docker_preflight --profile iam-nats )
```

Expected: each line exits 0. A local `version-lockstep --negative-control` can fail with `tar: Write error` and `INFRA: cannot stage`. That is a known host artifact (SMA-686), not a finding: record it, and run the plain `bash ci/version-lockstep/run.sh`.

- [ ] **Step 8: Read the actionlint verdict correctly**

If Step 6 selected `repo:actionlint`, read its output for the line `pipe capacity … bytes (floor 8192)`.
- The pipe holds 65536 bytes: run `/opt/homebrew/bin/bash ci/actionlint/run.sh` directly, and use its result.
- The gate exits rc 2 with a `small` message (the pipe holds 512 bytes): that is a host condition, not a finding. CI is then the only verdict. Record it.

- [ ] **Step 9: Report**

Report: the Step 1 baseline, the Step 3 mutation table, the Step 4 results, the Step 6 result for each failed task, the Step 7 verdicts, and the Step 8 case. Do not push. Do not open a PR.
