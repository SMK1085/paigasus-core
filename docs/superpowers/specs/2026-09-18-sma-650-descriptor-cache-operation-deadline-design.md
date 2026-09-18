# SMA-650 — the descriptor cache needs a per-operation deadline

- Linear: SMA-650 (related: SMA-648, whose spec challenge found this; SMA-651, the same class of
  gap in `@paigasus/auth`'s session store)
- Date: 2026-09-18
- Revision: 1. Approved by Sven on 2026-09-18.
- Scope: `ts/packages/paigasus-console-core/src/discovery.ts` and its tests. No change to
  `@paigasus/discovery`, to any contract, or to any app.

## 1. Problem

`@paigasus/console-core`'s descriptor cache can hang a render without a bound.

After SMA-648 the node-redis client carries four options
(`ts/packages/paigasus-console-core/src/discovery.ts:191-200`): `commandOptions.timeout`,
`pingInterval`, `socket.socketTimeout` and `socket.reconnectStrategy`. None of them bounds a
command that node-redis has already written to a Redis that accepts commands and never replies,
while other traffic continues.

- `commandOptions.timeout` bounds a command only while it is QUEUED, before the write.
- `socket.socketTimeout` is Node's idle timer. Any read OR write resets it. Under steady traffic
  each new write moves the deadline, so the timer never fires.

Every `await deps.cache.*` in `@paigasus/discovery`'s `src/core/single-flight.ts` then hangs — the
`get` at `:113`, the `set` at `:184`, the `tryAcquireLock` calls at `:276` and `:331`, and the
`releaseLock` at `:221`. The render path hangs with them. Nothing times out, and nothing recovers.

The gap existed on `main` before SMA-648. SMA-648 kept it out of scope and raised this issue.

## 2. Measured facts (read from `@redis/client` 6.2.1, the installed version)

These five facts decide the design. Each was read from
`ts/node_modules/.pnpm/@redis+client@6.2.1/node_modules/@redis/client/dist/lib`.

1. **The idle timer reaches the reconnect path through the socket, not the client.**
   `client/socket.js:293-300` arms `socket.setTimeout(socketTimeout)` and, when it fires, calls
   `socket.destroy(new SocketTimeoutError(...))` on the underlying `net.Socket`. That error reaches
   `#onSocketError` (`client/socket.js:313-333`), which sets `isReady = false`, emits `error`, and
   then reconnects — but only when `wasReady` is true, `#isOpen` is true, and the reconnect
   strategy returns a number.
2. **`client.destroy()` is terminal.** `client/index.js:1511-1519` clears the ping timer, flushes
   the command queue with a `DisconnectsClientError`, and calls the socket's `destroy()`.
   `client/socket.js:378-383` sets `#isOpen = false`. Fact 1's reconnect branch requires `#isOpen`,
   so a destroyed client never reconnects. Destroying the client on a hang would recreate the
   permanently dead client that SMA-648 fixed.
3. **No public API triggers fact 1's path.** The client holds its socket in a private field. The
   only accessor, `_ejectSocket()` (`client/index.js:764-770`), is marked `@internal` and calls
   `removeAllListeners()` before returning, so destroying the ejected socket does NOT reach
   `#onSocketError`. It would leave the client with no socket and no reconnect — worse than fact 2.
4. **node-redis stops writing by itself during a hang.** `#setPingTimer`
   (`client/index.js:691-703`) re-arms only inside `.finally()`, after the PING settles. While a
   PING is in flight with no reply, node-redis writes nothing more. The existing container test
   T3 already depends on this.
5. **A rejecting cache operation is contract-legal.** `ts/packages/paigasus-discovery/src/ports/
   cache.ts:12-13`: "Every method may throw. The caller degrades rather than propagating — one
   Redis blip must not 500 every server component rendering navigation."

Fact 4 is the one the design turns on. Our own writes are the only thing defeating the existing
recovery. If the wrapper stops writing, the socket becomes genuinely idle, the idle timer fires,
and SMA-648's `descriptorCacheReconnectStrategy` repairs the connection with no new machinery.

## 3. Decisions

| # | Decision | Reason |
|---|----------|--------|
| D1 | The deadline lives in `@paigasus/console-core`, in the wrapper `afterConnect` already occupies. It does NOT go into `@paigasus/discovery`'s `createRedisDescriptorCache`. | The bound is a property of how this app composes its cache, not of the adapter. `createRedisDescriptorCache` asserts client OPTIONS; it does not wrap behaviour. A consumer may want a different bound. SMA-651 covers `@paigasus/auth`, which does not use this adapter at all, so a shared helper would buy nothing here. |
| D2 | On expiry the operation REJECTS. No new return value and no new degraded reason. | § 2 fact 5. `@paigasus/discovery` already handles it: a cold render degrades to `cache-unavailable`, a warm render logs and serves the stale record (§ 5). |
| D3 | The deadline is `3 × timeoutMs` (3000 ms at the default). | It must be strictly above `socketTimeout` (`2 × timeoutMs`), so node-redis's own recovery wins whenever it CAN fire and this deadline is a pure backstop for the case the idle timer cannot see. An equal value would make the two mechanisms race, and would make `discovery.redis_connection_lost` nondeterministic in the container tier. |
| D4 | On expiry the wrapper opens a CIRCUIT: later operations reject at once, without touching the socket. It does NOT call `client.destroy()`. | § 2 facts 2 and 3: destroying is terminal and there is no public way to trigger a reconnect. § 2 fact 4: stopping our writes is what lets the idle timer fire, so the circuit IS the repair mechanism, not only a cost saving. |
| D5 | The circuit closes on the client's next `ready` event, OR after a cooldown of `4 × timeoutMs`, whichever is first. | `ready` is the exact signal that the reconnect finished. The cooldown is the second exit, and it is required: if the expiry came from a Redis that is slow rather than wedged, its late replies keep the socket active, the idle timer never fires, and `ready` never comes again. Without the cooldown a slow Redis would disable the cache for the life of the process. The value exceeds `socketTimeout`, because when the circuit opens the last write may be as recent as the expiry itself, so the idle timer can need a further `socketTimeout` to fire. |
| D6 | The abandoned `inner` promise gets a `.catch(() => undefined)`. A late success is discarded and never cached. | node-redis rejects the outstanding command when the socket is torn down. Without the handler that is an unhandled rejection, which in Node can end the process. A late success cannot be used: the caller has already degraded and moved on. |
| D7 | One new `AppEventName`, `discovery.redis_operation_timeout`, logged ONCE when the circuit opens, with fields `{ operation, deadlineMs }`. Operations rejected while the circuit is already open log nothing. | It carries no error object and no DSN, the rule `logger.ts:7-9` states. Logging every rejection would flood: a lasting wedge costs at most one line per `4 × timeoutMs`. The shape mirrors `watchConnectionLoss`'s once-per-loss discipline. |
| D8 | `close()` is not wrapped. The memory cache is not wrapped. | `close()` is teardown, not a render-path operation. The memory cache cannot hang. |

### 3.1 The four timing rungs

With `T = PAIGASUS_SESSION_REDIS_TIMEOUT_MS` (default 1000 ms), each mechanism now has its own
rung, and no two are equal:

| Rung | Value | What it bounds |
|---|---|---|
| `commandOptions.timeout` | `T` | the queued phase only |
| `pingInterval` | `T` | keeps a healthy idle socket alive |
| `socket.socketTimeout` | `2T` | the idle timer; node-redis's own recovery |
| operation deadline (NEW) | `3T` | one cache operation, end to end |
| circuit cooldown (NEW) | `4T` | how long the wrapper stops writing after an expiry |

## 4. Design

### 4.1 The wrapper

A new function in `src/discovery.ts`, applied to the same five operations `afterConnect` wraps —
`get`, `set`, `delete`, `tryAcquireLock`, `releaseLock`. `close` passes through (D8).

The deadline covers the WHOLE operation body, the existing `await ready` included, so `3T` is the
total bound and not an addition to the connect wait. `connectOnce`'s promise already resolves
within `T` and never rejects, so this is a safety margin rather than a second bound.

Per operation:

1. If the circuit is open and the cooldown has not elapsed, reject at once. Do not call `inner`.
2. Otherwise close the circuit if it was open, then race the operation against a `3T` timer.
3. If the operation settles first, clear the timer and return or rethrow its result.
4. If the timer fires first, open the circuit, log `discovery.redis_operation_timeout` once, attach
   the `.catch` of D6 to the abandoned promise, and reject.

The timer MUST be cleared when the operation wins (step 3), in a `finally`. An uncleared timer per
operation is a leak under load. It MUST also be `unref()`d, so a pending deadline cannot hold the
process open — the same treatment `connectOnce` gives its own timer (`discovery.ts:155-159`).

### 4.2 Circuit state

The state is two values held in the wrapper's closure, so it is per cache instance and
`resetDiscoveryForTest` clears it by clearing the process cache:

- `openedAt: number | null` — `Date.now()` when the circuit opened, else `null`.
- a `ready` listener that sets `openedAt = null`.

A timestamp comparison replaces a second timer. The circuit is open when `openedAt !== null` and
`Date.now() - openedAt < 4 × timeoutMs`.

### 4.3 The ready signal

The wrapper needs the client's `ready` event but must stay testable without a real client, so it
takes a STRUCTURAL source, the shape `watchConnectionLoss` already uses (`discovery.ts:86-90`):

```ts
type ReadySource = { on(event: 'ready', listener: () => void): unknown };
```

A real `RedisClientType` satisfies it. A plain `EventEmitter` satisfies it in the default test tier.
The wrapper registers one extra listener on the client; node-redis's default maximum is ten, and
`watchConnectionLoss` uses two.

### 4.4 The error

An exported `DescriptorCacheTimeoutError`, so tests assert on a type rather than a message string.
The message names the operation and the deadline. It NEVER carries the DSN, the URL, or a
node-redis error. It extends `Error` and sets `name`, because a production bundle can mangle
`constructor.name` — the same reasoning SMA-648 recorded for `connectionLossReason`
(`discovery.ts:76-80`).

### 4.5 Where it is applied

`redisDescriptorCache` (`discovery.ts:190-206`) composes it:

```
createRedisDescriptorCache(client)  ->  afterConnect(..., connectOnce(...))  ->  withOperationDeadline(..., client, timeoutMs, log)
```

The deadline wraps OUTSIDE `afterConnect`, so it covers the `await ready` (§ 4.1).

## 5. What callers see

No caller changes. `@paigasus/discovery` already treats a rejection as a cache failure:

- **Cold render** (no cached record): `single-flight.ts` logs `discovery.cache_unavailable` with a
  `stage` field and returns `degraded(service, 'cache-unavailable')` — at `:254-263` for the read,
  `:331-336` for the lock, `:338-343` for the probe and store. The UI renders it as "not answering
  (discovery cache unavailable)" (`ts/packages/paigasus-discovery/src/disabled.tsx:67`).
- **Warm render** (a record exists): the rejection is logged and swallowed, and the stale record is
  served (`single-flight.ts:274-323`). The render does not degrade.
- **Lock release**: each of the five `releaseLock` call sites has its own `try`/`catch` that
  swallows, so a deadline at release time cannot mask the probe's real outcome.

## 6. Tests

### 6.1 Default tier (`tests/unit/`, no Docker)

Against a fake inner cache and `vi.useFakeTimers()`, which also controls `Date.now()`:

| # | Case |
|---|------|
| U1 | An operation that settles inside the deadline returns its value, and the inner cache is called once. |
| U2 | An operation that never settles rejects with `DescriptorCacheTimeoutError` at `3 × timeoutMs`. |
| U3 | After an expiry, the next operation rejects IMMEDIATELY and the inner cache is NOT called. |
| U4 | A `ready` event closes the circuit: the next operation reaches the inner cache. |
| U5 | With no `ready` event, the circuit closes after `4 × timeoutMs` and the next operation reaches the inner cache. |
| U6 | The circuit opens once per wedge: `discovery.redis_operation_timeout` is logged once, with `{ operation, deadlineMs }`, no matter how many operations are rejected while it is open. |
| U7 | A rejection from the inner cache propagates unchanged and does NOT open the circuit. Only a deadline does. |
| U8 | The abandoned promise rejecting later produces no unhandled rejection. |
| U9 | All five operations are wrapped, and `close()` is not. |

### 6.2 Docker tier (`tests/containers/descriptor-cache-idle.test.ts`)

One new case, the acceptance criterion:

**T6 — a hang under STEADY TRAFFIC is bounded.** Warm the cache, then `CLIENT PAUSE` the server
while a driver loop keeps issuing cache operations, so the socket keeps being written to and the
idle timer cannot fire. Assert that each operation settles within roughly the deadline rather than
hanging for the length of the pause, and that the cache serves a read again after the pause ends.

T6 must fail before the fix and pass after it (red first). The existing T1–T5 must stay green: the
deadline is above `socketTimeout`, so T3's idle-timer path is unchanged.

The `test-e2e` Moon task already exists with `cache: false` and no skip hatch, and its `inputs`
already cover `src/**/*` and `tests/**/*`, so no Moon or CI registration changes.

## 7. Non-goals

- `@paigasus/auth`'s session store. That is SMA-651, and § 2 fact 2 applies there too.
- Any change to `@paigasus/discovery`, to `DescriptorCache`, or to the degraded reasons.
- A configurable deadline. A fifth Redis timing knob is not justified until an operator asks.
- A retry. A deadline that retries would multiply the wedge cost, and `single-flight` already owns
  the retry policy.
- Bounding the total number of cache operations per render. A cold render touches the cache several
  times; § 8 records what that costs.

## 8. Risks and open items

1. **Per-render cost under a wedge.** A cold render touches the cache several times. If each paid
   the deadline it would cost about `4 × 3T`. The circuit makes only the FIRST operation pay it;
   the rest reject at once. The claim that `disableOfflineQueue: true` ALSO makes them fail fast is
   NOT relied on — it must be verified during implementation, and the design holds either way.
2. **A slow-but-alive Redis loses one operation per `4T`.** D5's cooldown bounds the damage: at
   worst one operation expires, the circuit opens for `4T`, and then traffic resumes. A Redis
   slower than `3T` on a normal reply would degrade the cache badly, but such a Redis already
   breaks the render budget.
3. **The circuit is per process, not per service.** One wedged operation stops cache traffic for
   every service for up to `4T`. That is intended: the socket is shared, so the wedge is shared.
4. **Fake timers and real sockets.** The unit tier uses fake timers and a fake inner cache only.
   Everything touching a real socket is proven in the Docker tier, where the timers are real.
5. **T6 depends on the driver loop really keeping the socket busy.** If the loop's own operations
   are rejected by the circuit, the socket goes idle and T6 would then measure T3's path instead.
   The test must assert the distinguishing signal — that the FIRST operation's rejection arrives at
   about the deadline rather than at `socketTimeout` — and not merely that something failed.
