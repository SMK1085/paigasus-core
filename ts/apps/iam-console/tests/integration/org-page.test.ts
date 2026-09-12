// SPDX-License-Identifier: Apache-2.0
//
// The organization page loader (spec § 5.2). A non-UUID segment is notFound() with no IAM call. A
// denied GetOrganization is a PAGE error (the 403 view). An invalid-input answer to GetOrganization
// (IAM's prn-mismatch for a URL-built PRN) is notFound(). A denied list is a SECTION error.
import { Code } from '@connectrpc/connect';
import { disposeTransports } from '@paigasus/sdk/iam';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { loadOrganizationPage } from '../../app/(console)/orgs/[org]/load';
import { PAGE_SIZE } from '../../lib/paging';
import { organizationPrn, teamPrn } from '../../lib/prn';
import { denial, startFakeIam, type FakeIam, type FakeIamHandlers } from '../support/fake-iam';
import { IDS, callsSince, clientsFor, scriptedMayI } from './support';

let iam: FakeIam;

beforeAll(async () => {
  iam = await startFakeIam();
});

afterAll(async () => {
  disposeTransports();
  await iam.close();
});

const ORG_A = organizationPrn(IDS.orgA);

function teams(count: number) {
  return Array.from({ length: count }, (_, index) => ({
    prn: teamPrn(IDS.orgA, `0190a1b2-0000-7000-8000-${String(index).padStart(12, '0')}`),
    orgPrn: ORG_A,
    slug: `t-${String(index)}`,
    name: `Team ${String(index)}`,
  }));
}

function world(teamCount: number): FakeIamHandlers {
  return {
    'tenancy.getOrganization': (req: { prn: string }) => ({ organization: { prn: req.prn, slug: 'acme', name: 'Acme' } }),
    'tenancy.listTeams': () => ({ teams: teams(teamCount) }),
    // `filter` is a oneof: its type includes `{ case: undefined }`, so narrow instead of annotating.
    'tenancy.listMemberships': (req) => ({ memberships: [{ id: IDS.membership, principalPrn: IDS.principalPrn, nodePrn: req.filter.case === 'nodePrn' ? req.filter.value : '' }] }),
  };
}

const deps = (allowed: Parameters<typeof scriptedMayI>[0] = {}) => ({ tenancy: clientsFor(iam).tenancy, mayI: scriptedMayI(allowed) });

describe('loadOrganizationPage', () => {
  it('answers not-found for a segment that is not a UUID, and calls IAM not at all', async () => {
    iam.setHandlers(world(1));
    const start = iam.calls.length;

    expect(await loadOrganizationPage(deps(), { org: 'acme', offset: 0, membersOffset: 0 })).toEqual({ kind: 'not-found' });
    expect(iam.calls.length).toBe(start);
  });

  it('returns the page data, with teams, members and the three affordances', async () => {
    iam.setHandlers(world(2));
    const calls = callsSince(iam);
    const d = deps({ CreateTeam: true, AttachMembership: true, DetachMembership: false });

    const data = await loadOrganizationPage(d, { org: IDS.orgA.toUpperCase(), offset: 0, membersOffset: 0 });

    if (data.kind !== 'ok') throw new Error(`expected ok, got ${data.kind}`);
    expect(data.orgId).toBe(IDS.orgA);
    expect(data.orgPrn).toBe(ORG_A);
    expect(data.organization).toEqual({ name: 'Acme', slug: 'acme' });
    expect(data.teams).toEqual({
      ok: true,
      value: { offset: 0, nextOffset: null, rows: teams(2).map((team) => ({ prn: team.prn, teamId: team.prn.slice(-36), slug: team.slug, name: team.name })) },
    });
    expect(data.canCreateTeam).toBe(true);
    expect(data.members.canAttach).toBe(true);
    expect(data.members.canDetach).toBe(false);
    expect(data.members.list).toEqual({ ok: true, value: { offset: 0, nextOffset: null, rows: [{ id: IDS.membership, principalPrn: IDS.principalPrn }] } });
    expect(d.mayI.asked).toEqual(
      expect.arrayContaining([
        ['CreateTeam', ORG_A],
        ['AttachMembership', ORG_A],
        ['DetachMembership', ORG_A],
      ]),
    );
    expect(calls('tenancy.listMemberships').map((call) => call.request)).toEqual([expect.objectContaining({ filter: { case: 'nodePrn', value: ORG_A }, limit: PAGE_SIZE, offset: 0n })]);
  });

  it('pages the teams with the offset and offers "Next" only when the page is full', async () => {
    iam.setHandlers(world(PAGE_SIZE));
    const calls = callsSince(iam);

    const data = await loadOrganizationPage(deps(), { org: IDS.orgA, offset: 100, membersOffset: 50 });

    if (data.kind !== 'ok' || !data.teams.ok) throw new Error('expected a team list');
    expect(data.teams.value.nextOffset).toBe(150);
    expect(calls('tenancy.listTeams')[0]?.request).toMatchObject({ orgPrn: ORG_A, limit: PAGE_SIZE, offset: 100n });
    expect(calls('tenancy.listMemberships')[0]?.request).toMatchObject({ offset: 50n });
  });

  it('turns a denied GetOrganization into a page error and asks for nothing else', async () => {
    iam.setHandlers({
      ...world(1),
      'tenancy.getOrganization': () => {
        throw denial({ correlationId: 'corr-org-page' });
      },
    });
    const calls = callsSince(iam);

    const data = await loadOrganizationPage(deps({ CreateTeam: true }), { org: IDS.orgA, offset: 0, membersOffset: 0 });

    if (data.kind !== 'error') throw new Error(`expected an error, got ${data.kind}`);
    expect(data.error.presentation).toBe('forbidden');
    expect(data.error.correlationId).toBe('corr-org-page');
    expect(calls('tenancy.listTeams')).toHaveLength(0);
    expect(calls('tenancy.listMemberships')).toHaveLength(0);
  });

  it('answers not-found when IAM refuses the URL-built PRN as invalid input (prn-mismatch), and asks for nothing else', async () => {
    // Real IAM answers a PRN whose canonical form differs from the stored node with PrnMismatch,
    // which is InvalidArgument (adapters/grpc/tenancy.rs:176-178). Spec § 5.2: a mismatched URL is a 404.
    iam.setHandlers({
      ...world(1),
      'tenancy.getOrganization': () => {
        throw denial({ code: Code.InvalidArgument, reason: 'prn-mismatch', correlationId: 'corr-org-mismatch' });
      },
    });
    const calls = callsSince(iam);

    expect(await loadOrganizationPage(deps({ CreateTeam: true }), { org: IDS.orgA, offset: 0, membersOffset: 0 })).toEqual({ kind: 'not-found' });
    expect(calls('tenancy.getOrganization')).toHaveLength(1);
    expect(calls('tenancy.listTeams')).toHaveLength(0);
    expect(calls('tenancy.listMemberships')).toHaveLength(0);
  });

  it('keeps the page when a list is denied, and puts the error in that section', async () => {
    iam.setHandlers({
      ...world(1),
      'tenancy.listMemberships': () => {
        throw denial({ code: Code.PermissionDenied, correlationId: 'corr-members' });
      },
    });

    const data = await loadOrganizationPage(deps(), { org: IDS.orgA, offset: 0, membersOffset: 0 });

    if (data.kind !== 'ok' || data.members.list.ok) throw new Error('expected a members section error');
    expect(data.members.list.error.correlationId).toBe('corr-members');
    expect(data.teams.ok).toBe(true);
  });
});
