# SMA-732: does a Zitadel refresh extend the access token `exp`? (design)

- Linear: SMA-732. Related: SMA-703 (spec § 9 Q3).
- Status: draft, revised after the spec challenge, 2026-10-05.
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
  (`internal/command/oidc_session_model.go:114`). The creation date is Postgres `NOW()` of the
  transaction that pushes the NEW event (`cmd/setup/70.sql:112`). The JWT gets
  `exp = Expiration + ClockSkew` (`internal/api/oidc/token.go:119-141`) and
  `iat = nbf = time.Now() - ClockSkew` (`zitadel/oidc` v3.47.5, `pkg/oidc/token.go:107-124`).
  So the code says: a refresh sets `exp` to "refresh time + lifetime". `exp` and `iat` come from
  TWO clock reads: Postgres `NOW()` first, then Go `time.Now()` after the commit, the userinfo
  query and the Action. After truncation to whole seconds, `exp - iat` is `L` or, when the gap
  crosses a second boundary, less than `L`. It is never more than `L` when ClockSkew is 0.
- K2. Code reading. `expires_in` is `time.Until(session.Expiration)` in whole seconds
  (`internal/api/oidc/token.go:37`, `:115`). Zitadel calculates it before the JWT and the Action,
  from the same Postgres base as `exp`. So `iat + expires_in` is usually `exp - 1`, not `exp`.
- K3. Code reading. No feature flag selects a refresh path. The v1 fallback runs only for a v1
  refresh token that the v2 command cannot parse (`internal/api/oidc/token_refresh.go:33-38`).
  Each refresh issues a new refresh token (`internal/command/oidc_session.go:480-488`), and
  Zitadel refuses the old one with `OIDCS-28ubl` (`internal/command/oidc_session_model.go:147-149`).
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
- K6. Code reading. `prepareUpdateOIDCSettings` has no minimum lifetime. It refuses only a zero
  value and a body with no change (`internal/command/instance_oidc_settings.go:46-81`). The token
  path reads the lifetime from the eventstore, with no cache (`internal/command/oidc_session.go:541-560`).

## 3. Decisions

- D1. The measurement is a committed, Docker-gated e2e test, not a one-off script. It runs in CI
  and guards a Zitadel version bump. (Sven, 2026-10-05.)
- D2. The test is a new `#[tokio::test]` in `rs/crates/services/paigasus-iam/tests/zitadel_e2e.rs`
  with its own containers (approach A). The container start moves into a helper that both tests
  call. The new test uses a different instance setting, so it must not share an instance with the
  SMA-703 test. The SMA-703 test keeps every assertion unchanged. (Sven, 2026-10-05.)
- D3. The access token lifetime `L` is 10 s. (Sven, 2026-10-05.) If the assertion `iat1 < exp0`
  reds under load, the fix is a larger `L` (for example 20 s), not the removal of the assertion.
  The implementer asks Sven before that change.
- D4. Scope when the test shows that Zitadel extends `exp`: the test, a measurement doc, and notes
  on SMA-703 (design spec Q3 and measurement doc M4). No change to `paigasus-auth`, the runbook or
  the chart. The K5 gap goes into the measurement doc as an observation only. (Sven, 2026-10-05.)
- D5. The outcome is defined by value:
  - "extends": A3 and A4 pass.
  - "keeps": `exp1 == exp0` or `exp2 == exp0`.
  - "refuses after exp": the step-6 refresh returns an error.

  Only "keeps" and "refuses after exp" stop the work. The coordinator then asks Sven for the fix
  scope. No fix is made without that decision. Any other red assertion is a defect in the test.
- D6. The lifetime is set with the env var `ZITADEL_DEFAULTINSTANCE_OIDCSETTINGS_ACCESSTOKENLIFETIME=10s`
  on the new test's container (`cmd/defaults.yaml:1302`), not with the admin API. This removes
  the PUT body, the asynchronous settings projection that a GET would read, and the order
  dependency on the login. A1 on `T0` proves that Zitadel applied the value. (Spec challenge.)
- D7. The test does not check IAM. The earlier assertion A7 (IAM accepts `T2` and refuses `T0`)
  compared container token times with the host wall clock, and on Docker Desktop these are two
  different clocks. The validator unit tests already cover IAM's `exp` check
  (`src/adapters/oidc/validator.rs:706-720`). So the new test needs no IAM Postgres. (Spec
  challenge.)
- D8. The binary keeps `retries = 1` in `rs/.config/nextest.toml`. The SMA-703 test needs it for
  container-start flakes. A timing flake in the new test then shows as FLAKY, which is visible.

## 4. Design

### 4.1 The helper `start_zitadel`

Move lines 95-191 of `zitadel_e2e.rs` (Zitadel's Postgres, the TLS cert, the Zitadel container,
the readiness poll, the issuer assertion, the admin PAT, the management API readiness) into one
async helper. It takes a list of extra env vars `&[(&str, &str)]`; the SMA-703 test passes an
empty list. It returns `None` when `start_or_skip` skips, and otherwise a struct that owns the two
containers, the `reqwest::Client`, the issuer and the PAT. The containers must stay alive for the
life of the struct. IAM's own Postgres (`start_migrated_postgres`) stays in the SMA-703 test only,
because only that test needs the `db`.

Docker gating: the new test calls `start_or_skip` through the helper, so it follows the
`tests/support/docker.rs` policy. Without IAM Postgres, the Zitadel Postgres container is the first
container it starts.

### 4.2 The new test `zitadel_refresh_extends_access_token_exp`

Constants: `L = 10 s`.

1. Start the containers with `start_zitadel(&[("ZITADEL_DEFAULTINSTANCE_OIDCSETTINGS_ACCESSTOKENLIFETIME", "10s")])`.
   Run `setup_zitadel` (the same confidential WEB app, JWT access tokens, the same Action).
2. `human_login` gives `T0` (access token, refresh token `R0`, `expires_in`). Start an `Instant`.
3. Immediately refresh with `R0`. This gives `Tq` and `Rq`. This step only prints (K4: it shows
   the M4a case again). It asserts nothing about `iat` or `exp`.
4. Sleep until 7 s after step 2. Refresh with `Rq`. This gives `T1` and `R1`. Record the `Instant`
   when the response arrives as `t1`.
5. Sleep until the LATER of 13 s after step 2 and `t1 + 6 s`. `T0` has expired now. Refresh with
   `R1`. This gives `T2` and `R2`.
6. For `T0`, `Tq`, `T1` and `T2`: decode `iat`, `nbf`, `exp`, `jti` with `jwt_payload`, and print
   one line per token with `expires_in`, the `Instant` offset, and whether its refresh token is
   different from the previous one (as a boolean, never the token).

The test uses only Zitadel token times in assertions. It uses the host `Instant` only for the
waits.

Assertions:

- A1. For each token: `L - 2 <= exp - iat <= L`, and `nbf == iat`. (K1: two clock reads.)
- A2. For each token: `L - 2 <= expires_in <= L`. (K2.)
- A3. `iat1 - iat0 >= 6` and `exp1 - exp0 >= 6`.
- A4. `iat2 - iat1 >= 6` and `exp2 - exp1 >= 6`. The step-5 wait is relative to `t1`, so this is
  a lower bound under any load.
- A5. `iat1 < exp0` (refresh 1 is before the first `exp`) and `iat2 > exp0` (refresh 2 is after
  it). These two assertions prove the "before and after the first exp" part of the acceptance
  criteria in Zitadel time.
- A6. The `jti` values of `T0`, `Tq`, `T1` and `T2` are all different.
- A7. `Rq != R0`, `R1 != Rq` and `R2 != R1` (K3: rotation).
- A8. The step-5 refresh succeeds. If it fails, the panic message names outcome "refuses after
  exp" (D5) and prints the response body.

The panic messages of A3 and A4 name outcome "keeps" (D5) when `exp1 == exp0` or `exp2 == exp0`,
so a reader can tell a Zitadel result from a test defect.

### 4.3 Module doc and nextest comment

- Add one paragraph to the module doc of `zitadel_e2e.rs` for the second test: what it pins (K1-K3
  as measured facts), why it has its own instance (D2), and the D5 outcomes.
- Update the `binary(zitadel_e2e)` comment in `rs/.config/nextest.toml:73-81`. It describes one
  test. It must say that the binary now has two tests that start Zitadel in parallel.

## 5. Other outputs

- `docs/superpowers/specs/2026-10-05-sma-732-zitadel-refresh-exp-measurements.md`:
  - The date, the Zitadel tag, and the exact command (§ 7, step 1).
  - The printed lines of one local run.
  - A section that lists K1-K4 and says which of them the run confirms. The `Tq` line decides K4.
  - The mutation results of § 7.
  - The limits of the measurement: Login v1 only, a confidential web app, JWT access tokens,
    `L = 10 s`, Zitadel v4.15.3. The production default is 12 h; a deployment can use Login v2 or
    opaque tokens. The code paths for these are the same (K1), but that is code reading, not
    measurement.
  - The K5 observation (D4).
- `docs/superpowers/specs/2026-10-02-sma-703-zitadel-id-token-marker-claims-design.md`: add an
  answer under Q3 (§ 9) and change the table row "Refreshed access token keeps `iat`/`exp`" to
  refer to SMA-732. Keep the original text and add the answer below it. The answer states the
  limits above.
- `docs/superpowers/specs/2026-10-02-sma-703-zitadel-measurements.md`: add one line under the M4
  result (line 519) that refers to SMA-732. Keep the original text.

## 6. Cost and flakiness

- The new test starts two containers (Zitadel Postgres and Zitadel). The SMA-703 test starts
  three. nextest runs the two tests of the binary as parallel processes, so the second
  `start-from-init` can load the runner during the waits of the new test. The new test adds about
  13 s of waits.
- A1 and A2 have a tolerance of 2 s for the gap between the clock reads inside one request (K1,
  K2). A3 and A4 are lower bounds; a slow runner makes the waits longer, which cannot red them.
  A5's `iat1 < exp0` has a margin of about 3 s. If the step-4 refresh takes more than 3 s, it reds.
  D3 says what to do then.

## 7. Verification

1. `cargo nextest run -p paigasus-iam --test zitadel_e2e --no-capture` with Docker running, two
   times. Both tests pass, and the new test prints four token lines. nextest discards the output
   of a passing test without `--no-capture`.
2. Mutation M1: change `L` in the A1 assertion only to 13 s. A1 must red.
3. Mutation M2 ("refresh keeps exp"): give `T0`'s claims to the A3, A4 and A5 checks in place of
   `T1` and `T2`. A3, A4 and `iat2 > exp0` must red, and the messages must name "keeps".
4. Mutation M3: refresh in step 5 with `R0` instead of `R1`. Record what Zitadel returns. This is
   information for the doc (K3), not an assertion.
5. Restore each mutation by deleting the inserted change, not by `git checkout`. Re-run the whole
   test after the last restore.
6. `cargo fmt --check` and `cargo clippy -p paigasus-iam --tests -- -D warnings`.

## 8. Out of scope

- Any change to `paigasus-auth` (D4).
- An IAM check of the tokens (D7).
- A check at the 12 h default in the SMA-703 test (D2).
- The ID token lifetime and the refresh token idle and absolute expiry. The test keeps their
  defaults.
- A runbook recipe for token lifetimes.

## 9. Spec challenge changelog (2026-10-05)

Folded in: A1 as a range (BLOCKER); the step-5 wait relative to `t1`; A5 for "before and after the
first exp"; the removal of the IAM check (D7); the env var instead of the admin API (D6), which
also removes the projection race; the M2 mutation; `--no-capture`; the K2 correction; the K6
source facts; the container count and the nextest comment; the M4 line in the SMA-703 measurement
doc; the D5 outcomes by value; the rotation assertions (A7); the limits of the measurement; the
print-only `Tq` refresh for K4.

Rejected: `retries = 0` (D8: the other test needs the retry). A check at the 12 h default in the
SMA-703 test (D2 keeps that test unchanged). The `app_id` field for a `clockSkew` read: A1's upper
bound already catches a non-zero ClockSkew, so the read is not needed.
