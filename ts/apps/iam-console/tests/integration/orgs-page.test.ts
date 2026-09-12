// SPDX-License-Identifier: Apache-2.0
//
// The "All organizations" section (spec § 5.1 step 5) and the create affordance. The section is
// an affordance: when mayI() says no, it is hidden and issues NO call. When mayI() says yes and
// IAM says no, the section shows IAM's 403 inline and the page lives (AC 2).
import { ErrorReason } from '@paigasus/sdk/errors/types';
import { disposeTransports } from '@paigasus/sdk/iam';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { loadOrganizationsPage } from '../../app/(console)/orgs/load';
import { PAGE_SIZE } from '../../lib/paging';
import { ROOT_PRN, organizationPrn } from '../../lib/prn';
import { denial, startFakeIam, type FakeIam } from '../support/fake-iam';
import { callsSince, clientsFor, scriptedMayI } from './support';

let iam: FakeIam;

beforeAll(async () => {
  iam = await startFakeIam();
});

afterAll(async () => {
  disposeTransports();
  await iam.close();
});

function organizations(count: number) {
  return Array.from({ length: count }, (_, index) => ({
    prn: organizationPrn(`0190a100-0000-7000-8000-${String(index).padStart(12, '0')}`),
    slug: `org-${String(index)}`,
    name: `Org ${String(index)}`,
  }));
}

describe('loadOrganizationsPage', () => {
  it('hides "All organizations" and makes no ListOrganizations call when mayI says no', async () => {
    iam.setHandlers({ 'tenancy.listOrganizations': () => ({ organizations: organizations(1) }) });
    const calls = callsSince(iam);
    const mayI = scriptedMayI({ CreateOrganization: true });

    const data = await loadOrganizationsPage({ tenancy: clientsFor(iam).tenancy, mayI }, { offset: 0 });

    expect(data).toEqual({ canCreateOrganization: true, all: null });
    expect(calls('tenancy.listOrganizations')).toHaveLength(0);
    expect(mayI.asked).toEqual(
      expect.arrayContaining([
        ['ListOrganizations', ROOT_PRN],
        ['CreateOrganization', ROOT_PRN],
      ]),
    );
  });

  it('lists one page, links each row by UUID, and offers "Next" only when the page is full', async () => {
    iam.setHandlers({ 'tenancy.listOrganizations': (req: { offset: bigint }) => ({ organizations: organizations(req.offset === 0n ? PAGE_SIZE : 3) }) });
    const calls = callsSince(iam);
    const deps = { tenancy: clientsFor(iam).tenancy, mayI: scriptedMayI({ ListOrganizations: true }) };

    const first = await loadOrganizationsPage(deps, { offset: 0 });
    const second = await loadOrganizationsPage(deps, { offset: PAGE_SIZE });

    if (first.all?.ok !== true || second.all?.ok !== true) throw new Error('expected two listed pages');
    expect(first.all.value.rows).toHaveLength(PAGE_SIZE);
    expect(first.all.value.rows[0]).toEqual({ prn: organizationPrn('0190a100-0000-7000-8000-000000000000'), orgId: '0190a100-0000-7000-8000-000000000000', slug: 'org-0', name: 'Org 0' });
    expect(first.all.value.nextOffset).toBe(PAGE_SIZE);
    expect(second.all.value.nextOffset).toBeNull();
    expect(calls('tenancy.listOrganizations').map((call) => call.request)).toEqual([
      expect.objectContaining({ limit: PAGE_SIZE, offset: 0n }),
      expect.objectContaining({ limit: PAGE_SIZE, offset: BigInt(PAGE_SIZE) }),
    ]);
  });

  it("shows IAM's 403 inline when mayI says yes and IAM denies (AC 2)", async () => {
    iam.setHandlers({
      'tenancy.listOrganizations': () => {
        throw denial({ correlationId: 'corr-all-orgs' });
      },
    });
    const calls = callsSince(iam);

    const data = await loadOrganizationsPage({ tenancy: clientsFor(iam).tenancy, mayI: scriptedMayI({ ListOrganizations: true }) }, { offset: 0 });

    if (data.all === null || data.all.ok) throw new Error('expected an inline error');
    expect(data.all.error.presentation).toBe('forbidden');
    expect(data.all.error.reason).toBe(ErrorReason.FORBIDDEN);
    expect(data.all.error.correlationId).toBe('corr-all-orgs');
    expect(calls('tenancy.listOrganizations')).toHaveLength(1);
  });
});
