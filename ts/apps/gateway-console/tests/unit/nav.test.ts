// SPDX-License-Identifier: Apache-2.0
//
// buildNavEntries (spec § 10.2): every zone-map and service-state combination for the gateway's
// two entries — Overview (always present, self zone) and IAM (present only when the zone map
// names an `iam` zone). Every entry's state went through navStateOf().
import { describe, expect, it, vi } from 'vitest';
import type { ServiceState } from '@paigasus/discovery/types';

const navStateOf = vi.hoisted(() => vi.fn());
vi.mock('@paigasus/app-shell', async (importOriginal) => {
  const actual = await importOriginal<typeof import('@paigasus/app-shell')>();
  navStateOf.mockImplementation(actual.navStateOf);
  return { ...actual, navStateOf };
});

const { GATEWAY_BASE_PATH, buildNavEntries } = await import('../../lib/nav');

const ZONES = { iam: '/iam', gateway: '/gateway' };

const available = (service: string, capabilities: string[] = []): ServiceState => ({ state: 'available', service, descriptor: { service, version: '1.0.0', capabilities }, capabilities });
const degraded = (service: string): ServiceState => ({ state: 'degraded', service, reason: 'timeout', descriptor: null, capabilities: [] });
const absent = (service: string): ServiceState => ({ state: 'absent', service });

const byLabel = (entries: ReturnType<typeof buildNavEntries>) => Object.fromEntries(entries.map((entry) => [entry.label, entry]));

describe('buildNavEntries', () => {
  it('is /gateway', () => {
    expect(GATEWAY_BASE_PATH).toBe('/gateway');
  });

  it.each([
    ['available', available('gateway')],
    ['degraded', degraded('gateway')],
    ['absent', absent('gateway')],
  ])("always lists Overview, tracking the gateway's own %s state, whatever the zone map says", (_label, gateway) => {
    const entries = buildNavEntries({ iam: absent('iam'), gateway, zones: {} });
    const overview = byLabel(entries)['Overview'];
    expect(overview).toBeDefined();
    expect(overview?.state).toEqual(navStateOf(gateway));
  });

  it("Overview's href is /gateway/overview and does not depend on zones['gateway']", () => {
    const entries = buildNavEntries({ iam: absent('iam'), gateway: available('gateway'), zones: { gateway: '/somewhere-else' } });
    expect(byLabel(entries)['Overview']?.href).toBe('/gateway/overview');
  });

  it('includes the IAM entry when the zone map has an iam key', () => {
    const entries = byLabel(buildNavEntries({ iam: available('iam'), gateway: available('gateway'), zones: ZONES }));
    expect(entries['IAM']).toBeDefined();
  });

  it('omits the IAM entry when the zone map has no iam key, even when the IAM service is available', () => {
    const entries = byLabel(buildNavEntries({ iam: available('iam'), gateway: available('gateway'), zones: {} }));
    expect(entries['IAM']).toBeUndefined();
  });

  it("the IAM entry's href is `${zones['iam']}/orgs`, honouring a non-default mount prefix", () => {
    const entries = byLabel(buildNavEntries({ iam: available('iam'), gateway: available('gateway'), zones: { iam: '/identity' } }));
    expect(entries['IAM']?.href).toBe('/identity/orgs');
  });

  it.each([
    ['available', available('iam')],
    ['degraded', degraded('iam')],
    ['absent', absent('iam')],
  ])("gives the IAM entry the IAM service's own %s state", (_label, iam) => {
    const entries = byLabel(buildNavEntries({ iam, gateway: available('gateway'), zones: ZONES }));
    expect(entries['IAM']?.state).toEqual(navStateOf(iam));
  });

  it('orders Overview first, IAM second', () => {
    const entries = buildNavEntries({ iam: available('iam'), gateway: available('gateway'), zones: ZONES });
    expect(entries.map((e) => e.label)).toEqual(['Overview', 'IAM']);
  });

  it('takes EVERY entry’s state from navStateOf()', () => {
    navStateOf.mockClear();
    const entries = buildNavEntries({ iam: available('iam'), gateway: available('gateway'), zones: ZONES });
    expect(navStateOf).toHaveBeenCalledTimes(entries.length);
    expect(navStateOf.mock.results.map((r) => r.value as unknown)).toEqual(entries.map((e) => e.state));
  });
});
