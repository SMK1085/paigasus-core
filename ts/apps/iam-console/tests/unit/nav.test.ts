// SPDX-License-Identifier: Apache-2.0
//
// buildNavEntries (spec § 5.4, § 6.6, § 9.2): every IAM state, with and without `iam.audit`, with
// the audit question allowed and denied; the gateway absent, degraded and available; and every
// entry's state went through navStateOf().
import { describe, expect, it, vi } from 'vitest';
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
const IAM_DOWN: ServiceState = { state: 'degraded', service: 'iam', reason: 'timeout', descriptor: { service: 'iam', version: '1', capabilities: ['iam.audit'] }, capabilities: ['iam.audit'] };
const IAM_ABSENT: ServiceState = { state: 'absent', service: 'iam' };
const GATEWAY_ABSENT: ServiceState = { state: 'absent', service: 'gateway' };
const GATEWAY_DOWN: ServiceState = { state: 'degraded', service: 'gateway', reason: 'network', descriptor: null, capabilities: [] };
const GATEWAY_UP: ServiceState = { state: 'available', service: 'gateway', descriptor: { service: 'gateway', version: '1', capabilities: [] }, capabilities: [] };

const byLabel = (entries: ReturnType<typeof buildNavEntries>) => Object.fromEntries(entries.map((entry) => [entry.label, entry]));

describe('buildNavEntries', () => {
  it('lists Organizations, Audit and Gateway with full-path hrefs when everything is available', () => {
    const entries = buildNavEntries({ iam: iamUp(['iam.audit']), gateway: GATEWAY_UP, zones: ZONES, auditAllowed: true });
    expect(entries.map((e) => [e.label, e.zone, e.href, e.state.state])).toEqual([
      ['Organizations', 'iam', '/iam/orgs', 'available'],
      ['Audit', 'iam', '/iam/audit', 'available'],
      ['Gateway', 'gateway', '/gateway/', 'available'],
    ]);
  });

  it('makes Audit absent when IAM does not report iam.audit', () => {
    const entries = byLabel(buildNavEntries({ iam: iamUp([]), gateway: GATEWAY_UP, zones: ZONES, auditAllowed: true }));
    expect(entries['Audit']?.state).toEqual({ state: 'absent' });
    expect(entries['Organizations']?.state).toEqual({ state: 'available' });
  });

  it('omits Audit when mayI says no', () => {
    const entries = byLabel(buildNavEntries({ iam: iamUp(['iam.audit']), gateway: GATEWAY_UP, zones: ZONES, auditAllowed: false }));
    expect(entries['Audit']).toBeUndefined();
  });

  it('disables the IAM entries with a reason when IAM is degraded', () => {
    const entries = byLabel(buildNavEntries({ iam: IAM_DOWN, gateway: GATEWAY_UP, zones: ZONES, auditAllowed: true }));
    expect(entries['Organizations']?.state).toEqual({ state: 'degraded', service: 'iam', reason: 'timeout' });
    expect(entries['Audit']?.state).toEqual({ state: 'degraded', service: 'iam', reason: 'timeout' });
  });

  it('makes the IAM entries absent when IAM is absent', () => {
    const entries = byLabel(buildNavEntries({ iam: IAM_ABSENT, gateway: GATEWAY_UP, zones: ZONES, auditAllowed: true }));
    expect(entries['Organizations']?.state).toEqual({ state: 'absent' });
    expect(entries['Audit']?.state).toEqual({ state: 'absent' });
  });

  it('gives the Gateway entry the gateway’s own state: absent, or degraded with a reason', () => {
    expect(byLabel(buildNavEntries({ iam: iamUp([]), gateway: GATEWAY_ABSENT, zones: ZONES, auditAllowed: false }))['Gateway']?.state).toEqual({ state: 'absent' });
    expect(byLabel(buildNavEntries({ iam: iamUp([]), gateway: GATEWAY_DOWN, zones: ZONES, auditAllowed: false }))['Gateway']?.state).toEqual({
      state: 'degraded',
      service: 'gateway',
      reason: 'network',
    });
  });

  it('leaves out the Gateway entry when gateway is not a zone', () => {
    const entries = byLabel(buildNavEntries({ iam: iamUp([]), gateway: GATEWAY_UP, zones: { iam: '/iam' }, auditAllowed: false }));
    expect(entries['Gateway']).toBeUndefined();
  });

  it('takes EVERY entry’s state from navStateOf()', () => {
    navStateOf.mockClear();
    const entries = buildNavEntries({ iam: iamUp(['iam.audit']), gateway: GATEWAY_UP, zones: ZONES, auditAllowed: true });
    expect(navStateOf).toHaveBeenCalledTimes(entries.length);
    expect(navStateOf.mock.results.map((r) => r.value as unknown)).toEqual(entries.map((e) => e.state));
  });
});
