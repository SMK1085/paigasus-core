# SMA-698: log the reason when JIT provisioning fails

- Linear: SMA-698 (Urgent, milestone "IAM Gaps")
- Crate: `rs/crates/services/paigasus-iam`
- Status: revision 2, after the spec challenge on 2026-09-26. Waiting for approval.

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

The spec challenge found a second defect on the same path (section 4.6). In Postgres,
two concurrent first logins by the same identity give the loser `EmailTaken`, so the
loser gets a false `403 provisioning-failed` today.

## 2. Goal and success criteria

An operator can see why JIT provisioning failed from the logs and the metrics alone.

- S1. A provisioning failure writes a `warn` line that names the defect kind and the
  issuer. The line is rate-limited like SMA-686 D14: the first failure for an
  (issuer, defect) pair in a 10 s window writes a line. Later failures in the window are
  counted, and the next admitted line carries that count as `suppressed`.
- S2. The line holds no email address, no `sub` claim value, no `name` claim value and
  no token material. A test asserts this.
- S3. The HTTP path and the gRPC path both produce the line.
- S4. The counter `iam_jit_provisioning_failures_total` increments once for each rejected
  request, with the defect kind as its `defect` label. The rate limit does not apply to
  the counter. One person can make several rejected requests (a console session makes
  several IAM calls), so the counter counts requests, not identities.
- S5. A lost race between two first logins of the same identity is a success for both
  requests, in Postgres too. It writes no line and does not count.

## 3. Non-goals

- No change to a response, with one exception: the lost race in S5 changes from
  `403 provisioning-failed` to success. All other status codes, error codes, messages,
  bodies and the retryable header stay as they are.
- No alert rule and no dashboard panel. A threshold needs a real baseline. This is a
  follow-up issue (section 10).
- No `issuer` label on the counter (decision D3).
- No change to `paigasus-iam-core`. `ProvisioningDefect` keeps its current shape.
- No log line for `IdentityNotProvisioned` (JIT disabled for the issuer, or
  `Introspect`). That is a follow-up issue (section 10).
- No request id in the line. IAM opens no spans, and the correlation id lives in request
  extensions (`paigasus-observability/src/correlation.rs`). The defect kind and the
  issuer are enough to act on both defects.

## 4. Design

### 4.1 Where the log goes

Both transports reach `jit_provision` through one method:

- HTTP: `src/adapters/http/auth_middleware.rs:54` calls
  `state.authn.resolve(&token, Provisioning::Enabled)`.
- gRPC: `src/adapters/grpc/authn.rs:220` calls the same method.

`jit_provision` is private, and its only caller is the `Enabled` arm of `resolve`.
`Introspect` calls `resolve(.., Provisioning::Disabled)`. `WhoAmI` uses `context_for`
and does not resolve again. API keys use `api_key_auth.resolve`. `require_bearer` and
`AuthLayer` are each attached once per stack. So one request reaches `jit_provision` at
most once, and the log line and the counter go into the application layer, inside
`jit_provision`. The adapters do not change. This gives one log site for both
transports, as the issue asks.

### 4.2 The failure type and the helper

A local enum names each failure with its detail, so an invalid combination cannot be
built:

```rust
enum EmailClaim { Absent, Invalid }

enum JitFailure {
    MissingEmail(EmailClaim),
    EmailConflict,
}
```

`JitFailure::defect()` returns the `ProvisioningDefect`. `jit_provision` sets
`EmailClaim` from its current `match`: `claims.email` is `None` gives `Absent`;
`claims.email` is `Some(s)` and `Email::parse(s)` fails gives `Invalid`.

One method on `AuthenticateToken` handles a failure:

```rust
fn provisioning_failed(&self, issuer: &Issuer, failure: JitFailure) -> AuthnError
```

- It increments the counter (4.4), asks the rate limiter (4.5) whether to write the line,
  writes the line (4.3) if admitted, and returns
  `AuthnError::ProvisioningFailed(failure.defect())`.
- It is the only way a failure arm builds its error. It takes `&self` because the rate
  limiter lives in the use case.
- It never receives the email, the subject or the name. The PII guarantee therefore
  comes from the signature. The tests confirm it.

### 4.3 The log line

One `tracing::warn!` event per admitted failure. There are two message texts, one per
`JitFailure` variant, and the same field set:

| Field | Value | Present |
|---|---|---|
| `defect` | `missing_email` or `email_conflict` | always |
| `issuer` | `issuer.as_str()` | always |
| `email_claim` | `absent` or `invalid` | only for `missing_email` |
| `suppressed` | failures suppressed for this (issuer, defect) since the last line | always |

- The `defect` label comes from one function with an exhaustive `match` on
  `ProvisioningDefect` (no wildcard arm). The log field, the counter label and the prime
  (4.4) all use it, so they cannot differ, and a new variant does not compile until
  someone gives it a label.
- The issuer field uses the validator's form, `issuer = issuer.as_str()`
  (`src/adapters/oidc/validator.rs:173`), so both lines render the issuer the same way.
- The issuer is safe to log, and its values are limited. `ValidatedClaims.issuer` is a
  clone of the CONFIGURED issuer, made after an exact byte match with the token's `iss`
  (`src/adapters/oidc/validator.rs`, the issuer lookup before the signature check). It is
  never the token's own string. So the operator chooses the value, and the configuration
  limits how many values exist.
- The line does not contain the email, the `sub` claim, the `name` claim, `locale`,
  `zoneinfo` or the token. It also does not contain an `Email::parse` error:
  `DomainError::InvalidEmail` holds the raw claim value
  (`paigasus-iam-core/src/value.rs`).
- The messages follow the style of `src/application/bootstrap_admin.rs`: one lowercase
  sentence that says what happened and what the operator can check. The plan fixes the
  exact text.
  - `missing_email`: the access token has no usable `email` claim, and the operator must
    make the issuer put a valid `email` into the ACCESS token.
  - `email_conflict`: another user already has this email, and IAM does not link
    identities by email (the "no auto-linking by email (D5)" rule in the
    `jit_provision` doc comment).

### 4.4 The counter

- Name: `iam_jit_provisioning_failures_total`. Label: `defect`, with the two values from
  4.3. No `issuer` label (D3).
- `rs/crates/libs/paigasus-observability/src/names.rs`: add the constant
  `IAM_JIT_PROVISIONING_FAILURES_TOTAL` with a doc comment, and add it to `ALL`. The doc
  comment says that the counter counts rejected requests, not identities, and that the
  rate limit does not apply to it.
- `src/main.rs`: add a `describe_counter!` next to the other IAM counters, and update the
  metric-family count in the comment near `main.rs:645`.
- Prime: `authenticate_token.rs` gets
  `pub fn prime_jit_provisioning_failures()`. It loops over a const array of every
  `ProvisioningDefect` value, uses the label function from 4.3, and calls
  `increment(0)` on each series. `main.rs` calls it in the `metrics_handle.is_some()`
  block, next to the `main.rs:105` prime. This follows the label-const-plus-prime shape
  of `src/adapters/authz/generation.rs`. The prime runs whenever metrics are on, even when
  no issuer has JIT enabled: a zero series costs nothing, and an alert then has a series
  to read.
- Without the prime, a new series starts at 1 and `increase()` does not see the first
  failure.

### 4.5 The rate limit (SMA-686 D14)

- The validator's `RefusalLog` (`src/adapters/oidc/validator.rs:57-98`) moves to one
  shared, generic limiter keyed by (issuer, K), where K is the defect type. The validator
  uses it with `TokenDefect`; `AuthenticateToken` uses it with `ProvisioningDefect`. One
  policy then lives in one place. The validator's behavior and its tests do not change.
- The module location is for the plan to choose. It must not make the application layer
  depend on `adapters`.
- Interval: 10 s, the same as `REFUSAL_LOG_INTERVAL`.
- `AuthenticateToken` holds the limiter in an `Arc`. `AppState.authn` is held by value
  and cloned for each request (`src/adapters/grpc/authn.rs`, `AuthEnforce::call`), so a
  plain field would reset on each clone. `AuthenticateToken::new` builds the limiter
  itself, so its signature and its callers do not change.
- The limiter state holds at most `issuers × 2` entries.

### 4.6 The lost race (decision D1)

In Postgres, `PgExternalIdentityRepository::provision` inserts the `user` row before the
`external_identity` row (`src/adapters/persistence/pg_external_identities.rs:76-97`), and
`user.email` is unique. Two concurrent first logins by the same identity carry the same
`email` claim. The loser's `user` insert fails first, and `conflict_kind` maps
`user_email_key` to `EmailTaken` (`src/adapters/persistence/mod.rs:78-79`). So today the
loser gets `403 provisioning-failed`.

The fix, in `jit_provision`:

- In the `EmailTaken` arm, call `find_by_issuer_subject(&claims.issuer, &claims.subject)`
  again.
- `Some(identity)`: the winner has committed. Return its `principal_id`. No line, no
  count.
- `None`: a real email conflict. Call the helper with `JitFailure::EmailConflict`.
- `Err`: `Backend`, as for the other re-read.

The loser's insert waits on the unique index until the winner commits, so the re-read
after `EmailTaken` sees the winner's row. The Docker test in 6.2 measures this and does
not assume it.

### 4.7 What does not count as a failure

- The lost race, in both forms: `Conflict(ExternalIdentityExists)` (today's re-read) and
  `Conflict(EmailTaken)` with the identity present (4.6).
- `Backend` errors from `provision` or from a re-read. These already reach the `Backend`
  log in each adapter.
- A JIT-disabled issuer or `Provisioning::Disabled`. These return
  `IdentityNotProvisioned` before `jit_provision` runs.

## 5. Data flow

```text
HTTP require_bearer ─┐
                     ├─> AuthenticateToken::resolve(token, Enabled)
gRPC AuthEnforce ────┘        │ unknown (issuer, sub), JIT allowed
                              v
                        jit_provision
                          ├─ email absent / invalid ─> provisioning_failed(MissingEmail(..)) ─┐
                          ├─ EmailTaken ─> re-read ─┬─ found ─> winner's principal (silent)   │
                          │                         └─ none ──> provisioning_failed(EmailConflict)
                          │                                                                   ├─ counter +1 (always)
                          │                                                                   ├─ warn! line (if admitted)
                          │                                                                   └─ Err(ProvisioningFailed)
                          ├─ ExternalIdentityExists ─> re-read winner (silent, as today)
                          └─ other repo error ───────> Backend (adapter logs it, as today)
```

## 6. Testing

### 6.1 Unit tests (`src/application/authenticate_token.rs`, `mod tests`)

The tests use the existing in-memory fakes in that module.

- Log capture: one `#[cfg(test)] pub(crate)` capture module in the crate, with the
  `LogBuffer` + `set_default` pattern now in `src/adapters/oidc/validator.rs`. The
  validator tests and the new tests both use it. (A test module can import a
  `#[cfg(test)]` item from another module of the same crate, as
  `src/adapters/retryable.rs` does.)
- Counter capture: `DebuggingRecorder` with `metrics::set_default_local_recorder`, as in
  `src/application/bootstrap_admin.rs`. Each count assertion reads the VALUE,
  `DebugValue::Counter(n)`, on the key with the right `defect` label. A key count is not
  enough, because a series primed with `increment(0)` also has a key.
- Line selection: a test counts only the lines with the helper's fixed message text. The
  PII checks run on the whole capture.

| # | Case | Expected |
|---|---|---|
| U1 | claims with no `email` | one `WARN` line with `missing_email`, `absent` and the issuer; `defect="missing_email"` is `Counter(1)` |
| U2 | claims with `email = "alice.distinct@example.com@x"` (fails `Email::parse`) | one `WARN` line with `missing_email` and `invalid`; the capture does not contain `alice.distinct` |
| U3 | email already used by another principal, identity absent | one `WARN` line with `email_conflict` and the issuer; `defect="email_conflict"` is `Counter(1)`; the capture does not contain the email |
| U4 | successful JIT provision | no helper line; no counter series |
| U5 | lost race, `ExternalIdentityExists` | no helper line; no counter series; the winner's principal is returned |
| U6 | lost race, `EmailTaken` with the identity present after the re-read (a fake that returns `EmailTaken` and inserts the winner's identity) | success with the winner's principal; no helper line; no counter series |
| U7 | `provision` returns `RepositoryError` other than a conflict | `Backend`; no helper line; no counter series |
| U8 | JIT-disabled issuer, and `introspect` for an unknown identity | `IdentityNotProvisioned`; no helper line; no counter series |
| U9 | three `missing_email` failures for one issuer in one window | one helper line; `Counter(3)` |
| U10 | `prime_jit_provisioning_failures()` | both `defect` series exist at `Counter(0)` |

- Every failure test also asserts that the capture does not contain the subject string
  or the `name` claim value. The fixtures use distinctive values, so a substring check
  cannot pass by accident.
- The error each failure test gets back is still `AuthnError::ProvisioningFailed(<defect>)`.
- The window and the `suppressed` count on the next admitted line are tested on the
  limiter with an injected `Instant`, as `refusal_log_counts_suppressed_refusals` does
  today. U9 does not wait 10 s.

### 6.2 Docker-backed tests

| # | File | Case |
|---|---|---|
| T1 | `tests/http_authn.rs` | a token with no email (`idp.bearer(sub, None, ..)`) on a protected route gives 403 `provisioning-failed` and one helper line with `missing_email` |
| T2 | `tests/grpc_whoami.rs` | the same token on `WhoAmI` gives `PermissionDenied` with reason `provisioning-failed` and one helper line with `missing_email` |
| T3 | `tests/authn_identities.rs` (or the suite that owns Postgres JIT) | two concurrent `resolve(.., Enabled)` calls for one new identity with one email both succeed with the same principal, and no helper line is written |

- T1 drives the router in-process with `tower::ServiceExt::oneshot`. A thread-local
  subscriber sees it on the default current-thread `#[tokio::test]` runtime.
- T2: the tests in `tests/grpc_whoami.rs` use the default current-thread runtime, so the
  spawned tonic server task runs on the test thread and should see a `set_default`
  subscriber. The plan measures this first. Install the subscriber before the request,
  and bind the guard to a named variable (`_guard`, not `_`). If the measurement fails,
  the fallback is to drive the production composition in-process:
  `AuthLayer::new(state.clone()).layer(grpc::routes(state.clone()).await)` with `oneshot`
  and a hand-built `http::Request` to `/paigasus.iam.v1.AuthnService/WhoAmI`, with
  `content-type: application/grpc` and `authorization: Bearer …`. `AuthEnforce` rejects
  the request before the inner service runs, so an empty body is enough, and
  `tonic::Status::from_header_map` reads the trailers-only response. No task is spawned.
  Do not use `WithSubscriber` on the server future or `set_global_default`.
- T1 and T2 count only the helper's lines. `AppState::new` writes its own `warn` line
  when `accept_invalid_tls` is on, and `test_config` turns it on.
- T3 records the real result. If the race does not reproduce reliably, the plan says so
  and keeps U6 as the proof of the branch logic. It must not turn T3 into a test that
  passes without a race.
- T1 and T2 assert the negative too: the capture does not contain the subject.
- The Docker suites skip when Docker is not available (see `rs/CLAUDE.md`). Run a
  filtered run with `PAIGASUS_REQUIRE_DOCKER=1` so a skip cannot pass as green.
- `tests/support/mod.rs` gets one copy of the capture helper, with `#[allow(dead_code)]`.

### 6.3 Proof that the tests bite

Each mutation must compile. The crate denies warnings (`rs/Cargo.toml`), so a mutation
that fails at `rustc` proves nothing. Use `cargo nextest run --no-fail-fast`, and restore
each mutation by removing the inserted change, not with `git checkout --`.

| Mutation | Must go red |
|---|---|
| `warn!` changed to `debug!` | U1-U3, T1, T2 |
| the event deleted, bindings kept with `let _ = (..);` | U1-U3, T1, T2 |
| the counter increment deleted | U1, U3, U9 |
| `email = ?claims.email` added at a call site's log | U1-U3 PII checks |
| the helper called in the `ExternalIdentityExists` arm | U5 |
| the `EmailTaken` re-read removed | U6 (and T3 when the race reproduces) |
| the limiter bypassed (always admit) | U9 |
| the prime loop emptied | U10 |

## 7. Files touched

| File | Change |
|---|---|
| `rs/crates/services/paigasus-iam/src/application/authenticate_token.rs` | `JitFailure`, `EmailClaim`, helper, label function, prime function, `EmailTaken` re-read, limiter field, unit tests |
| shared limiter module (location per 4.5) | generic limiter moved out of `validator.rs` |
| `rs/crates/services/paigasus-iam/src/adapters/oidc/validator.rs` | use the shared limiter and the shared capture module |
| shared test capture module (`#[cfg(test)]`) | `LogBuffer` and `capture_logs` |
| `rs/crates/libs/paigasus-observability/src/names.rs` | constant plus `ALL` entry |
| `rs/crates/services/paigasus-iam/src/main.rs` | `describe_counter!`, prime call, metric-family count |
| `rs/crates/services/paigasus-iam/tests/support/mod.rs` | capture helper copy |
| `rs/crates/services/paigasus-iam/tests/http_authn.rs` | T1 |
| `rs/crates/services/paigasus-iam/tests/grpc_whoami.rs` | T2 |
| `rs/crates/services/paigasus-iam/tests/authn_identities.rs` | T3 |
| `docs/ops/RUNBOOK-chart.md` | at the missing-`email` notes (item 2 and the bootstrap bullet): name the `warn` line and its fields |
| `docs/ops/RUNBOOK-observability.md` | a §2.2 catalog row for the counter, and `defect` in the §5 label-key allowlist |
| `rs/crates/services/paigasus-iam/CHANGELOG.md` | `### Added` (log line, counter) and `### Fixed` (lost race) |

`tracing-subscriber` and `metrics-util` are already dev-dependencies of `paigasus-iam`.
No new dependency.

## 8. Decisions

| # | Decision | Reason |
|---|---|---|
| D1 | Fix the lost race here (4.6). | Sven chose this on 2026-09-26. The spec challenge found it: without the fix, the new line reports a harmless race as an email conflict, and the follow-up alert fires on it. |
| D2 | Rate-limit the line like SMA-686 D14, with one shared limiter. The counter is not limited. | Sven chose this on 2026-09-26. An IdP user with no `email` claim reaches the line on every protected request. One policy in one place, as D15 of SMA-686 asks. |
| D3 | The counter has no `issuer` label. | Sven chose this on 2026-09-26. The log line names the issuer. The label can come later. |
| D4 | One log site, in the application layer. | Both transports reach it through `resolve(.., Enabled)`, at most once per request (4.1). |
| D5 | No change to `paigasus-iam-core`. | The label function and `JitFailure` are local, so the change stays in one crate plus the name registry. |

## 9. Gates

- `repo:observability-drift`: a name in `ALL` does not need a dashboard or rule
  reference, so the gate stays green without ops changes. No test checks the set of
  described or primed metrics.
- The full graph from the root `CLAUDE.md` runs before the push, with the bash split
  that the root `CLAUDE.md` describes for this Mac.

## 10. Follow-up issues

These issues are open in Linear:

- SMA-706: an alert rule on `increase(iam_jit_provisioning_failures_total[..]) > 0`, with a
  promtool test, and a dashboard panel.
- SMA-707: a log line for a silent `403 identity-not-provisioned` on the `Enabled` path, when
  the issuer has JIT disabled (`authenticate_token.rs:117-119`).

## 11. Spec challenge changelog (2026-09-26)

Folded in:

- MAJOR lost race → D1, 4.6, S5, U6, T3.
- MAJOR rate limit → D2, S1, 4.5, U9.
- MAJOR untested prime → prime function beside the emitter, shared label function, U10.
- MAJOR missing documents → runbook, observability runbook, CHANGELOG and metric count in 7.
- MINOR T2 capture → fallback named in 6.2.
- MINOR mutations that do not compile → 6.3 rewritten.
- MINOR a second WARN line in the capture → tests count only the helper's lines.
- MINOR wrong reason for a third `LogBuffer` copy → one shared capture module.
- MINOR `Option<EmailClaim>` allows invalid states → `JitFailure`.
- MINOR no tests for the 4.7 negative cases → U7, U8.
- MINOR counter assertions by key → assertions by value.
- MINOR wrong reason why the issuer is safe → 4.3 now cites the configured-issuer match.
- MINOR U2 fixture → an email-like value that fails the parse.
- MINOR S4 wording → counts requests, not identities.

Not folded in:

- QUESTION on a request id for `email_conflict` → a non-goal (3). IAM opens no spans.
- QUESTION on priming only when an issuer has JIT on → rejected (4.4). A zero series costs
  nothing.
