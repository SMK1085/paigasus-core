// SPDX-License-Identifier: Apache-2.0
//
// Stale-while-revalidate with a single-flight lock, modelled on
// ts/packages/paigasus-auth/src/core/single-flight.ts. Its five invariants are preserved:
//
//  1. DOUBLE-CHECK after acquiring the lock. Another holder may have finished between the failed
//     read and the successful acquire.
//  2. A WAITER NEVER PROBES on timeout. It reports degraded/timeout instead. A fallback probe
//     would defeat the whole mechanism under exactly the load it exists for.
//  3. UNIQUE LOCK TOKEN plus compare-and-delete release, so a holder whose TTL expired cannot
//     release the next holder's lock.
//  4. RELEASE IN `finally`, and that release is itself wrapped in try/catch so a store error at
//     release time cannot mask the probe's real outcome.
//  5. THE WRITE IS FENCED on `rev`. A compare-and-delete protects the LOCK, never the WRITE.
//     This matters more here than in auth: a background revalidation has NO wall-clock bound
//     (AbortSignal.timeout bounds the network wait only, not parsing, not the write, not an
//     event-loop stall), so an old probe could otherwise overwrite a newer success with its own
//     stale failure and mask a healthy service until the hard TTL. The fencing token is the
//     record snapshot taken BEFORE the probe ran, never a value re-read just before writing —
//     re-reading immediately before the write would make the compare-and-set trivially agree
//     with whatever is already there and defeat the fence.

import { RECORD_VERSION, isFresh, toState, type CacheRecord, type Timings } from './record.js';
import type { ProbeOutcome } from '../probe.js';
import type { DescriptorCache } from '../ports/cache.js';
import type { DiscoveryEventFields, DiscoveryEventName, DiscoveryLogger } from '../ports/logger.js';
import type { DegradedReason, ServiceState } from '../types.js';

export type ResolveDeps = {
  readonly cache: DescriptorCache;
  readonly logger: DiscoveryLogger;
  readonly timings: Timings;
  readonly now: () => number;
  readonly probe: (service: string, token: string) => Promise<ProbeOutcome>;
  readonly waitUntil?: (p: Promise<unknown>) => void;
  readonly sleep?: (ms: number) => Promise<void>;
};

const defaultSleep = (ms: number): Promise<void> => new Promise((r) => setTimeout(r, ms));

/** Backoff with jitter, capped. Mirrors auth's waiter. */
const backoff = (attempt: number): number => Math.min(10 * 2 ** attempt, 100) * (0.5 + Math.random());

function newLockToken(): string {
  return `${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`;
}

/**
 * `resolveService` is documented to never reject. Two call sites below sit outside any
 * surrounding try: one runs INSIDE a catch block, where an unguarded throw would replace the
 * handled error with a new, unhandled one; the other has no enclosing try at all. A throwing
 * injected logger would therefore make a rejection reachable, and — because that rejected
 * promise is what the handle's memo map in server.ts stores — it would then be cached and
 * replayed for the handle's whole (per-request) life. Swallow rather than propagate.
 */
function safeLog(deps: ResolveDeps, name: DiscoveryEventName, fields: DiscoveryEventFields): void {
  try {
    deps.logger.event(name, fields);
  } catch {
    // Nothing to log to if the logger itself is broken.
  }
}

function degraded(service: string, reason: DegradedReason, rec: CacheRecord | null): ServiceState {
  return {
    state: 'degraded',
    service,
    reason,
    descriptor: rec?.descriptor ?? null,
    capabilities: rec?.descriptor?.capabilities ?? [],
  };
}

/** A 401/403 is about the CALLER, not the service, so it must never reach the shared entry. */
function isCallerScoped(reason: DegradedReason): boolean {
  return reason === 'unauthorized';
}

function recordFor(outcome: ProbeOutcome, prev: CacheRecord | null, now: number): CacheRecord {
  const rev = (prev?.rev ?? 0) + 1;
  if (outcome.ok) {
    return {
      version: RECORD_VERSION,
      rev,
      descriptor: outcome.descriptor,
      descriptorAt: now,
      outcome: 'ok',
      outcomeAt: now,
      reason: null,
    };
  }
  return {
    version: RECORD_VERSION,
    rev,
    // The last GOOD descriptor survives a failure. This is what lets a degraded service render
    // its known feature list, each item disabled with a reason, instead of collapsing the nav.
    descriptor: prev?.descriptor ?? null,
    descriptorAt: prev?.descriptorAt ?? 0,
    outcome: 'fail',
    outcomeAt: now,
    reason: outcome.reason,
  };
}

async function readRecord(deps: ResolveDeps, service: string): Promise<CacheRecord | null> {
  const rec = await deps.cache.get(service);
  if (rec === null) return null;
  // toState returns null for an internally impossible record (ok with no descriptor). Treat it
  // as corrupt: delete and re-probe.
  if (toState(service, rec) === null) {
    deps.logger.event('discovery.record_discarded', { service });
    await deps.cache.delete(service);
    return null;
  }
  return rec;
}

/**
 * Race `deps.probe` against `probeTimeoutMs`. `deps.probe` is invoked SYNCHRONOUSLY, as the very
 * first statement, so a caller that only needs the probe to have STARTED (never its result) can
 * rely on it having been called the instant this function is entered — no `await` stands between
 * this call and the underlying `deps.probe` call.
 *
 * On timeout the underlying promise is left to run to completion and its eventual result is
 * discarded; there is no cancellation channel across this interface. This is what invariant 5's
 * doc comment means by "no wall-clock bound" — the fenced write below is what makes a late,
 * discarded result safe to ignore.
 */
function probeWithTimeout(deps: ResolveDeps, service: string, token: string): Promise<ProbeOutcome> {
  const probePromise = deps.probe(service, token);
  return new Promise<ProbeOutcome>((resolve) => {
    let settled = false;
    const timer = setTimeout(() => {
      if (settled) return;
      settled = true;
      resolve({ ok: false, reason: 'timeout' });
    }, deps.timings.probeTimeoutMs);
    probePromise.then(
      (outcome) => {
        if (settled) return;
        settled = true;
        clearTimeout(timer);
        resolve(outcome);
      },
      () => {
        if (settled) return;
        settled = true;
        clearTimeout(timer);
        resolve({ ok: false, reason: 'network' });
      },
    );
  });
}

/**
 * Apply a probe outcome against the snapshot taken before probing (`fresh`), fenced on ITS rev —
 * never on a value re-read just before the write. `fresh` is also what a caller-scoped failure
 * (401/403) degrades against, so a known-good descriptor keeps rendering through it.
 */
async function settleProbe(deps: ResolveDeps, service: string, fresh: CacheRecord | null, outcome: ProbeOutcome): Promise<ServiceState> {
  if (!outcome.ok) {
    deps.logger.event('discovery.probe_failed', { service, reason: outcome.reason });
    // Never cache a 401/403: the descriptor is caller-independent but the auth outcome is not.
    if (isCallerScoped(outcome.reason)) {
      return degraded(service, outcome.reason, fresh);
    }
  } else if (outcome.descriptor.service !== service) {
    // The proto MUST: the descriptor's own `service` is advisory and never a cache key. A
    // mismatch is worth logging and nothing more.
    deps.logger.event('discovery.service_mismatch', {
      configured: service,
      reported: outcome.descriptor.service,
    });
  }

  const next = recordFor(outcome, fresh, deps.now());
  const written = await deps.cache.set(service, next, deps.timings.staleMs, fresh?.rev ?? null);
  if (!written) {
    // Invariant 5. Someone else already wrote since our snapshot; theirs is newer, so it wins.
    deps.logger.event('discovery.write_fenced', { service });
    const winner = await readRecord(deps, service);
    if (winner !== null) return toState(service, winner) ?? degraded(service, 'network', winner);
    // The CAS failed (a newer write existed) yet a re-read now finds nothing: the record expired
    // or was deleted concurrently. The cache itself answered fine both times, so this is NOT a
    // `cache-unavailable` failure — there is simply no descriptor to serve. `network` is the same
    // generic fallback `toState` uses when a stored record carries no more specific reason.
    return degraded(service, 'network', null);
  }
  return toState(service, next) ?? degraded(service, 'network', next);
}

/**
 * Start probing NOW (synchronously) and return a promise for the fully-settled ServiceState.
 * Splitting the "start" from the "await" is what lets a stale-serving caller with no `waitUntil`
 * still guarantee the probe was STARTED before it returns, without having to block on it.
 */
function beginProbe(deps: ResolveDeps, service: string, token: string, fresh: CacheRecord | null): Promise<ServiceState> {
  return probeWithTimeout(deps, service, token).then((outcome) => settleProbe(deps, service, fresh, outcome));
}

/** Probe under the lock (already held) and write the result, fenced. Releases in `finally`. */
async function probeAndStore(deps: ResolveDeps, service: string, token: string, lockToken: string): Promise<ServiceState> {
  try {
    // Invariant 1: double-check. Another holder may have written between our read and our
    // acquire. This same read also becomes the fencing snapshot below.
    const fresh = await readRecord(deps, service);
    if (fresh !== null && isFresh(fresh, deps.now(), deps.timings)) {
      return toState(service, fresh) ?? degraded(service, 'network', fresh);
    }
    return await beginProbe(deps, service, token, fresh);
  } finally {
    // Invariant 4: release in `finally`, and wrap the release itself so a store error here
    // cannot mask the real outcome above.
    try {
      await deps.cache.releaseLock(service, lockToken);
    } catch {
      deps.logger.event('discovery.cache_unavailable', { service, stage: 'release_lock' });
    }
  }
}

/**
 * Await an already-started probe (`revalidated`), then release the revalidation lock. Splitting
 * this from the lock acquisition and the double-check below is deliberate: those two steps are
 * real `await`s that this function would otherwise suspend on BEFORE calling `deps.probe`, which
 * would let `resolveService` return the stale value before revalidation had even started for a
 * caller with no `waitUntil`. Acquiring the lock, double-checking, and STARTING the probe must
 * therefore happen directly in `resolveService`'s own awaited control flow; only awaiting the
 * probe's result, writing it (fenced), and releasing the lock are left running in the background.
 */
async function finishRevalidation(deps: ResolveDeps, service: string, lockToken: string, revalidated: Promise<ServiceState>): Promise<void> {
  try {
    await revalidated;
  } finally {
    try {
      await deps.cache.releaseLock(service, lockToken);
    } catch {
      deps.logger.event('discovery.cache_unavailable', { service, stage: 'release_lock' });
    }
  }
}

export async function resolveService(deps: ResolveDeps, service: string, token: string): Promise<ServiceState> {
  const sleep = deps.sleep ?? defaultSleep;

  let rec: CacheRecord | null;
  try {
    rec = await readRecord(deps, service);
  } catch {
    // Never propagate. One Redis blip must not 500 every server component rendering navigation.
    // safeLog, not deps.logger.event directly: this call already runs inside a catch block, so
    // an unguarded throw here (a broken injected logger) would replace the handled cache error
    // with a new, unhandled one and defeat the very guarantee this catch exists for.
    safeLog(deps, 'discovery.cache_unavailable', { service, stage: 'read' });
    return degraded(service, 'cache-unavailable', null);
  }

  if (rec !== null && isFresh(rec, deps.now(), deps.timings)) {
    return toState(service, rec) ?? degraded(service, 'network', rec);
  }

  if (rec !== null) {
    // STALE: serve immediately, revalidate in the background. This is AC2.
    const stale = toState(service, rec) ?? degraded(service, 'network', rec);

    const lockToken = newLockToken();
    let acquired = false;
    try {
      acquired = await deps.cache.tryAcquireLock(service, lockToken, deps.timings.lockTtlMs);
    } catch {
      deps.logger.event('discovery.cache_unavailable', { service, stage: 'revalidate_lock' });
    }

    if (acquired) {
      // Invariant 1: double-check, AWAITED HERE (not inside a detached function) — see
      // finishRevalidation's doc comment for why that placement matters.
      const fresh = await readRecord(deps, service).catch(() => null);

      if (fresh !== null && isFresh(fresh, deps.now(), deps.timings)) {
        // Another caller already revalidated between our read and our acquire. Nothing to do.
        try {
          await deps.cache.releaseLock(service, lockToken);
        } catch {
          deps.logger.event('discovery.cache_unavailable', { service, stage: 'release_lock' });
        }
      } else {
        try {
          // START the probe NOW, synchronously, as part of this directly-awaited flow — this is
          // what guarantees a caller with no `waitUntil` still sees revalidation begin before
          // this function returns. Only the tail (awaiting the outcome, writing, releasing) is
          // handed off.
          const revalidated = beginProbe(deps, service, token, fresh);
          const revalidate = finishRevalidation(deps, service, lockToken, revalidated).catch(() => {
            deps.logger.event('discovery.cache_unavailable', { service, stage: 'revalidate' });
          });
          if (deps.waitUntil !== undefined) deps.waitUntil(revalidate);
        } catch {
          // `beginProbe` calls `deps.probe` synchronously (see its doc comment). `probeService`
          // never throws, but nothing in this interface forbids it, and an unguarded throw here
          // would propagate out of `resolveService` and leak this lock for its full TTL. Mirrors
          // `probeAndStore`'s try/finally for the cold path.
          deps.logger.event('discovery.cache_unavailable', { service, stage: 'revalidate' });
          try {
            await deps.cache.releaseLock(service, lockToken);
          } catch {
            deps.logger.event('discovery.cache_unavailable', { service, stage: 'release_lock' });
          }
        }
      }
    }

    return stale;
  }

  // COLD: take the lock and probe, or wait for whoever holds it.
  const lockToken = newLockToken();
  const deadline = deps.now() + deps.timings.lockWaitMs;
  for (let attempt = 0; ; attempt += 1) {
    let acquired: boolean;
    try {
      acquired = await deps.cache.tryAcquireLock(service, lockToken, deps.timings.lockTtlMs);
    } catch {
      deps.logger.event('discovery.cache_unavailable', { service, stage: 'lock' });
      return degraded(service, 'cache-unavailable', null);
    }
    if (acquired) {
      try {
        return await probeAndStore(deps, service, token, lockToken);
      } catch {
        deps.logger.event('discovery.cache_unavailable', { service, stage: 'probe_store' });
        return degraded(service, 'cache-unavailable', null);
      }
    }

    if (deps.now() >= deadline) {
      // Invariant 2: a waiter NEVER probes as a fallback. One final read first, in case the
      // winner wrote just as the deadline expired — otherwise we report a false outage.
      // safeLog, not deps.logger.event directly: this call has no enclosing try at all, so a
      // broken injected logger would otherwise reject resolveService's promise here.
      safeLog(deps, 'discovery.lock_timeout', { service });
      const last = await readRecord(deps, service).catch(() => null);
      if (last !== null) return toState(service, last) ?? degraded(service, 'timeout', last);
      return degraded(service, 'timeout', null);
    }

    await sleep(backoff(attempt));
    const reread = await readRecord(deps, service).catch(() => null);
    if (reread !== null && isFresh(reread, deps.now(), deps.timings)) {
      return toState(service, reread) ?? degraded(service, 'network', reread);
    }
  }
}
