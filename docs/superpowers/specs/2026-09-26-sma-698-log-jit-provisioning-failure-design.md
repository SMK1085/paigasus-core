# SMA-698: log the reason when JIT provisioning fails

- Linear: SMA-698 (Urgent, milestone "IAM Gaps")
- Crate: `rs/crates/services/paigasus-iam`
- Status: design approved in chat on 2026-09-26

## 1. Problem

On a live install on 2026-09-26, IAM answered `403 provisioning-failed` and wrote no
log line, at every log level. The cause was a Zitadel JWT access token with no `email`
claim. The only way to find the cause was to decode the session token by hand.

The silent sites:

- `src/application/authenticate_token.rs`, `jit_provision` (lines 200-233). The
  `MissingEmail` arm (line 203) and the `EmailConflict` arm (line 230) return
  `AuthnError::ProvisioningFailed(..)` and log nothing.
- `src/adapters/http/authn.rs`, `AuthnApiError::into_response` (lines 38-68). It maps
  `ProvisioningFailed(_)` to 403 and logs nothing. Only the `Backend` arm logs.
- `src/adapters/grpc/convert.rs:143` maps `ProvisioningFailed(_)` to
  `PermissionDenied` and logs nothing.

## 2. Goal and success criteria

An operator can see why JIT provisioning failed from the logs and the metrics alone.

- S1. A provisioning failure writes exactly one `warn` line. The line names the defect
  kind and the issuer.
- S2. The line holds no email address, no `sub` claim value, no `name` claim value and
  no token material. A test asserts this.
- S3. The HTTP path and the gRPC path both produce the line.
- S4. The counter `iam_jit_provisioning_failures_total` counts each failure once, with
  the defect kind as its `defect` label.

## 3. Non-goals

- No change to a response: status codes, error codes, messages, bodies and the
  retryable header stay as they are.
- No alert rule and no dashboard panel. A threshold needs a real baseline. This is a
  follow-up issue (see section 9).
- No change to `paigasus-iam-core`. `ProvisioningDefect` keeps its current shape.
- No log line for `IdentityNotProvisioned` (JIT disabled for the issuer, or
  `Introspect`). That is a policy result, not a failure.

## 4. Design

### 4.1 Where the log goes

Both transports reach `jit_provision` through one method:

- HTTP: `src/adapters/http/auth_middleware.rs:54` calls
  `state.authn.resolve(&token, Provisioning::Enabled)`.
- gRPC: `src/adapters/grpc/authn.rs:220` calls the same method.

`Introspect` calls `resolve(.., Provisioning::Disabled)` and never reaches
`jit_provision` (D10). So the log line and the counter go into the application layer,
inside `jit_provision`. The adapters do not change. This gives one log site for both
transports, as the issue asks.

### 4.2 The helper

`jit_provision` gets one private helper. Both failure arms call it, and it returns the
error the arm returned before:

```rust
fn provisioning_failed(issuer: &Issuer, defect: ProvisioningDefect, email_claim: Option<EmailClaim>) -> AuthnError
```

- It writes the log line (4.3), increments the counter (4.4), and returns
  `AuthnError::ProvisioningFailed(defect)`.
- It is a free function or an associated function. It needs no `self` state.
- The exact signature is for the plan to settle. The contract is fixed: one call per
  failure, and the call is the only way a failure arm builds its error.

`EmailClaim` is a small local enum with two values, `Absent` and `Invalid`. The
`MissingEmail` arm sets it from the current `match`:

- `claims.email` is `None` gives `Absent`.
- `claims.email` is `Some(s)` and `Email::parse(s)` fails gives `Invalid`.

The `EmailConflict` arm passes no `EmailClaim`.

### 4.3 The log line

One `tracing::warn!` event with these fields:

| Field | Value | Present |
|---|---|---|
| `defect` | `missing_email` or `email_conflict` | always |
| `issuer` | the issuer URL, `Display` form | always |
| `email_claim` | `absent` or `invalid` | only for `missing_email` |

- The `defect` label comes from a local exhaustive `match` on `ProvisioningDefect`
  (no wildcard arm). A new variant then does not compile until someone gives it a label.
  The log field and the counter label use the same function, so they cannot differ.
- The issuer is safe to log. The validator's refusal test
  (`src/adapters/oidc/validator.rs`, `refusal_logs_issuer_and_marker_only`) already
  asserts that the issuer IS logged.
- The line does not contain the email, the `sub` claim, the `name` claim, `locale`,
  `zoneinfo` or the token. The validator tests treat `sub` and the email as secrets.
- The message follows the style of `src/application/bootstrap_admin.rs`: one lowercase
  sentence that says what happened and what the operator can check. The plan fixes the
  exact text. It must name the email claim for `missing_email` (the operator must make
  the issuer put `email` into the access token) and an existing user with the same email
  for `email_conflict` (IAM does not link by email, D5).

### 4.4 The counter

- Name: `iam_jit_provisioning_failures_total`. Label: `defect`, with the two values from
  4.3.
- `rs/crates/libs/paigasus-observability/src/names.rs`: add the constant
  `IAM_JIT_PROVISIONING_FAILURES_TOTAL` with a doc comment, and add it to `ALL`.
- `src/main.rs`: add a `describe_counter!` next to the other IAM counters.
- `src/main.rs`: prime both label series at zero with `increment(0)` at start-up, as
  `main.rs:105` does for `IAM_OUTBOX_NOTIFYING_ENQUEUES_TOTAL`. Without the prime, a new
  series starts at 1 and `increase()` does not see the first failure.
- The increment happens in the helper, in the application layer. `bootstrap_admin.rs`
  and `dead_letters.rs` already emit counters from the application layer.

### 4.5 What does not count as a failure

- The lost race: `provision` returns `Conflict(ExternalIdentityExists)`, and the code
  re-reads the winner's row. That is a success. No line, no count.
- `Backend` errors from `provision` or from the re-read. These already reach the
  `Backend` log in each adapter. No line from the helper, no count.
- A JIT-disabled issuer or `Provisioning::Disabled`. These return
  `IdentityNotProvisioned` before `jit_provision` runs.

## 5. Data flow

```text
HTTP require_bearer ─┐
                     ├─> AuthenticateToken::resolve(token, Enabled)
gRPC AuthEnforce ────┘        │ unknown (issuer, sub), JIT allowed
                              v
                        jit_provision
                          ├─ email absent / invalid ─> provisioning_failed(MissingEmail) ─┐
                          ├─ EmailTaken conflict ────> provisioning_failed(EmailConflict) ┤
                          │                                                               ├─ warn! line
                          │                                                               ├─ counter +1
                          │                                                               └─ Err(ProvisioningFailed)
                          ├─ ExternalIdentityExists ─> re-read winner (success, silent)
                          └─ other repo error ───────> Backend (adapter logs it, as today)
```

## 6. Testing

### 6.1 Unit tests (`src/application/authenticate_token.rs`, `mod tests`)

The tests use the existing in-memory fakes in that module. Log capture follows the
`LogBuffer` pattern in `src/adapters/oidc/validator.rs`: a `fmt` subscriber that writes
to a shared buffer, installed with `tracing::subscriber::set_default`. The test module
cannot import a `#[cfg(test)]` helper from another module's tests, so it holds its own
copy, as the gateway crate does. Counter capture follows
`src/application/bootstrap_admin.rs`: `DebuggingRecorder` with
`metrics::set_default_local_recorder`.

| # | Case | Expected |
|---|---|---|
| U1 | claims with no `email` | one `WARN` line with `missing_email`, `absent` and the issuer; counter `defect="missing_email"` is 1 |
| U2 | claims with `email = "not-an-email"` | one `WARN` line with `missing_email` and `invalid`; the output does not contain `not-an-email` |
| U3 | email already used by another principal | one `WARN` line with `email_conflict` and the issuer; counter `defect="email_conflict"` is 1; the output does not contain the email |
| U4 | successful JIT provision | no `WARN` line from the helper; no counter series |
| U5 | lost race (`ExternalIdentityExists`) | no `WARN` line from the helper; no counter series |

- Every failure test asserts that the captured output does not contain the email string,
  the subject string or the `name` claim value. The test data uses distinctive values so
  a substring check cannot pass by accident.
- The fixtures vary the case of the email (for example `Alice@Example.COM`) and assert
  that neither the raw value nor its lowercase form appears.
- The error each test gets back is still `AuthnError::ProvisioningFailed(<defect>)`.

### 6.2 Transport tests (Docker-backed, S3)

| # | File | Case |
|---|---|---|
| T1 | `tests/http_authn.rs` | a token with no email (`idp.bearer(sub, None, ..)`) on a protected route gives 403 `provisioning-failed` and one `WARN` line with `missing_email` |
| T2 | `tests/grpc_whoami.rs` | the same token on `WhoAmI` gives `PermissionDenied` with reason `provisioning-failed` and one `WARN` line with `missing_email` |

- T1 drives the router in-process with `tower::ServiceExt::oneshot`. A thread-local
  subscriber sees it on the default current-thread `#[tokio::test]` runtime.
- T2 spawns a tonic server task. On a current-thread runtime that task runs on the test
  thread, so a thread-local subscriber should see it. This is NOT measured yet. The plan
  must measure it first. If the thread-local subscriber misses the server task, the plan
  uses a capture that covers the task and records the measurement.
- Both tests assert the negative too: the captured output does not contain the subject.
- The Docker suites skip when Docker is not available (see `rs/CLAUDE.md`). Run them with
  `PAIGASUS_REQUIRE_DOCKER=1` for a filtered run so a skip cannot pass as green.

### 6.3 Proof that the tests bite

- Delete the helper's `warn!` call. U1-U3, T1 and T2 must go red.
- Delete the helper's counter increment. U1 and U3 must go red.
- Make the helper log the email. The PII assertions must go red.
- Each mutation must compile (the crate denies warnings). A mutation that fails at
  `rustc` proves nothing. Use `cargo nextest run --no-fail-fast`, and restore each
  mutation by removing the inserted change, not with `git checkout --`.

## 7. Files touched

| File | Change |
|---|---|
| `rs/crates/services/paigasus-iam/src/application/authenticate_token.rs` | helper, `EmailClaim`, defect label, two call sites, unit tests |
| `rs/crates/libs/paigasus-observability/src/names.rs` | constant plus `ALL` entry |
| `rs/crates/services/paigasus-iam/src/main.rs` | `describe_counter!` plus zero prime |
| `rs/crates/services/paigasus-iam/tests/http_authn.rs` | T1 |
| `rs/crates/services/paigasus-iam/tests/grpc_whoami.rs` | T2 |

`tracing-subscriber` and `metrics-util` are already dev-dependencies of `paigasus-iam`.
No new dependency.

## 8. Gates

- `repo:observability-drift`: a name in `ALL` does not need a dashboard or rule
  reference, so the gate stays green without ops changes.
- The full graph from the root `CLAUDE.md` runs before the push, with the bash split
  that the root `CLAUDE.md` describes for this Mac.

## 9. Follow-up

- An alert rule on `increase(iam_jit_provisioning_failures_total[..]) > 0`, with a
  promtool test, and a dashboard panel. Open as a new Linear issue after this one merges.
