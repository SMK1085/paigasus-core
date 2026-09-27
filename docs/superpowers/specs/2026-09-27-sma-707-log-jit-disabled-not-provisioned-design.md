# SMA-707: log the silent 403 `identity-not-provisioned` when an issuer has JIT disabled

- Linear: SMA-707 (milestone "IAM Gaps")
- Related: SMA-698 (JIT provisioning failure line and counter), SMA-686 D14 (log rate-limit policy)
- Status: draft for approval

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

## 2. Goals

- G1. A protected request (HTTP or gRPC) for an unknown identity from a JIT-disabled issuer writes
  one `info` line that names the issuer.
- G2. `Introspect` for an unknown identity writes no line, also when the issuer has JIT disabled.
- G3. The line holds no subject, no email, no name, no other claim value and no token. A test
  asserts this.
- G4. The line is rate-limited by the SMA-686 D14 policy: one line per issuer in 10 s, and the next
  admitted line reports the number of suppressed events.

## 3. Non-goals

- No Prometheus counter (decision D3).
- No change to the HTTP status, the gRPC status, the error code or the error body.
- No change to `paigasus-iam-core`. `AuthnError` keeps its variants.
- No change to the SMA-698 JIT failure line or to `iam_jit_provisioning_failures_total`.
- No log line on the `Provisioning::Disabled` path.

## 4. Design

### 4.1 Log site

The line is written in `resolve`, in the `Provisioning::Enabled` arm, in the branch where
`!self.jit.allows(&claims.issuer)` is true. Only two callers use `Provisioning::Enabled`: the HTTP
auth middleware (`src/adapters/http/auth_middleware.rs:54`) and the gRPC authentication path
(`src/adapters/grpc/authn.rs:220`). So one log site covers both transports, and `Introspect`
cannot reach it. This follows SMA-698 D4.

The branch changes from `return Err(AuthnError::IdentityNotProvisioned)` to
`return Err(self.jit_disabled(&claims.issuer))`.

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
  `request refused: the identity is not provisioned, and just-in-time provisioning is disabled for this issuer; provision the user, or set jit_provisioning = true for the issuer`

The message starts with `request refused: the identity is not provisioned`. The runbook and the
integration tests use this prefix to find the line. The integration test support module gets a
constant for it, next to `JIT_FAILURE_LINE`.

### 4.4 Rate limit

A new field on `AuthenticateToken`:

```rust
not_provisioned_log: Arc<LogRateLimiter<()>>,
```

- It uses `LOG_RATE_LIMIT_INTERVAL` (10 s), the same as SMA-686 D14 and SMA-698 D2.
- The key is `(issuer, ())`, so there is one line per issuer in 10 s.
- It is a separate instance from `provisioning_log`, so its keys cannot collide with the SMA-698
  defect labels, and the doc comment of `provisioning_log` stays true.
- It is an `Arc`, because `AppState` clones the use case for each request. A plain field would
  reset on each clone.
- The map is bounded. The authenticator refuses a token from an issuer that is not configured
  before `resolve` reaches this branch, so the map holds at most one entry per configured issuer.

`LogRateLimiter<K>` requires `K: Copy + Eq + Hash`. `()` satisfies this. `log_rate_limit.rs` does
not change, apart from one line in its doc comment that names the new user.

### 4.5 Configuration

No change. `jit_provisioning` defaults to `true` (`src/config.rs:183-185`), so the line appears
only for an issuer that an operator set to `false`.

## 5. Decisions

- D1. One log site, in the application layer (approach 1). Alternatives rejected: a log site in
  each adapter (the adapters cannot see why the result is `IdentityNotProvisioned`, and the rule
  would exist twice); a new `AuthnError::JitDisabled` variant (it changes `paigasus-iam-core` and
  both error mappers for a log line only, against SMA-698 D5).
- D2. Level `info`, not `warn`. JIT off is a deliberate operator setting. In a deployment that
  pre-provisions its users, this refusal is a normal access refusal, not a defect. `info` stays
  visible at the default level. Sven chose this on 2026-09-27.
- D3. No counter. The line is for diagnosis. An alert on an expected refusal has no clear use. A
  later issue can add a counter if an alert becomes necessary. Sven chose this on 2026-09-27.
- D4. A separate limiter instance, keyed by `()` (section 4.4).

## 6. Tests

All log assertions use the existing capture helpers: `crate::log_capture::capture_logs()` in unit
tests and `support::capture_logs()` in integration tests. The fixtures use a distinctive subject
and a distinctive email, so that a substring check cannot pass by accident.

### 6.1 Unit tests (`src/application/authenticate_token.rs`)

The existing test `identity_not_provisioned_writes_no_line_and_no_series` (U8) asserts that the
JIT-disabled `Enabled` path writes no line. That assertion becomes false. Replace U8 with:

- U1. `resolve(.., Enabled)` with a JIT-disabled issuer and an unknown identity returns
  `IdentityNotProvisioned` and writes exactly one `INFO` line with the prefix from section 4.3. The
  line has `issuer`, `reason = "jit_disabled"` and `suppressed = 0`. The whole capture does not
  contain the subject or the email. `iam_jit_provisioning_failures_total` has no increment (the
  existing U8 series check stays).
- U2. `introspect` with the SAME JIT-disabled issuer and an unknown identity returns
  `IdentityNotProvisioned` and writes no line with the prefix. The issuer must be JIT-disabled: with
  a JIT-enabled issuer, a line put in the wrong place would not appear either, and the test would
  not detect the defect.
- U3. `resolve(.., Disabled)` with a JIT-disabled issuer writes no line. (The same guard as U2, one
  level down.)
- U4. `resolve(.., Enabled)` with a JIT-disabled issuer and a KNOWN identity succeeds and writes no
  line.
- U5. Two refusals for one issuer inside 10 s write one line. A refusal after the interval writes
  a second line with `suppressed = 1`. Follow the pattern of the existing SMA-698 test near line
  1753, which injects earlier instants through the private field.
- U6. A refusal for a second JIT-disabled issuer inside the same 10 s writes its own line.
- U7. A cloned use case shares the limiter. Follow `a_cloned_use_case_shares_the_rate_limiter`.

### 6.2 Integration tests

- T1 (HTTP). Extend `jit_disabled_unknown_identity_is_403` (`tests/http_authn.rs:329`): exactly one
  line with the prefix, level `INFO`, the issuer is present, the subject is absent, the email is
  absent. The status and the error code assertions stay.
- T2 (gRPC). Extend `who_am_i_obeys_the_jit_policy` (`tests/grpc_whoami.rs:143`) with the same
  assertions. Follow the capture set-up of the SMA-698 test
  `who_am_i_with_a_token_without_email_logs_the_provisioning_failure`, which already proves that the
  capture sees the gRPC server on the test thread.
- T3 (HTTP introspect). An HTTP introspect of an unknown identity from a JIT-disabled issuer returns
  403 and writes no line with the prefix. If `introspect_unknown_identity_is_403_and_never_provisions`
  (`tests/http_authn.rs:24`) uses a JIT-enabled issuer, add a new test rather than change that one.

### 6.3 Proof that the tests bite

Record each result in the plan's verification section.

- M1. Delete the log call from `jit_disabled`. U1, U5, U6, U7, T1 and T2 must fail.
- M2. Move the log call to before the `match provisioning`, so that it runs for every unknown
  identity from a JIT-disabled issuer. U2, U3 and T3 must fail.
- M3. Add `subject = claims.subject.as_str()` to the line (with the call site changed to pass the
  claims). U1, T1 and T2 must fail.

Each mutation must compile. Run with `cargo nextest run --no-fail-fast`, so that one failure does
not hide the others.

## 7. Files

| File | Change |
|---|---|
| `rs/crates/services/paigasus-iam/src/application/authenticate_token.rs` | `jit_disabled` helper, `not_provisioned_log` field, the branch in `resolve`, unit tests U1-U7 (U8 replaced) |
| `rs/crates/services/paigasus-iam/src/application/log_rate_limit.rs` | Doc comment only: name the new user |
| `rs/crates/services/paigasus-iam/tests/support/mod.rs` | A constant for the line prefix |
| `rs/crates/services/paigasus-iam/tests/http_authn.rs` | T1, T3 |
| `rs/crates/services/paigasus-iam/tests/grpc_whoami.rs` | T2 |
| `docs/ops/RUNBOOK-chart.md` | A paragraph next to the SMA-698 note (line 135): what the line means, and the two fixes |
| `rs/crates/services/paigasus-iam/CHANGELOG.md` | One entry |

`RUNBOOK-observability.md`, `paigasus-observability/src/names.rs`, `src/main.rs` and the
Prometheus rules do not change, because there is no new metric.

## 8. Verification

- `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`,
  `cargo nextest run -p paigasus-iam` in `rs/`.
- The mutations M1-M3 in section 6.3.
- The full `moon ci` target list from the root `CLAUDE.md` before the push.
