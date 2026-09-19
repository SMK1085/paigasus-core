// SPDX-License-Identifier: Apache-2.0
//
// The project settings loader (SMA-636 spec § 4.1, § 4.2). GetProject first. Then GetTeam and
// GetOrganization for the header and the breadcrumbs, and the section, in parallel. A failed
// GetTeam or GetOrganization keeps the page: the header shows the UUID instead of the name.
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { Code } from '@connectrpc/connect';
import type { ServiceState } from '@paigasus/discovery/types';
import { disposeTransports } from '@paigasus/sdk/iam';
import { NodeStatus } from '@paigasus/sdk/iam/types';
import { organizationPrn, projectPrn, teamPrn } from '@paigasus/console-core';
import { denial, startFakeIam, type FakeIam, type FakeIamHandlers } from '@paigasus/console-core/testing';
import { loadProjectSettings, type ProjectSettingsDeps } from '../../app/(console)/orgs/[org]/projects/[project]/load';
import { IDS, callsSince, clientsFor, scriptedMayI } from './support';

const ORG = organizationPrn(IDS.orgA);
const TEAM = teamPrn(IDS.orgA, IDS.teamA1);
const PROJECT = projectPrn(IDS.orgA, IDS.projectA1);
const NOW = Date.UTC(2026, 8, 18, 12, 0, 0);
const ACTIVE = { status: NodeStatus.ACTIVE, effectiveStatus: NodeStatus.ACTIVE };
const IAM: ServiceState = { state: 'available', service: 'iam', descriptor: { service: 'iam', version: '1.0.0', capabilities: ['iam.authz.cedar'] }, capabilities: ['iam.authz.cedar'] };

function world(): FakeIamHandlers {
  return {
    'tenancy.getProject': (req) => ({ project: { prn: req.prn, teamPrn: TEAM, orgPrn: ORG, slug: 'gw', name: 'Gateway', ...ACTIVE } }),
    'tenancy.getTeam': () => ({ team: { prn: TEAM, orgPrn: ORG, slug: 'platform', name: 'Platform', ...ACTIVE } }),
    'tenancy.getOrganization': () => ({ organization: { prn: ORG, slug: 'acme', name: 'Acme', ...ACTIVE } }),
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

function deps(): ProjectSettingsDeps {
  const clients = clientsFor(iam);
  return { tenancy: clients.tenancy, serviceAccounts: clients.serviceAccounts, authz: clients.authz, mayI: scriptedMayI({}), iam: IAM, now: () => NOW };
}

const params = (project: string = IDS.projectA1, org: string = IDS.orgA) => ({ org, project, saOffset: 0, keyOffset: 0, sa: null });

describe('loadProjectSettings', () => {
  it('answers not-found for a segment that is not a UUID, and calls IAM not at all', async () => {
    iam.setHandlers(world());
    const start = iam.calls.length;
    expect(await loadProjectSettings(deps(), params('gateway'))).toEqual({ kind: 'not-found' });
    expect(await loadProjectSettings(deps(), params(IDS.projectA1, 'acme'))).toEqual({ kind: 'not-found' });
    expect(iam.calls.length).toBe(start);
  });

  it('returns the project, its team and organization names, and a section owned by the project', async () => {
    iam.setHandlers(world());
    const calls = callsSince(iam);

    const data = await loadProjectSettings(deps(), params());

    if (data.kind !== 'ok') throw new Error(`expected ok, got ${data.kind}`);
    expect(data).toMatchObject({
      orgId: IDS.orgA,
      projectId: IDS.projectA1,
      projectPrn: PROJECT,
      project: { name: 'Gateway', slug: 'gw', lifecycle: { own: 'active', effective: 'active' } },
      team: { id: IDS.teamA1, name: 'Platform' },
      organizationName: 'Acme',
      section: { kind: 'ok', ownerPrn: PROJECT },
    });
    // The PRN is built from the URL's organization and project (§ 4.1).
    expect(calls('tenancy.getProject')[0]?.request).toMatchObject({ prn: PROJECT });
    expect(calls('tenancy.getTeam')[0]?.request).toMatchObject({ prn: TEAM });
    expect(calls('tenancy.getOrganization')[0]?.request).toMatchObject({ prn: ORG });
    expect(calls('serviceAccounts.listServiceAccounts')[0]?.request).toMatchObject({ ownerPrn: PROJECT });
  });

  it('keeps the page when GetTeam and GetOrganization fail: the names are null, the team id stays', async () => {
    iam.setHandlers({
      ...world(),
      'tenancy.getTeam': () => {
        throw denial();
      },
      'tenancy.getOrganization': () => {
        throw denial();
      },
    });
    const data = await loadProjectSettings(deps(), params());
    if (data.kind !== 'ok') throw new Error('expected ok');
    expect(data.team).toEqual({ id: IDS.teamA1, name: null });
    expect(data.organizationName).toBeNull();
  });

  it('is the page error for a denied project, and not-found for a mixed or missing one', async () => {
    iam.setHandlers({
      ...world(),
      'tenancy.getProject': () => {
        throw denial();
      },
    });
    expect((await loadProjectSettings(deps(), params())).kind).toBe('error');

    iam.setHandlers({
      ...world(),
      'tenancy.getProject': () => {
        throw denial({ code: Code.InvalidArgument, reason: 'prn-mismatch' });
      },
    });
    expect(await loadProjectSettings(deps(), params())).toEqual({ kind: 'not-found' });
  });
});
