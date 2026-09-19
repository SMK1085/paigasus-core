// SPDX-License-Identifier: Apache-2.0
//
// Capability discovery for this app (spec § 4.4). ONE Discovery handle PER REQUEST (React cache()),
// as @paigasus/discovery requires (ts/packages/paigasus-discovery/src/server.ts:63-82), over a
// descriptor cache that is a PROCESS singleton.
//
//   PAIGASUS_SESSION_STORE=redis  -> the SAME Redis URL as the session store, so an operator
//                                    configures one Redis. A lazy node-redis client, made on first
//                                    use with the preconditions createRedisDescriptorCache asserts
//                                    (ts/packages/paigasus-discovery/src/adapters/redis-cache.ts).
//   PAIGASUS_SESSION_STORE=memory -> the memory cache.
//
// There is NO silent fallback from Redis to memory. When Redis cannot connect, the cache fails
// fast, discovery reports its own `cache-unavailable` degraded reason, and this file logs
// `discovery.redis_connect_failed` once, with no DSN.
//
// THE REDIS CLIENT MUST SURVIVE AN IDLE SOCKET (SMA-648). Read from @redis/client 6.2.1:
//
// - `socket.socketTimeout` (2 × PAIGASUS_SESSION_REDIS_TIMEOUT_MS) is an IDLE timer, not a reply
//   deadline. node-redis calls Node's socket.setTimeout, which fires after that many ms with no
//   socket activity, and any read OR write resets it. So it fires on a quiet, healthy connection.
//   It bounds a command in flight only while nothing else is written to the socket: under steady
//   traffic each write moves the deadline, and a Redis that accepts commands and never replies can
//   go unnoticed. withOperationDeadline below is the end-to-end bound (SMA-650).
// - `pingInterval` (PAIGASUS_SESSION_REDIS_TIMEOUT_MS) sends PING while the socket is ready. The
//   PING and its reply are socket activity, so a healthy idle socket never reaches the idle timer.
//   The gap between "the ping is due" and "the idle timer fires" is one timeout. That gap is the
//   tolerance for event-loop lag.
// - The PING RATE depends on PAIGASUS_SESSION_REDIS_TIMEOUT_MS: one PING per timeout, per process.
//   A value of 1 gives one PING per ms. There is deliberately no floor: a floor would break the
//   `pingInterval <= socketTimeout / 2` precondition for small values (spec § 3).
// - node-redis's DEFAULT reconnect strategy returns `false` for a SocketTimeoutError, which closes
//   the client for the life of the process. descriptorCacheReconnectStrategy returns a delay for
//   EVERY cause and has no retry cap, for the reason @paigasus/auth's reconnectStrategy records:
//   one client, no recreate path, so a stop is permanent.
//
// The Redis user needs the `+ping` ACL permission. Without it every PING gets a `-NOPERM` reply.
// That reply is still socket activity, so the socket stays alive, and watchConnectionLoss ignores
// the error (the socket stays ready), so nothing reports the missing permission. Grant it.
import 'server-only';
import { createClient, SocketClosedUnexpectedlyError, SocketTimeoutError, type RedisClientType } from 'redis';
import { createDiscovery, createMemoryDescriptorCache, createRedisDescriptorCache, timingsFromEnv, type DescriptorCache, type Discovery } from '@paigasus/discovery/server';
import type { ConsoleCoreConfig } from './config-shape';
import { logger, type ConsoleLogger } from './logger';

// MODULE scope is not enough. `next dev` recompiles the module graph on every edit, so a
// module-level singleton is remade each time and its Redis client leaks — one per recompile, for
// the life of a dev session. @paigasus/auth's getAuthRuntime carries the same fix for the same
// reason (src/runtime.ts:171-230): hold the state on globalThis, under a key that survives a
// reload. `Symbol.for` resolves through a registry that is GLOBAL to the process, keyed by the
// string given to it — every module that calls `Symbol.for` with the same string gets the SAME
// symbol back, so the key must be specific to this cache AND carry a version segment, the same
// reasoning `runtime.ts`'s `RUNTIME_KEY_PREFIX` states for its own key. The key carries no zone
// segment, and that is deliberate: `descriptorCacheFor` is documented above as a process-wide
// singleton, so sharing one cache across every zone composed into this process is the wanted
// behaviour, not the bug the zone segment in `runtime.ts`'s key exists to avoid.
type DiscoveryState = { processCache?: DescriptorCache | undefined; redisClient?: RedisClientType | undefined };

const DISCOVERY_STATE = Symbol.for('paigasus.console-core.discovery-state.v1');

function state(): DiscoveryState {
  const holder = globalThis as typeof globalThis & { [DISCOVERY_STATE]?: DiscoveryState };
  holder[DISCOVERY_STATE] ??= {};
  return holder[DISCOVERY_STATE];
}

const MAX_RECONNECT_DELAY_MS = 2000;
const MAX_RECONNECT_JITTER_MS = 200;

/**
 * The descriptor cache's reconnect strategy (SMA-648 D3). It returns a delay for EVERY cause,
 * SocketTimeoutError included, and never `false` or an Error: `min(2^retries × 50, 2000)` ms plus
 * 0–199 ms of jitter. There is no retry cap (see the file header). node-redis starts the FIRST
 * reconnect at once and uses this value only as a type check (@redis/client socket.js
 * #onSocketError), so the delay applies from the second attempt.
 */
export function descriptorCacheReconnectStrategy(retries: number): number {
  const jitter = Math.floor(Math.random() * MAX_RECONNECT_JITTER_MS);
  return Math.min(2 ** retries * 50, MAX_RECONNECT_DELAY_MS) + jitter;
}

/** The fixed `reason` values of `discovery.redis_connection_lost` (SMA-648 D7). */
export type ConnectionLossReason = 'socket_timeout' | 'socket_closed' | 'other';

/**
 * The part of a node-redis client that watchConnectionLoss reads. Structural, so the default test
 * tier can drive it with a plain EventEmitter.
 */
export type ConnectionLossSource = {
  readonly isOpen: boolean;
  readonly isReady: boolean;
  on(event: 'ready' | 'error', listener: (error: unknown) => void): unknown;
};

/**
 * A fixed value from `instanceof` checks, never the message or `constructor.name` (SMA-648 D7). A
 * node-redis error message can embed the DSN, the error classes do not set `name`, and a
 * production bundle can mangle `constructor.name`. SocketTimeoutDuringMaintenanceError is not a
 * subclass of SocketTimeoutError, so it maps to `other`.
 */
function connectionLossReason(error: unknown): ConnectionLossReason {
  if (error instanceof SocketTimeoutError) return 'socket_timeout';
  if (error instanceof SocketClosedUnexpectedlyError) return 'socket_closed';
  return 'other';
}

/**
 * Registers the client's `ready` and `error` listeners (SMA-648 D6, D8). The `error` listener is
 * the one createRedisDescriptorCache REQUIRES, and it never reads the error message: node-redis
 * embeds the DSN in its connection errors.
 *
 * It logs `discovery.redis_connection_lost` ONCE PER LOSS and ONLY FOR A REAL LOSS: when it is
 * armed, `client.isOpen`, and `!client.isReady`. Then it disarms. A `ready` event arms it again.
 * node-redis emits `error` for events that do NOT drop the socket (an error reply to a PING, a
 * decoder error, a PING that destroy() flushes); in those the socket stays ready, or the client is
 * no longer open. A real loss passes #onSocketError, which sets isReady = false BEFORE it emits,
 * and under descriptorCacheReconnectStrategy isOpen stays true. Errors before the first `ready`
 * are connect failures, which connectOnce reports. Handshake errors inside a reconnect loop arrive
 * while it is disarmed.
 */
export function watchConnectionLoss(client: ConnectionLossSource, log: ConsoleLogger): void {
  let armed = false;
  client.on('ready', () => {
    armed = true;
  });
  client.on('error', (error: unknown) => {
    if (!armed || !client.isOpen || client.isReady) return;
    armed = false;
    log.appEvent('discovery.redis_connection_lost', { reason: connectionLossReason(error) });
  });
}

/**
 * Waits for the first connect, but never longer than one command timeout. node-redis's connect()
 * keeps retrying while its reconnect strategy returns a delay (@redis/client socket.js #connect),
 * so an unreachable Redis would otherwise hold every render. The connect keeps running in the
 * background, so the cache recovers when Redis does.
 *
 * The rejection handler logs NOTHING (SMA-648 D11). Under descriptorCacheReconnectStrategy,
 * connect() rejects only when destroy() runs during a connect, and "connect failed" is the wrong
 * line for that. The handler stays so that a rejected connect() is never an unhandled rejection.
 * The `connect_timeout` line still covers an unreachable Redis.
 */
function connectOnce(client: RedisClientType, timeoutMs: number, log: ConsoleLogger): Promise<void> {
  let settled = false;
  const connected = client.connect().then(
    () => {
      settled = true;
    },
    () => {
      settled = true;
    },
  );
  const bounded = new Promise<void>((resolve) => {
    setTimeout(() => {
      if (!settled) log.appEvent('discovery.redis_connect_failed', { stage: 'connect_timeout' });
      resolve();
    }, timeoutMs).unref();
  });
  return Promise.race([connected, bounded]);
}

/** Wraps a cache so that every operation first waits (boundedly) for the first connect. */
function afterConnect(inner: DescriptorCache, ready: Promise<void>): DescriptorCache {
  return {
    get: async (service) => {
      await ready;
      return inner.get(service);
    },
    set: async (service, rec, ttlMs, expectedRev) => {
      await ready;
      return inner.set(service, rec, ttlMs, expectedRev);
    },
    delete: async (service) => {
      await ready;
      return inner.delete(service);
    },
    tryAcquireLock: async (service, token, ttlMs) => {
      await ready;
      return inner.tryAcquireLock(service, token, ttlMs);
    },
    releaseLock: async (service, token) => {
      await ready;
      return inner.releaseLock(service, token);
    },
    close: () => inner.close(),
  };
}

/** How many command timeouts one cache operation may take, end to end (SMA-650 D3). */
const DEADLINE_FACTOR = 4;

/** How many command timeouts the wrapper stays silent after an expiry (SMA-650 D5). */
const COOLDOWN_FACTOR = 4;

/** Which half of the wrapper refused the operation (SMA-650 D9). */
export type DescriptorCacheTimeoutPhase = 'deadline' | 'circuit-open';

/**
 * The descriptor cache did not answer. `phase` says why: `deadline` means this operation itself ran
 * past its bound, `circuit-open` means an earlier one did and this one was refused without touching
 * the socket (SMA-650 D9).
 *
 * `name` is set explicitly, because a production bundle can mangle `constructor.name` — the same
 * reasoning connectionLossReason records above. The message NEVER carries the DSN, the URL, or a
 * node-redis error: `operation` is one of five fixed literals.
 */
export class DescriptorCacheTimeoutError extends Error {
  readonly phase: DescriptorCacheTimeoutPhase;

  constructor(operation: string, deadlineMs: number, phase: DescriptorCacheTimeoutPhase) {
    super(
      phase === 'deadline'
        ? `the descriptor cache operation "${operation}" did not answer within ${deadlineMs} ms`
        : `the descriptor cache is not answering: "${operation}" was refused while the circuit was open`,
    );
    this.name = 'DescriptorCacheTimeoutError';
    this.phase = phase;
  }
}

/**
 * The part of a node-redis client the deadline wrapper reads. Structural, so the default test tier
 * can drive it with a plain EventEmitter — the same shape ConnectionLossSource uses above.
 *
 * It carries `ready` ONLY. An `error` listener here would duplicate watchConnectionLoss's job and
 * would break its test's pin on listenerCount('error') === 1.
 */
export type ReadySource = { on(event: 'ready', listener: () => void): unknown };

/**
 * Bounds every cache operation (SMA-650). `commandOptions.timeout` covers only the queued phase and
 * `socketTimeout` is an idle timer that any write resets, so a Redis that accepts commands and never
 * replies is unbounded under steady traffic. This is the only end-to-end bound.
 */
export function withOperationDeadline(inner: DescriptorCache, client: ReadySource, timeoutMs: number, log: ConsoleLogger): DescriptorCache {
  const deadlineMs = timeoutMs * DEADLINE_FACTOR;
  const cooldownMs = timeoutMs * COOLDOWN_FACTOR;
  // null = closed. Otherwise the Date.now() at which it opened (D4, D5). One field, mutated only
  // from the event loop's single thread, so D6's read-then-write cannot interleave.
  let openedAt: number | null = null;

  // `ready` is the exact signal that node-redis finished a reconnect. NO error listener (D4 note).
  client.on('ready', () => {
    openedAt = null;
  });

  const bounded = async <T>(operation: string, run: () => Promise<T>): Promise<T> => {
    if (openedAt !== null) {
      // While the cooldown runs we write NOTHING. That silence is what lets node-redis's idle timer
      // fire and its reconnect strategy repair the socket (SMA-650 § 2 fact 4) — it is the repair
      // mechanism, not only a cost saving.
      // Date.now() is wall-clock, not monotonic (house style here — single-flight uses deps.now()).
      // If the host clock steps BACKWARDS (a VM resume, chrony makestep), this comparison stays
      // true for the length of the step, so the circuit stays open that much longer than
      // cooldownMs and every service's navigation degrades for that long. Containers normally
      // slew rather than step, so this is accepted, not fixed.
      if (Date.now() - openedAt < cooldownMs) throw new DescriptorCacheTimeoutError(operation, deadlineMs, 'circuit-open');
      // The cooldown elapsed with no `ready`. Let this operation through: if the client is still not
      // ready, node-redis refuses it at once (disableOfflineQueue), so the attempt costs nothing.
      openedAt = null;
    }
    let timer: ReturnType<typeof setTimeout> | undefined;
    const running = run();
    try {
      return await Promise.race([
        running,
        new Promise<never>((_resolve, reject) => {
          timer = setTimeout(() => reject(new DescriptorCacheTimeoutError(operation, deadlineMs, 'deadline')), deadlineMs);
          timer.unref();
        }),
      ]);
    } catch (error) {
      if (error instanceof DescriptorCacheTimeoutError && error.phase === 'deadline') {
        // D6: concurrent operations all expire together. Only the FIRST opens the circuit and logs,
        // or one wedge would log a line per operation and the cooldown would never elapse.
        if (openedAt === null) {
          openedAt = Date.now();
          log.appEvent('discovery.redis_operation_timeout', { operation, deadlineMs });
        }
        // D7: see Task 1.
        running.catch(() => undefined);
      }
      throw error;
    } finally {
      clearTimeout(timer);
    }
  };

  return {
    get: (service) => bounded('get', () => inner.get(service)),
    set: (service, rec, ttlMs, expectedRev) => bounded('set', () => inner.set(service, rec, ttlMs, expectedRev)),
    delete: (service) => bounded('delete', () => inner.delete(service)),
    tryAcquireLock: (service, token, ttlMs) => bounded('tryAcquireLock', () => inner.tryAcquireLock(service, token, ttlMs)),
    releaseLock: (service, token) => bounded('releaseLock', () => inner.releaseLock(service, token)),
    close: () => inner.close(),
  };
}

function redisDescriptorCache(url: string, timeoutMs: number, log: ConsoleLogger): DescriptorCache {
  const client: RedisClientType = createClient({
    url,
    disableOfflineQueue: true,
    commandOptions: { timeout: timeoutMs },
    // D2: a PING every timeout keeps a healthy idle socket under the idle timer (file header).
    pingInterval: timeoutMs,
    // D1: the idle timer stays; it is the only bound on a hung command. D3: the strategy reopens
    // the socket after the timer fires on a real hang.
    socket: { socketTimeout: timeoutMs * 2, connectTimeout: timeoutMs, reconnectStrategy: descriptorCacheReconnectStrategy },
  });
  // Registers the error listener createRedisDescriptorCache requires. It never logs the error's message.
  watchConnectionLoss(client, log);
  state().redisClient = client;
  const inner = createRedisDescriptorCache(client);
  // The deadline wraps OUTSIDE afterConnect, so it bounds the `await ready` too and `4 × timeoutMs`
  // is the whole bound, not an addition to the connect wait (SMA-650 § 4.1).
  return withOperationDeadline(afterConnect(inner, connectOnce(client, timeoutMs, log)), client, timeoutMs, log);
}

/**
 * The process-wide descriptor cache for this configuration.
 *
 * NO SILENT FALLBACK (see the file header), so `redis` with no URL THROWS. That pair reaches this
 * function: the app's flat zod config shape cannot express the cross-field rule, and
 * `createAuthRuntime` — which does hold that rule — may not have run yet for this request. The old
 * code read the pair as "use memory", which is the exact silent downgrade the header forbids: every
 * zone would then cache descriptors in its own process and AC 2 would not hold. The message names
 * the two variables and never the URL, which carries a password.
 */
export function descriptorCacheFor(config: ConsoleCoreConfig, log: ConsoleLogger = logger): DescriptorCache {
  const current = state();
  if (current.processCache !== undefined) return current.processCache;
  if (config.PAIGASUS_SESSION_STORE === 'redis') {
    if (config.PAIGASUS_SESSION_REDIS_URL === undefined) {
      throw new Error('PAIGASUS_SESSION_REDIS_URL is required when PAIGASUS_SESSION_STORE is "redis"');
    }
    current.processCache = redisDescriptorCache(config.PAIGASUS_SESSION_REDIS_URL, config.PAIGASUS_SESSION_REDIS_TIMEOUT_MS, log);
  } else {
    current.processCache = createMemoryDescriptorCache();
  }
  return current.processCache;
}

/**
 * Test and shutdown seam: forget the process cache and close the Redis client, if any.
 *
 * It destroys the client ONLY while it is still open (SMA-648 D10). node-redis's destroy() throws
 * ClientClosedError on a client that is already closed (@redis/client socket.js destroy()), and a
 * throw here would skip the two lines that clear the state, so the next caller would inherit a
 * dead cache.
 */
export function resetDiscoveryForTest(): void {
  const current = state();
  if (current.redisClient?.isOpen === true) current.redisClient.destroy();
  current.redisClient = undefined;
  current.processCache = undefined;
}

/** A Discovery handle for one request. Pure apart from the process cache: tests call it directly. */
export function createAppDiscovery(deps: { config: ConsoleCoreConfig; log?: ConsoleLogger; fetch?: typeof globalThis.fetch; waitUntil?: (p: Promise<unknown>) => void }): Discovery {
  const log = deps.log ?? logger;
  return createDiscovery({
    services: deps.config.PAIGASUS_SERVICES,
    cache: descriptorCacheFor(deps.config, log),
    logger: log,
    timings: timingsFromEnv(deps.config),
    ...(deps.fetch === undefined ? {} : { fetch: deps.fetch }),
    ...(deps.waitUntil === undefined ? {} : { waitUntil: deps.waitUntil }),
  });
}
