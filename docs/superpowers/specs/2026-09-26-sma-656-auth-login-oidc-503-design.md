# SMA-656: `/auth/login` gives a 503, not a 500, when OIDC discovery fails

- Linear: SMA-656
- Date: 2026-09-26
- Package: `ts/packages/paigasus-auth`
- Related: SMA-506 (auth design, § 7.1, § 12), SMA-653 (store 503, § 9 follow-up A), SMA-657 (D4, D7)
- Revision 2: the adversarial challenge findings are in (see § 9).

## 1. The problem

The SMA-506 design (§ 7.1) says: when OIDC discovery fails, login fails with a 503 page.

The code does not do this. `handleLogin` in `src/http/routes.ts` calls
`runtime.oidc.buildAuthorizationUrl` with no catch. The adapter runs discovery lazily inside that
call. A discovery failure goes out of `handleLogin` and out of `createAuthRoutes.handle`.
`createAuthRouteHandler` in `src/server.ts` catches only `CallbackRejected`, so it throws the error
again, and Next returns its default 500.

**When this path occurs.** The adapter keeps the discovered `Configuration` for the life of the
process (`configPromise ??= …` in `src/adapters/oidc.ts`). Only a failed discovery clears it. So
the 503 occurs only in a process that has not yet completed one discovery: after a restart, after a
scale-out, or when a new pod cannot reach the IdP (DNS, egress, TLS, a wrong issuer). An IdP outage
that starts AFTER the first successful discovery does not give this 503. In that case
`/auth/login` still sends the browser to the IdP, which does not answer. This spec does not change
that behavior. § 6 records it as a known limit.

## 2. Acceptance (from the issue)

- A1. A discovery failure in `buildAuthorizationUrl` gives a 503 with a retry affordance.
- A2. No error object and no URL from the OIDC library gets into the response or a log.
- A3. A test proves the mapping.

## 3. Decisions

- **D1. The route recognizes a discovery failure by a code.** The adapter throws a new
  `OidcDiscoveryFailed` error with `code = 'oidc_discovery_failed'`. The route calls
  `isOidcDiscoveryFailed`, which uses `hasAuthErrorCode`. The runtime and its `oidc` client are
  shared through `globalThis` between the module copies that Next makes. The adapter builds the
  error in the `getConfig` closure of one copy, and the route catches it outside that closure,
  possibly in the other copy. SMA-657 D7 says such an error must be classified by its `code`, not by
  `instanceof`.
- **D2. Every discovery failure becomes a 503. A failure after discovery stays a 500.** A1 says
  "a discovery failure", so the scope is every error from `getConfig`'s discovery. That includes
  failures that are configuration defects and that a retry cannot fix: a wrong issuer URL (HTTP
  404), an issuer mismatch (for example a trailing slash in `PAIGASUS_OIDC_ISSUER`), a body that is
  not JSON, and a TLS failure. The 503 is therefore NOT a promise that the fault is temporary. The
  `reason` field (D8) is how an operator tells a defect from an outage. The page text (D4) does not
  say "did not answer", because that is false for a 404 or an issuer mismatch.
  A failure AFTER discovery, `oidc build_authorization_url failed` (the discovered metadata has no
  `authorization_endpoint`), stays a plain `Error` and a 500.
- **D3. The error message does not change.** `OidcDiscoveryFailed`'s message is
  `oidc discovery failed: <ErrorName>`, the same text that `wrapError('discovery', …)` makes today.
  It holds only the library error's `name`. The adapter does NOT set `cause` on the new error: the
  `cause` of an issuer-mismatch `ClientError` holds the expected issuer and the metadata, and the
  `cause` of a 404 `ClientError` is a `Response` with a `url`. Node prints the `cause` chain when it
  logs an error.
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
- **D6. The retry target is the SMA-653 D9 target.** The route computes `presentedSid` and `retry`
  BEFORE it calls `buildAuthorizationUrl`. `readCookies` is pure and cannot throw. Both values read
  only the request and `returnTo`, and both exist before the call, so the move does not change
  them. The Fetch-Metadata 403 and the `returnTo` guard still run first. With a session cookie, the
  link goes to `returnTo`. Without one, it goes to `loginRetryHref(basePath, returnTo)`. The reason
  is the D9 reason: a link to `/auth/login` from a signed-in browser would delete a session that is
  still valid when the IdP recovers.
- **D7. One new log event, `oidc.discovery_failed`, with the fields `{ zone, stage, reason }`.**
  `stage` is a closed union with one value today, `'login'`, in the same shape as
  `store.unavailable`'s `stage`. A later callback or refresh stage then needs no new event name.
  The event does not use `store.unavailable`, because an operator must tell a Redis fault from an
  IdP fault. The event never carries the caught error, its message, its name or a URL.
  **Meaning of the event:** "this process has no discovered configuration, and a login needed
  one". Its absence does NOT mean that the IdP is healthy (§ 1).
- **D8. A closed `reason` on the error and the event.** Today the discovery error escapes to Next,
  and Next writes its message to stderr. After this change the route consumes the error, so the
  event is the only record. The name alone (`TypeError`, `ClientError`) does not tell a timeout
  from a wrong issuer. So `OidcDiscoveryFailed` carries
  `reason: 'timeout' | 'network' | 'http_status' | 'invalid_metadata' | 'issuer_mismatch' | 'other'`.
  The adapter sets it from the library error. The values are fixed literals: they are not an error
  object and not a URL, so A2 stays true. The mapping (openid-client 6.8.8, `errorHandler` in
  `build/index.js`):

  | Library error | `reason` |
  |---|---|
  | `ClientError` with `code` `OAUTH_TIMEOUT` or `OAUTH_ABORT` | `timeout` |
  | `TypeError` (a failed `fetch`: DNS, connection refused, TLS) | `network` |
  | `ClientError` with `code` `OAUTH_RESPONSE_IS_NOT_CONFORM` (an HTTP status that is not 200) | `http_status` |
  | `ClientError` with `code` `OAUTH_RESPONSE_IS_NOT_JSON`, `OAUTH_PARSE_ERROR`, `OAUTH_INVALID_RESPONSE`, `OAUTH_INVALID_SERVER_METADATA` or `OAUTH_MISSING_SERVER_METADATA` | `invalid_metadata` |
  | `ClientError` with `code` `OAUTH_JSON_ATTRIBUTE_COMPARISON_FAILED` | `issuer_mismatch` |
  | anything else | `other` |

  The adapter reads only `name`, `code` and the class. It reads `code` as a string compared against
  this closed list. An unknown code gives `other`, so no library value reaches the log.
- **D9. No store call and no state change on this path.** The discovery failure happens before
  `putTransaction` and before the session delete. The session record and the session cookie stay
  as they were. `login.started` is not logged.

## 4. Design

### 4.1 `src/core/errors.ts`

Add, after `SessionStoreTimeout` and its classifier:

```ts
/** Why OIDC discovery failed. A closed vocabulary: the only diagnostic value that is logged (SMA-656 D8). */
export type OidcDiscoveryFailureReason = 'timeout' | 'network' | 'http_status' | 'invalid_metadata' | 'issuer_mismatch' | 'other';

/** OIDC discovery did not complete (SMA-656). The message holds only the library error's name. */
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
export function isOidcDiscoveryFailed(err: unknown): boolean {
  return hasAuthErrorCode(err, 'oidc_discovery_failed');
}
```

`name` is set explicitly for the reason `SessionStoreTimeout` records: a production bundle can
mangle `constructor.name`. The class is not exported from `src/server.ts`, for the SMA-657 D4
reason: a caller of `createOidcClient` or `AuthRuntime.oidc` CAN receive one, but no consumer needs
to CLASSIFY one.

The route reads `reason` from a caught error that can come from another module copy. It must read
it defensively: a value that is not in the closed list gives `'other'`.

### 4.2 `src/adapters/oidc.ts`

- Add one helper that extracts the library error's name
  (`err instanceof Error ? err.name : 'unknown_error'`). `wrapError` and the new throw both use it.
- Add `classifyDiscoveryError(err): OidcDiscoveryFailureReason` with the D8 table. Reading
  `ClientError` with `instanceof` is safe here for the reason `classifyRefreshError` records: the
  check and the discovery call use the one module-level `client` binding.
- In `getConfig`'s discovery `.catch`, replace `throw wrapError('discovery', err)` with
  `throw new OidcDiscoveryFailed(\`oidc discovery failed: ${name}\`, classifyDiscoveryError(err))`.
  No `cause`.

Every public method calls `getConfig()`, so every method now throws `OidcDiscoveryFailed` on a
discovery failure. The challenge confirmed that no caller changes behavior:

- `handleCallback` catches every error from `authorizationCodeGrant` and maps it to
  `code_exchange_failed` (502) (`src/http/routes.ts` around line 289).
- `handleLogout` catches every error from `buildEndSessionUrl` and falls back to
  `runtime.postLogoutRedirectUri` (around line 506).
- `bestEffortRevoke` (`src/http/routes.ts` around line 361) and `revokeAll`
  (`src/core/single-flight.ts` around line 60) discard every error with no log line.
- The refresh path classifies only `isRefreshRejected` (`src/core/single-flight.ts` around line
  208). This error is not that, so it stays a transient failure, as today.
- `getSession` (`src/next/get-session.ts` around line 66) catches and logs `session.resolve_failed`,
  as today.
- `tests/e2e/fixture-server.ts` around line 169 prints the error name in a diagnostic line. It now
  prints `OidcDiscoveryFailed`, not `Error`. That is only a diagnostic line.

The implementation plan must read each of these sites again before it changes the adapter.

### 4.3 `src/ports/logger.ts`

- Add `'oidc.discovery_failed'` to `AuthEventName`.
- Add `export type OidcDiscoveryStage = 'login';` next to `StoreUnavailableStage`.

### 4.4 `src/http/store-unavailable.ts`

- Change `RetryAffordance` to
  `{ kind: 'link'; href: string; service?: 'session_store' | 'identity_provider' } | { kind: 'post'; action: string }`.
- For `link` with `service: 'identity_provider'`, use the D4 sentence. All other output is
  byte-identical to today.
- Update the file header: the builder serves the IdP case too.
- The names `storeUnavailableResponse`, `STORE_UNAVAILABLE_CSP` and `expectStoreUnavailable` stay.
  A rename is out of scope; the header states that the names are historical.

### 4.5 `src/http/routes.ts` (`handleLogin`)

1. Move the `presentedSid` and `retry` lines (and their D9 comment) above the
   `buildAuthorizationUrl` call.
2. Wrap only the `buildAuthorizationUrl` call. `handleCallback` already uses the same
   `let` + try/catch pattern. It needs `import type { AuthorizationRequest }` from
   `../adapters/oidc`.

   ```ts
   let authorization: AuthorizationRequest;
   try {
     authorization = await runtime.oidc.buildAuthorizationUrl({ … });
   } catch (err) {
     if (!isOidcDiscoveryFailed(err)) throw err;
     runtime.logger.event('oidc.discovery_failed', { zone: runtime.zone, stage: 'login', reason: discoveryReason(err) });
     return storeUnavailableResponse(retry.kind === 'link' ? { ...retry, service: 'identity_provider' } : retry);
   }
   ```

   `discoveryReason(err)` reads `reason` and returns `'other'` for any value outside the closed
   list. (`retry` is always a `link` in `handleLogin`. The plan may type it as the `link` variant
   and drop the conditional.)
3. Everything after the call stays the same.

## 5. Tests (Vitest)

### 5.0 Golden tests first (before any production change)

- T0. On the UNCHANGED code, add two tests that compare the full body with `toBe`: one for
  `{ kind: 'link', href: '/iam/auth/login' }` and one for `{ kind: 'post', action:
  '/iam/auth/logout' }`. Commit them before any change. They must pass with no edit after the
  change. This is the proof of "byte-identical" in D4. Today no test checks the session-service
  sentence.

### 5.1 Route tests (a new file, `tests/http/login-discovery-failed.test.ts`)

The header of `store-unavailable.test.ts` limits that file to store failures, so the new rows go
into a new file. Add a field `authorizationError?: Error` to `FakeOidc` in
`tests/support/store-failure.ts`, in the same pattern as `failRevoke`. When the field is set,
`buildAuthorizationUrl` rejects with it. A test error's message holds a sentinel URL:
`https://idp.invalid/.well-known/openid-configuration?sentinel-656`.

Make `expectStoreUnavailable` and `expectEventsClean` take the forbidden strings as a parameter,
with today's strings as the default, so the SMA-653 rows do not change.

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

### 5.2 Builder tests

- T8. `storeUnavailableResponse({ kind: 'link', href, service: 'identity_provider' })` holds the IdP
  sentence and not the session-service sentence, with the same headers as T0.

### 5.3 Adapter and error tests

- T9. `tests/adapters/oidc.test.ts`, one row per reason that a local fixture can produce: an
  unreachable issuer (`http://127.0.0.1:1`) → `network`; a fixture that answers 404 →
  `http_status`; a fixture that answers non-JSON → `invalid_metadata`; a fixture whose metadata
  names another issuer → `issuer_mismatch`. For each row: `isOidcDiscoveryFailed` is true, `name` is
  `OidcDiscoveryFailed`, the message matches `/^oidc discovery failed: \w+$/`, `cause` is
  `undefined`, and the message does not contain `127.0.0.1`. `timeout` is covered by a unit test of
  `classifyDiscoveryError` with a constructed `ClientError`, unless a fixture can produce it cheaply.
- T10. `isOidcDiscoveryFailed` is true for an `OidcDiscoveryFailed`, true for one from a second
  module copy, true for a plain `Error` with the code, false for a plain object with the code, and
  false for a plain `Error`.

### 5.4 End-to-end through the Next boundary

- T11. `tests/http/route-handler.test.ts` already builds a real `createOidcClient`. Add one row with
  the issuer `http://127.0.0.1:1` and `allowInsecureRequests: true`. A GET to `/auth/login` through
  `createAuthRouteHandler` gives a 503. This covers the place where the defect occurs today.

### 5.5 Proof that the tests bite

Run each mutation, record the result in the PR, and restore by an edit (not `git checkout`):

| Mutation | Must fail |
|---|---|
| Delete the catch in `handleLogin` | T1–T4, T6, T7, T11 |
| Catch every error (remove the `isOidcDiscoveryFailed` guard) | T5 |
| `isOidcDiscoveryFailed` always returns `false` | T1, T2, T6, T11 |
| Put `String(err)` into the event fields | T4 |
| Change the default `service` to `'identity_provider'` | T0 |
| Adapter goes back to `wrapError('discovery', …)` | T9, T11 |
| Adapter sets `cause: err` | T9 |
| `classifyDiscoveryError` always returns `'other'` | T9 |

## 6. Known limits

- An IdP outage after the first successful discovery does not give this 503 (§ 1). `/auth/login`
  then redirects to an IdP that does not answer. The absence of `oidc.discovery_failed` does not
  mean that the IdP is healthy.
- A configuration defect in discovery gives a 503 with a retry link that cannot work (D2). The
  `reason` field identifies it.

## 7. Documentation changes

- `src/http/routes.ts` header (around lines 27–31): it says the store failure is THE exception and
  that every other error propagates. Add the discovery failure as the second exception.
- `src/server.ts` (around lines 85–86): it names only the store 503 as the response that passes
  through. Add the discovery 503.
- `src/adapters/oidc.ts` header (around lines 18–20): it says every method rethrows through
  `wrapError`. Add that a discovery failure is an `OidcDiscoveryFailed` with a closed `reason`.
- `README.md` (around lines 123–142): describe the discovery 503, its `reason` values, the
  `oidc.discovery_failed` event, and the § 6 limits. Extend the ingress and mesh warning (SMA-653 §
  8) to this 503.
- SMA-506 design: add one line to § 7.1 that points to this spec, and add `oidc.discovery_failed`
  to the § 12 event list, as SMA-626 and SMA-653 did.

## 8. Out of scope

- The callback's discovery failure. It gives a 502 in plain text through `code_exchange_failed`,
  and its event looks the same as a token-endpoint failure. A follow-up issue records it (see the
  gate question).
- Logout. It already falls back when discovery fails.
- A rename of `storeUnavailableResponse`, `STORE_UNAVAILABLE_CSP` or `expectStoreUnavailable`.
- A change to the heading or to the logout page text.
- A Playwright e2e test with an IdP that is down. T11 covers the Next boundary.

## 9. Challenge changelog (revision 2)

Folded in:

- T7 (old) claimed a byte-identical proof that no test gave → new T0 golden tests first.
- D2 did not match how discovery fails → D2 now states that every discovery failure, including a
  configuration defect, gives a 503. The class is renamed `OidcDiscoveryFailed`. The page sentence
  no longer says "did not answer".
- The change removed the only diagnostic data → D8, a closed `reason` on the error and the event.
- The 503 occurs only before the first discovery succeeds → § 1, D7 and § 6.
- Four comments and the README become false → § 7.
- § 5.4 mutation table was wrong for T5 and had no redaction mutation → § 5.5.
- T4 reused sentinels that the IdP error does not contain → parameterized helpers, new sentinel.
- The harness option could not reach `harness()` → `FakeOidc.authorizationError`, new test file.
- No second-module-copy row → T6(a), T10.
- No test of the "no `cause`" rule → T9.
- The caller list was incomplete, and revoke does not log → § 4.2 corrected.
- The reason for not exporting the class was wrong → SMA-657 D4 reason.
- The `cause` parameter allowed an ignored state and looked like `Error.cause` → `service` on the
  `link` variant only.
- No test through the real adapter and the Next boundary → T11.
- The event shape → `oidc.discovery_failed { zone, stage, reason }`, as the challenger asked.

Not changed:

- `Retry-After: 5`. The challenger asked whether it fits. It is advisory, and a retry runs discovery
  again. A different number needs a measurement that this issue does not have.
