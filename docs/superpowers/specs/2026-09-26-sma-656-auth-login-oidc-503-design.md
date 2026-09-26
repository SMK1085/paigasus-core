# SMA-656: `/auth/login` and `/auth/callback` give a 503 when OIDC discovery fails

- Linear: SMA-656
- Date: 2026-09-26
- Package: `ts/packages/paigasus-auth`
- Related: SMA-506 (auth design, § 7.1, § 12), SMA-653 (store 503, § 9 follow-up A), SMA-657 (D4, D7)
- Revision 4. Two adversarial challenges are folded in (§ 9). The callback's discovery failure is in
  scope, at the issue owner's request at GATE 1 (A4, D10).

## 1. The problem

The SMA-506 design (§ 7.1) says: when OIDC discovery fails, login fails with a 503 page.

The code does not do this. `handleLogin` in `src/http/routes.ts` calls
`runtime.oidc.buildAuthorizationUrl` with no catch. The adapter runs discovery lazily inside that
call. A discovery failure goes out of `handleLogin` and out of `createAuthRoutes.handle`.
`createAuthRouteHandler` in `src/server.ts` catches only `CallbackRejected`, so it throws the error
again, and Next returns its default 500.

The callback has a second form of the same gap. `handleCallback` calls
`runtime.oidc.authorizationCodeGrant`, which also runs `getConfig()` first. Its bare `catch {}`
discards every error and maps it to `code_exchange_failed`. `src/server.ts` answers that with a
plain-text 502 and no retry link. The `login.callback_rejected { reason: 'code_exchange_failed' }`
event looks the same as a failure at the token endpoint. The callback can reach a process that has
not done discovery: the login ran on another pod, or the process restarted between the login and
the callback.

**When this path occurs.** The adapter keeps the discovered `Configuration` for the life of the
process (`configPromise ??= …` in `src/adapters/oidc.ts`). Only a failed discovery clears it. So
the 503 occurs only in a process that has not yet completed one discovery: after a restart, after a
scale-out, or when a new pod cannot reach the IdP (DNS, egress, TLS, a wrong issuer). An IdP outage
that starts AFTER the first successful discovery does not give this 503. In that case
`/auth/login` still sends the browser to the IdP, which does not answer. This spec does not change
that behavior. § 6 records it as a known limit.

## 2. Acceptance

- A1. A discovery failure in `buildAuthorizationUrl` gives a 503 with a retry affordance.
- A2. No error object and no URL from the OIDC library gets into the response or a log.
- A3. A test proves the mapping.
- A4 (added at GATE 1). A discovery failure in the callback's `authorizationCodeGrant` also gives a
  503 with a retry affordance, and not the `code_exchange_failed` 502.

## 3. Decisions

- **D1. The route recognizes a discovery failure by a code.** The adapter throws a new
  `OidcDiscoveryFailed` error with `code = 'oidc_discovery_failed'`. The route calls
  `isOidcDiscoveryFailed`, which uses `hasAuthErrorCode`. The runtime and its `oidc` client are
  shared through `globalThis` between the module copies that Next makes. The adapter builds the
  error in the `getConfig` closure of one copy, and the route catches it outside that closure,
  possibly in the other copy. SMA-657 D7 says such an error must be classified by its `code`, not by
  `instanceof`.
- **D2. Every discovery failure becomes a 503. A failure after discovery does not change.** A1
  says "a discovery failure", so the scope is every error from `getConfig`'s discovery. That
  includes configuration defects that a retry cannot fix: a wrong issuer URL (HTTP 404), an issuer
  mismatch (for example a trailing slash in `PAIGASUS_OIDC_ISSUER`), a body that is not JSON, and a
  TLS failure. The 503 is therefore NOT a promise that the fault is temporary. The `reason` field
  (D8) tells an operator which class of fault occurred. D8 states which reasons are probably a
  defect and which are probably an outage, and which reasons stay ambiguous. The page text (D4)
  does not say "did not answer", because that is false for a 404 or an issuer mismatch.
  - In login, a failure AFTER discovery, `oidc build_authorization_url failed` (the discovered
    metadata has no `authorization_endpoint`), stays a plain `Error` and a 500.
  - In the callback, a failure after discovery (the token endpoint fails or refuses the code) stays
    `code_exchange_failed` and the 502.
- **D3. The error message does not change, and the error has no `cause`.** `OidcDiscoveryFailed`'s
  message is `oidc discovery failed: <ErrorName>`, the same text that `wrapError('discovery', …)`
  makes today. It holds only the library error's `name`. The adapter does NOT set `cause` on the new
  error: the `cause` of an issuer-mismatch `ClientError` holds the expected issuer and the metadata,
  and the `cause` of a 404 `ClientError` is a `Response` with a `url`. Node prints the `cause` chain
  when it logs an error.
- **D4. The page names the identity provider, but does not say why it failed.** The retry
  affordance's `link` variant gets an optional field `service: 'session_store' |
  'identity_provider'`. A missing field means `'session_store'`, so every SMA-653 call site and its
  output stay byte-identical. For `'identity_provider'` the sentence is "The identity provider is not
  available. Try again in a few seconds." The heading stays "Sign-in is temporarily unavailable".
  The `post` variant (logout) has no `service` field, so the type cannot express a state that the
  builder ignores.
- **D5. The headers and the escaping do not change.** The discovery 503 uses the same builder, so
  it has `Retry-After: 5`, `Cache-Control: no-store`, `Referrer-Policy: no-referrer`,
  `Content-Type: text/html; charset=utf-8` and `STORE_UNAVAILABLE_CSP`. It has no `Set-Cookie`
  (SMA-653 D4). `Retry-After: 5` stays: it is advisory, and a retry runs discovery again because
  the adapter clears `configPromise` after a failure. A retry can wait up to
  `PAIGASUS_OIDC_HTTP_TIMEOUT_MS` before it gets its answer.
- **D6. The login retry target is the SMA-653 D9 target.** The route computes `presentedSid` and
  `retry` BEFORE it calls `buildAuthorizationUrl`. `readCookies` is pure and cannot throw. Both
  values read only the request and `returnTo`, and both exist before the call, so the move does not
  change them. The Fetch-Metadata 403 and the `returnTo` guard still run first. With a session
  cookie, the link goes to `returnTo`. Without one, it goes to `loginRetryHref(basePath, returnTo)`.
  The reason is the D9 reason: a link to `/auth/login` from a signed-in browser would delete a
  session that is still valid when the IdP recovers.
- **D7. One new log event, `oidc.discovery_failed`, with the fields `{ zone, stage, reason }`.**
  `stage` is a closed union, `'login' | 'callback'`, in the same shape as `store.unavailable`'s
  `stage`. A later refresh or logout stage then needs no new event name. The event does not use
  `store.unavailable`, because an operator must tell a Redis fault from an IdP fault. The event
  never carries the caught error, its message, its name or a URL. It carries no value that links a
  callback event to the `login.started` event on another pod: a shared value would be the `state`,
  which travels in the URL, and the event does not need it.
  **Meaning of the event:** "this process has no discovered configuration, and a login or a
  callback needed one". Its absence does NOT mean that the IdP is healthy (§ 1).
- **D8. A closed `reason` on the error and the event.** Today the login's discovery error escapes
  to Next, and Next writes its message to stderr. The callback's discovery error is discarded by the
  bare catch and leaves no record. After this change both routes consume the error, so the event is
  the only record. The name alone (`TypeError`, `ClientError`) does not tell a timeout from a wrong
  issuer. So `OidcDiscoveryFailed` carries a `reason` from a closed list. The adapter sets it from
  the library error. The values are fixed literals: they are not an error object and not a URL, so
  A2 stays true.

  The mapping, derived from openid-client 6.8.8 (`errorHandler` and `discovery` in
  `build/index.js`) and oauth4webapi. It is REASONED FROM THE CODE; T9 measures every row that a
  local fixture can produce. The adapter tests the rows in this order and takes the first match:

  | # | Library error | `reason` | Probably |
  |---|---|---|---|
  | 1 | `ClientError` with `code` `OAUTH_TIMEOUT`, or `OAUTH_ABORT` (defensive: the adapter passes no abort signal) | `timeout` | outage |
  | 2 | `ClientError` with `code` `OAUTH_RESPONSE_IS_NOT_CONFORM` and a `cause` that is a `Response` with `status` 500–599 | `http_server_error` | outage (for example an ingress 503 in front of a down Keycloak) |
  | 3 | `ClientError` with `code` `OAUTH_RESPONSE_IS_NOT_CONFORM`, any other status or no `Response` | `http_client_error` | defect (for example a 404 for a wrong realm) |
  | 4 | `ClientError` with `code` `OAUTH_PARSE_ERROR` whose nested `cause` has the name `TimeoutError` or `AbortError` (the body timed out after the headers) | `timeout` | outage |
  | 5 | `ClientError` with `code` `OAUTH_PARSE_ERROR` whose nested `cause` is a `TypeError` (the connection reset during the body) | `network` | outage |
  | 6 | `ClientError` with `code` `OAUTH_RESPONSE_IS_NOT_JSON`, `OAUTH_PARSE_ERROR` (other cases), `OAUTH_INVALID_RESPONSE`, or (defensive: discovery does not validate endpoints) `OAUTH_INVALID_SERVER_METADATA` or `OAUTH_MISSING_SERVER_METADATA` | `invalid_metadata` | defect |
  | 7 | `ClientError` with `code` `OAUTH_JSON_ATTRIBUTE_COMPARISON_FAILED` | `issuer_mismatch` | defect |
  | 8 | `TypeError` with its own `code` `ERR_INVALID_URL` (the metadata's `issuer` is not a URL; `new URL(as.issuer)` runs outside `errorHandler`) | `invalid_metadata` | defect |
  | 9 | `TypeError` with no own `code` (a failed `fetch`) whose `cause.code` is `ENOTFOUND` | `dns` | defect (the host does not exist) |
  | 10 | the same, `cause.code` is `CERT_HAS_EXPIRED`, `DEPTH_ZERO_SELF_SIGNED_CERT`, `SELF_SIGNED_CERT_IN_CHAIN`, `UNABLE_TO_VERIFY_LEAF_SIGNATURE`, `UNABLE_TO_GET_ISSUER_CERT_LOCALLY` or `ERR_TLS_CERT_ALTNAME_INVALID` | `tls` | defect (SMA-506 § 6.7 gives no CA-bundle setting) |
  | 11 | the same, any other `cause.code` (`ECONNREFUSED`, `ECONNRESET`, `EAI_AGAIN`, …) or none | `network` | outage, but ambiguous: an egress block also gives it |
  | 12 | anything else | `other` | unknown |

  The adapter reads only the class, `name`, `code`, `cause.code` and `cause.status`. It compares
  each string against the closed lists above and reads `status` only as a number range. An unknown
  value gives the fallback of its row, so no library value reaches the log.

  Ambiguity that stays: `network` covers an IdP that is down and an egress policy that blocks the
  pod. `http_server_error` from a misconfigured ingress is a defect too. The README states this.
- **D9. On the login path, no store call and no state change.** The discovery failure happens
  before `putTransaction` and before the session delete. The session record and the session cookie
  stay as they were. `login.started` is not logged. (The callback path is different; see D10.)
- **D10. The callback maps a discovery failure to a 503 that starts a new login.** In
  `handleCallback`, the catch around `authorizationCodeGrant` checks `isOidcDiscoveryFailed` first.
  For that error it logs `oidc.discovery_failed { zone, stage: 'callback', reason }` and returns the
  IdP 503. Every other error still gives `reject('code_exchange_failed')`, as today. The facts that
  decide this:
  - `takeTransaction` has already consumed the transaction, so a retry of the callback URL gives
    `state_unknown`. The retry must start a new login. SMA-653 row 3 has the same rule. This is the
    one state change on the callback path: the transaction is gone. No other store call runs.
  - **The no-revoke invariant.** `OidcDiscoveryFailed` comes only from `getConfig()`, and every
    adapter method awaits `getConfig()` before it sends any other request. So on this path the
    authorization code is NOT spent, and no tokens exist. No revoke is necessary (SMA-653 D10 does
    not apply). The class doc comment and the `oidc.ts` header state this invariant. A later change
    that throws `OidcDiscoveryFailed` after a token request (for example a re-discovery on a JWKS
    `kid` miss inside the grant) breaks D10 and must revoke.
  - **The retry target applies the D6 rule.** With a session cookie in the request, the link goes to
    `tx.returnTo`. Without one, it goes to `loginRetryHref(basePath, tx.returnTo)`. The reason: in a
    two-tab case, a second tab can create a new valid session after this login started, and its
    cookie arrives here (`routes.ts` keeps a delete at the callback for exactly that race). A link
    to `/auth/login` would delete that valid session. If the presented cookie is stale,
    `requireSession` sends the browser to `/auth/login` from `tx.returnTo`, so the result is the
    same. `tx.returnTo` passed `validateReturnTo` and the auth-route guard at login.
  - No `login.callback_rejected` event is logged, because the callback was not rejected. The
    `code_exchange_failed` event stays for a real token-endpoint failure, so the two are now
    different in the log.
  - No `Set-Cookie` (SMA-653 D4). The transaction cookie stays in the browser, but its transaction
    is gone. The cookie has `Max-Age=600`, so it expires by itself. The next successful callback
    also clears it, and the `MAX_OUTSTANDING_TXN_COOKIES` rotation can remove it.
  - A reload of the 503 page sends the callback URL again with the transaction cookie. That gives
    `state_unknown`, logs `login.callback_rejected { reason: 'state_unknown' }`, and sends the
    browser to `/auth/login` with no `returnTo` (`src/server.ts`). This is not a replay attack. The
    README states it.
  - `Referrer-Policy: no-referrer` matters on this row: the page is served at
    `/auth/callback?code=…&state=…`, and the code is not spent.

## 4. Design

### 4.1 `src/core/errors.ts`

Add, after `SessionStoreTimeout` and its classifier:

```ts
/** Why OIDC discovery failed (SMA-656 D8). The one list; the type derives from it. */
export const OIDC_DISCOVERY_FAILURE_REASONS = [
  'timeout', 'network', 'dns', 'tls', 'http_server_error', 'http_client_error', 'invalid_metadata', 'issuer_mismatch', 'other',
] as const;
export type OidcDiscoveryFailureReason = (typeof OIDC_DISCOVERY_FAILURE_REASONS)[number];

/**
 * OIDC discovery did not complete (SMA-656). The message holds only the library error's name, and
 * there is no `cause`. INVARIANT (D10): thrown only by the adapter's `getConfig()`, which every
 * method awaits before any other request — so when a caller sees this, no token request was sent.
 */
export class OidcDiscoveryFailed extends AuthError {
  readonly code = 'oidc_discovery_failed';
  readonly reason: OidcDiscoveryFailureReason;
  constructor(message: string, reason: OidcDiscoveryFailureReason) {
    super(message);
    this.name = 'OidcDiscoveryFailed';
    this.reason = reason;
  }
}

/** True for an OidcDiscoveryFailed from ANY copy of this module (SMA-656 D1). See `hasAuthErrorCode`. */
export function isOidcDiscoveryFailed(err: unknown): boolean { … hasAuthErrorCode(err, 'oidc_discovery_failed') … }

/** The `reason` of a caught discovery error, read defensively: a value outside the list gives 'other'. */
export function oidcDiscoveryReason(err: unknown): OidcDiscoveryFailureReason { … }
```

`name` is set explicitly for the reason `SessionStoreTimeout` records: a production bundle can
mangle `constructor.name`. The class is not exported from `src/server.ts`, for the SMA-657 D4
reason: a caller of `createOidcClient` or `AuthRuntime.oidc` CAN receive one, but no consumer needs
to CLASSIFY one.

### 4.2 `src/adapters/oidc.ts`

- Add one helper that extracts the library error's name
  (`err instanceof Error ? err.name : 'unknown_error'`). `wrapError` and the new throw both use it.
- Add and export (for tests only; not from `src/server.ts`)
  `classifyDiscoveryError(err: unknown): OidcDiscoveryFailureReason` with the D8 table. Reading
  `ClientError` with `instanceof` is safe here for the reason `classifyRefreshError` records: the
  check and the discovery call use the one module-level `client` binding. `TypeError` and
  `Response` are builtins, which both module copies share.
- In `getConfig`'s discovery `.catch`, replace `throw wrapError('discovery', err)` with
  `throw new OidcDiscoveryFailed(\`oidc discovery failed: ${name}\`, classifyDiscoveryError(err))`.
  No `cause`.

Every public method calls `getConfig()`, so every method now throws `OidcDiscoveryFailed` on a
discovery failure. The effect on each caller:

- `handleCallback`: CHANGES, by design. See D10 and § 4.6.
- `handleLogin`: CHANGES, by design. See § 4.5.
- `handleLogout` catches every error from `buildEndSessionUrl` and falls back to
  `runtime.postLogoutRedirectUri` (around line 506). No change.
- `bestEffortRevoke` (`src/http/routes.ts` around line 361) and `revokeAll`
  (`src/core/single-flight.ts` around line 60) discard every error with no log line. No change.
- The refresh path classifies only `isRefreshRejected` (`src/core/single-flight.ts` around line
  208). This error is not that, so it stays a transient failure. No change.
- `getSession` (`src/next/get-session.ts` around line 66) catches and logs `session.resolve_failed`.
  No change.
- `tests/e2e/fixture-server.ts` around line 169 prints the error name in a diagnostic line. It now
  prints `OidcDiscoveryFailed`, not `Error`. Only a diagnostic line.

The implementation plan must read each of these sites again before it changes the adapter.

### 4.3 `src/ports/logger.ts`

- Add `'oidc.discovery_failed'` to `AuthEventName`.
- Add `export type OidcDiscoveryStage = 'login' | 'callback';` next to `StoreUnavailableStage`.

`AuthEventFields` is a free record, so a bare literal would not be checked against
`OidcDiscoveryStage`. The one route helper in § 4.5 takes `stage: OidcDiscoveryStage` as a typed
parameter, and both routes call it. So both the stage and the reason have one owner each.

### 4.4 `src/http/store-unavailable.ts`

- Change `RetryAffordance` to
  `{ kind: 'link'; href: string; service?: 'session_store' | 'identity_provider' } | { kind: 'post'; action: string }`.
- For `link` with `service: 'identity_provider'`, use the D4 sentence. All other output is
  byte-identical to today.
- Update the file header: the builder serves the IdP case too, and `Retry-After: 5` has the D5
  reason for that case.
- The names `storeUnavailableResponse`, `STORE_UNAVAILABLE_CSP` and `expectStoreUnavailable` stay.
  A rename is out of scope; the header states that the names are historical.

### 4.5 `src/http/routes.ts`: the helper and `handleLogin`

Add one private helper, used by both routes:

```ts
function discoveryFailedResponse(runtime: AuthRuntime, stage: OidcDiscoveryStage, err: unknown, href: string): Response {
  runtime.logger.event('oidc.discovery_failed', { zone: runtime.zone, stage, reason: oidcDiscoveryReason(err) });
  return storeUnavailableResponse({ kind: 'link', href, service: 'identity_provider' });
}
```

In `handleLogin`:

1. Move the `presentedSid` and `retry` lines (and their D9 comment) above the
   `buildAuthorizationUrl` call. Type `retry` as the `link` variant, because it is always a link
   here.
2. Wrap only the `buildAuthorizationUrl` call. `handleCallback` already uses the same
   `let` + try/catch pattern. It needs `import type { AuthorizationRequest }` from
   `../adapters/oidc`.

   ```ts
   let authorization: AuthorizationRequest;
   try {
     authorization = await runtime.oidc.buildAuthorizationUrl({ … });
   } catch (err) {
     if (!isOidcDiscoveryFailed(err)) throw err;
     return discoveryFailedResponse(runtime, 'login', err, retry.href);
   }
   ```

3. Everything after the call stays the same.

### 4.6 `src/http/routes.ts`: `handleCallback`

Read `presentedSid` (`cookies.get(SESSION_COOKIE)`) before the exchange; the later session-fixation
delete then reuses it. Change the bare `catch` around `authorizationCodeGrant` to:

```ts
} catch (err) {
  if (isOidcDiscoveryFailed(err)) {
    const href = presentedSid !== undefined ? tx.returnTo : loginRetryHref(runtime.basePath, tx.returnTo);
    return discoveryFailedResponse(runtime, 'callback', err, href);
  }
  return reject('code_exchange_failed');
}
```

The comment above the catch states the D10 facts: the transaction is consumed, the code is not
spent (the no-revoke invariant), and the retry target follows D6.

## 5. Tests (Vitest)

### 5.0 Golden tests first (before any production change)

- T0. On the UNCHANGED code, add two tests that compare the full body with `toBe`: one for
  `{ kind: 'link', href: '/iam/auth/login' }` and one for `{ kind: 'post', action:
  '/iam/auth/logout' }`. Commit them before any change. They must pass with no edit after the
  change. This is the proof of "byte-identical" in D4. Today no test checks the session-service
  sentence.

### 5.1 Harness changes (`tests/support/store-failure.ts`)

- Add two fields to `FakeOidc`, in the same pattern as `failRevoke`: `authorizationError?: Error`
  (when set, `buildAuthorizationUrl` rejects with it) and `codeGrantError?: Error` (when set,
  `authorizationCodeGrant` rejects with it).
- Move `seedTransaction` and `callbackRequest` from `tests/http/store-unavailable.test.ts` into this
  file, so both test files use one copy.
- Make `expectStoreUnavailable` and `expectEventsClean` take the forbidden strings as a parameter,
  with today's strings as the default, so the SMA-653 rows do not change.
- Update the file header: the harness now also serves the discovery rows.
- A test error's message holds a sentinel URL:
  `https://idp.invalid/.well-known/openid-configuration?sentinel-656`.

### 5.2 Route tests: login (a new file, `tests/http/discovery-failed.test.ts`)

The header of `store-unavailable.test.ts` limits that file to store failures, so the new rows go
into a new file. The file holds the login and the callback rows.

- T1. No session cookie, `OidcDiscoveryFailed` → 503, the IdP sentence, NOT the session-service
  sentence, the link is `/iam/auth/login?returnTo=<encoded>`, all D5 headers, no `Set-Cookie`.
- T2. A session cookie is present, `OidcDiscoveryFailed` → 503, the link is `returnTo`, no
  `Set-Cookie`, and the session record is still in the store after the request.
- T3. For T1 and T2: no store method is called, exactly one event `oidc.discovery_failed` with the
  fields `{ zone: 'iam', stage: 'login', reason: <the error's reason> }`, and no `login.started`.
- T4. For T1 and T2: neither `idp.invalid` nor `sentinel-656` gets into the body, a header or a
  logged field.
- T5. A plain `Error` (the `build_authorization_url` case) propagates out of `handle()` (the
  promise rejects), and no event is logged.
- T6. Classification by code: (a) an `OidcDiscoveryFailed` from a SECOND module copy
  (`vi.resetModules()`, the SMA-653 and SMA-657 pattern) gives the 503; (b) a plain `Error` with
  `code: 'oidc_discovery_failed'` and no `reason` gives the 503 with `reason: 'other'`.
- T7. A `reason` outside the closed list (for example `'https://idp.invalid/x'`) is logged as
  `'other'`.

### 5.3 Route tests: callback (the same file)

Each row seeds a transaction and sends its cookie, so the callback reaches the code exchange.

- T12. No session cookie, `authorizationCodeGrant` rejects with `OidcDiscoveryFailed` → 503, the
  IdP sentence, the link is `/iam/auth/login?returnTo=<encoded tx.returnTo>`, all D5 headers, no
  `Set-Cookie`. It does NOT throw `CallbackRejected`.
- T12b. The same with a session cookie → the link is `tx.returnTo`, and the session record is still
  in the store.
- T13. For T12 and T12b: exactly one event, `oidc.discovery_failed { zone: 'iam', stage:
  'callback', reason }`; no `login.callback_rejected`; no `session.created`; no store call after
  `takeTransaction`; neither sentinel string in the body, a header or a logged field.
- T14. A plain `Error` from `authorizationCodeGrant` still gives `CallbackRejected` with reason
  `code_exchange_failed` and the `login.callback_rejected` event, as today.
- T15. An `OidcDiscoveryFailed` from a second module copy gives the T12 503.

(T13 does not check "no revoke call": the fake gives no tokens on this path, so that check could
not fail. The no-revoke invariant is a property of the adapter; D10 and the class doc state it.)

### 5.4 Builder tests

- T8. `storeUnavailableResponse({ kind: 'link', href, service: 'identity_provider' })` holds the IdP
  sentence and not the session-service sentence, with the same headers as T0.

### 5.5 Adapter and error tests

Add a small fixture module, `tests/fixtures/discovery-failures.ts`, next to `startOidcFixture`
(which has no hook for these cases). It starts a local HTTP server in one of these modes:
`status-404`, `status-503`, `not-json`, `wrong-issuer`, `issuer-not-a-url`, `hang`.

- T9. `tests/adapters/oidc.test.ts`, one row per mode, measured through the real adapter:

  | Fixture | Expected `reason` |
  |---|---|
  | unreachable issuer `http://127.0.0.1:1` | `network` |
  | `status-404` | `http_client_error` |
  | `status-503` | `http_server_error` |
  | `not-json` | `invalid_metadata` |
  | `wrong-issuer` | `issuer_mismatch` |
  | `issuer-not-a-url` | `invalid_metadata` |
  | `hang` with `httpTimeoutMs: 200` | `timeout` |

  For each row: `isOidcDiscoveryFailed` is true, `name` is `OidcDiscoveryFailed`, the message
  matches `/^oidc discovery failed: \w+$/`, `cause` is `undefined`, and the message does not contain
  `127.0.0.1`. One more row calls `authorizationCodeGrant` (not `buildAuthorizationUrl`) against the
  unreachable issuer and expects the same class. If a measured reason differs from the table, the
  implementer stops and reports; the D8 table is then corrected, not the test.
- T9b. A unit test of `classifyDiscoveryError` with constructed errors for the rows that a fixture
  cannot produce cheaply: `dns` (`TypeError` with `cause.code: 'ENOTFOUND'`), `tls` (each listed TLS
  code), rows 4 and 5 (`OAUTH_PARSE_ERROR` with a nested timeout or `TypeError`), `OAUTH_ABORT`, the
  two defensive metadata codes, an unknown `ClientError` code, and a non-`Error` value (→ `other`).
- T10. `isOidcDiscoveryFailed` is true for an `OidcDiscoveryFailed`, true for one from a second
  module copy, true for a plain `Error` with the code, false for a plain object with the code, and
  false for a plain `Error`. `oidcDiscoveryReason` returns each listed reason unchanged and `'other'`
  for a missing or an unknown value.

### 5.6 Through the Next boundary (`tests/http/route-handler.test.ts`)

The file's `beforeEach` runtime uses the healthy fixture. T11 and T16 build their own runtime with
`createOidcClient({ issuer: 'http://127.0.0.1:1', allowInsecureRequests: true, … })`.

- T11. A GET to `/auth/login` through `createAuthRouteHandler` gives a 503, checked with
  `expectStoreUnavailable` (target and headers). This covers the place where the defect occurs
  today.
- T16. Seed the store directly: `runtime.store.putTransaction(id, { …, secretHash:
  hashSecret(secret) }, ttl)`, and send the cookie `${txnCookieName(id)}=${secret}`. (A login first
  cannot seed it: on the unreachable issuer the login gives a 503.) A GET to
  `/auth/callback?code=x&state=<id>` through `createAuthRouteHandler` gives the 503, checked with
  `expectStoreUnavailable`, and not the `login failed` 502.

### 5.7 Proof that the tests bite

Run each mutation, record the result in the PR, and restore by an edit (not `git checkout`):

| Mutation | Must fail |
|---|---|
| Delete the catch in `handleLogin` | T1–T4, T6, T7, T11 |
| `handleLogin` catches every error (remove the `isOidcDiscoveryFailed` guard) | T5 |
| Delete the `isOidcDiscoveryFailed` branch in `handleCallback` | T12, T12b, T13, T15, T16 |
| The callback branch catches every error | T14 |
| The callback branch also logs `login.callback_rejected`, then returns the 503 | T13 |
| The callback link ignores the session cookie (always `loginRetryHref`) | T12b |
| `isOidcDiscoveryFailed` always returns `false` | T1, T2, T6, T9, T10, T11, T12, T15, T16 |
| `oidcDiscoveryReason` returns `err.reason` without the list check | T7, T10 |
| `discoveryFailedResponse` puts `String(err)` into the event fields | T4, T13 |
| Change the default `service` to `'identity_provider'` | T0 |
| Adapter goes back to `wrapError('discovery', …)` | T9, T11, T16 |
| Adapter sets `cause: err` | T9 |
| `classifyDiscoveryError` always returns `'other'` | T9, T9b |
| `classifyDiscoveryError` ignores the status range (always `http_client_error`) | T9 (`status-503`), T9b |

## 6. Known limits

- An IdP outage after the first successful discovery does not give this 503 (§ 1). `/auth/login`
  then redirects to an IdP that does not answer. The absence of `oidc.discovery_failed` does not
  mean that the IdP is healthy.
- A configuration defect in discovery gives a 503 with a retry link that cannot work (D2). The
  `reason` field shows the class of the fault. `network` and `http_server_error` stay ambiguous
  (D8).
- One pod that cannot discover, behind a round-robin balancer, fails the callbacks of logins that
  other pods started. Each retry then costs a full sign-in at the IdP. A readiness gate or an eager
  discovery at start would prevent this. It is not in this issue (see the gate questions).
- A reload of the callback 503 page logs a `state_unknown` rejection (D10).

## 7. Documentation changes

- `src/http/routes.ts` header (around lines 27–31): it says the store failure is THE exception and
  that every other error propagates. Add the discovery failure as the second exception.
- `src/http/routes.ts`, the comment at the `authorizationCodeGrant` catch (D10).
- `src/server.ts` (around lines 85–86): it names only the store 503 as the response that passes
  through. Add the discovery 503. The comment on the `code_exchange_failed` 502 must say that a
  discovery failure no longer reaches it.
- `src/adapters/oidc.ts` header (around lines 18–20): it says every method rethrows through
  `wrapError`. Add that a discovery failure is an `OidcDiscoveryFailed` with a closed `reason`, and
  state the D10 no-revoke invariant.
- `src/core/errors.ts` (around lines 57–59): "the three remaining sites" that test a non-builtin
  class become four, with `classifyDiscoveryError`. Name it and say which side of the D7 line it is
  on.
- `src/http/store-unavailable.ts` header: the IdP case and its `Retry-After` reason (§ 4.4).
- `tests/support/store-failure.ts` header (§ 5.1).
- `README.md` (around lines 123–142): describe the discovery 503 on both routes, the `reason`
  values with the "probably" column and the ambiguity of D8, the `oidc.discovery_failed` event, the
  reload behavior (D10), and the § 6 limits. Correct line 132: the IdP link carries `?returnTo=`.
  Extend the ingress and mesh warning (SMA-653 § 8) to this 503.
- SMA-506 design: add one line to § 7.1 that points to this spec (for the discovery row and for
  the token-exchange row, which now excludes a discovery failure), and add `oidc.discovery_failed`
  to the § 12 event list, as SMA-626 and SMA-653 did.

## 8. Out of scope

- A discovery failure in the refresh path (it stays a transient failure) and in logout (it already
  falls back).
- A readiness gate or an eager discovery at start (§ 6).
- A rename of `storeUnavailableResponse`, `STORE_UNAVAILABLE_CSP` or `expectStoreUnavailable`.
- A change to the heading or to the logout page text.
- A Playwright e2e test with an IdP that is down. T11 and T16 cover the Next boundary.

## 9. Challenge changelog

### Challenge 1 (revision 1 → 2)

- T7 (old) claimed a byte-identical proof that no test gave → T0 golden tests first.
- D2 did not match how discovery fails → every discovery failure gives a 503; the class is
  `OidcDiscoveryFailed`; the page no longer says "did not answer".
- The change removed the only diagnostic data → D8, a closed `reason`.
- The 503 occurs only before the first discovery succeeds → § 1, D7, § 6.
- Comments and the README become false → § 7.
- The mutation table was wrong for T5 and had no redaction row → § 5.7.
- T4 reused sentinels that the IdP error does not contain → parameterized helpers.
- The harness option could not reach `harness()` → `FakeOidc` fields, a new test file.
- No second-module-copy row → T6(a), T10.
- No test of the "no `cause`" rule → T9.
- The caller list was incomplete → § 4.2.
- The reason for not exporting the class was wrong → SMA-657 D4 reason.
- The `cause` parameter allowed an ignored state → `service` on the `link` variant only.
- No test through the real adapter and the Next boundary → T11.
- The event shape → `oidc.discovery_failed { zone, stage, reason }`.

### GATE 1 (the issue owner's decision)

- The callback's discovery failure is in scope → A4, D10, § 4.6, T12–T16.

### Challenge 2 (revision 3 → 4)

- Login-only text contradicted D10 (§ 4.2, D7, D8, D9) → corrected.
- The reasons did not separate a defect from an outage → `http_status` split into
  `http_server_error` and `http_client_error`; `network` split into `dns`, `tls` and `network`;
  the ambiguity that stays is stated in D8 and § 6.
- Three paths were in the wrong bucket → D8 rows 4, 5 and 8; the defensive rows are marked.
- The two-tab case → the callback link applies the D6 rule (T12b).
- Mutation table errors → § 5.7 rewritten.
- The closed lists had no single owner → `OIDC_DISCOVERY_FAILURE_REASONS`, `oidcDiscoveryReason`,
  and one route helper with a typed `stage`.
- The transaction cookie expiry and the reload behavior → D10, README.
- Harness details (shared helpers, T16 seeding, fixture modes, a cheap timeout fixture) → § 5.1,
  § 5.5, § 5.6.
- The no-revoke invariant was not stated → D10, the class doc, the `oidc.ts` header.
- More text that becomes false → § 7.
- Correlation between pods → D7: no correlation value, on purpose.

### Not changed

- `Retry-After: 5`. It is advisory, and a retry runs discovery again. A different number needs a
  measurement that this issue does not have.
