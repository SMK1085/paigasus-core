// SPDX-License-Identifier: Apache-2.0
//
// Tier 2 (spec § 9.3): loadMyScopes (spec § 5.1) against the fake IAM over real gRPC. Every case
// scripts the answers, and most cases count the calls. The "ListRoleGrants fails" case counts its
// call, because its result is the same when the call never happens. The cedarCapabilityOf cases at
// the bottom are pure: they cover the one check in myScopes() that selects "memberships only".
import { Code } from '@connectrpc/connect';
import type { ServiceState } from '@paigasus/discovery/types';
import { disposeTransports } from '@paigasus/sdk/iam';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { ROOT_PRN, organizationPrn, projectPrn, teamPrn } from '../../lib/prn';
import { SCOPE_CAP, cedarCapabilityOf, loadMyScopes } from '../../lib/scopes';
import { denial, startFakeIam, type FakeIam, type FakeIamHandlers } from '../support/fake-iam';
import { IDS, callsSince, clientsFor } from './support';

let iam: FakeIam;

beforeAll(async () => {
  iam = await startFakeIam();
});

afterAll(async () => {
  disposeTransports();
  await iam.close();
});

const ORG_A = organizationPrn(IDS.orgA);
const ORG_B = organizationPrn(IDS.orgB);
const TEAM_A1 = teamPrn(IDS.orgA, IDS.teamA1);
const TEAM_B1 = teamPrn(IDS.orgB, IDS.teamB1);
const PROJECT_B1 = projectPrn(IDS.orgB, IDS.projectB1);

function principal(...nodePrns: string[]) {
  return { prn: IDS.principalPrn, memberships: nodePrns.map((nodePrn) => ({ nodePrn })) };
}

function ports() {
  const { tenancy, authz } = clientsFor(iam);
  return { tenancy, authz };
}

/** GetOrganization, GetTeam and GetProject from one name table. An unknown PRN is NotFound. */
function tenancyTable(names: Readonly<Record<string, string>>): FakeIamHandlers {
  const name = (prn: string): string => {
    const value = names[prn];
    if (value === undefined) throw denial({ code: Code.NotFound, reason: 'not-found' });
    return value;
  };
  return {
    'tenancy.getOrganization': (req: { prn: string }) => ({ organization: { prn: req.prn, slug: 'slug', name: name(req.prn) } }),
    'tenancy.getTeam': (req: { prn: string }) => ({ team: { prn: req.prn, orgPrn: ORG_A, slug: 'slug', name: name(req.prn) } }),
    'tenancy.getProject': (req: { prn: string }) => ({ project: { prn: req.prn, teamPrn: TEAM_B1, orgPrn: ORG_B, slug: 'slug', name: name(req.prn) } }),
  };
}

function grants(...scopePrns: string[]): FakeIamHandlers {
  return {
    'authz.listRoleGrants': () => ({
      grants: scopePrns.map((scopePrn, index) => ({ id: `grant-${String(index)}`, principalPrn: IDS.principalPrn, roleKey: 'viewer', scopePrn })),
    }),
  };
}

describe('loadMyScopes', () => {
  it('lists the memberships only, and never calls ListRoleGrants, when IAM does not report iam.authz.cedar', async () => {
    iam.setHandlers({ ...tenancyTable({ [ORG_A]: 'Acme', [TEAM_A1]: 'Platform' }), ...grants(PROJECT_B1) });
    const calls = callsSince(iam);

    const result = await loadMyScopes({ ...ports(), principal: principal(ORG_A, TEAM_A1), cedarCapability: false });

    expect(result).toEqual({
      grantsListed: false,
      hiddenCount: 0,
      entries: [
        { kind: 'organization', prn: ORG_A, orgId: IDS.orgA, label: 'Acme', denied: false },
        { kind: 'team', prn: TEAM_A1, orgId: IDS.orgA, teamId: IDS.teamA1, label: 'Platform', denied: false },
      ],
    });
    expect(calls('authz.listRoleGrants')).toHaveLength(0);
  });

  it("adds the scopes of the principal's own role grants when IAM reports iam.authz.cedar", async () => {
    iam.setHandlers({ ...tenancyTable({ [ORG_A]: 'Acme', [PROJECT_B1]: 'Models' }), ...grants(PROJECT_B1) });
    const calls = callsSince(iam);

    const result = await loadMyScopes({ ...ports(), principal: principal(ORG_A), cedarCapability: true });

    expect(result.grantsListed).toBe(true);
    expect(result.entries).toEqual([
      { kind: 'organization', prn: ORG_A, orgId: IDS.orgA, label: 'Acme', denied: false },
      { kind: 'project', prn: PROJECT_B1, orgId: IDS.orgB, teamId: IDS.teamB1, projectId: IDS.projectB1, label: 'Models', denied: false },
    ]);
    const [listed] = calls('authz.listRoleGrants');
    expect(listed?.request).toMatchObject({ principalPrn: IDS.principalPrn, limit: 200, offset: 0n });
  });

  it('gives a team-only scope a direct entry and never asks for its organization', async () => {
    iam.setHandlers({ ...tenancyTable({ [TEAM_A1]: 'Platform' }), ...grants(TEAM_A1) });
    const calls = callsSince(iam);

    const result = await loadMyScopes({ ...ports(), principal: principal(), cedarCapability: true });

    expect(result.entries).toEqual([{ kind: 'team', prn: TEAM_A1, orgId: IDS.orgA, teamId: IDS.teamA1, label: 'Platform', denied: false }]);
    expect(calls('tenancy.getOrganization')).toHaveLength(0);
    expect(calls('tenancy.getTeam')).toHaveLength(1);
  });

  it("takes a project-only scope's team from GetProject", async () => {
    iam.setHandlers({ ...tenancyTable({ [PROJECT_B1]: 'Models' }), ...grants(PROJECT_B1) });
    const calls = callsSince(iam);

    const result = await loadMyScopes({ ...ports(), principal: principal(), cedarCapability: true });

    expect(result.entries).toEqual([{ kind: 'project', prn: PROJECT_B1, orgId: IDS.orgB, teamId: IDS.teamB1, projectId: IDS.projectB1, label: 'Models', denied: false }]);
    expect(calls('tenancy.getProject')).toHaveLength(1);
  });

  it('deduplicates a scope that is both a membership and a grant, in any UUID case', async () => {
    iam.setHandlers({ ...tenancyTable({ [ORG_A]: 'Acme' }), ...grants(ORG_A) });
    const calls = callsSince(iam);
    const upper = `prn:pgs:iam:::organization/${IDS.orgA.toUpperCase()}`;

    const result = await loadMyScopes({ ...ports(), principal: principal(ORG_A, upper), cedarCapability: true });

    expect(result.entries.map((entry) => entry.prn)).toEqual([ORG_A]);
    expect(calls('tenancy.getOrganization')).toHaveLength(1);
  });

  it('skips a grant whose scope is not a tenancy node, such as the root', async () => {
    iam.setHandlers({ ...tenancyTable({ [ORG_A]: 'Acme' }), ...grants(ROOT_PRN, ORG_A) });

    const result = await loadMyScopes({ ...ports(), principal: principal(), cedarCapability: true });

    expect(result.entries.map((entry) => entry.prn)).toEqual([ORG_A]);
  });

  it('marks a denied row, keeps its PRN, and does not fail the page', async () => {
    iam.setHandlers({
      ...tenancyTable({ [TEAM_A1]: 'Platform' }),
      'tenancy.getOrganization': () => {
        throw denial({ correlationId: 'corr-scope-row' });
      },
    });

    const result = await loadMyScopes({ ...ports(), principal: principal(ORG_A, TEAM_A1), cedarCapability: false });

    expect(result.entries).toEqual([
      { kind: 'organization', prn: ORG_A, orgId: IDS.orgA, label: null, denied: true },
      { kind: 'team', prn: TEAM_A1, orgId: IDS.orgA, teamId: IDS.teamA1, label: 'Platform', denied: false },
    ]);
  });

  it('gives a row that failed for another reason no label, but does not call it denied', async () => {
    iam.setHandlers(tenancyTable({}));

    const result = await loadMyScopes({ ...ports(), principal: principal(ORG_A), cedarCapability: false });

    expect(result.entries).toEqual([{ kind: 'organization', prn: ORG_A, orgId: IDS.orgA, label: null, denied: false }]);
  });

  it('shows at most SCOPE_CAP scopes, asks IAM about those only, and counts the rest', async () => {
    const teams = Array.from({ length: SCOPE_CAP + 10 }, (_, index) => teamPrn(IDS.orgA, `0190a1b2-0000-7000-8000-${String(index).padStart(12, '0')}`));
    iam.setHandlers({ 'tenancy.getTeam': (req: { prn: string }) => ({ team: { prn: req.prn, orgPrn: ORG_A, slug: 's', name: 'Team' } }) });
    const calls = callsSince(iam);

    const result = await loadMyScopes({ ...ports(), principal: principal(...teams), cedarCapability: false });

    expect(result.entries).toHaveLength(SCOPE_CAP);
    expect(result.hiddenCount).toBe(10);
    expect(calls('tenancy.getTeam')).toHaveLength(SCOPE_CAP);
  });

  it('lists the memberships only when ListRoleGrants fails', async () => {
    iam.setHandlers({
      ...tenancyTable({ [ORG_A]: 'Acme' }),
      'authz.listRoleGrants': () => {
        throw denial({ code: Code.Unavailable });
      },
    });
    const calls = callsSince(iam);

    const result = await loadMyScopes({ ...ports(), principal: principal(ORG_A), cedarCapability: true });

    expect(result.grantsListed).toBe(false);
    expect(result.entries.map((entry) => entry.prn)).toEqual([ORG_A]);
    // grantsListed is false on the no-call path too, so only the count proves that the call happened.
    expect(calls('authz.listRoleGrants')).toHaveLength(1);
  });

  it('pages ListRoleGrants until a page comes back short', async () => {
    const fullPage = Array.from({ length: 200 }, () => ROOT_PRN);
    iam.setHandlers({
      ...tenancyTable({ [ORG_A]: 'Acme' }),
      'authz.listRoleGrants': (req: { offset: bigint }) => ({
        grants: (req.offset === 0n ? fullPage : [ORG_A]).map((scopePrn, index) => ({ id: `g-${String(index)}`, principalPrn: IDS.principalPrn, roleKey: 'viewer', scopePrn })),
      }),
    });
    const calls = callsSince(iam);

    const result = await loadMyScopes({ ...ports(), principal: principal(), cedarCapability: true });

    expect(calls('authz.listRoleGrants').map((call) => (call.request as { offset: bigint }).offset)).toEqual([0n, 200n]);
    expect(result.entries.map((entry) => entry.prn)).toEqual([ORG_A]);
  });
});

describe('cedarCapabilityOf', () => {
  const CEDAR = ['iam.authz.cedar'];
  const descriptor = (capabilities: string[]) => ({ service: 'iam', version: '1', capabilities });

  it('lists the role grants only when IAM is available and reports iam.authz.cedar', () => {
    expect(cedarCapabilityOf({ state: 'available', service: 'iam', descriptor: descriptor(CEDAR), capabilities: CEDAR })).toBe(true);
  });

  const MEMBERSHIPS_ONLY: readonly (readonly [string, ServiceState])[] = [
    ['IAM is available without iam.authz.cedar', { state: 'available', service: 'iam', descriptor: descriptor(['iam.audit']), capabilities: ['iam.audit'] }],
    ['IAM is degraded, even with iam.authz.cedar in its last descriptor', { state: 'degraded', service: 'iam', reason: 'timeout', descriptor: descriptor(CEDAR), capabilities: CEDAR }],
    ['IAM is absent', { state: 'absent', service: 'iam' }],
  ];

  it.each(MEMBERSHIPS_ONLY)('selects "memberships only" when %s', (_label, state) => {
    expect(cedarCapabilityOf(state)).toBe(false);
  });
});
