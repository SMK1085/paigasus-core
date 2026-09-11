// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import { capabilityOutcome, type CapabilityOutcome } from '../src/client';
import type { CapabilityKey, ServiceState } from '../src/types.js';

const descriptor = { service: 'iam', version: '1.0.0', capabilities: ['iam.audit'] };
const absent: ServiceState = { state: 'absent', service: 'iam' };
const available: ServiceState = { state: 'available', service: 'iam', descriptor, capabilities: ['iam.audit'] };
const degradedWithDescriptor: ServiceState = { state: 'degraded', service: 'iam', reason: 'timeout', descriptor, capabilities: ['iam.audit'] };
const degradedCold: ServiceState = { state: 'degraded', service: 'iam', reason: 'network', descriptor: null, capabilities: [] };

// <Capability>'s branch table (spec F10, § 7.1). This is the ONE copy: <Capability> and
// @paigasus/app-shell's navStateOf both call capabilityOutcome, so no parity test is needed.
const ROWS: ReadonlyArray<readonly [string, ServiceState, CapabilityKey | undefined, CapabilityOutcome]> = [
  ['absent, no need', absent, undefined, 'hidden'],
  ['absent, with a need', absent, 'iam.audit', 'hidden'],
  ['available, no need', available, undefined, 'shown'],
  ['available, need listed', available, 'iam.audit', 'shown'],
  ['available, need NOT listed (the build lacks it; not an outage)', available, 'iam.apikeys', 'hidden'],
  ['degraded, the cached descriptor has the key', degradedWithDescriptor, 'iam.audit', 'degraded'],
  ['degraded, no descriptor at all', degradedCold, 'iam.apikeys', 'degraded'],
  ['degraded, no need', degradedCold, undefined, 'degraded'],
];

describe('capabilityOutcome', () => {
  it.each(ROWS)('%s', (_label, state, need, expected) => {
    expect(capabilityOutcome(state, need)).toBe(expected);
  });

  it('throws when the key belongs to another service (a programming error)', () => {
    expect(() => capabilityOutcome(available, 'gateway.chat.stream')).toThrow(/belongs to service "gateway"/);
  });

  it('checks the service BEFORE the state, so an absent state does not hide the mistake', () => {
    expect(() => capabilityOutcome(absent, 'gateway.chat.stream')).toThrow(/belongs to service "gateway"/);
  });
});
