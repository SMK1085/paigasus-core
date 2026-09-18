// SPDX-License-Identifier: Apache-2.0
//
// modelCallState (SMA-636 spec § 4.3, D8). It is NOT mayI(): mayI() fails open and asks about the
// current user. This asks IAM about a SERVICE ACCOUNT, and it FAILS CLOSED: a failed call never
// reads as "yes". No Cedar policy reads the principal's status, so IAM answers `allowed` for an
// archived account; that is why a status other than `active` never asks.
import { describe, expect, it } from 'vitest';
import { Code, ConnectError } from '@connectrpc/connect';
import { denial } from '@paigasus/console-core/testing';
import { modelCallState } from '../../app/(console)/service-accounts/model-call-state';

const SA = 'prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-0000000000a1';
const OWNER = 'prn:pgs:iam:::organization/0190a100-0000-7000-8000-00000000000a';

function authz(answer: boolean | Error) {
  const asked: unknown[] = [];
  const client = {
    isAuthorized: (request: unknown) => {
      asked.push(request);
      return answer instanceof Error ? Promise.reject(answer) : Promise.resolve({ allowed: answer, determiningPolicies: [], reason: '' });
    },
  };
  return { asked, client: client as never };
}

describe('modelCallState', () => {
  it('answers yes when IAM allows InvokeModel for the account at its owner node', async () => {
    const { asked, client } = authz(true);
    expect(await modelCallState(client, SA, OWNER, 'active')).toBe('yes');
    expect(asked).toEqual([{ principalPrn: SA, action: 'InvokeModel', resourcePrn: OWNER }]);
  });

  it('answers no when IAM denies it', async () => {
    const { client } = authz(false);
    expect(await modelCallState(client, SA, OWNER, 'active')).toBe('no');
  });

  it.each(['disabled', 'suspended', ''])('answers archived for status %j, and asks IAM nothing', async (status) => {
    const { asked, client } = authz(true);
    expect(await modelCallState(client, SA, OWNER, status)).toBe('archived');
    expect(asked).toEqual([]);
  });

  it('answers unknown, never yes, when the call fails — a refusal of the question included', async () => {
    for (const failure of [denial({ code: Code.PermissionDenied, reason: 'forbidden' }), new ConnectError('down', Code.Unavailable)]) {
      const { client } = authz(failure);
      expect(await modelCallState(client, SA, OWNER, 'active')).toBe('unknown');
    }
  });
});
