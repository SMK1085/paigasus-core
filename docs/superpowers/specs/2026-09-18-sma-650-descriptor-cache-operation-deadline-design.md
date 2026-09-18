# SMA-650 — the descriptor cache needs a per-operation deadline

- Linear: SMA-650 (related: SMA-648, whose spec challenge found this; SMA-651, the same class of
  gap in `@paigasus/auth`'s session store)
- Date: 2026-09-18
- Revision: 2, after the adversarial challenge (changelog in § 11). Approved by Sven on 2026-09-18.
- Scope: `ts/packages/paigasus-console-core` — `src/discovery.ts`, `src/logger.ts` and tests. Four
  documents carry a stale forward reference and are corrected (§ 9). No BEHAVIOURAL change to
  `@paigasus/discovery`, to `DescriptorCache`, or to any app.

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
`get` at `:113`, the `set` at `:184`, the `tryAcquireLock` calls at `:276` and `:332`, and the
`releaseLock` calls at `:222`, `:243`, `:292` and `:315`. The render path hangs with them.

The gap existed on `main` before SMA-648. SMA-648 kept it out of scope and raised this issue.

**This issue does not make a render bounded on its own.** `@paigasus/auth`'s session store creates
its OWN client (`ts/packages/paigasus-auth/src/adapters/redis-store.ts:239-250`) with no
`socketTimeout` and no `pingInterval`, and `requireSession` runs before any discovery call on a
console page. Against the same wedged Redis the render still hangs in the session store. That is
SMA-651. This issue bounds the descriptor cache's contribution, and nothing more. Any acceptance
criterion phrased as "a render is bounded" belongs to SMA-651, not here.

## 2. Measured facts (read from `@redis/client` 6.2.1, the installed version)

These seven facts decide the design. Each was read from
`ts/node_modules/.pnpm/@redis+client@6.2.1/node_modules/@redis/client/dist/lib`.

1. **The idle timer reaches the reconnect path through the socket, not the client.**
   `client/socket.js:293-301` arms `socket.setTimeout(socketTimeout)` and, when it fires, calls
   `socket.destroy(new SocketTimeoutError(...))` on the underlying `net.Socket`. That error reaches
   `#onSocketError` (`client/socket.js:313-336`), which sets `isReady = false`, emits `error`, and
   then reconnects — but only when `wasReady` is true, `#isOpen` is true, and the reconnect
   strategy returns a number (the guard at `client/socket.js:330`).
2. **`client.destroy()` is terminal.** `client/index.js:1511-1519` clears the ping timer, flushes
   the command queue with a `DisconnectsClientError`, and calls the socket's `destroy()`.
   `client/socket.js:378-384` sets `#isOpen = false`. Fact 1's reconnect branch requires `#isOpen`,
   so a destroyed client never reconnects.
3. **No public API triggers fact 1's path.** The client holds its socket in a private field. The
   only accessor, `_ejectSocket()` (`client/index.js:764-770`), is marked `@internal` and calls
   `removeAllListeners()` at `:768` before returning, so destroying the ejected socket does NOT
   reach `#onSocketError`. It would leave the client with no socket and no reconnect.
4. **node-redis writes exactly ONE more PING after a wedge begins, and then stops.**
   `#setPingTimer` (`client/index.js:691-703`) re-arms only inside `.finally()` at `:701`, after
   the PING settles. So the PING already scheduled when the wedge starts is still written — up to
   `pingInterval` later — and it resets the idle timer. Because it never settles, no further PING
   is written. **This fact sets the floor for every timing decision below** (§ 3.1). The existing
   container test encodes the same floor
   (`tests/containers/descriptor-cache-idle.test.ts:31-32`).
5. **A rejecting cache operation is contract-legal.** `ts/packages/paigasus-discovery/src/ports/
   cache.ts:12-13`: "Every method may throw. The caller degrades rather than propagating — one
   Redis blip must not 500 every server component rendering navigation."
6. **A command sent while the client is not ready fails immediately.** `client/index.js:1125-1132`:
   `sendCommand` rejects with `ClientClosedError` when the socket is not open, and with
   `ClientOfflineError` when it is open but not ready AND `disableOfflineQueue` is set. There is no
   third branch and no queueing. The client sets `disableOfflineQueue: true` (`discovery.ts:193`).
7. **A socket teardown rejects the in-flight commands.** `client/index.js:644-649`: on a socket
   error, with `disableOfflineQueue` set, the client takes the `flushAll(err)` branch. So an
   abandoned command does not hang forever — it rejects when the socket dies.

Fact 4 is the one the timing turns on. Facts 2 and 3 rule out destroying the client. Fact 6 is what
makes the recovery cheap.

## 3. Decisions

| # | Decision | Reason |
|---|----------|--------|
| D1 | The deadline lives in `@paigasus/console-core`, in the wrapper `afterConnect` already occupies. It does NOT go into `@paigasus/discovery`'s `createRedisDescriptorCache`. | The bound is a property of how this app composes its cache, not of the adapter. `createRedisDescriptorCache` asserts client OPTIONS and never wraps behaviour (`redis-cache.ts:94-134`). SMA-651 covers `@paigasus/auth`, which uses a different adapter and a different client, so a shared helper would buy nothing. |
| D2 | On expiry the operation REJECTS. No new return value and no new degraded reason. | § 2 fact 5. `@paigasus/discovery` already handles a rejection (§ 5). |
| D3 | The deadline is `4 × timeoutMs` (4000 ms at the default). | It must be strictly above the idle timer's WORST CASE, which § 3.1 derives as `pingInterval + socketTimeout = 3T`, not `socketTimeout = 2T`. At `3T` the two mechanisms would tie, which is the nondeterminism this decision exists to prevent. |
| D4 | On expiry the wrapper opens a CIRCUIT: later operations reject at once, without touching the socket. It does NOT call `client.destroy()`. | § 2 facts 2 and 3: destroying is terminal and there is no public way to trigger a reconnect. § 2 fact 4: stopping our writes is what lets the idle timer fire, so the circuit IS the repair mechanism, not only a cost saving. § 6 records the rebuild alternative and why it loses. |
| D5 | The circuit closes on the client's next `ready` event, OR after a cooldown of `4 × timeoutMs` measured from the moment it opened, whichever is first. | `ready` is the exact signal that the reconnect finished. The cooldown is the required second exit: if the expiry came from a Redis that is slow rather than wedged, its late replies keep the socket active, the idle timer never fires, and `ready` never comes again — without the cooldown a slow Redis would disable the cache for the life of the process. § 3.1 derives the value. |
| D6 | An operation that expires while the circuit is ALREADY open does NOT re-stamp `openedAt` and does NOT log. Step 4 of § 4.1 is guarded by `if (openedAt === null)`. | Several operations are in flight at once (§ 3.2), so without the guard one wedge would log one line per concurrent operation and the cooldown would slide forward with every late expiry, never expiring under load. |
| D7 | The abandoned `inner` promise gets a `.catch(() => undefined)`. A late success is discarded and never cached. | § 2 fact 7: node-redis rejects it when the socket dies. Without the handler that is an unhandled rejection, which in Node can end the process. A late success cannot be used: the caller has already degraded and moved on. |
| D8 | One new `AppEventName`, `discovery.redis_operation_timeout`, logged ONCE when the circuit opens, with fields `{ operation, deadlineMs }`. | It carries no error object and no DSN, the rule `logger.ts:7-9` states. With D6, a lasting wedge costs at most one line per `4 × timeoutMs`. `AppEventName` is a closed union (`src/logger.ts:14-21`), so `src/logger.ts` must change — it is in scope. |
| D9 | ONE exported error class, `DescriptorCacheTimeoutError`, with a readonly `phase: 'deadline' \| 'circuit-open'` field. | Both cases mean "this cache is not answering", so callers need not tell them apart — but tests must. One class with a discriminating field gives tests an exact assertion and keeps one export. It also distinguishes both from node-redis's own `ClientOfflineError`, which is what a post-cooldown operation gets (§ 2 fact 6). |
| D10 | `close()` is not wrapped. The memory cache is not wrapped. | The Redis adapter's `close()` is `return Promise.resolve()` and never touches the socket (`redis-cache.ts:186-190`), so it cannot hang. The memory cache cannot hang. |

### 3.1 The timing rungs

With `T = PAIGASUS_SESSION_REDIS_TIMEOUT_MS` (default 1000 ms):

| Rung | Value | What it bounds |
|---|---|---|
| `commandOptions.timeout` | `T` | the queued phase only |
| `pingInterval` | `T` | keeps a healthy idle socket alive |
| `socket.socketTimeout` | `2T` | the idle timer; node-redis's own recovery |
| operation deadline (NEW) | `4T` | one cache operation, end to end |
| circuit cooldown (NEW) | `4T` | how long the wrapper stops writing after an expiry |

**Why the deadline is `4T` and not `3T`.** Walk the idle case with `T = 1000`. A command is written
at `t = 0` and gets no reply. By § 2 fact 4 the PING already scheduled is still written, at up to
`t = T`. That write resets the idle timer, which then fires at `t = T + 2T = 3T`. No further PING
follows, so `3T` is the worst case. A deadline of `3T` would tie with it. `4T` is strictly above it,
so in the idle case node-redis's own recovery always wins and this deadline never fires — which is
what keeps T1–T5 and `discovery.redis_connection_lost` deterministic.

**Why the cooldown is `4T` and is NOT re-derived above the deadline.** The cooldown is measured
from the moment the circuit OPENS, not from the start of the wedge, so it does not compose with the
deadline. When the circuit opens there is no unanswered PING left to write (§ 2 fact 4: the one
trailing PING went out long before), so the last write is the last operation issued, at about the
moment the circuit opens. The idle timer therefore fires about `2T` later, and the first reconnect
attempt starts at once (`socket.js:330-333`). `4T` leaves roughly `2T` for the reconnect.

**If the cooldown expires before `ready`, nothing bad happens.** By § 2 fact 6 the next operation
gets an immediate `ClientOfflineError` from node-redis, at no cost, and the circuit re-opens only
if an operation actually reaches the socket and expires. The cooldown is therefore a floor on how
long we stay silent, not a promise that recovery has finished.

### 3.2 Concurrency

Operations are concurrent, not sequential. `createDiscovery` memoizes per `(service, token)`
(`ts/packages/paigasus-discovery/src/server.ts:125-130`), so every configured service resolves in
parallel within one render, and a `waitUntil` revalidation (`single-flight.ts:303-307`) adds more
outside it. When a wedge starts, every in-flight operation carries its own deadline and they all
expire at about the same time. D6 is what makes that one circuit opening and one log line.

`openedAt` is a plain field mutated from the event loop's single thread, so simultaneous expiries
cannot interleave mid-update. D6's guard is a read and a write in the same synchronous step.

## 4. Design

### 4.1 The wrapper

A new function in `src/discovery.ts`, applied to the five operations `afterConnect` wraps — `get`,
`set`, `delete`, `tryAcquireLock`, `releaseLock`. `close` passes through (D10).

The deadline covers the WHOLE operation body, the existing `await ready` included, so `4T` is the
total bound and not an addition to the connect wait.

Per operation:

1. If the circuit is open and the cooldown has not elapsed, reject at once with `phase:
   'circuit-open'`. Do not call `inner`.
2. Otherwise clear the circuit if it was open, then race the operation against a `4T` timer.
3. If the operation settles first, clear the timer and return or rethrow its result unchanged.
4. If the timer fires first, then — only `if (openedAt === null)` (D6) — stamp `openedAt` and log
   `discovery.redis_operation_timeout` once. In every case attach D7's `.catch` to the abandoned
   promise and reject with `phase: 'deadline'`.

The timer MUST be cleared when the operation wins (step 3), in a `finally`. An uncleared timer per
operation is a leak under load, and worse, a stale timer would open the circuit long after an
operation that already succeeded or already failed. It MUST also be `unref()`d, so a pending
deadline cannot hold the process open — the treatment `connectOnce` already gives its own timer
(`discovery.ts:155-159`).

A rejection from `inner` propagates unchanged and never opens the circuit. Only a deadline does.

### 4.2 Circuit state

Two values in the wrapper's closure, so the state is per cache instance and
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

A real `RedisClientType` satisfies it; a plain `EventEmitter` satisfies it in the default test tier.

**The wrapper must listen for `ready` ONLY, never for `error`.**
`tests/unit/discovery-redis-client.test.ts:70-72` pins `client.listenerCount('error') === 1`, and
Node's listener limit is per event name. An `error` listener here would red an existing test and
would duplicate `watchConnectionLoss`'s job.

### 4.4 The error

`DescriptorCacheTimeoutError extends Error`, exported from `src/discovery.ts`, with a readonly
`phase: 'deadline' | 'circuit-open'` (D9). It sets `name` explicitly, because a production bundle
can mangle `constructor.name` — the reasoning SMA-648 recorded for `connectionLossReason`
(`discovery.ts:76-80`). The message names the operation and the deadline. It NEVER carries the
DSN, the URL, or a node-redis error.

It is NOT added to `src/index.ts`. That file re-exports only `createAppDiscovery`,
`descriptorCacheFor` and `resetDiscoveryForTest` (`:21`), the package's `exports` map exposes only
`.` and `./testing`, and the tests import from `../../src/discovery` directly.

### 4.5 Where it is applied

`redisDescriptorCache` (`discovery.ts:190-206`) composes it:

```
createRedisDescriptorCache(client)  ->  afterConnect(..., connectOnce(...))  ->  withOperationDeadline(..., client, timeoutMs, log)
```

The deadline wraps OUTSIDE `afterConnect`, so it covers the `await ready` (§ 4.1).

## 5. What callers see

No caller changes, but the effect is NOT "stale records keep being served". It depends on WHICH
call rejects, and the first one usually does.

- **The first `get` rejects** — the common case, because `resolveService`'s first cache call is the
  `get` at `single-flight.ts:113`, reached from `:254`. The catch at `:255-262` logs
  `discovery.cache_unavailable` with `stage: 'read'` and returns
  `degraded(service, 'cache-unavailable', null)`. The stale record is never read, so it cannot be
  served. **So while the circuit is open, every service on every render shows "not answering
  (discovery cache unavailable)" (`ts/packages/paigasus-discovery/src/disabled.tsx:67`), including
  services whose records are fresh in Redis.** § 8 risk 3 states the blast radius.
- **A later call rejects** — reachable only when the first `get` SUCCEEDED. Then the stale path at
  `:274-323` logs and swallows, and the stale record is served without degrading.
- **Lock release** — each of the four `releaseLock` call sites (`:222`, `:243`, `:292`, `:315`) has
  its own `try`/`catch` that swallows, so a deadline at release time cannot mask the probe's real
  outcome.
- **Three call sites swallow silently**, with no log at all: `:284`, `:352` and `:358` use
  `.catch(() => null)`.

Note that one `DescriptorCache.get` is not always one Redis command: the adapter issues a second
`client.eval(DELETE_IF_UNCHANGED, ...)` when a record fails to parse
(`ts/packages/paigasus-discovery/src/adapters/redis-cache.ts:142-157`). One deadline covers the
whole method, which is correct and is what "end to end" in § 3.1 means.

## 6. Alternatives considered and rejected

- **Destroy the client and rebuild it.** `client.destroy()` is terminal (§ 2 fact 2), but that is
  only fatal because there is no recreate path — and the machinery exists: `state()` holds the
  client and the cache (`discovery.ts:57-65`, `:203`, `:225`), and `resetDiscoveryForTest`
  (`:240-245`) already performs the guarded teardown. Rejected because it puts a fresh
  `connectOnce` race on the render path, re-runs the six `createRedisDescriptorCache`
  preconditions, drops in-flight commands belonging to other renders, and replaces SMA-648's
  reconnect machinery with a second, parallel one. The circuit reuses what already works.
- **A half-open circuit** — after the cooldown, let exactly one probe through and keep blocking the
  rest until it settles. Rejected: D5's "next operation" already IS the probe, and by § 2 fact 6 a
  post-cooldown operation against a still-broken client costs nothing. The flag and the concurrent
  test it needs buy no measured benefit. This is the standard shape, so § 8 risk 2 records what
  rejecting it costs.
- **A configurable deadline.** Rejected: a fifth Redis timing knob is not justified until an
  operator asks for one.
- **A retry inside the wrapper.** Rejected: it would multiply the wedge cost, and `single-flight`
  already owns the retry policy.

## 7. Non-goals

- `@paigasus/auth`'s session store. That is SMA-651, and § 1 states why this issue alone cannot
  bound a render.
- Any behavioural change to `@paigasus/discovery`, to `DescriptorCache`, or to the degraded reasons.
- A Redis PROXY between the app and Redis. node-redis matches replies positionally against its
  waiting queue; a real Redis always replies in order, so an abandoned command is safe, but a proxy
  that dropped one reply and delivered later ones would desynchronise the stream. No such proxy is
  deployed, and this design does not defend against one.

## 8. Risks

1. **Per-render cost under a wedge.** For ONE service the chain is sequential — `get`,
   `tryAcquireLock`, `get`, `set`, `releaseLock` — so only the first operation pays the deadline
   and the rest reject at once. Across a render's CONCURRENT services (§ 3.2) every already-issued
   operation pays its own deadline in parallel, so the render's wall-clock cost is about one
   deadline, not one per service. After the cooldown, operations cost nothing (§ 2 fact 6).
2. **A transient stall costs a fixed penalty after Redis has recovered.** If the wedge ends between
   `4T` and `6T` — a BGSAVE fork pause, an AOF rewrite, a slow Lua script — the abandoned commands
   get their replies, the socket stays active, the idle timer never fires and `ready` never comes,
   so the circuit stays open for the FULL `4T` cooldown after Redis is healthy again. With § 5's
   blast radius that is four seconds of degraded navigation for a stall that already ended. This is
   the price of rejecting the half-open alternative (§ 6).
3. **The circuit is per process and per client, not per service.** One wedged operation stops cache
   traffic for every service for up to `4T`, and by § 5 that degrades every service's navigation,
   not only the one that expired. That is intended — the socket is shared, so the wedge is shared —
   but it is a bigger user-visible effect than "the cache is cold".
4. **Fake timers and real sockets.** The unit tier uses fake timers and a fake inner cache only.
   Everything touching a real socket is proven in the Docker tier, where the timers are real.

## 9. Documents to correct

Four in-tree sites carry a forward reference saying this gap is still open, and SMA-648's own spec
records that a wrong document "is how the bug shipped"
(`docs/superpowers/specs/2026-09-18-sma-648-descriptor-cache-redis-idle-design.md:41`):

- `ts/packages/paigasus-console-core/src/discovery.ts:24`
- `ts/packages/paigasus-discovery/src/adapters/redis-cache.ts:87`
- `ts/packages/paigasus-discovery/README.md:104`
- `CLAUDE.md:1059`

The agent memory note `node-redis-command-vs-socket-timeout`, whose `MEMORY.md` index line says
"gaps SMA-650/651", is updated in the same pass. The two `@paigasus/discovery` edits are COMMENT
and README text only — no behavioural change, which is what the Scope line means.

## 10. Tests

### 10.1 Default tier (`tests/unit/`, no Docker)

Against a fake inner cache and a fake `ReadySource`. The test file calls
`vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout', 'Date'] })` EXPLICITLY rather than
relying on vitest's default `toFake` set, because this repo has already been bitten by a vitest
default changing under it (CLAUDE.md, SMA-502).

| # | Case |
|---|------|
| U1 | An operation that settles inside the deadline returns its value, and the inner cache is called once. |
| U2 | An operation that never settles rejects at `4 × timeoutMs` with `DescriptorCacheTimeoutError` and `phase: 'deadline'`. |
| U3 | After an expiry, the next operation rejects IMMEDIATELY with `phase: 'circuit-open'`, and the inner cache is NOT called. |
| U4 | A `ready` event closes the circuit: the next operation reaches the inner cache. |
| U5 | With no `ready` event, the circuit closes after `4 × timeoutMs` and the next operation reaches the inner cache. |
| U6 | Operations rejected while the circuit is open log nothing: one wedge, one line. |
| U7 | THREE CONCURRENT operations expire together (D6): exactly one log line, and `openedAt` is stamped once — proven by the circuit closing `4T` after the FIRST expiry, not after the last. |
| U8 | A rejection from the inner cache propagates unchanged and does NOT open the circuit — and the timer was cleared: advancing `4 × timeoutMs` after the rejection opens no circuit and logs nothing. |
| U9 | The abandoned promise rejecting later produces no unhandled rejection. |
| U10 | All five operations are wrapped, and `close()` passes through unwrapped. |
| U11 | The wrapper registers a `ready` listener and NO `error` listener (§ 4.3). |

### 10.2 Docker tier (`tests/containers/descriptor-cache-idle.test.ts`)

One new case, the acceptance criterion.

**T6 — a hang under STEADY TRAFFIC is bounded.** Warm the cache, then `CLIENT PAUSE` the server
while a driver keeps the socket busy. The driver is **fire-and-forget, not awaited**, and issues an
operation every interval **strictly below `socketTimeout`**, keeping several operations in flight.
That is what stops the idle timer firing and makes the case distinct from T3.

The assertion is on the TYPE, not the clock: the first rejection must be a
`DescriptorCacheTimeoutError` with `phase: 'deadline'`. A node-redis teardown rejection can never
be that type, so this discriminates the new path from the existing idle-timer path exactly, with no
wall-clock window to go flaky on a loaded CI runner. The test then asserts the cache serves a read
again after the pause ends.

Every driver promise gets its own `.catch`. `afterEach` calls `resetDiscoveryForTest()`
(`descriptor-cache-idle.test.ts:55-57`), which destroys the client (`discovery.ts:242`) and
flushes the queue (§ 2 fact 7), rejecting every still-pending wrapper promise — an unhandled
rejection there fails the whole file.

**Red first.** T6 must fail before the fix. Without the wrapper there is no
`DescriptorCacheTimeoutError` at all, so the type assertion cannot pass by accident. Note that a
naive awaited driver loop WOULD pass before the fix: awaiting stops the traffic, the idle timer
fires, and node-redis rejects the operation at about `2T`–`3T` anyway. That is precisely why the
driver must not await.

**T1–T5 stay green because none of them arms a deadline**, not because of the deadline's value.
T3's pause happens with NO cache operation in flight (`descriptor-cache-idle.test.ts:143-146`:
`clientPause`, then `sleep`, then `get`), so no deadline is ever running during the pause.

### 10.3 Registration

`paigasus-console-core-ts:test-e2e` already exists with `cache: false`, no skip hatch, and `inputs`
covering `src/**/*` and `tests/**/*` (`moon.yml:65-96`), and `:test-e2e` is already in CLAUDE.md's
ci-targets command. The plan must confirm with `moon task paigasus-console-core-ts:test` that the
`test` task's RESOLVED inputs cover a new file under `tests/unit/` — its `moon.yml` block does not
list `tests/**/*` itself and relies on the inherited group.

Nothing in the repo pins the `AppEventName` union: `tests/unit/logger.test.ts` exercises only
`principal.resolve_failed` (`:34`) and `discovery.redis_connect_failed` (`:47`). Adding a member
needs no gate update.

## 11. Changelog, revision 1 -> 2

The adversarial challenge returned NEEDS REWORK. Every finding below was verified against the
named file before it was folded in.

**Folded in — the deadline was wrong.**

- The deadline moved from `3T` to `4T`. Revision 1 derived the floor as `socketTimeout = 2T` and
  so believed `3T` was "strictly above" it. It is not: node-redis writes one more PING after the
  wedge begins (§ 2 fact 4), which resets the idle timer, so the worst case is
  `pingInterval + socketTimeout = 3T`. Revision 1's deadline TIED with the race it existed to
  avoid. § 2 gained fact 4 and § 3.1 now shows the timeline.
- The cooldown stays `4T`, with the derivation stated. The challenge asked for it to be re-derived
  above the new deadline; it does not compose that way, because it is measured from the moment the
  circuit opens, not from the start of the wedge. § 3.1 says so explicitly, and adds why an early
  cooldown expiry is harmless (§ 2 fact 6).

**Folded in — three claims were false.**

- § 5 said a warm render serves its stale record. It usually does NOT: `resolveService`'s first
  cache call is the `get` at `single-flight.ts:113`, and its rejection returns
  `degraded('cache-unavailable')` at `:255-262`. So an open circuit degrades EVERY service's
  navigation. § 5 is rewritten around which call rejects, and § 8 risk 3 states the blast radius.
- § 6.2 said T1-T5 stay green "because the deadline is above socketTimeout". The real reason is
  that none of them has an operation in flight during its idle gap, so none arms a deadline.
- Revision 1's T6 would have PASSED before the fix. An awaited driver loop stops the traffic, so
  the idle timer fires and node-redis rejects at about `2T`-`3T` on unmodified code. T6 is now
  specified as fire-and-forget at an interval below `socketTimeout`, and asserts the ERROR TYPE
  rather than a wall-clock window.

**Folded in — three things were undefined.**

- Concurrency was never discussed, and it falsified the "one log line" claim: every concurrent
  operation expires at once. D6 now guards the circuit opening with `if (openedAt === null)`, § 3.2
  states the concurrency, and U7 tests three simultaneous expiries.
- A circuit-open rejection had no error type. D9 defines one class with a `phase` field.
- The Scope line forbade two changes the design requires: `AppEventName` is a closed union, so
  `src/logger.ts` must change, and four documents carry a stale "follow-up SMA-650" reference.
  § 9 lists them.

**Folded in — a deferred fact was measurable.** Revision 1 deferred "does `disableOfflineQueue`
make commands fail fast" to implementation and hedged that "the design holds either way". It is
readable at `client/index.js:1125-1132`. It is now § 2 fact 6 and the hedge is gone.

**Folded in — smaller corrections.** The rejected alternatives (destroy and rebuild, half-open)
are now named in § 6 with why they lose. Citations corrected: `tryAcquireLock` at `:332` not
`:331`, `releaseLock` at `:222` not `:221`, four release sites not five, `socket.js:293-301` and
`:313-336`. Also recorded: three call sites swallow with no log; `get` can issue a second Redis
command; `close()` cannot hang because the adapter's is `Promise.resolve()`; the wrapper must not
add an `error` listener, because an existing test pins `listenerCount('error') === 1`; the unit
tier pins `toFake` explicitly; T6's driver promises each need a `.catch`; U8 proves the deadline
timer was cleared.

**Accepted as correct, no change needed.** The challenge verified D4's central premise
independently — nothing else writes to this client's socket, so stopping our writes really does let
the idle timer fire. It also confirmed D1, and that `:test-e2e` needs no Moon or CI registration.

**Not folded in.** Nothing was rejected outright. Two items became explicit non-decisions instead:
the half-open circuit is recorded as a rejected alternative with its cost stated (§ 6, § 8 risk 2)
rather than adopted, and re-deriving the cooldown above the deadline was answered with a
derivation (§ 3.1) rather than a value change.
