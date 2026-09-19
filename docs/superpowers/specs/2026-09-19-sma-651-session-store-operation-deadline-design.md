# SMA-651 — the auth session store needs a per-operation deadline

- Linear: SMA-651 (related: SMA-648 and SMA-650, the same class of gap in `@paigasus/console-core`'s
  descriptor cache; SMA-626, which pinned the store's current client options; SMA-653, the route
  handlers' missing 503 mapping)
- Date: 2026-09-19
- Revision: 2, after the adversarial challenge (changelog in § 12). Revision 1's design was approved
  in chat by Sven on 2026-09-19.
- Scope: `ts/packages/paigasus-auth` — `src/adapters/redis-store.ts`, a new
  `src/adapters/operation-deadline.ts`, `src/core/errors.ts`, `src/ports/logger.ts`,
  `src/config.ts`, `src/runtime.ts`, the package README, and tests. One CLAUDE.md bullet carries a
  stale "still open, SMA-651" statement (§ 10). No change to `@paigasus/console-core`,
  `@paigasus/discovery` or any app.

## 1. Problem

`@paigasus/auth`'s Redis session store can hang a render without a bound, in two places.

**In flight.** The client (`src/adapters/redis-store.ts:239-250`) sets `disableOfflineQueue: true`,
`commandOptions.timeout`, `socket.connectTimeout` and `socket.reconnectStrategy`. It sets no
`socketTimeout` and no `pingInterval`. `commandOptions.timeout` bounds a command only while it is
QUEUED: its listener is removed when the command is written (`commands-queue.js:427-430`). After the
write, nothing bounds it. A Redis that accepts a command and never replies blocks the call without
end. Nothing tears down the wedged socket either: with no `socketTimeout`, only TCP keepalive can,
and that takes minutes on default kernel settings.

**At the first connect.** `createRedisSessionStore` awaits `client.connect()`
(`redis-store.ts:259-263`). `connectTimeout` bounds one TCP attempt only, and `#connect` loops while
the reconnect strategy returns a number (`client/socket.js:173-230`). This store's strategy always
returns a number, so `connect()` never rejects for an unreachable Redis, and the `catch` at
`:259-263` is dead code for that case. The runtime is built lazily, on the first request
(`ts/apps/iam-console/lib/auth.ts:5`, `app/auth/[...auth]/route.ts:17`; no `instrumentation` file
exists), and `getAuthRuntime` caches the pending promise (`src/runtime.ts:218-229`). So a Redis that
is down at a process's first request blocks every request of that process until Redis answers.

`requireSession()` calls `getSession()`, which calls `resolveSession()`, whose first call is
`store.get` (`src/core/single-flight.ts:101`). Every console page calls `requireSession()` before
any other work, so SMA-650's descriptor-cache deadline does not help: the session read comes first.

`tests/adapters/redis-client-options.test.ts` pins the options that are set. It does not cover
either phase above.

## 2. Measured facts (read from `@redis/client` 6.2.1, the installed version)

`ts/pnpm-workspace.yaml` pins `redis: ^6.2.1`, and only one copy is installed, so the seven facts in
SMA-650's spec § 2 apply here without change. The ones that decide this design are restated. Paths
are under `ts/node_modules/.pnpm/@redis+client@6.2.1/node_modules/@redis/client/dist/lib/client/`.

1. **`socketTimeout` is an idle timer** (`socket.js:293-301`). It fires after that many ms with no
   socket activity. Any read OR write resets it. When it fires, the socket is destroyed with a
   `SocketTimeoutError`, and `#onSocketError` reconnects if `#isOpen` and the strategy returns a
   number (`socket.js:313-336`).
2. **node-redis writes exactly ONE more PING after a wedge starts, then stops**
   (`index.js:691-703`: `#setPingTimer` re-arms only in `.finally()`). So without our traffic, the
   worst case before the idle timer fires is `pingInterval + socketTimeout`.
3. **Nothing else writes to this socket during a wedge.** Handshake commands go out only on a new
   socket; `get`'s DEL is sent only after a reply; `eval` sends plain EVAL, with no NOSCRIPT retry.
   (Verified by the challenge.)
4. **`client.destroy()` and `client.close()` are terminal.** Both set the socket's `#isOpen = false`
   (`socket.js:372-384`), and the reconnect branch requires `#isOpen`. Both THROW
   `ClientClosedError` when `#isOpen` is already false. So `destroy()` after `close()` throws.
5. **With `disableOfflineQueue`, a command sent while the client is not ready fails at once**
   (`index.js:1125-1132`, `ClientOfflineError` or `ClientClosedError`). At a teardown, every
   in-flight command is flushed with an error BEFORE the next `ready` (`index.js:644-648`), so a
   stale command cannot settle after a reconnect.
6. **`client.close()` waits for pending commands** (`index.js:1485-1508`). It resolves only when the
   queue is empty, and it checks only on a `data` event. Against a wedged Redis with a command in
   flight it never resolves.
7. **A custom reconnect strategy receives `(retries, cause)`** (`socket.js:52-70`, `:140-160`). A
   return of `false` or an `Error` stops reconnection for good. The existing
   `reconnectStrategy(retries)` (`redis-store.ts:63-65`) ignores `cause` and returns
   `Math.min(retries * 100, 2000)`, so it reconnects after a `SocketTimeoutError` too.

## 3. Decisions

T is `PAIGASUS_SESSION_REDIS_TIMEOUT_MS` (default 1000 ms, `src/config.ts:51`).

### D1 — the idle client must survive (client options)

Add `pingInterval: T` and `socket.socketTimeout: 2T`. Keep `commandOptions.timeout: T`,
`socket.connectTimeout: T`, `disableOfflineQueue: true` and the existing `reconnectStrategy`.

- `pingInterval <= socketTimeout / 2` holds for every T, so a healthy idle socket always sees a PING
  and its reply before the idle timer fires. This is the SMA-648 rule.
- In THIS package a missing ping does not close the client for good (fact 7), unlike SMA-648. It
  makes the idle timer drop and reopen the socket about every 2T. In each reconnect gap a render
  gets `ClientOfflineError` and the user is sent to login. So the ping is still required; the test
  in § 8.2 E1 detects a missing ping by counting reconnects.
- `socketTimeout` stays because it is the ONLY thing that tears down a wedged socket (§ 1). Without
  it the circuit in D3 could not repair anything.

### D2 — a per-operation deadline of 4T

Every port method except `close` (`get`, `set`, `delete`, `tryAcquireLock`, `releaseLock`,
`putTransaction`, `takeTransaction`) runs with a deadline of `4T`.

Why 4T. Without our traffic, node-redis's own recovery takes up to `pingInterval + socketTimeout =
3T` (fact 2). A deadline below that is not incorrect: the circuit would open and `ready` would close
it. But it would open the circuit, and sign renders out, on stalls that node-redis would repair
itself. 4T keeps a margin above that, and it matches SMA-650, so both Redis clients in a console
process behave the same. That consistency is a deliberate part of the choice.

The deadline covers the whole method. `get` can send two commands (GET, then DEL for a poisoned
value, `redis-store.ts:156-172`), and 4T bounds both together.

### D3 — a circuit that stops our writes (this is the repair, not only a cost saving)

Under steady traffic our own writes reset the idle timer (fact 1), so it never fires and the socket
is never repaired. So the first deadline expiry opens a circuit.

- While it is open, every call except `close` rejects at once with phase `circuit-open` and writes
  nothing to the socket. The last write happens at or before the moment the circuit opens, so the
  idle timer fires within 2T of that moment, well inside the cooldown.
- The client's `ready` event closes the circuit. `ready` is the exact signal that node-redis
  finished a reconnect.
- The cooldown is `4T`, measured from the moment the circuit opens. When it ends with no `ready`,
  `openedAt` is set back to `null` and EVERY call goes through again (not a single probe; SMA-650
  rejected a half-open probe design). If the client is not ready, each call fails at once
  (fact 5) with a plain `SessionStoreUnavailable`, which does not open the circuit. If the client is
  ready but Redis is wedged again, the next expiry opens the circuit again and logs a second line.
- Only a DEADLINE expiry opens the circuit. A command that rejects fast (for example
  `ClientOfflineError` during a reconnect) passes through as a plain `SessionStoreUnavailable` and
  leaves the circuit as it is.
- Concurrency: concurrent operations expire at about the same time. Only the FIRST opens the circuit
  and logs (`if (openedAt === null)`). The others only reject.
- The circuit reads a MONOTONIC clock (`performance.now()`), so a wall-clock step cannot hold it
  open. (SMA-650 uses `Date.now()` and accepted that trade-off; this package does not need to.)
- The wrapper attaches `.catch(() => undefined)` to the timed-out inner promise. `Promise.race`
  already registers a handler, so this line is defensive, and no test can red on its removal; it
  stays as documentation of intent.
- The deadline timer is cleared in `finally` and `unref()`ed, so a bounded call never keeps the
  process alive.
- The wrapper adds no `error` listener. The silent listener at `redis-store.ts:257` stays the only
  one.

### D4 — the deadline is a decorator over the `SessionStore` port

The port already names "a decorator" as a legal composition (`src/ports/session-store.ts:23`), and
SMA-650 used the same shape. So:

- New file `src/adapters/operation-deadline.ts` exports `withOperationDeadline(inner: SessionStore,
  client: ReadySource, timeoutMs: number, logger: AuthLogger): SessionStore`, where
  `ReadySource = { on(event: 'ready', listener: () => void): unknown }`.
- `createRedisSessionStore` returns `withOperationDeadline(new RedisSessionStore(…), client, T,
  logger)`. `RedisSessionStore` itself does not change, except its `close()` (D5).
- The deadline wraps OUTSIDE `RedisSessionStore#guarded`, so a timeout is never rewrapped into a
  plain `SessionStoreUnavailable` by that catch-all (`redis-store.ts:141-147`).
- Unit tests drive the decorator with a fake inner store and a plain `EventEmitter`. They need no
  `vi.mock('redis')`.

### D5 — `close()` destroys at once

`close()` has no production caller in the repo (only tests call it), but it is part of the port and
fact 6 means a graceful close can hang. A graceful close buys nothing here. So
`RedisSessionStore#close()` becomes `if (client.isOpen) client.destroy()`, with no timer.

- The `isOpen` guard is required: a second `destroy()` throws (fact 4). `discovery.ts:344-356`
  guards the same call for the same reason.
- `destroy()` rejects in-flight commands with `DisconnectsClientError`; each becomes a
  `SessionStoreUnavailable` through `#guarded`.
- The decorator passes `close` through with no deadline and no circuit check, so an open circuit
  never refuses a close.
- The `RedisClient` duck type (`redis-store.ts:105-111`) gains `on`, `isOpen` and `destroy`, and
  loses `close`.

### D6 — the first connect waits at most T

`createRedisSessionStore` races `client.connect()` against T, the same shape as console-core's
`connectOnce` (`discovery.ts:133-161`).

- If connect settles first and rejects, the store throws `SessionStoreUnavailable`, as today.
- If T elapses first, the store is returned anyway, and the connect keeps running in the
  background. Until `ready`, every call fails at once (fact 5), so renders degrade to "no session"
  rather than hang. When Redis becomes reachable, the store works with no restart.
- The background connect promise gets a rejection handler, so it is never an unhandled rejection.
- The race timer is `unref()`ed.

### D7 — one error class, a subclass of `SessionStoreUnavailable`

```ts
export type SessionStoreTimeoutPhase = 'deadline' | 'circuit-open';
export class SessionStoreTimeout extends SessionStoreUnavailable {
  readonly phase: SessionStoreTimeoutPhase;
}
```

- It is a SUBCLASS, so every consumer keeps working unchanged: `getSession()`'s
  `instanceof SessionStoreUnavailable` branch (`src/next/get-session.ts:76`) still logs
  `store.unavailable` and returns `null`, and `resolveSession()`'s release-time catch
  (`single-flight.ts:218-222`) still swallows it. A grep found no consumer that matches on the
  message or on `name`.
- `code` stays `'session_store_unavailable'` (inherited).
- `name` is set explicitly to `'SessionStoreTimeout'`, because a production bundle can mangle
  `constructor.name`.
- The message names the operation (one of SEVEN fixed literals) and the deadline in ms. It never
  holds the DSN, the URL or a node-redis error.
- It is NOT exported from `src/server.ts`. No consumer can observe it (`getSession` swallows every
  error, and the routes do not classify errors), and `RefreshRejected` sets the precedent
  (`src/core/errors.ts:46-48`). Tests import it from `src/`.

### D8 — one log line per circuit open

- `CreateRedisSessionStoreOptions` gains an optional `logger?: AuthLogger`, default `noopLogger`.
- `src/runtime.ts` passes its logger. Today it computes `deps.logger ?? noopLogger` at `:154`, after
  it creates the store at `:143`, so the logger is computed first.
- `AuthEventName` (`src/ports/logger.ts:14-25`) gains `'store.operation_timeout'`, with fields
  `{ operation, deadlineMs }`, logged once per circuit open. Widening the union needs no change in
  console-core's logger adapter.
- The silent `error` listener does not change. No connection-loss logging.

### D9 — an upper bound on T

`PAIGASUS_SESSION_REDIS_TIMEOUT_MS` gains `.max(536_870_911)`. Node changes any timer delay above
2^31 − 1 ms to 1 ms, and the largest timer here is `4T`. Without the bound, a very large T makes
every deadline, ping and idle timer fire after 1 ms. The `millis` helper (`src/config.ts:20`) is
shared, so the bound is added to this one key only.

## 4. Behaviour during a wedge

With the defaults (T = 1000 ms), a Redis that accepts commands and never replies gives:

- The first affected store call waits at most 4T = 4 s, then fails. `getSession()` returns `null`
  and `requireSession()` redirects to login.
- During the cooldown, every call fails at once and every render redirects to login.
- The idle timer fires within 2T of the circuit opening and node-redis reconnects. While Redis
  stays wedged, the reconnect handshake does not finish and no `ready` fires, so calls after the
  cooldown fail at once with `ClientOfflineError`. When Redis answers again, `ready` closes the
  circuit and renders work.

**The store adds at most one deadline to a render.** After the first expiry the circuit is open, so
later store calls in the same `resolveSession()` fail at once. Other costs are unchanged: a waiter
can still spend `lockWaitMs` (3000 ms) waiting for the lock, and a lock holder can still spend up to
2 × `PAIGASUS_OIDC_HTTP_TIMEOUT_MS` on the refresh.

## 5. User-visible consequences (read this before approving)

**A redirect to login destroys the session.** `handleLogin` deletes the presented session record and
clears the cookie (`src/http/routes.ts:188-196`). So a render that fails because of an expiry or an
open circuit sends the user to `/auth/login`, and as soon as that route runs against a store that
answers, the session is gone. A user with no IdP SSO session must enter credentials again.

Before this change, a stall of a few seconds made pages slow, and sessions survived. After it, a
stall longer than 4T — a BGSAVE fork stall, an AOF fsync stall, a managed failover — signs out
every user who renders in that window, permanently for those sessions.

This is not a new mechanism: a refused connection (Redis down) already takes this exact path today.
What changes is that a LONG STALL now takes it too, instead of hanging. The trade is a bounded,
lossy failure for an unbounded hang. An operator tunes the threshold with
`PAIGASUS_SESSION_REDIS_TIMEOUT_MS`. A 503 with a retry for `SessionStoreTimeout` would keep the
session, but it conflicts with SMA-506's rule that `getSession` never raises; it is recorded as a
rejected alternative (§ 6).

**Logout can be lost during a wedge.** `handleLogout` reads the record before it deletes it
(`routes.ts:356-360`). During a wedge, the read now expires (or the circuit refuses it), so the
delete is never sent, the route fails, and the session stays live for its TTL. Before this change,
if the stall ended, the hung read finished and the delete landed, because the handler keeps running
after the client disconnects. The window is narrow, and the user sees an error rather than a false
"signed out". This is a route-handler error-handling question, so it is added to SMA-653's scope
(the logout route must attempt the delete even when the read fails), not fixed here.

## 6. Late effects of a timed-out command

A command that expires can still run when Redis replies later. At each call site a store error was
already possible, so the only NEW outcome is the logout case in § 5.

| operation | late effect | outcome |
|---|---|---|
| `get` | none, or a late DEL of a poisoned value | harmless |
| `set` (CAS, after a refresh) | the rotated refresh token lands | correct; the fence on `rev` still applies |
| `set` (callback insert, `expectedRev = null`) | a record with live tokens lands, but the browser never got the cookie | an orphan that expires after `ttlMs`; unreachable without the sid |
| `tryAcquireLock` | the lock is held until its TTL (10 s default) | a waiter never refreshes on timeout (single-flight invariant 2) |
| `releaseLock` refused by the circuit, or expired | the lock stays held until its TTL | each render of that session waits `lockWaitMs` (3 s); if the access token has hard-expired the user is sent to login, which then deletes the session (§ 5) |
| `putTransaction` | the login state lands after the login request failed | expires by its own TTL, unused |
| `takeTransaction` | the login state is consumed after the callback failed | the user must start the login again |
| `delete` | the record is deleted late | the delete intent still holds |

**Lock TTL.** `runtime.ts:125-127` asserts `2 × OIDC_HTTP_TIMEOUT < LOCK_TTL`. With the store
deadline, a holder's time under the lock can reach 2 × 3500 + 4000 = 11000 ms, above the 10000 ms
default TTL. This is recorded, not asserted: after the first expiry the circuit fails every later
store call at once, so the holder cannot WRITE after that point, and any late write is fenced on
`rev` (invariant 5). Asserting it would reject today's shipped defaults.

## 7. Rejected alternatives

- **Deadline only, no `socketTimeout`, no ping, no circuit.** Each call is bounded, but a
  black-holed socket is never repaired until TCP keepalive gives up. Rejected by Sven.
- **One shared helper for `@paigasus/auth` and `@paigasus/console-core`.** SMA-650 D1 rejected it:
  the adapters, clients and error types differ. Possible later work.
- **Destroy and rebuild the client on expiry.** `destroy()` is terminal (fact 4), and the store has
  no recreate path. Going silent achieves the repair with node-redis's own reconnect.
- **Connection-loss logging.** Offered and not chosen; it would change the silent listener SMA-626
  pins.
- **A deadline below 3T.** Correct but noisier (§ D2).
- **`requireSession` shows a 503 for a `SessionStoreTimeout` instead of redirecting.** Keeps the
  session, but it contradicts SMA-506's "`getSession` never raises" contract and changes every
  console page's error surface. Not chosen; see § 5.
- **A graceful `close()` with a deadline, then `destroy()`.** Revision 1's design. `destroy()` after
  `close()` throws (fact 4), and a graceful close has no caller that needs it.

## 8. Tests

### 8.1 Unit tier (`test` task, no Docker)

New `tests/adapters/operation-deadline.test.ts`, driving the decorator with a fake inner store and
an `EventEmitter`. Fake timers with an explicit `toFake` list (`setTimeout`, `clearTimeout`,
`performance`).

- U1: a call that never settles rejects with `SessionStoreTimeout`, phase `deadline`, at 4T, and is
  still pending at 4T − 1.
- U2: that rejection is `instanceof SessionStoreUnavailable`, its `code` is
  `'session_store_unavailable'`, its `name` is `'SessionStoreTimeout'`.
- U3: after an expiry, the next call rejects at once with phase `circuit-open`, and the inner store
  records NO call.
- U4: after the cooldown with no `ready`, the next call reaches the inner store.
- U5: after the cooldown, a call that expires again re-opens the circuit and logs a SECOND line.
- U6: a `ready` event closes the circuit at once.
- U7: three concurrent expiries log `store.operation_timeout` exactly once.
- U8: a call that settles in time leaves no pending timer (`vi.getTimerCount() === 0`).
- U9: an inner call that REJECTS fast (a plain `SessionStoreUnavailable`) passes through unchanged
  and does not open the circuit.
- U10 (table-driven, all seven methods): each one, held pending, rejects with phase `deadline`
  naming its own operation. This pins the wiring of every method.
- U11: `close` passes through while the circuit is open, and has no deadline.
- U12: no message or log field holds the DSN.

`tests/adapters/redis-client-options.test.ts` gains:

- `pingInterval` equals T and `socket.socketTimeout` equals 2T, and `pingInterval <=
  socketTimeout / 2`.
- The CAPTURED `socket.reconnectStrategy` returns a number when called as
  `(n, new SocketTimeoutError(2T))` for n in 0..50. `SocketTimeoutError` comes from
  `vi.importActual('redis')`, because the file mocks `redis` with `createClient` only.
- Guard 2 is tightened: exactly ONE `error` registration, not "at least one".
- The fake client gains `isOpen` and a `destroy` that throws `ClientClosedError` when called while
  not open (the real behaviour, fact 4). `close()` is called twice; it destroys once and never
  throws.
- The stale comment at `:26-28` ("deliberately omits `on`") is corrected.

New unit tests for D6 (bounded connect), with the same mock: a `connect()` that never settles makes
`createRedisSessionStore` resolve at T; a `connect()` that rejects before T makes it throw
`SessionStoreUnavailable`.

New runtime wiring test (in `tests/runtime.test.ts` or a new file): mock `redis`, call
`createAuthRuntime` with `PAIGASUS_SESSION_STORE: 'redis'` and a logger, hold one command pending
past 4T, and assert that `store.operation_timeout` reaches THAT logger. This pins D8's runtime
line.

New config test: `PAIGASUS_SESSION_REDIS_TIMEOUT_MS=536870912` is refused.

### 8.2 Docker tier (`test-e2e` task)

New `tests/containers/redis-store-idle.test.ts`, run by the existing `vitest.containers.config.ts`.
It uses `redis:8-alpine` with `--requirepass`, so a log line can be checked for the password. T is
1000 ms. A second admin client, never the store's own, issues `CLIENT PAUSE` and reads `INFO stats`.
No skip hatch. Errors are matched on `name` and `phase` STRINGS, not with `instanceof`, so the file
runs against code that does not yet have `SessionStoreTimeout` (the SMA-648 pattern).

- E1 (idle): connect the admin client first. Write a session, read `total_connections_received`,
  sleep `3 × socketTimeout + 500` ms, then `get` returns the record, and
  `total_connections_received` did not change (no reconnect happened).
- E2 (hang under steady traffic): drive `get` fire-and-forget every 500 ms, `CLIENT PAUSE 6000 ALL`,
  sleep `4T + 2 × 500` ms inside the pause, stop the driver. At least one failure has
  `name === 'SessionStoreTimeout'` and `phase === 'deadline'`.
- E3 (repair under traffic): drive traffic for the WHOLE of a `CLIENT PAUSE 9000 ALL` (above 4T + 2T
  plus a margin). Assert that `total_connections_received` rose, which proves that the circuit's
  silence let the idle timer fire and node-redis reconnect. Then `get` eventually (5 s poll budget)
  returns the record.
- E4 (no secret): at least one `store.operation_timeout` line exists, and no log line holds the
  password.
- E5 (close under a wedge): with a command in flight under `CLIENT PAUSE`, `store.close()` resolves
  at once and does not throw.

### 8.3 Red-first (mandatory; record each observed output in the plan's task notes)

- E1 must FAIL with `pingInterval` deleted (a reconnect is counted).
- E2 must FAIL on unmodified code (no failure is a `SessionStoreTimeout`).
- E3 must FAIL with `socketTimeout` deleted, and must FAIL with the circuit's opening line deleted
  (our traffic keeps resetting the idle timer, so no reconnect is counted).
- U10 must FAIL with any one method's wrap deleted.
- The runtime wiring test must FAIL with the `logger:` line deleted from `runtime.ts`.
- U3 must FAIL with the circuit's opening line deleted. U5 must FAIL with the cooldown's
  `openedAt = null` reset deleted.
- E5 must FAIL on revision 1's design (`close()` then `destroy()`), and on unmodified code (graceful
  `close()` hangs).

### 8.4 Registration

`paigasus-auth-ts:test-e2e` already includes `tests/**/*` through `@group(tests)`, runs with
`cache: false`, and is selected by `:test-e2e` in CLAUDE.md's ci-targets command. The plan confirms
this with `moon task paigasus-auth-ts:test-e2e` before relying on it.

## 9. Operator requirements (for the README)

The README gains a Redis section that lists the complete ACL the store needs: `+get`, `+set`,
`+del`, `+eval`, `+ping`, and `+client|setinfo` (node-redis sends CLIENT SETINFO at connect and
ignores its error, so it is optional). It states the deadline (4T), the cooldown (4T), the PING rate
(one per T per client), the `store.operation_timeout` event, the § 5 consequences, and that T has a
maximum (D9) and no minimum (a floor would break `pingInterval <= socketTimeout / 2`).

## 10. Documents to correct

- `CLAUDE.md`, the "node-redis `socket.socketTimeout` is an IDLE timer" bullet: it says
  "`@paigasus/auth` still has none — SMA-651". Update it to name the new bound.
- `redis-store.ts:54-62`: the comment says "`disableOfflineQueue: true` plus the per-command timeout
  already reject in-flight calls quickly". That is false for the in-flight phase (§ 1). Correct it,
  and state that the strategy must return a number for a `SocketTimeoutError` too.
- The file header of `redis-store.ts`: name the decorator and the bounded connect.
- The memory note `node-redis-command-vs-socket-timeout.md` ("Still open: … SMA-651"): update after
  merge. It is outside the repo.

## 11. Risks

1. **A long stall now signs users out** (§ 5). This is the main cost of the change, accepted in
   exchange for a bound.
2. **The circuit refuses calls for up to 4T** after it opens, even if Redis recovers sooner and
   `ready` has not yet fired.
3. **More PINGs.** One per T per Redis client. A one-zone console process has two clients (the
   session store and the descriptor cache); a two-zone process (SMA-513, `runtime.ts:189-194`) has
   three.
4. **An event-loop stall longer than 4T opens the circuit against a healthy Redis.** The timers
   phase runs every expired deadline before the replies already in the socket buffer are read. So T
   must be sized against the worst event-loop lag, not only against Redis latency.
5. **A very small T.** T = 1 gives one PING per ms and a 4 ms deadline. There is no floor (§ 9).

## 12. Changelog, revision 1 -> 2

The adversarial challenge returned APPROVE WITH CHANGES. Each finding below was checked against the
named file before it was folded in.

**Folded in — BLOCKER.** Revision 1's `close()` called `destroy()` after a graceful `close()`. Both
set `#isOpen = false`, and `destroy()` then throws `ClientClosedError` (`socket.js:378-381`). D5 now
destroys at once behind an `isOpen` guard, and E5 plus the throwing fake prove it.

**Folded in — MAJOR.**
- The first `connect()` is unbounded and runs on a render; revision 1 said the opposite. D6 bounds
  it at T. This widens the scope slightly, in the same failure class.
- E1's red-first claim was false: this package's strategy reconnects after an idle timeout, so a
  missing ping causes reconnects, not a dead client. E1 now counts reconnects.
- The test plan did not pin the wiring or the repair. Added U10 (all seven methods), the runtime
  wiring test, E3 (reconnect under traffic) and U9 (a fast failure does not open the circuit).
- Logout can be lost during a wedge. Stated in § 5 and moved to SMA-653's scope.
- A redirect to login destroys the session. § 5 and Risk 1 now say so plainly.

**Folded in — MINOR.** Half-open behaviour made explicit, with U5. `close` passes the circuit.
revision 1's U8 dropped as a guard (it cannot red). E2 matches strings. `vi.importActual` for
`SocketTimeoutError`. Guard 2 asserts exactly one `error` listener. The duck type's members listed.
The false comment at `redis-store.ts:60-61` added to § 10. § 4 wording corrected. Two rows added to
§ 6. The lock-TTL interaction recorded. A monotonic clock. An upper bound on T (D9). The full ACL
list. Risks 3 and 4 detailed.

**Folded in — QUESTIONS.** 4T is justified honestly (margin plus consistency with SMA-650).
`SessionStoreTimeout` is not exported. The deadline is a decorator over the port. Seven literals,
not eight.

**Not folded in.** Asserting `2 × HTTP + 4T < LOCK_TTL`: it would reject today's shipped defaults,
and § 6 shows the fence keeps it safe. Recorded instead.
