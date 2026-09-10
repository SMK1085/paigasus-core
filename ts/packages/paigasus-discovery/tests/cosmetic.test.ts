// SPDX-License-Identifier: Apache-2.0
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { createMemoryDescriptorCache } from '../src/adapters/memory-cache.js';
import { createDiscovery } from '../src/server.js';
import * as server from '../src/server.js';
import type { ProbeOutcome } from '../src/probe.js';

describe('AC4: capability gating is cosmetic', () => {
  it('exposes no enforcement hook — only data', () => {
    // If the package could BLOCK a call, "the server remains authoritative" would be false.
    // hasCapability returns a boolean and nothing here wraps, guards, or refuses a request.
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
    // Nothing above returns a blocker, a throw, or a wrapped client. The caller is free to
    // proceed and the server answers authoritatively.
  });

  it('the README states the cosmetic rule and the not-a-security-boundary rule', () => {
    const readme = readFileSync(fileURLToPath(new URL('../README.md', import.meta.url)), 'utf8');
    expect(readme).toMatch(/cosmetic/i);
    expect(readme).toMatch(/not a security boundary/i);
  });
});
