# SMA-648 — the descriptor cache's Redis client must survive an idle socket

- Linear: SMA-648 (related: SMA-641, which found it)
- Date: 2026-09-18
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
After 2 s with no data on the socket, the client closes and does not open again. Every later read
throws `The client is closed`, discovery reports `cache-unavailable`, and both consoles disable
their navigation. The issue has the measured transcript.

## 2. Root cause (read from `@redis/client` 6.2.1, the installed version)

Two facts together cause the failure. Each one alone is harmless.

1. **`socketTimeout` is an idle timer.** `dist/lib/client/socket.js:293-300` calls Node's
   `socket.setTimeout(socketTimeout)` and destroys the socket with a `SocketTimeoutError` when it
   fires. Node's timer fires after N ms with **no socket activity**. It fires on a quiet, healthy
   connection. It also fires on a command that is in flight with no reply, which is why the repo
   wants it.
2. **The default reconnect strategy refuses a socket timeout.** `socket.js:402-406`,
   `defaultReconnectStrategy`: `if (cause instanceof SocketTimeoutError) return false;`. `false`
   means "do not reconnect, close the client" (`socket.d.ts:18-19`).

So the first idle gap longer than `socketTimeout` closes the client for the life of the process.

Two repo documents state fact 1 wrongly, and that error is how the bug shipped:

- `ts/packages/paigasus-discovery/src/adapters/redis-cache.ts:63-65` calls `socketTimeout` "the
  end-to-end deadline … node-redis closes the socket if no reply arrives within that many
  milliseconds". That is true for an in-flight command, but the doc does not say that the same
  timer also fires on an idle socket.
- The agent memory note `node-redis-command-vs-socket-timeout` makes the same claim.

`@paigasus/auth`'s session store (`src/adapters/redis-store.ts:239-250`) sets no `socketTimeout`, so
this bug does not affect it. Its own missing in-flight bound is a known, separate gap and is **out
of scope** here.

## 3. Decisions

| # | Decision | Reason |
|---|----------|--------|
| D1 | Keep `socketTimeout: timeoutMs * 2`. | It is the only bound on a command that is in flight with no reply. `createRedisDescriptorCache` asserts it, and that precondition stays. The issue's option 2 (remove it) is rejected. |
| D2 | Add `pingInterval: timeoutMs` to the client options. | node-redis sends `PING` every `pingInterval` ms while the socket is ready (`index.js:691-703`). A reply is socket activity, so a healthy idle socket never reaches the idle timer. `timeoutMs` is half of `socketTimeout`, which leaves one full `timeoutMs` for the PING round trip. |
| D3 | Add a `reconnectStrategy` that reconnects for **every** cause, `SocketTimeoutError` included, with a bounded delay and no retry cap. | A real hang (Redis accepts, never replies) still fires the idle timer, even with D2. Without D3 the client then closes forever, the same failure by a different path. The delay is `Math.min(2 ** retries * 50, 2000)` ms plus 0–199 ms of jitter, the same curve as node-redis's default. It never returns `false` or an `Error`, for the reason `@paigasus/auth`'s `reconnectStrategy` records (`redis-store.ts:53-62`): one client, no recreate path, so a stop is permanent. |
| D4 | `createRedisDescriptorCache` asserts a **fifth** precondition: `pingInterval` is a positive, finite number that is **smaller than** `socketTimeout`. | The helper already asserts the four options whose absence is silent until production. This bug is the same class. The precondition stops the next consumer from repeating it. It does not assert D3: a function's behaviour cannot be checked by reading options. The container test in § 5 proves D3. |
| D5 | Correct the `socketTimeout` doc comment in `redis-cache.ts` and its thrown message to say it is an **idle** timer that also bounds an in-flight command, and name `pingInterval` as what keeps an idle socket alive. | The wrong doc is the root of the shipped bug (§ 2). |
| D6 | Log a new app event, `discovery.redis_connection_lost`, with one field, `reason`. | Today `client.on('error', () => undefined)` hides the failure, so the only symptom was a degraded nav. |
| D7 | `reason` is a fixed value from `instanceof` checks: `socket_timeout` (`SocketTimeoutError`), `socket_closed` (`SocketClosedUnexpectedlyError`), or `other`. It never carries the error message or `constructor.name`. | node-redis error messages can embed the DSN. The error classes do not set `name` (`errors.js:22-27`), so `err.name` is always `"Error"`, and a production bundle can mangle `constructor.name`. The classes are exported from `redis` (it re-exports `@redis/client`, which re-exports `./lib/errors`). |
| D8 | Log D6 **once per loss**: only for an `error` that arrives while the client was ready. A client `ready` event arms it again. | node-redis emits `error` for each failed reconnect attempt too. One outage must give one line, not one line per retry. Errors before the first `ready` are connect failures, which `connectOnce` already logs as `discovery.redis_connect_failed`. |
| D9 | The regression test lives in a **new Docker-backed tier in `@paigasus/console-core`**, the same shape as `@paigasus/discovery` and `@paigasus/auth`: `testcontainers` + `redis:8-alpine`, a `vitest.containers.config.ts`, a `test-e2e` Moon task with `options.cache: false`, and `tests/containers/**` excluded from the default `vitest.config.ts`. | It drives the real call site (`descriptorCacheFor`), which is where the bad options lived. A test of a moved factory, or of a fake RESP server, would not. |

Rejected:

- **Reconnect only (issue option 3 alone).** The client closes after every idle gap and reopens
  some 50–250 ms later. With `disableOfflineQueue: true` a request in that window fails, so the
  nav still degrades now and then. D2 removes the close, and D3 covers only the real hang.
- **Ping only (issue option 1 alone).** One real hang, or one lost PING reply, still closes the
  client forever (§ 2, fact 2).
- **A `discovery.redis_reconnected` event.** Not requested. D8's re-arm on `ready` gives one
  `lost` line per outage, which is enough to see the failure.
- **Fixing `@paigasus/auth`'s missing in-flight bound.** A different defect in a different
  package (§ 2).

## 4. Changes

### 4.1 `ts/packages/paigasus-console-core/src/discovery.ts`

- `redisDescriptorCache` passes `pingInterval: timeoutMs` and
  `socket.reconnectStrategy: descriptorCacheReconnectStrategy` (D2, D3).
- `descriptorCacheReconnectStrategy(retries: number): number` is a named, exported function, so a
  unit test can pin its curve: it returns a number for all inputs, never above 2199, and never
  `false`.
- The `error` listener stays silent about the error object. It adds D6–D8: a `ready` listener
  sets a local "connected" flag. The `error` listener logs `discovery.redis_connection_lost` with
  `{ reason }` only when that flag is set, then clears it.
- The file-header comment and the `connectOnce` comment are updated to state the idle timer, the
  ping and the reconnect rule.

### 4.2 `ts/packages/paigasus-console-core/src/logger.ts`

- `AppEventName` gains `'discovery.redis_connection_lost'`.

### 4.3 `ts/packages/paigasus-discovery/src/adapters/redis-cache.ts`

- The fifth precondition (D4). The message names both options and says why.
- The doc comment and the `socketTimeout` message are corrected (D5).
- `tests/redis-cache-options.test.ts` gains rows: `pingInterval` missing, `0`, not finite, equal
  to `socketTimeout`, and greater than `socketTimeout` all throw; a valid value passes.
- `tests/containers/support/redis.ts`'s `connect()` gains a `pingInterval` below its
  `socketTimeout: 10_000`, or it now fails the new precondition.
- Any README text in that package that repeats the "end-to-end deadline" claim is corrected.

### 4.4 New tier in `ts/packages/paigasus-console-core`

- `package.json` devDependency `testcontainers: "catalog:"` (the catalog already pins `^12.1.0`).
- `vitest.containers.config.ts`, copied from `@paigasus/discovery`'s, with the package's own
  setup file so the `server-only` stub applies.
- `vitest.config.ts` excludes `tests/containers/**`.
- `moon.yml` gains `test-e2e`: `script` with `set -euo pipefail` and
  `pnpm exec vitest run --config vitest.containers.config.ts`, `deps: ['contracts:generate']`,
  `options.cache: false`, and inputs that cover this package's sources, tests, configs,
  `package.json`, `/ts/pnpm-lock.yaml`, and `@paigasus/discovery`'s `src/**/*` and `package.json`
  (the precondition this tier proves).
- `tests/containers/descriptor-cache-idle.test.ts` (§ 5).

### 4.5 CI registry fallout

`:test-e2e` is already in `ci.yml`'s `T=(…)` array, so no registry entry is new. But
`ci/affected-graph/run.sh` holds strict-equality task sets, and the new task can join any case
whose anchor file is in its inputs. The plan must **measure** which cases change with
`moon query tasks --affected` and re-baseline exactly those, with a comment that names SMA-648.
It must not type the sets by hand.

### 4.6 Documentation

- `CLAUDE.md` gains one Gotchas entry: in node-redis 6, `socketTimeout` is an idle timer, and the
  default reconnect strategy refuses a `SocketTimeoutError`, so the pair closes a client forever.
  Set `pingInterval` below it and a reconnect strategy that accepts the timeout.
- The agent memory note `node-redis-command-vs-socket-timeout` is corrected (outside the repo).

## 5. Tests

All in `tests/containers/descriptor-cache-idle.test.ts`, against one `redis:8-alpine` container,
with `PAIGASUS_SESSION_REDIS_TIMEOUT_MS: 200` (so `socketTimeout` is 400 ms and `pingInterval` is
200 ms). Each test calls `resetDiscoveryForTest()` in `afterEach`, which destroys the client and
clears the ping timer.

| # | Test | Must fail on `main` |
|---|------|---------------------|
| T1 | **Idle past the timeout, then read.** Write one descriptor record with `set`, wait 1200 ms (three times `socketTimeout`), then `get`. The `get` returns the record, and the log has no `discovery.redis_connection_lost`. | Yes: on `main` the `get` throws `The client is closed`. |
| T2 | **Idle twice.** The same, with two idle gaps and a read after each. | Yes. It catches a fix that survives only the first gap. |
| T3 | **A real hang recovers.** From a second admin client, send `CLIENT PAUSE 1000 ALL`. Redis then stops replying to every client, so the cache's next PING is in flight with no reply and the idle timer fires. After the pause ends, poll `get` every 100 ms for at most 5 s: it must succeed. The log must hold `discovery.redis_connection_lost` with `reason: "socket_timeout"` **exactly once**. | Yes: on `main` (and with D2 alone) the client stays closed. |
| T4 | **No DSN in any line.** The URL carries a password (`redis://:<secret>@host:port`). No logged line holds the secret. | Guards D7. |

Unit tests (default tier, no Docker):

- `descriptorCacheReconnectStrategy`: numbers only, `0 <= delay <= 2199`, for retries 0..20.
- The client-options pin (an existing test, or a new one next to `discovery-singleton.test.ts`)
  asserts `pingInterval < socketTimeout` and that `reconnectStrategy` is the exported function.

Each new container test is written first and seen to fail on unmodified code before the fix
(TDD). The plan records the failure output.

## 6. Out of scope

- `@paigasus/auth`'s missing in-flight bound (§ 2).
- Confirming the issue's inference about why the gateway-console e2e tier stayed green. T1–T3 are
  the direct control now, so the inference no longer carries weight.
