# SMA-704 — run OIDC discovery before the session lock, not under it

- Issue: [SMA-704](https://linear.app/smaschek/issue/SMA-704)
- Package: `ts/packages/paigasus-auth` (`@paigasus/auth`)
- Related: SMA-656 (its spec, § 8, defers this gap to SMA-704); SMA-705 (readiness gate or eager
  discovery at start)
- Status: approved on 2026-09-27 (Gate 1), after the spec challenge (§ 9).

## 1. Problem

`createAuthRuntime` (`src/runtime.ts:130-143`) refuses a configuration unless
`2 * PAIGASUS_OIDC_HTTP_TIMEOUT_MS < PAIGASUS_SESSION_LOCK_TTL_MS`. Call the timeout T. The rule
counts the IdP calls that `refresh()` makes while `resolveSession` holds the session lock:

1. the token call (`client.refreshTokenGrant`), bounded by T;
2. the JWKS call that the non-repudiation hook makes when the response has an `id_token`, bounded
   by T.

`refresh()` (`src/adapters/oidc.ts:412-413`) first awaits `getConfig()`. On a process that has not
completed OIDC discovery, `getConfig()` sends the discovery request. That is a third call bounded
by T, and it runs under the lock.

With the shipped defaults (T = 3500 ms, lock TTL = 10000 ms), 3T = 10500 ms, which is more than
the lock TTL. The lock can then expire while the first holder still refreshes. A second request
can take the lock and refresh the same refresh token. The `rev` fence (invariant 5 of
`src/core/single-flight.ts`) rejects only the loser's WRITE. It does not stop the second token
call. With an IdP that rotates refresh tokens and detects reuse, the second call can revoke the
token family and sign the user out.

A cold process is a normal case, not an edge case. No code warms discovery at start. After a
restart, a scale-out or a rolling deploy, a browser can send a valid session cookie to a new pod.
The first OIDC call of that pod is then `refresh()`.

Facts from `openid-client@6.8.8` (`build/index.js`), read on this branch:

- `discovery()` → `performDiscovery()` (`:260-302`) sends exactly ONE request, under one
  `AbortSignal.timeout(timeout * 1000)`. It does not prefetch JWKS.
- `refreshTokenGrant()` (`:999-1030`) sends the token request with `signal(timeout)`. When the
  response has an `id_token`, the non-repudiation hook (`:748-761`) fetches JWKS with its own
  `signal(timeout)`. `signal()` (`:1150-1152`) makes a NEW `AbortSignal.timeout` on each call, so
  each of the two calls is bounded by T separately, and together they take at most 2T.
- Without DPoP, openid-client adds no retries (`:1185-1190`).

So the IdP calls of a warm refresh take at most 2T, and those of a cold refresh at most 3T. This
is reasoned from the code. Test 13 (§ 5.4) measures it.

## 2. Decision

Discovery runs BEFORE `resolveSession` takes the lock. Under the lock, `getConfig()` is then a
resolved promise and sends nothing. So the IdP calls under the lock are at most 2T, and the
existing 2T rule covers all of them. The store calls under the lock are NOT in that rule; see § 6.

Rejected alternatives:

- **Put discovery in the bound (3T < lock TTL).** This needs a new default (lock TTL 12000 ms or
  T 3000 ms). A configuration that starts today, for example T = 4000 ms with TTL = 10000 ms, would
  then fail at start after an upgrade. A larger TTL also blocks refresh for longer after a holder
  crashes.
- **Both.** With discovery outside the lock, a 3T check refuses configurations that are safe.
- **Eager discovery at `getAuthRuntime` time.** It cannot give the guarantee alone: when the eager
  discovery fails, the lazy discovery in `refresh()` runs under the lock again. SMA-656 § 8 put it
  out of scope, and SMA-705 tracks it for readiness.
- **Make `refresh()` fail closed when the configuration is not resolved.** This would make a wiring
  mistake visible in every test. But it changes the contract of `refresh()` for every caller, and
  test 9 (§ 5.2) already guards the production wiring. Not done.

No default changes. No configuration rule changes.

## 3. Design

### 3.1 Port and adapter — `src/adapters/oidc.ts`

Add one method to `OidcClient`:

```ts
/**
 * Runs OIDC discovery if this process has not completed it, and waits for it. Sends no token
 * request. resolveSession calls it before it takes the session lock (SMA-704).
 */
ensureDiscovered(): Promise<void>;
```

The implementation is `await getConfig();`. It throws the same `OidcDiscoveryFailed` that
`getConfig()` throws today, with no change to its message, `reason` or the missing `cause`.

Why a later `refresh()` sends no discovery request: `getConfig()` assigns `configPromise` once
(`??=`) and clears it only in its own `.catch`. A promise that resolved is never cleared. So after
`ensureDiscovered()` resolves, every `getConfig()` in the same `OidcClient` returns that resolved
promise. Concurrent `ensureDiscovered()` calls share one discovery request for the same reason.

The SMA-656 D10 no-revoke invariant does not change: `ensureDiscovered()` sends no token request,
and `OidcDiscoveryFailed` still comes only from `getConfig()`.

### 3.2 Single-flight — `src/core/single-flight.ts`

`ResolveDeps` gets a REQUIRED member:

```ts
/**
 * Called at most once per resolveSession, only when a refresh is due and the record has a refresh
 * token, BEFORE the first tryAcquireLock (SMA-704). Production wires it to
 * OidcClient.ensureDiscovered, so the OIDC discovery request never runs under the lock. A
 * rejection is a transient refresh failure.
 */
prepareRefresh: () => Promise<void>;
```

It is required, not optional, so that the type check forces every caller to supply it
(`tsconfig.json` includes `tests/**`).

Order in `resolveSession`:

1. `store.get(sid)`; the absolute-expiry check; `shouldRefresh`. Unchanged. A read of a session
   that needs no refresh does not call `prepareRefresh`.
2. NEW: when `rec.refreshToken !== undefined`, `await prepareRefresh()`. A record with no refresh
   token skips this step, so it reaches the `no_refresh_token` delete under the lock as today and
   waits for no discovery.
3. `deadline = Date.now() + lockWaitMs`. This line moves AFTER step 2, so a waiter keeps its full
   `lockWaitMs` after discovery.
4. The lock loop. Unchanged.

When `prepareRefresh` rejects in step 2, `resolveSession` takes no lock and:

1. re-reads the record with `store.get(sid)`, as the lock-timeout branch does (`:329`). Up to T has
   passed, and another process may have refreshed or deleted the record in that time;
2. if the re-read gives `null`, returns `null`;
3. if `!shouldRefresh(Date.now(), reread.accessExpiresAt, skewMs)`, returns the re-read record.
   Another process refreshed it, so no refresh is needed (the same rule as invariant 1);
4. otherwise calls the transient helper (below) with the re-read record and the error.

A store failure during the re-read propagates, as any store failure in `resolveSession` does
today.

The transient helper: the transient half of the `refresh` catch (`:176-225`) moves into one helper,
and both sites call it. The helper:

- logs `session.refresh_failed` with `reason: 'transient'` and `degraded`, plus `oauthError` when
  the error has one (the site under the lock), or `stage: 'discovery'` (the new site). A discovery
  failure has no OAuth code, so the new site never sets `oauthError`;
- returns `{ ...record, refreshState: 'failed' }` when
  `Date.now() < min(record.accessExpiresAt, record.absoluteExpiresAt)`;
- otherwise throws the error. `get-session.ts`'s catch then handles it as it does today.

The helper covers ONLY the transient outcome. The site under the lock keeps its `rejected` branch
(the `session.refresh_failed` line with `reason: 'rejected'`, the delete, `session.deleted`) and
calls the helper for everything else. The new site calls the helper directly and never checks for
a rejection, so it never deletes a record. The `stage` field is new: `src/ports/logger.ts`
documents the fields of `session.refresh_failed`, and it gets the new field with the closed value
`'discovery'`.

Differences from today, stated on purpose:

- A record that passed its absolute cap during the discovery wait is not deleted on this path. The
  helper's `min(...)` finds it not live and throws, so `getSession` returns `null`. The next read
  deletes it through the outer absolute-expiry check. Today the site under the lock deletes it.
- On a cold pod, concurrent requests that need a refresh share ONE discovery request and ONE
  failure. Each of them logs one `session.refresh_failed` line with `stage: 'discovery'`. Today each
  lock holder in turn runs its own discovery under the lock.

### 3.3 Wiring — one function for production and the e2e fixture

A new exported function in `src/next/get-session.ts` (or a sibling module that the plan names)
builds the `ResolveDeps` from an `AuthRuntime`:

```ts
export function resolveDepsFor(runtime: AuthRuntime): ResolveDeps {
  return {
    store: runtime.store,
    prepareRefresh: () => runtime.oidc.ensureDiscovered(),
    refresh: (refreshToken) => runtime.oidc.refresh(refreshToken),
    revoke: (token) => runtime.oidc.revoke(token),
    logger: runtime.logger,
    skewMs: runtime.skewMs,
    lockTtlMs: runtime.lockTtlMs,
    lockWaitMs: runtime.lockWaitMs,
    ttlMs: runtime.ttlMs,
  };
}
```

`getSession` (`get-session.ts:53-65`) and `tests/e2e/fixture-server.ts:151-162` both call it. So the
e2e tier uses the production wiring, not a copy. All three IdP calls go to the same `runtime.oidc`
instance, so `prepareRefresh` and `refresh` share one discovery cache.

The other `resolveSession` callers pass their own `prepareRefresh`: `tests/core/single-flight.test.ts`,
`tests/fixtures/refresh-worker.ts`, `tests/containers/single-flight-redis.test.ts`,
`tests/http/logout.test.ts:236`. A fake with no discovery passes `async () => {}`.

The `OidcClient` fakes and wrappers get an `ensureDiscovered` that RESOLVES, so that their existing
tests keep their current path: `tests/next/get-session.test.ts:61-63` (`unusedOidc`),
`tests/http/logout.test.ts:78`, `tests/support/store-failure.ts:80`,
`tests/http/callback.test.ts:58-70`, `tests/http/route-handler.test.ts` (around `:46-67`). The plan
lists every fake that the type check reports; this list is the known set.

### 3.4 The runtime cache key

The cached runtime lives on `globalThis` under `paigasus.auth.runtime.v1:<zone>`
(`src/runtime.ts:217-230`). That comment's own rule is to change the version when the runtime's
shape changes incompatibly. The `OidcClient` inside it gains a required method, and a runtime that
an older module copy made (for example across a dev HMR reload) has no `ensureDiscovered`. So
`RUNTIME_KEY_PREFIX` becomes `paigasus.auth.runtime.v2`, and `tests/runtime.test.ts:155` follows.

### 3.5 The bound and its documentation

- `src/runtime.ts`: the 2T check and its message do not change. The invariant 3 comment says that
  discovery is NOT one of the calls under the lock, because `resolveSession` calls
  `prepareRefresh` first (SMA-704). It also says that the rule covers the IdP calls only, and names
  the store-call residual (§ 6).
- `src/core/single-flight.ts`: invariant 5's comment gets the same statement. It also says that
  the waiter's `lockWaitMs` starts after `prepareRefresh`.
- `src/adapters/oidc.ts`: the header's "DISCOVERY IS LAZY" paragraph names `ensureDiscovered` and
  the reason for it.
- `README.md`:
  - the rows for `PAIGASUS_OIDC_HTTP_TIMEOUT_MS` and `PAIGASUS_SESSION_LOCK_TTL_MS` say that
    discovery runs before the lock and is not in the 2x budget;
  - the limitation bullet at `README.md:239-241` ("SMA-704 tracks this") is rewritten. It keeps the
    first sentence, says that discovery now runs before the lock, and states the store-call
    residual (no issue tracks it).

## 4. What does not change

- Defaults, the `authEnvShape`, and the 2T rule.
- The Helm chart. It sets neither variable (checked: no match in `charts/`).
- Login, callback, logout. They take no session lock.
- The revokes after release (SMA-681).
- Latency on a warm process.

What changes for latency: on a cold process, a request that must refresh waits up to T for
discovery once, before the lock. A waiter on a cold pod can now wait up to T + `lockWaitMs`, plus
its store calls. Today it waits for the same discovery, but under the lock, and a waiter's
`lockWaitMs` includes it.

## 5. Tests

### 5.1 Unit — `tests/core/single-flight.test.ts`

With a recording store and a recording `prepareRefresh`:

1. When a refresh is due, `prepareRefresh` resolves BEFORE the first `tryAcquireLock` call. The fake
   resolves after a macrotask (`setTimeout(…, 20)`) and records its resolution time there, so a
   missing `await` fails the test.
2. When no refresh is due, `prepareRefresh` is not called.
3. When the record has no refresh token, `prepareRefresh` is not called, and the record is deleted
   with `reason: 'no_refresh_token'` as today.
4. `prepareRefresh` rejects, and the re-read access token is live: the result has
   `refreshState: 'failed'`; `session.refresh_failed` is logged with `reason: 'transient'`,
   `degraded: true` and `stage: 'discovery'`; `tryAcquireLock` is never called; nothing is deleted.
5. `prepareRefresh` rejects, and the re-read access token is expired: `resolveSession` rejects with
   that error, `tryAcquireLock` is never called, and nothing is deleted.
6. Tests 4 and 5 each run twice: once with a plain `Error`, and once with an error that carries the
   `RefreshRejected` code. The second run must also not delete. So a new site that goes through
   the full classifier fails the test.
7. `prepareRefresh` rejects, and during its wait another writer (a) refreshes the record, or (b)
   deletes it. (a) returns the refreshed record with no `refreshState`. (b) returns `null`.
8. The `lockWaitMs` deadline starts after `prepareRefresh` resolves. Another holder keeps the lock
   for the whole test, and `prepareRefresh` takes longer than `lockWaitMs`. The time from
   `prepareRefresh` resolving to the `'pending'` return, measured with `Date.now()` (the clock the
   code uses), is at least `lockWaitMs`, and the waiter makes more than one lock attempt. With the
   deadline set before `prepareRefresh`, the waiter makes exactly one attempt and returns at once,
   so this test fails. (The loop always makes one attempt before it checks the deadline, so "at
   least one attempt" would not fail.)

### 5.2 Wiring — `tests/next/get-session.test.ts`

9. `getSession` with a recording `oidc` whose `ensureDiscovered` resolves after a macrotask, and a
   recording store. When a refresh is due, `ensureDiscovered` resolves before the first
   `store.tryAcquireLock`. This is the test that guards the production line in `resolveDepsFor`.

### 5.3 Adapter — `tests/adapters/oidc.test.ts`

10. The discovery request count is 1 after `ensureDiscovered()` alone, and still 1 after a
    following `refresh()`. So a no-op `ensureDiscovered` fails the first assertion.
11. Two concurrent `ensureDiscovered()` calls on a cold client send one discovery request.
12. A failed `ensureDiscovered()` rejects with `OidcDiscoveryFailed`, and a second call retries (a
    second discovery request).

### 5.4 Measurement — the acceptance criterion

13. A real `createOidcClient` on a cold client, driven through `resolveSession` with a `ResolveDeps`
    whose `prepareRefresh` and `refresh` go to that client. The fixture IdP is
    `tests/fixtures/jwks.ts`. It gets two additions: a per-endpoint response delay, and a request
    log with a timestamp for each discovery, token and JWKS request. The response carries an
    `id_token`, so the JWKS call happens. The record's `idTokenClaims` use the fixture's issuer and
    `sub` (`jwks.ts:222-224`), so the refresh succeeds and `session.refreshed` is logged, and the
    D5 `id_token_mismatch` branch does not run. A store wrapper records when `tryAcquireLock`
    returns true and when `releaseLock` is called.

    The pass or fail assertion is CAUSAL, not a duration:
    - no discovery request reaches the fixture while the lock is held;
    - exactly one token request and exactly one JWKS request reach it while the lock is held;
    - exactly one discovery request reaches it before the lock is taken.

    The delays (discovery 0.6T, token and JWKS 0.3T each, T = 1000 ms) make the durations
    readable. Each leaves at least 400 ms before the client timeout. (Changed at plan review from
    0.9T and 0.55T: 0.9T left only 100 ms, and a late timer on a loaded runner would time out.) The durations are recorded, not asserted: the lock hold time, and the time of the
    whole `resolveSession`. The plan records them in § 7.

    The test is written first and run against the code before the fix. There it must FAIL on the
    first assertion (a discovery request while the lock is held). The plan records the lock hold
    time of that run in § 7.

    A second case uses a hanging discovery (the existing `startDiscoveryFailureFixture('hang')`)
    and asserts that `tryAcquireLock` is never called.

The worst case with every call AT its timeout is a failed refresh, not a slow success. So the
measurement uses delays below T, and the hanging case covers the timeout.

Test 13 runs in the default `test` task. Its assertions do not depend on timing, and it takes
about 2 s.

### 5.5 Proof that the tests bite

After the fix, make each change below, run the tests, and then restore the line by editing it
back, not by `git checkout` (which would also discard the fix):

- delete `await prepareRefresh()` in `single-flight.ts`: tests 1 and 13 go red;
- replace `resolveDepsFor`'s `prepareRefresh` with `async () => {}`: test 9 goes red;
- move the `deadline` line back before `prepareRefresh`: test 8 goes red;
- make `ensureDiscovered` a no-op: test 10 goes red;
- remove the re-read on failure (use `rec`): test 7 goes red;
- remove the `rec.refreshToken !== undefined` condition: test 3 goes red.

Every change must compile, so that the red comes from a test, not from `tsc`.

## 6. Residuals

- **Store calls under the lock are not in the 2T rule.** The lock TTL starts at the Redis
  `SET NX PX` inside `tryAcquireLock` (`single-flight.ts:144`). The post-lock `store.get` (`:150`)
  and the fenced `store.set` (`:278`) then run under the lock. Each store call is bounded only by
  `withOperationDeadline` at `DEADLINE_FACTOR` (4) × `PAIGASUS_SESSION_REDIS_TIMEOUT_MS`
  (`src/adapters/operation-deadline.ts:38`), 4000 ms at the default. So one slow `get` that still
  succeeds plus 2T is 11000 ms at the defaults, which is more than the 10000 ms TTL. Event-loop lag
  also delays `AbortSignal.timeout`. SMA-704 does not fix this. On 2026-09-27 Sven decided not to open
  a follow-up issue for it, so this residual is recorded here and in the README only.
- A custom `OidcClient` whose `ensureDiscovered` is a no-op, or whose `refresh` re-runs discovery,
  puts discovery back under the lock. Only the shipped adapter is covered.
- JWKS key rotation: an unknown `kid` makes oauth4webapi refetch JWKS inside the refresh. That
  refetch replaces the JWKS call; oauth4webapi fetches at most once per validation
  (`getPublicSigKeyFromIssuerJwksUri`, oauth4webapi `:1025-1098`). So it stays inside 2T. It is not
  discovery.

## 7. Measurements

To be filled in by the implementation: test 13 before the fix (lock hold time, discovery requests
under the lock) and after the fix.

## 8. Out of scope

- The store-call budget under the lock (§ 6; no issue tracks it).
- A readiness gate or an eager discovery at start (SMA-705).

## 9. Spec challenge (2026-09-27)

Verdict: APPROVE WITH CHANGES. Folded in:

- BLOCKER, no test guards the production wiring → `resolveDepsFor` (§ 3.3), test 9, § 5.5.
- MAJOR, the failure path was not today's outcome → the refresh-token condition, the re-read
  (§ 3.2), tests 3 and 7, and the stated differences.
- MAJOR, "true by construction" overclaimed → § 2 wording, the store-call residual (§ 6).
- MAJOR, the timing test could flake → causal assertions, asymmetric delays, T = 1000 ms, the named
  fixture, seeded claims (test 13).
- MINOR: all callers and fakes named (§ 3.3); test 1 macrotask; test 8 uses `Date.now()`; test 6
  (a rejection code); tests 10 and 11; the README bullet (§ 3.5); the per-call `signal()` wording
  (§ 1); the waiter latency (§ 3.5, § 4); `stage: 'discovery'` on the log line (§ 3.2); the cache
  key version (§ 3.4); the eager warm-up alternative (§ 2).

Considered and not done:

- A fail-closed `refresh()`: see § 2. Test 9 guards the wiring at a lower cost.
- One port object for `prepareRefresh`, `refresh` and `revoke`: `resolveDepsFor` already binds all
  three to one `runtime.oidc`. A new `ResolveDeps` shape is a larger change with no added
  guarantee here.
- A separate suite for test 13: its assertions are causal, not timing, so it stays in `test`.
