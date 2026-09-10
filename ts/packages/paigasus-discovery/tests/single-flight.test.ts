// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it, vi } from 'vitest';
import { createMemoryDescriptorCache } from '../src/adapters/memory-cache.js';
import { noopLogger } from '../src/adapters/noop-logger.js';
import { DEFAULT_TIMINGS, RECORD_VERSION, type CacheRecord } from '../src/core/record.js';
import { resolveService, type ResolveDeps } from '../src/core/single-flight.js';
import type { ProbeOutcome } from '../src/probe.js';
import type { DescriptorCache } from '../src/ports/cache.js';
import type { ServiceDescriptor } from '../src/types.js';

const descriptor: ServiceDescriptor = { service: 'iam', version: '1.0.0', capabilities: ['iam.audit'] };

function deps(over: Partial<ResolveDeps> = {}): ResolveDeps {
  return {
    cache: createMemoryDescriptorCache(),
    logger: noopLogger,
    timings: DEFAULT_TIMINGS,
    now: () => Date.now(),
    probe: (): Promise<ProbeOutcome> => Promise.resolve({ ok: true, descriptor }),
    ...over,
  };
}

function storedRecord(over: Partial<CacheRecord> = {}): CacheRecord {
  return {
    version: RECORD_VERSION,
    rev: 1,
    descriptor,
    descriptorAt: 0,
    outcome: 'ok',
    outcomeAt: 0,
    reason: null,
    ...over,
  };
}

describe('cold resolution', () => {
  it('probes and returns available', async () => {
    const state = await resolveService(deps(), 'iam', 'tok');
    expect(state).toMatchObject({ state: 'available', service: 'iam', capabilities: ['iam.audit'] });
  });

  it('caches a successful probe', async () => {
    const cache = createMemoryDescriptorCache();
    const probe = vi.fn((): Promise<ProbeOutcome> => Promise.resolve({ ok: true, descriptor }));
    await resolveService(deps({ cache, probe }), 'iam', 'tok');
    await resolveService(deps({ cache, probe }), 'iam', 'tok');
    expect(probe).toHaveBeenCalledTimes(1);
  });

  it('returns degraded and caches the failure', async () => {
    const cache = createMemoryDescriptorCache();
    const probe = vi.fn((): Promise<ProbeOutcome> => Promise.resolve({ ok: false, reason: 'network' }));
    const state = await resolveService(deps({ cache, probe }), 'iam', 'tok');
    expect(state).toEqual({
      state: 'degraded',
      service: 'iam',
      reason: 'network',
      descriptor: null,
      capabilities: [],
    });
    expect(await cache.get('iam')).toMatchObject({ outcome: 'fail', reason: 'network' });
  });

  it('NEVER caches a 401/403 outcome', async () => {
    // The descriptor is caller-independent; the auth outcome is not. Caching it would let one
    // user's expired cookie disable navigation for every user in the deployment for the
    // negative TTL — self-perpetuating on a low-traffic deployment.
    const cache = createMemoryDescriptorCache();
    const probe = (): Promise<ProbeOutcome> => Promise.resolve({ ok: false, reason: 'unauthorized' });
    const state = await resolveService(deps({ cache, probe }), 'iam', 'tok');
    expect(state).toMatchObject({ state: 'degraded', reason: 'unauthorized' });
    expect(await cache.get('iam')).toBeNull();
  });
});

describe('fresh and stale', () => {
  it('serves a fresh record without probing', async () => {
    const cache = createMemoryDescriptorCache();
    await cache.set('iam', storedRecord({ outcomeAt: 0 }), DEFAULT_TIMINGS.staleMs, null);
    const probe = vi.fn((): Promise<ProbeOutcome> => Promise.resolve({ ok: true, descriptor }));
    const state = await resolveService(deps({ cache, probe, now: () => 30_000 }), 'iam', 'tok');
    expect(state).toMatchObject({ state: 'available' });
    expect(probe).not.toHaveBeenCalled();
  });

  it('serves a fresh FAILURE without probing, for the negative TTL only', async () => {
    const cache = createMemoryDescriptorCache();
    await cache.set(
      'iam',
      storedRecord({ outcome: 'fail', reason: 'network', outcomeAt: 0 }),
      DEFAULT_TIMINGS.staleMs,
      null,
    );
    const probe = vi.fn((): Promise<ProbeOutcome> => Promise.resolve({ ok: true, descriptor }));
    expect(await resolveService(deps({ cache, probe, now: () => 5_000 }), 'iam', 'tok')).toMatchObject({
      state: 'degraded',
    });
    expect(probe).not.toHaveBeenCalled();
  });

  // AC2. NO WALL-CLOCK ASSERTION: a clock threshold passes for an implementation that awaits a
  // 1ms probe and fails on a loaded CI runner. A probe that NEVER settles makes an
  // await-the-probe implementation hang and fail deterministically at the suite timeout.
  it('AC2: a stale record returns immediately even when the probe never settles', async () => {
    const cache = createMemoryDescriptorCache();
    await cache.set('iam', storedRecord({ outcomeAt: 0 }), DEFAULT_TIMINGS.staleMs, null);
    let probeCalled = false;
    const probe = (): Promise<ProbeOutcome> => {
      probeCalled = true;
      return new Promise<ProbeOutcome>(() => {
        /* never settles */
      });
    };
    const state = await resolveService(deps({ cache, probe, now: () => 120_000 }), 'iam', 'tok');
    expect(state).toMatchObject({ state: 'available', capabilities: ['iam.audit'] });
    // The descriptor came from the cache, by identity, not from a fresh probe.
    expect((state as { descriptor: ServiceDescriptor }).descriptor).toEqual(descriptor);
    // ...and revalidation WAS attempted, so this is stale-while-revalidate and not stale-only.
    expect(probeCalled).toBe(true);
  });

  it('hands the background revalidation to waitUntil when given one', async () => {
    const cache = createMemoryDescriptorCache();
    await cache.set('iam', storedRecord({ outcomeAt: 0 }), DEFAULT_TIMINGS.staleMs, null);
    const handed: Promise<unknown>[] = [];
    await resolveService(
      deps({ cache, now: () => 120_000, waitUntil: (p) => handed.push(p) }),
      'iam',
      'tok',
    );
    expect(handed).toHaveLength(1);
    await handed[0];
    expect((await cache.get('iam'))?.rev).toBe(2);
  });
});

describe('AC3: single-flight', () => {
  it('a fast probe serves many concurrent cold callers with exactly one probe', async () => {
    const cache = createMemoryDescriptorCache();
    let calls = 0;
    const probe = async (): Promise<ProbeOutcome> => {
      calls += 1;
      await new Promise((r) => setTimeout(r, 5));
      return { ok: true, descriptor };
    };
    const d = deps({ cache, probe });
    const states = await Promise.all(
      Array.from({ length: 12 }, () => resolveService(d, 'iam', 'tok')),
    );
    expect(calls).toBe(1);
    for (const s of states) expect(s).toMatchObject({ state: 'available' });
  });

  // THE CASE THAT ACTUALLY OBSERVES INVARIANT 2. `probeTimeoutMs` (400) is set well ABOVE
  // `lockWaitMs` (60) so the winner's own probe is still in flight when every loser's deadline
  // passes — with the timings the other way around, the winner times out and writes a `fail`
  // record well before any loser's deadline, and every loser then picks that up from its in-loop
  // reread. That makes the deadline branch (invariant 2's fallback ban) dead code for the
  // duration of the test, and a mutant that adds a fallback probe in the deadline branch would
  // pass anyway — so the `discovery.lock_timeout` assertion below is not optional: it is what
  // proves the deadline branch actually ran, rather than merely that every state reported
  // 'timeout' for some other reason.
  it('a never-settling probe still yields exactly one probe, and losers report timeout', async () => {
    const cache = createMemoryDescriptorCache();
    const events: string[] = [];
    const logger = { event: (n: string) => events.push(n) };
    let calls = 0;
    const probe = (): Promise<ProbeOutcome> => {
      calls += 1;
      return new Promise<ProbeOutcome>(() => {
        /* never settles */
      });
    };
    const d = deps({
      cache,
      probe,
      logger,
      timings: { ...DEFAULT_TIMINGS, lockWaitMs: 60, probeTimeoutMs: 400, lockTtlMs: 5_000 },
    });
    const states = await Promise.all(
      Array.from({ length: 8 }, () => resolveService(d, 'iam', 'tok')),
    );
    expect(calls).toBe(1);
    const losers = states.filter((s) => s.state === 'degraded');
    expect(losers.length).toBeGreaterThanOrEqual(7);
    for (const s of losers) expect(s).toMatchObject({ reason: 'timeout' });
    // Proves the deadline branch was actually taken, not merely that the outcome happened to
    // read 'timeout' some other way. Without this a vacuous version of this test — one where the
    // winner settles before any loser's deadline — still reports every loser as 'timeout' by
    // reading a `fail` record the winner itself wrote, and a fallback-probing mutant slips through.
    expect(events.filter((e) => e === 'discovery.lock_timeout').length).toBeGreaterThanOrEqual(7);
  });

  // This proves a loser can pick up the lock-holder's write via its in-loop backoff-reread and
  // return 'available' without ever reaching its own deadline — NOT the final-read-at-deadline
  // path (see the next test for that): the write here lands well before any loser's deadline, so
  // the reread inside the retry loop is what catches it.
  it('a loser picks up the winner\'s write via its in-loop reread and returns available', async () => {
    const cache = createMemoryDescriptorCache();
    await cache.tryAcquireLock('iam', 'someone-else', 5_000);
    const d = deps({ cache, timings: { ...DEFAULT_TIMINGS, lockWaitMs: 40 } });
    const pending = resolveService(d, 'iam', 'tok');
    await cache.set('iam', storedRecord({ outcomeAt: Date.now() }), DEFAULT_TIMINGS.staleMs, null);
    expect(await pending).toMatchObject({ state: 'available' });
  });

  // Distinguishes this from the previous test: there the write lands before any reread ever
  // happens, so the in-loop reread already catches it and the deadline branch is never reached.
  // Here the write is engineered — via a `get` wrapper and a hand-driven virtual clock — to land
  // strictly AFTER the one reread that finds nothing and BEFORE the deadline's own final read, so
  // only invariant 2's final-read-before-degrading path can find it.
  it('a loser catches a just-landed write from the deadline\'s final read, not an in-loop reread', async () => {
    const base = createMemoryDescriptorCache();
    await base.tryAcquireLock('iam', 'someone-else', 5_000);
    let getCalls = 0;
    let clock = 0;
    const cache: DescriptorCache = {
      ...base,
      get: async (svc: string) => {
        const result = await base.get(svc);
        getCalls += 1;
        if (getCalls === 2) {
          // The winner's write lands in the gap right after this reread returned nothing.
          await base.set(svc, storedRecord({ outcomeAt: clock }), DEFAULT_TIMINGS.staleMs, null);
        }
        return result;
      },
    };
    const events: string[] = [];
    const logger = { event: (n: string) => events.push(n) };
    const d = deps({
      cache,
      logger,
      now: () => clock,
      sleep: () => {
        clock += 1000; // Jump straight past the deadline after the one reread.
        return Promise.resolve();
      },
      timings: { ...DEFAULT_TIMINGS, lockWaitMs: 100 },
    });
    const state = await resolveService(d, 'iam', 'tok');
    expect(state).toMatchObject({ state: 'available' });
    // Exactly 3 `get` calls: the initial read, the one reread (misses it), the final read at the
    // deadline (catches it). A 4th call, or a 2nd, would mean the write was caught somewhere else.
    expect(getCalls).toBe(3);
    expect(events).toContain('discovery.lock_timeout');
  });
});

describe('failure handling', () => {
  it('degrades with cache-unavailable when the cache throws, never propagating', async () => {
    const broken: DescriptorCache = {
      get: () => Promise.reject(new Error('redis down')),
      set: () => Promise.reject(new Error('redis down')),
      delete: () => Promise.resolve(),
      tryAcquireLock: () => Promise.reject(new Error('redis down')),
      releaseLock: () => Promise.resolve(),
      close: () => Promise.resolve(),
    };
    const state = await resolveService(deps({ cache: broken }), 'iam', 'tok');
    expect(state).toEqual({
      state: 'degraded',
      service: 'iam',
      reason: 'cache-unavailable',
      descriptor: null,
      capabilities: [],
    });
  });

  it('discards a write whose fence was lost rather than clobbering a newer record', async () => {
    // Invariant 5. A background probe with no wall-clock bound can resolve long after a newer
    // probe has already written; last-writer-wins would let an OLD failure mask a healthy
    // service until the hard TTL.
    const cache = createMemoryDescriptorCache();
    const events: string[] = [];
    const logger = { event: (n: string) => events.push(n) };
    await cache.set('iam', storedRecord({ rev: 7, outcomeAt: 0 }), DEFAULT_TIMINGS.staleMs, null);

    let release!: (o: ProbeOutcome) => void;
    const probe = (): Promise<ProbeOutcome> =>
      new Promise<ProbeOutcome>((resolve) => {
        release = resolve;
      });

    const d = deps({ cache, probe, logger, now: () => 120_000 });
    const handed: Promise<unknown>[] = [];
    await resolveService({ ...d, waitUntil: (p) => handed.push(p) }, 'iam', 'tok');

    // A newer writer lands while the slow probe is still in flight.
    await cache.set('iam', storedRecord({ rev: 8, outcomeAt: 119_000 }), DEFAULT_TIMINGS.staleMs, 7);
    release({ ok: false, reason: 'network' });
    await handed[0];

    const final = await cache.get('iam');
    expect(final?.rev).toBe(8);
    expect(final?.outcome).toBe('ok');
    expect(events).toContain('discovery.write_fenced');
  });

  it('deletes and re-probes a record that is internally impossible', async () => {
    // A `version: 99` record never reaches this module at all: the memory adapter's OWN
    // `parseRecord` guard rejects and deletes it inside `get`, so `readRecord` sees a plain
    // `null` and never exercises its own corrupt-record branch. Plant something that PARSES
    // (version and shape are correct) but is internally impossible instead — 'ok' with no
    // descriptor — which is exactly what `readRecord`'s own `toState(...) === null` check exists
    // to catch.
    const cache = createMemoryDescriptorCache();
    const events: string[] = [];
    const logger = { event: (n: string) => events.push(n) };
    await cache.writeRawForTest?.(
      'iam',
      JSON.stringify({
        version: RECORD_VERSION,
        rev: 1,
        descriptor: null,
        descriptorAt: 0,
        outcome: 'ok',
        outcomeAt: 0,
        reason: null,
      }),
    );
    const probe = vi.fn((): Promise<ProbeOutcome> => Promise.resolve({ ok: true, descriptor }));
    const state = await resolveService(deps({ cache, probe, logger }), 'iam', 'tok');
    expect(state).toMatchObject({ state: 'available' });
    expect(probe).toHaveBeenCalledTimes(1);
    expect(events).toContain('discovery.record_discarded');
  });
});
