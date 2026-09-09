<!-- SPDX-License-Identifier: Apache-2.0 -->

# SMA-506 — `@paigasus/auth`: OIDC relying party and Redis-backed session

**Issue:** [SMA-506](https://linear.app/smaschek/issue/SMA-506)
**ADR:** ADR-0017 (Console topology & session ownership); constrained by ADR-0015 (IAM is a pure resource server)
**Design source:** Frontend Architecture Scoping, §§ 2, 6, 7, 8
**Date:** 2026-09-09
**Revision:** 2 — after the adversarial spec challenge. § 20 records what changed.

**Blocked by:** SMA-502 (`@paigasus/next-config`), landed as `e30cd2d2`.
**Blocks:** SMA-510 (`@paigasus/app-shell`).
**Related:** SMA-508 (`@paigasus/sdk`) owns the deferred half of the session — § 18.
**Branch base:** `939281e3` (after SMA-503 `@paigasus/ui`).

---

## 1. Problem

IAM is a pure resource server. Per ADR-0015 it validates externally-issued access
tokens, performs no authorization-code flow, stores no passwords, and has no
concept of a session. Nothing in the repository runs a login flow.

The frontend tier must own the entire OIDC login flow and the session. This
package is that owner.

## 2. The principle this package encodes

**The browser holds an opaque session id and nothing else.**

Every other decision follows. Tokens live server-side, so the session needs a
server-side store. A server-side store makes real logout possible. Two zones
sharing one store makes a refresh race possible, so the refresh needs a lock. A
client bundle that could import the server surface would break the premise, so
the split is structural rather than a review convention.

## 3. Scope

A new package `ts/packages/paigasus-auth` (`@paigasus/auth`), `private: true`
and source-only. Plus edits to `@paigasus/next-config`'s eslint preset and its
boundary test, and CI registration for one new task name.

Every new source file carries an SPDX header per CLAUDE.md.

### 3.1 Layout

```
ts/packages/paigasus-auth/
  package.json          exports "./server", "./client", "./middleware" — NO "."
  moon.yml              id: paigasus-auth-ts, layer: library; adds test-e2e
  tsconfig.json         the ui/next-config shape — § 3.2
  vitest.config.ts      node env; ssr.resolve.conditions for server-only
  playwright.config.ts
  src/
    server.ts           the /server entry. `import 'server-only'` at the top.
    client.ts           the /client entry. React only.
    middleware.ts       the /middleware entry. Web APIs only.
    core/
      refresh-policy.ts  shouldRefresh() — pure, total
      return-to.ts       validateReturnTo() — pure, total
      session.ts         SessionRecord, SESSION_VIEW_KEYS, toSessionView()
      single-flight.ts   the refresh algorithm (§ 8.3)
      ids.ts             session id, lock token, transaction id
      errors.ts          the error taxonomy (§ 10)
    ports/
      session-store.ts      SessionStore
      principal-resolver.ts PrincipalResolver
      logger.ts             AuthLogger — § 12
    adapters/
      redis-store.ts
      memory-store.ts
      claims-resolver.ts
      oidc.ts           openid-client configuration and calls
      noop-logger.ts
    http/
      routes.ts         createAuthRoutes() — (Request) => Response
      cookies.ts
    next/
      get-session.ts    getSession(), requireSession()
    runtime.ts          createAuthRuntime() — the composition root, § 6.8
    config.ts           the zod raw shape this package owns
  tests/
```

`src/client.ts` is a single file, not a directory. § 5.2's globs are written so
that stays true without breaking if it later becomes one.

### 3.2 The tsconfig shape

Revision 1 said "the sdk/ui shape (`rootDir`/`outDir`/`noEmit`)". That names two
**incompatible** shapes and picks the wrong one.

`ts/packages/paigasus-sdk/tsconfig.json` has `rootDir`, `outDir` and
`include: ["src/**/*"]`. `ts/packages/paigasus-ui/tsconfig.json` has neither
`rootDir` nor `outDir`, and includes `tests/**/*` and `vitest.config.ts` — with a
comment stating the reason: `ts:lint` runs `eslint .` with `projectService: true`
over the whole tree, and **a file in no program is a fatal parse error there**.

This package takes the **ui / next-config shape**:

- no `rootDir` (it rejects files outside `src/` and is inert under `noEmit`),
- `include: ["src/**/*", "tests/**/*", "vitest.config.ts", "playwright.config.ts"]`.

Taking the sdk shape would leave `tests/`, `vitest.config.ts` and
`playwright.config.ts` outside the program and red `ts:lint`.

## 4. Package shape and the export surface

### 4.1 Three subpath exports and no root export

```json
"exports": {
  "./server":     "./src/server.ts",
  "./client":     "./src/client.ts",
  "./middleware": "./src/middleware.ts"
}
```

The absent `"."` is load-bearing. `@paigasus/next-config`'s existing
`paigasus/boundaries/app-shell` rule denies `@paigasus/auth/server` to
client-reachable packages. A root export re-exporting the server surface would
let `import { getSession } from '@paigasus/auth'` walk around that rule. With no
`"."`, the bare specifier does not resolve at all.

**A future contributor adding `"."` re-opens the hole.** § 11.5 pins the export
key set by strict equality so that edit reds a test.

### 4.2 Why `./middleware` is a separate entry

Middleware runs in the edge runtime. Next sets the `react-server` condition for
the middleware layer, so `import 'server-only'` resolves to its own `empty.js`
there and is a **no-op**. Measured and recorded in
`ts/packages/paigasus-next-config/src/runtime.ts:10-17` (SMA-502 measurement M6).

So `server-only` cannot keep the server surface out of middleware. A separate
entry point can. `src/middleware.ts` imports the cookie name and standard Web
APIs, and nothing else.

**This proves AC 4 for this package's own entry, not for a consumer.** Nothing
here stops an app's own `middleware.ts` from importing `@paigasus/auth/server`
directly. That gap is closed separately, by a boundary block scoped to
`apps/*/middleware.*` — § 5.2, block 3. Without it AC 4 is only half enforced.

### 4.3 Four layers enforcing AC 5

1. **Module resolution.** No `"."` export. `./client` and `./server` are separate
   graphs.
2. **`import 'server-only'`** at the top of `src/server.ts` — the client-bundle
   door only; § 4.2 states what it does not cover.
3. **The eslint boundary blocks** (§ 5).
4. **Import-graph and type tests** (§ 11.5).

Layers 1 and 2 fail at build time, 3 at lint, 4 at test. Layer 4 catches a *new*
file added later.

## 5. The eslint boundary blocks

### 5.1 Where they live

Added to `ts/packages/paigasus-next-config/src/eslint.mjs`, not exported from
`@paigasus/auth`. The value of that file is that the whole § 6 dependency graph
is readable in one place; splitting it scatters the graph. The cost is that this
issue edits another package's source and its tests.

### 5.2 The globs must derive a valid scope key

**This is the finding that most changes the work.**
`ts/packages/paigasus-next-config/tests/boundaries.test.ts:115-131` derives each
rule's scope as `entry.files[0].split('/**')[0]` and asserts a **two-way**
pairing against `BOUNDARY_SCOPES`. `expectedPackageName` at `:38-41` then maps
`packages/paigasus-<x>` to `@paigasus/<x>`.

So a rule whose `files[0]` is `packages/paigasus-auth/src/client/**/*.{…}`
derives the key `packages/paigasus-auth/src/client`, the reverse loop fails, and
the "obvious fix" of adding that key makes the liveness test hunt for a package
named `@paigasus/auth/src/client`, which can never exist.

**The fix is to order `files` so `files[0]` splits to `packages/paigasus-auth`.**
`split('/**')` cuts at the *first* `/**`, so any glob beginning
`packages/paigasus-auth/**` derives the package root:

```js
{
  name: 'paigasus/boundaries/auth-client',
  files: [
    'packages/paigasus-auth/**/client.ts',            // derives packages/paigasus-auth
    'packages/paigasus-auth/src/client/**/*.{ts,tsx,mts,cts,js,jsx,mjs,cjs}',
  ],
  rules: restrict([
    { group: ['openid-client', 'redis', 'server-only',
              'node:*', 'crypto', 'fs', 'net', 'tls', 'http', 'https',
              './adapters/**', './core/**', './ports/**', './http/**', './next/**',
              './runtime', './config',
              '../adapters/**', '../core/**', '../ports/**', '../http/**', '../next/**'],
      message: '@paigasus/auth/client is React-only and must never reach the server surface (AC 5).' },
  ]),
}
```

Two corrections inside that group, both from the challenge:

- **The `./` forms are the ones that matter.** `src/client.ts` reaches a sibling
  directory as `'./adapters/redis-store'`, not `'../…'`. `no-restricted-imports`
  matches the specifier **as written**, so a `../`-only group asserts nothing
  about the single file the rule exists to protect.
- **`'node:*'` does not match a bare builtin.** `import 'crypto'` and
  `import 'fs'` are unprefixed and need their own entries.

Block 2 scopes `packages/paigasus-auth/**/middleware.ts` and denies the store,
the resolver and `openid-client`.

Block 3 is the AC 4 half § 4.2 cannot reach:

```js
{
  name: 'paigasus/boundaries/app-middleware',
  files: ['apps/*/middleware.{ts,js,mts,cts,mjs,cjs}'],
  rules: restrict([
    { group: ['@paigasus/auth/server', '@paigasus/auth/server/**', '@paigasus/sdk', '@paigasus/sdk/**'],
      message: 'Next middleware must do cookie-presence checks only (ADR-0017 decision 7, CVE-2025-29927). Resolve the session in a server component or route handler.' },
  ]),
}
```

`files[0]` here is `apps/*/middleware.{…}`, which contains no `/**` and derives
itself. `expectedPackageName` returns `undefined` for a scope outside
`packages/`, so it needs a `BOUNDARY_SCOPES` entry keyed by that exact string.

**`boundaries.test.ts` needs new rows too** — DENIED rows using `./`-relative
specifiers from `packages/paigasus-auth/src/client.ts`, and a DENIED row for
`apps/paigasus-console/middleware.ts` importing `@paigasus/auth/server`. A rule
with no row asserts nothing.

### 5.3 Three stale issue references corrected

`eslint.mjs` carries a two-way swap of issue numbers across **three** sites.
Confirmed against Linear:

- **SMA-506** — this issue, `@paigasus/auth`.
- **SMA-508** — `ts: @paigasus/sdk — Connect-ES gRPC clients and error mapping`.
- **SMA-510** — `ts: @paigasus/app-shell — header, navigation and cross-zone linking`.

| Site | Says | Should say |
|---|---|---|
| `eslint.mjs:36` — "the app-shell BLOCK is INERT until …" | SMA-506 | **SMA-510** |
| `eslint.mjs:39` — "gains nothing when … lands" | SMA-508 | **SMA-506** |
| `eslint.mjs:53` — `BOUNDARY_SCOPES['packages/paigasus-app-shell']` | SMA-506 | **SMA-510** |

The third is most easily missed: it reads as data rather than prose, and it is
the string a future reader will trust, since it sits beside the glob it
describes. Left alone, both app-shell sites become false the moment this branch
merges.

This change corrects all three, adds the new scope keys, and updates the
comment's claim that there is "no separate auth scope", which § 5.2 makes untrue.

## 6. Configuration

### 6.1 The shape goes through `defineRuntimeConfig`; the cross-field rules do not

`@paigasus/auth/server` exports a zod `ZodRawShape`; the app composes it into
`defineRuntimeConfig(extraShape)`. That is the seam `next-config`'s own docstring
names (`runtime.ts:189-192`).

**Revision 1 claimed more than that seam can deliver.** Verified:
`defineRuntimeConfig` builds `z.object({ ...coreEnvShape, ...extra })`
(`runtime.ts:201`) and offers **no refinement hook**, and it **throws** if the
extra shape declares `PAIGASUS_ZONE` or `PAIGASUS_ZONES` (`runtime.ts:195-199`).
A `ZodRawShape` is a flat map of per-key schemas. So neither of these is
expressible there:

- "`PAIGASUS_SESSION_REDIS_URL` is required when `PAIGASUS_SESSION_STORE=redis`"
  — a cross-field rule.
- The derived redirect URI (§ 6.4), which needs the two zone keys this package
  is forbidden to declare.

**Resolution: the shape declares per-key types; `createAuthRuntime(cfg)` owns
every cross-field rule and every derivation** (§ 6.8). It receives the *parsed*
composed config, so it reads the zone keys from the result rather than declaring
them, and it runs at first request because `getRuntimeConfig()` does. Fail-closed
timing is preserved.

The rejected alternative was extending `defineRuntimeConfig` with a `refine`
callback. That is a change to another package's public API to serve one consumer,
and it would let any package inject a message into the error path — which is
exactly what `describeIssues` (§ 6.9) exists to prevent.

### 6.2 Variables this package owns

| Variable | Required | Default |
|---|---|---|
| `PAIGASUS_OIDC_ISSUER` | yes | — |
| `PAIGASUS_OIDC_CLIENT_ID` | yes | — |
| `PAIGASUS_OIDC_CLIENT_SECRET` | yes | — |
| `PAIGASUS_PUBLIC_ORIGIN` | yes | — |
| `PAIGASUS_OIDC_REDIRECT_URI` | no | derived, § 6.4 |
| `PAIGASUS_OIDC_POST_LOGOUT_REDIRECT_URI` | no | derived, § 6.4 |
| `PAIGASUS_OIDC_SCOPES` | no | `openid profile email offline_access` |
| `PAIGASUS_OIDC_CLOCK_TOLERANCE_SECONDS` | no | `30` |
| `PAIGASUS_OIDC_HTTP_TIMEOUT_MS` | no | `3500` |
| `PAIGASUS_SESSION_STORE` | yes | — (`redis` \| `memory`) |
| `PAIGASUS_SESSION_REDIS_URL` | when `redis` | — |
| `PAIGASUS_SESSION_REDIS_TIMEOUT_MS` | no | `1000` |
| `PAIGASUS_SESSION_TTL_SECONDS` | no | `28800` |
| `PAIGASUS_SESSION_ABSOLUTE_TTL_SECONDS` | no | `86400` |
| `PAIGASUS_SESSION_REFRESH_SKEW_SECONDS` | no | `30` |
| `PAIGASUS_SESSION_LOCK_TTL_MS` | no | `10000` — see below |
| `PAIGASUS_SESSION_LOCK_WAIT_MS` | no | `3000` |

**The lock TTL default changed during implementation, and the timing invariant
changed with it.** Revision 2 specified `5000` and the single-timeout bound
`httpTimeout < lockTtl`. Enabling `enableNonRepudiationChecks` (§ 11.3) makes one
`oidc.refresh()` into **two** sequential bounded calls — the token endpoint, then
JWKS — so that bound no longer bounds the refresh against its lock. With a
3500 ms timeout, a cold-cache refresh can take ~7000 ms while holding a 5000 ms
lock, which is exactly the overlap § 8.4 exists to prevent.

The implemented invariant is therefore **`2 × httpTimeout < lockTtl`**, asserted
at startup by `createAuthRuntime`, and the default rises to `10000` so the
shipped configuration satisfies it (3500 × 2 = 7000 < 10000). The `rev` fence
means the old bound degraded to a duplicate refresh rather than a stale write —
but it defeated a guarantee § 8.4 spends a whole subsection establishing.

### 6.3 The client secret is required

A BFF is a confidential client by definition. Something already trusted to hold
refresh tokens in Redis can hold a client secret. Making it optional would admit
a strictly weaker deployment for no gain. PKCE is used **in addition**, always.

### 6.4 Derived URIs

`redirect_uri` defaults to `${PAIGASUS_PUBLIC_ORIGIN}${basePath}/auth/callback`
and `post_logout_redirect_uri` to `${PAIGASUS_PUBLIC_ORIGIN}${basePath}/`, with
`basePath` read from the parsed zone map in `createAuthRuntime`.

Deriving means the mounted route and the redirect cannot disagree with each
other. It does not guarantee agreement with what the operator registered at the
IdP, so both are overridable, and both derived values are exposed for logging.

### 6.5 `offline_access` is in the default scope set

Revision 1 defaulted to `openid profile email`. Keycloak issues a refresh token
for the code flow without it, so the § 11.7 fixture would have passed — and
Entra ID and others would not. § 8.3 then takes
`if no refreshToken -> delete; return null` and logs the user out at **every**
access-token expiry, which on some IdPs is five minutes. AC 2 would be
unreachable in exactly the deployments the default was meant to serve.

`createAuthRuntime` additionally **fails loudly on the first login** if no
refresh token comes back, rather than silently at the first expiry.

### 6.6 The store backend is explicit, and `memory` is honest about itself

`PAIGASUS_SESSION_STORE` is a required enum. Presence-based selection would let a
typo silently downgrade production to an in-memory store.

**`memory` is single-process only, and revision 1 did not say so.** § 2 says "two
zones sharing one store", but each zone is a separate Next process. Under
`memory`: a user logging in on zone A is anonymous on zone B, and the § 8.3 lock
is per-process, so **AC 2's guarantee does not hold across processes**.

Therefore `memory` is documented as development and single-process test use only,
and `createAuthRuntime` **refuses** `memory` when the parsed zone map declares
more than one zone. A silent wrong answer becomes a startup error.

### 6.7 No CA-bundle knob; the cookie names are constants

CLAUDE.md documents four CA-bundle knobs with two incompatible semantics. This
package adds none: `NODE_EXTRA_CA_CERTS` already **adds** to the trust store and
needs no code. Playwright uses `ignoreHTTPSErrors` for the browser side.

Cookie names are constants (§ 9.3), not configuration. All zones share one cookie
on one origin, so a knob invites exactly one drift.

### 6.8 `createAuthRuntime` is the composition root

```ts
createAuthRuntime(cfg: ComposedConfig, deps?: {
  store?: SessionStore;
  resolver?: PrincipalResolver;
  logger?: AuthLogger;
}): AuthRuntime
```

It owns: cross-field validation (§ 6.1), URI derivation (§ 6.4), the
multi-zone/`memory` refusal (§ 6.6), adapter selection, and `openid-client`
discovery. `createAuthRoutes`, `getSession` and the middleware factory all take
the resulting `AuthRuntime`.

**Discovery is performed once per process, at first use, and cached.** JWKS
rotation is handled by `openid-client`'s own keystore, not by re-running
discovery.

### 6.9 What the `defineRuntimeConfig` seam does *not* give us

`describeIssues` (`runtime.ts:136-145`) renders a zod issue's `message` only when
the issue is `custom` **and** its path root is in `OWNED_KEYS`
(`PAIGASUS_ZONE`, `PAIGASUS_ZONES`). Every other issue renders as
`<key>: <code>`.

So a misconfigured `PAIGASUS_OIDC_ISSUER` produces `PAIGASUS_OIDC_ISSUER:
invalid_type` with no guidance — for all seventeen variables above.

**That filter is deliberate and correct**: it is what stops a package-authored
`.refine()` message leaking a secret into a log. Revision 1 claimed this package
"inherits" good behaviour without noting the cost.

**Accepted, not worked around.** The remedy is documentation, not a mechanism:
the variable names are self-describing, and the package README carries a table of
each variable with its expected form. Widening the trusted-message allowlist
would re-open the leak this repo already closed.

## 7. Failure policy

Revision 1 had none. ADR-0017 names Redis-on-the-login-path as a real new
coupling, and revision 1 did not answer it.

### 7.1 Every outbound call has a timeout

| Call | Timeout | On expiry |
|---|---|---|
| OIDC discovery | `PAIGASUS_OIDC_HTTP_TIMEOUT_MS` | login fails, 503 page |
| token exchange | same | callback fails, § 10 error page |
| refresh | same, and **strictly below `LOCK_TTL_MS`** | § 8.4 |
| revocation | same | logged, logout continues |
| Redis command | `PAIGASUS_SESSION_REDIS_TIMEOUT_MS` | § 7.2 |

**The refresh timeout must be below the lock TTL.** § 8.4 invariant 5 depends on
it, and it is stated here as a configuration invariant that
`createAuthRuntime` asserts at startup.

### 7.2 Redis unavailability

node-redis v6's default reconnect strategy retries indefinitely and **queues
commands**, so an outage becomes hung requests rather than fast failures — every
page in the console stalls instead of erroring.

The adapter therefore sets `disableOfflineQueue: true` and a bounded reconnect
strategy. A store call that cannot reach Redis fails fast and raises
`SessionStoreUnavailable`.

| When | Behaviour |
|---|---|
| at login | `/auth/login` returns 503 with a retry affordance; no partial state |
| mid-session, `get` fails | treated as **no session** -> redirect to login, not a 500 |
| mid-session, `set` fails after a successful refresh | § 8.4 |

## 8. The session

### 8.1 The record and the view

```ts
interface SessionRecord {
  version: 1;
  rev: number;                     // fencing counter — § 8.4
  accessToken: string;
  refreshToken?: string;
  accessExpiresAt: number;         // epoch ms
  absoluteExpiresAt: number;       // loginTime + ABSOLUTE_TTL; never extended
  idTokenClaims: IdTokenClaims;
  principal: ResolvedPrincipal;
}

export const SESSION_VIEW_KEYS = [
  'principalPrn', 'displayName', 'email', 'grants', 'grantsAvailable',
] as const;

interface SessionView {            // everything /client may ever see
  principalPrn: string | null;
  displayName: string | null;
  email: string | null;
  grants: RoleGrantRef[];
  grantsAvailable: boolean;        // § 13
}
```

`SESSION_VIEW_KEYS` is a runtime `as const` tuple and `SessionView` is derived
from it. Revision 1 said "strict key-set equality on `SessionView`", which is not
implementable — a TypeScript interface has no runtime key set. The
`PUBLIC_CONFIG_KEYS` pattern it cited works precisely because `runtime.ts:29`
exports a runtime tuple.

**`absoluteExpiresAt` is `loginTime + PAIGASUS_SESSION_ABSOLUTE_TTL_SECONDS` and
is never extended by a refresh.** A refresh clamps the new `accessExpiresAt` to
it. `PAIGASUS_SESSION_TTL_SECONDS` is the *idle* TTL written to the store on each
write; the two are different quantities and revision 1 conflated them.

**A `version` mismatch on `get` returns `null`** — the record is treated as
absent and deleted. The operational consequence, which revision 1 omitted: **the
deploy that bumps `version` logs out every active user.** There is no migration
path by design; the alternative is deserializing a record into a wrong-typed
object. This belongs in the release note for any such change.

### 8.2 The store port

```ts
interface SessionStore {
  get(sid: string): Promise<SessionRecord | null>;
  set(sid: string, rec: SessionRecord, ttlMs: number, expectedRev: number): Promise<boolean>;
  delete(sid: string): Promise<void>;

  tryAcquireLock(sid: string, token: string, ttlMs: number): Promise<boolean>;
  releaseLock(sid: string, token: string): Promise<void>;   // compare-and-delete

  putTransaction(txnId: string, tx: LoginTransaction, ttlMs: number): Promise<void>;
  takeTransaction(txnId: string): Promise<LoginTransaction | null>;  // atomic
}
```

`set` is a **compare-and-set on `rev`**, returning `false` when the stored `rev`
has moved. § 8.4 explains why. On Redis it is a Lua script; in memory it is a
synchronous check.

**The single-flight algorithm is not in the store.** The store exposes
primitives; the policy lives once in `core/single-flight.ts`, so AC 2's
correctness is written and tested once and both adapters inherit it.

`takeTransaction` is atomic get-and-delete (`GETDEL` on Redis) — that is what
makes a transaction single-use.

Keys: `pgs:sess:`, `pgs:lock:`, `pgs:txn:`.

### 8.3 Single-flight refresh — AC 2

```
getSession(sid):
  rec <- store.get(sid);                         if none -> return null
  if now() >= rec.absoluteExpiresAt -> store.delete(sid); return null
  if not shouldRefresh(now(), rec.accessExpiresAt, skew) -> return rec

  lockToken <- random();  deadline <- now() + lockWaitMs
  loop:
    if store.tryAcquireLock(sid, lockToken, lockTtlMs):
      try:
        fresh <- store.get(sid)
        if fresh is none -> return null                      # concurrent logout
        if not shouldRefresh(now(), fresh.accessExpiresAt, skew) -> return fresh
        if no fresh.refreshToken -> store.delete(sid); return null
        tokens <- oidc.refresh(fresh.refreshToken)           # timeout < lockTtlMs
        next   <- merge(fresh, tokens, rev = fresh.rev + 1)
        ok <- store.set(sid, next, ttl, expectedRev = fresh.rev)
        if not ok -> return store.get(sid)                   # someone fenced us
        return next
      finally: store.releaseLock(sid, lockToken)
    if now() >= deadline -> return onRefreshTimeout(rec)      # § 8.5
    sleep(backoffWithJitter())
    reread <- store.get(sid)
    if reread is none -> return null
    if not shouldRefresh(now(), reread.accessExpiresAt, skew) -> return reread
```

Three corrections to revision 1's pseudocode, all from the challenge and all
real:

- **`now()` is re-read every iteration.** Revision 1 bound it once before the
  loop, which makes `now >= deadline` never true — the waiter loops until the
  process dies. § 8.6 corrects the wording that caused it.
- **`fresh` is null-guarded.** A concurrent `POST /auth/logout` between the first
  `get` and the lock acquisition made revision 1 throw a `TypeError` inside the
  `try`.
- **The write is fenced.** See § 8.4.

### 8.4 Why the write is fenced — invariant 5 was false

Revision 1 claimed "there is no window in which the stored record holds a revoked
token." That was **false in two independent ways**, and both are now handled.

**(a) Lock TTL expiry.** If `oidc.refresh` exceeds `LOCK_TTL_MS`, holder B
acquires the lock, re-reads — the double-check *passes*, because A has not
written yet — and refreshes with the same refresh token. The IdP rotates and
revokes. A then completes and writes its now-revoked token over B's valid one.
Compare-and-delete stops A deleting B's *lock*; it never stopped A's *write*.

Two controls, both required:

1. `set` is a compare-and-set on `rev`. A's write carries
   `expectedRev = fresh.rev`, which B already incremented, so A's write is
   rejected and A re-reads B's record.
2. The refresh HTTP timeout is asserted **strictly below** `LOCK_TTL_MS` at
   startup (§ 7.1), so the overlap is rare rather than routine.

**(b) `store.set` fails after a successful `oidc.refresh`.** The rotated token is
lost, the store keeps the revoked one, and every later refresh fails — a brief
Redis blip logs out every session that happened to be refreshing, with no
recovery but re-login.

Stated policy: **retry `set` once; if it fails again, `delete` the record.** A
clean re-login is better than a session that is permanently unable to refresh.
The delete is logged as `session.refresh.persist_failed` (§ 12).

The five invariants, restated:

1. **The double-check after acquiring the lock.** Without it two *sequential*
   holders both refresh. A lock alone does not prevent this — the least obvious
   point in the design, and it survives the challenge unchanged.
2. **The waiter never refreshes on timeout** (§ 8.5).
3. **Unique lock token, compare-and-delete release.**
4. **Release in `finally`.**
5. **The write is fenced on `rev`, and the refresh timeout is below the lock
   TTL.** This replaces revision 1's false claim.

### 8.5 `onRefreshTimeout` — the caller contract

Revision 1 threw `RefreshTimeout` and never said what the caller does. A thrown
error from a server component reaches the nearest `error.tsx` and renders a
500-class page for what is transient lock contention.

```
onRefreshTimeout(rec):
  if now() < rec.accessExpiresAt:  return { ...rec, refreshPending: true }
  else:                            return null      # -> treated as signed out
```

The access token is often still valid inside the skew window, so the request
proceeds on the stale-but-live token and the next request refreshes. Only a
genuinely expired token degrades, and it degrades to "signed out", which has a
defined recovery path (§ 11.6), rather than to a 500.

The backoff is **exponential with jitter**. § 11.4 fires N concurrent callers in
one event loop, and a fixed backoff would synchronise every waiter into the same
wake-up and the same pair of Redis round trips.

### 8.6 No Clock port — corrected

The determination stands: `Clock` fails the "two implementations that ship in
production" criterion, and faking time would make the AC 2 test **hang** rather
than fail, because the waiter's backoff uses `setTimeout`.

**One correction.** Revision 1 wrote "`Date.now()` is read once, by the caller
that already performs I/O." That is wrong and it is what produced the
non-terminating loop in § 8.3. The correct statement: **the clock is read once
per loop iteration**, and `shouldRefresh` remains a pure function taking `now` as
an argument.

Clock skew against the IdP is handled by `PAIGASUS_OIDC_CLOCK_TOLERANCE_SECONDS`
(default 30), passed to `openid-client`. Revision 1 omitted it: with a 60-second
skew and no tolerance, **every login fails**. § 11.3 carries a negative test.

**Reversal cost** for the no-port decision is low — adding `now?: () => number`
later touches a handful of internal call sites.

## 9. Routes and cookies

| Route | Method | Purpose |
|---|---|---|
| `/auth/login` | GET | start the flow |
| `/auth/callback` | GET | complete it |
| `/auth/logout` | POST | revoke server-side, then redirect to the IdP |
| `/auth/logout/callback` | GET | IdP returns here — § 9.5 |

Handlers are Web-standard `(Request) => Promise<Response>`, mounted at
`app/auth/[...paigasus]/route.ts`. Logout is POST because a `GET` logout is
triggerable by an `<img src>` from any page on the internet.

`SameSite=Lax` already blocks a cross-site form POST, so no CSRF token is added
to logout. Stated explicitly rather than left implicit.

### 9.1 `returnTo` is an open-redirect surface

Accepted only as a same-origin relative path: must start with `/`, must not start
with `//`, must not contain a scheme or a backslash. Table-tested with
`//evil.com`, `/\evil.com`, and percent-encoded variants.

CR/LF is not validated here: the Web `Headers` API rejects it, so the mitigation
is the API rather than the validator. Stated so nobody adds a hand-rolled header
writer later without re-checking.

### 9.2 The login transaction must be bound to the browser

**The obvious design has a real hole.** The transaction (`code_verifier`,
`nonce`, `returnTo`) stored server-side keyed by `state` does **not** bind the
flow to the browser that started it.

The attack:

1. The attacker starts a login and authenticates at the IdP **as themselves**.
2. The IdP redirects to `/auth/callback?code=…&state=…`. The attacker captures
   the URL without following it.
3. The attacker sends it to the victim.
4. The victim's browser hits the callback. The server exchanges a valid code and
   issues the victim a session **as the attacker**.
5. Everything the victim does lands in the attacker's account.

The defence is a browser-bound secret:

- `/auth/login` mints a transaction id and a 32-byte secret, stores the
  transaction under the id with only the secret's SHA-256, and sets a cookie
  carrying the secret.
- `/auth/callback` requires the cookie, and requires it to hash to the stored
  value, compared with `crypto.timingSafeEqual`. Absent or mismatched -> reject
  **before any token exchange**.

**The cookie is per transaction**, not a single fixed name. Revision 1 used one
name at `Path=/`, so two tabs starting a login overwrite each other's secret and
the first tab's callback fails with a security-flavoured error. A stale tab, a
back-button retry, or two zones opened at once all trigger it.

Name: `__Host-pgs_txn_<txnId>`, where `<txnId>` is a short base64url id also
carried in `state`. At most 4 outstanding transaction cookies are kept; on the
5th, the oldest is cleared. All are cleared on a successful callback.

`SameSite` must be `Lax`, not `Strict` — the cookie has to survive the cross-site
top-level redirect back from the IdP.

**`iss` on the callback (RFC 9207) is not validated**, because this design
configures exactly **one** issuer. That is the mix-up-attack defence, so it is
recorded here as a precondition: **adding a second issuer requires `iss`
validation before anything else.**

### 9.3 Cookies use the `__Host-` prefix

| Cookie | Attributes | Lifetime |
|---|---|---|
| `__Host-pgs_sid` | `HttpOnly`, `Secure`, `SameSite=Lax`, `Path=/`, no `Domain` | idle TTL |
| `__Host-pgs_txn_<id>` | same | 10 min |

Revision 1 said "host-only means no `Domain` attribute" and treated that as the
defence. **It is not.** It constrains what *this* server writes, not what a
sibling host writes. A compromised `*.example.com` host can set
`pgs_sid=<attacker value>; Domain=example.com; Path=/`. The browser then sends
two `pgs_sid` values in one header and the server cannot tell them apart — most
parsers take the first. That is cookie tossing, and it is session fixation by
another route.

This is a multi-zone console on a shared origin, so a sibling subdomain is a
normal part of the deployment, not a hypothesis.

The `__Host-` prefix forces `Secure`, `Path=/` and no `Domain`, and browsers
**refuse** a `Domain`-scoped write of a `__Host-`-prefixed name. It costs nothing
because § 6.7 already fixes the names as constants. § 11.8 asserts the prefix in
Playwright.

### 9.4 Session id, fixation, and the cookie's lifetime

Session ids are 32 bytes from `crypto.randomBytes`, base64url, opaque. **The sid
rotates on every login**: the callback issues a fresh sid and deletes any
pre-existing record.

**Sliding expiry, which revision 1 left undefined.** A refresh slides the
*record*'s TTL. A server component cannot set a cookie in Next, so it cannot
slide the *cookie*. Two quantities would then drift apart and an active user
would be logged out mid-session — the exact "randomly logged out" class this
design exists to prevent.

Resolution: **`__Host-pgs_sid` is a browser-session cookie with no `Max-Age`.**
Lifetime is bounded server-side by the record's idle TTL and its
`absoluteExpiresAt`, both of which the store enforces. Nothing needs to re-issue
the cookie, so nothing needs to write one from a server component.

### 9.5 Logout — AC 3

`POST /auth/logout`:

1. delete the record server-side,
2. clear `__Host-pgs_sid` and any outstanding txn cookies,
3. revoke the refresh token at the IdP if advertised (RFC 7009), best-effort,
4. redirect to `end_session_endpoint` with `post_logout_redirect_uri` and a
   `state` bound to this logout.

**Order matters.** Deletion precedes every network call, so a slow or unreachable
IdP cannot leave a live session behind.

**`id_token_hint` is deliberately omitted from step 4 (task 9 review).** It needs
the raw, signed ID token JWT, not decoded claims — `SessionRecord` stores only
`idTokenClaims`, and `OidcTokens` (the OIDC adapter's `authorizationCodeGrant`
result) never surfaces the raw token either. Storing it would add a THIRD bearer
credential to `SessionRecord` beside the access and refresh tokens, widening the
blast radius of a Redis compromise, for no benefit to either provider this design
targets.

Identification is carried instead by `client_id`: `openid-client@6.8.8` appends
it to the end-session parameters unconditionally whenever the caller does not
supply one (`build/index.js:1129-1141`). `client_id` plus a registered
`post_logout_redirect_uri` is enough for Keycloak (this package's own e2e
fixture) and Entra ID to skip the confirmation interstitial and honour the
redirect, with no `id_token_hint` needed.

**Named residual.** This is insufficient for an identity provider that MANDATES
`id_token_hint` and does not accept `client_id` as a substitute — Okta documents
it as required. Logging out against such a provider still succeeds server-side
(step 1 already deleted the record), but the end-session redirect will not
complete: a UX failure there, not a security one.

`/auth/logout/callback` — unspecified in revision 1 — validates the `state`,
clears any residual cookie, and redirects to the zone root. **If the IdP never
redirects back, the user is already logged out**, because step 1 happened first;
the callback is cosmetic and its failure is not a security event.

## 10. Middleware and session resolution — AC 4

```ts
createAuthMiddleware({ publicPaths, loginPath }): (req) => Response | undefined
```

Cookie **presence** only. No store read, no validation, no grants. A forged or
expired cookie passes middleware and is rejected downstream. That is the control,
not a limitation: CVE-2025-29927 was a middleware auth bypass.

### 10.1 The dead-cookie trap, and how it is closed

Revision 1 had a real defect here. When § 8.3 does `store.delete(sid); return
null`, or the record is simply gone, **the browser keeps sending the cookie**.
Middleware checks presence, so it does not redirect. A server component cannot
delete a cookie in Next. Every page then renders unauthenticated forever with no
path to `/auth/login` — the console looks broken and the user cannot fix it.

Two functions with distinct contracts close it:

```ts
getSession():     Promise<SessionRecord | null>   // never redirects
requireSession(): Promise<SessionRecord>          // redirect() to loginPath on null
```

- A page that needs a session calls `requireSession()`, which issues a Next
  `redirect()` to `${loginPath}?returnTo=…`. Redirecting *is* permitted from a
  server component; setting a cookie is not.
- `/auth/login` **always clears `__Host-pgs_sid` before starting a flow**, so the
  stale cookie dies at the one place that is allowed to write cookies.

That closes the loop without weakening middleware. § 11.8 carries a Playwright
row for it: delete the record out of band, load a guarded page, and assert the
browser lands on the login page rather than an empty one.

### 10.2 Passing the session to the client

`toSessionView(record)` produces the sanitized view; `<SessionProvider>`'s prop
type is `SessionView`. The two shapes are deliberately **not** structurally
compatible — `SessionRecord` nests identity under `principal`, `SessionView` is
flat — so `<SessionProvider session={record}>` is a type error, not a silent
token leak.

## 11. Testing

### 11.1 Tiers and the tasks that run them

Revision 1's tier table contradicted its own task split: it listed "memory +
Redis" for two tiers while claiming `test` needed no Docker. Corrected:

| Tier | Task | Needs |
|---|---|---|
| Pure unit | `test` | nothing |
| Token verification (`jose`) | `test` | nothing |
| Structural (AC 4, AC 5) | `test` | nothing |
| Store contract — memory | `test` | nothing |
| Store contract — Redis | `test-e2e` | Redis container |
| Single-flight — both adapters (**AC 2**) | `test-e2e` | Redis container |
| Playwright (**AC 1**, **AC 3**) | `test-e2e` | Keycloak + Redis + Chromium |

Two tasks, not three: `test-e2e` runs **both** the container-backed vitest suites
and Playwright. A third task name would be a third `T` registration for no
separation that matters — everything in it needs Docker.

### 11.2 "Fail loud, not skip silent"

A task whose only purpose is container tests has no reason to skip. If Docker is
unreachable, `test-e2e` fails. No hatch, no canary, no `CI`-outranks-the-hatch
rule, because no code path decides whether to run.

**The honest cost, which revision 1 hid.** CLAUDE.md instructs contributors to
run the full marker-delimited graph before pushing, and § 16 adds `:test-e2e` to
it. Every contributor's pre-push run then needs Docker, a Keycloak pull, a Redis
container and an installed Chromium.

That is accepted rather than exempted, for one reason: **the repository already
requires Docker for a full local graph run**, because `paigasus-iam-rs:test`'s
Docker suites are in `T` today. Adding a browser is an increment on an existing
requirement, not a new class of one. `CONTRIBUTING.md` gains the provisioning
step.

The rejected alternative was a `T_EXEMPT` entry with a dedicated workflow. That
keeps the local run cheap but removes the E2E tier from the affected-graph gate,
which is the only thing that runs it on a PR that touches this package.

### 11.3 Token verification, framed honestly

`openid-client` owns the cryptography; re-testing its signature validation would
be testing someone else's library. These tests prove **our wiring passes the
right expectations** — `expectedNonce`, `expectedState`, the audience, and the
clock tolerance — so a token that should be rejected is. The negative cases are
the point.

**Two corrections, both MEASURED during implementation. Revision 2 was wrong on
each.** See the measurements document, § M1 and its addenda.

**(a) `openid-client` does NOT verify the ID token's signature on this path.**
The paragraph above is true in general and false for the authorization-code
grant specifically. `oauth4webapi`'s `validateIdTokenClaims` checks claims only
— issuer, audience, subject presence, `exp`/`nbf`/`nonce`/`auth_time` — and never
calls `validateJwsSignature`. This is deliberate and spec-compliant: OIDC Core
allows TLS to the token endpoint to authenticate the issuer in place of a
signature check. Measured: a token signed by a key never published in the JWKS,
under a `kid` impersonating a published one, was **accepted**.

So this package opts in, passing `enableNonRepudiationChecks` to `discovery()`
unconditionally. Four costs follow, and they are accepted rather than unnoticed:
a JWKS outage now fails login **and** refresh where before it failed neither;
key rotation opens a failure window of up to 60 s; HS256 ID tokens become fatal;
and one refresh becomes two bounded calls, which is why § 8.4's timing invariant
is `2 × httpTimeout < lockTtl` rather than the single-timeout bound revision 2
assumed. HS256 is ruled out of scope rather than made configurable, because
ADR-0015 already pins IAM to RS256/ES256 for access tokens.

**(b) The clock-skew figures below are unreachable.** `exp`'s tolerance bounds
only the past, and `nbf` is the only future-direction tolerance-gated check, so
a 45 s and a 120 s future skew both reject under a 30 s tolerance. The test uses
`nbf` skews that straddle the boundary instead — and must use a **non-default**
tolerance, because `getClockTolerance` returns 30 when none is set, so a test at
30 cannot distinguish a wired tolerance from an absent one.

~~Includes a clock-skew case: a token minted 45 s in the future is accepted
under a 30 s tolerance only if the tolerance is wired, and a 120 s skew is
rejected.~~ Superseded by (b).

### 11.4 AC 2's test

Fire N concurrent `getSession()` calls at an already-expired fixture against a
counting fake IdP. Assert the refresh endpoint was hit **exactly once** and that
all N callers receive the new access token. Both adapters, **real timers**.

**What the Redis run proves that the memory run does not** — revision 1 did not
say, and the honest answer is: within one process, very little, since both are
single-process. What it proves is that the **Lua compare-and-set and
compare-and-delete scripts are correct**, which the memory adapter's synchronous
checks cannot exercise.

The genuinely cross-process case — two Node processes contending on one Redis —
is covered by one additional test that forks a child process. Without it, AC 2's
claim about *zones* is untested, and § 17's AC 2 row would overclaim.

### 11.5 Structural tests

- Transitive import graph of `src/client.ts` reaches nothing under `adapters/`,
  `core/`, `ports/`, `http/`, `next/`.
- Transitive import graph of `src/middleware.ts` reaches no store, resolver or
  `openid-client`.
- Strict equality on `SESSION_VIEW_KEYS` (§ 8.1).
- Strict equality on `package.json`'s `exports` keys, so adding `"."` reds.
- `expectTypeOf`: `SessionRecord` is not assignable to `SessionView`.

**`expectTypeOf` is not enforced by `test`.** `.moon/tasks/typescript-project.yml`
runs `vitest run --passWithNoTests` with no `--typecheck`, so it is a runtime
no-op there. It is enforced by `tsc --noEmit` in `build`/`typecheck` — and only
because § 3.2's tsconfig includes `tests/**/*`. Recorded so nobody "simplifies"
either half.

### 11.6 Recovery paths, tested

- Record deleted out of band -> guarded page redirects to login (§ 10.1).
- `version` mismatch -> treated as absent, same path.
- Redis unreachable mid-session -> redirect to login, not a 500 (§ 7.2).

### 11.7 The Keycloak fixture

Mirrors the Rust recipe: `quay.io/keycloak/keycloak:26.4`, realm
`paigasus-test`, self-signed HTTPS on 8443, 240 s startup
(`rs/crates/services/paigasus-iam/tests/keycloak_e2e.rs:46-82`).

**Its realm client cannot be reused.** `paigasus-cli` is `publicClient: true`,
`standardFlowEnabled: false` — a password-grant client. This package ships its
own realm JSON with a **confidential** code-flow client: `standardFlowEnabled:
true`, a secret, a registered redirect URI, and `offline_access` in the default
client scopes (§ 6.5).

Containers are started by **`testcontainers` (npm)** from Playwright's
`globalSetup`. Revision 1 described the recipe and never named what runs it; the
Rust side uses `testcontainers-modules`, a Rust crate with no TS equivalent here.
GitHub Actions `services:` was rejected — it cannot do the certificate copy and
realm import this recipe needs.

### 11.8 Playwright rows

- **AC 1:** full round trip through Keycloak's login form; assert cookie flags
  **and the `__Host-` prefix** via `context.cookies()`.
- **AC 3:** log in, capture the cookie, log out, **replay that cookie in a fresh
  context**, assert rejection.
- **§ 10.1:** stale-cookie recovery.
- **§ 9.2:** two concurrent login flows in two tabs both complete.

### 11.9 What Playwright drives

A fixture server in `tests/`, not the console app: a minimal Node server mounting
the Web-standard handlers plus a public and a guarded page.

**It must run with `NODE_OPTIONS=--conditions=react-server`.** `src/server.ts`
opens with `import 'server-only'`, whose exports map resolves to an unconditional
`throw` under every condition except `react-server`
(`ts/packages/paigasus-next-config/vitest.config.ts:9-12`). Vitest solves this
with `ssr.resolve.conditions`; a plain Node process has no such setting.
Revision 1 missed this and the fixture server would not have started. M8.

**Stated limitation:** this proves cookie behaviour, the round trip and logout
revocation, but **not** Next's own integration. The middleware matcher and
`requireSession()` in a real server component are exercised only when the first
zone app mounts the package — the follow-up issue in § 19.

## 12. Observability

Revision 1 had none: seventeen sections with no logging, tracing or metrics. A
callback rejected for a txn-secret mismatch (§ 9.2) is a **security event** and
was silent. A failing refresh was invisible, which makes the "users randomly
logged out" class undiagnosable in production — the very failure AC 2 exists to
prevent.

This is a port decision, not a detail: adding a logger later changes every
constructor in `adapters/` and `http/`.

```ts
interface AuthLogger {
  event(name: AuthEventName, fields: Record<string, string | number | boolean>): void;
}
```

Injected through `createAuthRuntime`; the default is a no-op adapter, so the
package emits nothing unless an app opts in.

Minimum event set: `login.started`, `login.callback_rejected` (with a `reason`
enum — `txn_missing`, `txn_mismatch`, `state_unknown`, `code_exchange_failed`),
`session.created`, `session.refreshed`, `session.refresh_failed`,
`session.refresh_timeout`, `session.refresh.persist_failed`, `session.deleted`,
`logout.completed`, `store.unavailable`.

**Redaction rule, stated per field:** no event carries a token, a refresh token,
an authorization code, the client secret, the Redis DSN, or a txn secret. `sid`
is logged **truncated to 8 characters** — enough to correlate, not enough to
replay. This is what § 15's redaction wrapper is *for*; revision 1 promised
redaction and named no consumer.

**Metrics are deliberately out of scope.** The Rust services emit through
`paigasus-observability`, which has no TS counterpart, and inventing one here
would pre-empt that work. The event set above is designed so a metrics adapter
can be layered on it later without changing call sites.

## 13. `can()` must fail *open* while grants are deferred

`can(view, scopePrn, roleKey)` is cosmetic — IAM's Cedar evaluation is
authoritative and the UI must render its 403 correctly.

**But the deferral in § 18 makes the naive version actively harmful.** With an
empty grant set, `can()` returns false for everything. SMA-510
(`@paigasus/app-shell`) is the next consumer; if it gates navigation on `can()`,
the console renders with **no navigation at all** and looks broken — and the
"let IAM answer" argument never runs, because the link is not there to click.

Therefore `SessionView` carries `grantsAvailable: boolean`, and **`can()` returns
`true` whenever grants are unavailable.** Cosmetic gating degrades to "show it
and let IAM answer", which is the correct failure direction for a cosmetic
control. This is stated in `can()`'s doc comment and passed to SMA-510 as a
constraint.

## 14. The package does not depend on `@paigasus/proto`

`RoleGrantRef` and `Membership` are proto messages. Importing them would make
`@paigasus/auth` a dependent of `@paigasus/proto`, contradicting the § 6
dependency graph and — by reasoning, not measurement — redding
`ci/affected-graph/run.sh`'s strict-equality `contracts->proto` case, since
`assert_case` there queries `--affected --downstream deep`.

So the port speaks **domain types defined locally**:
`RoleGrantRef { scopePrn, roleKey }` as a plain interface. When
`IntrospectPrincipalResolver` lands in SMA-508, the *adapter* maps proto to these
types. Ports speak the domain's language; adapters own the wire format.

M9 measures the affected-graph claim rather than asserting it.

## 15. Dependencies

New catalog entries in `ts/pnpm-workspace.yaml`:

| Dependency | Role |
|---|---|
| `openid-client` | the relying party — ADR-0017 decision 4 |
| `jose` | locally minted JWKS in tests (AC 6) |
| `redis` | node-redis, the `RedisSessionStore` adapter |
| `@playwright/test` | the E2E tier |
| `testcontainers` | starts Keycloak and Redis (§ 11.7) |

Already in the catalog and used by this package: `zod`, `server-only`, `react`
(peer, for `<SessionProvider>`), `next` (peer), `typescript`, `vitest`.

pnpm 11's 24-hour `minimumReleaseAge` means a same-day release of any of them
reds CI.

**node-redis over ioredis:** the official client, actively developed, first-party
types, and a DSN format matching the Rust side verbatim.

**Redaction has a limit, and revision 1 overstated it.** A newtype wrapper only
helps if both `toString` and `toJSON` are overridden **and** the value never
reaches a library that formats it itself. node-redis embeds the DSN in its own
connection errors and `openid-client` may include a URL. So the rule is: wrap our
own values, and **never log a caught library error object directly** — extract
`name` and a fixed message instead. § 12's event set is built that way.

## 16. Registration obligations

`:test-e2e` is a new task name. Two edits, which `ci/affected-graph/ci_targets.py`
asserts agree:

1. `.github/workflows/ci.yml`'s `T=(…)` array — single-line bash array (SMA-541).
2. The marker-delimited command in `CLAUDE.md`.

Not a `repo:*` gate, so `SELF_SCHEDULED_GATES`, `SELF_TASK_EXPECTED_GLOBS` and
`REQUIRED_REPO_TASKS` do not apply.

**The trap that applies regardless:** `moon ci` exits **0** on a target that
resolves to nothing. A typo or a non-CI-eligible task is a silent no-op on every
PR. `ci_targets.py`'s resolution assertion is what catches it, so the entry must
be covered by it and must **not** carry a `T_EXEMPT` entry — an exemption would
switch the whole E2E tier off invisibly.

### 16.1 Task inputs

Revision 1 declared none, which is this repository's recorded silent-failure
class: `inputs` are the only thing conferring affectedness, and `test-e2e` is a
**new task name**, so it inherits nothing from `test`.

`test-e2e` inputs: `@group(sources)`, `@group(tests)`, `playwright.config.ts`,
`vitest.config.ts`, the Keycloak realm fixture, `package.json`,
`/ts/pnpm-lock.yaml`.

`test` additionally lists `/ts/eslint.config.js`, because § 11.5's structural
tests assert against the boundary preset — the same reason
`paigasus-next-config-ts` lists it.

`repo:affected-smoke` should gain a `run_task_case_ci` row anchoring an auth
source file to `paigasus-auth-ts:{build,test,test-e2e}`. Nothing today asserts
the new task is reachable from an edit to the package.

### 16.2 CI cost and container contention

- The `playwright install --with-deps chromium` step is **unconditional**. A
  path-filtered `if:` can be skipped on a run where `moon ci` still selects
  `test-e2e` as affected, and the task then fails for a missing browser. An
  unconditional step costs every run; a wrong one is a red on a green PR.
- `moon ci` runs tasks in parallel, so this task's Keycloak, Redis and Chromium
  may start alongside `paigasus-iam-rs:test`'s own Keycloak and Postgres.
  CLAUDE.md records a ~14 GB runner disk exhausting on Rust dependencies alone,
  and `rs/.config/nextest.toml` caps container concurrency for the Rust side
  only. M10 measures peak disk and whether serialisation is needed.

## 17. Acceptance criteria mapping

| AC | Satisfied by | Proven by |
|---|---|---|
| 1 — round trip, config only | § 9, discovery-driven | Playwright vs Keycloak |
| 2 — exactly one refresh | § 8.3, five invariants | § 11.4, both adapters + a 2-process case |
| 3 — logout revokes | § 9.5, delete-first | § 11.8 cookie replay |
| 4 — no authz in middleware | § 4.2 entry + § 5.2 block 3 | § 11.5 import graph + a DENIED row |
| 5 — `/client` cannot reach a token | § 4.3 four layers | § 11.5 graph + type test |
| 6 — JWKS units, one integration | § 11.3, § 11.8 | both, in CI |

## 18. The grants snapshot is deferred

The issue's scope line says the session holds a "memberships/grants snapshot".
This issue ships the **slot** — typed, sanitized, carried to `can()` — but empty.

Verified: `contracts/buf.gen.yaml:51-57` runs only `bufbuild/es` for TypeScript
(descriptors, no client); `ts/packages/paigasus-proto/src/index.ts` re-exports
only `common/v1`; `ts/packages/paigasus-sdk/src/index.ts` is `export {};`. The
descriptors exist, so the remaining work is wiring — a Connect-ES dependency, an
export change, and a real SDK. That is **SMA-508**.

Note also ADR-0015's consequence that `role_groups` stays empty until Cedar
lands, so even a wired Introspect returns an empty grant set today. § 13 is what
keeps that from rendering an empty console.

`IntrospectResponse`, for the future adapter
(`contracts/proto/paigasus/iam/v1/iam.proto:231-249`):

```proto
message IntrospectResponse {
  string principal_prn = 1;  string status = 2;
  string issuer = 3;         string subject = 4;
  google.protobuf.Timestamp expires_at = 5;
  repeated Membership memberships = 6;
  reserved 7; reserved "role_group_prns";
  repeated RoleGrantRef role_grants = 8;
}
```

## 19. Out of scope

| Deferred | Why | Where |
|---|---|---|
| `IntrospectPrincipalResolver`, real grants | no TS transport — § 18 | **SMA-508** |
| Mounting auth in a zone app | keeps this a pure package issue; § 11.9 states the untested surface | follow-up issue |
| `<ZoneLink>`, app-shell | different package | SMA-510 |
| Capability discovery | different ADR | ADR-0020 |
| Error model `(domain, reason)` | Rust + contracts work | ADR-0019 |
| Metrics | no TS observability crate yet — § 12 | with the TS observability work |
| A bundled IdP | ADR-0017 defers it | not scheduled |

## 20. Measurements — NOT YET TAKEN

Behavioural claims in this repository are measured, not assumed. The following
are **assumptions** and must be measured during implementation, recorded in
`docs/superpowers/specs/2026-09-09-sma-506-measurements.md`.

| # | Claim |
|---|---|
| M1 | `openid-client` v6's API for discovery, `authorizationCodeGrant`, refresh, revocation, and where clock tolerance is set. §§ 8.3 and 9 assume a shape. |
| M2 | node-redis **v6**'s `SET … NX PX` option object, `GETDEL`, Lua `EVAL` for CAS, and that `disableOfflineQueue` behaves as § 7.2 needs. v6 changed the client API surface from v5, so none of this carries over from prior knowledge. |
| M3 | `NODE_EXTRA_CA_CERTS` + Playwright `ignoreHTTPSErrors` suffice for the self-signed cert on both sides. |
| M4 | `moon ci :test-e2e` resolves to a real task and `ci_targets.py`'s assertion passes. A `T` entry resolving to nothing exits **0**. |
| M5 | **The AC 2 test can fail.** Delete the lock acquisition; the refresh counter must exceed 1. |
| M6 | The Keycloak realm JSON for a confidential code-flow client on tag 26.4. |
| M7 | `server-only` is still a no-op in middleware on the Next version in use. |
| M8 | The fixture server starts under `--conditions=react-server` with this module graph (§ 11.9). |
| M9 | Whether a `@paigasus/proto` dependency would in fact red `contracts->proto` (§ 14) — asserted by reasoning today. |
| M10 | Peak CI disk and whether `test-e2e` must be serialised against `paigasus-iam-rs:test` (§ 16.2). |
| M11 | `__Host-`-prefixed cookies work over `http://localhost` in Chromium under Playwright (§ 9.3). |

**M5 is the one that matters most.** An AC 2 test that cannot fail with the lock
deleted proves nothing.

## 21. What changed in revision 2

The adversarial challenge returned **NEEDS REWORK** with nine blockers. All nine
are folded in; the challenge's verified repo claims were re-verified against the
files before acting.

**Blockers.** The boundary globs derived an impossible package name and would
have redded `paigasus-next-config-ts:test` (§ 5.2). The client deny-patterns used
`../` where `src/client.ts` writes `./`, so the rule asserted nothing about the
one file it protects (§ 5.2). `defineRuntimeConfig` cannot express cross-field
rules or read the zone keys, so both were moved into `createAuthRuntime`
(§ 6.1, § 6.8). The § 8.3 pseudocode bound `now` once and never terminated, and
dereferenced a possibly-null `fresh` (§ 8.3). Invariant 5 was **false** in two
ways, now fixed with a fenced compare-and-set write and a timeout ordering
(§ 8.4). The tier table contradicted the task split (§ 11.1). An expired session
left a cookie that permanently satisfied middleware, stranding the user
(§ 10.1). Host-only cookies do not stop cookie tossing; both cookies now carry
the `__Host-` prefix (§ 9.3). The package emitted no logs at all (§ 12).

**Majors folded in.** Per-transaction txn cookies (§ 9.2). `memory` is
single-process and now refuses a multi-zone config (§ 6.6). `offline_access` in
the default scopes (§ 6.5). A boundary block for `apps/*/middleware.*`, without
which AC 4 was half-enforced (§ 5.2). Timeouts and a Redis failure policy (§ 7).
`can()` fails open while grants are deferred (§ 13). Sliding-expiry resolved by
making the sid a browser-session cookie (§ 9.4). The tsconfig shape corrected
(§ 3.2). `absoluteExpiresAt` given its own variable and a clamping rule (§ 8.1).
`test-e2e` inputs declared (§ 16.1). `testcontainers` named (§ 11.7). The fixture
server's `--conditions=react-server` requirement (§ 11.9). The logout callback
specified (§ 9.5). `describeIssues`' cost accepted and documented (§ 6.9).
`version`-bump mass logout stated (§ 8.1). Clock tolerance (§ 8.6). CI cost and
container contention (§ 16.2).

**Minors folded in.** `SESSION_VIEW_KEYS` as a runtime tuple, since an interface
has no key set (§ 8.1). `timingSafeEqual` (§ 9.2). The one-issuer precondition
for skipping RFC 9207 `iss` validation (§ 9.2). CR/LF handled by the `Headers`
API, stated (§ 9.1). `expectTypeOf` enforced by `typecheck`, not `test` (§ 11.5).
Bare node builtins added to the deny group (§ 5.2). Jittered backoff (§ 8.5).
Unit suffixes reconciled. The redaction wrapper's limit stated (§ 15). The
citation corrected to `runtime.ts:10-17` (§ 4.2). The `contracts->proto` claim
demoted to reasoning plus M9 (§ 14).

**Not accepted.** One only: the challenge suggested three test tasks
(`test` / `test-integration` / `test-e2e`). Two is enough — everything beyond the
pure tier needs Docker, so a third `T` registration buys no separation
(§ 11.1). Its underlying finding, that revision 1's tier table and task split
contradicted each other, is accepted and fixed.
