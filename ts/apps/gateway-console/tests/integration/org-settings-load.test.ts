// SPDX-License-Identifier: Apache-2.0
//
// The organization settings loader (SMA-636 spec § 4.2). GetOrganization first: a denial is the
// PAGE error (403), and nothing else runs. Then the section and the Projects list, in parallel.
// A Projects failure is a SECTION error: the page stays 200.
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { Code, ConnectError } from '@connectrpc/connect';
import type { ServiceState } from '@paigasus/discovery/types';
import { disposeTransports } from '@paigasus/sdk/iam';
import { NodeStatus } from '@paigasus/sdk/iam/types';
import { organizationPrn, projectPrn, teamPrn } from '@paigasus/console-core';
import { denial, startFakeIam, type FakeIam, type FakeIamHandlers } from '@paigasus/console-core/testing';
import { loadOrganizationSettings, type OrganizationSettingsDeps } from '../../app/(console)/orgs/[org]/load';
import { IDS, callsSince, clientsFor, scriptedMayI } from './support';

const ORG = organizationPrn(IDS.orgA);
const NOW = Date.UTC(2026, 8, 18, 12, 0, 0);
const ACTIVE = { status: NodeStatus.ACTIVE, effectiveStatus: NodeStatus.ACTIVE };
const IAM: ServiceState = {
  state: 'available',
  service: 'iam',
  descriptor: { service: 'iam', version: '1.0.0', capabilities: ['iam.authz.cedar', 'iam.apikeys'] },
  capabilities: ['iam.authz.cedar', 'iam.apikeys'],
};

const uuid = (tag: string, index: number): string => `0190a1${tag}-0000-7000-8000-${String(index).padStart(12, '0')}`;

function teams(count: number) {
  return Array.from({ length: count }, (_, index) => ({ prn: teamPrn(IDS.orgA, uuid('b2', index)), orgPrn: ORG, slug: `t-${String(index)}`, name: `Team ${String(index)}`, ...ACTIVE }));
}

function projects(teamPrnValue: string, count: number) {
  return Array.from({ length: count }, (_, index) => ({
    prn: projectPrn(IDS.orgA, uuid('c3', index)),
    teamPrn: teamPrnValue,
    orgPrn: ORG,
    slug: `p-${String(index)}`,
    name: `Project ${String(index)}`,
    ...ACTIVE,
  }));
}

function world(opts: { teams?: number; projectsPerTeam?: number } = {}): FakeIamHandlers {
  return {
    'tenancy.getOrganization': (req) => ({ organization: { prn: req.prn, slug: 'acme', name: 'Acme', ...ACTIVE } }),
    'tenancy.listTeams': () => ({ teams: teams(opts.teams ?? 1) }),
    'tenancy.listProjects': (req) => ({ projects: projects(req.teamPrn, opts.projectsPerTeam ?? 1) }),
    'serviceAccounts.listServiceAccounts': () => ({ serviceAccounts: [] }),
  };
}

let iam: FakeIam;

beforeAll(async () => {
  iam = await startFakeIam();
});

afterAll(async () => {
  disposeTransports();
  await iam.close();
});

function deps(): OrganizationSettingsDeps {
  const clients = clientsFor(iam);
  return { tenancy: clients.tenancy, serviceAccounts: clients.serviceAccounts, authz: clients.authz, mayI: scriptedMayI({}), iam: IAM, now: () => NOW };
}

const params = (org: string = IDS.orgA) => ({ org, saOffset: 0, keyOffset: 0, sa: null });

describe('loadOrganizationSettings', () => {
  it('answers not-found for a segment that is not a UUID, and calls IAM not at all', async () => {
    iam.setHandlers(world());
    const start = iam.calls.length;
    expect(await loadOrganizationSettings(deps(), params('acme'))).toEqual({ kind: 'not-found' });
    expect(iam.calls.length).toBe(start);
  });

  it('returns the organization, its section and its projects grouped by team', async () => {
    iam.setHandlers(world({ teams: 2 }));
    const calls = callsSince(iam);

    const data = await loadOrganizationSettings(deps(), params(IDS.orgA.toUpperCase()));

    if (data.kind !== 'ok') throw new Error(`expected ok, got ${data.kind}`);
    expect(data.orgId).toBe(IDS.orgA);
    expect(data.orgPrn).toBe(ORG);
    expect(data.organization).toEqual({ name: 'Acme', slug: 'acme', lifecycle: { own: 'active', effective: 'active' } });
    expect(data.section).toMatchObject({ kind: 'ok', ownerPrn: ORG });
    expect(data.projects).toEqual({
      kind: 'ok',
      moreTeams: false,
      teams: [0, 1].map((index) => ({
        teamId: uuid('b2', index),
        name: `Team ${String(index)}`,
        moreProjects: false,
        projects: [{ projectId: uuid('c3', 0), name: 'Project 0', slug: 'p-0' }],
      })),
    });
    expect(calls('tenancy.listTeams')[0]?.request).toMatchObject({ orgPrn: ORG, limit: 51, offset: 0n });
    expect(calls('tenancy.listProjects').map((call) => call.request)).toEqual([
      expect.objectContaining({ teamPrn: teamPrn(IDS.orgA, uuid('b2', 0)), limit: 51 }),
      expect.objectContaining({ teamPrn: teamPrn(IDS.orgA, uuid('b2', 1)), limit: 51 }),
    ]);
  });

  it('is the page error for a denied organization, and runs nothing else', async () => {
    iam.setHandlers({
      ...world(),
      'tenancy.getOrganization': () => {
        throw denial();
      },
    });
    const calls = callsSince(iam);

    const data = await loadOrganizationSettings(deps(), params());

    expect(data.kind).toBe('error');
    if (data.kind !== 'error') throw new Error('expected error');
    expect(data.error.presentation).toBe('forbidden');
    expect(calls('tenancy.listTeams')).toHaveLength(0);
    expect(calls('serviceAccounts.listServiceAccounts')).toHaveLength(0);
  });

  it.each([
    [Code.InvalidArgument, 'prn-mismatch'],
    [Code.NotFound, 'not-found'],
  ])('answers not-found for code %s (%s)', async (code, reason) => {
    iam.setHandlers({
      ...world(),
      'tenancy.getOrganization': () => {
        throw denial({ code, reason });
      },
    });
    expect(await loadOrganizationSettings(deps(), params())).toEqual({ kind: 'not-found' });
  });

  it('shows 50 teams with no note for exactly 50, and 50 teams with the note for 51', async () => {
    iam.setHandlers(world({ teams: 50 }));
    const fifty = await loadOrganizationSettings(deps(), params());
    if (fifty.kind !== 'ok' || fifty.projects.kind !== 'ok') throw new Error('expected ok');
    expect(fifty.projects.teams).toHaveLength(50);
    expect(fifty.projects.moreTeams).toBe(false);

    iam.setHandlers(world({ teams: 51 }));
    const calls = callsSince(iam);
    const more = await loadOrganizationSettings(deps(), params());
    if (more.kind !== 'ok' || more.projects.kind !== 'ok') throw new Error('expected ok');
    expect(more.projects.teams).toHaveLength(50);
    expect(more.projects.moreTeams).toBe(true);
    // Only the SHOWN teams get a ListProjects call.
    expect(calls('tenancy.listProjects')).toHaveLength(50);
  });

  it('notes more projects for a team that has 51', async () => {
    iam.setHandlers(world({ projectsPerTeam: 51 }));
    const data = await loadOrganizationSettings(deps(), params());
    if (data.kind !== 'ok' || data.projects.kind !== 'ok') throw new Error('expected ok');
    expect(data.projects.teams[0]?.projects).toHaveLength(50);
    expect(data.projects.teams[0]?.moreProjects).toBe(true);
  });

  it('keeps the page when ListTeams fails: the Projects section carries the error', async () => {
    iam.setHandlers({
      ...world(),
      'tenancy.listTeams': () => {
        throw new ConnectError('down', Code.Unavailable);
      },
    });
    const data = await loadOrganizationSettings(deps(), params());
    if (data.kind !== 'ok') throw new Error('expected ok');
    expect(data.projects).toMatchObject({ kind: 'error', error: { presentation: 'degraded' } });
    expect(data.section.kind).toBe('ok');
  });

  it('fails the Projects section when one ListProjects fails', async () => {
    iam.setHandlers({
      ...world({ teams: 3 }),
      'tenancy.listProjects': (req) => {
        if (req.teamPrn === teamPrn(IDS.orgA, uuid('b2', 1))) throw denial();
        return { projects: [] };
      },
    });
    const data = await loadOrganizationSettings(deps(), params());
    if (data.kind !== 'ok') throw new Error('expected ok');
    expect(data.projects).toMatchObject({ kind: 'error', error: { presentation: 'forbidden' } });
  });

  it('never has more than 8 ListProjects calls in flight', async () => {
    let inFlight = 0;
    let peak = 0;
    iam.setHandlers({
      ...world({ teams: 20 }),
      'tenancy.listProjects': async () => {
        inFlight += 1;
        peak = Math.max(peak, inFlight);
        await new Promise((resolve) => setTimeout(resolve, 20));
        inFlight -= 1;
        return { projects: [] };
      },
    });
    await loadOrganizationSettings(deps(), params());
    expect(peak).toBe(8);
  });
});
