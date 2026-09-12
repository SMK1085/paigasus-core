// SPDX-License-Identifier: Apache-2.0
//
// Capability discovery for this app (spec § 4.4). ONE Discovery handle PER REQUEST (React cache()),
// as @paigasus/discovery requires (ts/packages/paigasus-discovery/src/server.ts:63-82), over a
// descriptor cache that is a PROCESS singleton.
//
//   PAIGASUS_SESSION_STORE=redis  -> the SAME Redis URL as the session store, so an operator
//                                    configures one Redis. A lazy node-redis client, made on first
//                                    use with the four preconditions createRedisDescriptorCache
//                                    asserts (src/adapters/redis-cache.ts:67-90).
//   PAIGASUS_SESSION_STORE=memory -> the memory cache.
//
// There is NO silent fallback from Redis to memory. When Redis cannot connect, the cache fails
// fast, discovery reports its own `cache-unavailable` degraded reason, and this file logs
// `discovery.redis_connect_failed` once, with no DSN.
import 'server-only';
import { after } from 'next/server';
import { cache } from 'react';
import { createClient, type RedisClientType } from 'redis';
import { createDiscovery, createMemoryDescriptorCache, createRedisDescriptorCache, timingsFromEnv, type DescriptorCache, type Discovery } from '@paigasus/discovery/server';
import { getRuntimeConfig, type ConsoleConfig } from './config';
import { logger, type ConsoleLogger } from './logger';

let processCache: DescriptorCache | undefined;
let redisClient: RedisClientType | undefined;

/**
 * Waits for the first connect, but never longer than one command timeout. node-redis's connect()
 * keeps retrying while its reconnect strategy returns a delay (@redis/client socket.js #connect),
 * so an unreachable Redis would otherwise hold every render. The connect keeps running in the
 * background, so the cache recovers when Redis does.
 */
function connectOnce(client: RedisClientType, timeoutMs: number, log: ConsoleLogger): Promise<void> {
  let settled = false;
  const connected = client.connect().then(
    () => {
      settled = true;
    },
    () => {
      settled = true;
      log.appEvent('discovery.redis_connect_failed', { stage: 'connect' });
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

function redisDescriptorCache(url: string, timeoutMs: number, log: ConsoleLogger): DescriptorCache {
  const client: RedisClientType = createClient({ url, disableOfflineQueue: true, commandOptions: { timeout: timeoutMs }, socket: { socketTimeout: timeoutMs * 2, connectTimeout: timeoutMs } });
  // REQUIRED by createRedisDescriptorCache, and it must NEVER log the error: node-redis embeds the
  // DSN in its connection errors. Reconnects fire it repeatedly, so it logs nothing at all.
  client.on('error', () => undefined);
  redisClient = client;
  const inner = createRedisDescriptorCache(client);
  return afterConnect(inner, connectOnce(client, timeoutMs, log));
}

/** The process-wide descriptor cache for this configuration. */
export function descriptorCacheFor(config: ConsoleConfig, log: ConsoleLogger = logger): DescriptorCache {
  if (processCache !== undefined) return processCache;
  if (config.PAIGASUS_SESSION_STORE === 'redis' && config.PAIGASUS_SESSION_REDIS_URL !== undefined) {
    processCache = redisDescriptorCache(config.PAIGASUS_SESSION_REDIS_URL, config.PAIGASUS_SESSION_REDIS_TIMEOUT_MS, log);
  } else {
    processCache = createMemoryDescriptorCache();
  }
  return processCache;
}

/** Test and shutdown seam: forget the process cache and close the Redis client, if any. */
export function resetDiscoveryForTest(): void {
  redisClient?.destroy();
  redisClient = undefined;
  processCache = undefined;
}

/** A Discovery handle for one request. Pure apart from the process cache: tests call it directly. */
export function createAppDiscovery(deps: { config: ConsoleConfig; log?: ConsoleLogger; fetch?: typeof globalThis.fetch; waitUntil?: (p: Promise<unknown>) => void }): Discovery {
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

/** One handle per request; background revalidation runs after the response, through after(). */
export const discovery: () => Discovery = cache(() => createAppDiscovery({ config: getRuntimeConfig(), waitUntil: (p) => after(p) }));
