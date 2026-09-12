// SPDX-License-Identifier: Apache-2.0
//
// createMayI (spec § 4.6, § 6.3): it asks IsAuthorized about the current principal, memoizes per
// (action, resource), and FAILS OPEN.
import { describe, expect, it, vi } from 'vitest';
import { Code, ConnectError } from '@connectrpc/connect';
import { createMayI } from '../../lib/authorize';

const ME = 'prn:pgs:iam:::principal/0192f1c0-0000-7000-8000-000000000001';
const ORG = 'prn:pgs:iam::0192f1c0-0000-7000-8000-000000000002:organization/0192f1c0-0000-7000-8000-000000000002';

function fakeAuthz(answer: (req: { principalPrn: string; action: string; resourcePrn: string }) => boolean | Error) {
  const calls: { principalPrn: string; action: string; resourcePrn: string }[] = [];
  const isAuthorized = vi.fn((req: { principalPrn: string; action: string; resourcePrn: string }) => {
    calls.push(req);
    const result = answer(req);
    return result instanceof Error ? Promise.reject(result) : Promise.resolve({ allowed: result, determiningPolicies: [], reason: '' });
  });
  return { authz: { isAuthorized } as never, calls };
}

describe('createMayI', () => {
  it('asks IsAuthorized about the current principal with the PascalCase action', async () => {
    const { authz, calls } = fakeAuthz(() => false);
    const mayI = createMayI({ authz, principalPrn: ME, logger: { appEvent: vi.fn() } });
    expect(await mayI('CreateTeam', ORG)).toBe(false);
    expect(calls).toEqual([{ principalPrn: ME, action: 'CreateTeam', resourcePrn: ORG }]);
  });

  it('asks once per (action, resource) for the instance’s lifetime', async () => {
    const { authz, calls } = fakeAuthz(() => true);
    const mayI = createMayI({ authz, principalPrn: ME, logger: { appEvent: vi.fn() } });
    await Promise.all([mayI('CreateTeam', ORG), mayI('CreateTeam', ORG), mayI('AttachMembership', ORG)]);
    await mayI('CreateTeam', ORG);
    expect(calls.map((c) => c.action)).toEqual(['CreateTeam', 'AttachMembership']);
  });

  it('fails OPEN on a failed query, and logs authorize.query_failed', async () => {
    const { authz } = fakeAuthz(() => new ConnectError('down', Code.Unavailable));
    const appEvent = vi.fn();
    const mayI = createMayI({ authz, principalPrn: ME, logger: { appEvent } });
    expect(await mayI('ListAuditLog', ORG)).toBe(true);
    expect(appEvent).toHaveBeenCalledWith('authorize.query_failed', { action: 'ListAuditLog', presentation: 'degraded' });
  });

  it('answers true without a call when IAM could not say who the principal is', async () => {
    const { authz, calls } = fakeAuthz(() => false);
    const mayI = createMayI({ authz, principalPrn: null, logger: { appEvent: vi.fn() } });
    expect(await mayI('CreateOrganization', ORG)).toBe(true);
    expect(calls).toEqual([]);
  });
});
