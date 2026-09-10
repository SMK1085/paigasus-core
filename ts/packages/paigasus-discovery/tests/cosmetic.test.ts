// SPDX-License-Identifier: Apache-2.0
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { createMemoryDescriptorCache } from '../src/adapters/memory-cache.js';
import { createDiscovery } from '../src/server.js';
import * as server from '../src/server.js';
import type { ProbeOutcome } from '../src/probe.js';

// STRICT EQUALITY, deliberately, the same pattern `ci/affected-graph/run.sh` and
// @paigasus/sdk's tests/index-barrel.test.ts use for the same reason: any new export — named
// `veto`, `deny`, `block`, or anything else — must red this test until a human adds it here on
// purpose. An enforcement hook added to this module would silently make capability gating
// authoritative, which ADR-0020 says it must never be.
const EXPECTED_SERVER_EXPORTS = [
  'DEFAULT_TIMINGS',
  'createDiscovery',
  'createMemoryDescriptorCache',
  'createRedisDescriptorCache',
  'discoveryEnvShape',
  'noopLogger',
  'parseServiceMap',
  'timingsFromEnv',
];

// STRICT EQUALITY, same reason: any method added to the returned handle — the actual place a
// blocking call would be enforced — must red this test until a human adds it here on purpose.
const EXPECTED_HANDLE_KEYS = ['getServiceState', 'hasCapability'];

describe('AC4: capability gating is cosmetic', () => {
  it('exports exactly this set — no enforcement hook can be added here silently', () => {
    expect(Object.keys(server).sort()).toEqual(EXPECTED_SERVER_EXPORTS);
  });

  it('the discovery handle exposes exactly getServiceState and hasCapability', () => {
    const discovery = createDiscovery({
      services: { iam: 'http://iam:8080' },
      cache: createMemoryDescriptorCache(),
    });
    expect(Object.keys(discovery).sort()).toEqual(EXPECTED_HANDLE_KEYS);
  });

  it('exposes no enforcement hook — only data (cheap extra signal, not the guarantee)', () => {
    // This name scan is a CHEAP EXTRA SIGNAL, not the guarantee: it only catches a hook named
    // one of these five words, and a hook named anything else (e.g. `veto`) sails through it
    // undetected — MEASURED. The strict-equality pins above are what actually provide the
    // guarantee, because ANY new export or handle method reds them regardless of its name.
    const names = Object.keys(server);
    for (const forbidden of ['enforce', 'require', 'guard', 'assert', 'authorize']) {
      expect(names.filter((n) => n.toLowerCase().includes(forbidden))).toEqual([]);
    }
  });

  it('a 404 from an older service degrades rather than throwing', () => {
    // ADR-0020 A2: a disabled capability's routes are unmounted (404), indistinguishable from a
    // build predating the feature. Neither may crash the console.
    const discovery = createDiscovery({
      services: { iam: 'http://iam:8080' },
      cache: createMemoryDescriptorCache(),
      probe: (): Promise<ProbeOutcome> => Promise.resolve({ ok: false, reason: 'not-implemented' }),
    });
    return expect(discovery.getServiceState('iam', 'tok')).resolves.toMatchObject({
      state: 'degraded',
      reason: 'not-implemented',
    });
  });

  it('hasCapability being false never prevents the call being made', async () => {
    const discovery = createDiscovery({
      services: { iam: 'http://iam:8080' },
      cache: createMemoryDescriptorCache(),
      probe: (): Promise<ProbeOutcome> =>
        Promise.resolve({
          ok: true,
          descriptor: { service: 'iam', version: '1.0.0', capabilities: [] },
        }),
    });
    expect(await discovery.hasCapability('iam.audit', 'tok')).toBe(false);
    // This demonstrates only that THIS call returns a plain boolean and nothing more — no
    // throw, no wrapped client, no side effect that could stop a caller who ignores the result.
    // It does not by itself prove no enforcement hook exists anywhere in the package; the
    // strict-equality export and handle-surface pins above provide that guarantee.
  });

  it('the README states the cosmetic rule and the not-a-security-boundary rule', () => {
    const readme = readFileSync(fileURLToPath(new URL('../README.md', import.meta.url)), 'utf8');
    expect(readme).toMatch(/cosmetic/i);
    expect(readme).toMatch(/not a security boundary/i);
  });
});
