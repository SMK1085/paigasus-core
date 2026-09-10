// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import { Capability, CapabilitySchema, capabilityWireKey } from '@paigasus/proto';
import { CAPABILITY_KEYS, SERVICE_SLUGS } from '../src/core/state.js';
import { SERVICE_STATES, type CapabilityKey } from '../src/types.js';

describe('capability vocabulary', () => {
  it('derives every non-sentinel capability key from the proto registry', () => {
    // Derived here INDEPENDENTLY of the implementation, from the generated schema, so this test
    // fails if the implementation hand-tabulates a list that drifts from the proto.
    const expected = Object.values(Capability)
      .filter((v): v is Capability => typeof v === 'number')
      .map((v) => capabilityWireKey(v))
      .filter((k): k is string => k !== undefined)
      .sort();

    expect([...CAPABILITY_KEYS].sort()).toEqual(expected);
  });

  it('excludes the zero sentinel', () => {
    expect(CAPABILITY_KEYS).not.toContain(undefined);
    expect(capabilityWireKey(Capability.UNSPECIFIED)).toBeUndefined();
    expect(CAPABILITY_KEYS.length).toBe(Object.keys(CapabilitySchema.value).length - 1);
  });

  it('derives service slugs as the first dot-segment of every key', () => {
    expect([...SERVICE_SLUGS].sort()).toEqual(['gateway', 'iam']);
  });

  it('every capability key starts with one of the service slugs', () => {
    for (const key of CAPABILITY_KEYS) {
      expect(SERVICE_SLUGS).toContain(key.split('.')[0]);
    }
  });

  it('exposes the three state names', () => {
    expect(SERVICE_STATES).toEqual(['absent', 'available', 'degraded']);
  });

  it('the CapabilityKey union matches the registry exactly', () => {
    // CapabilityKey is a HAND-DECLARED closed union in src/types.ts, because a template literal
    // like `${string}.${string}` would accept the typo `iam.audits` and silently defeat the whole
    // reason the type is not `string`. Hand-declaring it re-opens a drift risk, and THIS
    // assertion is what closes it: the union's members are listed once here and compared to the
    // registry-derived runtime list, so a new capability in the proto reds this test.
    const declared: CapabilityKey[] = ['iam.authz.cedar', 'iam.apikeys', 'iam.audit', 'gateway.chat.stream'];
    expect([...declared].sort()).toEqual([...CAPABILITY_KEYS].sort());
  });
});
