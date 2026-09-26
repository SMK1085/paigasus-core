# SMA-656: `/auth/login` gives a 503, not a 500, when OIDC discovery fails

- Linear: SMA-656
- Date: 2026-09-26
- Package: `ts/packages/paigasus-auth`
- Related: SMA-506 (auth design, § 7.1), SMA-653 (store 503, § 9 follow-up A), SMA-657 (D7, code classification)

## 1. The problem

The SMA-506 design (§ 7.1) says: when OIDC discovery fails, login fails with a 503 page.

The code does not do this. `handleLogin` in `src/http/routes.ts` calls
`runtime.oidc.buildAuthorizationUrl` with no catch. The adapter runs discovery lazily inside that
call. A discovery failure goes out of `handleLogin` and out of `createAuthRoutes.handle`.
`createAuthRouteHandler` in `src/server.ts` catches only `CallbackRejected`, so it throws the error
again, and Next returns its default 500.

## 2. Acceptance (from the issue)

- A1. A discovery failure in `buildAuthorizationUrl` gives a 503 with a retry affordance.
- A2. No error object and no URL from the OIDC library gets into the response or a log.
- A3. A test proves the mapping.

## 3. Decisions

- **D1. The route recognizes a discovery failure by a code.** The adapter throws a new
  `OidcUnavailable` error with `code = 'oidc_unavailable'`. The route calls `isOidcUnavailable`,
  which uses `hasAuthErrorCode`. The runtime and its `oidc` client are shared through `globalThis`
  between the module copies that Next makes. The adapter builds the error in one copy and the route
  can catch it in the other copy. SMA-657 D7 says such an error must be classified by its `code`,
  not by `instanceof`.
- **D2. Only a discovery failure becomes a 503.** `buildAuthorizationUrl` can also throw
  `oidc build_authorization_url failed` when the discovered metadata has no
  `authorization_endpoint`. That is a configuration defect, not an outage. A retry link cannot fix
  it. It stays a plain `Error` and stays a 500, so an operator sees it as a defect.
- **D3. The error message does not change.** `OidcUnavailable`'s message is
  `oidc discovery failed: <ErrorName>`, the same text that `wrapError('discovery', …)` makes today.
  It holds only the library error's `name`. No existing log line or test changes.
- **D4. The page says which service failed.** `storeUnavailableResponse` gets an optional second
  parameter, `cause: 'session_store' | 'identity_provider'`. The default is `'session_store'`, so
  every SMA-653 call site and its output stay the same. For `'identity_provider'` the sentence is
  "The identity provider did not answer. Try again in a few seconds." The heading stays
  "Sign-in is temporarily unavailable".
- **D5. The headers and the escaping do not change.** The discovery 503 uses the same builder, so
  it has `Retry-After: 5`, `Cache-Control: no-store`, `Referrer-Policy: no-referrer`,
  `Content-Type: text/html; charset=utf-8` and `STORE_UNAVAILABLE_CSP`. It has no `Set-Cookie`
  (SMA-653 D4). `Retry-After: 5` is advisory. Discovery is not cached after a failure (the adapter
  clears `configPromise`), so each retry runs discovery again.
- **D6. The retry target is the SMA-653 D9 target.** The route computes `presentedSid` and `retry`
  BEFORE it calls `buildAuthorizationUrl`. Both values read only the request cookie and `returnTo`,
  and both exist before the call, so the move does not change them. With a session cookie, the link
  goes to `returnTo`. Without one, it goes to `loginRetryHref(basePath, returnTo)`. The reason is
  the D9 reason: a link to `/auth/login` from a signed-in browser would delete a session that is
  still valid when the IdP recovers.
- **D7. A new log event, `login.oidc_unavailable`, with the field `{ zone }` only.** The event does
  not use `store.unavailable`, because an operator must be able to tell a Redis outage from an IdP
  outage. The event never carries the caught error, its message, its name or a URL.
- **D8. No store call and no state change on this path.** The discovery failure happens before
  `putTransaction` and before the session delete. The session record and the session cookie stay
  as they were. `login.started` is not logged.

## 4. Design

### 4.1 `src/core/errors.ts`

Add, after `SessionStoreTimeout` and its classifier:

```ts
/** OIDC discovery did not complete (SMA-656). The message holds only the library error's name. */
export class OidcUnavailable extends AuthError {
  readonly code = 'oidc_unavailable';
  constructor(message: string) {
    super(message);
    this.name = 'OidcUnavailable';
  }
}

/** True for an OidcUnavailable from ANY copy of this module (SMA-656 D1). See `hasAuthErrorCode`. */
export function isOidcUnavailable(err: unknown): boolean {
  return hasAuthErrorCode(err, 'oidc_unavailable');
}
```

`name` is set explicitly for the reason `SessionStoreTimeout` records: a production bundle can
mangle `constructor.name`. The class is not exported from `src/server.ts`, because no consumer can
see one: the route turns it into a 503.

### 4.2 `src/adapters/oidc.ts`

In `getConfig`'s discovery `.catch`, replace `throw wrapError('discovery', err)` with a throw of an
`OidcUnavailable` that has the same message. The name extraction must stay the same as
`wrapError`'s: `err instanceof Error ? err.name : 'unknown_error'`. The adapter must not put the
caught error into the new error as `cause`, because the rule is "no library error object".

Every public method calls `getConfig()`, so `authorizationCodeGrant`, `refresh`, `revoke` and
`buildEndSessionUrl` now also throw `OidcUnavailable` on a discovery failure. Their callers do not
change behavior:

- `handleCallback` catches every error from `authorizationCodeGrant` and maps it to
  `code_exchange_failed` (502).
- `handleLogout` catches every error from `buildEndSessionUrl` and degrades to the zone root.
- The refresh path classifies only `oidc_refresh_rejected`. Any other error, this one included,
  stays a transient failure, as it is today.
- The revoke call site catches and logs, as it does today.

The implementation plan must confirm each of these four call sites by reading the code.

### 4.3 `src/ports/logger.ts`

Add `'login.oidc_unavailable'` to `AuthEventName`.

### 4.4 `src/http/store-unavailable.ts`

- Add `export type UnavailableCause = 'session_store' | 'identity_provider';`.
- Change the signature to
  `storeUnavailableResponse(retry: RetryAffordance, cause: UnavailableCause = 'session_store')`.
- The `post` affordance ignores `cause`. Only logout uses `post`, and logout never gives
  `identity_provider`.
- For `link` + `identity_provider`, use the D4 sentence. All other output is byte-identical to
  today.
- Update the file header so it says the builder serves the IdP case too.

### 4.5 `src/http/routes.ts` (`handleLogin`)

1. Move the `presentedSid` and `retry` lines (and their D9 comment) above the
   `buildAuthorizationUrl` call.
2. Wrap only the `buildAuthorizationUrl` call:

   ```ts
   let authorization: AuthorizationRequest;
   try {
     authorization = await runtime.oidc.buildAuthorizationUrl({ … });
   } catch (err) {
     if (!isOidcUnavailable(err)) throw err;
     runtime.logger.event('login.oidc_unavailable', { zone: runtime.zone });
     return storeUnavailableResponse(retry, 'identity_provider');
   }
   ```

3. Everything after the call stays the same.

## 5. Tests (Vitest)

### 5.1 Route tests (`tests/http/store-unavailable.test.ts` or a new sibling file)

Extend `fakeOidc()` in `tests/support/store-failure.ts` with an option that makes
`buildAuthorizationUrl` reject with a given error. A test error must hold a sentinel URL in its
message (for example `https://idp.invalid/.well-known/openid-configuration?sentinel`).

- T1. No session cookie, `OidcUnavailable` → 503, the IdP sentence, the link is
  `/iam/auth/login?returnTo=<encoded>`, all D5 headers, no `Set-Cookie`.
- T2. A session cookie is present, `OidcUnavailable` → 503, the link is `returnTo`, no
  `Set-Cookie`, and the session record is still in the store after the request.
- T3. For T1 and T2: no store method is called (`putTransaction`, `delete`), exactly one event
  `login.oidc_unavailable` with fields `{ zone: 'iam' }`, and no `login.started` event.
- T4. For T1 and T2: no byte of the sentinel gets into the body, a header or a logged field.
- T5. A plain `Error` (the `build_authorization_url` case) from `buildAuthorizationUrl` propagates
  out of `handle()` (the promise rejects), and no event is logged.
- T6. An `Error` that is not an `OidcUnavailable` instance but carries `code: 'oidc_unavailable'`
  gives the 503. This proves the route classifies by code (D1).

### 5.2 Builder tests

- T7. `storeUnavailableResponse(link)` output is byte-identical to the output before this change.
  The existing SMA-653 tests prove this. They must pass with no edit.
- T8. `storeUnavailableResponse(link, 'identity_provider')` holds the IdP sentence and not the
  session-service sentence, with the same headers.

### 5.3 Adapter and error tests

- T9. `tests/adapters/oidc.test.ts`: for an unreachable issuer, `buildAuthorizationUrl` rejects
  with an error for which `isOidcUnavailable` is true, whose `name` is `OidcUnavailable`, and whose
  message matches `/^oidc discovery failed: \w+$/`.
- T10. `isOidcUnavailable` is true for an `OidcUnavailable`, true for a plain `Error` with
  `code: 'oidc_unavailable'`, false for a plain object with that code, false for a plain `Error`.

### 5.4 Proof that the tests bite

Delete the catch in `handleLogin` and run T1–T6: they must fail. Replace `isOidcUnavailable` with a
check that always returns `false`: T1, T2 and T6 must fail. Remove the `OidcUnavailable` throw in
the adapter (go back to `wrapError`): T9 must fail. Record the results in the PR.

## 6. Out of scope

- The callback's discovery failure. It gives a 502 today through `code_exchange_failed`. SMA-506
  § 7.1 gives the token exchange its own § 10 error page, so this is not a defect in this issue.
- Logout. It already degrades when discovery fails.
- An e2e test with an IdP that is down. The route tests prove the mapping (A3).
- A change to the heading or to the logout page text.
