// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it, vi } from 'vitest';
import { createMemoryDescriptorCache } from '../src/adapters/memory-cache.js';
import { createDiscovery, type CreateDiscoveryDeps } from '../src/server.js';
import type { ProbeOutcome } from '../src/probe.js';
import type { CapabilityKey } from '../src/types.js';

const descriptor = { service: 'iam', version: '1.0.0', capabilities: ['iam.audit'] };

function make(over: Partial<CreateDiscoveryDeps> = {}) {
  return createDiscovery({
    services: { iam: 'http://iam:8080' },
    cache: createMemoryDescriptorCache(),
    probe: (): Promise<ProbeOutcome> => Promise.resolve({ ok: true, descriptor }),
    ...over,
  });
}

describe('createDiscovery', () => {
  it('reports absent for a service missing from the map, without touching the cache', async () => {
    const cache = createMemoryDescriptorCache();
    const get = vi.spyOn(cache, 'get');
    const d = make({ cache });
    expect(await d.getServiceState('gateway', 'tok')).toEqual({ state: 'absent', service: 'gateway' });
    expect(get).not.toHaveBeenCalled();
  });

  it('reports absent for an inherited Object.prototype key, never available/degraded (F4)', async () => {
    // THE REGRESSION THIS TEST EXISTS FOR. `deps.services` is typed as a plain
    // `Readonly<Record<string, string>>`, so a caller may pass an object literal rather than
    // `parseServiceMap`'s null-prototype result. `deps.services['constructor'] === undefined` is
    // FALSE — it resolves to the inherited `Function` — which used to skip the `absent` branch
    // and hand a non-URL to `probeService`, reporting `degraded` for a service that was never
    // configured at all.
    const cache = createMemoryDescriptorCache();
    const get = vi.spyOn(cache, 'get');
    const d = make({ cache });
    for (const service of ['constructor', '__proto__']) {
      expect(await d.getServiceState(service, 'tok')).toEqual({ state: 'absent', service });
    }
    expect(get).not.toHaveBeenCalled();
  });

  it('reports available for a configured, answering service', async () => {
    expect(await make().getServiceState('iam', 'tok')).toMatchObject({ state: 'available' });
  });

  it('memoizes within one handle so N calls cost one probe', async () => {
    const probe = vi.fn((): Promise<ProbeOutcome> => Promise.resolve({ ok: true, descriptor }));
    const d = make({ probe });
    await Promise.all([d.getServiceState('iam', 'tok'), d.getServiceState('iam', 'tok'), d.getServiceState('iam', 'tok')]);
    expect(probe).toHaveBeenCalledTimes(1);
  });

  it('rejects a timing order that would make a cold loser report a false outage', () => {
    // A loser whose deadline expires before the winner can finish probing AND writing reports a
    // healthy service as down on the first render after every deploy.
    expect(() => make({ timings: { probeTimeoutMs: 3_000, lockWaitMs: 2_000 } })).toThrow(/lockWaitMs/);
    expect(() => make({ timings: { lockWaitMs: 6_000, lockTtlMs: 5_000 } })).toThrow(/lockTtlMs/);
    expect(() => make({ timings: { negativeMs: 90_000, freshMs: 60_000 } })).toThrow(/freshMs/);
    expect(() => make({ timings: { freshMs: 700_000, staleMs: 600_000 } })).toThrow(/staleMs/);
  });
});

describe('hasCapability', () => {
  it('is true only when available and the key is present', async () => {
    const d = make();
    expect(await d.hasCapability('iam.audit', 'tok')).toBe(true);
    expect(await d.hasCapability('iam.apikeys', 'tok')).toBe(false);
  });

  it('is false for a degraded service even when the stale descriptor has the key', async () => {
    // The feature must not be invoked. <Capability> still RENDERS it disabled — the two
    // deliberately disagree, and the README says so.
    const d = make({ probe: (): Promise<ProbeOutcome> => Promise.resolve({ ok: false, reason: 'network' }) });
    expect(await d.hasCapability('iam.audit', 'tok')).toBe(false);
  });

  it('is false for an absent service', async () => {
    expect(await make().hasCapability('gateway.chat.stream', 'tok')).toBe(false);
  });

  it('is false for an unknown key, without throwing', async () => {
    // Decision 6: unknown key -> ignore. A key outside the closed union needs an explicit cast,
    // which is the point: this can only reach the runtime from an older or newer build, never
    // from a typo in our own source.
    expect(await make().hasCapability('iam.future' as CapabilityKey, 'tok')).toBe(false);
  });
});
