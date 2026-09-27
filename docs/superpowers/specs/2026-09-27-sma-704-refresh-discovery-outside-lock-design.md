# SMA-704 — run OIDC discovery before the session lock, not under it

- Issue: [SMA-704](https://linear.app/smaschek/issue/SMA-704)
- Package: `ts/packages/paigasus-auth` (`@paigasus/auth`)
- Related: SMA-656 (its spec, § 8, defers this gap to SMA-704)
- Status: design approved in chat on 2026-09-27; written spec awaits review

## 1. Problem

`createAuthRuntime` (`src/runtime.ts:130-143`) refuses a configuration unless
`2 * PAIGASUS_OIDC_HTTP_TIMEOUT_MS < PAIGASUS_SESSION_LOCK_TTL_MS`. Call the timeout T. The rule
counts the calls that `refresh()` makes while `resolveSession` holds the session lock:

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
- `refreshTokenGrant()` (`:999-1030`) sends the token request with `signal(timeout)`, then, when the
  response has an `id_token`, the non-repudiation hook (`:748-761`) fetches JWKS with the same
  `signal(timeout)`.

So a warm refresh is at most 2T, and a cold refresh is at most 3T. This is reasoned from the code.
The measurement test in § 5 measures it.

## 2. Decision

Discovery runs BEFORE `resolveSession` takes the lock. Under the lock, `getConfig()` is then a
resolved promise and sends nothing, so the existing 2T rule is true by construction.

Rejected alternatives:

- **Put discovery in the bound (3T < lock TTL).** This needs a new default (lock TTL 12000 ms or
  T 3000 ms). A configuration that starts today, for example T = 4000 ms with TTL = 10000 ms, would
  then fail at start after an upgrade. A larger TTL also blocks refresh for longer after a holder
  crashes.
- **Both.** With discovery outside the lock, a 3T check refuses configurations that are safe.

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
promise.

The SMA-656 D10 no-revoke invariant does not change: `ensureDiscovered()` sends no token request,
and `OidcDiscoveryFailed` still comes only from `getConfig()`.

### 3.2 Single-flight — `src/core/single-flight.ts`

`ResolveDeps` gets a REQUIRED member:

```ts
/**
 * Called once per resolveSession, only when a refresh is due, BEFORE the first tryAcquireLock
 * (SMA-704). Production wires it to OidcClient.ensureDiscovered, so the OIDC discovery request
 * never runs under the lock. A rejection is a transient refresh failure.
 */
prepareRefresh: () => Promise<void>;
```

It is required, not optional, so that a caller cannot omit it and silently put discovery back under
the lock.

Order in `resolveSession`:

1. `store.get(sid)`; the absolute-expiry check; `shouldRefresh`. Unchanged. A read of a session
   that needs no refresh does not call `prepareRefresh`.
2. NEW: `await prepareRefresh()`.
3. `deadline = Date.now() + lockWaitMs`. This line moves AFTER step 2, so a waiter keeps its full
   `lockWaitMs` after discovery.
4. The lock loop. Unchanged.

Concurrent requests on one process share one discovery request, because `getConfig()` caches the
promise.

Failure of `prepareRefresh`: the outcome is the outcome of today's transient refresh failure. Today
a discovery failure throws `OidcDiscoveryFailed` from `refresh()` under the lock. It is never a
`RefreshRejected`, so it takes the transient branch. To keep one definition of that branch, the
failure logic in the `refresh` catch (`:176-225`) moves into one helper, and both sites call it.
For the new site the helper:

- logs `session.refresh_failed` with `reason: 'transient'` and `degraded`, and with no `oauthError`
  (a discovery failure has no OAuth code — `refreshFailureCode` returns `undefined` for it);
- returns `{ ...rec, refreshState: 'failed' }` when `Date.now() < min(rec.accessExpiresAt,
  rec.absoluteExpiresAt)`;
- otherwise throws the error, as today. `get-session.ts`'s catch then handles it as it does today.

The lock is never taken on this path. The new site passes `rec`, the record read before the lock,
because no lock is held and there is no fresher record.

The helper covers ONLY the transient outcome. The site under the lock keeps its `rejected` branch
(the `session.refresh_failed` line with `reason: 'rejected'`, the delete, `session.deleted`) and
calls the helper for everything else. The new site calls the helper directly and never checks for
a rejection. So the new site can never delete a record. Test 3 and test 4 pin that.

### 3.3 Wiring — `src/next/get-session.ts`

```ts
prepareRefresh: () => runtime.oidc.ensureDiscovered(),
```

Every other `resolveSession` caller (tests and fixtures, for example
`tests/fixtures/refresh-worker.ts` and `tests/containers/single-flight-redis.test.ts`) passes a
`prepareRefresh`. A fake that has no discovery passes `async () => {}`.

### 3.4 The bound and its documentation

- `src/runtime.ts`: the 2T check and its message do not change. The invariant 3 comment adds that
  discovery is NOT one of the calls under the lock, because `resolveSession` calls
  `prepareRefresh` first (SMA-704).
- `src/core/single-flight.ts`: invariant 5's comment gets the same statement.
- `src/adapters/oidc.ts`: the header's "DISCOVERY IS LAZY" paragraph names `ensureDiscovered` and
  the reason for it.
- `README.md` rows for `PAIGASUS_OIDC_HTTP_TIMEOUT_MS` and `PAIGASUS_SESSION_LOCK_TTL_MS`: add
  that discovery runs before the lock and is not in the 2x budget.

## 4. What does not change

- Defaults, the `authEnvShape`, and the 2T rule.
- The Helm chart. It sets neither variable (checked: no match in `charts/`).
- Login, callback, logout. They take no session lock.
- The revokes after release (SMA-681).
- Latency on a warm process. On a cold process, a request that must refresh waits up to T for
  discovery once, before the lock. Today it waits for the same discovery, under the lock.

## 5. Tests

### 5.1 Unit — `tests/core/single-flight.test.ts`

With a recording store and a recording `prepareRefresh`:

1. When a refresh is due, `prepareRefresh` resolves BEFORE the first `tryAcquireLock` call.
2. When no refresh is due, `prepareRefresh` is not called.
3. `prepareRefresh` rejects, and the access token is live: the result has `refreshState: 'failed'`,
   `session.refresh_failed` is logged with `reason: 'transient'` and `degraded: true`,
   `tryAcquireLock` is never called, and nothing is deleted.
4. `prepareRefresh` rejects, and the access token is expired: `resolveSession` rejects with that
   error, `tryAcquireLock` is never called, and nothing is deleted.
5. The `lockWaitMs` deadline starts after `prepareRefresh` resolves. Another holder keeps the lock
   for the whole test, and `prepareRefresh` takes longer than `lockWaitMs`. The time from
   `prepareRefresh` resolving to the `'pending'` return is at least `lockWaitMs`, and the waiter
   makes more than one lock attempt. With the deadline set before `prepareRefresh`, the waiter
   makes exactly one attempt and returns at once, so this test fails. (The loop always makes one
   attempt before it checks the deadline, so "at least one attempt" would not bite.)

### 5.2 Adapter — `tests/adapters/oidc.test.ts`

6. `ensureDiscovered()` then `refresh()` sends exactly one discovery request in total.
7. A failed `ensureDiscovered()` rejects with `OidcDiscoveryFailed`, and a second call retries.

### 5.3 Measurement — the acceptance criterion

8. A real `createOidcClient` on a cold client, against a fixture IdP that delays the discovery
   response, the token response and the JWKS response by about 0.9 T each (T small, for example
   300 ms). The response carries an `id_token`, so the JWKS call happens. A store wrapper records
   the time between `tryAcquireLock` returning true and `releaseLock`. Assertions:
   - the lock is held for less than 2T;
   - the whole `resolveSession` takes more than 2T (so discovery really ran, outside the lock).

   The test is written first and run against the code before the fix. There it must FAIL, with the
   lock held for about 2.7T. The plan records the measured value in this spec's § 7.

The worst case with every call AT its timeout is a failed refresh, not a slow success, so the test
uses delays just below T. A companion case with a hanging discovery (the existing
`startDiscoveryFailureFixture('hang')`) asserts that `tryAcquireLock` is never called.

### 5.4 Proof that the tests bite

After the fix, delete the `await prepareRefresh()` line. Tests 1 and 8 must go red. Then delete only
the `deadline` move; test 5 must go red. Restore by editing the line back, not by `git checkout`.

## 6. Residuals

- A custom `OidcClient` whose `ensureDiscovered` is a no-op, or whose `refresh` re-runs discovery,
  would put discovery back under the lock. Only the shipped adapter is covered.
- JWKS key rotation: an unknown `kid` makes oauth4webapi refetch JWKS inside the refresh. That is
  still the one JWKS call bounded by T, so it stays inside 2T. It is not discovery.

## 7. Measurements

To be filled in by the implementation (test 8 before and after the fix).
