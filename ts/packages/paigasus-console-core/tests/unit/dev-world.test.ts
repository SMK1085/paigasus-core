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
  'authz.grantRole',
  'authz.revokeRole',
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
  'outbox.listDeadLetters',
  'outbox.replayDeadLetter',
  'outbox.discardDeadLetter',
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

  it('scripts authn.whoAmI with the same dev principal as authn.introspect, so the console sees its scopes', () => {
    const handlers = devWorld();
    expect(Object.keys(handlers)).toContain('authn.whoAmI');
    expect(handlers['authn.whoAmI']?.({} as never, {} as never)).toEqual(handlers['authn.introspect']?.({ token: 't' } as never, {} as never));
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
    // (src/scopes.ts:116-118), the audit page needs iam.audit, and the dead-letters page needs
    // iam.deadletters. The dev world is a CURRENT IAM, which always reports both iam.deadletters
    // (SMA-629) and iam.authn.grants (SMA-633).
    expect(DEV_IAM_DESCRIPTOR.capabilities).toEqual(['iam.authz.cedar', 'iam.audit', 'iam.deadletters', 'iam.authn.grants']);
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

  // SMA-629 spec § 5.3: three parked entries with RFC 4122 ids, newest first, as IAM orders them.
  const RFC_4122 = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
  type Listed = { entries: { id: string; eventType: string }[]; nextCursor: string };
  const list = (handlers: ReturnType<typeof devWorld>, eventType = ''): Listed => handlers['outbox.listDeadLetters']?.({ eventType } as never, {} as never) as Listed;

  it('lists three dead letters with RFC 4122 ids, in descending id order', () => {
    const listed = list(devWorld());
    expect(listed.entries).toHaveLength(3);
    for (const entry of listed.entries) expect(entry.id).toMatch(RFC_4122);
    const ids = listed.entries.map((entry) => entry.id);
    expect(ids).toEqual([...ids].sort().reverse());
    expect(listed.nextCursor).toBe('');
  });

  it('filters by the exact event type', () => {
    const handlers = devWorld();
    const [first] = list(handlers).entries;
    expect(first).toBeDefined();
    const filtered = list(handlers, first?.eventType);
    expect(filtered.entries.every((entry) => entry.eventType === first?.eventType)).toBe(true);
    expect(filtered.entries.length).toBeGreaterThan(0);
  });

  it.each(['outbox.replayDeadLetter', 'outbox.discardDeadLetter'] as const)('%s removes the entry, and a second call answers NotFound', (method) => {
    const handlers = devWorld();
    const id = list(handlers).entries[0]?.id;
    expect(id).toBeDefined();
    const answer = handlers[method]?.({ id } as never, {} as never) as { entry: { id: string } };
    expect(answer.entry.id).toBe(id);
    expect(list(handlers).entries.map((entry) => entry.id)).not.toContain(id);
    let caught: unknown;
    try {
      void handlers[method]?.({ id } as never, {} as never);
    } catch (error) {
      caught = error;
    }
    expect(caught).toBeInstanceOf(ConnectError);
    expect((caught as ConnectError).code).toBe(Code.NotFound);
  });

  type Grant = { id: string; principalPrn: string; roleKey: string; scopePrn: string };
  const ctx = { token: 'dev', correlationId: 'corr-dev' };

  it('grants idempotently, lists at a scope, and revokes (SMA-676 D9)', () => {
    const handlers = devWorld();
    const org = 'prn:pgs:iam:::organization/0190a100-0000-7000-8000-00000000d002';
    const person = 'prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-00000000d0aa';

    const first = (handlers['authz.grantRole']?.({ principalPrn: person, roleKey: 'gateway_user', scopePrn: org } as never, ctx) as { grant: Grant }).grant;
    const second = (handlers['authz.grantRole']?.({ principalPrn: person, roleKey: 'gateway_user', scopePrn: org } as never, ctx) as { grant: Grant }).grant;
    expect(second.id).toBe(first.id);
    expect((handlers['authz.listRoleGrants']?.({ principalPrn: '', scopePrn: org, roleKey: 'gateway_user', principalKind: 1 } as never, ctx as never) as { grants: Grant[] }).grants).toEqual([first]);
    expect((handlers['authz.listRoleGrants']?.({ principalPrn: '', scopePrn: org, roleKey: 'gateway_user', principalKind: 2 } as never, ctx as never) as { grants: Grant[] }).grants).toEqual([]);

    void handlers['authz.revokeRole']?.({ id: first.id } as never, ctx);
    expect((handlers['authz.listRoleGrants']?.({ principalPrn: '', scopePrn: org, roleKey: 'gateway_user', principalKind: 0 } as never, ctx as never) as { grants: Grant[] }).grants).toEqual([]);
    expect(() => handlers['authz.revokeRole']?.({ id: first.id } as never, ctx as never)).toThrow(ConnectError);
  });

  it('answers no member for a service-account kind filter (SMA-676 D8)', () => {
    const handlers = devWorld();
    const org = 'prn:pgs:iam:::organization/0190a100-0000-7000-8000-00000000d002';
    expect((handlers['tenancy.listMemberships']?.({ filter: { case: 'nodePrn', value: org }, principalKind: 1 } as never, ctx as never) as { memberships: unknown[] }).memberships).toHaveLength(1);
    expect((handlers['tenancy.listMemberships']?.({ filter: { case: 'nodePrn', value: org }, principalKind: 2 } as never, ctx as never) as { memberships: unknown[] }).memberships).toHaveLength(0);
  });
});
