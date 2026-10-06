# SMA-732 Zitadel refresh `exp` measurement

- Date (UTC): 2026-10-06. The runs started at 04:48 and ended at 04:52 (UTC times of the log files).
  `date -u` printed `2026-10-06 04:54` when this doc was written.
- Zitadel: `ghcr.io/zitadel/zitadel:v4.15.3`, Login v1, a confidential web app, JWT access tokens.
- Access token lifetime `L`: 10 s, set with `ZITADEL_DEFAULTINSTANCE_OIDCSETTINGS_ACCESSTOKENLIFETIME=10s`.
- Spec: `2026-10-05-sma-732-zitadel-refresh-exp-design.md`.
- Command (from `rs/`, Docker running):

  ```bash
  PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run -p paigasus-iam --test zitadel_e2e --no-capture
  ```

## Result

Outcome: "extends" (spec D5). A refresh gives an access token with `exp` = refresh time + `L`, before
and after the first `exp`.

## Run 1

```text
SMA-732 T0: iat=1791262099 nbf=1791262099 exp=1791262109 exp-iat=10 expires_in=9 jti=V2_393872156682944515-at_393872156683010051 offset_ms=0 refresh_rotated=n/a
SMA-732 Tq: iat=1791262099 nbf=1791262099 exp=1791262109 exp-iat=10 expires_in=9 jti=V2_393872156682944515-at_393872156699721731 offset_ms=7 refresh_rotated=true
SMA-732 waiting 6992 ms for refresh 1 (before exp0)
SMA-732 T1: iat=1791262106 nbf=1791262106 exp=1791262116 exp-iat=10 expires_in=9 jti=V2_393872156682944515-at_393872168477327363 offset_ms=7037 refresh_rotated=true
SMA-732 waiting 5999 ms for refresh 2 (after exp0)
SMA-732 T2: iat=1791262112 nbf=1791262112 exp=1791262122 exp-iat=10 expires_in=9 jti=V2_393872156682944515-at_393872178593988611 offset_ms=13058 refresh_rotated=true
SMA-732 outcome: "extends" (spec D5): A1-A8 pass
```

## Run 2

```text
SMA-732 T0: iat=1791262130 nbf=1791262130 exp=1791262140 exp-iat=10 expires_in=9 jti=V2_393872207702392835-at_393872207702458371 offset_ms=0 refresh_rotated=n/a
SMA-732 Tq: iat=1791262130 nbf=1791262130 exp=1791262140 exp-iat=10 expires_in=9 jti=V2_393872207702392835-at_393872207719170051 offset_ms=6 refresh_rotated=true
SMA-732 waiting 6993 ms for refresh 1 (before exp0)
SMA-732 T1: iat=1791262137 nbf=1791262137 exp=1791262147 exp-iat=10 expires_in=9 jti=V2_393872207702392835-at_393872219463286787 offset_ms=7017 refresh_rotated=true
SMA-732 waiting 5999 ms for refresh 2 (after exp0)
SMA-732 T2: iat=1791262143 nbf=1791262143 exp=1791262153 exp-iat=10 expires_in=9 jti=V2_393872207702392835-at_393872229563170819 offset_ms=13037 refresh_rotated=true
SMA-732 outcome: "extends" (spec D5): A1-A8 pass
```

Neither run was FLAKY. Both runs ended with `2 tests run: 2 passed, 0 skipped`.

## What the run confirms

| Fact | Source before | Result of this run |
|---|---|---|
| K1: a refresh sets `exp` to refresh time + `L`; `exp - iat` is `L` or less; `nbf == iat` | code reading | Confirmed. A1, A3 and A4 passed in both runs. `exp-iat=10` on all eight token lines (T0, Tq, T1, T2 in each run). `nbf` equals `iat` on all eight lines. T1 has `iat` +7 s and `exp` +7 s from T0 in both runs. T2 has `iat` +13 s and `exp` +13 s from T0 in run 1 (`1791262112` and `1791262122` against `1791262099` and `1791262109`). Run 2 shows the same (`1791262143` and `1791262153` against `1791262130` and `1791262140`). |
| K2: `expires_in` is in `L - 2 ..= L`; `iat + expires_in` is usually `exp - 1` | code reading | `expires_in=9` on all eight lines. `iat + expires_in` equals `exp - 1` on 8 of 8 lines. Run 1: T0 `1791262108` (`exp` 1791262109), Tq `1791262108` (1791262109), T1 `1791262115` (1791262116), T2 `1791262121` (1791262122). Run 2: T0 `1791262139` (1791262140), Tq `1791262139` (1791262140), T1 `1791262146` (1791262147), T2 `1791262152` (1791262153). |
| K3: each refresh issues a new refresh token, and Zitadel refuses the old one | code reading | Rotation confirmed. `refresh_rotated=true` on all six refresh lines (Tq, T1, T2 in each run). For the refusal, see M3 below. In M3, Zitadel refused the reused refresh token R0 with HTTP 400 and the message key `Errors.OIDCSession.RefreshTokenInvalid`. The `OIDCS-28ubl` code did not appear in the body, so this run did not observe that code. |
| K4: the SMA-703 M4a refresh came about 10 ms after the login, so its `iat`/`exp` looked equal | inference from `jti` values | Consistent with K4: an immediate refresh shows the same second. Tq has the same `iat` and `exp` as T0 in both runs (run 1: `iat=1791262099 exp=1791262109`; run 2: `iat=1791262130 exp=1791262140`). The `offset_ms` of Tq is 7 in run 1 and 6 in run 2. Both values are below 1000. The `jti` of Tq differs from the `jti` of T0, so Zitadel issued a new token. |

## Mutations (spec § 7)

The mutation runs use nextest `retries = 1`, so each failing log holds two attempts. The two attempts of M1 and M2 showed the same failures. The text below pastes the lines of the first attempt only.

- M1, `L` in A1 only set to 13 s: the test failed. Zitadel kept `exp-iat=10`, so A1 failed for all four tokens.

  ```text
  A1: T0: exp - iat = 10, expected 11..=13 (spec K1)
  A1: Tq: exp - iat = 10, expected 11..=13 (spec K1)
  A1: T1: exp - iat = 10, expected 11..=13 (spec K1)
  A1: T2: exp - iat = 10, expected 11..=13 (spec K1)
  SMA-732: 4 check(s) failed. No D5 outcome matched: a red check here is a defect in the test (spec D5).
  ```

- M2, T0 given to A3-A5 in place of T1 and T2: the test failed with three checks. The second attempt showed the same three failures, with a different `iat2` value in A5.

  ```text
  A3: T0 vs T0: iat +0, exp +0, expected both >= 6. Outcome "keeps" (spec D5): exp did not move.
  A4: T0 vs T0: iat +0, exp +0, expected both >= 6. Outcome "keeps" (spec D5): exp did not move.
  A5: iat2 1791262231 <= exp0 1791262241: refresh 2 came before the first exp. Outcome "keeps" (spec D5): exp did not move.
  SMA-732: 3 check(s) failed. Outcome "keeps" (spec D5): Zitadel did not extend exp. STOP: report to the coordinator; do not change L or the scope.
  ```

- M3, refresh 2 with R0 in place of R1: the test failed on both attempts with the same line.

  ```text
  A8: outcome "refuses after exp" (spec D5): refresh 2 (T2) failed (400 Bad Request): {"error":"invalid_request","error_description":"Errors.OIDCSession.RefreshTokenInvalid"}
  ```

  The "refuses after exp" label in this message comes from the mutation: R0 was already used, so the refusal is the K3 rotation check, not the `exp` check.
- After the last restore, the whole binary passed: `Summary [  26.346s] 2 tests run: 2 passed, 0 skipped` (`after-mutations.log`).

## Limits

- Login v1 only, a confidential web app, JWT access tokens, `L = 10 s`, Zitadel v4.15.3.
- The production default of `L` is 12 h. A deployment can use Login v2 or opaque access tokens.
  The code path for these is the same (K1), but that is code reading, not a measurement.
- The waits use host time; every check uses Zitadel token times only (spec § 4.2).
- `--no-capture` runs the two tests one at a time. The parallel run of Task 4 is the CI case.

## Observation: `paigasus-auth` (K5)

`ts/packages/paigasus-auth` never decodes the access token. It sets the session expiry to
`Date.now() + expires_in` (`src/core/single-flight.ts:325-331`, `src/http/routes.ts:380`). This run
shows that a refresh gives a new `expires_in` near `L`, so a refresh extends the session as
expected. No test of the package covers a refresh whose `expires_in` does not increase. The F7
floor raises a TTL below 31 s to 31 s. This is an observation only (spec D4); no change is made.
