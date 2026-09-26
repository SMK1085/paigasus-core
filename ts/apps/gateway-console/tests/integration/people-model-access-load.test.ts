// SPDX-License-Identifier: Apache-2.0
//
// The "Model access for people" loader (SMA-676 spec § 4.4, § 5.1, § 7.2) against the fake IAM:
// the disabled state makes no call (D15), the join offers the org creator as a candidate (D12,
// § 1.1 fact 6), a holder who is not a member shows the mark (D13), and each list reads pages of
// 200 with at most five pages and one N+1 probe.
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { Code } from '@connectrpc/connect';
import type { ServiceState } from '@paigasus/discovery/types';
import { disposeTransports } from '@paigasus/sdk/iam';
import { PrincipalKind } from '@paigasus/sdk/iam/types';
import { organizationPrn } from '@paigasus/console-core';
import { denial, startFakeIam, type FakeIam, type FakeIamHandlers } from '@paigasus/console-core/testing';
import { LIST_ROW_CAP, loadPeopleModelAccess, type PeopleModelAccessDeps } from '../../app/(console)/people-model-access/load';
import { IDS, callsSince, clientsFor, scriptedMayI } from './support';

const ORG = organizationPrn(IDS.orgA);
const CEDAR: ServiceState = { state: 'available', service: 'iam', descriptor: { service: 'iam', version: '1.0.0', capabilities: ['iam.authz.cedar'] }, capabilities: ['iam.authz.cedar'] };
const NO_CEDAR: ServiceState = { state: 'available', service: 'iam', descriptor: { service: 'iam', version: '1.0.0', capabilities: [] }, capabilities: [] };
const person = (n: number): string => `prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-${String(n).padStart(12, '0')}`;
const grant = (n: number, principalPrn: string, roleKey: string) => ({ id: `0190a1d4-0000-7000-8000-${String(n).padStart(12, '0')}`, principalPrn, roleKey, scopePrn: ORG });
const member = (principalPrn: string) => ({ id: `m-${principalPrn.slice(-4)}`, principalPrn, nodePrn: ORG });

let iam: FakeIam;
beforeAll(async () => {
  iam = await startFakeIam();
});
afterAll(async () => {
  disposeTransports();
  await iam.close();
});

function deps(allowed: { GrantRole?: boolean; RevokeRole?: boolean } = { GrantRole: true, RevokeRole: true }, state: ServiceState = CEDAR): PeopleModelAccessDeps {
  const clients = clientsFor(iam);
  return { authz: clients.authz, tenancy: clients.tenancy, mayI: scriptedMayI(allowed), iam: state };
}

/** A world of `grantRows` grants and `memberRows` members, both served with IAM's offset paging. */
function world(grantRows: readonly ReturnType<typeof grant>[], memberRows: readonly ReturnType<typeof member>[]): FakeIamHandlers {
  const slice = <T>(rows: readonly T[], req: { limit: number; offset: bigint }): T[] => rows.slice(Number(req.offset), Number(req.offset) + req.limit);
  return {
    'authz.listRoleGrants': (req) => ({ grants: slice(grantRows, req) }),
    'tenancy.listMemberships': (req) => ({ memberships: slice(memberRows, req) }),
  };
}

describe('loadPeopleModelAccess', () => {
  it('makes no IAM call when iam.authz.cedar is off (D15)', async () => {
    iam.setHandlers({});
    const calls = callsSince(iam);
    const may = scriptedMayI({ GrantRole: true, RevokeRole: true });
    const view = await loadPeopleModelAccess({ ...deps(), mayI: may, iam: NO_CEDAR }, { orgPrn: ORG, orgActive: true });
    expect(view).toEqual({ kind: 'disabled' });
    expect(calls('authz.listRoleGrants')).toHaveLength(0);
    expect(calls('tenancy.listMemberships')).toHaveLength(0);
    expect(may.asked).toEqual([]);
  });

  it('asks for user grants at the org and user members of the org, in pages of 200', async () => {
    iam.setHandlers(world([], []));
    const calls = callsSince(iam);
    await loadPeopleModelAccess(deps(), { orgPrn: ORG, orgActive: true });
    expect(calls('authz.listRoleGrants')[0]?.request).toMatchObject({ principalPrn: '', scopePrn: ORG, principalKind: PrincipalKind.USER, limit: 200, offset: 0n });
    expect(calls('tenancy.listMemberships')[0]?.request).toMatchObject({ filter: { case: 'nodePrn', value: ORG }, principalKind: PrincipalKind.USER, limit: 200, offset: 0n });
  });

  it('joins holders and candidates, offering the org creator who is not a member (D12, D13)', async () => {
    const creator = person(1);
    const memberHolder = person(2);
    const plainMember = person(3);
    iam.setHandlers(world([grant(1, creator, 'org_admin'), grant(2, memberHolder, 'gateway_user'), grant(3, person(4), 'gateway_user')], [member(memberHolder), member(plainMember)]));
    const view = await loadPeopleModelAccess(deps(), { orgPrn: ORG, orgActive: true });
    expect(view).toEqual({
      kind: 'ok',
      orgPrn: ORG,
      readOnly: false,
      holders: [
        { grantId: grant(2, memberHolder, 'gateway_user').id, principalPrn: memberHolder, member: true },
        { grantId: grant(3, person(4), 'gateway_user').id, principalPrn: person(4), member: false },
      ],
      candidates: [{ principalPrn: creator }, { principalPrn: plainMember }],
      flags: { canGrant: true, canRevoke: true, grantsTruncated: false, membersTruncated: false },
    });
  });

  it('shows controls only when mayI allows them and the org is active (D14)', async () => {
    iam.setHandlers(world([], []));
    const inactive = await loadPeopleModelAccess(deps(), { orgPrn: ORG, orgActive: false });
    expect(inactive).toMatchObject({ kind: 'ok', readOnly: true, flags: { canGrant: false, canRevoke: false } });
    const noRevoke = await loadPeopleModelAccess(deps({ GrantRole: true, RevokeRole: false }), { orgPrn: ORG, orgActive: true });
    expect(noRevoke).toMatchObject({ kind: 'ok', flags: { canGrant: true, canRevoke: false } });
  });

  it('reads five full pages and probes once; exactly 1000 rows is not truncated', async () => {
    const rows = Array.from({ length: LIST_ROW_CAP }, (_, i) => grant(10_000 + i, person(10_000 + i), 'org_admin'));
    iam.setHandlers(world(rows, []));
    const calls = callsSince(iam);
    const view = await loadPeopleModelAccess(deps(), { orgPrn: ORG, orgActive: true });
    const requests = calls('authz.listRoleGrants').map((call) => call.request as { limit: number; offset: bigint });
    expect(requests.map((r) => [r.limit, r.offset])).toEqual([
      [200, 0n],
      [200, 200n],
      [200, 400n],
      [200, 600n],
      [200, 800n],
      [1, 1000n],
    ]);
    expect(view).toMatchObject({ kind: 'ok', flags: { grantsTruncated: false } });
    if (view.kind === 'ok') expect(view.candidates).toHaveLength(LIST_ROW_CAP);
  });

  it('marks a list truncated when the probe finds row 1001, and hides the not-a-member mark when the member list is truncated', async () => {
    const holder = person(1);
    const members = Array.from({ length: LIST_ROW_CAP + 1 }, (_, i) => member(person(20_000 + i)));
    iam.setHandlers(world([grant(1, holder, 'gateway_user')], members));
    const view = await loadPeopleModelAccess(deps(), { orgPrn: ORG, orgActive: true });
    expect(view).toMatchObject({ kind: 'ok', flags: { grantsTruncated: false, membersTruncated: true } });
    if (view.kind === 'ok') expect(view.holders).toEqual([{ grantId: grant(1, holder, 'gateway_user').id, principalPrn: holder, member: null }]);
  });

  it('stops after a short page: one call for a list of 199', async () => {
    iam.setHandlers(
      world(
        Array.from({ length: 199 }, (_, i) => grant(30_000 + i, person(30_000 + i), 'org_admin')),
        [],
      ),
    );
    const calls = callsSince(iam);
    await loadPeopleModelAccess(deps(), { orgPrn: ORG, orgActive: true });
    expect(calls('authz.listRoleGrants')).toHaveLength(1);
  });

  it('is denied when either list is forbidden, and an error for another failure; the page stays up', async () => {
    iam.setHandlers({
      'authz.listRoleGrants': () => {
        throw denial();
      },
      'tenancy.listMemberships': () => ({ memberships: [] }),
    });
    expect(await loadPeopleModelAccess(deps(), { orgPrn: ORG, orgActive: true })).toEqual({ kind: 'denied' });
    iam.setHandlers({
      'authz.listRoleGrants': () => ({ grants: [] }),
      'tenancy.listMemberships': () => {
        throw denial({ code: Code.Unavailable, reason: 'internal' });
      },
    });
    expect(await loadPeopleModelAccess(deps(), { orgPrn: ORG, orgActive: true })).toMatchObject({ kind: 'error' });
  });
});
