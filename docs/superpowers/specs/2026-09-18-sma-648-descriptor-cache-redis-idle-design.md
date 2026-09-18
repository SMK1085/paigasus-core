# SMA-648 — the descriptor cache's Redis client must survive an idle socket

- Linear: SMA-648 (related: SMA-641, which found it)
- Date: 2026-09-18
- Revision: 2, after the adversarial challenge (changelog in § 8)
- Scope: `@paigasus/console-core` (the client), `@paigasus/discovery` (the precondition), one new
  Docker-backed test tier in `@paigasus/console-core`.

## 1. Problem

`ts/packages/paigasus-console-core/src/discovery.ts` (`redisDescriptorCache`) makes its node-redis
client with these options:

```ts
createClient({ url, disableOfflineQueue: true, commandOptions: { timeout: timeoutMs },
  socket: { socketTimeout: timeoutMs * 2, connectTimeout: timeoutMs } });
```

`timeoutMs` is `PAIGASUS_SESSION_REDIS_TIMEOUT_MS` (default 1000), so `socketTimeout` is 2000 ms.
After 2 s with no traffic on the socket, the client closes and does not open again. Every later
read throws `The client is closed`, discovery reports `cache-unavailable`, and both consoles
disable their navigation. The issue has the measured transcript.

## 2. Root cause (read from `@redis/client` 6.2.1, the installed version)

Two facts together cause the failure. Each one alone is harmless.

1. **`socketTimeout` is an idle timer.** `dist/lib/client/socket.js:293-300` calls Node's
   `socket.setTimeout(socketTimeout)` and destroys the socket with a `SocketTimeoutError` when it
   fires. Node's timer measures **inactivity**, and both a read and a write reset it
   (`net.Socket#_writeGeneric` calls `_unrefTimer()`). So the timer fires on a quiet, healthy
   connection. It also fires on a command that is in flight with no reply, but **only while
   nothing else is written to the socket**. Under steady traffic, each new write moves the
   deadline, so a Redis that accepts commands and never replies can go unnoticed.
2. **The default reconnect strategy refuses a socket timeout.** `socket.js:403-407`,
   `defaultReconnectStrategy`: `if (cause instanceof SocketTimeoutError) return false;`. `false`
   means "do not reconnect, close the client" (`socket.d.ts:18-19`, `socket.js:142-151`).

So the first idle gap longer than `socketTimeout` closes the client for the life of the process.

Two repo documents state fact 1 wrongly, and that error is how the bug shipped:

- `ts/packages/paigasus-discovery/src/adapters/redis-cache.ts:63-65` and the package README
  (`README.md:97`) call `socketTimeout` "the end-to-end deadline … node-redis closes the socket if
  no reply arrives within that many milliseconds".
- The agent memory note `node-redis-command-vs-socket-timeout`, and its `MEMORY.md` index line,
  make the same claim.

`@paigasus/auth`'s session store (`src/adapters/redis-store.ts:239-250`) sets no `socketTimeout`, so
this bug does not affect it.

## 3. Decisions

| # | Decision | Reason |
|---|----------|--------|
| D1 | Keep `socketTimeout: timeoutMs * 2`. | It is the only bound on a hung command, even if a partial one (§ 2, fact 1). `createRedisDescriptorCache` asserts it, and that precondition stays. The issue's option 2 (remove it) is rejected. |
| D2 | Add `pingInterval: timeoutMs` to the client options. | node-redis sends `PING` every `pingInterval` ms while the socket is ready (`index.js:691-703`). The PING write, and then its reply, are socket activity, so a healthy idle socket never reaches the idle timer. The gap between "the ping is due" and "the idle timer fires" is `socketTimeout - pingInterval = timeoutMs`. That gap is the tolerance for event-loop lag. |
| D3 | Add a `reconnectStrategy` that returns a delay for **every** cause, `SocketTimeoutError` included, and never returns `false` or an `Error`. The delay is `Math.min(2 ** retries * 50, 2000)` ms plus 0–199 ms of jitter. | A real hang still fires the idle timer, even with D2. Without D3 the client then closes forever. There is no retry cap, for the reason `@paigasus/auth`'s `reconnectStrategy` records (`redis-store.ts:53-62`): one client, no recreate path, so a stop is permanent. Note: node-redis starts the FIRST reconnect at once and uses the strategy's value only as a type check (`socket.js:330-333`). The delay applies from the second attempt. |
| D4 | `createRedisDescriptorCache` asserts two more preconditions, checked **after** the four that exist: (a) `pingInterval` is a positive, finite number and `pingInterval <= socketTimeout / 2`; (b) `socket.reconnectStrategy` accepts a socket timeout. (b) rejects `undefined` and `false`, accepts a number, and for a function calls it once with `(0, new SocketTimeoutError(socketTimeout))` and requires a finite, non-negative number back. | This bug is the same class as the four options the helper already asserts: silent until production. (a) enforces D2's margin, not only "smaller than". (b) stops the next consumer that sets a ping but no strategy: one lost PING reply would close its client forever. The probe is cheap, and `redis` is already a runtime dependency of `@paigasus/discovery` (`package.json:23`). |
| D5 | Correct every `socketTimeout` doc (comment, thrown message, README, `CLAUDE.md` entry, memory note) to say: an idle timer that any read OR write resets; it bounds a hung command only while the socket is otherwise silent; `pingInterval` keeps an idle socket alive; a reconnect strategy must accept a `SocketTimeoutError`. The README's "all four are asserted" becomes "all six". | The wrong doc is the root of the shipped bug (§ 2). The corrected doc must not carry a new error. |
| D6 | Log a new app event, `discovery.redis_connection_lost`, with one field, `reason`. | Today `client.on('error', () => undefined)` hides the failure, so the only symptom was a degraded nav. |
| D7 | `reason` is a fixed value from `instanceof` checks: `socket_timeout` (`SocketTimeoutError`), `socket_closed` (`SocketClosedUnexpectedlyError`), or `other`. It never carries the error message or `constructor.name`. `SocketTimeoutDuringMaintenanceError` (Redis Enterprise maintenance only, `errors.js:86-91`) is not a subclass of `SocketTimeoutError` and maps to `other`. That is acceptable. | node-redis error messages can embed the DSN. The error classes do not set `name` (`errors.js:22-27`), and a production bundle can mangle `constructor.name`. The classes are exported from `redis`. |
| D8 | Log D6 **once per loss**, and **only for a real loss**: log when all three are true — the logger is armed, `client.isOpen`, and `!client.isReady`. Then disarm it. A client `ready` event arms it again. The logic lives in one small exported function, `watchConnectionLoss(client, log)`, so the default tier can unit-test it with a fake emitter. | node-redis emits `error` for events that do NOT drop the socket: an error reply to a PING (`index.js:698-701`, for example `-NOPERM`), a decoder error (`index.js:632-634`), and a PING that `destroy()` flushes (`index.js:1511-1514`). With a plain "log if armed" rule, one such event writes a false line and disarms the logger, and no `ready` follows, so every later real loss is silent. `#onSocketError` sets `isReady = false` before it emits (`socket.js:314-324`), and under D3 `isOpen` stays true during a real loss; after `destroy()`, `isOpen` is false. Errors before the first `ready` are connect failures, which `connectOnce` logs as `discovery.redis_connect_failed`. Handshake errors during a reconnect loop arrive while the logger is disarmed. |
| D9 | The regression test lives in a **new Docker-backed tier in `@paigasus/console-core`**, the same shape as `@paigasus/discovery` and `@paigasus/auth` (§ 4.4). | It drives the real call site (`descriptorCacheFor`), which is where the bad options lived. Sven chose this over a moved factory or a Docker-free RESP stub. |
| D10 | `resetDiscoveryForTest` destroys the client only when `client.isOpen`. This is the **first** change, before any red run. | `RedisSocket.destroy()` throws `ClientClosedError` on a closed client (`socket.js:378-381`). On `main`, T1's `afterEach` would throw before it clears the `globalThis` state, so T2–T5 would inherit a dead cache and fail for the wrong reason. The seam is also the documented shutdown seam, so the fix has production value. |
| D11 | In `connectOnce`, the `stage: 'connect'` log branch goes. The rejection handler stays, with no log, so a rejected `connect()` is never unhandled. | Under D3, `client.connect()` never rejects except on `destroy()` during a connect, and "connect failed" is the wrong line for that. The `connect_timeout` branch stays and still covers an unreachable Redis. |

Rejected or out of scope:

- **Reconnect only (issue option 3 alone).** The client closes after every idle gap and reopens
  at once. With `disableOfflineQueue: true`, a request in the gap fails. T1's "no `lost` line"
  check fails for this option, which is the evidence.
- **Ping only (issue option 1 alone).** One real hang, or one lost PING reply, still closes the
  client forever. T3 fails for this option.
- **A per-operation deadline in `afterConnect`.** Under steady traffic, `socketTimeout` does not
  find a Redis that accepts commands and never replies (§ 2, fact 1). This gap exists on `main`
  and is not the reported bug. **Out of scope, with a proposed follow-up Linear issue.** § 2 and
  D5 state the gap in the docs.
- **A floor on `pingInterval`.** `PAIGASUS_SESSION_REDIS_TIMEOUT_MS` accepts any positive
  integer, so a value of 1 gives one PING per ms. The dependency is documented in the
  `discovery.ts` comment instead; a floor would break D4 (a) for small values.
- **A `discovery.redis_reconnected` event.** Not requested. It is cheap once D8 exists, so it is
  offered at the approval gate as an option.
- **`@paigasus/auth`'s missing in-flight bound.** A different defect in a different package. No
  Linear issue exists; a follow-up is proposed at the gate.

## 4. Changes

### 4.1 `ts/packages/paigasus-console-core/src/discovery.ts`

- D10 first: `resetDiscoveryForTest` checks `isOpen`.
- `redisDescriptorCache` passes `pingInterval: timeoutMs` and
  `socket.reconnectStrategy: descriptorCacheReconnectStrategy` (D2, D3).
- `descriptorCacheReconnectStrategy(retries: number): number` is exported.
- `watchConnectionLoss(client, log)` is exported (D8). It registers the `ready` listener and the
  `error` listener. The `error` listener never reads the error message.
- `connectOnce` loses its `stage: 'connect'` log (D11).
- The file-header comment states the idle timer, the ping, the reconnect rule, the PING-rate
  dependency on `PAIGASUS_SESSION_REDIS_TIMEOUT_MS`, and that the Redis user needs `+ping` (a
  `-NOPERM` reply still keeps the socket alive, and D8 ignores it).

### 4.2 `ts/packages/paigasus-console-core/src/logger.ts`

- `AppEventName` gains `'discovery.redis_connection_lost'`.

### 4.3 `ts/packages/paigasus-discovery`

- `src/adapters/redis-cache.ts`: D4 (a) and (b), checked last, each with its own message; the D5
  doc corrections.
- `tests/redis-cache-options.test.ts`: the default `fakeClient` gains a valid `pingInterval` and
  `reconnectStrategy`, so each row trips only its own precondition. New rows: `pingInterval`
  missing, `0`, not finite, `> socketTimeout / 2`; strategy `undefined`, `false`, a function that
  returns `false`, a function that returns an `Error`; valid values pass. Each precondition's
  rows match a message fragment unique to that precondition, so a deleted check cannot hide
  behind another check's message.
- `tests/containers/support/redis.ts`'s `connect()` gains `pingInterval` (at most half of its
  `socketTimeout: 10_000`) and a reconnect strategy, or it fails D4.
- `README.md`: D5 (lines 89 and 97, and any other copy).

### 4.4 New tier in `ts/packages/paigasus-console-core`

- `package.json` devDependency `testcontainers: "catalog:"` (the catalog pins `^12.1.0`).
- `vitest.containers.config.ts`, copied from `@paigasus/discovery`'s, with the same
  `resolve.alias` for `server-only` as the package's `vitest.config.ts` (that alias is what makes
  the stub apply, not the setup file).
- `vitest.config.ts` excludes `tests/containers/**`.
- `tsconfig.json` `include` gains `vitest.containers.config.ts`. Without it, `ts:lint` fails,
  because `ts/eslint.config.js` uses `projectService: true` with no default project.
- `moon.yml` gains `test-e2e`: `script` with `set -euo pipefail` and
  `pnpm exec vitest run --config vitest.containers.config.ts`, `deps: ['contracts:generate']`,
  `options.cache: false`, and inputs that cover this package's `src/**/*`, `tests/**/*`, both
  vitest configs, `tsconfig.json`, `package.json`, `/ts/pnpm-lock.yaml`, and
  `@paigasus/discovery`'s `src/**/*` and `package.json`. It does NOT list `testing/**/*`.
- **No skip hatch.** Like `@paigasus/auth`'s tier, the task fails when Docker is unreachable. The
  package README says that a `console-core` source edit now needs Docker for its `test-e2e` task.
- `tests/containers/descriptor-cache-idle.test.ts` (§ 5).

### 4.5 CI registry fallout

`:test-e2e` is already in `ci.yml`'s `T=(…)` array. In `ci/affected-graph/run.sh`:

- The comment at `:89-99` counts projects that declare `test-e2e` by hand ("FIVE"). It becomes six.
- From the inputs above, these strict-equality cases are expected to gain
  `paigasus-console-core-ts:test-e2e`: `discovery->discovery-tasks` (`:509`),
  `discovery-adapters->discovery-tasks` (`:527`), `console-core->consumers` (`:673`),
  `console-core-prn-tenancy->consumers` (`:675`). The plan must **measure** each case with
  `moon query tasks --affected` and re-baseline only what the measurement shows, with a comment
  that names SMA-648. It must not type the sets by hand.

### 4.6 Documentation

- `CLAUDE.md` gains one Gotchas entry, in the D5 wording.
- The memory note `node-redis-command-vs-socket-timeout` and its `MEMORY.md` index line are
  corrected (outside the repo).

## 5. Tests

### 5.1 Container tier: `tests/containers/descriptor-cache-idle.test.ts`

One `redis:8-alpine` container per file. `PAIGASUS_SESSION_REDIS_TIMEOUT_MS: 500`, so
`socketTimeout` is 1000 ms and `pingInterval` is 500 ms. Each test has its own log sink, so a late
microtask from the previous test's `destroy()` cannot write into the next test's lines. Each test
waits, with a bounded poll (at most 5 s), until a first `set` succeeds before it starts to idle.
`afterEach` calls `resetDiscoveryForTest()` (safe after D10).

| # | Test | On `main` |
|---|------|-----------|
| T1 | **Idle past the timeout, then read.** `set` one descriptor record, idle 3000 ms (three times `socketTimeout`), then `get`. The `get` returns the record, and the sink has no `discovery.redis_connection_lost`. | Fails: `The client is closed`. |
| T2 | **Idle twice.** As T1, with two 3000 ms gaps and a read after each. | Fails. It catches a fix that survives only the first gap. |
| T3 | **A real hang recovers.** From a second admin client, send `CLIENT PAUSE 3000 ALL` (more than `pingInterval + 2 × socketTimeout` = 2500 ms). The cache's next PING is in flight with no reply, and nothing else writes, so the idle timer fires. After the pause ends, poll `get` every 100 ms for at most 5 s: it must succeed. The sink holds `discovery.redis_connection_lost` with `reason: "socket_timeout"` **exactly once**. | Fails: the client stays closed. |
| T4 | **A server-side close recovers.** From the admin client, `CLIENT KILL` the cache's connection (found with `CLIENT LIST`, by the client name or the non-admin id). Poll `get` as in T3: it must succeed. The sink holds exactly one `lost` line with `reason: "socket_closed"`. | On `main` the client reconnects (the default strategy accepts this cause), so the `get` passes, but there is no `lost` line. It fails on the log check. |
| T5 | **No DSN in any line.** The URL carries a password (`redis://:<secret>@host:port`, set with `--requirepass` on the container). Force one loss as in T4. Assert that at least one `lost` line exists, then that no line holds the secret. | A guard, exempt from the red-first rule: on `main` it fails only because no line exists. |

T1–T4 are written first and seen to fail on unmodified code (after D10) before the fix. The plan
records the failure output.

### 5.2 Default tier (no Docker)

- `descriptorCacheReconnectStrategy`: returns a finite number in `[0, 2199]` for retries 0..20.
- `watchConnectionLoss` against a fake `EventEmitter` with settable `isOpen`/`isReady`, five cases:
  an `error` while ready (an error reply) gives no line; an `error` after destroy (`isOpen`
  false) gives no line; a socket loss (`isOpen` true, `isReady` false) gives one line; further
  handshake errors in the same loop give no line; `ready` followed by a second loss gives one
  more line. Plus the three `reason` values from the three error classes.
- Client options pin: read
  `globalThis[Symbol.for('paigasus.console-core.discovery-state.v1')].redisClient.options` after
  `descriptorCacheFor` with a Redis config and an unreachable URL. Assert
  `pingInterval <= socketTimeout / 2` and `socket.reconnectStrategy === descriptorCacheReconnectStrategy`.
- `resetDiscoveryForTest` on a client that is already closed does not throw (D10).
- The D4 rows in `@paigasus/discovery` (§ 4.3).

## 6. Risks accepted

- In `next dev`, a compile can block the event loop for more than `timeoutMs`. The idle timer can
  then fire, the client reconnects at once, and one `lost` line appears. That is dev noise, and
  it recovers without help.

## 7. Out of scope

- The per-operation deadline and `@paigasus/auth`'s in-flight bound (§ 3, rejected list).
- Confirming why the gateway-console e2e tier stayed green. T1–T4 are the direct control now.

## 8. Changelog (revision 2)

Folded in from the challenge: D8 real-loss guard and its helper; the D5 wording (reads and writes
reset the timer); D10 seam fix before the red run; 500 ms timings and a bounded warm-up poll; D4
strategy probe and the `/ 2` margin; `tsconfig.json`, the "FIVE" comment, the named cases, and the
no-skip policy; T4 `socket_closed` and T5 with a forced log line; per-precondition message
fragments; the corrected "reconnect only" timing; D11; the `server-only` alias reason; the named
options-pin method; the README and `MEMORY.md` lines; the maintenance-error note; the `+ping`
requirement.

Rejected from the challenge: a per-operation deadline (out of scope, follow-up proposed); a
`pingInterval` floor (conflicts with D4 (a), documented instead); a Docker-free RESP stub (Sven
chose the container tier).
