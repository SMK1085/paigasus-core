// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import { Code, ConnectError } from '@connectrpc/connect';
import { NodeStatus } from '@paigasus/sdk/iam/types';
import { DEV_GATEWAY_DESCRIPTOR, DEV_IAM_DESCRIPTOR, devWorld } from '../../testing/index';

/**
 * Every method the two consoles reach. The fake answers `Unimplemented` for anything absent, so a
 * missing key is a broken page rather than a failing assertion somewhere else.
 */
const REQUIRED = [
  'authn.introspect',
  'authz.isAuthorized',
  'authz.listRoleGrants',
  'tenancy.getOrganization',
  'tenancy.getTeam',
  'tenancy.getProject',
  'tenancy.listOrganizations',
  'tenancy.listTeams',
  'tenancy.listProjects',
  'tenancy.listMemberships',
  'tenancy.createOrganization',
  'tenancy.createTeam',
  'tenancy.createProject',
  'tenancy.renameOrganization',
  'tenancy.renameTeam',
  'tenancy.renameProject',
  'tenancy.archiveOrganization',
  'tenancy.archiveTeam',
  'tenancy.archiveProject',
  'tenancy.restoreOrganization',
  'tenancy.restoreTeam',
  'tenancy.restoreProject',
  'tenancy.attachMembership',
  'tenancy.detachMembership',
  'audit.listAuditEntries',
] as const;

describe('devWorld', () => {
  it('scripts every method the consoles reach', () => {
    const handlers = devWorld();
    for (const method of REQUIRED) expect(Object.keys(handlers)).toContain(method);
  });

  it('leaves serviceInfo.getServiceInfo unscripted, so the gRPC answer stays tied to setServiceInfo()', () => {
    // fake-iam.ts's dispatch() always prefers a scripted handler over defaults(), so scripting
    // this method here would freeze the gRPC answer at whatever this map returned — even after a
    // later setServiceInfo() call moved the HTTP answer, breaking the fake's documented contract
    // that a descriptor change updates both answers (fake-iam.ts:130-134). Leaving the key absent
    // lets defaults() keep serving the mutable descriptor. Do not re-add this key.
    const handlers = devWorld();
    expect(Object.keys(handlers)).not.toContain('serviceInfo.getServiceInfo');
  });

  it('allows every action, so a dev session is never denied', () => {
    const handlers = devWorld();
    const isAuthorized = handlers['authz.isAuthorized'];
    expect(isAuthorized).toBeDefined();
    expect(isAuthorized?.({ action: 'ArchiveProject' } as never, {} as never)).toMatchObject({ allowed: true });
  });

  it('marks every node ACTIVE — without a status every row reads "Status unknown"', () => {
    const handlers = devWorld();
    const listed = handlers['tenancy.listOrganizations']?.({} as never, {} as never) as { organizations: { status: number; effectiveStatus: number }[] };
    expect(listed.organizations.length).toBeGreaterThan(0);
    for (const organization of listed.organizations) {
      expect(organization.status).toEqual(NodeStatus.ACTIVE);
      expect(organization.effectiveStatus).toEqual(NodeStatus.ACTIVE);
    }
    const listedTeams = handlers['tenancy.listTeams']?.({} as never, {} as never) as { teams: { prn: string }[] };
    const team = handlers['tenancy.getTeam']?.({ prn: listedTeams.teams[0]?.prn } as never, {} as never) as { team: { status: number; effectiveStatus: number } };
    expect(team.team.status).toEqual(NodeStatus.ACTIVE);
    expect(team.team.effectiveStatus).toEqual(NodeStatus.ACTIVE);
    const listedProjects = handlers['tenancy.listProjects']?.({} as never, {} as never) as { projects: { prn: string }[] };
    const project = handlers['tenancy.getProject']?.({ prn: listedProjects.projects[0]?.prn } as never, {} as never) as { project: { status: number; effectiveStatus: number } };
    expect(project.project.status).toEqual(NodeStatus.ACTIVE);
    expect(project.project.effectiveStatus).toEqual(NodeStatus.ACTIVE);
  });

  it('pins the two descriptors the consoles switch on', () => {
    // myScopes() lists role grants only when discovery reports iam.authz.cedar
    // (src/scopes.ts:116-118), and the audit page needs iam.audit.
    expect(DEV_IAM_DESCRIPTOR.capabilities).toEqual(['iam.authz.cedar', 'iam.audit']);
    expect(DEV_GATEWAY_DESCRIPTOR.capabilities).toEqual(['gateway.chat.stream']);
  });

  it('returns the submitted name from get, not the fixture, for an organization the developer creates', () => {
    const handlers = devWorld();
    const created = handlers['tenancy.createOrganization']?.({ slug: 'acme', name: 'Acme' } as never, {} as never) as { organization: { prn: string; name: string } };
    expect(created.organization.name).toBe('Acme');
    const fetched = handlers['tenancy.getOrganization']?.({ prn: created.organization.prn } as never, {} as never) as { organization: { name: string } };
    expect(fetched.organization.name).toBe('Acme');
  });

  it('renames a node and returns the new name', () => {
    const handlers = devWorld();
    const listed = handlers['tenancy.listOrganizations']?.({} as never, {} as never) as { organizations: { prn: string }[] };
    const prn = listed.organizations[0]?.prn;
    expect(prn).toBeDefined();
    const renamed = handlers['tenancy.renameOrganization']?.({ prn, newName: 'Renamed Org' } as never, {} as never) as { organization: { name: string } };
    expect(renamed.organization.name).toBe('Renamed Org');
    const fetched = handlers['tenancy.getOrganization']?.({ prn } as never, {} as never) as { organization: { name: string } };
    expect(fetched.organization.name).toBe('Renamed Org');
  });

  it('archives a node and changes its status to archived', () => {
    const handlers = devWorld();
    const listed = handlers['tenancy.listTeams']?.({} as never, {} as never) as { teams: { prn: string }[] };
    const prn = listed.teams[0]?.prn;
    expect(prn).toBeDefined();
    const result = handlers['tenancy.archiveTeam']?.({ prn } as never, {} as never) as { team: { status: number; effectiveStatus: number } };
    expect(result.team.status).toEqual(NodeStatus.ARCHIVED);
    expect(result.team.effectiveStatus).toEqual(NodeStatus.ARCHIVED);
  });

  it('refuses to get an unknown PRN', () => {
    const handlers = devWorld();
    let caught: unknown;
    try {
      void handlers['tenancy.getProject']?.({ prn: 'prn:pgs:iam:::project/00000000-0000-0000-0000-000000000000' } as never, {} as never);
    } catch (error) {
      caught = error;
    }
    expect(caught).toBeInstanceOf(ConnectError);
    expect((caught as ConnectError).code).toBe(Code.NotFound);
  });
});
