// SPDX-License-Identifier: Apache-2.0
//
// SMA-511 spec § 7.3. The iam console provisions its principal with
// ServiceInfoService.GetServiceInfo (spec § 4.5), and apps must not import @paigasus/proto, so the
// SDK's ./iam entry re-exports the service. It must be the paigasus.common.v1 service from the proto
// ROOT entry — never anything from @paigasus/proto/iam, which keeps a deprecated ServiceInfo.
import { afterEach, describe, expect, it } from 'vitest';
import { ServiceInfoService as ProtoServiceInfoService } from '@paigasus/proto';
import { ServiceInfoService, createIamClient, disposeTransports } from '../src/iam.js';
import * as barrel from '../src/index.js';

afterEach(() => {
  disposeTransports();
});

describe('@paigasus/sdk/iam re-exports ServiceInfoService (SMA-511 spec § 7.3)', () => {
  it('is the SAME object as the @paigasus/proto root export', () => {
    expect(ServiceInfoService).toBe(ProtoServiceInfoService);
  });

  it('is the common.v1 service with its one rpc', () => {
    expect(ServiceInfoService.typeName).toBe('paigasus.common.v1.ServiceInfoService');
    expect(Object.keys(ServiceInfoService.method)).toEqual(['getServiceInfo']);
  });

  it('builds a request-scoped client through createIamClient', () => {
    const client = createIamClient(ServiceInfoService, { baseUrl: 'http://127.0.0.1:9' }, { bearer: 'token' });
    expect(typeof client.getServiceInfo).toBe('function');
  });

  // Both sides are checked for a value first. Before the re-export both are `undefined`, and `toBe`
  // alone passes on `undefined === undefined` (measured, pre-flight T6.a).
  it('the root barrel serves the same object', () => {
    expect(barrel.ServiceInfoService).toBeDefined();
    expect(ServiceInfoService).toBeDefined();
    expect(barrel.ServiceInfoService).toBe(ServiceInfoService);
  });
});
