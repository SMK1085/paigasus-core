# SMA-707: log the silent 403 `identity-not-provisioned` when an issuer has JIT disabled

- Linear: SMA-707 (milestone "IAM Gaps")
- Related: SMA-698 (JIT provisioning failure line and counter), SMA-686 D14 (log rate-limit policy)
- Status: approved by Sven on 2026-09-27, after the spec challenge (section 10)

## 1. Problem

On the authenticated path, `AuthenticateToken::resolve(.., Provisioning::Enabled)` returns
`AuthnError::IdentityNotProvisioned` when the identity is unknown and the issuer has
`jit_provisioning = false` (`rs/crates/services/paigasus-iam/src/application/authenticate_token.rs:188-191`).
The caller gets `403 identity-not-provisioned` over HTTP, or `PermissionDenied` over gRPC. IAM
writes no log line. The operator cannot see in the logs that the JIT flag of the issuer is the
cause.

SMA-698 section 3 put this case out of scope and named SMA-707 as the follow-up (SMA-698 section 10).

`Introspect` calls `resolve(.., Provisioning::Disabled)` and also returns `IdentityNotProvisioned`
for an unknown identity. That is by design (D10: introspect never provisions). It is a different
case, and it must not write the line.

### 1.1 What JIT off means today

Just-in-time provisioning is the only production path that creates an `external_identity` row.
The only production call of `ExternalIdentityRepository::provision` is in `jit_provision`
(`authenticate_token.rs:297`). No RPC and no HTTP route links an `(issuer, subject)` pair to a
user. `POST /v1/users` creates a user with no identity link (`src/adapters/http/users.rs:51-58`).
IAM does not link identities by email (SMA-698 D5).

So an issuer with JIT off admits only the identities that IAM already created while JIT was on.
This is a "closed user set": an operator turns JIT off to stop new users from this issuer. An
operator cannot pre-provision an OIDC identity. The line must not tell them to try.

A chart deployment cannot turn JIT off. The chart renders `IAM_AUTHN__ISSUERS` with no
`jit_provisioning` key (`charts/paigasus/templates/backend-deployment.yaml:123-124`), so the flag
takes its default `true` (`src/config.rs:183-185`). `extraEnv` refuses to override
`IAM_AUTHN__ISSUERS` (`charts/paigasus/templates/_iam-backend.tpl:17`, `:94`). Only a deployment
that configures IAM through `iam.toml` or its own environment can set the flag to `false`.

## 2. Goals

These are the SMA-707 acceptance criteria, with G4 added from the issue's proposal.

- G1. A protected IAM request, over HTTP or gRPC, for an unknown identity from a JIT-disabled
  issuer writes one `info` line that names the issuer. (AC 1 and AC 4.)
- G2. `Introspect` for an unknown identity writes no line, also when the issuer has JIT disabled.
  (AC 2.)
- G3. The line holds no subject, no email, no name, no other claim value and no token. A test
  asserts this. (AC 3.)
- G4. The line is rate-limited by the SMA-686 D14 policy: one line per issuer in 10 s, and the next
  admitted line reports the number of suppressed events.

## 3. Non-goals

- No Prometheus counter (decision D3).
- No change to the HTTP status, the gRPC status, the error code or the error body.
- No change to `paigasus-iam-core`. `AuthnError` keeps its variants.
- No change to the SMA-698 JIT failure line or to `iam_jit_provisioning_failures_total`.
- No log line on the `Provisioning::Disabled` path.
- No change to the chart. The chart cannot set `jit_provisioning = false` (section 1.1).
- **The gateway path stays silent.** The gateway's model path calls IAM `Introspect`
  (`Provisioning::Disabled`) first, and it answers `401` for `identity-not-provisioned`
  (`rs/crates/services/paigasus-gateway/src/adapters/http/auth.rs:97-117`, `:419-426`). The gateway
  calls a bearer-enforced IAM route only after `Introspect` succeeds. So a user who reaches IAM
  only through the gateway, from a JIT-disabled issuer, produces no line (G2, D10). The
  `iam.toml.example` text states this.

## 4. Design

### 4.1 Log site

The line is written in `resolve`, in the `Provisioning::Enabled` arm, in the branch where
`!self.jit.allows(&claims.issuer)` is true. Only two callers use `Provisioning::Enabled`: the HTTP
auth middleware (`src/adapters/http/auth_middleware.rs:54`) and the gRPC authentication path
(`src/adapters/grpc/authn.rs:220`). So one log site covers both transports, and `Introspect`
cannot reach it. This follows SMA-698 D4.

The branch changes from `return Err(AuthnError::IdentityNotProvisioned)` to
`return Err(self.jit_disabled(&claims.issuer))`.

The doc comments on `Provisioning` (`authenticate_token.rs:21-24`) and on `resolve`
(`:176-180`) get one sentence each that names the line.

### 4.2 The helper

A private method on `AuthenticateToken`:

```rust
fn jit_disabled(&self, issuer: &Issuer) -> AuthnError
```

It asks the rate limiter, writes the line when the limiter admits it, and returns
`AuthnError::IdentityNotProvisioned`. It receives only the issuer. It never receives the claims,
the subject, the email or the token, so the line cannot carry them. This is the same
signature-level guarantee as SMA-698 S2 (`provisioning_failed`).

### 4.3 The line

- Level: `info` (decision D2).
- Fields:
  - `issuer` — `issuer.as_str()`.
  - `reason` — the fixed string `"jit_disabled"`. It gives a stable value to filter on.
  - `suppressed` — the count from the rate limiter.
- Message, a fixed string:
  `request refused: the identity is not provisioned, and just-in-time provisioning is disabled for this issuer; IAM creates an identity only by just-in-time provisioning, so set jit_provisioning = true for the issuer to let new users of this issuer sign in`

The message starts with `request refused: the identity is not provisioned`. The integration tests
and `iam.toml.example` use this prefix to find the line. The integration test support module gets
a constant for it, `JIT_DISABLED_LINE`, next to `JIT_FAILURE_LINE`.

The message must not contain `just-in-time provisioning failed`. The SMA-698 selectors
(`JIT_LINE` in the unit tests, `JIT_FAILURE_LINE` in the integration tests) and the chart runbook
search for that text. U1 asserts that the new line does not match either selector.

### 4.4 Rate limit

A new field on `AuthenticateToken`:

```rust
not_provisioned_log: Arc<LogRateLimiter<()>>,
```

- It uses `LOG_RATE_LIMIT_INTERVAL` (10 s), the same as SMA-686 D14 and SMA-698 D2.
- The key is `(issuer, ())`, so there is one line per issuer in 10 s.
- It is an `Arc`, because `AppState` clones the use case for each request. A plain field would
  reset on each clone.
- The map is bounded. The validator returns only a configured issuer
  (`src/adapters/oidc/validator.rs:306-307`), and `JitPolicy` is built from the same config list
  (`src/adapters/http/mod.rs:771-785`). So the map holds at most one entry per configured issuer.

`LogRateLimiter<K>` requires `K: Copy + Eq + Hash` (`log_rate_limit.rs:22`). `()` satisfies this.
`log_rate_limit.rs` does not change, apart from its doc comment, which names the new user.

### 4.5 Configuration

No change. The line appears only for an issuer that an operator set to `false` in `iam.toml` or in
the environment (section 1.1).

## 5. Decisions

- D1. One log site, in the application layer (approach 1). Alternatives rejected: a log site in
  each adapter (the adapters cannot see why the result is `IdentityNotProvisioned`, and the rule
  would exist twice); a new `AuthnError::JitDisabled` variant (it changes `paigasus-iam-core` and
  both error mappers for a log line only, against SMA-698 D5).
- D2. Level `info`, not `warn`. JIT off closes the user set of an issuer (section 1.1). A refusal of
  an unknown identity is then the intended result of the operator's setting, not a defect. `info`
  stays visible at the production default level (`src/config.rs:814`). Sven chose `info` on
  2026-09-27. The first draft gave a wrong reason ("a deployment that pre-provisions its users").
  IAM cannot pre-provision an OIDC identity. Sven confirmed D2 with the corrected reason on 2026-09-27.
- D3. No counter. The line is for diagnosis. An alert on an intended refusal has no clear use. A
  later issue can add a counter if an alert becomes necessary. Sven chose this on 2026-09-27.
- D4. A separate limiter instance, keyed by `()`. The SMA-698 keys and these keys cannot collide:
  one issuer is either JIT-enabled or JIT-disabled, and the config refuses a duplicate issuer
  (`src/config.rs:1042`). The reason for the separate instance is clarity. `provisioning_log` is
  documented as keyed by the SMA-698 `defect` label, and `"jit_disabled"` is not a defect. A reuse
  of `provisioning_log` with the key `"jit_disabled"` also works and saves one field. It is
  rejected only to keep that documented meaning true.
- D5. The documentation for the line goes into `iam.toml.example`, next to `jit_provisioning`, and
  into the CHANGELOG. It does not go into `RUNBOOK-chart.md`, because a chart deployment cannot
  produce the line (section 1.1).

## 6. Tests

All log assertions use the existing capture helpers: `crate::log_capture::capture_logs()` in unit
tests and `support::capture_logs()` in integration tests. Both capture at TRACE
(`src/log_capture.rs:43`, `tests/support/mod.rs:996`). So a level change can be detected only by a
level assertion, and every positive test asserts the level `INFO`.

The fixtures use a distinctive subject, email, name, locale, zoneinfo and bearer string, as the
SMA-698 U1 fixture does (`authenticate_token.rs:1475-1490`). So a substring check cannot pass by
accident.

### 6.1 Unit tests (`src/application/authenticate_token.rs`)

The existing test `identity_not_provisioned_writes_no_line_and_no_series` (U8) stays green after
the change, because its selector `JIT_LINE` does not match the new line. It is still replaced,
because its introspect half uses a JIT-enabled issuer (`authenticate_token.rs:1622`, `:1463`) and
so cannot detect a misplaced line. Its assertions move into U1 and U2.

- U1. `resolve(.., Enabled)` with a JIT-disabled issuer and an unknown identity returns
  `IdentityNotProvisioned` and writes exactly one line with the prefix from section 4.3. The line
  has level `INFO`, `issuer`, `reason = "jit_disabled"` and `suppressed = 0`. The whole capture
  does not contain the subject, the email, the name, the locale, the zoneinfo or the bearer
  string. No captured line matches `JIT_LINE`. `iam_jit_provisioning_failures_total` has no
  increment for any `defect` (the U8 series check).
- U2. `introspect` with the SAME JIT-disabled issuer and an unknown identity returns
  `IdentityNotProvisioned` and writes no line with the prefix. The issuer must be JIT-disabled:
  with a JIT-enabled issuer, a misplaced line would not appear either. Control in the same test:
  after the introspect, `resolve(.., Enabled)` with the same token writes exactly one line. This
  proves on every run that the capture can see the line.
- U3. `resolve(.., Disabled)` with a JIT-disabled issuer writes no line. The same guard as U2, one
  level down, with the same control.
- U4. `resolve(.., Enabled)` with a JIT-disabled issuer and a KNOWN identity succeeds and writes no
  line.
- U5a. Two real refusals for one issuer write one line (use `QueueAuthenticator`, or the existing
  fixture for two tokens).
- U5b. Two `admit_at` calls injected in an old window through the private field, then one real
  refusal, write one line with `suppressed = 1`. Follow the SMA-698 test at
  `authenticate_token.rs:1753-1772`.
- U6. A refusal for a second JIT-disabled issuer, just after a refusal for the first one, writes
  its own line. Each line names its own issuer.
- U7. A cloned use case shares the limiter. Follow `a_cloned_use_case_shares_the_rate_limiter`.

### 6.2 Integration tests

- T1 (HTTP). Extend `jit_disabled_unknown_identity_is_403` (`tests/http_authn.rs:329`): exactly one
  line with `JIT_DISABLED_LINE`, level `INFO`, the issuer is present, the subject is absent, the
  email is absent. The status and the error code assertions stay.
- T2 (gRPC). Extend `who_am_i_obeys_the_jit_policy` (`tests/grpc_whoami.rs:143`) with the same
  assertions. Follow the capture set-up of
  `who_am_i_with_a_token_without_email_logs_the_provisioning_failure`
  (`tests/grpc_whoami.rs:344-370`), which already proves that the capture sees the gRPC server on
  the current-thread runtime. Also correct the stale line reference in its doc comment
  (`tests/grpc_whoami.rs:139` cites `authenticate_token.rs:107-109`; the branch is at `:189-191`).
- T3 (HTTP introspect). A new test. It uses `test_config_with(&[(&idp, false)], 30)`, so that the
  issuer is JIT-disabled. An HTTP introspect of an unknown identity returns 403 and writes no line
  with `JIT_DISABLED_LINE`. Control in the same test: a protected request with the same token then
  writes exactly one line. `introspect_unknown_identity_is_403_and_never_provisions` does not
  change, because its issuer is JIT-enabled (`tests/support/mod.rs:453-455`, `:524-526`).

### 6.3 Proof that the tests bite

Rules, from SMA-698:

- Each mutation must compile. The workspace sets `warnings = "deny"` (`rs/Cargo.toml:274-275`), so
  a mutation that leaves a binding unused keeps the binding with `let _ = (..);`.
- Run with `cargo nextest run -p paigasus-iam --no-fail-fast`, so that one failure does not hide
  the others.
- Run the Docker-backed integration tests with `PAIGASUS_REQUIRE_DOCKER=1`. Without it,
  `start_or_skip` skips them (`tests/support/docker.rs:257-279`), and they pass under every
  mutation.
- Restore each mutation by removing the inserted change, not with `git checkout --`, which would
  also revert the uncommitted feature code.
- Record each result in the plan's verification section.

Mutations:

- M1. Delete the `tracing::info!` event in `jit_disabled`, and keep the bindings with
  `let _ = (issuer, suppressed);`. U1, U2 (control), U3 (control), U5a, U5b, U6, U7, T1, T2 and T3
  (control) must fail.
- M2. In the `None =>` arm of `resolve`, before the `match provisioning`, insert
  `if !self.jit.allows(&claims.issuer) { let _ = self.jit_disabled(&claims.issuer); }`, and change
  the `Enabled` branch back to `return Err(AuthnError::IdentityNotProvisioned)`. U2, U3 and T3 must
  fail.
- M3. Pass the claims to the helper and add `subject = claims.subject.as_str()` to the line. U1,
  T1 and T2 must fail.
- M4. Always admit (ignore the result of `admit_at`, write the line with `suppressed = 0`). U5a and
  U7 must fail.
- M5. Key the limiter with `""` instead of `issuer.as_str()`. U6 must fail.
- M6. Hard-code `suppressed = 0` in the line. U5b must fail.
- M7. Change `info!` to `debug!`, and in a separate run to `warn!`. U1, T1 and T2 must fail in both
  runs.
- M8. Remove the `issuer` field from the line. U1, U6, T1 and T2 must fail.

## 7. Files

| File | Change |
|---|---|
| `rs/crates/services/paigasus-iam/src/application/authenticate_token.rs` | `jit_disabled` helper, `not_provisioned_log` field, the branch in `resolve`, two doc comments, unit tests U1-U7 (U8 replaced) |
| `rs/crates/services/paigasus-iam/src/application/log_rate_limit.rs` | Doc comment only: name the new user |
| `rs/crates/services/paigasus-iam/tests/support/mod.rs` | The constant `JIT_DISABLED_LINE` |
| `rs/crates/services/paigasus-iam/tests/http_authn.rs` | T1, T3 |
| `rs/crates/services/paigasus-iam/tests/grpc_whoami.rs` | T2, and the stale doc comment |
| `rs/crates/services/paigasus-iam/iam.toml.example` | Next to `jit_provisioning`: what `false` means (a closed user set), the line it writes, and that gateway-only users produce no line |
| `rs/crates/services/paigasus-iam/CHANGELOG.md` | One entry under `[Unreleased]` / `Added` |

`RUNBOOK-chart.md`, `RUNBOOK-observability.md`, `paigasus-observability/src/names.rs`,
`src/main.rs`, the chart and the Prometheus rules do not change.

## 8. Verification

- `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`,
  `PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam` in `rs/`.
- The mutations M1-M8 in section 6.3.
- The full `moon ci` target list from the root `CLAUDE.md` before the push.

## 9. Follow-ups

- The gateway-only case (section 3). A user who reaches IAM only through the gateway, from a
  JIT-disabled issuer, gets a silent 401. Sven decided on 2026-09-27 not to open an issue for it.

## 10. Spec challenge, 2026-09-27

Verdict: APPROVE WITH CHANGES. Folded in:

- BLOCKER: the message told the operator to "provision the user". IAM has no path for that
  (section 1.1). The message and D2 now describe the closed user set.
- MAJOR: the documentation moved from `RUNBOOK-chart.md` to `iam.toml.example` (D5), because the
  chart cannot turn JIT off.
- MAJOR: D2 had a wrong reason. It is corrected, and Sven confirmed it.
- MAJOR: the gateway path is now a stated non-goal (sections 3 and 9).
- MAJOR: mutations M4-M8 cover the rate limit, the level and the issuer field.
- MINOR: M1 and M2 now compile and are exact. The Docker and restore rules are added. The U8
  premise is corrected. U5 is split. T3 is a new test with a JIT-disabled issuer. U2, U3 and T3
  have an in-test control. U1 checks all claim values and the token. The D4 reason is corrected.
  The stale doc comment in `grpc_whoami.rs` is in T2.

Rejected: none.
