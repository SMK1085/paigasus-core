# SMA-653: map a session-store failure on the auth routes to 503

- Issue: SMA-653 (found while specifying SMA-651).
- Related: SMA-506 (the auth design, § 7.2), SMA-651 (the store operation deadline).
- Package: `ts/packages/paigasus-auth` (`@paigasus/auth`).
- Revision 2: folds in the spec challenge (§ 11).

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

The page path (`getSession`) keeps its behaviour: it degrades to "no session". Its classification
check changes (D8).

## 2. Decisions

- **D1. The mapping lives in `createAuthRoutes` (`src/http/routes.ts`), not in
  `createAuthRouteHandler`.** The 503 is a fixed rule of § 7.2, not a caller choice. Only
  `routes.ts` knows which store step failed, and the logout fix (D5) needs that. Every consumer gets
  the mapping: today the two console apps and the e2e fixture server all go through
  `createAuthRouteHandler`, which passes a `Response` through unchanged (`server.ts:106`).
  `createAuthRoutes` is also exported, and a direct caller gets the same 503. This differs from
  `CallbackRejected`, which `routes.ts` still lets propagate so that the Next boundary decides its
  response. The header comment of `routes.ts` records the difference.
- **D2. Only a store-unavailable error is mapped, and it is classified by its `code`, not by
  `instanceof`.** A new function in `src/core/errors.ts`:

  ```ts
  export function isSessionStoreUnavailable(err: unknown): boolean {
    return err instanceof Error && (err as { code?: unknown }).code === 'session_store_unavailable';
  }
  ```

  The reason is measured module duplication. `runtime.ts:176-188` records that on Next 16.3.4 a route
  handler and a page load SEPARATE copies of this package, and the runtime is shared between them
  through `globalThis` (`runtime.ts:222-234`). So the store, and every error class it throws, comes
  from the copy of the layer that called `getAuthRuntime` first. `iam-console/lib/auth.ts:23-25`
  says that both orders occur. When a page builds the runtime first, the store throws the page
  copy's `SessionStoreUnavailable`, and `err instanceof SessionStoreUnavailable` in the route copy is
  `false`. The failure mode is then the old 500, silently, for the life of the process. (This is
  inferred from the measured duplication; the `instanceof` failure itself was not measured.)

  The `code` class field (`errors.ts:10`) is an own property of each instance, and
  `SessionStoreTimeout` inherits it, so the check covers both classes from any copy. Every other
  error propagates as before. The callback's `throw new Error('failed to persist a newly minted
  session: …')` for a `set` that returns `false` stays a 500. It is out of scope.
- **D3. The retry affordance is a `Retry-After` header plus a small static HTML page with a retry
  control.** The control is a link (GET) or a `<form method="post">` (for logout, because a link
  cannot send a POST). § 4 gives the target for each row.
- **D4. A 503 sets no cookie.** The response has no `Set-Cookie` header. The browser keeps the state
  it had before the request. This is the "no partial state" rule of § 7.2, applied to the browser.
  § 4 lists the server-side and identity-provider leftovers for each row.
- **D5. Logout attempts the delete even when the read fails.** A failed read means that there is no
  refresh token to revoke, so revocation is skipped. The delete still runs. Note the limit: during a
  real wedge the read reaches its 4T deadline and opens the SMA-651 circuit, and the circuit then
  refuses the delete at once. So D5 changes the outcome for a TRANSIENT read failure (a refused
  connection that recovers, a single blip). During a wedge, the usual result is the 503 of D6.
- **D6. When the logout delete fails, the route returns 503 and keeps the session cookie.** It does
  not redirect to the identity provider. A cleared cookie would show the user a false "signed out"
  while a copied cookie stays live for its TTL. The user sees that logout did not finish and can
  retry from the form, which sends the cookie again.
- **D7. `Retry-After` is a fixed 5 seconds.** With the shipped `PAIGASUS_SESSION_REDIS_TIMEOUT_MS`
  of 1000 ms, the SMA-651 circuit cooldown is 4 × 1000 = 4000 ms (`operation-deadline.ts:26`), so a
  retry after 5 s reaches a closed or re-probing circuit. An operator who raises the timeout makes
  the cooldown longer than 5 s. The value is advisory, so a fixed constant is acceptable; it is not
  derived from config.
- **D8. `get-session.ts:76` uses the same `isSessionStoreUnavailable` check.** It has the same
  module-duplication defect as D2: a foreign-copy error is logged as `session.resolve_failed`
  instead of `store.unavailable`. The behaviour (return `null`) does not change; only the event name
  becomes correct. The same shape in `single-flight.ts:170` (`RefreshRejected`) is NOT fixed here; a
  follow-up issue records it (§ 9).
- **D9. A login retry keeps a session that survived.** During a wedge, `requireSession` sends a
  signed-in user to `/auth/login`. With this change, `putTransaction` fails first
  (`routes.ts:180`, before the delete at `:188-191`), so the route returns 503 and the session record
  and its cookie both survive. A retry link to `/auth/login` would then delete that still-valid
  session after Redis recovers, which is the session loss SMA-651 § 5 describes. So on the two login
  rows, when the request carried `__Host-pgs_sid`, the retry link points at the validated `returnTo`
  itself. If the session is alive, the page renders. If it is gone, `requireSession` sends the user
  to login, as before. When the request carried no session cookie, the link points at the login
  route.
- **D10. On the two callback rows after a successful code exchange, the route revokes the new
  refresh token, best effort, before the 503.** The exchange has already created a session at the
  identity provider, and with the default `offline_access` scope that is an offline session with a
  live refresh token. Without a revoke, every retry during an outage leaves one more at the IdP. The
  rule is the same as logout step 3 (`routes.ts:371-385`): a revoke failure is swallowed, never
  logged as an error object, and never changes the response. The call adds at most
  `PAIGASUS_OIDC_HTTP_TIMEOUT_MS` to a response that is already an error.

## 3. The 503 response

A new module, `src/http/store-unavailable.ts`, builds the response. It is private to the package (it
is not exported from `src/server.ts`).

```ts
type RetryAffordance =
  | { kind: 'link'; href: string } // a GET
  | { kind: 'post'; action: string }; // a POST form to the logout route

function storeUnavailableResponse(retry: RetryAffordance): Response;
```

- Status `503`.
- Headers:
  - `Retry-After: 5`
  - `Cache-Control: no-store`
  - `Content-Type: text/html; charset=utf-8`
  - `Referrer-Policy: no-referrer`. A callback 503 is served at `/auth/callback?code=…&state=…`, and
    on the `callback_take_transaction` row the code is not yet spent. Without this header, the retry
    link sends that URL, with the code, as the `Referer` (RFC 9700 § 4.2.4).
  - `Content-Security-Policy: default-src 'none'; form-action 'self'; frame-ancestors 'none';
    base-uri 'none'`. No CSP exists in the apps today. This policy makes a future escaping defect
    in the reflected `returnTo` inert, prevents framing, and `base-uri 'none'` stops an injected
    `<base>` from moving the absolute-path links.
  - No `Set-Cookie`.
- The body is a fixed HTML document: a title and heading "Sign-in is temporarily unavailable" (for
  logout: "Sign-out did not complete"), one sentence, and the retry control. It has no script and no
  style. No error text, error name or DSN is in the body.
- The `href` and `action` values are full paths that include the basePath, because the browser
  resolves them against the current URL. Two escaping layers apply, in this order:
  1. The `returnTo` query value is encoded with `encodeURIComponent`. Without this, a `returnTo`
     that holds `&` or `#` splits the query or becomes a fragment.
  2. The whole attribute value is HTML-escaped (`&` → `&amp;`, `<` → `&lt;`, `>` → `&gt;`,
     `"` → `&quot;`, `'` → `&#39;`) and put in a DOUBLE-quoted attribute. A D9 link to the raw
     `returnTo` path needs this layer, because the path is not URL-encoded.

  `returnTo` is influenced by an attacker, although `validateReturnTo` and the auth-route guard
  already reduce it to a same-origin path that is not an auth route and cannot be `//host` or
  `javascript:`.

## 4. Behaviour for each route

| # | Route | Failing store call | Response | Retry target | Leftover |
|---|---|---|---|---|---|
| 1 | `GET /auth/login` | `putTransaction` | 503 | with a session cookie: `returnTo` (D9); without: `<basePath>/auth/login?returnTo=<returnTo>` | none, or a late-landing transaction that expires after 10 min |
| 2 | `GET /auth/login` | `delete(old sid)` | 503 | the same as row 1 | a stored transaction with no cookie; it expires unused after 10 min. The old session stays live. |
| 3 | `GET /auth/callback` | `takeTransaction` | 503 | `<basePath>/auth/login` | the transaction may still exist; it expires after 10 min |
| 4 | `GET /auth/callback` | `delete(old sid)` | 503, after a best-effort revoke (D10) | `<basePath>/auth/login?returnTo=<tx.returnTo>` | the old session may stay live; the code is spent. At the IdP: a session, and an offline session if the revoke failed. |
| 5 | `GET /auth/callback` | `set(new sid)` | 503, after a best-effort revoke (D10) | the same as row 4 | the old session is deleted. A late-landing new record has no cookie and expires after `ttlMs` (SMA-651 § 6); it holds the refresh token, which is dead if the revoke succeeded. At the IdP: as row 4. |
| 6 | `POST /auth/logout` | `get` only | the delete still runs (D5) and succeeds; the normal 302 logout, `revoked: false` | — | none |
| 7 | `POST /auth/logout` | `delete` only | 503, cookie kept, no revoke, no IdP redirect | a POST form to `<basePath>/auth/logout` | the session stays live until the retry or its TTL |
| 8 | `POST /auth/logout` | `get` and `delete` | 503, cookie kept, no IdP redirect | the same as row 7 | the same as row 7. This is the usual row during a wedge (D5). |

`<returnTo>` for login is the value that `handleLogin` computes after `validateReturnTo` and the
auth-route guard, so a retry can never target an auth route. `tx.returnTo` was validated the same
way when the transaction was stored.

The callback steps that come before the first store call (`state` missing, the transaction cookie
missing) are unchanged. A `CallbackRejected` still propagates. A store error cannot hide one, because
every rejection in `routes.ts:220-266` happens before the failing store calls or instead of them. The
principal resolvers make no store call.

`handleLogoutCallback` makes no store call and does not change.

Whether row 7 should revoke the refresh token that `get` returned is open question Q1 (§ 10).

## 5. Logging

Each failing store call emits the existing `store.unavailable` event once. A response can therefore
carry two events (row 8: `logout_get`, then `logout_delete`).

```ts
runtime.logger.event('store.unavailable', { zone, stage, ...(sid ? { sid: sidTag(sid) } : {}) });
```

`stage` is a closed set of literals. The union type moves to `src/ports/logger.ts`, and it covers
ALL emitters of this event, not only the routes:

- existing: `get_session` (`get-session.ts:77`), `release_lock` (`single-flight.ts:221`);
- new: `login_put_transaction`, `login_delete`, `callback_take_transaction`, `callback_delete`,
  `callback_set`, `logout_get`, `logout_delete`.

The two existing emitters change to use the typed value; their output does not change. The `sid`
field is present only for a step that acts on a session id, and it is always the 8-character
`sidTag`.

The fields never hold the caught error, its message, or its name. The `SessionStoreUnavailable`
message holds the REDACTED DSN (`redis-store.ts:155`), and the logger port's contract forbids a DSN in
any form.

A best-effort revoke on rows 4 and 5 (D10) records no separate event. Its outcome is not needed to
operate the system, and the swallow rule of logout step 3 applies.

In row 6, `store.unavailable` (stage `logout_get`) comes first, then `logout.completed` with
`revoked: false`.

## 6. Tests

All tests are Vitest unit tests in `ts/packages/paigasus-auth/tests/`. No Docker is needed.

1. **`tests/http/store-unavailable.test.ts` (new), table-driven.** A stub store wraps
   `MemorySessionStore` and throws on a named SET of methods. There is one row for each row of the
   § 4 table; rows 1 and 2 each run with and without a session cookie. Each row runs twice: once with
   `SessionStoreUnavailable` and once with `SessionStoreTimeout`. Each row asserts:
   - the status, and for a 503 every header in § 3, and that there is no `Set-Cookie` header;
   - the retry target (the `href` or form `action`), parsed from the body;
   - the exact ordered list of `store.unavailable` events with their `stage` and `sid` tag.
2. **Foreign module copy (D2).** A row throws an error built from a SECOND copy of
   `src/core/errors.ts` (loaded with `vi.resetModules()` and a dynamic `import`) and asserts a 503.
   The same test asserts that `getSession` logs `store.unavailable`, not `session.resolve_failed`,
   for such an error (D8). Both assertions must fail with the old `instanceof` form.
3. **Redaction.** The thrown error's message holds a sentinel DSN string (for example
   `redis://user:sentinel-pw@redis.invalid:6379`). For every row, the test asserts that the sentinel
   is absent from the body, every response header, and every logged field of every event.
4. **Logout delete after a failed read (row 6).** `get` throws. The test asserts that `delete` was
   called with the presented sid, the response is a 302 to the end-session URL, the session cookie
   is cleared, `oidc.revoke` was not called, and the events are `store.unavailable` (`logout_get`,
   with the sid tag), then `logout.completed` with `revoked: false`.
5. **Escaping and round trip.** The input `returnTo` holds `"`, `<`, `>`, `&`, `'`, `#` and a space
   (all allowed by `validateReturnTo`). The test asserts:
   - the exact attribute bytes (for example `&#39;` for `'`, and a double-quoted attribute);
   - a round trip: after an HTML-decode of the attribute,
     `new URL(href, origin).searchParams.get('returnTo')` equals the input (login link), and
     `new URL(href, origin).pathname + search` equals the input (D9 link).
6. **Revoke on the callback rows (D10).** On rows 4 and 5, `oidc.revoke` is called with the new
   refresh token. A second case makes `revoke` reject and asserts that the response is still the same
   503 and that no event holds the error.
7. **Other errors still propagate.** A store method that throws a plain `Error` still rejects
   `handle()`. This proves the D2 filter.
8. **Next boundary.** One row in `tests/http/route-handler.test.ts`: a failing `putTransaction`
   through `createAuthRouteHandler` under basePath `/iam`, with no session cookie, returns 503, and
   the retry link starts with `/iam/auth/login`.

**Mutation check.** After the implementation, each item below is removed or reverted, one at a time,
and at least one test must go red for each:

- each new catch site (login, callback, logout read, logout delete);
- `isSessionStoreUnavailable` replaced by `instanceof` (in `routes.ts`, and in `get-session.ts`);
- the logout order reverted to read-then-delete;
- the D9 branch;
- each escaping layer (`encodeURIComponent`, and the HTML escaper);
- each security header in § 3;
- the D10 revoke.

A test that passes only because the code did not exist yet does not count (red-first is not proof).

## 7. Documentation changes

- `src/http/routes.ts`: the header comment states D1 (the store-error mapping lives here, unlike
  `CallbackRejected`). The `handleLogout` comment block states D5 and D6.
- `src/server.ts`: the `createAuthRouteHandler` doc comment says that a store failure arrives as a
  503 `Response` from `createAuthRoutes`, and is not mapped here.
- `src/core/errors.ts`: the doc comment on `isSessionStoreUnavailable` records the reason for D2.
- `ts/packages/paigasus-auth/README.md` (lines 115-126): replace the text that says a failed login
  delete leaves the record and that "SMA-653 tracks the fix". Describe the 503, the retry control,
  D9, rows 6-8, and the new `stage` values.
- `docs/superpowers/specs/2026-09-09-sma-506-auth-design.md` § 7.2: one line that points to this
  spec for the route behaviour.

## 8. Assumptions

- Nothing in front of the app replaces or retries a 503. An ingress with custom error pages for 503
  removes the retry control, and a service mesh that retries 503 replays each login GET and logout
  POST. The old 500 did not have this exposure. A deployment that has such a policy must exempt the
  auth routes.

## 9. Out of scope, with follow-ups

- The page path's behaviour (`getSession`, `requireSession`). SMA-651 § 7 rejected a 503 there. Only
  the classification check changes (D8).
- The callback's `set` returning `false` (a plain `Error`, a 500).
- `/auth/logout/callback`, which makes no store call.
- An end-to-end test with a real wedged Redis. The unit tests cover each mapping. The SMA-651
  Docker suite covers the store's own timeout behaviour.
- **Follow-up issue A:** SMA-506 § 7.1 specifies "login fails, 503 page" for an OIDC discovery
  failure. `buildAuthorizationUrl` (`routes.ts:172`) still gives a 500. It can reuse the new 503
  builder.
- **Follow-up issue B:** `RefreshRejected` in `single-flight.ts:170` is classified with `instanceof`
  and has the D2 module-duplication defect.
- **Not a follow-up:** a first-connect `SessionStoreUnavailable` from `createRedisSessionStore`
  (`redis-store.ts:313-315`) is raised from `await authRuntime()` in each app's `route.ts`, outside
  `createAuthRoutes`, so it stays a 500. `redis-store.ts:260-262` says that this path is defensive
  only: the first connect is bounded and the store is created even when Redis is unreachable.

## 10. Open questions for the gate

- **Q1.** On row 7 (`get` returned a refresh token, `delete` failed), should the route revoke the
  refresh token, best effort, before the 503? This does not break the SMA-506 § 9.5 order rule,
  because the delete has already failed. Benefit: a copied cookie then works only until its access
  token expires, not for the session TTL. Cost: up to `PAIGASUS_OIDC_HTTP_TIMEOUT_MS` more on an
  error response, and the session record then points at a dead refresh token until the retry.
  Recommendation: yes.

## 11. Rejected alternatives

- **Map in `createAuthRouteHandler`.** It keeps the `CallbackRejected` pattern, but the Next boundary
  cannot tell a failed logout read from a failed logout delete, so it cannot implement D5 and D6.
- **Plain-text body, or no body.** The user gets no way to retry except the back button.
- **Clear the cookie when the logout delete fails.** It shows a false "signed out" while the
  server-side session stays live, and a retry then has no cookie to send.
- **Retry the callback URL itself on row 3.** The code is not spent there, and a retry keeps
  `returnTo`. Rejected: it puts the authorization code into the page, and the transaction may be
  consumed late (SMA-651 § 6), so the retry can fail anyway.
- **A `cause` field (`timeout`, `circuit_open`, `unavailable`) on `store.unavailable`.** It would
  separate a circuit refusal from an offline client, which the once-per-opening
  `store.operation_timeout` event does not. Not taken here: it changes all three emitters and no
  acceptance criterion needs it.
