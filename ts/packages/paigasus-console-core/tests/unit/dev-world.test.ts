// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import { DEV_GATEWAY_DESCRIPTOR, DEV_IAM_DESCRIPTOR, devWorld } from '../../testing/index';

/**
 * Every method the two consoles reach. The fake answers `Unimplemented` for anything absent, so a
 * missing key is a broken page rather than a failing assertion somewhere else.
 */
const REQUIRED = [
  'authn.introspect',
  'authz.isAuthorized',
  'authz.listRoleGrants',
  'serviceInfo.getServiceInfo',
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
      expect(organization.status).toEqual(organization.effectiveStatus);
      expect(organization.status).not.toEqual(0); // 0 is UNSPECIFIED
    }
  });

  it('pins the two descriptors the consoles switch on', () => {
    // myScopes() lists role grants only when discovery reports iam.authz.cedar
    // (src/scopes.ts:116-118), and the audit page needs iam.audit.
    expect(DEV_IAM_DESCRIPTOR.capabilities).toEqual(['iam.authz.cedar', 'iam.audit']);
    expect(DEV_GATEWAY_DESCRIPTOR.capabilities).toEqual(['gateway.chat.stream']);
  });
});
