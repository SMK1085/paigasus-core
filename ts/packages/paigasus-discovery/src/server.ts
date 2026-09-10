// SPDX-License-Identifier: Apache-2.0
import 'server-only';

import { DEFAULT_TIMINGS, type Timings } from './core/record.js';
import { serviceOf } from './core/state.js';
import { resolveService, type ResolveDeps } from './core/single-flight.js';
import { probeService, type ProbeOutcome } from './probe.js';
import { noopLogger } from './adapters/noop-logger.js';
import type { DescriptorCache } from './ports/cache.js';
import type { DiscoveryLogger } from './ports/logger.js';
import type { CapabilityKey, ServiceState } from './types.js';

export type { DescriptorCache } from './ports/cache.js';
export type { DiscoveryLogger, DiscoveryEventName, DiscoveryEventFields } from './ports/logger.js';
export { createMemoryDescriptorCache } from './adapters/memory-cache.js';
export { createRedisDescriptorCache } from './adapters/redis-cache.js';
export { noopLogger } from './adapters/noop-logger.js';
export { discoveryEnvShape, parseServiceMap, timingsFromEnv } from './config.js';
export type { DiscoveryEnv } from './config.js';
export { DEFAULT_TIMINGS } from './core/record.js';
export type { Timings } from './core/record.js';
export type { ServiceState, ServiceDescriptor, DegradedReason, CapabilityKey } from './types.js';

const MAX_DESCRIPTOR_BYTES = 64 * 1024;

export type CreateDiscoveryDeps = {
  readonly services: Readonly<Record<string, string>>;
  readonly cache: DescriptorCache;
  readonly logger?: DiscoveryLogger;
  readonly fetch?: typeof globalThis.fetch;
  readonly waitUntil?: (p: Promise<unknown>) => void;
  readonly timings?: Partial<Timings>;
  readonly now?: () => number;
  /** Test seam. Production always uses `probeService`. */
  readonly probe?: (service: string, token: string) => Promise<ProbeOutcome>;
};

export type Discovery = {
  getServiceState(service: string, token: string): Promise<ServiceState>;
  hasCapability(key: CapabilityKey, token: string): Promise<boolean>;
};

function resolveTimings(over: Partial<Timings> | undefined): Timings {
  const t: Timings = { ...DEFAULT_TIMINGS, ...over };
  // The probe must fit inside the wait, and the wait inside the lock's lifetime. A loser whose
  // deadline expires before the winner can probe AND write reports a false outage on the first
  // render after every deploy.
  if (!(t.probeTimeoutMs < t.lockWaitMs)) {
    throw new Error(`discovery: probeTimeoutMs (${t.probeTimeoutMs}) must be < lockWaitMs (${t.lockWaitMs})`);
  }
  if (!(t.lockWaitMs < t.lockTtlMs)) {
    throw new Error(`discovery: lockWaitMs (${t.lockWaitMs}) must be < lockTtlMs (${t.lockTtlMs})`);
  }
  if (!(t.negativeMs < t.freshMs)) {
    throw new Error(`discovery: negativeMs (${t.negativeMs}) must be < freshMs (${t.freshMs})`);
  }
  if (!(t.freshMs < t.staleMs)) {
    throw new Error(`discovery: freshMs (${t.freshMs}) must be < staleMs (${t.staleMs})`);
  }
  return t;
}

/**
 * Build a discovery handle. Call this ONCE PER REQUEST.
 *
 * This is the OPPOSITE lifetime from @paigasus/auth's createAuthRuntime(), which
 * (runtime.ts:24-33) is ONE RUNTIME PER PROCESS. Do not read the two as analogous: the memo
 * below never evicts an entry, because there is deliberately nothing to evict FOR — the handle
 * dies with the request. A process-wide handle would keep the FIRST caller's outcome forever:
 * a second caller with a different token would receive the identical object the first caller
 * got, so one rejected token would make every later request see degraded/unauthorized for the
 * process's life, and freshness, stale-while-revalidate and the negative TTL would all become
 * unreachable, because the memo answers before the cache is ever consulted.
 *
 * There is deliberately NO module-level singleton: the composition root builds a fresh handle
 * for every request, which is also what makes this testable without module resets.
 *
 * `getServiceState` memoizes per handle. Without it a nav with eight <Capability> items over two
 * services costs eight cache reads per render, because React renders server components in tree
 * order and each sibling would resolve serially — which is also why the "one lock wait"
 * worst-case latency bound is stated conditionally on concurrent resolution.
 */
export function createDiscovery(deps: CreateDiscoveryDeps): Discovery {
  const timings = resolveTimings(deps.timings);
  const logger = deps.logger ?? noopLogger;
  const fetchImpl = deps.fetch ?? globalThis.fetch;
  // Keyed on service AND token, as a cheap second guard beyond the per-request lifetime rule
  // above — so two callers with different tokens for the same service never share a result.
  // Entries are never removed: the handle is per-request, so nothing here can outlive its
  // request, and evicting would only reintroduce cost this memo exists to avoid.
  const inflight = new Map<string, Promise<ServiceState>>();

  const probe =
    deps.probe ??
    ((service: string, token: string): Promise<ProbeOutcome> =>
      probeService({
        baseUrl: deps.services[service] ?? '',
        token,
        timeoutMs: timings.probeTimeoutMs,
        maxBytes: MAX_DESCRIPTOR_BYTES,
        fetch: fetchImpl,
      }));

  const resolveDeps: ResolveDeps = {
    cache: deps.cache,
    logger,
    timings,
    now: deps.now ?? Date.now,
    probe,
    ...(deps.waitUntil === undefined ? {} : { waitUntil: deps.waitUntil }),
  };

  function getServiceState(service: string, token: string): Promise<ServiceState> {
    // Object.hasOwn, never a bare `=== undefined` check: `deps.services` is typed as a plain
    // `Readonly<Record<string, string>>`, so a caller CAN pass an object literal rather than
    // `parseServiceMap`'s null-prototype result. `deps.services['constructor'] === undefined` is
    // false — it finds the inherited `Function` on `Object.prototype` — so `service: 'constructor'`
    // (or `'__proto__'`, `'toString'`, ...) would skip the `absent` branch entirely and hand a
    // non-URL to `probeService`, which then reports a configured service as `degraded` instead of
    // `absent`. The package targets ES2022, so `Object.hasOwn` is available.
    if (!Object.hasOwn(deps.services, service)) {
      // ABSENT is decided from config alone. No cache read, no probe.
      return Promise.resolve({ state: 'absent', service });
    }
    const key = `${service}:${token}`;
    const existing = inflight.get(key);
    if (existing !== undefined) return existing;
    const pending = resolveService(resolveDeps, service, token);
    inflight.set(key, pending);
    return pending;
  }

  async function hasCapability(key: CapabilityKey, token: string): Promise<boolean> {
    const state = await getServiceState(serviceOf(key), token);
    // `degraded` is FALSE: the feature must not be invoked. <Capability> still renders it
    // disabled rather than hidden — the two deliberately disagree.
    return state.state === 'available' && state.capabilities.includes(key);
  }

  return { getServiceState, hasCapability };
}
