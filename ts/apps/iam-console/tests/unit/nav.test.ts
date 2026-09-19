// SPDX-License-Identifier: Apache-2.0
//
// buildNavEntries (spec § 5.4, § 6.6, § 9.2; SMA-629 spec § 6.1): every IAM state, with and without
// `iam.audit` and `iam.deadletters`, with each mayI question allowed and denied; the gateway absent,
// degraded and available; and every entry's state went through navStateOf().
import { describe, expect, it, vi } from 'vitest';
import type { NavEntry } from '@paigasus/app-shell';
import type { ServiceState } from '@paigasus/discovery/types';

const navStateOf = vi.hoisted(() => vi.fn());
vi.mock('@paigasus/app-shell', async (importOriginal) => {
  const actual = await importOriginal<typeof import('@paigasus/app-shell')>();
  navStateOf.mockImplementation(actual.navStateOf);
  return { ...actual, navStateOf };
});

const { buildNavEntries } = await import('../../lib/nav');

const ZONES = { iam: '/iam', gateway: '/gateway' };
const iamUp = (capabilities: string[]): ServiceState => ({ state: 'available', service: 'iam', descriptor: { service: 'iam', version: '1', capabilities }, capabilities });
const iamDown = (capabilities: string[]): ServiceState => ({ state: 'degraded', service: 'iam', reason: 'timeout', descriptor: { service: 'iam', version: '1', capabilities }, capabilities });
const IAM_DOWN = iamDown(['iam.audit']);
const IAM_ABSENT: ServiceState = { state: 'absent', service: 'iam' };
const GATEWAY_ABSENT: ServiceState = { state: 'absent', service: 'gateway' };
const GATEWAY_DOWN: ServiceState = { state: 'degraded', service: 'gateway', reason: 'network', descriptor: null, capabilities: [] };
const GATEWAY_UP: ServiceState = { state: 'available', service: 'gateway', descriptor: { service: 'gateway', version: '1', capabilities: [] }, capabilities: [] };
const DEGRADED: NavEntry['state'] = { state: 'degraded', service: 'iam', reason: 'timeout' };

const byLabel = (entries: ReturnType<typeof buildNavEntries>) => Object.fromEntries(entries.map((entry) => [entry.label, entry]));

describe('buildNavEntries', () => {
  it('lists Organizations, Audit, Dead letters and Gateway, in that order, with full-path hrefs when everything is available', () => {
    const entries = buildNavEntries({ iam: iamUp(['iam.audit', 'iam.deadletters']), gateway: GATEWAY_UP, zones: ZONES, auditAllowed: true, deadLettersAllowed: true });
    expect(entries.map((e) => [e.label, e.zone, e.href, e.state.state])).toEqual([
      ['Organizations', 'iam', '/iam/orgs', 'available'],
      ['Audit', 'iam', '/iam/audit', 'available'],
      ['Dead letters', 'iam', '/iam/dead-letters', 'available'],
      ['Gateway', 'gateway', '/gateway/', 'available'],
    ]);
  });

  it('makes Audit absent when IAM does not report iam.audit', () => {
    const entries = byLabel(buildNavEntries({ iam: iamUp([]), gateway: GATEWAY_UP, zones: ZONES, auditAllowed: true, deadLettersAllowed: false }));
    expect(entries['Audit']?.state).toEqual({ state: 'absent' });
    expect(entries['Organizations']?.state).toEqual({ state: 'available' });
  });

  it('omits Audit when mayI says no', () => {
    const entries = byLabel(buildNavEntries({ iam: iamUp(['iam.audit']), gateway: GATEWAY_UP, zones: ZONES, auditAllowed: false, deadLettersAllowed: false }));
    expect(entries['Audit']).toBeUndefined();
  });

  it('disables the IAM entries with a reason when IAM is degraded', () => {
    const entries = byLabel(buildNavEntries({ iam: IAM_DOWN, gateway: GATEWAY_UP, zones: ZONES, auditAllowed: true, deadLettersAllowed: true }));
    expect(entries['Organizations']?.state).toEqual(DEGRADED);
    expect(entries['Audit']?.state).toEqual(DEGRADED);
    expect(entries['Dead letters']?.state).toEqual(DEGRADED);
  });

  it('makes the IAM entries absent when IAM is absent', () => {
    const entries = byLabel(buildNavEntries({ iam: IAM_ABSENT, gateway: GATEWAY_UP, zones: ZONES, auditAllowed: true, deadLettersAllowed: true }));
    expect(entries['Organizations']?.state).toEqual({ state: 'absent' });
    expect(entries['Audit']?.state).toEqual({ state: 'absent' });
    expect(entries['Dead letters']?.state).toEqual({ state: 'absent' });
  });

  it('gives the Gateway entry the gateway’s own state: absent, or degraded with a reason', () => {
    expect(byLabel(buildNavEntries({ iam: iamUp([]), gateway: GATEWAY_ABSENT, zones: ZONES, auditAllowed: false, deadLettersAllowed: false }))['Gateway']?.state).toEqual({ state: 'absent' });
    expect(byLabel(buildNavEntries({ iam: iamUp([]), gateway: GATEWAY_DOWN, zones: ZONES, auditAllowed: false, deadLettersAllowed: false }))['Gateway']?.state).toEqual({
      state: 'degraded',
      service: 'gateway',
      reason: 'network',
    });
  });

  it('leaves out the Gateway entry when gateway is not a zone', () => {
    const entries = byLabel(buildNavEntries({ iam: iamUp([]), gateway: GATEWAY_UP, zones: { iam: '/iam' }, auditAllowed: false, deadLettersAllowed: false }));
    expect(entries['Gateway']).toBeUndefined();
  });

  it('takes EVERY entry’s state from navStateOf()', () => {
    navStateOf.mockClear();
    const entries = buildNavEntries({ iam: iamUp(['iam.audit', 'iam.deadletters']), gateway: GATEWAY_UP, zones: ZONES, auditAllowed: true, deadLettersAllowed: true });
    expect(navStateOf).toHaveBeenCalledTimes(entries.length);
    expect(navStateOf.mock.results.map((r) => r.value as unknown)).toEqual(entries.map((e) => e.state));
  });
});

// SMA-629 spec § 6.1, § 7.2, AC 1 and AC 2. The matrix: the mayI answer × the key present or absent ×
// IAM up, degraded or absent. `undefined` means no entry at all.
describe('the Dead letters entry', () => {
  it.each<[string, ServiceState, boolean, NavEntry['state'] | undefined]>([
    ['allowed, IAM up with the key', iamUp(['iam.deadletters']), true, { state: 'available' }],
    ['allowed, IAM up without the key (an older IAM, AC 2)', iamUp(['iam.audit']), true, { state: 'absent' }],
    ['allowed, IAM degraded with the key', iamDown(['iam.deadletters']), true, DEGRADED],
    ['allowed, IAM degraded without the key (the § 8 exception)', iamDown([]), true, DEGRADED],
    ['allowed, IAM absent', IAM_ABSENT, true, { state: 'absent' }],
    ['denied, IAM up with the key', iamUp(['iam.deadletters']), false, undefined],
    ['denied, IAM up without the key', iamUp([]), false, undefined],
    ['denied, IAM degraded', iamDown(['iam.deadletters']), false, undefined],
    ['denied, IAM absent', IAM_ABSENT, false, undefined],
  ])('%s', (_label, iam, allowed, expected) => {
    const entry = byLabel(buildNavEntries({ iam, gateway: GATEWAY_UP, zones: ZONES, auditAllowed: true, deadLettersAllowed: allowed }))['Dead letters'];
    expect(entry?.state).toEqual(expected);
    if (expected !== undefined) expect(entry?.href).toBe('/iam/dead-letters');
  });
});
