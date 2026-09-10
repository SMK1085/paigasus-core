<!-- SPDX-License-Identifier: Apache-2.0 -->

# SMA-626 — `@paigasus/auth`: observability gaps and untested guards

**Status:** revision 2, after the adversarial challenge (2026-09-10)
**Issue:** [SMA-626](https://linear.app/smaschek/issue/SMA-626)
**Baseline:** `main` at `51a2d859` (PR #230, SMA-506). `pnpm exec vitest run` reports
234 tests in 22 files, all pass.
**Related:** SMA-506 (the package), SMA-511 (first Next integration), SMA-508 (merged as PR #229).

Revision 2 changes four designs. Section 9 lists what the challenge moved and why.

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
but the false store signal stays. Section 2.4 closes it.

### 1.2 Out of scope

Item 6 moves to a separate issue: a correlation id on `login.started`, the `Sec-Fetch-Mode`
absent-header decision, rate limiting on `/auth/login`, and the store contract's fencing
assertion. Rate limiting is an ingress concern the issue itself judged to sit outside this
package.

Mounting the package in a Next app belongs to SMA-511. The real grants snapshot belongs to
SMA-508, now merged.

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

### 2.2 What counts as definitive

**`DEFINITIVE = { invalid_grant }`, and nothing else.** This is revision 2's largest change.

Revision 1 also listed `invalid_client`, `unauthorized_client`, `invalid_scope` and
`invalid_request`. All four are per-DEPLOYMENT faults, not per-session ones: they hit every
session at the same instant, and none of them means a refresh token was revoked. Rotating the
OIDC client secret without updating `PAIGASUS_OIDC_CLIENT_SECRET` would sign out the whole fleet
at once, and the re-login they are sent to uses the same broken credential —
`authorizationCodeGrant` fails, `handleCallback` rejects with `code_exchange_failed`
(`src/http/routes.ts:205-207`), and `createAuthRouteHandler` maps that to HTTP 502
(`src/server.ts:70-71`). A recoverable misconfiguration would become a total outage.

Everything except `invalid_grant` is transient: a network failure, a timeout, an abort, a 5xx,
`server_error`, `temporarily_unavailable`, `invalid_client`, and any code this package does not
know. A transient classification keeps a session alive for at most `skewMs`, which is the bounded
side of the choice.

### 2.3 The design

**A new error type in `src/core/errors.ts`:**

```ts
/**
 * The identity provider REFUSED this refresh token, as opposed to failing to answer. The value is
 * the OAuth 2.0 error code, and the TYPE is the closed set this package admits — widening the set
 * is the same edit as widening what may be logged, which is what keeps the redaction claim and
 * the membership test one fact rather than two.
 */
export class RefreshRejected extends AuthError {
  readonly code = 'oidc_refresh_rejected';
  constructor(readonly oauthError: 'invalid_grant') {
    super(`oidc refresh rejected: ${oauthError}`);
  }
}
```

It lives in `core/`, not in `adapters/`. `core/single-flight.ts` must catch it, and `core` may not
import `adapters` — the adapter maps its library's error onto the core vocabulary, which is the
direction the hexagon already runs everywhere else in this package.

It is **NOT** exported from `src/server.ts`. No consumer can produce or observe one: `getSession`
swallows every failure into `null`, and `CreateAuthRuntimeDeps` exposes no OIDC override. An
export with no consumer is surface this package does not need. (`tests/structure/exports.test.ts:33`
asserts `package.json`'s exports map, not `server.ts`'s export list, so this decision is not
forced by that test — revision 1 gave the wrong reason for the right call.)

**Classification in `src/adapters/oidc.ts`'s `refresh()`. This must be MEASURED, not assumed.**

`oauth4webapi@3.8.8`'s `checkOAuthBodyError` calls `checkAuthenticationChallenges(response)`
BEFORE it parses the body (`node_modules/.pnpm/oauth4webapi@3.8.8/.../build/index.js:917-937`). A
token endpoint that answers with a `WWW-Authenticate` header — which RFC 6749 § 5.2 says it SHOULD
do for `invalid_client` — throws `WWWAuthenticateChallengeError`, which carries no `.error` field
at all. `openid-client@6.8.8` defaults to `ClientSecretPost` (`.../openid-client/build/index.js:483-486`),
which may suppress that header, but that is an assumption until it is measured.

The implementation therefore begins with a measurement task: drive a real `invalid_grant` through
`refresh()` and record which error class arrives and what it carries. The classifier is written
against the measured shape, and the measurement is recorded in the adapter comment. If
`invalid_grant` can arrive as something other than a `ResponseBodyError`, the classifier handles
both classes.

The classifier reads the error CODE only. It never holds the error object, its message, or a URL.

**Degrade in `src/core/single-flight.ts`:**

```
catch (err)
  rejected = err instanceof RefreshRejected
  liveUntil = Math.min(fresh.accessExpiresAt, fresh.absoluteExpiresAt)
  degraded  = !rejected && Date.now() < liveUntil

  logger.event('session.refresh_failed', { sid, reason: rejected ? 'rejected' : 'transient', degraded })

  if (degraded) return { ...fresh, refreshState: 'failed' }
  if (rejected) { await store.delete(sid); logger.event('session.deleted', { sid, reason: 'refresh_rejected' }) }
  throw err
```

Four points, each of which revision 1 got wrong.

**`reason` is required, not optional.** With `degraded` alone, `session.refresh_failed { degraded: false }`
fires for two different causes — a definitive rejection (benign: an administrator ended one
session) and a transient failure against an already hard-expired token (an outage that signs users
out). Those are the exact pair § 2.1 says an operator must separate, and § 2.4 gives both the same
second line. `reason` is what separates them. Three of the four combinations are reachable;
`reason: 'rejected'` with `degraded: true` is not, because a rejection never degrades.

**A rejected refresh DELETES the record.** `resolveSession` already deletes on `no_refresh_token`
(`:113-117`), on `absolute_expiry` (`:107-111`), and on a twice-failed persist (`:155-158`, whose
comment reads "delete rather than leave a record holding a refresh token the IdP has already
revoked"). Rethrowing without deleting would leave a revoked refresh token and a live access token
in Redis for the full `ttlMs`, and every later `getSession()` would re-take the lock and re-call
the token endpoint until then. That is the exposure § 2.1 names, left in place.

**The cap is re-checked.** `fresh.absoluteExpiresAt` is read at `:107`, before a `refresh()` call
that `src/runtime.ts:118-120` bounds at 2x `PAIGASUS_OIDC_HTTP_TIMEOUT_MS`. Testing only
`accessExpiresAt` is not enough: `handleCallback` sets the two independently
(`src/http/routes.ts:233-234`), and `accessExpiresAt` is clamped to the cap only on a refresh
write (`:141`). An identity provider whose `expires_in` exceeds
`PAIGASUS_SESSION_ABSOLUTE_TTL_SECONDS` therefore mints a first record with
`accessExpiresAt > absoluteExpiresAt`. **The lock-timeout branch at `:180-185` holds the same
hole and is fixed in the same way.**

**`refreshPending` becomes `refreshState`.** `ResolvedSession` becomes
`SessionRecord & { refreshState?: 'pending' | 'failed' }`. The timeout branch sets `'pending'`
("another holder is refreshing; the next request is fresh"). The degrade sets `'failed'` ("nobody
is refreshing; the next request fails the same way"). One boolean cannot carry both, and a
consumer that read it as "retry shortly" would hot-loop through an IdP outage. A repository-wide
search finds no consumer outside the type itself and one assertion at
`tests/core/single-flight.test.ts:163`, so this is the cheapest moment to split it.

The lock release in `finally` is untouched. Invariant 4 still holds: the degrade returns from
inside the `try`, so the `finally` still runs. Invariant 5 (write fencing) is untouched, because
the degrade performs no write.

### 2.4 Narrowing the store signal

`src/next/get-session.ts`'s catch becomes:

```
err instanceof SessionStoreUnavailable
  -> logger.event('store.unavailable',      { sid, stage: 'get_session' })
otherwise
  -> logger.event('session.resolve_failed', { sid, stage: 'get_session' })
return null
```

`session.resolve_failed` joins the `AuthEventName` union in `src/ports/logger.ts`, and the
"minimum event set" in `docs/superpowers/specs/2026-09-09-sma-506-auth-design.md:1027-1031` is
updated to match. Every failure still logs exactly one line, so § 10.1's "the degrade must not be
silent" holds.

**This narrowing tightens a port contract that is not written down.** `SessionStore` is exported
publicly (`src/server.ts:95`) and `src/ports/session-store.ts` never says an implementation must
signal failure with `SessionStoreUnavailable`. A store injected through
`CreateAuthRuntimeDeps.store` (`src/runtime.ts:55-59`), a decorator, or a future adapter that
throws anything else loses the `store.unavailable` signal. The existing test survives only because
its fixture throws the right class (`tests/next/get-session.test.ts:33-44`). So the requirement is
written into the `SessionStore` doc comment, and a test asserts that a store failing with a
different error class produces `session.resolve_failed` — a documented, deliberate outcome rather
than an accident.

**§ 2.4 depends on § 4 and must not land before it.** A stored literal `null` makes `#guarded`
mint a `SessionStoreUnavailable` against a perfectly healthy Redis
(`src/adapters/redis-store.ts:140-146` reached from `:168`), which this catch routes straight to
`store.unavailable`. Until § 4's predicate lands, "`store.unavailable` means the store" is false,
and the counter-example is on this branch.

## 3. One route table (item 3)

### 3.1 The problem

`authRoutePaths` (`src/middleware.ts:63-65`) and `createAuthRoutes`'s dispatch arms
(`src/http/routes.ts:59-82`) are two independent lists. `tests/middleware.test.ts:76` proves every
DERIVED path is recognised, so a stale or renamed path reds. A route ADDED to the dispatch and
forgotten in `authRoutePaths` does not red, and its failure mode is the infinite redirect loop the
helper exists to prevent: `/auth/login` clears the session cookie, the identity provider's
redirect arrives with no cookie, middleware bounces it back to `/auth/login`, and the user loops
with no error anywhere.

There is a **third** copy: `README.md:112-115` documents the same four routes in a table. It is
prose, nothing gates it, and it stays out of scope — noted here so "two lists" is not read as a
complete census.

### 3.2 The design: a table-driven dispatch

**New `src/http/route-table.ts`, importing nothing:**

```ts
export const AUTH_ROUTE_SUFFIXES = ['/auth/login', '/auth/callback', '/auth/logout', '/auth/logout/callback'] as const;
export type AuthRouteSuffix = (typeof AUTH_ROUTE_SUFFIXES)[number];
```

The module has no imports of its own, so it is reachable from the middleware entry point without
touching AC 4's import-graph guarantee. `src/middleware.ts` already imports `./http/cookies.js`,
so a second pure module under `http/` follows the established shape. The AC 4 assertion
(`tests/middleware.test.ts:112-114`) is a predicate over banned MODULES, not a strict file set, so
the new file needs no re-baselining there. `src/client.ts` does not import it, so AC 5 is
untouched.

**`src/http/routes.ts` replaces its four `if (pathname === …)` arms with a lookup.** A
`Record<AuthRouteSuffix, { method, handle }>` maps each suffix to its method and handler.
`createAuthRoutes` builds a `Map` from `AUTH_ROUTE_SUFFIXES` mapped over `runtime.basePath`, and
`handle()` does one lookup: no entry is a 404, a method mismatch is a 405, otherwise it delegates.

This is what revision 1 got wrong. Revision 1 kept the `if` arms and proposed counting
`pathname ===` occurrences in the source. That count is not a gate: `pathname == p`,
`pathname===p`, `p === pathname`, a `switch (pathname)`, an aliased variable, a `startsWith`, a
nested dispatch inside `handleLogin`, or an arm in a delegated file all defeat it, and any
`pathname ===` text in a COMMENT reds it falsely.

**The `Record` closes both directions in the type system.** A fifth suffix added to
`AUTH_ROUTE_SUFFIXES` with no handler fails typecheck (a missing `Record` key), and a handler for
a suffix not in the table fails typecheck (an excess key). Adding a route therefore reaches
`publicPaths` by construction, and `authRoutePaths` becomes `AUTH_ROUTE_SUFFIXES.map(...)` over
`basePath`.

**One structural backstop stays**, in a much stronger form than a count: assert that `routes.ts`
contains **no** direct comparison of `pathname` at all (`pathname` adjacent to `==`, in either
operand order). The expected occurrence count is zero, so any hand-written arm reds it. Residual,
stated: a comparison written some other way — a `switch`, an alias, a delegated file — still
escapes it. The `Record` is the primary control; this is the backstop.

The existing 405/404 cross-check at `tests/middleware.test.ts:76` stays and keeps covering the
stale direction.

## 4. A total record-shape policy (item 5)

### 4.1 Two holes the current guard does not cover

The Redis adapter treats an unparseable value as absent-and-deleted
(`src/adapters/redis-store.ts:154-172`). Reading the code found two values that defeat it.
**Both are to be proven by a failing test before the fix lands** — they are read from the code,
not measured yet.

**Hole 1 — a stored literal `null`.** `JSON.parse('null')` SUCCEEDS, so the parse guard never
fires. The next line reads `parsed.version` on `null`, throws a `TypeError`, and `#guarded`'s
catch-all converts it to `SessionStoreUnavailable`.

The consequence is a **false store-outage signal against a healthy Redis**, which is § 2.4's
argument, not a permanent sign-out loop. Revision 1 said "permanent" and that was wrong:
`handleLogin` deletes the presented sid unconditionally (`src/http/routes.ts:136-139`, the I6
fix), so the first `/auth/login` clears the poisoned key.

**Hole 2 — a body that parses to `{ version: 1 }` and nothing else.** Two NaN comparisons in
sequence let it through. `Date.now() >= rec.absoluteExpiresAt` at `src/core/single-flight.ts:90`
is `false` against `undefined`, so the record is not deleted; then
`shouldRefresh(now, undefined, skewMs)` at `:95` is `now >= NaN`, also `false`, so `resolveSession`
returns it as a LIVE session with `accessToken: undefined`. `toSessionView` then throws at
`src/core/session.ts:40` on `rec.idTokenClaims.email`, so a server component 500s.

**Hole 3 — a `principal` that is present but incomplete.** This is revision 2's addition, and it
is the worst of the three. `can()` **fails open**: `if (!session.grantsAvailable) return true;`
(`src/client.ts:67`). That is deliberate and correct — an unavailable grant set means UNKNOWN, not
DENIED, so `@paigasus/app-shell` does not render a console with no navigation. But it means a
record whose `principal` is `{ roleGrants: [] }` produces `grantsAvailable: undefined` through
`toSessionView` (`src/core/session.ts:55`), and **every browser-side `can()` check then returns
`true`**. § 4.3 states that any application sharing the Redis instance can write to `pgs:sess:*`.
The predicate is the only control on that path.

### 4.2 The design

`src/core/session.ts` gains a total predicate:

```ts
export function isSessionRecord(value: unknown): value is SessionRecord
```

It checks every field the read path actually consumes, which is wider than revision 1 said:

| Field | Requirement | Why |
|---|---|---|
| `version` | `=== 1` | replaces both adapters' standalone check |
| `rev`, `accessExpiresAt`, `absoluteExpiresAt` | finite numbers | hole 2's NaN comparisons |
| `accessToken` | string | `resolveSession` returns it as usable |
| `refreshToken` | string, or absent — **`null` is rejected explicitly** | `exactOptionalPropertyTypes`; a stored `null` is neither |
| `idTokenClaims` | non-null object with string `iss` and `sub` | `ports/principal-resolver.ts:44-45` declares both required |
| `principal.principalPrn` | string or `null` | `toSessionView` (`session.ts:45`) |
| `principal.grantsAvailable` | **boolean** | hole 3 — the fail-open gate |
| `principal.roleGrants` | array, every element an object with string `scopePrn` and `roleKey` | `toSessionView` spreads each element (`session.ts:54`) |

Both adapters replace their standalone `version !== 1` check with it, so the policy is stated
once. `src/adapters/redis-store.ts` deletes the key and reports absent whenever the predicate
fails.

**A predicate can drift from its type, and TypeScript will not say so.** A `value is SessionRecord`
predicate that under-checks compiles silently, and one that over-checks makes
`MemorySessionStore.get` return `null` for a valid record (`src/adapters/memory-store.ts:34-42`) —
which reads to a user as "I was logged out", not as a type error. The control is a table-driven
test: `isSessionRecord(makeRecord())` is true, and deleting each required field **one at a time**
makes it false. `makeRecord` already exists at `tests/store-contract.ts:6-18`.

The predicate is internal and is not exported from `src/server.ts`.

### 4.3 What else can poison a key

Reviewed and stated, so the answer is on the record rather than assumed:

- **A partial write** cannot happen. `SET` and the `SET_CAS` script both write a complete JSON
  string atomically.
- **A cross-namespace collision** cannot happen. The three key shapes are `pgs:sess:`, `pgs:lock:`
  and `pgs:txn:`, and a sid is 256 bits of randomness.
- **Another application sharing the Redis instance** can write anything to `pgs:sess:*`.
  `keyPrefix` is `''` today (`src/runtime.ts:140`). This is outside the package's control, and it
  is exactly the case § 4.2 makes survivable rather than fatal. Hole 3 is what that case looks
  like when it goes wrong.
- **A `version` bump** is already handled, and stays handled by the same predicate.
- **The memory adapter** holds typed objects and never parses bytes. It adopts the predicate
  anyway, so one rule covers both adapters — and § 4.2's drift test is what stops the predicate
  from silently deleting valid records there.

## 5. The four untested guards (item 4)

Each guard is correct today. None would red if deleted. That is the criterion every new test here
must meet, and **each test carries a comment recording the measured mutation result** — the guard
was deleted, the test red, the guard restored by reverting the marked edit (never by
`git checkout --`, which would also discard uncommitted work). The battery is re-run whole after
any later fix.

| # | Guard | Site | Test | Reds on deletion because |
|---|---|---|---|---|
| 1 | `disableOfflineQueue: true` | `redis-store.ts:241` | `vi.mock('redis')`, assert the `createClient` options object | the flag IS the assertion |
| 2 | the silent `client.on('error', …)` | `redis-store.ts:254` | see below | no listener means no handler to capture |
| 3 | `getAuthRuntime`'s failure reset | `runtime.ts:179-184` | `vi.resetModules()` ONCE, then two dynamic imports | without the reset the second call replays the cached rejection |
| 4 | the single CAS retry | `single-flight.ts:151-153` | a store whose `set` returns `false` exactly once against unchanged state | today's test stubs `set` to fail forever, so deleting the retry still yields `null` and stays green |

**Guard 2 asserts silence, not survival.** Revision 1 proposed "emit `'error'`, assert no throw",
which is vacuous: against a mock the test invokes `() => undefined`, which cannot throw. The
property the code claims is that the handler is **intentionally silent, because the node-redis
error object embeds the DSN** (`redis-store.ts:250-253`). Replacing `() => undefined` with
`(e) => console.error(e)` would keep revision 1's test green while leaking the Redis DSN on every
connection blip, breaking the absolute redaction rule. So the test captures the registered
handler, calls it with `new Error('connect ECONNREFUSED redis://user:pw@host:6379')`, and asserts
both that it does not throw and that **no console method and no injected logger recorded
anything**.

**Guard 3 has a vacuous mode of its own.** `vi.resetModules()` must run ONCE, before both dynamic
imports, so both calls reach the same fresh module instance. A reset between the two calls hands
the second call a module whose `sharedRuntime` is already `undefined`, and the test then passes
with `sharedRuntime = undefined` deleted from `src/runtime.ts:180`. The call count and its
position are part of the test's specification, not an implementation detail.

**Guard 4's stub is installed after the setup write.** Every existing fixture writes the record
with `await store.set('s', makeRecord(...), 60_000, null)` first, so a stub that fails "the first
`set` call" would break that setup. The stub returns `false` only for the call whose
`expectedRev === fresh.rev`, and it leaves the stored record at `fresh.rev` so `resolveSession`
reaches the retry branch rather than the "another writer owns it" branch. A second test keeps
today's fail-forever case, which must still delete the record and log
`session.refresh.persist_failed`.

**`vi.mock('redis')` needs a stated cast.** `client.on` is deliberately absent from the local
`RedisClient` interface (`redis-store.ts:104-110`), while `:254` calls it on the real node-redis
client type. The mock factory returns an object satisfying `RedisClient` plus `on`, cast once at
the factory's return, so `paigasus-auth-ts:typecheck` stays green without widening the production
interface.

**Named residual.** The mock proves `disableOfflineQueue` is SET. It does not prove node-redis
then fails fast during a real outage, and it cannot prove that a missing listener crashes the
process. A container test that stops Redis mid-suite would prove the first and was rejected as
slow and flake-prone. The behavioural claim in `redis-store.ts:237-240`'s comment stays unproven.

## 6. Files

**Source (13, one of them new):**

| File | Change |
|---|---|
| `src/core/errors.ts` | add `RefreshRejected` (internal) |
| `src/adapters/oidc.ts` | classify `refresh()` failures onto `RefreshRejected`, against a MEASURED error shape |
| `src/core/single-flight.ts` | degrade on transient; delete on rejected; `reason` + `degraded` fields; `refreshState`; cap re-check in BOTH branches |
| `src/next/get-session.ts` | narrow `store.unavailable`; add `session.resolve_failed` |
| `src/ports/logger.ts` | add `session.resolve_failed` to `AuthEventName` |
| `src/ports/session-store.ts` | document the `SessionStoreUnavailable` requirement |
| `src/http/route-table.ts` | NEW — `AUTH_ROUTE_SUFFIXES` and its type |
| `src/http/routes.ts` | table-driven dispatch through a `Record<AuthRouteSuffix, …>` |
| `src/middleware.ts` | `authRoutePaths` derives from the table |
| `src/core/session.ts` | add `isSessionRecord` |
| `src/adapters/redis-store.ts` | apply the predicate; drop the standalone version check |
| `src/adapters/memory-store.ts` | apply the predicate |
| `docs/superpowers/specs/2026-09-09-sma-506-auth-design.md` | § 12's minimum event set gains `session.resolve_failed` |

`src/server.ts` is NOT changed: `RefreshRejected` stays internal (§ 2.3).

**Tests (8):** `tests/adapters/oidc.test.ts`, `tests/core/single-flight.test.ts`,
`tests/next/get-session.test.ts`, `tests/adapters/redis-store.test.ts`,
`tests/adapters/memory-store.test.ts`, `tests/core/session.test.ts`, `tests/runtime.test.ts`,
`tests/middleware.test.ts`.

No new dependency. No container test and no e2e test added.

## 7. Verification

**Three existing assertions are deliberate re-baselines, not regressions.** An implementer who
reads a green baseline as the acceptance criterion would misread these three reds:

| Assertion | Why it changes |
|---|---|
| `tests/core/single-flight.test.ts:219` | `toContainEqual` is a recursive equality; `session.refresh_failed` gains `reason` and `degraded` |
| `tests/next/get-session.test.ts:165` | same event, same reason |
| `tests/core/single-flight.test.ts:163` | `refreshPending` becomes `refreshState: 'pending'` |

Everything else must stay green. The baseline is `Test Files 22 passed (22)` /
`Tests 234 passed (234)`, measured with `pnpm exec vitest run` in
`ts/packages/paigasus-auth`. (`tests/client.test.tsx` is the 22nd file; `tests/containers/**` and
`tests/e2e/**` are excluded by `vitest.config.ts`.)

- `pnpm exec vitest run` — the three re-baselines updated, every other existing test green, every
  new test present.
- Each § 5 test verified by DELETING its guard and watching it red, then restoring the guard.
  The result is written into a comment beside the test.
- `moon run paigasus-auth-ts:lint`, `:typecheck`, `:fmt`.
- The full graph as CI runs it, per CLAUDE.md, before pushing.

## 8. Residuals carried forward

1. `disableOfflineQueue`'s real-outage behaviour is unproven, and so is the claim that a missing
   `'error'` listener crashes the process (§ 5).
2. § 3.2's structural backstop asserts zero direct `pathname` comparisons in `routes.ts`. A
   comparison written another way — a `switch`, an alias, a delegated file — escapes it. The
   `Record` is the primary control.
3. Item 6's four points are untouched and need their own issue.
4. An unknown OAuth error code reads as transient (§ 2.2), so an identity provider that invents a
   revocation code other than `invalid_grant` keeps a session alive for up to `skewMs`.
5. **An IdP outage produces one token-endpoint call per `getSession()` per session.** Each call
   takes the lock, fails, releases, and degrades. This is NOT a regression — today's code makes
   exactly the same call and then signs the user out — but the degrade makes the traffic
   long-lived rather than one-shot. A refresh backoff is out of scope and belongs with item 6's
   rate-limiting question.
6. **Nothing consumes `reason`, `degraded`, or `session.resolve_failed` yet.** `AuthLogger` is a
   port; the application supplies the adapter, and the first one lands with SMA-511. Until then
   these events are structure, not an operator-visible signal.
7. The `README.md:112-115` route table is a third, ungated copy of the route list (§ 3.1).

## 9. What the adversarial challenge changed

| Finding | Verdict | Change |
|---|---|---|
| BLOCKER — the DEFINITIVE set turns a deployment fault into a fleet-wide sign-out | justified | `DEFINITIVE = { invalid_grant }` only (§ 2.2) |
| BLOCKER — `degraded` alone re-creates the conflation § 2 exists to remove | justified | added `reason: 'rejected' \| 'transient'` (§ 2.3) |
| BLOCKER — the predicate omits `grantsAvailable`, which gates a fail-open `can()` | justified, verified at `src/client.ts:67` | the § 4.2 field table, and hole 3 (§ 4.1) |
| MAJOR — a rejected refresh leaves the dead record in the store | justified | delete + `session.deleted { reason: 'refresh_rejected' }` (§ 2.3) |
| MAJOR — `refreshPending` would carry two opposite meanings | justified | `refreshState: 'pending' \| 'failed'` (§ 2.3) |
| MAJOR — the degrade does not re-check the absolute cap | justified | `Math.min(...)`, in BOTH branches (§ 2.3) |
| MAJOR — § 2.4 tightens an undocumented port contract | justified | documented in `ports/session-store.ts` + a test (§ 2.4) |
| MAJOR — § 2.4's headline claim is false while hole 1 is open | justified | ordering requirement stated (§ 2.4) |
| MAJOR — "every existing test still green" is false | justified | the three re-baselines named (§ 7) |
| MAJOR — the `pathname ===` arm count is not a gate | justified | table-driven dispatch; the count replaced by a zero-occurrence backstop (§ 3.2) |
| MAJOR — guard 2's behavioural half is vacuous and asserts the wrong property | justified | assert SILENCE against a DSN-bearing error (§ 5) |
| MAJOR — predicate/type drift has no control | justified | the field-deletion table test (§ 4.2) |
| MAJOR — `oauthError: string` leaves redaction to the call site | justified | typed as the admitted literal (§ 2.3) |
| QUESTION — is `ResponseBodyError` measured, or assumed? | justified, and important | a measurement task precedes the classifier (§ 2.3) |
| QUESTION — what bounds token-endpoint traffic during an outage? | answered | residual 5 |
| QUESTION — who consumes the new events? | answered | residual 6 |
| QUESTION — what can a consumer do with `RefreshRejected`? | answered: nothing | kept internal (§ 2.3) |
| QUESTION — where is the mutation evidence recorded? | answered | a comment beside each test (§ 5) |
| MINORs — hole 1's severity, hole 2's throw site, § 3.1's citation, the README copy, the design-doc event set, guard 4's stub ordering, guard 3's reset position, the `vi.mock` cast, § 4.2's wrong reason, `refreshToken: null` | justified | all folded in |
| MINOR — "the baseline count is unverifiable; it is 21 files, not 22" | **REJECTED** | measured: `vitest run` reports 22 files / 234 tests. The count omitted `tests/client.test.tsx`, which `include`'s `tests/**/*.test.tsx` pattern selects. |
