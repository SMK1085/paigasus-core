// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import { capabilityWireKey } from './capability.js';
import { Capability, ServiceInfoService } from './generated/paigasus/common/v1/service_info_pb.js';

describe('capabilityWireKey', () => {
  it('spells the ADR-0020 keys exactly', () => {
    expect(capabilityWireKey(Capability.IAM_AUTHZ_CEDAR)).toBe('iam.authz.cedar');
    expect(capabilityWireKey(Capability.IAM_APIKEYS)).toBe('iam.apikeys');
    expect(capabilityWireKey(Capability.IAM_AUDIT)).toBe('iam.audit');
    expect(capabilityWireKey(Capability.GATEWAY_CHAT_STREAM)).toBe('gateway.chat.stream');
  });

  it('has no wire key for the zero sentinel', () => {
    expect(capabilityWireKey(Capability.UNSPECIFIED)).toBeUndefined();
  });

  it('has no wire key for a number the enum does not know', () => {
    // Realistic, not just defensive: a newer service can advertise a
    // capability this build's generated enum predates.
    expect(capabilityWireKey(99 as Capability)).toBeUndefined();
  });

  it('covers exactly the registered capabilities', () => {
    // Guards against a fifth capability being registered without a
    // corresponding assertion above.
    const members = Object.values(Capability).filter((v) => typeof v === 'number');
    expect(members).toHaveLength(5);
  });

  it('produces keys matching the documented grammar for every non-sentinel capability', () => {
    const members = Object.values(Capability).filter((v): v is Capability => typeof v === 'number');
    for (const capability of members) {
      if (capability === Capability.UNSPECIFIED) {
        continue;
      }
      const key = capabilityWireKey(capability);
      expect(key).toMatch(/^[a-z][a-z0-9]*(\.[a-z0-9]+)*$/);
    }
  });
});

describe('generated ServiceInfoService', () => {
  it('is declared in paigasus.common.v1', () => {
    expect(ServiceInfoService.typeName).toBe('paigasus.common.v1.ServiceInfoService');
  });
});
