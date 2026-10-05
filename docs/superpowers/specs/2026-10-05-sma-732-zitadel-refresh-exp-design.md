# SMA-732: does a Zitadel refresh extend the access token `exp`? (design)

- Linear: SMA-732. Related: SMA-703 (spec § 9 Q3).
- Status: draft, 2026-10-05.
- Zitadel version under test: `ghcr.io/zitadel/zitadel:v4.15.3` (the pin of `zitadel_e2e.rs`).

## 1. Problem

The SMA-703 measurement (`2026-10-02-sma-703-zitadel-measurements.md`, M4a and M4b) shows a
refreshed access token with a new `jti` but the same `iat` and `exp` as the token of the login.
SMA-703 § 9 Q3 asks whether this is true in general. If it is true, a console session cannot
extend its access token by refresh. The user must then log in again after the first `exp`
(12 h by default).

## 2. What is known before the measurement

These facts come from a reading of the code and of the SMA-703 data. They are NOT a
measurement. This issue exists to replace them with a measurement.

- K1. Code reading, Zitadel `v4.15.3` (local clone of the tag). The refresh grant runs
  `ExchangeOIDCSessionRefreshAndAccessToken` (`internal/command/oidc_session.go:199-225`). It
  pushes a new `AccessTokenAdded` event with `Lifetime = AccessTokenLifetime`. The write model sets
  `AccessTokenExpiration = event.CreationDate().Add(Lifetime)`
  (`internal/command/oidc_session_model.go:114`). The creation date is the database `created_at`
  of the NEW event. The JWT gets `exp = Expiration + ClockSkew` (`internal/api/oidc/token.go:119-141`)
  and `iat = nbf = time.Now() - ClockSkew` (`zitadel/oidc` v3.47.5, `pkg/oidc/token.go:107-124`).
  So the code says: a refresh sets `exp` to "refresh time + lifetime".
- K2. Code reading. `expires_in` is `time.Until(session.Expiration)` in whole seconds
  (`internal/api/oidc/token.go:34-39`, `:115`). It agrees with the JWT `exp` when ClockSkew is 0.
- K3. Code reading. No feature flag selects a refresh path. The v1 fallback runs only for a v1
  refresh token that the v2 command cannot parse (`internal/api/oidc/token_refresh.go:33-38`).
- K4. Inference from the SMA-703 data. The `jti` ids of M1 and M4a are sonyflake ids
  (`internal/id/sonyflake.go:43-50`). They differ by exactly 2^24 = 16 777 216, and their low 24
  bits are equal. In the sonyflake layout (39 bits of time in 10 ms units, 8 bits of sequence, 16
  bits of machine id) that is one time unit: the two tokens were made about 10 ms apart. `iat` and
  `exp` have a resolution of one second, so they must look equal. The sonyflake bit layout was not
  read from the library source; the arithmetic agrees with it. This conflicts with the issue text
  ("about a minute after the login").
- K5. Code reading, `ts/packages/paigasus-auth`. The package never decodes the access token. It
  sets the session expiry to `Date.now() + expires_in` (`src/core/single-flight.ts:325-331`,
  `src/http/routes.ts:380`). The F7 floor raises a TTL below `skew + 1 s` (31 s) to 31 s. No test
  covers a refresh whose `expires_in` does not increase.

## 3. Decisions

- D1. The measurement is a committed, Docker-gated e2e test, not a one-off script. It runs in CI
  and guards a Zitadel version bump. (Sven, 2026-10-05.)
- D2. The test is a new `#[tokio::test]` in `rs/crates/services/paigasus-iam/tests/zitadel_e2e.rs`
  with its own containers (approach A). The container start moves into a helper that both tests
  call. The new test changes an instance setting, so it must not share an instance with the
  SMA-703 test. (Sven, 2026-10-05.)
- D3. The access token lifetime `L` is 10 s. (Sven, 2026-10-05.)
- D4. Scope when the test shows that Zitadel extends `exp`: the test, a measurement doc, and a note
  on SMA-703 Q3. No change to `paigasus-auth`, the runbook or the chart. The K5 gap goes into the
  measurement doc as an observation only. (Sven, 2026-10-05.)
- D5. If the test shows that Zitadel does NOT extend `exp`, the work stops. The coordinator asks
  Sven for the fix scope. No fix is made without that decision.

## 4. Design

### 4.1 The helper `start_zitadel`

Move lines 95-191 of `zitadel_e2e.rs` (Zitadel's Postgres, the TLS cert, the Zitadel container,
the readiness poll, the issuer assertion, the admin PAT, the management API readiness) into one
async helper. It returns `None` when `start_or_skip` skips, and otherwise a struct that owns the
two containers, the `reqwest::Client`, the issuer and the PAT. The containers must stay alive for
the life of the struct. The existing test calls the helper and keeps every assertion unchanged.
IAM's own Postgres (`start_migrated_postgres`) stays in each test, because the test needs the
`db`.

### 4.2 The helper `set_access_token_lifetime`

`PUT /admin/v1/settings/oidc` with the PAT and this body (durations as proto JSON strings):

```json
{ "accessTokenLifetime": "10s", "idTokenLifetime": "43200s",
  "refreshTokenIdleExpiration": "2592000s", "refreshTokenExpiration": "7776000s" }
```

The three other values are the `v4.15.3` defaults (`cmd/defaults.yaml:645-650`). The update RPC
replaces all four values, so the body must send all four. Then `GET /admin/v1/settings/oidc` and
assert that `accessTokenLifetime` is `"10s"`. If Zitadel refuses `10s`, the test fails with the
response body; the implementer then uses the smallest value that Zitadel accepts, records the
refusal text in the measurement doc, and adjusts `L` and the waits in proportion.

### 4.3 The new test `zitadel_refresh_extends_access_token_exp`

Constants: `L = 10 s`, margin `M = 2 s`.

1. Start the containers with `start_zitadel`. Run `setup_zitadel` (the same confidential WEB app,
   JWT access tokens, the same Action).
2. Read the app's OIDC config (`GET /management/v1/projects/{project}/apps/{app}`) and assert that
   `clockSkew` is absent or `"0s"`. The `exp - iat == L` assertion depends on it.
3. Set the lifetime (4.2). This must happen BEFORE the login: a lifetime applies only to tokens
   that Zitadel mints after the change.
4. `human_login` gives `T0` (access token, refresh token `R0`, `expires_in`). Start an `Instant`.
5. Sleep until `L/2 + M` (7 s) after step 4. Refresh with `R0`. This gives `T1` and `R1`.
6. Sleep until `L + M + 1` (13 s) after step 4. `T0` has expired now. Refresh with `R1` (the
   rotated token, not `R0`). This gives `T2`.
7. For `T0`, `T1` and `T2`: decode `iat`, `nbf`, `exp`, `jti` with `jwt_payload`, and print one
   line per token with `expires_in` and the `Instant` offset. The measurement doc copies these
   lines.

Assertions (all times are Zitadel token times; the host clock is used only for the waits):

- A1. For each token: `exp - iat == L` and `nbf == iat`.
- A2. For each token: `L - 2 <= expires_in <= L`.
- A3. `iat1 - iat0 >= 6` and `exp1 - exp0 >= 6` (the 7 s wait, minus one second for truncation).
- A4. `iat2 - iat1 >= 5` and `exp2 > exp0 + L`. Lower bounds only, so a slow runner cannot red
  them.
- A5. The `jti` values of the three tokens are different.
- A6. The refresh in step 6 succeeds, after the `exp` of `T0`.
- A7. IAM, with the Zitadel recipe and `leeway_secs = 0`, accepts `T2` and refuses `T0` as
  expired. The default `zitadel_config` has a leeway of 60 s (`zitadel_e2e.rs:734`); without the
  override IAM would accept `T0`. Assert the defect through `state.authn.resolve`, as the SMA-703
  test does. The implementer reads the exact `TokenDefect` variant for an expired token from
  `paigasus_iam_core` and pins that variant.

### 4.4 Module doc

Add one paragraph to the module doc of `zitadel_e2e.rs` for the second test: what it pins (K1-K2
as measured facts) and why it has its own instance (D2).

## 5. Other outputs

- `docs/superpowers/specs/2026-10-05-sma-732-zitadel-refresh-exp-measurements.md`: the date, the
  Zitadel tag, the command, and the printed lines of one local run. One section lists K1-K4 and
  says which of them the run confirms. One section holds the K5 observation (D4).
- `docs/superpowers/specs/2026-10-02-sma-703-zitadel-id-token-marker-claims-design.md`: add an
  answer under Q3 (§ 9) and change the table row "Refreshed access token keeps `iat`/`exp`" to
  refer to SMA-732. Keep the original text and add the answer below it.

## 6. Cost and flakiness

- The new test adds one Postgres and one Zitadel start (about 2-10 s each on an idle machine) and
  13 s of waits. It runs under the nextest Docker container cap and retry budget of
  `rs/.config/nextest.toml`.
- Each timing assertion is a lower bound or an exact identity inside one token. A slow runner makes
  the waits longer, which cannot red a lower bound. A2 is an upper and lower bound on a value that
  Zitadel calculates in one request, so runner load does not change it.

## 7. Verification

- `cargo nextest run -p paigasus-iam --test zitadel_e2e` with Docker running, two times: both tests
  pass, and the new test prints three token lines.
- A mutation check: change `L` in the assertion only (not in the setting) to 11 s, and confirm that
  A1 reds. Change the step-6 refresh to use `R0` instead of `R1` and record what Zitadel does (this
  is information for the doc, not an assertion).
- `cargo fmt --check` and `cargo clippy -p paigasus-iam --tests -- -D warnings`.

## 8. Out of scope

- Any change to `paigasus-auth` (D4).
- The ID token lifetime and the refresh token idle and absolute expiry. The test keeps their
  defaults.
- A runbook recipe for token lifetimes.
