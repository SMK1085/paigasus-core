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
});

describe('generated ServiceInfoService', () => {
  it('is declared in paigasus.common.v1', () => {
    expect(ServiceInfoService.typeName).toBe('paigasus.common.v1.ServiceInfoService');
  });
});
