# SMA-681: send `id_token_hint` on logout

- Linear: [SMA-681](https://linear.app/smaschek/issue/SMA-681)
- Related: SMA-506 (the original design), SMA-514 (the kind E2E journeys), SMA-682 (no SSO session after a console login)
- Status: design approved in chat on 2026-09-25. This file is the written spec.

## 1. Problem

`handleLogout` in `@paigasus/auth` (`ts/packages/paigasus-auth/src/http/routes.ts`) redirects to
the IdP's `end_session_endpoint` with `client_id`, `post_logout_redirect_uri` and `state`. It sends
no `id_token_hint`. When the user has a live Keycloak SSO session, Keycloak 26.4 then shows the page
"Do you want to log out?" before it ends the session.

This is correct IdP behaviour. OpenID Connect RP-Initiated Logout 1.0, section 2, says:

> Furthermore, the OP MUST ask the End-User this question if an id_token_hint was not provided or
> if the supplied ID Token does not belong to the current OP session with the RP and/or currently
> logged in End-User.

SMA-506 did not store the raw ID token on purpose (the comment on `handleLogout`, and design doc
§ 9.5). Its argument: `client_id` plus a registered `post_logout_redirect_uri` is sufficient for
Keycloak. The SMA-506 measurement (`2026-09-09-sma-506-measurements.md:815-845`) saw no confirmation
page. The measurement in § 3 explains why: that test realm grants `offline_access` as a default
client scope, so no online SSO session existed (SMA-682), and Keycloak had no session to ask about.

Today the page does not appear on the kind stack, for the same reason (SMA-682). It becomes visible
to every user when SMA-682 restores the SSO session.

## 2. Decisions

| # | Decision | Chosen | Rejected |
|---|---|---|---|
| D1 | Store the raw ID token | Yes, as a plain field in `SessionRecord`, next to the access and refresh tokens. | Encryption at rest for this field only (no other token has it). Not storing it (keeps the confirmation page). |
| D2 | Proof of AC 1 ("no confirmation page") | Automated tests prove that the redirect carries the stored token. The measurement in § 3 is the evidence that Keycloak then shows no page. SMA-682 adds the live end-to-end check. | Changing the package e2e realm now (does part of SMA-682, tests a scope setting production does not use). Waiting for SMA-682. |
| D3 | Schema change | `idToken` is **required**, and the record changes to `version: 2`. No deployment exists, so the forced logout costs nothing. | An optional field with `version: 1`. |
| D4 | An expired stored token | Always send it. No local `exp` check. | Omit an expired token (the user would see the page after about 5 minutes without a refresh). |
| D5 | A refreshed ID token | Store it when its `iss` and `sub` match the login claims. | Ignore refreshed tokens (the stored token then ages without limit; harmless per § 3 M-d, but staler than necessary). |
| D6 | A refreshed ID token whose `iss` or `sub` does NOT match | A definitive refresh failure: delete the record, revoke the new refresh token best effort, return `null`. The user signs in again. (Spec challenge, MAJOR 1.) | Keep the old ID token and store the new access token: the record would join the login principal with an access token that the IdP issued for a different subject. |

### Why D1 is acceptable (the reversal of SMA-506)

SMA-506 called the raw ID token "a THIRD bearer credential" that widens the effect of a Redis
compromise. The added risk is small:

- Redis already holds the access token and the refresh token. Both are stronger than the ID token.
- The ID token's `aud` is the console client. A resource server does not accept it as an access
  token.
- It adds personal data to the record. `toIdTokenClaims` keeps only `iss`, `sub`, `email` and
  `name` (`oidc.ts:204-212`). The raw token also holds `preferred_username`, `given_name`,
  `family_name`, `sid` and any mapper claims.
- The one new capability is in § 3 M-h: with the ID token, a person can end the user's SSO session
  at Keycloak, with no cookies. That is a forced logout, not access.
- ADR-0017, Decision 3, lists the contents of the Redis session. It does not name the ID token, so
  this change extends that list. See § 4.7.

### Why D4 is correct

RP-Initiated Logout 1.0, section 2:

> The OP SHOULD accept ID Tokens when the RP identified by the ID Token's aud claim and/or sid claim
> has a current session or had a recent session at the OP, even when the exp time has passed.

Keycloak 26.4 does this (§ 3 M-d). An IdP that rejects an expired hint shows an error or the
confirmation page. That is the state before this change, not a new failure.

## 3. Measurement (2026-09-25)

Keycloak 26.4.7, the digest-pinned image from `ci/kind/manifests/keycloak.yaml`
(`quay.io/keycloak/keycloak:26.4@sha256:9409c59bdfb65dbffa20b11e6f18b8abb9281d480c7ca402f51ed3d5977e6007`),
`start-dev --import-realm`, run locally with `docker run`.

The realm is a scratch copy of `ci/kind/realm/paigasus-realm.json` with three changes:
`offline_access` moved from the default to the optional client scopes (so a login creates an online
SSO session), `accessTokenLifespan: 20`, and `http://localhost:9999/*` added to the client's redirect
URIs and `post.logout.redirect.uris`. No repository file changed.

The login is a real authorization-code flow with curl and one cookie jar, with scope
`openid profile email`. The token response contains `id_token` and an online `refresh_token`
(`typ: Refresh`).

| Row | Request | Status | SSO session afterwards |
|---|---|---|---|
| M-a | Control: a new `/auth` request in the same jar | 302 with a code | alive |
| M-b | Logout, no `id_token_hint` | 200, "Do you want to log out?" | alive |
| M-c | Logout, fresh hint | 302 to `post_logout_redirect_uri?state=…` | ended |
| M-d | Logout, hint expired 15 s past `exp` | 302 to `post_logout_redirect_uri?state=…` | ended |
| M-e | Refresh, then logout with the refreshed hint | 302 | ended |
| M-f | Hint from an ended session, a different session live in the cookies | 200, confirmation page | the live session is not changed |
| M-g | Hint with a mismatched `client_id` (`account`) | 400, `Invalid parameter: id_token_hint` | alive |
| M-h | Valid hint, an empty cookie jar | 302 | ended |

- M-e: the refresh response contains a new `id_token`. Its `sid` and `sub` are the same as the
  login token's.
- M-d did not find Keycloak's exact tolerance. It shows only that 15 s past `exp` is accepted.
- M-g does not apply to this package: `openid-client` sets `client_id` from the client's own
  configuration, which is the hint's `aud`.
- Not measured: an IdP other than Keycloak, and submitting the confirmation form.

### 3.1 The shipped configuration (spec challenge, MAJOR 2)

Rows M-a to M-h use an online session. The package default scope is
`openid profile email offline_access` (`src/config.ts:46`), and both e2e realms grant
`offline_access` as a default scope. So a second run used the UNCHANGED
`ci/kind/realm/paigasus-realm.json` (only the secret placeholders filled and
`http://localhost:9999/*` added), the realm default token lifespan (300 s), and the package
default scope.

The token response contains `id_token` and a refresh token with `typ: Offline`. A new `/auth`
request in the same jar right after the exchange shows the login form: no SSO session exists
(SMA-682, reproduced).

| Row | Request | Status | Result |
|---|---|---|---|
| M-i1 | Logout with the fresh hint, the same cookie jar | 302 to `post_logout_redirect_uri?state=…` | redirect, no page |
| M-i2 | New login, logout with the hint, an empty cookie jar | 302 to `post_logout_redirect_uri?state=…` | redirect, no page |
| M-i3 | After M-i1, `grant_type=refresh_token` with that login's offline refresh token | 400 `invalid_grant`, "Offline user session not found" | the hint logout also ended the offline session |
| M-i4 | New login, refresh (the response contains an `id_token`), logout with the refreshed hint | 302 | redirect, no page |

So the change does not break today's logout: with the shipped configuration, Keycloak accepts the
hint of an offline-only session. Not measured in this run: whether M-i2 also ended its offline
session, and whether the `sid` changes on an offline refresh.

## 4. Design

### 4.1 Data: `src/core/session.ts`

```ts
export interface SessionRecord {
  version: 2;
  rev: number;
  accessToken: string;
  refreshToken?: string;
  accessExpiresAt: number;
  absoluteExpiresAt: number;
  /** The raw, signed ID token JWT: the newest one the IdP issued for this session. */
  idToken: string;
  /** The DECODED claims of the LOGIN id_token. Not updated by a refresh. */
  idTokenClaims: IdTokenClaims;
  principal: ResolvedPrincipal;
}
```

- `idToken` and `idTokenClaims` can come from different tokens after a refresh. Only logout reads
  `idToken`. The principal and the display name read `idTokenClaims`. The field comments state this.
- The `version` comment keeps its operational warning: the deploy of this change logs out every
  active user. It adds two more costs. During a rolling update, old and new pods delete each
  other's records, so users can see a login loop until the rollout ends. A rollback forces a second
  logout. Both zones default to `.Chart.AppVersion` (`charts/paigasus/values.yaml`), so this lasts
  only for the rollout. No deployment exists on 2026-09-25.
- `isSessionRecord` requires `value.version === 2` and `typeof value.idToken === 'string'` with a
  length above zero. A version 1 record fails, so both store adapters treat it as absent and delete
  it. This is the existing policy, with no new code path.
- `toSessionView` does not change. The raw token never crosses to the browser.

### 4.2 OIDC adapter: `src/adapters/oidc.ts`

- `OidcTokens` gets `idToken: string`. `authorizationCodeGrant` already sets `idTokenExpected: true`
  and throws when there are no claims. It also checks that `tokens.id_token` is a string, in the
  same `wrapError('authorization_code_grant', …)` path. This check only narrows the type: with
  `idTokenExpected: true`, oauth4webapi already rejects a missing or empty `id_token`
  (`oauth4webapi/build/index.js:1480-1482`). It is unreachable, so it gets no test.
- The core port type `RefreshedTokens` in `src/core/single-flight.ts:10-14` gets
  `idToken?: string` and `idTokenClaims?: IdTokenClaims`. `IdTokenClaims` is imported from
  `ports/principal-resolver`, as `core/session.ts` does. Core must not import from `adapters/`.
  The adapter's `RefreshedTokens` in `adapters/oidc.ts` gets the same two fields.
- `refresh` sets both only when the response contains an ID token (`tokens.id_token` is a string
  and `tokens.claims()` is defined). Validation of such a token: oauth4webapi checks presence, `iss`
  against the discovered issuer, `aud`, `exp`/`iat`/`nbf` and the `sub` type
  (`oauth4webapi/build/index.js:1321-1331`, `1393-1398`). The **signature** is checked by
  openid-client's `nonRepudiation` hook (`openid-client/build/index.js:1029`), which runs because
  `oidc.ts` enables it. Nothing compares `sub` with the login token.
- The adapter never logs a token. This is the existing rule in `src/ports/logger.ts`.

### 4.3 Refresh: `src/core/single-flight.ts`

The refresh write builds `next` from `fresh`. The new `idToken` rule:

- The response has `idToken` and `idTokenClaims`, and `idTokenClaims.iss === fresh.idTokenClaims.iss`
  and `idTokenClaims.sub === fresh.idTokenClaims.sub`: `next.idToken = tokens.idToken`.
- The response has an ID token, but `iss` or `sub` differs (D6): OIDC Core § 12.2 requires the
  match, and nothing upstream checks it (§ 4.2). This is a definitive refresh failure, with the same
  shape as the `refresh_rejected` branch (`single-flight.ts:182-185`): do not write `next`, delete
  the record, revoke `tokens.refreshToken` best effort when it exists, log
  `session.refresh.id_token_mismatch` and `session.deleted` with `reason: 'id_token_mismatch'`
  (both carry `{ sid: sidTag(sid) }` only), and return `null`. The `iss` half cannot occur in
  production, because oauth4webapi requires `iss === as.issuer` on every response, and the login
  token has the same value. The check stays, because it costs nothing.
- The response has no ID token: keep `fresh.idToken`.
- `session.refreshed` gets `idTokenRotated: boolean` (true when a new ID token was stored). Then
  production shows whether refreshes deliver ID tokens, which is the reason D5 exists.
- Both new event names go into the closed `AuthEventName` union (`src/ports/logger.ts:14-26`).
- `sid` is not compared. OIDC Core § 12.2 names `iss` and `sub`, and only Keycloak is measured.

`idTokenClaims` is never changed by a refresh (§ 4.1).

### 4.4 Callback: `src/http/routes.ts` `handleCallback`

The new record sets `version: 2` and `idToken: tokens.idToken`.

### 4.5 Logout: `src/http/routes.ts` `handleLogout`

- Step 1 already reads the record to find the refresh token. The same read now also gives
  `idToken`. A failed read (`STORE_DOWN`) or no record gives no token. This does not change the
  delete-first order or the SMA-653 store-failure rules.
- Step 4 calls `buildEndSessionUrl({ postLogoutRedirectUri, state, idTokenHint })`, with
  `idTokenHint` only when a token exists.
- The fallback (AC 3): no session cookie, no record, or a failed read. The request is then the same
  as today's request (`client_id`, `post_logout_redirect_uri`, `state`).
- The 503 branch of a failed delete (SMA-653 D6) does not redirect to the IdP, so it does not change.
- `logout.completed` gets `idTokenHintSent: boolean`. It is true only when the redirect `Location`
  is an end-session URL that carries the hint. When `buildEndSessionUrl` throws
  (`endSessionRedirected: false`), it is false. The name is not `idTokenHint`, so that a redaction
  rule or a reader does not take the field for the token. It never contains the token.
- The long comment above `handleLogout` is rewritten. It must state: the SMA-506 decision and why it
  is reversed (§ 2 D1); the RP-Initiated Logout "MUST ask" rule; the § 3 measurement; and the
  residual risk (§ 5). The "NAMED RESIDUAL" about an IdP that requires `id_token_hint` becomes
  smaller, but stays: such an IdP still fails when there is no cookie, no record, or a failed read,
  and when the record is already gone (absolute expiry, or the `version: 2` deploy). The comment
  must not repeat the unmeasured Entra ID claim; § 3 measured only Keycloak.

### 4.6 Tests

`paigasus-auth` unit tests:

- `tests/core/session.test.ts`: `isSessionRecord` accepts version 2 with `idToken`. It rejects
  version 1, a missing `idToken`, a non-string `idToken`, and an empty string. Add `'idToken'` to
  the missing-field table (`:107`). The test "carries no token field under any name" (`:32-39`)
  gets an `idToken` value in `RECORD` and in its must-not-contain list.
- Tests that the compiler does NOT flag (cast through `unknown`, raw JSON, or strict event
  equality). Each needs a deliberate change, or it passes for the wrong reason:
  - `tests/core/session.test.ts:139-141`, `tests/adapters/redis-store-parse.test.ts:56-62` and
    `tests/adapters/memory-store.test.ts:22` test HOLE 2 with `{ version: 1 }` bodies. Change them
    to `version: 2`. Otherwise they fail at the version check and no longer reach the field checks.
  - `tests/adapters/redis-store-parse.test.ts:78-82` and `tests/core/session.test.ts:143-145` use
    `version: 2` as the MISMATCH value. Change them to `version: 1`. The Redis case is then the
    adapter-level proof of the D3 forced logout.
  - `tests/support/store-failure.ts:65-71` builds `OidcTokens` and needs `idToken`. Its fake
    `buildEndSessionUrl` (`:77`) ignores its argument. It must record its parameters.
  - `tests/http/store-unavailable.test.ts:217-220` (row 6, a failed read) checks
    `logout.completed` with a strict `toEqual`. Add `idTokenHintSent: false`, and assert that the
    recorded `buildEndSessionUrl` parameters have no `idTokenHint`. This is the "failed read, no
    hint" case.
- `tests/http/callback.test.ts`: the stored record has `version: 2` and the fake adapter's raw
  `idToken`.
- `tests/http/logout.test.ts`: the end-session call receives `idTokenHint` equal to the stored
  token. With no cookie, and with no record, it receives no `idTokenHint`, and the logout still
  redirects to the IdP. `logout.completed` carries the right `idTokenHintSent` value in each case,
  including `false` when `buildEndSessionUrl` throws. A D4 guard: a JWT-shaped stored token with a
  past `exp` is passed unchanged.
- `tests/core/single-flight.test.ts`: a matching refreshed token is stored, and `session.refreshed`
  has `idTokenRotated: true`. No ID token in the response keeps the old token, with
  `idTokenRotated: false`. A mismatched `sub`, and separately a mismatched `iss`, delete the
  record, revoke the new refresh token, log both events, and return `null`.
- `tests/adapters/oidc.test.ts`: `authorizationCodeGrant` returns the raw `idToken`, equal to the
  minted string. `refresh` returns `idToken` equal to the minted string and `idTokenClaims.sub`
  equal to a non-default `sub` (the fixture default is `'user-1'`, `tests/fixtures/jwks.ts:216`;
  `setNextIdToken` stays set across requests, `:171`). With no `id_token` in the response, it
  returns neither. A refreshed ID token signed with `wrongKey` rejects. A `buildEndSessionUrl` case
  with `idTokenHint` asserts that `client_id` is still appended (§ 3 M-g needs the two to match).
  The case at `:167-173` already exists. Rewrite the witness comment at `:175-182`.
- All typed fixtures that build a `SessionRecord` change to `version: 2` with an `idToken`: the
  files under `ts/packages/paigasus-auth/tests/`, and
  `ts/apps/gateway-console/tests/integration/support.ts`. The fixture in
  `ts/apps/iam-console/tests/unit/action-session.test.ts:60-74` is cast through `unknown`, already
  has `idToken`, and its store never calls `isSessionRecord`. Change its `version` for consistency
  only. It proves nothing.

End-to-end tests:

- `ts/packages/paigasus-auth/tests/e2e/logout.spec.ts`: the assertion "`id_token_hint` must be
  absent" changes to "present". The value is a three-part JWT whose decoded payload has `sub` equal
  to the subject that the fixture page shows (`principal-subject`,
  `tests/e2e/fixture-server.ts:182`; the realm does not pin a user id) and `aud` containing
  `KEYCLOAK_CLIENT_ID`. Rewrite the comment at `:24-27`.
- `ts/apps/iam-console/tests/cluster/journeys/auth-roundtrip.spec.ts` (J1, step 4): assert that the
  captured end-session request carries an `id_token_hint` that is a three-part JWT with `aud` or
  `azp` equal to `paigasus-console`. Keep the step title unchanged, because
  `ci/kind/journeys-report.mjs:42` pins it. Rewrite the D9 comment (`:24-28`). J1 has no
  confirmation click to remove (the SMA-681 comment of 2026-09-24).
- A proof in a required CI tier: the chart job that runs J1 is not a required check
  (`.github/workflows/chart.yml:57`). So the console fake IdP
  (`ts/packages/paigasus-console-core/testing/fake-idp.ts:193-205`) records `id_token_hint`, and
  the iam-console e2e R12 (`ts/apps/iam-console/tests/e2e/login.spec.ts:60-80`) asserts it. The PR
  also links a green chart run as the J1 evidence.

Stale comments to rewrite: `tests/http/logout.test.ts:16-27` and `:321-325`,
`tests/containers/single-flight-multiprocess.test.ts:78` (`version: 1`), and the two e2e comments
above.

### 4.7 Evidence and follow-up

- § 3 is the evidence for AC 1.
- A comment on SMA-682: when it restores the SSO session, its J1 SSO check must also assert that
  logout shows no confirmation page, which proves AC 1 end to end.
- ADR-0017 gets a dated note under Decision 3: the Redis session also holds the raw ID token, for
  `id_token_hint` at logout (SMA-681). This matches the SMA-511 note already on that ADR.
- The PR description states that the deploy logs out every active user (`version: 2`).

## 5. Residual risk

- The ID token is in the redirect URL. It goes into the browser history, the IdP's access log, and
  any TLS-terminating proxy log. It carries the user's email, name and username in base64url, which
  is not encryption. Per § 3 M-h, a person with the token can end the user's Keycloak SSO session.
  The token gives no access to an API. Logout by POST (RP-Initiated Logout 1.0 permits it) would
  remove this, but it needs an auto-submitting HTML form and a CSP `form-action` change. It is out
  of scope.
- An IdP with large ID tokens (many mapper claims) can exceed the request-line limit of a server or
  a proxy. Not measured.
- The ID token adds one more token and more personal data to a Redis compromise (§ 2 D1).
- An IdP that rejects an expired hint behaves as before this change (§ 2 D4). Only Keycloak is
  measured.
- An IdP that requires `id_token_hint` still fails in the no-record cases (§ 4.5).

## 6. Acceptance criteria

1. With a stored session, the end-session redirect carries `id_token_hint` equal to the newest ID
   token for that session. Proven by the unit tests and both e2e assertions. That Keycloak 26.4 then
   shows no confirmation page is proven by § 3 (M-c, M-d, M-e) and, for the shipped `offline_access`
   configuration, by § 3.1 (M-i1, M-i2, M-i4). SMA-682 adds the live check.
2. J1 has no confirmation click (none exists today). J1 asserts that the hint is sent.
3. With no cookie, no record, or a failed store read, logout still redirects to the IdP without the
   hint, and the session cookie is cleared. An expired stored token is still sent, and Keycloak
   accepts it (§ 3 M-d).
4. `SessionRecord` is `version: 2` with a required `idToken`, and `isSessionRecord` enforces both.

## 7. Out of scope

- The `offline_access` problem and the restored SSO session (SMA-682).
- Encryption at rest for any token.
- Logout by POST.
- Updating `idTokenClaims` on refresh.
