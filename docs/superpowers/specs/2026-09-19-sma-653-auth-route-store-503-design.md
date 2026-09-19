# SMA-653: map a session-store failure on the auth routes to 503

- Issue: SMA-653 (found while specifying SMA-651).
- Related: SMA-506 (the auth design, § 7.2), SMA-651 (the store operation deadline).
- Package: `ts/packages/paigasus-auth` (`@paigasus/auth`).

## 1. Problem

The SMA-506 design, § 7.2, says: a store failure at login returns 503 with a retry affordance, and
leaves no partial state. The code does not do this.

- `handleLogin`, `handleCallback` and `handleLogout` in `src/http/routes.ts` do not catch
  `SessionStoreUnavailable`.
- `createAuthRouteHandler` in `src/server.ts` maps only `CallbackRejected`. It re-throws every other
  error.

So a store error on an auth route escapes as an unhandled route error, and Next returns its default
500. After SMA-651, a wedged Redis gives a bounded `SessionStoreTimeout` (a subclass of
`SessionStoreUnavailable`) on these routes, so the 500 is now the normal outcome of a wedge.

A second defect: `handleLogout` reads the record before it deletes it (`routes.ts:356-360`). If the
read fails, the delete is never sent, and the session stays live for its TTL.

The page path (`getSession`) is not in scope. It already degrades to "no session".

## 2. Decisions

- **D1. The mapping lives in `createAuthRoutes` (`src/http/routes.ts`), not in
  `createAuthRouteHandler`.** The 503 is a fixed rule of § 7.2, not a caller choice. Only
  `routes.ts` knows which store step failed, and the logout fix (D5) needs that. Every consumer gets
  the mapping: today the two console apps and the e2e fixture server all go through
  `createAuthRouteHandler`, which passes a `Response` through unchanged. `createAuthRoutes` is also
  exported, and a direct caller gets the same 503. This differs from `CallbackRejected`, which
  `routes.ts` still lets propagate so that the Next boundary decides its response. The header comment
  of `routes.ts` records the difference.
- **D2. Only `SessionStoreUnavailable` (and its subclasses) is mapped.** The check is
  `err instanceof SessionStoreUnavailable`. Every other error propagates as before. The callback's
  `throw new Error('failed to persist a newly minted session: …')` for a `set` that returns `false`
  stays a 500. It is out of scope.
- **D3. The retry affordance is a `Retry-After` header plus a small static HTML page with a retry
  control.** The login and callback pages have a link to the login route. The logout page has a
  `<form method="post">` to the logout route, because a link cannot send a POST.
- **D4. A 503 sets no cookie.** The response has no `Set-Cookie` header. The browser keeps the state
  it had before the request. This is the "no partial state" rule of § 7.2, applied to the browser.
  Server-side leftovers are listed in § 4 and all expire by their own TTL.
- **D5. Logout attempts the delete even when the read fails.** A failed read means that there is no
  refresh token to revoke, so revocation is skipped. The delete still runs.
- **D6. When the logout delete fails, the route returns 503 and keeps the session cookie.** It does
  not redirect to the identity provider. A cleared cookie would show the user a false "signed out"
  while a copied cookie stays live for its TTL. The user sees that logout did not finish and can
  retry from the form, which sends the cookie again.
- **D7. `Retry-After` is a fixed 5 seconds.** With the shipped `PAIGASUS_SESSION_REDIS_TIMEOUT_MS`
  of 1000 ms, the SMA-651 circuit cooldown is 4 × 1000 = 4000 ms, so a retry after 5 s reaches a
  closed or re-probing circuit. An operator who raises the timeout makes the cooldown longer than
  5 s. The value is advisory, so a fixed constant is acceptable; it is not derived from config.

## 3. The 503 response

A new module, `src/http/store-unavailable.ts`, builds the response. It is private to the package (it
is not exported from `src/server.ts`).

```ts
type RetryAffordance =
  | { kind: 'link'; href: string } // a GET to the login route
  | { kind: 'post'; action: string }; // a POST form to the logout route

function storeUnavailableResponse(retry: RetryAffordance): Response;
```

- Status `503`.
- Headers: `Retry-After: 5`, `Cache-Control: no-store`, `Content-Type: text/html; charset=utf-8`.
  No `Set-Cookie`.
- The body is a fixed HTML document: a title and heading "Sign-in is temporarily unavailable" (for
  logout: "Sign-out did not complete"), one sentence, and the retry control. It has no script and no
  inline style. No error text, error name or DSN is in the body.
- The `href` and `action` values are full paths that include the basePath, because the browser
  resolves them against the current URL. Each value is HTML-escaped (`&`, `<`, `>`, `"`, `'`) before
  it goes into the attribute. The `returnTo` query value is encoded with `encodeURIComponent`.
  `returnTo` is influenced by an attacker, although it is already a validated same-origin path.

## 4. Behaviour for each route

| Route | Failing store call | Response | Retry target | Server-side leftover |
|---|---|---|---|---|
| `GET /auth/login` | `putTransaction` | 503 | `<basePath>/auth/login?returnTo=<returnTo>` | none, or a late-landing transaction that expires after 10 min |
| `GET /auth/login` | `delete(old sid)` | 503 | the same link | a stored transaction with no cookie; it expires unused after 10 min. The old session stays live. |
| `GET /auth/callback` | `takeTransaction` | 503 | `<basePath>/auth/login` | the transaction may still exist; it expires after 10 min |
| `GET /auth/callback` | `delete(old sid)` | 503 | `<basePath>/auth/login?returnTo=<tx.returnTo>` | the old session may stay live; the authorization code is spent |
| `GET /auth/callback` | `set(new sid)` | 503 | the same link | the old session is deleted; a late-landing new record has no cookie and expires after `ttlMs` (SMA-651 § 6) |
| `POST /auth/logout` | `get` | the delete still runs (D5); if it succeeds, the normal 302 logout | — | none |
| `POST /auth/logout` | `delete` (after a `get` that succeeded or failed) | 503, the cookie is kept, no IdP redirect | a POST form to `<basePath>/auth/logout` | the session stays live until the retry or its TTL |

`<returnTo>` for login is the value that `handleLogin` computes after `validateReturnTo` and the
auth-route guard, so a retry can never target an auth route. `tx.returnTo` was validated the same
way when the transaction was stored.

The callback steps that come before the first store call (`state` missing, the transaction cookie
missing) are unchanged. A `CallbackRejected` still propagates, and a store error from a later step
cannot hide it, because the rejections happen before the failing calls or instead of them.

`handleLogoutCallback` makes no store call and does not change.

## 5. Logging

Each mapped failure emits the existing `store.unavailable` event once:

```ts
runtime.logger.event('store.unavailable', { zone, stage, ...(sid ? { sid: sidTag(sid) } : {}) });
```

`stage` is a closed set of literals, declared as a union type in `store-unavailable.ts`:
`login_put_transaction`, `login_delete`, `callback_take_transaction`, `callback_delete`,
`callback_set`, `logout_get`, `logout_delete`. The `sid` field is present only for a step that acts
on a session id, and it is always the 8-character `sidTag`.

The fields never hold the caught error, its message, or its name. The `SessionStoreUnavailable`
message holds the REDACTED DSN (`redis-store.ts:155`), and the logger port's contract forbids a DSN in
any form. A `SessionStoreTimeout` already logs `store.operation_timeout` from the deadline decorator,
so this event does not repeat its phase.

When the logout read fails and the delete succeeds, `store.unavailable` (stage `logout_get`) is
emitted, and `logout.completed` follows with `revoked: false`, as today for a session that has no
refresh token.

## 6. Tests

All tests are Vitest unit tests in `ts/packages/paigasus-auth/tests/http/`. No Docker is needed.

1. **`store-unavailable.test.ts` (new), table-driven.** A stub store wraps `MemorySessionStore` and
   throws on one named method. There is one row for each row of the § 4 table. Each row runs twice:
   once with `SessionStoreUnavailable` and once with `SessionStoreTimeout`. Each row asserts:
   - the status, and for a 503: `Retry-After: 5`, `Cache-Control: no-store`, the HTML content type,
     and no `Set-Cookie` header;
   - the retry target (the `href` or form `action`), parsed from the body;
   - exactly one `store.unavailable` event with the expected `stage`, and the `sid` tag where § 5
     says so.
2. **Redaction.** The thrown error's message holds a sentinel DSN string (for example
   `redis://user:sentinel-pw@redis.invalid:6379`). The test asserts that the sentinel is absent from
   the body, every response header, and every logged field of every event, for every row.
3. **Logout delete after a failed read.** `get` throws. The test asserts that `delete` was called
   with the presented sid, the response is a 302 to the end-session URL, the session cookie is
   cleared, and `oidc.revoke` was not called.
4. **Escaping.** A `returnTo` that holds `"`, `<`, `&` and `'` characters (all allowed by
   `validateReturnTo`) appears in the body only in its encoded form. No raw `<` from the input
   reaches the body.
5. **Other errors still propagate.** A store method that throws a plain `Error` still rejects
   `handle()`. This proves D2.
6. **Next boundary.** One row in `route-handler.test.ts`: a failing `putTransaction` through
   `createAuthRouteHandler` under basePath `/iam` returns 503, and the retry link starts with
   `/iam/auth/login`.

**Mutation check.** After the implementation, each new catch is deleted, one at a time, and at least
one test must go red for each deletion. The logout fix is reverted to the old read-then-delete order,
and test 3 must go red. A test that passes only because the code did not exist yet does not count
(red-first is not proof).

## 7. Documentation changes

- `src/http/routes.ts`: the header comment states D1 (the store-error mapping lives here, unlike
  `CallbackRejected`). The `handleLogout` comment block states D5 and D6.
- `src/server.ts`: the `createAuthRouteHandler` doc comment says that a store failure arrives as a
  503 `Response` from `createAuthRoutes`, and is not mapped here.
- `docs/superpowers/specs/2026-09-09-sma-506-auth-design.md` § 7.2: one line that points to this
  spec for the route behaviour.

## 8. Out of scope

- The page path (`getSession`, `requireSession`). SMA-651 § 7 rejected a 503 there.
- The callback's `set` returning `false` (a plain `Error`, a 500).
- A store error on a route that makes no store call (`/auth/logout/callback`).
- An end-to-end test with a real wedged Redis. The unit tests cover each mapping. The SMA-651
  Docker suite already covers the store's own timeout behaviour.

## 9. Rejected alternatives

- **Map in `createAuthRouteHandler`.** It keeps the `CallbackRejected` pattern, but the Next boundary
  cannot tell a failed logout read from a failed logout delete, so it cannot implement D5 and D6.
- **Plain-text body, or no body.** The user gets no way to retry except the back button.
- **Clear the cookie when the logout delete fails.** It shows a false "signed out" while the
  server-side session stays live, and a retry then has no cookie to send.
