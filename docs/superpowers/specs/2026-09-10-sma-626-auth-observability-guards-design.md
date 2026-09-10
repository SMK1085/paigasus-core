<!-- SPDX-License-Identifier: Apache-2.0 -->

# SMA-626 — `@paigasus/auth`: observability gaps and untested guards

**Status:** approved (design gate, 2026-09-10)
**Issue:** [SMA-626](https://linear.app/smaschek/issue/SMA-626)
**Baseline:** `main` at `51a2d859` (PR #230, SMA-506). 234 tests in 22 files, all pass.
**Related:** SMA-506 (the package), SMA-511 (first Next integration), SMA-508 (real grants).

## 1. Context

PR #230 merged `@paigasus/auth`. Its final whole-branch review and its CodeRabbit rounds named
six follow-up items. None blocked the merge. SMA-626 carries them.

This spec covers items 2, 3, 4 and 5, plus one residual of item 1 that the issue does not name.
It does not cover item 6.

### 1.1 Item 1 is already closed on `main`

The issue text predates the "I4 final fix wave" commit on the SMA-506 branch. `session.refresh_failed`
is now emitted where the failure happens, and three tests assert the distinction:

| Site | What it holds |
|---|---|
| `src/core/single-flight.ts:120-135` | emits `session.refresh_failed` when `refresh()` throws, then rethrows unchanged |
| `tests/core/single-flight.test.ts:212` | an IdP failure logs `session.refresh_failed` |
| `tests/core/single-flight.test.ts:228` | a STORE failure does NOT log `session.refresh_failed` |
| `tests/next/get-session.test.ts:152` | the event survives to the Next read path |

**The residual.** `src/next/get-session.ts:70` catches every failure and logs `store.unavailable`
with `stage: 'get_session'`. So during an identity-provider outage an operator sees the
`store.unavailable` rate rise while Redis is healthy. `session.refresh_failed` appears beside it,
but the false store signal stays. Section 2.3 closes it.

### 1.2 Out of scope

Item 6 moves to a separate issue: a correlation id on `login.started`, the `Sec-Fetch-Mode`
absent-header decision, rate limiting on `/auth/login`, and the store contract's fencing
assertion. Rate limiting is an ingress concern the issue itself judged to sit outside this
package.

Mounting the package in a Next app belongs to SMA-511. The real grants snapshot belongs to
SMA-508.

## 2. Refresh-failure classification (item 2, and the item-1 residual)

### 2.1 The problem

`resolveSession` degrades to `{ ...last, refreshPending: true }` when the lock wait times out and
the access token is still live (`src/core/single-flight.ts:177-186`). A thrown `refresh` gets no
such treatment: it propagates, `get-session.ts` catches it, and the user signs out with up to
`skewMs` of token life left.

A blanket degrade is wrong, because the two causes are not the same. An identity provider
returning 503 is transient — the refresh token is still good, and the access token still works.
An `invalid_grant` means the refresh token is revoked: an administrator ended the session, or the
user signed out elsewhere. Keeping that session alive for up to `skewMs` is a real exposure.

`src/adapters/oidc.ts:124-127`'s `wrapError` keeps only the caught error's `name`. It discards the
OAuth error code, so `core/single-flight.ts` cannot tell the two apart today.

### 2.2 The design

**A new error type in `src/core/errors.ts`:**

```ts
/**
 * The identity provider REFUSED the refresh, as opposed to failing to answer. `oauthError` is the
 * OAuth 2.0 error code — a fixed enum string from RFC 6749 § 5.2. It carries no URL, no token and
 * no client secret, so it is safe to hold and to log.
 */
export class RefreshRejected extends AuthError {
  readonly code = 'oidc_refresh_rejected';
  constructor(readonly oauthError: string) {
    super(`oidc refresh rejected: ${oauthError}`);
  }
}
```

It lives in `core/`, not in `adapters/`. `core/single-flight.ts` must catch it, and `core` may not
import `adapters` — the adapter maps its library's error onto the core vocabulary, which is the
direction the hexagon already runs everywhere else in this package.

**Classification in `src/adapters/oidc.ts`'s `refresh()`:**

```
DEFINITIVE = { invalid_grant, invalid_client, unauthorized_client, invalid_scope, invalid_request }

caught error is a ResponseBodyError AND its .error is in DEFINITIVE
  -> throw new RefreshRejected(error)
everything else
  -> throw wrapError('refresh_token_grant', err)     (unchanged)
```

"Everything else" covers a network failure, a timeout, an abort, `server_error`,
`temporarily_unavailable`, and any OAuth code not in the set. An unknown code therefore reads as
transient, which keeps a session alive for at most `skewMs`. That is the bounded side of the
choice; today's behaviour is the other side, and it signs users out on a network blip.

The classification must not widen `wrapError`'s redaction contract. `RefreshRejected` carries the
error CODE only, read from the library error's own `.error` field — never the error object, never
its message, never a URL.

**Degrade in `src/core/single-flight.ts`:**

```
catch (err)
  degraded = !(err instanceof RefreshRejected) && Date.now() < fresh.accessExpiresAt
  logger.event('session.refresh_failed', { sid: sidTag(sid), degraded })
  if (degraded) return { ...fresh, refreshPending: true }
  throw err
```

The `degraded` field is what makes the two outcomes tellable apart in a log. Without it, a
degrade and a sign-out produce the identical line.

The lock is still released in `finally`, unchanged. Invariant 4 is untouched: the degrade returns
from inside the `try`, so the `finally` still runs.

### 2.3 Narrowing the store signal

`src/next/get-session.ts`'s catch becomes:

```
err instanceof SessionStoreUnavailable
  -> logger.event('store.unavailable',      { sid, stage: 'get_session' })
otherwise
  -> logger.event('session.resolve_failed', { sid, stage: 'get_session' })
return null
```

`session.resolve_failed` joins the `AuthEventName` union in `src/ports/logger.ts`.

Every failure still logs exactly one line, so § 10.1's "the degrade must not be silent" holds.
`store.unavailable` now means the store, and nothing else.

Note the interaction with § 2.2: once a transient IdP failure degrades rather than throwing, it
no longer reaches this catch at all while the token is live. What reaches it is a
`RefreshRejected`, a hard-expired token under a transient failure, or a genuine store outage.

## 3. One route table (item 3)

### 3.1 The problem

`authRoutePaths` (`src/middleware.ts:63-65`) and `createAuthRoutes`'s dispatch
(`src/http/routes.ts:49-52`) are two independent lists. `tests/middleware.test.ts:76` proves every
DERIVED path is recognised, so a stale or renamed path reds. A route ADDED to the dispatch and
forgotten in `authRoutePaths` does not red, and its failure mode is the infinite redirect loop the
helper exists to prevent: `/auth/login` clears the session cookie, the identity provider's
redirect arrives with no cookie, middleware bounces it back to `/auth/login`, and the user loops
with no error anywhere.

### 3.2 The design

**New `src/http/route-table.ts`, importing nothing:**

```ts
export const AUTH_ROUTE_SUFFIXES = ['/auth/login', '/auth/callback', '/auth/logout', '/auth/logout/callback'] as const;
export type AuthRouteSuffix = (typeof AUTH_ROUTE_SUFFIXES)[number];
```

The module has no imports of its own, so it is reachable from the middleware entry point without
touching AC 4's import-graph guarantee. `src/middleware.ts` already imports `./http/cookies.js`,
so a second pure module under `http/` follows the established shape. The AC 4 assertion
(`tests/middleware.test.ts:112-114`) is a predicate over banned MODULES, not a strict file set, so
the new file needs no re-baselining there.

**`src/http/routes.ts`** derives its four named constants through a typed helper, so the dispatch
stays as readable as it is today while a renamed suffix fails typecheck:

```ts
const path = (suffix: AuthRouteSuffix): string => `${runtime.basePath}${suffix}`;
const loginPath = path('/auth/login');
// ...
```

**`src/middleware.ts`**'s `authRoutePaths` maps the same table over `basePath`.

Adding a suffix to the table therefore reaches `publicPaths` by construction. The direction that
stays possible is a suffix in the table with no dispatch arm — `publicPaths` becomes a superset,
which is the safe direction (a public path that serves 404 loops nothing).

### 3.3 The remaining hole, and the arm count

A hand-written dispatch arm that never touches the table still escapes. One structural test counts
`pathname ===` occurrences in `routes.ts` and asserts the count equals `AUTH_ROUTE_SUFFIXES.length`.
A fifth arm reds it.

This test parses source text, which is mildly brittle. It is accepted because it is the only thing
that can see an arm the table does not know about, and its failure message will name both counts
so a legitimate re-baseline is a one-line, reviewed edit.

## 4. A total record-shape policy (item 5)

### 4.1 Two holes the current guard does not cover

The Redis adapter treats an unparseable value as absent-and-deleted
(`src/adapters/redis-store.ts:154-172`). Reading the code found two values that defeat it. Both
are the failure the guard exists to prevent. **Both are to be proven by a failing test before the
fix lands** — they are read from the code, not measured yet.

**Hole 1 — a stored literal `null`.** `JSON.parse('null')` SUCCEEDS, so the parse guard never
fires. The next line reads `parsed.version` on `null`, throws a `TypeError`, and `#guarded`'s
catch-all converts it to `SessionStoreUnavailable`. That is the permanent sign-out loop, restored
by a value the guard was written to handle.

**Hole 2 — a body that parses to an object with `version: 1` and nothing else valid.**
`shouldRefresh(now, undefined, skewMs)` is `now >= NaN`, which is `false`, so `resolveSession`
returns the record as a LIVE session with `accessToken: undefined`. `toSessionView` then throws on
`rec.principal.roleGrants`, so a server component 500s. That is worse than a sign-out loop: the
package hands out a session it knows nothing about.

### 4.2 The design

`src/core/session.ts` gains a total predicate:

```ts
export function isSessionRecord(value: unknown): value is SessionRecord
```

It checks the fields `resolveSession` and `toSessionView` actually read: `version === 1`; `rev`,
`accessExpiresAt` and `absoluteExpiresAt` are finite numbers; `accessToken` is a string;
`refreshToken` is a string or absent; `idTokenClaims` is a non-null object; `principal` is a
non-null object whose `roleGrants` is an array.

Both adapters replace their standalone `version !== 1` check with it, so the policy is stated
once. `src/adapters/redis-store.ts` deletes the key and reports absent whenever the predicate
fails — closing the parse case, hole 1 and hole 2 under one rule.

The predicate is internal. It is NOT added to `src/server.ts`'s export list, so
`tests/structure/exports.test.ts`'s strict `package.json` assertion is untouched.

### 4.3 What else can poison a key

Reviewed and stated, so the answer is on the record rather than assumed:

- **A partial write** cannot happen. `SET` and the `SET_CAS` script both write a complete JSON
  string atomically.
- **A cross-namespace collision** cannot happen. The three key shapes are `pgs:sess:`, `pgs:lock:`
  and `pgs:txn:`, and a sid is 256 bits of randomness.
- **Another application sharing the Redis instance** can write anything to `pgs:sess:*`.
  `keyPrefix` is `''` today (`src/runtime.ts:140`). This is outside the package's control, and it
  is exactly the case § 4.2 makes survivable rather than fatal.
- **A `version` bump** is already handled, and stays handled by the same predicate.
- **The memory adapter** holds typed objects and never parses bytes, so only a caller violating
  the type could poison it. It adopts the predicate anyway, so one rule covers both adapters.

## 5. The four untested guards (item 4)

Each guard is correct today. None would red if deleted. That is the criterion every new test here
must meet.

| # | Guard | Site | Test | Reds on deletion because |
|---|---|---|---|---|
| 1 | `disableOfflineQueue: true` | `redis-store.ts:241` | `vi.mock('redis')`, assert the `createClient` options object | the flag IS the assertion |
| 2 | the silent `client.on('error', …)` | `redis-store.ts:254` | same mock: assert a listener is registered, emit `'error'`, assert no throw | no listener means no registration to assert |
| 3 | `getAuthRuntime`'s failure reset | `runtime.ts:179-184` | `vi.resetModules()` + dynamic import; a bad config rejects, then a good config resolves | without the reset the second call replays the cached rejection |
| 4 | the single CAS retry | `single-flight.ts:151-153` | a store whose `set` returns `false` exactly ONCE against unchanged state, then succeeds | today's test stubs `set` to fail forever, so deleting the retry still yields `null` and stays green |

Guard 3's test must use `vi.resetModules()` and a dynamic import, because `sharedRuntime` is
module-level state and `tests/runtime.test.ts:108` already memoises a success in the same file.
An isolated module instance keeps the two independent.

Guard 4's stub must leave the stored record at `fresh.rev` when it returns `false`, so
`resolveSession` reaches the `winner.rev === fresh.rev` branch — the retry — rather than the
"another writer owns it" branch. A second test keeps today's fail-forever case, which must still
delete the record and log `session.refresh.persist_failed`.

**Named residual.** The mock proves `disableOfflineQueue` is SET. It does not prove node-redis
then fails fast during a real outage. A container test that stops Redis mid-suite would prove it
and was rejected as slow and flake-prone. The behavioural claim in `redis-store.ts:237-240`'s
comment stays unproven.

## 6. Files

**Source (12, one of them new):**

| File | Change |
|---|---|
| `src/core/errors.ts` | add `RefreshRejected` |
| `src/adapters/oidc.ts` | classify `refresh()` failures onto `RefreshRejected` |
| `src/core/single-flight.ts` | degrade on a transient failure; add `degraded` to `session.refresh_failed` |
| `src/next/get-session.ts` | narrow `store.unavailable`; add `session.resolve_failed` |
| `src/ports/logger.ts` | add `session.resolve_failed` to `AuthEventName` |
| `src/http/route-table.ts` | NEW — `AUTH_ROUTE_SUFFIXES` and its type |
| `src/http/routes.ts` | derive the four paths from the table |
| `src/middleware.ts` | `authRoutePaths` derives from the table |
| `src/core/session.ts` | add `isSessionRecord` |
| `src/adapters/redis-store.ts` | apply the predicate; drop the standalone version check |
| `src/adapters/memory-store.ts` | apply the predicate |
| `src/server.ts` | export `RefreshRejected` |

**Tests (7):** `tests/adapters/oidc.test.ts`, `tests/core/single-flight.test.ts`,
`tests/next/get-session.test.ts`, `tests/adapters/redis-store.test.ts`,
`tests/adapters/memory-store.test.ts`, `tests/runtime.test.ts`, `tests/middleware.test.ts`.

No new dependency. No container test and no e2e test added.

## 7. Verification

- `pnpm exec vitest run` in `ts/packages/paigasus-auth` — every new test present, every existing
  test still green. Baseline is 234.
- Each § 5 test is verified by DELETING its guard and watching the test red, then restoring the
  guard. The restore is done by reverting the marked edit, never by `git checkout --`, which would
  also discard uncommitted work.
- `moon run paigasus-auth-ts:lint`, `:typecheck`, `:fmt`.
- The full graph as CI runs it, per CLAUDE.md, before pushing.

## 8. Residuals carried forward

1. `disableOfflineQueue`'s real-outage behaviour is unproven (§ 5).
2. The § 3.3 arm count parses source text and can rot if the dispatch is rewritten in a shape the
   regex does not match. It fails loudly rather than silently, because a count of 0 reds too.
3. Item 6's four points are untouched and need their own issue.
4. An unknown OAuth error code reads as transient (§ 2.2), so an identity provider inventing a
   definitive code this set does not list keeps a session alive for up to `skewMs`.
