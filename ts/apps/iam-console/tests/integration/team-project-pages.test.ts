// SPDX-License-Identifier: Apache-2.0
//
// The team and project page loaders and the create-project command (spec § 5.2, § 5.3), against the
// fake IAM. The consistency cases prove that a URL whose segments disagree with IAM's answer is a
// 404, and the non-UUID cases prove that IAM is not called for a malformed URL. The prn-mismatch
// cases script the answer real IAM gives for a wrong [org] (tenancy.rs:331-333, :480-482); the
// other-organization cases script an answer real IAM never gives, and hold the sameNode guard.
import { Code } from '@connectrpc/connect';
import { disposeTransports } from '@paigasus/sdk/iam';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { createProject, createProjectForm } from '../../app/(console)/orgs/[org]/teams/[team]/commands';
import { loadTeamPage } from '../../app/(console)/orgs/[org]/teams/[team]/load';
import { loadProjectPage } from '../../app/(console)/orgs/[org]/teams/[team]/projects/[project]/load';
import { PAGE_SIZE } from '../../lib/paging';
import { organizationPrn, projectPrn, teamPrn } from '../../lib/prn-tenancy';
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
const ORG_B = organizationPrn(IDS.orgB);
const TEAM_A1 = teamPrn(IDS.orgA, IDS.teamA1);
const PROJECT_A1 = projectPrn(IDS.orgA, IDS.projectA1);

function world(overrides: { teamOrgPrn?: string; projectTeamPrn?: string; projectOrgPrn?: string } = {}): FakeIamHandlers {
  return {
    'tenancy.getTeam': (req: { prn: string }) => ({ team: { prn: req.prn, orgPrn: overrides.teamOrgPrn ?? ORG_A, slug: 'platform', name: 'Platform' } }),
    'tenancy.getProject': (req: { prn: string }) => ({
      project: { prn: req.prn, teamPrn: overrides.projectTeamPrn ?? TEAM_A1, orgPrn: overrides.projectOrgPrn ?? ORG_A, slug: 'models', name: 'Models' },
    }),
    'tenancy.listProjects': () => ({ projects: [{ prn: PROJECT_A1, teamPrn: TEAM_A1, orgPrn: ORG_A, slug: 'models', name: 'Models' }] }),
    'tenancy.listMemberships': () => ({ memberships: [] }),
  };
}

const deps = (allowed: Parameters<typeof scriptedMayI>[0] = {}) => ({ tenancy: clientsFor(iam).tenancy, mayI: scriptedMayI(allowed) });

describe('loadTeamPage', () => {
  it('answers not-found for a non-UUID segment, and calls IAM not at all', async () => {
    iam.setHandlers(world());
    const start = iam.calls.length;
    expect(await loadTeamPage(deps(), { org: IDS.orgA, team: 'platform', offset: 0, membersOffset: 0 })).toEqual({ kind: 'not-found' });
    expect(await loadTeamPage(deps(), { org: 'acme', team: IDS.teamA1, offset: 0, membersOffset: 0 })).toEqual({ kind: 'not-found' });
    expect(iam.calls.length).toBe(start);
  });

  it('answers not-found when the team belongs to another organization than the URL says', async () => {
    iam.setHandlers(world({ teamOrgPrn: ORG_B }));
    const calls = callsSince(iam);

    expect(await loadTeamPage(deps(), { org: IDS.orgA, team: IDS.teamA1, offset: 0, membersOffset: 0 })).toEqual({ kind: 'not-found' });
    expect(calls('tenancy.listProjects')).toHaveLength(0);
  });

  it('answers not-found when IAM refuses the URL-built team PRN with prn-mismatch (a wrong [org])', async () => {
    iam.setHandlers({
      ...world(),
      'tenancy.getTeam': () => {
        throw denial({ code: Code.InvalidArgument, reason: 'prn-mismatch', correlationId: 'corr-team-mismatch' });
      },
    });
    const calls = callsSince(iam);

    expect(await loadTeamPage(deps({ CreateProject: true }), { org: IDS.orgB, team: IDS.teamA1, offset: 0, membersOffset: 0 })).toEqual({ kind: 'not-found' });
    expect(calls('tenancy.getTeam').map((call) => call.request)).toEqual([expect.objectContaining({ prn: teamPrn(IDS.orgB, IDS.teamA1) })]);
    expect(calls('tenancy.listProjects')).toHaveLength(0);
    expect(calls('tenancy.listMemberships')).toHaveLength(0);
  });

  it('returns the team, its projects, its members and the create-project affordance', async () => {
    iam.setHandlers(world());
    const calls = callsSince(iam);
    const d = deps({ CreateProject: true });

    const data = await loadTeamPage(d, { org: IDS.orgA, team: IDS.teamA1, offset: 0, membersOffset: 0 });

    if (data.kind !== 'ok') throw new Error(`expected ok, got ${data.kind}`);
    expect(data).toMatchObject({ orgId: IDS.orgA, teamId: IDS.teamA1, teamPrn: TEAM_A1, team: { name: 'Platform', slug: 'platform' }, canCreateProject: true });
    expect(data.projects).toEqual({ ok: true, value: { offset: 0, nextOffset: null, rows: [{ prn: PROJECT_A1, projectId: IDS.projectA1, slug: 'models', name: 'Models' }] } });
    expect(d.mayI.asked).toEqual(
      expect.arrayContaining([
        ['CreateProject', TEAM_A1],
        ['AttachMembership', TEAM_A1],
        ['DetachMembership', TEAM_A1],
      ]),
    );
    expect(calls('tenancy.getTeam')[0]?.request).toMatchObject({ prn: TEAM_A1 });
    expect(calls('tenancy.listProjects')[0]?.request).toMatchObject({ teamPrn: TEAM_A1, limit: PAGE_SIZE, offset: 0n });
  });

  it('turns a denied GetTeam into a page error', async () => {
    iam.setHandlers({
      ...world(),
      'tenancy.getTeam': () => {
        throw denial({ correlationId: 'corr-team' });
      },
    });

    const data = await loadTeamPage(deps(), { org: IDS.orgA, team: IDS.teamA1, offset: 0, membersOffset: 0 });

    if (data.kind !== 'error') throw new Error(`expected an error, got ${data.kind}`);
    expect(data.error.presentation).toBe('forbidden');
    expect(data.error.correlationId).toBe('corr-team');
  });
});

describe('loadProjectPage', () => {
  const params = { org: IDS.orgA, team: IDS.teamA1, project: IDS.projectA1, membersOffset: 0 };

  it('answers not-found for a non-UUID segment, and calls IAM not at all', async () => {
    iam.setHandlers(world());
    const start = iam.calls.length;
    expect(await loadProjectPage(deps(), { ...params, project: 'models' })).toEqual({ kind: 'not-found' });
    expect(iam.calls.length).toBe(start);
  });

  it('answers not-found when the project belongs to another team or another organization', async () => {
    iam.setHandlers(world({ projectTeamPrn: teamPrn(IDS.orgA, IDS.teamB1) }));
    expect(await loadProjectPage(deps(), params)).toEqual({ kind: 'not-found' });

    iam.setHandlers(world({ projectOrgPrn: ORG_B }));
    expect(await loadProjectPage(deps(), params)).toEqual({ kind: 'not-found' });
  });

  it("answers not-found for IAM's prn-mismatch on the URL-built project PRN (a wrong [org]), but a page error for a denial", async () => {
    iam.setHandlers({
      ...world(),
      'tenancy.getProject': () => {
        throw denial({ code: Code.InvalidArgument, reason: 'prn-mismatch', correlationId: 'corr-project-mismatch' });
      },
    });
    const calls = callsSince(iam);

    expect(await loadProjectPage(deps({ AttachMembership: true }), { ...params, org: IDS.orgB })).toEqual({ kind: 'not-found' });
    expect(calls('tenancy.getProject').map((call) => call.request)).toEqual([expect.objectContaining({ prn: projectPrn(IDS.orgB, IDS.projectA1) })]);
    expect(calls('tenancy.listMemberships')).toHaveLength(0);

    // Only invalid-input becomes a 404: a denied GetProject stays the 403 view.
    iam.setHandlers({
      ...world(),
      'tenancy.getProject': () => {
        throw denial({ correlationId: 'corr-project' });
      },
    });
    const denied = await loadProjectPage(deps(), params);
    if (denied.kind !== 'error') throw new Error(`expected an error, got ${denied.kind}`);
    expect(denied.error.presentation).toBe('forbidden');
    expect(denied.error.correlationId).toBe('corr-project');
  });

  it('returns the project and its members', async () => {
    iam.setHandlers(world());
    const calls = callsSince(iam);
    const d = deps({ AttachMembership: true });

    const data = await loadProjectPage(d, params);

    if (data.kind !== 'ok') throw new Error(`expected ok, got ${data.kind}`);
    expect(data).toMatchObject({ orgId: IDS.orgA, teamId: IDS.teamA1, projectId: IDS.projectA1, projectPrn: PROJECT_A1, project: { name: 'Models', slug: 'models' } });
    expect(data.members.canAttach).toBe(true);
    expect(calls('tenancy.getProject')[0]?.request).toMatchObject({ prn: PROJECT_A1 });
    expect(calls('tenancy.listMemberships')[0]?.request).toMatchObject({ filter: { case: 'nodePrn', value: PROJECT_A1 } });
  });
});

describe('createProject', () => {
  it('creates a project under the team from the hidden field', async () => {
    iam.setHandlers({
      'tenancy.createProject': (req: { teamPrn: string; slug: string; name: string }) => ({ project: { prn: PROJECT_A1, teamPrn: req.teamPrn, orgPrn: ORG_A, slug: req.slug, name: req.name } }),
    });
    const calls = callsSince(iam);

    expect(await createProject({ tenancy: clientsFor(iam).tenancy }, { teamPrn: TEAM_A1, slug: 'models', name: 'Models' })).toEqual({ ok: true });
    expect(calls('tenancy.createProject').map((call) => call.request)).toEqual([expect.objectContaining({ teamPrn: TEAM_A1, slug: 'models', name: 'Models' })]);
  });

  it("returns IAM's 403 with the correlation id", async () => {
    iam.setHandlers({
      'tenancy.createProject': () => {
        throw denial({ correlationId: 'corr-create-project' });
      },
    });

    const result = await createProject({ tenancy: clientsFor(iam).tenancy }, { teamPrn: TEAM_A1, slug: 'models', name: 'Models' });

    if (result.ok) throw new Error('expected a denial');
    expect(result.error.correlationId).toBe('corr-create-project');
  });

  // The same defect as minor 11's orgPrn, one directory down: teamPrn had no .trim() either.
  it('trims the parent PRN, and refuses one that is only whitespace', () => {
    expect(createProjectForm.safeParse({ teamPrn: ` ${TEAM_A1} `, slug: 'a', name: 'A' }).data?.teamPrn).toBe(TEAM_A1);
    expect(createProjectForm.safeParse({ teamPrn: '  ', slug: 'a', name: 'A' }).success).toBe(false);
  });
});
