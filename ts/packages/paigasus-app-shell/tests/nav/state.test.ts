// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import type { CapabilityKey, ServiceState } from '@paigasus/discovery/types';
import { navStateOf, type NavEntryState } from '../../src/nav/state';

const descriptor = { service: 'gateway', version: '9.9.9-probe', capabilities: ['gateway.chat.stream'] };
const absent: ServiceState = { state: 'absent', service: 'gateway' };
const available: ServiceState = { state: 'available', service: 'gateway', descriptor, capabilities: ['gateway.chat.stream'] };
const availableWithout: ServiceState = { state: 'available', service: 'gateway', descriptor: { ...descriptor, capabilities: [] }, capabilities: [] };
const degraded: ServiceState = { state: 'degraded', service: 'gateway', reason: 'timeout', descriptor, capabilities: ['gateway.chat.stream'] };

// Spec § 7.1. The branch itself is @paigasus/discovery's capabilityOutcome (one copy, tested there).
const ROWS: ReadonlyArray<readonly [string, ServiceState, CapabilityKey | undefined, NavEntryState]> = [
  ['absent, no need', absent, undefined, { state: 'absent' }],
  ['absent, with a need', absent, 'gateway.chat.stream', { state: 'absent' }],
  ['available, no need', available, undefined, { state: 'available' }],
  ['available, need listed', available, 'gateway.chat.stream', { state: 'available' }],
  ['available, need NOT listed: absent, because the build lacks it (not an outage)', availableWithout, 'gateway.chat.stream', { state: 'absent' }],
  ['degraded', degraded, 'gateway.chat.stream', { state: 'degraded', service: 'gateway', reason: 'timeout' }],
];

describe('navStateOf', () => {
  it.each(ROWS)('%s', (_label, serviceState, need, expected) => {
    expect(navStateOf(serviceState, need)).toStrictEqual(expected);
  });

  it('drops the descriptor: no version and no capability list reach the browser', () => {
    const result = navStateOf(degraded);
    expect(Object.keys(result).sort()).toEqual(['reason', 'service', 'state']);
    expect(JSON.stringify(result)).not.toContain('9.9.9-probe');
    expect(JSON.stringify(result)).not.toContain('capabilities');
  });

  it('a key from another service throws (a programming error)', () => {
    expect(() => navStateOf(available, 'iam.audit')).toThrow(/belongs to service "iam"/);
  });
});
